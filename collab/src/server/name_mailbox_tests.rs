use super::name_state::NameOccupancy;
use super::*;

fn holder(worker: &str, session: &str, revision: u64) -> NameOccupancy {
    NameOccupancy { name: "Alice".into(), worker_id: worker.into(), session_id: session.into(),
        occupancy_revision: revision, updated_ms: revision as i64, pane_anchor: None }
}

fn message(id: &str, status: &str) -> Message {
    Message { id: id.into(), from: "sender".into(), to: "old".into(), mtype: "notify".into(),
        subject: Some(id.into()), body: "original body".into(), in_reply_to: None, created_ms: now_ms(),
        state: status.into(), wake_attempt_count: 9, last_wake_attempt_ms: 1, retry_attempted: true }
}

// Publish the current carrier first, then apply the existing name effects.
// Previous identity comes from the map even when business occupancy was released.
fn publish_holder(server: &Server, name: &str, worker: &str) {
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let binding = control.active_values().find(|value| value.binding.agent_id.as_str() == worker).unwrap().clone();
    let key = binding_slot::IdentityBindingKey::new(binding.binding.project_scope.clone(), binding.binding.app_scope_id.clone(), name);
    let source = control.key_for_worker(&key.project_scope, &key.app_scope_id, worker).unwrap().unwrap();
    let previous = control.slot(&key).and_then(|slot| slot.active_binding()).map(|value| {
        let mut occupancy = holder(value.binding.agent_id.as_str(), value.binding.session_id.as_ref().unwrap().as_str(), 1);
        occupancy.name = name.into(); occupancy
    });
    let mut next = holder(worker, binding.binding.session_id.as_ref().unwrap().as_str(), 2);
    next.name = name.into();
    store.commit(binding_store::ControlChange::MoveName {
        target: key.clone(), value: binding, source_names: vec![source.name.clone()], expected_generation: None,
    }).unwrap();
    let mut state = server.state.lock().unwrap();
    let events = name_resolution::build_name_resolution_events_with_rename_from(&state,
        key.project_scope.as_str(), &next, &[], previous.as_ref());
    server.commit_locked_checked(&mut state, &events).unwrap();
}

fn mailbox_name_selection(release_previous: bool) {
    let (server, root) = peer_tests::test_server();
    for id in ["alice", "sender", "old", "new"] {
        assert!(peer_tests::register_synthetic_peer(&server, id, id).ok);
    }
    publish_holder(&server, "Jenny", "old");
    let scope = GlobalState::canonical_project_scope(&root).unwrap();
    let mut ids = Vec::new();
    for name in ["Jenny", "Alice"] {
        let sent = handle_send(&server, "sender".into(), name.into(), "notify".into(),
            Some("same content for distinct names".into()), "same body".into(), None, "queued".into());
        assert!(sent.ok, "{sent:?}");
        ids.push(sent.data["msg_id"].as_str().unwrap().to_owned());
    }
    assert_ne!(ids[0], ids[1], "distinct name addresses must not deduplicate");
    if release_previous {
        server.state.lock().unwrap().name_occupancies.get_mut(scope.as_str()).unwrap().remove("Jenny");
    }
    publish_holder(&server, "Jenny", "new");
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[&ids[0]].to, "new");
    assert_eq!(state.msgs[&ids[1]].to, "alice");
    assert_eq!(state.msgs[&ids[0]].body, "same body");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn name_mailbox_address_selects_only_requested_name() { mailbox_name_selection(false); }

#[test]
fn name_mailbox_address_survives_released_occupancy() { mailbox_name_selection(true); }

