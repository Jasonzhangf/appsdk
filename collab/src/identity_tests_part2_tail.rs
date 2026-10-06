#[test]
fn identifier_validation_rejects_empty_and_control_values() {
    assert!(AgentId::new("").is_err());
    assert!(RuntimeId::new("runtime\n1").is_err());
    assert!(DispatchId::new("d".repeat(MAX_ID_LENGTH + 1)).is_err());
}

#[test]
fn identity_anchor_conflict_checks_overlap_not_project_residency() {
    let base = Identity {
        worker_id: "base-peer".into(),
        token: "base-token".into(),
        project_scope: None,
        runtime: Some(RuntimeIdentity::cli_adapter("base-peer").unwrap()),
        transport: Some(SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("other-session".into()),
            thread_id: Some("other-thread".into()),
            tmux_endpoint: None,
            capabilities: vec![],
            self_check: "ok".into(),
        }),
    };

    let candidate =
        |pane_id: &str| tmux_candidate(Some("new-session"), Some("new-thread"), pane_id);

    // A live peer on an unrelated thread/session must not block the current pane.
    assert!(!identity_anchor_conflicts_with_candidate(&base, Some(&candidate("%900"))));

    // The same native thread is the same durable principal, regardless of pane.
    let mut same_thread = base.clone();
    same_thread.worker_id = "same-thread-peer".into();
    same_thread.transport.as_mut().unwrap().thread_id = Some("new-thread".into());
    assert!(identity_anchor_conflicts_with_candidate(&same_thread, Some(&candidate("%901"))));

    // The same tmux pane route is the same durable principal regardless of session.
    let mut same_pane = base.clone();
    same_pane.worker_id = "same-pane-peer".into();
    same_pane.transport.as_mut().unwrap().kind = TransportKind::Tmux;
    same_pane.transport.as_mut().unwrap().tmux_endpoint = Some(candidate("%902").endpoint.clone());
    assert!(identity_anchor_conflicts_with_candidate(&same_pane, Some(&candidate("%902"))));
}

#[test]
fn appserver_pane_recovery_anchor_does_not_hide_a_distinct_live_thread() {
    let pane = tmux_candidate(Some("old-session"), Some("old-thread"), "%910");
    let base = Identity {
        worker_id: "appserver-with-pane-anchor".into(),
        token: "appserver-with-pane-anchor-token".into(),
        project_scope: None,
        runtime: Some(RuntimeIdentity::cli_adapter("appserver-with-pane-anchor").unwrap()),
        transport: Some(SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("old-session".into()),
            thread_id: Some("old-thread".into()),
            tmux_endpoint: Some(pane.endpoint.clone()),
            capabilities: vec![],
            self_check: "ok".into(),
        }),
    };

    // A different App Server thread in the same pane is a different peer: the
    // pane is only this peer's recovery anchor, not its identity.
    let other_thread = tmux_candidate(Some("new-session"), Some("new-thread"), "%910");
    assert!(!identity_anchor_conflicts_with_candidate(
        &base,
        Some(&other_thread)
    ));

    // The same thread still overlaps wherever it runs.
    let same_thread = tmux_candidate(Some("new-session"), Some("old-thread"), "%911");
    assert!(identity_anchor_conflicts_with_candidate(
        &base,
        Some(&same_thread)
    ));

    // A pane-only candidate carries no Codex IDs, so the recovery anchor decides.
    let pane_only = tmux_candidate(None, None, "%910");
    assert!(identity_anchor_conflicts_with_candidate(
        &base,
        Some(&pane_only)
    ));
}

