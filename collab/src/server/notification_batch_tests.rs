    use super::*;
    use crate::server::state::{Event, NotificationSubscription, State, WorkerRec};
    use std::sync::{Arc, Mutex};

    fn test_server() -> (Arc<Server>, std::path::PathBuf) {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "collab-notification-batch-{}-{sequence}",
            std::process::id()
        ));
        let server_dir = root.join(".agent-collab/server");
        std::fs::create_dir_all(&server_dir).unwrap();
        let journal = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(server_dir.join("journal.jsonl"))
            .unwrap();
        (
            Arc::new(Server {
                config: crate::config::Config::default(),
                root: root.clone(),
                storage_root: root.clone(),
                journal_path: root.join(".agent-collab/server/journal.jsonl"),
                host_paths: HostPaths::for_state_root(root.join("host-state")).unwrap(),
                state: Mutex::new(State::default()),
                journal: Mutex::new(journal),
                appserver_candidate_check: Arc::new(|candidate| {
                    Ok(SelectedTransport {
                        kind: TransportKind::AppServer,
                        endpoint: Some(candidate.endpoint.clone()),
                        namespace: Some(candidate.namespace.clone()),
                        session_id: Some(candidate.session_id.clone()),
                        thread_id: Some(candidate.thread_id.clone()),
                        tmux_endpoint: None,
                        capabilities: vec!["send_message_to_thread".into()],
                        self_check: "test appserver".into(),
                    })
                }),
                appserver_notification_sink: Arc::new(|_, _, _, _, _, _| {
                    Ok(serde_json::json!({"accepted": true}))
                }),
                appserver_thread_status: Arc::new(|_, thread_id| {
                    Ok(serde_json::json!({
                        "thread": {
                            "id": thread_id,
                            "status": {"type": "idle"},
                            "canAcceptDirectInput": true
                        }
                    }))
                }),
                appserver_thread_archive: Arc::new(|_, _| {
                    Ok(serde_json::json!({"archived": true}))
                }),
                mailbox_notify: tokio::sync::Notify::new(),
            }),
            root,
        )
    }

    fn register_and_subscribe(server: &Server, worker_id: &str) -> String {
        let now = now_ms();
        let subscription_id = format!("sub-{worker_id}");
        server.commit(&[
            Event::Registered {
                worker: WorkerRec {
                    id: worker_id.into(),
                    token: format!("token-{worker_id}"),
                    cwd: "/tmp".into(),
                    registered_ms: now,
                    transport: Some(SelectedTransport {
                        kind: TransportKind::AppServer,
                        endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
                        namespace: Some("codex_tui".into()),
                        session_id: Some(format!("session-{worker_id}")),
                        thread_id: Some(format!("thread-{worker_id}")),
                        tmux_endpoint: None,
                        capabilities: vec!["send_message_to_thread".into()],
                        self_check: "test appserver".into(),
                    }),
                },
            },
            Event::NotificationSubscribed {
                subscription: NotificationSubscription {
                    id: subscription_id.clone(),
                    worker_id: worker_id.into(),
                    event: "direct-message".into(),
                    subject: None,
                    target: format!("thread-{worker_id}"),
                    method: "appserver".into(),
                    trigger_ms: None,
                    trigger_times_ms: Vec::new(),
                    interval_ms: None,
                    repeat_count: 1,
                    fired_count: 0,
                    expires_ms: now + 300_000,
                    status: "armed".into(),
                    created_ms: now,
                    updated_ms: now,
                    status_reason: None,
                },
            },
        ]);
        subscription_id
    }

    fn queue_message(
        server: &Server,
        worker_id: &str,
        subscription_id: &str,
        message_id: &str,
        created_ms: i64,
    ) {
        server.commit(&[
            Event::Sent {
                msg: Message {
                    id: message_id.into(),
                    from: "peer".into(),
                    to: worker_id.into(),
                    mtype: "notify".into(),
                    subject: Some(format!("topic-{message_id}")),
                    body: format!("DETAIL-{message_id}"),
                    in_reply_to: None,
                    created_ms,
                    state: "pending".into(),
                    wake_attempt_count: 0,
                    last_wake_attempt_ms: 0,
                    retry_attempted: false,
                },
            },
            Event::WakeBound {
                message_id: message_id.into(),
                subscription_id: subscription_id.into(),
            },
        ]);
    }

    #[test]
    fn explicit_notification_uses_sender_native_thread_for_delegation() {
        let (mut server, root) = test_server();
        let subscription_id = register_and_subscribe(&server, "recipient");
        register_and_subscribe(&server, "sender");
        let now = now_ms();
        server.commit(&[
            Event::Sent {
                msg: Message {
                    id: "sender-attribution".into(),
                    from: "sender".into(),
                    to: "recipient".into(),
                    mtype: "notify".into(),
                    subject: Some("topic".into()),
                    body: "DETAIL".into(),
                    in_reply_to: None,
                    created_ms: now,
                    state: "pending".into(),
                    wake_attempt_count: 0,
                    last_wake_attempt_ms: 0,
                    retry_attempted: false,
                },
            },
            Event::WakeBound {
                message_id: "sender-attribution".into(),
                subscription_id: subscription_id.clone(),
            },
            Event::DeliveryMode {
                msg_id: "sender-attribution".into(),
                mode: "explicit-notification".into(),
                source_thread_id: None,
            },
        ]);

        let observed = Arc::new(Mutex::new(None));
        {
            let observed = Arc::clone(&observed);
            Arc::get_mut(&mut server)
                .expect("unique test server")
                .appserver_notification_sink = Arc::new(move |_, source, _, _, explicit, _mode| {
                *observed.lock().unwrap() = Some((source.map(str::to_owned), explicit));
                Ok(json!({"accepted": true}))
            });
        }

        assert!(attempt_notification_with_at(
            &server,
            "sender-attribution",
            &subscription_id,
            now,
        ));
        assert_eq!(
            observed.lock().unwrap().as_ref(),
            Some(&(Some("thread-sender".into()), true))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn automatic_notification_uses_immediate_without_sender_thread() {
        let (mut server, root) = test_server();
        let subscription_id = register_and_subscribe(&server, "recipient");
        let now = now_ms();
        queue_message(
            &server,
            "recipient",
            &subscription_id,
            "automatic-notice",
            now - 120_001,
        );
        let observed = Arc::new(Mutex::new(None));
        {
            let observed = Arc::clone(&observed);
            Arc::get_mut(&mut server)
                .expect("unique test server")
                .appserver_notification_sink = Arc::new(move |_, source, _, _, explicit, _mode| {
                *observed.lock().unwrap() = Some((source.map(str::to_owned), explicit));
                Ok(json!({"accepted": true}))
            });
        }

        assert!(attempt_notification_with_at(
            &server,
            "automatic-notice",
            &subscription_id,
            now,
        ));
        assert_eq!(observed.lock().unwrap().as_ref(), Some(&(None, false)));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn queued_delivery_mode_reaches_notification_sink() {
        let (mut server, root) = test_server();
        let subscription_id = register_and_subscribe(&server, "recipient");
        let now = now_ms();
        queue_message(
            &server,
            "recipient",
            &subscription_id,
            "queued-notice",
            now - 120_001,
        );
        server.commit(&[Event::DeliveryMode {
            msg_id: "queued-notice".into(),
            mode: "queued".into(),
            source_thread_id: None,
        }]);
        let observed = Arc::new(Mutex::new(None));
        {
            let observed = Arc::clone(&observed);
            Arc::get_mut(&mut server)
                .expect("unique test server")
                .appserver_notification_sink = Arc::new(move |_, _, _, _, _, mode| {
                *observed.lock().unwrap() = Some(mode.to_string());
                Ok(json!({"accepted": true}))
            });
        }

        assert!(attempt_notification_with_at(
            &server,
            "queued-notice",
            &subscription_id,
            now,
        ));
        assert_eq!(observed.lock().unwrap().as_deref(), Some("queued"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn immediate_and_queued_messages_are_not_batched_into_one_mode() {
        let (mut server, root) = test_server();
        let subscription_id = register_and_subscribe(&server, "recipient");
        let now = now_ms();
        queue_message(
            &server,
            "recipient",
            &subscription_id,
            "immediate-notice",
            now - 120_001,
        );
        queue_message(
            &server,
            "recipient",
            &subscription_id,
            "queued-notice",
            now - 120_001,
        );
        server.commit(&[
            Event::DeliveryMode {
                msg_id: "immediate-notice".into(),
                mode: "immediate".into(),
                source_thread_id: None,
            },
            Event::DeliveryMode {
                msg_id: "queued-notice".into(),
                mode: "queued".into(),
                source_thread_id: None,
            },
        ]);

        let observed = Arc::new(Mutex::new(None));
        {
            let observed = Arc::clone(&observed);
            Arc::get_mut(&mut server)
                .expect("unique test server")
                .appserver_notification_sink = Arc::new(move |_, _, _, _, _, mode| {
                *observed.lock().unwrap() = Some(mode.to_string());
                Ok(json!({"accepted": true}))
            });
        }

        assert!(attempt_notification_with_at(
            &server,
            "immediate-notice",
            &subscription_id,
            now,
        ));
        assert_eq!(observed.lock().unwrap().as_deref(), Some("immediate"));
        {
            let state = server.state.lock().unwrap();
            assert_eq!(state.msgs["immediate-notice"].wake_attempt_count, 1);
            assert_eq!(state.msgs["queued-notice"].wake_attempt_count, 0);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_appserver_sink_does_not_fallback_to_tmux() {
        let transport = SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-thread-recipient".into()),
            thread_id: Some("thread-recipient".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message_to_thread".into()],
            self_check: "test appserver".into(),
        };
        let sink = default_appserver_notification_sink();
        let error = sink(
            &transport,
            None,
            "explicit",
            "message-explicit",
            true,
            "immediate",
        )
        .unwrap_err();
        assert!(
            error.contains("ADAPTER_ROUTE_UNAVAILABLE:")
                || error.contains("APPSERVER_ENDPOINT_REJECTED:")
                || error.contains("RPC")
        );
    }

    #[test]
    fn automatic_batch_does_not_cross_the_first_notice_window() {
        let (mut server, root) = test_server();
        let subscription_id = register_and_subscribe(&server, "recipient");
        let now = now_ms();
        queue_message(
            &server,
            "recipient",
            &subscription_id,
            "old-notice",
            now - 120_001,
        );
        queue_message(&server, "recipient", &subscription_id, "late-notice", now);

        let delivered = Arc::new(Mutex::new(Vec::new()));
        {
            let delivered = Arc::clone(&delivered);
            Arc::get_mut(&mut server)
                .expect("unique test server")
                .appserver_notification_sink = Arc::new(move |_, _, text, _, _, _mode| {
                delivered.lock().unwrap().push(text.to_string());
                Ok(serde_json::json!({"accepted": true}))
            });
        }
        assert!(attempt_notification_with_at(
            &server,
            "old-notice",
            &subscription_id,
            now,
        ));

        let text = delivered.lock().unwrap().join("\n");
        assert!(text.contains("old-notice"));
        assert!(
            !text.contains("late-notice"),
            "a notice arriving after the first 120-second window must remain pending"
        );
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs["old-notice"].state, "pending");
        assert_eq!(state.msgs["old-notice"].wake_attempt_count, 1);
        assert_eq!(state.msgs["late-notice"].state, "pending");
        drop(state);
        let mailbox =
            std::fs::read_to_string(root.join(".agent-collab/mailbox/recipient-recipient.jsonl"))
                .unwrap();
        assert!(mailbox.contains("DETAIL-old-notice"));
        assert!(mailbox.contains("DETAIL-late-notice"));
        assert!(
            !text.contains("DETAIL-old-notice") && !text.contains("DETAIL-late-notice"),
            "batch wake must carry task summary while full details stay in JSONL"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn four_notice_batch_uses_the_original_window_start_after_capping() {
        let (mut server, root) = test_server();
        let subscription_id = register_and_subscribe(&server, "recipient");
        let window_start = 10_000_000;
        for (index, offset) in [(0, 0), (1, 60_000), (2, 70_000), (3, 80_000)] {
            queue_message(
                &server,
                "recipient",
                &subscription_id,
                &format!("backlog-{index}"),
                window_start + offset,
            );
        }

        let delivered = Arc::new(Mutex::new(Vec::new()));
        {
            let delivered = Arc::clone(&delivered);
            Arc::get_mut(&mut server)
                .expect("unique test server")
                .appserver_notification_sink = Arc::new(move |_, _, text, _, _, _mode| {
                delivered.lock().unwrap().push(text.to_string());
                Ok(serde_json::json!({"accepted": true}))
            });
        }
        assert!(attempt_notification_with_at(
            &server,
            "backlog-0",
            &subscription_id,
            window_start + 120_000,
        ));
        let text = delivered.lock().unwrap().join("\n");
        assert!(!text.contains("backlog-0"));
        assert!(text.contains("backlog-1"));
        assert!(text.contains("backlog-2"));
        assert!(text.contains("backlog-3"));
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs["backlog-0"].state, "pending");
        assert_eq!(state.msgs["backlog-1"].state, "pending");
        assert_eq!(state.msgs["backlog-1"].wake_attempt_count, 1);
        assert_eq!(state.msgs["backlog-2"].state, "pending");
        assert_eq!(state.msgs["backlog-2"].wake_attempt_count, 1);
        assert_eq!(state.msgs["backlog-3"].state, "pending");
        assert_eq!(state.msgs["backlog-3"].wake_attempt_count, 1);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_notice_is_immediate_and_isolated_from_automatic_batching() {
        let (mut server, root) = test_server();
        let subscription_id = register_and_subscribe(&server, "recipient");
        let now = now_ms();
        queue_message(
            &server,
            "recipient",
            &subscription_id,
            "automatic-notice",
            now - 120_001,
        );
        queue_message(
            &server,
            "recipient",
            &subscription_id,
            "explicit-notice",
            now,
        );
        server.commit(&[Event::DeliveryMode {
            msg_id: "explicit-notice".into(),
            mode: "explicit-notification".into(),
            source_thread_id: None,
        }]);

        let delivered = Arc::new(Mutex::new(Vec::new()));
        {
            let delivered = Arc::clone(&delivered);
            Arc::get_mut(&mut server)
                .expect("unique test server")
                .appserver_notification_sink = Arc::new(move |_, _, text, _, explicit, _mode| {
                delivered.lock().unwrap().push((explicit, text.to_string()));
                Ok(serde_json::json!({"accepted": true}))
            });
        }
        assert!(attempt_notification_with_at(
            &server,
            "explicit-notice",
            &subscription_id,
            now,
        ));
        let delivered_snapshot = delivered.lock().unwrap();
        assert_eq!(delivered_snapshot.len(), 1);
        assert!(
            delivered_snapshot[0].0,
            "explicit sendmessage must use immediate notify"
        );
        let text = &delivered_snapshot[0].1;
        assert!(text.contains("explicit-notice"));
        assert!(!text.contains("automatic-notice"));
        drop(delivered_snapshot);
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs["explicit-notice"].state, "pending");
        assert_eq!(state.msgs["explicit-notice"].wake_attempt_count, 1);
        assert_eq!(state.msgs["automatic-notice"].state, "pending");
        drop(state);
        assert!(attempt_notification_with_at(
            &server,
            "automatic-notice",
            &subscription_id,
            now,
        ));
        let delivered = delivered.lock().unwrap();
        assert!(delivered
            .iter()
            .any(|(explicit, text)| { !explicit && text.contains("automatic-notice") }));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mailbox_status_reports_a_valid_but_incomplete_projection() {
        let (mut server, root) = test_server();
        let subscription_id = register_and_subscribe(&server, "recipient");
        queue_message(
            &server,
            "recipient",
            &subscription_id,
            "projection-gap",
            now_ms(),
        );
        std::fs::write(
            root.join(".agent-collab/mailbox/recipient-recipient.jsonl"),
            "",
        )
        .unwrap();

        let response = dispatch(
            &server,
            Req::MailboxRead {
                all: false,
                sort: Some("time-asc".into()),
                worker_id: Some("recipient".into()),
            },
        );
        assert!(response.ok);
        assert_eq!(response.data["recipient_jsonl"]["status"], "incomplete");
        assert_eq!(
            response.data["recipient_jsonl"]["missing_message_ids"],
            serde_json::json!(["projection-gap"])
        );
        assert!(response.data["recipient_jsonl"]["exact_error"]
            .as_str()
            .unwrap()
            .contains("projection-gap"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_reservation_is_durable_and_is_never_replayed() {
        let (mut server, root) = test_server();
        let subscription_id = register_and_subscribe(&server, "recipient");
        let now = now_ms();
        queue_message(
            &server,
            "recipient",
            &subscription_id,
            "reserved-once",
            now - 120_001,
        );

        Arc::get_mut(&mut server)
            .expect("unique test server")
            .appserver_notification_sink =
            Arc::new(|_, _, _, _, _, _| Err("test sink rejected".into()));
        assert!(!attempt_notification_with_at(
            &server,
            "reserved-once",
            &subscription_id,
            now,
        ));
        assert_eq!(
            server.state.lock().unwrap().msgs["reserved-once"].state,
            "pending"
        );
        assert_eq!(
            server.state.lock().unwrap().msgs["reserved-once"].wake_attempt_count,
            1
        );
        assert!(!attempt_notification_with_at(
            &server,
            "reserved-once",
            &subscription_id,
            now + 300_000,
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
