use super::name_state::{AgentPresence, AgentPresenceRecord, NameOccupancy, TaskAssignment};

fn board_name_fixture() -> (Server, std::path::PathBuf, RouteScope) {
    let (server, root) = test_server();
    for worker in ["old", "current"] {
        let response = crate::server::peer_tests::register_synthetic_peer(&server, worker, worker);
        assert!(response.ok, "{:?}", response.error);
    }
    let mut state = server.state.lock().unwrap();
    let route = server_route_scope(&server, &state).unwrap().unwrap();
    state
        .apply_checked(&Event::TaskCreated {
            task: task("named-work", "old"),
        })
        .unwrap();
    state
        .board_details
        .get_mut("named-work")
        .unwrap()
        .public_visibility = true;
    state
        .apply_task_assignment(&TaskAssignment {
            task_id: "named-work".into(),
            name: "Alice".into(),
            session_id: "session-old".into(),
            assignment_revision: 1,
            reflow: None,
            updated_ms: 1,
        })
        .unwrap();
    state
        .name_occupancies
        .entry(route.project_scope_id.as_str().into())
        .or_default()
        .insert(
            "Alice".into(),
            NameOccupancy {
                name: "Alice".into(),
                worker_id: "old".into(),
                session_id: "session-old".into(),
                occupancy_revision: 1,
                updated_ms: 1,
                pane_anchor: None,
            },
        );
    drop(state);
    (server, root, route)
}

fn board_name_presence(
    state: &mut State,
    route: &RouteScope,
    worker: &str,
    session: &str,
    presence: AgentPresence,
) {
    state
        .apply_agent_presence(&AgentPresenceRecord {
            project_scope: route.project_scope_id.as_str().into(),
            worker_id: worker.into(),
            session_id: session.into(),
            presence,
            reason: "test observation".into(),
            observed_ms: 2,
        })
        .unwrap();
}

