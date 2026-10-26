#[test]
fn test_anonymous_send_succeeds_with_unknown_from() {
    let (server, root) = test_server();
    register(&server, "receiver", "%receiver");
    let server_arc = Arc::new(server);
    let resp = dispatch(
        &server_arc,
        Req::Send {
            from: "spoofed-sender".into(),
            worker_id: None,
            token: None,
            command: None,
            to: "receiver".into(),
            mtype: "notify".into(),
            subject: Some("test-anonymous".into()),
            body: "anonymous body".into(),
            in_reply_to: None,
            reply_policy: None,
            target_session: None,
            delivery: "immediate".into(),
        },
    );
    assert!(resp.ok, "anonymous send should succeed: {:?}", resp.error);
    let msg_id = resp.data["msg_id"].as_str().unwrap();
    let st = server_arc.state.lock().unwrap();
    let msg = st.msgs.get(msg_id).expect("message should be stored");
    assert_eq!(msg.from, "unknown", "server must fix from to unknown");
    drop(st);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn test_anonymous_send_rejects_command_envelope_and_reply_obligations() {
    let (server, root) = test_server();
    register(&server, "receiver", "%receiver");
    let server_arc = Arc::new(server);

    // 1. Carrying command envelope
    let resp = dispatch(
        &server_arc,
        Req::Send {
            from: "unknown".into(),
            worker_id: None,
            token: None,
            command: Some(send_command(&root, "fake")),
            to: "receiver".into(),
            mtype: "notify".into(),
            subject: Some("envelope-test".into()),
            body: "should fail".into(),
            in_reply_to: None,
            reply_policy: None,
            target_session: None,
            delivery: "immediate".into(),
        },
    );
    assert!(!resp.ok);
    assert!(resp
        .error
        .as_deref()
        .unwrap()
        .contains("ANONYMOUS_SEND_REJECTED"));

    // 2. Requesting reply obligation
    let resp_reply = dispatch(
        &server_arc,
        Req::Send {
            from: "unknown".into(),
            worker_id: None,
            token: None,
            command: None,
            to: "receiver".into(),
            mtype: "notify".into(),
            subject: Some("reply-test".into()),
            body: "should fail".into(),
            in_reply_to: None,
            reply_policy: Some(crate::proto::ReplyPolicy::Required),
            target_session: None,
            delivery: "immediate".into(),
        },
    );
    assert!(!resp_reply.ok);
    assert!(resp_reply
        .error
        .as_deref()
        .unwrap()
        .contains("ANONYMOUS_SEND_REJECTED"));

    // 3. Partial credentials: only worker_id
    let resp_partial = dispatch(
        &server_arc,
        Req::Send {
            from: "unknown".into(),
            worker_id: Some("worker-a".into()),
            token: None,
            command: None,
            to: "receiver".into(),
            mtype: "notify".into(),
            subject: Some("partial-cred".into()),
            body: "should fail".into(),
            in_reply_to: None,
            reply_policy: None,
            target_session: None,
            delivery: "immediate".into(),
        },
    );
    assert!(!resp_partial.ok);
    assert!(resp_partial
        .error
        .as_deref()
        .unwrap()
        .contains("LEGACY_SEND_REJECTED"));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn test_directory_query_empty_and_populated_without_presence_side_effects() {
    let (server, root) = test_server();
    let server_arc = Arc::new(server);

    // 1. Empty directory
    let resp_empty = dispatch(&server_arc, Req::Directory);
    assert!(resp_empty.ok);
    assert_eq!(resp_empty.data["count"], 0);
    assert_eq!(resp_empty.data["targets"].as_array().unwrap().len(), 0);

    // Verify no presence written
    {
        let st = server_arc.state.lock().unwrap();
        assert!(st.keepalives.is_empty());
    }

    // 2. Register two workers
    register(&server_arc, "alice", "%pane-alice");
    register(&server_arc, "bob", "%pane-bob");

    let resp_populated = dispatch(&server_arc, Req::Directory);
    assert!(resp_populated.ok);
    assert_eq!(resp_populated.data["count"], 2);
    let targets = resp_populated.data["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 2);

    let t0 = &targets[0];
    assert_eq!(t0["worker_id"], "alice");
    assert!(t0["display_name"].is_string());
    assert!(t0.get("token").is_none(), "token must not be exposed");
    assert!(
        t0.get("tasks").is_none(),
        "task details must not be exposed"
    );
    assert!(
        t0.get("role_brief").is_none(),
        "role details must not be exposed"
    );
    assert_eq!(t0["endpoint_live"], serde_json::Value::Null);

    let transport = &t0["transport"];
    assert_eq!(transport["kind"], "tmux");
    assert!(
        transport.get("endpoint").is_none(),
        "endpoint details must not be exposed"
    );
    assert!(
        transport.get("self_check").is_none(),
        "self_check must not be exposed"
    );

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn test_target_session_constraint_enforcement() {
    let (server, root) = test_server();
    register(&server, "worker-target", "%pane-target");

    // Get current session for worker-target
    let target_session_id = {
        let st = server.state.lock().unwrap();
        st.workers
            .get("worker-target")
            .unwrap()
            .transport
            .as_ref()
            .and_then(|t| t.session_id.clone())
            .unwrap_or_else(|| "session-target".to_string())
    };

    // Ensure worker transport has session_id set
    {
        let mut st = server.state.lock().unwrap();
        let w = st.workers.get_mut("worker-target").unwrap();
        if let Some(ref mut t) = w.transport {
            t.session_id = Some(target_session_id.clone());
        }
    }

    let server_arc = Arc::new(server);

    // 1. Mismatched target-session at send time fails immediately
    let resp_mismatch = dispatch(
        &server_arc,
        Req::Send {
            from: "unknown".into(),
            worker_id: None,
            token: None,
            command: None,
            to: "worker-target".into(),
            mtype: "notify".into(),
            subject: Some("session-mismatch-test".into()),
            body: "should fail at binding".into(),
            in_reply_to: None,
            reply_policy: None,
            target_session: Some("different-session-xyz".into()),
            delivery: "immediate".into(),
        },
    );
    assert!(!resp_mismatch.ok);
    assert!(resp_mismatch
        .error
        .as_deref()
        .unwrap()
        .contains("TARGET_SESSION_MISMATCH"));

    // 2. Matching target-session succeeds
    let resp_match = dispatch(
        &server_arc,
        Req::Send {
            from: "unknown".into(),
            worker_id: None,
            token: None,
            command: None,
            to: "worker-target".into(),
            mtype: "notify".into(),
            subject: Some("session-match-test".into()),
            body: "should succeed".into(),
            in_reply_to: None,
            reply_policy: None,
            target_session: Some(target_session_id.clone()),
            delivery: "immediate".into(),
        },
    );
    assert!(
        resp_match.ok,
        "matching target-session should succeed: {:?}",
        resp_match.error
    );
    let msg_id = resp_match.data["msg_id"].as_str().unwrap();

    // Verify TargetSessionBound was recorded in state
    {
        let st = server_arc.state.lock().unwrap();
        assert_eq!(st.target_sessions.get(msg_id), Some(&target_session_id));
    }

    // 3. If recipient's session changes, async delivery attempt fails
    {
        let mut st = server_arc.state.lock().unwrap();
        let w = st.workers.get_mut("worker-target").unwrap();
        if let Some(ref mut t) = w.transport {
            t.session_id = Some("new-changed-session".to_string());
        }
    }

    // Attempt delivery again
    let attempt = attempt_notification_detailed_with_at(
        &server_arc,
        msg_id,
        &default_direct_message_id("worker-target"),
        now_ms(),
    );
    match attempt {
        NotificationAttempt::NotAttempted(err) => {
            assert!(
                err.contains("TARGET_SESSION_MISMATCH"),
                "expected TARGET_SESSION_MISMATCH but got: {err}"
            );
        }
        other => panic!("expected NotAttempted with TARGET_SESSION_MISMATCH but got: {other:?}"),
    }

    // Retention removes the message and its control constraint together.
    {
        let mut state = server_arc.state.lock().unwrap();
        state.drop_message(msg_id);
        assert!(!state.target_sessions.contains_key(msg_id));
    }

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn red_test_cross_project_directory_query_rejected() {
    let (server, root) = test_server();
    register(&server, "alice", "%pane-alice");
    let server_arc = Arc::new(server);

    let foreign_root = std::env::temp_dir().join(format!("foreign-proj-b-{}", std::process::id()));
    std::fs::create_dir_all(foreign_root.join(".agent-collab")).unwrap();
    let foreign_canonical = std::fs::canonicalize(&foreign_root).unwrap();

    let foreign_context = crate::proto::ProjectContext::for_registered_root_with_app(
        &foreign_canonical,
        crate::identity::AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
    )
    .unwrap();

    // Querying server A with Project B's context must be rejected
    let req = Req::Directory;
    let validation = validate_request_context(&server_arc, &req, Some(&foreign_context));
    assert!(
        validation.is_err(),
        "cross-project directory query must be rejected but got Ok: {:?}",
        validation
    );

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(foreign_root).ok();
}

#[test]
fn red_test_target_session_queued_batch_and_poll_recv_isolation() {
    let (server, root) = test_server();
    register(&server, "worker-target", "%pane-target");

    // Initially worker is on session-1
    {
        let mut st = server.state.lock().unwrap();
        let w = st.workers.get_mut("worker-target").unwrap();
        if let Some(ref mut t) = w.transport {
            t.session_id = Some("session-1".to_string());
        }
    }
    let server_arc = Arc::new(server);

    // 1. Send queued message with --target-session session-1
    let resp_msg1 = dispatch(
        &server_arc,
        Req::Send {
            from: "unknown".into(),
            worker_id: None,
            token: None,
            command: None,
            to: "worker-target".into(),
            mtype: "notify".into(),
            subject: Some("target-msg-1".into()),
            body: "content-pinned-to-session-1".into(),
            in_reply_to: None,
            reply_policy: None,
            target_session: Some("session-1".into()),
            delivery: "queued".into(),
        },
    );
    assert!(resp_msg1.ok);

    // 2. Worker switches session to session-2
    {
        let mut st = server_arc.state.lock().unwrap();
        let w = st.workers.get_mut("worker-target").unwrap();
        if let Some(ref mut t) = w.transport {
            t.session_id = Some("session-2".to_string());
        }
    }

    // 3. Send message 2 without target session
    let resp_msg2 = dispatch(
        &server_arc,
        Req::Send {
            from: "unknown".into(),
            worker_id: None,
            token: None,
            command: None,
            to: "worker-target".into(),
            mtype: "notify".into(),
            subject: Some("normal-msg-2".into()),
            body: "content-normal-for-session-2".into(),
            in_reply_to: None,
            reply_policy: None,
            target_session: None,
            delivery: "queued".into(),
        },
    );
    assert!(resp_msg2.ok);

    // 4. In session-2, check inbox_of("worker-target"): it must NOT contain msg1!
    {
        let st = server_arc.state.lock().unwrap();
        let unread = st.inbox_of("worker-target");
        let bodies: Vec<&str> = unread.iter().map(|m| m.body.as_str()).collect();
        assert!(
            !bodies.contains(&"content-pinned-to-session-1"),
            "new session must not receive old session's target-session message in inbox_of: {:?}",
            bodies
        );
        assert!(
            bodies.contains(&"content-normal-for-session-2"),
            "new session must receive non-fixed message in inbox_of: {:?}",
            bodies
        );
    }

    let msg1_id = resp_msg1.data["msg_id"].as_str().unwrap();

    // 5. Attempt notification for msg1 in session-2: it must fail with TARGET_SESSION_MISMATCH
    let attempt = attempt_notification_detailed_with_at(
        &server_arc,
        msg1_id,
        &default_direct_message_id("worker-target"),
        now_ms(),
    );
    match attempt {
        NotificationAttempt::NotAttempted(err) => {
            assert!(
                err.contains("TARGET_SESSION_MISMATCH"),
                "expected TARGET_SESSION_MISMATCH, got: {err}"
            );
        }
        other => panic!("expected NotAttempted with TARGET_SESSION_MISMATCH, got: {other:?}"),
    }

    // 6. Check Req::MsgStatus for msg1: target_session and failure must be visible
    let status_resp = dispatch(
        &server_arc,
        Req::MsgStatus {
            msg_id: msg1_id.to_string(),
        },
    );
    assert!(status_resp.ok);
    assert_eq!(
        status_resp.data["target_session"], "session-1",
        "target_session must be projected in MsgStatus"
    );
    let failure = &status_resp.data["notification_delivery_failure"];
    assert!(
        !failure.is_null(),
        "notification_delivery_failure must be recorded"
    );
    assert_eq!(
        failure["operation"], "notification.target_session_mismatch",
        "operation must be target_session_mismatch"
    );
    assert_eq!(failure["retryable"], false, "failure must not be retryable");

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn test_anonymous_send_keeps_mixed_target_routes_and_directory_namespaces() {
    let (mut server, root) = test_server();
    let cwd = server.root.display().to_string();
    let app = AppServerId::new("tui-default").unwrap();

    let tmux_trans = SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some("/tmp/fake-tmux.sock".into()),
        namespace: Some("$1001".into()),
        session_id: Some("session-tmux".into()),
        thread_id: Some("thread-tmux".into()),
        tmux_endpoint: Some(crate::proto::TmuxEndpoint {
            socket_path: "/tmp/fake-tmux.sock".into(),
            server_pid: 1,
            tmux_session_id: "$1001".into(),
            pane_id: "%1001".into(),
            pane_pid: 2,
            codex_session_id: Some("session-tmux".into()),
            codex_thread_id: Some("thread-tmux".into()),
        }),
        capabilities: vec!["send_message_to_pane".into()],
        self_check: "fixture".into(),
    };
    let dsh_trans = SelectedTransport {
        kind: TransportKind::Dsh,
        endpoint: Some("/tmp/fake-dsh.sock".into()),
        namespace: Some("dsh-namespace".into()),
        session_id: Some("session-dsh".into()),
        thread_id: Some("thread-dsh".into()),
        tmux_endpoint: None,
        capabilities: vec!["dsh_channel".into()],
        self_check: "fixture".into(),
    };
    let desktop_trans = SelectedTransport {
        kind: TransportKind::DesktopBridge,
        endpoint: Some("/tmp/fake-desktop.sock".into()),
        namespace: Some("desktop-namespace".into()),
        session_id: Some("session-desktop".into()),
        thread_id: Some("thread-desktop".into()),
        tmux_endpoint: None,
        capabilities: vec!["desktop_bridge".into()],
        self_check: "fixture".into(),
    };

    seed_committed_owner(
        &server,
        "worker-tmux",
        "token-tmux",
        &tmux_trans,
        &cwd,
        Some(app.clone()),
    );
    seed_committed_owner(
        &server,
        "worker-dsh",
        "token-dsh",
        &dsh_trans,
        &cwd,
        Some(app.clone()),
    );
    seed_committed_owner(
        &server,
        "worker-desktop",
        "token-desktop",
        &desktop_trans,
        &cwd,
        Some(app.clone()),
    );

    let delivered = Arc::new(Mutex::new(Vec::new()));
    let delivered_clone = Arc::clone(&delivered);
    server.appserver_notification_sink = Arc::new(
        move |transport, _source_thread, text, msg_id, _explicit, _mode| {
            delivered_clone.lock().unwrap().push((
                transport.kind.clone(),
                msg_id.to_string(),
                text.to_string(),
            ));
            Ok(json!({"delivered": true}))
        },
    );

    let server_arc = Arc::new(server);

    // 1. Verify directory lists all 3 with their respective transports and namespaces
    let dir_resp = dispatch(&server_arc, Req::Directory);
    assert!(dir_resp.ok);
    assert_eq!(dir_resp.data["count"], 3);
    let targets = dir_resp.data["targets"].as_array().unwrap();
    let tmux_entry = targets
        .iter()
        .find(|t| t["worker_id"] == "worker-tmux")
        .unwrap();
    assert_eq!(tmux_entry["transport"]["kind"], "tmux");
    let dsh_entry = targets
        .iter()
        .find(|t| t["worker_id"] == "worker-dsh")
        .unwrap();
    assert_eq!(dsh_entry["transport"]["kind"], "dsh");
    assert_eq!(dsh_entry["transport"]["namespace"], "dsh-namespace");
    let desktop_entry = targets
        .iter()
        .find(|t| t["worker_id"] == "worker-desktop")
        .unwrap();
    assert_eq!(desktop_entry["transport"]["kind"], "desktop_bridge");
    assert_eq!(desktop_entry["transport"]["namespace"], "desktop-namespace");

    // 2. Anonymous send to each adaptor
    for to_worker in ["worker-tmux", "worker-dsh", "worker-desktop"] {
        let send_resp = dispatch(
            &server_arc,
            Req::Send {
                from: "unknown".into(),
                worker_id: None,
                token: None,
                command: None,
                to: to_worker.into(),
                mtype: "notify".into(),
                subject: Some(format!("msg-for-{to_worker}")),
                body: format!("hello-{to_worker}"),
                in_reply_to: None,
                reply_policy: None,
                target_session: None,
                delivery: "immediate".into(),
            },
        );
        assert!(
            send_resp.ok,
            "send to {to_worker} failed: {:?}",
            send_resp.error
        );
        let msg_id = send_resp.data["msg_id"].as_str().unwrap();

        // Verify message stored with from = unknown
        let st = server_arc.state.lock().unwrap();
        let m = st.msgs.get(msg_id).unwrap();
        assert_eq!(m.from, "unknown");
        assert_eq!(m.to, to_worker);
    }

    // 3. Verify the selected sink receives each target's transport. This unit
    // seam checks mixed routes, not external adaptor I/O; public CLI fixtures
    // cover real native and gateway consumers separately.
    let deliveries = delivered.lock().unwrap().clone();
    assert_eq!(deliveries.len(), 3, "expected 3 deliveries: {deliveries:?}");
    assert!(
        deliveries
            .iter()
            .any(|d| d.0 == TransportKind::Tmux && d.2.contains("msg-for-worker-tmux")),
        "tmux delivery missing: {deliveries:?}"
    );
    assert!(
        deliveries
            .iter()
            .any(|d| d.0 == TransportKind::Dsh && d.2.contains("msg-for-worker-dsh")),
        "dsh delivery missing: {deliveries:?}"
    );
    assert!(
        deliveries
            .iter()
            .any(|d| d.0 == TransportKind::DesktopBridge && d.2.contains("msg-for-worker-desktop")),
        "desktop delivery missing: {deliveries:?}"
    );

    // 4. Verify no peer or tasks created for "unknown"
    {
        let st = server_arc.state.lock().unwrap();
        assert!(!st.workers.contains_key("unknown"));
        assert!(!st.tasks.values().any(|t| t.owner == "unknown"));
    }

    std::fs::remove_dir_all(root).ok();
}
