use super::*;

fn request(actor: &str, owner: &str, goal: &str, desired: &str) -> Req {
    Req::GoalScheduleReconcile {
        worker_id: actor.into(),
        token: format!("token-{actor}"),
        goal_id: goal.into(),
        goal_owner: owner.into(),
        project: None,
        app: None,
        content: (desired == "active").then(|| "Continue the owned work".into()),
        desired: desired.into(),
        trigger_ms: None,
        ttl_seconds: Some(3600),
        interval_ms: Some(60000),
        repeat_count: Some(3),
        goal_subject: None,
        reason: None,
        operation: None,
    }
}

fn fixture() -> (Arc<Server>, std::path::PathBuf, String, String) {
    let (mut server, root) = peer_tests::test_server();
    server.root = std::fs::canonicalize(&root).unwrap();
    let server = Arc::new(server);
    for worker in ["owner", "manager", "intruder"] {
        let transport = SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("test://goal-management".into()),
            namespace: Some("test".into()),
            session_id: Some(format!("session-{worker}")),
            thread_id: Some(format!("thread-{worker}")),
            tmux_endpoint: None,
            rex_endpoint: None,
            capabilities: vec!["send_message_to_thread".into()],
            self_check: "isolated non-tmux consumer".into(),
        };
        let registered = peer_tests::seed_committed_owner(
            &server,
            worker,
            &format!("token-{worker}"),
            &transport,
            root.to_str().unwrap(),
            Some(AppServerId::new("tui-default").unwrap()),
        );
        assert!(registered.ok, "{registered:?}");
    }
    peer_tests::promote_master(&server, "manager", "isolated test manager");
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let route = server_route_scope(&server, &server.state.lock().unwrap())
        .unwrap()
        .unwrap();
    let key = control
        .key_for_worker(&route.project_scope_id, &route.app_scope_id, "owner")
        .unwrap()
        .unwrap()
        .clone();
    let name = key.name.clone();
    let active = dispatch(&server, request("owner", &name, "owned", "active"));
    assert!(active.ok, "{active:?}");
    let id = active.data["subscription"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    store
        .commit(binding_store::ControlChange::Unregister {
            key,
            expected_generation: None,
        })
        .unwrap();
    server
        .state
        .lock()
        .unwrap()
        .workers
        .get_mut("owner")
        .unwrap()
        .transport = None;
    (server, root, name, id)
}

