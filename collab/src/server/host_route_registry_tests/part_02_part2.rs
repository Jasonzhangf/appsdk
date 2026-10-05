    #[tokio::test]
    async fn split_journal_same_pane_master_is_fenced_until_host_reconcile() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let worker = "split-pane-master";
        let token = "token-split-pane-master";
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
        let old_candidates = test_candidates("split-pane-old").unwrap();
        let first = handle_register_with_app_scope(
            &runtime,
            worker.into(),
            token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(old_candidates.clone()),
        );
        assert!(first.ok, "{first:?}");
        let promoted = super::handle_master_promote(
            &runtime,
            worker.into(),
            token.into(),
            "user approved split-pane-master".into(),
        );
        assert!(promoted.ok, "{promoted:?}");
        let old = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &crate::server::global_state::RouteScope {
                    app_scope_id: app.clone(),
                    project_scope_id: GlobalState::canonical_project_scope(&project_root).unwrap(),
                },
                &BindingId::new("binding-split-pane-master").unwrap(),
            )
            .unwrap()
            .clone();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: old.clone(),
        }])
        .unwrap();
        manager.install_runtime(
            &(
                app.as_str().to_owned(),
                old.project_scope.as_str().to_owned(),
            ),
            runtime.clone(),
            None,
        );
        let mut new_candidates = old_candidates;
        let endpoint = &mut new_candidates.tmux.as_mut().unwrap().endpoint;
        endpoint.codex_session_id = Some("session-split-pane-new".into());
        endpoint.codex_thread_id = Some("split-pane-new".into());
        let context = context_with_app(&project_root, crate::identity::CLI_APP_SERVER_ID);
        let previous = manager
            .registration_rollback_state(&runtime, &context, worker)
            .unwrap();
        let second = handle_register_with_app_scope_unfinalized(
            &runtime,
            worker.into(),
            token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(new_candidates.clone()),
        );
        assert!(second.ok, "{second:?}");
        manager
            .fail_current_thread_route_publish
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let failed = manager.commit_current_thread_route(
            &runtime,
            &context,
            worker,
            previous.0.as_ref(),
            previous.1.as_ref(),
            previous.2.as_ref(),
            &previous.3,
        );
        assert!(failed.is_err());
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
        assert_eq!(
            runtime
                .state
                .lock()
                .unwrap()
                .global
                .lookup_binding_for(&old.route_scope(), &old.binding_id)
                .unwrap(),
            &old,
        );
        manager
            .fail_current_thread_route_publish
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let retry = handle_register_with_app_scope_unfinalized(
            &runtime,
            worker.into(),
            token.into(),
            project_root.display().to_string(),
            Some(app),
            Some(new_candidates.clone()),
        );
        assert!(retry.ok, "{retry:?}");
        assert_eq!(retry.data["command"]["binding"]["endpoint_generation"], old.endpoint_generation + 1);
        assert!(manager.same_pane_master_route_ready(&runtime).is_err());
        let pane = old.tmux_endpoint.as_ref().unwrap();
        assert!(manager.resolve_route_by_tmux_endpoint(pane).is_err());
        assert!(manager.resolve_staged_pane_recovery(pane, worker, "wrong-token").is_err());
        let mut wrong_pane = pane.clone();
        wrong_pane.pane_pid += 1;
        assert!(manager.resolve_staged_pane_recovery(&wrong_pane, worker, token).is_err());
        let recovered = manager.resolve_staged_pane_recovery(pane, worker, token).unwrap();
        assert_eq!(recovered.endpoint_generation, old.endpoint_generation);
        assert_eq!(recovered.agent_id, old.agent_id);
        assert!(manager.same_pane_master_route_ready(&runtime).is_err());
        let previous_runtime = crate::identity::RuntimeIdentity {
            agent_id: old.agent_id.clone(),
            runtime_id: old.runtime_id.clone(),
            appserver_id: old.app_scope_id.clone(),
            endpoint_generation: old.endpoint_generation,
            binding_id: old.binding_id.clone(),
            session_id: old.session_id.clone(),
            native_thread_id: old.native_thread_id.clone(),
        };
        let (_, completed) = manager.dispatch_sync(
            Some(context_with_runtime(&project_root, crate::identity::CLI_APP_SERVER_ID, &previous_runtime)),
            Req::register(worker.into(), token.into(), project_root.display().to_string(), Some(new_candidates)),
        );
        assert!(completed.ok, "{completed:?}");
        assert_eq!(completed.data["command"]["binding"]["endpoint_generation"], old.endpoint_generation + 1);
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn startup_reconciles_a_lagged_same_pane_master_route() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let worker = "lagged-startup-pane-master";
        let token = "token-lagged-startup-pane-master";
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
        let candidates = test_candidates("lagged-startup-pane-master").unwrap();
        let first = handle_register_with_app_scope(
            &runtime,
            worker.into(),
            token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(candidates.clone()),
        );
        assert!(first.ok, "{first:?}");
        let promoted = handle_master_promote(
            &runtime,
            worker.into(),
            token.into(),
            "user approved lagged startup pane master".into(),
        );
        assert!(promoted.ok, "{promoted:?}");
        let mut binding = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &crate::server::global_state::RouteScope {
                    app_scope_id: app.clone(),
                    project_scope_id: GlobalState::canonical_project_scope(&project_root).unwrap(),
                },
                &BindingId::new("binding-lagged-startup-pane-master").unwrap(),
            )
            .unwrap()
            .clone();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: binding.clone(),
        }])
        .unwrap();
        binding.endpoint_generation += 4;
        manager
            .install_runtime(
                &(
                    app.as_str().to_owned(),
                    binding.project_scope.as_str().to_owned(),
                ),
                runtime.clone(),
                None,
            );
        manager.reconcile_same_pane_master_routes().unwrap();
        let endpoint = binding.tmux_endpoint.as_ref().expect("same-pane binding has a pane route");
        let reconciled = host
            .state
            .lock()
            .unwrap()
            .global
            .lookup_unique_tmux_pane_route(endpoint)
            .unwrap()
            .clone();
        assert_eq!(reconciled.endpoint_generation, binding.endpoint_generation - 4);
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn superseded_same_pane_master_does_not_fence_project_route() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();

        // Worker A is the stale same-pane master.
        let master_worker = "stale-same-pane-master";
        let master_token = "token-stale-same-pane-master";
        let master_candidates = test_candidates(master_worker).unwrap();
        let first = handle_register_with_app_scope_unfinalized(
            &runtime,
            master_worker.into(),
            master_token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(master_candidates.clone()),
        );
        assert!(first.ok, "{first:?}");
        let promoted = handle_master_promote(
            &runtime,
            master_worker.into(),
            master_token.into(),
            "user approved same-pane master".into(),
        );
        assert!(promoted.ok, "{promoted:?}");
        let scope = crate::server::global_state::RouteScope {
            app_scope_id: app.clone(),
            project_scope_id: GlobalState::canonical_project_scope(&project_root).unwrap(),
        };
        let master_binding = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(&scope, &BindingId::new(format!("binding-{master_worker}")).unwrap())
            .unwrap()
            .clone();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: master_binding.clone(),
        }])
        .unwrap();
        manager.install_runtime(
            &(app.as_str().to_owned(), master_binding.project_scope.as_str().to_owned()),
            runtime.clone(),
            None,
        );
        // With only the master route published the fence is satisfied.
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());

        // Worker B takes over the same tmux pane with a live native thread.
        let peer_worker = "live-same-pane-peer";
        let peer_token = "token-live-same-pane-peer";
        let mut peer_candidates = master_candidates;
        let endpoint = &mut peer_candidates.tmux.as_mut().unwrap().endpoint;
        endpoint.codex_session_id = Some("session-live-same-pane-peer".into());
        endpoint.codex_thread_id = Some("live-same-pane-peer-thread".into());
        let peer = handle_register_with_app_scope_unfinalized(
            &runtime,
            peer_worker.into(),
            peer_token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(peer_candidates),
        );
        assert!(peer.ok, "{peer:?}");
        let peer_binding = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &scope,
                &BindingId::new(format!("binding-{peer_worker}")).unwrap(),
            )
            .unwrap()
            .clone();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: peer_binding.clone(),
        }])
        .unwrap();

        // Both bindings now share the pane in the host index. The stale master
        // anchor is superseded by the live same-scope peer and must no longer
        // fence the project route.
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
        manager.reconcile_same_pane_master_routes().unwrap();
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    /// A pane may be shared by two projects.  The contract is one claimant per
    /// project scope, so a claimant from another project must neither fence nor
    /// be retired by this project.
    #[tokio::test]
    async fn cross_scope_pane_claimant_does_not_fence_the_project_route() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();

        let master_worker = "cross-scope-pane-master";
        let master_token = "token-cross-scope-pane-master";
        let candidates = test_candidates(master_worker).unwrap();
        let registered = handle_register_with_app_scope_unfinalized(
            &runtime,
            master_worker.into(),
            master_token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(candidates.clone()),
        );
        assert!(registered.ok, "{registered:?}");
        let promoted = handle_master_promote(
            &runtime,
            master_worker.into(),
            master_token.into(),
            "user approved cross-scope pane master".into(),
        );
        assert!(promoted.ok, "{promoted:?}");
        let scope = crate::server::global_state::RouteScope {
            app_scope_id: app.clone(),
            project_scope_id: GlobalState::canonical_project_scope(&project_root).unwrap(),
        };
        let master_binding = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &scope,
                &BindingId::new(format!("binding-{master_worker}")).unwrap(),
            )
            .unwrap()
            .clone();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: master_binding.clone(),
        }])
        .unwrap();
        manager.install_runtime(
            &(
                app.as_str().to_owned(),
                master_binding.project_scope.as_str().to_owned(),
            ),
            runtime.clone(),
            None,
        );

        // A second project registers a peer on the same tmux pane.  Its codex
        // session and thread differ, so it is a separate route address, and its
        // project scope differs, so it is a separate claimant group.
        let (peer_runtime, peer_root, _) = test_server();
        let peer_worker = "cross-scope-pane-peer";
        let mut peer_candidates = candidates;
        let peer_endpoint = &mut peer_candidates.tmux.as_mut().unwrap().endpoint;
        peer_endpoint.codex_session_id = Some("session-cross-scope-pane-peer".into());
        peer_endpoint.codex_thread_id = Some("cross-scope-pane-peer-thread".into());
        let peer = handle_register_with_app_scope_unfinalized(
            &peer_runtime,
            peer_worker.into(),
            "token-cross-scope-pane-peer".into(),
            peer_root.display().to_string(),
            Some(app.clone()),
            Some(peer_candidates),
        );
        assert!(peer.ok, "{peer:?}");
        let peer_scope = crate::server::global_state::RouteScope {
            app_scope_id: app.clone(),
            project_scope_id: GlobalState::canonical_project_scope(&peer_root).unwrap(),
        };
        let peer_binding = peer_runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &peer_scope,
                &BindingId::new(format!("binding-{peer_worker}")).unwrap(),
            )
            .unwrap()
            .clone();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: peer_binding.clone(),
        }])
        .unwrap();

        let endpoint = master_binding
            .tmux_endpoint
            .clone()
            .expect("the master has a pane route");
        {
            let state = host.state.lock().unwrap();
            assert_eq!(
                state.global.tmux_pane_route_claimants(&endpoint).len(),
                2,
                "the pane is shared host-wide"
            );
            assert_eq!(
                state
                    .global
                    .tmux_pane_route_claimants_in_scope(&scope, &endpoint)
                    .len(),
                1,
                "this project has exactly one claimant"
            );
            assert_eq!(
                state
                    .global
                    .lookup_unique_tmux_pane_route_in_scope(&scope, &endpoint)
                    .map(|binding| binding.binding_id.clone()),
                Some(master_binding.binding_id.clone())
            );
            assert!(
                state
                    .global
                    .lookup_unique_tmux_pane_route(&endpoint)
                    .is_none(),
                "the host-wide query stays ambiguous"
            );
        }
        assert!(
            manager.same_pane_master_route_ready(&runtime).is_ok(),
            "a cross-scope claimant must not fence this project's route"
        );
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
        std::fs::remove_dir_all(peer_root).unwrap();
    }

    /// Two claimants inside one project scope stay ambiguous.  The fence must
    /// name the pane, each claimant, and the remedy instead of reporting a
    /// generation mismatch that hides the cause.
    #[tokio::test]
    async fn same_scope_pane_claimants_are_named_in_the_fence_error() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();

        let master_worker = "ambiguous-pane-master";
        let master_token = "token-ambiguous-pane-master";
        let candidates = test_candidates(master_worker).unwrap();
        let registered = handle_register_with_app_scope_unfinalized(
            &runtime,
            master_worker.into(),
            master_token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(candidates),
        );
        assert!(registered.ok, "{registered:?}");
        let promoted = handle_master_promote(
            &runtime,
            master_worker.into(),
            master_token.into(),
            "user approved ambiguous pane master".into(),
        );
        assert!(promoted.ok, "{promoted:?}");
        let scope = crate::server::global_state::RouteScope {
            app_scope_id: app.clone(),
            project_scope_id: GlobalState::canonical_project_scope(&project_root).unwrap(),
        };
        let master_binding = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &scope,
                &BindingId::new(format!("binding-{master_worker}")).unwrap(),
            )
            .unwrap()
            .clone();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: master_binding.clone(),
        }])
        .unwrap();
        manager.install_runtime(
            &(
                app.as_str().to_owned(),
                master_binding.project_scope.as_str().to_owned(),
            ),
            runtime.clone(),
            None,
        );

        // A second claimant in the same scope, on the same pane, whose worker is
        // gone.  It is not a live owner, so it cannot supersede the master, and
        // the pane stays ambiguous inside this scope.
        let mut stale = master_binding.clone();
        stale.agent_id = crate::identity::AgentId::new("stale-ambiguous-claimant").unwrap();
        stale.binding_id = BindingId::new("binding-stale-ambiguous-claimant").unwrap();
        stale.endpoint_generation = 1;
        stale.session_id =
            Some(crate::identity::SessionId::new("session-stale-ambiguous-claimant").unwrap());
        stale.native_thread_id =
            Some(crate::identity::NativeThreadId::new("thread-stale-ambiguous-claimant").unwrap());
        // A distinct Codex address on the same pane, so the binding validates
        // and does not collide with the master's thread address.
        if let Some(endpoint) = stale.tmux_endpoint.as_mut() {
            endpoint.codex_session_id = Some("session-stale-ambiguous-claimant".into());
            endpoint.codex_thread_id = Some("thread-stale-ambiguous-claimant".into());
        }
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: stale.clone(),
        }])
        .unwrap();

        let endpoint = master_binding
            .tmux_endpoint
            .clone()
            .expect("the master has a pane route");
        let error = manager
            .same_pane_master_route_ready(&runtime)
            .expect_err("two same-scope claimants must fence the route");
        assert!(error.contains("RECOVERY_RECONCILE_REQUIRED"), "{error}");
        assert!(
            error.contains(&format!("{}:{}", endpoint.tmux_session_id, endpoint.pane_id)),
            "the error must name the pane: {error}"
        );
        assert!(error.contains(master_worker), "the error must name each claimant: {error}");
        assert!(
            error.contains("stale-ambiguous-claimant"),
            "the error must name each claimant: {error}"
        );
        assert!(
            error.contains("--keep"),
            "the error must name the remedy: {error}"
        );
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn same_pane_master_still_fences_when_host_route_is_missing() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();

        let master_worker = "missing-host-route-pane-master";
        let master_token = "token-missing-host-route-pane-master";
        let candidates = test_candidates(master_worker).unwrap();
        let first = handle_register_with_app_scope_unfinalized(
            &runtime,
            master_worker.into(),
            master_token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(candidates.clone()),
        );
        assert!(first.ok, "{first:?}");
        let promoted = handle_master_promote(
            &runtime,
            master_worker.into(),
            master_token.into(),
            "user approved same-pane master".into(),
        );
        assert!(promoted.ok, "{promoted:?}");
        let scope = crate::server::global_state::RouteScope {
            app_scope_id: app.clone(),
            project_scope_id: GlobalState::canonical_project_scope(&project_root).unwrap(),
        };
        let master_binding = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(&scope, &BindingId::new(format!("binding-{master_worker}")).unwrap())
            .unwrap()
            .clone();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: master_binding.clone(),
        }])
        .unwrap();
        manager.install_runtime(
            &(app.as_str().to_owned(), master_binding.project_scope.as_str().to_owned()),
            runtime.clone(),
            None,
        );
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());

        // The host route is missing again, but a live same-scope peer owns the
        // same pane in the host index. The master anchor is superseded by that
        // peer, so the project route must not fail closed.
        host.commit_checked(&[Event::GlobalCurrentThreadRouteRetired {
            binding: master_binding.clone(),
        }])
        .unwrap();
        let mut peer_candidates = candidates.clone();
        let endpoint = &mut peer_candidates.tmux.as_mut().unwrap().endpoint;
        endpoint.codex_session_id = Some("session-active-same-pane-peer".into());
        endpoint.codex_thread_id = Some("active-same-pane-peer-thread".into());
        let peer = handle_register_with_app_scope_unfinalized(
            &runtime,
            "active-same-pane-peer".into(),
            "token-active-same-pane-peer".into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(peer_candidates),
        );
        assert!(peer.ok, "{peer:?}");
        let peer_binding = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &scope,
                &BindingId::new("binding-active-same-pane-peer".to_string()).unwrap(),
            )
            .unwrap()
            .clone();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: peer_binding,
        }])
        .unwrap();
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn closed_same_pane_peer_does_not_supersede_the_master_anchor() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();

        let master_worker = "closed-peer-pane-master";
        let master_token = "token-closed-peer-pane-master";
        let candidates = test_candidates(master_worker).unwrap();
        let first = handle_register_with_app_scope_unfinalized(
            &runtime,
            master_worker.into(),
            master_token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(candidates.clone()),
        );
        assert!(first.ok, "{first:?}");
        let promoted = handle_master_promote(
            &runtime,
            master_worker.into(),
            master_token.into(),
            "user approved same-pane master".into(),
        );
        assert!(promoted.ok, "{promoted:?}");
        let scope = crate::server::global_state::RouteScope {
            app_scope_id: app.clone(),
            project_scope_id: GlobalState::canonical_project_scope(&project_root).unwrap(),
        };
        let master_binding = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(&scope, &BindingId::new(format!("binding-{master_worker}")).unwrap())
            .unwrap()
            .clone();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: master_binding.clone(),
        }])
        .unwrap();
        manager.install_runtime(
            &(app.as_str().to_owned(), master_binding.project_scope.as_str().to_owned()),
            runtime.clone(),
            None,
        );
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());

        // A second peer takes over the pane with a live native thread, then
        // closes. Its host route survives the close.
        let peer_worker = "closed-same-pane-peer";
        let peer_token = "token-closed-same-pane-peer";
        let mut peer_candidates = candidates.clone();
        let endpoint = &mut peer_candidates.tmux.as_mut().unwrap().endpoint;
        endpoint.codex_session_id = Some("session-closed-same-pane-peer".into());
        endpoint.codex_thread_id = Some("closed-same-pane-peer-thread".into());
        let peer = handle_register_with_app_scope_unfinalized(
            &runtime,
            peer_worker.into(),
            peer_token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(peer_candidates),
        );
        assert!(peer.ok, "{peer:?}");
        let peer_binding = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &scope,
                &BindingId::new(format!("binding-{peer_worker}")).unwrap(),
            )
            .unwrap()
            .clone();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: peer_binding.clone(),
        }])
        .unwrap();
        // Closing the peer through the real close path needs the snapshot
        // receipt its guard requires; the fixture supplies it directly.
        let peer_thread_id = runtime
            .state
            .lock()
            .unwrap()
            .workers
            .get(peer_worker)
            .and_then(|worker| worker.transport.as_ref())
            .and_then(|transport| transport.thread_id.clone())
            .unwrap();
        runtime
            .state
            .lock()
            .unwrap()
            .worker_snapshots
            .insert(
                peer_worker.to_string(),
                crate::server::state::WorkerSnapshotReceipt {
                    worker_id: peer_worker.to_string(),
                    thread_id: peer_thread_id,
                    captured_ms: 0,
                },
            );
        let closed = handle_worker_close(
            &runtime,
            master_worker.into(),
            master_token.into(),
            peer_worker.into(),
            "peer finished".into(),
        );
        assert!(closed.ok, "{closed:?}");
        assert!(runtime
            .state
            .lock()
            .unwrap()
            .workers
            .get(peer_worker)
            .is_none());
        assert!(
            host.state
                .lock()
                .unwrap()
                .global
                .current_thread_routes
                .values()
                .any(|route| route.binding_id == peer_binding.binding_id),
            "a closed peer keeps its durable host route"
        );

        // The master host route is gone again. The durable route of the closed
        // peer is not a live pane owner, so the anchor still fences the project
        // route instead of being silently superseded.
        host.commit_checked(&[Event::GlobalCurrentThreadRouteRetired {
            binding: master_binding,
        }])
        .unwrap();
        let fenced = manager.same_pane_master_route_ready(&runtime);
        assert!(fenced.is_err(), "{fenced:?}");
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

include!("part_02_tail.rs");