#[test]
fn name_mailbox_typed_address_current_state_and_reset() {
    let mut state = State::default();
    for (id, name, status, project) in [
        ("jenny", "Jenny", "pending", "/project"), ("alice", "Alice", "delivered", "/project"),
        ("read", "Jenny", "read", "/project"), ("superseded", "Jenny", "superseded", "/project"),
        ("foreign", "Jenny", "pending", "/other"),
    ] {
        state.apply_checked(&Event::Sent { msg: message(id, status) }).unwrap();
        state.apply_checked(&Event::MailboxNameAddressed { message_id: id.into(),
            address: name_mailbox::MailboxNameAddress { project_scope: project.into(), name: name.into() } }).unwrap();
    }
    let mut next = holder("new", "s2", 2); next.name = "Jenny".into();
    for event in name_mailbox::handoff_events(&state, "/project", &next, &[], None) {
        state.apply_checked(&event).unwrap();
    }
    assert_eq!(state.msgs["jenny"].to, "new");
    for id in ["alice", "read", "superseded", "foreign"] { assert_eq!(state.msgs[id].to, "old", "{id}"); }
    assert_eq!(state.msgs["jenny"].body, "original body");
    assert_eq!(state.msgs["jenny"].from, "sender");
    assert!(name_mailbox::handoff_events(&state, "/project", &next, &[], None).is_empty(),
        "vacancy recovery must not repeat a current carrier's handoff wake");
    let mut loaded: State = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    assert_eq!(loaded.mailbox_name_addresses, state.mailbox_name_addresses);
    assert_eq!(serde_json::to_value(&loaded.msgs).unwrap(), serde_json::to_value(&state.msgs).unwrap());
    loaded.drop_message("jenny");
    assert!(!loaded.mailbox_name_addresses.contains_key("jenny"));
    project_reset::clear_project(&mut loaded, "/project", false, &["old".into()]);
    for id in ["alice", "read", "superseded"] {
        assert!(!loaded.msgs.contains_key(id));
        assert!(!loaded.mailbox_name_addresses.contains_key(id));
    }
    assert!(loaded.msgs.contains_key("foreign"));
    assert!(loaded.mailbox_name_addresses.contains_key("foreign"));
}

#[test]
fn name_mailbox_handoff_preserves_consumed_messages_and_current_payloads() {
    let mut state = State::default();
    let previous = holder("old", "s1", 1);
    let next = holder("new", "s2", 2);
    for (id, status) in [("pending", "pending"), ("delivered", "delivered"), ("read", "read"), ("superseded", "superseded")] {
        state.apply_checked(&Event::Sent { msg: message(id, status) }).unwrap();
    }
    state.apply_checked(&Event::NotificationDeliveryAccepted { message_id: "pending".into(), accepted_ms: 1,
        evidence: Some(json!({"accepted":true})) }).unwrap();
    for event in name_mailbox::handoff_events(&state, "/project", &next, &[], Some(&previous)) {
        state.apply_checked(&event).unwrap();
    }
    assert!(state.inbox_of("old").is_empty());
    assert_eq!(state.inbox_of("new").len(), 2);
    for id in ["pending", "delivered"] {
        assert_eq!(state.msgs[id].to, "new"); assert_eq!(state.msgs[id].body, "original body");
        assert_eq!(state.msgs[id].from, "sender"); assert_eq!(state.msgs[id].wake_attempt_count, 9);
        assert_eq!(state.wake_bindings[id], mailbox::default_direct_message_id("new"));
    }
    assert_eq!(state.msgs["read"].to, "old");
    assert_eq!(state.msgs["superseded"].to, "old");
    assert_eq!(state.notification_delivery_accepted["pending"], 1);
    assert!(name_mailbox::handoff_events(&state, "/project", &next, &[], Some(&next)).is_empty());
    let loaded: State = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&loaded.msgs).unwrap(), serde_json::to_value(&state.msgs).unwrap());
    assert_eq!(loaded.wake_bindings, state.wake_bindings);
    assert_eq!(loaded.mailbox_handoff_wakes, state.mailbox_handoff_wakes);
}

