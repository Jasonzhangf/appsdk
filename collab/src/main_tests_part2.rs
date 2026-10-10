use super::*;

/// Build the exact `Resp` wire the daemon emits for a typed operation result:
/// outer `ok` owns success and the flattened payload holds one `result` object.
fn typed_operation_wire(ok: bool, result: Value) -> Resp {
    Resp {
        ok,
        error: None,
        data: json!({"result": result}),
    }
}

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
fn subagent_action_routing_sends_only_list_and_status_to_observe() {
    let actions = [
        subagent::Action::Dispatch {
            request_id: "request".into(),
            subject: "subject".into(),
            body: "body".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            next_step: None,
        },
        subagent::Action::List,
        subagent::Action::Status { id: "child".into() },
        subagent::Action::Snapshot {
            id: "child".into(),
            lines: 40,
        },
        subagent::Action::Rearm { id: "child".into() },
        subagent::Action::Send {
            id: "child".into(),
            subject: "subject".into(),
            body: "body".into(),
        },
        subagent::Action::Ready { id: "child".into() },
        subagent::Action::Working { id: "child".into() },
        subagent::Action::Close { id: "child".into() },
    ];

    for action in &actions {
        let observe_query = subagent_observe_query(action);
        assert_eq!(
            observe_query.is_some(),
            matches!(
                action,
                subagent::Action::List | subagent::Action::Status { .. }
            ),
            "unexpected observe routing for {action:?}"
        );
    }
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
fn context_query_uses_read_only_bootstrap_and_preserves_typed_error_data() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("context-query-pure");
    let state_root = std::env::temp_dir().join(format!(
        "cq-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&state_root).unwrap();
    let host_paths = scope::HostPaths::from_state_root(&state_root).unwrap();
    let canonical_root = root.canonicalize().unwrap();
    let route = json!({
        "version": 1,
        "op": "register",
        "app_scope_id": identity::CLI_APP_SERVER_ID,
        "project_scope": canonical_root,
        "canonical_root": canonical_root,
        "storage_root": canonical_root,
        "registered_ms": 1
    });
    std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();
    let project_context = crate::context_operation::cli_project_context_for_root(
        &root,
        &identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap(),
    )
    .unwrap();
    crate::context_operation::prepare_test_proof(&host_paths, &project_context, "ctxop-pure-query")
        .unwrap();
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(&root).unwrap();
    std::env::set_var(crate::scope::COLLAB_STATE_DIR_ENV, &state_root);
    let result = context_operation(Some("ctxop-pure-query".into()), None, true, Some(&root));
    std::env::remove_var(crate::scope::COLLAB_STATE_DIR_ENV);
    std::env::set_current_dir(previous).unwrap();
    let error = result.expect_err("query without a route or baseline must fail closed");
    assert!(
        error.to_string().starts_with("COLLAB_CONTEXT_UNRESOLVED"),
        "{error:#}"
    );
    assert!(!root.join(".agent-collab").exists());
    assert!(!state_root.join("server.pid").exists());
    assert!(!state_root.join("events.jsonl").exists());

    // The daemon carries a typed incomplete result as outer `ok:false` with a
    // single flattened `result` object. Round-trip it through the real wire
    // serde path: an inner `ok` would collapse into `Resp.ok` and lose the
    // typed payload, so exactly one `ok` survives serialization.
    // A committed nested Register with an incomplete later phase projects as
    // `partial`, never as success: the daemon reports outer `ok:false` with a
    // single flattened `result` object. Round-trip it through the real wire
    // serde path; an inner `ok` would collapse into `Resp.ok` and lose the
    // typed payload, so exactly one `ok` must survive serialization.
    let typed_incomplete = typed_operation_wire(
        false,
        json!({
            "operation_id": "ctxop-pure-query",
            "invocation": "automatic",
            "action": "context",
            "phase": "inner_dispatched",
            "outcome": "partial",
            "committed_phases": ["validating", "inner_dispatched"],
            "failed_phase": "route",
            "requires": {
                "kind": "repair",
                "fields": [],
                "sources": {},
                "approval": null,
                "repair_invocation": null
            },
            "snapshot": null,
            "owner_readback": {"route": {"state": "unknown"}},
            "queried_operation": null
        }),
    );
    let wire = serde_json::to_string(&typed_incomplete).unwrap();
    let error = client::ServerResponseError {
        response: serde_json::from_str(&wire).unwrap(),
    };
    assert_eq!(
        wire.matches("\"ok\"").count(),
        1,
        "the flattened wire must keep exactly one ok: {wire}"
    );
    assert_eq!(error.response.data.get("ok"), None);
    assert_eq!(error.response.ok, false);
    assert_eq!(error.response.data["result"]["outcome"], "partial");
    // The public mutation projection keeps the typed incomplete payload and
    // reports `ok:false` so the CLI exit/MCP isError mapping stays honest.
    let projected =
        main_context::public_context_response(error.response, false, "ctxop-pure-query").unwrap();
    assert_eq!(projected["ok"], false);
    assert_eq!(projected["result"]["operation_id"], "ctxop-pure-query");
    assert_eq!(projected["result"]["outcome"], "partial");
    assert_eq!(projected["result"]["phase"], "inner_dispatched");
    assert_eq!(
        projected["result"]["committed_phases"],
        json!(["validating", "inner_dispatched"])
    );

    // A completed operation projects as outer `ok:true` with the same
    // collision-free shape.
    let typed_complete = typed_operation_wire(
        true,
        json!({
            "operation_id": "ctxop-pure-query",
            "invocation": "automatic",
            "action": "context",
            "phase": "completed",
            "outcome": "completed",
            "committed_phases": ["validating", "inner_dispatched", "effect_observed", "completed"],
            "failed_phase": null,
            "requires": {
                "kind": null,
                "fields": [],
                "sources": {},
                "approval": null,
                "repair_invocation": null
            },
            "snapshot": {"registered": true},
            "owner_readback": {},
            "queried_operation": null
        }),
    );
    let wire = serde_json::to_string(&typed_complete).unwrap();
    assert_eq!(wire.matches("\"ok\"").count(), 1, "{wire}");
    let projected = main_context::public_context_response(
        serde_json::from_str(&wire).unwrap(),
        false,
        "ctxop-pure-query",
    )
    .unwrap();
    assert_eq!(projected["ok"], true);
    assert_eq!(projected["result"]["outcome"], "completed");
    // A transport/protocol failure carries no typed `result` and stays an
    // error rather than a projected payload.
    let transport = Resp::err("IDENTITY_OPERATION_UNKNOWN: no durable operation");
    let wire = serde_json::to_string(&transport).unwrap();
    let transport_error = client::ServerResponseError {
        response: serde_json::from_str(&wire).unwrap(),
    };
    assert!(main_context::public_context_response(
        transport_error.response,
        false,
        "ctxop-pure-query"
    )
    .is_err());
    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(state_root).ok();
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
