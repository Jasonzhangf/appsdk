use super::*;
use crate::identity::RuntimeIdentity;
use crate::proto::AppServerCandidate;
use crate::server::state::WorkerRec;

thread_local! {
    static TEST_TMUX: std::cell::RefCell<Option<(super::peer_tests::IsolatedTmux, usize)>> = const { std::cell::RefCell::new(None) };
    static TEST_PROJECT_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

static TEST_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn test_server() -> (Arc<Server>, PathBuf, PathBuf) {
    let id = TEST_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("collab-host-route-{id}-{}", std::process::id()));
    let server_dir = root.join(".agent-collab").join("server");
    std::fs::create_dir_all(&server_dir).unwrap();
    TEST_PROJECT_ROOT.with(|current| *current.borrow_mut() = Some(root.clone()));
    let journal_path = server_dir.join("journal.jsonl");
    let journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&journal_path)
        .unwrap();
    let server = Arc::new(Server {
        config: crate::config::Config::default(),
        root: root.clone(),
        storage_root: root.clone(),
        journal_path: journal_path.clone(),
        host_paths: HostPaths::for_state_root(root.join("host-state")).unwrap(),
        state: Mutex::new(State::default()),
        journal: Mutex::new(journal),
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
    (server, root, journal_path)
}

fn with_appserver_check(
    server: &mut Arc<Server>,
    check: impl Fn(&AppServerCandidate) -> Result<SelectedTransport, String> + Send + Sync + 'static,
) {
    let server = Arc::get_mut(server).expect("unique test server");
    server.appserver_candidate_check = Arc::new(check);
}

fn with_appserver_notification_sink(
    server: &mut Arc<Server>,
    sink: impl Fn(
            &SelectedTransport,
            Option<&str>,
            &str,
            &str,
            bool,
            &str,
        ) -> Result<serde_json::Value, String>
        + Send
        + Sync
        + 'static,
) {
    let server = Arc::get_mut(server).expect("unique test server");
    server.appserver_notification_sink = Arc::new(sink);
}

/// Seed the same committed state that a successful typed Register leaves
/// behind. The endpoint is deliberately not probed here; these tests cover
/// durable identity/role readback and lock behavior, while the public gate
/// tests cover initial registration success and loopback failure.
fn commit_tmux_typed_fixture(
    server: &Server,
    worker_id: &str,
    token: &str,
    app_scope: AppServerId,
    endpoint: crate::proto::TmuxEndpoint,
) -> RuntimeBinding {
    let project_scope = GlobalState::canonical_project_scope(&server.root).unwrap();
    let route_scope = crate::scope::RouteScope {
        project_scope_id: project_scope.clone(),
        app_scope_id: app_scope.clone(),
    };
    let binding_id = crate::identity::BindingId::new(format!("binding-{worker_id}")).unwrap();
    let transport = SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some(endpoint.socket_path.clone()),
        namespace: None,
        session_id: endpoint.codex_session_id.clone(),
        thread_id: endpoint.codex_thread_id.clone(),
        tmux_endpoint: Some(endpoint),
        capabilities: vec!["direct-input".into()],
        self_check: "typed fixture committed pane receipt".into(),
    };
    let mut binding = RuntimeBinding::new_with_session(
        project_scope.clone(),
        app_scope.clone(),
        crate::identity::AgentId::new(worker_id).unwrap(),
        crate::identity::RuntimeId::new(format!("runtime-{worker_id}")).unwrap(),
        binding_id.clone(),
        1,
        Some(crate::identity::SessionId::new(transport.session_id.clone().unwrap()).unwrap()),
        Some(crate::identity::NativeThreadId::new(transport.thread_id.clone().unwrap()).unwrap()),
    )
    .unwrap();
    binding.tmux_endpoint = transport.tmux_endpoint.clone();
    let registration = crate::server::global_state::ProjectRegistration::with_registered_at(
        project_scope.clone(),
        app_scope,
        crate::server::state::now_ms(),
    )
    .unwrap();
    let worker = WorkerRec {
        id: worker_id.into(),
        token: token.into(),
        cwd: server.root.display().to_string(),
        registered_ms: crate::server::state::now_ms(),
        transport: Some(transport),
    };
    let envelope = CommandEnvelope::new(
        crate::identity::CommandId::new(format!("fixture-register-{worker_id}")).unwrap(),
        crate::identity::OperationId::new(format!("fixture-register-op-{worker_id}")).unwrap(),
        binding_id,
        1,
        route_scope,
        None,
        None,
        None,
        None,
    );
    let typed = crate::server::state::TypedEnvelope {
        command: crate::server::state::TypedCommand::RegisterWorker {
            registration,
            binding: binding.clone(),
            worker,
        },
        envelope,
    };
    server.typed_dispatch(typed).unwrap();
    // A successful registration also publishes the host route that makes the
    // pane the current durable address. `typed_dispatch` emits only the project
    // registration and runtime binding, so publish the same committed route the
    // production registration leaves behind.
    server.commit(&[Event::GlobalCurrentThreadRouteSet {
        binding: binding.clone(),
    }]);
    binding
}

fn verified_appserver(candidate: &AppServerCandidate) -> SelectedTransport {
    SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some(candidate.endpoint.clone()),
        namespace: Some(candidate.namespace.clone()),
        session_id: Some(candidate.session_id.clone()),
        thread_id: Some(candidate.thread_id.clone()),
        tmux_endpoint: None,
        capabilities: vec![
            "session_status".into(),
            "read_thread".into(),
            "send_message_to_thread".into(),
        ],
        self_check: "server verified App Server candidate".into(),
    }
}