#[test]
fn name_mailbox_session_change_and_project_isolation() {
    let (server, root) = peer_tests::test_server();
    for id in ["old", "new"] { assert!(peer_tests::register_synthetic_peer(&server, id, id).ok); }
    let scope = GlobalState::canonical_project_scope(&root).unwrap();
    server.commit_checked(&[Event::Sent { msg: message("local", "pending") },
        Event::MailboxNameAddressed { message_id: "local".into(), address: name_mailbox::MailboxNameAddress {
            project_scope: scope.as_str().into(), name: "Alice".into() } },
        Event::Sent { msg: message("foreign", "pending") },
        Event::MailboxNameAddressed { message_id: "foreign".into(), address: name_mailbox::MailboxNameAddress {
            project_scope: "/other".into(), name: "Alice".into() } },
    ]).unwrap();
    publish_holder(&server, "Alice", "new");
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs["local"].to, "new");
    assert_eq!(state.msgs["foreign"].to, "old");
    assert_eq!(state.msgs["local"].wake_attempt_count, 9);
    assert!(state.handoff_wake_pending(&state.msgs["local"]));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn name_mailbox_new_holder_wake_ignores_old_budget_and_consumes_no_old_subscription() {
    let (mut server, root) = peer_tests::test_server();
    server.config.notifications.batch_window_seconds = 0;
    let targets = Arc::new(Mutex::new(Vec::new()));
    let observed = targets.clone();
    server.appserver_notification_sink = Arc::new(move |transport, _, _, _, _, _| {
        observed.lock().unwrap().push(transport.thread_id.clone()); Ok(json!({"accepted": true}))
    });
    let server = Arc::new(server);
    for id in ["old", "new"] {
        let transport = SelectedTransport { kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/name-mailbox.sock".into()), namespace: Some("test".into()),
            session_id: Some(format!("session-{id}")), thread_id: Some(id.into()), tmux_endpoint: None,
            rex_endpoint: None,
            capabilities: vec!["send_message_to_thread".into()], self_check: "committed fixture".into() };
        assert!(peer_tests::seed_committed_owner(&server, id, &format!("token-{id}"), &transport,
            &root.display().to_string(), Some(AppServerId::new("name-mailbox").unwrap())).ok);
    }
    let mut old_subscription = server.state.lock().unwrap().notification_subscriptions[&mailbox::default_direct_message_id("old")].clone();
    old_subscription.id = "old-timer".into(); old_subscription.event = "deadline".into();
    old_subscription.trigger_ms = Some(1); old_subscription.status = "cancelled".into();
    let mut initial = vec![Event::NotificationSubscribed { subscription: old_subscription }];
    for id in ["pending", "delivered"] {
        initial.push(Event::Sent { msg: message(id, id) });
        initial.push(Event::WakeBound { message_id: id.into(), subscription_id: "old-timer".into() });
        initial.push(Event::NotificationDeliveryAccepted { message_id: id.into(), accepted_ms: 1, evidence: None });
    }
    server.commit_checked(&initial).unwrap();
    publish_holder(&server, "Alice", "new");
    timers::tick_with_idle_at(&server, now_ms(), &|_| true);
    assert_eq!(*targets.lock().unwrap(), vec![Some("new".into())]);
    timers::tick_with_idle_at(&server, now_ms(), &|_| true);
    assert_eq!(targets.lock().unwrap().len(), 1);
    let received = poll_messages_with_context(&server, "new", None, None, Some("new-read")).unwrap();
    assert!(received.ok, "{received:?}"); assert_eq!(received.data["count"], 2);
    let state = server.state.lock().unwrap();
    assert_eq!(state.notification_subscriptions["old-timer"].fired_count, 0);
    assert_eq!(state.receive_receipts["new-read"].worker_id, "new");
    for id in ["pending", "delivered"] {
        assert_eq!(state.msgs[id].wake_attempt_count, 10); assert_eq!(state.msgs[id].state, "read");
        assert!(!state.handoff_wake_pending(&state.msgs[id]));
    }
    let loaded = current_state::load(&server.state_path, false).unwrap();
    assert_eq!(serde_json::to_value(&loaded.msgs).unwrap(), serde_json::to_value(&state.msgs).unwrap());
    assert_eq!(serde_json::to_value(&loaded.receive_receipts).unwrap(), serde_json::to_value(&state.receive_receipts).unwrap());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn name_mailbox_rejected_handoff_keeps_current_state_and_map_unchanged() {
    let (server, root) = peer_tests::test_server();
    let before = std::fs::read(&server.state_path).unwrap();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let map_before = store.load().unwrap();
    let mut state = server.state.lock().unwrap();
    let previous = holder("old", "s1", 1);
    state.name_occupancies.entry("/project".into()).or_default().insert("Alice".into(), previous.clone());
    state.apply(&Event::Sent { msg: message("pending", "pending") });
    state.responsibility_fences.insert("close-new".into(), state::ResponsibilityFence {
        operation_id: "close-new".into(), worker_id: "new".into(), project_scope: "/project".into(),
        app_scope: "app".into(), binding_id: "binding".into(), snapshot: state::ResponsibilitySnapshot::default(),
        state: "closing".into(), created_ms: 1,
    });
    let events = name_resolution::build_name_resolution_events_with_rename_from(&state, "/project", &holder("new", "s2", 2), &[], Some(&previous));
    let error = server.commit_locked_checked(&mut state, &events).unwrap_err();
    assert!(error.to_string().contains("target_closing"), "{error}");
    assert_eq!(state.name_occupancy("/project", "Alice").unwrap().worker_id, "old");
    assert_eq!(state.msgs["pending"].to, "old"); assert!(state.mailbox_handoff_wakes.is_empty());
    assert_eq!(std::fs::read(&server.state_path).unwrap(), before);
    assert_eq!(store.load().unwrap(), map_before);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
