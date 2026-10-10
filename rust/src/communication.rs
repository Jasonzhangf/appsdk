use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::global_registry;

pub const PROTOCOL: &str = "appsdk-comm/v1";
pub const DEFAULT_BATCH_WINDOW_SECONDS: i64 = 120;
pub const DEFAULT_MASTER_REMINDER_LIMIT: u8 = 3;
pub const DEFAULT_AGENT_LEASE_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const BUG_LOOP_TRIGGER: &str = "event:bug.reported";
const BUG_LOOP_WORK: &str = "triage -> fix in an independent worktree";
const BUG_LOOP_GATE: &str = "project verification and review";
const BUG_LOOP_STATE: &str = "persist bug evidence and next action";
const BUG_LOOP_STOP: &str = "resolved, merged, and reporter notified";
const APPSERVER_SEND_CAPABILITY: &str = "send_message_to_thread";

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize)]
pub struct CommError {
    pub code: String,
    pub message: String,
    pub context: Value,
}

impl CommError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            context: Value::Null,
        }
    }
}

impl Display for CommError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl Error for CommError {}

type CommResult<T> = Result<T, CommError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Address {
    #[serde(rename = "scopeId", alias = "scope_id")]
    pub scope_id: String,
    #[serde(rename = "sessionId", alias = "session_id")]
    pub session_id: String,
}

impl Address {
    fn key(&self) -> String {
        structured_key(&[&self.scope_id, &self.session_id])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    P0,
    P1,
    P2,
    P3,
}

impl Priority {
    fn parse(value: &str) -> CommResult<Self> {
        match value.to_ascii_lowercase().as_str() {
            "p0" => Ok(Self::P0),
            "p1" => Ok(Self::P1),
            "p2" => Ok(Self::P2),
            "p3" => Ok(Self::P3),
            _ => Err(CommError::new(
                "invalid_priority",
                format!("priority must be p0, p1, p2 or p3: {value}"),
            )),
        }
    }

