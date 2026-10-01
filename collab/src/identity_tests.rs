use super::*;

static ENV_LOCK: &std::sync::Mutex<()> = &crate::scope::TEST_ENV_LOCK;

fn test_scope(root: PathBuf) -> Scope {
    Scope { root }
}

fn canonical_test_scope(scope: &Scope) -> String {
    scope
        .route_scope(AppServerId::new(CLI_APP_SERVER_ID).unwrap())
        .unwrap()
        .project_scope_id
        .as_str()
        .to_owned()
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

#[test]
fn tmux_identity_recovers_by_each_unique_anchor() {
    let root = test_root("ci-tmux-anchor");
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
    let found = identity_by_tmux_anchor_at(
        &host_paths,
        &scope,
        &tmux_candidate(Some("session-1"), None, "%1"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(found.worker_id, "session-peer");

    saved_identity(
        &host_paths,
        &scope,
        "thread-peer",
        None,
        Some("thread-2"),
        None,
    );
    let found = identity_by_tmux_anchor_at(
        &host_paths,
        &scope,
        &tmux_candidate(None, Some("thread-2"), "%2"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(found.worker_id, "thread-peer");

    let endpoint = tmux_candidate(None, None, "%3").endpoint;
    saved_identity(
        &host_paths,
        &scope,
        "pane-peer",
        None,
        None,
        Some(endpoint.clone()),
    );
    let found =
        identity_by_tmux_anchor_at(&host_paths, &scope, &tmux_candidate(None, None, "%3"))
            .unwrap()
            .unwrap();
    assert_eq!(found.worker_id, "pane-peer");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn tmux_pane_identity_does_not_survive_server_or_pane_pid_reuse() {
    let root = test_root("ci-tmux-anchor-pid-reuse");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    let persisted_endpoint = tmux_candidate(None, None, "%3").endpoint;
    saved_identity(
        &host_paths,
        &scope,
        "stale-pane-peer",
        None,
        None,
        Some(persisted_endpoint.clone()),
    );

    for changed in [
        crate::proto::TmuxEndpoint {
            server_pid: persisted_endpoint.server_pid + 1,
            ..persisted_endpoint.clone()
        },
        crate::proto::TmuxEndpoint {
            pane_pid: persisted_endpoint.pane_pid + 1,
            ..persisted_endpoint.clone()
        },
    ] {
        let mut candidate = tmux_candidate(None, None, "%3");
        candidate.endpoint = changed;
        assert!(identity_by_tmux_anchor_at(&host_paths, &scope, &candidate)
            .unwrap()
            .is_none());
    }

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn pane_only_candidate_adopts_appserver_identity_with_pane_recovery_anchor() {
    let root = test_root("ci-pane-only-appserver-identity");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();

    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: "/tmp/appserver-pane.sock".into(),
        server_pid: 77,
        tmux_session_id: "$8".into(),
        pane_id: "%44".into(),
        pane_pid: 88,
        codex_session_id: Some("session-appserver".into()),
        codex_thread_id: Some("thread-appserver".into()),
    };
    let runtime = RuntimeIdentity {
        agent_id: AgentId::new("appserver-peer").unwrap(),
        runtime_id: RuntimeId::new("runtime-appserver-peer").unwrap(),
        appserver_id: AppServerId::new(CLI_APP_SERVER_ID).unwrap(),
        endpoint_generation: 1,
        binding_id: BindingId::new("binding-appserver-peer").unwrap(),
        session_id: Some(SessionId::new("session-appserver").unwrap()),
        native_thread_id: Some(NativeThreadId::new("thread-appserver").unwrap()),
    };
    let project_scope = scope
        .route_scope(runtime.appserver_id.clone())
        .unwrap()
        .project_scope_id;
    let identity = Identity {
        worker_id: "appserver-peer".into(),
        token: "token-appserver-peer".into(),
        project_scope: Some(project_scope),
        runtime: Some(runtime),
        transport: Some(SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/appserver.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-appserver".into()),
            thread_id: Some("thread-appserver".into()),
            tmux_endpoint: Some(endpoint.clone()),
            capabilities: vec![],
            self_check: "test transport".into(),
        }),
    };
    write_identity(
        &identity_path_at(&host_paths, "appserver-peer").unwrap(),
        &identity,
    )
    .unwrap();
    let found = identity_by_tmux_anchor_at(
        &host_paths,
        &scope,
        &crate::proto::TmuxCandidate {
            endpoint: crate::proto::TmuxEndpoint {
                codex_session_id: None,
                codex_thread_id: None,
                ..endpoint.clone()
            },
            cwd: "/tmp/project".into(),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(found.worker_id, "appserver-peer");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn tmux_identity_recovery_rejects_anchor_conflict_and_cross_project() {
    let root = test_root("ci-tmux-conflict");
    let other = root.join("other-project");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(other.join(".agent-collab")).unwrap();
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
    let endpoint = tmux_candidate(None, None, "%4").endpoint;
    saved_identity(&host_paths, &scope, "pane-peer", None, None, Some(endpoint));
    let conflict = identity_by_tmux_anchor_at(
        &host_paths,
        &scope,
        &tmux_candidate(Some("session-1"), None, "%4"),
    )
    .unwrap_err()
    .to_string();
    assert!(
        conflict.starts_with("IDENTITY_RESTORE_CONFLICT:"),
        "{conflict}"
    );

    let other_scope = test_scope(other.clone());
    saved_identity(
        &host_paths,
        &other_scope,
        "foreign-peer",
        Some("session-foreign"),
        None,
        None,
    );
    let cross_project = identity_by_tmux_anchor_at(
        &host_paths,
        &scope,
        &tmux_candidate(Some("session-foreign"), None, "%5"),
    )
    .unwrap_err()
    .to_string();
    assert!(
        cross_project.starts_with("IDENTITY_RESTORE_CROSS_PROJECT:"),
        "{cross_project}"
    );

    saved_identity(
        &host_paths,
        &scope,
        "duplicate-peer-a",
        Some("session-duplicate"),
        None,
        None,
    );
    saved_identity(
        &host_paths,
        &scope,
        "duplicate-peer-b",
        Some("session-duplicate"),
        None,
        None,
    );
    // Two App Server records claim the same anchor but neither is live (the
    // probe cannot reach the fake endpoint), so this is the peer's own drifted
    // registration: recovery adopts one deterministically instead of failing.
    let adopted = identity_by_tmux_anchor_at(
        &host_paths,
        &scope,
        &tmux_candidate(Some("session-duplicate"), None, "%6"),
    )
    .unwrap()
    .expect("non-live anchor duplicates must auto-adopt");
    assert!(
        adopted.worker_id == "duplicate-peer-a" || adopted.worker_id == "duplicate-peer-b",
        "{adopted:?}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn scope_rebind_command_retires_a_unique_cross_project_tmux_anchor() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-cross-rebind");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join("global");
    let scope = test_scope(root.clone());
    let other = root.join("other-project");
    std::fs::create_dir_all(other.join(".agent-collab")).unwrap();
    let other_scope = test_scope(other.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();

    let candidate = tmux_candidate(Some("session-cross"), Some("thread-cross"), "%5");
    saved_identity(
        &host_paths,
        &other_scope,
        "codex-%5",
        Some("session-cross"),
        Some("thread-cross"),
        Some(candidate.endpoint.clone()),
    );

    // Non-rebind paths (scope resolution / init) still fail closed on a
    // cross-project unique anchor instead of displacing another project.
    let previous_thread = std::env::var_os("CODEX_THREAD_ID");
    let previous_session = std::env::var_os("CODEX_SESSION_ID");
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    let previous_pane = std::env::var_os("TMUX_PANE");
    std::env::set_var("TMUX_PANE", "%5");
    std::env::set_var("CODEX_THREAD_ID", "thread-cross");
    std::env::set_var("CODEX_SESSION_ID", "session-cross");
    std::env::remove_var("COLLAB_WORKER");
    let hard_fail = load_or_create_resolved_at(&host_paths, &scope, None, false);
    let hard_fail_error = hard_fail.as_ref().unwrap_err().to_string();
    assert!(
        hard_fail_error.starts_with("IDENTITY_RESTORE_CROSS_PROJECT:"),
        "{hard_fail_error}"
    );

    // Another unverifiable peer in the current project must not cause the
    // cross-project record to be claimed or archived.
    let blocker_scope = scope
        .route_scope(AppServerId::new(CLI_APP_SERVER_ID).unwrap())
        .unwrap()
        .project_scope_id;
    let blocker = Identity {
        worker_id: "blocker-peer".into(),
        token: "blocker-token".into(),
        project_scope: Some(blocker_scope),
        runtime: None,
        transport: None,
    };
    write_identity(
        &identity_path_at(&host_paths, &blocker.worker_id).unwrap(),
        &blocker,
    )
    .unwrap();

    // Ordinary command path (allow_scope_rebind=true) still fails closed: the
    // cross-project record cannot be proven dead, so it must not be archived
    // (or claimed) by scope mismatch alone.
    let rebound = load_or_create_resolved_at(&host_paths, &scope, None, true);
    let rebound_error = rebound.as_ref().unwrap_err().to_string();
    assert!(
        rebound_error.starts_with("IDENTITY_RESTORE_CROSS_PROJECT:"),
        "{rebound_error}"
    );

    // The foreign peer stays in place and is never archived.
    assert!(
        read_identity(&identity_path_at(&host_paths, "codex-%5").unwrap())
            .unwrap()
            .is_some(),
        "non-dead cross-project peer must remain in the live identity set"
    );
    let archives = host_paths.state_root().join("archives");
    let archived_foreign = std::fs::read_dir(&archives)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .any(|entry| entry.path().join("codex-%5").is_dir())
        })
        .unwrap_or(false);
    assert!(!archived_foreign, "non-dead cross-project peer must not be archived");

    match previous_thread {
        Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
        None => std::env::remove_var("CODEX_THREAD_ID"),
    }
    match previous_session {
        Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
        None => std::env::remove_var("CODEX_SESSION_ID"),
    }
    match previous_worker {
        Some(value) => std::env::set_var("COLLAB_WORKER", value),
        None => std::env::remove_var("COLLAB_WORKER"),
    }
    match previous_pane {
        Some(value) => std::env::set_var("TMUX_PANE", value),
        None => std::env::remove_var("TMUX_PANE"),
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn tmux_identity_rebind_adopts_single_unknown_peer_without_anchor() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-tmux-unknown");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    saved_identity(
        &host_paths,
        &scope,
        "known-peer",
        Some("known-session"),
        None,
        None,
    );
    let previous_pane = std::env::var_os("TMUX_PANE");
    std::env::remove_var("TMUX_PANE");
    let result = identity_for_scope_rebind_at(&host_paths, &scope, None);
    match previous_pane {
        Some(value) => std::env::set_var("TMUX_PANE", value),
        None => std::env::remove_var("TMUX_PANE"),
    }
    assert!(matches!(result, Ok(ScopeRebindOutcome::Adopted(identity)) if identity.worker_id == "known-peer"));
    assert_eq!(
        std::fs::read_dir(host_paths.state_root().join("identities"))
            .unwrap()
            .count(),
        1
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn identity_lives_in_the_global_state_root() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-global");
    let state_root = root.join("global");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let identity =
        load_or_create_resolved_at(&host_paths, &scope, Some("thread-worker".into()), false)
            .unwrap();
    let path = identity_path_at(&host_paths, &identity.worker_id).unwrap();
    assert!(path.starts_with(state_root.join("identities")));
    assert!(!root
        .join(".agent-collab/runs")
        .join(&identity.worker_id)
        .exists());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn identity_reuses_the_unique_persisted_binding_for_a_codex_thread() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-reuse");
    let state_root = root.join("global");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let mut identity =
        load_or_create_resolved_at(&host_paths, &scope, Some("managed-worker".into()), false)
            .unwrap();
    persist_registration_at(
        &host_paths,
        &scope,
        &mut identity,
        runtime_identity(4, "binding-managed"),
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-1".into()),
            thread_id: Some("thread-1".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message".into()],
            self_check: "ok".into(),
        },
    )
    .unwrap();

    let previous_thread = std::env::var_os("CODEX_THREAD_ID");
    let previous_session = std::env::var_os("CODEX_SESSION_ID");
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    let previous_pane = std::env::var_os("TMUX_PANE");
    std::env::set_var("CODEX_THREAD_ID", "thread-1");
    std::env::set_var("CODEX_SESSION_ID", "session-1");
    std::env::remove_var("COLLAB_WORKER");
    std::env::remove_var("TMUX_PANE");
    let resolved = load_or_create_resolved_at(&host_paths, &scope, None, false).unwrap();
    match previous_thread {
        Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
        None => std::env::remove_var("CODEX_THREAD_ID"),
    }
    match previous_session {
        Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
        None => std::env::remove_var("CODEX_SESSION_ID"),
    }
    match previous_worker {
        Some(value) => std::env::set_var("COLLAB_WORKER", value),
        None => std::env::remove_var("COLLAB_WORKER"),
    }
    match previous_pane {
        Some(value) => std::env::set_var("TMUX_PANE", value),
        None => std::env::remove_var("TMUX_PANE"),
    }

    assert_eq!(resolved.worker_id, "managed-worker");
    assert_eq!(resolved.token, identity.token);
    assert_eq!(resolved.runtime, identity.runtime);
    assert_eq!(resolved.transport, identity.transport);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn identity_can_restore_by_a_unique_thread_anchor_after_session_rotation() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-session-thread-key");
    let state_root = root.join("global");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let mut identity =
        load_or_create_resolved_at(&host_paths, &scope, Some("managed-worker".into()), false)
            .unwrap();
    let mut runtime = runtime_identity(4, "binding-managed");
    runtime.session_id = Some(SessionId::new("session-old").unwrap());
    runtime.native_thread_id = Some(NativeThreadId::new("thread-shared").unwrap());
    persist_registration_at(
        &host_paths,
        &scope,
        &mut identity,
        runtime,
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-old".into()),
            thread_id: Some("thread-shared".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message".into()],
            self_check: "ok".into(),
        },
    )
    .unwrap();

    let previous_thread = std::env::var_os("CODEX_THREAD_ID");
    let previous_session = std::env::var_os("CODEX_SESSION_ID");
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    std::env::set_var("CODEX_THREAD_ID", "thread-shared");
    std::env::set_var("CODEX_SESSION_ID", "session-new");
    std::env::remove_var("COLLAB_WORKER");
    let resolved = load_existing_at(&host_paths, &scope, None).unwrap();
    match previous_thread {
        Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
        None => std::env::remove_var("CODEX_THREAD_ID"),
    }
    match previous_session {
        Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
        None => std::env::remove_var("CODEX_SESSION_ID"),
    }
    match previous_worker {
        Some(value) => std::env::set_var("COLLAB_WORKER", value),
        None => std::env::remove_var("COLLAB_WORKER"),
    }

    assert_eq!(resolved.unwrap().worker_id, "managed-worker");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn identity_selection_rejects_an_unscoped_legacy_thread_binding() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-legacy-thread-recover");
    let state_root = root.join("global");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    // A durable record from before the dual key existed: it has a native
    // thread but no persisted session id. It is written directly because
    // the current registration validator refuses to create that shape;
    // only the upgrade path may transition an existing record forward.
    let mut runtime = runtime_identity(3, "binding-legacy");
    runtime.session_id = None;
    runtime.native_thread_id = Some(NativeThreadId::new("thread-legacy").unwrap());
    write_identity(
        &identity_path_at(&host_paths, "legacy-worker").unwrap(),
        &Identity {
            worker_id: "legacy-worker".into(),
            token: "legacy-token".into(),
            project_scope: None,
            runtime: Some(runtime),
            transport: Some(SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some("unix:///tmp/codex.sock".into()),
                namespace: Some("codex_tui".into()),
                session_id: Some("session-host".into()),
                thread_id: Some("thread-legacy".into()),
                tmux_endpoint: None,
                capabilities: vec!["send_message".into()],
                self_check: "ok".into(),
            }),
        },
    )
    .unwrap();

    let previous_thread = std::env::var_os("CODEX_THREAD_ID");
    let previous_session = std::env::var_os("CODEX_SESSION_ID");
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    std::env::set_var("CODEX_THREAD_ID", "thread-legacy");
    std::env::set_var("CODEX_SESSION_ID", "session-host");
    std::env::remove_var("COLLAB_WORKER");
    let resolved = load_existing_at(&host_paths, &scope, None);
    match previous_thread {
        Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
        None => std::env::remove_var("CODEX_THREAD_ID"),
    }
    match previous_session {
        Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
        None => std::env::remove_var("CODEX_SESSION_ID"),
    }
    match previous_worker {
        Some(value) => std::env::set_var("COLLAB_WORKER", value),
        None => std::env::remove_var("COLLAB_WORKER"),
    }

    let error = resolved.unwrap_err().to_string();
    assert!(
        error.starts_with("IDENTITY_RESTORE_CROSS_PROJECT:"),
        "{error}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn identity_selection_can_recover_by_thread_when_session_differs() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-legacy-session-mismatch");
    let state_root = root.join("global");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let mut identity =
        load_or_create_resolved_at(&host_paths, &scope, Some("dual-worker".into()), false)
            .unwrap();
    let mut runtime = runtime_identity(4, "binding-dual");
    runtime.session_id = Some(SessionId::new("session-bound").unwrap());
    runtime.native_thread_id = Some(NativeThreadId::new("thread-bound").unwrap());
    persist_registration_at(
        &host_paths,
        &scope,
        &mut identity,
        runtime,
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-bound".into()),
            thread_id: Some("thread-bound".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message".into()],
            self_check: "ok".into(),
        },
    )
    .unwrap();

    let previous_thread = std::env::var_os("CODEX_THREAD_ID");
    let previous_session = std::env::var_os("CODEX_SESSION_ID");
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    std::env::set_var("CODEX_THREAD_ID", "thread-bound");
    std::env::set_var("CODEX_SESSION_ID", "session-other");
    std::env::remove_var("COLLAB_WORKER");
    let resolved = load_existing_at(&host_paths, &scope, None).unwrap();
    match previous_thread {
        Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
        None => std::env::remove_var("CODEX_THREAD_ID"),
    }
    match previous_session {
        Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
        None => std::env::remove_var("CODEX_SESSION_ID"),
    }
    match previous_worker {
        Some(value) => std::env::set_var("COLLAB_WORKER", value),
        None => std::env::remove_var("COLLAB_WORKER"),
    }

    // The thread is an independently valid unique recovery anchor.
    assert_eq!(resolved.unwrap().worker_id, "dual-worker");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn identity_selection_recovers_by_thread_without_a_session_anchor() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-session-thread-required");
    let state_root = root.join("global");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let mut identity =
        load_or_create_resolved_at(&host_paths, &scope, Some("managed-worker".into()), false)
            .unwrap();
    persist_registration_at(
        &host_paths,
        &scope,
        &mut identity,
        runtime_identity(4, "binding-managed"),
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-1".into()),
            thread_id: Some("thread-1".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message".into()],
            self_check: "ok".into(),
        },
    )
    .unwrap();

    let previous_thread = std::env::var_os("CODEX_THREAD_ID");
    let previous_session = std::env::var_os("CODEX_SESSION_ID");
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    let previous_pane = std::env::var_os("TMUX_PANE");
    std::env::set_var("CODEX_THREAD_ID", "thread-1");
    std::env::remove_var("CODEX_SESSION_ID");
    std::env::remove_var("COLLAB_WORKER");
    std::env::remove_var("TMUX_PANE");

    let existing = load_existing_at(&host_paths, &scope, None)
        .unwrap()
        .expect("the thread anchor uniquely identifies the peer");
    let created = load_or_create_resolved_at(&host_paths, &scope, None, false).unwrap();

    match previous_thread {
        Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
        None => std::env::remove_var("CODEX_THREAD_ID"),
    }
    match previous_session {
        Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
        None => std::env::remove_var("CODEX_SESSION_ID"),
    }
    match previous_worker {
        Some(value) => std::env::set_var("COLLAB_WORKER", value),
        None => std::env::remove_var("COLLAB_WORKER"),
    }
    match previous_pane {
        Some(value) => std::env::set_var("TMUX_PANE", value),
        None => std::env::remove_var("TMUX_PANE"),
    }

    assert_eq!(existing.worker_id, "managed-worker");
    assert_eq!(created.worker_id, "managed-worker");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn existing_identity_prefers_the_global_thread_binding_over_a_stale_alias() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-existing-thread-authority");
    let state_root = root.join("global");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let mut authoritative =
        load_or_create_resolved_at(&host_paths, &scope, Some("authoritative".into()), false)
            .unwrap();
    let mut runtime = runtime_identity(7, "binding-authoritative");
    runtime.native_thread_id = Some(NativeThreadId::new("thread-current").unwrap());
    persist_registration_at(
        &host_paths,
        &scope,
        &mut authoritative,
        runtime,
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-1".into()),
            thread_id: Some("thread-current".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message".into()],
            self_check: "ok".into(),
        },
    )
    .unwrap();
    write_identity(
        &identity_path_at(&host_paths, "codex-thread-current").unwrap(),
        &Identity {
            worker_id: "stale-alias".into(),
            token: "stale-token".into(),
            project_scope: None,
            runtime: None,
            transport: None,
        },
    )
    .unwrap();

    let previous_thread = std::env::var_os("CODEX_THREAD_ID");
    let previous_session = std::env::var_os("CODEX_SESSION_ID");
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    std::env::set_var("CODEX_THREAD_ID", "thread-current");
    std::env::set_var("CODEX_SESSION_ID", "session-1");
    std::env::remove_var("COLLAB_WORKER");
    let resolved = load_existing_at(&host_paths, &scope, None).unwrap();
    match previous_thread {
        Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
        None => std::env::remove_var("CODEX_THREAD_ID"),
    }
    match previous_session {
        Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
        None => std::env::remove_var("CODEX_SESSION_ID"),
    }
    match previous_worker {
        Some(value) => std::env::set_var("COLLAB_WORKER", value),
        None => std::env::remove_var("COLLAB_WORKER"),
    }

    assert_eq!(resolved.unwrap().worker_id, "authoritative");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn identity_requires_a_codex_thread_when_no_worker_is_given() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-thread");
    std::fs::create_dir_all(root.join(".agent-collab/runs")).unwrap();
    let state_root = root.join(".collab-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let previous_thread = std::env::var_os("CODEX_THREAD_ID");
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    let previous_pane = std::env::var_os("TMUX_PANE");
    std::env::remove_var("CODEX_THREAD_ID");
    std::env::remove_var("COLLAB_WORKER");
    std::env::remove_var("TMUX_PANE");
    let result = load_or_create(&scope, None, None);
    if let Some(value) = previous_thread {
        std::env::set_var("CODEX_THREAD_ID", value);
    }
    if let Some(value) = previous_worker {
        std::env::set_var("COLLAB_WORKER", value);
    }
    match previous_pane {
        Some(value) => std::env::set_var("TMUX_PANE", value),
        None => std::env::remove_var("TMUX_PANE"),
    }
    assert!(result.is_err());
    assert!(!state_root.join("identities").exists());
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn first_appserver_peer_uses_thread_identity_without_a_tmux_pane() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("first-appserver-peer-without-pane");
    std::fs::create_dir_all(root.join(".agent-collab/runs")).unwrap();
    let state_root = root.join(".collab-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let previous_thread = std::env::var_os("CODEX_THREAD_ID");
    let previous_session = std::env::var_os("CODEX_SESSION_ID");
    let previous_pane = std::env::var_os("TMUX_PANE");
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    let previous_socket = std::env::var_os("COLLAB_APPSERVER_SOCKET");
    std::env::set_var("CODEX_THREAD_ID", "desktop-first-thread");
    std::env::set_var("CODEX_SESSION_ID", "desktop-first-session");
    std::env::set_var(
        "COLLAB_APPSERVER_SOCKET",
        "/tmp/desktop-first-appserver.sock",
    );
    std::env::remove_var("TMUX_PANE");
    std::env::remove_var("COLLAB_WORKER");

    let result = load_or_create_for_init_at(&host_paths, &scope, None);

    match previous_thread {
        Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
        None => std::env::remove_var("CODEX_THREAD_ID"),
    }
    match previous_session {
        Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
        None => std::env::remove_var("CODEX_SESSION_ID"),
    }
    match previous_pane {
        Some(value) => std::env::set_var("TMUX_PANE", value),
        None => std::env::remove_var("TMUX_PANE"),
    }
    match previous_worker {
        Some(value) => std::env::set_var("COLLAB_WORKER", value),
        None => std::env::remove_var("COLLAB_WORKER"),
    }
    match previous_socket {
        Some(value) => std::env::set_var("COLLAB_APPSERVER_SOCKET", value),
        None => std::env::remove_var("COLLAB_APPSERVER_SOCKET"),
    }

    let identity = result.unwrap();
    assert_eq!(
        identity.worker_id,
        "codex-thread-6465736b746f702d66697273742d746872656164"
    );
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn desktop_peer_rebinds_a_unique_prior_identity_when_no_anchor_matches() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("desktop-thread-without-pane");
    std::fs::create_dir_all(root.join(".agent-collab/runs")).unwrap();
    let state_root = root.join(".collab-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let project_scope = scope
        .route_scope(AppServerId::new(CLI_APP_SERVER_ID).unwrap())
        .unwrap()
        .project_scope_id;
    let unrelated = Identity {
        worker_id: "prior-peer".into(),
        token: "prior-token".into(),
        project_scope: Some(project_scope),
        runtime: None,
        transport: None,
    };
    write_identity(
        &identity_path_at(&host_paths, &unrelated.worker_id).unwrap(),
        &unrelated,
    )
    .unwrap();
    let previous_thread = std::env::var_os("CODEX_THREAD_ID");
    let previous_session = std::env::var_os("CODEX_SESSION_ID");
    let previous_pane = std::env::var_os("TMUX_PANE");
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    std::env::set_var("CODEX_THREAD_ID", "desktop-thread");
    std::env::set_var("CODEX_SESSION_ID", "desktop-session");
    std::env::remove_var("TMUX_PANE");
    std::env::remove_var("COLLAB_WORKER");

    let recovered = identity_for_scope_rebind_at(&host_paths, &scope, None);
    assert!(matches!(
        recovered,
        Ok(ScopeRebindOutcome::Adopted(identity)) if identity.worker_id == "prior-peer"
    ));
    let result = load_or_create_for_init_at(&host_paths, &scope, None);

    match previous_thread {
        Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
        None => std::env::remove_var("CODEX_THREAD_ID"),
    }
    match previous_session {
        Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
        None => std::env::remove_var("CODEX_SESSION_ID"),
    }
    match previous_pane {
        Some(value) => std::env::set_var("TMUX_PANE", value),
        None => std::env::remove_var("TMUX_PANE"),
    }
    match previous_worker {
        Some(value) => std::env::set_var("COLLAB_WORKER", value),
        None => std::env::remove_var("COLLAB_WORKER"),
    }
    let identity = result.unwrap();
    assert_eq!(identity.worker_id, "prior-peer");
    assert!(identity_path_at(&host_paths, &identity.worker_id)
        .unwrap()
        .is_file());
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn appserver_thread_status_retires_only_proven_dead() {
    // A cold thread can still be resumed through turn/start, so it is `Cold`,
    // not `Dead`: it is preserved and never archived.
    assert!(matches!(
        classify_thread_status(&serde_json::json!({
            "thread": {"status": {"type": "notLoaded"}}
        })),
        PeerLiveness::Cold
    ));
    assert!(matches!(
        classify_thread_status(&serde_json::json!({
            "thread": {"status": {"type": "systemError"}}
        })),
        PeerLiveness::Dead
    ));
    assert!(matches!(
        classify_thread_status(&serde_json::json!({
            "thread": {"status": {"type": "idle"}}
        })),
        PeerLiveness::Live
    ));
    assert!(matches!(
        classify_thread_status(&serde_json::json!({"thread": {}})),
        PeerLiveness::Unknown
    ));
    assert!(matches!(
        classify_probe_error("thread not loaded: deadbeef"),
        PeerLiveness::Unknown
    ));
    assert!(matches!(
        classify_probe_error("no rollout found for thread id deadbeef"),
        PeerLiveness::Dead
    ));
    assert!(matches!(
        classify_probe_error("connection refused"),
        PeerLiveness::Unknown
    ));
}

#[test]
fn archive_dead_peers_moves_bytes_and_keeps_the_project_unblocked() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-archive-dead");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
    saved_identity(
        &host_paths,
        &scope,
        "dead-peer",
        Some("session-dead"),
        Some("thread-dead"),
        None,
    );
    let source = host_paths.state_root().join("identities/dead-peer");
    assert!(source.is_dir());

    let dead = read_identity(&source.join("identity.json"))
        .unwrap()
        .unwrap();
    archive_dead_peers(&host_paths, std::slice::from_ref(&dead)).unwrap();
    assert!(!source.exists(), "archived identity must leave the live set");
    let archive_root = host_paths.state_root().join("archives");
    let entries: Vec<_> = std::fs::read_dir(&archive_root)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .collect();
    assert!(!entries.is_empty(), "archive directory must exist");
    assert!(entries
        .iter()
        .any(|entry| entry.path().join("dead-peer").is_dir()));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn archived_pane_credential_requires_exact_route_generation_and_scope() {
    let root = test_root("ci-archive-pane-recovery");
    std::fs::create_dir_all(&root).unwrap();
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
    archive_dead_peers(&host_paths, std::slice::from_ref(&original)).unwrap();
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
    let selected = archived_pane_identity_at(&host_paths, &scope, &candidate, &route)
        .unwrap()
        .unwrap();
    assert_eq!(selected.token, original.token);
    let mut committed = route.clone();
    committed.endpoint_generation += 1;
    committed.session_id = SessionId::new("session-new").unwrap();
    committed.native_thread_id = NativeThreadId::new("thread-new").unwrap();
    assert_eq!(
        archived_pane_identity_at(&host_paths, &scope, &candidate, &committed)
            .unwrap()
            .unwrap()
            .token,
        original.token,
    );
    let mut wrong_generation = route.clone();
    wrong_generation.endpoint_generation += 2;
    assert!(archived_pane_identity_at(&host_paths, &scope, &candidate, &wrong_generation)
        .unwrap()
        .is_none());
    std::fs::remove_dir_all(root).unwrap();
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

#[path = "identity_tests_part2.rs"]
mod part2;
