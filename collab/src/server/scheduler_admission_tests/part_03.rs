    #[test]
    fn stale_attempt_cannot_clear_newer_notification_claim() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register(&server, "peer", "%peer");
        server.config.notifications.enabled = true;
        promote_master(&server);
        let server = Arc::new(server);
        let first = dispatch(
            &server,
            Req::Subagent {
                worker_id: "master".into(),
                token: "token-master".into(),
                command: crate::subagent::Action::Dispatch {
                    request_id: "req-stale-owner".into(),
                    subject: "Stale owner".into(),
                    body: "Old attempt must not clear a newer claim".into(),
                    feature_id: None,
                    worktree_path: None,
                    branch: None,
                    base_commit: None,
                    priority: "p1".into(),
                    next_step: None,
                },
                launch_env: Default::default(),
            },
        );
        assert!(first.ok, "{first:?}");

        let old_claim = now_ms() - state::REQUEST_COOLDOWN_MS - 1;
        {
            let mut state = server.state.lock().unwrap();
            server.commit_locked(
                &mut state,
                &[Event::SchedulerAdmissionStatus {
                    request_id: "req-stale-owner".into(),
                    status: "notifying".into(),
                    error: None,
                    updated_ms: old_claim,
                }],
            );
        }
        {
            let mut state = server.state.lock().unwrap();
            server.commit_locked(
                &mut state,
                &[Event::SchedulerAdmissionStatus {
                    request_id: "req-stale-owner".into(),
                    status: "notifying".into(),
                    error: None,
                    updated_ms: old_claim + 1,
                }],
            );
        }
        clear_scheduler_notification_claim(&server, "req-stale-owner", old_claim);
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.scheduler_admissions["req-stale-owner"].status,
            "notifying"
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