    fn is_breakthrough(&self) -> bool {
        matches!(self, Self::P0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum DeliveryMode {
    Direct,
    Idle,
}

impl DeliveryMode {
    fn parse(value: Option<&str>) -> CommResult<Self> {
        match value.unwrap_or("idle").to_ascii_lowercase().as_str() {
            "direct" => Ok(Self::Direct),
            "idle" | "batched" => Ok(Self::Idle),
            other => Err(CommError::new(
                "invalid_delivery_mode",
                format!("delivery mode must be direct or idle: {other}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum AgentState {
    Working,
    Idle,
}

impl AgentState {
    fn parse(value: &str) -> CommResult<Self> {
        match value.to_ascii_lowercase().as_str() {
            "working" => Ok(Self::Working),
            "idle" => Ok(Self::Idle),
            other => Err(CommError::new(
                "invalid_agent_state",
                format!("agent state must be working or idle: {other}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScopeRequest {
    #[serde(rename = "scopeId", alias = "scope_id")]
    scope_id: String,
    #[serde(rename = "appserverId", alias = "appserver_id")]
    appserver_id: String,
    namespace: String,
    endpoint: String,
    #[serde(rename = "projectRoot", alias = "project_root")]
    project_root: String,
    #[serde(default, rename = "sessionIds", alias = "session_ids")]
    session_ids: Vec<String>,
    #[serde(default, rename = "runtimeId", alias = "runtime_id")]
    runtime_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentRequest {
    #[serde(rename = "scopeId", alias = "scope_id")]
    scope_id: String,
    #[serde(rename = "sessionId", alias = "session_id")]
    session_id: String,
    #[serde(rename = "agentId", alias = "agent_id")]
    agent_id: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default, rename = "masterGrant", alias = "master_grant")]
    master_grant: Option<String>,
    #[serde(default)]
    parent: Option<Address>,
    #[serde(default, rename = "leaseMs", alias = "lease_ms")]
    lease_ms: Option<u64>,
    #[serde(default, rename = "runtimeId", alias = "runtime_id")]
    runtime_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RebindAgentRequest {
    from: Address,
    to: Address,
    #[serde(rename = "runtimeId", alias = "runtime_id")]
    runtime_id: String,
    #[serde(default, alias = "observedAt", alias = "observed_at")]
    at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RuntimeRequest {
    #[serde(rename = "runtimeId", alias = "runtime_id")]
    runtime_id: String,
    #[serde(rename = "appserverId", alias = "appserver_id")]
    appserver_id: String,
    namespace: String,
    endpoint: String,
    #[serde(rename = "projectRoot", alias = "project_root")]
    project_root: String,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(rename = "processId", alias = "process_id")]
    process_id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MessageRequest {
    from: Address,
    to: Address,
    title: String,
    priority: String,
    body: String,
    #[serde(default, rename = "deliveryMode", alias = "delivery", alias = "mode")]
    delivery_mode: Option<String>,
    #[serde(default, rename = "coalesceKey", alias = "coalesce_key")]
    coalesce_key: Option<String>,
    #[serde(default, rename = "issueId", alias = "issue_id")]
    issue_id: Option<String>,
    #[serde(default, rename = "conversationId", alias = "conversation_id")]
    conversation_id: Option<String>,
    #[serde(default, rename = "messageId", alias = "message_id")]
    message_id: Option<String>,
    #[serde(default, rename = "createdAt", alias = "created_at")]
    created_at: Option<String>,
    #[serde(default, rename = "adapterId", alias = "adapter_id")]
    adapter_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeliveryRequest {
    #[serde(rename = "messageId", alias = "message_id")]
    message_id: String,
    #[serde(rename = "attemptId", alias = "attempt_id")]
    attempt_id: String,
    nonce: String,
    state: String,
    #[serde(rename = "runtimeId", alias = "runtime_id")]
    runtime_id: String,
    evidence: Value,
    #[serde(default, rename = "observedAt", alias = "observed_at")]
    observed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BugRequest {
    #[serde(rename = "bugId", alias = "bug_id")]
    bug_id: String,
    #[serde(rename = "scopeId", alias = "scope_id")]
    scope_id: String,
    title: String,
    priority: String,
    description: String,
    reporter: Address,
    #[serde(default, rename = "worktreeId", alias = "worktree_id")]
    worktree_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LoopRequest {
    #[serde(rename = "loopId", alias = "loop_id")]
    loop_id: String,
    kind: String,
    owner: Address,
    trigger: String,
    work: String,
    gate: String,
    state: String,
    stop: String,
    #[serde(
        default = "default_max_iterations",
        rename = "maxIterations",
        alias = "max_iterations"
    )]
    max_iterations: u32,
    #[serde(default, rename = "deadlineAt", alias = "deadline_at")]
    deadline_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AdapterRequest {
    #[serde(rename = "adapterId", alias = "adapter_id")]
    adapter_id: String,
    kind: String,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default, rename = "execute", alias = "allowExecution")]
    execute: Option<bool>,
    #[serde(default)]
    recipient: Option<Address>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ScopeRecord {
    #[serde(rename = "scopeId")]
    scope_id: String,
    #[serde(rename = "appserverId")]
    appserver_id: String,
    namespace: String,
    endpoint: String,
    #[serde(rename = "projectRoot")]
    project_root: String,
    #[serde(rename = "sessionIds")]
    session_ids: Vec<String>,
    #[serde(rename = "registeredAt")]
    registered_at: String,
    #[serde(rename = "lastObservedAt")]
    last_observed_at: String,
    #[serde(rename = "masterSessionId")]
    master_session_id: Option<String>,
    #[serde(default, rename = "runtimeId")]
    runtime_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AgentRecord {
    #[serde(rename = "scopeId")]
    scope_id: String,
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "agentId")]
    agent_id: String,
    role: String,
    #[serde(rename = "masterGrant")]
    master_grant: Option<String>,
    parent: Option<Address>,
    #[serde(rename = "leaseMs")]
    lease_ms: u64,
    #[serde(rename = "registeredAt")]
    registered_at: String,
    #[serde(rename = "lastObservedAt")]
    last_observed_at: String,
    #[serde(rename = "expiresAt")]
    expires_at: String,
    state: AgentState,
    #[serde(rename = "lastStateAt")]
    last_state_at: String,
    #[serde(default, rename = "runtimeId")]
    runtime_id: Option<String>,
}

impl AgentRecord {
    fn address(&self) -> Address {
        Address {
            scope_id: self.scope_id.clone(),
            session_id: self.session_id.clone(),
        }
    }

    fn live_at(&self, at: &str) -> bool {
        parse_time(at)
            .ok()
            .zip(parse_time(&self.expires_at).ok())
            .map(|(now, expires)| expires > now)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AgentTombstone {
    address: Address,
    #[serde(rename = "agentId")]
    agent_id: String,
    #[serde(rename = "runtimeId")]
    runtime_id: String,
    #[serde(rename = "reboundTo")]
    rebound_to: Address,
    #[serde(rename = "reboundAt")]
    rebound_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AgentReboundEvent {
    from: AgentRecord,
    to: AgentRecord,
    tombstone: AgentTombstone,
}

/// A local, append-only intent that bridges the project mailbox and the
/// host-wide discovery index.  The mailbox is the source of truth; the host
/// index is a rebuildable projection.  Keeping the complete local record in
/// the intent lets recovery finish a local commit if the process stops between
/// the two durable stores.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum DiscoveryOperation {
    Scope { record: ScopeRecord },
    Agent { record: AgentRecord },
    Rebind { event: AgentReboundEvent },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DiscoveryPendingRecord {
    #[serde(rename = "pendingId")]
    pending_id: String,
    operation: DiscoveryOperation,
    #[serde(rename = "createdAt")]
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RouteRecord {
    mode: String,
    #[serde(rename = "sameAppserver")]
    same_appserver: bool,
    #[serde(rename = "sameProject")]
    same_project: bool,
    #[serde(rename = "sourceRole")]
    source_role: String,
    #[serde(rename = "targetRole")]
    target_role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeliveryEvidence {
    state: String,
    at: String,
    details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ErrorRecord {
    code: String,
    message: String,
    context: Value,
    at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AdapterRecord {
    #[serde(rename = "adapterId")]
    adapter_id: String,
    kind: String,
    target: Option<String>,
    enabled: bool,
    execute: bool,
    #[serde(default)]
    recipient: Option<Address>,
    #[serde(rename = "registeredAt")]
    registered_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TransportReceipt {
    #[serde(rename = "adapterId")]
    adapter_id: String,
    kind: String,
    state: String,
    target: Option<String>,
    evidence: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeliveryAttempt {
    #[serde(rename = "attemptId")]
    attempt_id: String,
    operation: String,
    #[serde(rename = "adapterId")]
    adapter_id: String,
    #[serde(rename = "startedAt")]
    started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "batchId")]
    batch_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct MessageDeliveryAttempt {
    #[serde(rename = "attemptId")]
    attempt_id: String,
    #[serde(rename = "messageId")]
    message_id: String,
    operation: String,
    #[serde(rename = "adapterId")]
    adapter_id: String,
    #[serde(rename = "runtimeId")]
    runtime_id: String,
    #[serde(rename = "runtimeFingerprint")]
    runtime_fingerprint: String,
    target: Option<String>,
    nonce: String,
    #[serde(rename = "startedAt")]
    started_at: String,
}

trait CommunicationAdapter {
    fn deliver(&self, message: &MessageRecord) -> CommResult<TransportReceipt>;
    fn emit_batch(&self, batch: &NotificationBatch) -> CommResult<TransportReceipt>;
}

struct MailboxAdapter {
    adapter_id: String,
    path: PathBuf,
}

impl CommunicationAdapter for MailboxAdapter {
    fn deliver(&self, _message: &MessageRecord) -> CommResult<TransportReceipt> {
        Ok(TransportReceipt {
            adapter_id: self.adapter_id.clone(),
            kind: "mailbox".into(),
            state: "accepted".into(),
            target: Some(self.path.display().to_string()),
            evidence: json!({ "durable": true, "format": "jsonl" }),
        })
    }

    fn emit_batch(&self, _batch: &NotificationBatch) -> CommResult<TransportReceipt> {
        Ok(TransportReceipt {
            adapter_id: self.adapter_id.clone(),
            kind: "mailbox".into(),
            state: "accepted".into(),
            target: Some(self.path.display().to_string()),
            evidence: json!({ "durable": true, "format": "jsonl", "batch": true }),
        })
    }
}

struct AppserverAdapter {
    adapter_id: String,
    runtime_id: String,
    endpoint: String,
    capability: String,
}

impl CommunicationAdapter for AppserverAdapter {
    fn deliver(&self, message: &MessageRecord) -> CommResult<TransportReceipt> {
        Ok(TransportReceipt {
            adapter_id: self.adapter_id.clone(),
            kind: "appserver".into(),
            state: "intent".into(),
            target: Some(self.endpoint.clone()),
            evidence: json!({
                "messageId": message.message_id,
                "hostMustExecute": true,
                "runtimeId": self.runtime_id,
                "capability": self.capability
            }),
        })
    }

    fn emit_batch(&self, batch: &NotificationBatch) -> CommResult<TransportReceipt> {
        Ok(TransportReceipt {
            adapter_id: self.adapter_id.clone(),
            kind: "appserver".into(),
            state: "intent".into(),
            target: Some(self.endpoint.clone()),
            evidence: json!({
                "batchId": batch.batch_id,
                "hostMustExecute": true,
                "runtimeId": self.runtime_id,
                "capability": self.capability
            }),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MessageRecord {
    protocol: String,
    #[serde(rename = "messageId")]
    message_id: String,
    #[serde(rename = "conversationId")]
    conversation_id: String,
    from: Address,
    to: Address,
    title: String,
    priority: Priority,
    body: String,
    #[serde(rename = "deliveryMode")]
    delivery_mode: DeliveryMode,
    #[serde(rename = "coalesceKey")]
    coalesce_key: Option<String>,
    #[serde(rename = "issueId")]
    issue_id: Option<String>,
    #[serde(rename = "adapterId")]
    adapter_id: String,
    #[serde(default, rename = "deliveryAttemptRequired")]
    delivery_attempt_required: bool,
    #[serde(rename = "createdAt")]
    created_at: String,
    state: String,
    evidence: Vec<DeliveryEvidence>,
    route: RouteRecord,
    #[serde(rename = "lastError")]
    last_error: Option<ErrorRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NotificationRecord {
    #[serde(rename = "notificationId")]
    notification_id: String,
    #[serde(rename = "messageId")]
    message_id: String,
    /// Monotonically increases whenever an idle coalescing bucket reopens
    /// after a terminal outcome.  Legacy records omit this field and belong
    /// to generation zero.
    #[serde(default)]
    generation: u64,
    recipient: Address,
    title: String,
    priority: Priority,
    #[serde(rename = "issueId")]
    issue_id: Option<String>,
    #[serde(rename = "coalesceKey")]
    coalesce_key: Option<String>,
    body: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "availableAt")]
    available_at: String,
    status: String,
    #[serde(rename = "emittedAt")]
    emitted_at: Option<String>,
    #[serde(rename = "adapterId")]
    adapter_id: String,
    #[serde(default, rename = "transportReceipt")]
    transport_receipt: Option<TransportReceipt>,
    #[serde(default, rename = "lastError")]
    last_error: Option<ErrorRecord>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "deliveryAttempt"
    )]
    delivery_attempt: Option<DeliveryAttempt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NotificationSummary {
    #[serde(rename = "notificationId")]
    notification_id: String,
    #[serde(rename = "messageId")]
    message_id: String,
    #[serde(default)]
    generation: u64,
    title: String,
    priority: Priority,
    #[serde(rename = "issueId")]
    issue_id: Option<String>,
    #[serde(rename = "coalesceKey")]
    coalesce_key: Option<String>,
    #[serde(rename = "createdAt")]
    created_at: String,
}

impl NotificationRecord {
    fn summary(&self) -> NotificationSummary {
        NotificationSummary {
            notification_id: self.notification_id.clone(),
            message_id: self.message_id.clone(),
            generation: self.generation,
            title: self.title.clone(),
            priority: self.priority.clone(),
            issue_id: self.issue_id.clone(),
            coalesce_key: self.coalesce_key.clone(),
            created_at: self.created_at.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NotificationBatch {
    #[serde(rename = "batchId")]
    batch_id: String,
    recipient: Address,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "adapterId")]
    adapter_id: String,
    items: Vec<NotificationSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WakeupRecord {
    address: Address,
    /// Address the wakeup's generated reminder identities were minted from.
    /// Rebinding moves `address` (the delivery target) while queued/emitted
    /// reminder messages keep their original identity, so reminder replay must
    /// resolve identities through this origin.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "identityOrigin"
    )]
    identity_origin: Option<Address>,
    #[serde(rename = "idleSince")]
    idle_since: Option<String>,
    #[serde(rename = "remindersSent")]
    reminders_sent: u8,
    #[serde(rename = "nextDueAt")]
    next_due_at: Option<String>,
    stopped: bool,
    #[serde(rename = "lastReminderAt")]
    last_reminder_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct MasterWakeSignal {
    #[serde(rename = "signalId")]
    signal_id: String,
    key: String,
    /// `key`/`signal_id` as they were when any message for this signal was
    /// minted. Worker-sourced signals remap both when their worker rebinds so
    /// the wake bookkeeping follows the new address, while already minted
    /// messages keep their original identity. Pinning the pre-remap pair keeps
    /// that message identity resolvable after the remap.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "identityKey"
    )]
    identity_key: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "identitySignalId"
    )]
    identity_signal_id: Option<String>,
    kind: String,
    title: String,
    priority: Priority,
    summary: String,
    #[serde(rename = "issueId")]
    issue_id: Option<String>,
    source: Option<Address>,
    #[serde(rename = "observedAt")]
    observed_at: String,
    #[serde(rename = "directDispatched")]
    direct_dispatched: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MasterWakeAccumulator {
    address: Address,
    /// Address the accumulator's generated message identities were minted from.
    /// Rebinding moves `address`, which is only the current delivery target, while
    /// persisted messages, notifications and delivery attempts keep the identity
    /// they were minted with, so every wake replay path must resolve identities
    /// through this origin instead of the rebound address.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "identityOrigin"
    )]
    identity_origin: Option<Address>,
    generation: u64,
    pending: bool,
    #[serde(rename = "firstObservedAt")]
    first_observed_at: Option<String>,
    #[serde(rename = "lastObservedAt")]
    last_observed_at: Option<String>,
    #[serde(rename = "nextDueAt")]
    next_due_at: Option<String>,
    #[serde(rename = "remindersSent")]
    reminders_sent: u8,
    stopped: bool,
    #[serde(rename = "lastBriefingGeneration")]
    last_briefing_generation: Option<u64>,
    #[serde(rename = "lastBriefingAt")]
    last_briefing_at: Option<String>,
    #[serde(default)]
    held: bool,
    #[serde(default)]
    signals: BTreeMap<String, MasterWakeSignal>,
    #[serde(default, rename = "consumedSignals")]
    consumed_signals: BTreeMap<String, MasterWakeSignal>,
}

impl MasterWakeAccumulator {
    fn identity_address(&self) -> &Address {
        self.identity_origin.as_ref().unwrap_or(&self.address)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MasterWakeSignalRequest {
    #[serde(rename = "signalId", alias = "signal_id")]
    signal_id: Option<String>,
    key: String,
    kind: String,
    title: String,
    priority: String,
    summary: String,
    #[serde(default, rename = "issueId", alias = "issue_id")]
    issue_id: Option<String>,
    #[serde(default)]
    source: Option<Address>,
    #[serde(default, rename = "observedAt", alias = "observed_at")]
    observed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BugRecord {
    #[serde(rename = "bugId")]
    bug_id: String,
    #[serde(rename = "scopeId")]
    scope_id: String,
    title: String,
    priority: Priority,
    description: String,
    reporter: Address,
    status: String,
    #[serde(rename = "worktreeId")]
    worktree_id: Option<String>,
    #[serde(rename = "loopId")]
    loop_id: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    #[serde(default, rename = "resolutionEvidence")]
    resolution_evidence: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LoopRecord {
    #[serde(rename = "loopId")]
    loop_id: String,
    kind: String,
    owner: Address,
    trigger: String,
    work: String,
    gate: String,
    state: String,
    stop: String,
    #[serde(rename = "maxIterations")]
    max_iterations: u32,
    #[serde(rename = "deadlineAt")]
    deadline_at: Option<String>,
    phase: String,
    status: String,
    iteration: u32,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    #[serde(
        default,
        rename = "completionEvidence",
        skip_serializing_if = "Option::is_none"
    )]
    completion_evidence: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Projection {
    scopes: BTreeMap<String, ScopeRecord>,
    agents: BTreeMap<String, AgentRecord>,
    #[serde(default, rename = "agentTombstones")]
    agent_tombstones: BTreeMap<String, AgentTombstone>,
    #[serde(default, rename = "discoveryPending")]
    discovery_pending: BTreeMap<String, DiscoveryPendingRecord>,
    messages: BTreeMap<String, MessageRecord>,
    #[serde(default, rename = "messageDeliveryAttempts")]
    message_delivery_attempts: BTreeMap<String, MessageDeliveryAttempt>,
    notifications: BTreeMap<String, NotificationRecord>,
    adapters: BTreeMap<String, AdapterRecord>,
    wakeup: BTreeMap<String, WakeupRecord>,
    #[serde(default)]
    master_wake: BTreeMap<String, MasterWakeAccumulator>,
    bugs: BTreeMap<String, BugRecord>,
    loops: BTreeMap<String, LoopRecord>,
    #[serde(default)]
    batches: Vec<NotificationBatch>,
    #[serde(skip)]
    completed_attempts: BTreeMap<String, String>,
    #[serde(skip)]
    event_ordinal: u64,
    #[serde(skip)]
    message_ordinals: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EventRecord {
    protocol: String,
    #[serde(rename = "eventId")]
    event_id: String,
    at: String,
    kind: String,
    data: Value,
}

pub struct CommunicationStore {
    mailbox_path: PathBuf,
    project_root: PathBuf,
    projection: Projection,
    _lock: CommunicationLock,
}

struct CommunicationLock {
    _file: Option<File>,
}

impl CommunicationLock {
    fn read_only() -> Self {
        Self { _file: None }
    }

    fn acquire(mailbox_path: &Path) -> CommResult<Self> {
        let lock_path = mailbox_path.with_extension("jsonl.lock");
        reject_symlink_components(&lock_path, "communication_lock")?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| {
                CommError::new(
                    "communication_lock_open_failed",
                    format!("{}: {error}", lock_path.display()),
                )
            })?;

        match crate::platform::try_lock_exclusive(&file) {
            Ok(()) => {}
            Err(crate::platform::LockAttemptError::WouldBlock) => {
                return Err(CommError::new(
                    "communication_busy",
                    format!("communication mailbox is locked: {}", lock_path.display()),
                ));
            }
            Err(crate::platform::LockAttemptError::Io(error)) => {
                return Err(CommError::new(
                    "communication_lock_failed",
                    format!("{}: {error}", lock_path.display()),
                ));
            }
        }

        Ok(Self { _file: Some(file) })
    }
}

#[path = "communication/store_core.rs"]
mod store_core;
#[path = "communication/store_delivery.rs"]
mod store_delivery;
#[path = "communication/store_events.rs"]
mod store_events;
#[path = "communication/store_journal.rs"]
mod store_journal;
#[path = "communication/store_messaging.rs"]
mod store_messaging;
#[path = "communication/store_runtime.rs"]
mod store_runtime;
#[path = "communication/validation.rs"]
mod validation;
use validation::*;
#[path = "communication/helpers.rs"]
mod helpers;
use helpers::*;

#[allow(dead_code)]
pub fn run_cli(args: Vec<String>) -> CommResult<()> {
    if args.len() == 1 && args[0] == "capabilities" {
        println!("{}", serde_json::to_string_pretty(&capabilities()).unwrap());
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("reset-runtime-registry") {
        let mut discard_legacy = false;
        let mut approval = None;
        let mut index = 1;
        while index < args.len() {
            match args[index].as_str() {
                "--discard-legacy" => discard_legacy = true,
                "--approval" => {
                    index += 1;
                    approval = args.get(index).cloned();
                }
                value => {
                    return Err(CommError::new(
                        "usage",
                        format!("unknown reset-runtime-registry option: {value}"),
                    ))
                }
            }
            index += 1;
        }
        let approval = approval.ok_or_else(|| {
            CommError::new(
                "usage",
                "appsdk communication reset-runtime-registry --discard-legacy --approval <text>",
            )
        })?;
        let receipt = global_registry::reset_runtime_registry(discard_legacy, &approval)
            .map_err(|error| CommError::new("runtime_registry_reset_failed", error))?;
        println!("{}", serde_json::to_string_pretty(&receipt).unwrap());
        return Ok(());
    }
    let root = args.first().ok_or_else(|| {
        CommError::new("usage", "appsdk communication <project> --json '<request>'")
    })?;
    if args.get(1).map(String::as_str) != Some("--json") || args.len() != 3 {
        return Err(CommError::new(
            "usage",
            "appsdk communication <project> --json '<request>'",
        ));
    }
    let request: Value = serde_json::from_str(&args[2])
        .map_err(|error| CommError::new("invalid_request_json", error.to_string()))?;
    let mut store = CommunicationStore::open(Path::new(root))?;
    match store.dispatch(&request) {
        Ok(result) => {
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Err(error) => match store.record_error_event(&error) {
            Ok(()) => Err(error),
            Err(record_error) => Err(with_secondary_error(error, record_error, "error.recorded")),
        },
    }
}

pub fn capabilities() -> Value {
    json!({
        "protocol": PROTOCOL,
        "execution": [
                "register_runtime", "register_adapter", "register_scope", "register_agent", "refresh_agent", "rebind_agent",
                "send", "record_delivery", "set_agent_state", "tick", "accumulate_wake", "record_wake",
            "master_wake_decide", "flush_notifications", "report_bug",
            "update_bug", "create_loop", "advance_loop", "record_error"
        ],
        "query": ["status", "capabilities"],
        "namespaces": ["codex_app", "codex_tui"],
        "roles": ["master", "peer", "subagent"],
        "routeRules": [
            "same_appserver_same_project_peer",
            "same_scope_parent",
            "cross_scope_master"
        ],
        "deliveryStates": [
            "created", "accepted", "delivered", "executed", "replied", "read", "consumed"
        ],
        "notification": {
            "batchWindowSeconds": DEFAULT_BATCH_WINDOW_SECONDS,
            "masterReminderLimit": DEFAULT_MASTER_REMINDER_LIMIT,
            "p0Breakthrough": true,
            "masterWake": "accumulate-while-working-and-brief-when-idle"
        },
        "facts": {
            "format": "jsonl",
            "path": ".appsdk-control/communication/mailbox.jsonl",
            "projection": "replayed",
            "lock": "exclusive-command-lifecycle"
        },
        "runtimeIdentity": {
            "registry": "~/.appsdk/runtimes.jsonl",
            "stableAcross": ["conversation-compaction", "conversation-fork"],
            "scopeBinding": "runtimeId",
            "proof": "host-registration-receipt"
        },
        "adapters": ["mailbox", "appserver"],
        "adapterBinding": "recipient-address",
        "completionEvidence": {
            "bug": ["fix", "verification", "merge"],
            "loop": ["gate", "verification"]
        }
    })
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "appsdk-communication-lock-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        // Communication enforces an absolute canonical project root. The OS
        // temp dir is not canonical on every platform (macOS `/var` alias,
        // Windows 8.3 short names), so resolve it once for the fixture.
        root.canonicalize().unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn canonical_native_prefix_root_passes_lexical_check() {
        let root = temp_root("native-prefix");
        let canonical = root.canonicalize().unwrap();
        let text = canonical.to_str().unwrap();
        assert!(
            text.starts_with(r"\\?\"),
            "expected a native verbatim prefix: {text}"
        );
        assert!(
            is_lexically_canonical_absolute(&canonical),
            "canonical native prefix root must pass the lexical check: {text}"
        );
        // Redundant separators, embedded dot/parent segments and trailing
        // separators stay rejected on the same canonical prefix root.
        for variant in [
            format!("{text}\\"),
            format!("{text}\\\\child"),
            format!("{text}\\.\\child"),
            format!("{text}\\..\\child"),
        ] {
            assert!(
                !is_lexically_canonical_absolute(Path::new(&variant)),
                "non-canonical variant must be rejected: {variant}"
            );
        }
        fs::remove_dir_all(root).ok();
    }

    #[cfg(windows)]
    #[test]
    fn native_prefix_path_stat_preserves_symlink_refusal() {
        use std::path::Component;

        // Drive, verbatim drive, UNC and verbatim UNC roots all expose a
        // leading `Prefix` followed by an explicit `RootDir`. These are pure
        // component shapes; no network share is touched.
        for raw in [
            r"C:\dir",
            r"\\?\C:\dir",
            r"\\server\share\dir",
            r"\\?\UNC\server\share\dir",
        ] {
            let mut parts = Path::new(raw).components();
            assert!(
                matches!(parts.next(), Some(Component::Prefix(_))),
                "expected a leading prefix for {raw}"
            );
            assert!(
                matches!(parts.next(), Some(Component::RootDir)),
                "expected a root dir after the prefix for {raw}"
            );
        }

        // The canonical temp root carries a real verbatim prefix and must now
        // pass the component stat instead of failing on the bare `\\?\C:`.
        let root = temp_root("native-prefix-stat");
        let canonical = root.canonicalize().unwrap();
        validate_communication_root_input(&canonical)
            .expect("canonical native prefix root must pass component stat");

        // A trailing component that does not exist still ends the walk via the
        // existing NotFound behavior rather than a stat failure.
        validate_communication_root_input(&canonical.join("does-not-exist"))
            .expect("missing trailing component must keep the NotFound behavior");

        // Real directory symlinks as a trailing and an intermediate component
        // must still be refused.
        let target = root.join("real-dir");
        fs::create_dir_all(&target).unwrap();
        let link = root.join("link");
        std::os::windows::fs::symlink_dir(&target, &link)
            .expect("creating a Windows directory symlink must succeed, not skip");
        let error = validate_communication_root_input(&link)
            .expect_err("trailing symlink component must be refused");
        assert_eq!(error.code, "communication_path_symlink");
        let error = validate_communication_root_input(&link.join("child"))
            .expect_err("intermediate symlink component must be refused");
        assert_eq!(error.code, "communication_path_symlink");

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn store_lock_is_exclusive_for_store_lifetime() {
        let root = temp_root("lifetime");
        let store = CommunicationStore::open(&root).unwrap();

        // The store keeps the lock File alive, so a second open on the same
        // mailbox must fail on the real OS lock.
        let blocked = match CommunicationStore::open(&root) {
            Ok(_) => panic!("second store unexpectedly acquired the mailbox lock"),
            Err(error) => error,
        };
        assert_eq!(blocked.code, "communication_busy");

        drop(store);
        let reopened = CommunicationStore::open(&root).unwrap();
        drop(reopened);

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn acquire_reports_lock_open_failure() {
        let root = temp_root("open-failure");
        let mailbox_path = root.join(".appsdk-control/communication/mailbox.jsonl");
        // A directory occupying the lock path makes the open step fail before
        // any locking is attempted.
        let lock_path = mailbox_path.with_extension("jsonl.lock");
        fs::create_dir_all(&lock_path).unwrap();

        let error = match CommunicationLock::acquire(&mailbox_path) {
            Ok(_) => panic!("lock acquire unexpectedly succeeded on a directory path"),
            Err(error) => error,
        };
        assert_eq!(error.code, "communication_lock_open_failed");

        fs::remove_dir_all(root).ok();
    }
}