fn test_candidates(thread_id: &str) -> Option<TransportCandidates> {
    let root = TEST_PROJECT_ROOT.with(|current| current.borrow().clone()).expect("test project root");
    test_candidates_at(thread_id, &root)
}

fn test_candidates_at(thread_id: &str, cwd: &Path) -> Option<TransportCandidates> {
    let mut endpoint = TEST_TMUX.with(|fixture| {
        let mut fixture = fixture.borrow_mut();
        let (tmux, endpoint_index) = fixture.get_or_insert_with(|| {
            (
                super::peer_tests::IsolatedTmux::start(cwd),
                0,
            )
        });
        let endpoint = if *endpoint_index == 0 {
            tmux.endpoints().remove(0)
        } else {
            tmux.add_session_at(cwd)
        };
        *endpoint_index += 1;
        endpoint
    });
    endpoint.codex_session_id = Some(format!("session-{thread_id}"));
    endpoint.codex_thread_id = Some(thread_id.to_owned());
    Some(TransportCandidates {
        desktop_bridge: None,
        appserver: None,
        dsh: None,
        tmux: Some(crate::proto::TmuxCandidate {
            endpoint,
            cwd: cwd.display().to_string(),
        }),
    })
}

/// A durable address for tests of an already registered owner. It deliberately
/// has no live consumer; initial admission tests must use real candidates.
fn committed_tmux_endpoint(root: &Path, thread_id: &str) -> crate::proto::TmuxEndpoint {
    crate::proto::TmuxEndpoint {
        socket_path: root.join(format!("{thread_id}.sock")).display().to_string(),
        server_pid: 42,
        tmux_session_id: "$1".into(),
        pane_id: "%1".into(),
        pane_pid: 99,
        codex_session_id: Some(format!("session-{thread_id}")),
        codex_thread_id: Some(thread_id.into()),
    }
}

/// Establish an already committed typed owner for recovery/role/route fixtures.
/// This is not the public initial-registration entry: there is no admission or
/// communication claim. The tested public action runs after this precondition.
fn seed_committed_registration_fixture(
    server: &Server,
    worker_id: String,
    token: String,
    cwd: String,
    app_scope: Option<AppServerId>,
    candidates: Option<TransportCandidates>,
) -> Resp {
    let candidate = candidates.expect("committed fixture candidates").tmux
        .expect("committed tmux fixture");
    let endpoint = candidate.endpoint;
    let transport = SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some(endpoint.socket_path.clone()),
        namespace: Some(endpoint.tmux_session_id.clone()),
        session_id: endpoint.codex_session_id.clone(),
        thread_id: endpoint.codex_thread_id.clone(),
        tmux_endpoint: Some(endpoint),
        capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
        self_check: "previously committed typed-owner fixture".into(),
    };
    super::peer_tests::seed_committed_owner(server, &worker_id, &token, &transport, &cwd, app_scope)
}

fn seed_published_registration_fixture(
    server: &Server,
    worker_id: String,
    token: String,
    cwd: String,
    app_scope: Option<AppServerId>,
    candidates: Option<TransportCandidates>,
) -> Resp {
    let response = seed_committed_registration_fixture(server, worker_id.clone(), token,
        cwd.clone(), app_scope.clone(), candidates);
    assert!(response.ok, "prior committed fixture: {response:?}");
    commit_current_thread_route_for_runtime(server, server, &worker_id, &cwd, app_scope.as_ref()).unwrap();
    response
}

fn test_selected_transport(thread_id: &str) -> SelectedTransport {
    let candidate = test_candidates(thread_id)
        .and_then(|candidates| candidates.tmux)
        .expect("tmux candidate");
    SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some(candidate.endpoint.socket_path.clone()),
        namespace: Some(candidate.endpoint.tmux_session_id.clone()),
        session_id: candidate
            .endpoint
            .codex_session_id
            .clone()
            .or_else(|| Some(candidate.endpoint.tmux_session_id.clone())),
        thread_id: candidate
            .endpoint
            .codex_thread_id
            .clone()
            .or_else(|| Some(candidate.endpoint.pane_id.clone())),
        tmux_endpoint: Some(candidate.endpoint),
        capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
        self_check: "test selected tmux transport".into(),
    }
}

#[test]
fn notification_subscription_transport_must_match_tmux_or_appserver_kind() {
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: "/tmp/collab-test.sock".into(),
        server_pid: 12,
        tmux_session_id: "$1".into(),
        pane_id: "%2".into(),
        pane_pid: 34,
        codex_session_id: None,
        codex_thread_id: None,
    };
    let transport = SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some(endpoint.socket_path.clone()),
        namespace: Some(endpoint.tmux_session_id.clone()),
        session_id: Some(endpoint.tmux_session_id.clone()),
        thread_id: Some(endpoint.pane_id.clone()),
        tmux_endpoint: Some(endpoint),
        capabilities: vec!["send_message_to_pane".into()],
        self_check: "test tmux transport".into(),
    };
    let subscription = |method: &str| NotificationSubscription {
        id: "subscription-1".into(),
        worker_id: "worker-1".into(),
        event: "direct-message".into(),
        subject: None,
        target: "%2".into(),
        method: method.into(),
        trigger_ms: None,
        trigger_times_ms: vec![],
        interval_ms: None,
        repeat_count: 1,
        fired_count: 0,
        expires_ms: i64::MAX,
        status: "armed".into(),
        created_ms: 0,
        updated_ms: 0,
        status_reason: None,
    };
    assert!(subscription_matches_transport(
        &subscription("tmux"),
        &transport
    ));
    assert!(!subscription_matches_transport(
        &subscription("appserver"),
        &transport
    ));
}

