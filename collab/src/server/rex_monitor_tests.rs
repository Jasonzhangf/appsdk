use super::*;
use crate::client::adapters::rex::{
    RexEvent, RexEventBatch, RexProcessObservation, RexProgramStatusOwner, RexProgramStatusRecord,
};
use crate::proto::{RexEndpoint, SelectedTransport, TransportKind};
use crate::scope::HostPaths;
use crate::server::name_state::{AgentActivity, AgentPresence};
use crate::server::state::{State, WorkerRec};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn endpoint(block: &str) -> RexEndpoint {
    RexEndpoint {
        server: "unix:///tmp/rex-monitor.sock".into(),
        session_id: "session:11111111-1111-1111-1111-111111111111".into(),
        block_id: block.into(),
        host_id: Some("host".into()),
        instance_id: "instance".into(),
        cwd: "/tmp".into(),
    }
}

fn target() -> MonitorTarget {
    MonitorTarget {
        endpoint: endpoint("block:22222222-2222-2222-2222-222222222222"),
        agent_session_id: "agent-session".into(),
    }
}

fn record(state: &str, kind: Option<&str>) -> RexProgramStatusRecord {
    owned_record(
        state,
        kind,
        Some(RexProgramStatusOwner {
            pid: 4242,
            name: "python3".into(),
        }),
    )
}

fn owned_record(
    state: &str,
    kind: Option<&str>,
    owner: Option<RexProgramStatusOwner>,
) -> RexProgramStatusRecord {
    RexProgramStatusRecord {
        id: Some("agent".into()),
        state: Some(state.into()),
        kind: kind.map(str::to_owned),
        app: Some("codex".into()),
        msg: None,
        updated_at: None,
        owner,
    }
}

fn observation(
    records: Vec<RexProgramStatusRecord>,
) -> crate::client::adapters::rex::RexObservation {
    crate::client::adapters::rex::RexObservation {
        endpoint: endpoint("block:22222222-2222-2222-2222-222222222222"),
        process: Some(RexProcessObservation {
            pid: Some(4242),
            name: Some("python3".into()),
            path: None,
            cwd: None,
        }),
        pwd: None,
        program_status: records,
        sources: vec![
            "com.superlogical.terminal.process".into(),
            "com.superlogical.terminal.program_status".into(),
        ],
        source_errors: Vec::new(),
        gap_possible: false,
    }
}

fn rex_server(target: &MonitorTarget) -> (Arc<Server>, PathBuf) {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "collab-rex-monitor-{}-{sequence}",
        std::process::id()
    ));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let state_path = server_dir.join("state.json");
    crate::server::current_state::save(&state_path, &State::default()).unwrap();
    let server = Arc::new(Server {
        config: Default::default(),
        root: root.clone(),
        storage_root: root.clone(),
        state_path,
        host_paths: HostPaths::for_state_root(root.join("host-state")).unwrap(),
        state: Mutex::new(State::default()),
        appserver_candidate_check: Arc::new(|_| unreachable!()),
        appserver_notification_sink: Arc::new(|_, _, _, _, _, _| unreachable!()),
        appserver_thread_status: Arc::new(|_, _| unreachable!()),
        appserver_thread_archive: Arc::new(|_, _| unreachable!()),
        register_gate: Arc::new(Mutex::new(())),
        rex_monitor: Default::default(),
        mailbox_notify: tokio::sync::Notify::new(),
    });
    server.state.lock().unwrap().workers.insert(
        "worker".into(),
        WorkerRec {
            id: "worker".into(),
            token: "test".into(),
            cwd: root.to_string_lossy().into(),
            registered_ms: 0,
            transport: Some(SelectedTransport {
                kind: TransportKind::Rex,
                endpoint: Some(target.endpoint.server.clone()),
                namespace: None,
                session_id: Some(target.agent_session_id.clone()),
                thread_id: None,
                tmux_endpoint: None,
                rex_endpoint: Some(target.endpoint.clone()),
                capabilities: vec![],
                self_check: "test".into(),
            }),
        },
    );
    (server, root)
}

fn event(name: &str, block: &str, creator: &str) -> RexEvent {
    RexEvent {
        source: "rex.events:session".into(),
        raw: json!({}),
        session_id: "session:11111111-1111-1111-1111-111111111111".into(),
        block_id: block.into(),
        creator_name: creator.into(),
        name: name.into(),
        payload: json!({}),
    }
}

