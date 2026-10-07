    /// Registers a master through the manager, gives the host the master's
    /// current-thread route, and hands back what a promotion test needs. The
    /// runtime deliberately never receives that host route event, which is the
    /// real split: the runtime's own route index stays empty.
    fn fenced_master_fixture(
        master_worker: &str,
    ) -> (
        std::sync::Arc<Server>,
        std::path::PathBuf,
        std::sync::Arc<Server>,
        std::path::PathBuf,
        std::sync::Arc<ProjectRuntimeManager>,
        AppServerId,
        TransportCandidates,
        crate::server::global_state::RuntimeBinding,
    ) {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();

        let candidates = test_candidates(master_worker).unwrap();
        let first = handle_register_with_app_scope_unfinalized(
            &runtime,
            master_worker.into(),
            format!("token-{master_worker}"),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(candidates.clone()),
        );
        assert!(first.ok, "{first:?}");
        let promoted = handle_master_promote(
            &runtime,
            master_worker.into(),
            format!("token-{master_worker}"),
            "user approved the fenced master".into(),
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
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
        assert!(
            runtime
                .state
                .lock()
                .unwrap()
                .global
                .current_thread_routes
                .is_empty(),
            "the runtime does not receive host route events, so its route index is not authoritative"
        );
        (
            host,
            host_root,
            runtime,
            project_root,
            manager,
            app,
            candidates,
            master_binding,
        )
    }

    /// An explicit, user-approved promotion replaces the recorded incumbent.
    /// The incumbent's pane is only an address, so no probe can veto the
    /// replacement: the approval itself is the authority. A promotion without
    /// approval stays refused.
    #[tokio::test]
    async fn approved_promotion_replaces_the_incumbent_without_a_pane_taker() {
        let (_host, host_root, runtime, project_root, _manager, app, _candidates, _master) =
            fenced_master_fixture("fenced-pane-master");

        // The peer registers on a different pane, so nobody took the anchor.
        let peer_worker = "fenced-other-pane-peer";
        let peer_token = "token-fenced-other-pane-peer";
        let peer = handle_register_with_app_scope_unfinalized(
            &runtime,
            peer_worker.into(),
            peer_token.into(),
            project_root.display().to_string(),
            Some(app),
            test_candidates("fenced-other-pane-peer-thread"),
        );
        assert!(peer.ok, "{peer:?}");

        let unapproved = handle_master_promote(
            &runtime,
            peer_worker.into(),
            peer_token.into(),
            "   ".into(),
        );
        assert!(!unapproved.ok, "{unapproved:?}");

        let promoted = handle_master_promote(
            &runtime,
            peer_worker.into(),
            peer_token.into(),
            "user approved the peer".into(),
        );
        assert!(promoted.ok, "{promoted:?}");
        assert_eq!(promoted.data["master"], peer_worker);
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    /// The live shape that produced the dead loop: a later Codex thread runs in
    /// the recorded master's pane and is present, so the master can no longer
    /// act on that anchor. The owner's explicit approval must then be able to
    /// replace it instead of leaving the project with an unreachable authority.
    #[tokio::test]
    async fn approved_promotion_replaces_the_master_when_a_live_peer_took_its_pane() {
        let (_host, host_root, runtime, project_root, _manager, app, mut candidates, _master) =
            fenced_master_fixture("taken-pane-master");

        let peer_worker = "taken-pane-peer";
        let peer_token = "token-taken-pane-peer";
        let endpoint = &mut candidates.tmux.as_mut().unwrap().endpoint;
        endpoint.codex_session_id = Some("session-taken-pane-peer".into());
        endpoint.codex_thread_id = Some("taken-pane-peer-thread".into());
        let peer = handle_register_with_app_scope_unfinalized(
            &runtime,
            peer_worker.into(),
            peer_token.into(),
            project_root.display().to_string(),
            Some(app),
            Some(candidates),
        );
        assert!(peer.ok, "{peer:?}");
        let promoted = handle_master_promote(
            &runtime,
            peer_worker.into(),
            peer_token.into(),
            "user approved the peer".into(),
        );
        assert!(promoted.ok, "{promoted:?}");
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    /// A peer that moved off the master pane is still replaced by an approved
    /// promotion: pane position is an address, and the approval is the whole
    /// authority for the transition.
    #[tokio::test]
    async fn approved_promotion_replaces_the_incumbent_even_after_the_peer_moved_on() {
        let (_host, host_root, runtime, project_root, _manager, app, mut candidates, _master) =
            fenced_master_fixture("moved-pane-master");

        let peer_worker = "moved-off-pane-peer";
        let peer_token = "token-moved-off-pane-peer";
        let endpoint = &mut candidates.tmux.as_mut().unwrap().endpoint;
        endpoint.codex_session_id = Some("session-moved-off-pane-peer".into());
        endpoint.codex_thread_id = Some("moved-off-pane-peer-thread".into());
        let peer = handle_register_with_app_scope_unfinalized(
            &runtime,
            peer_worker.into(),
            peer_token.into(),
            project_root.display().to_string(),
            Some(app),
            Some(candidates),
        );
        assert!(peer.ok, "{peer:?}");

        let moved = test_candidates("moved-off-pane-peer-new-thread")
            .unwrap()
            .tmux
            .expect("tmux candidate")
            .endpoint;
        runtime
            .state
            .lock()
            .unwrap()
            .workers
            .get_mut(peer_worker)
            .unwrap()
            .transport
            .as_mut()
            .unwrap()
            .tmux_endpoint = Some(moved);

        let promoted = handle_master_promote(
            &runtime,
            peer_worker.into(),
            peer_token.into(),
            "user approved the peer".into(),
        );
        assert!(promoted.ok, "{promoted:?}");
        assert_eq!(promoted.data["master"], peer_worker);
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn same_pane_peer_that_moved_away_still_owns_the_pane() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();

        let master_worker = "moved-peer-pane-master";
        let master_token = "token-moved-peer-pane-master";
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

        // A peer registers on the same pane with a live native thread, so it
        // takes the pane over from the stale master anchor.
        let peer_worker = "moved-same-pane-peer";
        let peer_token = "token-moved-same-pane-peer";
        let mut peer_candidates = candidates.clone();
        let endpoint = &mut peer_candidates.tmux.as_mut().unwrap().endpoint;
        endpoint.codex_session_id = Some("session-moved-same-pane-peer".into());
        endpoint.codex_thread_id = Some("moved-same-pane-peer-thread".into());
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
        host.commit_checked(&[Event::GlobalCurrentThreadRouteRetired {
            binding: master_binding.clone(),
        }])
        .unwrap();
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());

        // The peer rebinds to another pane. Its durable route on the master pane
        // is untouched: pane ownership is an index fact, not the peer's current
        // transport, so the moved peer still owns the master pane.
        let moved = handle_register_with_app_scope_unfinalized(
            &runtime,
            peer_worker.into(),
            peer_token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            test_candidates("moved-same-pane-peer"),
        );
        assert!(moved.ok, "{moved:?}");
        let current = runtime
            .state
            .lock()
            .unwrap()
            .workers
            .get(peer_worker)
            .and_then(|worker| worker.transport.as_ref())
            .and_then(|transport| transport.tmux_endpoint.clone())
            .expect("moved peer transport");
        assert!(
            !crate::client::adapters::tmux::same_pane_route(
                &current,
                peer_binding.tmux_endpoint.as_ref().unwrap()
            ),
            "the moved peer must no longer be on the master pane"
        );

        // A durable route owns its pane until the owner removes it or a fresh
        // registration takes the pane. The peer that moved away left its route
        // on the master pane, so that route still owns the pane and the stale
        // master anchor is superseded instead of fencing the project route.
        let fenced = manager.same_pane_master_route_ready(&runtime);
        assert!(fenced.is_ok(), "{fenced:?}");
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn same_pane_master_still_fences_when_no_peer_owns_the_pane() {
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
            Some(candidates),
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

        // Retire the host route and leave no superseding same-scope peer: the
        // project route must still fail closed instead of being silently
        // served through a missing index entry.
        host.commit_checked(&[Event::GlobalCurrentThreadRouteRetired {
            binding: master_binding.clone(),
        }])
        .unwrap();
        assert!(manager.same_pane_master_route_ready(&runtime).is_err());
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn pane_owner_in_another_pane_does_not_supersede_master() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();

        let master_worker = "other-pane-peer-master";
        let master_token = "token-other-pane-peer-master";
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
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());

        // A live peer in the same project but on a different pane is not a
        // pane owner for the master's pane and must not supersede it. The
        // unique-pane lookup for the master pane must still resolve exactly
        // the master binding.
        let other_worker = "other-pane-live-peer";
        let other_token = "token-other-pane-live-peer";
        let other = handle_register_with_app_scope_unfinalized(
            &runtime,
            other_worker.into(),
            other_token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(test_candidates(other_worker).unwrap()),
        );
        assert!(other.ok, "{other:?}");
        let other_binding = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(&scope, &BindingId::new(format!("binding-{other_worker}")).unwrap())
            .unwrap()
            .clone();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: other_binding,
        }])
        .unwrap();
        let master_pane = master_binding.tmux_endpoint.as_ref().unwrap();
        assert!(host
            .state
            .lock()
            .unwrap()
            .global
            .lookup_unique_tmux_pane_route(master_pane)
            .is_some_and(|route| route.binding_id == master_binding.binding_id));
        assert!(manager.same_pane_master_route_ready(&runtime).is_ok());
        assert!(manager
            .reconcile_same_pane_master_routes()
            .is_ok());
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn startup_publishes_a_missing_current_thread_route_from_the_project_owner() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let worker = "startup-thread-route-recovery";
        let token = "token-startup-thread-route-recovery";
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
        let candidates = test_candidates("startup-thread-route-recovery").unwrap();
        let registered = handle_register_with_app_scope_unfinalized(
            &runtime,
            worker.into(),
            token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(candidates),
        );
        assert!(registered.ok, "{registered:?}");
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
                &BindingId::new("binding-startup-thread-route-recovery").unwrap(),
            )
            .unwrap()
            .clone();
        assert!(host
            .state
            .lock()
            .unwrap()
            .global
            .lookup_current_thread_route(&binding.session_id.clone().unwrap(), &binding.native_thread_id.clone().unwrap())
            .is_none());
        manager
            .install_runtime(
                &(
                    app.as_str().to_owned(),
                    binding.project_scope.as_str().to_owned(),
                ),
                runtime.clone(),
                None,
            );

        manager.reconcile_started_thread_routes().unwrap();

        let published = host
            .state
            .lock()
            .unwrap()
            .global
            .lookup_current_thread_route(&binding.session_id.clone().unwrap(), &binding.native_thread_id.clone().unwrap())
            .unwrap()
            .clone();
        assert_eq!(published, binding);
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn startup_reconciles_a_lagged_current_thread_route_before_request_delivery() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let worker = "startup-stale-thread-route-recovery";
        let token = "token-startup-stale-thread-route-recovery";
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
        let candidates = test_candidates("startup-stale-thread-route-recovery").unwrap();
        let registered = handle_register_with_app_scope_unfinalized(
            &runtime,
            worker.into(),
            token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(candidates),
        );
        assert!(registered.ok, "{registered:?}");
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
                &BindingId::new("binding-startup-stale-thread-route-recovery").unwrap(),
            )
            .unwrap()
            .clone();
        let lagged_binding = RuntimeBinding::new_with_session(
            binding.project_scope.clone(),
            binding.app_scope_id.clone(),
            binding.agent_id.clone(),
            binding.runtime_id.clone(),
            binding.binding_id.clone(),
            1,
            binding.session_id.clone(),
            binding.native_thread_id.clone(),
        )
        .unwrap();
        let mut current_binding = binding.clone();
        current_binding.endpoint_generation = 4;
        runtime
            .commit_checked(&[Event::GlobalRuntimeBound {
                binding: current_binding.clone(),
            }])
            .map_err(|error| error.to_string())
            .unwrap();
        binding = current_binding;
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: lagged_binding,
        }])
        .unwrap();
        manager
            .install_runtime(
                &(
                    app.as_str().to_owned(),
                    binding.project_scope.as_str().to_owned(),
                ),
                runtime.clone(),
                None,
            );

        manager.reconcile_started_thread_routes().unwrap();

        let published = host
            .state
            .lock()
            .unwrap()
            .global
            .lookup_current_thread_route(&binding.session_id.clone().unwrap(), &binding.native_thread_id.clone().unwrap())
            .unwrap()
            .clone();
        assert_eq!(published, binding);
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn failed_first_registration_route_commit_removes_unpublished_worker() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app = "first-route-compensation-app";
        let context = context_with_app(&root, app);
        let worker_id = "first-route-compensation-worker";
        let token = "token-first-route-compensation-worker";
        manager
            .fail_current_thread_route_publish
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let (_, response) = manager.dispatch_sync(
            Some(context),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates("thread-first-route-compensation")),
        );
        assert!(!response.ok, "{response:?}");
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("ROUTE_TRANSITION_DURABILITY_FAILED:")),
            "{response:?}"
        );
        {
            let state = server.state.lock().unwrap();
            assert!(!state.workers.contains_key(worker_id));
            assert!(state
                .notification_subscriptions
                .values()
                .all(|subscription| subscription.worker_id != worker_id));
            assert!(state
                .global
                .projects
                .values()
                .flat_map(|project| project.runtime_bindings.values())
                .all(|binding| binding.agent_id.as_str() != worker_id));
            assert!(state
                .global
                .current_thread_routes
                .values()
                .all(|binding| binding.agent_id.as_str() != worker_id));
        }

        drop(manager);
        let replayed = ProjectRuntimeManager::new(server, &host_paths).unwrap();
        let missing = replayed
            .resolve_route_by_native_thread(
                "session-thread-first-route-compensation",
                "thread-first-route-compensation",
            )
            .unwrap_err();
        assert!(missing.starts_with("ROUTE_RESOLVE_NOT_FOUND"), "{missing}");
        let replayed_state = replayed.host.state.lock().unwrap();
        assert!(!replayed_state.workers.contains_key(worker_id));
        assert!(replayed_state
            .notification_subscriptions
            .values()
            .all(|subscription| subscription.worker_id != worker_id));

        std::fs::remove_dir_all(root).unwrap();
    }

include!("part_02_tail2.rs");
