
pub(crate) struct IsolatedTmux {
    socket: PathBuf,
    owner_root: PathBuf,
    pane_command: String,
    sessions: RefCell<Vec<String>>,
}

impl IsolatedTmux {
    pub(crate) fn start(root: &std::path::Path) -> Self {
        Self::start_with_spare_pane(root, true, "sleep 60")
    }

    pub(crate) fn start_single(root: &std::path::Path) -> Self {
        Self::start_with_spare_pane(root, false, "sleep 60")
    }

    fn start_with_spare_pane(root: &std::path::Path, spare_pane: bool, pane_command: &str) -> Self {
        static SOCKET_SEQ: AtomicU64 = AtomicU64::new(0);
        let socket = std::env::temp_dir().join(format!(
            "ctmux-{}-{}.sock",
            std::process::id(),
            SOCKET_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let output = Command::new("tmux")
            .args([
                "-S",
                socket.to_str().unwrap(),
                "new-session",
                "-d",
                "-c",
                root.to_str().unwrap(),
                "-s",
                "worker-snapshot-test",
                pane_command,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "start isolated tmux: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if spare_pane {
            let split = Command::new("tmux")
                .args([
                    "-S",
                    socket.to_str().unwrap(),
                    "split-window",
                    "-d",
                    "-c",
                    root.to_str().unwrap(),
                    "-t",
                    "worker-snapshot-test:0",
                    pane_command,
                ])
                .output()
                .unwrap();
            assert!(
                split.status.success(),
                "split isolated tmux: {}",
                String::from_utf8_lossy(&split.stderr)
            );
        }
        Self {
            socket,
            owner_root: root.to_path_buf(),
            pane_command: pane_command.into(),
            sessions: RefCell::new(vec!["worker-snapshot-test".into()]),
        }
    }

    pub(crate) fn endpoints(&self) -> Vec<crate::proto::TmuxEndpoint> {
        let panes = Command::new("tmux")
            .args([
                "-S",
                self.socket.to_str().unwrap(),
                "list-panes",
                "-a",
                "-F",
                "#{pane_id}",
            ])
            .output()
            .unwrap();
        assert!(panes.status.success());
        String::from_utf8(panes.stdout)
            .unwrap()
            .lines()
            .map(|pane_id| {
                let output = Command::new("tmux")
                    .args([
                        "-S",
                        self.socket.to_str().unwrap(),
                        "display-message",
                        "-p",
                        "-t",
                        pane_id,
                        "#{pid}\t#{session_id}\t#{pane_id}\t#{pane_pid}",
                    ])
                    .output()
                    .unwrap();
                assert!(output.status.success());
                let line = String::from_utf8(output.stdout).unwrap();
                let fields = line.trim_end().split('\t').collect::<Vec<_>>();
                assert_eq!(fields.len(), 4);
                crate::proto::TmuxEndpoint {
                    socket_path: self.socket.to_string_lossy().into_owned(),
                    server_pid: fields[0].parse().unwrap(),
                    tmux_session_id: fields[1].into(),
                    pane_id: fields[2].into(),
                    pane_pid: fields[3].parse().unwrap(),
                    codex_session_id: None,
                    codex_thread_id: None,
                }
            })
            .collect()
    }

    pub(crate) fn add_session(&self) -> crate::proto::TmuxEndpoint {
        self.add_session_at(&self.owner_root)
    }

    pub(crate) fn add_session_at(&self, root: &Path) -> crate::proto::TmuxEndpoint {
        static SESSION_SEQ: AtomicU64 = AtomicU64::new(0);
        let session = format!(
            "worker-snapshot-test-{}-{}",
            std::process::id(),
            SESSION_SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let created = Command::new("tmux")
            .args([
                "-S",
                self.socket.to_str().unwrap(),
                "new-session",
                "-d",
                "-c",
                root.to_str().unwrap(),
                "-s",
                &session,
                &self.pane_command,
            ])
            .output()
            .unwrap();
        assert!(
            created.status.success(),
            "add isolated tmux session: {}",
            String::from_utf8_lossy(&created.stderr)
        );
        self.sessions.borrow_mut().push(session.clone());
        let output = Command::new("tmux")
            .args([
                "-S",
                self.socket.to_str().unwrap(),
                "display-message",
                "-p",
                "-t",
                &format!("{session}:0"),
                "#{pid}\t#{session_id}\t#{pane_id}\t#{pane_pid}",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "inspect isolated tmux session: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let fields = String::from_utf8(output.stdout)
            .unwrap()
            .trim_end()
            .split('\t')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(fields.len(), 4);
        crate::proto::TmuxEndpoint {
            socket_path: self.socket.to_string_lossy().into_owned(),
            server_pid: fields[0].parse().unwrap(),
            tmux_session_id: fields[1].clone(),
            pane_id: fields[2].clone(),
            pane_pid: fields[3].parse().unwrap(),
            codex_session_id: None,
            codex_thread_id: None,
        }
    }

    fn kill_pane(&self, pane_id: &str) {
        let output = Command::new("tmux")
            .args([
                "-S",
                self.socket.to_str().unwrap(),
                "kill-pane",
                "-t",
                pane_id,
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "kill test-owned tmux pane {pane_id}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn close_owned_sessions(&self) {
        let sessions = self.sessions.borrow_mut().drain(..).collect::<Vec<_>>();
        for session in sessions {
            let _ = Command::new("tmux")
                .args([
                    "-S",
                    self.socket.to_str().unwrap(),
                    "kill-session",
                    "-t",
                    &session,
                ])
                .output();
        }
    }
}

thread_local! {
    static REGISTERED_TEST_TMUX: RefCell<Option<(IsolatedTmux, usize)>> = const { RefCell::new(None) };
}

impl Drop for IsolatedTmux {
    fn drop(&mut self) {
        self.close_owned_sessions();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Defined committed owner for tests that begin with an existing peer. This
/// seeds typed state; it does not test public initial participant admission.
pub(super) fn register_tmux(
    server: &Server,
    id: &str,
    endpoint: crate::proto::TmuxEndpoint,
) -> Resp {
    let app = AppServerId::new("tui-default").unwrap();
    let cwd = server.root.display().to_string();
    let transport = SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some(endpoint.socket_path.clone()),
        namespace: Some(endpoint.tmux_session_id.clone()),
        session_id: endpoint.codex_session_id.clone().or_else(|| Some(endpoint.tmux_session_id.clone())),
        thread_id: endpoint.codex_thread_id.clone().or_else(|| Some(endpoint.pane_id.clone())),
        tmux_endpoint: Some(endpoint),
        capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
        self_check: "prior committed peer fixture".into(),
    };
    let response = seed_committed_owner(server, id, &format!("token-{id}"), &transport, &cwd, Some(app.clone()));
    if response.ok {
        commit_current_thread_route_for_runtime(server, server, id, &cwd, Some(&app)).unwrap();
        restore_registered_peer_default_leases(server);
    }
    response
}

/// Seed a defined prior typed owner. This is fixture state, not evidence that
/// a public initial Register passed participant admission.
pub(super) fn seed_committed_owner(
    server: &Server,
    worker_id: &str,
    token: &str,
    transport: &SelectedTransport,
    cwd: &str,
    app_scope: Option<AppServerId>,
) -> Resp {
    let prepared = prepare_register_typed(
        server, worker_id, token, transport, cwd,
        Some(GlobalState::canonical_project_scope(&server.root).unwrap()), app_scope, false,
    ).unwrap();
    consume_prepared_register_typed(server, worker_id, transport, prepared, None)
}

fn retire_registered_test_pane(binding: &crate::server::global_state::RuntimeBinding) {
    let pane_id = binding
        .tmux_endpoint
        .as_ref()
        .expect("test peer has a tmux endpoint")
        .pane_id
        .clone();
    REGISTERED_TEST_TMUX.with(|fixture| {
        fixture
            .borrow()
            .as_ref()
            .expect("test tmux fixture exists")
            .0
            .kill_pane(&pane_id)
    });
}

pub(super) fn kill_registered_worker_pane(server: &Server, worker_id: &str) {
    let endpoint = server
        .state
        .lock()
        .unwrap()
        .workers
        .get(worker_id)
        .and_then(|worker| worker.transport.as_ref())
        .and_then(|transport| transport.tmux_endpoint.as_ref())
        .expect("test worker has a registered tmux endpoint")
        .clone();
    REGISTERED_TEST_TMUX.with(|fixture| {
        fixture
            .borrow()
            .as_ref()
            .expect("test tmux fixture exists")
            .0
            .kill_pane(&endpoint.pane_id)
    });
    eprintln!("owned killed pane worker={worker_id} endpoint_probe={:?}",
        crate::client::adapters::tmux::probe(&endpoint));
}

fn stop_registered_test_tmux_server() {
    REGISTERED_TEST_TMUX.with(|fixture| {
        let mut fixture = fixture.borrow_mut();
        fixture
            .as_ref()
            .expect("test tmux fixture exists")
            .0
            .close_owned_sessions();
        *fixture = None;
    });
}

#[test]
fn handoff_to_unregistered_recipient_fails_typed_before_recording() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "operator".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        "candidate awaits routing".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok);
    let error = response.error.unwrap_or_default();
    assert!(
        error.contains("HANDOFF_TARGET_UNRESOLVED") && error.contains("operator"),
        "unexpected error: {error}"
    );
    assert_eq!(response.data["error_code"], "HANDOFF_TARGET_UNRESOLVED");
    assert_eq!(response.data["unresolved"]["kind"], "recipient");
    assert_eq!(response.data["unresolved"]["value"], "operator");
    assert_eq!(response.data["reason"], "recipient_not_registered");
    assert_eq!(response.data["recorded"], false);
    assert!(
        server.state.lock().unwrap().msgs.is_empty(),
        "an unresolved handoff target must not create a durable message"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_missing_worktree_path_fails_typed_before_recording() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let missing = root.join("playground/does-not-exist").display().to_string();
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        format!("preflight implementation lives at {missing}"),
        None,
        "immediate".into(),
    );
    assert!(!response.ok);
    let error = response.error.unwrap_or_default();
    assert!(
        error.contains("HANDOFF_TARGET_UNRESOLVED") && error.contains("does-not-exist"),
        "unexpected error: {error}"
    );
    assert_eq!(response.data["unresolved"]["kind"], "worktree");
    assert_eq!(response.data["reason"], "worktree_path_missing");
    assert_eq!(response.data["recorded"], false);
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_registered_recipient_and_existing_path_succeeds() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let sender_endpoint = registered_binding(&server, "handoff-sender")
        .tmux_endpoint
        .expect("sender has a tmux binding");
    let recipient_endpoint = registered_binding(&server, "handoff-owner")
        .tmux_endpoint
        .expect("recipient has a tmux binding");
    assert_eq!(sender_endpoint.socket_path, recipient_endpoint.socket_path);
    assert_ne!(
        sender_endpoint.tmux_session_id,
        recipient_endpoint.tmux_session_id
    );
    assert_ne!(sender_endpoint.pane_id, recipient_endpoint.pane_id);
    let worktree = root.join("playground/handoff-live");
    std::fs::create_dir_all(&worktree).unwrap();
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        format!("preflight implementation lives at {}", worktree.display()),
        None,
        "immediate".into(),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["durable"], true);
    assert!(response.data["msg_id"].is_string());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_relative_missing_worktree_fails_typed() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    std::fs::create_dir_all(root.join("playground")).unwrap();
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        "preflight implementation lives at playground/gone-relative".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok, "{response:?}");
    let error = response.error.unwrap_or_default();
    assert!(
        error.contains("HANDOFF_TARGET_UNRESOLVED") && error.contains("gone-relative"),
        "unexpected error: {error}"
    );
    assert_eq!(response.data["reason"], "worktree_path_missing");
    assert_eq!(response.data["recorded"], false);
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_unrelated_absolute_worktree_leaf_still_delivers() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let unrelated = "/Volumes/extension/code/other-project/worktree";
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("unrelated worktree-leaf path".into()),
        format!("see {unrelated} for reference"),
        None,
        "immediate".into(),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["durable"], true);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_prose_url_worktree_segment_still_delivers() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("prose url".into()),
        "reference https://example.com/docs/worktree for background".into(),
        None,
        "immediate".into(),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["durable"], true);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_dot_prefixed_missing_worktree_fails_typed() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    std::fs::create_dir_all(root.join("playground")).unwrap();
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("dot-prefixed worktree reference".into()),
        "preflight implementation lives at ./playground/gone-wt".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok, "{response:?}");
    let error = response.error.unwrap_or_default();
    assert!(
        error.contains("HANDOFF_TARGET_UNRESOLVED") && error.contains("gone-wt"),
        "unexpected error: {error}"
    );
    assert_eq!(response.data["reason"], "worktree_path_missing");
    assert_eq!(response.data["recorded"], false);
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_registered_task_worktree_outside_playground_fails_typed() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let registered = root.join("outside/wt");
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "task-handoff-registered-wt".into(),
            owner: "handoff-owner".into(),
            created_by: "handoff-owner".into(),
            feature_id: None,
            worktree_path: Some(registered.display().to_string()),
            branch: None,
            base_commit: None,
            priority: "p1".into(),
            status: "working".into(),
            next_step: None,
            wait: None,
            isolation_mode: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        },
    }]);
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("registered worktree reference".into()),
        format!("preflight implementation lives at {}", registered.display()),
        None,
        "immediate".into(),
    );
    assert!(!response.ok, "{response:?}");
    assert_eq!(response.data["reason"], "worktree_path_missing");
    assert_eq!(response.data["recorded"], false);
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_unrelated_playground_path_still_delivers() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("unrelated path mention".into()),
        "see /Volumes/extension/code/other-project/playground/other-worktree for reference".into(),
        None,
        "immediate".into(),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["durable"], true);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_dedup_resend_survives_a_path_that_disappeared_after_delivery() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let worktree = root.join("playground/handoff-dedup");
    std::fs::create_dir_all(&worktree).unwrap();
    let body = format!("preflight implementation lives at {}", worktree.display());
    let first = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        body.clone(),
        None,
        "immediate".into(),
    );
    assert!(first.ok, "{first:?}");
    let msg_id = first.data["msg_id"].as_str().unwrap().to_owned();
    std::fs::remove_dir_all(&worktree).unwrap();
    let resend = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        body,
        None,
        "immediate".into(),
    );
    assert_eq!(resend.data["deduplicated"], true);
    assert_eq!(resend.data["msg_id"], msg_id.as_str());
    std::fs::remove_dir_all(root).unwrap();
}

pub(crate) fn test_server() -> (Server, PathBuf) {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("collab-peer-{}-{n}", std::process::id()));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(server_dir.join("journal.jsonl"))
        .unwrap();
    (
        Server {
            config: crate::config::Config::default(),
            root: root.clone(),
            storage_root: root.clone(),
            journal_path: root.join(".agent-collab/server/journal.jsonl"),
            host_paths: HostPaths::for_state_root(root.join("host-state")).unwrap(),
            state: Mutex::new(State::default()),
            journal: Mutex::new(journal),
            appserver_candidate_check: Arc::new(|candidate| {
                Ok(test_appserver_transport(&candidate.thread_id))
            }),
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
            mailbox_notify: tokio::sync::Notify::new(),
        },
        root,
    )
}

fn configured_test_worktree(server: &mut Server, root: &Path, slug: &str) -> PathBuf {
    let base = root.with_file_name(format!(
        "{}-external",
        root.file_name().unwrap().to_string_lossy()
    ));
    let path = base.join(configured_project_key(root).unwrap()).join(slug);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    server.config.worktree.base = Some(base);
    path
}

pub(crate) fn test_appserver_transport(thread_id: &str) -> SelectedTransport {
    SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some(format!("session-{thread_id}")),
        thread_id: Some(thread_id.into()),
        tmux_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    }
}

fn test_tmux_transport(thread_id: &str) -> SelectedTransport {
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: "/tmp/collab-test-tmux.sock".into(),
        server_pid: 1,
        tmux_session_id: "$test".into(),
        pane_id: "%0".into(),
        pane_pid: 2,
        codex_session_id: Some(format!("session-{thread_id}")),
        codex_thread_id: Some(thread_id.into()),
    };
    SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some(endpoint.socket_path.clone()),
        namespace: Some(endpoint.tmux_session_id.clone()),
        session_id: endpoint.codex_session_id.clone(),
        thread_id: endpoint.codex_thread_id.clone(),
        tmux_endpoint: Some(endpoint),
        capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
        self_check: "test tmux transport".into(),
    }
}

fn fixture_peer_endpoint(server: &Server, thread_id: &str) -> crate::proto::TmuxEndpoint {
    let thread_id = thread_id
        .strip_prefix('%')
        .map(|legacy_fixture| format!("thread-{legacy_fixture}"))
        .unwrap_or_else(|| thread_id.to_string());
    let mut endpoint = REGISTERED_TEST_TMUX.with(|fixture| {
        let mut fixture = fixture.borrow_mut();
        // A test can create multiple owned roots. The participant script and
        // pane cwd belong to one root and must not outlive its fixture.
        if fixture.as_ref().is_some_and(|(tmux, _)| tmux.owner_root != server.root) {
            *fixture = None;
        }
        let (tmux, endpoint_index) =
            fixture.get_or_insert_with(|| (IsolatedTmux::start(&server.root), 0));
        let endpoint = if *endpoint_index == 0 {
            tmux.endpoints().remove(0)
        } else {
            tmux.add_session()
        };
        *endpoint_index += 1;
        endpoint
    });
    endpoint.codex_session_id = Some(format!("session-{thread_id}"));
    endpoint.codex_thread_id = Some(thread_id);
    endpoint
}

/// Prior-state fixture for task, notification and readback tests. Explicit
/// registration effects use register_public and its real participant return.
pub(crate) fn register(server: &Server, id: &str, thread_id: &str) -> Resp {
    register_tmux(server, id, fixture_peer_endpoint(server, thread_id))
}

/// Seed a committed peer without starting a tmux server. Role projection and
/// scheduling contracts do not need a live pane.
pub(crate) fn register_synthetic_peer(server: &Server, id: &str, thread_id: &str) -> Resp {
    static PANE_SEQ: AtomicU64 = AtomicU64::new(0);
    let pane = PANE_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    register_tmux(
        server,
        id,
        crate::proto::TmuxEndpoint {
            socket_path: format!("/tmp/collab-synthetic-{}-{pane}.sock", std::process::id()),
            server_pid: 1,
            tmux_session_id: format!("${pane}"),
            pane_id: format!("%{pane}"),
            pane_pid: 2,
            codex_session_id: Some(format!("session-{thread_id}")),
            codex_thread_id: Some(thread_id.into()),
        },
    )
}

fn register_public(server: &Arc<Server>, id: &str, thread_id: &str) -> Resp {
    let endpoint = fixture_peer_endpoint(server, thread_id);
    let app = AppServerId::new("tui-default").unwrap();
    let prior = server.state.lock().unwrap().global.projects.values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| binding.agent_id.as_str() == id && binding.app_scope_id == app).cloned();
    let context = match prior {
        Some(binding) => ProjectContext::for_registered_route(&server.root, &crate::identity::RuntimeIdentity {
            agent_id: binding.agent_id, runtime_id: binding.runtime_id, appserver_id: binding.app_scope_id,
            endpoint_generation: binding.endpoint_generation, binding_id: binding.binding_id,
            session_id: binding.session_id, native_thread_id: binding.native_thread_id,
        }).unwrap(),
        None => ProjectContext::for_registered_root_with_app(&server.root, app).unwrap(),
    };
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    super::host_route_registry_tests::register_host_fixture(&manager, Some(context),
        Req::register(id.into(), format!("token-{id}"), server.root.display().to_string(),
            Some(TransportCandidates { desktop_bridge: None, appserver: None, dsh: None,
                tmux: Some(crate::proto::TmuxCandidate { endpoint, cwd: server.root.display().to_string() }) })),
    ).1
}

/// Model the provenance committed by an explicit managed-child Bind after
/// the child has already been registered in this test server.
pub(crate) fn bind_test_subagent_record(
    server: &Server,
    mut record: crate::subagent::Record,
) -> crate::subagent::Record {
    let state = server.state.lock().unwrap();
    let binding = state
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| {
            binding.agent_id.as_str() == record.peer
                && binding.native_thread_id.as_ref().map(|thread| thread.as_str())
                    == record.thread_id.as_deref()
        })
        .unwrap_or_else(|| panic!("registered child binding missing for {}", record.peer));
    record.create_operation_id = Some(format!("test-create-{}", record.id));
    record.binding_id = Some(binding.binding_id.as_str().to_owned());
    record.endpoint_generation = Some(binding.endpoint_generation);
    record
}

fn promote_master(server: &Server, worker_id: &str, approval: &str) {
    let response = handle_master_promote(
        server,
        worker_id.into(),
        format!("token-{worker_id}"),
        approval.into(),
    );
    assert!(
        response.ok,
        "master promotion for {worker_id} failed: {:?}",
        response.error
    );
}

fn send_command(root: &Path, id: &str) -> crate::proto::CommandEnvelope {
    use crate::identity::{AppServerId, BindingId, CommandId, OperationId};
    use crate::proto::CommandEnvelope;
    let app = AppServerId::new("tui-default").unwrap();
    let scope = crate::scope::RouteScope::for_registered_project(app, root).unwrap();
    CommandEnvelope::new(
        CommandId::new(format!("command-{id}")).unwrap(),
        OperationId::new(format!("operation-{id}")).unwrap(),
        BindingId::new(format!("binding-{id}")).unwrap(),
        1,
        scope,
        None,
        None,
        None,
        None,
    )
}

