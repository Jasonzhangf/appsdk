use super::*;

const INTERVAL: i64 = 1_500_000;

#[test]
fn deadline_summary_uses_deadline_policy_and_preserves_ordinary_batching() {
    let (server, root, _) = fixture(true);
    tick_with_deadline_wake_at(&server, now_ms());
    let mut ordinary = {
        let state = server.state.lock().unwrap();
        state.msgs.values().next().unwrap().clone()
    };
    ordinary.id = "ordinary-with-deadline-text".into();
    // Identical text is not control truth. Only typed membership selects policy.
    server.commit(&[Event::Sent { msg: ordinary }]);
    let state = server.state.lock().unwrap();
    let summary = state.deadline_summaries.keys().next().unwrap();
    let mut config = crate::config::Notifications::default();
    assert_eq!(
        crate::server::notification_delivery_delay_ms(&state, summary, "direct-message", &config),
        0
    );
    assert_eq!(
        crate::server::notification_delivery_delay_ms(
            &state,
            "ordinary-with-deadline-text",
            "direct-message",
            &config
        ),
        120_000
    );
    config.batch_window_seconds = 7;
    config.events.get_mut("deadline").unwrap().mode = "batch".into();
    assert_eq!(
        crate::server::notification_delivery_delay_ms(&state, summary, "direct-message", &config),
        7_000,
        "an explicitly batched deadline must retain its configured delay"
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn deadline_summary_delayed_native_delivery_is_not_early_or_duplicated() {
    let (mut server, root, id) = fixture(true);
    let config = &mut Arc::get_mut(&mut server).unwrap().config.notifications;
    config.batch_window_seconds = 7;
    config.events.get_mut("deadline").unwrap().mode = "batch".into();
    let deliveries = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&deliveries);
    Arc::get_mut(&mut server)
        .unwrap()
        .appserver_notification_sink = Arc::new(move |_, _, text, _, _, _| {
        captured.lock().unwrap().push(text.to_string());
        Ok(serde_json::json!({"accepted": true}))
    });
    let now = now_ms();
    tick_with_deadline_wake_at(&server, now);
    let message_id = server
        .state
        .lock()
        .unwrap()
        .deadline_summaries
        .keys()
        .next()
        .unwrap()
        .clone();
    tick_with_deadline_wake_at(&server, now + 6_999);
    assert!(deliveries.lock().unwrap().is_empty());
    tick_with_deadline_wake_at(&server, now + 7_000);
    assert_eq!(deliveries.lock().unwrap().len(), 1);
    {
        let state = server.state.lock().unwrap();
        assert!(state
            .notification_delivery_accepted
            .contains_key(&message_id));
        assert_eq!(state.notification_subscriptions[&id].fired_count, 0);
    }
    tick_with_deadline_wake_at(&server, now + 14_000);
    assert_eq!(deliveries.lock().unwrap().len(), 1);
    assert!(
        crate::server::poll_messages_with_context(&server, "owner", None, None, None)
            .unwrap()
            .ok
    );
    assert_eq!(
        server.state.lock().unwrap().notification_subscriptions[&id].fired_count,
        1
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn wallclock_summary_two_overdue_subjects_before_receive() {
    let (server, root, id) = fixture(true);
    let mut second = server.state.lock().unwrap().notification_subscriptions[&id].clone();
    second.id = "overdue-second".into();
    second.subject = Some("goal:second".into());
    server.commit(&[Event::NotificationSubscribed {
        subscription: second,
    }]);
    let now = now_ms();
    tick_with_deadline_wake_at(&server, now);
    {
        let state = server.state.lock().unwrap();
        let messages: Vec<_> = state
            .msgs
            .values()
            .filter(|m| m.to == "owner" && m.mtype == "notification")
            .collect();
        assert_eq!(messages.len(), 1, "one recipient receives one due summary");
        assert!(messages[0].body.contains("goal:overdue"));
        assert!(messages[0].body.contains("goal:second"));
        for key in [&id, &"overdue-second".to_string()] {
            assert!(
                state.notification_subscriptions[key].trigger_ms.unwrap() > now,
                "schedule advances without receive"
            );
        }
    }
    // Several physical intervals pass without a read. Keep the original
    // unread summary and fold time, including across durable reload.
    let later = now + INTERVAL * 4;
    for activity in ["busy", "idle"] {
        server
            .state
            .lock()
            .unwrap()
            .keepalives
            .entry("owner".into())
            .or_default()
            .observed = activity.into();
        tick_with_deadline_wake_at(&server, later);
    }
    let before = server
        .state
        .lock()
        .unwrap()
        .notification_subscriptions
        .clone();
    assert!(before[&id].trigger_ms.unwrap() > later);
    assert_eq!(
        before[&id].fired_count, 0,
        "unread and missed periods do not fabricate consumption"
    );
    let loaded = current_state::load(&server.state_path, false).unwrap();
    *server.state.lock().unwrap() = loaded;
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| tick_with_deadline_wake_at(&server, later));
        }
    });
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .msgs
            .values()
            .filter(|m| m.mtype == "notification")
            .count(),
        1
    );
    let received = crate::server::poll_messages_with_context(
        &server,
        "owner",
        None,
        None,
        Some("summary-consume"),
    )
    .unwrap();
    assert!(received.ok, "{received:?}");
    let snapshot = server
        .state
        .lock()
        .unwrap()
        .notification_subscriptions
        .clone();
    for key in [&id, &"overdue-second".to_string()] {
        assert_eq!(snapshot[key].trigger_ms, before[key].trigger_ms);
        assert_eq!(snapshot[key].fired_count, 1);
    }
    assert!(
        crate::server::poll_messages_with_context(
            &server,
            "owner",
            None,
            None,
            Some("summary-consume")
        )
        .unwrap()
        .ok
    );
    assert_eq!(
        server.state.lock().unwrap().notification_subscriptions,
        snapshot
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn fixture(periodic: bool) -> (Arc<Server>, std::path::PathBuf, String) {
    let (server, root) = test_server();
    let transport = SelectedTransport {
        kind: crate::proto::TransportKind::AppServer,
        endpoint: Some("test://overdue".into()),
        namespace: Some("test".into()),
        session_id: Some("overdue-session".into()),
        thread_id: Some("overdue-thread".into()),
        tmux_endpoint: None,
        rex_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "isolated appserver".into(),
    };
    let response = crate::server::peer_tests::seed_committed_owner(
        &server,
        "owner",
        "token-owner",
        &transport,
        &root.display().to_string(),
        Some(crate::identity::AppServerId::new("tui-default").unwrap()),
    );
    assert!(response.ok, "{response:?}");
    let (target, method) = subscription_target(&server, "owner");
    let now = now_ms();
    let id = "overdue-goal".to_string();
    server.commit(&[Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: id.clone(),
            worker_id: "owner".into(),
            event: "deadline".into(),
            subject: Some("goal:overdue".into()),
            target,
            method,
            trigger_ms: Some(now - INTERVAL * 5),
            trigger_times_ms: vec![],
            interval_ms: Some(INTERVAL),
            repeat_count: 3,
            fired_count: 0,
            expires_ms: now + INTERVAL * 10,
            status: "armed".into(),
            created_ms: now - INTERVAL * 6,
            updated_ms: now,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: periodic,
            goal_content: None,
        },
    }]);
    (server, root, id)
}

