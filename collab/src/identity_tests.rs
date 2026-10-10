use super::*;

static ENV_LOCK: &std::sync::Mutex<()> = &crate::scope::TEST_ENV_LOCK;

fn test_scope(root: PathBuf) -> Scope {
    Scope { root }
}

fn test_root(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn tmux_candidate(
    codex_session_id: Option<&str>,
    codex_thread_id: Option<&str>,
    pane_id: &str,
) -> crate::proto::TmuxCandidate {
    crate::proto::TmuxCandidate {
        endpoint: crate::proto::TmuxEndpoint {
            socket_path: "/tmp/tmux-test.sock".into(),
            server_pid: 42,
            tmux_session_id: "$7".into(),
            pane_id: pane_id.into(),
            pane_pid: 99,
            codex_session_id: codex_session_id.map(str::to_owned),
            codex_thread_id: codex_thread_id.map(str::to_owned),
        },
        cwd: "/tmp/project".into(),
    }
}

fn observation(
    session: Option<&str>,
    thread: Option<&str>,
    tmux: Option<crate::proto::TmuxCandidate>,
) -> AnchorObservation {
    AnchorObservation {
        session_id: session.map(str::to_owned),
        thread_id: thread.map(str::to_owned),
        tmux,
        dsh_session_id: None,
    }
}

fn facts(
    session: Option<&str>,
    thread: Option<&str>,
    tmux: Option<crate::proto::TmuxCandidate>,
) -> crate::proto::IdentityFacts {
    crate::proto::IdentityFacts {
        session_id: session.map(str::to_owned),
        thread_id: thread.map(str::to_owned),
        endpoint: None,
        namespace: None,
        tmux,
        dsh_session_id: None,
    }
}

fn runtime_identity(generation: u64, binding: &str) -> RuntimeIdentity {
    RuntimeIdentity {
        agent_id: AgentId::new("agent-1").unwrap(),
        runtime_id: RuntimeId::new("runtime-1").unwrap(),
        appserver_id: AppServerId::new("appserver-1").unwrap(),
        endpoint_generation: generation,
        binding_id: BindingId::new(binding).unwrap(),
        session_id: Some(SessionId::new("session-1").unwrap()),
        native_thread_id: Some(NativeThreadId::new("thread-1").unwrap()),
    }
}

fn saved_identity(
    host_paths: &HostPaths,
    scope: &Scope,
    worker_id: &str,
    session_id: Option<&str>,
    thread_id: Option<&str>,
    tmux_endpoint: Option<crate::proto::TmuxEndpoint>,
) {
    let runtime = RuntimeIdentity {
        agent_id: AgentId::new(worker_id).unwrap(),
        runtime_id: RuntimeId::new(format!("runtime-{worker_id}")).unwrap(),
        appserver_id: AppServerId::new(CLI_APP_SERVER_ID).unwrap(),
        endpoint_generation: 1,
        binding_id: BindingId::new(format!("binding-{worker_id}")).unwrap(),
        session_id: session_id.map(|value| SessionId::new(value).unwrap()),
        native_thread_id: thread_id.map(|value| NativeThreadId::new(value).unwrap()),
    };
    let kind = if tmux_endpoint.is_some() {
        TransportKind::Tmux
    } else {
        TransportKind::AppServer
    };
    let transport = SelectedTransport {
        kind,
        endpoint: Some(tmux_endpoint.as_ref().map_or_else(
            || "unix:///tmp/codex.sock".to_owned(),
            |endpoint| endpoint.socket_path.clone(),
        )),
        namespace: Some(
            if tmux_endpoint.is_some() {
                "$7"
            } else {
                "codex_tui"
            }
            .into(),
        ),
        session_id: Some(tmux_endpoint.as_ref().map_or_else(
            || session_id.unwrap_or("session-old").to_owned(),
            |endpoint| endpoint.tmux_session_id.clone(),
        )),
        thread_id: Some(tmux_endpoint.as_ref().map_or_else(
            || thread_id.unwrap_or("thread-old").to_owned(),
            |endpoint| endpoint.pane_id.clone(),
        )),
        tmux_endpoint,
        capabilities: vec![],
        self_check: "test transport".into(),
    };
    let project_scope = scope
        .route_scope(runtime.appserver_id.clone())
        .unwrap()
        .project_scope_id;
    let identity = Identity {
        worker_id: worker_id.into(),
        token: format!("token-{worker_id}"),
        project_scope: Some(project_scope),
        runtime: Some(runtime),
        transport: Some(transport),
    };
    write_identity(&identity_path_at(host_paths, worker_id).unwrap(), &identity).unwrap();
}

#[test]
fn resolver_recovers_existing_identity_by_each_unique_anchor() {
    let root = test_root("ci-resolver-anchor");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();

    saved_identity(
        &host_paths,
        &scope,
        "session-peer",
        Some("session-1"),
        None,
        None,
    );
    let found =
        resolve_for_daemon_at(&host_paths, &scope, &facts(Some("session-1"), None, None)).unwrap();
    assert_eq!(found.worker_id, "session-peer");
    assert_eq!(found.token, "token-session-peer");

    saved_identity(
        &host_paths,
        &scope,
        "thread-peer",
        None,
        Some("thread-2"),
        None,
    );
    let found =
        resolve_for_daemon_at(&host_paths, &scope, &facts(None, Some("thread-2"), None)).unwrap();
    assert_eq!(found.worker_id, "thread-peer");

    let candidate = tmux_candidate(None, None, "%3");
    saved_identity(
        &host_paths,
        &scope,
        "pane-peer",
        None,
        None,
        Some(candidate.endpoint.clone()),
    );
    let found =
        resolve_for_daemon_at(&host_paths, &scope, &facts(None, None, Some(candidate))).unwrap();
    assert_eq!(found.worker_id, "pane-peer");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn resolver_returns_the_exact_stored_token() {
    let root = test_root("ci-resolver-token");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    saved_identity(
        &host_paths,
        &scope,
        "peer",
        Some("session-1"),
        Some("thread-1"),
        None,
    );
    let stored = read_persisted(&host_paths, "peer").unwrap().unwrap();
    let resolved =
        resolve_for_daemon_at(&host_paths, &scope, &facts(Some("session-1"), None, None)).unwrap();
    assert_eq!(resolved.token, stored.token);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn resolver_mints_a_deterministic_draft_without_writing() {
    let root = test_root("ci-resolver-draft");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    let thread_facts = facts(None, Some("thread-new"), None);
    let first = resolve_for_daemon_at(&host_paths, &scope, &thread_facts).unwrap();
    let second = resolve_for_daemon_at(&host_paths, &scope, &thread_facts).unwrap();
    assert_eq!(first.worker_id, second.worker_id);
    assert_eq!(first.worker_id, "codex-thread-7468726561642d6e6577");
    assert!(first.runtime.is_none());
    assert!(first.transport.is_none());
    assert_ne!(first.token, second.token, "draft tokens are freshly minted");
    assert!(
        read_persisted(&host_paths, &first.worker_id)
            .unwrap()
            .is_none(),
        "the resolver never writes a draft before Register"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn resolver_uses_the_pane_naming_for_a_new_pane() {
    let root = test_root("ci-resolver-pane-draft");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    let candidate = tmux_candidate(None, None, "%9");
    let draft =
        resolve_for_daemon_at(&host_paths, &scope, &facts(None, None, Some(candidate))).unwrap();
    assert_eq!(draft.worker_id, "codex-%9");
    assert!(read_persisted(&host_paths, "codex-%9").unwrap().is_none());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn resolver_requires_an_anchor() {
    let root = test_root("ci-resolver-no-anchor");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    let error = resolve_for_daemon_at(&host_paths, &scope, &facts(None, None, None))
        .unwrap_err()
        .to_string();
    assert!(
        error.starts_with("COLLAB_IDENTITY_ANCHOR_MISSING:"),
        "{error}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn resolver_never_adopts_an_unrelated_same_project_peer() {
    let root = test_root("ci-resolver-no-unrelated");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    saved_identity(
        &host_paths,
        &scope,
        "cold-peer",
        Some("session-victim"),
        Some("thread-victim"),
        None,
    );
    let draft =
        resolve_for_daemon_at(&host_paths, &scope, &facts(None, Some("thread-new"), None)).unwrap();
    assert_ne!(draft.worker_id, "cold-peer");
    assert_eq!(draft.worker_id, "codex-thread-7468726561642d6e6577");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn resolver_rejects_an_ambiguous_anchor_without_guessing_from_recency() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-resolver-ambiguous");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    saved_identity(
        &host_paths,
        &scope,
        "a-peer",
        Some("session-dup"),
        None,
        None,
    );
    saved_identity(
        &host_paths,
        &scope,
        "b-peer",
        Some("session-dup"),
        None,
        None,
    );
    // Unreachable endpoints cannot prove which of two records owns an anchor.
    let error = resolve_for_daemon_at(&host_paths, &scope, &facts(Some("session-dup"), None, None))
        .unwrap_err();
    assert!(error.to_string().starts_with("IDENTITY_RESTORE_AMBIGUOUS:"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn resolver_rejects_a_cross_project_anchor() {
    let root = test_root("ci-resolver-cross-project");
    let other = root.join("other-project");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(other.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let other_scope = test_scope(other.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    saved_identity(
        &host_paths,
        &other_scope,
        "foreign-peer",
        Some("session-foreign"),
        None,
        None,
    );
    let error = resolve_for_daemon_at(
        &host_paths,
        &scope,
        &facts(Some("session-foreign"), None, None),
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.starts_with("IDENTITY_RESTORE_CROSS_PROJECT:"),
        "{error}"
    );
    assert!(read_persisted(&host_paths, "foreign-peer")
        .unwrap()
        .is_some());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn preserved_draft_token_is_not_reminted() {
    let root = test_root("ci-resolver-preserve-draft");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    let draft_identity = Identity {
        worker_id: "codex-%5".into(),
        token: "stored-draft-token".into(),
        project_scope: Some(
            scope
                .route_scope(AppServerId::new(CLI_APP_SERVER_ID).unwrap())
                .unwrap()
                .project_scope_id,
        ),
        runtime: None,
        transport: None,
    };
    write_identity(
        &identity_path_at(&host_paths, "codex-%5").unwrap(),
        &draft_identity,
    )
    .unwrap();
    let candidate = tmux_candidate(None, None, "%5");
    let resolved =
        resolve_for_daemon_at(&host_paths, &scope, &facts(None, None, Some(candidate))).unwrap();
    assert_eq!(resolved.token, "stored-draft-token");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn deterministic_name_does_not_adopt_a_registered_identity_with_different_anchors() {
    let root = test_root("ci-name-collision");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    let observed = facts(Some("session-new"), Some("thread-new"), None);
    let draft = resolve_for_daemon_at(&host_paths, &scope, &observed).unwrap();
    saved_identity(
        &host_paths,
        &scope,
        &draft.worker_id,
        Some("session-other"),
        Some("thread-other"),
        None,
    );
    let error = resolve_for_daemon_at(&host_paths, &scope, &observed).unwrap_err();
    assert!(error.to_string().starts_with("IDENTITY_RESTORE_CONFLICT:"));
    std::fs::remove_dir_all(root).unwrap();
}

/// Place one identity under a retired generation directory, as the daemon's
/// Register transaction would. Recovery only reads this path.
fn archive_identity(host_paths: &HostPaths, worker_id: &str, identity: &Identity) {
    let path = host_paths
        .state_root()
        .join("archives")
        .join("identities-retired-1")
        .join(worker_id)
        .join("identity.json");
    write_identity(&path, identity).unwrap();
    std::fs::remove_dir_all(host_paths.state_root().join("identities").join(worker_id)).ok();
}

#[test]
fn archived_route_recovery_reads_without_calling_back() {
    let root = test_root("ci-archive-pane-recovery");
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: "/tmp/archive-pane.sock".into(),
        server_pid: 31,
        tmux_session_id: "$7".into(),
        pane_id: "%7".into(),
        pane_pid: 41,
        codex_session_id: Some("session-old".into()),
        codex_thread_id: Some("thread-old".into()),
    };
    saved_identity(
        &host_paths,
        &scope,
        "pane-worker",
        Some("session-old"),
        Some("thread-old"),
        Some(endpoint.clone()),
    );
    let original = read_persisted(&host_paths, "pane-worker").unwrap().unwrap();
    archive_identity(&host_paths, "pane-worker", &original);
    let mut current = endpoint;
    current.codex_session_id = Some("session-new".into());
    current.codex_thread_id = Some("thread-new".into());
    let candidate = crate::proto::TmuxCandidate {
        endpoint: current,
        cwd: root.display().to_string(),
    };
    let route = crate::proto::RouteResolution {
        app_scope_id: AppServerId::new(CLI_APP_SERVER_ID).unwrap(),
        project_scope: scope
            .route_scope(AppServerId::new(CLI_APP_SERVER_ID).unwrap())
            .unwrap()
            .project_scope_id,
        canonical_root: root.display().to_string(),
        storage_root: root.display().to_string(),
        agent_id: AgentId::new("pane-worker").unwrap(),
        binding_id: BindingId::new("binding-pane-worker").unwrap(),
        endpoint_generation: 1,
        session_id: SessionId::new("session-old").unwrap(),
        native_thread_id: NativeThreadId::new("thread-old").unwrap(),
    };
    // No socket exists under the state root, so any daemon-to-self RPC would
    // fail; the route-aware resolver returns the archived credential directly.
    let facts = crate::proto::IdentityFacts {
        session_id: Some("session-new".into()),
        thread_id: Some("thread-new".into()),
        endpoint: None,
        namespace: None,
        tmux: Some(candidate.clone()),
        dsh_session_id: None,
    };
    let selected =
        resolve_for_daemon_with_route_at(&host_paths, &scope, &facts, Some(&route)).unwrap();
    assert_eq!(selected.token, original.token);
    let mut committed = route.clone();
    committed.endpoint_generation += 1;
    committed.session_id = SessionId::new("session-new").unwrap();
    committed.native_thread_id = NativeThreadId::new("thread-new").unwrap();
    assert_eq!(
        recover_archived_pane_at(&host_paths, &scope, &candidate, Some(&committed))
            .unwrap()
            .unwrap()
            .token,
        original.token,
    );
    let mut wrong_generation = route.clone();
    wrong_generation.endpoint_generation += 2;
    assert!(
        recover_archived_pane_at(&host_paths, &scope, &candidate, Some(&wrong_generation))
            .unwrap()
            .is_none()
    );
    assert!(
        recover_archived_pane_at(&host_paths, &scope, &candidate, None)
            .unwrap()
            .is_none()
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn persisted_registration_records_the_current_project_scope() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-project-scope");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join(".collab-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let mut identity = Identity {
        worker_id: "agent-1".into(),
        token: "token-1".into(),
        project_scope: None,
        runtime: None,
        transport: None,
    };

    persist_registration_at(
        &host_paths,
        &scope,
        &mut identity,
        runtime_identity(7, "binding-7"),
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-1".into()),
            thread_id: Some("thread-1".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message".into()],
            self_check: "server verified".into(),
        },
    )
    .unwrap();

    let expected = scope
        .route_scope(AppServerId::new("appserver-1").unwrap())
        .unwrap()
        .project_scope_id;
    assert_eq!(identity.project_scope.as_ref(), Some(&expected));
    let persisted = read_identity(&identity_path_at(&host_paths, "agent-1").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(persisted.project_scope.as_ref(), Some(&expected));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn identity_files_are_written_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-identity-mode");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join(".collab-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let mut identity = Identity {
        worker_id: "agent-mode".into(),
        token: "secret-token".into(),
        project_scope: None,
        runtime: None,
        transport: None,
    };
    persist_registration_at(
        &host_paths,
        &scope,
        &mut identity,
        runtime_identity(7, "binding-7"),
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-1".into()),
            thread_id: Some("thread-1".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message".into()],
            self_check: "server verified".into(),
        },
    )
    .unwrap();
    let path = identity_path_at(&host_paths, "agent-mode").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "identity file must be owner-only, got {mode:o}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn identity_writes_use_distinct_temporary_paths() {
    let path = std::path::Path::new("/tmp/collab-identity/identity.json");
    let first = identity_temp_path(path);
    let second = identity_temp_path(path);
    assert_ne!(first, second);
}

#[test]
fn binding_validation_rejects_stale_generation_without_mutating_inputs() {
    let registered = runtime_identity(4, "binding-1");
    let incoming = runtime_identity(3, "binding-1");
    let registered_before = registered.clone();
    let incoming_before = incoming.clone();

    assert!(matches!(
        validate_binding(&registered, &incoming),
        Err(BindingValidationError::StaleGeneration {
            expected: 4,
            observed: 3
        })
    ));
    assert_eq!(registered, registered_before);
    assert_eq!(incoming, incoming_before);
}

#[test]
fn binding_validation_rejects_wrong_binding_without_mutating_inputs() {
    let registered = runtime_identity(4, "binding-1");
    let incoming = runtime_identity(4, "binding-2");
    let registered_before = registered.clone();
    let incoming_before = incoming.clone();

    assert!(matches!(
        validate_binding(&registered, &incoming),
        Err(BindingValidationError::Mismatch {
            field: "binding_id",
            ..
        })
    ));
    assert_eq!(registered, registered_before);
    assert_eq!(incoming, incoming_before);
}

#[test]
fn identifier_validation_rejects_empty_and_control_values() {
    assert!(AgentId::new("").is_err());
    assert!(RuntimeId::new("runtime\n1").is_err());
    assert!(DispatchId::new("d".repeat(MAX_ID_LENGTH + 1)).is_err());
}

#[test]
fn observation_anchors_include_tmux_codex_ids() {
    let candidate = tmux_candidate(Some("session-tmux"), Some("thread-tmux"), "%1");
    let observed = observation(Some("session-fact"), None, Some(candidate));
    assert_eq!(
        observed.session_anchors(),
        vec!["session-fact", "session-tmux"]
    );
    assert_eq!(observed.thread_anchors(), vec!["thread-tmux"]);
    assert!(observed.has_anchor());
    assert!(!observation(None, None, None).has_anchor());
}

/// The dsh anchor is a designed anchor, not a guess: `DSH_SESSION_ID` alone
/// must make the caller resolvable and recover the persisted dsh identity.
#[test]
fn resolver_recovers_a_dsh_identity_from_its_session_anchor_alone() {
    let root = test_root("ci-resolver-dsh-anchor");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();

    let worker_id = "dsh-thread-6162";
    let runtime = RuntimeIdentity {
        agent_id: AgentId::new(worker_id).unwrap(),
        runtime_id: RuntimeId::new("runtime-dsh").unwrap(),
        appserver_id: AppServerId::new(CLI_APP_SERVER_ID).unwrap(),
        endpoint_generation: 3,
        binding_id: BindingId::new("binding-dsh").unwrap(),
        session_id: Some(SessionId::new("6162").unwrap()),
        native_thread_id: Some(NativeThreadId::new("agent-7").unwrap()),
    };
    let transport = SelectedTransport {
        kind: TransportKind::Dsh,
        endpoint: Some("unix:///tmp/gateway-control.sock".into()),
        namespace: Some("runtime-dsh".into()),
        session_id: Some("6162".into()),
        thread_id: Some("agent-7".into()),
        tmux_endpoint: None,
        capabilities: vec!["enqueue_wake".into()],
        self_check: "test dsh transport".into(),
    };
    let project_scope = scope
        .route_scope(runtime.appserver_id.clone())
        .unwrap()
        .project_scope_id;
    let identity = Identity {
        worker_id: worker_id.into(),
        token: "token-dsh".into(),
        project_scope: Some(project_scope),
        runtime: Some(runtime),
        transport: Some(transport),
    };
    write_identity(
        &identity_path_at(&host_paths, worker_id).unwrap(),
        &identity,
    )
    .unwrap();

    let mut facts = facts(None, None, None);
    facts.dsh_session_id = Some("6162".into());
    let found = resolve_for_daemon_at(&host_paths, &scope, &facts).unwrap();
    assert_eq!(found.worker_id, worker_id);
    assert_eq!(found.token, "token-dsh");
    std::fs::remove_dir_all(root).ok();
}

/// A valid but oversized `DSH_SESSION_ID` hex-doubles past `MAX_ID_LENGTH` when
/// building the worker id. It must fail as a bad anchor, not as a generic
/// "identifier exceeds 256 bytes", and not silently drop to the codex branch.
#[test]
fn resolver_rejects_an_oversized_dsh_session_anchor() {
    let root = test_root("ci-resolver-dsh-anchor-oversized");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();

    let oversized = "s".repeat(MAX_ID_LENGTH);
    let mut facts = facts(None, None, None);
    facts.dsh_session_id = Some(oversized);
    let error = resolve_for_daemon_at(&host_paths, &scope, &facts)
        .unwrap_err()
        .to_string();
    assert!(error.contains("COLLAB_IDENTITY_ANCHOR_INVALID"), "{error}");
    assert!(error.contains("DSH_SESSION_ID"), "{error}");
    std::fs::remove_dir_all(root).ok();
}

/// Without any designed anchor the resolver still fails closed, so the new
/// dsh arm cannot turn an anonymous caller into a peer.
#[test]
fn resolver_still_refuses_a_caller_with_no_anchor() {
    let root = test_root("ci-resolver-no-anchor");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    let error = resolve_for_daemon_at(&host_paths, &scope, &facts(None, None, None))
        .unwrap_err()
        .to_string();
    assert!(error.contains("COLLAB_IDENTITY_ANCHOR_MISSING"), "{error}");
    std::fs::remove_dir_all(root).ok();
}

/// The tmux anchor is the owned pane, so two panes of one tmux session must
/// never resolve to one identity.
#[test]
fn a_second_pane_of_the_same_tmux_session_is_not_that_identity() {
    let root = test_root("ci-resolver-tmux-session");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    let first = tmux_candidate(None, None, "%1");
    saved_identity(
        &host_paths,
        &scope,
        "pane-peer",
        None,
        None,
        Some(first.endpoint.clone()),
    );
    let second = tmux_candidate(None, None, "%2");
    let draft = resolve_for_daemon_at(&host_paths, &scope, &facts(None, None, Some(second)))
        .expect("a different pane is a new identity, not an ambiguity error");
    assert_eq!(draft.worker_id, "codex-%2");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn tmux_endpoint_serde_round_trips() {
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: "/tmp/sock".into(),
        server_pid: 1,
        tmux_session_id: "$1".into(),
        pane_id: "%1".into(),
        pane_pid: 2,
        codex_session_id: None,
        codex_thread_id: None,
    };
    let json = serde_json::to_string(&endpoint).unwrap();
    let back: crate::proto::TmuxEndpoint = serde_json::from_str(&json).unwrap();
    assert_eq!(endpoint, back);
}
