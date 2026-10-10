use super::*;
use crate::proto::TmuxCandidate;
use crate::server::operation_journal::{inject_next_append_fault, OperationJournal};
use crate::server::peer_tests::test_server;
use std::sync::Arc;

fn tmux_anchor() -> TmuxCandidate {
    TmuxCandidate {
        endpoint: crate::proto::TmuxEndpoint {
            socket_path: "/tmp/tmux-test.sock".into(),
            server_pid: 42,
            tmux_session_id: "$7".into(),
            pane_id: "%3".into(),
            pane_pid: 99,
            codex_session_id: None,
            codex_thread_id: None,
        },
        cwd: "/tmp/project".into(),
    }
}

/// A dsh anchor with no observed App Server endpoint is complete on its own.
#[test]
fn dsh_anchor_alone_requires_no_appserver_fields() {
    let facts = IdentityFacts {
        dsh_session_id: Some("session-1".into()),
        ..IdentityFacts::default()
    };
    assert!(required_fields(&facts).is_empty());
}

/// A tmux anchor with no observed endpoint is complete on its own.
#[test]
fn tmux_anchor_alone_requires_no_appserver_fields() {
    let facts = IdentityFacts {
        tmux: Some(tmux_anchor()),
        ..IdentityFacts::default()
    };
    assert!(required_fields(&facts).is_empty());
}

/// An observed App Server endpoint selects the App Server candidate, which
/// reads all four fields unconditionally. The anchor must not short-circuit
/// that request: doing so reaches `expect("complete native facts")` and
/// panics the daemon handler instead of asking the caller.
#[test]
fn an_observed_endpoint_still_requires_the_four_appserver_fields() {
    let facts = IdentityFacts {
        dsh_session_id: Some("session-1".into()),
        endpoint: Some("unix:///tmp/appserver.sock".into()),
        ..IdentityFacts::default()
    };
    assert_eq!(
        required_fields(&facts),
        vec!["session_id", "thread_id", "namespace"]
    );
    let facts = IdentityFacts {
        tmux: Some(tmux_anchor()),
        endpoint: Some("unix:///tmp/appserver.sock".into()),
        ..IdentityFacts::default()
    };
    assert_eq!(
        required_fields(&facts),
        vec!["session_id", "thread_id", "namespace"]
    );
}

/// With no anchor and no endpoint all four facts are requested.
#[test]
fn no_anchor_requests_all_four_fields() {
    let facts = IdentityFacts::default();
    assert_eq!(
        required_fields(&facts),
        vec!["session_id", "thread_id", "endpoint", "namespace"]
    );
}

#[test]
fn typed_operation_request_preserves_action_intent_and_capability_shape() {
    let request = IdentityContextRequest {
        operation_id: "ctxop-typed".into(),
        invocation: "automatic".into(),
        action: "context".into(),
        facts: IdentityFacts::default(),
        approval: None,
        grant_approval: None,
        query: false,
        query_capability: "base64url:capability".into(),
        invocation_ticket: String::new(),
    };
    assert!(validate_operation_request(&request).is_ok());
    assert_eq!(
        serde_json::to_value(&request).unwrap()["operation_id"],
        "ctxop-typed"
    );
}

#[test]
fn query_and_missing_capability_are_rejected_before_operation_admission() {
    let query = IdentityContextRequest {
        operation_id: "ctxop-query".into(),
        invocation: "query".into(),
        action: "query".into(),
        facts: IdentityFacts::default(),
        approval: None,
        grant_approval: None,
        query: true,
        query_capability: "base64url:capability".into(),
        invocation_ticket: String::new(),
    };
    assert_eq!(
        validate_operation_request(&query).unwrap_err(),
        "IDENTITY_OPERATION_QUERY_SHAPE_INVALID"
    );

    let missing_capability = IdentityContextRequest {
        invocation: "automatic".into(),
        action: "context".into(),
        query: false,
        query_capability: String::new(),
        invocation_ticket: String::new(),
        ..query
    };
    assert_eq!(
        validate_operation_request(&missing_capability).unwrap_err(),
        "IDENTITY_OPERATION_CAPABILITY_REQUIRED: mutating context requires a query capability"
    );
}

