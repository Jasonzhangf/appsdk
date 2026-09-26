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
    async fn not_ready_route_does_not_mutate_state_or_primary_journal() {
        let (server, root, journal_path) = test_server();
        let other_root = root.with_file_name(format!(
            "{}-other",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&other_root).unwrap();
        register_known_project(&server, &other_root);
        let before_journal = std::fs::read(&journal_path).unwrap();
        let before_revision = server.state.lock().unwrap().revision;
        let response =
            dispatch_wire(server.clone(), Some(context(&other_root)), Req::StatusAll).await;
        assert!(!response.ok);
        assert!(response
            .error
            .unwrap()
            .starts_with("PROJECT_ROUTE_NOT_READY/UNSUPPORTED:"));
        assert_eq!(std::fs::read(&journal_path).unwrap(), before_journal);
        assert_eq!(server.state.lock().unwrap().revision, before_revision);
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(other_root).unwrap();
    }

    #[tokio::test]
    async fn cross_project_wire_request_is_rejected_without_resident_mutation() {
        let (server, root, journal_path) = test_server();
        let project_context = context_with_app(&root, "app-wire");
        let registered = dispatch_wire(
            server.clone(),
            Some(project_context.clone()),
            Req::Register {
                worker_id: "target-master".into(),
                token: "token-target-master".into(),
                cwd: root.display().to_string(),
                candidates: test_candidates("thread-target-master"),
            },
        )
        .await;
        assert!(registered.ok, "{registered:?}");
        let promoted = handle_master_promote(
            &server,
            "target-master".into(),
            "token-target-master".into(),
            "user approved target-master".into(),
        );
        assert!(promoted.ok, "{promoted:?}");

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
        let before_journal = std::fs::read(&journal_path).unwrap();
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
                source_master_assigned_by: "source-master".into(),
                source_master_approval: Some("user approved source-master".into()),
                source_master_assigned_ms: 1,
                to: "target-master".into(),
                subject: "cross-project".into(),
                body: "must remain outside resident reducer".into(),
                in_reply_to: None,
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
        assert_eq!(std::fs::read(&journal_path).unwrap(), before_journal);
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
    fn persisted_resident_route_replays_and_admits_register() {
        let (mut server, root, _) = test_server();
        with_appserver_check(&mut server, |candidate| Ok(verified_appserver(candidate)));
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let canonical_root = root.canonicalize().unwrap();
        let record = HostRouteRecord {
            version: 1,
            op: "register".into(),
            app_scope_id: crate::identity::CLI_APP_SERVER_ID.into(),
            project_scope: canonical_root.to_string_lossy().into_owned(),
            canonical_root: canonical_root.to_string_lossy().into_owned(),
            storage_root: canonical_root.to_string_lossy().into_owned(),
            registered_ms: 1,
        };
        std::fs::write(
            &route_journal,
            format!("{}\n", serde_json::to_string(&record).unwrap()),
        )
        .unwrap();

        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let registration = server
            .state
            .lock()
            .unwrap()
            .global
            .lookup_registration(
                &GlobalState::canonical_project_scope(&root).unwrap(),
                &AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
            )
            .cloned()
            .expect("resident route replay must restore the project registration");
        assert_eq!(registration.registered_at_ms, 1);

        let worker_id = "resident-route-worker";
        let thread_id = "thread-resident-route-worker";
        let (_, response) = manager.dispatch_sync(
            Some(context_with_app(&root, crate::identity::CLI_APP_SERVER_ID)),
            Req::Register {
                worker_id: worker_id.into(),
                token: "token-resident-route-worker".into(),
                cwd: root.display().to_string(),
                candidates: test_candidates(thread_id),
            },
        );
        assert!(response.ok, "{response:?}");
        let resolved = manager
            .resolve_route_by_native_thread(&format!("session-{thread_id}"), thread_id)
            .unwrap();
        assert_eq!(resolved.agent_id.as_str(), worker_id);
        assert_eq!(
            resolved.project_scope.as_str(),
            canonical_root.to_string_lossy()
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn non_cli_resident_route_replays_with_host_runtime_and_restores_registration() {
        let (mut server, host_root, _) = test_server();
        with_appserver_check(&mut server, |candidate| Ok(verified_appserver(candidate)));
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app_scope = "resident-app";
        let context = context_with_app(&host_root, app_scope);

        let (resident_runtime, registered) = manager.dispatch_sync(
            Some(context.clone()),
            Req::Register {
                worker_id: "resident-replay-worker".into(),
                token: "token-resident-replay-worker".into(),
                cwd: host_root.display().to_string(),
                candidates: test_candidates("thread-resident-replay-worker"),
            },
        );
        assert!(registered.ok, "{registered:?}");
        assert!(Arc::ptr_eq(&resident_runtime, &server));
        let storage_root = resident_runtime.storage_root.clone();
        let records = load_host_route_records(&route_journal).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].app_scope_id, app_scope);
        assert_eq!(
            storage_owner_path(Path::new(&records[0].storage_root)).unwrap(),
            storage_owner_path(&storage_root).unwrap()
        );

        drop(resident_runtime);
        drop(manager);
        {
            let mut state = server.state.lock().unwrap();
            state.global.projects.clear();
        }

        let replayed_manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let replayed_runtime = replayed_manager.select_runtime(&context).unwrap();
        assert!(Arc::ptr_eq(&replayed_runtime, &server));
        assert_eq!(replayed_runtime.storage_root, storage_root);
        assert!(server
            .state
            .lock()
            .unwrap()
            .global
            .lookup_registration(
                &GlobalState::canonical_project_scope(&host_root).unwrap(),
                &AppServerId::new(app_scope).unwrap(),
            )
            .is_some());

        let worker_id = "resident-replay-after-restart";
        let thread_id = "thread-resident-replay-after-restart";
        let (selected, response) = replayed_manager.dispatch_sync(
            Some(context),
            Req::Register {
                worker_id: worker_id.into(),
                token: "token-resident-replay-after-restart".into(),
                cwd: host_root.display().to_string(),
                candidates: test_candidates(thread_id),
            },
        );
        assert!(response.ok, "{response:?}");
        assert!(Arc::ptr_eq(&selected, &server));
        let resolved = replayed_manager
            .resolve_route_by_native_thread(&format!("session-{thread_id}"), thread_id)
            .unwrap();
        assert_eq!(resolved.agent_id.as_str(), worker_id);
        assert_eq!(resolved.app_scope_id.as_str(), app_scope);

        std::fs::remove_dir_all(host_root).unwrap();
    }

    #[test]
    fn resident_route_publish_failure_does_not_leave_a_replayable_route() {
        let (mut server, root, _) = test_server();
        with_appserver_check(&mut server, |candidate| Ok(verified_appserver(candidate)));
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        let reducer_journal = root.join(".agent-collab/server/journal.jsonl");
        let reducer_journal_before = std::fs::read(&reducer_journal).unwrap();
        let mut manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let route_journal_blocker = root.join("route-journal-blocker");
        std::fs::create_dir_all(&route_journal_blocker).unwrap();
        Arc::get_mut(&mut manager).unwrap().route_journal = route_journal_blocker;
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
        let (_, failed) = manager.dispatch_sync(
            Some(context),
            Req::Register {
                worker_id: "resident-route-failure-worker".into(),
                token: "token-resident-route-failure-worker".into(),
                cwd: root.display().to_string(),
                candidates: test_candidates("thread-resident-route-failure-worker"),
            },
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
            !route_journal.exists(),
            "failed publish wrote a route journal"
        );
        assert_eq!(
            std::fs::read(&reducer_journal).unwrap(),
            reducer_journal_before,
            "failed route publication mutated the reducer journal"
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
    fn malformed_resident_route_storage_is_rejected_during_replay() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let canonical_root = root.canonicalize().unwrap();
        let record = HostRouteRecord {
            version: 1,
            op: "register".into(),
            app_scope_id: crate::identity::CLI_APP_SERVER_ID.into(),
            project_scope: canonical_root.to_string_lossy().into_owned(),
            canonical_root: canonical_root.to_string_lossy().into_owned(),
            storage_root: app_scope_storage_path(&canonical_root, "app-a")
                .to_string_lossy()
                .into_owned(),
            registered_ms: 1,
        };
        std::fs::write(
            &route_journal,
            format!("{}\n", serde_json::to_string(&record).unwrap()),
        )
        .unwrap();

        let error = match ProjectRuntimeManager::new(server, &host_paths) {
            Ok(_) => panic!("malformed resident route was replayed"),
            Err(error) => error,
        };
        assert!(
            error.contains("resident route storage root"),
            "unexpected replay error: {error}"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_project_secondary_app_is_not_ready_without_a_second_reducer() {
        let (server, root, _) = test_server();
        register_known_project_with_app(&server, &root, "app-a");
        assert!(
            handle_register_with_app_scope(
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
    fn legacy_multiple_app_routes_fail_replay_before_opening_a_second_reducer() {
        let (server, root, journal_path) = test_server();
        register_known_project_with_app(&server, &root, "app-a");
        assert!(
            handle_register_with_app_scope(
                &server,
                "legacy-resident".into(),
                "token-legacy-resident".into(),
                root.display().to_string(),
                Some(AppServerId::new("app-a").unwrap()),
                test_candidates("thread-legacy-resident"),
            )
            .ok
        );
        register_known_project_with_app(&server, &root, "app-b");

        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        std::fs::write(&route_journal, b"").unwrap();
        let before_route_journal = std::fs::read(&route_journal).unwrap();
        let before_resident_journal = std::fs::read(&journal_path).unwrap();

        let error = match ProjectRuntimeManager::new(server.clone(), &host_paths) {
            Ok(_) => panic!("legacy pending route must not open a second reducer"),
            Err(error) => error,
        };
        assert!(error.starts_with("HOST_ROUTE_REPLAY_FAILED:"), "{error}");
        assert!(error.contains("resident host"), "{error}");
        assert_eq!(std::fs::read(&route_journal).unwrap(), before_route_journal);
        assert_eq!(
            std::fs::read(&journal_path).unwrap(),
            before_resident_journal
        );
        assert!(!root.join(".agent-collab/server/runtimes").exists());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_route_root_is_ignored_without_rewriting_route_journal() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let missing_root = root.join("missing-project");
        let record = HostRouteRecord {
            version: 1,
            op: "register".into(),
            app_scope_id: "appserver-cli".into(),
            project_scope: missing_root.to_string_lossy().into_owned(),
            canonical_root: missing_root.to_string_lossy().into_owned(),
            storage_root: missing_root.to_string_lossy().into_owned(),
            registered_ms: 1,
        };
        let contents = format!("{}\n", serde_json::to_string(&record).unwrap());
        std::fs::write(&route_journal, &contents).unwrap();

        let manager = ProjectRuntimeManager::new(server, &host_paths).unwrap();

        assert!(manager.routes.lock().unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&route_journal).unwrap(), contents);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resident_route_replays_from_host_journal() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let canonical_root = root.canonicalize().unwrap();
        let record = HostRouteRecord {
            version: 1,
            op: "register".into(),
            app_scope_id: "appserver-cli".into(),
            project_scope: canonical_root.to_string_lossy().into_owned(),
            canonical_root: canonical_root.to_string_lossy().into_owned(),
            storage_root: canonical_root.to_string_lossy().into_owned(),
            registered_ms: 1,
        };
        std::fs::write(
            &route_journal,
            format!("{}\n", serde_json::to_string(&record).unwrap()),
        )
        .unwrap();

        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let routes = manager.routes.lock().unwrap();
        let route = routes
            .get(&(
                "appserver-cli".into(),
                canonical_root.to_string_lossy().into_owned(),
            ))
            .unwrap();
        assert!(route
            .runtime
            .as_ref()
            .is_some_and(|runtime| Arc::ptr_eq(runtime, &server)));
        assert_eq!(route.storage_root, canonical_root);
        assert_eq!(
            std::fs::read_to_string(&route_journal).unwrap(),
            format!("{}\n", serde_json::to_string(&record).unwrap())
        );
        assert_eq!(
            server
                .state
                .lock()
                .unwrap()
                .global
                .lookup_registration(
                    &GlobalState::canonical_project_scope(&root).unwrap(),
                    &AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap()
                )
                .map(|registration| registration.project_scope.as_str().to_owned()),
            Some(canonical_root.to_string_lossy().into_owned())
        );
        assert_eq!(
            server
                .state
                .lock()
                .unwrap()
                .global
                .lookup_registration(
                    &GlobalState::canonical_project_scope(&root).unwrap(),
                    &AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap()
                )
                .map(|registration| registration.registered_at_ms),
            Some(1)
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resident_route_repairs_from_host_registration_after_crash() {
        let (mut server, root, _) = test_server();
        with_appserver_check(&mut server, |candidate| Ok(verified_appserver(candidate)));
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        let registered = handle_register_with_app_scope_unfinalized(
            &server,
            "resident-repair-worker".into(),
            "token-resident-repair-worker".into(),
            root.display().to_string(),
            Some(AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap()),
            test_candidates("thread-resident-repair-worker"),
        );
        assert!(registered.ok, "{registered:?}");
        assert!(
            !route_journal.exists(),
            "unfinalized registration unexpectedly published a route"
        );

        let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
        let registration = server
            .state
            .lock()
            .unwrap()
            .global
            .lookup_registration(
                &project_scope,
                &AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
            )
            .cloned()
            .unwrap();

        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let records = load_host_route_records(&route_journal).unwrap();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].registered_ms, registration.registered_at_ms);
        assert_eq!(
            records[0].storage_root,
            root.canonicalize().unwrap().to_string_lossy()
        );
        assert!(manager
            .routes
            .lock()
            .unwrap()
            .get(&(
                crate::identity::CLI_APP_SERVER_ID.into(),
                root.canonicalize().unwrap().to_string_lossy().into_owned(),
            ))
            .and_then(|route| route.runtime.as_ref())
            .is_some_and(|runtime| Arc::ptr_eq(runtime, &server)));
        assert!(server
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &RouteScope {
                    app_scope_id: AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
                    project_scope_id: project_scope.clone(),
                },
                &BindingId::new("binding-resident-repair-worker").unwrap(),
            )
            .is_some());

        let retried = manager.dispatch_sync(
            Some(context_with_runtime(
                &root,
                crate::identity::CLI_APP_SERVER_ID,
                &RuntimeIdentity::cli_adapter("resident-repair-worker").unwrap(),
            )),
            Req::Register {
                worker_id: "resident-repair-worker".into(),
                token: "token-resident-repair-worker".into(),
                cwd: root.display().to_string(),
                candidates: test_candidates("thread-resident-repair-worker"),
            },
        );
        assert!(retried.1.ok, "{:?}", retried.1);
        commit_current_thread_route_for_runtime(
            &server,
            &server,
            "resident-repair-worker",
            &root.display().to_string(),
            Some(&AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap()),
        )
        .unwrap();
        assert_eq!(
            manager
                .resolve_route_by_native_thread(
                    "session-thread-resident-repair-worker",
                    "thread-resident-repair-worker",
                )
                .unwrap()
                .agent_id
                .as_str(),
            "resident-repair-worker"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resident_route_repair_failure_blocks_startup() {
        let (server, root, _) = test_server();
        register_known_project_with_app(&server, &root, crate::identity::CLI_APP_SERVER_ID);
        let blocker = root.join("route-journal-state-root");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let blocked_host_paths = HostPaths::for_state_root(blocker).unwrap();

        let error = match ProjectRuntimeManager::new(server, &blocked_host_paths) {
            Ok(_) => panic!("route repair failure must block daemon startup"),
            Err(error) => error,
        };
        assert!(error.starts_with("HOST_ROUTE_REPLAY_FAILED:"), "{error}");
        assert!(error.contains("route journal"), "{error}");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn uninitialized_route_root_is_ignored_without_rewriting_route_journal() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let uninitialized_root = root.join("uninitialized-project");
        std::fs::create_dir_all(&uninitialized_root).unwrap();
        let canonical_root = uninitialized_root.canonicalize().unwrap();
        let record = HostRouteRecord {
            version: 1,
            op: "register".into(),
            app_scope_id: "appserver-cli".into(),
            project_scope: canonical_root.to_string_lossy().into_owned(),
            canonical_root: canonical_root.to_string_lossy().into_owned(),
            storage_root: canonical_root.to_string_lossy().into_owned(),
            registered_ms: 1,
        };
        let contents = format!("{}\n", serde_json::to_string(&record).unwrap());
        std::fs::write(&route_journal, &contents).unwrap();

        let manager = ProjectRuntimeManager::new(server, &host_paths).unwrap();

        assert!(manager.routes.lock().unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&route_journal).unwrap(), contents);

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

        let real_future_path = real_parent.join("future").join("journal");
        let aliased_future_path = alias_parent.join("future").join("journal");
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

        let target_path = missing_parent.join("future").join("journal");
        let dangling_path = dangling_parent.join("future").join("journal");
        assert_eq!(
            storage_owner_path(&target_path).unwrap(),
            storage_owner_path(&dangling_path).unwrap()
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_storage_symlink_escape_rejects_register_before_journal_write() {
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
        let canonical_project_root = project_root.canonicalize().unwrap();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let seed_context = context_with_app(&project_root, "runtime-seed");
        let (_, seed_response) = manager.dispatch_sync(
            Some(seed_context),
            Req::Register {
                worker_id: "runtime-seed-worker".into(),
                token: "token-runtime-seed-worker".into(),
                cwd: project_root.display().to_string(),
                candidates: test_candidates("thread-runtime-seed-worker"),
            },
        );
        assert!(seed_response.ok, "{seed_response:?}");

        let escape_context = context_with_app(&project_root, "runtime-escape");
        let escape_storage_root = app_scope_storage_path(&canonical_project_root, "runtime-escape");
        symlink(&external_root, &escape_storage_root).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        let before_route_journal = std::fs::read(&route_journal).unwrap();
        let (_, escape_response) = manager.dispatch_sync(
            Some(escape_context.clone()),
            Req::Register {
                worker_id: "runtime-escape-worker".into(),
                token: "token-runtime-escape-worker".into(),
                cwd: project_root.display().to_string(),
                candidates: test_candidates("thread-runtime-escape-worker"),
            },
        );

        assert!(!escape_response.ok, "{escape_response:?}");
        assert!(
            escape_response.error.as_deref().is_some_and(|error| {
                error.starts_with("HOST_ROUTE_DURABILITY_FAILED:")
                    && error.contains("resolves outside project runtime storage")
            }),
            "{escape_response:?}"
        );
        assert_eq!(std::fs::read(&route_journal).unwrap(), before_route_journal);
        assert!(!manager
            .routes
            .lock()
            .unwrap()
            .contains_key(&ProjectRuntimeManager::route_key(&escape_context)));
        assert!(!external_root
            .join(".agent-collab/server/journal.jsonl")
            .exists());

        let build_error = match manager.build_runtime(&project_root, &escape_storage_root) {
            Ok(_) => panic!("runtime storage symlink escape must fail closed"),
            Err(error) => error,
        };
        assert!(
            build_error.starts_with("PROJECT_ROUTE_NOT_READY/UNSUPPORTED:")
                && build_error.contains("resolves outside project runtime storage"),
            "{build_error}"
        );
        assert!(!external_root
            .join(".agent-collab/server/journal.jsonl")
            .exists());

        drop(manager);
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
        std::fs::remove_dir_all(external_root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn runtime_storage_symlink_escape_rejects_replay_before_runtime_creation() {
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
        let storage_root = app_scope_storage_path(&canonical_project_root, "replay-escape");
        symlink(&external_root, &storage_root).unwrap();
        let project_scope = GlobalState::canonical_project_scope(&canonical_project_root).unwrap();
        let record = HostRouteRecord {
            version: 1,
            op: "register".into(),
            app_scope_id: "replay-escape".into(),
            project_scope: project_scope.as_str().into(),
            canonical_root: project_scope.as_str().into(),
            storage_root: storage_root.to_string_lossy().into_owned(),
            registered_ms: 1,
        };
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        std::fs::write(
            &route_journal,
            format!("{}\n", serde_json::to_string(&record).unwrap()),
        )
        .unwrap();

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
            .join(".agent-collab/server/journal.jsonl")
            .exists());

        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
        std::fs::remove_dir_all(external_root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_storage_owner_rejects_alias_route_before_journal_write() {
        use std::os::unix::fs::symlink;

        let (server, root, _) = test_server();
        let runtimes = root.join(".agent-collab/server/runtimes");
        let real_parent = runtimes.join("real-parent");
        let alias_parent = runtimes.join("alias-parent");
        std::fs::create_dir_all(&real_parent).unwrap();
        symlink(&real_parent, &alias_parent).unwrap();

        let real_storage_root = real_parent.join("future");
        let aliased_storage_root = alias_parent.join("future");
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        std::fs::write(&route_journal, b"").unwrap();
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
        let before_route_journal = std::fs::read(&route_journal).unwrap();
        let error = manager
            .append_route_record(&context, &aliased_storage_root)
            .unwrap_err();

        assert!(
            error.starts_with("HOST_ROUTE_DURABILITY_FAILED:")
                && error.contains("already owned by route (existing-app, /existing-project)"),
            "{error}"
        );
        assert_eq!(
            std::fs::read(&route_journal).unwrap(),
            before_route_journal,
            "an alias collision must be rejected before route journal publication"
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

        let (mut server, root, _) = test_server();
        let blocked_parent = root.with_file_name(format!(
            "{}-blocked-owner",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&blocked_parent).unwrap();
        let blocked_storage = blocked_parent.join("future-storage");
        {
            let host = Arc::get_mut(&mut server).unwrap();
            host.storage_root = blocked_storage;
        }
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let external_root = root.with_file_name(format!(
            "{}-permission-external",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(external_root.join(".agent-collab/server")).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        std::fs::write(&route_journal, b"").unwrap();
        let before_route_journal = std::fs::read(&route_journal).unwrap();

        std::fs::set_permissions(&blocked_parent, std::fs::Permissions::from_mode(0o000)).unwrap();
        let context = context_with_app(&external_root, "permission-external-app");
        let (_runtime, response) = manager.dispatch_sync(
            Some(context.clone()),
            Req::Register {
                worker_id: "permission-external-worker".into(),
                token: "token-permission-external".into(),
                cwd: external_root.display().to_string(),
                candidates: test_candidates("thread-permission-external-worker"),
            },
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
        assert_eq!(std::fs::read(&route_journal).unwrap(), before_route_journal);
        assert!(!manager
            .routes
            .lock()
            .unwrap()
            .contains_key(&ProjectRuntimeManager::route_key(&context)));
        assert!(!external_root
            .join(".agent-collab/server/journal.jsonl")
            .exists());

        drop(manager);
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(external_root).unwrap();
        std::fs::remove_dir_all(blocked_parent).unwrap();
    }

    #[tokio::test]
    async fn first_register_uses_the_explicit_context_app_scope() {
        let (server, root, journal_path) = test_server();
        let project_context = context_with_app(&root, "app-wire");
        let response = dispatch_wire(
            server.clone(),
            Some(project_context),
            Req::Register {
                worker_id: "wire-worker".into(),
                token: "token-wire-worker".into(),
                cwd: root.display().to_string(),
                candidates: test_candidates("thread-wire-worker"),
            },
        )
        .await;
        assert!(response.ok, "{response:?}");
        let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
        let grant = {
            let mut state = server.state.lock().unwrap();
            let project = state.global.lookup_project(&project_scope).unwrap();
            let binding = project
                .lookup_binding(&BindingId::new("binding-wire-worker").unwrap())
                .unwrap();
            let grant = crate::server::global_state::MasterGrant::new(
                project_scope.clone(),
                AppServerId::new("app-wire").unwrap(),
                AgentId::new("wire-worker").unwrap(),
                "route-scoped",
                "operator",
                "approved",
                binding.binding_id.clone(),
                binding.endpoint_generation,
                now_ms(),
            )
            .unwrap();
            state.global.grant_master(grant.clone()).unwrap();
            grant
        };
        let runtime = runtime_for_registered(&server, &root, "wire-worker", "app-wire");
        let before_journal = std::fs::read(&journal_path).unwrap();
        let before_revision = server.state.lock().unwrap().revision;
        let repeated = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, "app-wire", &runtime)),
            Req::Register {
                worker_id: "wire-worker".into(),
                token: "token-wire-worker".into(),
                cwd: root.display().to_string(),
                candidates: test_candidates_for_registered(
                    &server,
                    &root,
                    "wire-worker",
                    "app-wire",
                ),
            },
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
        assert_eq!(binding.endpoint_generation, grant.endpoint_generation);
        assert_eq!(
            project.lookup_master_grant(&binding.binding_id),
            Some(&grant)
        );
        assert_eq!(state.revision, before_revision);
        drop(state);
        assert_eq!(std::fs::read(&journal_path).unwrap(), before_journal);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn first_register_accepts_the_explicit_appserver_runtime() {
        let (server, root, journal_path) = test_server();
        let app = "app-wire";
        let worker_id = "wire-runtime-worker";
        let runtime = RuntimeIdentity {
            agent_id: AgentId::new(worker_id).unwrap(),
            runtime_id: RuntimeId::new("runtime-wire-runtime-worker").unwrap(),
            appserver_id: AppServerId::new(app).unwrap(),
            endpoint_generation: 0,
            binding_id: BindingId::new("binding-wire-runtime-worker").unwrap(),
            session_id: Some(
                crate::identity::SessionId::new("session-thread-wire-runtime-worker").unwrap(),
            ),
            native_thread_id: Some(NativeThreadId::new("thread-wire-runtime-worker").unwrap()),
        };

        let response = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, app, &runtime)),
            Req::Register {
                worker_id: worker_id.into(),
                token: "token-wire-runtime-worker".into(),
                cwd: root.display().to_string(),
                candidates: test_candidates("thread-wire-runtime-worker"),
            },
        )
        .await;
        assert!(response.ok, "{response:?}");
        let registered = runtime_for_registered(&server, &root, worker_id, app);
        assert_eq!(registered.agent_id, runtime.agent_id);
        assert_eq!(registered.appserver_id, runtime.appserver_id);
        assert_eq!(registered.binding_id, runtime.binding_id);
        assert_eq!(registered.native_thread_id, runtime.native_thread_id);
        assert!(registered.endpoint_generation > 0);
        assert!(journal_path.is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delayed_second_app_register_is_rejected_without_resident_mutation() {
        let (server, root, journal_path) = test_server();
        let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
        let first_context = context_with_app(&root, "app-a");
        let second_context = context_with_app(&root, "app-b");
        let first_request = Req::Register {
            worker_id: "worker-a".into(),
            token: "token-worker-a".into(),
            cwd: root.display().to_string(),
            candidates: test_candidates("thread-worker-a"),
        };
        let second_request = Req::Register {
            worker_id: "worker-b".into(),
            token: "token-worker-b".into(),
            cwd: root.display().to_string(),
            candidates: test_candidates("thread-worker-b"),
        };

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

        let before_second_journal = std::fs::read(&journal_path).unwrap();
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
        assert_eq!(std::fs::read(&journal_path).unwrap(), before_second_journal);
        assert!(validate_request_context(&server, &Req::StatusAll, Some(&first_context)).is_ok());
        assert!(!validate_request_context(&server, &Req::StatusAll, Some(&second_context)).is_ok());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn subagent_wire_registration_keeps_parent_app_scope() {
        let (server, root, _) = test_server();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
        let context = ProjectContext::for_registered_root_with_app(&root, app.clone()).unwrap();
        let parent = dispatch_wire(
            server.clone(),
            Some(context.clone()),
            Req::Register {
                worker_id: "parent".into(),
                token: "token-parent".into(),
                cwd: root.display().to_string(),
                candidates: test_candidates("thread-parent"),
            },
        )
        .await;
        assert!(parent.ok, "{parent:?}");

        let child = handle_register_with_app_scope(
            &server,
            "child".into(),
            "token-child".into(),
            root.display().to_string(),
            Some(app.clone()),
            test_candidates("thread-child"),
        );
        assert!(child.ok, "{child:?}");
        let child_route = server
            .state
            .lock()
            .unwrap()
            .global
            .lookup_current_thread_route(
                &crate::identity::SessionId::new("session-thread-child").unwrap(),
                &NativeThreadId::new("thread-child").unwrap(),
            )
            .cloned()
            .expect("direct child registration must publish its current thread route");
        assert_eq!(child_route.agent_id.as_str(), "child");
        assert_eq!(child_route.app_scope_id, app.clone());

        let listed = dispatch_wire(
            server.clone(),
            Some(context),
            Req::Subagent {
                worker_id: "parent".into(),
                token: "token-parent".into(),
                command: crate::subagent::Action::List,
                launch_env: Default::default(),
            },
        )
        .await;
        assert!(listed.ok, "{listed:?}");

        let forged = dispatch_wire(
            server.clone(),
            Some(context_with_app(&root, "other-app")),
            Req::Subagent {
                worker_id: "parent".into(),
                token: "token-parent".into(),
                command: crate::subagent::Action::List,
                launch_env: Default::default(),
            },
        )
        .await;
        assert!(!forged.ok, "{forged:?}");
        assert!(forged
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("PROJECT_SCOPE_UNKNOWN:")));

        let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
        let state = server.state.lock().unwrap();
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

    #[test]
    fn direct_registration_route_failure_reports_cleanup_failure() {
        let (mut server, root, _) = test_server();
        with_appserver_check(&mut server, |candidate| Ok(verified_appserver(candidate)));
        inject_current_thread_route_journal_fault(CurrentThreadRouteJournalFault::Sync);
        let registered = handle_register_with_app_scope(
            &server,
            "route-failure-worker".into(),
            "token-route-failure-worker".into(),
            root.display().to_string(),
            Some(AppServerId::new("route-failure-app").unwrap()),
            test_candidates("thread-route-failure-worker"),
        );
        assert!(!registered.ok, "{registered:?}");
        let error = registered.error.unwrap_or_default();
        assert!(
            error.contains("ROUTE_TRANSITION_DURABILITY_FAILED:")
                && error.contains("registration cleanup failed:"),
            "{error}"
        );

        let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
        let state = server.state.lock().unwrap();
        assert!(state.workers.contains_key("route-failure-worker"));
        let binding = state
            .global
            .lookup_binding_for(
                &RouteScope {
                    app_scope_id: AppServerId::new("route-failure-app").unwrap(),
                    project_scope_id: project_scope,
                },
                &BindingId::new("binding-route-failure-worker").unwrap(),
            )
            .cloned()
            .expect("failed registration keeps an explicitly retired binding");
        assert!(binding.native_thread_id.is_some());
        assert!(state
            .global
            .lookup_current_thread_route(
                &crate::identity::SessionId::new("session-thread-route-failure-worker").unwrap(),
                &NativeThreadId::new("thread-route-failure-worker").unwrap()
            )
            .is_none());
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
