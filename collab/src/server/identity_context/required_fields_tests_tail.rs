#[test]
fn approved_identity_digest_excludes_descriptive_metadata_but_binds_facts_and_fence() {
    let facts = IdentityFacts {
        thread_id: Some("thread-a".into()),
        ..Default::default()
    };
    let mut approval = json!({
        "decision":"approved", "decided_by":"user", "target_identity":"peer-a",
        "project_scope":"/project", "app_scope_id":"appserver-cli",
        "action":"restore_identity",
        "expected_incumbent":{"binding_id":"bind-7", "endpoint_generation":7},
        "intent_digest":"sha256:placeholder", "approved_at_ms":1770000000000_i64
    });
    let expected = approval_digest(&approval, &facts);
    approval["intent_digest"] = expected.clone().into();
    assert_eq!(
        parse_identity_approval(&approval).unwrap().intent_digest,
        expected
    );
    approval["decided_by"] = "operator".into();
    approval["approved_at_ms"] = 1770000000001_i64.into();
    assert_eq!(approval_digest(&approval, &facts), expected);
    approval["expected_incumbent"]["endpoint_generation"] = 8.into();
    assert_ne!(approval_digest(&approval, &facts), expected);
    assert_ne!(
        approval_digest(&approval, &IdentityFacts::default()),
        expected
    );
}

#[test]
fn approved_recovery_without_durable_incumbent_is_denied_before_admission() {
    let (server, root) = test_server();
    let host_paths = server.host_paths.clone();
    let journal = Arc::new(OperationJournal::open(host_paths.journal_path()).unwrap());
    let manager = ProjectRuntimeManager::new_with_operation_journal(
        Arc::new(server),
        &host_paths,
        journal.clone(),
    )
    .unwrap();
    let context = ProjectContext::for_registered_root_with_app(
        &root,
        AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
    )
    .unwrap();
    let facts = IdentityFacts {
        tmux: Some(tmux_anchor()),
        ..Default::default()
    };
    let mut approval = json!({
        "decision":"approved", "decided_by":"user", "target_identity":"peer-missing",
        "project_scope":context.project_scope.as_str(), "app_scope_id":context.app_scope_id.as_str(),
        "action":"restore_identity",
        "expected_incumbent":{"binding_id":"bind-7", "endpoint_generation":7},
        "intent_digest":"sha256:placeholder", "approved_at_ms":1770000000000_i64
    });
    approval["intent_digest"] = approval_digest(&approval, &facts).into();
    let request = IdentityContextRequest {
        operation_id: "ctxop-approved-no-incumbent".into(),
        invocation: "approved_recovery".into(),
        // The request action is the context operation; the approval's own
        // `action` field carries `restore_identity` / `replace_binding`.
        action: "context".into(),
        facts,
        approval: Some(approval),
        grant_approval: None,
        query: false,
        query_capability: "base64url:capability".into(),
        invocation_ticket: String::new(),
    };
    let (_, response) = manager.identity_context(context.clone(), request.clone());
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some("IDENTITY_APPROVAL_IDENTITY_NOT_FOUND")
    );
    assert!(journal
        .query(
            &request,
            context.project_scope.as_str(),
            context.app_scope_id.as_str()
        )
        .is_err());
    std::fs::remove_dir_all(root).unwrap();
}

/// One verified AppServer candidate for the fake socket the test serves.
fn appserver_candidate(
    root: &Path,
    endpoint: &str,
    session: &str,
    thread: &str,
) -> TransportCandidates {
    TransportCandidates {
        appserver: Some(AppServerCandidate {
            endpoint: format!("unix://{endpoint}"),
            namespace: "codex_tui".into(),
            session_id: session.into(),
            thread_id: thread.into(),
            cwd: root.to_string_lossy().into_owned(),
        }),
        tmux: None,
        dsh: None,
    }
}

fn recovery_facts(endpoint: &str, session: &str, thread: &str) -> IdentityFacts {
    IdentityFacts {
        session_id: Some(session.into()),
        thread_id: Some(thread.into()),
        endpoint: Some(format!("unix://{endpoint}")),
        namespace: Some("codex_tui".into()),
        ..IdentityFacts::default()
    }
}