/// A short temp project root: the fake route-authority socket lives under the
/// state root, so the combined path must stay under `sockaddr_un`'s limit.
fn short_test_root() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    // Keep the whole `<root>/global/server.sock` path short: unix socket paths
    // are limited to ~104 bytes and the macOS temp dir already uses ~48.
    std::env::temp_dir().join(format!(
        "cs{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Build one persisted identity whose dual key is the given address.
fn persist_peer_at(
    host_paths: &HostPaths,
    scope: &Scope,
    worker: &str,
    session: &str,
    thread: &str,
    generation: u64,
) -> Identity {
    persist_appserver_peer_at(
        host_paths,
        scope,
        worker,
        session,
        thread,
        "unix:///tmp/codex.sock",
        generation,
    )
}

/// Build one persisted App Server identity whose probe endpoint is `endpoint`.
/// A test that binds a live fake App Server there makes the peer genuinely
/// `Live`; the shared `/tmp/codex.sock` default keeps every other peer
/// unproven.
fn persist_appserver_peer_at(
    host_paths: &HostPaths,
    scope: &Scope,
    worker: &str,
    session: &str,
    thread: &str,
    endpoint: &str,
    generation: u64,
) -> Identity {
    let mut identity =
        load_or_create_resolved_at(host_paths, scope, Some(worker.into()), false).unwrap();
    let mut runtime = runtime_identity(generation, &format!("binding-{worker}"));
    runtime.session_id = Some(SessionId::new(session).unwrap());
    runtime.native_thread_id = Some(NativeThreadId::new(thread).unwrap());
    persist_registration_at(
        host_paths,
        scope,
        &mut identity,
        runtime,
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some(endpoint.into()),
            namespace: Some("codex_tui".into()),
            session_id: Some(session.into()),
            thread_id: Some(thread.into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message".into()],
            self_check: "server verified".into(),
        },
    )
    .unwrap();
    identity
}

/// Minimal fake App Server that answers the handshake, `initialize`, and
/// `thread/read` (as a live thread) so `persisted_peer_liveness` classifies a
/// persisted peer as `Live`. It serves connections until `stop` is set.
fn spawn_live_app_server(
    socket: PathBuf,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    spawn_app_server_with_thread_status(socket, stop, "active")
}

/// Same fake App Server, but `thread/read` reports the given thread status so a
/// test can make a persisted peer provably `Dead` (`systemError`).
fn spawn_app_server_with_thread_status(
    socket: PathBuf,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread_status: &'static str,
) -> std::thread::JoinHandle<()> {
    use std::os::unix::net::UnixListener;
    let listener = UnixListener::bind(&socket).expect("bind fake App Server socket");
    listener.set_nonblocking(true).unwrap();
    std::thread::spawn(move || {
        while !stop.load(std::sync::atomic::Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // BSD/macOS accepted sockets inherit the listener's
                    // non-blocking flag, so restore blocking reads.
                    stream.set_nonblocking(false).unwrap();
                    serve_app_server(&mut stream, thread_status);
                }
                Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                Err(_) => break,
            }
        }
    })
}

fn serve_app_server(stream: &mut std::os::unix::net::UnixStream, thread_status: &str) {
    use std::io::{Read, Write};
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        if stream.read_exact(&mut byte).is_err() {
            return;
        }
        request.push(byte[0]);
    }
    if stream
        .write_all(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
        )
        .is_err()
    {
        return;
    }
    loop {
        let Some(frame) = read_client_frame(stream) else {
            return;
        };
        let value: serde_json::Value =
            serde_json::from_slice(&frame).unwrap_or(serde_json::Value::Null);
        if value["method"] == "thread/read" {
            let response = serde_json::json!({
                "id": value["id"],
                "result": {"thread": {"status": {"type": thread_status}}}
            });
            let _ = stream.write_all(&encode_frame(0x1, &serde_json::to_vec(&response).unwrap()));
            return;
        }
        if value.get("id").is_some() {
            let response = serde_json::json!({"id": value["id"], "result": {}});
            let _ = stream.write_all(&encode_frame(0x1, &serde_json::to_vec(&response).unwrap()));
        }
    }
}

fn read_client_frame(stream: &mut std::os::unix::net::UnixStream) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header).ok()?;
    let masked = header[1] & 0x80 != 0;
    let mut length = (header[1] & 0x7f) as usize;
    if length == 126 {
        let mut bytes = [0_u8; 2];
        stream.read_exact(&mut bytes).ok()?;
        length = u16::from_be_bytes(bytes) as usize;
    } else if length == 127 {
        let mut bytes = [0_u8; 8];
        stream.read_exact(&mut bytes).ok()?;
        length = u64::from_be_bytes(bytes) as usize;
    }
    let mut mask = [0_u8; 4];
    if masked {
        stream.read_exact(&mut mask).ok()?;
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).ok()?;
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    Some(payload)
}