#[test]
fn edit_existing_goal_without_holder_preserves_batch_and_rejects_other_actor() {
    for status in [
        "armed",
        "waiting-owner",
        "consumed",
        "expired",
        "cancelled",
        "suppressed",
    ] {
        let (server, root, name, id) = fixture();
        server
            .state
            .lock()
            .unwrap()
            .notification_subscriptions
            .get_mut(&id)
            .unwrap()
            .status = status.into();
        let before = server.state.lock().unwrap().notification_subscriptions[&id].clone();
        let edit = |actor: &str| {
            let mut req = request(actor, &name, "owned", "active");
            if let Req::GoalScheduleReconcile {
                operation,
                content,
                interval_ms,
                repeat_count,
                ttl_seconds,
                ..
            } = &mut req
            {
                *operation = Some("edit".into());
                *content = Some("Edited without a carrier".into());
                *interval_ms = None;
                *repeat_count = None;
                *ttl_seconds = None;
            }
            req
        };
        let snapshot = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        let denied = dispatch(&server, edit("intruder"));
        assert!(!denied.ok, "{denied:?}");
        assert_eq!(
            serde_json::to_value(&*server.state.lock().unwrap()).unwrap(),
            snapshot
        );
        let result = dispatch(&server, edit("manager"));
        assert!(result.ok, "no-holder edit {status}: {result:?}");
        let state = server.state.lock().unwrap();
        let after = &state.notification_subscriptions[&id];
        assert_eq!(after.status, before.status);
        assert_eq!(after.fired_count, before.fired_count);
        assert_eq!(after.trigger_ms, before.trigger_ms);
        assert_eq!(after.expires_ms, before.expires_ms);
        assert_eq!(after.repeat_count, before.repeat_count);
        assert_eq!(after.interval_ms, before.interval_ms);
        assert_eq!(
            after.goal_content.as_ref().unwrap().source,
            Some(GoalScheduleContent::Inline {
                content: "Edited without a carrier".into()
            })
        );
        drop(state);
        let repeat = dispatch(&server, edit("owner"));
        assert!(repeat.ok, "{repeat:?}");
        assert_eq!(repeat.data["deduplicated"], true);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn master_and_owner_end_existing_goal_without_active_binding() {
    for actor in ["manager", "owner"] {
        let (server, root, name, id) = fixture();
        let before = server.state.lock().unwrap().notification_subscriptions[&id].clone();
        let unrelated: Vec<_> = server
            .state
            .lock()
            .unwrap()
            .notification_subscriptions
            .values()
            .filter(|subscription| subscription.id != id)
            .cloned()
            .collect();
        let count = server
            .state
            .lock()
            .unwrap()
            .notification_subscriptions
            .len();
        let result = dispatch(&server, request(actor, &name, "owned", "ended"));
        assert!(result.ok, "authorized existing goal end: {result:?}");
        let state = server.state.lock().unwrap();
        let after = &state.notification_subscriptions[&id];
        assert_eq!(after.status, "cancelled");
        assert_eq!(after.fired_count, before.fired_count);
        assert_eq!(after.trigger_ms, before.trigger_ms);
        assert_eq!(after.expires_ms, before.expires_ms);
        assert_eq!(state.notification_subscriptions.len(), count);
        for subscription in unrelated {
            assert_eq!(
                state.notification_subscriptions[&subscription.id],
                subscription
            );
        }
        drop(state);
        let repeated = dispatch(&server, request(actor, &name, "owned", "ended"));
        assert!(repeated.ok, "{repeated:?}");
        assert_eq!(repeated.data["deduplicated"], true);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn no_holder_denials_and_terminal_end_do_not_rearm() {
    let (server, root, name, id) = fixture();
    for (actor, owner, goal, desired, error) in [
        (
            "intruder",
            name.as_str(),
            "owned",
            "ended",
            "GOAL_SCHEDULE_OWNER_MISMATCH:",
        ),
        (
            "manager",
            "missing-name",
            "owned",
            "ended",
            "GOAL_SCHEDULE_TARGET_UNAVAILABLE",
        ),
        (
            "manager",
            "",
            "owned",
            "ended",
            "GOAL_SCHEDULE_OWNER_REQUIRED",
        ),
        (
            "manager",
            name.as_str(),
            "",
            "ended",
            "GOAL_SCHEDULE_ID_REQUIRED",
        ),
        (
            "manager",
            name.as_str(),
            "missing-goal",
            "ended",
            "GOAL_SCHEDULE_NOT_FOUND",
        ),
        (
            "manager",
            name.as_str(),
            "owned",
            "active",
            "GOAL_SCHEDULE_TARGET_UNAVAILABLE",
        ),
    ] {
        let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        let response = dispatch(&server, request(actor, owner, goal, desired));
        assert!(!response.ok, "{response:?}");
        assert!(
            response.error.as_deref().unwrap().starts_with(error),
            "{response:?}"
        );
        assert_eq!(
            serde_json::to_value(&*server.state.lock().unwrap()).unwrap(),
            before
        );
    }
    for terminal in ["consumed", "expired", "cancelled", "suppressed"] {
        {
            let mut state = server.state.lock().unwrap();
            let subscription = state.notification_subscriptions.get_mut(&id).unwrap();
            subscription.status = terminal.into();
            subscription.fired_count = 3;
        }
        let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        let response = dispatch(&server, request("manager", &name, "owned", "ended"));
        assert!(response.ok, "{response:?}");
        assert_eq!(
            serde_json::to_value(&*server.state.lock().unwrap()).unwrap(),
            before
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn released_independent_goal_does_not_recover_owner_from_previous_binding() {
    let (server, root, name, _) = fixture();
    let project = GlobalState::canonical_project_scope(&root).unwrap();
    server
        .state
        .lock()
        .unwrap()
        .name_occupancies
        .get_mut(project.as_str())
        .unwrap()
        .remove(&name);
    let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    let response = dispatch(&server, request("manager", &name, "owned", "ended"));
    assert!(!response.ok, "{response:?}");
    assert_eq!(
        response.error.as_deref(),
        Some("GOAL_SCHEDULE_TARGET_UNAVAILABLE")
    );
    assert_eq!(
        serde_json::to_value(&*server.state.lock().unwrap()).unwrap(),
        before
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn public_manager_scope_end_and_cross_scope_denial() {
    let (server, root, name, id) = fixture();
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let route = server_route_scope(&server, &server.state.lock().unwrap())
        .unwrap()
        .unwrap();
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let binding = &control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "manager")
        .unwrap()
        .unwrap()
        .active_binding()
        .unwrap()
        .binding;
    let context = ProjectContext {
        app_scope_id: route.app_scope_id.clone(),
        project_scope: route.project_scope_id.clone(),
        canonical_root: std::fs::canonicalize(&root)
            .unwrap()
            .to_str()
            .unwrap()
            .into(),
        runtime_context: Some(crate::identity::RuntimeIdentity {
            agent_id: binding.agent_id.clone(),
            runtime_id: binding.runtime_id.clone(),
            appserver_id: binding.app_scope_id.clone(),
            binding_id: binding.binding_id.clone(),
            session_id: binding.session_id.clone(),
            native_thread_id: binding.native_thread_id.clone(),
        }),
    };
    manager.install_runtime(
        &ProjectRuntimeManager::route_key(&context),
        server.clone(),
        None,
    );
    let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    let mut cross_scope = request("manager", &name, "owned", "ended");
    if let Req::GoalScheduleReconcile { app, .. } = &mut cross_scope {
        *app = Some("another-app".into());
    }
    let (_, response) = manager.dispatch_sync(Some(context.clone()), cross_scope);
    assert!(!response.ok, "{response:?}");
    assert!(
        response
            .error
            .as_deref()
            .unwrap()
            .starts_with("CEO_AUTHORITY_REQUIRED:"),
        "{response:?}"
    );
    assert_eq!(
        serde_json::to_value(&*server.state.lock().unwrap()).unwrap(),
        before
    );
    let (_, response) =
        manager.dispatch_sync(Some(context), request("manager", &name, "owned", "ended"));
    assert!(response.ok, "{response:?}");
    assert_eq!(
        server.state.lock().unwrap().notification_subscriptions[&id].status,
        "cancelled"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_ends_waiting_task_goal_after_holder_and_occupancy_release() {
    let (server, root, name, id) = fixture();
    {
        let mut state = server.state.lock().unwrap();
        let project = GlobalState::canonical_project_scope(&root).unwrap();
        state
            .name_occupancies
            .get_mut(project.as_str())
            .unwrap()
            .remove(&name);
        state.workers.remove("owner");
        let now = now_ms();
        state.tasks.insert(
            "owned-task".into(),
            TaskRec {
                id: "owned-task".into(),
                owner: "owner".into(),
                created_by: "manager".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p2".into(),
                status: "running".into(),
                next_step: None,
                wait: None,
                isolation_mode: None,
                created_ms: now,
                updated_ms: now,
            },
        );
        state.task_assignments.insert(
            "owned-task".into(),
            name_state::TaskAssignment {
                task_id: "owned-task".into(),
                name: name.clone(),
                session_id: "session-owner".into(),
                assignment_revision: 1,
                reflow: None,
                updated_ms: now,
            },
        );
        state.board_details.insert(
            "owned-task".into(),
            crate::board::BoardTaskDetails {
                long_horizon: Some(crate::board::LongHorizonTaskSpec {
                    content: "Preserved task work".into(),
                    interval_ms: 60000,
                    repeat_count: 3,
                    ttl_seconds: 3600,
                }),
                ..Default::default()
            },
        );
        let subscription = state.notification_subscriptions.get_mut(&id).unwrap();
        subscription.subject = Some("goal:task:owned-task".into());
        subscription.status = "waiting-owner".into();
        subscription.fired_count = 1;
        subscription.goal_content.as_mut().unwrap().source = Some(GoalScheduleContent::BoardTask {
            task_id: "owned-task".into(),
        });
    }
    let before = server.state.lock().unwrap().notification_subscriptions[&id].clone();
    let denied = dispatch(
        &server,
        request("manager", "wrong-name", "task:owned-task", "ended"),
    );
    assert!(!denied.ok, "{denied:?}");
    assert_eq!(
        denied.error.as_deref(),
        Some("GOAL_SCHEDULE_TASK_OWNER_MISMATCH")
    );
    let mut edit = request("manager", &name, "task:owned-task", "active");
    if let Req::GoalScheduleReconcile {
        operation, content, ..
    } = &mut edit
    {
        *operation = Some("edit".into());
        *content = Some("Continue the preserved task without a holder".into());
    }
    let edited = dispatch(&server, edit);
    assert!(edited.ok, "waiting task edit: {edited:?}");
    {
        let state = server.state.lock().unwrap();
        let after = &state.notification_subscriptions[&id];
        assert_eq!(after.status, "waiting-owner");
        assert_eq!(after.fired_count, before.fired_count);
        assert_eq!(after.trigger_ms, before.trigger_ms);
        assert_eq!(after.expires_ms, before.expires_ms);
        assert_eq!(
            state.board_details["owned-task"]
                .long_horizon
                .as_ref()
                .unwrap()
                .content,
            "Continue the preserved task without a holder"
        );
    }
    let result = dispatch(
        &server,
        request("manager", &name, "task:owned-task", "ended"),
    );
    assert!(result.ok, "{result:?}");
    let state = server.state.lock().unwrap();
    let after = &state.notification_subscriptions[&id];
    assert_eq!(after.status, "cancelled");
    assert_eq!(after.fired_count, before.fired_count);
    assert_eq!(after.trigger_ms, before.trigger_ms);
    assert_eq!(after.expires_ms, before.expires_ms);
    assert_eq!(after.goal_content, before.goal_content);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
