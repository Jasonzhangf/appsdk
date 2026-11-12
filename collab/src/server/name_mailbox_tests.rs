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
fn name_mailbox_source_name_address_is_not_reassigned_to_target_name() {
    let mut state = State::default();
    for (id, target, name) in [
        ("frank-message", "frank", "Frank"),
        ("bob-message", "bob", "Bob"),
    ] {
        state
            .apply_checked(&Event::Sent {
                msg: Message {
                    id: id.into(),
                    from: "sender".into(),
                    to: target.into(),
                    mtype: "notify".into(),
                    subject: Some(id.into()),
                    body: "durable body".into(),
                    in_reply_to: None,
                    created_ms: now_ms(),
                    state: "pending".into(),
                    wake_attempt_count: 0,
                    last_wake_attempt_ms: 0,
                    retry_attempted: false,
                },
            })
            .unwrap();
        state
            .apply_checked(&Event::MailboxNameAddressed {
                message_id: id.into(),
                address: name_mailbox::MailboxNameAddress {
                    project_scope: "/project".into(),
                    name: name.into(),
                },
            })
            .unwrap();
    }
    let previous = NameOccupancy {
        name: "Bob".into(),
        worker_id: "frank".into(),
        session_id: "session-before".into(),
        occupancy_revision: 1,
        updated_ms: 1,
        pane_anchor: None,
    };
    let next = NameOccupancy {
        name: "Bob".into(),
        worker_id: "frank".into(),
        session_id: "session-after".into(),
        occupancy_revision: 2,
        updated_ms: 2,
        pane_anchor: None,
    };
    let events = name_mailbox::handoff_events(
        &state,
        "/project",
        &next,
        &["Frank".into()],
        Some(&previous),
    );
    assert_eq!(events.len(), 1, "only the target name may move: {events:?}");
    for event in events {
        state.apply_checked(&event).unwrap();
    }
    assert_eq!(state.msgs["bob-message"].to, "frank");
    assert_eq!(state.mailbox_name_addresses["bob-message"].name, "Bob");
    assert_eq!(state.msgs["frank-message"].to, "frank");
    assert_eq!(state.mailbox_name_addresses["frank-message"].name, "Frank");
}

#[test]
fn mailbox_current_name_gate_filters_released_source_name_for_reused_actor() {
    let mut state = State::default();
    for (id, name, status) in [
        ("frank-typed", "Frank", "pending"),
        ("bob-typed", "Bob", "pending"),
        ("raw-actor", "Frank", "pending"),
        ("frank-consumed", "Frank", "read"),
    ] {
        state
            .apply_checked(&Event::Sent {
                msg: Message {
                    id: id.into(),
                    from: "sender".into(),
                    to: "actor".into(),
                    mtype: "notify".into(),
                    subject: Some(id.into()),
                    body: "durable body".into(),
                    in_reply_to: None,
                    created_ms: now_ms(),
                    state: status.into(),
                    wake_attempt_count: 0,
                    last_wake_attempt_ms: 0,
                    retry_attempted: false,
                },
            })
            .unwrap();
        if id != "raw-actor" {
            state
                .apply_checked(&Event::MailboxNameAddressed {
                    message_id: id.into(),
                    address: name_mailbox::MailboxNameAddress {
                        project_scope: "/project".into(),
                        name: name.into(),
                    },
                })
                .unwrap();
        }
    }
    // The same actor now holds only Bob. Frank was released and must wait for
    // a legitimate Frank holder; raw actor mail keeps its direct-send meaning.
    state
        .name_occupancies
        .entry("/project".into())
        .or_default()
        .insert(
            "Bob".into(),
            NameOccupancy {
                name: "Bob".into(),
                worker_id: "actor".into(),
                session_id: "session-bob".into(),
                occupancy_revision: 2,
                updated_ms: 2,
                pane_anchor: None,
            },
        );
    let ids: Vec<String> = state
        .inbox_of("actor")
        .iter()
        .map(|m| m.id.clone())
        .collect();
    assert!(
        ids.contains(&"bob-typed".into()),
        "current Bob typed mail stays receivable: {ids:?}"
    );
    assert!(
        ids.contains(&"raw-actor".into()),
        "raw actor mail keeps direct-send semantics: {ids:?}"
    );
    assert!(
        !ids.contains(&"frank-typed".into()),
        "released Frank typed mail must wait for its current holder: {ids:?}"
    );
    assert!(
        !ids.contains(&"frank-consumed".into()),
        "consumed history stays out of the inbox: {ids:?}"
    );
    // Explicit and bulk ack plus wake candidate selection reuse this same
    // current-recipient predicate, so this fixture gates the shared consumer.
    assert!(!state.message_receivable_by("actor", &state.msgs["frank-typed"]));
    assert!(state.message_receivable_by("actor", &state.msgs["bob-typed"]));
    assert!(state.message_receivable_by("actor", &state.msgs["raw-actor"]));
}

