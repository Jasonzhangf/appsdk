//! Peer lifecycle coordination. Read never commits or repairs owner state.
use super::*;
use crate::client::adapters::codex_app_server::{
    archive_thread, immediate_notify, read_thread_history, start_thread, update_thread_cwd,
    ThreadSettingsUpdate,
};
use crate::client::adapters::AdapterError;
use crate::proto::*;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn result(action: PeerLifecycleAction, outcome: PeerLifecycleOutcome) -> PeerLifecycleResult {
    PeerLifecycleResult {
        action,
        operation_id: None,
        target: None,
        outcome,
        source: PeerLifecycleSource::CommittedProjection,
        phase: None,
        projection: None,
        update: None,
        close: None,
        create: None,
        requires: ContextOperationRequires::default(),
    }
}

fn response(result: PeerLifecycleResult, error: Option<&str>) -> Resp {
    Resp {
        ok: error.is_none(),
        error: error.map(str::to_owned),
        data: json!({"result": result}),
    }
}

pub(super) fn unavailable(request: &PeerLifecycleRequest, error: &str, refused: bool) -> Resp {
    let action = match request {
        PeerLifecycleRequest::Create { .. } => PeerLifecycleAction::Create,
        PeerLifecycleRequest::Update { .. } => PeerLifecycleAction::Update,
        PeerLifecycleRequest::Close { .. } => PeerLifecycleAction::Close,
        _ => PeerLifecycleAction::Read,
    };
    response(
        result(
            action,
            if refused {
                PeerLifecycleOutcome::Refused
            } else {
                PeerLifecycleOutcome::Unknown
            },
        ),
        Some(error),
    )
}

fn exact_target(
    state: &State,
    context: &ProjectContext,
    worker: &WorkerRec,
) -> Result<PeerLifecycleTarget, PeerLifecycleOutcome> {
    let mut bindings = state
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .filter(|binding| binding.agent_id.as_str() == worker.id);
    let binding = bindings.next().ok_or(PeerLifecycleOutcome::Unknown)?;
    if bindings.next().is_some() {
        return Err(PeerLifecycleOutcome::Unknown);
    }
    if binding.project_scope != context.project_scope
        || binding.app_scope_id != context.app_scope_id
    {
        return Err(PeerLifecycleOutcome::Refused);
    }
    let mut routes = state
        .global
        .current_thread_routes
        .values()
        .filter(|route| route.agent_id.as_str() == worker.id);
    if routes.next() != Some(binding) || routes.next().is_some() {
        return Err(PeerLifecycleOutcome::Unknown);
    }
    let transport = worker
        .transport
        .as_ref()
        .ok_or(PeerLifecycleOutcome::Unknown)?;
    if binding.session_id.as_ref().map(|id| id.as_str()) != transport.session_id.as_deref()
        || binding.native_thread_id.as_ref().map(|id| id.as_str()) != transport.thread_id.as_deref()
        || binding.tmux_endpoint != transport.tmux_endpoint
    {
        return Err(PeerLifecycleOutcome::Unknown);
    }
    Ok(PeerLifecycleTarget {
        worker_id: worker.id.clone(),
        project_scope: binding.project_scope.clone(),
        app_scope_id: binding.app_scope_id.clone(),
        binding_id: binding.binding_id.clone(),
        endpoint_generation: binding.endpoint_generation,
        transport: PeerLifecycleTransport {
            kind: transport.kind.clone(),
            endpoint: transport.endpoint.clone(),
            namespace: transport.namespace.clone(),
            session_id: transport.session_id.clone(),
            thread_id: transport.thread_id.clone(),
            tmux_endpoint: transport.tmux_endpoint.clone(),
        },
    })
}

fn authorized(server: &Server, state: &State, actor: &str, target: &str) -> bool {
    actor == target
        || current_master_holder(server, state)
            .ok()
            .flatten()
            .as_deref()
            == Some(actor)
        || state
            .subagents
            .values()
            .any(|record| record.peer == target && record.parent == actor)
}

fn legacy_close() -> PeerLifecycleClose {
    let unknown = PeerLifecycleStageReadback {
        state: PeerLifecycleStage::Unknown,
        challenge: None,
    };
    PeerLifecycleClose {
        close_outcome: "closed_record_only".into(),
        runtime_archive: unknown.clone(),
        worker_retirement: unknown.clone(),
        binding_retirement: unknown.clone(),
        route_retirement: unknown.clone(),
        lease_retirement: unknown.clone(),
        subscription_retirement: unknown,
    }
}

