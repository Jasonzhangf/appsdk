use super::*;
use crate::server::name_state::{AgentPresence, AgentPresenceRecord};
use crate::server::peer_tests::{
    register_tmux, seed_committed_owner, test_appserver_transport, test_server,
};
use crate::server::state::{ReplyObligation, ResponsibilityFence, TaskRec};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

fn install_appserver_status(server: &mut Server) {
    server.appserver_thread_status = Arc::new(|_, thread_id| match thread_id {
        "thread-present" => Ok(serde_json::json!({
            "thread": {
                "id": thread_id,
                "status": {"type": "idle"},
                "canAcceptDirectInput": true
            }
        })),
        "thread-cold" => Ok(serde_json::json!({
            "thread": {
                "id": thread_id,
                "status": {"type": "notLoaded"},
                "canAcceptDirectInput": false
            }
        })),
        "thread-timeout" => Err("ADAPTER_TIMEOUT: thread/read timed out".into()),
        "thread-authdenied" => Err("ADAPTER_AUTH_DENIED: permission denied".into()),
        "thread-unknown" => Err("fixture App Server status unavailable".into()),
        "thread-missing" => Err("thread not found".into()),
        "thread-system-error" => Ok(serde_json::json!({
            "thread": {
                "id": thread_id,
                "status": {"type": "systemError"},
                "canAcceptDirectInput": false
            }
        })),
        other => panic!("unexpected App Server thread {other}"),
    });
}

fn register_appserver(server: &Server, worker_id: &str, thread_id: &str) -> RuntimeBinding {
    let app = AppServerId::new("tui-default").unwrap();
    let response = seed_committed_owner(
        server,
        worker_id,
        &format!("token-{worker_id}"),
        &test_appserver_transport(thread_id),
        &server.root.display().to_string(),
        Some(app.clone()),
    );
    assert!(response.ok, "seed AppServer peer: {response:?}");
    server
        .state
        .lock()
        .unwrap()
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| binding.agent_id.as_str() == worker_id && binding.app_scope_id == app)
        .cloned()
        .unwrap_or_else(|| panic!("current binding for {worker_id}"))
}

fn binding_for(server: &Server, worker_id: &str) -> RuntimeBinding {
    let state = server.state.lock().unwrap();
    let mut bindings = state
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .filter(|binding| binding.agent_id.as_str() == worker_id);
    let binding = bindings.next().cloned().expect("worker binding");
    assert!(bindings.next().is_none(), "worker must have one binding");
    binding
}

fn entry<'a>(
    summary: &'a serde_json::Value,
    bucket: &str,
    worker_id: &str,
) -> &'a serde_json::Value {
    summary[bucket]
        .as_array()
        .unwrap_or_else(|| panic!("summary bucket {bucket}: {summary}"))
        .iter()
        .find(|entry| entry["worker_id"] == worker_id)
        .unwrap_or_else(|| panic!("{worker_id} missing from {bucket}: {summary}"))
}

fn assert_only_bucket(summary: &serde_json::Value, bucket: &str, worker_id: &str) {
    for other in ["retired", "retained", "unknown"] {
        let contains = summary[other]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["worker_id"] == worker_id);
        assert_eq!(contains, other == bucket, "{summary}");
    }
}

fn open_task(owner: &str, id: &str) -> TaskRec {
    TaskRec {
        id: id.into(),
        owner: owner.into(),
        created_by: owner.into(),
        feature_id: None,
        worktree_path: None,
        branch: None,
        base_commit: None,
        priority: "P1".into(),
        status: "working".into(),
        next_step: Some("retain this owner".into()),
        wait: None,
        isolation_mode: None,
        created_ms: 1,
        updated_ms: 1,
    }
}

fn exact_fence(worker_id: &str, binding: &RuntimeBinding) -> ResponsibilityFence {
    ResponsibilityFence {
        operation_id: format!("close-{worker_id}"),
        worker_id: worker_id.into(),
        project_scope: binding.project_scope.as_str().into(),
        app_scope: binding.app_scope_id.as_str().into(),
        binding_id: binding.binding_id.as_str().into(),
        endpoint_generation: binding.endpoint_generation,
        snapshot: state::ResponsibilitySnapshot::default(),
        state: "closing".into(),
        created_ms: 1,
    }
}