#[test]
fn overdue_goal_receive_coalesces_missed_periods() {
    for receive_id in [None, Some("overdue-receive")] {
        let (server, root, id) = fixture(true);
        tick_with_deadline_wake_at(&server, now_ms());
        let scheduled = server.state.lock().unwrap().notification_subscriptions[&id]
            .trigger_ms
            .unwrap();
        let response =
            crate::server::poll_messages_with_context(&server, "owner", None, None, receive_id)
                .expect("a real timer message must be available");
        assert!(response.ok, "{response:?}");
        let next = {
            let state = server.state.lock().unwrap();
            let sub = &state.notification_subscriptions[&id];
            assert_eq!(
                sub.fired_count, 1,
                "missed periods do not count as deliveries"
            );
            assert_eq!(sub.status, "armed");
            let next = sub.trigger_ms.unwrap();
            assert!(
                next == scheduled,
                "receive must preserve the daemon's future physical trigger"
            );
            next
        };
        let snapshot =
            serde_json::to_value(&server.state.lock().unwrap().notification_subscriptions[&id])
                .unwrap();
        let replay =
            crate::server::poll_messages_with_context(&server, "owner", None, None, receive_id);
        if receive_id.is_some() {
            assert!(replay.unwrap().ok);
        } else {
            assert!(replay.is_none());
        }
        assert_eq!(
            serde_json::to_value(&server.state.lock().unwrap().notification_subscriptions[&id])
                .unwrap(),
            snapshot
        );
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| tick_with_deadline_wake_at(&server, next - 1));
            }
        });
        assert_eq!(
            subscription_messages(&server.state.lock().unwrap(), &id).len(),
            1
        );
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| tick_with_deadline_wake_at(&server, next));
            }
        });
        assert_eq!(
            subscription_messages(&server.state.lock().unwrap(), &id).len(),
            2
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn overdue_goal_finite_on_time_one_shot_expiry_and_cancel_controls() {
    for mode in ["finite", "ordinary-finite", "one-shot", "expiry", "cancel"] {
        let (server, root, id) = fixture(mode != "one-shot");
        {
            let mut state = server.state.lock().unwrap();
            let sub = state.notification_subscriptions.get_mut(&id).unwrap();
            sub.interval_ms = Some(40);
            sub.trigger_ms = Some(now_ms());
            if mode == "ordinary-finite" {
                sub.subject = Some("ordinary-finite".into());
                sub.periodic_declared = false;
            }
            if mode == "expiry" {
                sub.expires_ms = now_ms() - 1;
            }
            if mode == "cancel" {
                sub.status = "cancelled".into();
            }
        }
        let expected = if matches!(mode, "finite" | "ordinary-finite") {
            3
        } else if mode == "one-shot" {
            1
        } else {
            0
        };
        for count in 0..expected {
            tick_with_deadline_wake_at(&server, now_ms());
            let response = crate::server::poll_messages_with_context(
                &server,
                "owner",
                None,
                None,
                Some(&format!("{mode}-{count}")),
            )
            .expect("due occurrence");
            assert!(response.ok, "{response:?}");
            assert_eq!(
                server.state.lock().unwrap().notification_subscriptions[&id].fired_count,
                count + 1
            );
            std::thread::sleep(std::time::Duration::from_millis(45));
        }
        tick_with_deadline_wake_at(&server, now_ms());
        let state = server.state.lock().unwrap();
        assert_eq!(subscription_messages(&state, &id).len(), expected as usize);
        assert_eq!(
            state.notification_subscriptions[&id].status,
            match mode {
                "expiry" => "expired",
                "cancel" => "cancelled",
                _ => "consumed",
            }
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn wallclock_summary_preserves_explicit_finite_time_list() {
    let (server, root, id) = fixture(false);
    let now = now_ms();
    let mut sub = server.state.lock().unwrap().notification_subscriptions[&id].clone();
    sub.subject = Some("explicit-time-list".into());
    sub.interval_ms = None;
    sub.trigger_ms = None;
    sub.trigger_times_ms = vec![now, now + INTERVAL];
    server.commit(&[Event::NotificationSubscribed { subscription: sub }]);
    tick_with_deadline_wake_at(&server, now);
    assert!(
        crate::server::poll_messages_with_context(&server, "owner", None, None, None)
            .unwrap()
            .ok
    );
    assert_eq!(
        server.state.lock().unwrap().notification_subscriptions[&id].fired_count,
        1
    );
    tick_with_deadline_wake_at(&server, now + INTERVAL - 1);
    assert!(
        crate::server::poll_messages_with_context(&server, "owner", None, None, None).is_none()
    );
    tick_with_deadline_wake_at(&server, now + INTERVAL);
    assert!(
        crate::server::poll_messages_with_context(&server, "owner", None, None, None)
            .unwrap()
            .ok
    );
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions[&id].trigger_times_ms,
        vec![now, now + INTERVAL]
    );
    assert_eq!(state.notification_subscriptions[&id].fired_count, 2);
    assert_eq!(state.notification_subscriptions[&id].status, "consumed");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn wallclock_summary_preserves_unread_members_on_cancel_and_expiry() {
    for expired in [false, true] {
        let (server, root, id) = fixture(true);
        let mut second = server.state.lock().unwrap().notification_subscriptions[&id].clone();
        second.id = "preserved-second".into();
        second.subject = Some("goal:preserved".into());
        server.commit(&[Event::NotificationSubscribed {
            subscription: second,
        }]);
        let now = now_ms();
        tick_with_deadline_wake_at(&server, now);
        let message_id = subscription_messages(&server.state.lock().unwrap(), &id)[0]
            .id
            .clone();
        let original = server.state.lock().unwrap().msgs[&message_id].body.clone();
        if expired {
            let mut sub = server.state.lock().unwrap().notification_subscriptions[&id].clone();
            sub.expires_ms = now + 1;
            server.commit(&[Event::NotificationSubscribed { subscription: sub }]);
            tick_with_deadline_wake_at(&server, now + 2);
        } else {
            let owner_name = server
                .state
                .lock()
                .unwrap()
                .name_occupancies
                .values()
                .flat_map(|names| names.values())
                .find(|occupancy| occupancy.worker_id == "owner")
                .unwrap()
                .name
                .clone();
            let cancelled = crate::server::handle_goal_schedule_reconcile(
                &server,
                "owner".into(),
                "token-owner".into(),
                "overdue".into(),
                owner_name,
                "ended".into(),
                None,
                None,
                None,
                Some("test-cancel".into()),
            );
            assert!(cancelled.ok, "{cancelled:?}");
        }
        assert_eq!(
            server.state.lock().unwrap().msgs[&message_id].body,
            original
        );
        assert_ne!(
            server.state.lock().unwrap().msgs[&message_id].state,
            "superseded"
        );
        let received =
            crate::server::poll_messages_with_context(&server, "owner", None, None, None).unwrap();
        assert!(received.ok, "{received:?}");
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.notification_subscriptions[&id].status,
            if expired { "expired" } else { "cancelled" }
        );
        assert_eq!(
            state.notification_subscriptions["preserved-second"].fired_count,
            1
        );
        assert_eq!(state.msgs[&message_id].state, "read");
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn overdue_goal_explicit_ack_preserves_interval_and_is_idempotent() {
    let (server, root, id) = fixture(true);
    tick_with_deadline_wake_at(&server, now_ms());
    tick_with_deadline_wake_at(&server, now_ms() + INTERVAL * 3);
    let message_id = subscription_messages(&server.state.lock().unwrap(), &id)[0]
        .id
        .clone();
    let mut state = server.state.lock().unwrap();
    let before = serde_json::to_value(&*state).unwrap();
    let denied = crate::server::handle_ack_locked(
        &server,
        &mut state,
        None,
        "owner",
        "bad-token",
        vec![message_id.clone()],
    );
    assert!(!denied.ok);
    assert_eq!(serde_json::to_value(&*state).unwrap(), before);
    let next = state.notification_subscriptions[&id].trigger_ms;
    let ack = crate::server::handle_ack_locked(
        &server,
        &mut state,
        None,
        "owner",
        "token-owner",
        vec![message_id.clone()],
    );
    assert!(ack.ok, "{ack:?}");
    assert_eq!(state.notification_subscriptions[&id].trigger_ms, next);
    assert_eq!(state.notification_subscriptions[&id].fired_count, 1);
    let before = serde_json::to_value(&*state).unwrap();
    assert!(
        crate::server::handle_ack_locked(
            &server,
            &mut state,
            None,
            "owner",
            "token-owner",
            vec![message_id],
        )
        .ok
    );
    assert_eq!(serde_json::to_value(&*state).unwrap(), before);
    drop(state);
    let loaded = current_state::load(&server.state_path, false).unwrap();
    assert_eq!(
        loaded.notification_subscriptions[&id],
        server.state.lock().unwrap().notification_subscriptions[&id]
    );
    std::fs::remove_dir_all(root).unwrap();
}
