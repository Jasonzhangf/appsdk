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

    /// C1/C2: a host pane-route index entry whose binding is missing from the
    /// owning project runtime is a STALE INDEX, and the identity gate must treat
    /// it as "no usable route evidence" instead of aborting the bootstrap.
    #[tokio::test]
    async fn stale_pane_route_index_is_named_and_the_identity_gate_bootstraps_over_it() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
        let project_scope = GlobalState::canonical_project_scope(&project_root).unwrap();
        let candidates = test_candidates("stale-index-pane").unwrap();
        let endpoint = candidates.tmux.as_ref().unwrap().endpoint.clone();

        // The route table needs a runtime, but the target pane must stay free so
        // the bootstrap can register a fresh provisional runtime for it.
        let other_candidates = test_candidates("stale-index-other").unwrap();
        let registered = handle_register_with_app_scope(
            &runtime,
            "stale-index-owner".into(),
            "token-stale-index-owner".into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(other_candidates),
        );
        assert!(registered.ok, "{registered:?}");
        manager.install_runtime(
            &(app.as_str().to_owned(), project_scope.as_str().to_owned()),
            runtime.clone(),
            None,
        );

        // The host index keeps an entry for this pane that the runtime never had.
        let mut ghost = RuntimeBinding::new_with_session(
            project_scope.clone(),
            app.clone(),
            AgentId::new("ghost-agent").unwrap(),
            RuntimeId::new("runtime-ghost").unwrap(),
            BindingId::new("binding-ghost").unwrap(),
            1,
            Some(
                crate::identity::SessionId::new(
                    endpoint.codex_session_id.clone().expect("test session id"),
                )
                .unwrap(),
            ),
            Some(
                NativeThreadId::new(endpoint.codex_thread_id.clone().expect("test thread id"))
                    .unwrap(),
            ),
        )
        .unwrap();
        ghost.tmux_endpoint = Some(endpoint.clone());
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet { binding: ghost }])
            .unwrap();

        let error = manager.resolve_route_by_tmux_endpoint(&endpoint).unwrap_err();
        assert!(error.starts_with("ROUTE_RESOLVE_STALE_INDEX:"), "{error}");
        assert!(error.contains("missing runtime binding"), "{error}");

        let context = context_with_app(&project_root, crate::identity::CLI_APP_SERVER_ID);
        let (_, response) = manager.dispatch_sync(
            Some(context),
            Req::IdentityContext {
                facts: crate::proto::IdentityFacts {
                    tmux: candidates.tmux.clone(),
                    ..Default::default()
                },
            },
        );
        assert!(response.ok, "{response:?}");

        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    fn write_runtime_less_identity(
        host_paths: &HostPaths,
        worker_id: &str,
        token: &str,
        project_scope: &crate::scope::ProjectScopeId,
    ) {
        let path = host_paths
            .state_root()
            .join("identities")
            .join(worker_id)
            .join("identity.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let identity = serde_json::json!({
            "worker_id": worker_id,
            "token": token,
            "project_scope": project_scope,
        });
        std::fs::write(path, serde_json::to_string_pretty(&identity).unwrap()).unwrap();
    }

    fn persisted_identity_token(host_paths: &HostPaths, worker_id: &str) -> String {
        let path = host_paths
            .state_root()
            .join("identities")
            .join(worker_id)
            .join("identity.json");
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        value["token"].as_str().unwrap().to_owned()
    }

    /// C5: a runtime-less draft must not stop the daemon from recovering the
    /// committed credential for the proven anchor. Before the fix the guard
    /// early-returns on "a file exists" and Register fails with TOKEN_MISMATCH.
    #[tokio::test]
    async fn runtime_less_draft_recovers_the_committed_credential() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
        let project_scope = GlobalState::canonical_project_scope(&project_root).unwrap();
        let candidates = test_candidates("runtime-less-draft-committed").unwrap();
        let endpoint = candidates.tmux.as_ref().unwrap().endpoint.clone();
        let worker = format!("codex-{}", endpoint.pane_id);
        let committed_token = "token-committed-runtime-less";
        let draft_token = "token-draft-runtime-less";

        let registered = handle_register_with_app_scope(
            &runtime,
            worker.clone(),
            committed_token.into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(candidates.clone()),
        );
        assert!(registered.ok, "{registered:?}");
        let binding_id = BindingId::new(sanitize_identifier(&format!("binding-{worker}"))).unwrap();
        let binding = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &crate::server::global_state::RouteScope {
                    app_scope_id: app.clone(),
                    project_scope_id: project_scope.clone(),
                },
                &binding_id,
            )
            .cloned()
            .unwrap();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: binding.clone(),
        }])
        .unwrap();
        manager.install_runtime(
            &(app.as_str().to_owned(), project_scope.as_str().to_owned()),
            runtime.clone(),
            None,
        );
        write_runtime_less_identity(&host_paths, &worker, draft_token, &project_scope);

        let context = context_with_app(&project_root, crate::identity::CLI_APP_SERVER_ID);
        let (_, response) = manager.dispatch_sync(
            Some(context),
            Req::IdentityContext {
                facts: crate::proto::IdentityFacts {
                    tmux: candidates.tmux.clone(),
                    ..Default::default()
                },
            },
        );
        assert!(response.ok, "{response:?}");
        assert_eq!(
            persisted_identity_token(&host_paths, &worker),
            committed_token,
            "the committed credential must replace the runtime-less draft"
        );

        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    /// C5 regression: without a committed record for the anchor the draft keeps
    /// its exact token. The daemon must never remint a stored credential.
    #[tokio::test]
    async fn runtime_less_draft_keeps_its_token_without_a_committed_record() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
        let project_scope = GlobalState::canonical_project_scope(&project_root).unwrap();
        let candidates = test_candidates("runtime-less-draft-fresh").unwrap();
        let endpoint = candidates.tmux.as_ref().unwrap().endpoint.clone();
        let worker = format!("codex-{}", endpoint.pane_id);
        let draft_token = "token-draft-kept";

        // A different peer owns a different pane route; the draft's own name has
        // no committed record anywhere.
        let other_candidates = test_candidates("runtime-less-draft-other").unwrap();
        let registered = handle_register_with_app_scope(
            &runtime,
            "runtime-less-draft-other-owner".into(),
            "token-runtime-less-draft-other-owner".into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(other_candidates),
        );
        assert!(registered.ok, "{registered:?}");
        let other_binding_id = BindingId::new(sanitize_identifier(
            "binding-runtime-less-draft-other-owner",
        ))
        .unwrap();
        let other_binding = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &crate::server::global_state::RouteScope {
                    app_scope_id: app.clone(),
                    project_scope_id: project_scope.clone(),
                },
                &other_binding_id,
            )
            .cloned()
            .unwrap();
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: other_binding,
        }])
        .unwrap();
        manager.install_runtime(
            &(app.as_str().to_owned(), project_scope.as_str().to_owned()),
            runtime.clone(),
            None,
        );
        write_runtime_less_identity(&host_paths, &worker, draft_token, &project_scope);

        let context = context_with_app(&project_root, crate::identity::CLI_APP_SERVER_ID);
        let (_, response) = manager.dispatch_sync(
            Some(context),
            Req::IdentityContext {
                facts: crate::proto::IdentityFacts {
                    tmux: candidates.tmux.clone(),
                    ..Default::default()
                },
            },
        );
        assert!(response.ok, "{response:?}");
        assert_eq!(
            persisted_identity_token(&host_paths, &worker),
            draft_token,
            "a draft without a committed record must keep its exact token"
        );

        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    /// C4 boundary: a host index entry that disagrees with its runtime binding
    /// is a REAL conflict, not a stale index. It must stay fatal so the identity
    /// gate keeps failing closed instead of overwriting the committed state.
    #[tokio::test]
    async fn conflicting_pane_route_index_stays_fatal_for_the_identity_gate() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
        let project_scope = GlobalState::canonical_project_scope(&project_root).unwrap();
        let candidates = test_candidates("conflicting-index-pane").unwrap();
        let endpoint = candidates.tmux.as_ref().unwrap().endpoint.clone();
        let worker = format!("codex-{}", endpoint.pane_id);

        let registered = handle_register_with_app_scope(
            &runtime,
            worker.clone(),
            "token-conflicting-index".into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(candidates.clone()),
        );
        assert!(registered.ok, "{registered:?}");
        manager.install_runtime(
            &(app.as_str().to_owned(), project_scope.as_str().to_owned()),
            runtime.clone(),
            None,
        );
        let binding_id = BindingId::new(sanitize_identifier(&format!("binding-{worker}"))).unwrap();
        let route_scope = crate::server::global_state::RouteScope {
            app_scope_id: app.clone(),
            project_scope_id: project_scope.clone(),
        };
        let committed = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(&route_scope, &binding_id)
            .cloned()
            .unwrap();
        let mut conflicting = committed.clone();
        conflicting.endpoint_generation += 1;
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: conflicting,
        }])
        .unwrap();

        let error = manager.resolve_route_by_tmux_endpoint(&endpoint).unwrap_err();
        assert!(error.starts_with("ROUTE_RESOLVE_INVALID:"), "{error}");
        assert!(error.contains("conflicts with its runtime binding"), "{error}");

        let context = context_with_app(&project_root, crate::identity::CLI_APP_SERVER_ID);
        let (_, response) = manager.dispatch_sync(
            Some(context),
            Req::IdentityContext {
                facts: crate::proto::IdentityFacts {
                    tmux: candidates.tmux.clone(),
                    ..Default::default()
                },
            },
        );
        assert!(!response.ok, "{response:?}");
        let message = response.error.clone().unwrap_or_default();
        assert!(message.starts_with("ROUTE_RESOLVE_INVALID"), "{message}");

        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }

    /// D3: an orphan host-index claim whose generation is higher than the
    /// fresh registration must not block the identity gate.
    ///
    /// The host index can keep a route for a binding its reducer holds no
    /// runtime binding for. Nothing can resolve that route, so the daemon takes
    /// the pane by default: one `collab context` recovers, with no second
    /// command. Before the orphan rule the write path failed with StaleBinding
    /// and the daemon could not restart afterwards.
    #[tokio::test]
    async fn an_orphan_stale_index_claim_does_not_block_the_identity_gate_takeover() {
        let (host, host_root, _) = test_server();
        let (runtime, project_root, _) = test_server();
        let host_paths = HostPaths::for_state_root(host_root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(host.clone(), &host_paths).unwrap();
        let app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
        let project_scope = GlobalState::canonical_project_scope(&project_root).unwrap();

        let candidates = test_candidates("orphan-takeover").unwrap();
        let endpoint = candidates.tmux.as_ref().unwrap().endpoint.clone();
        let worker = format!("codex-{}", endpoint.pane_id);
        let binding_id =
            BindingId::new(sanitize_identifier(&format!("binding-{worker}"))).unwrap();

        // The route table needs a runtime, but this pane must stay free so the
        // bootstrap can register a fresh provisional runtime for it.
        let other_candidates = test_candidates("orphan-takeover-other").unwrap();
        let registered = handle_register_with_app_scope(
            &runtime,
            "orphan-takeover-owner".into(),
            "token-orphan-takeover-owner".into(),
            project_root.display().to_string(),
            Some(app.clone()),
            Some(other_candidates),
        );
        assert!(registered.ok, "{registered:?}");
        manager.install_runtime(
            &(app.as_str().to_owned(), project_scope.as_str().to_owned()),
            runtime.clone(),
            None,
        );

        // The host index claims this pane for the SAME principal the fresh
        // registration will use, at a generation the runtime never had. The
        // runtime holds no such binding, so this claim is an orphan.
        let mut orphan = RuntimeBinding::new_with_session(
            project_scope.clone(),
            app.clone(),
            AgentId::new(&worker).unwrap(),
            RuntimeId::new("runtime-orphan").unwrap(),
            binding_id.clone(),
            7,
            Some(
                crate::identity::SessionId::new(
                    endpoint.codex_session_id.clone().expect("test session id"),
                )
                .unwrap(),
            ),
            Some(
                NativeThreadId::new(endpoint.codex_thread_id.clone().expect("test thread id"))
                    .unwrap(),
            ),
        )
        .unwrap();
        orphan.tmux_endpoint = Some(endpoint.clone());
        host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: orphan.clone(),
        }])
        .unwrap();
        assert!(
            runtime
                .state
                .lock()
                .unwrap()
                .global
                .lookup_binding_for(&orphan.route_scope(), &binding_id)
                .is_none(),
            "the runtime must hold no binding for the orphan claim"
        );

        // One gate call must recover: it takes the pane instead of failing.
        let context = context_with_app(&project_root, crate::identity::CLI_APP_SERVER_ID);
        let (_, response) = manager.dispatch_sync(
            Some(context),
            Req::IdentityContext {
                facts: crate::proto::IdentityFacts {
                    tmux: candidates.tmux.clone(),
                    ..Default::default()
                },
            },
        );
        assert!(
            response.ok,
            "an orphan stale claim must not block recovery: {response:?}"
        );

        // The pane now carries the fresh claim, and the orphan generation no
        // longer gates anything.
        let current = host
            .state
            .lock()
            .unwrap()
            .global
            .lookup_unique_tmux_pane_route(&endpoint)
            .cloned()
            .expect("the pane is owned after the takeover");
        assert_eq!(current.binding_id, binding_id);
        assert!(
            current.endpoint_generation < orphan.endpoint_generation,
            "the takeover legitimately regressed the orphan's generation"
        );

        // The host journal must replay: this is the durability property whose
        // loss made the daemon unstartable.
        let replayed = replay(&host_root).unwrap();
        assert!(
            replayed
                .global
                .lookup_unique_tmux_pane_route(&endpoint)
                .is_some(),
            "the takeover must replay from the host journal"
        );

        std::fs::remove_dir_all(host_root).unwrap();
        std::fs::remove_dir_all(project_root).unwrap();
    }