fn capability_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn update_intent_digest(operation_id: &str, target: &PeerLifecycleTarget, cwd: &str) -> String {
    let intent = json!({
        "action": "update",
        "operation_id": operation_id,
        "target": target,
        "cwd": cwd,
    });
    let bytes = serde_json::to_vec(&intent).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn close_intent_digest(operation_id: &str, target: &PeerLifecycleTarget, reason: &str) -> String {
    let intent = json!({
        "action": "close",
        "operation_id": operation_id,
        "target": target,
        "reason": reason,
    });
    let bytes = serde_json::to_vec(&intent).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn normalize_cwd(cwd: &str) -> Result<String, String> {
    let canonical = std::fs::canonicalize(cwd)
        .map_err(|error| format!("PEER_LIFECYCLE_CWD_INVALID: {cwd}: {error}"))?;
    if !canonical.is_dir() {
        return Err(format!(
            "PEER_LIFECYCLE_CWD_INVALID: {} is not a directory",
            canonical.display()
        ));
    }
    Ok(canonical.to_string_lossy().into_owned())
}

fn phase_outcome(phase: &PeerLifecyclePhase) -> PeerLifecycleOutcome {
    match phase {
        PeerLifecyclePhase::IntentPersisted
        | PeerLifecyclePhase::HostDispatched
        | PeerLifecyclePhase::ReadbackPending
        | PeerLifecyclePhase::Partial => PeerLifecycleOutcome::Partial,
        PeerLifecyclePhase::Complete => PeerLifecycleOutcome::Complete,
        PeerLifecyclePhase::Refused => PeerLifecycleOutcome::Refused,
        PeerLifecyclePhase::HostDispatchClaimed | PeerLifecyclePhase::Unknown => {
            PeerLifecycleOutcome::Unknown
        }
        PeerLifecyclePhase::Cancelled => PeerLifecycleOutcome::Cancelled,
        PeerLifecyclePhase::CleanupOpen => PeerLifecycleOutcome::CleanupOpen,
    }
}

fn record_result(record: &state::PeerLifecycleOperationRecord) -> PeerLifecycleResult {
    PeerLifecycleResult {
        action: record.action.clone(),
        operation_id: Some(record.operation_id.clone()),
        target: record.target.clone(),
        outcome: phase_outcome(&record.phase),
        source: PeerLifecycleSource::LifecycleOperation,
        phase: Some(record.phase.clone()),
        projection: None,
        update: record.update.clone(),
        close: record.close.clone(),
        create: record.create.clone(),
        requires: ContextOperationRequires::default(),
    }
}

fn result_error(result: &PeerLifecycleResult) -> Option<&'static str> {
    match result.outcome {
        PeerLifecycleOutcome::Ok | PeerLifecycleOutcome::Complete => None,
        PeerLifecycleOutcome::Refused => Some("PEER_LIFECYCLE_REFUSED"),
        PeerLifecycleOutcome::Partial => Some("PEER_LIFECYCLE_PARTIAL"),
        PeerLifecycleOutcome::Unknown => Some("PEER_LIFECYCLE_UNKNOWN"),
        PeerLifecycleOutcome::Cancelled => Some("PEER_LIFECYCLE_CANCELLED"),
        PeerLifecycleOutcome::CleanupOpen => Some("PEER_LIFECYCLE_CLEANUP_OPEN"),
        PeerLifecycleOutcome::Missing => Some("PEER_LIFECYCLE_MISSING"),
        PeerLifecycleOutcome::Closed => Some("PEER_LIFECYCLE_CLOSED"),
    }
}

fn commit_record(
    server: &Server,
    operation: &state::PeerLifecycleOperationRecord,
) -> Result<(), String> {
    server.try_commit(&[state::Event::PeerLifecycleOperationRecorded {
        operation: operation.clone(),
    }])
}

fn advance_record(
    record: &state::PeerLifecycleOperationRecord,
    phase: PeerLifecyclePhase,
    update: Option<PeerLifecycleUpdate>,
) -> state::PeerLifecycleOperationRecord {
    advance_record_with_close(record, phase, update, None)
}

fn advance_record_with_close(
    record: &state::PeerLifecycleOperationRecord,
    phase: PeerLifecyclePhase,
    update: Option<PeerLifecycleUpdate>,
    close: Option<PeerLifecycleClose>,
) -> state::PeerLifecycleOperationRecord {
    let mut next = record.clone();
    next.phase = phase;
    if update.is_some() {
        next.update = update;
    }
    if close.is_some() {
        next.close = close;
    }
    next.updated_ms = now_ms();
    next
}

fn selected_transport(target: &PeerLifecycleTarget) -> SelectedTransport {
    SelectedTransport {
        kind: target.transport.kind.clone(),
        endpoint: target.transport.endpoint.clone(),
        namespace: target.transport.namespace.clone(),
        session_id: target.transport.session_id.clone(),
        thread_id: target.transport.thread_id.clone(),
        tmux_endpoint: target.transport.tmux_endpoint.clone(),
        capabilities: Vec::new(),
        self_check: "peer_lifecycle_exact_target".into(),
    }
}

/// Prefix for the operation-owned challenge marker. It is deliberately a
/// dotfile so it cannot collide with ordinary project content.
const CHALLENGE_MARKER_PREFIX: &str = ".collab-peer-lifecycle-challenge-";
/// The marker file name is carried in the prompt inside this token so the
/// peer knows which relative file to read. Only the file name travels in the
/// prompt; the marker contents never do.
const CHALLENGE_MARKER_TOKEN: &str = "[[marker:";

/// How many bounded history reads the initial response may spend trying to
/// observe an already-finished challenge turn. Exhausting this window returns
/// a truthful pending result; a later same-operation readback finalizes it.
const CHALLENGE_INITIAL_READS: usize = 3;

fn hash_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Prepare one operation-owned challenge in the intended execution directory.
///
/// Returns `(marker_file, marker_sha256, prompt)`. The marker file is created
/// exclusively so a user file is never overwritten, and its unpredictable
/// contents are not included in the prompt.
fn prepare_challenge(cwd: &str) -> Result<(String, String, String), String> {
    use rand::RngCore;
    let mut rng = rand::thread_rng();
    let mut name_bytes = [0_u8; 16];
    let mut secret_bytes = [0_u8; 24];
    rng.fill_bytes(&mut name_bytes);
    rng.fill_bytes(&mut secret_bytes);
    let name_token: String = name_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let marker: String = secret_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let marker_file = format!("{CHALLENGE_MARKER_PREFIX}{name_token}.txt");
    let path = Path::new(cwd).join(&marker_file);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("PEER_LIFECYCLE_CHALLENGE_MARKER_FAILED: {error}"))?;
    use std::io::Write;
    file.write_all(marker.as_bytes())
        .map_err(|error| format!("PEER_LIFECYCLE_CHALLENGE_MARKER_FAILED: {error}"))?;
    drop(file);
    let prompt = format!(
        "Run pwd in the current working directory, then read the relative file \
         {CHALLENGE_MARKER_TOKEN}{marker_file}]] from that same directory. Reply with \
         exactly post-update:<pwd>:<marker> and nothing else."
    );
    Ok((marker_file, hash_hex(marker.as_bytes()), prompt))
}

