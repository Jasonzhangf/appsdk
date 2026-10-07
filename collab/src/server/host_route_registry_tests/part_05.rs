    #[test]
    fn nonresident_child_route_is_committed_to_host_owner() {
        let (host, host_root, _) = test_server();
        let project_root = host_root.with_file_name(format!(
            "{}-child-route",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let context = context_with_app(&project_root, "child-route-app");
        let (runtime, parent) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register("child-route-parent".into(),
                 "token-child-route-parent".into(),
                 project_root.display().to_string(),
                 test_candidates("thread-child-route-parent")),
        );
        assert!(parent.ok, "{parent:?}");
        assert!(!Arc::ptr_eq(&runtime, &host));

        let child = handle_register_with_app_scope_unfinalized(
            &runtime,
            "child-route-child".into(),
            "token-child-route-child".into(),
            project_root.display().to_string(),
            Some(AppServerId::new("child-route-app").unwrap()),
            test_candidates("thread-child-route-child"),
        );
        assert!(child.ok, "{child:?}");
        commit_current_thread_route_for_runtime(
            &host,
            &runtime,
            "child-route-child",
            &project_root.display().to_string(),
            Some(&AppServerId::new("child-route-app").unwrap()),
        )
        .unwrap();

        let child_thread = NativeThreadId::new("thread-child-route-child").unwrap();
        let child_session =
            crate::identity::SessionId::new("session-thread-child-route-child").unwrap();
        assert!(host
            .state
            .lock()
            .unwrap()
            .global
            .lookup_current_thread_route(&child_session, &child_thread)
            .is_some());
        assert!(runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_current_thread_route(&child_session, &child_thread)
            .is_none());
        let route = manager.resolve_route_by_native_thread(
            "session-thread-child-route-child",
            "thread-child-route-child",
        );
        assert!(route.is_ok(), "{route:?}");
        retire_current_thread_route_after_launch_failure(
            &host,
            &runtime,
            "child-route-child",
            &project_root.display().to_string(),
            Some(&AppServerId::new("child-route-app").unwrap()),
        )
        .unwrap();
        assert!(host
            .state
            .lock()
            .unwrap()
            .global
            .lookup_current_thread_route(&child_session, &child_thread)
            .is_none());

        let failed_child = handle_register_with_app_scope_unfinalized(
            &runtime,
            "child-route-failed".into(),
            "token-child-route-failed".into(),
            project_root.display().to_string(),
            Some(AppServerId::new("child-route-app").unwrap()),
            test_candidates("thread-child-route-failed"),
        );
        assert!(failed_child.ok, "{failed_child:?}");
        inject_current_thread_route_journal_fault(CurrentThreadRouteJournalFault::Sync);
        let route_error = commit_current_thread_route_for_runtime(
            &host,
            &runtime,
            "child-route-failed",
            &project_root.display().to_string(),
            Some(&AppServerId::new("child-route-app").unwrap()),
        )
        .unwrap_err();
        assert!(route_error.starts_with("ROUTE_TRANSITION_DURABILITY_FAILED:"));
        retire_runtime_binding_after_route_failure(
            &runtime,
            "child-route-failed",
            &project_root.display().to_string(),
            Some(&AppServerId::new("child-route-app").unwrap()),
            "child-route-parent",
            "route publication failed",
        )
        .unwrap();
        let failed_binding = {
            let state = runtime.state.lock().unwrap();
            state
                .global
                .lookup_binding_for(
                    &RouteScope {
                        app_scope_id: AppServerId::new("child-route-app").unwrap(),
                        project_scope_id: GlobalState::canonical_project_scope(&project_root)
                            .unwrap(),
                    },
                    &BindingId::new("binding-child-route-failed").unwrap(),
                )
                .cloned()
                .unwrap()
        };
        assert!(failed_binding.native_thread_id.is_none());
        assert!(runtime
            .state
            .lock()
            .unwrap()
            .workers
            .get("child-route-failed")
            .is_none());
        assert!(host
            .state
            .lock()
            .unwrap()
            .global
            .lookup_current_thread_route(
                &crate::identity::SessionId::new("session-thread-child-route-failed").unwrap(),
                &NativeThreadId::new("thread-child-route-failed").unwrap(),
            )
            .is_none());

        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn wire_notification_mutation_requires_runtime_and_read_status_stays_compatible() {
        let (server, root, journal_path) = test_server();
        let app = "app-wire";
        let worker_id = "notification-worker";
        let token = "token-notification-worker";
        let registered = dispatch_wire(
            server.clone(),
            Some(context_with_app(&root, app)),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates(&format!("thread-{worker_id}"))),
        )
        .await;
        assert!(registered.ok, "{registered:?}");

        let mailbox_dir = root.join(".agent-collab/mailbox");
        let before = mutation_snapshot(&server);
        let before_journal = std::fs::read(&journal_path).unwrap();
        let before_mailbox = directory_snapshot(&mailbox_dir);
        let missing_runtime = dispatch_wire(
            server.clone(),
            Some(context_with_app(&root, app)),
            notification_subscribe_request(worker_id, token),
        )
        .await;
        assert!(!missing_runtime.ok, "{missing_runtime:?}");
        assert_eq!(
            missing_runtime.error.as_deref(),
            Some(
                "PROJECT_CONTEXT_REQUIRED: project-scoped mutation requires typed runtime context"
            )
        );
        let after = mutation_snapshot(&server);
        assert_eq!(after, before);
        assert_eq!(std::fs::read(&journal_path).unwrap(), before_journal);
        assert_eq!(directory_snapshot(&mailbox_dir), before_mailbox);

        let runtime = runtime_for_registered(&server, &root, worker_id, app);
        let accepted = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, app, &runtime)),
            notification_subscribe_request(worker_id, token),
        )
        .await;
        assert!(accepted.ok, "{accepted:?}");
        let subscription_id = accepted.data["subscription"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let status = dispatch_wire(
            server.clone(),
            Some(context_with_app(&root, app)),
            Req::NotificationStatus {
                worker_id: worker_id.into(),
                token: token.into(),
            },
        )
        .await;
        assert!(status.ok, "{status:?}");
        assert!(status.data["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|subscription| subscription["id"] == subscription_id));

        let cancelled = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, app, &runtime)),
            Req::NotificationUnsubscribe {
                worker_id: worker_id.into(),
                token: token.into(),
                subscription_id,
            },
        )
        .await;
        assert!(cancelled.ok, "{cancelled:?}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn wire_notification_mutation_rejects_stale_ambiguous_unbound_and_wrong_routes() {
        async fn register_worker(
            app: &str,
        ) -> (
            Arc<Server>,
            PathBuf,
            PathBuf,
            RuntimeIdentity,
            String,
            String,
        ) {
            let (server, root, journal_path) = test_server();
            let worker_id = "route-worker".to_owned();
            let token = "token-route-worker".to_owned();
            let response = dispatch_wire(
                server.clone(),
                Some(context_with_app(&root, app)),
                Req::register(worker_id.clone(),
                     token.clone(),
                     root.display().to_string(),
                     test_candidates(&format!("thread-{worker_id}"))),
            )
            .await;
            assert!(response.ok, "{response:?}");
            let runtime = runtime_for_registered(&server, &root, &worker_id, app);
            (server, root, journal_path, runtime, worker_id, token)
        }

        {
            let (server, root, journal_path, runtime, worker_id, token) =
                register_worker("app-stale").await;
            let mut stale = runtime.clone();
            stale.endpoint_generation = runtime.endpoint_generation.saturating_sub(1);
            let before = mutation_snapshot(&server);
            let journal = std::fs::read(&journal_path).unwrap();
            let mailbox = directory_snapshot(&root.join(".agent-collab/mailbox"));
            let rejected = dispatch_wire(
                server.clone(),
                Some(context_with_runtime(&root, "app-stale", &stale)),
                notification_subscribe_request(&worker_id, &token),
            )
            .await;
            assert!(!rejected.ok, "{rejected:?}");
            assert!(rejected
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("SESSION_THREAD_BINDING_MISMATCH:")));
            let after = mutation_snapshot(&server);
            assert_eq!(after, before);
            assert_eq!(std::fs::read(&journal_path).unwrap(), journal);
            assert_eq!(
                directory_snapshot(&root.join(".agent-collab/mailbox")),
                mailbox
            );

            server
                .state
                .lock()
                .unwrap()
                .global
                .projects
                .values_mut()
                .next()
                .unwrap()
                .runtime_bindings
                .get_mut(runtime.binding_id.as_str())
                .unwrap()
                .endpoint_generation = 0;
            let before = mutation_snapshot(&server);
            let journal = std::fs::read(&journal_path).unwrap();
            let mailbox = directory_snapshot(&root.join(".agent-collab/mailbox"));
            let rejected = dispatch_wire(
                server.clone(),
                Some(context_with_runtime(&root, "app-stale", &runtime)),
                notification_subscribe_request(&worker_id, &token),
            )
            .await;
            assert!(!rejected.ok, "{rejected:?}");
            assert!(rejected
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("RUNTIME_BINDING_REJECTED:")));
            let after = mutation_snapshot(&server);
            assert_eq!(after, before);
            assert_eq!(std::fs::read(&journal_path).unwrap(), journal);
            assert_eq!(
                directory_snapshot(&root.join(".agent-collab/mailbox")),
                mailbox
            );
            std::fs::remove_dir_all(root).unwrap();
        }

        {
            let (server, root, journal_path, runtime, worker_id, token) =
                register_worker("app-ambiguous").await;
            let scope = GlobalState::canonical_project_scope(&root).unwrap();
            let app = AppServerId::new("app-ambiguous").unwrap();
            let ambiguous = RuntimeBinding::new(
                scope.clone(),
                app,
                AgentId::new(worker_id.clone()).unwrap(),
                RuntimeId::new("runtime-ambiguous").unwrap(),
                BindingId::new("binding-ambiguous").unwrap(),
                runtime.endpoint_generation,
                None,
            )
            .unwrap();
            server
                .state
                .lock()
                .unwrap()
                .global
                .projects
                .get_mut(scope.as_str())
                .unwrap()
                .runtime_bindings
                .insert(ambiguous.binding_id.as_str().into(), ambiguous);
            let before = mutation_snapshot(&server);
            let journal = std::fs::read(&journal_path).unwrap();
            let mailbox = directory_snapshot(&root.join(".agent-collab/mailbox"));
            let rejected = dispatch_wire(
                server.clone(),
                Some(context_with_runtime(&root, "app-ambiguous", &runtime)),
                notification_subscribe_request(&worker_id, &token),
            )
            .await;
            assert!(!rejected.ok, "{rejected:?}");
            assert!(rejected
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("RUNTIME_BINDING_REJECTED:")));
            let after = mutation_snapshot(&server);
            assert_eq!(after, before);
            assert_eq!(std::fs::read(&journal_path).unwrap(), journal);
            assert_eq!(
                directory_snapshot(&root.join(".agent-collab/mailbox")),
                mailbox
            );
            std::fs::remove_dir_all(root).unwrap();
        }

        {
            let (server, root, journal_path, runtime, worker_id, token) =
                register_worker("app-unbound").await;
            let scope = GlobalState::canonical_project_scope(&root).unwrap();
            server
                .state
                .lock()
                .unwrap()
                .global
                .projects
                .get_mut(scope.as_str())
                .unwrap()
                .runtime_bindings
                .remove(runtime.binding_id.as_str());
            let before = mutation_snapshot(&server);
            let journal = std::fs::read(&journal_path).unwrap();
            let mailbox = directory_snapshot(&root.join(".agent-collab/mailbox"));
            let rejected = dispatch_wire(
                server.clone(),
                Some(context_with_runtime(&root, "app-unbound", &runtime)),
                notification_subscribe_request(&worker_id, &token),
            )
            .await;
            assert!(!rejected.ok, "{rejected:?}");
            assert!(rejected
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("PROJECT_ROUTE_NOT_READY/UNSUPPORTED:")));
            let after = mutation_snapshot(&server);
            assert_eq!(after, before);
            assert_eq!(std::fs::read(&journal_path).unwrap(), journal);
            assert_eq!(
                directory_snapshot(&root.join(".agent-collab/mailbox")),
                mailbox
            );
            std::fs::remove_dir_all(root).unwrap();
        }

        {
            let (server, root, journal_path, runtime, worker_id, token) =
                register_worker("app-route").await;
            let mut wrong_app_runtime = runtime.clone();
            wrong_app_runtime.appserver_id = AppServerId::new("app-other").unwrap();
            let wrong_root = root.with_file_name(format!(
                "{}-other",
                root.file_name().unwrap().to_string_lossy()
            ));
            std::fs::create_dir_all(&wrong_root).unwrap();
            let before = mutation_snapshot(&server);
            let journal = std::fs::read(&journal_path).unwrap();
            let mailbox = directory_snapshot(&root.join(".agent-collab/mailbox"));
            let wrong_app = dispatch_wire(
                server.clone(),
                Some(context_with_runtime(&root, "app-other", &wrong_app_runtime)),
                notification_subscribe_request(&worker_id, &token),
            )
            .await;
            assert!(!wrong_app.ok, "{wrong_app:?}");
            assert!(wrong_app
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("PROJECT_SCOPE_UNKNOWN:")));
            let wrong_project = dispatch_wire(
                server.clone(),
                Some(context_with_runtime(&wrong_root, "app-route", &runtime)),
                notification_subscribe_request(&worker_id, &token),
            )
            .await;
            assert!(!wrong_project.ok, "{wrong_project:?}");
            assert!(wrong_project
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("PROJECT_SCOPE_UNKNOWN:")));
            let after = mutation_snapshot(&server);
            assert_eq!(after, before);
            assert_eq!(std::fs::read(&journal_path).unwrap(), journal);
            assert_eq!(
                directory_snapshot(&root.join(".agent-collab/mailbox")),
                mailbox
            );
            std::fs::remove_dir_all(root).unwrap();
            std::fs::remove_dir_all(wrong_root).unwrap();
        }
    }

    #[tokio::test]
    async fn authenticated_send_uses_context_app_scope_and_rejects_forged_scope() {
        let (server, root, journal_path) = test_server();
        let app = "app-wire";
        for worker_id in ["sender", "recipient"] {
            let response = dispatch_wire(
                server.clone(),
                Some(context_with_app(&root, app)),
                Req::register(worker_id.into(),
                     format!("token-{worker_id}"),
                     root.display().to_string(),
                     test_candidates(&format!("thread-{worker_id}"))),
            )
            .await;
            assert!(response.ok, "{response:?}");
        }
        let sender_runtime = runtime_for_registered(&server, &root, "sender", app);
        let context = || context_with_runtime(&root, app, &sender_runtime);

        let scope = GlobalState::canonical_project_scope(&root).unwrap();
        let binding_generation = server
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &RouteScope {
                    app_scope_id: AppServerId::new(app).unwrap(),
                    project_scope_id: scope.clone(),
                },
                &BindingId::new("binding-sender").unwrap(),
            )
            .unwrap()
            .endpoint_generation;
        let command = |app_scope: &str, command_id: &str| {
            CommandEnvelope::new(
                CommandId::new(command_id).unwrap(),
                OperationId::new(format!("operation-{command_id}")).unwrap(),
                BindingId::new("binding-sender").unwrap(),
                binding_generation,
                RouteScope {
                    app_scope_id: AppServerId::new(app_scope).unwrap(),
                    project_scope_id: scope.clone(),
                },
                None,
                None,
                None,
                None,
            )
        };
        let accepted = dispatch_wire(
            server.clone(),
            Some(context()),
            Req::Send {
                from: "sender".into(),
                worker_id: Some("sender".into()),
                token: Some("token-sender".into()),
                command: Some(command(app, "wire-send-ok")),
                to: "recipient".into(),
                mtype: "notify".into(),
                subject: Some("scope".into()),
                body: "body".into(),
                in_reply_to: None,
                delivery: "immediate".into(),
            },
        )
        .await;
        assert!(accepted.ok, "{accepted:?}");
        let before_journal = std::fs::read(&journal_path).unwrap();
        let before_messages = server.state.lock().unwrap().msgs.len();
        let forged = dispatch_wire(
            server.clone(),
            Some(context()),
            Req::Send {
                from: "sender".into(),
                worker_id: Some("sender".into()),
                token: Some("token-sender".into()),
                command: Some(command("app-forged", "wire-send-forged")),
                to: "recipient".into(),
                mtype: "notify".into(),
                subject: Some("scope".into()),
                body: "body".into(),
                in_reply_to: None,
                delivery: "immediate".into(),
            },
        )
        .await;
        assert!(!forged.ok);
        assert!(forged
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("SEND_BINDING_REJECTED:")));
        assert_eq!(server.state.lock().unwrap().msgs.len(), before_messages);
        assert_eq!(std::fs::read(&journal_path).unwrap(), before_journal);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn wire_cli_recover_rebinds_generation_and_fences_old_context() {
        let (server, root, journal_path) = test_server();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let worker_id = "recover-worker";
        let token = "token-recover-worker";
        let first = dispatch_wire(
            server.clone(),
            Some(context_with_app(&root, app)),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates(&format!("thread-{worker_id}"))),
        )
        .await;
        assert!(first.ok, "{first:?}");
        commit_current_thread_route_for_runtime(
            &server,
            &server,
            worker_id,
            &root.display().to_string(),
            Some(&AppServerId::new(app).unwrap()),
        )
        .unwrap();
        let old_runtime = runtime_for_registered(&server, &root, worker_id, app);
        let old_context = context_with_runtime(&root, app, &old_runtime);
        let stale_request = notification_subscribe_request(worker_id, token);
        // The request is valid at admission time.  Rebind must make the
        // subsequent execution fail closed instead of relying on this stale
        // preflight result.
        assert!(validate_request_context(&server, &stale_request, Some(&old_context)).is_ok());

        let provisional = RuntimeIdentity::cli_adapter(worker_id).unwrap();
        let recovered = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, app, &provisional)),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates(&format!("thread-{worker_id}"))),
        )
        .await;
        assert!(recovered.ok, "{recovered:?}");
        let new_runtime = runtime_for_registered(&server, &root, worker_id, app);
        assert_eq!(
            new_runtime.endpoint_generation,
            old_runtime.endpoint_generation + 1
        );
        assert_eq!(new_runtime.runtime_id, old_runtime.runtime_id);
        assert_eq!(
            recovered.data["command"]["binding"]["endpoint_generation"],
            new_runtime.endpoint_generation
        );

        let accepted = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, app, &new_runtime)),
            notification_subscribe_request(worker_id, token),
        )
        .await;
        assert!(accepted.ok, "{accepted:?}");

        let before = mutation_snapshot(&server);
        let before_journal = std::fs::read(&journal_path).unwrap();
        let before_mailbox = directory_snapshot(&root.join(".agent-collab/mailbox"));
        let rejected = dispatch_wire(server.clone(), Some(old_context), stale_request).await;
        assert!(!rejected.ok, "{rejected:?}");
        assert!(rejected
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("SESSION_THREAD_BINDING_MISMATCH:")));
        assert_eq!(mutation_snapshot(&server), before);
        assert_eq!(std::fs::read(&journal_path).unwrap(), before_journal);
        assert_eq!(
            directory_snapshot(&root.join(".agent-collab/mailbox")),
            before_mailbox
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn wire_cli_register_rejects_token_rotation_for_the_same_thread_and_route() {
        let (server, root, journal_path) = test_server();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let worker_id = "lost-token-worker";
        let old_token = "old-token-lost-token-worker";
        let new_token = "new-token-lost-token-worker";
        let first = dispatch_wire(
            server.clone(),
            Some(context_with_app(&root, app)),
            Req::register(worker_id.into(),
                 old_token.into(),
                 root.display().to_string(),
                 test_candidates(&format!("thread-{worker_id}"))),
        )
        .await;
        assert!(first.ok, "{first:?}");
        commit_current_thread_route_for_runtime(
            &server,
            &server,
            worker_id,
            &root.display().to_string(),
            Some(&AppServerId::new(app).unwrap()),
        )
        .unwrap();
        let old_runtime = runtime_for_registered(&server, &root, worker_id, app);
        let provisional = RuntimeIdentity::cli_adapter(worker_id).unwrap();
        let recovery_candidates =
            test_candidates_for_registered(&server, &root, worker_id, app).unwrap();
        let tmux_endpoint = &recovery_candidates.tmux.as_ref().unwrap().endpoint;
        assert_eq!(
            server
                .state
                .lock()
                .unwrap()
                .global
                .lookup_tmux_route(tmux_endpoint)
                .map(|binding| binding.agent_id.as_str()),
            Some(worker_id)
        );
        assert_eq!(
            provisional,
            RuntimeIdentity::cli_adapter(worker_id).unwrap()
        );

        let recovered = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, app, &provisional)),
            Req::register(worker_id.into(),
                 new_token.into(),
                 root.display().to_string(),
                 Some(recovery_candidates)),
        )
        .await;
        assert!(!recovered.ok, "{recovered:?}");
        assert!(recovered.error.as_deref().is_some_and(|error| error.starts_with("TOKEN_MISMATCH:")));

        let new_runtime = runtime_for_registered(&server, &root, worker_id, app);
        assert_eq!(
            new_runtime.endpoint_generation,
            old_runtime.endpoint_generation
        );
        assert_eq!(new_runtime.runtime_id, old_runtime.runtime_id);
        assert_eq!(
            server
                .state
                .lock()
                .unwrap()
                .workers
                .get(worker_id)
                .unwrap()
                .token,
            old_token
        );
        assert!(!std::fs::read(&journal_path).unwrap().is_empty());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn wire_register_rejects_token_rotation_with_the_persisted_runtime() {
        let (server, root, _) = test_server();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let worker_id = "worker-recover-worker";
        let old_token = "old-token-worker-recover-worker";
        let new_token = "new-token-worker-recover-worker";
        let first = dispatch_wire(
            server.clone(),
            Some(context_with_app(&root, app)),
            Req::register(worker_id.into(),
                 old_token.into(),
                 root.display().to_string(),
                 test_candidates(&format!("thread-{worker_id}"))),
        )
        .await;
        assert!(first.ok, "{first:?}");
        commit_current_thread_route_for_runtime(
            &server,
            &server,
            worker_id,
            &root.display().to_string(),
            Some(&AppServerId::new(app).unwrap()),
        )
        .unwrap();
        let persisted_runtime = runtime_for_registered(&server, &root, worker_id, app);

        let recovered = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, app, &persisted_runtime)),
            Req::register(worker_id.into(),
                 new_token.into(),
                 root.display().to_string(),
                 test_candidates_for_registered(&server, &root, worker_id, app)),
        )
        .await;
        assert!(!recovered.ok, "{recovered:?}");
        assert!(recovered.error.as_deref().is_some_and(|error| error.starts_with("TOKEN_MISMATCH:")));
        assert_eq!(runtime_for_registered(&server, &root, worker_id, app), persisted_runtime);
        assert_eq!(
            server
                .state
                .lock()
                .unwrap()
                .workers
                .get(worker_id)
                .unwrap()
                .token,
            old_token
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn wire_register_recovers_an_orphan_binding_on_the_same_route() {
        let (server, root, _) = test_server();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let worker_id = "orphan-binding-worker";
        let token = "token-orphan-binding-worker";
        let registered = dispatch_wire(
            server.clone(),
            Some(context_with_app(&root, app)),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates(&format!("thread-{worker_id}"))),
        )
        .await;
        assert!(registered.ok, "{registered:?}");
        commit_current_thread_route_for_runtime(
            &server,
            &server,
            worker_id,
            &root.display().to_string(),
            Some(&AppServerId::new(app).unwrap()),
        )
        .unwrap();
        let old_runtime = runtime_for_registered(&server, &root, worker_id, app);
        write_global_identity(
            &server.host_paths,
            worker_id,
            token,
            Some(&GlobalState::canonical_project_scope(&root).unwrap()),
            &old_runtime,
        );

        // A daemon restart may replay the durable binding while the resident
        // worker projection is absent.  The same identity and route must be
        // able to recover that binding instead of being treated as a
        // cross-project registration.
        server.state.lock().unwrap().workers.remove(worker_id);
        let provisional = RuntimeIdentity::cli_adapter(worker_id).unwrap();
        let recovered = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, app, &provisional)),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates_for_registered(&server, &root, worker_id, app)),
        )
        .await;
        assert!(recovered.ok, "{recovered:?}");

        let state = server.state.lock().unwrap();
        assert_eq!(state.workers[worker_id].token, token);
        let bindings = state
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .filter(|binding| binding.agent_id.as_str() == worker_id)
            .collect::<Vec<_>>();
        assert_eq!(bindings.len(), 1);
        assert_eq!(
            bindings[0].endpoint_generation,
            old_runtime.endpoint_generation + 1
        );
        assert_eq!(
            bindings[0].binding_id,
            BindingId::new(format!("binding-{worker_id}")).unwrap()
        );
        assert_eq!(bindings[0].app_scope_id.as_str(), app);
        assert_eq!(
            bindings[0].project_scope,
            GlobalState::canonical_project_scope(&root).unwrap()
        );
        assert_eq!(
            bindings[0].native_thread_id.as_ref().unwrap().as_str(),
            format!("thread-{worker_id}")
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn wire_register_orphan_recovery_rejects_an_unbound_replacement_thread() {
        let (server, root, journal_path) = test_server();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let worker_id = "orphan-binding-negative-worker";
        let token = "token-orphan-binding-negative-worker";
        let registered = dispatch_wire(
            server.clone(),
            Some(context_with_app(&root, app)),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates(&format!("thread-{worker_id}"))),
        )
        .await;
        assert!(registered.ok, "{registered:?}");
        commit_current_thread_route_for_runtime(
            &server,
            &server,
            worker_id,
            &root.display().to_string(),
            Some(&AppServerId::new(app).unwrap()),
        )
        .unwrap();
        let runtime = runtime_for_registered(&server, &root, worker_id, app);
        write_global_identity(
            &server.host_paths,
            worker_id,
            token,
            Some(&GlobalState::canonical_project_scope(&root).unwrap()),
            &runtime,
        );

        server.state.lock().unwrap().workers.remove(worker_id);
        let provisional = RuntimeIdentity::cli_adapter(worker_id).unwrap();
        let before = mutation_snapshot(&server);
        let before_journal = std::fs::read(&journal_path).unwrap();
        let before_mailbox = directory_snapshot(&root.join(".agent-collab/mailbox"));
        let forged = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, app, &provisional)),
            Req::register(worker_id.into(),
                 "forged-token".into(),
                 root.display().to_string(),
                 test_candidates("thread-unbound-replacement")),
        )
        .await;
        assert!(!forged.ok, "{forged:?}");
        assert!(forged
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("RUNTIME_BINDING_REJECTED:")));
        assert_eq!(mutation_snapshot(&server), before);
        assert_eq!(std::fs::read(&journal_path).unwrap(), before_journal);
        assert_eq!(
            directory_snapshot(&root.join(".agent-collab/mailbox")),
            before_mailbox
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn wire_register_orphan_recovery_rejects_a_forged_token_on_the_persisted_thread() {
        let (server, root, journal_path) = test_server();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let worker_id = "orphan-binding-forged-token-worker";
        let token = "token-orphan-binding-forged-token-worker";
        let registered = dispatch_wire(
            server.clone(),
            Some(context_with_app(&root, app)),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates(&format!("thread-{worker_id}"))),
        )
        .await;
        assert!(registered.ok, "{registered:?}");
        let runtime = runtime_for_registered(&server, &root, worker_id, app);
        write_global_identity(
            &server.host_paths,
            worker_id,
            token,
            Some(&GlobalState::canonical_project_scope(&root).unwrap()),
            &runtime,
        );

        server.state.lock().unwrap().workers.remove(worker_id);
        let provisional = RuntimeIdentity::cli_adapter(worker_id).unwrap();
        let before = mutation_snapshot(&server);
        let before_journal = std::fs::read(&journal_path).unwrap();
        let before_mailbox = directory_snapshot(&root.join(".agent-collab/mailbox"));
        let forged = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, app, &provisional)),
            Req::register(worker_id.into(),
                 "forged-token".into(),
                 root.display().to_string(),
                 test_candidates(&format!("thread-{worker_id}"))),
        )
        .await;
        assert!(!forged.ok, "{forged:?}");
        assert!(forged
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("RUNTIME_BINDING_REJECTED:")));
        assert_eq!(mutation_snapshot(&server), before);
        assert_eq!(std::fs::read(&journal_path).unwrap(), before_journal);
        assert_eq!(
            directory_snapshot(&root.join(".agent-collab/mailbox")),
            before_mailbox
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn wire_register_orphan_recovery_rejects_ambiguous_global_bindings() {
        let (server, root, journal_path) = test_server();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let worker_id = "orphan-binding-ambiguous-worker";
        let token = "token-orphan-binding-ambiguous-worker";
        let registered = dispatch_wire(
            server.clone(),
            Some(context_with_app(&root, app)),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates(&format!("thread-{worker_id}"))),
        )
        .await;
        assert!(registered.ok, "{registered:?}");
        let runtime = runtime_for_registered(&server, &root, worker_id, app);
        write_global_identity(
            &server.host_paths,
            worker_id,
            token,
            Some(&GlobalState::canonical_project_scope(&root).unwrap()),
            &runtime,
        );
        let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
        let ambiguous = RuntimeBinding::new_with_session(
            project_scope.clone(),
            AppServerId::new(app).unwrap(),
            AgentId::new(worker_id.to_owned()).unwrap(),
            RuntimeId::new("runtime-orphan-ambiguous").unwrap(),
            BindingId::new("binding-orphan-ambiguous").unwrap(),
            runtime.endpoint_generation,
            runtime.session_id.clone(),
            runtime.native_thread_id.clone(),
        )
        .unwrap();
        server
            .state
            .lock()
            .unwrap()
            .global
            .projects
            .get_mut(project_scope.as_str())
            .unwrap()
            .runtime_bindings
            .insert(ambiguous.binding_id.as_str().into(), ambiguous);
        server.state.lock().unwrap().workers.remove(worker_id);
        let provisional = RuntimeIdentity::cli_adapter(worker_id).unwrap();
        let before = mutation_snapshot(&server);
        let before_journal = std::fs::read(&journal_path).unwrap();
        let before_mailbox = directory_snapshot(&root.join(".agent-collab/mailbox"));
        let rejected = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, app, &provisional)),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates(&format!("thread-{worker_id}"))),
        )
        .await;
        assert!(!rejected.ok, "{rejected:?}");
        assert!(rejected
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("RUNTIME_BINDING_REJECTED:")
                || error.starts_with("PROJECT_ROUTE_NOT_READY/UNSUPPORTED:")));
        assert_eq!(mutation_snapshot(&server), before);
        assert_eq!(std::fs::read(&journal_path).unwrap(), before_journal);
        assert_eq!(
            directory_snapshot(&root.join(".agent-collab/mailbox")),
            before_mailbox
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn wire_cli_recover_rejects_a_forged_token_and_a_forged_route() {
        let (server, root, journal_path) = test_server();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let worker_id = "recover-negative-worker";
        let token = "token-recover-negative-worker";
        let registered = dispatch_wire(
            server.clone(),
            Some(context_with_app(&root, app)),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates(&format!("thread-{worker_id}"))),
        )
        .await;
        assert!(registered.ok, "{registered:?}");
        commit_current_thread_route_for_runtime(
            &server,
            &server,
            worker_id,
            &root.display().to_string(),
            Some(&AppServerId::new(app).unwrap()),
        )
        .unwrap();
        let runtime = runtime_for_registered(&server, &root, worker_id, app);
        let provisional = RuntimeIdentity::cli_adapter(worker_id).unwrap();

        let before = mutation_snapshot(&server);
        let before_journal = std::fs::read(&journal_path).unwrap();
        let before_mailbox = directory_snapshot(&root.join(".agent-collab/mailbox"));
        let wrong_token = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, app, &provisional)),
            Req::register(worker_id.into(),
                 "forged-token".into(),
                 root.display().to_string(),
                 test_candidates("thread-forged-token")),
        )
        .await;
        assert!(!wrong_token.ok, "{wrong_token:?}");
        assert!(wrong_token
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("TOKEN_MISMATCH:")));
        assert_eq!(mutation_snapshot(&server), before);
        assert_eq!(std::fs::read(&journal_path).unwrap(), before_journal);
        assert_eq!(
            directory_snapshot(&root.join(".agent-collab/mailbox")),
            before_mailbox
        );

        let other = dispatch_wire(
            server.clone(),
            Some(context_with_app(&root, app)),
            Req::register("other-worker".into(),
                 "token-other-worker".into(),
                 root.display().to_string(),
                 test_candidates("thread-another-worker")),
        )
        .await;
        assert!(other.ok, "{other:?}");
        commit_current_thread_route_for_runtime(
            &server,
            &server,
            "other-worker",
            &root.display().to_string(),
            Some(&AppServerId::new(app).unwrap()),
        )
        .unwrap();

        // The same pane, claimed by another worker. A pane has one owner, so the
        // later registrant takes it and the previous claimant is closed. The
        // Codex thread the request carries is not the anchor and cannot fence
        // that replacement.
        let taken = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&root, app, &provisional)),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates_for_registered(&server, &root, "other-worker", app)),
        )
        .await;
        assert!(taken.ok, "{taken:?}");
        {
            let state = server.state.lock().unwrap();
            assert!(
                !state.workers.contains_key("other-worker"),
                "the replaced claimant must be closed; workers={:?} bindings={:?}",
                state.workers.keys().collect::<Vec<_>>(),
                state
                    .global
                    .projects
                    .values()
                    .flat_map(|project| project.runtime_bindings.values())
                    .map(|binding| (
                        binding.agent_id.as_str().to_owned(),
                        binding.tmux_endpoint.is_some(),
                        binding
                            .native_thread_id
                            .as_ref()
                            .map(|thread| thread.as_str().to_owned()),
                    ))
                    .collect::<Vec<_>>()
            );
        }

        let wrong_root = root.with_file_name(format!(
            "{}-wrong-route",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&wrong_root).unwrap();
        let before = mutation_snapshot(&server);
        let before_journal = std::fs::read(&journal_path).unwrap();
        let before_mailbox = directory_snapshot(&root.join(".agent-collab/mailbox"));
        let wrong_route = dispatch_wire(
            server.clone(),
            Some(context_with_runtime(&wrong_root, app, &provisional)),
            Req::register(worker_id.into(),
                 token.into(),
                 wrong_root.display().to_string(),
                 test_candidates(&format!("thread-{worker_id}"))),
        )
        .await;
        assert!(!wrong_route.ok, "{wrong_route:?}");
        assert!(wrong_route
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("PROJECT_SCOPE_UNKNOWN:")));
        assert_eq!(mutation_snapshot(&server), before);
        assert_eq!(std::fs::read(&journal_path).unwrap(), before_journal);
        assert_eq!(
            directory_snapshot(&root.join(".agent-collab/mailbox")),
            before_mailbox
        );

        let forged_worker = "forged-worker";
        let forged_context = context_with_runtime(&root, app, &runtime);
        let before = mutation_snapshot(&server);
        let before_journal = std::fs::read(&journal_path).unwrap();
        let before_mailbox = directory_snapshot(&root.join(".agent-collab/mailbox"));
        let forged = dispatch_wire(
            server.clone(),
            Some(forged_context),
            Req::register(forged_worker.into(),
                 "token-forged-worker".into(),
                 root.display().to_string(),
                 test_candidates("thread-forged-worker")),
        )
        .await;
        assert!(!forged.ok, "{forged:?}");
        assert!(forged
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("RUNTIME_BINDING_REJECTED:")));
        assert_eq!(mutation_snapshot(&server), before);
        assert_eq!(std::fs::read(&journal_path).unwrap(), before_journal);
        assert_eq!(
            directory_snapshot(&root.join(".agent-collab/mailbox")),
            before_mailbox
        );

        assert_eq!(runtime.endpoint_generation, 1);
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(wrong_root).unwrap();
    }

    /// A worker that lost its pane to a later registrant is not registered any
    /// more: the takeover left a retirement tombstone (no thread, no pane) and
    /// closed the worker record. When that worker applies again on the same pane
    /// the daemon must accept the later registrant and hand it the pane, even
    /// though the identity it persisted before the takeover still names the
    /// pre-retirement generation. A tombstone owns nothing, so it cannot fence
    /// its own worker.
    #[tokio::test]
    async fn a_retired_worker_reclaims_the_pane_from_the_later_registrant() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let context = context_with_app(&root, app);
        let worker_id = "reclaim-tombstone-worker";
        let token = "token-reclaim-tombstone-worker";
        let taker_id = "reclaim-tombstone-taker";
        let candidates = test_candidates(&format!("thread-{worker_id}")).unwrap();
        let endpoint = candidates.tmux.as_ref().unwrap().endpoint.clone();
        let (_, registered) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 Some(candidates)),
        );
        assert!(registered.ok, "{registered:?}");

        // The CLI persists the runtime the daemon just gave it. The takeover
        // below advances the daemon's generation, so this record goes stale.
        let registered_binding = {
            let runtime = manager.select_runtime(&context).unwrap();
            let state = runtime.state.lock().unwrap();
            state
                .global
                .projects
                .values()
                .flat_map(|project| project.runtime_bindings.values())
                .find(|binding| binding.agent_id.as_str() == worker_id)
                .cloned()
                .expect("the registration binding is durable")
        };
        let persisted = RuntimeIdentity {
            agent_id: registered_binding.agent_id.clone(),
            runtime_id: registered_binding.runtime_id.clone(),
            appserver_id: registered_binding.app_scope_id.clone(),
            endpoint_generation: registered_binding.endpoint_generation,
            binding_id: registered_binding.binding_id.clone(),
            session_id: registered_binding.session_id.clone(),
            native_thread_id: registered_binding.native_thread_id.clone(),
        };
        write_global_identity(
            &server.host_paths,
            worker_id,
            token,
            Some(&GlobalState::canonical_project_scope(&root).unwrap()),
            &persisted,
        );

        // A later registration on the same pane wins it and retires the first
        // worker in the same commit.
        let mut taker_candidates = test_candidates(&format!("thread-{taker_id}")).unwrap();
        {
            let taker_endpoint = &mut taker_candidates.tmux.as_mut().unwrap().endpoint;
            taker_endpoint.socket_path = endpoint.socket_path.clone();
            taker_endpoint.server_pid = endpoint.server_pid;
            taker_endpoint.tmux_session_id = endpoint.tmux_session_id.clone();
            taker_endpoint.pane_id = endpoint.pane_id.clone();
            taker_endpoint.pane_pid = endpoint.pane_pid;
        }
        let (_, taken) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register(taker_id.into(),
                 format!("token-{taker_id}"),
                 root.display().to_string(),
                 Some(taker_candidates)),
        );
        assert!(taken.ok, "{taken:?}");
        {
            let runtime = manager.select_runtime(&context).unwrap();
            let state = runtime.state.lock().unwrap();
            assert!(
                !state.workers.contains_key(worker_id),
                "the takeover closes the superseded worker"
            );
            let tombstone = state
                .global
                .projects
                .values()
                .flat_map(|project| project.runtime_bindings.values())
                .find(|binding| binding.agent_id.as_str() == worker_id)
                .cloned()
                .expect("the superseded binding stays in the ledger");
            assert!(
                tombstone.native_thread_id.is_none() && tombstone.tmux_endpoint.is_none(),
                "the superseded binding is a retirement tombstone: {tombstone:?}"
            );
        }

        // The same worker applies again on the same pane with the identity it
        // persisted before the takeover.
        let provisional = RuntimeIdentity::cli_adapter(worker_id).unwrap();
        let mut reclaim_candidates = test_candidates(&format!("thread-{worker_id}")).unwrap();
        reclaim_candidates.tmux.as_mut().unwrap().endpoint = endpoint.clone();
        let (_, reclaimed) = manager.dispatch_sync(
            Some(context_with_runtime(&root, app, &provisional)),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 Some(reclaim_candidates)),
        );
        assert!(reclaimed.ok, "{reclaimed:?}");

        let pane_owners = server
            .state
            .lock()
            .unwrap()
            .global
            .current_thread_routes
            .values()
            .filter(|binding| {
                binding.tmux_endpoint.as_ref().is_some_and(|current| {
                    crate::client::adapters::tmux::same_owned_pane(current, &endpoint)
                })
            })
            .map(|binding| binding.agent_id.as_str().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(pane_owners, vec![worker_id.to_owned()]);
        {
            let runtime = manager.select_runtime(&context).unwrap();
            let state = runtime.state.lock().unwrap();
            assert!(
                !state.workers.contains_key(taker_id),
                "the reclaimed pane closes the taker"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_forged_token_cannot_reclaim_a_retired_workers_pane() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let context = context_with_app(&root, app);
        let worker_id = "forged-tombstone-worker";
        let token = "token-forged-tombstone-worker";
        let taker_id = "forged-tombstone-taker";
        let candidates = test_candidates(&format!("thread-{worker_id}")).unwrap();
        let endpoint = candidates.tmux.as_ref().unwrap().endpoint.clone();
        let (_, registered) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register(
                worker_id.into(),
                token.into(),
                root.display().to_string(),
                Some(candidates),
            ),
        );
        assert!(registered.ok, "{registered:?}");
        let registered_binding = {
            let runtime = manager.select_runtime(&context).unwrap();
            let state = runtime.state.lock().unwrap();
            state
                .global
                .projects
                .values()
                .flat_map(|project| project.runtime_bindings.values())
                .find(|binding| binding.agent_id.as_str() == worker_id)
                .cloned()
                .expect("the registration binding is durable")
        };
        let persisted = RuntimeIdentity {
            agent_id: registered_binding.agent_id.clone(),
            runtime_id: registered_binding.runtime_id.clone(),
            appserver_id: registered_binding.app_scope_id.clone(),
            endpoint_generation: registered_binding.endpoint_generation,
            binding_id: registered_binding.binding_id.clone(),
            session_id: registered_binding.session_id.clone(),
            native_thread_id: registered_binding.native_thread_id.clone(),
        };
        write_global_identity(
            &server.host_paths,
            worker_id,
            token,
            Some(&GlobalState::canonical_project_scope(&root).unwrap()),
            &persisted,
        );

        // A later registration on the same pane retires the first worker to a
        // tombstone. The tombstone owns no resource, but the persisted identity
        // still owns the worker name, so a forged token must not ride the
        // tombstone exemption back onto the pane.
        let mut taker_candidates = test_candidates(&format!("thread-{taker_id}")).unwrap();
        {
            let taker_endpoint = &mut taker_candidates.tmux.as_mut().unwrap().endpoint;
            taker_endpoint.socket_path = endpoint.socket_path.clone();
            taker_endpoint.server_pid = endpoint.server_pid;
            taker_endpoint.tmux_session_id = endpoint.tmux_session_id.clone();
            taker_endpoint.pane_id = endpoint.pane_id.clone();
            taker_endpoint.pane_pid = endpoint.pane_pid;
        }
        let (_, taken) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register(
                taker_id.into(),
                format!("token-{taker_id}"),
                root.display().to_string(),
                Some(taker_candidates),
            ),
        );
        assert!(taken.ok, "{taken:?}");

        let provisional = RuntimeIdentity::cli_adapter(worker_id).unwrap();
        let mut forged_candidates = test_candidates(&format!("thread-{worker_id}")).unwrap();
        forged_candidates.tmux.as_mut().unwrap().endpoint = endpoint.clone();
        let (_, forged) = manager.dispatch_sync(
            Some(context_with_runtime(&root, app, &provisional)),
            Req::register(
                worker_id.into(),
                "token-forged".into(),
                root.display().to_string(),
                Some(forged_candidates),
            ),
        );
        assert!(!forged.ok, "a forged token reclaimed the pane: {forged:?}");
        let error = forged.error.unwrap_or_default();
        assert!(error.contains("RUNTIME_BINDING_REJECTED"), "{error}");
        let pane_owners = server
            .state
            .lock()
            .unwrap()
            .global
            .current_thread_routes
            .values()
            .filter(|binding| {
                binding.tmux_endpoint.as_ref().is_some_and(|current| {
                    crate::client::adapters::tmux::same_owned_pane(current, &endpoint)
                })
            })
            .map(|binding| binding.agent_id.as_str().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            pane_owners,
            vec![taker_id.to_owned()],
            "a refused forged registration must leave the pane with its owner"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
