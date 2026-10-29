    #[tokio::test]
    async fn manager_isolates_app_and_project_routes_and_replays_each_runtime() {
        let (server, host_root, host_journal) = test_server();
        let project_a = host_root.with_file_name(format!(
            "{}-project-a",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        let project_b = host_root.with_file_name(format!(
            "{}-project-b",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        for project_root in [&project_a, &project_b] {
            std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
        }
        let canonical_project_a = project_a.canonicalize().unwrap();
        let canonical_project_b = project_b.canonicalize().unwrap();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();

        let register = |context: ProjectContext, worker_id: &str, token: &str| {
            let cwd = if worker_id == "app-a-worker" || worker_id == "app-b-worker" {
                &project_a
            } else {
                &project_b
            };
            let (runtime, response) = register_host_fixture(&manager,
                Some(context),
                Req::register(worker_id.into(),
                     token.into(),
                     cwd.display().to_string(),
                     test_candidates_at(&format!("thread-{worker_id}"), cwd)),
            );
            assert!(response.ok, "registration {worker_id}: {response:?}");
            runtime
        };

        // The same project may have two appserver routes. Each route gets a
        // distinct reducer/storage namespace, so app A cannot observe app B.
        let app_a_context = context_with_app(&project_a, "app/a");
        let app_b_context = context_with_app(&project_a, "app:a");
        let project_b_context = context_with_app(&project_b, "app/a");
        let app_a_runtime = register(app_a_context.clone(), "app-a-worker", "token-app-a");
        let app_b_runtime = register(app_b_context.clone(), "app-b-worker", "token-app-b");
        let project_b_runtime = register(
            project_b_context.clone(),
            "project-b-worker",
            "token-project-b",
        );
        assert!(!Arc::ptr_eq(&app_a_runtime, &app_b_runtime));
        assert!(!Arc::ptr_eq(&app_a_runtime, &project_b_runtime));
        assert!(!Arc::ptr_eq(&app_b_runtime, &project_b_runtime));
        assert_ne!(app_a_runtime.storage_root, app_b_runtime.storage_root);
        assert_ne!(app_a_runtime.storage_root, project_b_runtime.storage_root);
        assert_ne!(app_b_runtime.storage_root, project_b_runtime.storage_root);
        assert_eq!(
            app_a_runtime.storage_root,
            host_paths
                .app_storage_root(
                    app_a_context.project_scope.as_str(),
                    app_a_context.app_scope_id.as_str(),
                )
                .canonicalize()
                .unwrap()
        );
        assert_eq!(
            app_b_runtime.storage_root,
            host_paths
                .app_storage_root(
                    app_b_context.project_scope.as_str(),
                    app_b_context.app_scope_id.as_str(),
                )
                .canonicalize()
                .unwrap()
        );
        assert_eq!(
            project_b_runtime.storage_root,
            host_paths
                .app_storage_root(
                    project_b_context.project_scope.as_str(),
                    project_b_context.app_scope_id.as_str(),
                )
                .canonicalize()
                .unwrap()
        );
        assert_ne!(canonical_project_a, canonical_project_b);

        let app_a_identity =
            runtime_for_registered(&app_a_runtime, &project_a, "app-a-worker", "app/a");
        let app_b_identity =
            runtime_for_registered(&app_b_runtime, &project_a, "app-b-worker", "app:a");
        let project_b_identity =
            runtime_for_registered(&project_b_runtime, &project_b, "project-b-worker", "app/a");
        let app_a_runtime_context = context_with_runtime(&project_a, "app/a", &app_a_identity);
        let app_b_runtime_context = context_with_runtime(&project_a, "app:a", &app_b_identity);
        let project_b_runtime_context =
            context_with_runtime(&project_b, "app/a", &project_b_identity);

        for (context, runtime, worker_id) in [
            (
                app_a_runtime_context.clone(),
                app_a_runtime.clone(),
                "app-a-worker",
            ),
            (
                app_b_runtime_context.clone(),
                app_b_runtime.clone(),
                "app-b-worker",
            ),
            (
                project_b_runtime_context.clone(),
                project_b_runtime.clone(),
                "project-b-worker",
            ),
        ] {
            let (selected, response) = manager.dispatch_sync(Some(context), Req::StatusAll);
            assert!(response.ok, "{worker_id} status: {response:?}");
            assert!(Arc::ptr_eq(&selected, &runtime));
            assert_eq!(response.data["summary"]["workers"], 1);
            assert_eq!(response.data["workers"][0]["id"], worker_id);
        }

        let mut subscription_ids = Vec::new();
        for (context, worker_id, token) in [
            (app_a_runtime_context.clone(), "app-a-worker", "token-app-a"),
            (app_b_runtime_context.clone(), "app-b-worker", "token-app-b"),
            (
                project_b_runtime_context.clone(),
                "project-b-worker",
                "token-project-b",
            ),
        ] {
            let (_, response) = manager.dispatch_sync(
                Some(context.clone()),
                Req::TaskRegister {
                    worker_id: worker_id.into(),
                    token: token.into(),
                    task_id: "same-task-id".into(),
                    owner: None,
                    feature_id: Some(format!("feature-{worker_id}")),
                    worktree_path: None,
                    branch: None,
                    base_commit: None,
                    priority: "p1".into(),
                    next_step: Some(format!("verify {worker_id}")),
                    goal_prompt: None,
                },
            );
            assert!(response.ok, "{worker_id} task register: {response:?}");
            let (_, response) = manager.dispatch_sync(
                Some(context.clone()),
                notification_subscribe_request(worker_id, token),
            );
            assert!(response.ok, "{worker_id} subscription: {response:?}");
            subscription_ids.push(
                response.data["subscription"]["id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
        }

        let send_command =
            |identity: &RuntimeIdentity, project_root: &Path, app_scope: &str, suffix: &str| {
                CommandEnvelope::new(
                    CommandId::new(format!("route-isolation-command-{suffix}")).unwrap(),
                    OperationId::new(format!("route-isolation-operation-{suffix}")).unwrap(),
                    identity.binding_id.clone(),
                    identity.endpoint_generation,
                    RouteScope {
                        app_scope_id: AppServerId::new(app_scope).unwrap(),
                        project_scope_id: GlobalState::canonical_project_scope(project_root)
                            .unwrap(),
                    },
                    None,
                    None,
                    None,
                    None,
                )
            };
        let sends = [
            (
                app_a_runtime_context.clone(),
                app_a_identity.clone(),
                "app-a-worker",
                "token-app-a",
                &project_a,
                "app/a",
                "app-a",
            ),
            (
                app_b_runtime_context.clone(),
                app_b_identity.clone(),
                "app-b-worker",
                "token-app-b",
                &project_a,
                "app:a",
                "app-b",
            ),
            (
                project_b_runtime_context.clone(),
                project_b_identity.clone(),
                "project-b-worker",
                "token-project-b",
                &project_b,
                "app/a",
                "project-b",
            ),
        ];
        let mut message_ids = Vec::new();
        for (context, identity, worker_id, token, project_root, app_scope, suffix) in sends {
            let (_, response) = manager.dispatch_sync(
                Some(context.clone()),
                Req::Send {
                    from: worker_id.into(),
                    worker_id: Some(worker_id.into()),
                    token: Some(token.into()),
                    command: Some(send_command(&identity, project_root, app_scope, suffix)),
                    to: worker_id.into(),
                    mtype: "notify".into(),
                    subject: Some(format!("isolated message {suffix}")),
                    body: format!("body for {suffix}"),
                    in_reply_to: None,
                    reply_policy: None,
                    target_session: None,
                    delivery: "immediate".into(),
                },
            );
            assert!(response.ok, "{worker_id} send: {response:?}");
            message_ids.push(response.data["msg_id"].as_str().unwrap().to_owned());
        }

        // A recipient in another app/project route is absent from this
        // reducer, so cross-route send cannot silently fall back to a resident
        // or neighboring runtime.
        let cross_route_send = manager.dispatch_sync(
            Some(app_a_runtime_context.clone()),
            Req::Send {
                from: "app-a-worker".into(),
                worker_id: Some("app-a-worker".into()),
                token: Some("token-app-a".into()),
                command: Some(send_command(
                    &app_a_identity,
                    &project_a,
                    "app/a",
                    "cross-route",
                )),
                to: "app-b-worker".into(),
                mtype: "notify".into(),
                subject: Some("must stay isolated".into()),
                body: "cross-route delivery must fail closed".into(),
                in_reply_to: None,
                reply_policy: None,
                target_session: None,
                delivery: "immediate".into(),
            },
        );
        assert!(!cross_route_send.1.ok, "{:?}", cross_route_send.1);
        assert!(cross_route_send
            .1
            .error
            .as_deref()
            .is_some_and(|error| error.contains("recipient app-b-worker not registered")));

        // Snapshot is scoped to one project reducer. A target owned by a
        // neighboring route is absent from this state and must fail closed
        // before any transport probe.
        let cross_route_snapshot = manager.dispatch_sync(
            Some(app_a_runtime_context.clone()),
            Req::WorkerSnapshot {
                worker_id: "app-a-worker".into(),
                token: "token-app-a".into(),
                target_id: "project-b-worker".into(),
                lines: 40,
            },
        );
        assert!(!cross_route_snapshot.1.ok, "{:?}", cross_route_snapshot.1);
        assert!(cross_route_snapshot
            .1
            .error
            .as_deref()
            .is_some_and(|error| error.contains("target worker project-b-worker not registered")));

        for (context, worker_id, token, message_id, suffix) in [
            (
                app_a_runtime_context.clone(),
                "app-a-worker",
                "token-app-a",
                message_ids[0].clone(),
                "app-a",
            ),
            (
                app_b_runtime_context.clone(),
                "app-b-worker",
                "token-app-b",
                message_ids[1].clone(),
                "app-b",
            ),
            (
                project_b_runtime_context.clone(),
                "project-b-worker",
                "token-project-b",
                message_ids[2].clone(),
                "project-b",
            ),
        ] {
            let (_, polled) = dispatch_wire_routed(
                manager.clone(),
                Some(context.clone()),
                Req::Poll {
                    worker_id: worker_id.into(),
                    token: token.into(),
                    timeout_ms: 0,
                    receive_id: None,
                },
                tokio::sync::watch::channel(false).1,
            )
            .await;
            assert!(polled.ok, "{suffix} poll: {polled:?}");
            assert_eq!(polled.data["count"], 1);
            assert_eq!(polled.data["messages"][0]["id"], message_id);
            assert!(polled.data["messages"][0]["body"]
                .as_str()
                .unwrap()
                .contains(suffix));
        }

        let route_journal = host_paths.state_root().join("routes.jsonl");
        let route_records = load_host_route_records(&route_journal).unwrap();
        assert_eq!(route_records.len(), 3);
        let route_keys = route_records
            .iter()
            .map(|record| (record.app_scope_id.clone(), record.project_scope.clone()))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(route_keys.len(), 3);
        assert!(
            !std::fs::read(&host_journal).unwrap().is_empty(),
            "successful routed registrations must persist host current-thread transitions"
        );
        assert!(std::fs::read(&app_a_runtime.journal_path)
            .unwrap()
            .windows(b"app-a-worker".len())
            .any(|window| window == b"app-a-worker"));
        assert!(std::fs::read(&app_b_runtime.journal_path)
            .unwrap()
            .windows(b"app-b-worker".len())
            .any(|window| window == b"app-b-worker"));
        assert!(std::fs::read(&project_b_runtime.journal_path)
            .unwrap()
            .windows(b"project-b-worker".len())
            .any(|window| window == b"project-b-worker"));

        drop(app_a_runtime);
        drop(app_b_runtime);
        drop(project_b_runtime);
        drop(manager);
        let replayed_manager = ProjectRuntimeManager::new(server, &host_paths).unwrap();
        assert_eq!(replayed_manager.runtimes().len(), 4);

        for (context, worker_id, message_id, subscription_id, suffix) in [
            (
                app_a_runtime_context,
                "app-a-worker",
                message_ids[0].clone(),
                subscription_ids[0].clone(),
                "app-a",
            ),
            (
                app_b_runtime_context,
                "app-b-worker",
                message_ids[1].clone(),
                subscription_ids[1].clone(),
                "app-b",
            ),
            (
                project_b_runtime_context,
                "project-b-worker",
                message_ids[2].clone(),
                subscription_ids[2].clone(),
                "project-b",
            ),
        ] {
            let (selected, status) =
                replayed_manager.dispatch_sync(Some(context.clone()), Req::StatusAll);
            assert!(status.ok, "{suffix} replay status: {status:?}");
            assert_eq!(status.data["summary"]["workers"], 1);
            assert_eq!(status.data["workers"][0]["id"], worker_id);
            assert_eq!(status.data["summary"]["tasks"], 1);
            assert!(Arc::ptr_eq(
                &selected,
                &replayed_manager.select_runtime(&context).unwrap()
            ));

            let (_, task_status) = replayed_manager.dispatch_sync(
                Some(context.clone()),
                Req::TaskStatus {
                    task_id: Some("same-task-id".into()),
                },
            );
            assert!(task_status.ok, "{suffix} replay task: {task_status:?}");
            assert_eq!(task_status.data["owner"], worker_id);

            let (_, message_status) = replayed_manager
                .dispatch_sync(Some(context.clone()), Req::MsgStatus { msg_id: message_id });
            assert!(
                message_status.ok,
                "{suffix} replay message: {message_status:?}"
            );
            assert_eq!(message_status.data["to"], worker_id);

            let (_, notification_status) = replayed_manager.dispatch_sync(
                Some(context),
                Req::NotificationStatus {
                    worker_id: worker_id.into(),
                    token: match worker_id {
                        "app-a-worker" => "token-app-a",
                        "app-b-worker" => "token-app-b",
                        "project-b-worker" => "token-project-b",
                        _ => unreachable!(),
                    }
                    .into(),
                },
            );
            assert!(
                notification_status.ok,
                "{suffix} replay subscription: {notification_status:?}"
            );
            assert!(notification_status.data["subscriptions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|subscription| subscription["id"] == subscription_id));
        }

        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_a).unwrap();
        std::fs::remove_dir_all(project_b).unwrap();
    }

    #[tokio::test]
    async fn manager_cross_project_appserver_masters_send_and_reject_forged_source_evidence() {
        let (server, host_root, _) = test_server();

        let project_a = host_root.with_file_name(format!(
            "{}-cross-project-a",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        let project_b = host_root.with_file_name(format!(
            "{}-cross-project-b",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        for project_root in [&project_a, &project_b] {
            std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
        }
        let project_a = project_a.canonicalize().unwrap();
        let project_b = project_b.canonicalize().unwrap();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server, &host_paths).unwrap();

        let app_a = "cross-project-app-a";
        let app_b = "cross-project-app-b";
        let master_a = "cross-project-master-a";
        let master_b = "cross-project-master-b";
        let token_a = "token-cross-project-master-a";
        let token_b = "token-cross-project-master-b";
        let (source_runtime, source_registration) = register_host_fixture(&manager,
            Some(context_with_app(&project_a, app_a)),
            Req::register(master_a.into(),
                 token_a.into(),
                 project_a.display().to_string(),
                 test_candidates_at("thread-a", &project_a)),
        );
        assert!(source_registration.ok, "{source_registration:?}");
        assert_eq!(
            source_registration.data["transport_selected"]["kind"],
            "tmux"
        );
        assert_eq!(
            source_registration.data["transport_selected"]["thread_id"],
            "thread-a"
        );
        // The wire registration above creates the route and runtime. Rebind the
        // binding to a native App Server transport through the typed path, since
        // wire registration verifies App Server candidates by dialing a socket
        // and this suite has no live socket. Presence then comes from the App
        // Server status probe, not the tmux pane.
        register_native_peer(&source_runtime, &project_a, master_a, token_a, "thread-a", app_a);

        let (target_runtime, target_registration) = register_host_fixture(&manager,
            Some(context_with_app(&project_b, app_b)),
            Req::register(master_b.into(),
                 token_b.into(),
                 project_b.display().to_string(),
                 test_candidates_at("thread-b", &project_b)),
        );
        assert!(target_registration.ok, "{target_registration:?}");
        assert_eq!(
            target_registration.data["transport_selected"]["kind"],
            "tmux"
        );
        assert_eq!(
            target_registration.data["transport_selected"]["thread_id"],
            "thread-b"
        );
        register_native_peer(&target_runtime, &project_b, master_b, token_b, "thread-b", app_b);

        let source_identity = runtime_for_registered(&source_runtime, &project_a, master_a, app_a);
        let target_identity = runtime_for_registered(&target_runtime, &project_b, master_b, app_b);
        let source_context = context_with_runtime(&project_a, app_a, &source_identity);
        let target_context = context_with_runtime(&project_b, app_b, &target_identity);

        for (context, worker_id, token, approval) in [
            (
                source_context.clone(),
                master_a,
                token_a,
                "user approved cross-project-master-a as collab master",
            ),
            (
                target_context.clone(),
                master_b,
                token_b,
                "user approved cross-project-master-b as collab master",
            ),
        ] {
            let (_, promoted) = manager.dispatch_sync(
                Some(context),
                Req::MasterPromote {
                    worker_id: worker_id.into(),
                    token: token.into(),
                    approval: approval.into(),
                },
            );
            assert!(promoted.ok, "{worker_id} promotion: {promoted:?}");
        }

        let (assigned_by, approval, assigned_ms) = {
            let state = source_runtime.state.lock().unwrap();
            let route_scope = server_route_scope(&source_runtime, &state)
                .unwrap()
                .unwrap();
            let grant = current_master_grant(&state, Some(&route_scope)).unwrap();
            (
                grant.granted_by.clone(),
                Some(grant.approval.clone()),
                grant.granted_at_ms,
            )
        };
        let cross_project_send = |assigned_ms: i64| Req::CrossProjectSend {
            from: master_a.into(),
            from_project: project_a.display().to_string(),
            source_master_assigned_by: assigned_by.clone(),
            source_master_approval: approval.clone(),
            source_master_assigned_ms: assigned_ms,
            to: master_b.into(),
            subject: "cross-project tmux route".into(),
            body: "durable cross-project message".into(),
            in_reply_to: None,
        };

        let (selected, delivered) = manager.dispatch_sync(
            Some(target_context.clone()),
            cross_project_send(assigned_ms),
        );
        assert!(Arc::ptr_eq(&selected, &target_runtime));
        assert!(delivered.ok, "{delivered:?}");
        assert_eq!(delivered.data["durable"], true);
        assert_eq!(delivered.data["cross_project"], true);
        assert_eq!(delivered.data["source_master"], master_a);
        assert_eq!(delivered.data["target_master"], master_b);
        assert_eq!(delivered.data["notification"], "appserver-input-submitted");
        assert_eq!(delivered.data["consumed"], false);
        let message_id = delivered.data["msg_id"].as_str().unwrap().to_owned();
        assert!(target_runtime
            .state
            .lock()
            .unwrap()
            .msgs
            .contains_key(&message_id));

        let (_, received) = dispatch_wire_routed(
            manager.clone(),
            Some(target_context.clone()),
            Req::Poll {
                worker_id: master_b.into(),
                token: token_b.into(),
                timeout_ms: 0,
                receive_id: None,
            },
            tokio::sync::watch::channel(false).1,
        )
        .await;
        assert!(received.ok, "{received:?}");
        assert_eq!(received.data["count"], 1);
        assert_eq!(received.data["messages"][0]["id"], message_id);
        assert_eq!(received.data["messages"][0]["to"], master_b);

        let target_journal_before = std::fs::read(&target_runtime.journal_path).unwrap();
        let target_message_count = target_runtime.state.lock().unwrap().msgs.len();
        let (_, forged) =
            manager.dispatch_sync(Some(target_context), cross_project_send(assigned_ms + 1));
        assert!(!forged.ok, "{forged:?}");
        assert!(forged
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("CROSS_PROJECT_SOURCE_REJECTED:")));
        assert!(!forged
            .error
            .as_deref()
            .is_some_and(|error| error.contains("PROJECT_ROUTE_NOT_READY")));
        assert_eq!(
            std::fs::read(&target_runtime.journal_path).unwrap(),
            target_journal_before
        );
        assert_eq!(
            target_runtime.state.lock().unwrap().msgs.len(),
            target_message_count
        );

        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_a).unwrap();
        std::fs::remove_dir_all(project_b).unwrap();
    }

    #[tokio::test]
    async fn nonresident_route_replays_full_query_mutation_and_notification_surface() {
        let (server, host_root, host_journal) = test_server();
        let project_root = host_root.with_file_name(format!(
            "{}-full-replay-surface",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app_scope = "full-replay-app";
        let sender_id = "full-replay-sender";
        let recipient_id = "full-replay-recipient";
        let sender_token = "token-full-replay-sender";
        let recipient_token = "token-full-replay-recipient";

        let (sender_runtime, sender_registration) = register_host_fixture(&manager,
            Some(context_with_app(&project_root, app_scope)),
            Req::register(sender_id.into(),
                 sender_token.into(),
                 project_root.display().to_string(),
                 test_candidates_at("thread-full-replay-sender", &project_root)),
        );
        assert!(sender_registration.ok, "{sender_registration:?}");
        let (recipient_runtime, recipient_registration) = register_host_fixture(&manager,
            Some(context_with_app(&project_root, app_scope)),
            Req::register(recipient_id.into(),
                 recipient_token.into(),
                 project_root.display().to_string(),
                 test_candidates_at("thread-full-replay-recipient", &project_root)),
        );
        assert!(recipient_registration.ok, "{recipient_registration:?}");
        assert!(Arc::ptr_eq(&sender_runtime, &recipient_runtime));
        assert!(!Arc::ptr_eq(&sender_runtime, &server));

        let sender_identity =
            runtime_for_registered(&sender_runtime, &project_root, sender_id, app_scope);
        let recipient_identity =
            runtime_for_registered(&recipient_runtime, &project_root, recipient_id, app_scope);
        let sender_context = context_with_runtime(&project_root, app_scope, &sender_identity);
        let recipient_context = context_with_runtime(&project_root, app_scope, &recipient_identity);

        let (_, task_registration) = manager.dispatch_sync(
            Some(sender_context.clone()),
            Req::TaskRegister {
                worker_id: sender_id.into(),
                token: sender_token.into(),
                task_id: "full-replay-task".into(),
                owner: None,
                feature_id: Some("full-replay-feature".into()),
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p1".into(),
                next_step: Some("continue after runtime replay".into()),
                goal_prompt: None,
            },
        );
        assert!(task_registration.ok, "{task_registration:?}");
        let (_, subscription) = manager.dispatch_sync(
            Some(recipient_context.clone()),
            notification_subscribe_request(recipient_id, recipient_token),
        );
        assert!(subscription.ok, "{subscription:?}");

        let send_command = |suffix: &str| {
            CommandEnvelope::new(
                CommandId::new(format!("full-replay-command-{suffix}")).unwrap(),
                OperationId::new(format!("full-replay-operation-{suffix}")).unwrap(),
                sender_identity.binding_id.clone(),
                sender_identity.endpoint_generation,
                RouteScope {
                    app_scope_id: AppServerId::new(app_scope).unwrap(),
                    project_scope_id: GlobalState::canonical_project_scope(&project_root).unwrap(),
                },
                None,
                None,
                None,
                None,
            )
        };
        let (_, first_send) = manager.dispatch_sync(
            Some(sender_context.clone()),
            Req::Send {
                from: sender_id.into(),
                worker_id: Some(sender_id.into()),
                token: Some(sender_token.into()),
                command: Some(send_command("before-replay")),
                to: recipient_id.into(),
                mtype: "notify".into(),
                subject: Some("full replay before".into()),
                body: "message retained across runtime replay".into(),
                in_reply_to: None,
                reply_policy: None,
                target_session: None,
                delivery: "immediate".into(),
            },
        );
        assert!(first_send.ok, "{first_send:?}");
        let first_message_id = first_send.data["msg_id"].as_str().unwrap().to_owned();
        let runtime_storage = sender_runtime.storage_root.clone();
        let runtime_journal = sender_runtime.journal_path.clone();
        let mailbox_path = runtime_storage
            .join(".agent-collab/mailbox")
            .join(format!("recipient-{recipient_id}.jsonl"));
        assert!(mailbox_path.exists());
        assert!(std::fs::read(&mailbox_path)
            .unwrap()
            .windows(first_message_id.len())
            .any(|window| window == first_message_id.as_bytes()));
        let route_journal = host_paths.state_root().join("routes.jsonl");
        let route_journal_before_replay = std::fs::read(&route_journal).unwrap();
        assert!(
            !std::fs::read(&host_journal).unwrap().is_empty(),
            "successful routed registrations must persist host current-thread transitions"
        );

        drop(sender_runtime);
        drop(recipient_runtime);
        drop(manager);
        let replayed_manager = ProjectRuntimeManager::new(server, &host_paths).unwrap();
        let replayed_runtime = replayed_manager.select_runtime(&sender_context).unwrap();
        assert_eq!(replayed_runtime.storage_root, runtime_storage);
        assert_eq!(replayed_runtime.journal_path, runtime_journal);
        assert!(!Arc::ptr_eq(&replayed_runtime, &replayed_manager.host));

        let (selected, status) =
            replayed_manager.dispatch_sync(Some(sender_context.clone()), Req::StatusAll);
        assert!(status.ok, "{status:?}");
        assert!(Arc::ptr_eq(&selected, &replayed_runtime));
        assert_eq!(status.data["summary"]["workers"], 2);
        let (_, workers) =
            replayed_manager.dispatch_sync(Some(recipient_context.clone()), Req::Workers);
        assert!(workers.ok, "{workers:?}");
        assert_eq!(workers.data["count"], 2);
        let (_, task_status) = replayed_manager.dispatch_sync(
            Some(sender_context.clone()),
            Req::TaskStatus {
                task_id: Some("full-replay-task".into()),
            },
        );
        assert!(task_status.ok, "{task_status:?}");
        assert_eq!(task_status.data["owner"], sender_id);
        let (_, notification_status) = replayed_manager.dispatch_sync(
            Some(recipient_context.clone()),
            Req::NotificationStatus {
                worker_id: recipient_id.into(),
                token: recipient_token.into(),
            },
        );
        assert!(notification_status.ok, "{notification_status:?}");
        assert!(!notification_status.data["subscriptions"]
            .as_array()
            .unwrap()
            .is_empty());

        let (_, second_send) = replayed_manager.dispatch_sync(
            Some(sender_context),
            Req::Send {
                from: sender_id.into(),
                worker_id: Some(sender_id.into()),
                token: Some(sender_token.into()),
                command: Some(send_command("after-replay")),
                to: recipient_id.into(),
                mtype: "notify".into(),
                subject: Some("full replay after".into()),
                body: "message sent by the replayed runtime".into(),
                in_reply_to: None,
                reply_policy: None,
                target_session: None,
                delivery: "immediate".into(),
            },
        );
        assert!(second_send.ok, "{second_send:?}");
        let second_message_id = second_send.data["msg_id"].as_str().unwrap().to_owned();
        let (_, polled) = dispatch_wire_routed(
            replayed_manager.clone(),
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
        let polled_ids = polled.data["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|message| message["id"].as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(polled_ids.contains(first_message_id.as_str()));
        assert!(polled_ids.contains(second_message_id.as_str()));
        assert_eq!(
            std::fs::read(&route_journal).unwrap(),
            route_journal_before_replay
        );
        assert!(std::fs::read(&mailbox_path)
            .unwrap()
            .windows(second_message_id.len())
            .any(|window| window == second_message_id.as_bytes()));

        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn manager_app_scope_storage_encoding_is_collision_free_and_replayed() {
        let (server, host_root, _) = test_server();
        let project_root = host_root.with_file_name(format!(
            "{}-encoded-scope",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
        let canonical_project_root = project_root.canonicalize().unwrap();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();

        let register = |context: ProjectContext, worker_id: &str, token: &str| {
            let (runtime, response) = register_host_fixture(&manager,
                Some(context),
                Req::register(worker_id.into(),
                     token.into(),
                     project_root.display().to_string(),
                     test_candidates_at(&format!("thread-{worker_id}"), &project_root)),
            );
            assert!(response.ok, "registration {worker_id}: {response:?}");
            runtime
        };
        // A first route owns the project's legacy storage root. The two
        // colliding sanitized forms are deliberately later routes, where the
        // encoded namespace is selected.
        let seed_runtime = register(
            context_with_app(&project_root, "seed"),
            "encoded-seed-worker",
            "token-encoded-seed",
        );
        let slash_runtime = register(
            context_with_app(&project_root, "app/a"),
            "encoded-slash-worker",
            "token-encoded-slash",
        );
        let colon_runtime = register(
            context_with_app(&project_root, "app:a"),
            "encoded-colon-worker",
            "token-encoded-colon",
        );
        let project_scope = GlobalState::canonical_project_scope(&canonical_project_root).unwrap();
        let slash_storage = host_paths
            .app_storage_root(project_scope.as_str(), "app/a")
            .canonicalize()
            .unwrap();
        let colon_storage = host_paths
            .app_storage_root(project_scope.as_str(), "app:a")
            .canonicalize()
            .unwrap();
        assert_ne!(slash_storage, colon_storage);
        assert_eq!(slash_runtime.storage_root, slash_storage);
        assert_eq!(colon_runtime.storage_root, colon_storage);
        assert!(!Arc::ptr_eq(&slash_runtime, &colon_runtime));

        let routes = [
            (
                context_with_runtime(
                    &project_root,
                    "seed",
                    &runtime_for_registered(
                        &seed_runtime,
                        &project_root,
                        "encoded-seed-worker",
                        "seed",
                    ),
                ),
                "encoded-seed-worker",
                "token-encoded-seed",
            ),
            (
                context_with_runtime(
                    &project_root,
                    "app/a",
                    &runtime_for_registered(
                        &slash_runtime,
                        &project_root,
                        "encoded-slash-worker",
                        "app/a",
                    ),
                ),
                "encoded-slash-worker",
                "token-encoded-slash",
            ),
            (
                context_with_runtime(
                    &project_root,
                    "app:a",
                    &runtime_for_registered(
                        &colon_runtime,
                        &project_root,
                        "encoded-colon-worker",
                        "app:a",
                    ),
                ),
                "encoded-colon-worker",
                "token-encoded-colon",
            ),
        ];
        for (context, worker_id, token) in &routes {
            let (_selected, status) = manager.dispatch_sync(Some(context.clone()), Req::StatusAll);
            assert!(status.ok, "{worker_id} status: {status:?}");
            assert_eq!(status.data["summary"]["workers"], 1);
            assert_eq!(status.data["workers"][0]["id"], *worker_id);
            let (_, task) = manager.dispatch_sync(
                Some(context.clone()),
                Req::TaskRegister {
                    worker_id: (*worker_id).into(),
                    token: (*token).into(),
                    task_id: "encoded-scope-task".into(),
                    owner: None,
                    feature_id: Some(format!("feature-{worker_id}")),
                    worktree_path: None,
                    branch: None,
                    base_commit: None,
                    priority: "p1".into(),
                    next_step: Some("verify encoded scope replay".into()),
                    goal_prompt: None,
                },
            );
            assert!(task.ok, "{worker_id} task: {task:?}");
        }

        let route_journal = host_paths.state_root().join("routes.jsonl");
        let route_records = load_host_route_records(&route_journal).unwrap();
        assert_eq!(route_records.len(), 3);
        let storage_roots = route_records
            .iter()
            .map(|record| record.storage_root.clone())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(storage_roots.len(), 3);

        drop(seed_runtime);
        drop(slash_runtime);
        drop(colon_runtime);
        drop(manager);
        let replayed_manager = ProjectRuntimeManager::new(server, &host_paths).unwrap();
        for (context, worker_id, _token) in routes {
            let (selected, status) =
                replayed_manager.dispatch_sync(Some(context.clone()), Req::StatusAll);
            assert!(status.ok, "{worker_id} replay status: {status:?}");
            assert_eq!(status.data["summary"]["workers"], 1);
            assert_eq!(status.data["workers"][0]["id"], worker_id);
            assert_eq!(status.data["summary"]["tasks"], 1);
            assert!(Arc::ptr_eq(
                &selected,
                &replayed_manager.select_runtime(&context).unwrap()
            ));
        }

        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn prospective_storage_collision_rejects_register_before_route_publish() {
        use std::os::unix::fs::symlink;

        let (server, host_root, _) = test_server();
        let project_root = host_root.with_file_name(format!(
            "{}-prospective-collision",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
        let canonical_project_root = project_root.canonicalize().unwrap();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let existing_app_scope = "central-storage-owner";
        let candidate_app_scope = "abc";
        let project_scope = GlobalState::canonical_project_scope(&canonical_project_root).unwrap();
        let existing_storage_root =
            host_paths.app_storage_root(project_scope.as_str(), existing_app_scope);
        let candidate_storage_root =
            host_paths.app_storage_root(project_scope.as_str(), candidate_app_scope);
        std::fs::create_dir_all(existing_storage_root.parent().unwrap()).unwrap();
        symlink(&existing_storage_root, &candidate_storage_root).unwrap();
        let legacy_record = HostRouteRecord {
            version: 1,
            op: "register".into(),
            app_scope_id: existing_app_scope.into(),
            project_scope: project_scope.as_str().into(),
            canonical_root: project_scope.as_str().into(),
            storage_root: existing_storage_root.to_string_lossy().into_owned(),
            registered_ms: 1,
        };
        let legacy_line = serde_json::to_string(&legacy_record).unwrap();
        std::fs::write(&route_journal, format!("{legacy_line}\n")).unwrap();
        let before_collision = std::fs::read(&route_journal).unwrap();

        // The candidate app's actual central target aliases the existing
        // route owner. Startup must replay the valid owner, while the new
        // route must not share that storage target.
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let (_runtime, collision_response) = register_host_fixture(&manager,
            Some(context_with_app(&project_root, candidate_app_scope)),
            Req::register("prospective-collision-worker".into(),
                 "token-prospective-collision".into(),
                 project_root.display().to_string(),
                 test_candidates_at("thread-prospective-collision-worker", &project_root)),
        );
        assert!(!collision_response.ok, "{collision_response:?}");
        assert!(
            collision_response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("runtime storage root")),
            "{collision_response:?}"
        );
        assert_eq!(std::fs::read(&route_journal).unwrap(), before_collision);

        // The rejected prospective route must not poison the old route that
        // was successfully replayed from the durable journal.
        let legacy_context = context_with_app(&project_root, existing_app_scope);
        let (_runtime, registration_response) = register_host_fixture(&manager,
            Some(legacy_context.clone()),
            Req::register("legacy-replayed-worker".into(),
                 "token-legacy-replayed".into(),
                 project_root.display().to_string(),
                 test_candidates_at("thread-legacy-replayed-worker", &project_root)),
        );
        assert!(registration_response.ok, "{registration_response:?}");
        let (_runtime, status_response) =
            manager.dispatch_sync(Some(legacy_context), Req::StatusAll);
        assert!(status_response.ok, "{status_response:?}");
        assert_eq!(
            status_response.data["workers"][0]["id"],
            "legacy-replayed-worker"
        );
        assert_eq!(std::fs::read(&route_journal).unwrap(), before_collision);

        drop(manager);
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn resident_storage_collision_rejects_register_before_route_publish() {
        let (mut server, host_root, _) = test_server();
        let collision_project = host_root.with_file_name(format!(
            "{}-resident-collision-project",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(collision_project.join(".agent-collab/server")).unwrap();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let collision_context = context_with_app(&collision_project, "collision-app");
        let resident_storage_root = host_paths.app_storage_root(
            collision_context.project_scope.as_str(),
            collision_context.app_scope_id.as_str(),
        );
        let resident_server_dir = resident_storage_root.join(".agent-collab/server");
        std::fs::create_dir_all(&resident_server_dir).unwrap();
        let resident_storage_root = resident_storage_root.canonicalize().unwrap();
        let resident_journal_path = resident_server_dir.join("journal.jsonl");
        let resident_journal = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&resident_journal_path)
            .unwrap();
        {
            let host = Arc::get_mut(&mut server).unwrap();
            host.storage_root = resident_storage_root.clone();
            host.journal_path = resident_journal_path;
            host.journal = Mutex::new(resident_journal);
        }
        let route_journal = host_paths.state_root().join("routes.jsonl");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        std::fs::write(&route_journal, b"").unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();

        // First establish the resident route. Its reducer owns the storage
        // root and the canonical route is durably published.
        let resident_context = context_with_app(&host_root, "resident-app");
        let (_runtime, resident_response) = register_host_fixture(&manager,
            Some(resident_context.clone()),
            Req::register("resident-worker".into(),
                 "token-resident-worker".into(),
                 host_root.display().to_string(),
                 test_candidates("thread-resident-worker")),
        );
        assert!(resident_response.ok, "{resident_response:?}");
        let after_resident = std::fs::read(&route_journal).unwrap();
        let resident_records = load_host_route_records(&route_journal).unwrap();
        assert_eq!(resident_records.len(), 1);
        assert_eq!(resident_records[0].app_scope_id, "resident-app");
        assert_eq!(
            resident_records[0].canonical_root,
            host_root.canonicalize().unwrap().to_string_lossy()
        );

        // A second project whose storage root equals the resident reducer's
        // root must fail before a route record or second reducer is created.
        let (_runtime, collision_response) = register_host_fixture(&manager,
            Some(collision_context.clone()),
            Req::register("resident-collision-worker".into(),
                 "token-resident-collision".into(),
                 collision_project.display().to_string(),
                 test_candidates_at("thread-resident-collision-worker", &collision_project)),
        );
        assert!(!collision_response.ok, "{collision_response:?}");
        assert!(
            collision_response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("owned by resident host")),
            "{collision_response:?}"
        );
        assert_eq!(std::fs::read(&route_journal).unwrap(), after_resident);
        assert!(!manager
            .routes
            .lock()
            .unwrap()
            .contains_key(&ProjectRuntimeManager::route_key(&collision_context)));

        // The resident route remains the only owner and is still queryable.
        let (_runtime, status_response) =
            manager.dispatch_sync(Some(resident_context), Req::StatusAll);
        assert!(status_response.ok, "{status_response:?}");
        assert_eq!(status_response.data["workers"][0]["id"], "resident-worker");
        assert_eq!(std::fs::read(&route_journal).unwrap(), after_resident);

        drop(manager);
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(collision_project).unwrap();
    }

    #[tokio::test]
    async fn prospective_nested_project_collision_rejects_register_before_route_publish() {
        let (server, host_root, _) = test_server();
        let project_root = host_root.with_file_name(format!(
            "{}-nested-collision",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
        let canonical_project_root = project_root.canonicalize().unwrap();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();

        let (seed_runtime, seed_response) = register_host_fixture(&manager,
            Some(context_with_app(&project_root, "seed")),
            Req::register("nested-seed-worker".into(),
                 "token-nested-seed".into(),
                 project_root.display().to_string(),
                 test_candidates_at("thread-nested-seed-worker", &project_root)),
        );
        assert!(seed_response.ok, "{seed_response:?}");
        let (app_runtime, app_response) = register_host_fixture(&manager,
            Some(context_with_app(&project_root, "app/a")),
            Req::register("nested-app-worker".into(),
                 "token-nested-app".into(),
                 project_root.display().to_string(),
                 test_candidates_at("thread-nested-app-worker", &project_root)),
        );
        assert!(app_response.ok, "{app_response:?}");
        assert!(!Arc::ptr_eq(&seed_runtime, &app_runtime));

        let nested_root = host_paths
            .app_storage_root(
                GlobalState::canonical_project_scope(&canonical_project_root)
                    .unwrap()
                    .as_str(),
                "app/a",
            )
            .canonicalize()
            .unwrap();
        assert_eq!(app_runtime.root, canonical_project_root);
        assert_eq!(app_runtime.storage_root, nested_root);
        std::fs::create_dir_all(nested_root.join(".agent-collab/server")).unwrap();

        let route_journal = host_paths.state_root().join("routes.jsonl");
        let before_collision = std::fs::read(&route_journal).unwrap();
        let (_runtime, collision_response) = register_host_fixture(&manager,
            Some(context_with_app(&nested_root, "nested-app")),
            Req::register("nested-collision-worker".into(),
                 "token-nested-collision".into(),
                 nested_root.display().to_string(),
                 test_candidates_at("thread-nested-collision-worker", &nested_root)),
        );
        assert!(!collision_response.ok, "{collision_response:?}");
        assert!(
            collision_response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("runtime storage root")),
            "{collision_response:?}"
        );
        assert_eq!(std::fs::read(&route_journal).unwrap(), before_collision);

        // The existing app/a route remains routable after the prospective
        // nested project was rejected, and its route journal entry is intact.
        let (_runtime, status_response) = manager.dispatch_sync(
            Some(context_with_app(&project_root, "app/a")),
            Req::StatusAll,
        );
        assert!(status_response.ok, "{status_response:?}");
        assert_eq!(
            status_response.data["workers"][0]["id"],
            "nested-app-worker"
        );
        assert_eq!(std::fs::read(&route_journal).unwrap(), before_collision);

        drop(manager);
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn pending_runtime_initialization_is_one_arc_under_concurrent_dispatch() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server, &host_paths).unwrap();
        let context = context_with_app(&root, "pending-concurrent");
        let key = ProjectRuntimeManager::route_key(&context);
        let canonical_root = root.canonicalize().unwrap();
        let storage_root =
            host_paths.app_storage_root(context.project_scope.as_str(), context.app_scope_id.as_str());
        manager.install_pending_route(&key, &canonical_root, &storage_root);

        let first_manager = manager.clone();
        let first_context = context.clone();
        let second_manager = manager.clone();
        let second_context = context.clone();
        let (first, second) = tokio::join!(
            tokio::task::spawn_blocking(move || first_manager.select_runtime(&first_context)),
            tokio::task::spawn_blocking(move || second_manager.select_runtime(&second_context)),
        );
        let first = first.unwrap().unwrap();
        let second = second.unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let installed = manager
            .routes
            .lock()
            .unwrap()
            .get(&key)
            .and_then(|route| route.runtime.clone())
            .unwrap();
        assert!(Arc::ptr_eq(&first, &installed));
        assert_eq!(manager.runtimes().len(), 2);
        assert_eq!(
            first.journal_path,
            storage_root
                .canonicalize()
                .unwrap()
                .join(".agent-collab/server/journal.jsonl")
        );

        drop(first);
        drop(second);
        drop(manager);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn startup_materializes_a_durable_route_only_after_its_first_selection() {
        let (host, host_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let project_root = host_root.with_file_name(format!(
            "{}-pending",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
        let context = context_with_app(&project_root, "startup-pending-app");
        let route_journal = host_paths.state_root().join("routes.jsonl");
        let storage_root =
            host_paths.app_storage_root(context.project_scope.as_str(), context.app_scope_id.as_str());
        append_host_route_record(
            &route_journal,
            &HostRouteRecord {
                version: 1,
                op: "register".into(),
                app_scope_id: context.app_scope_id.as_str().into(),
                project_scope: context.project_scope.as_str().into(),
                canonical_root: context.canonical_root.clone(),
                storage_root: storage_root.to_string_lossy().into_owned(),
                registered_ms: 1,
            },
        )
        .unwrap();

        let manager = ProjectRuntimeManager::new(host, &host_paths).unwrap();
        let key = ProjectRuntimeManager::route_key(&context);
        assert!(
            manager.routes.lock().unwrap()[&key].runtime.is_none(),
            "an unused durable route must stay pending at startup"
        );
        assert!(
            manager.project_locks.lock().unwrap().is_empty(),
            "an unused route must not hold a writer-lock descriptor"
        );

        let selected = manager.select_runtime(&context).unwrap();
        assert!(Arc::ptr_eq(
            &selected,
            &manager.routes.lock().unwrap()[&key]
                .runtime
                .clone()
                .expect("selection materializes the route")
        ));
        assert_eq!(
            manager.project_locks.lock().unwrap().len(),
            1,
            "the first selection must acquire exactly one writer lock"
        );
        assert_eq!(
            selected.journal_path,
            storage_root
                .canonicalize()
                .unwrap()
                .join(".agent-collab/server/journal.jsonl")
        );

        drop(selected);
        drop(manager);
        std::fs::remove_dir_all(project_root).unwrap();
        std::fs::remove_dir_all(host_root).unwrap();
    }

    #[test]
    fn identity_context_prepare_bootstraps_the_empty_resident_route() {
        let (host, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let context = context_with_app(&root, crate::identity::CLI_APP_SERVER_ID);
        let request = crate::proto::IdentityContextRequest {
            operation_id: "ctxop-resident-bootstrap".into(),
            invocation: "automatic".into(),
            action: "prepare_invocation".into(),
            facts: crate::proto::IdentityFacts::default(),
            approval: None,
            grant_approval: None,
            query: false,
            query_capability: "resident-bootstrap-capability".into(),
            invocation_ticket: String::new(),
        };

        let (runtime, response) = manager.dispatch_sync(
            Some(context.clone()),
            Req::IdentityContext {
                facts: crate::proto::IdentityFacts::default(),
                identity_context: Some(request),
            },
        );
        assert!(response.ok, "{response:?}");
        assert!(Arc::ptr_eq(&runtime, &host));
        assert!(manager
            .routes
            .lock()
            .unwrap()
            .get(&ProjectRuntimeManager::route_key(&context))
            .and_then(|route| route.runtime.as_ref())
            .is_some_and(|runtime| Arc::ptr_eq(runtime, &host)));
        let records =
            load_host_route_records(&host_paths.state_root().join("routes.jsonl")).unwrap();
        assert_eq!(records.len(), 1);
        let canonical_root = root.canonicalize().unwrap().to_string_lossy().into_owned();
        assert_eq!(records[0].version, 1);
        assert_eq!(records[0].op, "register");
        assert_eq!(records[0].app_scope_id, crate::identity::CLI_APP_SERVER_ID);
        assert_eq!(records[0].project_scope, canonical_root);
        assert_eq!(records[0].canonical_root, canonical_root);
        assert_eq!(records[0].storage_root, canonical_root);

        drop(manager);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn identity_context_prepare_rejects_a_foreign_resident_storage_owner_without_corruption() {
        let (host, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let owner = context_with_app(&root, "resident-owner-app");
        manager.append_resident_route_record(&owner).unwrap();
        let request = crate::proto::IdentityContextRequest {
            operation_id: "ctxop-resident-storage-owner".into(),
            invocation: "automatic".into(),
            action: "prepare_invocation".into(),
            facts: crate::proto::IdentityFacts::default(),
            approval: None,
            grant_approval: None,
            query: false,
            query_capability: "resident-storage-owner-capability".into(),
            invocation_ticket: String::new(),
        };

        let (_runtime, response) = manager.dispatch_sync(
            Some(context_with_app(&root, crate::identity::CLI_APP_SERVER_ID)),
            Req::IdentityContext {
                facts: crate::proto::IdentityFacts::default(),
                identity_context: Some(request),
            },
        );
        assert!(!response.ok, "{response:?}");
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.contains("is already owned by")),
            "{response:?}"
        );
        let route_journal = host_paths.state_root().join("routes.jsonl");
        let records = load_host_route_records(&route_journal).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].app_scope_id, "resident-owner-app");
        assert_eq!(
            records[0].storage_root,
            root.canonicalize().unwrap().to_string_lossy()
        );

        ProjectRuntimeManager::new(host, &host_paths)
            .expect("the rejected bootstrap must leave the route journal replayable");
        drop(manager);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn route_publication_gate_serializes_resident_and_external_appends() {
        let (host, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let resident = context_with_app(&root, crate::identity::CLI_APP_SERVER_ID);
        let external_root = root.with_file_name(format!(
            "{}-route-publication",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(external_root.join(".agent-collab/server")).unwrap();
        let external = context_with_app(&external_root, "external-publication-app");
        let external_storage = external_root.canonicalize().unwrap();
        let gate = manager.register_gate.lock().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();

        let resident_manager = manager.clone();
        let resident_done = done_tx.clone();
        let resident_thread = std::thread::spawn(move || {
            let result = resident_manager.append_resident_route_record(&resident);
            resident_done.send(("resident", result)).unwrap();
        });
        let external_manager = manager.clone();
        let external_done = done_tx;
        let external_thread = std::thread::spawn(move || {
            let result =
                external_manager.append_route_record(&external, &external_storage);
            external_done.send(("external", result)).unwrap();
        });

        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "route publication must wait for the shared writer gate"
        );
        drop(gate);
        let first = done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the first route publication must finish after gate release");
        let second = done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the second route publication must finish after gate release");
        resident_thread.join().unwrap();
        external_thread.join().unwrap();
        assert!(first.1.is_ok(), "{first:?}");
        assert!(second.1.is_ok(), "{second:?}");

        let records =
            load_host_route_records(&host_paths.state_root().join("routes.jsonl")).unwrap();
        assert_eq!(records.len(), 2);
        assert!(records
            .iter()
            .any(|record| record.app_scope_id == crate::identity::CLI_APP_SERVER_ID));
        assert!(records
            .iter()
            .any(|record| record.app_scope_id == "external-publication-app"));

        drop(manager);
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(external_root).unwrap();
    }

    #[test]
    fn materialized_runtime_budget_rejects_the_route_above_the_cap() {
        assert!(
            materialized_runtime_budget_error(
                MAX_MATERIALIZED_EXTERNAL_RUNTIMES - 1,
                MAX_MATERIALIZED_EXTERNAL_RUNTIMES,
            )
            .is_none()
        );
        let error = materialized_runtime_budget_error(
            MAX_MATERIALIZED_EXTERNAL_RUNTIMES,
            MAX_MATERIALIZED_EXTERNAL_RUNTIMES,
        )
        .expect("the budget gate rejects the route at the cap");
        assert!(error.starts_with("PROJECT_ROUTE_RESOURCE_BUDGET_EXCEEDED:"), "{error}");
        assert!(error.contains(&MAX_MATERIALIZED_EXTERNAL_RUNTIMES.to_string()));
    }

    #[tokio::test]
    async fn concurrent_distinct_route_admission_respects_one_cap() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server, &host_paths).unwrap();
        manager
            .materialization_cap
            .store(1, std::sync::atomic::Ordering::Relaxed);
        manager
            .materialization_build_delay_ms
            .store(250, std::sync::atomic::Ordering::Relaxed);

        let mut projects = Vec::new();
        let mut contexts = Vec::new();
        for suffix in ["a", "b"] {
            let project_root = root.with_file_name(format!(
                "{}-budget-{suffix}",
                root.file_name().unwrap().to_string_lossy()
            ));
            std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
            let canonical_root = project_root.canonicalize().unwrap();
            let context = context_with_app(&project_root, "budget-app");
            manager.install_pending_route(
                &ProjectRuntimeManager::route_key(&context),
                &canonical_root,
                &canonical_root,
            );
            projects.push(project_root);
            contexts.push(context);
        }

        let start = Arc::new(std::sync::Barrier::new(3));
        let mut tasks = Vec::new();
        for context in contexts {
            let manager = manager.clone();
            let start = start.clone();
            tasks.push(tokio::task::spawn_blocking(move || {
                start.wait();
                manager.select_runtime(&context)
            }));
        }
        start.wait();
        let mut admitted = 0;
        for task in tasks {
            if task.await.unwrap().is_ok() {
                admitted += 1;
            }
        }
        assert_eq!(admitted, 1, "only one route may pass admission at the cap");
        assert_eq!(
            manager.runtimes().len(),
            2,
            "the host plus exactly one external reducer"
        );

        drop(manager);
        for project_root in projects {
            std::fs::remove_dir_all(project_root).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn startup_recovers_committed_project_binding_missing_from_host_index() {
        let (host, host_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let seed_manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();

        let project_root = host_root.with_file_name(format!(
            "{}-split-journal",
            host_root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(project_root.join(".agent-collab/server")).unwrap();
        let canonical_root = project_root.canonicalize().unwrap();
        let context = context_with_app(&project_root, "split-journal-app");
        let key = ProjectRuntimeManager::route_key(&context);
        let storage_root =
            host_paths.app_storage_root(context.project_scope.as_str(), context.app_scope_id.as_str());
        seed_manager.install_pending_route(&key, &canonical_root, &storage_root);
        let runtime = seed_manager.select_runtime(&context).unwrap();
        let binding = commit_tmux_typed_fixture(
            &runtime,
            "split-journal-worker",
            "token-split-journal",
            context.app_scope_id.clone(),
            committed_tmux_endpoint(&project_root, "split-journal-thread"),
        );
        drop(runtime);
        drop(seed_manager);

        // The central project commit is durable, but this host journal never
        // received the matching current-thread publication.
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        append_host_route_record(
            &route_journal,
            &HostRouteRecord {
                version: 1,
                op: "register".into(),
                app_scope_id: context.app_scope_id.as_str().into(),
                project_scope: context.project_scope.as_str().into(),
                canonical_root: context.canonical_root.clone(),
                storage_root: storage_root.display().to_string(),
                registered_ms: 1,
            },
        )
        .unwrap();

        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        assert!(
            manager.routes.lock().unwrap()[&key].runtime.is_some(),
            "a committed project binding missing from the host index must materialize"
        );
        let published = {
            let session_id = binding.session_id.clone().unwrap();
            let native_thread_id = binding.native_thread_id.clone().unwrap();
            host.state
                .lock()
                .unwrap()
                .global
                .lookup_current_thread_route(&session_id, &native_thread_id)
                .cloned()
        };
        assert_eq!(
            published,
            Some(binding),
            "startup must republish the committed project binding"
        );

        drop(manager);
        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    #[tokio::test]
    async fn empty_registry_admits_only_exact_cli_host_operator_shutdown() {
        // Shutdown authorization reads the live `~/.appsdk/config.toml`. Do not
        // override HOME here: that mutates process-wide state and races every
        // parallel test that resolves its own config or state paths. Instead
        // adapt to the configured password (or its absence) so the assertions
        // stay deterministic on any host. The exact-reject/accept matching is
        // covered by the bind-free `password_shutdown_tests` and by the
        // `lifecycle_boundary_cli` fixtures, which configure HOME per child.
        let configured = crate::config::maintenance_password_opt().unwrap();
        let accepted_password = configured
            .as_ref()
            .map(|value| crate::proto::MaintenancePassword::new(value.clone()));

        let (server, root, journal_path) = test_server();
        let resident_context = context_with_app(&root, crate::identity::CLI_APP_SERVER_ID);
        let before_journal = std::fs::read(&journal_path).unwrap();

        std::fs::create_dir_all(server.host_paths.state_root()).unwrap();

        let response = dispatch_wire(
            server.clone(),
            Some(resident_context.clone()),
            Req::Shutdown { password: accepted_password.clone() },
        )
        .await;
        assert!(response.ok, "{response:?}");
        {
            let state = server.state.lock().unwrap();
            assert!(state.workers.is_empty());
            assert!(state.global.projects.is_empty());
            assert_eq!(state.revision, 0);
            assert_eq!(state.sequence, 0);
        }
        assert_eq!(std::fs::read(&journal_path).unwrap(), before_journal);

        let wrong_root = root.with_file_name(format!(
            "{}-wrong-root",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&wrong_root).unwrap();
        let wrong_root_error = validate_request_context(
            &server,
            &Req::Shutdown { password: accepted_password.clone() },
            Some(&context_with_app(
                &wrong_root,
                crate::identity::CLI_APP_SERVER_ID,
            )),
        )
        .unwrap_err();
        assert!(wrong_root_error.starts_with("PROJECT_SCOPE_UNKNOWN:"));

        let wrong_app_error = validate_request_context(
            &server,
            &Req::Shutdown { password: accepted_password.clone() },
            Some(&context_with_app(&root, "other-app")),
        )
        .unwrap_err();
        assert!(wrong_app_error.starts_with("PROJECT_SCOPE_UNKNOWN:"));

        if configured.is_some() {
            // A configured password must not be bypassed by omitting it.
            let non_operator_error = validate_request_context(
                &server,
                &Req::Shutdown { password: None },
                Some(&resident_context),
            )
            .unwrap_err();
            assert!(non_operator_error.starts_with("MAINTENANCE_AUTHENTICATION_FAILED"));
        }

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(wrong_root).unwrap();
    }

    #[tokio::test]
    async fn external_project_register_is_admitted_and_replayed_from_host_journal() {
        let (server, root, _) = test_server();
        let external_root = root.with_file_name(format!(
            "{}-external",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(external_root.join(".agent-collab").join("server")).unwrap();
        let context = context_with_app(&external_root, "routecodex-app");
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();

        let (runtime, response) = register_host_fixture(&manager,
            Some(context.clone()),
            Req::register("routecodex-master".into(),
                 "token-routecodex-master".into(),
                 external_root.display().to_string(),
                 test_candidates_at("thread-routecodex-master", &external_root)),
        );
        assert!(response.ok, "{response:?}");

        let project_scope = GlobalState::canonical_project_scope(&external_root).unwrap();
        let state = runtime.state.lock().unwrap();
        let project = state.global.lookup_project(&project_scope).unwrap();
        assert!(project
            .lookup_registration(&AppServerId::new("routecodex-app").unwrap())
            .is_some());
        assert!(project
            .lookup_binding(&BindingId::new("binding-routecodex-master").unwrap())
            .is_some());
        drop(state);

        // A route that is not owned by this resident reducer may still be
        // re-registered through the same explicit route. Reconnect remains
        // idempotent and does not create a second registration or binding.
        let runtime_identity = runtime_for_registered(
            &runtime,
            &external_root,
            "routecodex-master",
            "routecodex-app",
        );
        let (_, repeated) = manager.dispatch_sync(
            Some(context_with_runtime(
                &external_root,
                "routecodex-app",
                &runtime_identity,
            )),
            Req::register("routecodex-master".into(),
                 "token-routecodex-master".into(),
                 external_root.display().to_string(),
                 test_candidates_for_registered(
                    &runtime,
                    &external_root,
                    "routecodex-master",
                    "routecodex-app",
                )),
        );
        assert!(repeated.ok, "{repeated:?}");
        assert_eq!(repeated.data["replayed"], true);

        // The host route journal is the durable route index. The external
        // runtime owns the project's reducer journal and replays that state.
        let route_journal = host_paths.state_root().join("routes.jsonl");
        let route_records = load_host_route_records(&route_journal).unwrap();
        assert!(route_records.iter().any(|record| {
            record.app_scope_id == "routecodex-app"
                && record.project_scope == project_scope.as_str()
        }));
        let replayed_external = replay(&runtime.storage_root).unwrap();
        let replayed_project = replayed_external
            .global
            .lookup_project(&project_scope)
            .expect("external runtime replay must retain the project route");
        assert!(replayed_project
            .lookup_registration(&AppServerId::new("routecodex-app").unwrap())
            .is_some());

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(external_root).unwrap();
    }

    #[tokio::test]
    async fn uninitialized_external_project_cannot_create_host_route() {
        let (server, root, journal_path) = test_server();
        let external_root = root.with_file_name(format!(
            "{}-uninitialized",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&external_root).unwrap();
        let context = context_with_app(&external_root, "uninitialized-app");
        let response = dispatch_wire(
            server.clone(),
            Some(context),
            Req::register("uninitialized-worker".into(),
                 "token-uninitialized-worker".into(),
                 external_root.display().to_string(),
                 test_candidates("thread-uninitialized-worker")),
        )
        .await;
        assert!(!response.ok, "{response:?}");
        assert!(response
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("PROJECT_SCOPE_UNKNOWN:")));
        assert!(server.state.lock().unwrap().global.projects.is_empty());
        assert!(std::fs::read(&journal_path).unwrap().is_empty());

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(external_root).unwrap();
    }

    #[tokio::test]
    async fn external_project_register_requires_exact_registered_root() {
        let (server, root, journal_path) = test_server();
        let external_root = root.with_file_name(format!(
            "{}-exact-root",
            root.file_name().unwrap().to_string_lossy()
        ));
        let child_root = external_root.join("child");
        std::fs::create_dir_all(external_root.join(".agent-collab").join("server")).unwrap();
        std::fs::create_dir_all(child_root.join(".agent-collab").join("server")).unwrap();
        let context = context_with_app(&external_root, "exact-root-app");
        let response = dispatch_wire(
            server.clone(),
            Some(context),
            Req::register("wrong-cwd-worker".into(),
                 "token-wrong-cwd-worker".into(),
                 child_root.display().to_string(),
                 test_candidates("thread-wrong-cwd-worker")),
        )
        .await;
        assert!(!response.ok, "{response:?}");
        assert!(response
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("PROJECT_SCOPE_MISMATCH:")));
        assert!(server.state.lock().unwrap().global.projects.is_empty());
        assert!(std::fs::read(&journal_path).unwrap().is_empty());

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(external_root).unwrap();
    }