fn reply_obligation(worker_id: &str, message_id: &str) -> ReplyObligation {
    ReplyObligation {
        message_id: message_id.into(),
        project: "fixture-project".into(),
        from_project: None,
        from_ceo: false,
        from_worker_id: "requester".into(),
        from_name: None,
        recipient_worker_id: worker_id.into(),
        recipient_name: None,
        policy: crate::proto::ReplyPolicy::Required,
        created_ms: 1,
        reminded_idle_episode: false,
    }
}

fn tmux(socket: &Path, args: &[&str]) -> std::process::Output {
    Command::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(args)
        .output()
        .expect("run isolated tmux command")
}

struct OwnedTmux {
    socket: PathBuf,
    sessions: Vec<String>,
}

impl OwnedTmux {
    fn start(root: &Path, target: &str, keeper: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let socket = PathBuf::from(format!(
            "/tmp/collab-stale-maintenance-{}-{}.sock",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        for session in [target, keeper] {
            let output = tmux(
                &socket,
                &[
                    "new-session",
                    "-d",
                    "-s",
                    session,
                    "-c",
                    root.to_str().unwrap(),
                    "sleep 600",
                ],
            );
            assert!(
                output.status.success(),
                "create isolated tmux session {session}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Self {
            socket,
            sessions: vec![target.into(), keeper.into()],
        }
    }

    fn endpoint(&self, session: &str) -> TmuxEndpoint {
        let output = tmux(
            &self.socket,
            &[
                "display-message",
                "-p",
                "-t",
                &format!("{session}:0"),
                "#{pid}\t#{session_id}\t#{pane_id}\t#{pane_pid}",
            ],
        );
        assert!(
            output.status.success(),
            "inspect isolated tmux session: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = String::from_utf8(output.stdout).unwrap();
        let fields = output.trim_end().split('\t').collect::<Vec<_>>();
        assert_eq!(fields.len(), 4, "{output}");
        TmuxEndpoint {
            socket_path: self.socket.to_string_lossy().into_owned(),
            server_pid: fields[0].parse().unwrap(),
            tmux_session_id: fields[1].into(),
            pane_id: fields[2].into(),
            pane_pid: fields[3].parse().unwrap(),
            codex_session_id: Some(format!("session-{session}")),
            codex_thread_id: Some(format!("thread-{session}")),
        }
    }

    fn close_session(&mut self, session: &str) {
        if !self.sessions.iter().any(|owned| owned == session) {
            return;
        }
        let output = tmux(&self.socket, &["kill-session", "-t", session]);
        assert!(
            output.status.success(),
            "close test-owned tmux session {session}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        self.sessions.retain(|owned| owned != session);
    }
}

impl Drop for OwnedTmux {
    fn drop(&mut self) {
        for session in self.sessions.clone() {
            let _ = tmux(&self.socket, &["kill-session", "-t", &session]);
        }
    }
}

#[test]
fn definitive_missing_appserver_worker_retires_without_self_removal() {
    let (mut server, root) = test_server();
    install_appserver_status(&mut server);
    let binding = register_appserver(&server, "missing-peer", "thread-missing");
    let promoted = handle_master_promote(
        &server,
        "missing-peer".into(),
        "token-missing-peer".into(),
        "fixture approval for stale master retention".into(),
    );
    assert!(promoted.ok, "{promoted:?}");
    let route_scope = binding.route_scope();
    assert!(server
        .state
        .lock()
        .unwrap()
        .global
        .lookup_master_grant_for(&route_scope, &binding.binding_id)
        .is_some());
    let identity_path = server
        .host_paths
        .state_root()
        .join("identities")
        .join("missing-peer")
        .join("identity.json");
    std::fs::create_dir_all(identity_path.parent().unwrap()).unwrap();
    std::fs::write(&identity_path, br#"{"worker_id":"missing-peer"}"#).unwrap();

    let summary = maintain_definitive_missing(&server, 1_234);

    assert_only_bucket(&summary, "retired", "missing-peer");
    let retired = entry(&summary, "retired", "missing-peer");
    assert_eq!(
        retired["operation_id"],
        format!("stale-identity-maintenance:{}", binding.binding_id.as_str())
    );
    assert_eq!(retired["close"]["runtime_archive"]["state"], "missing");
    let operation_id = retired["operation_id"].as_str().unwrap();
    let state = server.state.lock().unwrap();
    let operation = &state.peer_lifecycle_operations[operation_id];
    assert_eq!(operation.action, PeerLifecycleAction::Close);
    assert_eq!(operation.phase, PeerLifecyclePhase::Complete);
    assert!(
        !state.responsibility_fences.contains_key(operation_id),
        "a completed maintenance operation releases its fence"
    );
    assert!(!state.workers.contains_key("missing-peer"));
    let closure = &state.worker_closures["missing-peer"];
    assert_ne!(closure.closed_by, "missing-peer");
    assert!(closure.closed_by.starts_with("daemon:"));
    let retired_binding = state
        .global
        .lookup_binding_for(&route_scope, &binding.binding_id)
        .unwrap();
    assert_eq!(
        retired_binding.endpoint_generation,
        binding.endpoint_generation + 1
    );
    assert!(retired_binding.native_thread_id.is_none());
    assert!(retired_binding.tmux_endpoint.is_none());
    assert!(
        state
            .global
            .lookup_master_grant_for(&route_scope, &binding.binding_id)
            .is_some(),
        "daemon maintenance must not revoke master"
    );
    drop(state);
    assert!(
        identity_path.exists(),
        "daemon maintenance must not clear the persisted identity"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn definitive_missing_worker_with_open_task_is_retained() {
    let (mut server, root) = test_server();
    install_appserver_status(&mut server);
    let binding = register_appserver(&server, "task-owner", "thread-missing");
    server.commit(&[Event::TaskCreated {
        task: open_task("task-owner", "open-task"),
    }]);

    let summary = maintain_definitive_missing(&server, 2_000);

    assert_only_bucket(&summary, "retained", "task-owner");
    let retained = entry(&summary, "retained", "task-owner");
    assert_eq!(retained["reason"]["kind"], "peer_responsibility");
    assert_eq!(retained["reason"]["fields"], json!(["tasks"]));
    assert_eq!(retained["reason"]["sources"]["tasks"], json!(["open-task"]));
    let state = server.state.lock().unwrap();
    assert!(state.workers.contains_key("task-owner"));
    assert_eq!(state.tasks["open-task"].owner, "task-owner");
    assert_eq!(
        state
            .global
            .lookup_binding_for(&binding.route_scope(), &binding.binding_id)
            .unwrap()
            .endpoint_generation,
        binding.endpoint_generation
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn non_definitive_presence_is_retained_without_mutation() {
    for (thread_id, expected) in [
        ("thread-present", "present"),
        ("thread-cold", "cold"),
        ("thread-unknown", "unknown"),
        ("thread-timeout", "unknown"),
        ("thread-authdenied", "unknown"),
        ("thread-system-error", "unknown"),
    ] {
        let (mut server, root) = test_server();
        install_appserver_status(&mut server);
        let binding = register_appserver(&server, "observed-peer", thread_id);
        let sequence = server.state.lock().unwrap().sequence;

        let summary = maintain_definitive_missing(&server, 3_000);

        assert_only_bucket(&summary, "retained", "observed-peer");
        let retained = entry(&summary, "retained", "observed-peer");
        assert_eq!(retained["reason"]["kind"], "identity_presence");
        assert_eq!(retained["reason"]["presence"], expected);
        let state = server.state.lock().unwrap();
        assert_eq!(state.sequence, sequence, "{thread_id}");
        assert!(state.workers.contains_key("observed-peer"), "{thread_id}");
        assert_eq!(
            state
                .global
                .lookup_binding_for(&binding.route_scope(), &binding.binding_id)
                .unwrap()
                .endpoint_generation,
            binding.endpoint_generation
        );
        assert!(!state.worker_closures.contains_key("observed-peer"));
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn stale_offline_telemetry_does_not_retire_a_live_appserver_thread() {
    let (mut server, root) = test_server();
    install_appserver_status(&mut server);
    let binding = register_appserver(&server, "telemetry-peer", "thread-present");
    let session = server.state.lock().unwrap().workers["telemetry-peer"]
        .transport
        .as_ref()
        .and_then(|transport| transport.session_id.clone())
        .expect("AppServer fixture has a session");
    server.commit(&[Event::AgentPresenceObserved {
        presence: AgentPresenceRecord {
            project_scope: binding.project_scope.as_str().into(),
            worker_id: "telemetry-peer".into(),
            session_id: session,
            presence: AgentPresence::Offline,
            reason: "stale telemetry fixture".into(),
            observed_ms: 1,
        },
    }]);

    let summary = maintain_definitive_missing(&server, 3_500);

    assert_only_bucket(&summary, "retained", "telemetry-peer");
    let retained = entry(&summary, "retained", "telemetry-peer");
    assert_eq!(retained["reason"]["kind"], "identity_presence");
    assert_eq!(retained["reason"]["presence"], "present");
    let state = server.state.lock().unwrap();
    assert!(state.workers.contains_key("telemetry-peer"));
    assert!(!state.worker_closures.contains_key("telemetry-peer"));
    assert!(
        state.peer_lifecycle_operations.is_empty()
            || !state
                .peer_lifecycle_operations
                .values()
                .any(|operation| operation
                    .target
                    .as_ref()
                    .is_some_and(|target| { target.worker_id == "telemetry-peer" }))
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn maintenance_incomplete_result_keeps_durable_operation_and_fence() {
    let (mut server, root) = test_server();
    install_appserver_status(&mut server);
    let mut binding = register_appserver(&server, "exhausted-peer", "thread-missing");
    binding.endpoint_generation = u64::MAX;
    server.commit(&[Event::GlobalRuntimeBound {
        binding: binding.clone(),
    }]);

    let summary = maintain_definitive_missing(&server, 8_000);

    assert_only_bucket(&summary, "unknown", "exhausted-peer");
    let unknown = entry(&summary, "unknown", "exhausted-peer");
    assert_eq!(unknown["reason"]["kind"], "retirement_incomplete");
    let operation_id = unknown["operation_id"].as_str().unwrap().to_owned();
    let state = server.state.lock().unwrap();
    let operation = &state.peer_lifecycle_operations[&operation_id];
    assert_eq!(operation.action, PeerLifecycleAction::Close);
    assert_eq!(operation.phase, PeerLifecyclePhase::CleanupOpen);
    let fence = &state.responsibility_fences[&operation_id];
    assert!(fence.is_active(), "cleanup-open must retain its fence");
    assert_eq!(fence.worker_id, "exhausted-peer");
    drop(state);

    // The durable fence rejects a responsibility added after admission, so a
    // late task cannot be removed together with the target.
    let rejected = server.try_commit(&[Event::TaskCreated {
        task: open_task("exhausted-peer", "late-task"),
    }]);
    assert!(rejected.is_err(), "late responsibility must be fenced");
    assert!(server
        .state
        .lock()
        .unwrap()
        .tasks
        .get("late-task")
        .is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn active_responsibility_fence_is_retained() {
    let (mut server, root) = test_server();
    install_appserver_status(&mut server);
    let binding = register_appserver(&server, "fenced-peer", "thread-missing");
    let fence = exact_fence("fenced-peer", &binding);
    server.commit(&[Event::ResponsibilityFenceSet {
        fence: fence.clone(),
    }]);

    let summary = maintain_definitive_missing(&server, 4_000);

    assert_only_bucket(&summary, "retained", "fenced-peer");
    let retained = entry(&summary, "retained", "fenced-peer");
    assert_eq!(retained["reason"]["kind"], "active_responsibility_fence");
    assert_eq!(
        retained["reason"]["operation_ids"],
        json!([fence.operation_id])
    );
    let state = server.state.lock().unwrap();
    assert!(state.workers.contains_key("fenced-peer"));
    assert!(state
        .responsibility_fences
        .contains_key(&fence.operation_id));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pending_reply_obligation_is_retained() {
    let (mut server, root) = test_server();
    install_appserver_status(&mut server);
    register_appserver(&server, "reply-owner", "thread-missing");
    let obligation = reply_obligation("reply-owner", "reply-message");
    server.commit(&[Event::ReplyObligationSet {
        obligation: obligation.clone(),
    }]);

    let summary = maintain_definitive_missing(&server, 5_000);

    assert_only_bucket(&summary, "retained", "reply-owner");
    let retained = entry(&summary, "retained", "reply-owner");
    assert_eq!(retained["reason"]["kind"], "pending_reply_obligation");
    assert_eq!(
        retained["reason"]["message_ids"],
        json!([obligation.message_id])
    );
    let state = server.state.lock().unwrap();
    assert!(state.workers.contains_key("reply-owner"));
    assert!(state.reply_obligations.contains_key("reply-message"));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_missing_probe_does_not_create_a_permanent_tombstone() {
    let (mut server, root) = test_server();
    let status = Arc::new(AtomicU8::new(0));
    let callback_status = Arc::clone(&status);
    server.appserver_thread_status =
        Arc::new(
            move |_, thread_id| match callback_status.load(Ordering::SeqCst) {
                0 => Err("thread not found".into()),
                1 => Ok(serde_json::json!({
                    "thread": {
                        "id": thread_id,
                        "status": {"type": "idle"},
                        "canAcceptDirectInput": true
                    }
                })),
                _ => unreachable!(),
            },
        );
    register_appserver(&server, "recovering-peer", "thread-recovering");
    server.commit(&[Event::TaskCreated {
        task: open_task("recovering-peer", "retain-first-round"),
    }]);

    let first = maintain_definitive_missing(&server, 6_000);
    assert_only_bucket(&first, "retained", "recovering-peer");
    assert_eq!(
        entry(&first, "retained", "recovering-peer")["reason"]["kind"],
        "peer_responsibility"
    );

    status.store(1, Ordering::SeqCst);
    let mut task = server.state.lock().unwrap().tasks["retain-first-round"].clone();
    task.status = "closed".into();
    server.commit(&[Event::TaskUpdated { task }]);
    let second = maintain_definitive_missing(&server, 6_001);

    assert_only_bucket(&second, "retained", "recovering-peer");
    assert_eq!(
        entry(&second, "retained", "recovering-peer")["reason"]["presence"],
        "present"
    );
    assert!(server
        .state
        .lock()
        .unwrap()
        .workers
        .contains_key("recovering-peer"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn responsibility_added_during_probe_is_retained_before_admission() {
    let (mut server, root) = test_server();
    let (probe_started_tx, probe_started_rx) = std::sync::mpsc::channel::<()>();
    let (commit_done_tx, commit_done_rx) = std::sync::mpsc::channel::<()>();
    let probe_started_tx = Mutex::new(probe_started_tx);
    let commit_done_rx = Mutex::new(commit_done_rx);
    server.appserver_thread_status = Arc::new(move |_, thread_id| {
        if thread_id != "thread-race" {
            panic!("unexpected App Server thread {thread_id}");
        }
        // This is the exact window the reviewer identified: the host probe
        // runs after the candidate snapshot and before admission, with the
        // state lock free for a concurrent writer.
        probe_started_tx.lock().unwrap().send(()).unwrap();
        commit_done_rx.lock().unwrap().recv().unwrap();
        Err("thread not found".into())
    });
    let server = Arc::new(server);
    register_appserver(&server, "race-peer", "thread-race");
    let writer_server = Arc::clone(&server);
    let writer = std::thread::spawn(move || {
        probe_started_rx.recv().unwrap();
        writer_server
            .try_commit(&[Event::TaskCreated {
                task: open_task("race-peer", "task-arrived-during-probe"),
            }])
            .unwrap();
        commit_done_tx.send(()).unwrap();
    });

    let summary = maintain_definitive_missing(&server, 6_100);
    writer.join().unwrap();

    assert_only_bucket(&summary, "retained", "race-peer");
    assert_eq!(
        entry(&summary, "retained", "race-peer")["reason"]["kind"],
        "responsibility_added_during_probe"
    );
    let state = server.state.lock().unwrap();
    assert!(state.workers.contains_key("race-peer"));
    assert!(state.tasks.contains_key("task-arrived-during-probe"));
    assert!(state
        .peer_lifecycle_operations
        .values()
        .all(|operation| !operation
            .operation_id
            .starts_with("stale-identity-maintenance:")));
    assert!(state.responsibility_fences.is_empty());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn isolated_tmux_present_is_retained_and_definitive_missing_is_retired() {
    let (server, root) = test_server();
    let mut owned_tmux = OwnedTmux::start(&root, "stale-target", "keeper");
    let endpoint = owned_tmux.endpoint("stale-target");
    let response = register_tmux(&server, "tmux-peer", endpoint);
    assert!(response.ok, "{response:?}");
    let binding = binding_for(&server, "tmux-peer");

    let present = maintain_definitive_missing(&server, 7_000);
    assert_only_bucket(&present, "retained", "tmux-peer");
    assert_eq!(
        entry(&present, "retained", "tmux-peer")["reason"]["presence"],
        "present"
    );

    // Keep the test-owned server alive through the keeper session, then close
    // only the exact session that owns the registered pane.
    owned_tmux.close_session("stale-target");
    let missing = maintain_definitive_missing(&server, 7_001);

    assert_only_bucket(&missing, "retired", "tmux-peer");
    assert_eq!(
        entry(&missing, "retired", "tmux-peer")["close"]["runtime_archive"]["state"],
        "missing"
    );
    let state = server.state.lock().unwrap();
    assert!(!state.workers.contains_key("tmux-peer"));
    assert_eq!(
        state
            .global
            .lookup_binding_for(&binding.route_scope(), &binding.binding_id)
            .unwrap()
            .endpoint_generation,
        binding.endpoint_generation + 1
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
