use super::operation_journal::{
    capability_hash, inject_next_append_fault, OperationAdmission, OperationJournal,
};
use super::*;
use crate::proto::{
    IdentityContextRequest, IdentityOperationPhase, RequestEnvelope, TransportCandidates,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn test_root(name: &str) -> PathBuf {
    // The macOS temporary directory can itself be long enough to push an AF_UNIX
    // socket path past SUN_LEN. Keep the private fixture root short so the test
    // exercises daemon behavior rather than sockaddr path limits.
    let root = PathBuf::from(format!(
        "/tmp/co-{name}-{}-{}",
        std::process::id(),
        now_ms()
    ));
    std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
    root
}

fn admission(operation_id: &str, intent_digest: &str) -> OperationAdmission {
    OperationAdmission {
        operation_id: operation_id.into(),
        project_scope: "/tmp/project".into(),
        app_scope_id: crate::identity::CLI_APP_SERVER_ID.into(),
        action: "context".into(),
        invocation: "automatic".into(),
        intent_digest: intent_digest.into(),
        query_capability_hash: capability_hash("raw-capability-that-must-not-persist"),
        phase: IdentityOperationPhase::Admitted,
        committed_phases: Vec::new(),
        nested_command_id: None,
        nested_operation_id: None,
        approval_evidence: None,
    }
}

fn query_request(capability: &str) -> IdentityContextRequest {
    IdentityContextRequest {
        operation_id: "ctxop-1".into(),
        invocation: "query".into(),
        action: "query".into(),
        facts: Default::default(),
        approval: None,
        grant_approval: None,
        query: true,
        query_capability: capability.into(),
        invocation_ticket: String::new(),
    }
}

fn malformed_query_request(capability: &str) -> IdentityContextRequest {
    IdentityContextRequest {
        operation_id: "ctxop-1".into(),
        invocation: "automatic".into(),
        action: "query".into(),
        facts: Default::default(),
        approval: None,
        grant_approval: None,
        query: false,
        query_capability: capability.into(),
        invocation_ticket: String::new(),
    }
}

#[test]
fn identity_operation_envelope_matches_the_frozen_top_level_wire_shape() {
    let context = ProjectContext {
        app_scope_id: AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
        canonical_root: "/tmp/project".into(),
        project_scope: crate::scope::ProjectScopeId::new("/tmp/project").unwrap(),
        runtime_context: None,
    };
    let request = RequestEnvelope::with_context(
        Req::IdentityContext {
            facts: Default::default(),
            identity_context: Some(query_request("capability")),
        },
        context,
    );
    let wire = serde_json::to_value(request).unwrap();
    assert_eq!(wire["op"], "IdentityContext");
    assert!(wire.get("project_context").is_some());
    assert!(wire.get("identity_context").is_some());
    assert!(wire.get("facts").is_none());
    assert!(wire["identity_context"].get("facts").is_some());
}

#[test]
fn append_sync_reopen_and_replay_returns_the_exact_projection() {
    let root = test_root("replay");
    let journal_path = root.join("journal.jsonl");
    let journal = OperationJournal::open(&journal_path).unwrap();
    let first = journal.append(admission("ctxop-1", "digest-1")).unwrap();
    assert_eq!(first.operation_id, "ctxop-1");
    assert_eq!(first.phase, IdentityOperationPhase::Admitted);
    assert!(first.committed_phases.is_empty());
    drop(journal);

    let reopened = OperationJournal::open(&journal_path).unwrap();
    let queried = reopened
        .query(
            &query_request("raw-capability-that-must-not-persist"),
            "/tmp/project",
            crate::identity::CLI_APP_SERVER_ID,
        )
        .unwrap();
    assert_eq!(queried.result, first);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn business_receipts_survive_reopen_and_query_without_rewriting_lifecycle() {
    let root = test_root("business-receipts");
    let journal_path = root.join("journal.jsonl");
    let journal = OperationJournal::open(&journal_path).unwrap();
    journal.append(admission("ctxop-1", "digest-1")).unwrap();
    journal
        .transition("ctxop-1", IdentityOperationPhase::Validating, None, None)
        .unwrap();
    journal
        .transition(
            "ctxop-1",
            IdentityOperationPhase::InnerDispatched,
            Some("command-1".into()),
            Some("operation-1".into()),
        )
        .unwrap();
    let receipts = vec![
        "nested_register".into(),
        "route".into(),
        "credential".into(),
        "lease".into(),
        "context_complete".into(),
    ];
    journal
        .transition_with_business_receipts(
            "ctxop-1",
            IdentityOperationPhase::EffectObserved,
            None,
            None,
            Some(receipts.clone()),
        )
        .unwrap();
    journal
        .transition_with_business_receipts(
            "ctxop-1",
            IdentityOperationPhase::Completed,
            None,
            None,
            Some(receipts.clone()),
        )
        .unwrap();
    drop(journal);

    let reopened = OperationJournal::open(&journal_path).unwrap();
    let queried = reopened
        .query(
            &query_request("raw-capability-that-must-not-persist"),
            "/tmp/project",
            crate::identity::CLI_APP_SERVER_ID,
        )
        .unwrap();
    assert_eq!(queried.result.business_receipts, receipts);
    assert_eq!(
        queried.result.committed_phases,
        vec![
            IdentityOperationPhase::Validating,
            IdentityOperationPhase::InnerDispatched,
            IdentityOperationPhase::EffectObserved,
            IdentityOperationPhase::Completed,
        ]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn same_key_same_intent_is_a_noop_and_changed_intent_conflicts() {
    let root = test_root("idempotency");
    let journal = OperationJournal::open(root.join("journal.jsonl")).unwrap();
    let first = journal.append(admission("ctxop-1", "digest-1")).unwrap();
    let before = std::fs::read(journal.path()).unwrap();
    let repeated = journal.append(admission("ctxop-1", "digest-1")).unwrap();
    assert_eq!(repeated, first);
    assert_eq!(before, std::fs::read(journal.path()).unwrap());
    let error = journal
        .append(admission("ctxop-1", "digest-2"))
        .expect_err("changed intent must conflict");
    assert!(
        error.starts_with("IDENTITY_OPERATION_INTENT_CONFLICT"),
        "{error}"
    );
    assert_eq!(before, std::fs::read(journal.path()).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn incomplete_trailing_record_keeps_valid_prefix_and_poisons_appends() {
    let root = test_root("torn-tail");
    let journal_path = root.join("journal.jsonl");
    let journal = OperationJournal::open(&journal_path).unwrap();
    journal.append(admission("ctxop-1", "digest-1")).unwrap();
    journal
        .transition("ctxop-1", IdentityOperationPhase::Validating, None, None)
        .unwrap();
    journal
        .transition(
            "ctxop-1",
            IdentityOperationPhase::InnerDispatched,
            None,
            None,
        )
        .unwrap();
    drop(journal);
    let mut bytes = std::fs::read(&journal_path).unwrap();
    bytes.extend_from_slice(b"{\"schema_version\":1,\"record_type\":\"identity_operation\"");
    std::fs::write(&journal_path, bytes).unwrap();

    let reopened = OperationJournal::open(&journal_path).unwrap();
    assert!(reopened.is_append_poisoned());
    let queried = reopened
        .query(
            &query_request("raw-capability-that-must-not-persist"),
            "/tmp/project",
            crate::identity::CLI_APP_SERVER_ID,
        )
        .unwrap();
    assert_eq!(queried.result.operation_id, "ctxop-1");
    assert_eq!(queried.result.phase, IdentityOperationPhase::Unknown);
    assert_eq!(queried.result.outcome, "unknown");
    assert_eq!(
        queried.result.committed_phases,
        vec![
            IdentityOperationPhase::Validating,
            IdentityOperationPhase::InnerDispatched,
        ]
    );
    let error = reopened
        .append(admission("ctxop-2", "digest-2"))
        .expect_err("poisoned append must fail");
    assert!(
        error.starts_with("IDENTITY_OPERATION_DURABILITY_FAILED"),
        "{error}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn append_fault_is_typed_and_does_not_report_durable_success() {
    let root = test_root("append-fault");
    let journal = OperationJournal::open(root.join("journal.jsonl")).unwrap();
    inject_next_append_fault();
    let error = journal
        .append(admission("ctxop-1", "digest-1"))
        .expect_err("injected append fault must fail");
    assert!(
        error.starts_with("IDENTITY_OPERATION_DURABILITY_FAILED"),
        "{error}"
    );
    assert_eq!(std::fs::read(journal.path()).unwrap(), b"");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_complete_record_fails_closed() {
    let root = test_root("malformed");
    let journal_path = root.join("journal.jsonl");
    std::fs::write(&journal_path, b"{malformed}\n").unwrap();
    let error = OperationJournal::open(&journal_path)
        .err()
        .expect("malformed complete records must fail closed");
    assert!(error.to_string().contains("malformed complete record"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn replay_rejects_skipped_phase_transitions() {
    let root = test_root("illegal-replay-phase");
    let journal_path = root.join("journal.jsonl");
    let journal = OperationJournal::open(&journal_path).unwrap();
    journal.append(admission("ctxop-1", "digest-1")).unwrap();
    drop(journal);

    let original = std::fs::read_to_string(&journal_path).unwrap();
    let mut invalid: serde_json::Value =
        serde_json::from_str(original.lines().next().unwrap()).unwrap();
    invalid["sequence"] = serde_json::json!(1);
    invalid["phase"] = serde_json::json!("completed");
    invalid["committed_phases"] = serde_json::json!(["completed"]);
    let mut bytes = original.into_bytes();
    bytes.extend_from_slice(&serde_json::to_vec(&invalid).unwrap());
    bytes.push(b'\n');
    std::fs::write(&journal_path, bytes).unwrap();

    let error = OperationJournal::open(&journal_path)
        .err()
        .expect("replay must reject skipped phase transitions");
    assert!(error.to_string().contains("illegal phase transition"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn query_rejects_wrong_scope_or_capability_and_never_persists_secrets() {
    let root = test_root("query-proof");
    let journal_path = root.join("journal.jsonl");
    let journal = OperationJournal::open(&journal_path).unwrap();
    journal.append(admission("ctxop-1", "digest-1")).unwrap();
    let denied = journal
        .query(
            &query_request("wrong-capability"),
            "/tmp/project",
            crate::identity::CLI_APP_SERVER_ID,
        )
        .unwrap_err();
    assert!(
        denied.starts_with("IDENTITY_OPERATION_QUERY_DENIED"),
        "{denied}"
    );
    let wrong_scope = journal
        .query(
            &query_request("raw-capability-that-must-not-persist"),
            "/tmp/other",
            crate::identity::CLI_APP_SERVER_ID,
        )
        .unwrap_err();
    assert!(
        wrong_scope.starts_with("IDENTITY_OPERATION_QUERY_DENIED"),
        "{wrong_scope}"
    );
    let raw = std::fs::read_to_string(&journal_path).unwrap();
    assert!(!raw.contains("raw-capability-that-must-not-persist"));
    assert!(!raw.contains("approval"));
    assert!(!raw.contains("token"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn nested_receipt_ids_survive_replay_without_dispatching_an_effect() {
    let root = test_root("nested-receipt");
    let journal_path = root.join("journal.jsonl");
    let journal = OperationJournal::open(&journal_path).unwrap();
    journal.append(admission("ctxop-1", "digest-1")).unwrap();
    journal
        .transition("ctxop-1", IdentityOperationPhase::Validating, None, None)
        .unwrap();
    journal
        .transition(
            "ctxop-1",
            IdentityOperationPhase::InnerDispatched,
            Some("command-1".into()),
            Some("operation-1".into()),
        )
        .unwrap();
    drop(journal);

    let reopened = OperationJournal::open(&journal_path).unwrap();
    let queried = reopened
        .query(
            &query_request("raw-capability-that-must-not-persist"),
            "/tmp/project",
            crate::identity::CLI_APP_SERVER_ID,
        )
        .unwrap();
    assert_eq!(
        queried.result.phase,
        IdentityOperationPhase::InnerDispatched
    );
    assert_eq!(
        queried.result.nested_command_id.as_deref(),
        Some("command-1")
    );
    assert_eq!(
        queried.result.nested_operation_id.as_deref(),
        Some("operation-1")
    );
    assert_eq!(
        queried.result.committed_phases,
        vec![
            IdentityOperationPhase::Validating,
            IdentityOperationPhase::InnerDispatched,
        ]
    );
    assert_eq!(
        std::fs::read_dir(root.join("project"))
            .map(|mut it| it.next().is_some())
            .unwrap_or(false),
        false
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn illegal_and_terminal_phase_transitions_are_rejected_without_journal_delta() {
    let root = test_root("phase-transition");
    let journal = OperationJournal::open(root.join("journal.jsonl")).unwrap();
    journal.append(admission("ctxop-1", "digest-1")).unwrap();
    let before = std::fs::read(journal.path()).unwrap();
    let illegal = journal
        .transition("ctxop-1", IdentityOperationPhase::Completed, None, None)
        .expect_err("completion cannot skip validating, dispatch, and readback");
    assert!(illegal.starts_with("IDENTITY_OPERATION_PHASE_CONFLICT"));
    assert_eq!(before, std::fs::read(journal.path()).unwrap());

    journal
        .transition("ctxop-1", IdentityOperationPhase::Unknown, None, None)
        .unwrap();
    let unresolved = std::fs::read(journal.path()).unwrap();
    let terminal = journal
        .transition("ctxop-1", IdentityOperationPhase::Validating, None, None)
        .expect_err("unknown stays unresolved until a separate owner action");
    assert!(terminal.starts_with("IDENTITY_OPERATION_PHASE_CONFLICT"));
    assert_eq!(unresolved, std::fs::read(journal.path()).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}

async fn send_request(socket: &Path, request: &RequestEnvelope) -> crate::proto::Resp {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut stream = tokio::net::UnixStream::connect(socket).await.unwrap();
    let mut line = serde_json::to_vec(request).unwrap();
    line.push(b'\n');
    stream.write_all(&line).await.unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}

async fn wait_for_socket(socket: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if crate::client::alive(socket) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "degraded daemon did not publish socket {}",
        socket.display()
    );
}

#[tokio::test]
async fn degraded_startup_serves_only_authorized_query_and_keeps_project_journal_unchanged() {
    let _startup_test_lock = startup_test_lock();
    let root = test_root("degraded");
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let context = ProjectContext::for_registered_root_with_app(
        &root,
        AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
    )
    .unwrap();
    let journal_path = host_paths.journal_path();
    let journal = OperationJournal::open(&journal_path).unwrap();
    let mut operation = admission("ctxop-1", "digest-1");
    operation.project_scope = context.project_scope.as_str().to_owned();
    operation.app_scope_id = context.app_scope_id.as_str().to_owned();
    journal.append(operation).unwrap();
    drop(journal);

    let project_journal = root.join(".agent-collab/server/journal.jsonl");
    std::fs::write(&project_journal, b"{\"broken\":\"project-record\"}\n").unwrap();
    let project_before = std::fs::read(&project_journal).unwrap();
    let host_before = std::fs::read(host_paths.journal_path()).unwrap();

    let socket = host_paths.socket_path();
    let running = tokio::spawn(run_with_host_paths(
        Scope { root: root.clone() },
        host_paths.clone(),
    ));
    wait_for_socket(&socket).await;

    let response = send_request(
        &socket,
        &RequestEnvelope::with_context(
            Req::IdentityContext {
                facts: Default::default(),
                identity_context: Some(malformed_query_request("raw-malformed-capability")),
            },
            context.clone(),
        ),
    )
    .await;
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some("IDENTITY_OPERATION_QUERY_SHAPE_INVALID")
    );

    let response = send_request(
        &socket,
        &RequestEnvelope::with_context(
            Req::IdentityContext {
                facts: Default::default(),
                identity_context: Some(query_request("raw-capability-that-must-not-persist")),
            },
            context.clone(),
        ),
    )
    .await;
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["result"]["operation_id"], "ctxop-1");
    assert_eq!(
        host_before,
        std::fs::read(host_paths.journal_path()).unwrap()
    );

    let response = send_request(
        &socket,
        &RequestEnvelope::with_context(
            Req::Register {
                worker_id: "worker-1".into(),
                token: "token-1".into(),
                cwd: root.display().to_string(),
                candidates: Some(TransportCandidates::default()),
            },
            context,
        ),
    )
    .await;
    assert!(!response.ok);
    assert!(response
        .error
        .as_deref()
        .unwrap_or_default()
        .starts_with("PROJECT_RUNTIME_UNAVAILABLE"));
    assert_eq!(project_before, std::fs::read(&project_journal).unwrap());
    assert_eq!(
        host_before,
        std::fs::read(host_paths.journal_path()).unwrap()
    );

    running.abort();
    let _ = running.await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn healthy_startup_serves_host_query_before_project_route_admission() {
    let _startup_test_lock = startup_test_lock();
    let root = test_root("healthy");
    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    host_paths.ensure_root().unwrap();
    let context = ProjectContext::for_registered_root_with_app(
        &root,
        AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
    )
    .unwrap();
    let journal = OperationJournal::open(host_paths.journal_path()).unwrap();
    let mut operation = admission("ctxop-1", "digest-1");
    operation.project_scope = context.project_scope.as_str().to_owned();
    operation.app_scope_id = context.app_scope_id.as_str().to_owned();
    journal.append(operation).unwrap();
    drop(journal);

    let socket = host_paths.socket_path();
    let running = tokio::spawn(run_with_host_paths(
        Scope { root: root.clone() },
        host_paths.clone(),
    ));
    wait_for_socket(&socket).await;

    let project_journal = root.join(".agent-collab/server/journal.jsonl");
    let project_before = std::fs::read(&project_journal).unwrap();
    let host_before = std::fs::read(host_paths.journal_path()).unwrap();
    let activity_path = root.join(".agent-collab/server/events.jsonl");
    let activity_before = std::fs::read(&activity_path).ok();
    let response = send_request(
        &socket,
        &RequestEnvelope::with_context(
            Req::IdentityContext {
                facts: Default::default(),
                identity_context: Some(malformed_query_request("raw-malformed-capability")),
            },
            context.clone(),
        ),
    )
    .await;
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some("IDENTITY_OPERATION_QUERY_SHAPE_INVALID")
    );
    assert_eq!(project_before, std::fs::read(&project_journal).unwrap());
    assert_eq!(
        host_before,
        std::fs::read(host_paths.journal_path()).unwrap()
    );
    assert_eq!(activity_before, std::fs::read(&activity_path).ok());

    let response = send_request(
        &socket,
        &RequestEnvelope::with_context(
            Req::IdentityContext {
                facts: Default::default(),
                identity_context: Some(query_request("raw-capability-that-must-not-persist")),
            },
            context,
        ),
    )
    .await;
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["result"]["operation_id"], "ctxop-1");
    assert_eq!(project_before, std::fs::read(&project_journal).unwrap());
    assert_eq!(
        host_before,
        std::fs::read(host_paths.journal_path()).unwrap()
    );

    running.abort();
    let _ = running.await;
    std::fs::remove_dir_all(root).unwrap();
}
