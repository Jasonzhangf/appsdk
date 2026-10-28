use super::*;

    #[test]
    fn fresh_batch_waits_two_minutes_and_absent_batch_is_not_replayed() {
        let (server, root) = test_server();
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
        assert!(!super::super::attempt_notification_with_default(
            &server,
            &id,
            &sub,
            &|_| true,
            &|_, _| panic!("early delivery")
        ));
        server
            .state
            .lock()
            .unwrap()
            .msgs
            .get_mut(&id)
            .unwrap()
            .created_ms -= 120_001;
        assert!(!super::super::attempt_notification_with_default(
            &server,
            &id,
            &sub,
            &|_| false,
            &|_, _| panic!("absent delivery")
        ));
        assert!(!super::super::attempt_notification_with_default(
            &server,
            &id,
            &sub,
            &|_| true,
            &|_, _| panic!("late replay")
        ));
        assert_eq!(server.state.lock().unwrap().msgs[&id].wake_attempt_count, 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn automatic_retry_uses_bound_event_policy_window() {
        let (mut server, root) = test_server();
        Arc::get_mut(&mut server)
            .unwrap()
            .config
            .notifications
            .batch_window_seconds = 7;
        register(&server, "owner");
        let subscription_id = subscribe(&server, "owner", "direct-message", None, None);
        let message_id = bind_message(&server, "owner", &subscription_id);
        let base = now_ms();
        let delay_ms = server.config.notifications.delay_ms("direct-message");
        {
            let mut state = server.state.lock().unwrap();
            state
                .notification_subscriptions
                .get_mut(&subscription_id)
                .unwrap()
                .expires_ms = base + 300_000;
            state.msgs.get_mut(&message_id).unwrap().created_ms = base;
        }

        assert_eq!(delay_ms, 7_000);
        let tick_before = base + delay_ms - 1;
        tick_with_idle_at(&server, tick_before, &|_| true);
        assert_eq!(
            server.state.lock().unwrap().msgs[&message_id].wake_attempt_count,
            0,
            "the timer must not reserve before the bound direct-message window"
        );
        assert_eq!(
            server.state.lock().unwrap().msgs[&message_id].last_wake_attempt_ms,
            0
        );

        let tick_at = base + delay_ms;
        tick_with_idle_at(&server, tick_at, &|_| true);
        assert_eq!(
            server.state.lock().unwrap().msgs[&message_id].wake_attempt_count,
            1,
            "the timer must reserve exactly at the bound direct-message window"
        );
        assert_eq!(
            server.state.lock().unwrap().msgs[&message_id].last_wake_attempt_ms,
            tick_at,
            "the timer reservation must carry the policy-boundary timestamp"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn immediate_event_policy_is_not_delayed_by_the_batch_window() {
        let (server, root) = test_server();
        register(&server, "owner");
        let subscription_id = subscribe(&server, "owner", "deadline", Some("due"), None);
        let message_id = bind_message(&server, "owner", &subscription_id);
        let base = now_ms();
        let delay_ms = server.config.notifications.delay_ms("deadline");
        assert_eq!(delay_ms, 0);
        {
            let mut state = server.state.lock().unwrap();
            state
                .notification_subscriptions
                .get_mut(&subscription_id)
                .unwrap()
                .expires_ms = base + 300_000;
            state.msgs.get_mut(&message_id).unwrap().created_ms = base;
        }
        let tick_before = base - 1;
        tick_with_idle_at(&server, tick_before, &|_| true);
        assert_eq!(
            server.state.lock().unwrap().msgs[&message_id].wake_attempt_count,
            0,
            "the timer must leave an immediate event unreserved before its message timestamp"
        );

        let tick_at = base;
        tick_with_idle_at(&server, tick_at, &|_| true);
        assert_eq!(
            server.state.lock().unwrap().msgs[&message_id].wake_attempt_count,
            1,
            "the timer must reserve an immediate event without the batch window"
        );
        assert_eq!(
            server.state.lock().unwrap().msgs[&message_id].last_wake_attempt_ms,
            tick_at
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scheduler_tick_reuses_timestamp_for_expiry_and_retry() {
        let (mut server, root) = test_server();
        Arc::get_mut(&mut server).unwrap().config.keepalive.enabled = false;
        register(&server, "owner");
        let expired_subscription = subscribe(&server, "owner", "deadline", Some("expired"), None);
        let retry_subscription = subscribe(&server, "owner", "direct-message", None, None);
        let message_id = bind_message(&server, "owner", &retry_subscription);
        let scheduler_now = now_ms();
        let retry_delay_ms = server.config.notifications.delay_ms("direct-message");
        {
            let mut state = server.state.lock().unwrap();
            let expired = state
                .notification_subscriptions
                .get_mut(&expired_subscription)
                .unwrap();
            expired.expires_ms = scheduler_now;
            expired.updated_ms = scheduler_now - 1;
            let retry = state
                .notification_subscriptions
                .get_mut(&retry_subscription)
                .unwrap();
            retry.expires_ms = scheduler_now + 60_000;
            retry.updated_ms = scheduler_now - 1;
            let message = state.msgs.get_mut(&message_id).unwrap();
            message.created_ms = scheduler_now - retry_delay_ms;
            message.last_wake_attempt_ms = 0;
        }

        tick_at(&server, scheduler_now);

        let state = server.state.lock().unwrap();
        assert_eq!(
            state.notification_subscriptions[&expired_subscription].status,
            "expired"
        );
        assert_eq!(
            state.notification_subscriptions[&expired_subscription].updated_ms, scheduler_now,
            "subscription expiry must use the scheduler timestamp"
        );
        assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
        assert_eq!(
            state.msgs[&message_id].last_wake_attempt_ms, scheduler_now,
            "automatic retry reservation must use the scheduler timestamp"
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn failed_explicit_message_is_not_automatically_replayed(message_type: &str) {
        let (mut server, root) = test_server();
        Arc::get_mut(&mut server)
            .unwrap()
            .appserver_notification_sink =
            Arc::new(|_, _, _, _, _, _| Err("test sink rejected".into()));
        register(&server, "owner");
        let subscription_id = subscribe(&server, "owner", "direct-message", None, None);
        let message_id = bind_message_with_type(
            &server,
            "owner",
            &subscription_id,
            &format!("explicit-{message_type}"),
            message_type,
        );
        let base = now_ms();
        let retry_delay_ms = server.config.notifications.delay_ms("direct-message");
        {
            let mut state = server.state.lock().unwrap();
            state
                .notification_subscriptions
                .get_mut(&subscription_id)
                .unwrap()
                .expires_ms = base + 300_000;
            state.msgs.get_mut(&message_id).unwrap().created_ms = base;
        }
        server.commit(&[Event::DeliveryMode {
            msg_id: message_id.clone(),
            mode: "explicit-notification".into(),
            source_thread_id: None,
        }]);

        {
            let state = server.state.lock().unwrap();
            assert!(super::super::mailbox::is_explicit_delivery_mode(
                &state,
                &state.msgs[&message_id]
            ));
        }
        let timer_at = base + retry_delay_ms;
        tick_with_idle_at(&server, timer_at, &|_| true);
        {
            let state = server.state.lock().unwrap();
            assert_eq!(state.msgs[&message_id].state, "pending");
            assert_eq!(
                state.msgs[&message_id].wake_attempt_count, 0,
                "eligible timer ticks must exclude explicit {message_type} delivery"
            );
            assert_eq!(state.msgs[&message_id].last_wake_attempt_ms, 0);
        }

        // The explicit operation remains available after the timer exclusion;
        // its failed attempt is the only reservation recorded for this message.
        assert!(!super::super::attempt_notification_with_at(
            &server,
            &message_id,
            &subscription_id,
            timer_at,
        ));
        assert_eq!(
            server.state.lock().unwrap().msgs[&message_id].wake_attempt_count,
            1
        );

        tick_with_idle_at(&server, timer_at + retry_delay_ms, &|_| true);
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[&message_id].state, "pending");
        assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_explicit_peer_notification_is_not_automatically_replayed() {
        failed_explicit_message_is_not_automatically_replayed("notify");
    }

    #[test]
    fn failed_explicit_request_is_not_automatically_replayed() {
        failed_explicit_message_is_not_automatically_replayed("request");
    }

    #[test]
    fn failed_explicit_reply_is_not_automatically_replayed() {
        failed_explicit_message_is_not_automatically_replayed("reply");
    }

    #[test]
    fn explicit_typed_messages_are_isolated_from_immediate_system_batches() {
        for message_type in ["request", "reply"] {
            let (server, root) = test_server();
            register(&server, "owner");
            let direct_subscription = subscribe(&server, "owner", "direct-message", None, None);
            let deadline_subscription = subscribe(&server, "owner", "deadline", Some("due"), None);
            let explicit_id = bind_message_with_type(
                &server,
                "owner",
                &direct_subscription,
                &format!("explicit-{message_type}"),
                message_type,
            );
            let automatic_id = bind_message_with_type(
                &server,
                "owner",
                &deadline_subscription,
                "automatic-deadline",
                "notification",
            );
            server.commit(&[Event::DeliveryMode {
                msg_id: explicit_id.clone(),
                mode: "explicit-notification".into(),
                source_thread_id: None,
            }]);

            assert!(super::super::attempt_notification_with_at(
                &server,
                &explicit_id,
                &direct_subscription,
                now_ms(),
            ));

            let state = server.state.lock().unwrap();
            assert_eq!(state.msgs[&explicit_id].state, "pending");
            assert_eq!(state.msgs[&explicit_id].wake_attempt_count, 1);
            assert_eq!(state.msgs[&automatic_id].state, "pending");
            assert_eq!(state.msgs[&automatic_id].wake_attempt_count, 0);
            drop(state);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn deadline_subscription_emits_once_only_after_trigger() {
        let (server, root) = test_server();
        register(&server, "owner");
        let subscription_id = subscribe(
            &server,
            "owner",
            "deadline",
            Some("timer"),
            Some(now_ms() - 1),
        );
        tick_with_idle(&server, &|_| false);
        tick_with_idle(&server, &|_| false);
        let state = server.state.lock().unwrap();
        assert_eq!(
            state
                .wake_bindings
                .values()
                .filter(|bound| *bound == &subscription_id)
                .count(),
            1
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn goal_deadline_consumes_one_occurrence_even_when_legacy_record_is_recurring() {
        let (server, root) = test_server();
        register_master(&server);
        let now = now_ms();
        let subscription_id = "sub-goal-legacy".to_string();
        server.commit(&[Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: subscription_id.clone(),
                worker_id: "master".into(),
                event: "deadline".into(),
                subject: Some("goal:inactive".into()),
                target: "thread-master".into(),
                method: "tmux".into(),
                trigger_ms: Some(now - 1),
                trigger_times_ms: Vec::new(),
                interval_ms: Some(600_000),
                repeat_count: 100,
                fired_count: 26,
                expires_ms: now + 86_400_000,
                status: "armed".into(),
                created_ms: now - 26 * 600_000,
                updated_ms: now,
                status_reason: None,
            },
        }]);

        tick_with_idle(&server, &|_| false);
        tick_with_idle(&server, &|_| false);
        let state = server.state.lock().unwrap();
        assert!(
            subscription_messages(&state, &subscription_id).is_empty(),
            "a fired inactive goal cannot rearm"
        );
        assert_eq!(
            state.notification_subscriptions[&subscription_id].status,
            "consumed",
            "fired={} reason={:?}",
            state.notification_subscriptions[&subscription_id].fired_count,
            state.notification_subscriptions[&subscription_id].status_reason
        );
        assert_eq!(
            state.notification_subscriptions[&subscription_id]
                .status_reason
                .as_deref(),
            Some("goal-deadline-one-shot-already-fired")
        );
        let snapshot = state.snapshot_events();
        drop(state);

        let (replayed, replay_root) = test_server();
        for event in &snapshot {
            replayed.commit(std::slice::from_ref(event));
        }
        tick_with_idle(&replayed, &|_| false);
        let replayed_state = replayed.state.lock().unwrap();
        assert!(subscription_messages(&replayed_state, &subscription_id).is_empty());
        assert_eq!(
            replayed_state.notification_subscriptions[&subscription_id]
                .status_reason
                .as_deref(),
            Some("goal-deadline-one-shot-already-fired")
        );
        drop(replayed_state);
        std::fs::remove_dir_all(replay_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn goal_deadline_success_consumes_legacy_periodic_shape_once() {
        let (mut server, root) = test_server();
        Arc::get_mut(&mut server).unwrap().config.notifications.mode = "immediate".into();
        register_master(&server);
        let now = now_ms();
        let subscription_id = "sub-goal-active".to_string();
        server.commit(&[Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: subscription_id.clone(),
                worker_id: "master".into(),
                event: "deadline".into(),
                subject: Some("goal:active".into()),
                target: "thread-master".into(),
                method: "tmux".into(),
                trigger_ms: Some(now - 1),
                trigger_times_ms: Vec::new(),
                interval_ms: Some(600_000),
                repeat_count: 100,
                fired_count: 0,
                expires_ms: now + 86_400_000,
                status: "armed".into(),
                created_ms: now - 600_000,
                updated_ms: now,
                status_reason: None,
            },
        }]);

        tick_with_idle(&server, &|_| false);
        let message_id = server
            .state
            .lock()
            .unwrap()
            .wake_bindings
            .iter()
            .find_map(|(message_id, bound)| {
                (bound == &subscription_id).then_some(message_id.clone())
            })
            .expect("goal deadline message");
        server.commit(&[Event::NotificationConsumed {
            subscription_id: subscription_id.clone(),
            message_id: message_id.clone(),
            consumed_ms: now_ms(),
        }]);
        tick_with_idle(&server, &|_| false);
        let state = server.state.lock().unwrap();
        assert_eq!(subscription_messages(&state, &subscription_id).len(), 1);
        assert_eq!(
            state.notification_subscriptions[&subscription_id].status,
            "consumed",
            "fired={} reason={:?}",
            state.notification_subscriptions[&subscription_id].fired_count,
            state.notification_subscriptions[&subscription_id].status_reason
        );
        assert_eq!(
            state.notification_subscriptions[&subscription_id]
                .status_reason
                .as_deref(),
            Some("goal-deadline-one-shot-delivered")
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn duplicate_goal_deadline_records_emit_one_wake_for_the_same_deadline() {
        let (server, root) = test_server();
        register_master(&server);
        let now = now_ms();
        for (subscription_id, created_ms) in [
            ("sub-goal-duplicate-a", now - 2),
            ("sub-goal-duplicate-b", now - 1),
        ] {
            server.commit(&[Event::NotificationSubscribed {
                subscription: NotificationSubscription {
                    id: subscription_id.into(),
                    worker_id: "master".into(),
                    event: "deadline".into(),
                    subject: Some("goal:revision-7".into()),
                    target: "thread-master".into(),
                    method: "tmux".into(),
                    trigger_ms: Some(now - 1),
                    trigger_times_ms: Vec::new(),
                    interval_ms: None,
                    repeat_count: 1,
                    fired_count: 0,
                    expires_ms: now + 86_400_000,
                    status: "armed".into(),
                    created_ms,
                    updated_ms: now,
                    status_reason: None,
                },
            }]);
        }

        tick_with_idle(&server, &|_| false);
        tick_with_idle(&server, &|_| false);
        let state = server.state.lock().unwrap();
        assert_eq!(
            state
                .msgs
                .values()
                .filter(|message| message.subject.as_deref() == Some("deadline:goal:revision-7"))
                .count(),
            1
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn idle_periodic_deadline_sends_lightweight_wake_distinct_from_master_idle() {
        let (mut server, root) = test_server();
        Arc::get_mut(&mut server).unwrap().config.notifications.mode = "immediate".into();
        register_master(&server);
        let subscription_id = periodic_deadline_subscription(&server, "master", "goal:idle-master");
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

        tick_with_idle(&server, &|_| true);

        let deliveries = delivered.lock().unwrap();
        // Stewardship uses explicit notifications. This timer's automatic
        // delivery is independent of that name-adjudication traffic.
        let delivered: Vec<_> = deliveries.iter().filter(|(explicit, _)| !explicit).collect();
        assert_eq!(delivered.len(), 1);
        assert!(
            !delivered[0].0,
            "deadline timer must use automatic delivery"
        );
        assert!(
            delivered[0].1.len() < 1024,
            "deadline wake preview must stay bounded and lightweight"
        );
        assert!(delivered[0].1.contains("deadline:goal:idle-master"));
        assert!(
            !delivered[0].1.contains("MASTER_IDLE_WAKE"),
            "deadline timer and idle notification paths must stay distinct"
        );
        drop(delivered);
        let state = server.state.lock().unwrap();
        assert!(state
            .wake_bindings
            .values()
            .any(|bound| bound == &subscription_id));
        // The adaptor accepted this one-shot goal deadline, so the latch is
        // cleared while the revision stays recorded for same-revision dedup.
        assert!(!state.master_wake.goal_due);
        assert!(state.master_wake.active_goal_revision.is_some());
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn deadline_delivery_failure_is_visible_without_successful_master_wake() {
        let (mut server, root) = test_server();
        Arc::get_mut(&mut server).unwrap().config.notifications.mode = "immediate".into();
        Arc::get_mut(&mut server)
            .unwrap()
            .appserver_notification_sink = Arc::new(|_, _, _, _, _, _| {
            Err("ADAPTER_UNKNOWN: native frame exceeds maximum size".into())
        });
        register_master(&server);
        let subscription_id =
            periodic_deadline_subscription(&server, "master", "goal:oversized-frame");

        tick_with_idle(&server, &|_| true);

        let state = server.state.lock().unwrap();
        let messages = subscription_messages(&state, &subscription_id);
        assert_eq!(messages.len(), 1);
        let message_id = &messages[0].id;
        assert_eq!(state.msgs[message_id].wake_attempt_count, 1);
        assert!(state.notification_delivery_failures[message_id]
            .error
            .contains("native frame exceeds maximum size"));
        assert_eq!(state.master_wake.delivery_state, "delivery_failed");
        assert_ne!(state.master_wake.delivery_state, "notified_unconsumed");
        assert!(
            state.master_wake.goal_due,
            "delivery failure must keep the master goal-due latch raised"
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn peer_goal_deadline_wakes_its_owner_once_without_master_wake() {
        let (mut server, root) = test_server();
        Arc::get_mut(&mut server).unwrap().config.notifications.mode = "immediate".into();
        register_master(&server);
        register(&server, "worker");
        let now = now_ms();
        let subscription_id = "sub-goal-worker".to_string();
        let (target, method) = subscription_target(&server, "worker");
        server.commit(&[Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: subscription_id.clone(),
                worker_id: "worker".into(),
                event: "deadline".into(),
                subject: Some("goal:worker".into()),
                target,
                method,
                trigger_ms: Some(now - 1),
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                fired_count: 0,
                expires_ms: now + 86_400_000,
                status: "armed".into(),
                created_ms: now,
                updated_ms: now,
                status_reason: None,
            },
        }]);

        tick_with_idle(&server, &|_| false);
        tick_with_idle(&server, &|_| false);
        let state = server.state.lock().unwrap();
        let messages = subscription_messages(&state, &subscription_id);
        assert_eq!(messages.len(), 1, "peer goal must emit exactly once");
        assert_eq!(messages[0].to, "worker");
        let message_id = messages[0].id.clone();
        assert!(!state.master_wake.goal_due, "peer goal cannot contaminate master wake");
        drop(state);
        server.commit(&[Event::NotificationConsumed {
            subscription_id: subscription_id.clone(),
            message_id,
            consumed_ms: now_ms(),
        }]);
        tick_with_idle(&server, &|_| false);
        let state = server.state.lock().unwrap();
        assert_eq!(subscription_messages(&state, &subscription_id).len(), 1);
        assert_eq!(
            state.notification_subscriptions[&subscription_id].status,
            "consumed",
            "subscription={:?} failures={:?}",
            state.notification_subscriptions[&subscription_id],
            state.notification_delivery_failures,
        );
        assert!(!state.master_wake.goal_due, "peer goal cannot contaminate master wake");
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cancelled_goal_deadline_never_rearms_after_duplicate_ticks() {
        let (server, root) = test_server();
        register_master(&server);
        let now = now_ms();
        let subscription_id = "sub-goal-cancelled".to_string();
        server.commit(&[Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: subscription_id.clone(),
                worker_id: "master".into(),
                event: "deadline".into(),
                subject: Some("goal:cancelled".into()),
                target: "thread-master".into(),
                method: "appserver".into(),
                trigger_ms: Some(now - 1),
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                fired_count: 0,
                expires_ms: now + 86_400_000,
                status: "armed".into(),
                created_ms: now,
                updated_ms: now,
                status_reason: None,
            },
        }]);

        let cancelled = super::super::handle_goal_schedule_reconcile(
            &server,
            "master".into(),
            "token-master".into(),
            "cancelled".into(),
            "Jenny".into(),
            "ended".into(),
            None,
            None,
            None,
            Some("goal-finished".into()),
        );
        assert!(cancelled.ok, "{}", cancelled.error.unwrap_or_default());
        tick_with_idle(&server, &|_| false);

        let state = server.state.lock().unwrap();
        assert!(subscription_messages(&state, &subscription_id).is_empty());
        assert_eq!(
            state.notification_subscriptions[&subscription_id].status,
            "cancelled"
        );
        assert_eq!(
            state.notification_subscriptions[&subscription_id].fired_count,
            0
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn goal_reconcile_retires_transport_lost_and_rearm_leaves_one_active() {
        let (server, root) = test_server();
        register_master(&server);
        let now = now_ms();
        let transport = server
            .state
            .lock()
            .unwrap()
            .workers
            .get("master")
            .and_then(|worker| worker.transport.clone())
            .unwrap();
        let seed_lost = |id: &str, subject: &str| {
            server.commit(&[Event::NotificationSubscribed {
                subscription: NotificationSubscription {
                    id: id.into(),
                    worker_id: "master".into(),
                    event: "deadline".into(),
                    subject: Some(subject.into()),
                    target: "thread-master".into(),
                    method: "tmux".into(),
                    trigger_ms: Some(now + 60_000),
                    trigger_times_ms: Vec::new(),
                    interval_ms: None,
                    repeat_count: 1,
                    fired_count: 0,
                    expires_ms: now + 86_400_000,
                    status: "transport-lost".into(),
                    created_ms: now,
                    updated_ms: now,
                    status_reason: Some("fixture transport-lost".into()),
                },
            }]);
        };

        seed_lost("sub-goal-lost", "goal:shared");
        let ended = super::super::handle_goal_schedule_reconcile(
            &server,
            "master".into(),
            "token-master".into(),
            "shared".into(),
            "Jenny".into(),
            "ended".into(),
            None,
            None,
            None,
            Some("goal-finished".into()),
        );
        assert!(ended.ok, "{}", ended.error.unwrap_or_default());
        {
            let state = server.state.lock().unwrap();
            assert_eq!(
                state.notification_subscriptions["sub-goal-lost"].status,
                "cancelled"
            );
            // Context retarget only restores armed/transport-lost occurrences.
            let retarget = super::super::retarget_owned_notification_events(
                &state,
                "master",
                &transport,
                now + 1,
            );
            assert!(
                retarget
                    .iter()
                    .all(|event| !matches!(event, Event::NotificationSubscribed {
                        subscription
                    } if subscription.id == "sub-goal-lost")),
                "a cancelled transport-lost goal must stay terminal: {retarget:?}"
            );
            let snapshot = state.snapshot_events();
            drop(state);
            let (replayed, replay_root) = test_server();
            for event in &snapshot {
                replayed.commit(std::slice::from_ref(event));
            }
            let replay_state = replayed.state.lock().unwrap();
            assert_eq!(
                replay_state.notification_subscriptions["sub-goal-lost"].status,
                "cancelled"
            );
            drop(replay_state);
            std::fs::remove_dir_all(replay_root).ok();
        }

        seed_lost("sub-goal-rearm-lost", "goal:shared");
        let rearmed = super::super::handle_goal_schedule_reconcile(
            &server,
            "master".into(),
            "token-master".into(),
            "shared".into(),
            "Jenny".into(),
            "active".into(),
            Some(now + 120_000),
            Some(3600),
            None,
            Some("goal-rearm".into()),
        );
        assert!(rearmed.ok, "{}", rearmed.error.unwrap_or_default());
        {
            let state = server.state.lock().unwrap();
            assert_eq!(
                state.notification_subscriptions["sub-goal-rearm-lost"].status,
                "cancelled"
            );
            let active = state
                .notification_subscriptions
                .values()
                .filter(|subscription| {
                    subscription.worker_id == "master"
                        && is_goal_deadline(subscription)
                        && subscription.subject.as_deref() == Some("goal:shared")
                        && subscription.status == "armed"
                })
                .count();
            assert_eq!(active, 1, "rearm must leave exactly one active occurrence");
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn goal_reconcile_retained_occurrence_retires_transport_lost_predecessor() {
        let (server, root) = test_server();
        register_master(&server);
        let now = now_ms();
        let transport = server
            .state
            .lock()
            .unwrap()
            .workers
            .get("master")
            .and_then(|worker| worker.transport.clone())
            .expect("registered master transport");
        let (target, method) = subscription_target(&server, "master");
        let seed = |id: &str, status: &str, trigger_ms: i64| {
            server.commit(&[Event::NotificationSubscribed {
                subscription: NotificationSubscription {
                    id: id.into(),
                    worker_id: "master".into(),
                    event: "deadline".into(),
                    subject: Some("goal:retained".into()),
                    target: target.clone(),
                    method: method.clone(),
                    trigger_ms: Some(trigger_ms),
                    trigger_times_ms: Vec::new(),
                    interval_ms: None,
                    repeat_count: 1,
                    fired_count: 0,
                    expires_ms: now + 86_400_000,
                    status: status.into(),
                    created_ms: now - 1_000,
                    updated_ms: now,
                    status_reason: (status == "transport-lost")
                        .then(|| "fixture transport-lost".into()),
                },
            }]);
        };
        let retained_trigger = now + 120_000;
        seed("sub-goal-predecessor", "transport-lost", now + 60_000);
        seed("sub-goal-retained", "armed", retained_trigger);
        // A pending scheduler message from the older occurrence must be
        // superseded in the same commit as the retirement.
        let pending_id = bind_message_with_id(
            &server,
            "master",
            "sub-goal-predecessor",
            "msg-predecessor-deadline",
        );

        let reconciled = super::super::handle_goal_schedule_reconcile(
            &server,
            "master".into(),
            "token-master".into(),
            "retained".into(),
            "Jenny".into(),
            "active".into(),
            Some(retained_trigger),
            Some(3600),
            None,
            Some("goal-rearm".into()),
        );
        assert!(reconciled.ok, "{}", reconciled.error.unwrap_or_default());
        assert_eq!(
            reconciled.data["retired_subscription_ids"],
            serde_json::json!(["sub-goal-predecessor"])
        );
        assert_eq!(
            reconciled.data["superseded_message_ids"],
            serde_json::json!(["msg-predecessor-deadline"])
        );
        {
            let state = server.state.lock().unwrap();
            assert_eq!(
                state.notification_subscriptions["sub-goal-predecessor"].status,
                "cancelled"
            );
            assert_eq!(
                state.notification_subscriptions["sub-goal-retained"].status,
                "armed"
            );
            assert_eq!(state.msgs[&pending_id].state, "superseded");
            let recoverable = state
                .notification_subscriptions
                .values()
                .filter(|subscription| {
                    subscription.worker_id == "master"
                        && is_goal_deadline(subscription)
                        && subscription.subject.as_deref() == Some("goal:retained")
                        && matches!(subscription.status.as_str(), "armed" | "transport-lost")
                })
                .count();
            assert_eq!(
                recoverable, 1,
                "retaining a matching occurrence must leave exactly one recoverable occurrence"
            );
        }

        // Context retarget must not revive the retired predecessor.
        let retarget = super::super::retarget_owned_notification_events(
            &server.state.lock().unwrap(),
            "master",
            &transport,
            now + 1,
        );
        assert!(
            retarget.iter().all(|event| !matches!(event, Event::NotificationSubscribed {
                subscription
            } if subscription.id == "sub-goal-predecessor")),
            "a retired predecessor must stay terminal: {retarget:?}"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn master_goal_due_reconciles_post_transition_and_ignores_peer_goals() {
        let (server, root) = test_server();
        register_master(&server);
        register(&server, "worker");
        let now = now_ms();
        let set_due = |goal_due: bool, revision: u64| {
            server.commit(&[crate::server::state::Event::MasterWakeUpdated {
                accumulator: crate::server::state::MasterWakeAccumulator {
                    goal_due,
                    active_goal_revision: Some(revision),
                    last_updated_ms: now,
                    ..Default::default()
                },
            }]);
        };
        let seed_due_goal = |worker: &str, subject: &str| -> String {
            let id = format!("sub-due-{worker}-{subject}");
            let (target, method) = subscription_target(&server, worker);
            server.commit(&[Event::NotificationSubscribed {
                subscription: NotificationSubscription {
                    id: id.clone(),
                    worker_id: worker.into(),
                    event: "deadline".into(),
                    subject: Some(subject.into()),
                    target,
                    method,
                    trigger_ms: Some(now - 1),
                    trigger_times_ms: Vec::new(),
                    interval_ms: None,
                    repeat_count: 1,
                    fired_count: 0,
                    expires_ms: now + 86_400_000,
                    status: "armed".into(),
                    created_ms: now,
                    updated_ms: now,
                    status_reason: None,
                },
            }]);
            id
        };

        set_due(true, 7);
        let due_master = seed_due_goal("master", "goal:due-master");
        let _due_peer = seed_due_goal("worker", "goal:due-peer");
        let ended = super::super::handle_goal_schedule_reconcile(
            &server,
            "master".into(),
            "token-master".into(),
            "due-master".into(),
            "Jenny".into(),
            "ended".into(),
            None,
            None,
            None,
            Some("goal-finished".into()),
        );
        assert!(ended.ok, "{}", ended.error.unwrap_or_default());
        {
            let state = server.state.lock().unwrap();
            assert!(
                !state.master_wake.goal_due,
                "ending the only due master goal must clear the latch despite a due peer goal"
            );
            assert!(state.master_wake.active_goal_revision.is_none());
            assert_eq!(
                state.notification_subscriptions[&due_master].status,
                "cancelled"
            );
        }
        tick_with_idle_at(&server, now + 1, &|_| false);
        {
            let state = server.state.lock().unwrap();
            assert!(
                !state.master_wake.goal_due,
                "a later idle batch must not advertise the ended master goal"
            );
            assert!(state.master_wake.active_goal_revision.is_none());
        }

        // A second still-due master goal keeps the latch raised.
        set_due(true, 9);
        let _other_due = seed_due_goal("master", "goal:other-due");
        let _second_due = seed_due_goal("master", "goal:second-due");
        let ended = super::super::handle_goal_schedule_reconcile(
            &server,
            "master".into(),
            "token-master".into(),
            "other-due".into(),
            "Jenny".into(),
            "ended".into(),
            None,
            None,
            None,
            Some("goal-finished".into()),
        );
        assert!(ended.ok, "{}", ended.error.unwrap_or_default());
        {
            let state = server.state.lock().unwrap();
            assert!(
                state.master_wake.goal_due,
                "another applicable master goal remains due"
            );
            assert_eq!(state.master_wake.active_goal_revision, Some(9));
        }

        // Replacing an already-due occurrence with a future one must clear the
        // latch for the retired occurrence while the unrelated peer goal stays due.
        let ended_second = super::super::handle_goal_schedule_reconcile(
            &server,
            "master".into(),
            "token-master".into(),
            "second-due".into(),
            "Jenny".into(),
            "ended".into(),
            None,
            None,
            None,
            Some("goal-finished".into()),
        );
        assert!(
            ended_second.ok,
            "{}",
            ended_second.error.unwrap_or_default()
        );
        set_due(true, 11);
        let due_rearm = seed_due_goal("master", "goal:rearm-due");
        let rearmed = super::super::handle_goal_schedule_reconcile(
            &server,
            "master".into(),
            "token-master".into(),
            "rearm-due".into(),
            "Jenny".into(),
            "active".into(),
            Some(now + 120_000),
            Some(3600),
            None,
            Some("goal-rearm".into()),
        );
        assert!(rearmed.ok, "{}", rearmed.error.unwrap_or_default());
        {
            let state = server.state.lock().unwrap();
            assert!(
                !state.master_wake.goal_due,
                "rearming the only due master goal must clear the latch despite a due peer goal"
            );
            assert!(state.master_wake.active_goal_revision.is_none());
            assert_eq!(
                state.notification_subscriptions[&due_rearm].status,
                "cancelled"
            );
            let active = state
                .notification_subscriptions
                .values()
                .filter(|subscription| {
                    subscription.worker_id == "master"
                        && is_goal_deadline(subscription)
                        && subscription.subject.as_deref() == Some("goal:rearm-due")
                        && subscription.status == "armed"
                })
                .count();
            assert_eq!(active, 1, "rearm must leave exactly one active occurrence");
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unknown_adjudication_does_not_rearm_cancelled_deadline_or_spend_idle_budget() {
        use crate::server::name_state::{
            AgentActivity, AgentActivityRecord, AgentPresence, NameRecoveryProgress,
        };

        let (server, root) = test_server();
        register_master(&server);
        let deadline = periodic_deadline_subscription(&server, "master", "goal:cancelled-unknown");
        let cancelled = super::super::handle_goal_schedule_reconcile(
            &server,
            "master".into(),
            "token-master".into(),
            "cancelled-unknown".into(),
            "Jenny".into(),
            "ended".into(),
            None,
            None,
            None,
            Some("goal-finished".into()),
        );
        assert!(cancelled.ok, "{cancelled:?}");
        let idle = master_idle_subscription(&server, 15 * 60 * 1000);
        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record: Record {
                observed: "working".into(),
                ..Record::default()
            },
        }]);

        tick_with_idle(&server, &|_| false);
        let adjudication_id = {
            let state = server.state.lock().unwrap();
            let recoveries: Vec<_> = state.name_recoveries.values().collect();
            assert_eq!(recoveries.len(), 1);
            let recovery = recoveries[0];
            assert_eq!(recovery.target_name, "Jenny");
            assert_eq!(
                recovery.progress,
                NameRecoveryProgress::AdjudicationRequested
            );
            assert_eq!(
                state
                    .agent_presence(&recovery.project_scope, "master")
                    .unwrap()
                    .presence,
                AgentPresence::Unknown
            );
            let id = format!("name-adjudication-{}", recovery.operation_id);
            let message = &state.msgs[&id];
            assert_eq!(message.mtype, "notification");
            assert_eq!(message.to, "master");
            assert_eq!(
                message.state, "pending",
                "adapter acceptance is not consumption"
            );
            assert_eq!(state.wake_bindings[&id], "sub-default-direct-message-master");
            assert!(subscription_messages(&state, &deadline).is_empty());
            assert!(subscription_messages(&state, &idle).is_empty());
            assert_eq!(state.keepalives["master"].idle_episode_notices, 0);
            id
        };

        // Keepalive reads current typed activity. A bare KeepaliveUpdated
        // cannot override the monitor's Unknown activity on the next tick.
        // Presence adjudication remains pending while activity is now idle.
        let activity = {
            let state = server.state.lock().unwrap();
            let recovery = state.name_recoveries.values().next().unwrap();
            AgentActivityRecord {
                project_scope: recovery.project_scope.clone(),
                worker_id: "master".into(),
                session_id: recovery.occupancy_ref.session_id.clone(),
                activity: AgentActivity::Idle,
                background: false,
                reason: "controlled current idle observation after unknown".into(),
                observed_ms: now_ms(),
            }
        };
        let idle_at = activity.observed_ms + 1;
        server.commit(&[Event::AgentActivityObserved { activity }]);
        tick_with_idle_at(&server, idle_at, &|_| false);
        let idle_message = {
            let state = server.state.lock().unwrap();
            let messages = subscription_messages(&state, &idle);
            assert_eq!(
                messages.len(), 1,
                "adjudication must leave the idle episode budget available"
            );
            assert_eq!(state.keepalives["master"].idle_episode_notices, 1);
            messages[0].id.clone()
        };
        server.commit(&[Event::NotificationConsumed {
            subscription_id: idle.clone(),
            message_id: idle_message,
            consumed_ms: idle_at + 1,
        }]);
        tick_with_idle_at(&server, idle_at + 2, &|_| false);
        tick_with_idle_at(&server, idle_at + 3, &|_| false);
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.msgs.len(), 2,
            "one adjudication and one idle occurrence only"
        );
        assert_eq!(state.msgs[&adjudication_id].state, "pending");
        assert_eq!(state.name_recoveries.len(), 1);
        assert_eq!(state.keepalives["master"].idle_episode_notices, 1);
        assert_eq!(subscription_messages(&state, &idle).len(), 1);
        assert_eq!(state.notification_subscriptions[&idle].fired_count, 1);
        assert!(subscription_messages(&state, &deadline).is_empty());
        assert_eq!(state.notification_subscriptions[&deadline].status, "cancelled");
        assert_eq!(state.notification_subscriptions[&deadline].fired_count, 0);
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cancelled_master_idle_subscription_stays_cancelled_after_read_and_replay() {
        let (server, root) = test_server();
        register_master(&server);
        let subscription_id = master_idle_subscription(&server, 15 * 60 * 1000);
        let created_ms =
            server.state.lock().unwrap().notification_subscriptions[&subscription_id].created_ms;
        let mut record = server.state.lock().unwrap().keepalives["master"].clone();
        record.idle_since_ms = created_ms + 1;
        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record,
        }]);

        tick_with_idle(&server, &|_| false);
        let message_id = server
            .state
            .lock()
            .unwrap()
            .wake_bindings
            .iter()
            .find_map(|(message_id, bound)| {
                (bound == &subscription_id).then_some(message_id.clone())
            })
            .expect("master idle wake");
        server.commit(&[Event::NotificationSuppressed {
            subscription_id: subscription_id.clone(),
            status: "cancelled".into(),
            reason: "historical-explicit-unsubscribe".into(),
            updated_ms: now_ms(),
        }]);

        server.commit(&[
            Event::Delivered {
                ids: vec![message_id.clone()],
            },
            Event::Acked {
                ids: vec![message_id.clone()],
            },
        ]);
        tick_with_idle(&server, &|_| false);

        let state = server.state.lock().unwrap();
        let subscription = &state.notification_subscriptions[&subscription_id];
        assert_eq!(subscription.status, "cancelled");
        assert_eq!(subscription.fired_count, 0);
        assert_eq!(
            state
                .wake_bindings
                .values()
                .filter(|bound| *bound == &subscription_id)
                .count(),
            1,
            "a cancelled master idle subscription cannot create another wake"
        );
        let snapshot = state.snapshot_events();
        drop(state);

        let (replayed, replay_root) = test_server();
        for event in &snapshot {
            replayed.commit(std::slice::from_ref(event));
        }
        tick_with_idle(&replayed, &|_| false);
        let replayed_state = replayed.state.lock().unwrap();
        assert_eq!(
            replayed_state.notification_subscriptions[&subscription_id].status,
            "cancelled"
        );
        assert_eq!(
            replayed_state
                .wake_bindings
                .values()
                .filter(|bound| *bound == &subscription_id)
                .count(),
            1,
            "cancellation must survive replay without re-arming the timer"
        );
        drop(replayed_state);
        std::fs::remove_dir_all(replay_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn master_idle_subscription_emits_at_15_minutes_only_for_live_idle_master() {
        let (server, root) = test_server();
        register_master(&server);
        let subscription_id = master_idle_subscription(&server, 15 * 60 * 1000);
        let created_ms =
            server.state.lock().unwrap().notification_subscriptions[&subscription_id].created_ms;
        let mut record = server.state.lock().unwrap().keepalives["master"].clone();
        record.idle_since_ms = created_ms + 1;
        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record,
        }]);

        tick_with_idle(&server, &|_| false);
        tick_with_idle(&server, &|_| false);

        let state = server.state.lock().unwrap();
        assert_eq!(
            state
                .wake_bindings
                .values()
                .filter(|bound| *bound == &subscription_id)
                .count(),
            1
        );
        let message_id = state
            .wake_bindings
            .iter()
            .find_map(|(message_id, bound)| (bound == &subscription_id).then_some(message_id))
            .unwrap();
        assert_eq!(state.msgs[message_id].to, "master");
        assert!(state.msgs[message_id].body.contains("MASTER_IDLE_WAKE"));
        assert!(state.msgs[message_id]
            .body
            .contains("Do not rearm this finite master-idle schedule"));
        assert!(
            !state.msgs[message_id].body.contains("notify unsubscribe"),
            "idle timer must not ask the agent to close base reception: {}",
            state.msgs[message_id].body
        );
        assert!(
            state.msgs[message_id].body.contains("appsdk goal cancel"),
            "idle timer must name the finite goal lifecycle owner: {}",
            state.msgs[message_id].body
        );
        assert!(
            state.msgs[message_id]
                .body
                .contains("Direct-message reception is daemon-owned"),
            "idle timer must keep daemon reception explicit: {}",
            state.msgs[message_id].body
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn master_idle_subscription_accepts_60_minute_interval_and_wakes_master() {
        let (server, root) = test_server();
        register_master(&server);
        let subscription_id = master_idle_subscription(&server, 60 * 60 * 1000);
        let created_ms =
            server.state.lock().unwrap().notification_subscriptions[&subscription_id].created_ms;
        let mut record = server.state.lock().unwrap().keepalives["master"].clone();
        record.idle_since_ms = created_ms + 1;
        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record,
        }]);

        tick_with_idle(&server, &|_| false);
        let state = server.state.lock().unwrap();
        let master_wakes = state
            .wake_bindings
            .values()
            .filter(|bound| *bound == &subscription_id)
            .count();
        assert_eq!(master_wakes, 1, "60-minute master wake must be non-vacuous");
        assert!(subscription_messages(&state, &subscription_id)
            .iter()
            .all(|message| message.to == "master"));
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn master_idle_wake_waits_for_actionable_tasks_to_clear() {
        let (server, root) = test_server();
        register_master(&server);
        let subscription_id = master_idle_subscription(&server, 15 * 60 * 1000);
        working_task(&server, "master");

        tick_with_idle(&server, &|_| false);

        let state = server.state.lock().unwrap();
        assert!(state
            .wake_bindings
            .values()
            .all(|bound| bound != &subscription_id));
        assert!(subscription_messages(&state, &subscription_id).is_empty());
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn master_idle_wake_requires_observed_idle_state() {
        let (server, root) = test_server();
        register_master(&server);
        let subscription_id = master_idle_subscription(&server, 15 * 60 * 1000);

        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record: Record {
                observed: "working".into(),
                idle_since_ms: 0,
                ..Record::default()
            },
        }]);
        tick_with_idle(&server, &|_| false);

        let state = server.state.lock().unwrap();
        assert!(state
            .wake_bindings
            .values()
            .all(|bound| bound != &subscription_id));
        assert!(subscription_messages(&state, &subscription_id).is_empty());
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn overdue_master_idle_cursor_does_not_catch_up_after_consumption() {
        let (server, root) = test_server();
        register_master(&server);
        let subscription_id = master_idle_subscription(&server, 15 * 60 * 1000);
        let now = now_ms();
        server
            .state
            .lock()
            .unwrap()
            .notification_subscriptions
            .get_mut(&subscription_id)
            .unwrap()
            .trigger_ms = Some(now - 15 * 60 * 1000 - 1);
        let mut record = server.state.lock().unwrap().keepalives["master"].clone();
        record.idle_since_ms = now.saturating_add(1);
        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record,
        }]);

        tick_with_idle(&server, &|_| false);
        let first_message_id = server
            .state
            .lock()
            .unwrap()
            .wake_bindings
            .iter()
            .find_map(|(message_id, bound)| {
                (bound == &subscription_id).then_some(message_id.clone())
            })
            .expect("first master idle wake");
        let attempted_at = now_ms();
        server.commit(&[
            Event::WakeAttempted {
                ids: vec![first_message_id.clone()],
                attempted_ms: attempted_at,
                retry: false,
            },
            Event::Delivered {
                ids: vec![first_message_id.clone()],
            },
            Event::Acked {
                ids: vec![first_message_id],
            },
        ]);

        tick_with_idle(&server, &|_| false);
        let state = server.state.lock().unwrap();
        assert_eq!(
            state
                .wake_bindings
                .values()
                .filter(|bound| *bound == &subscription_id)
                .count(),
            1,
            "an overdue cursor must wait for the next interval after consumption"
        );
        assert_eq!(
            state.notification_subscriptions[&subscription_id].fired_count,
            1
        );
        assert_eq!(
            state.notification_subscriptions[&subscription_id].trigger_ms,
            Some(attempted_at + 15 * 60 * 1000)
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn master_idle_timer_respects_keepalive_episode_notice_budget() {
        let (server, root) = test_server();
        register_master(&server);
        let subscription_id = master_idle_subscription(&server, 15 * 60 * 1000);
        let mut record = server.state.lock().unwrap().keepalives["master"].clone();
        record.idle_episode_notices = 3;
        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record,
        }]);

        tick_with_idle(&server, &|_| false);

        let state = server.state.lock().unwrap();
        assert!(
            subscription_messages(&state, &subscription_id).is_empty(),
            "three keepalive notices stop timer wakes"
        );
        assert_eq!(
            state.notification_subscriptions[&subscription_id].fired_count,
            0
        );
        assert_eq!(state.keepalives["master"].idle_episode_notices, 3);
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn master_idle_timer_counts_its_wake_in_keepalive_episode_gate() {
        let (server, root) = test_server();
        register_master(&server);
        let subscription_id = master_idle_subscription(&server, 15 * 60 * 1000);
        let created_ms =
            server.state.lock().unwrap().notification_subscriptions[&subscription_id].created_ms;
        let mut record = server.state.lock().unwrap().keepalives["master"].clone();
        record.idle_since_ms = created_ms + 1;
        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record,
        }]);

        tick_with_idle(&server, &|_| false);

        let state = server.state.lock().unwrap();
        assert_eq!(subscription_messages(&state, &subscription_id).len(), 1);
        assert_eq!(state.keepalives["master"].idle_episode_notices, 1);
        assert_eq!(
            state
                .wake_bindings
                .values()
                .filter(|bound| *bound == &subscription_id)
                .count(),
            1
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn master_idle_timer_stops_after_keepalive_working_to_waiting() {
        let (server, root) = test_server();
        register_master(&server);
        let subscription_id = master_idle_subscription(&server, 15 * 60 * 1000);
        let now = now_ms();
        let created_ms =
            server.state.lock().unwrap().notification_subscriptions[&subscription_id].created_ms;
        server
            .state
            .lock()
            .unwrap()
            .notification_subscriptions
            .get_mut(&subscription_id)
            .unwrap()
            .trigger_ms = Some(now - 15 * 60 * 1000 - 1);
        let mut record = server.state.lock().unwrap().keepalives["master"].clone();
        record.idle_since_ms = created_ms + 1;
        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record,
        }]);

        tick_with_idle(&server, &|_| false);
        let first_message_id = server
            .state
            .lock()
            .unwrap()
            .wake_bindings
            .iter()
            .find_map(|(message_id, bound)| {
                (bound == &subscription_id).then_some(message_id.clone())
            })
            .expect("first master idle timer wake");
        server.commit(&[
            Event::Delivered {
                ids: vec![first_message_id.clone()],
            },
            Event::NotificationConsumed {
                subscription_id: subscription_id.clone(),
                message_id: first_message_id,
                consumed_ms: now_ms(),
            },
        ]);

        let working_at = now_ms();
        super::super::keepalive::tick_at(&server, working_at);
        super::super::keepalive::tick_at(&server, working_at + 1);
        tick_with_idle(&server, &|_| false);

        let state = server.state.lock().unwrap();
        assert_eq!(
            state
                .wake_bindings
                .values()
                .filter(|bound| *bound == &subscription_id)
                .count(),
            1,
            "Working -> Waiting must not reopen the consumed timer occurrence"
        );
        assert_eq!(subscription_messages(&state, &subscription_id).len(), 1);
        assert_eq!(state.keepalives["master"].idle_episode_notices, 1);
        assert!(!state.keepalives["master"].idle_episode_stopped);
        assert_eq!(
            state.notification_subscriptions[&subscription_id].fired_count,
            1
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn master_idle_timer_and_keepalive_share_episode_notice_budget() {
        let (server, root) = test_server();
        register_master(&server);
        let timer_subscription_id = master_idle_subscription(&server, 15 * 60 * 1000);
        let now = now_ms();
        let mut record = server.state.lock().unwrap().keepalives["master"].clone();
        record.idle_since_ms = now + 1;
        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record,
        }]);
        server
            .state
            .lock()
            .unwrap()
            .notification_subscriptions
            .get_mut(&timer_subscription_id)
            .unwrap()
            .trigger_ms = Some(now - 15 * 60 * 1000 - 1);

        tick_with_idle(&server, &|_| false);
        let timer_message_id = server
            .state
            .lock()
            .unwrap()
            .wake_bindings
            .iter()
            .find_map(|(message_id, bound)| {
                (bound == &timer_subscription_id).then_some(message_id.clone())
            })
            .expect("timer consumes the shared notice");
        {
            let state = server.state.lock().unwrap();
            assert_eq!(state.keepalives["master"].idle_episode_notices, 1);
            assert_eq!(subscription_messages(&state, &timer_subscription_id).len(), 1);
        }
        server.commit(&[
            Event::Delivered {
                ids: vec![timer_message_id.clone()],
            },
            Event::NotificationConsumed {
                subscription_id: timer_subscription_id.clone(),
                message_id: timer_message_id,
                consumed_ms: now_ms(),
            },
        ]);

        tick_with_idle(&server, &|_| false);
        let state = server.state.lock().unwrap();
        assert_eq!(
            state
                .wake_bindings
                .values()
                .filter(|bound| *bound == &timer_subscription_id)
                .count(),
            1,
            "timer must stop after its consumed occurrence"
        );
        assert_eq!(state.keepalives["master"].idle_episode_notices, 1);
        assert_eq!(
            state.notification_subscriptions[&timer_subscription_id].fired_count,
            1
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn master_idle_timer_does_not_wake_an_episode_seen_before_subscription() {
        let (server, root) = test_server();
        register_master(&server);
        let subscription_id = master_idle_subscription(&server, 15 * 60 * 1000);
        let created_ms =
            server.state.lock().unwrap().notification_subscriptions[&subscription_id].created_ms;
        let mut record = server.state.lock().unwrap().keepalives["master"].clone();
        record.idle_since_ms = created_ms - 1;
        record.idle_episode_notices = 1;
        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record,
        }]);

        tick_with_idle(&server, &|_| false);

        let state = server.state.lock().unwrap();
        assert!(subscription_messages(&state, &subscription_id).is_empty());
        assert_eq!(
            state.notification_subscriptions[&subscription_id].fired_count,
            0
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn wait_expiry_without_live_master_does_not_fabricate_a_recipient() {
        let (server, root) = test_server();
        register(&server, "waiter");
        working_task(&server, "waiter");
        let now = now_ms();
        let mut task = server.state.lock().unwrap().tasks["task"].clone();
        task.status = "waiting".into();
        task.wait = Some(WaitSpec {
            waiter: "waiter".into(),
            waiting_for: "holder".into(),
            responsible_actor: "holder-owner".into(),
            reason: "resource_conflict".into(),
            deadline_ms: now - 1,
            resume_on: vec!["resource_released".into()],
            escalation: "resource_owner_and_waiter_recheck".into(),
        });
        server.commit(&[Event::TaskUpdated { task }]);
        tick_with_idle(&server, &|_| false);
        let state = server.state.lock().unwrap();
        assert_eq!(state.tasks["task"].status, "blocked");
        // There is no live owner for a scheduling reason, so the timeout is
        // still durable in the blocked task and does not invent a mailbox
        // recipient or wake the waiting worker.
        assert!(state.tasks["task"]
            .next_step
            .as_deref()
            .unwrap()
            .contains("reason=resource_conflict"));
        assert!(state.msgs.is_empty());
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn wait_timeout_without_direct_subscription_stays_in_master_mailbox() {
        let (server, root) = test_server();
        register_master(&server);
        register(&server, "waiter");
        {
            let mut state = server.state.lock().unwrap();
            state
                .notification_subscriptions
                .remove("sub-default-direct-message-master");
            // Without a selected transport the daemon has no address to
            // reconcile the daemon-owned lease to. This is the remaining
            // mailbox-only case; a registered, reachable master now receives
            // a refreshed default lease from ordinary maintenance.
            state.workers.get_mut("master").unwrap().transport = None;
        }
        working_task(&server, "waiter");
        let now = now_ms();
        let mut task = server.state.lock().unwrap().tasks["task"].clone();
        task.status = "waiting".into();
        task.wait = Some(WaitSpec {
            waiter: "waiter".into(),
            waiting_for: "holder".into(),
            responsible_actor: "holder-owner".into(),
            reason: "resource_conflict".into(),
            deadline_ms: now - 1,
            resume_on: vec!["resource_released".into()],
            escalation: "resource_owner_and_waiter_recheck".into(),
        });
        server.commit(&[Event::TaskUpdated { task }]);

        tick_with_idle(&server, &|_| false);
        tick_with_idle(&server, &|_| false);
        let state = server.state.lock().unwrap();
        assert_eq!(state.tasks["task"].status, "blocked");
        let messages = wait_timeout_messages(&state);
        assert_eq!(messages.len(), 1);
        let message = messages[0];
        assert_eq!(message.to, "master");
        assert_eq!(message.state, "pending");
        assert!(!state.wake_bindings.contains_key(&message.id));
        assert!(!state.msgs.values().any(|message| message.to == "waiter"));
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn wait_timeout_reason_is_durable_and_visible_only_to_live_master() {
        let (mut server, root) = test_server();
        Arc::get_mut(&mut server).unwrap().config.notifications.mode = "immediate".into();
        register_master(&server);
        register(&server, "waiter");
        working_task(&server, "waiter");
        let subscription_id = "sub-default-direct-message-master".to_owned();
        let now = now_ms();
        let mut task = server.state.lock().unwrap().tasks["task"].clone();
        task.status = "waiting".into();
        task.wait = Some(WaitSpec {
            waiter: "waiter".into(),
            waiting_for: "holder".into(),
            responsible_actor: "holder-owner".into(),
            reason: "resource_conflict".into(),
            deadline_ms: now - 1,
            resume_on: vec!["resource_released".into()],
            escalation: "resource_owner_and_waiter_recheck".into(),
        });
        server.commit(&[Event::TaskUpdated { task }]);

        // A timeout is recorded while the live master is working. Its reason
        // stays pending in the durable mailbox and no worker is notified.
        tick_with_idle(&server, &|_| false);
        let (message_id, message) = {
            let state = server.state.lock().unwrap();
            assert_eq!(state.tasks["task"].status, "blocked");
            assert_eq!(
                state.tasks["task"].next_step.as_deref(),
                Some(
                    "WAIT_TIMEOUT waiting_for=holder responsible_actor=holder-owner reason=resource_conflict escalation=resource_owner_and_waiter_recheck"
                )
            );
            let messages = wait_timeout_messages(&state);
            assert_eq!(messages.len(), 1);
            let message = messages[0];
            let message_id = &message.id;
            assert_eq!(message.to, "master");
            assert_eq!(message.subject.as_deref(), Some("wait-timeout:task"));
            assert!(message.body.contains("reason=resource_conflict"));
            assert!(message.body.contains("WAIT_TIMEOUT"));
            assert_eq!(message.state, "pending");
            assert_eq!(message.wake_attempt_count, 1);
            assert_eq!(state.wake_bindings.get(message_id), Some(&subscription_id));
            assert!(!state.msgs.values().any(|message| message.to == "waiter"));
            (message_id.clone(), message.clone())
        };

        // A repeated tick observes the blocked task and cannot create a second
        // scheduling reason or a second wake attempt during the retry window.
        tick_with_idle(&server, &|_| false);
        assert_eq!(wait_timeout_messages(&server.state.lock().unwrap()).len(), 1);
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[&message_id].to, message.to);
        assert_eq!(state.msgs[&message_id].subject, message.subject);
        assert_eq!(state.msgs[&message_id].body, message.body);
        assert_eq!(state.msgs[&message_id].state, "pending");
        assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
        assert_eq!(wait_timeout_messages(&state).len(), 1);
        drop(state);

        // Snapshot replay retains the blocked task and its single mailbox
        // reason; another timer tick still has no timeout transition to emit.
        let snapshot = server.state.lock().unwrap().snapshot_events();
        let (mut replayed, replay_root) = test_server();
        for event in &snapshot {
            replayed.commit(std::slice::from_ref(event));
        }
        tick_with_idle(&replayed, &|_| false);
        let replayed_state = replayed.state.lock().unwrap();
        assert_eq!(replayed_state.tasks["task"].status, "blocked");
        assert_eq!(wait_timeout_messages(&replayed_state).len(), 1);
        assert_eq!(replayed_state.msgs[&message_id].to, "master");
        drop(replayed_state);
        std::fs::remove_dir_all(replay_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unacked_notifications_limit_pauses_wakes_and_resumes_after_ack() {
        let (mut server, root) = test_server();
        Arc::get_mut(&mut server).unwrap().config.notifications.mode = "immediate".into();
        register(&server, "ack-worker");
        let sub = subscribe(&server, "ack-worker", "direct-message", None, None);

        // Deliver 3 notifications (max_unacked = 3).
        let mut msg_ids = Vec::new();
        for i in 0..3 {
            let id = bind_message_with_id(&server, "ack-worker", &sub, &format!("msg-ack-{i}"));
            assert!(super::super::attempt_notification_with_default(
                &server,
                &id,
                &sub,
                &|_| true,
                &|_, _| true,
            ));
            server.commit(&[Event::Delivered {
                ids: vec![id.clone()],
            }]);
            msg_ids.push(id);
        }

        // 3 messages are now delivered and unacked.
        assert_eq!(
            server
                .state
                .lock()
                .unwrap()
                .msgs
                .values()
                .filter(|m| m.to == "ack-worker" && m.state == "delivered")
                .count(),
            3
        );

        let id4 = bind_message_with_id(&server, "ack-worker", &sub, "msg-ack-4");

        assert!(!super::super::attempt_notification_with_default(
            &server,
            &id4,
            &sub,
            &|_| true,
            &|_, _| panic!("unacked notification limit must defer delivery"),
        ));
        assert_eq!(
            server.state.lock().unwrap().msgs[&id4].wake_attempt_count,
            0
        );

        server.commit(&[Event::Acked { ids: msg_ids }]);
        assert_eq!(
            server
                .state
                .lock()
                .unwrap()
                .msgs
                .values()
                .filter(|m| m.to == "ack-worker" && m.state == "delivered")
                .count(),
            0
        );

        assert!(super::super::attempt_notification_with_default(
            &server,
            &id4,
            &sub,
            &|_| true,
            &|_, _| true,
        ));
        assert_eq!(
            server.state.lock().unwrap().msgs[&id4].wake_attempt_count,
            1
        );

        assert!(!super::super::attempt_notification_with_default(
            &server,
            &id4,
            &sub,
            &|_| true,
            &|_, _| panic!("already delivered message must not replay"),
        ));
        assert_eq!(
            server.state.lock().unwrap().msgs[&id4].wake_attempt_count,
            1
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn immediate_automatic_delivery_resumes_after_max_unacked_ack() {
        let (mut server, root) = test_server();
        Arc::get_mut(&mut server).unwrap().config.notifications.mode = "immediate".into();
        let submitted = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        {
            let submitted = Arc::clone(&submitted);
            Arc::get_mut(&mut server).unwrap().appserver_notification_sink =
                Arc::new(move |_transport, _, _, message_id, _, _mode| {
                    submitted.lock().unwrap().push(message_id.to_string());
                    Ok(serde_json::json!({"accepted": true}))
                });
        }
        let server = Arc::new(server);
        register(&server, "recovered-peer");
        let subscription_id = subscribe(&server, "recovered-peer", "direct-message", None, None);

        // Saturate the ACK safety limit with three distinct delivered rows.
        let mut delivered_ids = Vec::new();
        for index in 0..3 {
            let id = bind_message_with_id(
                &server,
                "recovered-peer",
                &subscription_id,
                &format!("msg-unacked-{index}"),
            );
            assert!(super::super::attempt_notification_with_default(
                &server,
                &id,
                &subscription_id,
                &|_| true,
                &|_, _| true,
            ));
            server.commit(&[Event::Delivered { ids: vec![id.clone()] }]);
            delivered_ids.push(id);
        }

        // The recovered immediate report has persisted immediate mode and zero
        // attempt facts, like a daemon-restored pending direct message.
        let recovered_id = bind_message_with_id(
            &server,
            "recovered-peer",
            &subscription_id,
            "msg-recovered-immediate",
        );
        server.commit(&[Event::DeliveryMode {
            msg_id: recovered_id.clone(),
            mode: "immediate".into(),
            source_thread_id: None,
        }]);

        // The first automatic pass is a pre-I/O admission deferral: it records
        // the retryable not_attempted failure but no wake attempt.
        let now = now_ms();
        tick_with_idle_at(&server, now, &|_| true);
        {
            let state = server.state.lock().unwrap();
            let message = &state.msgs[&recovered_id];
            assert_eq!(message.wake_attempt_count, 0, "safety deferral must not claim an attempt");
            assert_eq!(message.last_wake_attempt_ms, 0);
            assert_eq!(
                state.notification_delivery_failures[&recovered_id].operation,
                "notification.not_attempted"
            );
            assert!(state.notification_delivery_failures[&recovered_id].retryable);
            assert!(!state.notification_delivery_accepted.contains_key(&recovered_id));
        }
        assert!(submitted.lock().unwrap().is_empty());

        // ACK clears the safety limit. The zero-attempt message must stay
        // eligible for the same automatic attempt owner.
        server.commit(&[Event::Acked { ids: delivered_ids }]);
        assert_eq!(
            server
                .state
                .lock()
                .unwrap()
                .msgs
                .values()
                .filter(|message| message.to == "recovered-peer" && message.state == "delivered")
                .count(),
            0
        );

        tick_with_idle_at(&server, now + 1, &|_| true);
        assert_eq!(
            submitted.lock().unwrap().as_slice(),
            [recovered_id.as_str()],
            "the never-attempted immediate message must resume after ACK"
        );
        {
            let state = server.state.lock().unwrap();
            assert_eq!(state.msgs[&recovered_id].wake_attempt_count, 1);
            assert!(state.notification_delivery_accepted.contains_key(&recovered_id));
        }

        // Accepted delivery is never resubmitted by a later tick.
        tick_with_idle_at(&server, now + 2, &|_| true);
        assert_eq!(submitted.lock().unwrap().len(), 1);
        assert_eq!(
            server.state.lock().unwrap().msgs[&recovered_id].wake_attempt_count,
            1
        );

        drop(server);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn backlog_sends_latest_three_once_and_truncates_to_1024_chars() {
        let (server, root) = test_server();
        register(&server, "batch-worker");
        let sub = subscribe(&server, "batch-worker", "direct-message", None, None);

        // Queue 5 messages with unique IDs for batch-worker
        for i in 0..5 {
            let _ = bind_message_with_id(&server, "batch-worker", &sub, &format!("msg-batch-{i}"));
        }
        server
            .state
            .lock()
            .unwrap()
            .msgs
            .get_mut("msg-batch-4")
            .unwrap()
            .body = "界".repeat(2_000);

        let first_id = "msg-batch-0";
        let delivered_text = std::sync::Mutex::new(String::new());
        assert!(super::super::attempt_notification_with_default(
            &server,
            first_id,
            &sub,
            &|_| true,
            &|_, text| {
                *delivered_text.lock().unwrap() = text.to_string();
                true
            },
        ));

        let text = delivered_text.lock().unwrap().clone();
        assert!(text.chars().count() <= 1024);
        assert!(!text.contains("msg-batch-0"));
        assert!(!text.contains("msg-batch-1"));
        assert!(text.contains("msg-batch-2"));
        assert!(text.contains("msg-batch-3"));
        assert!(text.contains("msg-batch-4"));
        assert!(text.contains("message_ids="));
        assert!(text.contains("entries=msg-batch-2:"));
        assert!(text.contains("msg-batch-3:"));
        assert!(text.contains("msg-batch-4:"));
        assert!(!text.contains("task_ids"), "{text}");
        assert!(!text.contains("collab inbox"), "{text}");

        let state = server.state.lock().unwrap();
        let queued_count = state
            .msgs
            .values()
            .filter(|m| m.to == "batch-worker" && m.state == "pending")
            .count();
        assert_eq!(queued_count, 5);
        assert_eq!(state.msgs["msg-batch-0"].state, "pending");
        assert_eq!(state.msgs["msg-batch-1"].state, "pending");
        assert_eq!(state.msgs["msg-batch-0"].wake_attempt_count, 0);
        assert_eq!(state.msgs["msg-batch-1"].wake_attempt_count, 0);
        assert_eq!(state.msgs["msg-batch-2"].state, "pending");
        assert_eq!(state.msgs["msg-batch-3"].state, "pending");
        assert_eq!(state.msgs["msg-batch-4"].state, "pending");
        assert_eq!(state.msgs["msg-batch-2"].wake_attempt_count, 1);
        assert_eq!(state.msgs["msg-batch-3"].wake_attempt_count, 1);
        assert_eq!(state.msgs["msg-batch-4"].wake_attempt_count, 1);
        drop(state);
        assert!(!super::super::attempt_notification_with_default(
            &server,
            "msg-batch-0",
            &sub,
            &|_| true,
            &|_, _| panic!("older backlog must not be pushed later"),
        ));
        std::fs::remove_dir_all(root).ok();
    }

    // The pause hook is process-global, so the two interleaving tests that use
    // it run serially. Each hook names its own subscription to avoid pausing
    // unrelated timer tests that run concurrently.
    static DUE_BATCH_RACE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct DueBatchPause {
        entered: bool,
        release: bool,
    }

    fn install_due_batch_pause(
        subscription_id: &str,
    ) -> Arc<(std::sync::Mutex<DueBatchPause>, std::sync::Condvar)> {
        let pause = Arc::new((
            std::sync::Mutex::new(DueBatchPause {
                entered: false,
                release: false,
            }),
            std::sync::Condvar::new(),
        ));
        let hook_pause = Arc::clone(&pause);
        *super::TIMER_DUE_COMMIT_PAUSE.lock().unwrap() = Some((
            subscription_id.to_owned(),
            Box::new(move || {
                let (lock, cv) = &*hook_pause;
                let mut state = lock.lock().unwrap();
                state.entered = true;
                cv.notify_all();
                while !state.release {
                    let (guard, timed_out) = cv
                        .wait_timeout(state, std::time::Duration::from_secs(15))
                        .unwrap();
                    state = guard;
                    if timed_out.timed_out() {
                        break;
                    }
                }
            }),
        ));
        pause
    }

    fn wait_for_due_batch_pause(
        pause: &Arc<(std::sync::Mutex<DueBatchPause>, std::sync::Condvar)>,
    ) {
        let (lock, cv) = &**pause;
        let mut state = lock.lock().unwrap();
        while !state.entered {
            let (guard, _) = cv
                .wait_timeout(state, std::time::Duration::from_secs(10))
                .unwrap();
            state = guard;
            assert!(state.entered, "timer must reach the due-batch pause");
        }
    }

    fn release_due_batch_pause(
        pause: &Arc<(std::sync::Mutex<DueBatchPause>, std::sync::Condvar)>,
    ) {
        let (lock, cv) = &**pause;
        lock.lock().unwrap().release = true;
        cv.notify_all();
    }

    /// Run one timer tick paused before its due commit, then run the goal
    /// reconcile in the only order today's state lock permits. A serialized
    /// commit lands first and the reconcile supersedes it; the old two-phase
    /// commit released the guard, so the reconcile lands first and the stale
    /// batch arrives afterwards.
    fn tick_with_paused_due_batch(
        server: &Arc<Server>,
        now: i64,
        subscription_id: &str,
        mut reconcile: impl FnMut(),
    ) {
        let pause = install_due_batch_pause(subscription_id);
        let tick_server = Arc::clone(server);
        let tick = std::thread::spawn(move || {
            tick_with_idle_at(&tick_server, now, &|_| false);
        });
        wait_for_due_batch_pause(&pause);
        let commit_serialized = server.state.try_lock().is_err();
        if !commit_serialized {
            reconcile();
        }
        release_due_batch_pause(&pause);
        tick.join().expect("timer tick must finish");
        *super::TIMER_DUE_COMMIT_PAUSE.lock().unwrap() = None;
        if commit_serialized {
            reconcile();
        }
    }

    #[test]
    fn goal_reconcile_ended_receipt_survives_concurrent_timer_due_batch() {
        let _race = DUE_BATCH_RACE_LOCK.lock().unwrap();
        let (server, root) = test_server();
        register_master(&server);
        let now = now_ms();
        let (target, method) = subscription_target(&server, "master");
        let subscription_id = "sub-goal-ended-race".to_string();
        server.commit(&[Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: subscription_id.clone(),
                worker_id: "master".into(),
                event: "deadline".into(),
                subject: Some("goal:ended-race".into()),
                target,
                method,
                trigger_ms: Some(now - 1),
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                fired_count: 0,
                expires_ms: now + 86_400_000,
                status: "armed".into(),
                created_ms: now,
                updated_ms: now,
                status_reason: None,
            },
        }]);
        server.commit(&[Event::MasterWakeUpdated {
            accumulator: crate::server::state::MasterWakeAccumulator {
                goal_due: true,
                active_goal_revision: Some(5),
                last_updated_ms: now,
                ..Default::default()
            },
        }]);

        tick_with_paused_due_batch(&server, now + 1, &subscription_id, || {
            let ended = super::super::handle_goal_schedule_reconcile(
                &server,
                "master".into(),
                "token-master".into(),
                "ended-race".into(),
                "Jenny".into(),
                "ended".into(),
                None,
                None,
                None,
                Some("goal-finished".into()),
            );
            assert!(ended.ok, "{}", ended.error.unwrap_or_default());
        });

        let state = server.state.lock().unwrap();
        let subscription = &state.notification_subscriptions[&subscription_id];
        assert_eq!(subscription.status, "cancelled");
        let due_messages = state
            .wake_bindings
            .iter()
            .filter_map(|(message_id, bound)| {
                let message = state.msgs.get(message_id)?;
                (message.to == "master" && bound == &subscription_id).then(|| message_id.clone())
            })
            .collect::<Vec<_>>();
        assert!(
            !due_messages
                .iter()
                .any(|id| state.msgs[id].state == "pending"),
            "a late timer batch must not leave a pending scheduler message after an ended receipt: {due_messages:?}"
        );
        assert!(
            !state.master_wake.goal_due,
            "an ended receipt must stay terminal against a late timer batch"
        );
        assert!(state.master_wake.active_goal_revision.is_none());
        drop(state);

        tick_with_idle_at(&server, now + 2, &|_| false);
        let state = server.state.lock().unwrap();
        assert!(
            state
                .msgs
                .values()
                .filter(|message| message.to == "master" && message.state == "pending")
                .all(|message| !state
                    .wake_bindings
                    .get(&message.id)
                    .is_some_and(|bound| bound == &subscription_id)),
            "a repeated tick must not re-emit the ended occurrence"
        );
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn goal_reconcile_rearm_receipt_survives_concurrent_timer_due_batch() {
        let _race = DUE_BATCH_RACE_LOCK.lock().unwrap();
        let (server, root) = test_server();
        register_master(&server);
        let now = now_ms();
        let (target, method) = subscription_target(&server, "master");
        let subscription_id = "sub-goal-rearm-race".to_string();
        server.commit(&[Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: subscription_id.clone(),
                worker_id: "master".into(),
                event: "deadline".into(),
                subject: Some("goal:rearm-race".into()),
                target: target.clone(),
                method: method.clone(),
                trigger_ms: Some(now - 1),
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                fired_count: 0,
                expires_ms: now + 86_400_000,
                status: "armed".into(),
                created_ms: now,
                updated_ms: now,
                status_reason: None,
            },
        }]);
        server.commit(&[Event::MasterWakeUpdated {
            accumulator: crate::server::state::MasterWakeAccumulator {
                goal_due: true,
                active_goal_revision: Some(7),
                last_updated_ms: now,
                ..Default::default()
            },
        }]);

        tick_with_paused_due_batch(&server, now + 1, &subscription_id, || {
            let rearmed = super::super::handle_goal_schedule_reconcile(
                &server,
                "master".into(),
                "token-master".into(),
                "rearm-race".into(),
                "Jenny".into(),
                "active".into(),
                Some(now + 120_000),
                Some(3600),
                None,
                Some("goal-rearm".into()),
            );
            assert!(rearmed.ok, "{}", rearmed.error.unwrap_or_default());
        });

        let state = server.state.lock().unwrap();
        assert_eq!(
            state.notification_subscriptions[&subscription_id].status,
            "cancelled",
            "the replaced occurrence must be retired"
        );
        let active = state
            .notification_subscriptions
            .values()
            .filter(|subscription| {
                subscription.worker_id == "master"
                    && is_goal_deadline(subscription)
                    && subscription.subject.as_deref() == Some("goal:rearm-race")
                    && subscription.status == "armed"
            })
            .count();
        assert_eq!(active, 1, "rearm must leave exactly one active occurrence");
        let pending_from_retired = state
            .wake_bindings
            .iter()
            .filter(|(message_id, bound)| {
                **bound == subscription_id
                    && state
                        .msgs
                        .get(*message_id)
                        .is_some_and(|m| m.state == "pending")
            })
            .count();
        assert_eq!(
            pending_from_retired, 0,
            "a late timer batch must not leave a pending scheduler message for the replaced occurrence"
        );
        assert!(
            !state.master_wake.goal_due,
            "replacing a due occurrence must clear the master latch against a late timer batch"
        );
        assert!(state.master_wake.active_goal_revision.is_none());
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }
