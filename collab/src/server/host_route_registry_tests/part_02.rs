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
    let context_root = context.canonical_root.clone();
    let request_cwd = match &request {
        Req::Register { cwd, .. } => Some(cwd.clone()),
        Req::IdentityContext { identity_context, .. } =>
            identity_context.as_ref().and_then(|request| request.invocation_cwd.clone()),
        _ => None,
    };
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
        _ => panic!("initial fixture requires selected Register or context"),
    };
    let endpoint_socket = candidate.endpoint.socket_path.clone();
    let endpoint_session = candidate.endpoint.tmux_session_id.clone();
    let endpoint_pane = candidate.endpoint.pane_id.clone();
    eprintln!(
        "COLLAB_CI_FIXTURE participant_before_query context_root={context_root} request_cwd={request_cwd:?} candidate_cwd_before={} socket={endpoint_socket} session={endpoint_session} pane={endpoint_pane}",
        candidate.cwd,
    );
    let observed = std::process::Command::new("tmux").args([
        "-S", &endpoint_socket, "display-message", "-p", "-t",
        &endpoint_pane, "#{pane_current_path}",
    ]).output().unwrap();
    eprintln!(
        "COLLAB_CI_FIXTURE participant_query context_root={context_root} socket={endpoint_socket} session={endpoint_session} pane={endpoint_pane} exit_code={:?} success={} stdout={:?} stderr={:?}",
        observed.status.code(),
        observed.status.success(),
        String::from_utf8_lossy(&observed.stdout),
        String::from_utf8_lossy(&observed.stderr),
    );
    assert!(observed.status.success(), "inspect selected participant");
    candidate.cwd = String::from_utf8(observed.stdout).unwrap().trim().to_owned();
    let candidate_cwd_after = candidate.cwd.clone();
    eprintln!(
        "COLLAB_CI_FIXTURE participant_after_query context_root={context_root} candidate_cwd_after={candidate_cwd_after} socket={endpoint_socket} session={endpoint_session} pane={endpoint_pane}",
    );
    let (server, response) = dispatch(context, request);
    eprintln!(
        "COLLAB_CI_FIXTURE participant_response context_root={context_root} request_cwd={request_cwd:?} candidate_cwd_after={candidate_cwd_after} socket={endpoint_socket} session={endpoint_session} pane={endpoint_pane} ok={} error={:?}",
        response.ok,
        response.error,
    );
    (server, response)
}

fn runtime_fixture_for_host(host: &Arc<Server>) -> (Arc<Server>, PathBuf, PathBuf) {
    let (mut runtime, root, state_path) = test_server();
    Arc::get_mut(&mut runtime).unwrap().host_paths = host.host_paths.clone();
    (runtime, root, state_path)
}

fn reload_owned_test_server(server: &Arc<Server>) -> Arc<Server> {
    Arc::new(Server {
        config: server.config.clone(),
        root: server.root.clone(), storage_root: server.storage_root.clone(),
        state_path: server.state_path.clone(), host_paths: server.host_paths.clone(),
        state: Mutex::new(current_state::load(&server.state_path, false).unwrap()),
        appserver_candidate_check: server.appserver_candidate_check.clone(),
        appserver_notification_sink: server.appserver_notification_sink.clone(),
        appserver_thread_status: server.appserver_thread_status.clone(),
        appserver_thread_archive: server.appserver_thread_archive.clone(),
        mailbox_notify: Notify::new(),
    })
}

// Establish the current map and address projection for readback tests.
fn commit_host_route_fixture(
    manager: &ProjectRuntimeManager,
    server: &Arc<Server>,
    worker_id: &str,
    token: &str,
    app: &str,
    endpoint: crate::proto::TmuxEndpoint,
) -> RuntimeBinding {
    let scope = GlobalState::canonical_project_scope(&server.root).unwrap();
    let committed = commit_tmux_typed_fixture(server, worker_id, token, AppServerId::new(app).unwrap(), endpoint);
    manager.install_runtime(&(app.into(), scope.as_str().into()), server.clone(), None);
    committed
}

#[test]
fn native_thread_route_resolution_ignores_the_execution_cwd() {
    let (server, root, _) = test_server();
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let other = root.with_file_name(format!("{}-other-cwd", root.file_name().unwrap().to_string_lossy()));
    std::fs::create_dir_all(&other).unwrap();
    let session = "session-thread-cwd-route";
    let thread = "thread-cwd-route";
    commit_host_route_fixture(&manager, &server, "worker-cwd-route", "token-worker-cwd-route", "app-cwd-route",
        crate::proto::TmuxEndpoint {
            socket_path: root.join("committed-native-route.sock").display().to_string(),
            server_pid: 42, tmux_session_id: "$1".into(), pane_id: "%1".into(), pane_pid: 99,
            codex_session_id: Some(session.into()), codex_thread_id: Some(thread.into()),
        });
    let resolved = manager.resolve_route_by_native_thread(session, thread).unwrap();
    assert_eq!(resolved.agent_id.as_str(), "worker-cwd-route");
    assert_eq!(resolved.canonical_root, root.canonicalize().unwrap().to_string_lossy());
    assert_eq!(manager.resolve_route_by_native_thread(session, thread).unwrap(), resolved);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(other).unwrap();
}

#[test]
fn tmux_route_resolution_uses_the_registered_canonical_project() {
    let (server, root, _) = test_server();
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: root.join("committed-route.sock").display().to_string(),
        server_pid: 42, tmux_session_id: "$1".into(), pane_id: "%1".into(), pane_pid: 99,
        codex_session_id: Some("session-cwd-owner".into()), codex_thread_id: Some("thread-cwd-owner".into()),
    };
    commit_host_route_fixture(&manager, &server, "worker-cwd-owner", "token-worker-cwd-owner", "app-cwd-owner", endpoint.clone());
    let resolved = manager.resolve_route_by_tmux_endpoint(&endpoint).unwrap();
    assert_eq!(resolved.agent_id.as_str(), "worker-cwd-owner");
    assert_eq!(resolved.canonical_root, root.canonicalize().unwrap().to_string_lossy());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn wire_route_resolution_uses_socket_server_and_pane_without_mutation() {
    let (server, root, state_path) = test_server();
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: root.join("wire-readback.sock").display().to_string(),
        server_pid: 42, tmux_session_id: "$1".into(), pane_id: "%1".into(), pane_pid: 99,
        codex_session_id: Some("session-wire".into()), codex_thread_id: Some("thread-wire".into()),
    };
    let binding = commit_host_route_fixture(&manager, &server, "agent-wire-route", "token-wire-route", "app-wire-route", endpoint.clone());
    assert!(validate_request_context(&server, &Req::RouteResolve { tmux_endpoint: endpoint.clone() }, None).is_ok());
    let before = mutation_snapshot(&server);
    let bytes_before = std::fs::read(&state_path).unwrap();
    let map = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let map_before = map.load().unwrap();
    let (_, valid) = dispatch_wire_routed(manager.clone(), None,
        Req::RouteResolve { tmux_endpoint: endpoint.clone() }, tokio::sync::watch::channel(false).1).await;
    assert!(valid.ok, "{valid:?}");
    assert_eq!(valid.data["agent_id"], binding.agent_id.as_str());
    let mut wrong_socket = endpoint.clone(); wrong_socket.socket_path.push_str("-other");
    let mut wrong_server = endpoint.clone(); wrong_server.server_pid += 1;
    let mut wrong_pane = endpoint.clone(); wrong_pane.pane_id = "%999".into();
    let mut display_changed = endpoint;
    display_changed.tmux_session_id = "$999".into();
    display_changed.pane_pid += 1;
    assert_eq!(manager.resolve_route_by_tmux_endpoint(&display_changed).unwrap().agent_id, binding.agent_id);
    for observed in [wrong_socket, wrong_server, wrong_pane] {
        assert!(manager.resolve_route_by_tmux_endpoint(&observed).unwrap_err().starts_with("ROUTE_RESOLVE_NOT_FOUND:"));
        let (_, rejected) = dispatch_wire_routed(manager.clone(), None,
            Req::RouteResolve { tmux_endpoint: observed }, tokio::sync::watch::channel(false).1).await;
        assert!(!rejected.ok, "{rejected:?}");
        assert!(rejected.error.as_deref().unwrap().starts_with("ROUTE_RESOLVE_NOT_FOUND:"), "{rejected:?}");
        assert_eq!(mutation_snapshot(&server), before);
        assert_eq!(std::fs::read(&state_path).unwrap(), bytes_before);
        assert_eq!(map.load().unwrap(), map_before);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn tmux_routes_with_same_session_and_pane_on_different_sockets_are_distinct() {
    let (server, root, _) = test_server();
    let endpoint_a = crate::proto::TmuxEndpoint {
        socket_path: root.join("route-a.sock").display().to_string(),
        server_pid: 42, tmux_session_id: "$1".into(), pane_id: "%1".into(), pane_pid: 99,
        codex_session_id: Some("session-a".into()), codex_thread_id: Some("thread-a".into()),
    };
    let endpoint_b = crate::proto::TmuxEndpoint {
        socket_path: root.join("route-b.sock").display().to_string(),
        codex_session_id: Some("session-b".into()), codex_thread_id: Some("thread-b".into()),
        ..endpoint_a.clone()
    };
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    for (id, token, endpoint) in [
        ("worker-tmux-a", "token-a", endpoint_a.clone()), ("worker-tmux-b", "token-b", endpoint_b.clone()),
    ] {
        commit_host_route_fixture(&manager, &server, id, token, "app-tmux-collision", endpoint);
    }
    assert_eq!(manager.resolve_route_by_tmux_endpoint(&endpoint_a).unwrap().agent_id.as_str(), "worker-tmux-a");
    assert_eq!(manager.resolve_route_by_tmux_endpoint(&endpoint_b).unwrap().agent_id.as_str(), "worker-tmux-b");
    let loaded = current_state::load(&server.state_path, false).unwrap();
    assert_eq!(loaded.global.lookup_tmux_route(&endpoint_a).unwrap().agent_id.as_str(), "worker-tmux-a");
    assert_eq!(loaded.global.lookup_tmux_route(&endpoint_b).unwrap().agent_id.as_str(), "worker-tmux-b");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn route_publication_failure_precedes_external_reducer_mutation() {
    let (server, root, state_path) = test_server();
    let external_root = root.with_file_name(format!("{}-atomic-register", root.file_name().unwrap().to_string_lossy()));
    std::fs::create_dir_all(external_root.join(".agent-collab/server")).unwrap();
    let mut manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let blocker = root.join("route-publication-blocker");
    std::fs::create_dir_all(&blocker).unwrap();
    Arc::get_mut(&mut manager).unwrap().route_file = blocker;
    let before = std::fs::read(&state_path).unwrap();
    let map = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let map_before = map.load().unwrap();
    let context = context_with_app(&external_root, "atomic-register-app");
    let (_, response) = register_host_fixture(&manager, Some(context.clone()),
        Req::register("atomic-register-worker".into(), "token-atomic-register".into(),
            external_root.display().to_string(), test_candidates_at("thread-atomic-register", &external_root)));
    assert!(!response.ok, "{response:?}");
    assert!(response.error.as_deref().unwrap().starts_with("HOST_ROUTE_DURABILITY_FAILED:"), "{response:?}");
    assert!(!manager.routes.lock().unwrap().contains_key(&ProjectRuntimeManager::route_key(&context)));
    assert!(!external_root.join(".agent-collab/server/state.json").exists());
    assert_eq!(std::fs::read(&state_path).unwrap(), before);
    assert_eq!(map.load().unwrap(), map_before);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(external_root).unwrap();
}

#[tokio::test]
async fn tmux_projects_with_repeated_native_facts_keep_scoped_routes() {
    let (server, root, _) = test_server();
    let project_a = root.with_file_name(format!("{}-current-a", root.file_name().unwrap().to_string_lossy()));
    let project_b = root.with_file_name(format!("{}-current-b", root.file_name().unwrap().to_string_lossy()));
    for project in [&project_a, &project_b] {
        std::fs::create_dir_all(project.join(".agent-collab/server")).unwrap();
    }
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let context_a = context_with_app(&project_a, "current-app-a");
    let context_b = context_with_app(&project_b, "current-app-b");
    let first_candidates = test_candidates_at("thread-cross-project-current", &project_a).unwrap();
    let first_endpoint = first_candidates.tmux.as_ref().unwrap().endpoint.clone();
    let (_, first) = register_host_fixture(&manager, Some(context_a.clone()),
        Req::register("current-worker-a".into(), "token-a".into(), project_a.display().to_string(), Some(first_candidates)));
    assert!(first.ok, "{first:?}");
    let (_, second) = register_host_fixture(&manager, Some(context_b.clone()),
        Req::register("current-worker-b".into(), "token-b".into(), project_b.display().to_string(),
            test_candidates_at("thread-cross-project-current", &project_b)));
    assert!(second.ok, "{second:?}");
    let current = manager.resolve_route_by_tmux_endpoint(&first_endpoint).unwrap();
    assert_eq!(current.project_scope, context_a.project_scope);
    assert_eq!(current.app_scope_id.as_str(), "current-app-a");
    assert_eq!(current.agent_id.as_str(), "current-worker-a");
    let map = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let map_before = map.load().unwrap();
    let (_, rejected) = register_host_fixture(&manager, Some(context_b),
        Req::register("rejected-worker".into(), "token-rejected".into(), project_a.display().to_string(),
            test_candidates_at("thread-cross-project-current", &project_a)));
    assert!(!rejected.ok, "{rejected:?}");
    assert_eq!(manager.resolve_route_by_tmux_endpoint(&first_endpoint).unwrap(), current);
    assert_eq!(map.load().unwrap(), map_before);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(project_a).unwrap();
    std::fs::remove_dir_all(project_b).unwrap();
}

include!("part_02_tail.rs");