#[test]
fn mailbox_current_name_gate_covers_inbox_explicit_and_bulk_ack() {
    let (server, root) = peer_tests::test_server();
    assert!(peer_tests::register_synthetic_peer(&server, "actor", "actor").ok);
    let token = server
        .state
        .lock()
        .unwrap()
        .workers
        .get("actor")
        .unwrap()
        .token
        .clone();

    for (id, status) in [
        ("frank-explicit", "pending"),
        ("bob-explicit", "pending"),
        ("raw-explicit", "pending"),
        ("frank-bulk", "pending"),
        ("bob-bulk", "pending"),
        ("raw-bulk", "pending"),
    ] {
        let mut message = message(id, status);
        message.to = "actor".into();
        server
            .commit_checked(&[Event::Sent { msg: message }])
            .unwrap();
    }
    for (id, name) in [
        ("frank-explicit", "Frank"),
        ("bob-explicit", "Bob"),
        ("frank-bulk", "Frank"),
        ("bob-bulk", "Bob"),
    ] {
        server
            .commit_checked(&[Event::MailboxNameAddressed {
                message_id: id.into(),
                address: name_mailbox::MailboxNameAddress {
                    project_scope: "/project".into(),
                    name: name.into(),
                },
            }])
            .unwrap();
    }
    server
        .commit_checked(&[Event::NameResolutionCommitted {
            project_scope: "/project".into(),
            occupancy: NameOccupancy {
                name: "Bob".into(),
                worker_id: "actor".into(),
                session_id: "session-actor".into(),
                occupancy_revision: 2,
                updated_ms: 2,
                pane_anchor: None,
            },
            assignments: Vec::new(),
        }])
        .unwrap();

    let server = Arc::new(server);
    let inbox = dispatch_with_route_context(
        &server,
        Req::Inbox {
            worker_id: "actor".into(),
            token: token.clone(),
        },
        None,
    );
    assert!(inbox.ok, "{inbox:?}");
    let inbox_ids: Vec<&str> = inbox.data["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["id"].as_str())
        .collect();
    assert!(inbox_ids.contains(&"bob-explicit"), "{inbox_ids:?}");
    assert!(inbox_ids.contains(&"raw-explicit"), "{inbox_ids:?}");
    assert!(!inbox_ids.contains(&"frank-explicit"), "{inbox_ids:?}");

    let explicit = dispatch_with_route_context(
        &server,
        Req::Ack {
            worker_id: "actor".into(),
            token: token.clone(),
            ids: vec!["frank-explicit".into()],
        },
        None,
    );
    assert!(explicit.ok, "{explicit:?}");
    assert_eq!(explicit.data["not_found"], json!(["frank-explicit"]));
    assert_eq!(explicit.data["acked"], json!([]));

    let explicit = dispatch_with_route_context(
        &server,
        Req::Ack {
            worker_id: "actor".into(),
            token: token.clone(),
            ids: vec!["bob-explicit".into(), "raw-explicit".into()],
        },
        None,
    );
    assert!(explicit.ok, "{explicit:?}");
    assert_eq!(
        explicit.data["acked"],
        json!(["bob-explicit", "raw-explicit"])
    );

    let bulk = dispatch_with_route_context(
        &server,
        Req::Ack {
            worker_id: "actor".into(),
            token,
            ids: Vec::new(),
        },
        None,
    );
    assert!(bulk.ok, "{bulk:?}");
    let bulk_acked = bulk.data["acked"].as_array().unwrap();
    assert!(bulk_acked.iter().any(|id| id == "bob-bulk"), "{bulk:?}");
    assert!(bulk_acked.iter().any(|id| id == "raw-bulk"), "{bulk:?}");
    assert!(!bulk_acked.iter().any(|id| id == "frank-bulk"), "{bulk:?}");

    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs["frank-explicit"].state, "pending");
    assert_eq!(state.msgs["frank-bulk"].state, "pending");
    assert_eq!(state.msgs["bob-bulk"].state, "read");
    assert_eq!(state.msgs["raw-bulk"].state, "read");
    drop(state);
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

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

#[test]
fn notification_attempt_rechecks_current_map_before_waking_old_holder() {
    let (mut server, root) = peer_tests::test_server();
    server.config.notifications.batch_window_seconds = 0;
    let observed_threads = Arc::new(Mutex::new(Vec::new()));
    let sink_threads = observed_threads.clone();
    server.appserver_notification_sink = Arc::new(move |transport, _, _, _, _, _| {
        sink_threads.lock().unwrap().push(transport.thread_id.clone());
        Ok(json!({"accepted": true}))
    });
    let server = Arc::new(server);
    let app = AppServerId::new("notification-current-map").unwrap();
    let old_worker = "wake-map-old";
    let new_worker = "wake-map-new";
    for worker in [old_worker, new_worker] {
        let transport = peer_tests::test_appserver_transport(&format!("thread-{worker}"));
        assert!(peer_tests::seed_committed_owner(
            &server,
            worker,
            &format!("token-{worker}"),
            &transport,
            &root.display().to_string(),
            Some(app.clone()),
        )
        .ok);
    }

    let scope = GlobalState::canonical_project_scope(&root).unwrap();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let old_key = control
        .key_for_worker(&scope, &app, old_worker)
        .unwrap()
        .unwrap()
        .clone();
    let old_name = old_key.name.clone();
    let new_key = control
        .key_for_worker(&scope, &app, new_worker)
        .unwrap()
        .unwrap()
        .clone();
    let new_value = control
        .current_for_worker(&scope, &app, new_worker)
        .unwrap()
        .unwrap()
        .active_binding()
        .unwrap()
        .clone();
    let old_subscription = default_direct_message_id(old_worker);
    let typed_id = "wake-map-typed";
    let raw_id = "wake-map-raw";
    let mut events = vec![
        Event::Sent {
            msg: Message {
                id: typed_id.into(),
                from: "sender".into(),
                to: old_worker.into(),
                mtype: "notify".into(),
                subject: Some(typed_id.into()),
                body: "typed wake must follow current map".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::MailboxNameAddressed {
            message_id: typed_id.into(),
            address: name_mailbox::MailboxNameAddress {
                project_scope: scope.as_str().into(),
                name: old_name.clone(),
            },
        },
        Event::WakeBound {
            message_id: typed_id.into(),
            subscription_id: old_subscription.clone(),
        },
        Event::DeliveryMode {
            msg_id: typed_id.into(),
            mode: "immediate".into(),
            source_thread_id: None,
        },
    ];
    server.commit_checked(&events).unwrap();
    events.clear();

    store
        .commit(binding_store::ControlChange::MoveName {
            target: old_key,
            value: new_value,
            source_names: vec![new_key.name],
            expected_generation: None,
        })
        .unwrap();
    assert!(!attempt_notification_with_at(
        &server,
        typed_id,
        &old_subscription,
        now_ms(),
    ));
    assert_eq!(*observed_threads.lock().unwrap(), Vec::<Option<String>>::new());
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[typed_id].state, "pending");
        assert!(!state.notification_delivery_accepted.contains_key(typed_id));
    }

    events.extend([
        Event::Sent {
            msg: Message {
                id: raw_id.into(),
                from: "sender".into(),
                to: old_worker.into(),
                mtype: "notify".into(),
                subject: Some(raw_id.into()),
                body: "raw direct wake must keep direct semantics".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::WakeBound {
            message_id: raw_id.into(),
            subscription_id: old_subscription.clone(),
        },
        Event::DeliveryMode {
            msg_id: raw_id.into(),
            mode: "immediate".into(),
            source_thread_id: None,
        },
    ]);
    server.commit_checked(&events).unwrap();
    assert!(attempt_notification_with_at(
        &server,
        raw_id,
        &old_subscription,
        now_ms(),
    ));
    assert_eq!(
        *observed_threads.lock().unwrap(),
        vec![Some("thread-wake-map-old".into())]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn late_notification_acceptance_stays_with_original_holder_after_name_handoff() {
    use std::sync::mpsc;

    let (mut server, root) = peer_tests::test_server();
    server.config.notifications.batch_window_seconds = 0;
    let server = Arc::new(server);
    let app = AppServerId::new("notification-late-map").unwrap();
    let old_worker = "late-wake-old";
    let new_worker = "late-wake-new";
    for worker in [old_worker, new_worker] {
        let transport = peer_tests::test_appserver_transport(&format!("thread-{worker}"));
        assert!(peer_tests::seed_committed_owner(
            &server,
            worker,
            &format!("token-{worker}"),
            &transport,
            &root.display().to_string(),
            Some(app.clone()),
        )
        .ok);
    }

    let scope = GlobalState::canonical_project_scope(&root).unwrap();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let old_key = control
        .key_for_worker(&scope, &app, old_worker)
        .unwrap()
        .unwrap()
        .clone();
    let old_name = old_key.name.clone();
    let old_binding = control
        .current_for_worker(&scope, &app, old_worker)
        .unwrap()
        .unwrap()
        .active_binding()
        .unwrap()
        .clone();
    let new_key = control
        .key_for_worker(&scope, &app, new_worker)
        .unwrap()
        .unwrap()
        .clone();
    let new_binding = control
        .current_for_worker(&scope, &app, new_worker)
        .unwrap()
        .unwrap()
        .active_binding()
        .unwrap()
        .clone();
    let new_session = new_binding
        .binding
        .session_id
        .as_ref()
        .unwrap()
        .to_string();
    let subscription_id = default_direct_message_id(old_worker);
    let message_id = "late-wake-typed";
    server
        .commit_checked(&[
            Event::Sent {
                msg: Message {
                    id: message_id.into(),
                    from: "sender".into(),
                    to: old_worker.into(),
                    mtype: "notify".into(),
                    subject: Some(message_id.into()),
                    body: "accepted by the old endpoint after handoff".into(),
                    in_reply_to: None,
                    created_ms: now_ms(),
                    state: "pending".into(),
                    wake_attempt_count: 0,
                    last_wake_attempt_ms: 0,
                    retry_attempted: false,
                },
            },
            Event::MailboxNameAddressed {
                message_id: message_id.into(),
                address: name_mailbox::MailboxNameAddress {
                    project_scope: scope.as_str().into(),
                    name: old_name.clone(),
                },
            },
            Event::WakeBound {
                message_id: message_id.into(),
                subscription_id: subscription_id.clone(),
            },
            Event::DeliveryMode {
                msg_id: message_id.into(),
                mode: "immediate".into(),
                source_thread_id: None,
            },
        ])
        .unwrap();

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let callback_owner = old_worker.to_owned();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let mut worker = Arc::try_unwrap(server).ok().unwrap();
    worker.appserver_notification_sink = Arc::new(move |_, _, _, _, _, _| {
        entered_tx.send(()).unwrap();
        release_rx.lock().unwrap().recv().unwrap();
        Ok(json!({"accepted": true, "owner": callback_owner}))
    });
    let server = Arc::new(worker);
    let delivery_server = server.clone();
    let delivery = std::thread::spawn(move || {
        attempt_notification_detailed_with_at(
            &delivery_server,
            message_id,
            &subscription_id,
            now_ms(),
        )
    });
    entered_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();

    store
        .commit(binding_store::ControlChange::MoveName {
            target: old_key.clone(),
            value: new_binding,
            source_names: vec![new_key.name],
            expected_generation: None,
        })
        .unwrap();
    let previous = name_state::NameOccupancy {
        name: old_name.clone(),
        worker_id: old_worker.into(),
        session_id: old_binding.binding.session_id.as_ref().unwrap().to_string(),
        occupancy_revision: 1,
        updated_ms: 1,
        pane_anchor: None,
    };
    let next = name_state::NameOccupancy {
        name: old_name.clone(),
        worker_id: new_worker.into(),
        session_id: new_session,
        occupancy_revision: 2,
        updated_ms: 2,
        pane_anchor: None,
    };
    let mut handoff = vec![Event::NameResolutionCommitted {
        project_scope: scope.as_str().into(),
        occupancy: next.clone(),
        assignments: Vec::new(),
    }];
    handoff.extend({
        let state = server.state.lock().unwrap();
        name_mailbox::handoff_events(
            &state,
            scope.as_str(),
            &next,
            &[],
            Some(&previous),
        )
    });
    server.commit_checked(&handoff).unwrap();

    release_tx.send(()).unwrap();
    assert!(delivery.join().unwrap().accepted());
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[message_id].to, new_worker);
    assert!(state.handoff_wake_pending(&state.msgs[message_id]));
    assert!(!state.notification_delivery_accepted.contains_key(message_id));
    assert!(!state.notification_delivery_evidence.contains_key(message_id));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn notification_attempt_rechecks_same_principal_exact_binding_after_map_refresh() {
    let (mut server, root) = peer_tests::test_server();
    server.config.notifications.batch_window_seconds = 0;
    let observed_threads = Arc::new(Mutex::new(Vec::new()));
    let sink_threads = observed_threads.clone();
    server.appserver_notification_sink = Arc::new(move |transport, _, _, _, _, _| {
        sink_threads.lock().unwrap().push(transport.thread_id.clone());
        Ok(json!({"accepted": true}))
    });
    let server = Arc::new(server);
    let app = AppServerId::new("notification-exact-map").unwrap();
    let worker = "wake-exact-old";
    let transport = peer_tests::test_appserver_transport("thread-wake-exact-old");
    assert!(peer_tests::seed_committed_owner(
        &server,
        worker,
        "token-wake-exact-old",
        &transport,
        &root.display().to_string(),
        Some(app.clone()),
    )
    .ok);

    let scope = GlobalState::canonical_project_scope(&root).unwrap();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let key = control
        .key_for_worker(&scope, &app, worker)
        .unwrap()
        .unwrap()
        .clone();
    let name = key.name.clone();
    let old_value = control
        .current_for_worker(&scope, &app, worker)
        .unwrap()
        .unwrap()
        .active_binding()
        .unwrap()
        .clone();
    let mut refreshed = old_value;
    refreshed.binding.session_id =
        Some(crate::identity::SessionId::new("session-wake-exact-refreshed").unwrap());

    let subscription_id = default_direct_message_id(worker);
    let typed_id = "wake-exact-typed";
    let raw_id = "wake-exact-raw";
    server
        .commit_checked(&[
            Event::Sent {
                msg: Message {
                    id: typed_id.into(),
                    from: "sender".into(),
                    to: worker.into(),
                    mtype: "notify".into(),
                    subject: Some(typed_id.into()),
                    body: "typed wake must match the refreshed binding".into(),
                    in_reply_to: None,
                    created_ms: now_ms(),
                    state: "pending".into(),
                    wake_attempt_count: 0,
                    last_wake_attempt_ms: 0,
                    retry_attempted: false,
                },
            },
            Event::MailboxNameAddressed {
                message_id: typed_id.into(),
                address: name_mailbox::MailboxNameAddress {
                    project_scope: scope.as_str().into(),
                    name,
                },
            },
            Event::WakeBound {
                message_id: typed_id.into(),
                subscription_id: subscription_id.clone(),
            },
            Event::DeliveryMode {
                msg_id: typed_id.into(),
                mode: "immediate".into(),
                source_thread_id: None,
            },
        ])
        .unwrap();
    store
        .commit(binding_store::ControlChange::Commit {
            key,
            value: refreshed,
            expected_generation: None,
        })
        .unwrap();

    assert!(!attempt_notification_with_at(
        &server,
        typed_id,
        &subscription_id,
        now_ms(),
    ));
    assert_eq!(*observed_threads.lock().unwrap(), Vec::<Option<String>>::new());
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[typed_id].state, "pending");
        assert!(!state.notification_delivery_accepted.contains_key(typed_id));
    }

    server
        .commit_checked(&[
            Event::Sent {
                msg: Message {
                    id: raw_id.into(),
                    from: "sender".into(),
                    to: worker.into(),
                    mtype: "notify".into(),
                    subject: Some(raw_id.into()),
                    body: "raw direct wake keeps direct semantics".into(),
                    in_reply_to: None,
                    created_ms: now_ms(),
                    state: "pending".into(),
                    wake_attempt_count: 0,
                    last_wake_attempt_ms: 0,
                    retry_attempted: false,
                },
            },
            Event::WakeBound {
                message_id: raw_id.into(),
                subscription_id: subscription_id.clone(),
            },
            Event::DeliveryMode {
                msg_id: raw_id.into(),
                mode: "immediate".into(),
                source_thread_id: None,
            },
        ])
        .unwrap();
    assert!(attempt_notification_with_at(
        &server,
        raw_id,
        &subscription_id,
        now_ms(),
    ));
    assert_eq!(
        *observed_threads.lock().unwrap(),
        vec![Some("thread-wake-exact-old".into())]
    );
    std::fs::remove_dir_all(root).unwrap();
}