/// Re-read the operation-owned marker and confirm it still matches the hash
/// persisted at dispatch. A missing or tampered marker yields no proof.
fn read_challenge_marker(cwd: &str, marker_file: &str, expected_sha256: &str) -> Option<String> {
    let bytes = std::fs::read(Path::new(cwd).join(marker_file)).ok()?;
    if hash_hex(&bytes) != expected_sha256 {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn cleanup_challenge_marker(cwd: &str, marker_file: &str) -> Result<(), String> {
    match std::fs::remove_file(Path::new(cwd).join(marker_file)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "PEER_LIFECYCLE_CHALLENGE_MARKER_CLEANUP_FAILED: {error}"
        )),
    }
}

fn challenge_envelope(cwd: &str, marker: &str) -> String {
    format!("post-update:{cwd}:{marker}")
}

/// Prove the exact correlated challenge result: the frozen thread, the exact
/// challenge turn, a completion not older than the dispatch, and an
/// agentMessage equal to the exact expected envelope. Quotes, failure
/// reports, prefix matches, stale pre-dispatch turns and any other thread all
/// fail this predicate.
fn challenge_execution_turn(
    value: &serde_json::Value,
    thread_id: &str,
    turn_id: &str,
    expected: &str,
    dispatched_ms: i64,
) -> bool {
    let Some(thread) = value.get("thread") else {
        return false;
    };
    if thread.get("id").and_then(serde_json::Value::as_str) != Some(thread_id) {
        return false;
    }
    let dispatched_secs = dispatched_ms.div_euclid(1000);
    thread
        .get("turns")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|turns| {
            turns.iter().any(|turn| {
                turn.get("id").and_then(serde_json::Value::as_str) == Some(turn_id)
                    && turn.get("status").and_then(serde_json::Value::as_str) == Some("completed")
                    && turn
                        .get("completedAt")
                        .and_then(serde_json::Value::as_i64)
                        .is_some_and(|at| at >= dispatched_secs)
                    && turn
                        .get("items")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|items| {
                            items.iter().any(|item| {
                                item.get("type").and_then(serde_json::Value::as_str)
                                    == Some("agentMessage")
                                    && item.get("text").and_then(serde_json::Value::as_str)
                                        == Some(expected)
                            })
                        })
            })
        })
}

/// Verify a Create readiness challenge against the exact created thread.
fn verify_create_challenge(
    history: &serde_json::Value,
    record: &state::PeerLifecycleOperationRecord,
) -> Option<String> {
    let create = record.create.as_ref()?;
    let cwd = create.cwd.as_str();
    let thread_id = create.thread_id.as_deref()?;
    let challenge = create.readiness.challenge.as_ref()?;
    let turn_id = challenge.turn_id.as_deref()?;
    let marker = read_challenge_marker(cwd, &challenge.marker_file, &challenge.marker_sha256)?;
    let expected = challenge_envelope(cwd, &marker);
    challenge_execution_turn(
        history,
        thread_id,
        turn_id,
        &expected,
        challenge.dispatched_ms,
    )
    .then(|| turn_id.to_owned())
}

