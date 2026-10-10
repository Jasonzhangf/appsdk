use crate::proto::{
    IdentityContextCancelAck, IdentityContextRequest, IdentityContextResponseEnvelope,
    IdentityOperationPhase, IdentityOperationProjection,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub(crate) const OPERATION_JOURNAL_SCHEMA_VERSION: u8 = 1;
const OPERATION_RECORD_TYPE: &str = "identity_operation";

const ERR_OPERATION_APPEND: &str = "IDENTITY_OPERATION_DURABILITY_FAILED";
const ERR_OPERATION_CONFLICT: &str = "IDENTITY_OPERATION_INTENT_CONFLICT";
const ERR_OPERATION_PHASE_CONFLICT: &str = "IDENTITY_OPERATION_PHASE_CONFLICT";
const ERR_OPERATION_DENIED: &str = "IDENTITY_OPERATION_QUERY_DENIED";
const ERR_OPERATION_UNKNOWN: &str = "IDENTITY_OPERATION_UNKNOWN";
const ERR_OPERATION_NOT_ACTIVE: &str = "IDENTITY_OPERATION_NOT_ACTIVE";

#[cfg(any(test, feature = "context-cancel-test-hooks"))]
thread_local! {
    static NEXT_APPEND_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(any(test, feature = "context-cancel-test-hooks"))]
pub(crate) fn inject_next_append_fault() {
    NEXT_APPEND_FAULT.with(|fault| fault.set(true));
}

#[cfg(any(test, feature = "context-cancel-test-hooks"))]
fn take_append_fault() -> bool {
    NEXT_APPEND_FAULT.with(|fault| fault.replace(false))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperationJournalRecord {
    pub schema_version: u8,
    pub record_type: String,
    pub sequence: u64,
    pub operation_id: String,
    pub project_scope: String,
    pub app_scope_id: String,
    pub action: String,
    pub invocation: String,
    pub intent_digest: String,
    pub query_capability_hash: String,
    pub phase: IdentityOperationPhase,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub committed_phases: Vec<IdentityOperationPhase>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub business_receipts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nested_command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nested_operation_id: Option<String>,
    /// Immutable approval audit evidence. It is not a business receipt and is
    /// retained across replay even when cancellation wins before owner start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_evidence: Option<serde_json::Value>,
    pub recorded_at_ms: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct OperationAdmission {
    pub operation_id: String,
    pub project_scope: String,
    pub app_scope_id: String,
    pub action: String,
    pub invocation: String,
    pub intent_digest: String,
    pub query_capability_hash: String,
    pub phase: IdentityOperationPhase,
    pub committed_phases: Vec<IdentityOperationPhase>,
    pub nested_command_id: Option<String>,
    pub nested_operation_id: Option<String>,
    pub approval_evidence: Option<serde_json::Value>,
}

impl OperationAdmission {
    fn to_record(self, sequence: u64, recorded_at_ms: i64) -> OperationJournalRecord {
        OperationJournalRecord {
            schema_version: OPERATION_JOURNAL_SCHEMA_VERSION,
            record_type: OPERATION_RECORD_TYPE.into(),
            sequence,
            operation_id: self.operation_id,
            project_scope: self.project_scope,
            app_scope_id: self.app_scope_id,
            action: self.action,
            invocation: self.invocation,
            intent_digest: self.intent_digest,
            query_capability_hash: self.query_capability_hash,
            phase: self.phase,
            committed_phases: self.committed_phases,
            business_receipts: Vec::new(),
            nested_command_id: self.nested_command_id,
            nested_operation_id: self.nested_operation_id,
            approval_evidence: self.approval_evidence,
            recorded_at_ms,
        }
    }
}

/// Memory-only reservation created before mutation bytes are sent. It binds
/// one exact operation/scope/action/digest/capability tuple to a single-use
/// daemon-incarnation ticket.
#[derive(Debug, Clone)]
struct PendingInvocation {
    project_scope: String,
    app_scope_id: String,
    action: String,
    invocation: String,
    intent_digest: String,
    capability_hash: String,
    ticket: String,
    cancelled: bool,
    consumed: bool,
}

/// Live arbitration state for one executing context operation. It is never
/// serialized; restart reconstructs no live entry from journal contents.
#[derive(Debug, Clone)]
enum LiveBoundary {
    Safe { business_receipts: Vec<String> },
    OwnerActive { owner_phase: String },
    Cancelled,
}

/// Result of one cancellation request against the current daemon incarnation.
#[derive(Debug, Clone)]
pub(crate) enum CancelOutcome {
    /// Cancellation won before admission; no durable record exists.
    NotAdmitted,
    /// Cancellation won at a safe boundary; the durable cancelled projection.
    Cancelled(IdentityOperationProjection),
    /// The owner already started; cancellation is refused for this invocation.
    OwnerStarted(IdentityOperationProjection),
    /// The operation is already terminal or has no live execution entry.
    NotActive(IdentityOperationProjection),
}

#[derive(Debug)]
pub(crate) struct OperationJournal {
    path: PathBuf,
    file: Mutex<File>,
    state: Mutex<OperationJournalState>,
}

#[derive(Debug, Clone)]
struct OperationJournalState {
    next_sequence: u64,
    operations: BTreeMap<String, OperationJournalRecord>,
    pending: BTreeMap<String, PendingInvocation>,
    live: BTreeMap<String, LiveBoundary>,
    incarnation: String,
    next_ticket: u64,
    append_poisoned: Option<String>,
    valid_prefix_bytes: u64,
    incomplete_tail: bool,
}

impl OperationJournal {
    pub(crate) fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)?;
        let replay = replay_operation_journal(&mut file)?;
        Ok(Self {
            path,
            file: Mutex::new(file),
            state: Mutex::new(replay),
        })
    }

    /// Reserve one context invocation before mutation bytes are sent. The
    /// returned ticket is valid for exactly this daemon incarnation.
    pub(crate) fn prepare_invocation(
        &self,
        operation_id: &str,
        project_scope: &str,
        app_scope_id: &str,
        action: &str,
        invocation: &str,
        intent_digest: &str,
        capability_hash: &str,
    ) -> Result<String, String> {
        let mut state = self.state.lock().unwrap();
        if let Some(existing) = state.operations.get(operation_id) {
            if existing.project_scope == project_scope
                && existing.app_scope_id == app_scope_id
                && existing.action == action
                && existing.query_capability_hash == capability_hash
            {
                // A durable operation owns this key already. Let the following
                // identity request return its retained projection or typed
                // intent conflict; do not reject a changed intent in the
                // preparatory reservation call before the public result path.
                return Ok(String::new());
            }
            return Err(format!(
                "{ERR_OPERATION_DENIED}: operation {operation_id} is unavailable for this scope or capability"
            ));
        }
        if let Some(existing) = state.pending.get(operation_id) {
            if existing.project_scope == project_scope
                && existing.app_scope_id == app_scope_id
                && existing.action == action
                && existing.invocation == invocation
                && existing.intent_digest == intent_digest
                && existing.capability_hash == capability_hash
                && !existing.cancelled
            {
                return Ok(existing.ticket.clone());
            }
            return Err(format!(
                "{ERR_OPERATION_CONFLICT}: operation {operation_id} already has a different reservation"
            ));
        }
        state.next_ticket = state.next_ticket.saturating_add(1);
        let ticket = format!("{}:{}", state.incarnation, state.next_ticket);
        state.pending.insert(
            operation_id.to_owned(),
            PendingInvocation {
                project_scope: project_scope.to_owned(),
                app_scope_id: app_scope_id.to_owned(),
                action: action.to_owned(),
                invocation: invocation.to_owned(),
                intent_digest: intent_digest.to_owned(),
                capability_hash: capability_hash.to_owned(),
                ticket: ticket.clone(),
                cancelled: false,
                consumed: false,
            },
        );
        Ok(ticket)
    }

    /// Validate and consume the reservation for one mutation. A missing
    /// reservation is accepted only when the caller carries no ticket, which
    /// preserves the legacy path for unaffected callers.
    pub(crate) fn claim_invocation(
        &self,
        operation_id: &str,
        ticket: &str,
        intent_digest: &str,
        capability_hash: &str,
    ) -> Result<(), String> {
        if ticket.is_empty() {
            return Ok(());
        }
        let mut state = self.state.lock().unwrap();
        let Some(pending) = state.pending.get_mut(operation_id) else {
            return Err(format!(
                "{ERR_OPERATION_DENIED}: reservation for operation {operation_id} is not live in this daemon"
            ));
        };
        if pending.ticket != ticket {
            return Err(format!(
                "{ERR_OPERATION_DENIED}: reservation ticket for operation {operation_id} is invalid"
            ));
        }
        if pending.cancelled {
            return Err("IDENTITY_OPERATION_CANCELLED".into());
        }
        if pending.consumed {
            return Err(format!(
                "{ERR_OPERATION_CONFLICT}: reservation for operation {operation_id} was already consumed"
            ));
        }
        if pending.intent_digest != intent_digest || pending.capability_hash != capability_hash {
            return Err(format!(
                "{ERR_OPERATION_CONFLICT}: reservation intent changed for operation {operation_id}"
            ));
        }
        pending.consumed = true;
        Ok(())
    }

    /// Release the one-shot reservation when identity preflight proves that
    /// required facts are missing. This is the retryable pre-admission result:
    /// no operation record or owner effect exists, so a later supplement may
    /// reserve the same operation key with its completed facts.
    pub(crate) fn release_missing_facts_invocation(
        &self,
        operation_id: &str,
        ticket: &str,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        let Some(pending) = state.pending.get(operation_id) else {
            return Err(format!(
                "IDENTITY_OPERATION_RESERVATION_RELEASE_FAILED: no reservation for {operation_id}"
            ));
        };
        if pending.ticket != ticket
            || !pending.consumed
            || pending.cancelled
            || state.operations.contains_key(operation_id)
        {
            return Err(format!(
                "IDENTITY_OPERATION_RESERVATION_RELEASE_FAILED: reservation for {operation_id} is not an uncommitted missing-facts invocation"
            ));
        }
        state.pending.remove(operation_id);
        Ok(())
    }

    pub(crate) fn install_safe(&self, operation_id: &str, business_receipts: Vec<String>) {
        let mut state = self.state.lock().unwrap();
        state.live.insert(
            operation_id.to_owned(),
            LiveBoundary::Safe { business_receipts },
        );
    }

    pub(crate) fn mark_safe(&self, operation_id: &str, business_receipts: Vec<String>) {
        let mut state = self.state.lock().unwrap();
        if matches!(
            state.live.get(operation_id),
            Some(LiveBoundary::Safe { .. })
        ) {
            state.live.insert(
                operation_id.to_owned(),
                LiveBoundary::Safe { business_receipts },
            );
        }
    }

    pub(crate) fn remove_live(&self, operation_id: &str) {
        self.state.lock().unwrap().live.remove(operation_id);
    }

    /// Re-open the safe boundary after one owner finished its proven work and
    /// before the next owner starts. A cancellation that already won stays
    /// absorbing; an unknown/absent entry is left untouched.
    pub(crate) fn reopen_safe(&self, operation_id: &str, business_receipts: Vec<String>) {
        let mut state = self.state.lock().unwrap();
        match state.live.get(operation_id) {
            Some(LiveBoundary::Cancelled) | None => {}
            Some(_) => {
                state.live.insert(
                    operation_id.to_owned(),
                    LiveBoundary::Safe { business_receipts },
                );
            }
        }
    }

    /// True only when cancellation already won at a safe boundary for this
    /// invocation. The caller returns the durable cancelled projection and
    /// never starts the next owner.
    pub(crate) fn is_cancelled(&self, operation_id: &str) -> bool {
        matches!(
            self.state.lock().unwrap().live.get(operation_id),
            Some(LiveBoundary::Cancelled)
        )
    }

    pub(crate) fn live_receipts(&self, operation_id: &str) -> Option<Vec<String>> {
        let state = self.state.lock().unwrap();
        match state.live.get(operation_id) {
            Some(LiveBoundary::Safe { business_receipts }) => Some(business_receipts.clone()),
            _ => None,
        }
    }

    /// Arbitration point shared by cancellation and every owner-start
    /// decision. A `Safe` boundary becomes `OwnerActive`; a cancelled entry
    /// refuses the owner and a repeated owner-start refuses cancellation.
    pub(crate) fn begin_owner_start(
        &self,
        operation_id: &str,
        owner_phase: &str,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        match state.live.get(operation_id) {
            Some(LiveBoundary::Safe { .. }) => {
                state.live.insert(
                    operation_id.to_owned(),
                    LiveBoundary::OwnerActive {
                        owner_phase: owner_phase.to_owned(),
                    },
                );
                Ok(())
            }
            Some(LiveBoundary::Cancelled) => Err("IDENTITY_OPERATION_CANCELLED".into()),
            Some(LiveBoundary::OwnerActive { .. }) => {
                Err("IDENTITY_OPERATION_OWNER_STARTED".into())
            }
            None => Err(format!(
                "{ERR_OPERATION_NOT_ACTIVE}: operation {operation_id} has no live execution entry"
            )),
        }
    }

    /// One cancellation decision against the live arbitration state and the
    /// durable operation journal. Cancellation never takes `identity_gate`.
    pub(crate) fn cancel(
        &self,
        operation_id: &str,
        project_scope: &str,
        app_scope_id: &str,
        capability_hash: &str,
    ) -> Result<CancelOutcome, String> {
        let mut state = self.state.lock().unwrap();
        if let Some(pending) = state.pending.get_mut(operation_id) {
            if pending.project_scope != project_scope
                || pending.app_scope_id != app_scope_id
                || pending.capability_hash != capability_hash
            {
                return Err(format!(
                    "{ERR_OPERATION_DENIED}: reservation does not belong to this project/app scope or capability"
                ));
            }
            if !pending.consumed {
                pending.cancelled = true;
                return Ok(CancelOutcome::NotAdmitted);
            }
        }
        let Some(existing) = state.operations.get(operation_id).cloned() else {
            return Err(format!(
                "{ERR_OPERATION_UNKNOWN}: no durable operation {operation_id} exists"
            ));
        };
        if existing.project_scope != project_scope || existing.app_scope_id != app_scope_id {
            return Err(format!(
                "{ERR_OPERATION_DENIED}: operation {operation_id} does not belong to this project/app scope"
            ));
        }
        if existing.query_capability_hash != capability_hash {
            return Err(format!(
                "{ERR_OPERATION_DENIED}: cancellation capability does not match operation {operation_id}"
            ));
        }
        match state.live.get(operation_id).cloned() {
            Some(LiveBoundary::Safe { business_receipts }) => {
                if !valid_transition(existing.phase, IdentityOperationPhase::Cancelled) {
                    state
                        .live
                        .insert(operation_id.to_owned(), LiveBoundary::Cancelled);
                    return Ok(CancelOutcome::NotActive(projection(&existing)));
                }
                let mut committed_phases = existing.committed_phases.clone();
                committed_phases.push(IdentityOperationPhase::Cancelled);
                let record = OperationJournalRecord {
                    sequence: state.next_sequence,
                    phase: IdentityOperationPhase::Cancelled,
                    committed_phases,
                    business_receipts,
                    recorded_at_ms: crate::server::state::now_ms(),
                    ..existing
                };
                let appended = append_record(&mut state, &self.file, record)?;
                state
                    .live
                    .insert(operation_id.to_owned(), LiveBoundary::Cancelled);
                Ok(CancelOutcome::Cancelled(appended))
            }
            Some(LiveBoundary::OwnerActive { .. }) | Some(LiveBoundary::Cancelled) => {
                Ok(CancelOutcome::OwnerStarted(projection(&existing)))
            }
            None => Ok(CancelOutcome::NotActive(projection(&existing))),
        }
    }

    pub(crate) fn cancel_ack(outcome: CancelOutcome) -> IdentityContextCancelAck {
        match outcome {
            CancelOutcome::NotAdmitted => IdentityContextCancelAck {
                operation_id: String::new(),
                disposition: "not_admitted_cancelled".into(),
                projection: None,
            },
            CancelOutcome::Cancelled(projection) => IdentityContextCancelAck {
                operation_id: projection.operation_id.clone(),
                disposition: "cancelled".into(),
                projection: Some(projection),
            },
            CancelOutcome::OwnerStarted(projection) => IdentityContextCancelAck {
                operation_id: projection.operation_id.clone(),
                disposition: "owner_started".into(),
                projection: Some(projection),
            },
            CancelOutcome::NotActive(projection) => IdentityContextCancelAck {
                operation_id: projection.operation_id.clone(),
                disposition: "not_active".into(),
                projection: Some(projection),
            },
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn is_append_poisoned(&self) -> bool {
        self.state.lock().unwrap().append_poisoned.is_some()
    }

    pub(crate) fn append(
        &self,
        admission: OperationAdmission,
    ) -> Result<IdentityOperationProjection, String> {
        let mut state = self.state.lock().unwrap();
        if let Some(error) = &state.append_poisoned {
            return Err(format!("{ERR_OPERATION_APPEND}: {error}"));
        }
        if let Some(existing) = state.operations.get(&admission.operation_id) {
            if existing.project_scope == admission.project_scope
                && existing.app_scope_id == admission.app_scope_id
                && existing.action == admission.action
                && existing.invocation == admission.invocation
                && existing.intent_digest == admission.intent_digest
                && existing.query_capability_hash == admission.query_capability_hash
            {
                return Ok(projection(existing));
            }
            return Err(format!(
                "{ERR_OPERATION_CONFLICT}: operation {} is already bound to a different normalized intent",
                admission.operation_id
            ));
        }
        if admission.phase != IdentityOperationPhase::Admitted {
            return Err(format!(
                "{ERR_OPERATION_PHASE_CONFLICT}: first durable operation phase must be admitted"
            ));
        }
        if !admission.committed_phases.is_empty() {
            return Err(format!(
                "{ERR_OPERATION_PHASE_CONFLICT}: admission cannot contain committed business phases"
            ));
        }
        let sequence = state.next_sequence;
        let mut admission = admission;
        admission.committed_phases.clear();
        let record = admission.to_record(sequence, crate::server::state::now_ms());
        append_record(&mut state, &self.file, record)
    }

    /// Append one explicit phase transition for an already admitted operation.
    /// Unresolved terminal phases remain queryable and cannot be advanced here.
    pub(crate) fn transition(
        &self,
        operation_id: &str,
        phase: IdentityOperationPhase,
        nested_command_id: Option<String>,
        nested_operation_id: Option<String>,
    ) -> Result<IdentityOperationProjection, String> {
        self.transition_with_business_receipts(
            operation_id,
            phase,
            nested_command_id,
            nested_operation_id,
            None,
        )
    }

    pub(crate) fn transition_with_business_receipts(
        &self,
        operation_id: &str,
        phase: IdentityOperationPhase,
        nested_command_id: Option<String>,
        nested_operation_id: Option<String>,
        business_receipts: Option<Vec<String>>,
    ) -> Result<IdentityOperationProjection, String> {
        let mut state = self.state.lock().unwrap();
        if let Some(error) = &state.append_poisoned {
            return Err(format!("{ERR_OPERATION_APPEND}: {error}"));
        }
        let Some(existing) = state.operations.get(operation_id).cloned() else {
            return Err(format!(
                "{ERR_OPERATION_UNKNOWN}: operation {operation_id} is not admitted"
            ));
        };
        if !valid_transition(existing.phase, phase) {
            return Err(format!(
                "{ERR_OPERATION_PHASE_CONFLICT}: illegal phase transition {} -> {} for operation {operation_id}",
                existing.phase.as_str(), phase.as_str()
            ));
        }
        if nested_command_id.as_ref().is_some_and(|id| {
            existing
                .nested_command_id
                .as_ref()
                .is_some_and(|old| old != id)
        }) || nested_operation_id.as_ref().is_some_and(|id| {
            existing
                .nested_operation_id
                .as_ref()
                .is_some_and(|old| old != id)
        }) {
            return Err(format!(
                "{ERR_OPERATION_PHASE_CONFLICT}: nested receipt IDs are immutable for operation {operation_id}"
            ));
        }
        let business_receipts =
            business_receipts.unwrap_or_else(|| existing.business_receipts.clone());
        if business_receipts
            .iter()
            .any(|receipt| !valid_business_receipt(receipt))
            || !business_receipts.starts_with(&existing.business_receipts)
        {
            return Err(format!(
                "{ERR_OPERATION_PHASE_CONFLICT}: invalid business receipt history for operation {operation_id}"
            ));
        }
        let mut committed_phases = existing.committed_phases.clone();
        committed_phases.push(phase);
        let record = OperationJournalRecord {
            sequence: state.next_sequence,
            phase,
            committed_phases,
            business_receipts,
            nested_command_id: nested_command_id.or(existing.nested_command_id.clone()),
            nested_operation_id: nested_operation_id.or(existing.nested_operation_id.clone()),
            recorded_at_ms: crate::server::state::now_ms(),
            ..existing
        };
        append_record(&mut state, &self.file, record)
    }

    pub(crate) fn query(
        &self,
        request: &IdentityContextRequest,
        project_scope: &str,
        app_scope_id: &str,
    ) -> Result<IdentityContextResponseEnvelope, String> {
        let state = self.state.lock().unwrap();
        let Some(record) = state.operations.get(&request.operation_id) else {
            return Err(format!(
                "{ERR_OPERATION_UNKNOWN}: no durable operation {} exists",
                request.operation_id
            ));
        };
        if record.project_scope != project_scope || record.app_scope_id != app_scope_id {
            return Err(format!(
                "{ERR_OPERATION_DENIED}: operation {} does not belong to this project/app scope",
                request.operation_id
            ));
        }
        let supplied_hash = capability_hash(&request.query_capability);
        if !constant_time_eq(
            supplied_hash.as_bytes(),
            record.query_capability_hash.as_bytes(),
        ) {
            return Err(format!(
                "{ERR_OPERATION_DENIED}: query capability does not match operation {}",
                request.operation_id
            ));
        }
        Ok(IdentityContextResponseEnvelope::query(projection(record)))
    }
}

fn append_record(
    state: &mut OperationJournalState,
    file: &Mutex<File>,
    record: OperationJournalRecord,
) -> Result<IdentityOperationProjection, String> {
    let mut line = serde_json::to_vec(&record)
        .map_err(|error| format!("{ERR_OPERATION_APPEND}: serialize record: {error}"))?;
    line.push(b'\n');
    #[cfg(any(test, feature = "context-cancel-test-hooks"))]
    if take_append_fault() {
        let message = "injected operation journal append failure".to_string();
        state.append_poisoned = Some(message.clone());
        return Err(format!("{ERR_OPERATION_APPEND}: {message}"));
    }
    let mut file = file.lock().unwrap();
    if let Err(error) = file.seek(SeekFrom::Start(state.valid_prefix_bytes)) {
        let message = format!("seek valid prefix: {error}");
        state.append_poisoned = Some(message.clone());
        return Err(format!("{ERR_OPERATION_APPEND}: {message}"));
    }
    if let Err(error) = file.write_all(&line) {
        let message = format!("append: {error}");
        state.append_poisoned = Some(message.clone());
        return Err(format!("{ERR_OPERATION_APPEND}: {message}"));
    }
    if let Err(error) = file.sync_all() {
        let message = format!("sync_all: {error}");
        state.append_poisoned = Some(message.clone());
        return Err(format!("{ERR_OPERATION_APPEND}: {message}"));
    }
    state.valid_prefix_bytes = state.valid_prefix_bytes.saturating_add(line.len() as u64);
    state.next_sequence = state.next_sequence.saturating_add(1);
    state.incomplete_tail = false;
    state
        .operations
        .insert(record.operation_id.clone(), record.clone());
    Ok(projection(&record))
}

fn valid_transition(from: IdentityOperationPhase, to: IdentityOperationPhase) -> bool {
    use IdentityOperationPhase::*;
    matches!(
        (from, to),
        (
            Admitted,
            Validating | Refused | Failed | Cancelled | Unknown
        ) | (
            Validating,
            InnerDispatched | Refused | Failed | Cancelled | Unknown
        ) | (
            InnerDispatched,
            EffectObserved | Failed | Unknown | Partial | Cancelled
        ) | (
            EffectObserved,
            Completed | Failed | Unknown | Partial | Cancelled
        )
    )
}

fn replay_operation_journal(file: &mut File) -> anyhow::Result<OperationJournalState> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;

    let mut state = OperationJournalState {
        next_sequence: 0,
        operations: BTreeMap::new(),
        pending: BTreeMap::new(),
        live: BTreeMap::new(),
        incarnation: format!("{}-{}", std::process::id(), crate::server::state::now_ms()),
        next_ticket: 0,
        append_poisoned: None,
        valid_prefix_bytes: 0,
        incomplete_tail: false,
    };
    let mut offset = 0usize;
    while offset < bytes.len() {
        let relative_end = bytes[offset..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|position| offset + position + 1);
        let Some(line_end) = relative_end else {
            state.incomplete_tail = true;
            state.append_poisoned = Some(format!(
                "incomplete trailing record at byte {offset}; host reset is required"
            ));
            break;
        };
        let line = &bytes[offset..line_end];
        let record: OperationJournalRecord = serde_json::from_slice(line).map_err(|error| {
            anyhow::anyhow!(
                "operation journal replay failed at byte {offset}: malformed complete record: {error}"
            )
        })?;
        validate_record(&record, state.next_sequence)?;
        if let Some(existing) = state.operations.get(&record.operation_id) {
            if existing.project_scope != record.project_scope
                || existing.app_scope_id != record.app_scope_id
                || existing.action != record.action
                || existing.invocation != record.invocation
                || existing.intent_digest != record.intent_digest
                || existing.query_capability_hash != record.query_capability_hash
                || existing
                    .nested_command_id
                    .as_ref()
                    .is_some_and(|id| record.nested_command_id.as_ref() != Some(id))
                || existing
                    .nested_operation_id
                    .as_ref()
                    .is_some_and(|id| record.nested_operation_id.as_ref() != Some(id))
                || existing
                    .approval_evidence
                    .as_ref()
                    .is_some_and(|evidence| record.approval_evidence.as_ref() != Some(evidence))
            {
                anyhow::bail!(
                    "operation journal replay rejected changed immutable fields for operation {}",
                    record.operation_id
                );
            }
            if !valid_transition(existing.phase, record.phase) {
                anyhow::bail!(
                    "operation journal replay rejected illegal phase transition {} -> {} for operation {}",
                    existing.phase.as_str(),
                    record.phase.as_str(),
                    record.operation_id
                );
            }
            let mut committed = existing.committed_phases.clone();
            committed.push(record.phase);
            if record.committed_phases != committed {
                anyhow::bail!(
                    "operation journal replay rejected committed phase history for operation {}",
                    record.operation_id
                );
            }
            if !record
                .business_receipts
                .starts_with(&existing.business_receipts)
            {
                anyhow::bail!(
                    "operation journal replay rejected business receipt history for operation {}",
                    record.operation_id
                );
            }
        } else if record.phase != IdentityOperationPhase::Admitted {
            anyhow::bail!(
                "operation journal replay rejected phase {} without an admitted operation {}",
                record.phase.as_str(),
                record.operation_id
            );
        } else if !record.committed_phases.is_empty() {
            anyhow::bail!(
                "operation journal replay rejected committed business phases on admission for operation {}",
                record.operation_id
            );
        } else if !record.business_receipts.is_empty() {
            anyhow::bail!(
                "operation journal replay rejected business receipts on admission for operation {}",
                record.operation_id
            );
        }
        state.next_sequence = state.next_sequence.saturating_add(1);
        state.operations.insert(record.operation_id.clone(), record);
        offset = line_end;
        state.valid_prefix_bytes = offset as u64;
    }
    if state.incomplete_tail {
        for record in state.operations.values_mut() {
            if matches!(
                record.phase,
                IdentityOperationPhase::Admitted
                    | IdentityOperationPhase::Validating
                    | IdentityOperationPhase::InnerDispatched
                    | IdentityOperationPhase::EffectObserved
            ) {
                record.phase = IdentityOperationPhase::Unknown;
            }
        }
    }
    Ok(state)
}

fn validate_record(record: &OperationJournalRecord, expected_sequence: u64) -> anyhow::Result<()> {
    if record.schema_version != OPERATION_JOURNAL_SCHEMA_VERSION {
        anyhow::bail!(
            "operation journal replay rejected schema version {}",
            record.schema_version
        );
    }
    if record.record_type != OPERATION_RECORD_TYPE {
        anyhow::bail!(
            "operation journal replay rejected record type {}",
            record.record_type
        );
    }
    if record.sequence != expected_sequence {
        anyhow::bail!(
            "operation journal replay rejected sequence {}; expected {}",
            record.sequence,
            expected_sequence
        );
    }
    if record.operation_id.trim().is_empty()
        || record.project_scope.trim().is_empty()
        || record.app_scope_id.trim().is_empty()
        || record.action.trim().is_empty()
        || record.invocation.trim().is_empty()
        || record.intent_digest.trim().is_empty()
        || record.query_capability_hash.trim().is_empty()
    {
        anyhow::bail!("operation journal replay rejected an empty required field");
    }
    let invalid_committed_history = if record.phase == IdentityOperationPhase::Admitted {
        !record.committed_phases.is_empty()
    } else {
        record.committed_phases.last() != Some(&record.phase)
    };
    if invalid_committed_history {
        anyhow::bail!(
            "operation journal replay rejected committed phase list for operation {}",
            record.operation_id
        );
    }
    if record
        .business_receipts
        .iter()
        .any(|receipt| !valid_business_receipt(receipt))
    {
        anyhow::bail!(
            "operation journal replay rejected an unknown business receipt for operation {}",
            record.operation_id
        );
    }
    Ok(())
}

fn valid_business_receipt(receipt: &str) -> bool {
    matches!(
        receipt,
        "approval_decision"
            | "nested_register"
            | "route"
            | "credential"
            | "grant"
            | "lease"
            | "context_complete"
    )
}

fn projection(record: &OperationJournalRecord) -> IdentityOperationProjection {
    let outcome = match record.phase {
        IdentityOperationPhase::Admitted => "unknown",
        IdentityOperationPhase::Validating => "unknown",
        IdentityOperationPhase::InnerDispatched => "unknown",
        IdentityOperationPhase::EffectObserved => "partial",
        IdentityOperationPhase::Completed => "completed",
        IdentityOperationPhase::Refused => "denied",
        IdentityOperationPhase::Failed => "failed",
        IdentityOperationPhase::Unknown => "unknown",
        IdentityOperationPhase::Partial => "partial",
        IdentityOperationPhase::Cancelled => "cancelled",
    };
    IdentityOperationProjection {
        operation_id: record.operation_id.clone(),
        phase: record.phase.clone(),
        outcome: outcome.into(),
        committed_phases: record.committed_phases.clone(),
        business_receipts: record.business_receipts.clone(),
        nested_command_id: record.nested_command_id.clone(),
        nested_operation_id: record.nested_operation_id.clone(),
    }
}

pub(crate) fn capability_hash(capability: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(capability.as_bytes());
    format!("sha256:{:x}", digest.finalize())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        difference |= a ^ b;
    }
    difference == 0
}

#[cfg(test)]
pub(crate) fn peek_query_journal(path: &Path) -> Vec<IdentityOperationProjection> {
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .expect("open operation journal");
    let replay = replay_operation_journal(&mut file).expect("replay operation journal");
    replay.operations.values().map(projection).collect()
}
