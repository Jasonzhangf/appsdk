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
        assert!(state.msgs.is_empty(), "a fired inactive goal cannot rearm");
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
        assert!(replayed_state.msgs.is_empty());
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
            .keys()
            .next()
            .cloned()
            .expect("goal deadline message");
        server.commit(&[Event::NotificationConsumed {
            subscription_id: subscription_id.clone(),
            message_id: message_id.clone(),
            consumed_ms: now_ms(),
        }]);
        tick_with_idle(&server, &|_| false);
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs.len(), 1);
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

        let delivered = delivered.lock().unwrap();
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
        assert!(state.master_wake.goal_due);
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
        periodic_deadline_subscription(&server, "master", "goal:oversized-frame");

        tick_with_idle(&server, &|_| true);

        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs.len(), 1);
        let message_id = state.msgs.keys().next().unwrap();
        assert_eq!(state.msgs[message_id].wake_attempt_count, 1);
        assert!(state.notification_delivery_failures[message_id]
            .error
            .contains("native frame exceeds maximum size"));
        assert_eq!(state.master_wake.delivery_state, "delivery_failed");
        assert_ne!(state.master_wake.delivery_state, "notified_unconsumed");
        drop(state);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn goal_deadline_never_wakes_a_non_master_subscription() {
        let (server, root) = test_server();
        register_master(&server);
        register(&server, "worker");
        let now = now_ms();
        let subscription_id = "sub-goal-worker".to_string();
        server.commit(&[Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: subscription_id.clone(),
                worker_id: "worker".into(),
                event: "deadline".into(),
                subject: Some("goal:worker".into()),
                target: "thread-worker".into(),
                method: "tmux".into(),
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
        let state = server.state.lock().unwrap();
        assert!(state.msgs.is_empty());
        assert_eq!(
            state.notification_subscriptions[&subscription_id].status,
            "suppressed"
        );
        assert_eq!(
            state.notification_subscriptions[&subscription_id]
                .status_reason
                .as_deref(),
            Some("goal-deadline-requires-current-master")
        );
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

        let cancelled = super::super::handle_notification_unsubscribe(
            &server,
            "master".into(),
            "token-master".into(),
            subscription_id.clone(),
        );
        assert!(cancelled.ok, "{}", cancelled.error.unwrap_or_default());
        tick_with_idle(&server, &|_| false);

        let state = server.state.lock().unwrap();
        assert!(state.msgs.is_empty());
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
        let cancelled = super::super::handle_notification_unsubscribe(
            &server,
            "master".into(),
            "token-master".into(),
            subscription_id.clone(),
        );
        assert!(cancelled.ok, "{}", cancelled.error.unwrap_or_default());

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
        assert!(state.msgs[message_id].body.contains("scheduling"));
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
        assert!(state
            .wake_bindings
            .values()
            .all(|bound| bound == &subscription_id));
        assert!(state.msgs.values().all(|message| message.to == "master"));
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
        assert!(state.msgs.is_empty());
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
        assert!(state.msgs.is_empty());
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
            state.msgs.is_empty(),
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
        assert_eq!(state.msgs.len(), 1);
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
        assert_eq!(state.msgs.len(), 1);
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
            assert_eq!(state.msgs.len(), 1);
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
        assert!(state.msgs.is_empty());
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
        server
            .state
            .lock()
            .unwrap()
            .notification_subscriptions
            .remove("sub-default-direct-message-master");
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
        assert_eq!(state.msgs.len(), 1);
        let message = state.msgs.values().next().unwrap();
        assert_eq!(message.to, "master");
        assert_eq!(message.state, "pending");
        assert!(state.wake_bindings.is_empty());
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
            assert_eq!(state.msgs.len(), 1);
            let (message_id, message) = state.msgs.iter().next().unwrap();
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
        assert_eq!(server.state.lock().unwrap().msgs.len(), 1);
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[&message_id].to, message.to);
        assert_eq!(state.msgs[&message_id].subject, message.subject);
        assert_eq!(state.msgs[&message_id].body, message.body);
        assert_eq!(state.msgs[&message_id].state, "pending");
        assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
        assert_eq!(state.msgs.len(), 1);
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
        assert_eq!(replayed_state.msgs.len(), 1);
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
        assert!(text.contains("task_ids=none"));
        assert!(text.contains("action_categories="));
        assert!(text.contains("collab inbox"));

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
