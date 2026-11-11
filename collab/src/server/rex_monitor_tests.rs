use super::*;
use crate::client::adapters::rex::{
    RexEvent, RexEventBatch, RexProcessObservation, RexProgramStatusOwner, RexProgramStatusRecord,
};
use crate::proto::RexEndpoint;
use crate::server::name_state::AgentActivity;
use serde_json::json;

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
        sources: vec!["com.superlogical.terminal.program_status".into()],
        source_errors: Vec::new(),
        gap_possible: false,
    }
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
fn status_mapping_covers_working_blocked_idle_and_unknown() {
    assert_eq!(
        project_observation(&observation(vec![record("working", None)])).0,
        AgentActivity::Busy
    );
    assert_eq!(
        project_observation(&observation(vec![record("blocked", Some("question"))])).0,
        AgentActivity::WaitingUser
    );
    assert_eq!(
        project_observation(&observation(vec![record("blocked", Some("approval"))])).0,
        AgentActivity::WaitingUser
    );
    assert_eq!(
        project_observation(&observation(Vec::new())).0,
        AgentActivity::Unknown
    );
    assert_eq!(
        project_observation(&observation(vec![
            record("working", None),
            record("blocked", Some("question")),
        ]))
        .0,
        AgentActivity::WaitingUser
    );
    for state in ["idle", "done", "error", "clear"] {
        assert_eq!(
            project_observation(&observation(vec![record(state, None)])).0,
            AgentActivity::Idle,
            "state {state} must project idle"
        );
    }
    let mut failed = observation(vec![record("working", None)]);
    failed.source_errors = vec!["com.superlogical.terminal.process: unavailable".into()];
    failed.gap_possible = true;
    let (activity, reason) = project_observation(&failed);
    assert_eq!(activity, AgentActivity::Unknown);
    assert!(reason.contains("rex.osc7501"));
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
        project_observation(&observation(vec![stale_name])).0,
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
        project_observation(&observation(vec![stale_pid])).0,
        AgentActivity::Unknown
    );
    // Unknown owner cannot be matched, so it stays out of the projection.
    let no_owner = owned_record("working", None, None);
    assert_eq!(
        project_observation(&observation(vec![no_owner])).0,
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
        project_observation(&observation(vec![stale_working, current_idle])).0,
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
            project_observation(&observation).0,
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
        project_observation(&blocked).0,
        AgentActivity::Unknown,
        "blocked record with incomplete foreground must stay unknown"
    );
}

#[test]
fn projection_keeps_source_and_emits_idle_or_busy() {
    let (activity, reason) = project_observation(&observation(vec![record("working", None)]));
    assert_eq!(activity, AgentActivity::Busy);
    assert!(reason.contains("rex.osc7501"));
    assert!(reason.contains("com.superlogical.terminal.program_status"));

    let (activity, reason) = project_observation(&observation(vec![record("idle", None)]));
    assert_eq!(activity, AgentActivity::Idle);
    assert!(reason.contains("projection=idle"));
    assert!(reason.contains("com.superlogical.terminal.program_status"));
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
    let (activity, reason) =
        project_stream_end(&disconnected).expect("disconnect must project an observation");
    assert_eq!(activity, AgentActivity::Unknown);
    assert!(reason.contains("events stream disconnected"));

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