/// Verify an Update challenge against the exact target thread.
fn verify_update_challenge(
    history: &serde_json::Value,
    record: &state::PeerLifecycleOperationRecord,
) -> Option<String> {
    let target = record.target.as_ref()?;
    let thread_id = target.transport.thread_id.as_deref()?;
    let update = record.update.as_ref()?;
    let challenge = update.challenge.as_ref()?;
    let turn_id = challenge.turn_id.as_deref()?;
    let marker = read_challenge_marker(
        &update.intended_cwd,
        &challenge.marker_file,
        &challenge.marker_sha256,
    )?;
    let expected = challenge_envelope(&update.intended_cwd, &marker);
    challenge_execution_turn(
        history,
        thread_id,
        turn_id,
        &expected,
        challenge.dispatched_ms,
    )
    .then(|| turn_id.to_owned())
}

/// Prepare the verified Create readiness receipt. Marker cleanup is deferred
/// until after this completion state has crossed the durable commit boundary.
fn mark_create_challenge_verified(
    record: &mut state::PeerLifecycleOperationRecord,
    verified_turn: &str,
) {
    record.phase = PeerLifecyclePhase::Complete;
    let Some(create) = record.create.as_mut() else {
        return;
    };
    create.readiness.state = PeerLifecycleStage::Verified;
    if let Some(challenge) = create.readiness.challenge.as_mut() {
        challenge.state = PeerLifecycleStage::Verified;
        challenge.turn_id = Some(verified_turn.to_owned());
        challenge.cleanup = Some(PeerLifecycleStage::Pending);
    }
    create
        .stages
        .push(format!("completed_execution_turn:{verified_turn}"));
}

/// Prepare the verified Update receipt without removing evidence needed to
/// recover if its completion commit fails.
fn mark_update_challenge_verified(
    record: &mut state::PeerLifecycleOperationRecord,
    thread_id: &str,
    verified_turn: &str,
) {
    record.phase = PeerLifecyclePhase::Complete;
    let Some(update) = record.update.as_mut() else {
        return;
    };
    update.effective_cwd = PeerEffectiveCwd::Verified {
        cwd: update.intended_cwd.clone(),
        thread_id: Some(thread_id.to_owned()),
        turn_id: Some(verified_turn.to_owned()),
    };
    if let Some(challenge) = update.challenge.as_mut() {
        challenge.state = PeerLifecycleStage::Verified;
        challenge.cleanup = Some(PeerLifecycleStage::Pending);
    }
}

/// Remove a persisted Update challenge marker on a terminal path where no
/// later readback can finalize it, and retain any cleanup failure on the
/// returned challenge receipt.
fn cleanup_record_challenge(
    record: &state::PeerLifecycleOperationRecord,
) -> Option<PeerLifecycleChallenge> {
    let (cwd, challenge) = if let Some(create) = record.create.as_ref() {
        (create.cwd.as_str(), create.readiness.challenge.as_ref()?)
    } else {
        let update = record.update.as_ref()?;
        (update.intended_cwd.as_str(), update.challenge.as_ref()?)
    };
    let mut challenge = challenge.clone();
    let cleanup = cleanup_challenge_marker(cwd, &challenge.marker_file);
    challenge.cleanup = Some(if cleanup.is_ok() {
        PeerLifecycleStage::Verified
    } else {
        PeerLifecycleStage::Failed
    });
    if let Err(error) = &cleanup {
        challenge.cleanup_error = Some(error.clone());
    }
    Some(challenge)
}

/// Persist verified execution evidence before deleting the marker that proves
/// it. If the completion commit fails, the original marker remains available
/// for an exact same-operation readback. Cleanup receipt persistence is a
/// second, non-host-effect commit; if it fails, the durable receipt remains
/// honestly marked cleanup-pending.
fn commit_verified_readback(
    server: &Server,
    mut record: state::PeerLifecycleOperationRecord,
) -> Result<state::PeerLifecycleOperationRecord, String> {
    commit_record(server, &record)?;
    let cleanup = cleanup_record_challenge(&record);
    if let Some(challenge) = cleanup {
        if let Some(create) = record.create.as_mut() {
            if let Some(current) = create.readiness.challenge.as_mut() {
                *current = challenge.clone();
            }
            create.stages.push(match challenge.cleanup {
                Some(PeerLifecycleStage::Verified) => "challenge_marker_cleanup:verified".into(),
                Some(PeerLifecycleStage::Failed) => format!(
                    "challenge_marker_cleanup:failed:{}",
                    challenge
                        .cleanup_error
                        .as_deref()
                        .unwrap_or("unknown error")
                ),
                _ => "challenge_marker_cleanup:pending".into(),
            });
        } else if let Some(update) = record.update.as_mut() {
            update.challenge = Some(challenge);
        }
        if commit_record(server, &record).is_err() {
            // The durable completion record written above retains cleanup as
            // pending. Do not turn this secondary receipt update into a false
            // host-operation failure or claim that cleanup was durably read back.
            let mut durable = record.clone();
            if let Some(create) = durable.create.as_mut() {
                if let Some(challenge) = create.readiness.challenge.as_mut() {
                    challenge.cleanup = Some(PeerLifecycleStage::Pending);
                    challenge.cleanup_error = None;
                }
                create
                    .stages
                    .retain(|stage| !stage.starts_with("challenge_marker_cleanup:"));
            } else if let Some(update) = durable.update.as_mut() {
                if let Some(challenge) = update.challenge.as_mut() {
                    challenge.cleanup = Some(PeerLifecycleStage::Pending);
                    challenge.cleanup_error = None;
                }
            }
            return Ok(durable);
        }
    }
    Ok(record)
}