#[test]
fn board_name_offline_binding_projects_empty_without_mutation() {
    let (server, root, route) = board_name_fixture();
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap();
    let mut state = server.state.lock().unwrap();
    board_name_presence(
        &mut state,
        &route,
        "old",
        "session-old",
        AgentPresence::Offline,
    );
    assert!(current_worker_binding(
        &state,
        &route.project_scope_id,
        Some(&route.app_scope_id),
        &state.workers["old"]
    )
    .is_some());
    let before = serde_json::to_value(&*state).unwrap();
    let view =
        board_task_view_with_master(&state, &control, &state.tasks["named-work"], None, None, Some(&route))
            .unwrap()
            .unwrap();
    assert_eq!(view.owner, "");
    assert!(view.needs_reassignment);
    assert_eq!(view.owner_name.as_deref(), Some("Alice"));
    assert_eq!(view.owner_session_id, None);
    assert_eq!(view.reassignment_reason.as_deref(), Some("owner_offline"));
    assert_eq!(
        serde_json::to_value(&*state).unwrap(),
        before
    );
    drop(state);
    let response = handle_board_show(&server, None);
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.data["tasks"][0]["owner"], "");
    assert_eq!(
        serde_json::to_value(&*server.state.lock().unwrap()).unwrap(),
        before
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_current_holder_and_unknown_presence_preserve_closed_history() {
    let (server, root, route) = board_name_fixture();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let source = control.key_for_worker(&route.project_scope_id, &route.app_scope_id, "current").unwrap().unwrap();
    let value = control.slot(source).unwrap().active_binding().unwrap().clone();
    store.commit(binding_store::ControlChange::MoveName {
        target: binding_slot::IdentityBindingKey::new(route.project_scope_id.clone(), route.app_scope_id.clone(), "Alice"),
        value,
        source_names: vec![source.name.clone()],
        expected_generation: None,
    }).unwrap();
    let control = store.load().unwrap();
    let mut state = server.state.lock().unwrap();
    let occupancy = state
        .name_occupancies
        .get_mut(route.project_scope_id.as_str())
        .unwrap()
        .get_mut("Alice")
        .unwrap();
    occupancy.worker_id = "current".into();
    occupancy.session_id = "session-current".into();
    state
        .task_assignments
        .get_mut("named-work")
        .unwrap()
        .session_id = "session-current".into();
    for presence in [AgentPresence::Online, AgentPresence::Unknown] {
        board_name_presence(&mut state, &route, "current", "session-current", presence);
        let view = board_task_view_with_master(
            &state,
            &control,
            &state.tasks["named-work"],
            None,
            None,
            Some(&route),
        )
        .unwrap().unwrap();
        assert_eq!(view.owner, "current");
        assert_eq!(view.owner_display_name, "Alice");
        let json = serde_json::to_value(view).unwrap();
        assert_eq!(json["owner_name"], "Alice");
        assert_eq!(json["owner_session_id"], "session-current");
        assert_eq!(json["needs_reassignment"], false);
    }
    let mut closed = state.tasks["named-work"].clone();
    closed.status = "closed".into();
    board_name_presence(
        &mut state,
        &route,
        "old",
        "session-old",
        AgentPresence::Offline,
    );
    assert_eq!(
        board_task_view_with_master(&state, &control, &closed, None, None, Some(&route))
            .unwrap()
            .unwrap()
            .owner,
        "old"
    );
    assert_eq!(state.tasks["named-work"].owner, "old");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_mutation_response_reuses_the_admitted_control_snapshot() {
    let (server, root, route) = board_name_fixture();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let source = control
        .key_for_worker(&route.project_scope_id, &route.app_scope_id, "current")
        .unwrap()
        .unwrap();
    let value = control.slot(source).unwrap().active_binding().unwrap().clone();
    store
        .commit(binding_store::ControlChange::MoveName {
            target: binding_slot::IdentityBindingKey::new(
                route.project_scope_id.clone(),
                route.app_scope_id.clone(),
                "Alice",
            ),
            value,
            source_names: vec![control
                .key_for_worker(&route.project_scope_id, &route.app_scope_id, "current")
                .unwrap()
                .unwrap()
                .name
                .clone()],
            expected_generation: None,
        })
        .unwrap();
    let admitted = store.load().unwrap();
    let mut state = server.state.lock().unwrap();
    let occupancy = state
        .name_occupancies
        .get_mut(route.project_scope_id.as_str())
        .unwrap()
        .get_mut("Alice")
        .unwrap();
    occupancy.worker_id = "current".into();
    occupancy.session_id = "session-current".into();
    state
        .task_assignments
        .get_mut("named-work")
        .unwrap()
        .session_id = "session-current".into();
    board_name_presence(
        &mut state,
        &route,
        "current",
        "session-current",
        AgentPresence::Online,
    );
    store.save(&binding_slot::BindingControlSet::new()).unwrap();

    let response = board_update_response(
        &server,
        &state,
        "named-work",
        "current",
        Resp::data(json!({"status": "working"})),
        Some(&admitted),
        Some(&route),
    );

    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["task"]["owner"], "current");
    assert_eq!(response.data["task"]["owner_session_id"], "session-current");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_claim_commits_current_occupancy_assignment() {
    let (server, root, route) = board_name_fixture();
    {
        let mut state = server.state.lock().unwrap();
        state.tasks.get_mut("named-work").unwrap().status = "pending".into();
        state.task_assignments.clear();
    }
    let response = handle_board_command(
        &server,
        "old".into(),
        "token-old".into(),
        crate::board::BoardCommand::Claim {
            id: "named-work".into(),
            expected_revision: 1,
        },
    );
    assert!(response.ok, "{:?}", response.error);
    let state = server.state.lock().unwrap();
    let assignment = state
        .task_assignments
        .get("named-work")
        .expect("claim persists typed assignment");
    assert_eq!(assignment.name, "Alice");
    assert_eq!(assignment.session_id, "session-old");
    assert_eq!(assignment.assignment_revision, 1);
    assert_eq!(state.tasks["named-work"].status, "working");
    assert_eq!(state.board_details["named-work"].revision, 2);
    let restored: State = serde_json::from_slice(&serde_json::to_vec(&*state).unwrap()).unwrap();
    assert_eq!(restored.task_assignment("named-work"), Some(assignment));
    assert_eq!(
        route.project_scope_id.as_str(),
        GlobalState::canonical_project_scope(&root)
            .unwrap()
            .as_str()
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_missing_map_row_does_not_fall_back_to_old_owner() {
    let (server, root, route) = board_name_fixture();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    store.commit(binding_store::ControlChange::Unregister {
        key: binding_slot::IdentityBindingKey::new(route.project_scope_id.clone(), route.app_scope_id.clone(), "Alice"),
        expected_generation: None,
    }).unwrap();
    let control = store.load().unwrap();
    let state = server.state.lock().unwrap();
    let before = serde_json::to_value(&*state).unwrap();
    let view =
        board_task_view_with_master(&state, &control, &state.tasks["named-work"], None, None, Some(&route))
            .unwrap()
            .unwrap();
    assert_eq!(view.owner, "");
    assert_eq!(view.reassignment_reason.as_deref(), Some("name_unoccupied"));
    assert_eq!(
        serde_json::to_value(&*state).unwrap(),
        before
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_old_session_offline_does_not_hide_current_holder() {
    let (server, root, route) = board_name_fixture();
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap();
    let mut state = server.state.lock().unwrap();
    board_name_presence(
        &mut state,
        &route,
        "old",
        "retired-session",
        AgentPresence::Offline,
    );
    let view =
        board_task_view_with_master(&state, &control, &state.tasks["named-work"], None, None, Some(&route))
            .unwrap()
            .unwrap();
    assert_eq!(view.owner, "old");
    assert!(!view.needs_reassignment);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_admitted_mutations_do_not_resolve_route_or_reload_control() {
    let (mut server, root, route) = board_name_fixture();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let admitted = store.load().unwrap();
    // An admitted phase must not need the live authority sources again.
    std::fs::write(store.path(), b"unavailable authority").unwrap();
    server.root = root.join("missing-route-root");
    let mut state = server.state.lock().unwrap();
    assert!(server_route_scope(&server, &state).is_err());
    let before = serde_json::to_value(&*state).unwrap();
    let denied = task_update_locked_at_route(
        &server, &mut state, Some(&admitted), Some(&route), true, false,
        "current".into(), "token-current".into(), "named-work".into(),
        Some("verifying".into()), None, None, None,
    );
    assert!(!denied.ok, "{denied:?}");
    assert_eq!(serde_json::to_value(&*state).unwrap(), before);
    state.board_details.get_mut("named-work").unwrap().work_policy.executor_name = Some("current".into());
    let before = serde_json::to_value(&*state).unwrap();
    let denied = task_update_locked_at_route(
        &server, &mut state, Some(&admitted), Some(&route), true, false,
        "old".into(), "token-old".into(), "named-work".into(),
        Some("verifying".into()), None, None, None,
    );
    assert!(denied.error.as_deref().unwrap().starts_with("WORK_EXECUTOR_REQUIRED:"), "{denied:?}");
    assert_eq!(serde_json::to_value(&*state).unwrap(), before);
    state.board_details.get_mut("named-work").unwrap().work_policy.executor_name = Some("Alice".into());
    for status in ["working", "verifying", "reviewed"] {
        let response = task_update_locked_at_route(
            &server, &mut state, Some(&admitted), Some(&route), true, false,
            "old".into(), "token-old".into(), "named-work".into(),
            Some(status.into()), None, None, None,
        );
        assert!(response.ok, "{status}: {response:?}");
        assert_eq!(response.data["task"]["status"], status);
    }
    let revision = state.board_details["named-work"].revision;
    let response = board_describe_locked(
        &server, &mut state, Some(&admitted), Some(&route),
        "old".into(), "named-work".into(), revision,
        "title".into(), "description".into(), "delivery".into(), "test".into(),
        Default::default(),
    );
    assert!(response.ok, "{response:?}");
    let before = serde_json::to_value(&*state).unwrap();
    let response = board_describe_locked(
        &server, &mut state, Some(&admitted), Some(&route),
        "current".into(), "named-work".into(), revision + 1,
        "title".into(), "description".into(), "delivery".into(), "test".into(),
        Default::default(),
    );
    assert!(!response.ok, "{response:?}");
    assert_eq!(serde_json::to_value(&*state).unwrap(), before);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_current_map_update_describe_reuse_admitted_actor_and_route() {
    let (server, root, route) = board_name_fixture();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let binding = control.current_for_worker(&route.project_scope_id, &route.app_scope_id, "old")
        .unwrap().unwrap().active_binding().unwrap().binding.clone();
    let runtime = crate::identity::RuntimeIdentity {
        agent_id: binding.agent_id,
        runtime_id: binding.runtime_id,
        appserver_id: binding.app_scope_id,
        binding_id: binding.binding_id,
        session_id: binding.session_id,
        native_thread_id: binding.native_thread_id,
    };
    let context = ProjectContext::for_registered_route(&root, &runtime).unwrap();
    let server = Arc::new(server);
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    manager.install_runtime(&ProjectRuntimeManager::route_key(&context), server.clone(), None);
    for command in [
        crate::board::BoardCommand::Update {
            id: "named-work".into(), expected_revision: 1,
            status: None, next: Some("continue admitted work".into()),
        },
        crate::board::BoardCommand::Describe {
            id: "named-work".into(), expected_revision: 2,
            title: "title".into(), description: "description".into(),
            delivery_condition: "delivery".into(), test_condition: "test".into(),
            long_horizon: crate::board::LongHorizonArgs {
                long_horizon_content: Some("continue the task".into()),
                long_horizon_every_ms: Some(60_000),
                ..Default::default()
            },
        },
    ] {
        let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        let denied = dispatch_board_command_current_map(
            &manager, &server, Some(&context), "old".into(), "wrong-token".into(), &command,
        );
        assert!(!denied.ok, "{denied:?}");
        assert_eq!(serde_json::to_value(&*server.state.lock().unwrap()).unwrap(), before);
        let path = store.path().to_owned();
        BOARD_ADMITTED_TEST_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(move |state| {
            // Any repeated token comparison, control load or route lookup must fail.
            state.workers.get_mut("old").unwrap().token = "post-admission-token".into();
            let project = state.global.projects.values_mut().next().unwrap();
            let mut registration = project.registrations.values().next().unwrap().clone();
            registration.app_scope_id = crate::identity::AppServerId::new("other-app").unwrap();
            project.registrations.insert("other-app".into(), registration);
            let mut extra = project.runtime_bindings.values().next().unwrap().clone();
            extra.app_scope_id = crate::identity::AppServerId::new("other-app").unwrap();
            extra.binding_id = crate::identity::BindingId::new("post-admission-route").unwrap();
            extra.runtime_id = crate::identity::RuntimeId::new("post-admission-runtime").unwrap();
            project.runtime_bindings.insert("post-admission-route".into(), extra);
            std::fs::write(path, b"unavailable authority").unwrap();
        })));
        let response = dispatch_board_command_current_map(
            &manager, &server, Some(&context), "old".into(), "token-old".into(), &command,
        );
        assert!(response.ok, "{command:?}: {response:?}");
        assert!(BOARD_ADMITTED_TEST_HOOK.with(|hook| hook.borrow().is_none()));
        let mut state = server.state.lock().unwrap();
        assert!(server_route_scope(&server, &state).is_err());
        assert_eq!(response.data["task"]["id"], "named-work");
        state.workers.get_mut("old").unwrap().token = "token-old".into();
        for project in state.global.projects.values_mut() {
            project.runtime_bindings.remove("post-admission-route");
            project.registrations.remove("other-app");
        }
        store.save(&control).unwrap();
    }
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["named-work"].next_step.as_deref(), Some("continue admitted work"));
    assert_eq!(state.board_details["named-work"].long_horizon.as_ref().unwrap().content, "continue the task");
    assert!(state.notification_subscriptions.values().any(|sub| sub.subject.as_deref() == Some("goal:task:named-work")));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_describe_terminal_schedule_reuses_admitted_route_without_rearm() {
    for terminal in ["expired", "consumed"] {
        let (server, root, route) = board_name_fixture();
        let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
        let control = store.load().unwrap();
        let binding = control.current_for_worker(&route.project_scope_id, &route.app_scope_id, "old")
            .unwrap().unwrap().active_binding().unwrap().binding.clone();
        let runtime = crate::identity::RuntimeIdentity {
            agent_id: binding.agent_id,
            runtime_id: binding.runtime_id,
            appserver_id: binding.app_scope_id,
            binding_id: binding.binding_id,
            session_id: binding.session_id,
            native_thread_id: binding.native_thread_id,
        };
        let context = ProjectContext::for_registered_route(&root, &runtime).unwrap();
        let server = Arc::new(server);
        let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
        manager.install_runtime(&ProjectRuntimeManager::route_key(&context), server.clone(), None);
        let describe = |revision, content: &str| crate::board::BoardCommand::Describe {
            id: "named-work".into(), expected_revision: revision,
            title: "title".into(), description: "description".into(),
            delivery_condition: "delivery".into(), test_condition: "test".into(),
            long_horizon: crate::board::LongHorizonArgs {
                long_horizon_content: Some(content.into()),
                long_horizon_every_ms: Some(60_000),
                ..Default::default()
            },
        };
        let response = dispatch_board_command_current_map(
            &manager, &server, Some(&context), "old".into(), "token-old".into(),
            &describe(1, "original goal"),
        );
        assert!(response.ok, "{response:?}");
        let before_subscriptions = {
            let mut state = server.state.lock().unwrap();
            let subscription = state.notification_subscriptions.values_mut()
                .find(|sub| sub.subject.as_deref() == Some("goal:task:named-work")).unwrap();
            subscription.status = terminal.into();
            if terminal == "expired" {
                subscription.expires_ms = now_ms() - 1;
            } else {
                subscription.fired_count = subscription.repeat_count;
            }
            serde_json::to_value(&state.notification_subscriptions).unwrap()
        };
        BOARD_ADMITTED_TEST_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(|state| {
            let project = state.global.projects.values_mut().next().unwrap();
            let mut registration = project.registrations.values().next().unwrap().clone();
            registration.app_scope_id = crate::identity::AppServerId::new("other-app").unwrap();
            project.registrations.insert("other-app".into(), registration);
            let mut extra = project.runtime_bindings.values().next().unwrap().clone();
            extra.app_scope_id = crate::identity::AppServerId::new("other-app").unwrap();
            extra.binding_id = crate::identity::BindingId::new("post-admission-route").unwrap();
            extra.runtime_id = crate::identity::RuntimeId::new("post-admission-runtime").unwrap();
            project.runtime_bindings.insert("post-admission-route".into(), extra);
        })));
        let response = dispatch_board_command_current_map(
            &manager, &server, Some(&context), "old".into(), "token-old".into(),
            &describe(2, "edited terminal goal"),
        );
        assert!(BOARD_ADMITTED_TEST_HOOK.with(|hook| hook.borrow().is_none()));
        let state = server.state.lock().unwrap();
        assert!(server_route_scope(&server, &state).is_err());
        assert!(response.ok, "{terminal}: {response:?}");
        assert_eq!(response.data["task"]["id"], "named-work");
        assert_eq!(state.board_details["named-work"].revision, 3);
        assert_eq!(state.board_details["named-work"].long_horizon.as_ref().unwrap().content,
            "edited terminal goal");
        assert_eq!(serde_json::to_value(&state.notification_subscriptions).unwrap(), before_subscriptions);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn board_name_describe_subscription_limit_preserves_durability_error() {
    let (server, root, _) = board_name_fixture();
    {
        let mut state = server.state.lock().unwrap();
        let template = state.notification_subscriptions.values()
            .find(|sub| sub.worker_id == "old" && sub.status == "armed").unwrap().clone();
        let active = state.notification_subscriptions.values()
            .filter(|sub| sub.worker_id == "old" && sub.status == "armed").count();
        for index in active..3 {
            let mut subscription = template.clone();
            subscription.id = format!("capacity-{index}");
            subscription.event = "deadline".into();
            subscription.subject = Some(format!("goal:capacity-{index}"));
            subscription.trigger_ms = Some(now_ms() + 60_000);
            state.apply_checked(&Event::NotificationSubscribed { subscription }).unwrap();
        }
        assert_eq!(state.notification_subscriptions.values()
            .filter(|sub| sub.worker_id == "old" && sub.status == "armed").count(), 3);
    }
    let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    let response = handle_board_command(&server, "old".into(), "token-old".into(),
        crate::board::BoardCommand::Describe {
            id: "named-work".into(), expected_revision: 1,
            title: "title".into(), description: "description".into(),
            delivery_condition: "delivery".into(), test_condition: "test".into(),
            long_horizon: crate::board::LongHorizonArgs {
                long_horizon_content: Some("new scheduled goal".into()),
                long_horizon_every_ms: Some(60_000),
                ..Default::default()
            },
        });
    assert_eq!(response.error.as_deref(),
        Some("BOARD_DURABILITY_FAILED: invalid command: maximum 3 active subscriptions per agent"));
    assert_eq!(serde_json::to_value(&*server.state.lock().unwrap()).unwrap(), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_legacy_missing_route_preserves_notice_error_without_mutation() {
    let (server, root, _) = board_name_fixture();
    {
        let mut state = server.state.lock().unwrap();
        state.task_assignments.clear();
        state.global.projects.clear();
        assert!(server_route_scope(&server, &state).unwrap().is_none());
    }
    let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    let response = handle_board_command(&server, "old".into(), "token-old".into(),
        crate::board::BoardCommand::Update {
            id: "named-work".into(), expected_revision: 1,
            status: None, next: Some("next-only update".into()),
        });
    assert_eq!(response.error.as_deref(), Some("BOARD_NOTICE_RECIPIENT_UNRESOLVED: current project route is missing"));
    assert_eq!(serde_json::to_value(&*server.state.lock().unwrap()).unwrap(), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_legacy_invalid_status_precedes_unavailable_authority() {
    let (server, root, _) = board_name_fixture();
    {
        let mut state = server.state.lock().unwrap();
        state.task_assignments.clear();
    }
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    std::fs::write(store.path(), b"invalid control").unwrap();
    let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    let response = handle_board_command(
        &server,
        "old".into(),
        "token-old".into(),
        crate::board::BoardCommand::Update {
            id: "named-work".into(),
            expected_revision: 1,
            status: Some("invalid-status".into()),
            next: None,
        },
    );
    assert!(!response.ok);
    assert!(response.error.as_deref().unwrap().starts_with("invalid status invalid-status;"), "{response:?}");
    assert_eq!(serde_json::to_value(&*server.state.lock().unwrap()).unwrap(), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_legacy_invitation_defaults_original_fields() {
    let invitation: crate::board::BoardInvitation = serde_json::from_value(json!({
        "peer_id": "peer", "binding_id": "binding",
        "message_id": "message", "created_ms": 1,
    }))
    .unwrap();
    assert!(invitation.original_status.is_none());
    assert!(invitation.original_owner.is_none());
}
