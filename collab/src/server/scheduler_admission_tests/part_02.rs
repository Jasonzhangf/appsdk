    #[test]
    fn scheduler_dispatch_concurrent_same_request_reserves_once() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        server.config.notifications.enabled = true;
        promote_master(&server);
        let server = Arc::new(server);
        let barrier = Arc::new(Barrier::new(2));
        let request = |server: Arc<Server>, barrier: Arc<Barrier>| {
            thread::spawn(move || {
                barrier.wait();
                dispatch(
                    &server,
                    Req::Subagent {
                        worker_id: "master".into(),
                        token: "token-master".into(),
                        command: crate::subagent::Action::Dispatch {
                            request_id: "req-concurrent-1".into(),
                            subject: "Concurrent task".into(),
                            body: "Reserve exactly once".into(),
                            feature_id: None,
                            worktree_path: None,
                            branch: None,
                            base_commit: None,
                            priority: "p2".into(),
                            next_step: None,
                        },
                        launch_env: Default::default(),
                    },
                )
            })
        };
        let first = request(Arc::clone(&server), Arc::clone(&barrier));
        let second = request(Arc::clone(&server), Arc::clone(&barrier));
        let first = first.join().unwrap();
        let second = second.join().unwrap();
        let completed = [&first, &second].into_iter()
            .filter(|resp| resp.ok && resp.data["decision"] == "reuse-idle-managed-subagent")
            .count();
        assert_eq!(
            completed, 1,
            "one request must complete the assignment: {first:?} {second:?}"
        );
        let repeated = [&first, &second].into_iter()
            .find(|resp| !resp.ok || resp.data["decision"] == "deduplicated")
            .expect("one request must observe the in-flight or completed claim");
        if repeated.ok {
            assert_eq!(repeated.data["deduplicated"], true);
        } else {
            assert!(repeated.error.as_deref().is_some_and(|error| error.contains("already in flight")), "{repeated:?}");
            assert_eq!(repeated.data["reservation"], true);
        }
        assert_eq!(first.data["task_id"], second.data["task_id"]);
        assert_eq!(first.data["message_id"], second.data["message_id"]);
        assert!(["reuse-idle-managed-subagent", "deduplicated"]
            .contains(&first.data["decision"].as_str().unwrap()));
        assert!(["reuse-idle-managed-subagent", "deduplicated"]
            .contains(&second.data["decision"].as_str().unwrap()));
        let state = server.state.lock().unwrap();
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.msgs.len(), 1);
        assert_eq!(
            state.tasks["task-scheduler-req-concurrent-1"].status,
            "assigned"
        );
        assert_eq!(
            state.scheduler_admissions["req-concurrent-1"].status,
            "succeeded"
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_concurrent_retry_notifies_once() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        server.config.notifications.enabled = true;
        promote_master(&server);
        let sink_calls = Arc::new(AtomicUsize::new(0));
        let sink_calls_for_sink = Arc::clone(&sink_calls);
        let sink_gate = Arc::new((Mutex::new(0usize), Condvar::new()));
        let sink_gate_for_sink = Arc::clone(&sink_gate);
        server.appserver_notification_sink = Arc::new(move |_, _, _, _, _, _| {
            let call = sink_calls_for_sink.fetch_add(1, Ordering::SeqCst) + 1;
            let (lock, ready) = &*sink_gate_for_sink;
            let mut entered = lock.lock().unwrap();
            *entered = (*entered).max(call);
            if call == 1 {
                let _ = ready
                    .wait_timeout_while(entered, Duration::from_millis(500), |seen| *seen < 2)
                    .unwrap();
            } else {
                ready.notify_all();
            }
            Ok(json!({"accepted": true}))
        });
        let server = Arc::new(server);
        let barrier = Arc::new(Barrier::new(2));
        let request = |server: Arc<Server>, barrier: Arc<Barrier>| {
            thread::spawn(move || {
                barrier.wait();
                dispatch(
                    &server,
                    Req::Subagent {
                        worker_id: "master".into(),
                        token: "token-master".into(),
                        command: crate::subagent::Action::Dispatch {
                            request_id: "req-concurrent-notify-1".into(),
                            subject: "Concurrent notification".into(),
                            body: "Notify exactly once".into(),
                            feature_id: None,
                            worktree_path: None,
                            branch: None,
                            base_commit: None,
                            priority: "p1".into(),
                            next_step: None,
                        },
                        launch_env: Default::default(),
                    },
                )
            })
        };
        let first = request(Arc::clone(&server), Arc::clone(&barrier));
        let second = request(Arc::clone(&server), Arc::clone(&barrier));
        let first = first.join().unwrap();
        let second = second.join().unwrap();

        assert_eq!(
            sink_calls.load(Ordering::SeqCst),
            1,
            "concurrent retry must trigger exactly one AppServer notification sink call"
        );
        let completed = [&first, &second].into_iter().filter(|resp| resp.ok).count();
        assert_eq!(
            completed, 1,
            "one request must own the notification: {first:?} {second:?}"
        );
        let in_flight = [&first, &second]
            .into_iter()
            .find(|resp| !resp.ok)
            .expect("one request must observe the in-flight claim");
        assert!(
            in_flight
                .error
                .as_deref()
                .is_some_and(|error| error.contains("already in flight")),
            "{in_flight:?}"
        );
        assert_eq!(in_flight.data["reservation"], true);
        assert_eq!(first.data["task_id"], second.data["task_id"]);
        assert_eq!(first.data["message_id"], second.data["message_id"]);
        let state = server.state.lock().unwrap();
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.msgs.len(), 1);
        assert_eq!(
            state.scheduler_admissions["req-concurrent-notify-1"].status,
            "succeeded"
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_rejection_clears_claim_without_deadlock() {
        let (server, root) = test_server();
        register(&server, "peer", "%peer");
        server.commit(&[Event::TaskCreated {
            task: TaskRec {
                id: "task-scheduler-req-rejection-deadlock".into(),
                owner: "peer".into(),
                created_by: "master".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p1".into(),
                status: "assigned".into(),
                next_step: None,
                wait: None,
                created_ms: now_ms(),
                updated_ms: now_ms(),
            },
        }]);
        server.commit(&[Event::SchedulerAdmission {
            admission: crate::server::state::SchedulerAdmissionRecord {
                request_id: "req-rejection-deadlock".into(),
                decision: "use-registered-peer".into(),
                worker_id: "peer".into(),
                managed_subagent_id: None,
                message_id: "scheduler-req-rejection-deadlock".into(),
                task_id: "task-scheduler-req-rejection-deadlock".into(),
                status: "pending".into(),
                error: None,
                created_ms: now_ms(),
                updated_ms: now_ms(),
            },
        }]);
        let response = scheduler_dispatch_recover_pending(&server, "req-rejection-deadlock")
            .expect("pending reservation must be recovered");
        assert!(!response.ok, "{response:?}");
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.scheduler_admissions["req-rejection-deadlock"].status,
            "pending"
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_dispatch_without_subscription_reports_repair_terminal() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        server.config.notifications.enabled = true;
        promote_master(&server);
        let server = Arc::new(server);
        // Remove the recipient's default lease so dispatch cannot select a sink.
        let subscription_id = server
            .state
            .lock()
            .unwrap()
            .notification_subscriptions
            .values()
            .find(|subscription| subscription.worker_id == "peer")
            .unwrap()
            .id
            .clone();
        server.commit(&[Event::NotificationSuppressed {
            subscription_id,
            status: "cancelled".into(),
            reason: EXPLICIT_UNSUBSCRIBE_REASON.into(),
            updated_ms: now_ms(),
        }]);
        let response = dispatch(
            &server,
            Req::Subagent {
                worker_id: "master".into(),
                token: "token-master".into(),
                command: crate::subagent::Action::Dispatch {
                    request_id: "req-no-subscription".into(),
                    subject: "No subscription".into(),
                    body: "must surface repair".into(),
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
        assert_eq!(
            response.data["notification"], "mailbox-only-no-subscription",
            "{response:?}"
        );
        assert_eq!(response.data["failure"], "notification_subscription_missing");
        assert_eq!(response.data["repair_required"], true);
        assert!(
            response.data["notification_error"].as_str().is_some(),
            "mailbox-only must carry a machine-readable reason: {response:?}"
        );
        assert!(
            response.data["escalation"].as_str().is_some(),
            "mailbox-only must carry the repair escalation: {response:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_notifying_claim_recovers_after_cooldown() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        server.config.notifications.enabled = true;
        promote_master(&server);
        let server = Arc::new(server);
        let first = dispatch(
            &server,
            Req::Subagent {
                worker_id: "master".into(),
                token: "token-master".into(),
                command: crate::subagent::Action::Dispatch {
                    request_id: "req-stale-notifying".into(),
                    subject: "Recover stale claim".into(),
                    body: "Retry after cooldown".into(),
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
        {
            let mut state = server.state.lock().unwrap();
            let stale_ms = now_ms() - state::REQUEST_COOLDOWN_MS;
            server
                .commit_locked(
                    &mut state,
                    &[Event::SchedulerAdmissionStatus {
                        request_id: "req-stale-notifying".into(),
                        status: "notifying".into(),
                        error: None,
                        updated_ms: stale_ms,
                    }],
                )
                .expect("commit stale notifying admission");
        }
        let retry = dispatch(
            &server,
            Req::Subagent {
                worker_id: "master".into(),
                token: "token-master".into(),
                command: crate::subagent::Action::Dispatch {
                    request_id: "req-stale-notifying".into(),
                    subject: "Recover stale claim".into(),
                    body: "Retry after cooldown".into(),
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
        assert!(retry.ok, "{retry:?}");
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.scheduler_admissions["req-stale-notifying"].status,
            "succeeded"
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_unknown_notification_claim_is_not_resent_after_cooldown() {
        let (mut server, root) = test_server();
        register(&server, "master", "%master");
        register_private_dispatch_peer(&server);
        server.config.notifications.enabled = true;
        promote_master(&server);
        let sink_calls = Arc::new(AtomicU64::new(0));
        let sink_calls_for_sink = Arc::clone(&sink_calls);
        server.appserver_notification_sink = Arc::new(move |_, _, _, _, _, _| {
            sink_calls_for_sink.fetch_add(1, Ordering::SeqCst);
            Err("ADAPTER_TIMEOUT: turn/start timed out".into())
        });
        let server = Arc::new(server);
        let request = || {
            dispatch(
                &server,
                Req::Subagent {
                    worker_id: "master".into(),
                    token: "token-master".into(),
                    command: crate::subagent::Action::Dispatch {
                        request_id: "req-stale-unknown".into(),
                        subject: "Unknown outcome".into(),
                        body: "Must not be resent".into(),
                        feature_id: None,
                        worktree_path: None,
                        branch: None,
                        base_commit: None,
                        priority: "p1".into(),
                        next_step: None,
                    },
                    launch_env: Default::default(),
                },
            )
        };
        let first = request();
        assert!(!first.ok, "{first:?}");
        let message_id = first.data["message_id"].as_str().unwrap().to_owned();
        {
            let mut state = server.state.lock().unwrap();
            let stale_ms = now_ms() - state::REQUEST_COOLDOWN_MS;
            server
                .commit_locked(
                    &mut state,
                    &[Event::SchedulerAdmissionStatus {
                        request_id: "req-stale-unknown".into(),
                        status: "notifying".into(),
                        error: None,
                        updated_ms: stale_ms,
                    }],
                )
                .expect("commit stale unknown admission");
        }

        let retry = request();
        assert!(!retry.ok, "{retry:?}");
        assert_eq!(sink_calls.load(Ordering::SeqCst), 1);
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
        assert!(!state.msgs[&message_id].retry_attempted);
        assert!(!state.notification_delivery_failures[&message_id].retryable);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