fn readback_commit_failure(
    server: &Server,
    record: &state::PeerLifecycleOperationRecord,
    error: &str,
) -> Resp {
    let stored = server
        .state
        .lock()
        .unwrap()
        .peer_lifecycle_operations
        .get(&record.operation_id)
        .cloned();
    let fallback = match record.action {
        PeerLifecycleAction::Create => {
            result(PeerLifecycleAction::Create, PeerLifecycleOutcome::Unknown)
        }
        PeerLifecycleAction::Update => {
            result(PeerLifecycleAction::Update, PeerLifecycleOutcome::Unknown)
        }
        _ => result(record.action.clone(), PeerLifecycleOutcome::Unknown),
    };
    response(
        stored.as_ref().map(record_result).unwrap_or(fallback),
        Some(&format!("PEER_LIFECYCLE_READBACK_COMMIT_FAILED: {error}")),
    )
}

/// Finalize one pending lifecycle operation from its already-dispatched
/// challenge turn. This never repeats a Native effect; it only reads history
/// and commits a verified receipt when the exact target is still current.
fn current_target_matches(
    server: &Server,
    context: &ProjectContext,
    record: &state::PeerLifecycleOperationRecord,
) -> bool {
    let Some(target) = record.target.as_ref() else {
        return false;
    };
    let state = server.state.lock().unwrap();
    state
        .workers
        .get(&target.worker_id)
        .and_then(|worker| exact_target(&state, context, worker).ok())
        .as_ref()
        == Some(target)
}

fn finalize_readback(
    server: &Server,
    context: &ProjectContext,
    record: &state::PeerLifecycleOperationRecord,
) -> Result<state::PeerLifecycleOperationRecord, String> {
    if record.phase != PeerLifecyclePhase::ReadbackPending {
        return Ok(record.clone());
    }
    if !current_target_matches(server, context, record) {
        return Ok(record.clone());
    }
    match record.action {
        PeerLifecycleAction::Create => finalize_create_readback(server, record),
        PeerLifecycleAction::Update => finalize_update_readback(server, record),
        _ => Ok(record.clone()),
    }
}

fn finalize_create_readback(
    server: &Server,
    record: &state::PeerLifecycleOperationRecord,
) -> Result<state::PeerLifecycleOperationRecord, String> {
    let Some(create) = record.create.as_ref() else {
        return Ok(record.clone());
    };
    let Some(thread_id) = create.thread_id.as_deref() else {
        return Ok(record.clone());
    };
    let Some(target) = record.target.as_ref() else {
        return Ok(record.clone());
    };
    let transport = selected_transport(target);
    let Ok(history) = read_thread_history(&transport, thread_id) else {
        return Ok(record.clone());
    };
    let Some(verified_turn) = verify_create_challenge(&history, record) else {
        return Ok(record.clone());
    };
    let mut next = record.clone();
    mark_create_challenge_verified(&mut next, &verified_turn);
    commit_verified_readback(server, next)
}

fn finalize_update_readback(
    server: &Server,
    record: &state::PeerLifecycleOperationRecord,
) -> Result<state::PeerLifecycleOperationRecord, String> {
    let Some(target) = record.target.as_ref() else {
        return Ok(record.clone());
    };
    let Some(thread_id) = target.transport.thread_id.as_deref() else {
        return Ok(record.clone());
    };
    let Some(update) = record.update.as_ref() else {
        return Ok(record.clone());
    };
    if update.challenge.is_none() {
        return Ok(record.clone());
    }
    let transport = selected_transport(target);
    let Ok(history) = read_thread_history(&transport, thread_id) else {
        return Ok(record.clone());
    };
    let Some(verified_turn) = verify_update_challenge(&history, record) else {
        return Ok(record.clone());
    };
    let mut next = record.clone();
    mark_update_challenge_verified(&mut next, thread_id, &verified_turn);
    commit_verified_readback(server, next)
}

fn responsibility_requires(
    snapshot: &state::ResponsibilitySnapshot,
) -> Option<ContextOperationRequires> {
    let mut fields = Vec::new();
    let mut sources = serde_json::Map::new();
    if !snapshot.task_ids.is_empty() {
        fields.push("tasks".into());
        sources.insert("tasks".into(), json!(snapshot.task_ids));
    }
    if !snapshot.scheduler_request_ids.is_empty() {
        fields.push("scheduler_requests".into());
        sources.insert(
            "scheduler_requests".into(),
            json!(snapshot.scheduler_request_ids),
        );
    }
    if !snapshot.worktree_binding_ids.is_empty() {
        fields.push("worktrees".into());
        sources.insert("worktrees".into(), json!(snapshot.worktree_binding_ids));
    }
    // Update preserves the same worker, binding, App Server thread and route.
    // Existing subscriptions remain attached to that identity and do not
    // depend on its execution cwd, so they are not an Update responsibility
    // conflict. Close uses its own lifecycle admission and retirement contract.
    if fields.is_empty() {
        None
    } else {
        Some(ContextOperationRequires {
            kind: Some("peer_responsibility".into()),
            fields,
            sources,
            approval: None,
            repair_invocation: None,
        })
    }
}

