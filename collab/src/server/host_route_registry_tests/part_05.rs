#[test]
fn nonresident_child_route_is_committed_to_host_owner() {
    let (host, host_root, _) = test_server();
    let project_root = host_root.with_file_name(format!("{}-child-route", host_root.file_name().unwrap().to_string_lossy()));
    std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
    let manager = ProjectRuntimeManager::new(host.clone(), &host.host_paths).unwrap();
    let (runtime, parent) = register_host_fixture(&manager, Some(context_with_app(&project_root, "child-route-app")),
        Req::register("child-route-parent".into(), "token-parent".into(), project_root.display().to_string(),
            test_candidates_at("thread-child-route-parent", &project_root)));
    assert!(parent.ok, "{parent:?}");
    assert!(!Arc::ptr_eq(&runtime, &host));
    let app = AppServerId::new("child-route-app").unwrap();
    let child = seed_committed_registration_fixture(&runtime, "child-route-child".into(), "token-child".into(),
        project_root.display().to_string(), Some(app.clone()), test_candidates_at("thread-child-route-child", &project_root));
    assert!(child.ok, "{child:?}");
    commit_current_thread_route_for_runtime(&host, &runtime, "child-route-child", &project_root.display().to_string(), Some(&app)).unwrap();
    let session = crate::identity::SessionId::new("session-thread-child-route-child").unwrap();
    let thread = NativeThreadId::new("thread-child-route-child").unwrap();
    assert!(host.state.lock().unwrap().global.lookup_current_thread_route(&session, &thread).is_some());
    assert!(runtime.state.lock().unwrap().global.lookup_current_thread_route(&session, &thread).is_none());
    assert!(manager.resolve_route_by_native_thread(session.as_str(), thread.as_str()).is_ok());
    retire_current_thread_route_after_launch_failure(&host, &runtime, "child-route-child", &project_root.display().to_string(), Some(&app)).unwrap();
    assert!(host.state.lock().unwrap().global.lookup_current_thread_route(&session, &thread).is_none());
    let loaded = current_state::load(&host.state_path, false).unwrap();
    assert!(loaded.global.lookup_current_thread_route(&session, &thread).is_none());
    std::fs::remove_dir_all(host_root).unwrap();
    std::fs::remove_dir_all(project_root).unwrap();
}