#[test]
fn observed_execution_survives_daemon_state_reload_for_precise_exit_probe() {
    let target = target();
    let (server, root) = rex_server(&target);
    let owner = RexProgramStatusOwner {
        pid: 4242,
        name: "codex".into(),
    };
    submit_activity_with_execution(
        &server,
        "worker",
        &target,
        RexProjection {
            presence: AgentPresence::Online,
            activity: AgentActivity::Idle,
            reason: "native current owner".into(),
        },
        Some(&owner),
    );
    let encoded = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    let restored: State = serde_json::from_value(encoded.clone()).unwrap();
    let record = restored.rex_executions.get("worker").unwrap();
    assert_eq!(record.endpoint, target.endpoint);
    assert_eq!(record.agent_session_id, target.agent_session_id);
    let mut execution = Some(record.owner.clone());
    let mut shell = observation(Vec::new());
    shell.process.as_mut().unwrap().pid = Some(100);
    shell.process.as_mut().unwrap().name = Some("zsh".into());
    assert_eq!(
        project_observation_with_execution(&shell, &mut execution, &|_| Ok(false)).presence,
        AgentPresence::Offline
    );
    let mut legacy = encoded;
    legacy.as_object_mut().unwrap().remove("rex_executions");
    assert!(serde_json::from_value::<State>(legacy)
        .unwrap()
        .rex_executions
        .is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_codex_exit_requires_the_observed_process_to_be_absent() {
    let mut execution = None;
    let online = observation(vec![record("idle", None)]);
    assert_eq!(
        project_observation_with_execution(&online, &mut execution, &|_| panic!(
            "sufficient native facts must stop probing"
        ))
        .presence,
        AgentPresence::Online
    );
    let mut shell = observation(Vec::new());
    shell.process.as_mut().unwrap().pid = Some(100);
    shell.process.as_mut().unwrap().name = Some("zsh".into());
    assert_eq!(
        project_observation_with_execution(&shell, &mut execution, &|pid| {
            assert_eq!(pid, 4242);
            Ok(false)
        })
        .presence,
        AgentPresence::Offline
    );
    assert_eq!(
        project_observation_with_execution(&shell, &mut execution, &|_| Ok(true)).presence,
        AgentPresence::Unknown
    );
    assert_eq!(
        project_observation_with_execution(&shell, &mut execution, &|_| Err("probe denied".into()))
            .presence,
        AgentPresence::Unknown
    );
    assert_eq!(
        project_observation_with_execution(&shell, &mut None, &|_| panic!("no known execution"))
            .presence,
        AgentPresence::Unknown
    );
    shell.process.as_mut().unwrap().name = Some("codex".into());
    assert_eq!(
        project_observation_with_execution(&shell, &mut execution, &|_| panic!(
            "a new foreground Codex remains unresolved"
        ))
        .presence,
        AgentPresence::Unknown
    );
    let mut returned = observation(vec![record("working", None)]);
    returned.process.as_mut().unwrap().pid = Some(999);
    returned.program_status[0].owner.as_mut().unwrap().pid = 999;
    assert_eq!(
        project_observation_with_execution(&returned, &mut execution, &|_| panic!(
            "current native owner wins over an old exit"
        ))
        .presence,
        AgentPresence::Online
    );
    assert_eq!(execution.unwrap().pid, 999);
}

#[test]
fn status_mapping_covers_working_blocked_idle_and_unknown() {
    assert_eq!(
        project_observation(&observation(vec![record("working", None)])).activity,
        AgentActivity::Busy
    );
    assert_eq!(
        project_observation(&observation(vec![record("blocked", Some("question"))])).activity,
        AgentActivity::WaitingUser
    );
    assert_eq!(
        project_observation(&observation(vec![record("blocked", Some("approval"))])).activity,
        AgentActivity::WaitingUser
    );
    assert_eq!(
        project_observation(&observation(Vec::new())).activity,
        AgentActivity::Unknown
    );
    assert_eq!(
        project_observation(&observation(vec![
            record("working", None),
            record("blocked", Some("question")),
        ]))
        .activity,
        AgentActivity::Unknown,
        "conflicting current records have no justified precedence"
    );
    for state in ["idle", "done"] {
        assert_eq!(
            project_observation(&observation(vec![record(state, None)])).activity,
            AgentActivity::Idle,
            "state {state} must project idle"
        );
    }
    assert_eq!(
        project_observation(&observation(vec![record("error", None)])).activity,
        AgentActivity::Idle,
        "a current error state must not be offered as busy"
    );
    assert_eq!(
        project_observation(&observation(vec![record("clear", None)])).activity,
        AgentActivity::Idle,
        "an unrecognized current state must project idle"
    );
    let mut failed = observation(vec![record("working", None)]);
    failed.sources = vec!["com.superlogical.terminal.program_status".into()];
    failed.source_errors = vec!["com.superlogical.terminal.process: unavailable".into()];
    failed.gap_possible = true;
    let projection = project_observation(&failed);
    assert_eq!(projection.activity, AgentActivity::Unknown);
    assert!(projection.reason.contains("rex.osc7501"));
}

#[test]
fn r1_current_codex_error_projects_idle() {
    let projection = project_observation(&observation(vec![record("error", None)]));

    assert!(
        projection.reason.contains("projection=idle"),
        "expected a current error state to project idle, got {}",
        projection.reason
    );
    assert_eq!(projection.activity, AgentActivity::Idle);
    assert_eq!(projection.presence, AgentPresence::Online);
}

#[test]
fn r1_unrecognized_current_codex_state_projects_idle() {
    let projection = project_observation(&observation(vec![record("future_state", None)]));

    assert!(
        projection.reason.contains("projection=idle"),
        "an unrecognized Rex state must not become busy, got {}",
        projection.reason
    );
    assert_eq!(projection.activity, AgentActivity::Idle);
    assert_eq!(projection.presence, AgentPresence::Online);
}

#[test]
fn r1_non_codex_current_owner_does_not_prove_codex_busy() {
    let mut shell_record = record("working", None);
    shell_record.app = Some("terminal-shell".into());

    let projection = project_observation(&observation(vec![shell_record]));

    assert_eq!(
        projection.activity,
        AgentActivity::Unknown,
        "a working record for another app is not evidence that Codex is busy"
    );
    assert_eq!(projection.presence, AgentPresence::Unknown);
}

#[test]
fn r1_unrelated_pwd_failure_does_not_erase_complete_codex_activity() {
    let mut current = observation(vec![record("working", None)]);
    current.sources = vec![
        "com.superlogical.terminal.process".into(),
        "com.superlogical.terminal.program_status".into(),
    ];
    current.source_errors = vec!["com.superlogical.terminal.pwd: unavailable".into()];
    current.gap_possible = true;

    let projection = project_observation(&current);

    assert_eq!(
        projection.activity,
        AgentActivity::Busy,
        "pwd is not used to match the foreground Codex owner or its program-status record"
    );
    assert_eq!(projection.presence, AgentPresence::Online);
}

#[test]
fn r1_missing_required_process_source_keeps_activity_unknown() {
    let mut current = observation(vec![record("working", None)]);
    current.sources = vec!["com.superlogical.terminal.program_status".into()];
    current.source_errors = vec!["com.superlogical.terminal.process: unavailable".into()];
    current.gap_possible = true;

    let projection = project_observation(&current);

    assert_eq!(projection.activity, AgentActivity::Unknown);
    assert_eq!(projection.presence, AgentPresence::Unknown);
}

#[test]
fn r1_current_codex_working_submission_is_online_and_busy() {
    let (server, root) = rex_server(&target());
    let projection = project_observation(&observation(vec![record("working", None)]));

    submit_activity(&server, "worker", &target(), projection);

    let scope = server
        .root
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let state = server.state.lock().unwrap();
    assert_eq!(
        state
            .agent_presence(&scope, "worker")
            .map(|record| record.presence),
        Some(AgentPresence::Online)
    );
    assert_eq!(
        state
            .agent_activity(&scope, "worker")
            .map(|record| record.activity),
        Some(AgentActivity::Busy)
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn r1_unrecognized_current_codex_state_submits_online_idle() {
    let (server, root) = rex_server(&target());
    let projection = project_observation(&observation(vec![record("future_state", None)]));

    submit_activity(&server, "worker", &target(), projection);

    let scope = server
        .root
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let state = server.state.lock().unwrap();
    assert_eq!(
        state
            .agent_presence(&scope, "worker")
            .map(|record| record.presence),
        Some(AgentPresence::Online)
    );
    assert_eq!(
        state
            .agent_activity(&scope, "worker")
            .map(|record| record.activity),
        Some(AgentActivity::Idle)
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn r1_replaced_target_rejects_late_activity_submission() {
    let current = target();
    let (server, root) = rex_server(&current);
    let mut stale = current.clone();
    stale.endpoint.block_id = "block:33333333-3333-3333-3333-333333333333".into();

    submit_activity(
        &server,
        "worker",
        &stale,
        RexProjection {
            presence: AgentPresence::Online,
            activity: AgentActivity::Busy,
            reason: "stale".into(),
        },
    );

    let scope = server
        .root
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let state = server.state.lock().unwrap();
    assert!(state.agent_presence(&scope, "worker").is_none());
    assert!(state.agent_activity(&scope, "worker").is_none());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_or_partial_owner_records_do_not_project_busy() {
    // Same pid, different name: a reused PID must not revive an old working
    // record once the foreground process changed.
    let stale_name = owned_record(
        "working",
        None,
        Some(RexProgramStatusOwner {
            pid: 4242,
            name: "other".into(),
        }),
    );
    assert_eq!(
        project_observation(&observation(vec![stale_name])).activity,
        AgentActivity::Unknown
    );
    // A different pid is also stale.
    let stale_pid = owned_record(
        "working",
        None,
        Some(RexProgramStatusOwner {
            pid: 9999,
            name: "python3".into(),
        }),
    );
    assert_eq!(
        project_observation(&observation(vec![stale_pid])).activity,
        AgentActivity::Unknown
    );
    // Unknown owner cannot be matched, so it stays out of the projection.
    let no_owner = owned_record("working", None, None);
    assert_eq!(
        project_observation(&observation(vec![no_owner])).activity,
        AgentActivity::Unknown
    );
    // An idle record that matches the foreground owner must win over a stale
    // working record from a previous process.
    let current_idle = record("idle", None);
    let stale_working = owned_record(
        "working",
        None,
        Some(RexProgramStatusOwner {
            pid: 9999,
            name: "old".into(),
        }),
    );
    assert_eq!(
        project_observation(&observation(vec![stale_working, current_idle])).activity,
        AgentActivity::Idle
    );
}

#[test]
fn incomplete_foreground_identity_never_projects_busy_or_waiting() {
    for foreground in [
        (Some(0), Some("python3")),
        (Some(4242), Some("")),
        (Some(4242), Some("   ")),
        (None, Some("python3")),
        (Some(4242), None),
    ] {
        let mut observation = observation(vec![record("working", None)]);
        observation.process = Some(RexProcessObservation {
            pid: foreground.0,
            name: foreground.1.map(str::to_owned),
            path: None,
            cwd: None,
        });
        assert_eq!(
            project_observation(&observation).activity,
            AgentActivity::Unknown,
            "incomplete foreground {foreground:?} must stay unknown"
        );
    }
    let mut blocked = observation(vec![record("blocked", Some("question"))]);
    blocked.process = Some(RexProcessObservation {
        pid: Some(0),
        name: Some("python3".into()),
        path: None,
        cwd: None,
    });
    assert_eq!(
        project_observation(&blocked).activity,
        AgentActivity::Unknown,
        "blocked record with incomplete foreground must stay unknown"
    );
}

#[test]
fn projection_keeps_source_and_emits_idle_or_busy() {
    let projection = project_observation(&observation(vec![record("working", None)]));
    assert_eq!(projection.activity, AgentActivity::Busy);
    assert_eq!(projection.presence, AgentPresence::Online);
    assert!(projection.reason.contains("rex.osc7501"));
    assert!(projection
        .reason
        .contains("com.superlogical.terminal.program_status"));

    let projection = project_observation(&observation(vec![record("idle", None)]));
    assert_eq!(projection.activity, AgentActivity::Idle);
    assert!(projection.reason.contains("projection=idle"));
    assert!(projection
        .reason
        .contains("com.superlogical.terminal.program_status"));
}

#[test]
fn events_require_exact_current_binding_and_relevant_name() {
    let target = target();
    assert!(event_matches_target(
        &event(
            "program_status_changed",
            &target.endpoint.block_id,
            TERMINAL_CREATOR
        ),
        &target
    ));
    assert!(!event_matches_target(
        &event("program_status_changed", "block:wrong", TERMINAL_CREATOR),
        &target
    ));
    assert!(!event_matches_target(
        &event("program_status_changed", &target.endpoint.block_id, "other"),
        &target
    ));
    assert!(!event_matches_target(
        &event("unrelated", &target.endpoint.block_id, TERMINAL_CREATOR),
        &target
    ));
}

#[test]
fn late_old_binding_is_not_equal() {
    let expected = target();
    let mut changed_session = expected.clone();
    changed_session.agent_session_id = "new-agent-session".into();
    let mut changed_endpoint = expected.clone();
    changed_endpoint.endpoint.block_id = "block:33333333-3333-3333-3333-333333333333".into();
    assert!(same_binding(&expected, &expected.clone()));
    assert!(!same_binding(&expected, &changed_session));
    assert!(!same_binding(&expected, &changed_endpoint));
}

#[test]
fn stream_disconnect_projects_unknown_and_cancel_does_not() {
    let disconnected = RexEventBatch {
        events: Vec::new(),
        gap_possible: true,
        detail: Some("events stream disconnected".into()),
    };
    let projection =
        project_stream_end(&disconnected).expect("disconnect must project an observation");
    assert_eq!(projection.activity, AgentActivity::Unknown);
    assert_eq!(projection.presence, AgentPresence::Unknown);
    assert!(projection.reason.contains("events stream disconnected"));

    let cancelled = RexEventBatch {
        events: Vec::new(),
        gap_possible: false,
        detail: None,
    };
    assert!(project_stream_end(&cancelled).is_none());
}

#[tokio::test]
async fn reconcile_replaces_old_binding_and_reaps_removed_binding() {
    let (cancel_a, _) = tokio::sync::watch::channel(false);
    let (cancel_b, _) = tokio::sync::watch::channel(false);
    let task_a = RexTask {
        target: target(),
        cancel: cancel_a,
        task: tokio::spawn(async {}),
    };
    let mut other = target();
    other.agent_session_id = "other-session".into();
    let task_b = RexTask {
        target: other,
        cancel: cancel_b,
        task: tokio::spawn(async {}),
    };
    let mut table = std::collections::HashMap::new();
    table.insert("a".into(), task_a);
    table.insert("b".into(), task_b);

    let mut changed = target();
    changed.agent_session_id = "new-agent-session".into();
    let desired = std::collections::HashMap::from([("a".into(), changed)]);
    let mut spawned = 0usize;
    let retired = reconcile_table(&mut table, desired, |_, _| {
        spawned += 1;
        let (cancel, _) = tokio::sync::watch::channel(false);
        RexTask {
            target: target(),
            cancel,
            task: tokio::spawn(async {}),
        }
    });
    assert_eq!(retired.len(), 2, "changed a and removed b must be retired");
    assert_eq!(spawned, 1, "one current task remains");
    assert_eq!(table.len(), 1);
}

#[tokio::test]
async fn cancel_sender_stops_owned_task() {
    let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        let _ = cancel_rx.changed().await;
    });
    cancel_tx.send(true).expect("cancel receiver alive");
    tokio::time::timeout(std::time::Duration::from_secs(1), task)
        .await
        .expect("task reaped after cancel")
        .expect("task joined");
}

#[tokio::test]
async fn reconcile_reconnects_a_finished_current_task() {
    let (cancel_a, _) = tokio::sync::watch::channel(false);
    let finished = tokio::spawn(async {});
    while !finished.is_finished() {
        tokio::task::yield_now().await;
    }
    let task_a = RexTask {
        target: target(),
        cancel: cancel_a,
        task: finished,
    };
    let mut table = std::collections::HashMap::new();
    table.insert("a".into(), task_a);

    let desired = std::collections::HashMap::from([("a".into(), target())]);
    let mut spawned = 0usize;
    let retired = reconcile_table(&mut table, desired, |_, _| {
        spawned += 1;
        let (cancel, _) = tokio::sync::watch::channel(false);
        RexTask {
            target: target(),
            cancel,
            task: tokio::spawn(async {}),
        }
    });
    assert_eq!(
        retired.len(),
        1,
        "finished stream must be retired before reconnect"
    );
    assert_eq!(spawned, 1, "a still-current worker must be reconnected");
    assert_eq!(table.len(), 1);
}
