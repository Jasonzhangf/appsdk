    #[test]
    fn registered_project_without_migrated_reducer_is_explicitly_not_ready() {
        let (server, root, _) = test_server();
        let other_root = root.with_file_name(format!(
            "{}-other",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&other_root).unwrap();
        register_known_project(&server, &other_root);
        let error = validate_request_context(&server, &Req::StatusAll, Some(&context(&other_root)))
            .unwrap_err();
        assert!(
            error.starts_with("PROJECT_ROUTE_NOT_READY/UNSUPPORTED:"),
            "{error}"
        );
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(other_root).unwrap();
    }

    #[test]
    fn context_scope_spoof_is_rejected_before_route_lookup() {
        let (server, root, _) = test_server();
        let other_root = root.with_file_name(format!(
            "{}-other",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&other_root).unwrap();
        let mut spoofed = context(&other_root);
        spoofed.project_scope = context(&root).project_scope;
        let error = validate_request_context(&server, &Req::StatusAll, Some(&spoofed)).unwrap_err();
        assert!(error.starts_with("PROJECT_CONTEXT_INVALID:"), "{error}");
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(other_root).unwrap();
    }

    #[tokio::test]
    async fn not_ready_route_does_not_mutate_current_state() {
        let (server, root, state_path) = test_server();
        let other_root = root.with_file_name(format!(
            "{}-other",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&other_root).unwrap();
        register_known_project(&server, &other_root);
        let before_state = std::fs::read(&state_path).unwrap();
        let before_revision = server.state.lock().unwrap().revision;
        let response =
            dispatch_wire(server.clone(), Some(context(&other_root)), Req::StatusAll).await;
        assert!(!response.ok);
        assert!(response
            .error
            .unwrap()
            .starts_with("PROJECT_ROUTE_NOT_READY/UNSUPPORTED:"));
        assert_eq!(std::fs::read(&state_path).unwrap(), before_state);
        assert_eq!(server.state.lock().unwrap().revision, before_revision);
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(other_root).unwrap();
    }

    #[tokio::test]
    async fn cross_project_wire_request_is_rejected_without_resident_mutation() {
        let (server, root, state_path) = test_server();
        let project_context = context_with_app(&root, "app-wire");
        let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
        let master_binding = commit_host_route_fixture(&manager, &server, "target-master", "token-target-master",
            project_context.app_scope_id.as_str(), committed_tmux_endpoint(&root, "thread-target-master"));
        grant_master_fixture(&server, &master_binding);

        let mailbox_dir = root.join(".agent-collab/mailbox");
        std::fs::create_dir_all(&mailbox_dir).unwrap();
        let mailbox_path = mailbox_dir.join("recipient-target-master.jsonl");
        std::fs::write(&mailbox_path, b"sentinel\n").unwrap();
        let mailbox_snapshot = |directory: &Path| {
            let mut entries = std::fs::read_dir(directory)
                .unwrap()
                .map(|entry| {
                    let path = entry.unwrap().path();
                    (
                        path.file_name().unwrap().to_string_lossy().into_owned(),
                        std::fs::read(path).unwrap(),
                    )
                })
                .collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            entries
        };
        let before_mailbox = mailbox_snapshot(&mailbox_dir);
        let before_state_file = std::fs::read(&state_path).unwrap();
        let message_snapshot = |messages: &std::collections::HashMap<String, Message>| {
            let mut entries = messages
                .iter()
                .map(|(id, message)| (id.clone(), serde_json::to_string(message).unwrap()))
                .collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            entries
        };
        let before_state = {
            let state = server.state.lock().unwrap();
            (
                state.revision,
                state.sequence,
                message_snapshot(&state.msgs),
                state.delivery_modes.clone(),
                state.wake_bindings.clone(),
                state.global.clone(),
            )
        };

        let response = dispatch_wire(
            server.clone(),
            Some(project_context),
            Req::CrossProjectSend {
                from: "source-master".into(),
                from_project: "/foreign/project".into(),
                worker_id: "source-master".into(),
                token: "token-source-master".into(),
                command: None,
                to: "target-master".into(),
                subject: "cross-project".into(),
                body: "must remain outside resident reducer".into(),
                in_reply_to: None,
                reply_policy: None,
                target_session: None,
            },
        )
        .await;
        assert!(!response.ok, "{response:?}");
        assert!(response
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("PROJECT_ROUTE_NOT_READY/UNSUPPORTED:")));

        let state = server.state.lock().unwrap();
        assert_eq!(state.revision, before_state.0);
        assert_eq!(state.sequence, before_state.1);
        assert_eq!(message_snapshot(&state.msgs), before_state.2);
        assert_eq!(state.delivery_modes, before_state.3);
        assert_eq!(state.wake_bindings, before_state.4);
        assert_eq!(state.global, before_state.5);
        drop(state);
        assert_eq!(std::fs::read(&state_path).unwrap(), before_state_file);
        assert_eq!(mailbox_snapshot(&mailbox_dir), before_mailbox);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn repeated_registry_builds_do_not_duplicate_or_replace_route_owner() {
        let (server, root, _) = test_server();
        register_known_project(&server, &root);
        let other_root = root.with_file_name(format!(
            "{}-other",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&other_root).unwrap();
        register_known_project(&server, &other_root);
        let first = HostRouteRegistry::for_server(&server).unwrap();
        let second = HostRouteRegistry::for_server(&server).unwrap();
        assert_eq!(first.routes, second.routes);
        assert!(matches!(
            first.lookup(&context(&root)),
            Some(HostRouteOwner::ResidentProject { .. })
        ));
        assert!(matches!(
            first.lookup(&context(&other_root)),
            Some(HostRouteOwner::RegisteredNotReady { .. })
        ));
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(other_root).unwrap();
    }

    #[test]
    fn resident_route_requires_its_registered_app_scope() {
        let (server, root, _) = test_server();
        register_known_project_with_app(&server, &root, "app-a");

        assert!(validate_request_context(
            &server,
            &Req::StatusAll,
            Some(&context_with_app(&root, "app-a"))
        )
        .is_ok());
        let unknown_app = validate_request_context(
            &server,
            &Req::StatusAll,
            Some(&context_with_app(&root, "app-b")),
        )
        .unwrap_err();
        assert!(
            unknown_app.starts_with("PROJECT_SCOPE_UNKNOWN:"),
            "{unknown_app}"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fixed_route_cannot_replay_the_host_storage_as_a_project() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_file = host_paths.state_root().join("routes.json");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let canonical_root = host_paths.state_root().canonicalize().unwrap();
        // Keep the route replayable so the actual host-storage exclusion,
        // rather than deinitialized-project pruning, decides the result.
        std::fs::create_dir_all(canonical_root.join(".agent-collab")).unwrap();
        let record = HostRouteRecord {
            app_scope_id: crate::identity::CLI_APP_SERVER_ID.into(),
            project_scope: canonical_root.to_string_lossy().into_owned(),
            canonical_root: canonical_root.to_string_lossy().into_owned(),
            storage_root: canonical_root.to_string_lossy().into_owned(),
            registered_ms: 1,
        };
        binding_store::publish_route_records(&route_file, &[record]).unwrap().require_confirmed_route().unwrap();

        let before_routes = std::fs::read(&route_file).unwrap();
        let before_state = std::fs::read(&server.state_path).unwrap();
        let error = match ProjectRuntimeManager::new(server.clone(), &host_paths) {
            Ok(_) => panic!("host storage cannot become a project reducer"),
            Err(error) => error,
        };
        assert!(error.starts_with("HOST_ROUTE_REPLAY_FAILED:"), "{error}");
        assert!(error.contains("daemon runtime storage root"), "{error}");
        assert_eq!(std::fs::read(&route_file).unwrap(), before_routes);
        assert_eq!(std::fs::read(&server.state_path).unwrap(), before_state);
        assert!(!host_paths.state_root().join("projects").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn non_cli_fixed_route_reopens_its_own_project_registration() {
        let (server, host_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let route_file = host_paths.state_root().join("routes.json");
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app_scope = "resident-app";
        let context = context_with_app(&host_root, app_scope);

        manager.reserve_external_route(&context, &manager.storage_root_for_new(
            &host_root, context.project_scope.as_str(), app_scope,
        )).unwrap();
        let runtime = manager.select_runtime(&context).unwrap();
        let registration = ProjectRegistration::with_registered_at(
            context.project_scope.clone(), context.app_scope_id.clone(), 23,
        ).unwrap();
        runtime.commit_checked(&[Event::GlobalProjectRegistered {
            registration: registration.clone(),
        }]).unwrap();
        assert!(!Arc::ptr_eq(&runtime, &server));
        let storage_root = runtime.storage_root.clone();
        let records = load_host_route_records(&route_file).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].app_scope_id, app_scope);
        assert_eq!(
            storage_owner_path(Path::new(&records[0].storage_root)).unwrap(),
            storage_owner_path(&storage_root).unwrap()
        );

        let before_routes = std::fs::read(&route_file).unwrap();
        let before_host = std::fs::read(&server.state_path).unwrap();
        drop(runtime);
        drop(manager);

        let replayed_manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let replayed_runtime = replayed_manager.select_runtime(&context).unwrap();
        assert!(!Arc::ptr_eq(&replayed_runtime, &server));
        assert_eq!(replayed_runtime.storage_root, storage_root);
        assert_eq!(replayed_runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_registration(
                &context.project_scope,
                &context.app_scope_id,
            )
            .cloned(), Some(registration));
        assert_eq!(std::fs::read(&route_file).unwrap(), before_routes);
        assert_eq!(std::fs::read(&server.state_path).unwrap(), before_host);
        drop(replayed_runtime);
        drop(replayed_manager);
        drop(server);
        std::fs::remove_dir_all(host_root).unwrap();
    }

    #[test]
    fn resident_route_publish_failure_does_not_leave_a_loadable_route() {
        let (mut server, root, _) = test_server();
        with_appserver_check(&mut server, |candidate| Ok(verified_appserver(candidate)));
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_file = host_paths.state_root().join("routes.json");
        let state_before_file = std::fs::read(&server.state_path).unwrap();
        let mut manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let route_blocker = root.join("route-publication-blocker");
        std::fs::create_dir_all(&route_blocker).unwrap();
        Arc::get_mut(&mut manager).unwrap().route_file = route_blocker;
        let state_before = {
            let state = server.state.lock().unwrap();
            (
                state.revision,
                state.sequence,
                state.global.clone(),
                state.workers.clone(),
            )
        };

        let context = context_with_app(&root, crate::identity::CLI_APP_SERVER_ID);
        let (_, failed) = register_host_fixture(&manager,
            Some(context),
            Req::register("resident-route-failure-worker".into(),
                 "token-resident-route-failure-worker".into(),
                 root.display().to_string(),
                 test_candidates("thread-resident-route-failure-worker")),
        );
        assert!(!failed.ok, "{failed:?}");
        assert!(
            failed
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("HOST_ROUTE_DURABILITY_FAILED:")),
            "{failed:?}"
        );
        assert!(
            !route_file.exists(),
            "failed publish installed a current route"
        );
        assert_eq!(
            std::fs::read(&server.state_path).unwrap(),
            state_before_file,
            "failed route publication mutated the current business state"
        );
        {
            let state = server.state.lock().unwrap();
            assert_eq!(state.revision, state_before.0);
            assert_eq!(state.sequence, state_before.1);
            assert_eq!(state.global, state_before.2);
            assert_eq!(
                state.workers.keys().cloned().collect::<Vec<_>>(),
                state_before.3.keys().cloned().collect::<Vec<_>>()
            );
        }

        drop(manager);
        let replayed = ProjectRuntimeManager::new(server, &host_paths).unwrap();
        let missing = replayed
            .resolve_route_by_native_thread(
                "session-thread-resident-route-failure-worker",
                "thread-resident-route-failure-worker",
            )
            .unwrap_err();
        assert!(missing.starts_with("ROUTE_RESOLVE_NOT_FOUND"), "{missing}");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_route_rejects_storage_for_another_app_during_load() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_file = host_paths.state_root().join("routes.json");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let canonical_root = root.canonicalize().unwrap();
        let record = HostRouteRecord {
            app_scope_id: crate::identity::CLI_APP_SERVER_ID.into(),
            project_scope: canonical_root.to_string_lossy().into_owned(),
            canonical_root: canonical_root.to_string_lossy().into_owned(),
            storage_root: app_scope_storage_path(&canonical_root, "app-a")
                .to_string_lossy()
                .into_owned(),
            registered_ms: 1,
        };
        binding_store::publish_route_records(&route_file, &[record]).unwrap().require_confirmed_route().unwrap();

        let error = match ProjectRuntimeManager::new(server, &host_paths) {
            Ok(_) => panic!("route using another app's storage was replayed"),
            Err(error) => error,
        };
        assert!(
            error.contains("is outside both central project storage"),
            "unexpected replay error: {error}"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn central_namespace_initializes_only_before_it_is_declared() {
        let (server, root, _) = test_server();
        let paths = &server.host_paths;
        let project = GlobalState::canonical_project_scope(&root).unwrap();
        let key = ("current-namespace".into(), project.as_str().into());
        let route_file = paths.state_root().join("routes.json");
        let storage = select_central_runtime_storage(paths, &route_file, &key, None).unwrap();
        let state_path = storage.join(".agent-collab/server/state.json");
        assert!(current_state::load(&state_path, false).unwrap().workers.is_empty());
        std::fs::remove_file(&state_path).unwrap();
        binding_store::publish_route_records(&route_file, &[HostRouteRecord {
            app_scope_id: key.0.clone(), project_scope: key.1.clone(), canonical_root: key.1,
            storage_root: storage.display().to_string(), registered_ms: 1,
        }]).unwrap().require_confirmed_route().unwrap();
        let before = std::fs::read(&route_file).unwrap();
        let error = select_central_runtime_storage(paths, &route_file, &(key.0, project.as_str().into()), None).unwrap_err();
        assert!(error.starts_with("CURRENT_STATE_MIGRATION_REQUIRED:"), "{error}");
        assert!(!state_path.exists());
        assert_eq!(std::fs::read(&route_file).unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_project_secondary_app_is_not_ready_without_a_second_reducer() {
        let (server, root, _) = test_server();
        register_known_project_with_app(&server, &root, "app-a");
        assert!(
            seed_published_registration_fixture(
                &server,
                "resident".into(),
                "token-resident".into(),
                root.display().to_string(),
                Some(AppServerId::new("app-a").unwrap()),
                test_candidates("thread-resident"),
            )
            .ok
        );
        register_known_project_with_app(&server, &root, "app-b");
        let registry = HostRouteRegistry::for_server(&server).unwrap();

        assert!(matches!(
            registry.lookup(&context_with_app(&root, "app-a")),
            Some(HostRouteOwner::ResidentProject { .. })
        ));
        assert!(matches!(
            registry.lookup(&context_with_app(&root, "app-b")),
            Some(HostRouteOwner::RegisteredNotReady { .. })
        ));
        let error = validate_request_context(
            &server,
            &Req::StatusAll,
            Some(&context_with_app(&root, "app-b")),
        )
        .unwrap_err();
        assert!(
            error.starts_with("PROJECT_ROUTE_NOT_READY/UNSUPPORTED:"),
            "{error}"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn host_registrations_do_not_replace_the_fixed_route_resource() {
        let (server, root, state_path) = test_server();
        register_known_project_with_app(&server, &root, "app-a");
        register_known_project_with_app(&server, &root, "app-b");

        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_file = host_paths.state_root().join("routes.json");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        binding_store::publish_route_records(&route_file, &[]).unwrap().require_confirmed_route().unwrap();
        let before_routes = std::fs::read(&route_file).unwrap();
        let before_state = std::fs::read(&state_path).unwrap();

        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths)
            .expect("ordinary startup must ignore project registrations in host state");
        assert!(manager.runtimes().is_empty());
        assert!(manager.routes.lock().unwrap().is_empty());
        assert_eq!(std::fs::read(&route_file).unwrap(), before_routes);
        assert_eq!(
            std::fs::read(&state_path).unwrap(),
            before_state
        );
        assert!(!root.join(".agent-collab/server/runtimes").exists());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_route_root_is_pruned_from_current_routes() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_file = host_paths.state_root().join("routes.json");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let missing_root = root.join("missing-project");
        let record = HostRouteRecord {
            app_scope_id: "appserver-cli".into(),
            project_scope: missing_root.to_string_lossy().into_owned(),
            canonical_root: missing_root.to_string_lossy().into_owned(),
            storage_root: missing_root.to_string_lossy().into_owned(),
            registered_ms: 1,
        };
        binding_store::publish_route_records(&route_file, &[record]).unwrap().require_confirmed_route().unwrap();

        let manager = ProjectRuntimeManager::new(server, &host_paths).unwrap();

        assert!(manager.routes.lock().unwrap().is_empty());
        assert!(load_host_route_records(&route_file).unwrap().is_empty());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fixed_route_at_host_project_root_loads_an_independent_owner() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_file = host_paths.state_root().join("routes.json");
        register_known_project_with_app(&server, &root, crate::identity::CLI_APP_SERVER_ID);
        let context = context_with_app(&root, crate::identity::CLI_APP_SERVER_ID);
        let storage = host_paths.app_storage_root(context.project_scope.as_str(), context.app_scope_id.as_str());
        let state_path = storage.join(".agent-collab/server/state.json");
        let mut project = State::default();
        project.target_sessions.insert("project-marker".into(), "project-value".into());
        project.master_wake.goal_due = true;
        current_state::save(&state_path, &project).unwrap();
        append_host_route_record(&route_file, &HostRouteRecord {
            app_scope_id: context.app_scope_id.as_str().into(),
            project_scope: context.project_scope.as_str().into(),
            canonical_root: context.canonical_root.clone(),
            storage_root: storage.to_string_lossy().into_owned(),
            registered_ms: 17,
        }).unwrap();
        let before_routes = std::fs::read(&route_file).unwrap();
        let before_host = std::fs::read(&server.state_path).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        assert!(manager.runtimes().is_empty(), "unused fixed routes remain lazy");
        let selected = manager.select_runtime(&context).unwrap();
        assert!(!Arc::ptr_eq(&selected, &server));
        assert_eq!(selected.state_path, state_path.canonicalize().unwrap());
        assert_eq!(selected.state.lock().unwrap().target_sessions["project-marker"], "project-value");
        assert!(!selected.state.lock().unwrap().master_wake.goal_due,
            "stale goal reconciliation belongs to the selected project");
        assert_eq!(manager.runtimes().len(), 1);
        assert!(Arc::ptr_eq(&manager.runtimes()[0], &selected));
        assert_eq!(std::fs::read(&route_file).unwrap(), before_routes);
        assert_eq!(std::fs::read(&server.state_path).unwrap(), before_host);
        drop(selected);
        drop(manager);
        drop(server);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreadable_fixed_route_resource_blocks_startup() {
        let (server, root, _) = test_server();
        register_known_project_with_app(&server, &root, crate::identity::CLI_APP_SERVER_ID);
        let blocker = root.join("route-publication-state-root");
        std::fs::create_dir_all(blocker.join("routes.json")).unwrap();
        let blocked_host_paths = HostPaths::for_state_root(blocker).unwrap();

        let error = match ProjectRuntimeManager::new(server, &blocked_host_paths) {
            Ok(_) => panic!("an unreadable fixed route resource must block daemon startup"),
            Err(error) => error,
        };
        assert!(error.starts_with("HOST_ROUTE_READ_FAILED:"), "{error}");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn uninitialized_route_root_is_pruned_from_current_routes() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_file = host_paths.state_root().join("routes.json");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let uninitialized_root = root.join("uninitialized-project");
        std::fs::create_dir_all(&uninitialized_root).unwrap();
        let canonical_root = uninitialized_root.canonicalize().unwrap();
        let record = HostRouteRecord {
            app_scope_id: "appserver-cli".into(),
            project_scope: canonical_root.to_string_lossy().into_owned(),
            canonical_root: canonical_root.to_string_lossy().into_owned(),
            storage_root: canonical_root.to_string_lossy().into_owned(),
            registered_ms: 1,
        };
        binding_store::publish_route_records(&route_file, &[record]).unwrap().require_confirmed_route().unwrap();

        let manager = ProjectRuntimeManager::new(server, &host_paths).unwrap();

        assert!(manager.routes.lock().unwrap().is_empty());
        assert!(load_host_route_records(&route_file).unwrap().is_empty());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn storage_owner_identity_resolves_symlinked_parent_with_missing_tail() {
        use std::os::unix::fs::symlink;

        let (_server, root, _) = test_server();
        let real_parent = root.join("real-parent");
        let alias_parent = root.join("alias-parent");
        std::fs::create_dir_all(&real_parent).unwrap();
        symlink(&real_parent, &alias_parent).unwrap();

        let real_future_path = real_parent.join("future").join("state");
        let aliased_future_path = alias_parent.join("future").join("state");
        assert_eq!(
            storage_owner_path(&real_future_path).unwrap(),
            storage_owner_path(&aliased_future_path).unwrap()
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn storage_owner_identity_resolves_dangling_symlink_with_missing_target() {
        use std::os::unix::fs::symlink;

        let (_server, root, _) = test_server();
        let missing_parent = root.join("missing-parent");
        let dangling_parent = root.join("dangling-parent");
        symlink(&missing_parent, &dangling_parent).unwrap();

        let target_path = missing_parent.join("future").join("state");
        let dangling_path = dangling_parent.join("future").join("state");
        assert_eq!(
            storage_owner_path(&target_path).unwrap(),
            storage_owner_path(&dangling_path).unwrap()
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_storage_symlink_escape_rejects_register_before_state_write() {
        use std::os::unix::fs::symlink;

        let (server, host_root, _) = test_server();
        let project_root = host_root.with_file_name(format!(
            "{}-runtime-escape",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(project_root.join(".agent-collab/server/runtimes")).unwrap();
        let external_root = host_root.with_file_name(format!(
            "{}-runtime-escape-target",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&external_root).unwrap();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let seed_context = context_with_app(&project_root, "runtime-seed");
        let (_, seed_response) = register_host_fixture(&manager,
            Some(seed_context),
            Req::register("runtime-seed-worker".into(),
                 "token-runtime-seed-worker".into(),
                 project_root.display().to_string(),
                 test_candidates_at("thread-runtime-seed-worker", &project_root)),
        );
        assert!(seed_response.ok, "{seed_response:?}");

        let escape_context = context_with_app(&project_root, "runtime-escape");
        let escape_storage_root = host_paths.app_storage_root(
            escape_context.project_scope.as_str(),
            escape_context.app_scope_id.as_str(),
        );
        std::fs::create_dir_all(escape_storage_root.parent().unwrap()).unwrap();
        symlink(&external_root, &escape_storage_root).unwrap();
        let route_file = host_paths.state_root().join("routes.json");
        let before_routes = std::fs::read(&route_file).unwrap();
        let (_, escape_response) = register_host_fixture(&manager,
            Some(escape_context.clone()),
            Req::register("runtime-escape-worker".into(),
                 "token-runtime-escape-worker".into(),
                 project_root.display().to_string(),
                 test_candidates_at("thread-runtime-escape-worker", &project_root)),
        );

        assert!(!escape_response.ok, "{escape_response:?}");
        assert!(
            escape_response.error.as_deref().is_some_and(|error| {
                error.starts_with("HOST_ROUTE_DURABILITY_FAILED:")
                    && error.contains("resolves outside project runtime storage")
            }),
            "{escape_response:?}"
        );
        assert_eq!(std::fs::read(&route_file).unwrap(), before_routes);
        assert!(!manager
            .routes
            .lock()
            .unwrap()
            .contains_key(&ProjectRuntimeManager::route_key(&escape_context)));
        assert!(!external_root
            .join(".agent-collab/server/state.json")
            .exists());

        let escape_key = (
            escape_context.app_scope_id.as_str().to_owned(),
            escape_context.project_scope.as_str().to_owned(),
        );
        let build_error = match manager.build_runtime(&escape_key, &project_root, &escape_storage_root) {
            Ok(_) => panic!("runtime storage symlink escape must fail closed"),
            Err(error) => error,
        };
        assert!(
            build_error.starts_with("PROJECT_ROUTE_NOT_READY/UNSUPPORTED:")
                && build_error.contains("resolves outside project runtime storage"),
            "{build_error}"
        );
        assert!(!external_root
            .join(".agent-collab/server/state.json")
            .exists());

        drop(manager);
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
        std::fs::remove_dir_all(external_root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn runtime_storage_symlink_escape_rejects_current_route_load_before_runtime_creation() {
        use std::os::unix::fs::symlink;

        let (server, host_root, _) = test_server();
        let project_root = host_root.with_file_name(format!(
            "{}-replay-runtime-escape",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(project_root.join(".agent-collab/server/runtimes")).unwrap();
        let external_root = host_root.with_file_name(format!(
            "{}-replay-runtime-escape-target",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&external_root).unwrap();
        let canonical_project_root = project_root.canonicalize().unwrap();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let project_scope = GlobalState::canonical_project_scope(&canonical_project_root).unwrap();
        let storage_root =
            host_paths.app_storage_root(project_scope.as_str(), "replay-escape");
        std::fs::create_dir_all(storage_root.parent().unwrap()).unwrap();
        symlink(&external_root, &storage_root).unwrap();
        let record = HostRouteRecord {
            app_scope_id: "replay-escape".into(),
            project_scope: project_scope.as_str().into(),
            canonical_root: project_scope.as_str().into(),
            storage_root: storage_root.to_string_lossy().into_owned(),
            registered_ms: 1,
        };
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let route_file = host_paths.state_root().join("routes.json");
        binding_store::publish_route_records(&route_file, &[record]).unwrap().require_confirmed_route().unwrap();

        let replay_error = match ProjectRuntimeManager::new(server, &host_paths) {
            Ok(_) => panic!("runtime storage symlink escape must fail during replay"),
            Err(error) => error,
        };
        assert!(
            replay_error.starts_with("HOST_ROUTE_REPLAY_FAILED:")
                && replay_error.contains("resolves outside project runtime storage"),
            "{replay_error}"
        );
        assert!(!external_root
            .join(".agent-collab/server/state.json")
            .exists());

        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
        std::fs::remove_dir_all(external_root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_storage_owner_rejects_alias_route_before_publication() {
        use std::os::unix::fs::symlink;

        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let central_apps = host_paths
            .app_storage_root(
                GlobalState::canonical_project_scope(&root).unwrap().as_str(),
                "existing-app",
            )
            .parent()
            .unwrap()
            .to_path_buf();
        let real_parent = central_apps.join("real-parent");
        let alias_parent = central_apps.join("alias-parent");
        std::fs::create_dir_all(&real_parent).unwrap();
        symlink(&real_parent, &alias_parent).unwrap();

        let real_storage_root = real_parent.join("future");
        let aliased_storage_root = alias_parent.join("future");
        let route_file = host_paths.state_root().join("routes.json");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        binding_store::publish_route_records(&route_file, &[]).unwrap().require_confirmed_route().unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        manager.routes.lock().unwrap().insert(
            ("existing-app".into(), "/existing-project".into()),
            RuntimeRoute {
                root: root.clone(),
                storage_root: real_storage_root,
                runtime: None,
            },
        );

        let context = context_with_app(&root, "dangling-alias-app");
        let before_routes = std::fs::read(&route_file).unwrap();
        let error = manager
            .append_route_record(&context, &aliased_storage_root)
            .unwrap_err();

        assert!(
            error.starts_with("HOST_ROUTE_DURABILITY_FAILED:")
                && error.contains("already owned by route (existing-app, /existing-project)"),
            "{error}"
        );
        assert_eq!(
            std::fs::read(&route_file).unwrap(),
            before_routes,
            "an alias collision must be rejected before current route publication"
        );
        assert!(!manager.routes.lock().unwrap().contains_key(&(
            "dangling-alias-app".into(),
            GlobalState::canonical_project_scope(&root)
                .unwrap()
                .as_str()
                .into(),
        )));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn storage_owner_permission_error_rejects_register_without_mutation() {
        use std::os::unix::fs::PermissionsExt;

        let (server, root, _) = test_server();
        let blocked_parent = root.with_file_name(format!(
            "{}-blocked-owner",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&blocked_parent).unwrap();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let external_root = root.with_file_name(format!(
            "{}-permission-external",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(external_root.join(".agent-collab/server")).unwrap();
        let route_file = host_paths.state_root().join("routes.json");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        binding_store::publish_route_records(&route_file, &[]).unwrap().require_confirmed_route().unwrap();
        let before_routes = std::fs::read(&route_file).unwrap();

        // The current owner is a real fixed route, not the retired host reducer.
        let mut records = load_host_route_records(&route_file).unwrap();
        records.push(HostRouteRecord {
            app_scope_id: "blocked-owner".into(),
            project_scope: GlobalState::canonical_project_scope(&root).unwrap().as_str().into(),
            canonical_root: root.canonicalize().unwrap().display().to_string(),
            storage_root: blocked_parent.join("future-storage").display().to_string(),
            registered_ms: now_ms(),
        });
        binding_store::publish_route_records(&route_file, &records).unwrap().require_confirmed_route().unwrap();
        let before_blocked_routes = std::fs::read(&route_file).unwrap();
        let before_map = manager.binding_control_store().load().unwrap();
        let central = host_paths.app_storage_root(context_with_app(&external_root, "permission-external-app").project_scope.as_str(), "permission-external-app");

        std::fs::set_permissions(&blocked_parent, std::fs::Permissions::from_mode(0o000)).unwrap();
        let context = context_with_app(&external_root, "permission-external-app");
        let (_runtime, response) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register("permission-external-worker".into(),
                 "token-permission-external".into(),
                 external_root.display().to_string(),
                 None),
        );
        std::fs::set_permissions(&blocked_parent, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(!response.ok, "{response:?}");
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("HOST_ROUTE_DURABILITY_FAILED:")
                    && error.contains("Permission denied")),
            "{response:?}"
        );
        assert_eq!(std::fs::read(&route_file).unwrap(), before_blocked_routes);
        assert_eq!(manager.binding_control_store().load().unwrap(), before_map);
        assert!(!central.exists(), "permission rejection must not create a namespace");
        assert!(!manager
            .routes
            .lock()
            .unwrap()
            .contains_key(&ProjectRuntimeManager::route_key(&context)));
        assert!(!external_root
            .join(".agent-collab/server/state.json")
            .exists());

        // Remove only this fixture's denied owner and prove a legal route is usable.
        std::fs::write(&route_file, before_routes).unwrap();
        let runtime = committed_native_scope_fixture(&manager, &context,
            "permission-positive", "token-positive", "thread-permission-positive");
        assert!(runtime.state.lock().unwrap().workers.contains_key("permission-positive"));

        drop(manager);
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(external_root).unwrap();
        std::fs::remove_dir_all(blocked_parent).unwrap();
    }

    #[tokio::test]
    async fn first_register_uses_the_explicit_context_app_scope() {
        let (server, root, state_path) = test_server();
        let project_context = context_with_app(&root, "app-wire");
        let response = register_wire_fixture(
            server.clone(),
            Some(project_context),
            Req::register("wire-worker".into(),
                 "token-wire-worker".into(),
                 root.display().to_string(),
                 test_candidates("thread-wire-worker")),
        ).await;
        assert!(response.ok, "{response:?}");
        let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
        super::peer_tests::promote_master(&server, "wire-worker", "fixture grants Jenny through its name owner");
        let grant = {
            let state = server.state.lock().unwrap();
            let project = state.global.lookup_project(&project_scope).unwrap();
            let binding = project
                .lookup_binding(&BindingId::new("binding-wire-worker").unwrap())
                .unwrap();
            project.lookup_master_grant(&binding.binding_id).unwrap().clone()
        };
        let runtime = runtime_for_registered(&server, &root, "wire-worker", "app-wire");
        let before_state = std::fs::read(&state_path).unwrap();
        let before_revision = server.state.lock().unwrap().revision;
        let repeated = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, "app-wire", &runtime)),
            Req::register("wire-worker".into(),
                 "token-wire-worker".into(),
                 root.display().to_string(),
                 test_candidates_for_registered(
                    &server,
                    &root,
                    "wire-worker",
                    "app-wire",
                )),
        )
        .await;
        assert!(repeated.ok, "{repeated:?}");
        assert_eq!(repeated.data["replayed"], true);
        let state = server.state.lock().unwrap();
        let project = state.global.lookup_project(&project_scope).unwrap();
        assert!(project
            .lookup_registration(&AppServerId::new("app-wire").unwrap())
            .is_some());
        assert!(project
            .lookup_registration(&AppServerId::new("tui-default").unwrap())
            .is_none());
        let binding = project
            .lookup_binding(&BindingId::new("binding-wire-worker").unwrap())
            .unwrap();
        assert_eq!(binding.agent_id, grant.agent_id);
        assert_eq!(
            project.lookup_master_grant(&binding.binding_id),
            Some(&grant)
        );
        assert_eq!(state.revision, before_revision);
        drop(state);
        assert_eq!(std::fs::read(&state_path).unwrap(), before_state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn first_register_accepts_the_explicit_appserver_runtime() {
        let (server, root, state_path) = test_server();
        let app = "app-wire";
        let worker_id = "wire-runtime-worker";
        let runtime = RuntimeIdentity {
            agent_id: AgentId::new(worker_id).unwrap(),
            runtime_id: RuntimeId::new("runtime-wire-runtime-worker").unwrap(),
            appserver_id: AppServerId::new(app).unwrap(),
            binding_id: BindingId::new("binding-wire-runtime-worker").unwrap(),
            session_id: Some(
                crate::identity::SessionId::new("session-thread-wire-runtime-worker").unwrap(),
            ),
            native_thread_id: Some(NativeThreadId::new("thread-wire-runtime-worker").unwrap()),
        };

        let response = register_wire_fixture(
            server.clone(),
            Some(context_with_runtime(&root, app, &runtime)),
            Req::register(worker_id.into(),
                 "token-wire-runtime-worker".into(),
                 root.display().to_string(),
                 test_candidates("thread-wire-runtime-worker")),
        ).await;
        assert!(response.ok, "{response:?}");
        let registered = runtime_for_registered(&server, &root, worker_id, app);
        assert_eq!(registered.agent_id, runtime.agent_id);
        assert_eq!(registered.appserver_id, runtime.appserver_id);
        assert_eq!(registered.binding_id, runtime.binding_id);
        assert_eq!(registered.native_thread_id, runtime.native_thread_id);
        assert_eq!(registered.session_id, runtime.session_id);
        assert!(state_path.is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delayed_second_app_register_is_rejected_without_resident_mutation() {
        let (server, root, state_path) = test_server();
        let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
        let first_context = context_with_app(&root, "app-a");
        let second_context = context_with_app(&root, "app-b");
        let first_request = Req::register("worker-a".into(),
             "token-worker-a".into(),
             root.display().to_string(),
             test_candidates("thread-worker-a"));
        let second_request = Req::register("worker-b".into(),
             "token-worker-b".into(),
             root.display().to_string(),
             test_candidates("thread-worker-b"));

        // Both wire requests can pass host admission before either handler
        // reaches the typed commit boundary.
        assert!(validate_request_context(&server, &first_request, Some(&first_context)).is_ok());
        assert!(validate_request_context(&server, &second_request, Some(&second_context)).is_ok());

        let first = server
            .typed_register_envelope_for_scope(
                "worker-a",
                "token-worker-a",
                &SelectedTransport {
                    kind: TransportKind::AppServer,
                    endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
                    namespace: Some("codex_tui".into()),
                    session_id: Some("session-thread-worker-a".into()),
                    thread_id: Some("thread-worker-a".into()),
                    tmux_endpoint: None,
                    rex_endpoint: None,
                    capabilities: vec!["send_message_to_thread".into()],
                    self_check: "test appserver".into(),
                },
                project_scope.clone(),
                &root.display().to_string(),
                AppServerId::new("app-a").unwrap(),
                false,
            )
            .unwrap();
        server.typed_dispatch(first).unwrap();

        let before_second_state_file = std::fs::read(&state_path).unwrap();
        let before_second_state = {
            let state = server.state.lock().unwrap();
            (
                state.revision,
                state.sequence,
                state.workers.clone(),
                state.global.clone(),
            )
        };

        // The delayed handler builds its envelope after app-a committed, so
        // its expected revision is current and a revision-only CAS cannot
        // detect the stale route admission.
        let second = server
            .typed_register_envelope_for_scope(
                "worker-b",
                "token-worker-b",
                &SelectedTransport {
                    kind: TransportKind::AppServer,
                    endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
                    namespace: Some("codex_tui".into()),
                    session_id: Some("session-thread-worker-b".into()),
                    thread_id: Some("thread-worker-b".into()),
                    tmux_endpoint: None,
                    rex_endpoint: None,
                    capabilities: vec!["send_message_to_thread".into()],
                    self_check: "test appserver".into(),
                },
                project_scope.clone(),
                &root.display().to_string(),
                AppServerId::new("app-b").unwrap(),
                false,
            )
            .unwrap();
        let error = server.typed_dispatch(second).unwrap_err().to_string();
        assert!(
            error.starts_with("invalid command: PROJECT_ROUTE_NOT_READY/UNSUPPORTED:"),
            "{error}"
        );

        let state = server.state.lock().unwrap();
        assert_eq!(state.revision, before_second_state.0);
        assert_eq!(state.sequence, before_second_state.1);
        assert_eq!(state.workers, before_second_state.2);
        assert_eq!(state.global, before_second_state.3);
        drop(state);
        assert_eq!(std::fs::read(&state_path).unwrap(), before_second_state_file);
        assert!(validate_request_context(&server, &Req::StatusAll, Some(&first_context)).is_ok());
        assert!(!validate_request_context(&server, &Req::StatusAll, Some(&second_context)).is_ok());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn subagent_wire_registration_keeps_parent_app_scope() {
        let (server, root, _) = test_server();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
        let context = ProjectContext::for_registered_root_with_app(&root, app.clone()).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
        let runtime = committed_native_scope_fixture(&manager, &context,
            "parent", "token-parent", "thread-parent");
        register_native_peer(&runtime, &root, "child", "token-child", "thread-child", app.as_str());
        let child_route = manager.resolve_route_by_native_thread("session-thread-child", "thread-child").unwrap();
        assert_eq!(child_route.agent_id.as_str(), "child");
        assert_eq!(child_route.app_scope_id, app.clone());

        let (_, listed) = dispatch_wire_routed(
            manager.clone(),
            Some(context.clone()),
            Req::Subagent {
                worker_id: "parent".into(),
                token: "token-parent".into(),
                command: crate::subagent::Action::List,
                launch_env: Default::default(),
            }, tokio::sync::watch::channel(false).1,
        )
        .await;
        assert!(listed.ok, "{listed:?}");

        let before_map = manager.binding_control_store().load().unwrap();
        let before_state = std::fs::read(&runtime.state_path).unwrap();
        let (_, forged) = dispatch_wire_routed(
            manager.clone(),
            Some(context_with_app(&root, "other-app")),
            Req::Subagent {
                worker_id: "parent".into(),
                token: "token-parent".into(),
                command: crate::subagent::Action::List,
                launch_env: Default::default(),
            }, tokio::sync::watch::channel(false).1,
        )
        .await;
        assert!(!forged.ok, "{forged:?}");
        assert_eq!(manager.binding_control_store().load().unwrap(), before_map);
        assert_eq!(std::fs::read(&runtime.state_path).unwrap(), before_state);
        assert!(forged
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("PROJECT_SCOPE_UNKNOWN:")));

        let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
        let state = runtime.state.lock().unwrap();
        let project = state.global.lookup_project(&project_scope).unwrap();
        assert!(project.lookup_registration(&app).is_some());
        assert!(project
            .lookup_registration(&AppServerId::new("tui-default").unwrap())
            .is_none());
        assert!(project
            .runtime_bindings
            .values()
            .filter(|binding| binding.agent_id.as_str() == "child")
            .all(|binding| binding.app_scope_id == app));
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