fn close_responsibility_requires(
    snapshot: &state::ResponsibilitySnapshot,
) -> Option<ContextOperationRequires> {
    let mut fields = Vec::new();
    let mut sources = serde_json::Map::new();
    if !snapshot.task_ids.is_empty() {
        fields.push("tasks".into());
        sources.insert("tasks".into(), json!(snapshot.task_ids));
    }
    if !snapshot.scheduler_request_ids.is_empty() {
        fields.push("scheduler_requests".into());
        sources.insert(
            "scheduler_requests".into(),
            json!(snapshot.scheduler_request_ids),
        );
    }
    if !snapshot.managed_subagent_ids.is_empty() {
        fields.push("managed_subagents".into());
        sources.insert(
            "managed_subagents".into(),
            json!(snapshot.managed_subagent_ids),
        );
    }
    if !snapshot.worktree_binding_ids.is_empty() {
        fields.push("worktrees".into());
        sources.insert("worktrees".into(), json!(snapshot.worktree_binding_ids));
    }
    if fields.is_empty() {
        None
    } else {
        Some(ContextOperationRequires {
            kind: Some("peer_responsibility".into()),
            fields,
            sources,
            approval: None,
            repair_invocation: None,
        })
    }
}

fn close_authorized(server: &Server, state: &State, actor: &str, target: &str) -> bool {
    actor != target
        && (current_master_holder(server, state)
            .ok()
            .flatten()
            .as_deref()
            == Some(actor)
            || state
                .subagents
                .values()
                .any(|record| record.peer == target && record.parent == actor))
}

fn stage_readback(state: PeerLifecycleStage) -> PeerLifecycleStageReadback {
    PeerLifecycleStageReadback {
        state,
        challenge: None,
    }
}

/// Build the mutable Update receipt while preserving the frozen previous and
/// intended cwd captured at intent commit.
fn update_receipt(
    record: &state::PeerLifecycleOperationRecord,
    settings_state: PeerSettingsState,
    effective_cwd: PeerEffectiveCwd,
    challenge: Option<PeerLifecycleChallenge>,
) -> Option<PeerLifecycleUpdate> {
    let existing = record.update.as_ref();
    Some(PeerLifecycleUpdate {
        previous_cwd: existing
            .map(|update| update.previous_cwd.clone())
            .unwrap_or_default(),
        intended_cwd: existing
            .map(|update| update.intended_cwd.clone())
            .unwrap_or_default(),
        settings: PeerLifecycleSettings {
            state: settings_state,
        },
        effective_cwd,
        challenge,
    })
}

fn close_readback(close_outcome: impl Into<String>) -> PeerLifecycleClose {
    PeerLifecycleClose {
        close_outcome: close_outcome.into(),
        runtime_archive: stage_readback(PeerLifecycleStage::NotAttempted),
        worker_retirement: stage_readback(PeerLifecycleStage::NotAttempted),
        binding_retirement: stage_readback(PeerLifecycleStage::NotAttempted),
        route_retirement: stage_readback(PeerLifecycleStage::NotAttempted),
        lease_retirement: stage_readback(PeerLifecycleStage::NotApplicable),
        subscription_retirement: stage_readback(PeerLifecycleStage::NotAttempted),
    }
}

enum HostTerminal {
    Verified,
    Missing,
    Unsupported(String),
    Refused(String),
    Unknown(String),
}

/// A native error only proves the exact thread is gone when it names that
/// thread as absent. Anything else stays unknown evidence.
fn detail_reports_missing(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("not found")
        || detail.contains("not loaded")
        || detail.contains("unknown thread")
        || detail.contains("no such thread")
}