fn encode_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x80 | opcode);
    let mask = [0x11_u8, 0x22, 0x33, 0x44];
    match payload.len() {
        length if length < 126 => frame.push(0x80 | length as u8),
        length if length <= u16::MAX as usize => {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        }
        length => {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % 4]),
    );
    frame
}
fn with_current_address<T>(thread: &str, session: &str, body: impl FnOnce() -> T) -> T {
    let previous_thread = std::env::var_os("CODEX_THREAD_ID");
    let previous_session = std::env::var_os("CODEX_SESSION_ID");
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    let previous_originator = std::env::var_os("CODEX_INTERNAL_ORIGINATOR_OVERRIDE");
    let previous_namespace = std::env::var_os("COLLAB_APPSERVER_NAMESPACE");
    std::env::set_var("CODEX_THREAD_ID", thread);
    std::env::set_var("CODEX_SESSION_ID", session);
    std::env::set_var("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop");
    std::env::remove_var("COLLAB_APPSERVER_NAMESPACE");
    std::env::remove_var("COLLAB_WORKER");
    let result = body();
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
    match previous_originator {
        Some(value) => std::env::set_var("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", value),
        None => std::env::remove_var("CODEX_INTERNAL_ORIGINATOR_OVERRIDE"),
    }
    match previous_namespace {
        Some(value) => std::env::set_var("COLLAB_APPSERVER_NAMESPACE", value),
        None => std::env::remove_var("COLLAB_APPSERVER_NAMESPACE"),
    }
    result
}

#[test]
fn appserver_exact_same_project_adopts_the_persisted_worker() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = short_test_root();
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join("global");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    persist_peer_at(
        &host_paths,
        &scope,
        "persisted-agent",
        "session-shared",
        "thread-shared",
        4,
    );

    let previous_pane = std::env::var_os("TMUX_PANE");
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    let previous_socket = std::env::var_os("COLLAB_APPSERVER_SOCKET");
    std::env::remove_var("TMUX_PANE");
    std::env::remove_var("COLLAB_WORKER");
    std::env::set_var(
        "COLLAB_APPSERVER_SOCKET",
        "/tmp/cross-project-appserver.sock",
    );
    let resolved = with_current_address("thread-shared", "session-shared", || {
        load_or_create_full(&host_paths, &scope, None, true, true)
    });
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

    let identity = resolved.unwrap();
    assert_eq!(identity.worker_id, "persisted-agent");
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn appserver_identity_rejects_a_cross_project_same_session_and_thread() {
    let _guard = ENV_LOCK.lock().unwrap();
    let first_root = short_test_root();
    std::fs::create_dir_all(first_root.join(".agent-collab")).unwrap();
    let state_root = first_root.join("global");
    std::fs::create_dir_all(&state_root).unwrap();
    let first_scope = test_scope(first_root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    persist_peer_at(
        &host_paths,
        &first_scope,
        "first-project-agent",
        "session-shared",
        "thread-shared",
        4,
    );

    let second_root = short_test_root();
    std::fs::create_dir_all(second_root.join(".agent-collab")).unwrap();
    let second_scope = test_scope(second_root.clone());
    let previous_pane = std::env::var_os("TMUX_PANE");
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    let previous_socket = std::env::var_os("COLLAB_APPSERVER_SOCKET");
    std::env::remove_var("TMUX_PANE");
    std::env::remove_var("COLLAB_WORKER");
    std::env::set_var(
        "COLLAB_APPSERVER_SOCKET",
        "/tmp/cross-project-appserver.sock",
    );
    let created = with_current_address("thread-shared", "session-shared", || {
        load_or_create_full(&host_paths, &second_scope, None, true, true)
    });
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

    // The foreign AppServer peer cannot be proven dead (its probe endpoint is
    // unreachable), so rebind must fail closed instead of archiving it on scope
    // mismatch alone. It stays in place and remains recoverable via --worker.
    let error = created.unwrap_err().to_string();
    assert!(
        error.starts_with("IDENTITY_RESTORE_CROSS_PROJECT:"),
        "{error}"
    );
    assert!(
        read_identity(&identity_path_at(&host_paths, "first-project-agent").unwrap())
            .unwrap()
            .is_some(),
        "non-dead cross-project peer must remain in the live identity set"
    );
    let archive_root = host_paths.state_root().join("archives");
    let archived = std::fs::read_dir(&archive_root)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .any(|entry| entry.path().join("first-project-agent").is_dir())
        })
        .unwrap_or(false);
    assert!(
        !archived,
        "non-dead cross-project peer must not be archived"
    );
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(first_root).ok();
    std::fs::remove_dir_all(second_root).ok();
}
