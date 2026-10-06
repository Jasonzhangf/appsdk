use super::*;
#[test]
fn board_identity_rejects_a_retired_appserver_runtime() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("registration-reuse");
    let state_root = root.join("global");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(&state_root).unwrap();
    std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
    let route = json!({
        "version": 1,
        "op": "register",
        "app_scope_id": identity::CLI_APP_SERVER_ID,
        "project_scope": root.canonicalize().unwrap(),
        "canonical_root": root.canonicalize().unwrap(),
        "storage_root": root.canonicalize().unwrap(),
        "registered_ms": 1
    });
    std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();
    let runtime = RuntimeIdentity::cli_adapter("worker-1").unwrap();
    let mut identity = identity_with_runtime(Some(runtime));
    identity.project_scope = Some(
        Scope { root: root.clone() }
            .route_scope(identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap())
            .unwrap()
            .project_scope_id,
    );
    identity.transport = Some(SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/codex.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some("session-worker-1".into()),
        thread_id: Some("thread-worker-1".into()),
        tmux_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    });
    assert!(!persisted_runtime_matches_scope(&Scope { root: root.clone() }, &identity).unwrap());
    std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn legacy_identity_without_project_scope_requires_registration() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("registration-legacy-scope");
    let state_root = root.join("global");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(&state_root).unwrap();
    std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
    let route = json!({
        "version": 1,
        "op": "register",
        "app_scope_id": identity::CLI_APP_SERVER_ID,
        "project_scope": root.canonicalize().unwrap(),
        "canonical_root": root.canonicalize().unwrap(),
        "storage_root": root.canonicalize().unwrap(),
        "registered_ms": 1
    });
    std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();
    let runtime = RuntimeIdentity::cli_adapter("worker-1").unwrap();
    let identity = identity_with_runtime(Some(runtime));

    assert!(!persisted_runtime_matches_scope(&Scope { root: root.clone() }, &identity).unwrap());

    std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn board_identity_rejects_a_thread_bound_to_another_project() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("registration-rebind");
    let old_root = root.join("old-project");
    let new_root = root.join("new-project");
    let state_root = root.join("global");
    std::fs::create_dir_all(&old_root).unwrap();
    std::fs::create_dir_all(&new_root).unwrap();
    std::fs::create_dir_all(old_root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(new_root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(&state_root).unwrap();
    std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
    let route = json!({
        "version": 1,
        "op": "register",
        "app_scope_id": identity::CLI_APP_SERVER_ID,
        "project_scope": old_root.canonicalize().unwrap(),
        "canonical_root": old_root.canonicalize().unwrap(),
        "storage_root": old_root.canonicalize().unwrap(),
        "registered_ms": 1
    });
    std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();
    let runtime = RuntimeIdentity::cli_adapter("worker-1").unwrap();
    let mut identity = identity_with_runtime(Some(runtime));
    identity.project_scope = Some(
        Scope {
            root: old_root.clone(),
        }
        .route_scope(identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap())
        .unwrap()
        .project_scope_id,
    );
    identity.transport = Some(SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/codex.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some("session-worker-1".into()),
        thread_id: Some("thread-worker-1".into()),
        tmux_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    });

    assert!(!persisted_runtime_matches_scope(
        &Scope {
            root: new_root.clone()
        },
        &identity
    )
    .unwrap());
    // A retired AppServer transport is not reusable even in its original
    // project; the next mutating command must bind the current tmux pane.
    assert!(!persisted_runtime_matches_scope(
        &Scope {
            root: old_root.clone()
        },
        &identity
    )
    .unwrap());

    std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn board_identity_rejects_a_legacy_thread_only_binding() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("registration-legacy-thread-only");
    let state_root = root.join("global");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(&state_root).unwrap();
    std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
    let route = json!({
        "version": 1,
        "op": "register",
        "app_scope_id": identity::CLI_APP_SERVER_ID,
        "project_scope": root.canonicalize().unwrap(),
        "canonical_root": root.canonicalize().unwrap(),
        "storage_root": root.canonicalize().unwrap(),
        "registered_ms": 1
    });
    std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

    // The pre-dual-key durable shape: a native thread with no session on
    // either the runtime or the selected transport.
    let runtime = RuntimeIdentity {
        agent_id: identity::AgentId::new("worker-1").unwrap(),
        runtime_id: identity::RuntimeId::new("runtime-legacy").unwrap(),
        appserver_id: identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap(),
        endpoint_generation: 1,
        binding_id: identity::BindingId::new("binding-legacy").unwrap(),
        session_id: None,
        native_thread_id: Some(identity::NativeThreadId::new("thread-legacy").unwrap()),
    };
    let mut identity = identity_with_runtime(Some(runtime));
    identity.project_scope = Some(
        Scope { root: root.clone() }
            .route_scope(identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap())
            .unwrap()
            .project_scope_id,
    );
    identity.transport = Some(SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/codex.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: None,
        thread_id: Some("thread-legacy".into()),
        tmux_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    });

    // A recovered legacy identity must not be reported as reusable: it
    // cannot resolve its own route until it re-registers with the host
    // session and upgrades to the strict dual-key binding.
    assert!(!persisted_runtime_matches_scope(&Scope { root: root.clone() }, &identity).unwrap());

    std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn init_runtime_projection_accepts_appserver_and_tmux_hosts() {
    let root = test_root("runtime-projection");
    let runtime = RuntimeIdentity {
        agent_id: identity::AgentId::new("worker-1").unwrap(),
        runtime_id: identity::RuntimeId::new("runtime-thread-1").unwrap(),
        appserver_id: identity::AppServerId::new("tui-default").unwrap(),
        endpoint_generation: 2,
        binding_id: identity::BindingId::new("binding-thread-1").unwrap(),
        session_id: Some(identity::SessionId::new("session-1").unwrap()),
        native_thread_id: Some(identity::NativeThreadId::new("thread-1").unwrap()),
    };
    let mut identity = identity_with_runtime(Some(runtime));
    identity.transport = Some(SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/codex.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some("session-1".into()),
        thread_id: Some("thread-1".into()),
        tmux_endpoint: None,
        capabilities: vec!["send_message_to_thread".into(), "read_thread".into()],
        self_check: "test appserver".into(),
    });
    let projection =
        registered_runtime_projection(&Scope { root: root.clone() }, &identity, 4242).unwrap();
    assert_eq!(projection["transport"], "appserver");
    assert_eq!(projection["threadId"], "thread-1");
    assert_eq!(projection["sessionId"], "session-1");
    assert_eq!(projection["endpoint"], "unix:///tmp/codex.sock");
    assert_eq!(projection["processId"], 4242);
    identity.transport = Some(SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some("/tmp/tmux-test.sock".into()),
        namespace: Some("$1".into()),
        session_id: Some("session-1".into()),
        thread_id: Some("thread-1".into()),
        tmux_endpoint: Some(proto::TmuxEndpoint {
            socket_path: "/tmp/tmux-test.sock".into(),
            server_pid: 42,
            tmux_session_id: "$1".into(),
            pane_id: "%1".into(),
            pane_pid: 43,
            codex_session_id: Some("session-1".into()),
            codex_thread_id: Some("thread-1".into()),
        }),
        capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
        self_check: "test tmux".into(),
    });
    let projection =
        registered_runtime_projection(&Scope { root: root.clone() }, &identity, 4242).unwrap();
    assert_eq!(projection["runtimeId"], "runtime-thread-1");
    assert_eq!(projection["appserverId"], "tui-default");
    assert_eq!(projection["transport"], "tmux");
    assert_eq!(projection["tmuxEndpoint"]["pane_id"], "%1");
    assert_eq!(
        projection["projectRoot"],
        root.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(projection["capabilities"][0], "send_message_to_pane");
    assert_eq!(projection["processId"], 4242);
    assert!(
        registered_runtime_projection(&Scope { root: root.clone() }, &identity, 0)
            .unwrap_err()
            .to_string()
            .contains("PID is zero")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn command_envelope_uses_the_registered_binding_and_generation() {
    let root = test_root("command-envelope");
    let runtime = RuntimeIdentity {
        agent_id: identity::AgentId::new("worker-1").unwrap(),
        runtime_id: identity::RuntimeId::new("runtime-live").unwrap(),
        appserver_id: identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap(),
        endpoint_generation: 9,
        binding_id: identity::BindingId::new("binding-live").unwrap(),
        session_id: None,
        native_thread_id: None,
    };
    let identity = identity_with_runtime(Some(runtime));
    let envelope = command_envelope(&Scope { root: root.clone() }, &identity).unwrap();
    assert_eq!(envelope.actor_binding_id.as_str(), "binding-live");
    assert_eq!(envelope.endpoint_generation, 9);
    assert_eq!(
        envelope.scope.app_scope_id.as_str(),
        identity::CLI_APP_SERVER_ID
    );
    assert_eq!(
        envelope.scope.project_scope_id.as_str(),
        root.canonicalize().unwrap().to_string_lossy()
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn subagent_snapshot_uses_the_authenticated_mutation_route() {
    assert!(subagent_observe_query(&subagent::Action::List).is_some());
    assert!(subagent_observe_query(&subagent::Action::Status { id: "child".into() }).is_some());
    assert!(subagent_observe_query(&subagent::Action::Snapshot {
        id: "child".into(),
        lines: 40,
    })
    .is_none());
}

#[test]
fn persisted_appserver_binding_requires_live_native_route_resolution() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("registration-appserver-live");
    // The daemon socket lives in the state root, so keep that path short
    // enough for `sockaddr_un`.
    let state_root = std::env::temp_dir().join(format!(
        "cs-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(&state_root).unwrap();
    std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
    set_current_session_thread("thread-1", "session-1");
    let route = json!({
        "version": 1,
        "op": "register",
        "app_scope_id": identity::CLI_APP_SERVER_ID,
        "project_scope": root.canonicalize().unwrap(),
        "canonical_root": root.canonicalize().unwrap(),
        "storage_root": root.canonicalize().unwrap(),
        "registered_ms": 1
    });
    std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

    let runtime = RuntimeIdentity {
        agent_id: identity::AgentId::new("worker-1").unwrap(),
        runtime_id: identity::RuntimeId::new("runtime-live-1").unwrap(),
        appserver_id: identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap(),
        endpoint_generation: 1,
        binding_id: identity::BindingId::new("binding-live-1").unwrap(),
        session_id: Some(identity::SessionId::new("session-1").unwrap()),
        native_thread_id: Some(identity::NativeThreadId::new("thread-1").unwrap()),
    };
    let mut identity = identity_with_runtime(Some(runtime));
    identity.project_scope = Some(
        Scope { root: root.clone() }
            .route_scope(identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap())
            .unwrap()
            .project_scope_id,
    );
    identity.transport = Some(SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/codex.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some("session-1".into()),
        thread_id: Some("thread-1".into()),
        tmux_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    });

    let socket = Scope { root: root.clone() }.sock_path();
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let root_string = root.canonicalize().unwrap().to_string_lossy().into_owned();
    let responder = std::thread::spawn(move || {
        use std::io::Write;
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        std::io::BufReader::new(&stream)
            .read_line(&mut line)
            .unwrap();
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["op"], "RouteResolveNative");
        assert_eq!(request["session_id"], "session-1");
        assert_eq!(request["native_thread_id"], "thread-1");
        let response = json!({
            "ok": true,
            "app_scope_id": identity::CLI_APP_SERVER_ID,
            "project_scope": root_string,
            "canonical_root": root_string,
            "storage_root": root_string,
            "agent_id": "worker-1",
            "binding_id": "binding-live-1",
            "endpoint_generation": 1,
            "session_id": "session-1",
            "native_thread_id": "thread-1"
        });
        stream
            .write_all(format!("{response}\n").as_bytes())
            .unwrap();
    });

    assert!(persisted_runtime_matches_scope(&Scope { root: root.clone() }, &identity).unwrap());
    responder.join().unwrap();
    clear_current_session_thread();
    std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn persisted_binding_for_another_session_or_thread_is_not_reusable() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("registration-foreign-address");
    let state_root = root.join("global");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(&state_root).unwrap();
    std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
    set_current_session_thread("thread-current", "session-current");
    let route = json!({
        "version": 1,
        "op": "register",
        "app_scope_id": identity::CLI_APP_SERVER_ID,
        "project_scope": root.canonicalize().unwrap(),
        "canonical_root": root.canonicalize().unwrap(),
        "storage_root": root.canonicalize().unwrap(),
        "registered_ms": 1
    });
    std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();
    let runtime = RuntimeIdentity {
        agent_id: identity::AgentId::new("worker-1").unwrap(),
        runtime_id: identity::RuntimeId::new("runtime-worker-1").unwrap(),
        appserver_id: identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap(),
        endpoint_generation: 1,
        binding_id: identity::BindingId::new("binding-worker-1").unwrap(),
        session_id: Some(identity::SessionId::new("session-other").unwrap()),
        native_thread_id: Some(identity::NativeThreadId::new("thread-other").unwrap()),
    };
    let mut identity = identity_with_runtime(Some(runtime));
    identity.project_scope = Some(
        Scope { root: root.clone() }
            .route_scope(identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap())
            .unwrap()
            .project_scope_id,
    );
    identity.transport = Some(SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/codex.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some("session-other".into()),
        thread_id: Some("thread-other".into()),
        tmux_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    });
    assert!(
        !persisted_runtime_matches_scope(&Scope { root: root.clone() }, &identity).unwrap(),
        "a binding for another session/thread must not be reused"
    );
    clear_current_session_thread();
    std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
    std::fs::remove_dir_all(root).ok();
}
