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

        // Publishing the peer at a second address evicts the master anchor from
        // the pane: one pane owns exactly one binding host-wide. The superseded
        // master anchor must no longer fence the project route, and the startup
        // reconciler must not resurrect it.
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
        manager.reconcile_same_pane_master_routes().unwrap();
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
        {
            let state = host.state.lock().unwrap();
            let endpoint = peer_binding.tmux_endpoint.as_ref().unwrap();
            assert_eq!(
                state.global.tmux_pane_route_claimants(endpoint).len(),
                1,
                "the reconciler must not republish a superseded pane claim"
            );
        }
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    /// One pane owns exactly one binding host-wide, across projects.  A second
    /// project that takes the pane evicts the first project's claimant, and the
    /// evicted claimant must still not fence its own project's route.
    #[tokio::test]
    async fn cross_scope_pane_claimant_takes_the_pane_without_fencing_the_project_route() {
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
            let claimants = state.global.tmux_pane_route_claimants(&endpoint);
            assert_eq!(
                claimants.len(),
                1,
                "one pane owns exactly one binding host-wide"
            );
            assert_eq!(
                claimants[0].binding_id, peer_binding.binding_id,
                "the later writer takes the pane"
            );
            assert_eq!(
                state
                    .global
                    .lookup_unique_tmux_pane_route(&endpoint)
                    .map(|binding| binding.binding_id.clone()),
                Some(peer_binding.binding_id.clone())
            );
            assert!(
                state
                    .global
                    .lookup_current_thread_route(
                        master_binding.session_id.as_ref().unwrap(),
                        master_binding.native_thread_id.as_ref().unwrap(),
                    )
                    .is_none(),
                "the evicted project's address is gone from the index"
            );
        }
        assert!(
            manager.same_pane_master_route_ready(&runtime).is_ok(),
            "an evicted claimant must not fence another project's route"
        );
        assert!(
            manager.reconcile_same_pane_master_routes().is_ok(),
            "the startup reconciler must skip the evicted claimant instead of failing"
        );
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
        std::fs::remove_dir_all(peer_root).unwrap();
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

        // The host route is gone and no route holds the pane, so the project
        // route is unowned and the request must fail closed with the remedy.
        host.commit_checked(&[Event::GlobalCurrentThreadRouteRetired {
            binding: master_binding.clone(),
        }])
        .unwrap();
        let fenced = manager
            .same_pane_master_route_ready(&runtime)
            .expect_err("a missing host route must fence the request");
        assert!(fenced.contains("RECOVERY_RECONCILE_REQUIRED"), "{fenced}");
        assert!(fenced.contains("re-register"), "{fenced}");

        // A second address then takes the same pane. The master anchor is
        // superseded by it, so the project route must not fail closed again.
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
    async fn closed_same_pane_peer_keeps_the_pane_ownership() {
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

        // A second peer takes over the pane. The recorded master loses the pane
        // and is closed, because a pane has one owner and can never prove
        // liveness.
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
        assert!(
            runtime
                .state
                .lock()
                .unwrap()
                .workers
                .get(master_worker)
                .is_none(),
            "the later registrant takes the pane and closes the previous claimant"
        );
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

        // Another master closes the peer. The peer's durable host route survives
        // the close, so the pane stays owned by a worker that is gone.
        let closer_worker = "closed-peer-pane-closer";
        let closer_token = "token-closed-peer-pane-closer";
        let closer = handle_register_with_app_scope_unfinalized(
            &runtime,
            closer_worker.into(),
            closer_token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(test_candidates(closer_worker).unwrap()),
        );
        assert!(closer.ok, "{closer:?}");
        let closer_promoted = handle_master_promote(
            &runtime,
            closer_worker.into(),
            closer_token.into(),
            "user approved closer".into(),
        );
        assert!(closer_promoted.ok, "{closer_promoted:?}");
        let closer_binding = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &scope,
                &BindingId::new(format!("binding-{closer_worker}")).unwrap(),
            )
            .unwrap()
            .clone();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: closer_binding,
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
            closer_worker.into(),
            closer_token.into(),
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

        // The pane is still held by the closed peer's durable route. Ownership
        // is a fact of the index and not of liveness, so the master route check
        // must not fence on it.
        let fenced = manager.same_pane_master_route_ready(&runtime);
        assert!(fenced.is_ok(), "{fenced:?}");
        {
            let state = host.state.lock().unwrap();
            assert_eq!(
                state.global.tmux_pane_route_claimants(
                    peer_binding.tmux_endpoint.as_ref().unwrap()
                )
                .len(),
                1,
                "the closed peer's route still owns the pane"
            );
        }
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

include!("part_02_tail.rs");
