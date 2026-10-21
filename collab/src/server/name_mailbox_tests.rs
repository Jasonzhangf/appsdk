use super::name_state::NameOccupancy;
use super::*;

fn holder(worker: &str, session: &str, revision: u64) -> NameOccupancy {
    NameOccupancy {
        name: "Alice".into(),
        worker_id: worker.into(),
        session_id: session.into(),
        occupancy_revision: revision,
        updated_ms: revision as i64,
    }
}

fn message(id: &str, status: &str) -> Message {
    Message {
        id: id.into(),
        from: "sender".into(),
        to: "old".into(),
        mtype: "notify".into(),
        subject: Some(id.into()),
        body: "original body".into(),
        in_reply_to: None,
        created_ms: now_ms(),
        state: status.into(),
        wake_attempt_count: 9,
        last_wake_attempt_ms: 1,
        retry_attempted: true,
    }
}

fn mailbox_name_selection(release_previous: bool) {
    let (server, root) = peer_tests::test_server();
    for id in ["sender", "w1", "w2"] {
        assert!(peer_tests::register_synthetic_peer(&server, id, id).ok);
    }
    let scope = server_route_scope(&server, &server.state.lock().unwrap())
        .unwrap()
        .unwrap()
        .project_scope_id
        .as_str()
        .to_owned();
    let occupancy = |name: &str, worker: &str, revision| NameOccupancy {
        name: name.into(),
        worker_id: worker.into(),
        session_id: worker.into(),
        occupancy_revision: revision,
        updated_ms: revision as i64,
    };
    let mut ids = Vec::new();
    for name in ["Jenny", "Alice"] {
        {
            let mut state = server.state.lock().unwrap();
            let events = name_resolution::build_name_resolution_events(
                &state,
                &scope,
                &occupancy(name, "w1", 1),
            );
            server.commit_locked_checked(&mut state, &events).unwrap();
        }
        let sent = handle_send(
            &server,
            "sender".into(),
            name.into(),
            "notify".into(),
            Some("same content for distinct names".into()),
            "same body".into(),
            None,
            "queued".into(),
        );
        assert!(sent.ok, "{:?}", sent.error);
        ids.push(sent.data["msg_id"].as_str().unwrap().to_owned());
    }
    assert_ne!(
        ids[0], ids[1],
        "distinct name addresses must not deduplicate"
    );
    {
        let mut state = server.state.lock().unwrap();
        // Exercise both legacy current-holder and released-occupancy paths.
        if !release_previous {
            state
                .name_occupancies
                .get_mut(&scope)
                .unwrap()
                .insert("Jenny".into(), occupancy("Jenny", "w1", 1));
        } else {
            state
                .name_occupancies
                .get_mut(&scope)
                .unwrap()
                .remove("Jenny");
        }
        let events = name_resolution::build_name_resolution_events(
            &state,
            &scope,
            &occupancy("Jenny", "w2", 2),
        );
        server.commit_locked_checked(&mut state, &events).unwrap();
        assert_eq!(
            state.msgs[&ids[0]].to, "w2",
            "Jenny message must follow its name"
        );
        assert_eq!(
            state.msgs[&ids[1]].to, "w1",
            "Alice message must stay with Alice"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn name_mailbox_address_selects_only_requested_name() {
    mailbox_name_selection(false);
}

#[test]
fn name_mailbox_address_survives_released_occupancy() {
    mailbox_name_selection(true);
}

#[test]
fn name_mailbox_typed_address_replay_snapshot_reset_and_history() {
    let mut state = State::default();
    let mut journal = Vec::new();
    for (id, name, status, project) in [
        ("jenny", "Jenny", "pending", "/project"),
        ("alice", "Alice", "delivered", "/project"),
        ("read", "Jenny", "read", "/project"),
        ("superseded", "Jenny", "superseded", "/project"),
        ("foreign", "Jenny", "pending", "/other"),
    ] {
        journal.push(Event::Sent {
            msg: message(id, status),
        });
        journal.push(Event::MailboxNameAddressed {
            message_id: id.into(),
            address: name_mailbox::MailboxNameAddress {
                project_scope: project.into(),
                name: name.into(),
            },
        });
    }
    // A legacy message has no association, even after journal replay.
    journal.push(Event::Sent {
        msg: message("legacy", "pending"),
    });
    for event in &journal {
        state.apply_checked(event).unwrap();
    }
    let mut next = holder("new", "s2", 2);
    next.name = "Jenny".into();
    let events = name_resolution::build_name_resolution_events(&state, "/project", &next);
    journal.extend(events);
    let mut replay = State::default();
    for event in &journal {
        let encoded = serde_json::to_string(event).unwrap();
        replay
            .apply_checked(&serde_json::from_str(&encoded).unwrap())
            .unwrap();
    }
    assert_eq!(replay.msgs["jenny"].to, "new");
    for id in ["alice", "read", "superseded", "foreign", "legacy"] {
        assert_eq!(replay.msgs[id].to, "old", "{id}");
    }
    assert_eq!(replay.msgs["jenny"].body, "original body");
    assert_eq!(replay.msgs["jenny"].from, "sender");
    assert_eq!(replay.mailbox_handoffs.len(), 1);
    let mut snapshot = State::default();
    for event in replay.snapshot_events() {
        snapshot.apply_checked(&event).unwrap();
    }
    assert_eq!(
        snapshot.mailbox_name_addresses,
        replay.mailbox_name_addresses
    );
    assert_eq!(
        serde_json::to_value(&snapshot.msgs).unwrap(),
        serde_json::to_value(&replay.msgs).unwrap()
    );
    // Legacy fallback follows only the current previous holder, and excludes
    // all explicitly associated other names and projects.
    let mut newer = next.clone();
    newer.worker_id = "newer".into();
    newer.session_id = "s3".into();
    newer.occupancy_revision = 3;
    snapshot.msgs.get_mut("legacy").unwrap().to = "new".into();
    let handoffs = name_mailbox::handoff_events(&snapshot, "/project", &newer);
    assert_eq!(handoffs.len(), 2);
    for event in handoffs {
        snapshot.apply_checked(&event).unwrap();
    }
    assert_eq!(snapshot.msgs["legacy"].to, "newer");
    assert_eq!(snapshot.msgs["alice"].to, "old");
    snapshot.drop_message("jenny");
    assert!(!snapshot.mailbox_name_addresses.contains_key("jenny"));
    project_reset::clear_project(&mut snapshot, "/project", false, &["old".into()]);
    for id in ["alice", "read", "superseded"] {
        assert!(!snapshot.msgs.contains_key(id));
        assert!(!snapshot.mailbox_name_addresses.contains_key(id));
    }
    assert!(snapshot.msgs.contains_key("foreign"));
    assert!(snapshot.mailbox_name_addresses.contains_key("foreign"));
}

#[test]
fn name_mailbox_handoff_preserves_consumption_history_and_replays() {
    let mut state = State::default();
    state
        .name_occupancies
        .entry("/project".into())
        .or_default()
        .insert("Alice".into(), holder("old", "s1", 1));
    for (id, status) in [
        ("pending", "pending"),
        ("delivered", "delivered"),
        ("read", "read"),
        ("superseded", "superseded"),
    ] {
        state.apply(&Event::Sent {
            msg: message(id, status),
        });
    }
    state.apply(&Event::NotificationDeliveryAccepted {
        message_id: "pending".into(),
        accepted_ms: 1,
        evidence: Some(json!({"accepted":true})),
    });
    let next = holder("new", "s2", 2);
    let events = name_resolution::build_name_resolution_events(&state, "/project", &next);
    for event in &events {
        state.apply_checked(event).unwrap();
    }
    assert!(state.inbox_of("old").is_empty());
    assert_eq!(state.inbox_of("new").len(), 2);
    for id in ["pending", "delivered"] {
        assert_eq!(state.msgs[id].to, "new");
        assert_eq!(state.msgs[id].body, "original body");
        assert_eq!(state.msgs[id].from, "sender");
        assert_eq!(state.msgs[id].wake_attempt_count, 9);
        assert_eq!(
            state.wake_bindings[id],
            mailbox::default_direct_message_id("new")
        );
    }
    assert_eq!(state.msgs["read"].to, "old");
    assert_eq!(state.msgs["superseded"].to, "old");
    assert_eq!(state.notification_delivery_accepted["pending"], 1);
    assert!(name_resolution::build_name_resolution_events(&state, "/project", &next).is_empty());
    for event in &events {
        state.apply_checked(event).unwrap();
    }
    let mut replay = State::default();
    for event in state.snapshot_events() {
        replay.apply_checked(&event).unwrap();
    }
    assert_eq!(
        serde_json::to_value(&replay.msgs).unwrap(),
        serde_json::to_value(&state.msgs).unwrap()
    );
    assert_eq!(replay.wake_bindings, state.wake_bindings);
}

#[test]
fn name_mailbox_session_change_and_project_isolation() {
    let mut state = State::default();
    state
        .name_occupancies
        .entry("/other".into())
        .or_default()
        .insert("Alice".into(), holder("old", "s1", 1));
    state.apply(&Event::Sent {
        msg: message("other", "pending"),
    });
    for event in
        name_resolution::build_name_resolution_events(&state, "/project", &holder("new", "s2", 2))
    {
        state.apply_checked(&event).unwrap();
    }
    assert_eq!(state.msgs["other"].to, "old");
    for event in
        name_resolution::build_name_resolution_events(&state, "/other", &holder("old", "s2", 2))
    {
        state.apply_checked(&event).unwrap();
    }
    assert_eq!(state.inbox_of("old").len(), 1);
    assert_eq!(state.msgs["other"].wake_attempt_count, 9);
    assert_eq!(state.mailbox_handoffs.len(), 1);
    assert!(state.handoff_wake_pending(&state.msgs["other"]));
}

#[test]
fn name_mailbox_new_holder_wake_ignores_old_budget_and_consumes_no_old_subscription() {
    let (mut server, root) = peer_tests::test_server();
    server.config.notifications.batch_window_seconds = 0;
    let targets = Arc::new(Mutex::new(Vec::new()));
    let observed = targets.clone();
    server.appserver_notification_sink = Arc::new(move |transport, _, _, _, _, _| {
        observed.lock().unwrap().push(transport.thread_id.clone());
        Ok(json!({"accepted": true}))
    });
    let server = Arc::new(server);
    for id in ["old", "new"] {
        assert!(peer_tests::register_synthetic_peer(&server, id, id).ok);
    }
    let scope = server_route_scope(&server, &server.state.lock().unwrap())
        .unwrap()
        .unwrap()
        .project_scope_id
        .as_str()
        .to_owned();
    {
        let mut state = server.state.lock().unwrap();
        state
            .name_occupancies
            .entry(scope.clone())
            .or_default()
            .insert("Alice".into(), holder("old", "s1", 1));
        let mut old_subscription =
            state.notification_subscriptions[&mailbox::default_direct_message_id("old")].clone();
        old_subscription.id = "old-timer".into();
        old_subscription.event = "deadline".into();
        old_subscription.trigger_ms = Some(1);
        old_subscription.status = "cancelled".into();
        let mut initial = vec![Event::NotificationSubscribed {
            subscription: old_subscription,
        }];
        for id in ["pending", "delivered"] {
            initial.push(Event::Sent {
                msg: message(id, id),
            });
            initial.push(Event::WakeBound {
                message_id: id.into(),
                subscription_id: "old-timer".into(),
            });
            initial.push(Event::NotificationDeliveryAccepted {
                message_id: id.into(),
                accepted_ms: 1,
                evidence: None,
            });
        }
        server.commit_locked_checked(&mut state, &initial).unwrap();
        let events =
            name_resolution::build_name_resolution_events(&state, &scope, &holder("new", "s2", 2));
        server.commit_locked_checked(&mut state, &events).unwrap();
    }
    timers::tick_with_idle_at(&server, now_ms(), &|_| true);
    assert_eq!(*targets.lock().unwrap(), vec![Some("new".into())]);
    timers::tick_with_idle_at(&server, now_ms(), &|_| true);
    assert_eq!(targets.lock().unwrap().len(), 1);
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs["pending"].wake_attempt_count, 10);
        assert_eq!(state.msgs["delivered"].wake_attempt_count, 10);
        assert_eq!(state.msgs["delivered"].state, "delivered");
    }
    let received =
        poll_messages_with_context(&server, "new", None, None, Some("new-read")).unwrap();
    assert!(received.ok);
    assert_eq!(received.data["count"], 2);
    let state = server.state.lock().unwrap();
    assert_eq!(state.notification_subscriptions["old-timer"].fired_count, 0);
    assert_eq!(state.receive_receipts["new-read"].worker_id, "new");
    assert_eq!(state.mailbox_handoffs.len(), 2);
    assert_eq!(state.mailbox_handoffs[0].accepted_ms, Some(1));
    assert!(state
        .msgs
        .values()
        .all(|message| !state.handoff_wake_pending(message)));
    let mut replay = State::default();
    for line in std::fs::read_to_string(&server.journal_path)
        .unwrap()
        .lines()
    {
        replay
            .apply_checked(&serde_json::from_str::<Event>(line).unwrap())
            .unwrap();
    }
    assert_eq!(
        serde_json::to_value(&replay.msgs).unwrap(),
        serde_json::to_value(&state.msgs).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&replay.receive_receipts).unwrap(),
        serde_json::to_value(&state.receive_receipts).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&replay.mailbox_handoffs).unwrap(),
        serde_json::to_value(&state.mailbox_handoffs).unwrap()
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn name_mailbox_rejected_handoff_keeps_journal_name_and_mailbox_unchanged() {
    let (server, root) = peer_tests::test_server();
    let before = std::fs::read(&server.journal_path).unwrap();
    let mut state = server.state.lock().unwrap();
    state
        .name_occupancies
        .entry("/project".into())
        .or_default()
        .insert("Alice".into(), holder("old", "s1", 1));
    state.apply(&Event::Sent {
        msg: message("pending", "pending"),
    });
    state.responsibility_fences.insert(
        "close-new".into(),
        state::ResponsibilityFence {
            operation_id: "close-new".into(),
            worker_id: "new".into(),
            project_scope: "/project".into(),
            app_scope: "app".into(),
            binding_id: "binding".into(),
            endpoint_generation: 1,
            snapshot: state::ResponsibilitySnapshot::default(),
            state: "closing".into(),
            created_ms: 1,
        },
    );
    let events =
        name_resolution::build_name_resolution_events(&state, "/project", &holder("new", "s2", 2));
    let error = server
        .commit_locked_checked(&mut state, &events)
        .unwrap_err();
    assert!(error.to_string().contains("target_closing"), "{error}");
    assert_eq!(
        state.name_occupancy("/project", "Alice").unwrap().worker_id,
        "old"
    );
    assert_eq!(state.msgs["pending"].to, "old");
    assert!(state.mailbox_handoffs.is_empty());
    assert_eq!(std::fs::read(&server.journal_path).unwrap(), before);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
