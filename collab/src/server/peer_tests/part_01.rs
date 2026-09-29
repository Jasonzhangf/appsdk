
pub(crate) struct IsolatedTmux {
    socket: PathBuf,
    owner_root: PathBuf,
}

impl IsolatedTmux {
    pub(crate) fn start(root: &std::path::Path) -> Self {
        Self::start_with_spare_pane(root, true)
    }

    pub(crate) fn start_single(root: &std::path::Path) -> Self {
        Self::start_with_spare_pane(root, false)
    }

    fn start_with_spare_pane(root: &std::path::Path, spare_pane: bool) -> Self {
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
                "-s",
                "worker-snapshot-test",
                "sleep 60",
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
                    "-t",
                    "worker-snapshot-test:0",
                    "sleep 60",
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
                "-s",
                &session,
                "sleep 60",
            ])
            .output()
            .unwrap();
        assert!(
            created.status.success(),
            "add isolated tmux session: {}",
            String::from_utf8_lossy(&created.stderr)
        );
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

    fn kill_server(&self) {
        let _ = Command::new("tmux")
            .args(["-S", self.socket.to_str().unwrap(), "kill-server"])
            .output();
    }
}

thread_local! {
    static REGISTERED_TEST_TMUX: RefCell<Option<(IsolatedTmux, usize)>> = const { RefCell::new(None) };
}

impl Drop for IsolatedTmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-S", self.socket.to_str().unwrap(), "kill-server"])
            .output();
        let _ = std::fs::remove_file(&self.socket);
    }
}

pub(super) fn register_tmux(
    server: &Server,
    id: &str,
    endpoint: crate::proto::TmuxEndpoint,
) -> Resp {
    handle_register_with_app_scope(
        server,
        id.into(),
        format!("token-{id}"),
        server.root.display().to_string(),
        Some(AppServerId::new("tui-default").unwrap()),
        Some(TransportCandidates {
            appserver: None,
            tmux: Some(crate::proto::TmuxCandidate {
                endpoint,
                cwd: server.root.display().to_string(),
            }),
        }),
    )
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
}

fn stop_registered_test_tmux_server() {
    REGISTERED_TEST_TMUX.with(|fixture| {
        let mut fixture = fixture.borrow_mut();
        fixture
            .as_ref()
            .expect("test tmux fixture exists")
            .0
            .kill_server();
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

pub(crate) fn register(server: &Server, id: &str, thread_id: &str) -> Resp {
    let thread_id = thread_id
        .strip_prefix('%')
        .map(|legacy_fixture| format!("thread-{legacy_fixture}"))
        .unwrap_or_else(|| thread_id.to_string());
    let mut endpoint = REGISTERED_TEST_TMUX.with(|fixture| {
        let mut fixture = fixture.borrow_mut();
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
    register_tmux(server, id, endpoint)
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
        Some("master-idle subscription requires the live registered master")
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
        Some("master-idle interval must be exactly 900000 or 3600000 ms")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn review_accept_registers_daemon_owned_pending_merge_with_master_notification() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master merge registration test");
    assert!(create_task(&server, "owner", "task", "feature").ok);

    accept_task(&server, "owner", "task");

    let state = server.state.lock().unwrap();
    assert!(
        state.pending_merges.contains_key("task"),
        "review --accept must register a durable daemon-owned pending merge"
    );
    let request = state.pending_merges["task"].clone();
    assert_eq!(request.owner, "owner");
    assert_eq!(request.task_id, "task");
    let notified = state.msgs.values().any(|message| {
        message.to == "master"
            && message
                .subject
                .as_deref()
                .is_some_and(|subject| subject == "merge-pending:task")
    });
    assert!(
        notified,
        "review --accept must create a durable merge-pending notice for the live master"
    );
    let notice_id = state
        .msgs
        .values()
        .find(|message| message.subject.as_deref() == Some("merge-pending:task"))
        .expect("merge-pending notice")
        .id
        .clone();
    assert_eq!(
        state.delivery_modes.get(&notice_id).map(String::as_str),
        Some("immediate"),
        "the master's merge obligation must wake immediately, not wait for the batch window"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn review_accept_without_master_direct_message_subscription_surfaces_repair() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master merge registration test");
    assert!(create_task(&server, "owner", "task", "feature").ok);

    // Simulate the master losing its default direct-message lease: the pending
    // merge must stay durable and the review response must carry the explicit
    // mailbox-only repair terminal, never a silent success.
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
    assert_eq!(
        response.data["notification"],
        serde_json::json!("mailbox-only-no-subscription")
    );
    assert_eq!(response.data["repair_required"], serde_json::json!(true));
    assert!(
        response.data["failure"]
            .as_str()
            .is_some_and(|f| f == "notification_subscription_missing")
    );
    assert!(
        server.state.lock().unwrap().pending_merges.contains_key("task"),
        "the obligation must remain durable even without a wake subscription"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn pending_merge_with_unbound_candidate_fails_closed_on_integrated() {
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
    // Deliver with a worktree that does not resolve to a git commit, so the
    // accepted candidate is unbound (None) while the pending merge exists.
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
    assert_eq!(
        response.data["merge_pending"],
        serde_json::json!(true),
        "a live-master accept that registers a pending merge must report it"
    );
    {
        let state = server.state.lock().unwrap();
        assert!(state.pending_merges.contains_key("task"));
        assert!(
            state.pending_merges["task"].candidate_commit.is_none(),
            "this test deliberately leaves the delivered candidate unbound"
        );
        assert_eq!(state.tasks["task"].status, "accepted");
    }

    // An unrelated main-reachable SHA must not satisfy an unbound obligation.
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
    assert_eq!(integrated.error.as_deref(), Some("TASK_MERGE_PENDING"));
    assert!(
        integrated.data["candidate_commit"].is_null(),
        "the fail-closed response must expose the unbound candidate"
    );
    {
        let state = server.state.lock().unwrap();
        assert!(state.pending_merges.contains_key("task"));
        assert_eq!(state.tasks["task"].status, "accepted");
    }

    // Rework is the explicit recovery path: it resolves the unprovable
    // obligation so the owner can re-deliver with a resolvable candidate.
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
    // Masterless accept leaves the owner-local lifecycle intact: no daemon
    // pending merge exists, so merge_pending must be false even though accept
    // succeeded.
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
    assert_eq!(response.data["merge_pending"], serde_json::json!(false));
    assert!(!server.state.lock().unwrap().pending_merges.contains_key("task"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn close_refuses_pending_merge_until_task_integrated_resolves_it() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master close gate test");
    initialize_main(&root);
    assert!(create_task(&server, "owner", "task", "feature").ok);

    accept_task(&server, "owner", "task");

    let refused = handle_task_close(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        false,
        None,
    );
    assert!(!refused.ok, "{refused:?}");
    assert_eq!(refused.error.as_deref(), Some("TASK_MERGE_PENDING"));

    let owner_integrated = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        current_head(&root),
        "owner bypass attempt".into(),
    );
    assert!(!owner_integrated.ok, "owner could not bypass the master merge obligation");
    assert_eq!(owner_integrated.error.as_deref(), Some("TASK_MERGE_PENDING"));

    let head = current_head(&root);
    assert!(
        handle_task_integrated(
            &server,
            "master".into(),
            "token-master".into(),
            "task".into(),
            head,
            "main merged".into(),
        )
        .ok
    );
    {
        let state = server.state.lock().unwrap();
        assert!(
            !state.pending_merges.contains_key("task"),
            "task integrated must resolve the pending merge"
        );
        assert_eq!(state.tasks["task"].status, "merged");
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
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn integrated_supersedes_stale_merge_pending_notice() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master merge notice test");
    initialize_main(&root);
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");

    let notice_id = {
        let state = server.state.lock().unwrap();
        state
            .msgs
            .values()
            .find(|message| {
                message.to == "master"
                    && message.subject.as_deref() == Some("merge-pending:task")
                    && matches!(message.state.as_str(), "pending" | "delivered")
            })
            .expect("merge-pending notice must exist")
            .id
            .clone()
    };

    let head = current_head(&root);
    assert!(
        handle_task_integrated(
            &server,
            "master".into(),
            "token-master".into(),
            "task".into(),
            head,
            "merged onto main".into(),
        )
        .ok
    );

    let state = server.state.lock().unwrap();
    assert!(!state.pending_merges.contains_key("task"));
    assert_eq!(
        state.msgs[&notice_id].state, "superseded",
        "a resolved obligation must not remain readable as an actionable P0 notice"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn review_accept_fails_closed_when_master_presence_is_unknown() {
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

    // Dropping the tmux server makes the master's presence probe Unknown, which
    // is ambiguous authority: accept must fail closed instead of silently
    // downgrading to owner self-integration.
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
    assert!(!response.ok);
    assert_eq!(response.error.as_deref(), Some("MASTER_PRESENCE_UNKNOWN"));
    let state = server.state.lock().unwrap();
    assert!(
        !state.pending_merges.contains_key("task"),
        "an ambiguous master presence must not register a merge obligation"
    );
    assert_eq!(state.tasks["task"].status, "delivered");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}
