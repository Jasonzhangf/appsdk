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

    #[tokio::test]
    async fn failed_registration_route_commit_is_explicit_and_replay_safe() {
        let (server, root, _) = test_server();
        let project_root = root.with_file_name(format!(
            "{}-route-compensation",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app = "route-compensation-app";
        let context = context_with_app(&project_root, app);
        let worker_id = "host-route-compensation-worker";
        let token = "token-host-route-compensation-worker";

        let (runtime, registered) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register(worker_id.into(),
                 token.into(),
                 project_root.display().to_string(),
                 test_candidates("thread-host-route-compensation-old")),
        );
        assert!(registered.ok, "{registered:?}");
        assert!(!Arc::ptr_eq(&runtime, &server));
        let binding = server
            .state
            .lock()
            .unwrap()
            .global
            .lookup_current_thread_route(
                &crate::identity::SessionId::new("session-thread-host-route-compensation-old")
                    .unwrap(),
                &NativeThreadId::new("thread-host-route-compensation-old").unwrap(),
            )
            .cloned()
            .unwrap();
        let old_route = manager
            .resolve_route_by_native_thread(
                "session-thread-host-route-compensation-old",
                "thread-host-route-compensation-old",
            )
            .unwrap();
        assert_eq!(old_route.agent_id.as_str(), worker_id);
        let old_grant = {
            let mut state = runtime.state.lock().unwrap();
            state
                .global
                .grant_master(
                    crate::server::global_state::MasterGrant::new(
                        binding.project_scope.clone(),
                        binding.app_scope_id.clone(),
                        binding.agent_id.clone(),
                        "project",
                        "operator",
                        "user approved",
                        binding.binding_id.clone(),
                        binding.endpoint_generation,
                        now_ms(),
                    )
                    .unwrap(),
                )
                .unwrap();
            state
                .global
                .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
                .cloned()
                .unwrap()
        };
        let old_endpoint = binding
            .tmux_endpoint
            .as_ref()
            .expect("registered test peer must have a tmux endpoint");
        let pane_termination = std::process::Command::new("tmux")
            .args([
                "-S",
                &old_endpoint.socket_path,
                "kill-pane",
                "-t",
                &old_endpoint.pane_id,
            ])
            .output()
            .unwrap();
        assert!(
            pane_termination.status.success(),
            "kill only the test-owned old master pane: {}",
            String::from_utf8_lossy(&pane_termination.stderr)
        );
        manager
            .fail_current_thread_route_publish
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let (_, response) = manager.dispatch_sync(
            Some(context_with_runtime(
                &project_root,
                app,
                &RuntimeIdentity {
                    agent_id: binding.agent_id.clone(),
                    runtime_id: binding.runtime_id.clone(),
                    appserver_id: binding.app_scope_id.clone(),
                    endpoint_generation: binding.endpoint_generation,
                    binding_id: binding.binding_id.clone(),
                    session_id: binding.session_id.clone(),
                    native_thread_id: binding.native_thread_id.clone(),
                },
            )),
            Req::register(worker_id.into(),
                 token.into(),
                 project_root.display().to_string(),
                 test_candidates("thread-host-route-compensation-new")),
        );
        assert!(!response.ok, "{response:?}");
        assert!(
            response.error.as_deref().is_some_and(|error| {
                error.starts_with("ROUTE_TRANSITION_DURABILITY_FAILED:")
                    && error.contains("injected current thread route publication failure")
                    && error.contains(
                        "previous worker transport, notification subscriptions, runtime binding, and master grant were restored",
                    )
            }),
            "{response:?}"
        );

        {
            let state = runtime.state.lock().unwrap();
            assert!(state.workers.contains_key(worker_id));
            let restored_worker = state.workers.get(worker_id).unwrap();
            assert_eq!(
                restored_worker
                    .transport
                    .as_ref()
                    .and_then(|transport| transport.thread_id.as_deref()),
                Some("thread-host-route-compensation-old")
            );
            let restored_subscriptions: Vec<_> = state
                .notification_subscriptions
                .values()
                .filter(|subscription| subscription.worker_id == worker_id)
                .collect();
            assert_eq!(restored_subscriptions.len(), 1);
            assert_eq!(
                restored_subscriptions[0].target,
                "thread-host-route-compensation-old"
            );
            assert_eq!(restored_subscriptions[0].status, "armed");
            let current = state
                .global
                .lookup_binding_for(
                    &RouteScope {
                        app_scope_id: AppServerId::new(app).unwrap(),
                        project_scope_id: GlobalState::canonical_project_scope(&project_root)
                            .unwrap(),
                    },
                    &binding.binding_id,
                )
                .unwrap();
            assert_eq!(current, &binding);
            assert_eq!(
                state
                    .global
                    .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id),
                Some(&old_grant)
            );
            assert_eq!(
                state
                    .global
                    .role_for_route(&binding.route_scope(), &binding.binding_id),
                crate::server::global_state::PeerRole::Master
            );
        }
        let old = manager
            .resolve_route_by_native_thread(
                "session-thread-host-route-compensation-old",
                "thread-host-route-compensation-old",
            )
            .unwrap_err();
        assert!(old.starts_with("ROUTE_RESOLVE_NOT_FOUND"), "{old}");
        assert!(old.contains("is gone"), "{old}");
        let new = manager
            .resolve_route_by_native_thread(
                "session-thread-host-route-compensation-new",
                "thread-host-route-compensation-new",
            )
            .unwrap_err();
        assert!(new.starts_with("ROUTE_RESOLVE_NOT_FOUND"), "{new}");

        drop(manager);
        let replayed = ProjectRuntimeManager::new(server, &host_paths).unwrap();
        let old = replayed
            .resolve_route_by_native_thread(
                "session-thread-host-route-compensation-old",
                "thread-host-route-compensation-old",
            )
            .unwrap_err();
        assert!(old.starts_with("ROUTE_RESOLVE_NOT_FOUND"), "{old}");
        let missing = replayed
            .resolve_route_by_native_thread(
                "session-thread-host-route-compensation-new",
                "thread-host-route-compensation-new",
            )
            .unwrap_err();
        assert!(missing.starts_with("ROUTE_RESOLVE_NOT_FOUND"), "{missing}");
        {
            let replayed_runtime = replayed
                .routes
                .lock()
                .unwrap()
                .get(&(
                    app.to_owned(),
                    GlobalState::canonical_project_scope(&project_root)
                        .unwrap()
                        .as_str()
                        .to_owned(),
                ))
                .and_then(|route| route.runtime.clone())
                .unwrap();
            let state = replayed_runtime.state.lock().unwrap();
            assert_eq!(
                state
                    .global
                    .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id),
                Some(&old_grant)
            );
            assert_eq!(
                state
                    .workers
                    .get(worker_id)
                    .unwrap()
                    .transport
                    .as_ref()
                    .and_then(|transport| transport.thread_id.as_deref()),
                Some("thread-host-route-compensation-old")
            );
            let replayed_subscriptions: Vec<_> = state
                .notification_subscriptions
                .values()
                .filter(|subscription| subscription.worker_id == worker_id)
                .collect();
            assert_eq!(replayed_subscriptions.len(), 1);
            assert_eq!(
                replayed_subscriptions[0].target,
                "thread-host-route-compensation-old"
            );
        }

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn ambiguous_registration_route_commit_does_not_restore_stale_binding() {
        let (server, root, _) = test_server();
        let project_root = root.with_file_name(format!(
            "{}-route-ambiguous",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app = "route-ambiguous-app";
        let context = context_with_app(&project_root, app);
        let worker_id = "host-route-ambiguous-worker";
        let token = "token-host-route-ambiguous-worker";

        let (runtime, registered) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register(worker_id.into(),
                 token.into(),
                 project_root.display().to_string(),
                 test_candidates("thread-host-route-ambiguous-old")),
        );
        assert!(registered.ok, "{registered:?}");
        let previous = server
            .state
            .lock()
            .unwrap()
            .global
            .lookup_current_thread_route(
                &crate::identity::SessionId::new("session-thread-host-route-ambiguous-old")
                    .unwrap(),
                &NativeThreadId::new("thread-host-route-ambiguous-old").unwrap(),
            )
            .cloned()
            .unwrap();
        inject_current_thread_route_journal_fault(CurrentThreadRouteJournalFault::Sync);

        let (_, response) = manager.dispatch_sync(
            Some(context_with_runtime(
                &project_root,
                app,
                &RuntimeIdentity {
                    agent_id: previous.agent_id.clone(),
                    runtime_id: previous.runtime_id.clone(),
                    appserver_id: previous.app_scope_id.clone(),
                    endpoint_generation: previous.endpoint_generation,
                    binding_id: previous.binding_id.clone(),
                    session_id: previous.session_id.clone(),
                    native_thread_id: previous.native_thread_id.clone(),
                },
            )),
            Req::register(worker_id.into(),
                 token.into(),
                 project_root.display().to_string(),
                 test_candidates("thread-host-route-ambiguous-new")),
        );
        assert!(!response.ok, "{response:?}");
        assert!(
            response.error.as_deref().is_some_and(|error| {
                error.starts_with("ROUTE_TRANSITION_DURABILITY_FAILED:")
                    && error.contains("publication outcome is unknown")
            }),
            "{response:?}"
        );
        let current = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(&previous.route_scope(), &previous.binding_id)
            .cloned()
            .expect("the durable new binding must not be rolled back");
        assert_eq!(
            current.native_thread_id.as_ref().unwrap().as_str(),
            "thread-host-route-ambiguous-new"
        );
        assert_eq!(
            current.endpoint_generation,
            previous.endpoint_generation + 1
        );
        let replayed_host = replay(&root).unwrap();
        let replayed_route = replayed_host
            .global
            .lookup_current_thread_route(
                &crate::identity::SessionId::new("session-thread-host-route-ambiguous-new")
                    .unwrap(),
                &NativeThreadId::new("thread-host-route-ambiguous-new").unwrap(),
            )
            .expect("the host journal must retain the complete published route");
        assert_eq!(
            replayed_route.endpoint_generation,
            current.endpoint_generation
        );
        assert!(replayed_host
            .global
            .lookup_current_thread_route(
                &crate::identity::SessionId::new("session-thread-host-route-ambiguous-old",)
                    .unwrap(),
                &NativeThreadId::new("thread-host-route-ambiguous-old").unwrap(),
            )
            .is_none());

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn manager_external_register_is_durable_and_replayed_in_its_runtime() {
        let (server, root, host_journal) = test_server();
        let external_root = root.with_file_name(format!(
            "{}-manager-external",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(external_root.join(".agent-collab/server")).unwrap();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let context = context_with_app(&external_root, "manager-external-app");
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();

        let (runtime, response) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register("manager-external-worker".into(),
                 "token-manager-external-worker".into(),
                 external_root.display().to_string(),
                 test_candidates("thread-manager-external-worker")),
        );
        assert!(response.ok, "{response:?}");
        assert!(!Arc::ptr_eq(&runtime, &server));
        assert_eq!(manager.runtimes().len(), 2);
        assert!(host_journal.as_path().exists());
        assert!(
            !std::fs::read(&host_journal).unwrap().is_empty(),
            "external registration must persist its host current-thread transition"
        );
        let route_journal = host_paths.state_root().join("routes.jsonl");
        let route_records = load_host_route_records(&route_journal).unwrap();
        assert_eq!(route_records.len(), 1);
        assert_eq!(route_records[0].app_scope_id, "manager-external-app");
        assert!(
            !std::fs::read(external_root.join(".agent-collab/server/journal.jsonl"))
                .unwrap()
                .is_empty()
        );

        let (_, status) = manager.dispatch_sync(Some(context.clone()), Req::StatusAll);
        assert!(status.ok, "{status:?}");
        assert_eq!(status.data["workers"][0]["id"], "manager-external-worker");

        drop(manager);
        let replayed_manager = ProjectRuntimeManager::new(server, &host_paths).unwrap();
        let (_, replayed_status) = replayed_manager.dispatch_sync(Some(context), Req::StatusAll);
        assert!(replayed_status.ok, "{replayed_status:?}");
        assert_eq!(
            replayed_status.data["workers"][0]["id"],
            "manager-external-worker"
        );

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(external_root).unwrap();
    }

    #[tokio::test]
    async fn manager_routes_project_queries_mutations_and_poll_to_one_runtime() {
        let (server, root, _) = test_server();
        let project_root = root.with_file_name(format!(
            "{}-route-aware",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app_scope = "route-aware-app";
        let sender_id = "route-aware-sender";
        let recipient_id = "route-aware-recipient";
        let sender_token = "token-route-aware-sender";
        let recipient_token = "token-route-aware-recipient";
        let context = context_with_app(&project_root, app_scope);

        let (sender_runtime, sender_registration) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register(sender_id.into(),
                 sender_token.into(),
                 project_root.display().to_string(),
                 test_candidates("thread-route-aware-sender")),
        );
        assert!(sender_registration.ok, "{sender_registration:?}");
        let (recipient_runtime, recipient_registration) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register(recipient_id.into(),
                 recipient_token.into(),
                 project_root.display().to_string(),
                 test_candidates("thread-route-aware-recipient")),
        );
        assert!(recipient_registration.ok, "{recipient_registration:?}");
        assert!(Arc::ptr_eq(&sender_runtime, &recipient_runtime));
        assert!(!Arc::ptr_eq(&sender_runtime, &manager.host));

        let sender_identity =
            runtime_for_registered(&sender_runtime, &project_root, sender_id, app_scope);
        let recipient_identity =
            runtime_for_registered(&recipient_runtime, &project_root, recipient_id, app_scope);
        let sender_context = context_with_runtime(&project_root, app_scope, &sender_identity);
        let recipient_context = context_with_runtime(&project_root, app_scope, &recipient_identity);

        let (_, status) = manager.dispatch_sync(Some(sender_context.clone()), Req::StatusAll);
        assert!(status.ok, "{status:?}");
        assert_eq!(status.data["summary"]["workers"], 2);
        let worker_ids = status.data["workers"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|worker| worker["id"].as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            worker_ids,
            std::collections::BTreeSet::from([sender_id, recipient_id])
        );

        let (_, workers) = manager.dispatch_sync(Some(recipient_context.clone()), Req::Workers);
        assert!(workers.ok, "{workers:?}");
        assert_eq!(workers.data["count"], 2);
        let recipient_worker = workers.data["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|worker| worker["id"] == recipient_id)
            .expect("recipient worker");
        assert_eq!(
            recipient_worker["transport"]["session_id"], "session-thread-route-aware-recipient",
            "live-closure worker-id target binding needs the target App Server session"
        );
        assert_eq!(
            recipient_worker["transport"]["thread_id"],
            "thread-route-aware-recipient"
        );

        let (_, task_registration) = manager.dispatch_sync(
            Some(sender_context.clone()),
            Req::TaskRegister {
                worker_id: sender_id.into(),
                token: sender_token.into(),
                task_id: "route-aware-task".into(),
                owner: None,
                feature_id: Some("route-aware-feature".into()),
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p1".into(),
                next_step: Some("verify route-aware task".into()),
                goal_prompt: None,
            },
        );
        assert!(task_registration.ok, "{task_registration:?}");
        let (_, task_status) = manager.dispatch_sync(
            Some(recipient_context.clone()),
            Req::TaskStatus {
                task_id: Some("route-aware-task".into()),
            },
        );
        assert!(task_status.ok, "{task_status:?}");
        assert_eq!(task_status.data["id"], "route-aware-task");
        assert_eq!(task_status.data["owner"], sender_id);

        let (_, subscription) = manager.dispatch_sync(
            Some(recipient_context.clone()),
            notification_subscribe_request(recipient_id, recipient_token),
        );
        assert!(subscription.ok, "{subscription:?}");
        assert_eq!(subscription.data["subscription"]["worker_id"], recipient_id);

        let scope = GlobalState::canonical_project_scope(&project_root).unwrap();
        let sender_command = CommandEnvelope::new(
            CommandId::new("route-aware-send-command").unwrap(),
            OperationId::new("route-aware-send-operation").unwrap(),
            sender_identity.binding_id.clone(),
            sender_identity.endpoint_generation,
            RouteScope {
                app_scope_id: AppServerId::new(app_scope).unwrap(),
                project_scope_id: scope,
            },
            None,
            None,
            None,
            None,
        );
        let (_, sent) = manager.dispatch_sync(
            Some(sender_context),
            Req::Send {
                from: sender_id.into(),
                worker_id: Some(sender_id.into()),
                token: Some(sender_token.into()),
                command: Some(sender_command),
                to: recipient_id.into(),
                mtype: "notify".into(),
                subject: Some("route-aware message".into()),
                body: "message must stay in the selected runtime".into(),
                in_reply_to: None,
                delivery: "immediate".into(),
            },
        );
        assert!(sent.ok, "{sent:?}");
        let sent_id = sent.data["msg_id"].as_str().unwrap().to_owned();

        let (_, polled) = dispatch_wire_routed(
            manager,
            Some(recipient_context),
            Req::Poll {
                worker_id: recipient_id.into(),
                token: recipient_token.into(),
                timeout_ms: 0,
                receive_id: None,
            },
            tokio::sync::watch::channel(false).1,
        )
        .await;
        assert!(polled.ok, "{polled:?}");
        assert_eq!(polled.data["count"], 1);
        assert_eq!(polled.data["messages"][0]["id"], sent_id);

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    /// A tmux pane is one owned resource. The takeover match is the pane
    /// identity itself — socket, session and pane id, the fields that name one
    /// pane on one tmux server — so a later registration in another app scope is
    /// planned as the pane's new owner even when tmux reused the pane id with a
    /// new pane pid. Planning is not committing: a registration that never
    /// completes must leave the incumbent bound to the pane.
    #[tokio::test]
    async fn pane_takeover_plans_a_foreign_claimant_without_evicting_it() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let context = context_with_app(&root, app);

        let candidates_a = test_candidates("thread-pane-pid-a").unwrap();
        let anchor = candidates_a.tmux.as_ref().unwrap().endpoint.clone();
        let (_, first) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register(
                "pane-pid-a".into(),
                "token-pane-pid-a".into(),
                root.display().to_string(),
                Some(candidates_a),
            ),
        );
        assert!(first.ok, "{first:?}");

        // The probe candidate carries no Codex anchor, so only the pane identity
        // can decide whether it is the same pane.
        let pane_candidate = |pane_id: &str, pane_pid: u32| {
            let mut candidates = test_candidates("thread-pane-pid-b").unwrap();
            let endpoint = &mut candidates.tmux.as_mut().unwrap().endpoint;
            endpoint.socket_path = anchor.socket_path.clone();
            endpoint.server_pid = anchor.server_pid;
            endpoint.tmux_session_id = anchor.tmux_session_id.clone();
            endpoint.pane_id = pane_id.to_string();
            endpoint.pane_pid = pane_pid;
            endpoint.codex_session_id = None;
            endpoint.codex_thread_id = None;
            candidates
        };
        let other_context = context_with_app(&root, "tui-other");

        // Another app scope has its own runtime, so the incumbent is foreign to
        // this registration and the takeover can only be planned here.
        let planned = manager
            .validate_current_thread_candidate(
                &other_context,
                &Req::register(
                    "pane-pid-b".into(),
                    "token-pane-pid-b".into(),
                    root.display().to_string(),
                    Some(pane_candidate(&anchor.pane_id, anchor.pane_pid + 1)),
                ),
            )
            .expect("a foreign claimant on the same pane is planned for takeover");
        assert_eq!(planned.len(), 1, "one pane has one incumbent");
        assert_eq!(planned[0].2.agent_id.as_str(), "pane-pid-a");

        // Planning is not committing. Nothing has retired the incumbent yet.
        let still_bound = server
            .state
            .lock()
            .unwrap()
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .any(|binding| binding.agent_id.as_str() == "pane-pid-a");
        assert!(
            still_bound,
            "a planned takeover must not evict the incumbent before the registration commits"
        );

        // A different pane is not a takeover at all.
        let unrelated = manager
            .validate_current_thread_candidate(
                &other_context,
                &Req::register(
                    "pane-pid-c".into(),
                    "token-pane-pid-c".into(),
                    root.display().to_string(),
                    Some(pane_candidate("%unrelated-pane", anchor.pane_pid)),
                ),
            )
            .expect("an unrelated pane is not a conflict");
        assert!(unrelated.is_empty(), "an unrelated pane plans no retirement");

        std::fs::remove_dir_all(root).unwrap();
    }

    /// A registration that is rejected after the pane scan must not evict the
    /// foreign claimant. The retirement is committed only after the registering
    /// runtime has committed the takeover, so a request that never reaches that
    /// commit leaves the pane owned by its incumbent.
    #[tokio::test]
    async fn a_rejected_registration_leaves_the_foreign_pane_claimant_bound() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();

        let candidates = test_candidates("thread-rejected-a").unwrap();
        let anchor = candidates.tmux.as_ref().unwrap().endpoint.clone();
        let (_, first) = manager.dispatch_sync(
            Some(context_with_app(&root, "appserver-cli")),
            Req::register(
                "rejected-incumbent".into(),
                "token-rejected-incumbent".into(),
                root.display().to_string(),
                Some(candidates),
            ),
        );
        assert!(first.ok, "{first:?}");

        let pane_only = || {
            let mut candidates = test_candidates("thread-rejected-b").unwrap();
            let endpoint = &mut candidates.tmux.as_mut().unwrap().endpoint;
            endpoint.socket_path = anchor.socket_path.clone();
            endpoint.server_pid = anchor.server_pid;
            endpoint.tmux_session_id = anchor.tmux_session_id.clone();
            endpoint.pane_id = anchor.pane_id.clone();
            endpoint.pane_pid = anchor.pane_pid;
            endpoint.codex_session_id = None;
            endpoint.codex_thread_id = None;
            candidates
        };

        // The worktree is outside the project root, so this registration is
        // rejected after the pane scan and before the takeover can commit.
        let (_, rejected) = manager.dispatch_sync(
            Some(context_with_app(&root, "tui-other")),
            Req::register(
                "rejected-taker".into(),
                "token-rejected-taker".into(),
                "/nonexistent/collab-rejected-registration".into(),
                Some(pane_only()),
            ),
        );
        assert!(!rejected.ok, "the foreign worktree must be rejected: {rejected:?}");

        let incumbent = server
            .state
            .lock()
            .unwrap()
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .find(|binding| binding.agent_id.as_str() == "rejected-incumbent")
            .cloned()
            .expect("a rejected registration must not evict the incumbent");
        assert!(
            incumbent.tmux_endpoint.is_some(),
            "the incumbent must keep the pane: {incumbent:?}"
        );
        assert!(
            incumbent.native_thread_id.is_some(),
            "the incumbent must keep its thread anchor: {incumbent:?}"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// A pane anchor is a resource, not a scope lock: a later registration in
    /// another scope takes the pane, retires the foreign route and owns the
    /// anchor. No extra flag is needed, because the later registrant wins.
    #[tokio::test]
    async fn a_later_registration_takes_a_foreign_scope_anchor() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();

        let candidates = test_candidates("thread-cross-scope-retire").unwrap();
        let anchor = candidates.tmux.as_ref().unwrap().endpoint.clone();
        let (_, first) = manager.dispatch_sync(
            Some(context_with_app(&root, "appserver-cli")),
            Req::register(
                "cross-scope-worker".into(),
                "token-cross-scope-worker".into(),
                root.display().to_string(),
                Some(candidates),
            ),
        );
        assert!(first.ok, "{first:?}");

        // Same pane, no native thread: only the pane anchor can match.
        let pane_only = || {
            let mut candidates = test_candidates("thread-cross-scope-probe").unwrap();
            let endpoint = &mut candidates.tmux.as_mut().unwrap().endpoint;
            endpoint.socket_path = anchor.socket_path.clone();
            endpoint.server_pid = anchor.server_pid;
            endpoint.tmux_session_id = anchor.tmux_session_id.clone();
            endpoint.pane_id = anchor.pane_id.clone();
            endpoint.pane_pid = anchor.pane_pid;
            endpoint.codex_session_id = None;
            endpoint.codex_thread_id = None;
            candidates
        };

        let (_, taken) = manager.dispatch_sync(
            Some(context_with_app(&root, "tui-other")),
            Req::register(
                "cross-scope-worker".into(),
                "token-cross-scope-worker".into(),
                root.display().to_string(),
                Some(pane_only()),
            ),
        );
        assert!(taken.ok, "{taken:?}");

        let pane_owners = |server: &Arc<Server>| {
            server
                .state
                .lock()
                .unwrap()
                .global
                .current_thread_routes
                .values()
                .filter(|binding| {
                    binding.tmux_endpoint.as_ref().is_some_and(|endpoint| {
                        endpoint.socket_path == anchor.socket_path
                            && endpoint.tmux_session_id == anchor.tmux_session_id
                            && endpoint.pane_id == anchor.pane_id
                            && endpoint.pane_pid == anchor.pane_pid
                    })
                })
                .map(|binding| binding.app_scope_id.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        let owners = pane_owners(&server);
        assert!(
            !owners.iter().any(|scope| scope == "appserver-cli"),
            "the foreign route must be retired: {owners:?}"
        );
        assert!(
            owners.iter().any(|scope| scope == "tui-other"),
            "the current scope must own the anchor: {owners:?}"
        );

        std::fs::remove_dir_all(root).unwrap();
    }
