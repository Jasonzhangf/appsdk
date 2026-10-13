#[test]
fn host_route_replay_requires_framed_unique_jsonl_records() {
    let (_server, root, _) = test_server();
    let route_journal = root.join(".agent-collab/server/routes.jsonl");
    let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
    let record = HostRouteRecord {
        version: 1,
        op: "register".into(),
        app_scope_id: "route-app".into(),
        project_scope: project_scope.as_str().into(),
        canonical_root: project_scope.as_str().into(),
        storage_root: root.to_string_lossy().into_owned(),
        registered_ms: 1,
    };
    let line = serde_json::to_string(&record).unwrap();

    std::fs::write(&route_journal, &line).unwrap();
    let missing_final_newline = load_host_route_records(&route_journal).unwrap_err();
    assert!(
        missing_final_newline.contains("must end with a newline"),
        "{missing_final_newline}"
    );

    std::fs::write(&route_journal, format!("{line}\n\n")).unwrap();
    let empty_line = load_host_route_records(&route_journal).unwrap_err();
    assert!(
        empty_line.contains("empty route journal line 2"),
        "{empty_line}"
    );

    std::fs::write(&route_journal, format!("{line}\n{line}\n")).unwrap();
    let duplicate = load_host_route_records(&route_journal).unwrap_err();
    assert!(duplicate.contains("duplicate route key"), "{duplicate}");
    assert!(duplicate.contains("lines 1 and 2"), "{duplicate}");

    std::fs::write(&route_journal, format!("{line}\r\n")).unwrap();
    let parsed = load_host_route_records(&route_journal).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].app_scope_id, "route-app");

    let mut colliding_record = record.clone();
    colliding_record.app_scope_id = "route-app-other".into();
    let colliding_line = serde_json::to_string(&colliding_record).unwrap();
    std::fs::write(&route_journal, format!("{line}\n{colliding_line}\n")).unwrap();
    let duplicate_storage = load_host_route_records(&route_journal).unwrap_err();
    assert!(
        duplicate_storage.contains("duplicate runtime storage root"),
        "{duplicate_storage}"
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn scheduler_runtime_list_deduplicates_routes_sharing_one_reducer() {
    let (server, root, _) = test_server();
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
    {
        let mut routes = manager.routes.lock().unwrap();
        for app_scope in ["app-a", "app-b"] {
            routes.insert(
                (app_scope.into(), "/shared-project".into()),
                RuntimeRoute {
                    root: root.clone(),
                    storage_root: root.clone(),
                    runtime: Some(server.clone()),
                },
            );
        }
    }

    let runtimes = manager.runtimes();
    assert_eq!(runtimes.len(), 1);
    assert!(Arc::ptr_eq(&runtimes[0], &server));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_thread_route_resolution_returns_one_complete_typed_route() {
    let (server, root, journal_path) = test_server();
    let (historical_server, historical_root, _) = test_server();
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
    let registration = server
        .state
        .lock()
        .unwrap()
        .global
        .lookup_registration(
            &GlobalState::canonical_project_scope(&root).unwrap(),
            &AppServerId::new("app-route").unwrap(),
        )
        .is_some();
    if !registration {
        register_known_project_with_app(&server, &root, "app-route");
    }
    let binding = RuntimeBinding::new_with_session(
        GlobalState::canonical_project_scope(&root).unwrap(),
        AppServerId::new("app-route").unwrap(),
        AgentId::new("agent-route").unwrap(),
        RuntimeId::new("runtime-route").unwrap(),
        BindingId::new("binding-route").unwrap(),
        9,
        Some(crate::identity::SessionId::new("session-thread-route").unwrap()),
        Some(NativeThreadId::new("thread-route").unwrap()),
    )
    .unwrap();
    server
        .state
        .lock()
        .unwrap()
        .global
        .bind_runtime(binding.clone())
        .unwrap();
    server.commit(&[Event::GlobalCurrentThreadRouteSet {
        binding: binding.clone(),
    }]);
    manager.install_runtime(
        &(
            binding.app_scope_id.as_str().to_owned(),
            binding.project_scope.as_str().to_owned(),
        ),
        server.clone(),
        None,
    );
    register_known_project_with_app(&historical_server, &historical_root, "app-route");
    let historical_binding = RuntimeBinding::new_with_session(
        GlobalState::canonical_project_scope(&historical_root).unwrap(),
        AppServerId::new("app-route").unwrap(),
        AgentId::new("agent-route").unwrap(),
        RuntimeId::new("runtime-route").unwrap(),
        BindingId::new("binding-route").unwrap(),
        9,
        Some(crate::identity::SessionId::new("session-thread-route").unwrap()),
        Some(NativeThreadId::new("thread-route").unwrap()),
    )
    .unwrap();
    historical_server
        .state
        .lock()
        .unwrap()
        .global
        .bind_runtime(historical_binding.clone())
        .unwrap();
    historical_server.commit(&[Event::Registered {
        worker: WorkerRec {
            id: "agent-route".into(),
            token: "historical-token".into(),
            cwd: historical_root.to_string_lossy().into_owned(),
            registered_ms: now_ms(),
            transport: Some(test_selected_transport("thread-route")),
        },
    }]);
    manager.routes.lock().unwrap().insert(
        (
            historical_binding.app_scope_id.as_str().to_owned(),
            historical_binding.project_scope.as_str().to_owned(),
        ),
        RuntimeRoute {
            root: historical_root.clone(),
            storage_root: historical_root.clone(),
            runtime: Some(historical_server),
        },
    );
    let runtime = RuntimeIdentity {
        agent_id: binding.agent_id.clone(),
        runtime_id: binding.runtime_id.clone(),
        appserver_id: binding.app_scope_id.clone(),
        endpoint_generation: binding.endpoint_generation,
        binding_id: binding.binding_id.clone(),
        session_id: binding.session_id.clone(),
        native_thread_id: binding.native_thread_id.clone(),
    };
    write_global_identity(&host_paths, "agent-route", "token-route", None, &runtime);
    server.commit(&[Event::Registered {
        worker: WorkerRec {
            id: "agent-route".into(),
            token: "token-route".into(),
            cwd: root.to_string_lossy().into_owned(),
            registered_ms: now_ms(),
            transport: Some(test_selected_transport("thread-route")),
        },
    }]);

    let state_before = mutation_snapshot(&server);
    let journal_before = std::fs::read(&journal_path).unwrap();
    let route = manager
        .resolve_route_by_native_thread("session-thread-route", "thread-route")
        .unwrap();
    assert_eq!(route.app_scope_id.as_str(), "app-route");
    assert_eq!(
        route.canonical_root,
        root.canonicalize().unwrap().to_string_lossy()
    );
    assert_eq!(route.storage_root, root.to_string_lossy());
    assert_eq!(route.agent_id.as_str(), "agent-route");
    assert_eq!(route.binding_id.as_str(), "binding-route");
    assert_eq!(route.endpoint_generation, 9);
    assert_eq!(
        route.native_thread_id.as_ref().unwrap().as_str(),
        "thread-route"
    );
    route.validate().unwrap();
    assert_eq!(mutation_snapshot(&server), state_before);
    assert_eq!(std::fs::read(&journal_path).unwrap(), journal_before);

    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(historical_root).unwrap();
}

#[test]
fn native_thread_route_resolution_ignores_history_and_fails_closed_for_invalid_state() {
    let (server, root, _) = test_server();
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
    register_known_project_with_app(&server, &root, "app-route");
    let scope = GlobalState::canonical_project_scope(&root).unwrap();
    let current = RuntimeBinding::new_with_session(
        scope.clone(),
        AppServerId::new("app-route").unwrap(),
        AgentId::new("agent-current").unwrap(),
        RuntimeId::new("runtime-current").unwrap(),
        BindingId::new("binding-current").unwrap(),
        1,
        Some(crate::identity::SessionId::new("session-thread-duplicate").unwrap()),
        Some(NativeThreadId::new("thread-duplicate").unwrap()),
    )
    .unwrap();
    let duplicate = RuntimeBinding::new_with_session(
        scope.clone(),
        AppServerId::new("app-route").unwrap(),
        AgentId::new("agent-a").unwrap(),
        RuntimeId::new("runtime-a").unwrap(),
        BindingId::new("binding-a").unwrap(),
        1,
        Some(crate::identity::SessionId::new("session-thread-duplicate-a").unwrap()),
        Some(NativeThreadId::new("thread-duplicate").unwrap()),
    )
    .unwrap();
    let second = RuntimeBinding::new_with_session(
        scope,
        AppServerId::new("app-route").unwrap(),
        AgentId::new("agent-b").unwrap(),
        RuntimeId::new("runtime-b").unwrap(),
        BindingId::new("binding-b").unwrap(),
        1,
        Some(crate::identity::SessionId::new("session-thread-duplicate-b").unwrap()),
        Some(NativeThreadId::new("thread-duplicate").unwrap()),
    )
    .unwrap();
    {
        let mut state = server.state.lock().unwrap();
        state.global.bind_runtime(current.clone()).unwrap();
        state.global.bind_runtime(duplicate).unwrap();
        state.global.bind_runtime(second).unwrap();
    }
    server.commit(&[Event::GlobalCurrentThreadRouteSet {
        binding: current.clone(),
    }]);
    manager.install_runtime(
        &(
            current.app_scope_id.as_str().to_owned(),
            current.project_scope.as_str().to_owned(),
        ),
        server.clone(),
        None,
    );
    write_global_identity(
        &host_paths,
        "agent-current",
        "token-current",
        Some(&current.project_scope),
        &RuntimeIdentity {
            agent_id: current.agent_id.clone(),
            runtime_id: current.runtime_id.clone(),
            appserver_id: current.app_scope_id.clone(),
            endpoint_generation: current.endpoint_generation,
            binding_id: current.binding_id.clone(),
            session_id: current.session_id.clone(),
            native_thread_id: current.native_thread_id.clone(),
        },
    );
    server.commit(&[Event::Registered {
        worker: WorkerRec {
            id: "agent-current".into(),
            token: "token-current".into(),
            cwd: root.to_string_lossy().into_owned(),
            registered_ms: now_ms(),
            transport: Some(test_selected_transport("thread-duplicate")),
        },
    }]);

    let missing = manager
        .resolve_route_by_native_thread("session-thread-missing", "thread-missing")
        .unwrap_err();
    assert!(missing.starts_with("ROUTE_RESOLVE_NOT_FOUND"), "{missing}");
    assert!(
        missing.contains(ROUTE_RESOLVE_NOT_FOUND_RECOVERY),
        "{missing}"
    );
    let current = manager
        .resolve_route_by_native_thread("session-thread-duplicate", "thread-duplicate")
        .unwrap();
    assert_eq!(current.agent_id.as_str(), "agent-current");
    assert_eq!(current.binding_id.as_str(), "binding-current");
    write_global_identity(
        &host_paths,
        "agent-other",
        "token-other",
        None,
        &RuntimeIdentity {
            agent_id: AgentId::new("agent-other").unwrap(),
            runtime_id: RuntimeId::new("runtime-other").unwrap(),
            appserver_id: AppServerId::new("app-route").unwrap(),
            endpoint_generation: 1,
            binding_id: BindingId::new("binding-other").unwrap(),
            session_id: Some(crate::identity::SessionId::new("session-thread-duplicate").unwrap()),
            native_thread_id: Some(NativeThreadId::new("thread-duplicate").unwrap()),
        },
    );
    let resolved = manager
        .resolve_route_by_native_thread("session-thread-duplicate", "thread-duplicate")
        .unwrap();
    assert_eq!(resolved, current);
    for invalid in ["", "thread\ninvalid"] {
        let error = manager
            .resolve_route_by_native_thread("session-thread-duplicate", invalid)
            .unwrap_err();
        assert!(error.starts_with("ROUTE_RESOLVE_INVALID"), "{error}");
    }

    let corrupt = RuntimeBinding::new_with_session(
        current.project_scope.clone(),
        current.app_scope_id.clone(),
        AgentId::new("agent-corrupt").unwrap(),
        RuntimeId::new("runtime-corrupt").unwrap(),
        BindingId::new("binding-corrupt").unwrap(),
        1,
        Some(crate::identity::SessionId::new("session-thread-duplicate").unwrap()),
        current.native_thread_id.clone(),
    )
    .unwrap();
    server.commit(&[Event::GlobalCurrentThreadRouteSet { binding: corrupt }]);
    let error = manager
        .resolve_route_by_native_thread("session-thread-duplicate", "thread-duplicate")
        .unwrap_err();
    assert!(error.starts_with("ROUTE_RESOLVE_STALE_INDEX"), "{error}");
    assert!(error.contains("missing runtime binding"), "{error}");

    std::fs::remove_dir_all(root).unwrap();
}
