#[test]
fn scheduler_runtime_list_deduplicates_routes_sharing_one_reducer() {
    let (server, root, _) = test_server();
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    {
        let mut routes = manager.routes.lock().unwrap();
        for app_scope in ["app-a", "app-b"] {
            routes.insert(
                (app_scope.into(), "/shared-project".into()),
                RuntimeRoute {
                    root: root.clone(), storage_root: root.clone(), runtime: Some(server.clone()),
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
    let (server, root, state_path) = test_server();
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let binding = commit_host_route_fixture(
        &manager, &server, "agent-route", "token-route", "app-route",
        crate::proto::TmuxEndpoint {
            socket_path: root.join("readback.sock").display().to_string(),
            server_pid: 42, tmux_session_id: "$1".into(), pane_id: "%1".into(), pane_pid: 99,
            codex_session_id: Some("session-thread-route".into()),
            codex_thread_id: Some("thread-route".into()),
        },
    );
    let state_before = mutation_snapshot(&server);
    let bytes_before = std::fs::read(&state_path).unwrap();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let map_before = store.load().unwrap();
    let route = manager.resolve_route_by_native_thread("session-thread-route", "thread-route").unwrap();
    assert_eq!(route.app_scope_id, binding.app_scope_id);
    assert_eq!(route.canonical_root, root.canonicalize().unwrap().to_string_lossy());
    assert_eq!(route.storage_root, root.to_string_lossy());
    assert_eq!(route.agent_id, binding.agent_id);
    assert_eq!(route.binding_id, binding.binding_id);
    assert_eq!(route.session_id, binding.session_id);
    assert_eq!(route.native_thread_id, binding.native_thread_id);
    route.validate().unwrap();
    assert_eq!(mutation_snapshot(&server), state_before);
    assert_eq!(std::fs::read(&state_path).unwrap(), bytes_before);
    assert_eq!(store.load().unwrap(), map_before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_thread_route_resolution_rejects_missing_invalid_and_mismatched_projections() {
    let (server, root, _) = test_server();
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let binding = commit_host_route_fixture(
        &manager, &server, "agent-current", "token-current", "app-route",
        crate::proto::TmuxEndpoint {
            socket_path: root.join("readback.sock").display().to_string(),
            server_pid: 42, tmux_session_id: "$1".into(), pane_id: "%1".into(), pane_pid: 99,
            codex_session_id: Some("session-current".into()),
            codex_thread_id: Some("thread-current".into()),
        },
    );
    let missing = manager.resolve_route_by_native_thread("session-missing", "thread-missing").unwrap_err();
    assert!(missing.starts_with("ROUTE_RESOLVE_NOT_FOUND"), "{missing}");
    assert!(missing.contains(ROUTE_RESOLVE_NOT_FOUND_RECOVERY), "{missing}");
    let current = manager.resolve_route_by_native_thread("session-current", "thread-current").unwrap();
    assert_eq!(current.agent_id, binding.agent_id);
    let map = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let map_before = map.load().unwrap();
    for invalid in ["", "thread\ninvalid"] {
        let error = manager.resolve_route_by_native_thread("session-current", invalid).unwrap_err();
        assert!(error.starts_with("ROUTE_RESOLVE_INVALID"), "{error}");
    }
    let corrupt = RuntimeBinding::new_with_session(
        binding.project_scope.clone(), binding.app_scope_id.clone(),
        AgentId::new("agent-corrupt").unwrap(), RuntimeId::new("runtime-corrupt").unwrap(),
        BindingId::new("binding-corrupt").unwrap(), binding.session_id.clone(), binding.native_thread_id.clone(),
    ).unwrap();
    server.commit(&[Event::GlobalCurrentThreadRouteSet { binding: corrupt }]);
    let state_before = std::fs::read(&server.state_path).unwrap();
    let error = manager.resolve_route_by_native_thread("session-current", "thread-current").unwrap_err();
    assert!(error.starts_with("ROUTE_RESOLVE_STALE_INDEX"), "{error}");
    assert!(error.contains("missing runtime binding"), "{error}");
    assert_eq!(map.load().unwrap(), map_before);
    assert_eq!(std::fs::read(&server.state_path).unwrap(), state_before);
    std::fs::remove_dir_all(root).unwrap();
}
