fn assert_authorized_promotion_replaces_holder(
    server: &Server,
    route: &RouteScope,
    response: &Resp,
    old_worker: &str,
    new_worker: &str,
) {
    assert!(response.ok, "{response:?}");
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap();
    let slot = control.current_for_name(&route.project_scope_id, &route.app_scope_id, "Jenny")
        .unwrap().expect("authorized replacement publishes current Jenny");
    assert_eq!(slot.active_binding().unwrap().binding.agent_id.as_str(), new_worker);
    let state = server.state.lock().unwrap();
    assert_eq!(current_master_worker_id(&state, Some(route)).as_deref(), Some(new_worker));
    assert!(!state.global.lookup_project_for_route(route).unwrap().master_grants.values()
        .any(|grant| grant.agent_id.as_str() == old_worker));
}

/// An unknown prior carrier cannot veto explicit user authorization. This
/// native transport case needs no pane or fixed-position fixture; the tmux
/// position-1 constraint remains covered by the public map/layout cases.
#[test]
fn approved_promotion_replaces_unknown_incumbent_without_a_pane_taker() {
    let (server, root, _) = test_server();
    let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
    for worker in ["unknown-master", "replacement"] {
        let transport = super::peer_tests::test_appserver_transport(&format!("thread-{worker}"));
        let registered = super::peer_tests::seed_committed_owner(
            &server, worker, &format!("token-{worker}"), &transport,
            &root.to_string_lossy(), Some(app.clone()),
        );
        assert!(registered.ok, "{registered:?}");
    }
    super::peer_tests::promote_master(&server, "unknown-master", "user approved fixture master");
    let route = RouteScope {
        project_scope_id: GlobalState::canonical_project_scope(&root).unwrap(), app_scope_id: app,
    };
    let master = runtime_for_registered(&server, &root, "unknown-master", crate::identity::CLI_APP_SERVER_ID);
    server.commit(&[Event::AgentPresenceObserved { presence: name_state::AgentPresenceRecord {
        project_scope: route.project_scope_id.as_str().into(), worker_id: "unknown-master".into(),
        session_id: master.session_id.unwrap().to_string(), presence: name_state::AgentPresence::Unknown,
        reason: "current typed unknown observation".into(), observed_ms: now_ms(),
    } }]);
    let identity = runtime_for_registered(&server, &root, "replacement", crate::identity::CLI_APP_SERVER_ID);
    let context = context_with_runtime(&root, crate::identity::CLI_APP_SERVER_ID, &identity);
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let before = serde_json::to_value(store.load().unwrap()).unwrap();
    let state_before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    let (_, rejected) = manager.dispatch_sync(Some(context.clone()), Req::MasterPromote {
        worker_id: "replacement".into(), token: "token-replacement".into(), approval: "   ".into(),
    });
    assert!(!rejected.ok, "{rejected:?}");
    assert_eq!(serde_json::to_value(store.load().unwrap()).unwrap(), before);
    assert_eq!(serde_json::to_value(&*server.state.lock().unwrap()).unwrap(), state_before);
    let (_, promoted) = manager.dispatch_sync(Some(context), Req::MasterPromote {
        worker_id: "replacement".into(), token: "token-replacement".into(),
        approval: "user explicitly authorizes replacing the unknown Jenny carrier".into(),
    });
    assert_authorized_promotion_replaces_holder(&server, &route, &promoted, "unknown-master", "replacement");
    drop(manager);
    std::fs::remove_dir_all(root).unwrap();
}

include!("part_02_tail2.rs");
