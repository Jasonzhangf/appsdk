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
            let (runtime, response) = manager.dispatch_sync(
                Some(context),
                Req::Register {
                    worker_id: worker_id.into(),
                    token: token.into(),
                    cwd: if worker_id == "app-a-worker" || worker_id == "app-b-worker" {
                        project_a.display().to_string()
                    } else {
                        project_b.display().to_string()
                    },
                    candidates: test_candidates(&format!("thread-{worker_id}")),
                },
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
        assert!(
            app_a_runtime.storage_root.starts_with(&canonical_project_a),
            "app-a storage {} is outside project {}",
            app_a_runtime.storage_root.display(),
            canonical_project_a.display()
        );
        assert!(app_b_runtime
            .storage_root
            .starts_with(canonical_project_a.join(".agent-collab/server/runtimes")));
        assert!(project_b_runtime
            .storage_root
            .starts_with(&canonical_project_b));

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
                delivery: "immediate".into(),
            },
        );
        assert!(!cross_route_send.1.ok, "{:?}", cross_route_send.1);
        assert!(cross_route_send
            .1
            .error
            .as_deref()
            .is_some_and(|error| error.contains("recipient app-b-worker not registered")));

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
    async fn manager_cross_project_tmux_masters_send_and_reject_forged_source_evidence() {
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
        let (source_runtime, source_registration) = manager.dispatch_sync(
            Some(context_with_app(&project_a, app_a)),
            Req::Register {
                worker_id: master_a.into(),
                token: token_a.into(),
                cwd: project_a.display().to_string(),
                candidates: test_candidates_at("thread-a", &project_a),
            },
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

        let (target_runtime, target_registration) = manager.dispatch_sync(
            Some(context_with_app(&project_b, app_b)),
            Req::Register {
                worker_id: master_b.into(),
                token: token_b.into(),
                cwd: project_b.display().to_string(),
                candidates: test_candidates_at("thread-b", &project_b),
            },
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
        assert_eq!(delivered.data["notification"], "tmux-input-submitted");
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

        let (sender_runtime, sender_registration) = manager.dispatch_sync(
            Some(context_with_app(&project_root, app_scope)),
            Req::Register {
                worker_id: sender_id.into(),
                token: sender_token.into(),
                cwd: project_root.display().to_string(),
                candidates: test_candidates("thread-full-replay-sender"),
            },
        );
        assert!(sender_registration.ok, "{sender_registration:?}");
        let (recipient_runtime, recipient_registration) = manager.dispatch_sync(
            Some(context_with_app(&project_root, app_scope)),
            Req::Register {
                worker_id: recipient_id.into(),
                token: recipient_token.into(),
                cwd: project_root.display().to_string(),
                candidates: test_candidates("thread-full-replay-recipient"),
            },
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
            let (runtime, response) = manager.dispatch_sync(
                Some(context),
                Req::Register {
                    worker_id: worker_id.into(),
                    token: token.into(),
                    cwd: project_root.display().to_string(),
                    candidates: test_candidates(&format!("thread-{worker_id}")),
                },
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
        let slash_storage = app_scope_storage_path(&canonical_project_root, "app/a");
        let colon_storage = app_scope_storage_path(&canonical_project_root, "app:a");
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

    #[tokio::test]
    async fn prospective_storage_collision_rejects_register_before_route_publish() {
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
        let legacy_app_scope = "v1-3-616263";
        let candidate_app_scope = "abc";
        let legacy_storage_root = canonical_project_root
            .join(".agent-collab/server/runtimes")
            .join(legacy_app_scope);
        assert_eq!(
            legacy_storage_root,
            app_scope_storage_path(&canonical_project_root, candidate_app_scope)
        );
        let project_scope = GlobalState::canonical_project_scope(&canonical_project_root).unwrap();
        let legacy_record = HostRouteRecord {
            version: 1,
            op: "register".into(),
            app_scope_id: legacy_app_scope.into(),
            project_scope: project_scope.as_str().into(),
            canonical_root: project_scope.as_str().into(),
            storage_root: legacy_storage_root.to_string_lossy().into_owned(),
            registered_ms: 1,
        };
        let legacy_line = serde_json::to_string(&legacy_record).unwrap();
        std::fs::write(&route_journal, format!("{legacy_line}\n")).unwrap();
        let before_collision = std::fs::read(&route_journal).unwrap();

        // The existing record uses the old lossy path convention. Startup
        // must replay it, while a new encoded route must be rejected before
        // the host route journal is replaced.
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let (_runtime, collision_response) = manager.dispatch_sync(
            Some(context_with_app(&project_root, candidate_app_scope)),
            Req::Register {
                worker_id: "prospective-collision-worker".into(),
                token: "token-prospective-collision".into(),
                cwd: project_root.display().to_string(),
                candidates: test_candidates("thread-prospective-collision-worker"),
            },
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
        let legacy_context = context_with_app(&project_root, legacy_app_scope);
        let (_runtime, registration_response) = manager.dispatch_sync(
            Some(legacy_context.clone()),
            Req::Register {
                worker_id: "legacy-replayed-worker".into(),
                token: "token-legacy-replayed".into(),
                cwd: project_root.display().to_string(),
                candidates: test_candidates("thread-legacy-replayed-worker"),
            },
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
        let resident_storage_root = host_root.with_file_name(format!(
            "{}-resident-storage",
            host_root.file_name().unwrap().to_string_lossy()
        ));
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
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let route_journal = host_paths.state_root().join("routes.jsonl");
        std::fs::create_dir_all(host_paths.state_root()).unwrap();
        std::fs::write(&route_journal, b"").unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();

        // First establish the resident route. Its reducer owns the storage
        // root and the canonical route is durably published.
        let resident_context = context_with_app(&host_root, "resident-app");
        let (_runtime, resident_response) = manager.dispatch_sync(
            Some(resident_context.clone()),
            Req::Register {
                worker_id: "resident-worker".into(),
                token: "token-resident-worker".into(),
                cwd: host_root.display().to_string(),
                candidates: test_candidates("thread-resident-worker"),
            },
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
        let collision_context = context_with_app(&resident_storage_root, "collision-app");
        let (_runtime, collision_response) = manager.dispatch_sync(
            Some(collision_context.clone()),
            Req::Register {
                worker_id: "resident-collision-worker".into(),
                token: "token-resident-collision".into(),
                cwd: resident_storage_root.display().to_string(),
                candidates: test_candidates("thread-resident-collision-worker"),
            },
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
        std::fs::remove_dir_all(resident_storage_root).unwrap();
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

        let (seed_runtime, seed_response) = manager.dispatch_sync(
            Some(context_with_app(&project_root, "seed")),
            Req::Register {
                worker_id: "nested-seed-worker".into(),
                token: "token-nested-seed".into(),
                cwd: project_root.display().to_string(),
                candidates: test_candidates("thread-nested-seed-worker"),
            },
        );
        assert!(seed_response.ok, "{seed_response:?}");
        let (app_runtime, app_response) = manager.dispatch_sync(
            Some(context_with_app(&project_root, "app/a")),
            Req::Register {
                worker_id: "nested-app-worker".into(),
                token: "token-nested-app".into(),
                cwd: project_root.display().to_string(),
                candidates: test_candidates("thread-nested-app-worker"),
            },
        );
        assert!(app_response.ok, "{app_response:?}");
        assert!(!Arc::ptr_eq(&seed_runtime, &app_runtime));

        let nested_root = app_scope_storage_path(&canonical_project_root, "app/a");
        assert_eq!(app_runtime.root, canonical_project_root);
        assert_eq!(app_runtime.storage_root, nested_root);
        std::fs::create_dir_all(nested_root.join(".agent-collab/server")).unwrap();

        let route_journal = host_paths.state_root().join("routes.jsonl");
        let before_collision = std::fs::read(&route_journal).unwrap();
        let (_runtime, collision_response) = manager.dispatch_sync(
            Some(context_with_app(&nested_root, "nested-app")),
            Req::Register {
                worker_id: "nested-collision-worker".into(),
                token: "token-nested-collision".into(),
                cwd: nested_root.display().to_string(),
                candidates: test_candidates("thread-nested-collision-worker"),
            },
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
        manager.install_pending_route(&key, &canonical_root, &canonical_root);

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
            canonical_root.join(".agent-collab/server/journal.jsonl")
        );

        drop(first);
        drop(second);
        drop(manager);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn empty_registry_admits_only_exact_cli_host_operator_shutdown() {
        let (server, root, journal_path) = test_server();
        let resident_context = context_with_app(&root, crate::identity::CLI_APP_SERVER_ID);
        let before_journal = std::fs::read(&journal_path).unwrap();

        let response = dispatch_wire(
            server.clone(),
            Some(resident_context.clone()),
            Req::Shutdown { operator: true },
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
            &Req::Shutdown { operator: true },
            Some(&context_with_app(
                &wrong_root,
                crate::identity::CLI_APP_SERVER_ID,
            )),
        )
        .unwrap_err();
        assert!(wrong_root_error.starts_with("PROJECT_SCOPE_UNKNOWN:"));

        let wrong_app_error = validate_request_context(
            &server,
            &Req::Shutdown { operator: true },
            Some(&context_with_app(&root, "other-app")),
        )
        .unwrap_err();
        assert!(wrong_app_error.starts_with("PROJECT_SCOPE_UNKNOWN:"));

        let non_operator_error = validate_request_context(
            &server,
            &Req::Shutdown { operator: false },
            Some(&resident_context),
        )
        .unwrap_err();
        assert!(non_operator_error.starts_with("PROJECT_SCOPE_UNKNOWN:"));

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

        let (runtime, response) = manager.dispatch_sync(
            Some(context.clone()),
            Req::Register {
                worker_id: "routecodex-master".into(),
                token: "token-routecodex-master".into(),
                cwd: external_root.display().to_string(),
                candidates: test_candidates("thread-routecodex-master"),
            },
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
            Req::Register {
                worker_id: "routecodex-master".into(),
                token: "token-routecodex-master".into(),
                cwd: external_root.display().to_string(),
                candidates: test_candidates_for_registered(
                    &runtime,
                    &external_root,
                    "routecodex-master",
                    "routecodex-app",
                ),
            },
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
        let replayed_external = replay(&external_root).unwrap();
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
            Req::Register {
                worker_id: "uninitialized-worker".into(),
                token: "token-uninitialized-worker".into(),
                cwd: external_root.display().to_string(),
                candidates: test_candidates("thread-uninitialized-worker"),
            },
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
            Req::Register {
                worker_id: "wrong-cwd-worker".into(),
                token: "token-wrong-cwd-worker".into(),
                cwd: child_root.display().to_string(),
                candidates: test_candidates("thread-wrong-cwd-worker"),
            },
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