/// A fake AppServer that serves the real admission handshake on every
/// connection the daemon opens, then stops when the test releases it.
///
/// The number of connections a valid path opens is a product fact: an initial
/// registration opens the verify, dispatch and observe connections, while an
/// already-committed restore opens fewer. A fixed count would block on an
/// accept the daemon will never make, so the loop is bounded by this explicit
/// owned shutdown instead: `join`/`drop` stop the serve loop even when a valid
/// path opened fewer connections.
struct FakeAppServer {
    stop: Arc<std::sync::atomic::AtomicBool>,
    socket: PathBuf,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl FakeAppServer {
    fn release(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        // Wake a pending accept so the serve loop observes the stop flag.
        let _ = std::os::unix::net::UnixStream::connect(&self.socket);
    }

    fn join(mut self) -> std::thread::Result<()> {
        self.release();
        self.handle.take().expect("fake appserver handle").join()
    }
}

impl Drop for FakeAppServer {
    fn drop(&mut self) {
        self.release();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Serve the exact AppServer admission handshake (`initialize` then
/// `thread/read`) so the daemon's real `verify_candidate` accepts the
/// candidate without any production fault seam.
fn serve_fake_appserver(
    listener: std::os::unix::net::UnixListener,
    session: &str,
    thread: &str,
    cwd: &Path,
) -> FakeAppServer {
    use std::io::{Read, Write};
    let session = session.to_owned();
    let thread = thread.to_owned();
    let cwd = cwd.to_string_lossy().into_owned();
    let socket = listener
        .local_addr()
        .ok()
        .and_then(|addr| addr.as_pathname().map(Path::to_path_buf))
        .expect("fake appserver listener has a socket path");
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        let mut registration: Option<(String, String, String, i64)> = None;
        while !flag.load(std::sync::atomic::Ordering::SeqCst) {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            if flag.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let mut header = Vec::new();
            let mut byte = [0_u8; 1];
            while !header.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
            }
            stream
                    .write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
                    .unwrap();
            loop {
                let Some(request) = read_test_frame(&mut stream) else {
                    break;
                };
                let request: serde_json::Value = serde_json::from_slice(&request).unwrap();
                let Some(id) = request.get("id").cloned() else {
                    continue;
                };
                let method = request["method"].as_str().unwrap();
                let response = match method {
                    "initialize" => json!({"id": id, "result": {}}),
                    "thread/resume" => json!({
                        "id": id,
                        "result": {"thread": {
                            "id": thread,
                            "sessionId": session,
                            "cwd": cwd,
                            "status": {"type": "idle"}
                        }}
                    }),
                    "thread/read" => {
                        let turns = if request["params"]["includeTurns"].as_bool() == Some(true) {
                            registration
                                .as_ref()
                                .map(|(turn_id, client_message_id, marker, completed_at)| {
                                    json!([{
                                        "id": turn_id,
                                        "status": "completed",
                                        "completedAt": completed_at,
                                        "items": [
                                            {
                                                "type": "userMessage",
                                                "id": client_message_id,
                                                "clientMessageId": client_message_id
                                            },
                                            {"type": "agentMessage", "text": marker}
                                        ]
                                    }])
                                })
                                .unwrap_or_else(|| json!([]))
                        } else {
                            json!([])
                        };
                        json!({
                            "id": id,
                            "result": {"thread": {
                                "id": thread,
                                "sessionId": session,
                                "cwd": cwd,
                                "status": {"type": "idle"},
                                "turns": turns
                            }}
                        })
                    }
                    "thread/items/list" => {
                        json!({"id": id, "error": {"code": -32601, "message": "unsupported"}})
                    }
                    "turn/start" => {
                        let thread_id = request["params"]["threadId"].as_str().unwrap_or_default();
                        if thread_id.is_empty() {
                            json!({
                                "id": id,
                                "result": {"turn": {"id": "probe-turn", "status": "inProgress"}}
                            })
                        } else {
                            let output = request["params"]["toolOutput"]["output"]
                                .as_str()
                                .expect("registration dispatch carries the delegated prompt");
                            let start = output
                                .find("COLLAB_REGISTER_CHALLENGE ")
                                .expect("delegated prompt carries the registration challenge");
                            let raw = &output[start + "COLLAB_REGISTER_CHALLENGE ".len()..];
                            let mut values = serde_json::Deserializer::from_str(raw)
                                .into_iter::<serde_json::Value>();
                            let challenge = values.next().unwrap().unwrap();
                            let marker = challenge["challenge"]
                                .as_str()
                                .expect("registration challenge carries its marker")
                                .to_owned();
                            let client_message_id = request["params"]["clientUserMessageId"]
                                .as_str()
                                .expect("registration dispatch carries its command id")
                                .to_owned();
                            let turn_id = format!("registration-turn-{thread_id}");
                            let completed_at = chrono::Utc::now().timestamp();
                            registration =
                                Some((turn_id.clone(), client_message_id, marker, completed_at));
                            json!({
                                "id": id,
                                "result": {"turn": {"id": turn_id, "status": "inProgress"}}
                            })
                        }
                    }
                    "turn/steer" => {
                        let thread_id = request["params"]["threadId"].as_str().unwrap_or_default();
                        let turn_id = if thread_id.is_empty() {
                            "probe-turn"
                        } else {
                            request["params"]["expectedTurnId"]
                                .as_str()
                                .unwrap_or_default()
                        };
                        json!({"id": id, "result": {"turnId": turn_id}})
                    }
                    "thread/turns/list" => json!({"id": id, "result": {"data": []}}),
                    other => panic!("unexpected AppServer method {other}"),
                };
                stream
                    .write_all(&encode_test_frame(
                        0x1,
                        &serde_json::to_vec(&response).unwrap(),
                    ))
                    .unwrap();
                if method == "thread/turns/list"
                    || (method == "thread/read"
                        && request["params"]["includeTurns"].as_bool() == Some(true))
                    || (method == "turn/start"
                        && request["params"]["threadId"]
                            .as_str()
                            .is_some_and(|thread_id| !thread_id.is_empty()))
                {
                    break;
                }
            }
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    });
    FakeAppServer {
        stop,
        socket,
        handle: Some(handle),
    }
}

/// Read one client frame, or `None` when the client closed the stream. A clean
/// close ends the connection instead of panicking the serve thread.
fn read_test_frame(stream: &mut std::os::unix::net::UnixStream) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header).ok()?;
    let masked = header[1] & 0x80 != 0;
    let mut length = (header[1] & 0x7f) as usize;
    if length == 126 {
        let mut bytes = [0_u8; 2];
        stream.read_exact(&mut bytes).ok()?;
        length = u16::from_be_bytes(bytes) as usize;
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

fn encode_test_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x80 | opcode];
    let length = payload.len();
    if length < 126 {
        frame.push(0x80 | length as u8);
    } else if length <= u16::MAX as usize {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(length as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(length as u64).to_be_bytes());
    }
    let mask = [0x11_u8, 0x22, 0x33, 0x44];
    frame.extend_from_slice(&mask);
    for (index, byte) in payload.iter().enumerate() {
        frame.push(byte ^ mask[index % 4]);
    }
    frame
}

