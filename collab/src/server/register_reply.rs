//! Durable evidence for explicit communication restoration; never registers identities.
use super::{PreparedRegisterEnvelope, RegisterOuterBinding};
use crate::proto::{IdentityContextRequest, ProjectContext, SelectedTransport, TransportKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

const LEDGER_FILE: &str = "register-reply-evidence.jsonl";

pub(crate) use crate::proto::RegisterReplyPurpose as Purpose;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Continuation {
    pub context: ProjectContext,
    pub request: IdentityContextRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Record {
    pub purpose: Purpose,
    pub operation_id: String,
    pub worker_id: String,
    pub transport: SelectedTransport,
    pub prepared: PreparedRegisterEnvelope,
    pub continuation: Option<Continuation>,
    marker: String,
    dispatched_ms: i64,
    /// Sync before I/O; a lost dispatch response never causes a resend.
    dispatch_started: bool,
    dispatch_receipt: Option<Value>,
    verified: Option<Value>,
}

pub(crate) use crate::proto::RegisterParticipantReply as ParticipantReply;

pub(crate) fn read(root: &Path, operation_id: &str) -> Result<Option<Record>, String> {
    Ok(records(root)?
        .into_iter()
        .rev()
        .find(|record| record.operation_id == operation_id))
}

fn records(root: &Path) -> Result<Vec<Record>, String> {
    let bytes = match std::fs::read(root.join(LEDGER_FILE)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("REGISTER_REPLY_LEDGER_READ_FAILED: {error}")),
    };
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err("REGISTER_REPLY_LEDGER_CORRUPT: incomplete durable record".into());
    }
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let value: Value = serde_json::from_slice(line)
                .map_err(|error| format!("REGISTER_REPLY_LEDGER_CORRUPT: {error}"))?;
            // Retired initial records are history only. They can neither
            // resume Register nor block explicit communication restoration.
            if matches!(
                value["purpose"].as_str(),
                Some("initial_register" | "desktop_registration")
            ) {
                return Ok(None);
            }
            serde_json::from_value(value)
                .map(Some)
                .map_err(|error| format!("REGISTER_REPLY_LEDGER_CORRUPT: {error}"))
        })
        .filter_map(Result::transpose)
        .collect()
}

fn append(root: &Path, record: &Record) -> Result<(), String> {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(root)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join(LEDGER_FILE))?;
        let mut line = serde_json::to_vec(record)?;
        line.push(b'\n');
        file.write_all(&line)?;
        file.sync_all()?;
        std::fs::File::open(root)?.sync_all()
    };
    write().map_err(|error| format!("REGISTER_REPLY_LEDGER_WRITE_FAILED: {error}"))
}

fn fresh_marker() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn ensure_pending(
    root: &Path,
    worker: &str,
    transport: &SelectedTransport,
    prepared: &PreparedRegisterEnvelope,
    outer: Option<&RegisterOuterBinding>,
    purpose: Purpose,
) -> Result<Record, String> {
    let operation_id = outer
        .map(|binding| binding.operation_id.as_str())
        .unwrap_or(&prepared.nested_operation_id);
    if let Some(record) = read(root, operation_id)? {
        if record.purpose != purpose
            || record.worker_id != worker
            || serde_json::to_value(&record.transport).unwrap()
                != serde_json::to_value(transport).unwrap()
            || record.prepared.typed != prepared.typed
        {
            return Err("REGISTER_REPLY_PENDING_MISMATCH: exact prepared intent changed".into());
        }
        return Ok(record);
    }
    let record = Record {
        purpose,
        operation_id: operation_id.to_owned(),
        worker_id: worker.to_owned(),
        transport: transport.clone(),
        prepared: prepared.clone(),
        continuation: outer.and_then(|binding| binding.continuation.clone()),
        marker: fresh_marker(),
        dispatched_ms: super::now_ms(),
        dispatch_started: false,
        dispatch_receipt: None,
        verified: None,
    };
    append(root, &record)?;
    Ok(record)
}

