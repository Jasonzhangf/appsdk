    #[test]
    fn native_thread_route_resolution_ignores_the_execution_cwd() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let other = root.with_file_name(format!(
            "{}-other-cwd",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&other).unwrap();
        let app = "app-cwd-route";
        let thread = "thread-cwd-route";
        let session = "session-thread-cwd-route";
        let (_, registered) = manager.dispatch_sync(
            Some(context_with_app(&root, app)),
            Req::register("worker-cwd-route".into(),
                 "token-worker-cwd-route".into(),
                 root.display().to_string(),
                 test_candidates(thread)),
        );
        assert!(registered.ok, "{registered:?}");

        // The daemon returns the canonical registered root for the dual key.
        // It does not consult or echo any caller cwd, so the same thread
        // resolves identically no matter which directory the caller ran in.
        let resolved = manager
            .resolve_route_by_native_thread(session, thread)
            .unwrap();
        assert_eq!(resolved.agent_id.as_str(), "worker-cwd-route");
        assert_eq!(
            resolved.canonical_root,
            root.canonicalize().unwrap().to_string_lossy()
        );
        assert_eq!(
            manager
                .resolve_route_by_native_thread(session, thread)
                .unwrap(),
            resolved
        );

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(other).unwrap();
    }

    #[test]
    fn tmux_route_resolution_uses_the_registered_canonical_project() {
        let (server, root, _) = test_server();
        let canonical_root = root.canonicalize().unwrap();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app = "app-thread-cwd-route";
        let thread = "thread-cwd-owner";
        let candidates = test_candidates_at(thread, &canonical_root).unwrap();
        let endpoint = candidates.tmux.as_ref().unwrap().endpoint.clone();
        let (_, registered) = manager.dispatch_sync(
            Some(context_with_app(&root, app)),
            Req::register("worker-cwd-owner".into(),
                 "token-worker-cwd-owner".into(),
                 root.display().to_string(),
                 Some(candidates)),
        );
        assert!(registered.ok, "{registered:?}");

        // Route resolution uses the persisted tmux endpoint and does not
        // derive or re-probe an App Server thread from caller cwd.
        let resolved = manager.resolve_route_by_tmux_endpoint(&endpoint).unwrap();
        assert_eq!(resolved.agent_id.as_str(), "worker-cwd-owner");
        assert_eq!(resolved.canonical_root, canonical_root.to_string_lossy());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn wire_route_resolution_checks_the_full_tmux_endpoint_and_is_read_only() {
        let (server, root, journal_path) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        register_known_project_with_app(&server, &root, "app-wire-route");
        let endpoint = crate::proto::TmuxEndpoint {
            socket_path: "/tmp/tmux-registered-route.sock".into(),
            server_pid: 42,
            tmux_session_id: "$1".into(),
            pane_id: "%1".into(),
            pane_pid: 99,
            codex_session_id: None,
            codex_thread_id: None,
        };
        let mut binding = RuntimeBinding::new_with_session(
            GlobalState::canonical_project_scope(&root).unwrap(),
            AppServerId::new("app-wire-route").unwrap(),
            AgentId::new("agent-wire-route").unwrap(),
            RuntimeId::new("runtime-wire-route").unwrap(),
            BindingId::new("binding-wire-route").unwrap(),
            3,
            Some(crate::identity::SessionId::new("$1").unwrap()),
            Some(NativeThreadId::new("%1").unwrap()),
        )
        .unwrap();
        binding.tmux_endpoint = Some(endpoint.clone());
        binding.validate().unwrap();
        server
            .state
            .lock()
            .unwrap()
            .global
            .bind_runtime(binding.clone())
            .unwrap();
        server.commit(&[Event::GlobalCurrentThreadRouteSet {
            binding: binding.clone(),
        }]);
        manager.install_runtime(
            &(
                binding.app_scope_id.as_str().to_owned(),
                binding.project_scope.as_str().to_owned(),
            ),
            server.clone(),
            None,
        );
        write_global_identity(
            &host_paths,
            "agent-wire-route",
            "token-wire-route",
            Some(&binding.project_scope),
            &RuntimeIdentity {
                agent_id: binding.agent_id.clone(),
                runtime_id: binding.runtime_id.clone(),
                appserver_id: binding.app_scope_id.clone(),
                endpoint_generation: binding.endpoint_generation,
                binding_id: binding.binding_id.clone(),
                session_id: binding.session_id.clone(),
                native_thread_id: binding.native_thread_id.clone(),
            },
        );
        server.commit(&[Event::Registered {
            worker: WorkerRec {
                id: "agent-wire-route".into(),
                token: "token-wire-route".into(),
                cwd: root.to_string_lossy().into_owned(),
                registered_ms: now_ms(),
                transport: Some(SelectedTransport {
                    kind: TransportKind::Tmux,
                    endpoint: Some("/tmp/tmux-registered-route.sock".into()),
                    namespace: Some("$1".into()),
                    session_id: Some("$1".into()),
                    thread_id: Some("%1".into()),
                    tmux_endpoint: Some(endpoint),
                    capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
                    self_check: "isolated route fixture".into(),
                }),
            },
        }]);

        assert!(validate_request_context(
            &server,
            &Req::RouteResolve {
                tmux_endpoint: crate::proto::TmuxEndpoint {
                    socket_path: "/tmp/tmux-wire-route.sock".into(),
                    server_pid: 1,
                    tmux_session_id: "$1".into(),
                    pane_id: "%1".into(),
                    pane_pid: 2,
                    codex_session_id: None,
                    codex_thread_id: None,
                },
            },
            None,
        )
        .is_ok());
        let before = mutation_snapshot(&server);
        let journal_before = std::fs::read(&journal_path).unwrap();
        let (_, response) = dispatch_wire_routed(
            manager,
            None,
            Req::RouteResolve {
                tmux_endpoint: crate::proto::TmuxEndpoint {
                    socket_path: "/tmp/tmux-wire-route.sock".into(),
                    server_pid: 1,
                    tmux_session_id: "$1".into(),
                    pane_id: "%1".into(),
                    pane_pid: 2,
                    codex_session_id: None,
                    codex_thread_id: None,
                },
            },
            tokio::sync::watch::channel(false).1,
        )
        .await;
        assert!(
            !response.ok,
            "a different tmux socket must not resolve this pane route"
        );
        assert!(response.error.as_deref().is_some_and(
            |error| error.contains("no registered Collab route is bound to tmux socket")
        ));
        assert_eq!(mutation_snapshot(&server), before);
        assert_eq!(std::fs::read(&journal_path).unwrap(), journal_before);

        std::fs::remove_dir_all(root).unwrap();
    }

    struct IsolatedTmuxServer {
        socket_path: PathBuf,
    }

    impl IsolatedTmuxServer {
        fn start(root: &Path, name: &str) -> anyhow::Result<Self> {
            let socket_path = root.join(format!("{name}.sock"));
            let output = std::process::Command::new("tmux")
                .args([
                    "-S",
                    socket_path.to_str().unwrap(),
                    "new-session",
                    "-d",
                    "-s",
                    "shared-name",
                    "sleep 60",
                ])
                .output()?;
            if !output.status.success() {
                anyhow::bail!(
                    "start isolated tmux {name}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Ok(Self { socket_path })
        }

        fn endpoint(&self) -> anyhow::Result<crate::proto::TmuxEndpoint> {
            let output = std::process::Command::new("tmux")
                .args([
                    "-S",
                    self.socket_path.to_str().unwrap(),
                    "display-message",
                    "-p",
                    "#{pid}\t#{session_id}\t#{pane_id}\t#{pane_pid}",
                ])
                .output()?;
            if !output.status.success() {
                anyhow::bail!(
                    "inspect isolated tmux: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let text = String::from_utf8(output.stdout)?;
            let fields = text.trim_end().split('\t').collect::<Vec<_>>();
            if fields.len() != 4 {
                anyhow::bail!("unexpected isolated tmux endpoint output: {text:?}");
            }
            Ok(crate::proto::TmuxEndpoint {
                socket_path: self.socket_path.to_string_lossy().into_owned(),
                server_pid: fields[0].parse()?,
                tmux_session_id: fields[1].to_owned(),
                pane_id: fields[2].to_owned(),
                pane_pid: fields[3].parse()?,
                codex_session_id: None,
                codex_thread_id: None,
            })
        }
    }

    impl Drop for IsolatedTmuxServer {
        fn drop(&mut self) {
            let _ = std::process::Command::new("tmux")
                .args(["-S", self.socket_path.to_str().unwrap(), "kill-server"])
                .output();
        }
    }

    #[tokio::test]
    async fn tmux_routes_with_same_session_and_pane_on_different_sockets_are_distinct() {
        let (server, root, _) = test_server();
        let tmux_a = IsolatedTmuxServer::start(&root, "route-a").unwrap();
        let tmux_b = IsolatedTmuxServer::start(&root, "route-b").unwrap();
        let endpoint_a = tmux_a.endpoint().unwrap();
        let endpoint_b = tmux_b.endpoint().unwrap();
        assert_eq!(endpoint_a.tmux_session_id, endpoint_b.tmux_session_id);
        assert_eq!(endpoint_a.pane_id, endpoint_b.pane_id);
        assert_ne!(endpoint_a.socket_path, endpoint_b.socket_path);

        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        register_known_project_with_app(&server, &root, "app-tmux-collision");
        for (worker_id, token, endpoint) in [
            ("worker-tmux-a", "token-tmux-a", endpoint_a.clone()),
            ("worker-tmux-b", "token-tmux-b", endpoint_b.clone()),
        ] {
            let (_, response) = manager.dispatch_sync(
                Some(context_with_app(&root, "app-tmux-collision")),
                Req::register(worker_id.into(),
                     token.into(),
                     root.to_string_lossy().into_owned(),
                     Some(TransportCandidates {
                        appserver: None,
                        dsh: None,
                        tmux: Some(crate::proto::TmuxCandidate {
                            endpoint,
                            cwd: root.to_string_lossy().into_owned(),
                        }),
                    })),
            );
            assert!(response.ok, "{response:?}");
        }

        let route_a = manager.resolve_route_by_tmux_endpoint(&endpoint_a).unwrap();
        let route_b = manager.resolve_route_by_tmux_endpoint(&endpoint_b).unwrap();
        assert_eq!(route_a.agent_id.as_str(), "worker-tmux-a");
        assert_eq!(route_b.agent_id.as_str(), "worker-tmux-b");

        let replayed = replay_from_journal(&server.root, &server.journal_path).unwrap();
        assert_eq!(
            replayed
                .global
                .lookup_tmux_route(&endpoint_a)
                .map(|binding| binding.agent_id.as_str()),
            Some("worker-tmux-a")
        );
        assert_eq!(
            replayed
                .global
                .lookup_tmux_route(&endpoint_b)
                .map(|binding| binding.agent_id.as_str()),
            Some("worker-tmux-b")
        );

        drop(tmux_b);
        drop(tmux_a);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn route_journal_failure_precedes_external_reducer_mutation() {
        let (server, root, host_journal) = test_server();
        let external_root = root.with_file_name(format!(
            "{}-atomic-register",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(external_root.join(".agent-collab/server")).unwrap();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let mut manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let route_journal_blocker = root.join("route-journal-blocker");
        std::fs::create_dir_all(&route_journal_blocker).unwrap();
        Arc::get_mut(&mut manager).unwrap().route_journal = route_journal_blocker;

        let context = context_with_app(&external_root, "atomic-register-app");
        let response = manager.dispatch_sync(
            Some(context.clone()),
            Req::register("atomic-register-worker".into(),
                 "token-atomic-register-worker".into(),
                 external_root.display().to_string(),
                 test_candidates("thread-atomic-register-worker")),
        );
        assert!(!response.1.ok, "{:?}", response.1);
        assert!(response
            .1
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("HOST_ROUTE_DURABILITY_FAILED:")));
        assert!(!manager
            .routes
            .lock()
            .unwrap()
            .contains_key(&ProjectRuntimeManager::route_key(&context)));
        assert!(
            !external_root
                .join(".agent-collab/server/journal.jsonl")
                .exists(),
            "route admission failure must not create an unreachable reducer journal"
        );
        assert!(std::fs::read(&host_journal).unwrap().is_empty());

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(external_root).unwrap();
    }

    #[tokio::test]
    async fn native_thread_route_resolution_rejects_cross_project_thread_reuse() {
        let (server, root, host_journal) = test_server();
        let project_a = root.with_file_name(format!(
            "{}-current-a",
            root.file_name().unwrap().to_string_lossy()
        ));
        let project_b = root.with_file_name(format!(
            "{}-current-b",
            root.file_name().unwrap().to_string_lossy()
        ));
        for project in [&project_a, &project_b] {
            std::fs::create_dir_all(project.join(".agent-collab/server")).unwrap();
        }
        let project_a = project_a.canonicalize().unwrap();
        let project_b = project_b.canonicalize().unwrap();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let shared_thread = "thread-cross-project-current";
        let context_a = context_with_app(&project_a, "current-app-a");
        let context_b = context_with_app(&project_b, "current-app-b");

        let (_, first) = manager.dispatch_sync(
            Some(context_a.clone()),
            Req::register("current-worker-a".into(),
                 "token-current-a".into(),
                 project_a.display().to_string(),
                 test_candidates(shared_thread)),
        );
        assert!(first.ok, "{first:?}");
        let (_, second) = manager.dispatch_sync(
            Some(context_b.clone()),
            Req::register("current-worker-b".into(),
                 "token-current-b".into(),
                 project_b.display().to_string(),
                 test_candidates(shared_thread)),
        );
        assert!(!second.ok, "{second:?}");
        assert!(
            second
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("RUNTIME_BINDING_REJECTED:")),
            "{second:?}"
        );

        let current = manager
            .resolve_route_by_native_thread(&format!("session-{shared_thread}"), shared_thread)
            .unwrap();
        assert_eq!(
            current.project_scope.as_str(),
            context_a.project_scope.as_str()
        );
        assert_eq!(current.app_scope_id.as_str(), "current-app-a");
        assert_eq!(current.agent_id.as_str(), "current-worker-a");
        assert_eq!(current.binding_id.as_str(), "binding-current-worker-a");

        let (_, rejected) = manager.dispatch_sync(
            Some(context_b.clone()),
            Req::register("rejected-worker".into(),
                 "token-rejected-worker".into(),
                 project_a.display().to_string(),
                 test_candidates(shared_thread)),
        );
        assert!(!rejected.ok, "{rejected:?}");
        assert_eq!(
            manager
                .resolve_route_by_native_thread(&format!("session-{shared_thread}"), shared_thread)
                .unwrap(),
            current
        );

        let host_state = server.state.lock().unwrap();
        server.rewrite_journal_locked(&host_state).unwrap();
        drop(host_state);
        assert!(
            !std::fs::read(&host_journal).unwrap().is_empty(),
            "the host current transition must be durable in the host journal"
        );
        drop(manager);

        let replayed_host = Arc::new(Server {
            config: crate::config::Config::default(),
            root: root.clone(),
            storage_root: root.clone(),
            journal_path: host_journal.clone(),
            host_paths: host_paths.clone(),
            state: Mutex::new(replay(&root).unwrap()),
            journal: Mutex::new(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&host_journal)
                    .unwrap(),
            ),
            appserver_candidate_check: Arc::new(|candidate| Ok(verified_appserver(candidate))),
            appserver_notification_sink: Arc::new(|_, _, _, _, _, _| {
                Ok(serde_json::json!({"accepted": true}))
            }),
            appserver_thread_status: Arc::new(|_, thread_id| {
                Ok(serde_json::json!({
                    "thread": {
                        "id": thread_id,
                        "status": {"type": "idle"},
                        "canAcceptDirectInput": true
                    }
                }))
            }),
            appserver_thread_archive: Arc::new(|_, _| Ok(serde_json::json!({"archived": true}))),
            mailbox_notify: Notify::new(),
        });
        let replayed_manager = ProjectRuntimeManager::new(replayed_host, &host_paths).unwrap();
        assert_eq!(
            replayed_manager
                .resolve_route_by_native_thread(&format!("session-{shared_thread}"), shared_thread)
                .unwrap(),
            current
        );

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(project_a).unwrap();
        std::fs::remove_dir_all(project_b).unwrap();
    }

    #[tokio::test]
    async fn same_thread_reregistration_retires_the_previous_current_route() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let worker_id = "same-thread-worker";
        let token = "token-same-thread-worker";
        let context = context_with_app(&root, app);
        let shared_thread = "thread-same-worker-old";

        let (_, first) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates(shared_thread)),
        );
        assert!(first.ok, "{first:?}");

        let provisional = RuntimeIdentity::cli_adapter(worker_id).unwrap();
        let (_, rebound) = manager.dispatch_sync(
            Some(context_with_runtime(&root, app, &provisional)),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates("thread-same-worker-new")),
        );
        assert!(rebound.ok, "{rebound:?}");

        let old = manager
            .resolve_route_by_native_thread(&format!("session-{shared_thread}"), shared_thread)
            .unwrap_err();
        assert!(old.starts_with("SESSION_THREAD_BINDING_STALE"), "{old}");
        assert!(
            old.contains("reboundTo=(session-thread-same-worker-new"),
            "{old}"
        );
        let current = manager
            .resolve_route_by_native_thread(
                "session-thread-same-worker-new",
                "thread-same-worker-new",
            )
            .unwrap();
        assert_eq!(current.agent_id.as_str(), worker_id);
        assert_eq!(current.native_thread_id.as_str(), "thread-same-worker-new");
        let replayed_host = replay(&root).unwrap();
        let replayed_old = replayed_host
            .global
            .lookup_current_thread_route_tombstone(
                &crate::identity::SessionId::new(format!("session-{shared_thread}")).unwrap(),
                &NativeThreadId::new(shared_thread).unwrap(),
            )
            .expect("daemon replay must retain the old route tombstone");
        assert_eq!(
            replayed_old
                .rebound_to
                .session_id
                .as_ref()
                .unwrap()
                .as_str(),
            "session-thread-same-worker-new"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn same_thread_session_rotation_rebinds_tmux_generation_and_fences_old_pair() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let worker_id = "same-thread-session-worker";
        let token = "token-same-thread-session-worker";
        let context = context_with_app(&root, app);
        let shared_thread = "thread-same-session-worker";
        let candidates = |session_id: &str| {
            let mut candidates = test_candidates(shared_thread).unwrap();
            candidates.tmux.as_mut().unwrap().endpoint.codex_session_id =
                Some(session_id.to_owned());
            Some(candidates)
        };

        let (_, first) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 candidates("session-same-thread-old")),
        );
        assert!(first.ok, "{first:?}");
        let previous = runtime_for_registered(&server, &root, worker_id, app);

        let (_, rebound) = manager.dispatch_sync(
            Some(context_with_runtime(&root, app, &previous)),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 candidates("session-same-thread-new")),
        );
        assert!(rebound.ok, "{rebound:?}");
        assert_eq!(rebound.data["recovered"], true);
        let current = runtime_for_registered(&server, &root, worker_id, app);
        assert_eq!(
            current.endpoint_generation,
            previous.endpoint_generation + 1
        );
        assert_eq!(
            current.session_id.as_ref().unwrap().as_str(),
            "session-same-thread-new"
        );
        assert_eq!(
            current.native_thread_id.as_ref().unwrap().as_str(),
            shared_thread
        );
        assert_eq!(current.runtime_id, previous.runtime_id);

        let old = manager
            .resolve_route_by_native_thread("session-same-thread-old", shared_thread)
            .unwrap_err();
        assert!(old.starts_with("SESSION_THREAD_BINDING_STALE"), "{old}");
        assert!(old.contains("reboundTo=(session-same-thread-new"), "{old}");
        let resolved = manager
            .resolve_route_by_native_thread("session-same-thread-new", shared_thread)
            .unwrap();
        assert_eq!(resolved.agent_id.as_str(), worker_id);
        assert_eq!(resolved.endpoint_generation, current.endpoint_generation);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn duplicate_codex_thread_anchor_cannot_bind_two_tmux_peers() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let context = context_with_app(&root, app);
        let shared_thread = "thread-session-pair-worker";
        let candidates = |session_id: &str| {
            let mut candidates = test_candidates(shared_thread).unwrap();
            candidates.tmux.as_mut().unwrap().endpoint.codex_session_id =
                Some(session_id.to_owned());
            Some(candidates)
        };

        let (_, first_registration) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register("session-pair-worker-a".into(),
                 "token-session-pair-worker-a".into(),
                 root.display().to_string(),
                 candidates("session-pair-a")),
        );
        assert!(first_registration.ok, "{first_registration:?}");

        let (_, second_registration) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register("session-pair-worker-b".into(),
                 "token-session-pair-worker-b".into(),
                 root.display().to_string(),
                 candidates("session-pair-b")),
        );
        assert!(!second_registration.ok, "{second_registration:?}");
        assert!(second_registration.error.as_deref().is_some_and(|error| {
            error.starts_with("RUNTIME_BINDING_REJECTED:")
                && error.contains("identity anchor is already bound")
        }));

        let first = manager
            .resolve_route_by_native_thread("session-pair-a", shared_thread)
            .unwrap();
        assert_eq!(first.agent_id.as_str(), "session-pair-worker-a");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn host_current_route_commit_failure_is_explicit() {
        let (server, root, host_journal) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let context = context_with_app(&root, app);
        let worker_id = "host-route-failure-worker";
        let token = "token-host-route-failure-worker";

        let (_, registered) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates("thread-host-route-failure-old")),
        );
        assert!(registered.ok, "{registered:?}");
        let binding = server
            .state
            .lock()
            .unwrap()
            .global
            .lookup_current_thread_route(
                &crate::identity::SessionId::new("session-thread-host-route-failure-old").unwrap(),
                &NativeThreadId::new("thread-host-route-failure-old").unwrap(),
            )
            .cloned()
            .unwrap();
        manager
            .fail_current_thread_route_publish
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let (_, response) = manager.dispatch_sync(
            Some(context_with_runtime(
                &root,
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
                 root.display().to_string(),
                 test_candidates("thread-host-route-failure-new")),
        );
        assert!(!response.ok, "{response:?}");
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("ROUTE_TRANSITION_DURABILITY_FAILED:")),
            "{response:?}"
        );
        assert!(!std::fs::read(&host_journal).unwrap().is_empty());
        assert_eq!(
            server
                .state
                .lock()
                .unwrap()
                .global
                .lookup_current_thread_route(
                    &crate::identity::SessionId::new("session-thread-host-route-failure-old",)
                        .unwrap(),
                    &NativeThreadId::new("thread-host-route-failure-old").unwrap()
                )
                .map(|binding| binding.native_thread_id.as_ref().unwrap().as_str()),
            Some("thread-host-route-failure-old")
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn live_tmux_master_rebinds_same_pane_without_new_promotion() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let worker = "same-pane-master";
        let token = "token-same-pane-master";
        let context = context_with_app(&root, app);
        let old_candidates = test_candidates("same-pane-old").unwrap();
        let (_, registered) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register(
                worker.into(),
                token.into(),
                root.display().to_string(),
                Some(old_candidates.clone()),
            ),
        );
        assert!(registered.ok, "{registered:?}");
        let promoted = super::handle_master_promote(
            &server,
            worker.into(),
            token.into(),
            "user approved same-pane-master".into(),
        );
        assert!(promoted.ok, "{promoted:?}");
        let old = runtime_for_registered(&server, &root, worker, app);
        let mut new_candidates = old_candidates;
        let endpoint = &mut new_candidates.tmux.as_mut().unwrap().endpoint;
        endpoint.codex_session_id = Some("session-same-pane-new".into());
        endpoint.codex_thread_id = Some("same-pane-new".into());
        let (_, denied) = manager.dispatch_sync(
            Some(context_with_runtime(&root, app, &old)),
            Req::register(
                worker.into(),
                "wrong-token".into(),
                root.display().to_string(),
                Some(new_candidates.clone()),
            ),
        );
        assert!(!denied.ok, "{denied:?}");
        assert_eq!(
            runtime_for_registered(&server, &root, worker, app).endpoint_generation,
            old.endpoint_generation,
        );
        manager
            .fail_current_thread_route_publish
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let (_, recovered) = manager.dispatch_sync(
            Some(context_with_runtime(&root, app, &old)),
            Req::register(
                worker.into(),
                token.into(),
                root.display().to_string(),
                Some(new_candidates.clone()),
            ),
        );
        assert!(recovered.ok, "{recovered:?}");
        let current = runtime_for_registered(&server, &root, worker, app);
        assert_eq!(current.endpoint_generation, old.endpoint_generation + 1);
        let (_, repeated) = manager.dispatch_sync(
            Some(context_with_runtime(&root, app, &current)),
            Req::register(
                worker.into(),
                token.into(),
                root.display().to_string(),
                Some(new_candidates),
            ),
        );
        assert!(repeated.ok, "{repeated:?}");
        assert_eq!(
            runtime_for_registered(&server, &root, worker, app).endpoint_generation,
            current.endpoint_generation,
        );
        assert_eq!(
            server
                .state
                .lock()
                .unwrap()
                .global
                .role_for_route(
                    &crate::server::global_state::RouteScope {
                        app_scope_id: current.appserver_id.clone(),
                        project_scope_id: GlobalState::canonical_project_scope(&root).unwrap(),
                    },
                    &current.binding_id,
                ),
            crate::server::global_state::PeerRole::Master,
        );
        std::fs::remove_dir_all(root).unwrap();
    }

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
