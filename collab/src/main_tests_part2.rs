use super::*;

#[test]
fn context_collector_never_derives_a_socket_from_the_tmux_environment() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let keys = ["TMUX", "TMUX_PANE", "CODEX_SESSION_ID", "CODEX_THREAD_ID"];
    let previous: Vec<_> = keys.iter().map(|key| std::env::var_os(key)).collect();
    std::env::set_var("TMUX", "/tmp/collab-no-live-collector.sock,123,0");
    std::env::set_var("TMUX_PANE", "%3");
    std::env::remove_var("CODEX_SESSION_ID");
    std::env::remove_var("CODEX_THREAD_ID");
    let result = main_context::collect_identity_facts(None);
    let root = test_root("stable-pane-readonly");
    let paths = crate::scope::HostPaths::for_state_root(root.join("host-state")).unwrap();
    let read_only = identity::registered_from_map(
        &paths,
        &Scope { root: root.clone() },
        &proto::IdentityFacts::default(),
    );
    for (key, value) in keys.iter().zip(previous) {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
    let facts = result.expect("stable pane facts reach daemon despite a live display failure");
    match read_only {
        Ok(identity) => assert!(identity.is_none(), "fake TMUX must not select an identity"),
        Err(error) => assert!(
            error
                .to_string()
                .contains("TMUX_OBSERVATION_CAPABILITY_MISSING"),
            "{error}"
        ),
    }
    assert!(facts.tmux_anchor.is_none());
    assert_eq!(facts.caller_pid, Some(std::process::id()));
    assert_eq!(facts.pane_hint.as_deref(), Some("%3"));
    assert!(
        facts.tmux.is_none(),
        "failed discovery must not invent endpoint fields"
    );
    assert!(facts.session_id.is_none() && facts.thread_id.is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_runtime_projection_accepts_appserver_and_tmux_hosts() {
    let root = test_root("runtime-projection");
    let runtime = RuntimeIdentity {
        agent_id: identity::AgentId::new("worker-1").unwrap(),
        runtime_id: identity::RuntimeId::new("runtime-thread-1").unwrap(),
        appserver_id: identity::AppServerId::new("tui-default").unwrap(),
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
        rex_endpoint: None,
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
        rex_endpoint: None,
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
fn command_envelope_uses_the_registered_binding_and_scope() {
    let root = test_root("command-envelope");
    let identity = identity_with_runtime(Some(RuntimeIdentity {
        agent_id: identity::AgentId::new("worker-1").unwrap(),
        runtime_id: identity::RuntimeId::new("runtime-live").unwrap(),
        appserver_id: identity::AppServerId::new(identity::CLI_APP_SERVER_ID).unwrap(),
        binding_id: identity::BindingId::new("binding-live").unwrap(),
        session_id: None,
        native_thread_id: None,
    }));
    let envelope = command_envelope(&Scope { root: root.clone() }, &identity).unwrap();
    assert_eq!(envelope.actor_binding_id.as_str(), "binding-live");
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
        lines: 40
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
        assert_eq!(
            subagent_observe_query(action).is_some(),
            matches!(
                action,
                subagent::Action::List | subagent::Action::Status { .. }
            ),
            "unexpected observe routing for {action:?}"
        );
    }
}

#[test]
fn context_response_preserves_typed_error_data_without_colliding_ok_fields() {
    for (ok, result) in [
        (
            false,
            json!({"action":"context", "outcome":"missing_facts", "requires":{
            "kind":"identity_facts", "fields":["session_id"], "sources":{"session_id":"Current runtime session identifier"},
            "approval":null, "repair_invocation":"collab context --provide '<JSON containing required_fields>'"}}),
        ),
        (
            true,
            json!({"action":"context", "outcome":"completed", "snapshot":{"registered":true}, "owner_readback":{}}),
        ),
    ] {
        let response = Resp {
            ok,
            error: None,
            data: json!({"result":result}),
        };
        let wire = serde_json::to_string(&response).unwrap();
        assert_eq!(wire.matches("\"ok\"").count(), 1, "{wire}");
        let decoded: Resp = serde_json::from_str(&wire).unwrap();
        assert_eq!(decoded.ok, ok);
        assert!(decoded.data.get("ok").is_none());
        assert_eq!(decoded.data["result"], result);
        let projected = main_context::public_context_response(decoded).unwrap();
        assert_eq!(projected["ok"], ok);
        assert_eq!(projected["result"], result);
    }
    let response = Resp::err("IDENTITY_MAP_DECODE_FAILED: invalid current map");
    let decoded: Resp = serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
    let error = main_context::public_context_response(decoded).unwrap_err();
    assert!(
        error.to_string().contains("IDENTITY_MAP_DECODE_FAILED"),
        "{error}"
    );
}