pub(crate) fn enforce(root: &Path, mut record: Record) -> Result<(), String> {
    if record.verified.is_some() {
        return Ok(());
    }
    if !record.dispatch_started {
        record.dispatch_started = true;
        append(root, &record)?;
        let prompt = format!(
            "COLLAB_REGISTER_CHALLENGE {}",
            json!({
                "operation_id":record.operation_id,"command_id":record.prepared.nested_command_id,
                "purpose":record.purpose,"challenge":record.marker
            })
        );
        let receipt = match record.transport.kind {
            TransportKind::Tmux => crate::client::adapters::tmux::notify(
                record
                    .transport
                    .tmux_endpoint
                    .as_ref()
                    .ok_or("REGISTER_REPLY_TRANSPORT_INCOMPLETE")?,
                &record.operation_id,
                &prompt,
            )?,
            TransportKind::AppServer => {
                crate::client::adapters::codex_app_server::immediate_notify(
                    &record.transport,
                    None,
                    &prompt,
                    &record.prepared.nested_operation_id,
                )
                .map_err(|error| format!("REGISTER_REPLY_DISPATCH_FAILED: {error}"))?
            }
            TransportKind::Dsh => {
                let transport = &record.transport;
                crate::client::adapters::dsh::notify_with_message_id(
                    transport
                        .endpoint
                        .as_deref()
                        .ok_or("REGISTER_REPLY_TRANSPORT_INCOMPLETE")?,
                    transport
                        .namespace
                        .as_deref()
                        .ok_or("REGISTER_REPLY_TRANSPORT_INCOMPLETE")?,
                    transport
                        .thread_id
                        .as_deref()
                        .ok_or("REGISTER_REPLY_TRANSPORT_INCOMPLETE")?,
                    "followup",
                    &record.worker_id,
                    &prompt,
                    &crate::client::adapters::dsh::registration_message_id(
                        &record.prepared.nested_operation_id,
                    )
                    .map_err(|error| error.to_string())?,
                )
                .map_err(|error| format!("REGISTER_REPLY_DISPATCH_FAILED: {error}"))?
            }
        };
        record.dispatch_receipt = Some(receipt);
        append(root, &record)?;
    }
    // Return to release tmux's initiating pane. Only a typed return verifies it.
    let evidence = match record.transport.kind {
        TransportKind::Tmux => {
            return Err("REGISTER_REPLY_PENDING: participant return required".into())
        }
        TransportKind::AppServer => match observe_appserver(&record) {
            Err(error) if error == "REGISTER_REPLY_NOT_OBSERVED" => {
                return Err("REGISTER_REPLY_PENDING: native participant return required".into())
            }
            result => result?,
        },
        TransportKind::Dsh => match observe_dsh(&record) {
            Err(error) if error == "REGISTER_REPLY_DSH_PENDING" => {
                return Err("REGISTER_REPLY_PENDING: native participant return required".into())
            }
            result => result?,
        },
    };
    record.verified = Some(evidence);
    append(root, &record)
}

fn observe_appserver(record: &Record) -> Result<Value, String> {
    let transport = &record.transport;
    let history = crate::client::adapters::codex_app_server::read_thread_history(
        transport,
        transport
            .thread_id
            .as_deref()
            .ok_or("REGISTER_REPLY_TRANSPORT_INCOMPLETE")?,
    )
    .map_err(|error| format!("REGISTER_REPLY_READ_FAILED: {error}"))?;
    let thread = history.get("thread").ok_or("REGISTER_REPLY_NOT_OBSERVED")?;
    if thread["id"].as_str() != transport.thread_id.as_deref()
        || thread["sessionId"].as_str() != transport.session_id.as_deref()
    {
        return Err("REGISTER_REPLY_TARGET_MISMATCH".into());
    }
    for turn in thread["turns"]
        .as_array()
        .ok_or("REGISTER_REPLY_NOT_OBSERVED")?
    {
        let Some(id) = turn["id"].as_str() else {
            continue;
        };
        if record
            .dispatch_receipt
            .as_ref()
            .and_then(|receipt| receipt.pointer("/turn/id"))
            .and_then(Value::as_str)
            .is_some_and(|expected| expected != id)
        {
            continue;
        }
        let items = turn["items"].as_array();
        let correlated = items.is_some_and(|items| {
            items.iter().any(|item| {
                item["type"] == "userMessage"
                    && (item["clientMessageId"] == record.prepared.nested_operation_id
                        || item["id"] == record.prepared.nested_operation_id)
            })
        });
        let replied = items.is_some_and(|items| {
            items.iter().any(|item| {
                item["type"] == "agentMessage"
                    && item["text"].as_str() == Some(record.marker.as_str())
            })
        });
        if correlated
            && replied
            && turn["status"] == "completed"
            && turn["completedAt"]
                .as_i64()
                .is_some_and(|at| at >= record.dispatched_ms.div_euclid(1000))
        {
            return Ok(json!({"turn_id":id}));
        }
    }
    Err("REGISTER_REPLY_NOT_OBSERVED".into())
}

fn observe_dsh(record: &Record) -> Result<Value, String> {
    use crate::client::adapters::dsh;
    let transport = &record.transport;
    let message = dsh::registration_message_id(&record.prepared.nested_operation_id)
        .map_err(|error| error.to_string())?;
    let reply = dsh::agent_reply(
        transport
            .endpoint
            .as_deref()
            .ok_or("REGISTER_REPLY_TRANSPORT_INCOMPLETE")?,
        transport
            .namespace
            .as_deref()
            .ok_or("REGISTER_REPLY_TRANSPORT_INCOMPLETE")?,
        transport
            .thread_id
            .as_deref()
            .ok_or("REGISTER_REPLY_TRANSPORT_INCOMPLETE")?,
        &message,
    )
    .map_err(|error| format!("REGISTER_REPLY_DSH_READ_FAILED: {error}"))?;
    match reply {
        dsh::AgentReply::Replied {
            agent_id,
            message_id,
            session_id,
            turn,
            step,
            input_seq,
            output_seq,
            completed_seq,
            text,
        } if Some(agent_id.as_str()) == transport.thread_id.as_deref()
            && message_id == message
            && Some(session_id.as_str()) == transport.session_id.as_deref()
            && text.trim() == record.marker
            && input_seq < output_seq
            && output_seq < completed_seq =>
        {
            Ok(
                json!({"turn":turn,"step":step,"input_seq":input_seq,"output_seq":output_seq,"completed_seq":completed_seq}),
            )
        }
        dsh::AgentReply::Pending => Err("REGISTER_REPLY_DSH_PENDING".into()),
        _ => Err("REGISTER_REPLY_DSH_MISMATCH".into()),
    }
}