#[test]
fn s23_dispatch_preserves_typed_identity_operation() {
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
    let request = IdentityContextRequest {
        operation_id: "ctxop-dispatch".into(),
        invocation: "automatic".into(),
        action: "context".into(),
        facts: IdentityFacts {
            tmux: Some(TmuxCandidate {
                endpoint: crate::proto::TmuxEndpoint {
                    socket_path: "/tmp/tmux-ctxop.sock".into(),
                    server_pid: 42,
                    tmux_session_id: "$7".into(),
                    pane_id: "%3".into(),
                    pane_pid: 99,
                    codex_session_id: None,
                    codex_thread_id: None,
                },
                cwd: root.display().to_string(),
            }),
            ..IdentityFacts::default()
        },
        approval: None,
        grant_approval: None,
        query: false,
        query_capability: "base64url:capability".into(),
        invocation_ticket: String::new(),
    };
    inject_next_append_fault();
    let (_, response) = manager.identity_context(context.clone(), request.clone());
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some("IDENTITY_OPERATION_DURABILITY_FAILED: injected operation journal append failure")
    );
    let replay = OperationJournal::open(host_paths.journal_path()).unwrap();
    let query = replay
        .query(
            &IdentityContextRequest {
                operation_id: request.operation_id.clone(),
                invocation: "query".into(),
                action: "query".into(),
                facts: IdentityFacts::default(),
                approval: None,
                grant_approval: None,
                query: true,
                query_capability: request.query_capability.clone(),
                invocation_ticket: String::new(),
            },
            context.project_scope.as_str(),
            context.app_scope_id.as_str(),
        )
        .unwrap_err();
    assert!(query.starts_with("IDENTITY_OPERATION_UNKNOWN"), "{query}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn same_key_retry_returns_the_retained_projection_without_reconciliation() {
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
    let request = IdentityContextRequest {
        operation_id: "ctxop-noop".into(),
        invocation: "automatic".into(),
        action: "context".into(),
        facts: IdentityFacts::default(),
        approval: None,
        grant_approval: None,
        query: false,
        query_capability: "base64url:capability".into(),
        invocation_ticket: String::new(),
    };
    let admitted = journal
        .append(crate::server::operation_journal::OperationAdmission {
            operation_id: request.operation_id.clone(),
            project_scope: context.project_scope.as_str().to_owned(),
            app_scope_id: context.app_scope_id.as_str().to_owned(),
            action: request.action.clone(),
            invocation: request.invocation.clone(),
            intent_digest: identity_operation_intent_digest(&context, &request),
            query_capability_hash: crate::server::operation_journal::capability_hash(
                &request.query_capability,
            ),
            phase: crate::proto::IdentityOperationPhase::Admitted,
            committed_phases: Vec::new(),
            nested_command_id: None,
            nested_operation_id: None,
            approval_evidence: None,
        })
        .unwrap();
    let before = std::fs::read(host_paths.journal_path()).unwrap();
    let (_, response) = manager.identity_context(context, request);
    let after = std::fs::read(host_paths.journal_path()).unwrap();
    // The retained projection is honestly incomplete: outer `ok` is false
    // for the `unknown` outcome, and the same key neither appends nor
    // dispatches a side effect.
    assert!(!response.ok, "{:?}", response.error);
    assert_eq!(before, after, "same-key retry must not append or dispatch");
    assert_eq!(
        response.data["result"]["operation_id"],
        admitted.operation_id
    );
    assert_eq!(response.data["result"]["outcome"], "unknown");
    std::fs::remove_dir_all(&root).unwrap();
}

fn b03_transport(thread: &str) -> crate::proto::SelectedTransport {
    crate::server::peer_tests::test_appserver_transport(thread)
}

fn b03_journal(server: &crate::server::Server) -> Arc<OperationJournal> {
    Arc::new(OperationJournal::open(server.host_paths.journal_path()).unwrap())
}