fn context_with_app(root: &Path, app_scope: &str) -> ProjectContext {
    ProjectContext::for_registered_root_with_app(root, AppServerId::new(app_scope).unwrap())
        .unwrap()
}

fn context_with_runtime(root: &Path, app_scope: &str, runtime: &RuntimeIdentity) -> ProjectContext {
    let mut context = context_with_app(root, app_scope);
    context.runtime_context = Some(runtime.clone());
    context
}

fn write_global_identity(
    host_paths: &HostPaths,
    worker_id: &str,
    token: &str,
    project_scope: Option<&crate::scope::ProjectScopeId>,
    runtime: &RuntimeIdentity,
) {
    let path = host_paths
        .state_root()
        .join("identities")
        .join(worker_id)
        .join("identity.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut identity = serde_json::json!({
        "worker_id": worker_id,
        "token": token,
        "runtime": runtime,
    });
    if let Some(project_scope) = project_scope {
        identity["project_scope"] = serde_json::json!(project_scope);
    }
    std::fs::write(path, serde_json::to_string_pretty(&identity).unwrap()).unwrap();
}

fn runtime_for_registered(
    server: &Server,
    root: &Path,
    worker_id: &str,
    app_scope: &str,
) -> RuntimeIdentity {
    let project_scope = GlobalState::canonical_project_scope(root).unwrap();
    let route_scope = RouteScope {
        app_scope_id: AppServerId::new(app_scope).unwrap(),
        project_scope_id: project_scope,
    };
    let binding = server
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(
            &route_scope,
            &BindingId::new(format!("binding-{worker_id}")).unwrap(),
        )
        .cloned()
        .unwrap();
    crate::identity::RuntimeIdentity {
        agent_id: binding.agent_id,
        runtime_id: binding.runtime_id,
        appserver_id: binding.app_scope_id,
        endpoint_generation: binding.endpoint_generation,
        binding_id: binding.binding_id,
        session_id: binding.session_id,
        native_thread_id: binding.native_thread_id,
    }
}

fn test_candidates_for_registered(
    server: &Server,
    root: &Path,
    worker_id: &str,
    app_scope: &str,
) -> Option<TransportCandidates> {
    let project_scope = GlobalState::canonical_project_scope(root).unwrap();
    let route_scope = RouteScope {
        app_scope_id: AppServerId::new(app_scope).unwrap(),
        project_scope_id: project_scope,
    };
    let binding = server
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(
            &route_scope,
            &BindingId::new(format!("binding-{worker_id}")).unwrap(),
        )
        .cloned()
        .unwrap();
    Some(TransportCandidates {
        desktop_bridge: None,
        appserver: None,
        dsh: None,
        tmux: Some(crate::proto::TmuxCandidate {
            endpoint: binding.tmux_endpoint?,
            cwd: root.display().to_string(),
        }),
    })
}

fn notification_subscribe_request(worker_id: &str, token: &str) -> Req {
    Req::NotificationSubscribe {
        worker_id: worker_id.into(),
        token: token.into(),
        event: "direct-message".into(),
        subject: None,
        trigger_ms: None,
        trigger_times_ms: Vec::new(),
        interval_ms: None,
        repeat_count: 1,
        ttl_seconds: 60,
    }
}

/// Register a peer whose selected transport is App Server, so presence comes
/// from the App Server status probe rather than a tmux pane. Wire
/// registration would call the production candidate verifier, which dials a
/// real socket, so this uses the typed registration path directly.
fn register_native_peer(
    server: &Server,
    root: &Path,
    worker_id: &str,
    token: &str,
    thread_id: &str,
    app_scope: &str,
) {
    let transport = SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some(format!("session-{thread_id}")),
        thread_id: Some(thread_id.to_owned()),
        tmux_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    };
    let project_scope = GlobalState::canonical_project_scope(root).unwrap();
    let envelope = server
        .typed_register_envelope_for_scope(
            worker_id,
            token,
            &transport,
            project_scope,
            root.to_str().unwrap(),
            AppServerId::new(app_scope).unwrap(),
            false,
        )
        .unwrap();
    server
        .typed_dispatch(envelope)
        .expect("native registration must succeed");
}