fn archive_terminal(target: &PeerLifecycleTarget) -> HostTerminal {
    if target.transport.kind != TransportKind::AppServer {
        return HostTerminal::Unsupported(
            "TRANSPORT_UNSUPPORTED: close requires an App Server thread archive".into(),
        );
    }
    let Some(thread_id) = target.transport.thread_id.as_deref() else {
        return HostTerminal::Unsupported(
            "TRANSPORT_UNSUPPORTED: close target has no App Server thread".into(),
        );
    };
    let transport = selected_transport(target);
    match archive_thread(&transport, thread_id) {
        Ok(_) => match read_thread_history(&transport, thread_id) {
            Ok(history) => {
                let thread = history.get("thread");
                let observed = thread
                    .and_then(|value| value.get("id"))
                    .and_then(serde_json::Value::as_str);
                let status = thread
                    .and_then(|value| value.get("status"))
                    .and_then(|value| value.get("type"))
                    .and_then(serde_json::Value::as_str);
                if observed == Some(thread_id)
                    && matches!(status, Some("notLoaded" | "archived" | "closed"))
                {
                    HostTerminal::Verified
                } else {
                    HostTerminal::Unknown(
                        "PEER_LIFECYCLE_ARCHIVE_READBACK_UNPROVEN: archive returned but the exact thread terminal was not proven"
                            .into(),
                    )
                }
            }
            // The archive RPC was accepted, but the endpoint became
            // unreachable before the exact terminal could be read back. That
            // is ambiguous, never a proven terminal.
            Err(AdapterError::RouteUnavailable { detail }) => {
                HostTerminal::Unknown(format!("PEER_LIFECYCLE_ARCHIVE_READBACK_UNKNOWN: {detail}"))
            }
            Err(AdapterError::EndpointUnavailable { .. }) => HostTerminal::Unknown(
                "PEER_LIFECYCLE_ARCHIVE_READBACK_UNKNOWN: selected endpoint unavailable".into(),
            ),
            Err(AdapterError::Unknown { detail, .. }) if detail_reports_missing(&detail) => {
                HostTerminal::Missing
            }
            Err(error) => {
                HostTerminal::Unknown(format!("PEER_LIFECYCLE_ARCHIVE_READBACK_UNKNOWN: {error}"))
            }
        },
        // A connection failure proves nothing about the exact target: the
        // endpoint may be down, or the daemon may simply not have observed the
        // host. Keep it unknown rather than claiming the target is missing.
        Err(AdapterError::RouteUnavailable { detail }) => HostTerminal::Unknown(format!(
            "PEER_LIFECYCLE_ARCHIVE_ROUTE_UNAVAILABLE: {detail}"
        )),
        Err(AdapterError::EndpointUnavailable { .. }) => HostTerminal::Unknown(
            "PEER_LIFECYCLE_ARCHIVE_ENDPOINT_UNAVAILABLE: selected endpoint unavailable".into(),
        ),
        Err(AdapterError::CapabilityUnavailable { .. }) => HostTerminal::Unsupported(
            "TRANSPORT_UNSUPPORTED: selected App Server adapter cannot archive the exact thread"
                .into(),
        ),
        Err(AdapterError::InvalidBinding { detail }) => HostTerminal::Refused(detail),
        // Only an explicit native refusal that names the exact thread as
        // absent is definitive missing evidence.
        Err(AdapterError::Unknown { detail, .. }) if detail_reports_missing(&detail) => {
            HostTerminal::Missing
        }
        Err(error) => HostTerminal::Unknown(format!("PEER_LIFECYCLE_ARCHIVE_UNKNOWN: {error}")),
    }
}

fn close_phase(close: &PeerLifecycleClose) -> PeerLifecyclePhase {
    if close.close_outcome.starts_with("complete") {
        PeerLifecyclePhase::Complete
    } else if close.close_outcome.starts_with("cleanup-open") {
        PeerLifecyclePhase::CleanupOpen
    } else if close.close_outcome.starts_with("refused") {
        PeerLifecyclePhase::Refused
    } else {
        PeerLifecyclePhase::Unknown
    }
}

