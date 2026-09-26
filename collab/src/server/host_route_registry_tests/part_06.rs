    #[tokio::test]
    async fn wire_poll_rechecks_generation_before_delivering_after_rebind() {
        let (server, root, _) = test_server();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let worker_id = "poll-recover-worker";
        let token = "token-poll-recover-worker";
        let registered = dispatch_wire(
            server.clone(),
            Some(context_with_app(&root, app)),
            Req::Register {
                worker_id: worker_id.into(),
                token: token.into(),
                cwd: root.display().to_string(),
                candidates: test_candidates(&format!("thread-{worker_id}")),
            },
        )
        .await;
        assert!(registered.ok, "{registered:?}");
        let old_runtime = runtime_for_registered(&server, &root, worker_id, app);
        let old_context = context_with_runtime(&root, app, &old_runtime);
        let provisional = RuntimeIdentity::cli_adapter(worker_id).unwrap();
        let recovery_context = context_with_runtime(&root, app, &provisional);
        let poll_server = server.clone();
        let rebind_server = server.clone();
        let rebind_root = root.clone();
        let poll = dispatch_wire(
            poll_server,
            Some(old_context),
            Req::Poll {
                worker_id: worker_id.into(),
                token: token.into(),
                timeout_ms: 1_000,
                receive_id: None,
            },
        );
        let rebind = async move {
            let response = dispatch_wire(
                rebind_server.clone(),
                Some(recovery_context),
                Req::Register {
                    worker_id: worker_id.into(),
                    token: token.into(),
                    cwd: rebind_root.display().to_string(),
                    candidates: test_candidates(&format!("thread-{worker_id}")),
                },
            )
            .await;
            assert!(response.ok, "{response:?}");
            rebind_server.commit(&[Event::Sent {
                msg: Message {
                    id: "poll-rebind-message".into(),
                    from: "sender".into(),
                    to: worker_id.into(),
                    mtype: "notify".into(),
                    subject: Some("rebind".into()),
                    body: "must remain pending".into(),
                    in_reply_to: None,
                    created_ms: now_ms(),
                    state: "pending".into(),
                    wake_attempt_count: 0,
                    last_wake_attempt_ms: 0,
                    retry_attempted: false,
                },
            }]);
            response
        };
        let (poll_result, rebind_result) = tokio::join!(poll, rebind);
        assert!(!poll_result.ok, "{poll_result:?}");
        assert!(poll_result
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("SESSION_THREAD_BINDING_MISMATCH:")));
        assert!(rebind_result.ok, "{rebind_result:?}");
        let state = server.state.lock().unwrap();
        let message = state.msgs.get("poll-rebind-message").unwrap();
        assert_eq!(message.state, "pending");
        assert_eq!(message.wake_attempt_count, 0);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
