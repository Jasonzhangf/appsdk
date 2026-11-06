// Current runtime selection and delivery-anchor tests. Historical journal
// rollback, draft identities, orphan recovery and endpoint counters are gone.

#[tokio::test]
async fn manager_routes_project_queries_mutations_and_poll_to_one_runtime() {
    let (host, root, _) = test_server();
    let project_root = root.with_file_name(format!("{}-route-aware", root.file_name().unwrap().to_string_lossy()));
    std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
    let manager = ProjectRuntimeManager::new(host.clone(), &host.host_paths).unwrap();
    let app = "route-aware-app";
    let context = context_with_app(&project_root, app);
    let (runtime, sender) = register_host_fixture(&manager, Some(context.clone()), Req::register(
        "sender".into(), "token-sender".into(), project_root.display().to_string(),
        test_candidates_at("thread-route-aware-sender", &project_root),
    ));
    assert!(sender.ok, "{sender:?}");
    let (recipient_runtime, recipient) = register_host_fixture(&manager, Some(context), Req::register(
        "recipient".into(), "token-recipient".into(), project_root.display().to_string(),
        test_candidates_at("thread-route-aware-recipient", &project_root),
    ));
    assert!(recipient.ok, "{recipient:?}");
    assert!(Arc::ptr_eq(&runtime, &recipient_runtime));
    assert!(!Arc::ptr_eq(&runtime, &host));
    let sender_identity = runtime_for_registered(&runtime, &project_root, "sender", app);
    let recipient_identity = runtime_for_registered(&runtime, &project_root, "recipient", app);
    let sender_context = context_with_runtime(&project_root, app, &sender_identity);
    let recipient_context = context_with_runtime(&project_root, app, &recipient_identity);
    let (_, status) = manager.dispatch_sync(Some(sender_context.clone()), Req::StatusAll);
    assert!(status.ok, "{status:?}");
    assert_eq!(status.data["summary"]["workers"], 2);
    let workers = status.data["workers"].as_array().unwrap().iter()
        .map(|worker| worker["id"].as_str().unwrap()).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(workers, std::collections::BTreeSet::from(["recipient", "sender"]));
    let (_, workers) = manager.dispatch_sync(Some(recipient_context.clone()), Req::Workers);
    assert!(workers.ok, "{workers:?}");
    assert_eq!(workers.data["count"], 2);
    let recipient = workers.data["workers"].as_array().unwrap().iter()
        .find(|worker| worker["id"] == "recipient").unwrap();
    assert_eq!(recipient["transport"]["session_id"], "session-thread-route-aware-recipient");
    assert_eq!(recipient["transport"]["thread_id"], "thread-route-aware-recipient");
    let (_, registered) = manager.dispatch_sync(Some(sender_context.clone()), Req::TaskRegister {
        worker_id: "sender".into(), token: "token-sender".into(), task_id: "route-aware-task".into(),
        owner: None, feature_id: Some("route-aware-feature".into()), worktree_path: None,
        branch: None, base_commit: None, priority: "p1".into(),
        next_step: Some("verify route-aware task".into()), goal_prompt: None,
    });
    assert!(registered.ok, "{registered:?}");
    let (_, task) = manager.dispatch_sync(Some(recipient_context.clone()), Req::TaskStatus {
        task_id: Some("route-aware-task".into()),
    });
    assert!(task.ok, "{task:?}");
    assert_eq!(task.data["id"], "route-aware-task");
    assert_eq!(task.data["owner"], "sender");
    let (_, subscription) = manager.dispatch_sync(Some(recipient_context.clone()),
        notification_subscribe_request("recipient", "token-recipient"));
    assert!(subscription.ok, "{subscription:?}");
    assert_eq!(subscription.data["subscription"]["worker_id"], "recipient");
    let command = cross_project_command(&runtime, &project_root, "sender", app, "route-aware-send");
    let (_, sent) = manager.dispatch_sync(Some(sender_context), Req::Send {
        from: "sender".into(), worker_id: Some("sender".into()), token: Some("token-sender".into()),
        command: Some(command), to: "recipient".into(), mtype: "notify".into(),
        subject: Some("route-aware message".into()), body: "message must stay in the selected runtime".into(),
        in_reply_to: None, reply_policy: None, target_session: None, delivery: "immediate".into(),
    });
    assert!(sent.ok, "{sent:?}");
    let id = sent.data["msg_id"].as_str().unwrap().to_owned();
    let (_, received) = dispatch_wire_routed(manager, Some(recipient_context), Req::Poll {
        worker_id: "recipient".into(), token: "token-recipient".into(), timeout_ms: 0, receive_id: None,
    }, tokio::sync::watch::channel(false).1).await;
    assert!(received.ok, "{received:?}");
    assert_eq!(received.data["count"], 1);
    assert_eq!(received.data["messages"][0]["id"], id);
    assert_eq!(received.data["messages"][0]["body"], "message must stay in the selected runtime");
    assert_eq!(runtime.state.lock().unwrap().msgs[&id].state, "read");
    assert!(host.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(project_root).unwrap();
}

/// Planning a delivery-anchor change cannot itself retire the current carrier.
#[tokio::test]
async fn pane_takeover_plans_a_foreign_claimant_without_evicting_it() {
    let (host, root, _) = test_server();
    let manager = ProjectRuntimeManager::new(host.clone(), &host.host_paths).unwrap();
    let candidates = test_candidates("thread-pane-pid-a").unwrap();
    let anchor = candidates.tmux.as_ref().unwrap().endpoint.clone();
    let (_, registered) = register_host_fixture(&manager,
        Some(context_with_app(&root, crate::identity::CLI_APP_SERVER_ID)),
        Req::register("pane-pid-a".into(), "token-pane-pid-a".into(), root.display().to_string(), Some(candidates)));
    assert!(registered.ok, "{registered:?}");
    let candidate = |pane: &str| {
        let mut candidates = test_candidates("thread-pane-pid-b").unwrap();
        let endpoint = &mut candidates.tmux.as_mut().unwrap().endpoint;
        *endpoint = anchor.clone();
        endpoint.pane_id = pane.into();
        endpoint.pane_pid += 1;
        endpoint.codex_session_id = None;
        endpoint.codex_thread_id = None;
        candidates
    };
    let before = serde_json::to_value(&*host.state.lock().unwrap()).unwrap();
    let context = context_with_app(&root, "tui-other");
    let planned = manager.validate_current_thread_candidate(&context, &Req::register(
        "pane-pid-b".into(), "token-pane-pid-b".into(), root.display().to_string(), Some(candidate(&anchor.pane_id)),
    )).unwrap();
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].2.agent_id.as_str(), "pane-pid-a");
    let unrelated = manager.validate_current_thread_candidate(&context, &Req::register(
        "pane-pid-c".into(), "token-pane-pid-c".into(), root.display().to_string(), Some(candidate("%unrelated-pane")),
    )).unwrap();
    assert!(unrelated.is_empty());
    assert_eq!(serde_json::to_value(&*host.state.lock().unwrap()).unwrap(), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_rejected_registration_leaves_the_foreign_pane_claimant_bound() {
    let (host, root, _) = test_server();
    let manager = ProjectRuntimeManager::new(host.clone(), &host.host_paths).unwrap();
    let candidates = test_candidates("thread-rejected-a").unwrap();
    let anchor = candidates.tmux.as_ref().unwrap().endpoint.clone();
    let (_, first) = register_host_fixture(&manager, Some(context_with_app(&root, "appserver-cli")),
        Req::register("rejected-incumbent".into(), "token-rejected-incumbent".into(),
            root.display().to_string(), Some(candidates)));
    assert!(first.ok, "{first:?}");
    let before = serde_json::to_value(&*host.state.lock().unwrap()).unwrap();
    let store = binding_store::BindingControlStore::new(host.host_paths.state_root());
    let map_before = serde_json::to_value(store.load().unwrap()).unwrap();
    let mut candidates = test_candidates("thread-rejected-b").unwrap();
    let endpoint = &mut candidates.tmux.as_mut().unwrap().endpoint;
    *endpoint = anchor;
    endpoint.codex_session_id = None;
    endpoint.codex_thread_id = None;
    let (_, rejected) = manager.dispatch_sync(Some(context_with_app(&root, "tui-other")),
        Req::register("rejected-taker".into(), "token-rejected-taker".into(),
            "/nonexistent/collab-rejected-registration".into(), Some(candidates)));
    assert!(!rejected.ok, "{rejected:?}");
    assert_eq!(serde_json::to_value(&*host.state.lock().unwrap()).unwrap(), before);
    assert_eq!(serde_json::to_value(store.load().unwrap()).unwrap(), map_before);
    std::fs::remove_dir_all(root).unwrap();
}

/// Agent session facts select identity; a pane only locates delivery.
#[tokio::test]
async fn context_agent_session_takes_a_foreign_delivery_anchor() {
    let (host, root, _) = test_server();
    let manager = ProjectRuntimeManager::new(host.clone(), &host.host_paths).unwrap();
    let candidates = test_candidates("thread-cross-scope-retire").unwrap();
    let anchor = candidates.tmux.as_ref().unwrap().endpoint.clone();
    let (_, first) = register_host_fixture(&manager, Some(context_with_app(&root, "tui-other")),
        Req::register("cross-scope-worker".into(), "token-cross-scope-worker".into(),
            root.display().to_string(), Some(candidates)));
    assert!(first.ok, "{first:?}");
    let reported = || {
        let mut candidates = test_candidates("thread-cross-scope-probe").unwrap();
        let endpoint = &mut candidates.tmux.as_mut().unwrap().endpoint;
        *endpoint = anchor.clone();
        endpoint.codex_session_id = Some("session-thread-cross-scope-probe".into());
        endpoint.codex_thread_id = Some("thread-cross-scope-probe".into());
        candidates
    };
    let mut missing = reported();
    missing.tmux.as_mut().unwrap().endpoint.codex_session_id = None;
    let (_, rejected) = register_host_fixture(&manager, Some(context_with_app(&root, "appserver-cli")),
        Req::IdentityContext { facts: crate::proto::IdentityFacts {
            caller_pid: Some(anchor.pane_pid), tmux: missing.tmux, ..Default::default()
        }, identity_context: None });
    assert_eq!(rejected.data["result"]["outcome"], "missing_facts", "{rejected:?}");
    assert!(rejected.data["result"]["requires"]["fields"]
        .as_array().unwrap().contains(&json!("session_id")), "{rejected:?}");
    let (_, taken) = register_host_fixture(&manager, Some(context_with_app(&root, "appserver-cli")),
        Req::IdentityContext { facts: crate::proto::IdentityFacts {
            caller_pid: Some(anchor.pane_pid), session_id: Some("session-thread-cross-scope-probe".into()),
            thread_id: Some("thread-cross-scope-probe".into()), tmux: reported().tmux, ..Default::default()
        }, identity_context: None });
    assert!(taken.ok, "{taken:?}");
    let state = host.state.lock().unwrap();
    let scopes = state.global.current_thread_routes.values().filter(|binding| {
        binding.tmux_endpoint.as_ref().is_some_and(|endpoint| {
            crate::client::adapters::tmux::same_owned_pane(endpoint, &anchor)
        })
    }).map(|binding| binding.app_scope_id.as_str()).collect::<Vec<_>>();
    assert!(!scopes.contains(&"tui-other"), "{scopes:?}");
    assert!(scopes.contains(&"appserver-cli"), "{scopes:?}");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