fn retire_control_plane(
    server: &Server,
    record: &state::PeerLifecycleOperationRecord,
    runtime_archive: PeerLifecycleStage,
    runtime_archive_detail: Option<String>,
    snapshot_captured_ms: Option<i64>,
) -> PeerLifecycleClose {
    let mut close = close_readback("cleanup-open");
    close.runtime_archive.state = runtime_archive.clone();
    let Some(snapshot) = record.responsibility_snapshot.clone() else {
        close.worker_retirement.state = PeerLifecycleStage::Unknown;
        close.binding_retirement.state = PeerLifecycleStage::Unknown;
        close.route_retirement.state = PeerLifecycleStage::Unknown;
        close.subscription_retirement.state = PeerLifecycleStage::Unknown;
        return close;
    };
    let now = now_ms();
    let (events, route_binding) = {
        let state = server.state.lock().unwrap();
        let route_scope = RouteScope {
            app_scope_id: record.exact_target().app_scope_id.clone(),
            project_scope_id: record.exact_target().project_scope.clone(),
        };
        let Some(binding) = state
            .global
            .lookup_binding_for(&route_scope, &record.exact_target().binding_id)
            .filter(|binding| {
                binding.agent_id.as_str() == record.exact_target().worker_id
                    && binding.endpoint_generation == record.exact_target().endpoint_generation
            })
            .cloned()
        else {
            close.worker_retirement.state = PeerLifecycleStage::Failed;
            close.binding_retirement.state = PeerLifecycleStage::Missing;
            close.route_retirement.state = PeerLifecycleStage::Missing;
            close.subscription_retirement.state = PeerLifecycleStage::Failed;
            return close;
        };
        let Some(next_generation) = binding.endpoint_generation.checked_add(1) else {
            // The exact binding generation cannot be advanced, so the control
            // plane cannot be retired. Host terminal stays verified; the
            // operation stays cleanup-open with its fence retained.
            close.worker_retirement.state = PeerLifecycleStage::Failed;
            close.binding_retirement.state = PeerLifecycleStage::Failed;
            close.route_retirement.state = PeerLifecycleStage::Failed;
            close.subscription_retirement.state = PeerLifecycleStage::Failed;
            close.close_outcome = "cleanup-open: PEER_LIFECYCLE_GENERATION_EXHAUSTED".into();
            return close;
        };
        let mut retired = binding.clone();
        retired.endpoint_generation = next_generation;
        retired.native_thread_id = None;
        retired.tmux_endpoint = None;
        let mut events = Vec::new();
        for subscription_id in &snapshot.subscription_ids {
            if state
                .notification_subscriptions
                .get(subscription_id)
                .is_some_and(|subscription| {
                    matches!(subscription.status.as_str(), "armed" | "notifying")
                })
            {
                events.push(state::Event::NotificationSuppressed {
                    subscription_id: subscription_id.clone(),
                    status: "cancelled".into(),
                    reason: format!("peer lifecycle close {}", record.operation_id),
                    updated_ms: now,
                });
            }
        }
        events.push(state::Event::GlobalCurrentThreadRouteRetired {
            binding: binding.clone(),
        });
        events.push(state::Event::GlobalRuntimeBound { binding: retired });
        events.push(state::Event::WorkerClosed {
            worker_id: record.exact_target().worker_id.clone(),
            closed_by: record.actor_id.clone(),
            reason: format!("peer lifecycle close {}", record.operation_id),
            snapshot_captured_ms,
            at_ms: now,
        });
        (events, binding)
    };
    if let Err(error) = server.try_commit(&events) {
        close.worker_retirement.state = PeerLifecycleStage::Unknown;
        close.binding_retirement.state = PeerLifecycleStage::Unknown;
        close.route_retirement.state = PeerLifecycleStage::Unknown;
        close.subscription_retirement.state = PeerLifecycleStage::Unknown;
        close.close_outcome = format!("cleanup-open: {error}");
        return close;
    }
    let state = server.state.lock().unwrap();
    close.worker_retirement.state = if state
        .worker_closures
        .contains_key(&record.exact_target().worker_id)
        && !state.workers.contains_key(&record.exact_target().worker_id)
    {
        PeerLifecycleStage::Verified
    } else {
        PeerLifecycleStage::Failed
    };
    let route_scope = RouteScope {
        app_scope_id: record.exact_target().app_scope_id.clone(),
        project_scope_id: record.exact_target().project_scope.clone(),
    };
    close.binding_retirement.state = match state
        .global
        .lookup_binding_for(&route_scope, &record.exact_target().binding_id)
    {
        Some(binding)
            if binding.agent_id.as_str() == record.exact_target().worker_id
                && binding.endpoint_generation
                    == record.exact_target().endpoint_generation.saturating_add(1)
                && binding.native_thread_id.is_none()
                && binding.tmux_endpoint.is_none() =>
        {
            PeerLifecycleStage::Verified
        }
        Some(_) => PeerLifecycleStage::Failed,
        None => PeerLifecycleStage::Missing,
    };
    close.route_retirement.state = match (
        route_binding.session_id.as_ref(),
        route_binding.native_thread_id.as_ref(),
    ) {
        (Some(session_id), Some(thread_id)) => {
            if state
                .global
                .lookup_current_thread_route(session_id, thread_id)
                .is_none()
            {
                PeerLifecycleStage::Verified
            } else {
                PeerLifecycleStage::Failed
            }
        }
        _ => PeerLifecycleStage::NotApplicable,
    };
    close.subscription_retirement.state = if snapshot.subscription_ids.is_empty() {
        PeerLifecycleStage::NotApplicable
    } else if snapshot.subscription_ids.iter().all(|subscription_id| {
        state
            .notification_subscriptions
            .get(subscription_id)
            .is_none_or(|subscription| {
                !matches!(subscription.status.as_str(), "armed" | "notifying")
            })
    }) {
        PeerLifecycleStage::Verified
    } else {
        PeerLifecycleStage::Failed
    };
    let control_complete = matches!(close.worker_retirement.state, PeerLifecycleStage::Verified)
        && matches!(close.binding_retirement.state, PeerLifecycleStage::Verified)
        && matches!(
            close.route_retirement.state,
            PeerLifecycleStage::Verified | PeerLifecycleStage::NotApplicable
        )
        && matches!(
            close.subscription_retirement.state,
            PeerLifecycleStage::Verified | PeerLifecycleStage::NotApplicable
        );
    let host_complete = matches!(
        close.runtime_archive.state,
        PeerLifecycleStage::Verified | PeerLifecycleStage::Missing
    );
    close.close_outcome = if control_complete && host_complete {
        "complete".into()
    } else {
        "cleanup-open".into()
    };
    if let Some(detail) = runtime_archive_detail {
        close.close_outcome = format!("{}: {detail}", close.close_outcome);
    }
    close
}

include!("peer_lifecycle/handlers.rs");
include!("peer_lifecycle/create_query.rs");
