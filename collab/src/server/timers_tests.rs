    use super::*;
    use crate::server::keepalive::Record;
    use crate::server::state::{
        Event, MigrationRecord, NotificationSubscription, State, TaskRec, WaitSpec,
    };
    use std::sync::Mutex;

    fn test_server() -> (Arc<Server>, std::path::PathBuf) {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "collab-notification-timer-{}-{sequence}",
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
                host_paths: crate::scope::HostPaths::for_state_root(root.join("host-state"))
                    .unwrap(),
                state: Mutex::new(State::default()),
                journal: Mutex::new(journal),
                appserver_candidate_check: Arc::new(|candidate| {
                    Ok(SelectedTransport {
                        kind: crate::proto::TransportKind::AppServer,
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

    fn register(server: &Server, worker_id: &str) {
        let response =
            crate::server::peer_tests::register(server, worker_id, &format!("thread-{worker_id}"));
        assert!(response.ok, "worker registration failed: {response:?}");
    }

    fn register_tmux(server: &Server, worker_id: &str, endpoint: crate::proto::TmuxEndpoint) {
        let response = crate::server::peer_tests::register_tmux(server, worker_id, endpoint);
        assert!(response.ok, "worker registration failed: {response:?}");
    }

    fn register_master(server: &Server) {
        register(server, "master");
        let now = now_ms();
        let promoted = crate::server::handle_master_promote(
            server,
            "master".into(),
            "token-master".into(),
            "user-approved".into(),
        );
        assert!(promoted.ok, "master promotion failed: {promoted:?}");
        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record: Record {
                observed: "idle".into(),
                idle_since_ms: now - 900_001,
                ..Record::default()
            },
        }]);
    }

    fn master_idle_subscription(server: &Server, interval_ms: i64) -> String {
        let now = now_ms();
        let id = format!("sub-master-idle-{interval_ms}");
        let (target, method) = subscription_target(server, "master");
        server.commit(&[Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: id.clone(),
                worker_id: "master".into(),
                event: "master-idle".into(),
                subject: Some("master-idle".into()),
                target,
                method,
                trigger_ms: Some(now - 1),
                trigger_times_ms: Vec::new(),
                interval_ms: Some(interval_ms),
                repeat_count: 3,
                fired_count: 0,
                expires_ms: now + 86_400_000,
                status: "armed".into(),
                created_ms: now - interval_ms,
                updated_ms: now,
                status_reason: None,
            },
        }]);
        id
    }

    fn subscribe(
        server: &Server,
        worker_id: &str,
        event: &str,
        subject: Option<&str>,
        trigger_ms: Option<i64>,
    ) -> String {
        let id = format!("sub-{worker_id}-{event}");
        let (target, method) = subscription_target(server, worker_id);
        server.commit(&[Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: id.clone(),
                worker_id: worker_id.into(),
                event: event.into(),
                subject: subject.map(str::to_owned),
                target,
                method,
                trigger_ms,
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                fired_count: 0,
                expires_ms: now_ms() + 60_000,
                status: "armed".into(),
                created_ms: now_ms(),
                updated_ms: now_ms(),
                status_reason: None,
            },
        }]);
        id
    }

    fn periodic_deadline_subscription(server: &Server, worker_id: &str, subject: &str) -> String {
        let now = now_ms();
        let id = format!("sub-{worker_id}-periodic-deadline");
        let (target, method) = subscription_target(server, worker_id);
        server.commit(&[Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: id.clone(),
                worker_id: worker_id.into(),
                event: "deadline".into(),
                subject: Some(subject.into()),
                target,
                method,
                trigger_ms: Some(now - 1),
                trigger_times_ms: Vec::new(),
                interval_ms: Some(600_000),
                repeat_count: 3,
                fired_count: 0,
                expires_ms: now + 86_400_000,
                status: "armed".into(),
                created_ms: now - 600_000,
                updated_ms: now,
                status_reason: None,
            },
        }]);
        id
    }

    fn subscription_target(server: &Server, worker_id: &str) -> (String, String) {
        server
            .state
            .lock()
            .unwrap()
            .workers
            .get(worker_id)
            .and_then(|worker| worker.transport.as_ref())
            .map(|transport| {
                (
                    transport.thread_id.clone().unwrap_or_default(),
                    transport.kind.as_str().to_owned(),
                )
            })
            .unwrap_or_else(|| (format!("thread-{worker_id}"), "appserver".into()))
    }

    fn freeze_admission(server: &Server) {
        let now = now_ms();
        server.commit(&[Event::MigrationUpdated {
            migration: MigrationRecord {
                id: "migration".into(),
                from_version: "v1".into(),
                to_version: "v1".into(),
                phase: "applied".into(),
                admission_frozen: true,
                snapshot_hash: None,
                worker_count: 1,
                task_count: 0,
                message_count: 0,
                operator: "test".into(),
                issues: Vec::new(),
                created_ms: now,
                updated_ms: now,
            },
        }]);
    }

    fn bind_message(server: &Server, worker_id: &str, subscription_id: &str) -> String {
        bind_message_with_id(
            server,
            worker_id,
            subscription_id,
            &format!("message-{worker_id}"),
        )
    }

    fn bind_message_with_type(
        server: &Server,
        worker_id: &str,
        subscription_id: &str,
        message_id: &str,
        message_type: &str,
    ) -> String {
        server.commit(&[
            Event::Sent {
                msg: Message {
                    id: message_id.to_string(),
                    from: "peer".into(),
                    to: worker_id.into(),
                    mtype: message_type.into(),
                    subject: Some("released:held".into()),
                    body: "RESOURCE_RELEASED task=held".into(),
                    in_reply_to: None,
                    created_ms: now_ms() - 120_001,
                    state: "pending".into(),
                    wake_attempt_count: 0,
                    last_wake_attempt_ms: 0,
                    retry_attempted: false,
                },
            },
            Event::WakeBound {
                message_id: message_id.to_string(),
                subscription_id: subscription_id.into(),
            },
        ]);
        message_id.to_string()
    }

    fn bind_message_with_id(
        server: &Server,
        worker_id: &str,
        subscription_id: &str,
        message_id: &str,
    ) -> String {
        bind_message_with_type(server, worker_id, subscription_id, message_id, "notify")
    }

    fn working_task(server: &Server, worker_id: &str) {
        let now = now_ms();
        server.commit(&[Event::TaskCreated {
            task: TaskRec {
                id: "task".into(),
                owner: worker_id.into(),
                created_by: worker_id.into(),
                feature_id: Some("feature".into()),
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p2".into(),
                status: "working".into(),
                next_step: Some("keep working".into()),
                wait: None,
                created_ms: now,
                updated_ms: now,
            },
        }]);
    }

    #[test]
    fn ordinary_work_never_generates_periodic_continuation() {
        let (server, root) = test_server();
        register(&server, "owner");
        working_task(&server, "owner");
        tick_with_idle(&server, &|_| true);
        assert!(server.state.lock().unwrap().msgs.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn admission_freeze_blocks_deadline_wakeup_without_appserver_probe() {
        let (mut server, root) = test_server();
        let tmux = crate::server::peer_tests::IsolatedTmux::start(&root);
        register_tmux(&server, "master", tmux.endpoints().remove(0));
        let now = now_ms();
        let promoted = crate::server::handle_master_promote(
            &server,
            "master".into(),
            "token-master".into(),
            "user-approved".into(),
        );
        assert!(promoted.ok, "master promotion failed: {promoted:?}");
        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record: Record {
                observed: "idle".into(),
                idle_since_ms: now - 900_001,
                ..Record::default()
            },
        }]);
        periodic_deadline_subscription(&server, "master", "goal:frozen");
        freeze_admission(&server);

        tick_at(&server, now_ms());

        assert!(server.state.lock().unwrap().msgs.is_empty());
        drop(tmux);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn deadline_wakeup_does_not_probe_appserver_readiness() {
        let (mut server, root) = test_server();
        let probes = Arc::new(Mutex::new(Vec::new()));
        {
            let probes = Arc::clone(&probes);
            Arc::get_mut(&mut server)
                .expect("unique test server")
                .appserver_thread_status = Arc::new(move |_, thread_id| {
                probes.lock().unwrap().push(thread_id.to_string());
                Ok(serde_json::json!({
                    "thread": {
                        "id": thread_id,
                        "status": {"type": "idle"},
                        "canAcceptDirectInput": true
                    }
                }))
            });
        }
        let tmux = crate::server::peer_tests::IsolatedTmux::start(&root);
        let endpoints = tmux.endpoints();
        register_tmux(&server, "master", endpoints[0].clone());
        register_tmux(&server, "worker", endpoints[1].clone());
        let now = now_ms();
        let promoted = crate::server::handle_master_promote(
            &server,
            "master".into(),
            "token-master".into(),
            "user-approved".into(),
        );
        assert!(promoted.ok, "master promotion failed: {promoted:?}");
        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record: Record {
                observed: "idle".into(),
                idle_since_ms: now - 900_001,
                ..Record::default()
            },
        }]);
        periodic_deadline_subscription(&server, "master", "periodic:due-master");
        subscribe(
            &server,
            "worker",
            "deadline",
            Some("periodic:not-due-worker"),
            Some(now_ms() + 600_000),
        );
        probes.lock().unwrap().clear();

        probes.lock().unwrap().clear();
        tick_at(&server, now_ms());

        assert!(probes.lock().unwrap().is_empty());
        assert!(server.state.lock().unwrap().msgs.values().any(|message| {
            message.to == "master"
                && message.subject.as_deref() == Some("deadline:periodic:due-master")
        }));
        drop(tmux);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn deadline_wakeup_does_not_depend_on_appserver_status_callback() {
        let (mut server, root) = test_server();
        let tmux = crate::server::peer_tests::IsolatedTmux::start(&root);
        register_tmux(&server, "worker", tmux.endpoints().remove(0));
        Arc::get_mut(&mut server)
            .expect("unique test server")
            .appserver_thread_status = Arc::new(|_, _| panic!("AppServer probe must not run"));
        let subscription_id = subscribe(
            &server,
            "worker",
            "deadline",
            Some("worker:one-shot"),
            Some(now_ms() - 1),
        );

        tick_at(&server, now_ms());

        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs.len(), 1);
        let subscription = &state.notification_subscriptions[&subscription_id];
        assert_eq!(subscription.status, "armed");
        drop(state);
        drop(tmux);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn configured_immediate_and_disabled_delivery_are_respected() {
        for (mode, enabled, expected) in [
            ("immediate", true, true),
            ("batch", true, false),
            ("immediate", false, false),
        ] {
            let (mut server, root) = test_server();
            let config = &mut Arc::get_mut(&mut server).unwrap().config;
            config.notifications.mode = mode.into();
            config.notifications.enabled = enabled;
            register(&server, "owner");
            let sub = subscribe(&server, "owner", "direct-message", None, None);
            let id = bind_message(&server, "owner", &sub);
            server
                .state
                .lock()
                .unwrap()
                .msgs
                .get_mut(&id)
                .unwrap()
                .created_ms = now_ms();
            assert_eq!(
                super::super::attempt_notification_with_default(
                    &server,
                    &id,
                    &sub,
                    &|_| true,
                    &|_, _| { true }
                ),
                expected
            );
            assert_eq!(
                server.state.lock().unwrap().msgs[&id].wake_attempt_count,
                if expected { 1 } else { 0 }
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn no_subscription_means_zero_wake_attempts() {
        let (server, root) = test_server();
        register(&server, "owner");
        server.commit(&[Event::Sent {
            msg: Message {
                id: "message".into(),
                from: "peer".into(),
                to: "owner".into(),
                mtype: "notify".into(),
                subject: Some("released:held".into()),
                body: "RESOURCE_RELEASED task=held".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        }]);
        tick_with_idle(&server, &|_| true);
        assert_eq!(
            server.state.lock().unwrap().msgs["message"].wake_attempt_count,
            0
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn failed_one_shot_notification_has_one_attempt_lifetime_cap() {
        let (server, root) = test_server();
        register(&server, "owner");
        let subscription_id = subscribe(&server, "owner", "resource-released", Some("held"), None);
        let message_id = bind_message(&server, "owner", &subscription_id);
        for _ in 0..4 {
            super::super::attempt_notification_with_default(
                &server,
                &message_id,
                &subscription_id,
                &|_| true,
                &|_, _| false,
            );
        }
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.msgs[&message_id].wake_attempt_count,
            MAX_WAKE_ATTEMPTS
        );
        assert_eq!(
            state.notification_subscriptions[&subscription_id].status,
            "armed"
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn failed_direct_message_exhausts_only_the_message() {
        let (server, root) = test_server();
        register(&server, "owner");
        let subscription_id = subscribe(&server, "owner", "direct-message", None, None);
        let message_id = bind_message(&server, "owner", &subscription_id);
        for _ in 0..MAX_WAKE_ATTEMPTS {
            super::super::attempt_notification_with_default(
                &server,
                &message_id,
                &subscription_id,
                &|_| true,
                &|_, _| false,
            );
        }
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.msgs[&message_id].wake_attempt_count,
            MAX_WAKE_ATTEMPTS
        );
        assert_eq!(
            state.notification_subscriptions[&subscription_id].status,
            "armed"
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn restart_does_not_reset_exhausted_direct_message_attempts() {
        let (server, root) = test_server();
        register(&server, "owner");
        let subscription_id = subscribe(&server, "owner", "direct-message", None, None);
        let message_id = bind_message(&server, "owner", &subscription_id);
        for _ in 0..MAX_WAKE_ATTEMPTS {
            super::super::attempt_notification_with_default(
                &server,
                &message_id,
                &subscription_id,
                &|_| true,
                &|_, _| false,
            );
        }
        drop(server);

        let replayed = super::super::replay(&root).unwrap();
        let journal = std::fs::OpenOptions::new()
            .append(true)
            .open(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap();
        let restarted = Server {
            config: crate::config::Config::default(),
            root: root.clone(),
            storage_root: root.clone(),
            journal_path: root.join(".agent-collab/server/journal.jsonl"),
            host_paths: crate::scope::HostPaths::for_state_root(root.join("host-state")).unwrap(),
            state: Mutex::new(replayed),
            journal: Mutex::new(journal),
            appserver_candidate_check: super::super::default_appserver_candidate_check(),
            appserver_notification_sink: super::super::default_appserver_notification_sink(),
            appserver_thread_status: super::super::default_appserver_thread_status(),
            appserver_thread_archive: super::super::default_appserver_thread_archive(),
            mailbox_notify: tokio::sync::Notify::new(),
        };
        let sent = std::sync::atomic::AtomicBool::new(false);
        assert!(!super::super::attempt_notification_with_default(
            &restarted,
            &message_id,
            &subscription_id,
            &|_| true,
            &|_, _| {
                sent.store(true, std::sync::atomic::Ordering::Relaxed);
                true
            },
        ));
        let state = restarted.state.lock().unwrap();
        assert!(!sent.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(
            state.msgs[&message_id].wake_attempt_count,
            MAX_WAKE_ATTEMPTS
        );
        assert_eq!(
            state.notification_subscriptions[&subscription_id].status,
            "armed"
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn successful_event_notification_consumes_one_shot_subscription() {
        let (server, root) = test_server();
        register(&server, "owner");
        let subscription_id = subscribe(&server, "owner", "resource-released", Some("held"), None);
        let message_id = bind_message(&server, "owner", &subscription_id);
        assert!(super::super::attempt_notification_with_default(
            &server,
            &message_id,
            &subscription_id,
            &|_| true,
            &|_, _| true,
        ));
        server.commit(&[Event::NotificationConsumed {
            subscription_id: subscription_id.clone(),
            message_id: message_id.clone(),
            consumed_ms: now_ms(),
        }]);
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[&message_id].state, "pending");
        assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
        assert_eq!(
            state.notification_subscriptions[&subscription_id].status,
            "consumed"
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn successful_direct_messages_reuse_subscription_without_a_burst() {
        let (server, root) = test_server();
        register(&server, "owner");
        let subscription_id = subscribe(&server, "owner", "direct-message", None, None);
        let first_id = bind_message(&server, "owner", &subscription_id);
        assert!(super::super::attempt_notification_with_default(
            &server,
            &first_id,
            &subscription_id,
            &|_| true,
            &|_, _| true,
        ));

        let second_id = "message-owner-second".to_string();
        server.commit(&[
            Event::Sent {
                msg: Message {
                    id: second_id.clone(),
                    from: "peer".into(),
                    to: "owner".into(),
                    mtype: "notify".into(),
                    subject: Some("second".into()),
                    body: "SECOND_NOTICE".into(),
                    in_reply_to: None,
                    created_ms: now_ms(),
                    state: "pending".into(),
                    wake_attempt_count: 0,
                    last_wake_attempt_ms: 0,
                    retry_attempted: false,
                },
            },
            Event::WakeBound {
                message_id: second_id.clone(),
                subscription_id: subscription_id.clone(),
            },
        ]);
        assert!(!super::super::attempt_notification_with_default(
            &server,
            &second_id,
            &subscription_id,
            &|_| true,
            &|_, _| true,
        ));
        assert_eq!(
            server.state.lock().unwrap().msgs[&second_id].wake_attempt_count,
            0
        );

        server.commit(&[Event::NotificationStatus {
            subscription_id: subscription_id.clone(),
            status: "armed".into(),
            updated_ms: now_ms() - super::super::DIRECT_MESSAGE_WAKE_COOLDOWN_MS - 1,
        }]);
        {
            let mut state = server.state.lock().unwrap();
            state.msgs.get_mut(&second_id).unwrap().created_ms = now_ms() - 120_001;
            state.msgs.get_mut(&first_id).unwrap().last_wake_attempt_ms = now_ms() - 120_001;
        }
        assert!(super::super::attempt_notification_with_default(
            &server,
            &second_id,
            &subscription_id,
            &|_| true,
            &|_, _| true,
        ));
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[&first_id].state, "pending");
        assert_eq!(state.msgs[&first_id].wake_attempt_count, 1);
        assert_eq!(state.msgs[&second_id].state, "pending");
        assert_eq!(state.msgs[&second_id].wake_attempt_count, 1);
        assert_eq!(
            state.notification_subscriptions[&subscription_id].status,
            "armed"
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn batch_excludes_messages_after_first_window_and_never_replays() {
        let (server, root) = test_server();
        register(&server, "owner");
        let subscription = subscribe(&server, "owner", "direct-message", None, None);
        let first = bind_message(&server, "owner", &subscription);
        let mut second = server.state.lock().unwrap().msgs[&first].clone();
        second.id = "second".into();
        second.subject = Some("new topic".into());
        second.created_ms = now_ms();
        server.commit(&[
            Event::Sent { msg: second },
            Event::WakeBound {
                message_id: "second".into(),
                subscription_id: subscription.clone(),
            },
        ]);
        let calls = std::cell::RefCell::new(Vec::new());
        assert!(super::super::attempt_notification_with_default(
            &server,
            &first,
            &subscription,
            &|_| true,
            &|_, text| {
                calls.borrow_mut().push(text.to_string());
                true
            }
        ));
        assert_eq!(calls.borrow().len(), 1);
        assert!(calls.borrow()[0].contains(&first));
        assert!(calls.borrow()[0].contains("message_ids=message-owner"));
        assert!(!calls.borrow()[0].contains("second"));
        assert!(calls.borrow()[0].contains("action_categories="));
        assert_eq!(server.state.lock().unwrap().msgs["second"].state, "pending");
        assert!(!super::super::attempt_notification_with_default(
            &server,
            &first,
            &subscription,
            &|_| true,
            &|_, _| panic!("duplicate delivery")
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_delivery_mode_bypasses_batch_window() {
        let (server, root) = test_server();
        register(&server, "owner");
        let sub = subscribe(&server, "owner", "direct-message", None, None);
        let id = bind_message(&server, "owner", &sub);
        server.commit(&[Event::DeliveryMode {
            msg_id: id.clone(),
            mode: "explicit-notification".into(),
            source_thread_id: None,
        }]);
        let calls = std::cell::Cell::new(0);
        assert!(super::super::attempt_notification_with_default(
            &server,
            &id,
            &sub,
            &|_| true,
            &|_, _| {
                calls.set(calls.get() + 1);
                true
            }
        ));
        assert_eq!(calls.get(), 1);
        assert_eq!(server.state.lock().unwrap().msgs[&id].state, "pending");
        assert_eq!(server.state.lock().unwrap().msgs[&id].wake_attempt_count, 1);
        std::fs::remove_dir_all(root).unwrap();
    }

include!("timers_tests_part2.rs");
