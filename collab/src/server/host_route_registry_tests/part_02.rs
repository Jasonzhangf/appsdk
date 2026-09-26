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
            Req::Register {
                worker_id: "worker-cwd-route".into(),
                token: "token-worker-cwd-route".into(),
                cwd: root.display().to_string(),
                candidates: test_candidates(thread),
            },
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
            Req::Register {
                worker_id: "worker-cwd-owner".into(),
                token: "token-worker-cwd-owner".into(),
                cwd: root.display().to_string(),
                candidates: Some(candidates),
            },
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
                Req::Register {
                    worker_id: worker_id.into(),
                    token: token.into(),
                    cwd: root.to_string_lossy().into_owned(),
                    candidates: Some(TransportCandidates {
                        appserver: None,
                        tmux: Some(crate::proto::TmuxCandidate {
                            endpoint,
                            cwd: root.to_string_lossy().into_owned(),
                        }),
                    }),
                },
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
            Req::Register {
                worker_id: "atomic-register-worker".into(),
                token: "token-atomic-register-worker".into(),
                cwd: external_root.display().to_string(),
                candidates: test_candidates("thread-atomic-register-worker"),
            },
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
            Req::Register {
                worker_id: "current-worker-a".into(),
                token: "token-current-a".into(),
                cwd: project_a.display().to_string(),
                candidates: test_candidates(shared_thread),
            },
        );
        assert!(first.ok, "{first:?}");
        let (_, second) = manager.dispatch_sync(
            Some(context_b.clone()),
            Req::Register {
                worker_id: "current-worker-b".into(),
                token: "token-current-b".into(),
                cwd: project_b.display().to_string(),
                candidates: test_candidates(shared_thread),
            },
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
            Req::Register {
                worker_id: "rejected-worker".into(),
                token: "token-rejected-worker".into(),
                cwd: project_a.display().to_string(),
                candidates: test_candidates(shared_thread),
            },
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
            Req::Register {
                worker_id: worker_id.into(),
                token: token.into(),
                cwd: root.display().to_string(),
                candidates: test_candidates(shared_thread),
            },
        );
        assert!(first.ok, "{first:?}");

        let provisional = RuntimeIdentity::cli_adapter(worker_id).unwrap();
        let (_, rebound) = manager.dispatch_sync(
            Some(context_with_runtime(&root, app, &provisional)),
            Req::Register {
                worker_id: worker_id.into(),
                token: token.into(),
                cwd: root.display().to_string(),
                candidates: test_candidates("thread-same-worker-new"),
            },
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
            Req::Register {
                worker_id: worker_id.into(),
                token: token.into(),
                cwd: root.display().to_string(),
                candidates: candidates("session-same-thread-old"),
            },
        );
        assert!(first.ok, "{first:?}");
        let previous = runtime_for_registered(&server, &root, worker_id, app);

        let (_, rebound) = manager.dispatch_sync(
            Some(context_with_runtime(&root, app, &previous)),
            Req::Register {
                worker_id: worker_id.into(),
                token: token.into(),
                cwd: root.display().to_string(),
                candidates: candidates("session-same-thread-new"),
            },
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
            Req::Register {
                worker_id: "session-pair-worker-a".into(),
                token: "token-session-pair-worker-a".into(),
                cwd: root.display().to_string(),
                candidates: candidates("session-pair-a"),
            },
        );
        assert!(first_registration.ok, "{first_registration:?}");

        let (_, second_registration) = manager.dispatch_sync(
            Some(context.clone()),
            Req::Register {
                worker_id: "session-pair-worker-b".into(),
                token: "token-session-pair-worker-b".into(),
                cwd: root.display().to_string(),
                candidates: candidates("session-pair-b"),
            },
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
            Req::Register {
                worker_id: worker_id.into(),
                token: token.into(),
                cwd: root.display().to_string(),
                candidates: test_candidates("thread-host-route-failure-old"),
            },
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
            Req::Register {
                worker_id: worker_id.into(),
                token: token.into(),
                cwd: root.display().to_string(),
                candidates: test_candidates("thread-host-route-failure-new"),
            },
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
            Req::Register {
                worker_id: worker_id.into(),
                token: token.into(),
                cwd: root.display().to_string(),
                candidates: test_candidates("thread-first-route-compensation"),
            },
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
            Req::Register {
                worker_id: worker_id.into(),
                token: token.into(),
                cwd: project_root.display().to_string(),
                candidates: test_candidates("thread-host-route-compensation-old"),
            },
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
            Req::Register {
                worker_id: worker_id.into(),
                token: token.into(),
                cwd: project_root.display().to_string(),
                candidates: test_candidates("thread-host-route-compensation-new"),
            },
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
            Req::Register {
                worker_id: worker_id.into(),
                token: token.into(),
                cwd: project_root.display().to_string(),
                candidates: test_candidates("thread-host-route-ambiguous-old"),
            },
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
            Req::Register {
                worker_id: worker_id.into(),
                token: token.into(),
                cwd: project_root.display().to_string(),
                candidates: test_candidates("thread-host-route-ambiguous-new"),
            },
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
            Req::Register {
                worker_id: "manager-external-worker".into(),
                token: "token-manager-external-worker".into(),
                cwd: external_root.display().to_string(),
                candidates: test_candidates("thread-manager-external-worker"),
            },
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
            Req::Register {
                worker_id: sender_id.into(),
                token: sender_token.into(),
                cwd: project_root.display().to_string(),
                candidates: test_candidates("thread-route-aware-sender"),
            },
        );
        assert!(sender_registration.ok, "{sender_registration:?}");
        let (recipient_runtime, recipient_registration) = manager.dispatch_sync(
            Some(context.clone()),
            Req::Register {
                worker_id: recipient_id.into(),
                token: recipient_token.into(),
                cwd: project_root.display().to_string(),
                candidates: test_candidates("thread-route-aware-recipient"),
            },
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