/// The exact prepared envelope is what gets consumed: the receipt's nested
/// command/operation IDs equal the IDs carried on the prepared envelope.
#[test]
fn s23_b03_prepared_register_envelope_ids_equal_the_consumed_receipt() {
    let (server, root) = test_server();
    let worker_id = "b03-worker-1";
    let token = "b03-token";
    let transport = b03_transport("thread-b03-1");
    let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = AppServerId::new("tui-default").unwrap();
    let prepared = server
        .prepare_register_envelope_for_scope(
            worker_id,
            token,
            &transport,
            project_scope.clone(),
            &root.to_string_lossy(),
            app_scope.clone(),
            false,
        )
        .unwrap();
    let prepared_command = prepared.nested_command_id.clone();
    let prepared_operation = prepared.nested_operation_id.clone();
    let response = consume_prepared_register_typed(&server, worker_id, &transport, prepared, None);
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.data["command_id"], prepared_command);
    assert_eq!(response.data["operation_id"], prepared_operation);
    assert_eq!(response.data["replayed"], false);
    std::fs::remove_dir_all(root).unwrap();
}

/// If the durable outer `Validating` sync fails, Register is never
/// consumed: no worker is registered and no nested receipt exists.
#[test]
fn s23_b03_outer_sync_failure_prevents_register_consume() {
    let (server, root) = test_server();
    let worker_id = "b03-worker-2";
    let token = "b03-token";
    let transport = b03_transport("thread-b03-2");
    let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = AppServerId::new("tui-default").unwrap();
    let journal = b03_journal(&server);
    journal
        .append(crate::server::operation_journal::OperationAdmission {
            operation_id: "ctxop-b03-bind".into(),
            project_scope: project_scope.as_str().to_owned(),
            app_scope_id: app_scope.as_str().to_owned(),
            action: "context".into(),
            invocation: "automatic".into(),
            intent_digest: "digest".into(),
            query_capability_hash: crate::server::operation_journal::capability_hash("cap"),
            phase: crate::proto::IdentityOperationPhase::Admitted,
            committed_phases: Vec::new(),
            nested_command_id: None,
            nested_operation_id: None,
            approval_evidence: None,
        })
        .unwrap();
    let binding = RegisterOuterBinding::new(journal, "ctxop-b03-bind".into());
    inject_next_append_fault();
    let mut before_consume =
        |prepared: &PreparedRegisterEnvelope| binding.bind_validating(prepared);
    let response = register_typed_observed(
        &server,
        worker_id,
        token,
        &transport,
        &root.to_string_lossy(),
        Some(project_scope),
        Some(app_scope),
        false,
        None,
        Some(&mut before_consume),
    );
    assert!(!response.ok, "{response:?}");
    assert!(
        response
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("IDENTITY_OPERATION_DURABILITY_FAILED")),
        "{:?}",
        response.error
    );
    assert!(binding.bound().is_none());
    assert!(!server.state.lock().unwrap().workers.contains_key(worker_id));
    assert!(server
        .state
        .lock()
        .unwrap()
        .global
        .command_receipts
        .is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

/// The pure readback returns the real inner receipt only when the nested
/// command id exists and the nested operation id matches exactly.
#[test]
fn s23_b03_inner_receipt_readback_requires_exact_nested_ids() {
    let (server, root) = test_server();
    let host_paths = server.host_paths.clone();
    let journal = Arc::new(OperationJournal::open(host_paths.journal_path()).unwrap());
    let manager = ProjectRuntimeManager::new_with_operation_journal(
        Arc::new(server),
        &host_paths,
        journal.clone(),
    )
    .unwrap();
    let worker_id = "b03-worker-3";
    let token = "b03-token";
    let transport = b03_transport("thread-b03-3");
    let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = AppServerId::new("tui-default").unwrap();
    let context = ProjectContext::for_registered_root_with_app(&root, app_scope.clone()).unwrap();
    manager.install_runtime(
        &(
            app_scope.as_str().to_owned(),
            project_scope.as_str().to_owned(),
        ),
        manager.host.clone(),
        None,
    );
    let prepared = manager
        .host
        .prepare_register_envelope_for_scope(
            worker_id,
            token,
            &transport,
            project_scope.clone(),
            &root.to_string_lossy(),
            app_scope,
            false,
        )
        .unwrap();
    let command_id = prepared.nested_command_id.clone();
    let operation_id = prepared.nested_operation_id.clone();
    let response =
        consume_prepared_register_typed(&manager.host, worker_id, &transport, prepared, None);
    assert!(
        response.ok,
        "error={:?} data={}",
        response.error, response.data
    );

    let projection = crate::proto::IdentityOperationProjection {
        operation_id: "ctxop-b03-read".into(),
        phase: crate::proto::IdentityOperationPhase::InnerDispatched,
        outcome: "unknown".into(),
        committed_phases: vec![crate::proto::IdentityOperationPhase::Validating],
        business_receipts: Vec::new(),
        nested_command_id: Some(command_id.clone()),
        nested_operation_id: Some(operation_id.clone()),
    };
    let readback = manager
        .inner_register_receipt_readback(&context, &projection)
        .unwrap();
    assert_eq!(readback["command_id"], command_id);
    assert_eq!(readback["operation_id"], operation_id);

    // A mismatched nested operation id must not be shown.
    let wrong = crate::proto::IdentityOperationProjection {
        nested_operation_id: Some("register-op-wrong".into()),
        ..projection.clone()
    };
    assert!(manager
        .inner_register_receipt_readback(&context, &wrong)
        .is_none());

    // A missing command id is never guessed.
    let missing = crate::proto::IdentityOperationProjection {
        nested_command_id: None,
        ..projection
    };
    assert!(manager
        .inner_register_receipt_readback(&context, &missing)
        .is_none());
    std::fs::remove_dir_all(root).unwrap();
}

/// A revision change between prepare and consume is rejected by the
/// existing CAS: no worker is registered, no receipt is created, and the
/// prepared nested IDs are not rewritten or retried.
#[test]
fn s23_b03_stale_prepared_revision_is_rejected_without_effect_or_reprepare() {
    let (server, root) = test_server();
    let worker_id = "b03-worker-4";
    let token = "b03-token";
    let transport = b03_transport("thread-b03-4");
    let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = AppServerId::new("tui-default").unwrap();
    let journal = b03_journal(&server);
    journal
        .append(crate::server::operation_journal::OperationAdmission {
            operation_id: "ctxop-b03-stale".into(),
            project_scope: project_scope.as_str().to_owned(),
            app_scope_id: app_scope.as_str().to_owned(),
            action: "context".into(),
            invocation: "automatic".into(),
            intent_digest: "digest".into(),
            query_capability_hash: crate::server::operation_journal::capability_hash("cap"),
            phase: crate::proto::IdentityOperationPhase::Admitted,
            committed_phases: Vec::new(),
            nested_command_id: None,
            nested_operation_id: None,
            approval_evidence: None,
        })
        .unwrap();
    let binding = RegisterOuterBinding::new(journal, "ctxop-b03-stale".into());
    let prepared = server
        .prepare_register_envelope_for_scope(
            worker_id,
            token,
            &transport,
            project_scope.clone(),
            &root.to_string_lossy(),
            app_scope.clone(),
            false,
        )
        .unwrap();
    let prepared_command = prepared.nested_command_id.clone();
    let prepared_operation = prepared.nested_operation_id.clone();
    binding.bind_validating(&prepared).unwrap();
    assert_eq!(
        binding
            .bound()
            .as_ref()
            .map(|(command, _)| command.as_str()),
        Some(prepared_command.as_str())
    );
    // A concurrent project mutation advances the reducer revision after
    // prepare, so the prepared envelope's CAS must now fail.
    server
        .commit_checked(&[Event::KeepaliveUpdated {
            worker_id: "b03-other-worker".into(),
            record: crate::server::keepalive::Record::default(),
        }])
        .unwrap();
    let response = server.consume_prepared_register(prepared).unwrap_err();
    assert!(response.to_string().contains("revision"), "{response}");
    let state = server.state.lock().unwrap();
    assert!(!state.workers.contains_key(worker_id));
    assert!(!state
        .global
        .lookup_command_receipt(&CommandId::new(prepared_command.clone()).unwrap())
        .is_some());
    drop(state);
    // The bound IDs stay exactly as prepared; no reprepare/new suffix.
    assert_eq!(
        binding.bound(),
        Some((prepared_command, prepared_operation))
    );
    std::fs::remove_dir_all(root).unwrap();
}

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

/// Serve the exact AppServer admission handshake (`initialize` then
/// `thread/read`) so the daemon's real `verify_candidate` accepts the
/// candidate without any production fault seam.
fn serve_fake_appserver(
    listener: std::os::unix::net::UnixListener,
    session: &str,
    thread: &str,
    cwd: &Path,
    connections: usize,
) -> std::thread::JoinHandle<()> {
    use std::io::{Read, Write};
    let session = session.to_owned();
    let thread = thread.to_owned();
    let cwd = cwd.to_string_lossy().into_owned();
    std::thread::spawn(move || {
        for _ in 0..connections {
            let (mut stream, _) = listener.accept().unwrap();
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
                let request = read_test_frame(&mut stream);
                let request: serde_json::Value = serde_json::from_slice(&request).unwrap();
                let Some(id) = request.get("id").cloned() else {
                    continue;
                };
                let method = request["method"].as_str().unwrap();
                let response = match method {
                    "initialize" => json!({"id": id, "result": {}}),
                    "thread/read" => {
                        json!({
                            "id": id,
                            "result": {"thread": {"id": thread, "sessionId": session, "cwd": cwd}}
                        })
                    }
                    "thread/items/list" => {
                        json!({"id": id, "error": {"code": -32601, "message": "unsupported"}})
                    }
                    "turn/start" | "turn/steer" => {
                        json!({"id": id, "error": {"code": -32600, "message": "invalid params"}})
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
                if method == "thread/turns/list" {
                    break;
                }
            }
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    })
}

fn read_test_frame(stream: &mut std::os::unix::net::UnixStream) -> Vec<u8> {
    use std::io::Read;
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header).unwrap();
    let masked = header[1] & 0x80 != 0;
    let mut length = (header[1] & 0x7f) as usize;
    if length == 126 {
        let mut bytes = [0_u8; 2];
        stream.read_exact(&mut bytes).unwrap();
        length = u16::from_be_bytes(bytes) as usize;
    }
    let mut mask = [0_u8; 4];
    if masked {
        stream.read_exact(&mut mask).unwrap();
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).unwrap();
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    payload
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
fn recovery_appserver(
    endpoint: &Path,
    session: &str,
    thread: &str,
    cwd: &Path,
    connections: usize,
) -> std::thread::JoinHandle<()> {
    let listener = std::os::unix::net::UnixListener::bind(endpoint).unwrap();
    serve_fake_appserver(listener, session, thread, cwd, connections)
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
        1,
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
    let recovery_server = recovery_appserver(
        &endpoint,
        facts.session_id.as_deref().expect("recovery session"),
        &thread,
        &root,
        1,
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
            Some("token-2")
        );
    }
    let after_grant = grant_for(&server, &root, &binding_id).expect("retained grant");
    assert_eq!(after_grant.agent_id, before_grant.agent_id);
    assert_eq!(after_grant.binding_id, before_grant.binding_id);
    assert_eq!(after_grant.endpoint_generation, new_generation);
    assert_eq!(after_grant.granted_by, before_grant.granted_by);
    std::fs::remove_dir_all(root).unwrap();
}

/// A changed incumbent fence is refused with the accepted typed conflict
/// before any owner effect, so no binding, credential, or grant moves.
#[test]
fn stale_incumbent_fence_is_refused_with_no_side_effects() {
    if !unix_sockets_available() {
        eprintln!("SKIP stale-incumbent owner test: sandbox denied unix socket bind");
        return;
    }
    let (server, root) = test_server();
    let server = Arc::new(server);
    let manager = recovery_manager(server.clone());
    let (facts, binding_id, incumbent_generation, _agent, _endpoint, _thread) =
        stale_master_incumbent(&server, &root, "peer-stale");
    let context = recovery_context(&root);
    let before_grant = grant_for(&server, &root, &binding_id).expect("master grant");
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
    assert!(!response.ok);
    assert_eq!(response.data["result"]["outcome"], "denied");
    assert_eq!(
        response.data["result"]["owner_readback"]["error"],
        "APPROVAL_STALE_CONFLICT"
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
            .expect("binding");
        assert_eq!(binding.endpoint_generation, incumbent_generation);
    }
    let after_grant = grant_for(&server, &root, &binding_id).expect("grant");
    assert_eq!(after_grant, before_grant);
    assert!(manager
        .operation_journal
        .query(
            &request,
            context.project_scope.as_str(),
            context.app_scope_id.as_str()
        )
        .is_err());
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
        1,
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