fn authenticated_send(root: &Path, from: &str, to: &str, subject: &str) -> Req {
    Req::Send {
        from: from.into(),
        worker_id: Some(from.into()),
        token: Some(format!("token-{from}")),
        command: Some(send_command(root, from)),
        to: to.into(),
        mtype: "notify".into(),
        subject: Some(subject.into()),
        body: "body".into(),
        in_reply_to: None,
        reply_policy: None,
        target_session: None,
        delivery: "immediate".into(),
    }
}

#[test]
fn master_idle_subscription_is_restricted_to_the_live_master_and_supported_intervals() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    register(&server, "worker", "%worker");
    promote_master(&server, "master", "user-approved");

    let worker = handle_notification_subscribe(
        &server,
        "worker".into(),
        "token-worker".into(),
        "master-idle".into(),
        Some("master-idle".into()),
        None,
        Vec::new(),
        Some(900_000),
        3,
        86_400,
    );
    assert!(!worker.ok);
    assert_eq!(
        worker.error.as_deref(),
        Some("master-idle subscription requires the current registered master")
    );

    let accepted = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "master-idle".into(),
        Some("master-idle".into()),
        None,
        Vec::new(),
        Some(3_600_000),
        3,
        86_400,
    );
    assert!(accepted.ok, "{}", accepted.error.unwrap_or_default());

    let invalid = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "master-idle".into(),
        Some("master-idle".into()),
        None,
        Vec::new(),
        Some(60_000),
        3,
        86_400,
    );
    assert!(!invalid.ok);
    assert_eq!(
        invalid.error.as_deref(),
        Some("master-idle interval must be project-patrol 1500000, compatibility 900000, or 3600000 ms")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn review_accept_registers_daemon_owned_pending_merge_with_master_notification() {
    // Contract change: review --accept no longer registers a daemon-owned
    // pending merge and no longer writes a merge-pending notice for the live
    // master. The accept response carries only status/delivery_complete/
    // reviewer/evidence/disposition/next_action; there is no merge_pending or
    // notification field. The accepted task carries responsibility to close.
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master merge registration test");
    assert!(create_task(&server, "owner", "task", "feature").ok);

    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate commit and gates passed".into()),
            Some("/tmp/candidate".into()),
        )
        .ok
    );
    let response = handle_task_review(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        true,
        false,
        "review passed".into(),
    );
    assert!(response.ok, "{}", response.error.unwrap_or_default());
    assert_eq!(response.data["status"], serde_json::json!("accepted"));
    assert_eq!(response.data["disposition"], serde_json::json!("accepted"));
    assert!(
        response.data.get("merge_pending").is_none(),
        "the accept response must not expose a merge_pending field"
    );
    assert!(
        response.data.get("notification").is_none(),
        "the accept response must not expose a merge notification field"
    );

    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["task"].status, "accepted");
    assert!(
        !state.pending_merges.contains_key("task"),
        "review --accept must not register a pending merge"
    );
    assert!(
        !state.msgs.values().any(|message| {
            message.to == "master"
                && message
                    .subject
                    .as_deref()
                    .is_some_and(|subject| subject == "merge-pending:task")
        }),
        "review --accept must not create a merge-pending notice for the master"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn review_accept_without_master_direct_message_subscription_surfaces_repair() {
    // Contract change: review --accept no longer registers a merge obligation,
    // so there is no mailbox-only repair terminal and no merge notification on
    // accept. Accept succeeds regardless of master subscription state. Open
    // gap: whether the master must still observe an accepted task through some
    // other channel is unresolved (no alternative black-box evidence yet).
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master merge registration test");
    assert!(create_task(&server, "owner", "task", "feature").ok);

    // Simulate the master losing its default direct-message lease.
    let drop_ids: Vec<String> = server
        .state
        .lock()
        .unwrap()
        .notification_subscriptions
        .values()
        .filter(|sub| sub.worker_id == "master" && sub.event == "direct-message")
        .map(|sub| sub.id.clone())
        .collect();
    {
        let mut state = server.state.lock().unwrap();
        for id in drop_ids {
            state.notification_subscriptions.remove(&id);
        }
    }
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate commit and gates passed".into()),
            Some("/tmp/candidate".into()),
        )
        .ok
    );
    let response = handle_task_review(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        true,
        false,
        "review passed".into(),
    );
    assert!(response.ok, "{}", response.error.unwrap_or_default());
    assert_eq!(response.data["status"], serde_json::json!("accepted"));
    assert_eq!(response.data["disposition"], serde_json::json!("accepted"));
    assert!(
        response.data.get("notification").is_none(),
        "review accept must not surface a notification repair terminal"
    );
    assert!(
        response.data.get("repair_required").is_none(),
        "review accept must not surface a repair_required field"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["task"].status, "accepted");
    assert!(
        !state.pending_merges.contains_key("task"),
        "review accept must not register a pending merge without a wake subscription"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn pending_merge_with_unbound_candidate_fails_closed_on_integrated() {
    // Contract change: delivery no longer binds a Git candidate commit and
    // review accept no longer registers a pending merge, so an unresolvable
    // worktree is no longer a review or integrated error. integrated/finalize
    // are disabled; rework -> re-deliver -> review -> close is the recovery
    // path.
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved unbound candidate test");
    initialize_main(&root);
    assert!(create_task(&server, "owner", "task", "feature").ok);
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate commit and gates passed".into()),
            Some("/tmp/candidate-nonexistent".into()),
        )
        .ok
    );
    let response = handle_task_review(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        true,
        false,
        "review passed".into(),
    );
    assert!(response.ok, "{}", response.error.unwrap_or_default());
    assert_eq!(response.data["status"], serde_json::json!("accepted"));
    assert_eq!(response.data["disposition"], serde_json::json!("accepted"));
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["task"].status, "accepted");
    assert!(
        !state.pending_merges.contains_key("task"),
        "an unbound candidate must not register a pending merge"
    );
    drop(state);

    // integrated is retired; it must fail closed without a side effect.
    let head = current_head(&root);
    let integrated = handle_task_integrated(
        &server,
        "master".into(),
        "token-master".into(),
        "task".into(),
        head,
        "unrelated main sha".into(),
    );
    assert!(!integrated.ok, "{integrated:?}");
    assert_eq!(
        integrated.error.as_deref(),
        Some("COLLAB_GIT_OWNERSHIP_DISABLED: task integration, finalize-cleanup and worktree/branch cleanup are retired; deliver, review and close carry task responsibility without git or resource operations")
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["task"].status,
        "accepted",
        "integrated must fail closed without changing task status"
    );

    // Rework is the explicit recovery path.
    assert!(
        handle_task_update(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("rework".into()),
            Some("candidate must be bound before re-accept".into()),
        )
        .ok
    );
    assert_eq!(server.state.lock().unwrap().tasks["task"].status, "rework");
    assert!(!server.state.lock().unwrap().pending_merges.contains_key("task"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn review_accept_merge_pending_field_reflects_registered_obligation() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate commit and gates passed".into()),
            Some("/tmp/candidate-nonexistent".into()),
        )
        .ok
    );
    // Contract change: the review response no longer carries a merge_pending
    // field at all. The public contract must state the new response shape
    // explicitly; there is no compatibility merge_pending field.
    let response = handle_task_review(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        true,
        false,
        "review passed".into(),
    );
    assert!(response.ok, "{}", response.error.unwrap_or_default());
    assert_eq!(response.data["status"], serde_json::json!("accepted"));
    assert_eq!(response.data["disposition"], serde_json::json!("accepted"));
    assert!(
        response.data.get("merge_pending").is_none(),
        "the accept response must not expose a merge_pending field"
    );
    assert!(!server.state.lock().unwrap().pending_merges.contains_key("task"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn close_refuses_pending_merge_until_task_integrated_resolves_it() {
    // Contract change: task integrated/finalize are disabled; deliver -> review
    // -> close carries task responsibility. A historical pending merge
    // inherited from migrated journal data must still be cleared by close so it
    // stops appearing in collab context / status. The assertion below verifies
    // that current close behavior clears the legacy obligation.
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master close gate test");
    initialize_main(&root);
    assert!(create_task(&server, "owner", "task", "feature").ok);

    accept_task(&server, "owner", "task");

    // Simulate a historical pending merge inherited from migrated journal data.
    {
        let mut state = server.state.lock().unwrap();
        state.pending_merges.insert(
            "task".into(),
            state::PendingMerge {
                task_id: "task".into(),
                owner: "owner".into(),
                requested_by: "master".into(),
                requested_ms: now_ms(),
                candidate_commit: None,
            },
        );
    }

    let closed = handle_task_close(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert_eq!(closed.data["disposition"], serde_json::json!("closed"));
    assert_eq!(
        closed.data["cleanup"]["result"],
        serde_json::json!("not_managed")
    );
    // Close clears the historical pending merge obligation.
    assert!(
        !server.state.lock().unwrap().pending_merges.contains_key("task"),
        "close must clear a historical pending merge obligation"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn integrated_supersedes_stale_merge_pending_notice() {
    // Contract change: review accept no longer creates a merge-pending notice
    // and integrated/finalize are disabled. Legacy merge-pending notices
    // inherited from migrated journal data must still be superseded when the
    // task closes so they do not remain actionable P0 notices. The assertion
    // below verifies the current close cleanup.
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master merge notice test");
    initialize_main(&root);
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");

    {
        let mut state = server.state.lock().unwrap();
        state.msgs.insert(
            "legacy-merge-notice".into(),
            Message {
                id: "legacy-merge-notice".into(),
                from: "collab-server".into(),
                to: "master".into(),
                mtype: "notification".into(),
                subject: Some("merge-pending:task".into()),
                body: "legacy merge pending".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        );
    }

    let closed = handle_task_close(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());

    let state = server.state.lock().unwrap();
    assert_eq!(
        state.msgs["legacy-merge-notice"].state, "superseded",
        "a resolved obligation must not remain readable as an actionable P0 notice"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn review_accept_succeeds_with_merge_obligation_when_master_presence_is_unknown() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master presence test");
    assert!(create_task(&server, "owner", "task", "feature").ok);

    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate commit and gates passed".into()),
            Some("/tmp/candidate".into()),
        )
        .ok
    );

    // Dropping the tmux server leaves the master's transport Unknown. Contract
    // change: transport reachability no longer gates review accept, and accept
    // no longer registers a master merge obligation or notice.
    stop_registered_test_tmux_server();

    let response = handle_task_review(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        true,
        false,
        "review passed".into(),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["status"], serde_json::json!("accepted"));
    assert_eq!(response.data["disposition"], serde_json::json!("accepted"));
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["task"].status, "accepted");
    assert!(
        !state.pending_merges.contains_key("task"),
        "review accept must not register a merge obligation when master presence is unknown"
    );
    assert!(
        !state.msgs.values().any(|message| {
            message.to == "master" && message.subject.as_deref() == Some("merge-pending:task")
        }),
        "review accept must not write a merge-pending notice"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}