fn test_socket_path(worker_id: &str, generation: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "collab-d2i-{}-{worker_id}-{generation}.sock",
        std::process::id()
    ))
}

/// True when this sandbox permits binding a unix socket. The managed
/// sandbox denies it, so real-transport owner tests skip rather than
/// report a false failure; the same pattern is used by the adapter tests.
fn unix_sockets_available() -> bool {
    let probe = std::env::temp_dir().join(format!(
        "collab-d2i-probe-{}-{}.sock",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    match std::os::unix::net::UnixListener::bind(&probe) {
        Ok(listener) => {
            drop(listener);
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("probe unix socket bind: {error}"),
    }
}

/// Owner-boundary proof consumption and the exact incumbent fence recheck,
/// exercised without a live transport. The Register owner must accept the
/// proof only for its exact target and route scope, and the fence must name
/// the committed incumbent binding and generation.
#[test]
fn register_approval_proof_is_scoped_and_rechecks_the_exact_incumbent_fence() {
    let project_scope = ProjectScopeId::new("/tmp/collab-d2i-fence".to_owned()).unwrap();
    let app_scope = AppServerId::new("appserver-cli").unwrap();
    let other_app_scope = AppServerId::new("tui-default").unwrap();
    let binding_id = BindingId::new("binding-peer-a").unwrap();
    let mut state = State::default();
    state
        .global
        .register_project(
            ProjectRegistration::with_registered_at(project_scope.clone(), app_scope.clone(), 1)
                .unwrap(),
        )
        .unwrap();
    state
        .global
        .bind_runtime(
            RuntimeBinding::new_with_session(
                project_scope.clone(),
                app_scope.clone(),
                AgentId::new("peer-a").unwrap(),
                RuntimeId::new("runtime-peer-a").unwrap(),
                binding_id.clone(),
                7,
                Some(crate::identity::SessionId::new("session-peer-a").unwrap()),
                Some(NativeThreadId::new("thread-peer-a").unwrap()),
            )
            .unwrap(),
        )
        .unwrap();

    let proof = RegisterApprovalProof {
        target_identity: "peer-a".into(),
        project_scope: project_scope.as_str().to_owned(),
        app_scope_id: app_scope.as_str().to_owned(),
        incumbent_binding_id: binding_id.as_str().to_owned(),
        incumbent_endpoint_generation: 7,
    };
    assert!(approved_register_authorized(
        Some(&proof),
        "peer-a",
        Some(&project_scope),
        Some(&app_scope),
    ));
    assert!(!approved_register_authorized(
        Some(&proof),
        "peer-b",
        Some(&project_scope),
        Some(&app_scope),
    ));
    assert!(!approved_register_authorized(
        Some(&proof),
        "peer-a",
        Some(&project_scope),
        Some(&other_app_scope),
    ));
    assert!(!approved_register_authorized(
        None,
        "peer-a",
        Some(&project_scope),
        Some(&app_scope),
    ));
    assert!(approved_register_fence_current(
        &state,
        &proof,
        &project_scope,
        &app_scope,
    ));
    let stale = RegisterApprovalProof {
        incumbent_endpoint_generation: 8,
        ..proof.clone()
    };
    assert!(!approved_register_fence_current(
        &state,
        &stale,
        &project_scope,
        &app_scope,
    ));
    let unknown_binding = RegisterApprovalProof {
        incumbent_binding_id: "binding-missing".into(),
        ..proof
    };
    assert!(!approved_register_fence_current(
        &state,
        &unknown_binding,
        &project_scope,
        &app_scope,
    ));
}

/// Spawn the fake AppServer that answers the recovery Register's own
/// `verify_candidate`, returning the request candidates bound to it.
fn recovery_appserver(endpoint: &Path, session: &str, thread: &str, cwd: &Path) -> FakeAppServer {
    let listener = std::os::unix::net::UnixListener::bind(endpoint).unwrap();
    serve_fake_appserver(listener, session, thread, cwd)
}

/// Register and promote the daemon's current credential, then persist a
/// stale local credential and return a separate recovery endpoint.
fn stale_master_incumbent(
    server: &Server,
    root: &Path,
    worker_id: &str,
) -> (IdentityFacts, BindingId, u64, String, PathBuf, String) {
    let first_socket = test_socket_path(worker_id, "1");
    let first_listener = std::os::unix::net::UnixListener::bind(&first_socket).unwrap();
    let first_thread = format!("thread-{worker_id}-1");
    let first_server = serve_fake_appserver(
        first_listener,
        &format!("session-{worker_id}-1"),
        &first_thread,
        root,
    );
    let first_endpoint = first_socket.to_string_lossy().into_owned();
    let first = handle_register_with_app_scope(
        server,
        worker_id.into(),
        "token-1".into(),
        root.to_string_lossy().into_owned(),
        Some(AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap()),
        Some(appserver_candidate(
            root,
            &first_endpoint,
            &format!("session-{worker_id}-1"),
            &first_thread,
        )),
    );
    assert!(first.ok, "{:?}", first.error);
    first_server.join().unwrap();
    let promote = handle_master_promote(
        server,
        worker_id.into(),
        "token-1".into(),
        "user-approved master promotion".into(),
    );
    assert!(promote.ok, "{:?}", promote.error);
    let scope = ProjectContext::for_registered_root_with_app(
        root,
        AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
    )
    .unwrap();
    let binding = {
        let state = server.state.lock().unwrap();
        state
            .global
            .lookup_binding_for(
                &RouteScope {
                    app_scope_id: scope.app_scope_id.clone(),
                    project_scope_id: scope.project_scope.clone(),
                },
                &BindingId::new(format!("binding-{worker_id}")).unwrap(),
            )
            .cloned()
            .expect("registered binding")
    };
    let transport = {
        let state = server.state.lock().unwrap();
        selected_transport_for_worker(state.workers.get(worker_id).expect("worker"))
            .expect("selected transport")
    };
    let recovery_socket = test_socket_path(worker_id, "recovery");
    let recovery_endpoint = recovery_socket.to_string_lossy().into_owned();
    let recovery_session = format!("session-{worker_id}-recovery");
    let recovery_thread = format!("thread-{worker_id}-recovery");
    let mut identity = identity::Identity {
        worker_id: worker_id.into(),
        token: "token-2".into(),
        project_scope: Some(scope.project_scope.clone()),
        runtime: None,
        transport: None,
    };
    identity::persist_registration_at(
        &server.host_paths,
        &Scope {
            root: root.to_path_buf(),
        },
        &mut identity,
        RuntimeIdentity {
            agent_id: binding.agent_id.clone(),
            runtime_id: binding.runtime_id.clone(),
            appserver_id: binding.app_scope_id.clone(),
            endpoint_generation: binding.endpoint_generation,
            binding_id: binding.binding_id.clone(),
            session_id: binding.session_id.clone(),
            native_thread_id: binding.native_thread_id.clone(),
        },
        transport.clone(),
    )
    .unwrap();
    (
        recovery_facts(&recovery_endpoint, &recovery_session, &recovery_thread),
        binding.binding_id.clone(),
        binding.endpoint_generation,
        binding.agent_id.as_str().to_owned(),
        recovery_socket,
        recovery_thread,
    )
}

fn recovery_manager(server: Arc<Server>) -> Arc<ProjectRuntimeManager> {
    let host_paths = server.host_paths.clone();
    let journal = Arc::new(OperationJournal::open(host_paths.journal_path()).unwrap());
    ProjectRuntimeManager::new_with_operation_journal(server, &host_paths, journal).unwrap()
}

fn recovery_context(root: &Path) -> ProjectContext {
    ProjectContext::for_registered_root_with_app(
        root,
        AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
    )
    .unwrap()
}

fn identity_approval(
    context: &ProjectContext,
    facts: &IdentityFacts,
    target: &str,
    incumbent_binding_id: &str,
    incumbent_generation: u64,
) -> serde_json::Value {
    let mut approval = json!({
        "decision":"approved",
        "decided_by":"user",
        "target_identity": target,
        "project_scope": context.project_scope.as_str(),
        "app_scope_id": context.app_scope_id.as_str(),
        "action":"replace_binding",
        "expected_incumbent":{
            "binding_id": incumbent_binding_id,
            "endpoint_generation": incumbent_generation
        },
        "intent_digest":"sha256:placeholder",
        "approved_at_ms":1770000000000_i64
    });
    approval["intent_digest"] = approval_digest(&approval, facts).into();
    approval
}

fn grant_approval(
    context: &ProjectContext,
    facts: &IdentityFacts,
    target: &str,
    grant_id: &str,
    grant_generation: u64,
) -> serde_json::Value {
    let mut approval = json!({
        "decision":"approved",
        "decided_by":"user",
        "target_identity": target,
        "project_scope": context.project_scope.as_str(),
        "app_scope_id": context.app_scope_id.as_str(),
        "action":"replace_master_grant",
        "expected_grant":{"grant_id": grant_id, "generation": grant_generation},
        "intent_digest":"sha256:placeholder",
        "approved_at_ms":1770000000000_i64
    });
    approval["intent_digest"] = approval_digest(&approval, facts).into();
    approval
}

fn grant_for(
    server: &Server,
    root: &Path,
    binding_id: &BindingId,
) -> Option<crate::server::global_state::MasterGrant> {
    let context = recovery_context(root);
    let state = server.state.lock().unwrap();
    current_master_grant(
        &state,
        Some(&RouteScope {
            app_scope_id: context.app_scope_id,
            project_scope_id: context.project_scope,
        }),
    )
    .filter(|grant| grant.binding_id == *binding_id)
}

fn binding_generation_for(
    server: &Server,
    context: &ProjectContext,
    binding_id: &BindingId,
) -> u64 {
    server
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(
            &RouteScope {
                app_scope_id: context.app_scope_id.clone(),
                project_scope_id: context.project_scope.clone(),
            },
            binding_id,
        )
        .expect("recovered binding")
        .endpoint_generation
}

/// Approved peer recovery consumes the daemon Register proof at the owner
/// boundary, commits the exact binding, and keeps the existing master grant
/// for an identity-only approval (no implicit promotion, clear, or replace).
#[test]
fn approved_register_recovery_commits_binding_and_retains_master_grant() {
    if !unix_sockets_available() {
        eprintln!("SKIP approved-register owner test: sandbox denied unix socket bind");
        return;
    }
    let (server, root) = test_server();
    let server = Arc::new(server);
    let manager = recovery_manager(server.clone());
    let (facts, binding_id, incumbent_generation, _agent, endpoint, thread) =
        stale_master_incumbent(&server, &root, "peer-a");
    let context = recovery_context(&root);
    let before_grant = grant_for(&server, &root, &binding_id).expect("master grant");
    let incumbent_token = server.state.lock().unwrap().workers["peer-a"].token.clone();
    assert_eq!(incumbent_token, "token-1");
    assert_eq!(
        identity::read_persisted(&server.host_paths, "peer-a")
            .unwrap()
            .unwrap()
            .token,
        "token-2",
        "the local credential is deliberately stale"
    );
    let recovery_server = recovery_appserver(
        &endpoint,
        facts.session_id.as_deref().expect("recovery session"),
        &thread,
        &root,
    );
    let request = IdentityContextRequest {
        operation_id: "ctxop-approved-peer".into(),
        invocation: "approved_recovery".into(),
        action: "context".into(),
        facts: facts.clone(),
        approval: Some(identity_approval(
            &context,
            &facts,
            "peer-a",
            binding_id.as_str(),
            incumbent_generation,
        )),
        grant_approval: None,
        query: false,
        query_capability: "base64url:capability".into(),
        invocation_ticket: String::new(),
    };
    let (runtime, response) = manager.identity_context(context.clone(), request);
    if response.ok {
        recovery_server.join().unwrap();
    } else {
        drop(recovery_server);
    }
    let host_token = server
        .state
        .lock()
        .unwrap()
        .workers
        .get("peer-a")
        .map(|worker| worker.token.clone());
    let runtime_token = runtime
        .state
        .lock()
        .unwrap()
        .workers
        .get("peer-a")
        .map(|worker| worker.token.clone());
    assert!(
        response.ok,
        "error={:?} data={} host_token={host_token:?} runtime_token={runtime_token:?}",
        response.error, response.data
    );
    assert_eq!(response.data["result"]["outcome"], "completed");
    assert_eq!(response.data["result"]["phase"], "context_complete");
    let new_generation = binding_generation_for(&server, &context, &binding_id);
    assert!(
        new_generation > incumbent_generation,
        "recovery must advance the endpoint generation"
    );
    {
        let state = server.state.lock().unwrap();
        let binding = state
            .global
            .lookup_binding_for(
                &RouteScope {
                    app_scope_id: context.app_scope_id.clone(),
                    project_scope_id: context.project_scope.clone(),
                },
                &binding_id,
            )
            .expect("recovered binding");
        assert_eq!(binding.endpoint_generation, new_generation);
        assert_eq!(
            state
                .workers
                .get("peer-a")
                .map(|worker| worker.token.as_str()),
            Some(incumbent_token.as_str()),
            "approved identity restoration keeps the current owner's credential"
        );
    }
    assert_eq!(host_token.as_deref(), Some(incumbent_token.as_str()));
    assert_eq!(runtime_token.as_deref(), Some(incumbent_token.as_str()));
    assert_eq!(
        identity::read_persisted(&server.host_paths, "peer-a")
            .unwrap()
            .unwrap()
            .token,
        incumbent_token,
        "the stale local credential must converge to the authoritative owner"
    );
    let after_grant = grant_for(&server, &root, &binding_id).expect("retained grant");
    assert_eq!(after_grant.agent_id, before_grant.agent_id);
    assert_eq!(after_grant.binding_id, before_grant.binding_id);
    assert_eq!(after_grant.endpoint_generation, new_generation);
    assert_eq!(after_grant.granted_by, before_grant.granted_by);
    std::fs::remove_dir_all(root).unwrap();
}

/// Explicit recovery reads the current incumbent internally. A stale caller
/// fence does not require another approval; the existing grant survives.
#[test]
fn approved_recovery_reads_current_incumbent_despite_stale_caller_fence() {
    if !unix_sockets_available() {
        eprintln!("SKIP stale-incumbent owner test: sandbox denied unix socket bind");
        return;
    }
    let (server, root) = test_server();
    let server = Arc::new(server);
    let manager = recovery_manager(server.clone());
    let (facts, binding_id, incumbent_generation, _agent, endpoint, thread) =
        stale_master_incumbent(&server, &root, "peer-stale");
    let context = recovery_context(&root);
    let before_grant = grant_for(&server, &root, &binding_id).expect("master grant");
    let recovery_server = recovery_appserver(
        &endpoint,
        facts.session_id.as_deref().unwrap(),
        &thread,
        &root,
    );
    let request = IdentityContextRequest {
        operation_id: "ctxop-approved-stale".into(),
        invocation: "approved_recovery".into(),
        action: "context".into(),
        facts: facts.clone(),
        approval: Some(identity_approval(
            &context,
            &facts,
            "peer-stale",
            binding_id.as_str(),
            incumbent_generation + 1,
        )),
        grant_approval: None,
        query: false,
        query_capability: "base64url:capability".into(),
        invocation_ticket: String::new(),
    };
    let (_, response) = manager.identity_context(context.clone(), request.clone());
    assert!(response.ok, "{response:?}");
    recovery_server.join().unwrap();
    assert_eq!(response.data["result"]["outcome"], "completed");
    {
        let state = server.state.lock().unwrap();
        let binding = state
            .global
            .lookup_binding_for(
                &RouteScope {
                    app_scope_id: context.app_scope_id.clone(),
                    project_scope_id: context.project_scope.clone(),
                },
                &binding_id,
            )
            .expect("binding");
        assert!(binding.endpoint_generation > incumbent_generation);
    }
    let after_grant = grant_for(&server, &root, &binding_id).expect("grant");
    assert_eq!(after_grant.resource_id(), before_grant.resource_id());
    assert_eq!(
        after_grant.resource_generation(),
        before_grant.resource_generation()
    );
    assert!(manager
        .operation_journal
        .query(
            &request,
            context.project_scope.as_str(),
            context.app_scope_id.as_str()
        )
        .is_ok());
    std::fs::remove_dir_all(root).unwrap();
}

/// One invocation carrying both approvals validates them before admission,
/// then commits the identity binding and the distinct grant replacement
/// under their separate owners and reads both back before `completed`.
#[test]
fn dual_approval_commits_identity_and_separate_grant_replacement() {
    if !unix_sockets_available() {
        eprintln!("SKIP dual-approval owner test: sandbox denied unix socket bind");
        return;
    }
    let (server, root) = test_server();
    let server = Arc::new(server);
    let manager = recovery_manager(server.clone());
    let (facts, binding_id, incumbent_generation, _agent, endpoint, thread) =
        stale_master_incumbent(&server, &root, "master-a");
    let context = recovery_context(&root);
    let before_grant = grant_for(&server, &root, &binding_id).expect("master grant");
    let recovery_server = recovery_appserver(
        &endpoint,
        facts.session_id.as_deref().expect("recovery session"),
        &thread,
        &root,
    );
    let request = IdentityContextRequest {
        operation_id: "ctxop-approved-dual".into(),
        invocation: "approved_recovery".into(),
        action: "context".into(),
        facts: facts.clone(),
        approval: Some(identity_approval(
            &context,
            &facts,
            "master-a",
            binding_id.as_str(),
            incumbent_generation,
        )),
        grant_approval: Some(grant_approval(
            &context,
            &facts,
            "master-a",
            binding_id.as_str(),
            before_grant.endpoint_generation,
        )),
        query: false,
        query_capability: "base64url:capability".into(),
        invocation_ticket: String::new(),
    };
    let (_, response) = manager.identity_context(context.clone(), request);
    if response.ok {
        recovery_server.join().unwrap();
    } else {
        drop(recovery_server);
    }
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.data["result"]["outcome"], "completed");
    let new_generation = binding_generation_for(&server, &context, &binding_id);
    let after_grant = grant_for(&server, &root, &binding_id).expect("replacement grant");
    assert_eq!(after_grant.agent_id, before_grant.agent_id);
    assert_eq!(after_grant.binding_id, before_grant.binding_id);
    assert_eq!(after_grant.endpoint_generation, new_generation);
    assert_eq!(after_grant.granted_by, "user");
    assert_ne!(after_grant.approval, before_grant.approval);
    std::fs::remove_dir_all(root).unwrap();
}