#[tokio::test]
async fn wire_notification_mutation_requires_runtime_and_read_status_stays_compatible() {
    let (server, root, state_path) = test_server();
    let app = "app-wire";
    let worker = "notification-worker";
    let token = "token-notification-worker";
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    commit_host_route_fixture(&manager, &server, worker, token, app, committed_tmux_endpoint(&root, "thread-notification"));
    let before = mutation_snapshot(&server);
    let bytes = std::fs::read(&state_path).unwrap();
    let missing = dispatch_wire(server.clone(), Some(context_with_app(&root, app)), notification_subscribe_request(worker, token)).await;
    assert!(!missing.ok, "{missing:?}");
    assert_eq!(missing.error.as_deref(), Some("PROJECT_CONTEXT_REQUIRED: project-scoped mutation requires typed runtime context"));
    assert_eq!(mutation_snapshot(&server), before);
    assert_eq!(std::fs::read(&state_path).unwrap(), bytes);
    let runtime = runtime_for_registered(&server, &root, worker, app);
    let accepted = dispatch_wire(server.clone(), Some(context_with_runtime(&root, app, &runtime)), notification_subscribe_request(worker, token)).await;
    assert!(accepted.ok, "{accepted:?}");
    let id = accepted.data["subscription"]["id"].as_str().unwrap();
    let status = dispatch_wire(server.clone(), Some(context_with_app(&root, app)),
        Req::NotificationStatus { worker_id: worker.into(), token: token.into() }).await;
    assert!(status.ok, "{status:?}");
    assert!(status.data["subscriptions"].as_array().unwrap().iter().any(|subscription| subscription["id"] == id));
    let reconciled = dispatch_wire(server.clone(), Some(context_with_runtime(&root, app, &runtime)),
        Req::GoalScheduleReconcile {
            worker_id: worker.into(), token: token.into(), goal_id: "wire-goal".into(), goal_owner: "Alice".into(),
            desired: "ended".into(), trigger_ms: None, ttl_seconds: None, interval_ms: None,
            repeat_count: None, goal_subject: None, reason: Some("fixture cleanup".into()),
        }).await;
    assert!(reconciled.ok, "{reconciled:?}");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn wire_notification_mutation_rejects_wrong_binding_unbound_and_wrong_routes() {
    let (server, root, state_path) = test_server();
    let app = "app-current-route";
    let worker = "route-worker";
    let token = "token-route-worker";
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let binding = commit_host_route_fixture(&manager, &server, worker, token, app, committed_tmux_endpoint(&root, "thread-route-worker"));
    let runtime = runtime_for_registered(&server, &root, worker, app);
    let mut wrong_binding = runtime.clone(); wrong_binding.binding_id = BindingId::new("binding-other").unwrap();
    let mut wrong_app = runtime.clone(); wrong_app.appserver_id = AppServerId::new("app-other").unwrap();
    let wrong_root = root.with_file_name(format!("{}-other", root.file_name().unwrap().to_string_lossy()));
    std::fs::create_dir_all(&wrong_root).unwrap();
    let map = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let map_before = map.load().unwrap();
    let before = mutation_snapshot(&server);
    let bytes = std::fs::read(&state_path).unwrap();
    for (context, expected) in [
        (context_with_runtime(&root, app, &wrong_binding), "SESSION_THREAD_BINDING_MISMATCH:"),
        (context_with_runtime(&root, "app-other", &wrong_app), "PROJECT_SCOPE_UNKNOWN:"),
        (context_with_runtime(&wrong_root, app, &runtime), "PROJECT_SCOPE_UNKNOWN:"),
    ] {
        let rejected = dispatch_wire(server.clone(), Some(context), notification_subscribe_request(worker, token)).await;
        assert!(!rejected.ok, "{rejected:?}");
        assert!(rejected.error.as_deref().unwrap().starts_with(expected), "{rejected:?}");
        assert_eq!(mutation_snapshot(&server), before);
        assert_eq!(std::fs::read(&state_path).unwrap(), bytes);
        assert_eq!(map.load().unwrap(), map_before);
    }
    // A missing current runtime projection is also rejected before mutation.
    server.state.lock().unwrap().global.projects.get_mut(binding.project_scope.as_str()).unwrap()
        .runtime_bindings.remove(binding.binding_id.as_str());
    let before = mutation_snapshot(&server);
    let rejected = dispatch_wire(server.clone(), Some(context_with_runtime(&root, app, &runtime)), notification_subscribe_request(worker, token)).await;
    assert!(!rejected.ok, "{rejected:?}");
    assert!(rejected.error.as_deref().unwrap().starts_with("PROJECT_ROUTE_NOT_READY/UNSUPPORTED:"), "{rejected:?}");
    assert_eq!(mutation_snapshot(&server), before);
    assert_eq!(std::fs::read(&state_path).unwrap(), bytes);
    assert_eq!(map.load().unwrap(), map_before);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(wrong_root).unwrap();
}

#[tokio::test]
async fn authenticated_send_uses_context_app_scope_and_rejects_forged_scope() {
    let (server, root, state_path) = test_server();
    let app = "app-wire-send";
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    for worker in ["sender", "recipient"] {
        commit_host_route_fixture(&manager, &server, worker, &format!("token-{worker}"), app,
            committed_tmux_endpoint(&root, &format!("thread-{worker}")));
    }
    let runtime = runtime_for_registered(&server, &root, "sender", app);
    let scope = GlobalState::canonical_project_scope(&root).unwrap();
    let command = |app_scope: &str, id: &str| CommandEnvelope::new(
        CommandId::new(id).unwrap(), OperationId::new(format!("operation-{id}")).unwrap(), runtime.binding_id.clone(),
        RouteScope { app_scope_id: AppServerId::new(app_scope).unwrap(), project_scope_id: scope.clone() }, None, None, None, None);
    let request = |command| Req::Send {
        from: "sender".into(), worker_id: Some("sender".into()), token: Some("token-sender".into()), command: Some(command),
        to: "recipient".into(), mtype: "notify".into(), subject: Some("scope".into()), body: "exact wire payload".into(),
        in_reply_to: None, reply_policy: None, target_session: None, delivery: "immediate".into(),
    };
    let accepted = dispatch_wire(server.clone(), Some(context_with_runtime(&root, app, &runtime)), request(command(app, "wire-send-ok"))).await;
    assert!(accepted.ok, "{accepted:?}");
    let id = accepted.data["msg_id"].as_str().unwrap();
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[id].from, "sender");
        assert_eq!(state.msgs[id].to, "recipient");
        assert_eq!(state.msgs[id].body, "exact wire payload");
        assert_ne!(state.msgs[id].state, "read");
    }
    let before = mutation_snapshot(&server);
    let bytes = std::fs::read(&state_path).unwrap();
    let forged = dispatch_wire(server.clone(), Some(context_with_runtime(&root, app, &runtime)), request(command("app-forged", "wire-send-forged"))).await;
    assert!(!forged.ok, "{forged:?}");
    assert!(forged.error.as_deref().unwrap().starts_with("SEND_BINDING_REJECTED:"), "{forged:?}");
    assert_eq!(mutation_snapshot(&server), before);
    assert_eq!(std::fs::read(&state_path).unwrap(), bytes);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn wire_cli_register_rejects_token_rotation_for_the_same_thread_and_route() {
    let (server, root, state_path) = test_server();
    let app = crate::identity::CLI_APP_SERVER_ID;
    let worker = "lost-token-worker";
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    commit_host_route_fixture(&manager, &server, worker, "old-token", app, committed_tmux_endpoint(&root, "thread-lost-token"));
    let runtime = runtime_for_registered(&server, &root, worker, app);
    let map = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let map_before = map.load().unwrap();
    let before = std::fs::read(&state_path).unwrap();
    let rejected = dispatch_wire(server.clone(), Some(context_with_runtime(&root, app, &runtime)),
        Req::register(worker.into(), "new-token".into(), root.display().to_string(),
            test_candidates_for_registered(&server, &root, worker, app))).await;
    assert!(!rejected.ok, "{rejected:?}");
    assert!(rejected.error.as_deref().unwrap().starts_with("TOKEN_MISMATCH:"), "{rejected:?}");
    assert_eq!(runtime_for_registered(&server, &root, worker, app), runtime);
    assert_eq!(server.state.lock().unwrap().workers[worker].token, "old-token");
    assert_eq!(std::fs::read(&state_path).unwrap(), before);
    assert_eq!(map.load().unwrap(), map_before);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn wire_current_registration_rejects_a_forged_token_and_a_forged_route() {
    let (server, root, state_path) = test_server();
    let app = crate::identity::CLI_APP_SERVER_ID;
    let worker = "current-negative-worker";
    let token = "token-current-negative";
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    commit_host_route_fixture(&manager, &server, worker, token, app, committed_tmux_endpoint(&root, "thread-current-negative"));
    let runtime = runtime_for_registered(&server, &root, worker, app);
    let wrong_root = root.with_file_name(format!("{}-wrong-route", root.file_name().unwrap().to_string_lossy()));
    std::fs::create_dir_all(&wrong_root).unwrap();
    let map = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let map_before = map.load().unwrap();
    let before = mutation_snapshot(&server);
    let bytes = std::fs::read(&state_path).unwrap();
    let candidates = || test_candidates_for_registered(&server, &root, worker, app);
    for (context, request, expected) in [
        (context_with_runtime(&root, app, &runtime), Req::register(worker.into(), "forged-token".into(), root.display().to_string(), candidates()), "TOKEN_MISMATCH:"),
        (context_with_runtime(&wrong_root, app, &runtime), Req::register(worker.into(), token.into(), wrong_root.display().to_string(), candidates()), "PROJECT_SCOPE_UNKNOWN:"),
        (context_with_runtime(&root, app, &runtime), Req::register("forged-worker".into(), "token-forged".into(), root.display().to_string(), candidates()), "RUNTIME_BINDING_REJECTED:"),
    ] {
        let rejected = dispatch_wire(server.clone(), Some(context), request).await;
        assert!(!rejected.ok, "{rejected:?}");
        assert!(rejected.error.as_deref().unwrap().starts_with(expected), "{rejected:?}");
        assert_eq!(mutation_snapshot(&server), before);
        assert_eq!(std::fs::read(&state_path).unwrap(), bytes);
        assert_eq!(map.load().unwrap(), map_before);
    }
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(wrong_root).unwrap();
}