pub(crate) fn accept(
    root: &Path,
    context: &ProjectContext,
    reply: &ParticipantReply,
) -> Result<Record, String> {
    let mut record = read(root, &reply.operation_id)?.ok_or("REGISTER_REPLY_PENDING_MISSING")?;
    let envelope = &record.prepared.typed.envelope;
    if envelope.scope.project_scope_id != context.project_scope
        || envelope.scope.app_scope_id != context.app_scope_id
    {
        return Err("REGISTER_REPLY_SCOPE_MISMATCH".into());
    }
    if !record.dispatch_started
        || reply.purpose != record.purpose
        || reply.command_id != record.prepared.nested_command_id
        || reply.challenge != record.marker
    {
        return Err("REGISTER_REPLY_CORRELATION_MISMATCH".into());
    }
    match (&record.transport.kind, &reply.anchor, &reply.native) {
        (TransportKind::Tmux, Some(anchor), None) => {
            let endpoint = record
                .transport
                .tmux_endpoint
                .as_ref()
                .ok_or("REGISTER_REPLY_TRANSPORT_INCOMPLETE")?;
            if endpoint.socket_path != anchor.socket_path || endpoint.pane_id != anchor.pane_id {
                return Err("REGISTER_REPLY_TARGET_MISMATCH".into());
            }
            match crate::client::adapters::tmux::probe(endpoint)? {
                crate::client::adapters::tmux::PanePresence::Present => {}
                _ => return Err("REGISTER_REPLY_PANE_NOT_PRESENT".into()),
            }
        }
        (
            TransportKind::AppServer,
            None,
            Some(crate::proto::RegisterNativeReply::AppServer { candidate }),
        ) => {
            let transport = &record.transport;
            if transport.endpoint.as_deref() != Some(candidate.endpoint.as_str())
                || transport.namespace.as_deref() != Some(candidate.namespace.as_str())
                || transport.session_id.as_deref() != Some(candidate.session_id.as_str())
                || transport.thread_id.as_deref() != Some(candidate.thread_id.as_str())
                || Path::new(&candidate.cwd)
                    .canonicalize()
                    .map_err(|error| error.to_string())?
                    != Path::new(&context.canonical_root)
                        .canonicalize()
                        .map_err(|error| error.to_string())?
            {
                return Err("REGISTER_REPLY_TARGET_MISMATCH".into());
            }
            crate::client::adapters::verify_candidate(candidate)
                .map_err(|error| error.to_string())?;
        }
        (TransportKind::Dsh, None, Some(crate::proto::RegisterNativeReply::Dsh { candidate })) => {
            let transport = &record.transport;
            if transport.endpoint.as_deref() != Some(candidate.endpoint.as_str())
                || transport.namespace.as_deref() != Some(candidate.runtime_id.as_str())
                || transport.session_id.as_deref() != Some(candidate.session_id.as_str())
                || transport.thread_id.as_deref() != Some(candidate.agent_id.as_str())
            {
                return Err("REGISTER_REPLY_TARGET_MISMATCH".into());
            }
            super::admit_dsh_candidate(
                candidate,
                &Path::new(&context.canonical_root)
                    .canonicalize()
                    .map_err(|error| error.to_string())?,
            )?;
        }
        _ => return Err("REGISTER_REPLY_TARGET_MISMATCH".into()),
    }
    let evidence = serde_json::to_value(reply).unwrap();
    if let Some(existing) = record.verified.as_ref() {
        if *existing != evidence {
            return Err("REGISTER_REPLY_ALREADY_VERIFIED".into());
        }
        return Ok(record);
    }
    record.verified = Some(evidence);
    append(root, &record)?;
    Ok(record)
}

pub(crate) fn verified_for_commit(root: &Path, record: &Record) -> Result<(), String> {
    let saved = read(root, &record.operation_id)?.ok_or("REGISTER_REPLY_PENDING_MISSING")?;
    if saved.prepared.typed != record.prepared.typed
        || saved.purpose != record.purpose
        || saved.worker_id != record.worker_id
        || saved.marker != record.marker
        || serde_json::to_value(&saved.transport).unwrap()
            != serde_json::to_value(&record.transport).unwrap()
        || saved.verified.is_none()
    {
        return Err("REGISTER_REPLY_NOT_VERIFIED".into());
    }
    Ok(())
}