#[test]
fn appserver_registration_is_rejected_without_candidate_check() {
    let (mut server, root, _) = test_server();
    with_appserver_check(&mut server, |_| {
        panic!("App Server candidate check must not run")
    });
    let appserver = AppServerCandidate {
        endpoint: "unix:///tmp/collab-test-appserver.sock".into(),
        namespace: "codex_tui".into(),
        session_id: "session-thread-1".into(),
        thread_id: "thread-1".into(),
        cwd: root.display().to_string(),
    };
    let error = validate_transport_candidates(
        &server,
        &TransportCandidates {
            desktop_bridge: None,
            appserver: Some(appserver),
            dsh: None,
            tmux: None,
        },
        root.to_str().unwrap(),
    )
    .unwrap_err();
    assert!(
        error.starts_with("APPSERVER_ENDPOINT_REJECTED:")
            || error.contains("ADAPTER_ROUTE_UNAVAILABLE"),
        "{error}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn appserver_self_check_failure_is_explicit_and_has_no_fallback() {
    let (mut server, root, _) = test_server();
    with_appserver_check(&mut server, |_| {
        panic!("App Server candidate check must not run")
    });
    let error = validate_transport_candidates(
        &server,
        &TransportCandidates {
            desktop_bridge: None,
            appserver: Some(AppServerCandidate {
                endpoint: "unix:///tmp/collab-missing-appserver.sock".into(),
                namespace: "codex_tui".into(),
                session_id: "session-thread-1".into(),
                thread_id: "thread-1".into(),
                cwd: root.display().to_string(),
            }),
            dsh: None,
            tmux: None,
        },
        root.to_str().unwrap(),
    )
    .unwrap_err();
    assert!(
        error.starts_with("APPSERVER_ENDPOINT_REJECTED:")
            || error.contains("ADAPTER_ROUTE_UNAVAILABLE"),
        "{error}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn registration_fails_when_no_server_verified_candidate_exists() {
    let (mut server, root, _) = test_server();
    with_appserver_check(&mut server, |_| {
        Err("server self-check rejected App Server candidate".into())
    });
    let error = validate_transport_candidates(
        &server,
        &TransportCandidates {
            desktop_bridge: None,
            appserver: None,
            dsh: None,
            tmux: None,
        },
        root.to_str().unwrap(),
    )
    .unwrap_err();
    assert!(error.starts_with("TRANSPORT_NONE:"), "{error}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn persisted_tmux_master_status_does_not_relock_state() {
    let (server, root, _) = test_server();
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: root.join("absent.sock").display().to_string(),
        server_pid: 1,
        tmux_session_id: "$1".into(),
        pane_id: "%1".into(),
        pane_pid: 2,
        codex_session_id: Some("session-pane-worker".into()),
        codex_thread_id: Some("thread-pane-worker".into()),
    };
    commit_tmux_typed_fixture(
        &server,
        "pane-worker",
        "pane-token",
        AppServerId::new("app-a").unwrap(),
        endpoint,
    );
    let promoted = handle_master_promote(
        &server,
        "pane-worker".into(),
        "pane-token".into(),
        "fixture user approved pane-worker".into(),
    );
    assert!(promoted.ok, "{promoted:?}");
    let (tx, rx) = std::sync::mpsc::channel();
    let join = std::thread::spawn(move || {
        let status = dispatch(&server, Req::MasterStatus);
        let (worker, issues, presence, retired_presence) = {
            let state = server.state.lock().unwrap();
            let worker = state.workers["pane-worker"].clone();
            let issues = migration_issues(&server, &state);
            let presence = worker_presence_in_state(&server, &state, &worker);
            let mut retired = worker.clone();
            retired.token = "retired-token".into();
            let retired_presence = worker_presence_in_state(&server, &state, &retired);
            (worker, issues, presence, retired_presence)
        };
        let observed = worker_identity_and_endpoint(&server, &worker).1;
        tx.send((status, issues, presence, observed, retired_presence))
            .unwrap();
    });
    let (status, issues, presence, observed, retired) = rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .expect("persisted pane callbacks recursively locked server.state");
    join.join().unwrap();
    assert!(status.ok, "{status:?}");
    assert_eq!(status.data["master"]["worker_id"], "pane-worker");
    assert!(status.data["master"]["grant_id"].is_string());
    assert_eq!(
        status.data["master"]["endpoint_live"],
        serde_json::json!(observed)
    );
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(presence, IdentityPresence::Present);
    assert_eq!(retired, IdentityPresence::Missing);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn tmux_registration_and_identity_do_not_require_a_live_probe() {
    let (server, root, _) = test_server();
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: root.join("absent.sock").display().to_string(),
        server_pid: 1,
        tmux_session_id: "$1".into(),
        pane_id: "%1".into(),
        pane_pid: 2,
        codex_session_id: Some("session-pane-worker".into()),
        codex_thread_id: Some("thread-pane-worker".into()),
    };
    let binding = commit_tmux_typed_fixture(
        &server,
        "pane-worker",
        "pane-token",
        AppServerId::new("app-a").unwrap(),
        endpoint.clone(),
    );
    let candidates = TransportCandidates {
        desktop_bridge: None,
        appserver: None,
        dsh: None,
        tmux: Some(crate::proto::TmuxCandidate {
            endpoint: endpoint.clone(),
            cwd: root.display().to_string(),
        }),
    };
    let worker = server.state.lock().unwrap().workers["pane-worker"].clone();
    assert_eq!(
        worker_identity_presence(&server, &worker),
        IdentityPresence::Present
    );
    assert_eq!(worker_presence(&server, &worker), IdentityPresence::Present);
    let (valid, reachable) = worker_identity_and_endpoint(&server, &worker);
    assert!(valid);
    assert_ne!(
        reachable,
        Some(true),
        "missing socket must not be reachable"
    );
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .global
            .lookup_unique_tmux_pane_route(&endpoint)
            .cloned()
            .unwrap(),
        binding
    );
    let mut malformed = candidates;
    malformed.tmux.as_mut().unwrap().endpoint.pane_id = "invalid".into();
    assert!(
        validate_transport_candidates(&server, &malformed, root.to_str().unwrap())
            .unwrap_err()
            .starts_with("TMUX_ENDPOINT_INVALID")
    );
    server.commit(&[Event::LegacyWorkerRemoved {
        worker_id: "pane-worker".into(),
    }]);
    assert_eq!(
        worker_identity_presence(&server, &worker),
        IdentityPresence::Missing
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// F3: durable identity, role and grant must survive a lost or unreadable
/// endpoint. `master status`, `context` peers/master and the worker list must
/// all report the same independent endpoint observation, never `true`, and a
/// read-only query must not retire the identity or drop the grant.
#[test]
fn identity_endpoint_projection_keeps_identity_and_never_reports_live() {
    let (server, root, journal_path) = test_server();
    // A plain file at the socket path makes the tmux probe fail without a
    // server, so reachability is undecided (`null`) rather than missing.
    let socket_path = root.join("probe-error.sock");
    std::fs::write(&socket_path, b"not a tmux socket").unwrap();
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: socket_path.display().to_string(),
        server_pid: 1,
        tmux_session_id: "$1".into(),
        pane_id: "%1".into(),
        pane_pid: 2,
        codex_session_id: Some("session-pane-worker".into()),
        codex_thread_id: Some("thread-pane-worker".into()),
    };
    let binding = commit_tmux_typed_fixture(
        &server,
        "pane-worker",
        "pane-token",
        AppServerId::new("app-a").unwrap(),
        endpoint,
    );
    let promoted = handle_master_promote(
        &server,
        "pane-worker".into(),
        "pane-token".into(),
        "fixture user approved pane-worker".into(),
    );
    assert!(promoted.ok, "{promoted:?}");

    // Each projection runs in its own thread so a hung probe is attributed to
    // one call instead of hanging the test.
    let projection = |label: &str, req: Req| -> Resp {
        let (tx, rx) = std::sync::mpsc::channel();
        let server = server.clone();
        std::thread::spawn(move || {
            tx.send(dispatch(&server, req)).unwrap();
        });
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("public projection {label} hung"))
    };

    let journal_before = std::fs::metadata(&journal_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    let status = projection("master status", Req::MasterStatus);
    assert!(status.ok, "{status:?}");
    let context = projection(
        "context",
        Req::Context {
            worker_id: "pane-worker".into(),
            token: "pane-token".into(),
        },
    );
    assert!(context.ok, "{context:?}");
    let workers = projection("worker list", Req::Workers);
    assert!(workers.ok, "{workers:?}");
    let board = projection("board show", Req::BoardShow);
    assert!(board.ok, "{board:?}");
    let journal_after = std::fs::metadata(&journal_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    assert_eq!(
        journal_after, journal_before,
        "a read-only projection appended a durable event"
    );

    let worker = server.state.lock().unwrap().workers["pane-worker"].clone();
    let observed = worker_identity_and_endpoint(&server, &worker).1;
    assert_ne!(observed, Some(true), "a failed probe must not read as live");
    assert_eq!(status.data["master"]["worker_id"], "pane-worker");
    assert!(status.data["master"]["grant_id"].is_string());
    assert_eq!(status.data["master"]["endpoint_live"], json!(observed));
    assert_eq!(context.data["identity"]["worker_id"], "pane-worker");
    assert_eq!(context.data["identity"]["role"], "master");
    assert_eq!(context.data["master"]["endpoint_live"], json!(observed));
    let peer_row = context.data["peers"]
        .as_array()
        .expect("context lists peers")
        .iter()
        .find(|peer| peer["worker_id"] == "pane-worker")
        .expect("context lists the registered peer");
    assert_eq!(peer_row["endpoint_live"], json!(observed));
    let worker_row = workers.data["workers"]
        .as_array()
        .expect("worker list is an array")
        .iter()
        .find(|worker| worker["id"] == "pane-worker")
        .expect("worker list includes the registered peer");
    assert_eq!(worker_row["endpoint_live"], json!(observed));
    assert_eq!(worker_row["identity_valid"], true);
    assert_eq!(board.data["master"]["worker_id"], "pane-worker");
    assert_eq!(board.data["master"]["grant_id"], status.data["master"]["grant_id"]);
    assert_eq!(board.data["master"]["endpoint_live"], json!(observed));
    let board_row = board.data["workers"].as_array().unwrap().iter()
        .find(|member| member["id"] == "pane-worker").unwrap();
    assert_eq!(board_row["role"], "master");
    assert_eq!(board_row["status"], "unknown");

    // The read-only projections must not retire the identity or drop the grant.
    assert_eq!(
        worker_identity_presence(&server, &worker),
        IdentityPresence::Present
    );
    let state = server.state.lock().unwrap();
    assert_eq!(
        state
            .global
            .lookup_binding_for(&binding.route_scope(), &binding.binding_id),
        Some(&binding)
    );
    assert!(state
        .global
        .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
        .is_some());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn tmux_registration_rejects_a_cwd_that_is_not_the_server_project_root() {
    let (mut server, root, _) = test_server();
    let other = root.with_file_name(format!(
        "{}-registration-other",
        root.file_name().unwrap().to_string_lossy()
    ));
    std::fs::create_dir_all(&other).unwrap();
    let error = validate_transport_candidates(
        &server,
        &TransportCandidates {
            desktop_bridge: None,
            appserver: None,
            dsh: None,
            tmux: Some(crate::proto::TmuxCandidate {
                endpoint: crate::proto::TmuxEndpoint {
                    socket_path: "/tmp/collab-test-tmux.sock".into(),
                    server_pid: 1,
                    tmux_session_id: "$1".into(),
                    pane_id: "%1".into(),
                    pane_pid: 2,
                    codex_session_id: None,
                    codex_thread_id: None,
                },
                cwd: other.display().to_string(),
            }),
        },
        other.to_str().unwrap(),
    )
    .unwrap_err();
    assert!(error.starts_with("RUNTIME_BINDING_REJECTED:"), "{error}");
    assert!(error.contains("does not match project root"), "{error}");
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(other).unwrap();
}

#[test]
fn registration_rejects_a_linked_worktree_root() {
    let id = TEST_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "collab-worktree-registration-{id}-{}",
        std::process::id()
    ));
    let canonical = root.join("project");
    let linked = canonical.join("playground/task-a");
    std::fs::create_dir_all(&canonical).unwrap();
    std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();

    let init = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&canonical)
        .status()
        .unwrap();
    assert!(init.success());
    let commit = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=Collab Test",
            "-c",
            "user.email=collab-test@example.invalid",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "initial",
        ])
        .current_dir(&canonical)
        .status()
        .unwrap();
    assert!(commit.success());
    let worktree = std::process::Command::new("git")
        .args([
            "worktree",
            "add",
            "-q",
            "-b",
            "task-a",
            linked.to_str().unwrap(),
            "main",
        ])
        .current_dir(&canonical)
        .status()
        .unwrap();
    assert!(worktree.success());

    let canonical = canonical.canonicalize().unwrap();
    let linked = linked.canonicalize().unwrap();
    std::fs::create_dir_all(linked.join(".agent-collab")).unwrap();

    validate_project_registration_cwd(canonical.to_str().unwrap(), canonical.as_path()).unwrap();
    let error =
        validate_project_registration_cwd(linked.to_str().unwrap(), linked.as_path()).unwrap_err();
    assert!(error.starts_with("PROJECT_SCOPE_INVALID:"), "{error}");
    assert!(error.contains("linked worktree"), "{error}");

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn registration_reuses_the_registered_tmux_pane_identity() {
    let (server, root, _) = test_server();
    let app_scope = AppServerId::new("app-a").unwrap();
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let (_, first) = register_host_fixture(&manager,
        Some(context_with_app(&root, app_scope.as_str())),
        Req::register("worker-1".into(), "token-1".into(), root.display().to_string(),
            test_candidates_at("thread-1", &root)),
    );
    assert!(first.ok, "{first:?}");

    let second = handle_register_with_app_scope(
        &server,
        "worker-1".into(),
        "token-1".into(),
        root.display().to_string(),
        Some(app_scope),
        test_candidates_for_registered(&server, &root, "worker-1", "app-a"),
    );
    assert!(second.ok, "{second:?}");
    assert_eq!(second.data["transport_selected"]["kind"], "tmux");
    let worker = server.state.lock().unwrap().workers["worker-1"].clone();
    assert_eq!(
        worker.transport.as_ref().map(|transport| &transport.kind),
        Some(&TransportKind::Tmux)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn tmux_notification_acceptance_stays_pending_and_unread() {
    let (server, root, _) = test_server();
    let app_scope = AppServerId::new("app-a").unwrap();
    let registered = seed_published_registration_fixture(
        &server,
        "worker-1".into(),
        "token-1".into(),
        root.display().to_string(),
        Some(app_scope),
        test_candidates("thread-1"),
    );
    assert!(registered.ok, "{registered:?}");
    let subscribed = handle_notification_subscribe(
        &server,
        "worker-1".into(),
        "token-1".into(),
        "direct-message".into(),
        None,
        None,
        Vec::new(),
        None,
        1,
        60,
    );
    assert!(subscribed.ok, "{subscribed:?}");
    let subscription_id = subscribed.data["subscription"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let message_id = gen_msg_id();
    server.commit(&[
        Event::Sent {
            msg: Message {
                id: message_id.clone(),
                from: "sender".into(),
                to: "worker-1".into(),
                mtype: "notify".into(),
                subject: Some("turn-accepted".into()),
                body: "must remain pending after turn acceptance".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::WakeBound {
            message_id: message_id.clone(),
            subscription_id: subscription_id.clone(),
        },
        Event::DeliveryMode {
            msg_id: message_id.clone(),
            mode: "explicit-notification".into(),
            source_thread_id: None,
        },
    ]);

    assert!(attempt_notification_with_at(
        &server,
        &message_id,
        &subscription_id,
        now_ms()
    ));
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[&message_id].state, "pending");
    assert_eq!(
        state.msgs[&message_id].wake_attempt_count, 1,
        "tmux input submission records one notification attempt"
    );
    assert_ne!(state.msgs[&message_id].state, "delivered");
    assert_ne!(state.msgs[&message_id].state, "read");
    assert!(state
        .inbox_of("worker-1")
        .iter()
        .any(|m| m.id == message_id));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_tmux_notification_keeps_message_unread_without_receive_receipt() {
    let (mut server, root, _) = test_server();
    let registered = seed_published_registration_fixture(
        &server,
        "worker-1".into(),
        "token-1".into(),
        root.display().to_string(),
        Some(AppServerId::new("app-a").unwrap()),
        test_candidates("thread-1"),
    );
    assert!(registered.ok, "{registered:?}");
    let subscribed = handle_notification_subscribe(
        &server,
        "worker-1".into(),
        "token-1".into(),
        "direct-message".into(),
        None,
        None,
        Vec::new(),
        None,
        1,
        60,
    );
    assert!(subscribed.ok, "{subscribed:?}");
    let subscription_id = subscribed.data["subscription"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let message_id = gen_msg_id();
    server.commit(&[
        Event::Sent {
            msg: Message {
                id: message_id.clone(),
                from: "sender".into(),
                to: "worker-1".into(),
                mtype: "notify".into(),
                subject: Some("wake-failed".into()),
                body: "failed tmux wake must not count as consumption".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::WakeBound {
            message_id: message_id.clone(),
            subscription_id: subscription_id.clone(),
        },
        Event::DeliveryMode {
            msg_id: message_id.clone(),
            mode: "explicit-notification".into(),
            source_thread_id: None,
        },
    ]);
    Arc::get_mut(&mut server)
        .unwrap()
        .appserver_notification_sink = Arc::new(|_, _, _, _, _, _| {
        Err("TMUX_ENTER_SUBMIT_FAILED: forced send-keys failure".into())
    });

    assert!(!attempt_notification_with_at(
        &server,
        &message_id,
        &subscription_id,
        now_ms()
    ));
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[&message_id].state, "pending");
    assert!(!state
        .notification_delivery_accepted
        .contains_key(&message_id));
    assert!(state.receive_receipts.is_empty());
    assert_eq!(
        state.notification_delivery_failures[&message_id].error,
        "TMUX_ENTER_SUBMIT_FAILED: forced send-keys failure"
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_closure_daemon_send_uses_daemon_identity_without_explicit_source_thread() {
    let (mut server, root, _) = test_server();
    with_appserver_check(&mut server, |candidate| Ok(verified_appserver(candidate)));
    let notification_calls = Arc::new(Mutex::new(Vec::new()));
    let notification_calls_for_sink = notification_calls.clone();
    with_appserver_notification_sink(
        &mut server,
        move |_, source_thread_id, text, message_id, explicit, _mode| {
            notification_calls_for_sink.lock().unwrap().push((
                source_thread_id.map(str::to_owned),
                text.to_owned(),
                message_id.to_owned(),
                explicit,
            ));
            Ok(json!({"accepted": true}))
        },
    );
    let app_scope = AppServerId::new("daemon-live-app").unwrap();
    for (worker_id, token, thread_id) in [
        ("requester", "token-requester", "thread-requester"),
        ("target", "token-target", "thread-target"),
    ] {
        register_native_peer(
            &server,
            &root,
            worker_id,
            token,
            thread_id,
            app_scope.as_str(),
        );
    }
    let subscribed = handle_notification_subscribe(
        &server,
        "target".into(),
        "token-target".into(),
        "direct-message".into(),
        None,
        None,
        Vec::new(),
        None,
        1,
        60,
    );
    assert!(subscribed.ok, "{subscribed:?}");

    let response = handle_live_closure_daemon_send(
        &server,
        "requester".into(),
        "token-requester".into(),
        "target".into(),
        "daemon_to_peer".into(),
        "challenge-daemon".into(),
        "challenge-daemon".into(),
        false,
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["daemon_sender"], true);
    assert_eq!(response.data["from"], "collab-server");
    assert_eq!(response.data["path"], "daemon_to_peer");
    assert_eq!(response.data["delivery_mode"], DAEMON_LIVE_CLOSURE_MODE);
    assert_eq!(response.data["restart_replay_pending"], false);
    let message_id = response.data["msg_id"].as_str().unwrap();

    let calls = notification_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, None);
    assert!(calls[0].1.contains("challenge-daemon"));
    // The sink receives the real mailbox id, not a decorated delivery id.
    // dsh forwards this value to the gateway, which strips only
    // `collab:<digest>:` before looking it up in the mailbox.
    assert_eq!(calls[0].2, message_id);
    assert!(!calls[0].3, "daemon live-closure is not explicit");
    drop(calls);

    let state = server.state.lock().unwrap();
    let msg = &state.msgs[message_id];
    assert_eq!(msg.from, "collab-server");
    assert_eq!(msg.to, "target");
    assert_eq!(msg.mtype, "notification");
    assert_eq!(msg.subject.as_deref(), Some("challenge-daemon"));
    assert_eq!(msg.body, "challenge-daemon");
    assert_eq!(state.delivery_modes[message_id], DAEMON_LIVE_CLOSURE_MODE);
    assert!(!state.delivery_source_threads.contains_key(message_id));
    assert_eq!(msg.wake_attempt_count, 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_closure_daemon_to_master_rejects_non_master_target() {
    let (mut server, root, _) = test_server();
    with_appserver_check(&mut server, |candidate| Ok(verified_appserver(candidate)));
    let app_scope = AppServerId::new("daemon-master-app").unwrap();
    for (worker_id, token, thread_id) in [
        ("requester", "token-requester", "thread-requester"),
        ("master", "token-master", "thread-master"),
        ("peer", "token-peer", "thread-peer"),
    ] {
        let registered = seed_published_registration_fixture(
            &server,
            worker_id.into(),
            token.into(),
            root.display().to_string(),
            Some(app_scope.clone()),
            Some(test_candidates(thread_id).unwrap()),
        );
        assert!(registered.ok, "{registered:?}");
    }
    let promoted = handle_master_promote(
        &server,
        "master".into(),
        "token-master".into(),
        "test approval".into(),
    );
    assert!(promoted.ok, "{promoted:?}");

    let rejected = handle_live_closure_daemon_send(
        &server,
        "requester".into(),
        "token-requester".into(),
        "peer".into(),
        "daemon_to_master".into(),
        "challenge-master".into(),
        "challenge-master".into(),
        false,
    );
    assert!(!rejected.ok, "{rejected:?}");
    assert_eq!(
        rejected.error.as_deref(),
        Some("COLLAB_LIVE_CLOSURE_MASTER_ROUTE_MISMATCH")
    );
    assert!(server.state.lock().unwrap().msgs.is_empty());

    for path in ["daemon_to_peer", "restart_replay"] {
        let rejected = handle_live_closure_daemon_send(
            &server,
            "requester".into(),
            "token-requester".into(),
            "master".into(),
            path.into(),
            format!("challenge-{path}"),
            format!("challenge-{path}"),
            path == "restart_replay",
        );
        assert!(!rejected.ok, "{path}: {rejected:?}");
        assert_eq!(
            rejected.error.as_deref(),
            Some("COLLAB_LIVE_CLOSURE_PEER_ROUTE_MISMATCH")
        );
        assert!(server.state.lock().unwrap().msgs.is_empty());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_closure_restart_replay_records_pending_contract_mode() {
    let (mut server, root, _) = test_server();
    with_appserver_check(&mut server, |candidate| Ok(verified_appserver(candidate)));
    with_appserver_notification_sink(&mut server, |_, source_thread_id, _, _, explicit, _mode| {
        assert!(source_thread_id.is_none());
        assert!(!explicit);
        Ok(json!({"accepted": true}))
    });
    let app_scope = AppServerId::new("restart-replay-app").unwrap();
    for (worker_id, token, thread_id) in [
        ("requester", "token-requester", "thread-requester"),
        ("target", "token-target", "thread-target"),
    ] {
        register_native_peer(
            &server,
            &root,
            worker_id,
            token,
            thread_id,
            app_scope.as_str(),
        );
    }
    let subscribed = handle_notification_subscribe(
        &server,
        "target".into(),
        "token-target".into(),
        "direct-message".into(),
        None,
        None,
        Vec::new(),
        None,
        1,
        60,
    );
    assert!(subscribed.ok, "{subscribed:?}");

    let response = handle_live_closure_daemon_send(
        &server,
        "requester".into(),
        "token-requester".into(),
        "target".into(),
        "restart_replay".into(),
        "challenge-restart".into(),
        "challenge-restart".into(),
        true,
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["restart_replay_pending"], true);
    assert_eq!(response.data["delivery_mode"], RESTART_REPLAY_PENDING_MODE);
    let message_id = response.data["msg_id"].as_str().unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.delivery_modes[message_id],
        RESTART_REPLAY_PENDING_MODE
    );
    assert_eq!(state.msgs[message_id].from, "collab-server");
    assert_eq!(state.msgs[message_id].mtype, "notification");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn appserver_subscription_is_bound_to_the_selected_thread() {
    let transport = SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some("session-thread-current".into()),
        thread_id: Some("thread-current".into()),
        tmux_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "server verified".into(),
    };
    let mut subscription = NotificationSubscription {
        id: "subscription-1".into(),
        worker_id: "worker-1".into(),
        target: "thread-stale".into(),
        method: "appserver".into(),
        event: "direct-message".into(),
        subject: None,
        trigger_ms: None,
        trigger_times_ms: Vec::new(),
        interval_ms: None,
        repeat_count: 1,
        fired_count: 0,
        status: "armed".into(),
        status_reason: None,
        created_ms: 0,
        updated_ms: 0,
        expires_ms: i64::MAX,
    };
    assert!(!subscription_matches_transport(&subscription, &transport));
    subscription.target = "thread-current".into();
    assert!(subscription_matches_transport(&subscription, &transport));
}

type MutationSnapshot = (
    u64,
    u64,
    std::collections::HashMap<String, WorkerRec>,
    Vec<(String, String)>,
    Vec<(String, String)>,
    std::collections::HashMap<String, String>,
    std::collections::HashMap<String, String>,
    crate::server::global_state::GlobalState,
);

fn ordered_map_snapshot<T: serde::Serialize>(
    values: &std::collections::HashMap<String, T>,
) -> Vec<(String, String)> {
    let mut snapshot = values
        .iter()
        .map(|(key, value)| (key.clone(), serde_json::to_string(value).unwrap()))
        .collect::<Vec<_>>();
    snapshot.sort_by(|left, right| left.0.cmp(&right.0));
    snapshot
}

fn mutation_snapshot(server: &Server) -> MutationSnapshot {
    let state = server.state.lock().unwrap();
    (
        state.revision,
        state.sequence,
        state.workers.clone(),
        ordered_map_snapshot(&state.msgs),
        ordered_map_snapshot(&state.notification_subscriptions),
        state.delivery_modes.clone(),
        state.wake_bindings.clone(),
        state.global.clone(),
    )
}

fn directory_snapshot(path: &Path) -> Vec<(String, Vec<u8>)> {
    let Some(entries) = std::fs::read_dir(path).ok() else {
        return Vec::new();
    };
    let mut snapshot = entries
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(path).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    snapshot.sort_by(|left, right| left.0.cmp(&right.0));
    snapshot
}

fn context(root: &Path) -> ProjectContext {
    context_with_app(root, "tui-default")
}

fn register_known_project(server: &Server, root: &Path) {
    register_known_project_with_app(server, root, "tui-default");
}

fn register_known_project_with_app(server: &Server, root: &Path, app_scope: &str) {
    let scope = GlobalState::canonical_project_scope(root).unwrap();
    let app = AppServerId::new(app_scope).unwrap();
    let registration = ProjectRegistration::new(scope, app).unwrap();
    server
        .state
        .lock()
        .unwrap()
        .global
        .register_project(registration)
        .unwrap();
}

#[test]
fn raw_ping_keeps_context_free_readiness() {
    let (server, root, _) = test_server();
    assert!(validate_request_context(&server, &Req::Ping, None).is_ok());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_project_context_is_not_reinterpreted_as_unscoped_request() {
    let line = serde_json::json!({
        "project_context": {
            "app_scope_id": "app-wire",
            "canonical_root": "/tmp",
            "project_scope": 42
        },
        "op": "Ping"
    })
    .to_string();
    let error = parse_wire_request(&line).unwrap_err();
    assert!(
        error.starts_with("bad request: invalid project context envelope:"),
        "{error}"
    );
}

#[test]
fn unknown_project_route_fails_closed() {
    let (server, root, _) = test_server();
    let other_root = root.with_file_name(format!(
        "{}-other",
        root.file_name().unwrap().to_string_lossy()
    ));
    std::fs::create_dir_all(&other_root).unwrap();
    let error = validate_request_context(&server, &Req::StatusAll, Some(&context(&other_root)))
        .unwrap_err();
    assert!(error.starts_with("PROJECT_SCOPE_UNKNOWN:"), "{error}");
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(other_root).unwrap();
}

include!("part_01_tail.rs");
