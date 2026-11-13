use super::*;
use crate::server::name_state::{AgentPresence, AgentPresenceRecord};
use crate::server::peer_tests::{
    promote_master, register_tmux, seed_committed_owner, test_appserver_transport, test_server,
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

fn register_desktop_bridge(
    server: &Server,
    worker_id: &str,
    thread_id: &str,
    endpoint: &str,
) -> RuntimeBinding {
    let app = AppServerId::new("desktop").unwrap();
    let transport = SelectedTransport {
        kind: TransportKind::DesktopBridge,
        endpoint: Some(endpoint.into()),
        namespace: Some("codex_app".into()),
        session_id: Some(format!("session-{worker_id}")),
        thread_id: Some(thread_id.into()),
        tmux_endpoint: None,
        rex_endpoint: None,
        capabilities: vec!["send_message_to_thread".into(), "read_thread".into()],
        self_check: "Desktop bridge verified registered thread and project".into(),
    };
    let response = seed_committed_owner(
        server,
        worker_id,
        &format!("token-{worker_id}"),
        &transport,
        &server.root.display().to_string(),
        Some(app.clone()),
    );
    assert!(response.ok, "seed DesktopBridge peer: {response:?}");
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

fn rex_transport(root: &Path) -> SelectedTransport {
    SelectedTransport {
        kind: TransportKind::Rex,
        endpoint: None,
        namespace: None,
        session_id: Some(format!(
            "agent-{}",
            root.file_name().unwrap().to_string_lossy()
        )),
        thread_id: None,
        tmux_endpoint: None,
        rex_endpoint: Some(crate::proto::RexEndpoint {
            server: "unix:///tmp/rex-fixture.sock".into(),
            session_id: "session:11111111-1111-1111-1111-111111111111".into(),
            block_id: "block:22222222-2222-2222-2222-222222222222".into(),
            host_id: Some("host-fixture".into()),
            instance_id: "instance-fixture".into(),
            cwd: root.display().to_string(),
        }),
        capabilities: vec!["send_message_to_block".into()],
        self_check: "maintenance Rex fixture".into(),
    }
}

struct RexFixture {
    dir: PathBuf,
    old_log_dir: Option<std::ffi::OsString>,
    old_scenario: Option<std::ffi::OsString>,
}

impl RexFixture {
    fn new(root: &Path, scenario: &str) -> Self {
        let dir = root.join("rex-cli-fixture");
        std::fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("rex-cli");
        std::fs::write(&fixture, include_str!("../../adapters/rex/fixture_rex.py")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::client::adapters::rex::set_test_cli(Some(fixture));
        crate::client::adapters::rex::set_test_timeout(Some(std::time::Duration::from_secs(5)));
        let old_log_dir = std::env::var_os("REXPRIMITIVE_LOG_DIR");
        let old_scenario = std::env::var_os("REXPRIMITIVE_SCENARIO");
        std::env::set_var("REXPRIMITIVE_LOG_DIR", &dir);
        std::env::set_var("REXPRIMITIVE_SCENARIO", scenario);
        Self {
            dir,
            old_log_dir,
            old_scenario,
        }
    }
}

impl Drop for RexFixture {
    fn drop(&mut self) {
        crate::client::adapters::rex::set_test_cli(None);
        crate::client::adapters::rex::set_test_timeout(None);
        match self.old_log_dir.take() {
            Some(value) => std::env::set_var("REXPRIMITIVE_LOG_DIR", value),
            None => std::env::remove_var("REXPRIMITIVE_LOG_DIR"),
        }
        match self.old_scenario.take() {
            Some(value) => std::env::set_var("REXPRIMITIVE_SCENARIO", value),
            None => std::env::remove_var("REXPRIMITIVE_SCENARIO"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_maintenance_retires_a_definitively_missing_block() {
    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let (server, root) = test_server();
    let _fixture = RexFixture::new(&root, "block_missing");
    let transport = rex_transport(&root);
    let response = seed_committed_owner(
        &server,
        "rex-missing-block",
        "token-rex-missing-block",
        &transport,
        root.to_str().unwrap(),
        None,
    );
    assert!(response.ok, "{response:?}");
    let server = Arc::new(server);
    let maintenance_server = server.clone();
    let summary = tokio::task::spawn_blocking(move || {
        maintain_definitive_missing(&maintenance_server, 7_500)
    })
    .await
    .unwrap();

    assert_eq!(
        entry(&summary, "retired", "rex-missing-block")["close"]["runtime_archive"]["state"],
        "missing",
        "{summary}"
    );
    assert!(!server
        .state
        .lock()
        .unwrap()
        .workers
        .contains_key("rex-missing-block"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_maintenance_keeps_identity_when_the_block_probe_is_unknown() {
    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let (server, root) = test_server();
    let _fixture = RexFixture::new(&root, "status_unknown");
    let transport = rex_transport(&root);
    let response = seed_committed_owner(
        &server,
        "rex-unknown-block",
        "token-rex-unknown-block",
        &transport,
        root.to_str().unwrap(),
        None,
    );
    assert!(response.ok, "{response:?}");
    let binding = binding_for(&server, "rex-unknown-block");
    let server = Arc::new(server);
    let maintenance_server = server.clone();
    let summary = tokio::task::spawn_blocking(move || {
        maintain_definitive_missing(&maintenance_server, 7_501)
    })
    .await
    .unwrap();

    assert_eq!(
        entry(&summary, "retained", "rex-unknown-block")["reason"]["presence"],
        "unknown",
        "{summary}"
    );
    let state = server.state.lock().unwrap();
    assert!(state.workers.contains_key("rex-unknown-block"));
    assert_eq!(
        state
            .global
            .lookup_binding_for(&binding.route_scope(), &binding.binding_id)
            .unwrap(),
        &binding
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_maintenance_keeps_identity_when_session_response_is_malformed() {
    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let (server, root) = test_server();
    let _fixture = RexFixture::new(&root, "session_malformed");
    let transport = rex_transport(&root);
    let response = seed_committed_owner(
        &server,
        "rex-malformed-session",
        "token-rex-malformed-session",
        &transport,
        root.to_str().unwrap(),
        None,
    );
    assert!(response.ok, "{response:?}");
    let binding = binding_for(&server, "rex-malformed-session");
    let server = Arc::new(server);
    let maintenance_server = server.clone();
    let summary = tokio::task::spawn_blocking(move || {
        maintain_definitive_missing(&maintenance_server, 7_502)
    })
    .await
    .unwrap();

    assert_eq!(
        entry(&summary, "retained", "rex-malformed-session")["reason"]["presence"],
        "unknown",
        "{summary}"
    );
    let state = server.state.lock().unwrap();
    assert!(state.workers.contains_key("rex-malformed-session"));
    assert_eq!(
        state
            .global
            .lookup_binding_for(&binding.route_scope(), &binding.binding_id)
            .unwrap(),
        &binding
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rex_maintenance_keeps_identity_when_block_response_is_malformed() {
    let _env_guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _fixture_guard = crate::client::adapters::rex::TEST_FIXTURE_LOCK.lock().await;
    let (server, root) = test_server();
    let _fixture = RexFixture::new(&root, "blocks_malformed");
    let transport = rex_transport(&root);
    let response = seed_committed_owner(
        &server,
        "rex-malformed-blocks",
        "token-rex-malformed-blocks",
        &transport,
        root.to_str().unwrap(),
        None,
    );
    assert!(response.ok, "{response:?}");
    let binding = binding_for(&server, "rex-malformed-blocks");
    let server = Arc::new(server);
    let maintenance_server = server.clone();
    let summary = tokio::task::spawn_blocking(move || {
        maintain_definitive_missing(&maintenance_server, 7_503)
    })
    .await
    .unwrap();

    assert_eq!(
        entry(&summary, "retained", "rex-malformed-blocks")["reason"]["presence"],
        "unknown",
        "{summary}"
    );
    let state = server.state.lock().unwrap();
    assert!(state.workers.contains_key("rex-malformed-blocks"));
    assert_eq!(
        state
            .global
            .lookup_binding_for(&binding.route_scope(), &binding.binding_id)
            .unwrap(),
        &binding
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
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
    promote_master(
        &server,
        "missing-peer",
        "fixture approval for stale master retention",
    );
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
    let default_id = mailbox::default_direct_message_id("missing-peer");
    let reception_before =
        serde_json::to_value(&server.state.lock().unwrap().notification_subscriptions[&default_id])
            .unwrap();

    let summary = maintain_definitive_missing(&server, 1_234);

    assert_only_bucket(&summary, "retired", "missing-peer");
    let retired = entry(&summary, "retired", "missing-peer");
    assert!(retired["operation_id"]
        .as_str()
        .unwrap()
        .starts_with(&format!(
            "stale-identity-maintenance:{}",
            binding.binding_id.as_str()
        )));
    assert_eq!(retired["close"]["runtime_archive"]["state"], "missing");
    let operation_id = retired["operation_id"].as_str().unwrap();
    let state = server.state.lock().unwrap();
    let operation = &state.peer_lifecycle_operations[operation_id];
    assert_eq!(operation.action, PeerLifecycleAction::Close);
    assert_eq!(operation.phase, PeerLifecyclePhase::Complete);
    assert!(operation
        .responsibility_snapshot
        .as_ref()
        .unwrap()
        .subscription_ids
        .contains(&default_id));
    assert_eq!(
        retired["close"]["subscription_retirement"]["state"],
        "not-applicable"
    );
    assert_eq!(
        serde_json::to_value(&state.notification_subscriptions[&default_id]).unwrap(),
        reception_before
    );
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
fn stale_desktop_projection_does_not_retire_a_live_bridge_thread() {
    let (mut server, root) = test_server();
    install_appserver_status(&mut server);
    let binding = register_desktop_bridge(
        &server,
        "bridge-peer",
        "thread-present",
        "unix:///tmp/collab-test-desktop-bridge.sock",
    );
    let session = server.state.lock().unwrap().workers["bridge-peer"]
        .transport
        .as_ref()
        .and_then(|transport| transport.session_id.clone())
        .expect("DesktopBridge fixture has a session");
    server.commit(&[Event::AgentPresenceObserved {
        presence: AgentPresenceRecord {
            project_scope: binding.project_scope.as_str().into(),
            worker_id: "bridge-peer".into(),
            session_id: session,
            presence: AgentPresence::Offline,
            reason: "stale desktop projection fixture".into(),
            observed_ms: 1,
        },
    }]);

    let summary = maintain_definitive_missing(&server, 3_600);

    assert_only_bucket(&summary, "retained", "bridge-peer");
    let retained = entry(&summary, "retained", "bridge-peer");
    assert_eq!(retained["reason"]["presence"], "present", "{summary}");
    let state = server.state.lock().unwrap();
    assert!(state.workers.contains_key("bridge-peer"));
    assert!(!state.worker_closures.contains_key("bridge-peer"));
    assert_eq!(
        state
            .global
            .lookup_binding_for(&binding.route_scope(), &binding.binding_id)
            .unwrap(),
        &binding
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn desktop_bridge_connection_failure_is_not_definitive_missing() {
    let (mut server, root) = test_server();
    server.appserver_thread_status = Arc::new(|_, thread_id| {
        if thread_id != "thread-desktop" {
            panic!("unexpected DesktopBridge thread {thread_id}");
        }
        Err(format!(
            "{} connection refused",
            crate::client::adapters::desktop_bridge::NOT_SUBMITTED_PREFIX
        ))
    });
    let binding = register_desktop_bridge(
        &server,
        "unsubmitted-peer",
        "thread-desktop",
        "unix:///tmp/collab-test-desktop-bridge.sock",
    );

    let summary = maintain_definitive_missing(&server, 3_700);

    assert_only_bucket(&summary, "retained", "unsubmitted-peer");
    assert_eq!(
        entry(&summary, "retained", "unsubmitted-peer")["reason"]["presence"],
        "unknown"
    );
    let state = server.state.lock().unwrap();
    assert!(state.workers.contains_key("unsubmitted-peer"));
    assert!(!state.worker_closures.contains_key("unsubmitted-peer"));
    assert!(state.peer_lifecycle_operations.values().all(|operation| {
        !operation
            .target
            .as_ref()
            .is_some_and(|target| target.worker_id == "unsubmitted-peer")
    }));
    drop(state);
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
            .unwrap(),
        &binding
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
        let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();

        let summary = maintain_definitive_missing(&server, 3_000);

        assert_only_bucket(&summary, "retained", "observed-peer");
        let retained = entry(&summary, "retained", "observed-peer");
        assert_eq!(retained["reason"]["kind"], "identity_presence");
        assert_eq!(retained["reason"]["presence"], expected);
        let state = server.state.lock().unwrap();
        assert_eq!(
            serde_json::to_value(&*state).unwrap(),
            before,
            "{thread_id}"
        );
        assert!(state.workers.contains_key("observed-peer"), "{thread_id}");
        assert_eq!(
            state
                .global
                .lookup_binding_for(&binding.route_scope(), &binding.binding_id)
                .unwrap(),
            &binding
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
fn active_responsibility_fence_is_retained() {
    let (mut server, root) = test_server();
    install_appserver_status(&mut server);
    let binding = register_appserver(&server, "fenced-peer", "thread-missing");
    let mut fence = exact_fence("fenced-peer", &binding);
    fence.operation_id = "stale-identity-maintenance:other-operation".into();
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
fn unread_message_alone_retains_missing_worker() {
    for status in ["pending", "delivered", "read"] {
        let (mut server, root) = test_server();
        install_appserver_status(&mut server);
        register_appserver(&server, "mail-peer", "thread-missing");
        let message = state::Message {
            id: "mail-only".into(),
            from: "sender".into(),
            to: "mail-peer".into(),
            mtype: "notification".into(),
            subject: None,
            body: "keep current message".into(),
            in_reply_to: None,
            created_ms: 1,
            state: status.into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        };
        server.try_commit(&[Event::Sent { msg: message }]).unwrap();
        let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        let map_before =
            std::fs::read(server.host_paths.state_root().join("identity-map.json")).unwrap();

        let summary = maintain_definitive_missing(&server, 4_100);

        if status == "read" {
            assert_only_bucket(&summary, "retired", "mail-peer");
        } else {
            assert_only_bucket(&summary, "retained", "mail-peer");
            assert_eq!(
                entry(&summary, "retained", "mail-peer")["reason"]["fields"],
                json!(["unread_messages"])
            );
            assert_eq!(
                serde_json::to_value(&*server.state.lock().unwrap()).unwrap(),
                before
            );
        }
        assert_eq!(
            std::fs::read(server.host_paths.state_root().join("identity-map.json")).unwrap(),
            map_before
        );
        let state = server.state.lock().unwrap();
        assert_eq!(
            serde_json::to_value(&state.msgs["mail-only"]).unwrap(),
            before["msgs"]["mail-only"]
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn active_subscription_alone_retains_missing_worker() {
    for (status, subscription_id) in [
        ("armed", "sub-only"),
        ("notifying", "sub-only"),
        ("armed", "sub-default-direct-message-other-worker"),
        ("cancelled", "sub-only"),
    ] {
        let (mut server, root) = test_server();
        install_appserver_status(&mut server);
        register_appserver(&server, "sub-peer", "thread-missing");
        let subscription: state::NotificationSubscription = serde_json::from_value(json!({
            "id": subscription_id, "worker_id": "sub-peer", "event": "deadline",
            "subject": "goal:retained", "trigger_ms": now_ms() + 60_000,
            "target": "thread-missing", "method": "appserver", "expires_ms": i64::MAX,
            "status": status, "created_ms": 1, "updated_ms": 1
        }))
        .unwrap();
        server
            .try_commit(&[Event::NotificationSubscribed { subscription }])
            .unwrap();
        let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        let map_before =
            std::fs::read(server.host_paths.state_root().join("identity-map.json")).unwrap();

        let summary = maintain_definitive_missing(&server, 4_200);

        if status == "cancelled" {
            assert_only_bucket(&summary, "retired", "sub-peer");
        } else {
            assert_only_bucket(&summary, "retained", "sub-peer");
            assert_eq!(
                entry(&summary, "retained", "sub-peer")["reason"]["fields"],
                json!(["subscriptions"])
            );
            assert_eq!(
                entry(&summary, "retained", "sub-peer")["reason"]["sources"]["subscriptions"],
                json!([subscription_id])
            );
            assert_eq!(
                serde_json::to_value(&*server.state.lock().unwrap()).unwrap(),
                before
            );
        }
        assert_eq!(
            std::fs::read(server.host_paths.state_root().join("identity-map.json")).unwrap(),
            map_before
        );
        let state = server.state.lock().unwrap();
        assert_eq!(
            serde_json::to_value(&state.notification_subscriptions[subscription_id]).unwrap(),
            before["notification_subscriptions"][subscription_id]
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
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
fn exact_target_rebind_during_probe_is_retained_before_admission() {
    let (mut server, root) = test_server();
    let (probe_started_tx, probe_started_rx) = std::sync::mpsc::channel::<()>();
    let (rebind_done_tx, rebind_done_rx) = std::sync::mpsc::channel::<()>();
    let probe_started_tx = Mutex::new(probe_started_tx);
    let rebind_done_rx = Mutex::new(rebind_done_rx);
    server.appserver_thread_status = Arc::new(move |_, thread_id| {
        if thread_id != "thread-rebind" {
            panic!("unexpected App Server thread {thread_id}");
        }
        probe_started_tx.lock().unwrap().send(()).unwrap();
        rebind_done_rx.lock().unwrap().recv().unwrap();
        Err("thread not found".into())
    });
    let server = Arc::new(server);
    let binding = register_appserver(&server, "rebind-peer", "thread-rebind");
    let writer_server = Arc::clone(&server);
    let writer = std::thread::spawn(move || {
        probe_started_rx.recv().unwrap();
        let mut rebound = writer_server
            .state
            .lock()
            .unwrap()
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .find(|binding| binding.agent_id.as_str() == "rebind-peer")
            .cloned()
            .expect("current rebind peer binding");
        rebound.runtime_id = RuntimeId::new("replacement-runtime").unwrap();
        writer_server
            .try_commit(&[Event::GlobalRuntimeBound { binding: rebound }])
            .unwrap();
        rebind_done_tx.send(()).unwrap();
    });

    let summary = maintain_definitive_missing(&server, 6_150);
    writer.join().unwrap();

    assert_only_bucket(&summary, "retained", "rebind-peer");
    assert_eq!(
        entry(&summary, "retained", "rebind-peer")["reason"]["kind"],
        "target_changed_during_probe"
    );
    let state = server.state.lock().unwrap();
    assert!(state.workers.contains_key("rebind-peer"));
    assert!(!state.worker_closures.contains_key("rebind-peer"));
    assert_eq!(
        state
            .global
            .lookup_binding_for(&binding.route_scope(), &binding.binding_id)
            .unwrap()
            .runtime_id
            .as_str(),
        "replacement-runtime"
    );
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
fn maintenance_operation_reuse_requires_exact_target() {
    let (mut server, root) = test_server();
    install_appserver_status(&mut server);
    let binding = register_appserver(&server, "operation-peer", "thread-missing");
    let candidate = {
        let state = server.state.lock().unwrap();
        sample_candidate(&state, &state.workers["operation-peer"], binding)
    };
    let admitted = {
        let mut state = server.state.lock().unwrap();
        admit_maintenance_close(&server, &mut state, &candidate).unwrap()
    };
    let reused = {
        let mut state = server.state.lock().unwrap();
        let current = sample_candidate(
            &state,
            &state.workers["operation-peer"],
            candidate.binding.clone(),
        );
        admit_maintenance_close(&server, &mut state, &current).unwrap()
    };
    assert_eq!(reused.operation_id, admitted.operation_id);
    let rejected = server.try_commit(&[Event::TaskCreated {
        task: open_task("operation-peer", "after-admission"),
    }]);
    assert!(rejected.unwrap_err().contains("target_closing"));
    let mut state = server.state.lock().unwrap();
    assert!(!state.tasks.contains_key("after-admission"));
    assert!(state.responsibility_fences[&admitted.operation_id].is_active());
    let mut changed = sample_candidate(
        &state,
        &state.workers["operation-peer"],
        candidate.binding.clone(),
    );
    changed.worker.token = "superseded-token".into();
    assert!(matches!(
        admit_maintenance_close(&server, &mut state, &changed),
        Err(MaintenanceAdmissionError::Retained(_))
    ));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn responsibility_before_locked_admission_is_retained() {
    let (server, root) = test_server();
    let binding = register_appserver(&server, "ordered-peer", "thread-missing");
    let candidate = {
        let state = server.state.lock().unwrap();
        sample_candidate(&state, &state.workers["ordered-peer"], binding)
    };
    server
        .try_commit(&[Event::TaskCreated {
            task: open_task("ordered-peer", "before-admission"),
        }])
        .unwrap();
    let mut state = server.state.lock().unwrap();
    assert!(matches!(
        admit_maintenance_close(&server, &mut state, &candidate),
        Err(MaintenanceAdmissionError::Retained(_))
    ));
    assert!(state.tasks.contains_key("before-admission"));
    assert!(state.peer_lifecycle_operations.is_empty());
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
            .native_thread_id,
        None
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
