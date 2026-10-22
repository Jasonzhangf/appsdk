    /// Observe the selected pane facts and dispatch registration once.
    pub(super) fn register_host_fixture(
        manager: &ProjectRuntimeManager,
        context: Option<ProjectContext>,
        request: Req,
    ) -> (Arc<Server>, Resp) {
        register_participant_fixture(context, request,
            |context, request| manager.dispatch_sync(Some(context), request))
    }

    async fn register_wire_fixture(server: Arc<Server>, context: Option<ProjectContext>, request: Req) -> Resp {
        let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
        let context = context.expect("wire participant context");
        manager.install_runtime(&ProjectRuntimeManager::route_key(&context), server.clone(), None);
        register_host_fixture(&manager, Some(context), request).1
    }

    fn register_participant_fixture(
        context: Option<ProjectContext>,
        mut request: Req,
        dispatch: impl FnOnce(ProjectContext, Req) -> (Arc<Server>, Resp),
    ) -> (Arc<Server>, Resp) {
        let context = context.expect("participant project context");
        if let Req::IdentityContext { facts, .. } = &mut request {
            if facts.session_id.is_none() {
                facts.session_id = facts.tmux.as_ref()
                    .and_then(|candidate| candidate.endpoint.codex_session_id.clone());
            }
        }
        if matches!(&request, Req::Register { candidates: Some(candidates), .. }
            if candidates.appserver.is_some() && candidates.tmux.is_none()) {
            return dispatch(context, request);
        }
        let candidate = match &mut request {
            Req::Register { candidates: Some(candidates), .. } =>
                candidates.tmux.as_mut().expect("selected tmux participant"),
            Req::IdentityContext { facts, identity_context: None } =>
                facts.tmux.as_mut().expect("selected context participant"),
            _ => panic!("initial fixture requires selected Register or legacy context"),
        };
        let observed = std::process::Command::new("tmux").args([
            "-S", &candidate.endpoint.socket_path, "display-message", "-p", "-t",
            &candidate.endpoint.pane_id, "#{pane_current_path}",
        ]).output().unwrap();
        assert!(observed.status.success(), "inspect selected participant");
        candidate.cwd = String::from_utf8(observed.stdout).unwrap().trim().to_owned();
        dispatch(context, request)
    }

    fn runtime_fixture_for_host(host: &Arc<Server>) -> (Arc<Server>, PathBuf, PathBuf) {
        let (mut runtime, root, journal) = test_server();
        Arc::get_mut(&mut runtime).unwrap().host_paths = host.host_paths.clone();
        (runtime, root, journal)
    }

    fn replay_owned_test_server(server: &Arc<Server>) -> Arc<Server> {
        Arc::new(Server {
            config: server.config.clone(),
            root: server.root.clone(), storage_root: server.storage_root.clone(),
            journal_path: server.journal_path.clone(), host_paths: server.host_paths.clone(),
            state: Mutex::new(replay_from_journal(&server.root, &server.journal_path).unwrap()),
            journal: Mutex::new(std::fs::OpenOptions::new().create(true).append(true)
                .open(&server.journal_path).unwrap()),
            appserver_candidate_check: server.appserver_candidate_check.clone(),
            appserver_notification_sink: server.appserver_notification_sink.clone(),
            appserver_thread_status: server.appserver_thread_status.clone(),
            appserver_thread_archive: server.appserver_thread_archive.clone(),
            mailbox_notify: Notify::new(),
        })
    }

    // Defined committed-owner state for resolver tests. This does not admit a
    // participant or return a Register response; the public owner is exercised
    // after these reducer events establish the test's durable precondition.
    fn commit_host_route_fixture(
        manager: &ProjectRuntimeManager,
        server: &Arc<Server>,
        worker_id: &str,
        token: &str,
        app: &str,
        endpoint: crate::proto::TmuxEndpoint,
    ) {
        let scope = GlobalState::canonical_project_scope(&server.root).unwrap();
        let app_scope = AppServerId::new(app).unwrap();
        let session = endpoint.codex_session_id.clone()
            .unwrap_or_else(|| endpoint.tmux_session_id.clone());
        let thread = endpoint.codex_thread_id.clone()
            .unwrap_or_else(|| endpoint.pane_id.clone());
        let mut binding = RuntimeBinding::new_with_session(
            scope.clone(), app_scope.clone(), AgentId::new(worker_id).unwrap(),
            RuntimeId::new(format!("runtime-{worker_id}")).unwrap(),
            BindingId::new(format!("binding-{worker_id}")).unwrap(), 1,
            Some(crate::identity::SessionId::new(session.clone()).unwrap()),
            Some(NativeThreadId::new(thread.clone()).unwrap()),
        ).unwrap();
        binding.tmux_endpoint = Some(endpoint.clone());
        let mut events = Vec::new();
        if server.state.lock().unwrap().global.lookup_registration(&scope, &app_scope).is_none() {
            events.push(Event::GlobalProjectRegistered {
                registration: ProjectRegistration::new(scope.clone(), app_scope.clone()).unwrap(),
            });
        }
        events.extend([
            Event::Registered { worker: WorkerRec {
                id: worker_id.into(), token: token.into(),
                cwd: server.root.display().to_string(), registered_ms: now_ms(),
                transport: Some(SelectedTransport {
                    kind: TransportKind::Tmux, endpoint: Some(endpoint.socket_path.clone()),
                    namespace: Some(endpoint.tmux_session_id.clone()),
                    session_id: Some(session), thread_id: Some(thread),
                    tmux_endpoint: Some(endpoint),
                    capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
                    self_check: "committed resolver fixture".into(),
                }),
            } },
            Event::GlobalRuntimeBound { binding: binding.clone() },
            Event::GlobalCurrentThreadRouteSet { binding },
        ]);
        server.commit_checked(&events).unwrap();
        manager.install_runtime(&(app.into(), scope.as_str().into()), server.clone(), None);
    }

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
        commit_host_route_fixture(&manager, &server, "worker-cwd-route",
            "token-worker-cwd-route", app, crate::proto::TmuxEndpoint {
                socket_path: root.join("committed-native-route.sock").display().to_string(),
                server_pid: 42, tmux_session_id: "$1".into(), pane_id: "%1".into(), pane_pid: 99,
                codex_session_id: Some(session.into()), codex_thread_id: Some(thread.into()),
            });

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
        let endpoint = crate::proto::TmuxEndpoint {
            socket_path: root.join("committed-route.sock").display().to_string(),
            server_pid: 42,
            tmux_session_id: "$1".into(),
            pane_id: "%1".into(),
            pane_pid: 99,
            codex_session_id: Some(format!("session-{thread}")),
            codex_thread_id: Some(thread.into()),
        };
        // This case reads an already committed route. Initial admission and
        // participant return are covered by the public registration gate.
        commit_host_route_fixture(
            &manager, &server,
            "worker-cwd-owner",
            "token-worker-cwd-owner",
            app,
            endpoint.clone(),
        );

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
        // Persisted ownership resolves even when the daemon cannot probe this
        // socket, and process/session drift is not a different pane address.
        let mut observed = server.state.lock().unwrap().workers["agent-wire-route"]
            .transport.as_ref().unwrap().tmux_endpoint.clone().unwrap();
        observed.server_pid += 1;
        observed.pane_pid += 1;
        observed.tmux_session_id = "$999".into();
        let resolved = manager.resolve_route_by_tmux_endpoint(&observed).unwrap();
        assert_eq!(resolved.agent_id.as_str(), "agent-wire-route");
        assert_eq!(mutation_snapshot(&server), before);
        assert_eq!(std::fs::read(&journal_path).unwrap(), journal_before);
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

    #[tokio::test]
    async fn tmux_routes_with_same_session_and_pane_on_different_sockets_are_distinct() {
        let (server, root, _) = test_server();
        // The resolver reads durable socket+pane ownership. These committed
        // addresses need not be live and must not trigger initial admission.
        let endpoint_a = crate::proto::TmuxEndpoint {
            socket_path: root.join("route-a.sock").display().to_string(),
            server_pid: 42,
            tmux_session_id: "$1".into(),
            pane_id: "%1".into(),
            pane_pid: 99,
            codex_session_id: None,
            codex_thread_id: None,
        };
        let endpoint_b = crate::proto::TmuxEndpoint {
            socket_path: root.join("route-b.sock").display().to_string(),
            ..endpoint_a.clone()
        };
        assert_eq!(endpoint_a.tmux_session_id, endpoint_b.tmux_session_id);
        assert_eq!(endpoint_a.pane_id, endpoint_b.pane_id);
        assert_ne!(endpoint_a.socket_path, endpoint_b.socket_path);

        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        for (worker_id, token, endpoint) in [
            ("worker-tmux-a", "token-tmux-a", endpoint_a.clone()),
            ("worker-tmux-b", "token-tmux-b", endpoint_b.clone()),
        ] {
            commit_host_route_fixture(
                &manager, &server,
                worker_id,
                token,
                "app-tmux-collision",
                endpoint,
            );
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
        let response = register_host_fixture(&manager,
            Some(context.clone()),
            Req::register("atomic-register-worker".into(),
                 "token-atomic-register-worker".into(),
                 external_root.display().to_string(),
                 test_candidates_at("thread-atomic-register-worker", &external_root)),
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
    async fn tmux_projects_with_repeated_native_facts_keep_scoped_routes() {
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
        let first_candidates = test_candidates_at(shared_thread, &project_a).unwrap();
        let first_endpoint = first_candidates.tmux.as_ref().unwrap().endpoint.clone();

        let (_, first) = register_host_fixture(&manager,
            Some(context_a.clone()),
            Req::register("current-worker-a".into(),
                 "token-current-a".into(),
                 project_a.display().to_string(),
                 Some(first_candidates)),
        );
        assert!(first.ok, "{first:?}");
        let (_, second) = register_host_fixture(&manager,
            Some(context_b.clone()),
            Req::register("current-worker-b".into(),
                 "token-current-b".into(),
                 project_b.display().to_string(),
                 test_candidates_at(shared_thread, &project_b)),
        );
        assert!(second.ok, "{second:?}");
        assert_eq!(second.data["command"]["binding"]["project_scope"], context_b.project_scope.as_str());

        let current = manager
            .resolve_route_by_tmux_endpoint(&first_endpoint)
            .unwrap();
        assert_eq!(
            current.project_scope.as_str(),
            context_a.project_scope.as_str()
        );
        assert_eq!(current.app_scope_id.as_str(), "current-app-a");
        assert_eq!(current.agent_id.as_str(), "current-worker-a");
        assert_eq!(current.binding_id.as_str(), "binding-current-worker-a");

        let (_, rejected) = register_host_fixture(&manager,
            Some(context_b.clone()),
            Req::register("rejected-worker".into(),
                 "token-rejected-worker".into(),
                 project_a.display().to_string(),
                 test_candidates_at(shared_thread, &project_a)),
        );
        assert!(!rejected.ok, "{rejected:?}");
        assert_eq!(
            manager
                .resolve_route_by_tmux_endpoint(&first_endpoint)
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
                .resolve_route_by_tmux_endpoint(&first_endpoint)
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

        let (_, first) = register_host_fixture(&manager,
            Some(context.clone()),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 test_candidates(shared_thread)),
        );
        assert!(first.ok, "{first:?}");

        let provisional = RuntimeIdentity::cli_adapter(worker_id).unwrap();
        let (_, rebound) = register_host_fixture(&manager,
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
        assert_eq!(current.native_thread_id.as_ref().unwrap().as_str(), "thread-same-worker-new");
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

        let (_, first) = register_host_fixture(&manager,
            Some(context.clone()),
            Req::register(worker_id.into(),
                 token.into(),
                 root.display().to_string(),
                 candidates("session-same-thread-old")),
        );
        assert!(first.ok, "{first:?}");
        let previous = runtime_for_registered(&server, &root, worker_id, app);

        let (_, rebound) = register_host_fixture(&manager,
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

    /// A pane has exactly one owner, so a second registration on the same pane
    /// id re-anchors to the later registrant and the previous claimant is closed
    /// in the same commit. The pane is a resource, never a liveness credential,
    /// so no probe in this path can veto the replacement.
    #[tokio::test]
    async fn a_second_claim_on_the_same_pane_replaces_the_first_claimant() {
        let (server, root, _) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let context = context_with_app(&root, app);
        let pane = test_candidates("thread-pane-owner-a")
            .unwrap()
            .tmux
            .unwrap()
            .endpoint;
        let on_pane = |thread_id: &str| {
            let mut candidates = test_candidates(thread_id).unwrap();
            let endpoint = &mut candidates.tmux.as_mut().unwrap().endpoint;
            endpoint.socket_path = pane.socket_path.clone();
            endpoint.server_pid = pane.server_pid;
            endpoint.tmux_session_id = pane.tmux_session_id.clone();
            endpoint.pane_id = pane.pane_id.clone();
            endpoint.pane_pid = pane.pane_pid;
            Some(candidates)
        };

        let (_, first_registration) = register_host_fixture(&manager,
            Some(context.clone()),
            Req::register("pane-owner-a".into(),
                 "token-pane-owner-a".into(),
                 root.display().to_string(),
                 on_pane("thread-pane-owner-a")),
        );
        assert!(first_registration.ok, "{first_registration:?}");

        let (_, second_registration) = register_host_fixture(&manager,
            Some(context.clone()),
            Req::register("pane-owner-b".into(),
                 "token-pane-owner-b".into(),
                 root.display().to_string(),
                 on_pane("thread-pane-owner-b")),
        );
        assert!(second_registration.ok, "{second_registration:?}");

        let owner = manager
            .resolve_route_by_native_thread("session-thread-pane-owner-b", "thread-pane-owner-b")
            .unwrap();
        assert_eq!(owner.agent_id.as_str(), "pane-owner-b");

        let (first_claimant_registered, retired) = {
            let state = server.state.lock().unwrap();
            let retired = state
                .global
                .projects
                .values()
                .flat_map(|project| project.runtime_bindings.values())
                .find(|binding| binding.agent_id.as_str() == "pane-owner-a")
                .cloned();
            (state.workers.contains_key("pane-owner-a"), retired)
        };
        assert!(
            !first_claimant_registered,
            "the replaced claimant must be closed, not left registered"
        );
        let retired = retired.expect("the retired binding stays in the ledger");
        assert!(retired.tmux_endpoint.is_none(), "{retired:?}");
        assert!(retired.native_thread_id.is_none(), "{retired:?}");

        std::fs::remove_dir_all(root).unwrap();
    }

    /// An agent session has one current carrier even when its pane moves to
    /// another tmux session. Placement cannot keep the old carrier current.
    #[tokio::test]
    async fn same_agent_session_on_another_tmux_session_replaces_previous_carrier() {
        let (server, root, journal_path) = test_server();
        let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &host_paths).unwrap();
        let app = crate::identity::CLI_APP_SERVER_ID;
        let context = context_with_app(&root, app);
        let shared_thread = "thread-shared-across-panes";

        let initial_candidates = test_candidates_at(shared_thread, &root).unwrap();
        let (_, first_registration) = register_host_fixture(&manager,
            Some(context.clone()),
            Req::register("thread-owner-a".into(),
                 "token-thread-owner-a".into(),
                 root.display().to_string(),
                 Some(initial_candidates.clone())),
        );
        assert!(first_registration.ok, "{first_registration:?}");
        assert_eq!(server.state.lock().unwrap().workers["thread-owner-a"].token, "token-thread-owner-a");
        let before = std::fs::read(&journal_path).unwrap();

        let (_, second_registration) = manager.dispatch_sync(
            Some(context.clone()),
            Req::register("thread-owner-b".into(),
                 "token-thread-owner-b".into(),
                 root.display().to_string(),
                 test_candidates_at(shared_thread, &root)),
        );
        assert!(second_registration.ok, "{second_registration:?}");
        assert!(std::fs::read(&journal_path).unwrap().starts_with(&before));
        let first_endpoint = &initial_candidates.tmux.as_ref().unwrap().endpoint;
        assert!(manager.resolve_route_by_tmux_endpoint(first_endpoint).is_err());
        assert!(!server.state.lock().unwrap().workers.contains_key("thread-owner-a"));
        let second_endpoint: crate::proto::TmuxEndpoint = serde_json::from_value(
            second_registration.data["command"]["binding"]["tmux_endpoint"].clone()).unwrap();
        let owner = manager.resolve_route_by_tmux_endpoint(&second_endpoint).unwrap();
        assert_eq!(owner.agent_id.as_str(), "thread-owner-b");

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

        let (_, registered) = register_host_fixture(&manager,
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

        let (_, response) = register_host_fixture(&manager,
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
        let old_candidates = test_candidates_at("same-pane-old", &root).unwrap();
        let (_, registered) = register_host_fixture(&manager,
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
        let (_, recovered) = register_host_fixture(&manager,
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

include!("part_02_part2.rs");
