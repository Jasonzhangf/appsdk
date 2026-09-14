use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
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
    #[serde(default, rename = "tmuxSession", alias = "tmux_session")]
    tmux_session: Option<String>,
    #[serde(default, rename = "tmuxPane", alias = "tmux_pane")]
    tmux_pane: Option<String>,
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

struct TmuxAdapter {
    adapter_id: String,
    runtime_id: String,
    target: String,
    execute: bool,
}

impl CommunicationAdapter for TmuxAdapter {
    fn deliver(&self, message: &MessageRecord) -> CommResult<TransportReceipt> {
        let preview = bounded_preview(message);
        if self.execute {
            let result = Command::new("tmux")
                .args(["send-keys", "-t", &self.target, &preview, "Enter"])
                .output()
                .map_err(|error| CommError::new("tmux_delivery_failed", error.to_string()))?;
            if !result.status.success() {
                return Err(CommError::new(
                    "tmux_delivery_failed",
                    String::from_utf8_lossy(&result.stderr).trim().to_string(),
                ));
            }
        }
        Ok(TransportReceipt {
            adapter_id: self.adapter_id.clone(),
            kind: "tmux".into(),
            state: if self.execute { "delivered" } else { "intent" }.into(),
            target: Some(self.target.clone()),
            evidence: json!({
                "preview": preview,
                "executed": self.execute,
                "runtimeId": self.runtime_id
            }),
        })
    }

    fn emit_batch(&self, batch: &NotificationBatch) -> CommResult<TransportReceipt> {
        let preview = format!(
            "[appsdk] {} updates for {}",
            batch.items.len(),
            batch.recipient.key()
        );
        if self.execute {
            let result = Command::new("tmux")
                .args(["send-keys", "-t", &self.target, &preview, "Enter"])
                .output()
                .map_err(|error| CommError::new("tmux_delivery_failed", error.to_string()))?;
            if !result.status.success() {
                return Err(CommError::new(
                    "tmux_delivery_failed",
                    String::from_utf8_lossy(&result.stderr).trim().to_string(),
                ));
            }
        }
        Ok(TransportReceipt {
            adapter_id: self.adapter_id.clone(),
            kind: "tmux".into(),
            state: if self.execute { "delivered" } else { "intent" }.into(),
            target: Some(self.target.clone()),
            evidence: json!({
                "preview": preview,
                "executed": self.execute,
                "runtimeId": self.runtime_id
            }),
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

        #[cfg(unix)]
        {
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result != 0 {
                let error = std::io::Error::last_os_error();
                let code = error.raw_os_error();
                if matches!(code, Some(libc::EAGAIN) | Some(libc::EACCES)) {
                    return Err(CommError::new(
                        "communication_busy",
                        format!("communication mailbox is locked: {}", lock_path.display()),
                    ));
                }
                return Err(CommError::new(
                    "communication_lock_failed",
                    format!("{}: {error}", lock_path.display()),
                ));
            }
        }

        Ok(Self { _file: Some(file) })
    }
}

impl CommunicationStore {
    pub fn open(root: &Path) -> CommResult<Self> {
        validate_communication_root_input(root)?;
        if !root.exists() {
            fs::create_dir_all(root).map_err(|error| {
                CommError::new(
                    "communication_root_create_failed",
                    format!("{}: {error}", root.display()),
                )
            })?;
        }
        let canonical_root = validate_communication_root(root)?;
        Self::open_mailbox(canonical_root.join(".appsdk-control/communication/mailbox.jsonl"))
    }

    pub fn open_mailbox(mailbox_path: PathBuf) -> CommResult<Self> {
        let project_root = infer_project_root(&mailbox_path)?;
        Self::open_mailbox_at(mailbox_path, project_root)
    }

    fn open_mailbox_read_only(mailbox_path: PathBuf) -> CommResult<Self> {
        let project_root = infer_project_root(&mailbox_path)?;
        reject_symlink_components(&mailbox_path, "communication_mailbox")?;
        if !mailbox_path.is_file() {
            return Err(CommError::new(
                "communication_mailbox_missing",
                format!(
                    "communication mailbox is missing: {}",
                    mailbox_path.display()
                ),
            ));
        }
        let mut store = Self {
            mailbox_path,
            project_root,
            projection: Projection::default(),
            _lock: CommunicationLock::read_only(),
        };
        store
            .projection
            .adapters
            .insert("mailbox".into(), default_mailbox_adapter());
        // A project store already holds its own exclusive mailbox lock while
        // resolving a cross-project target.  Replaying the target's complete
        // mailbox here would recursively acquire that lock (and can form an
        // A -> B -> A cycle).  Discovery only needs the target identity
        // projection; the target project performs full journal validation when
        // it is opened as the owner of its mailbox.
        store.replay_identity_only()?;
        Ok(store)
    }

    fn open_mailbox_at(mailbox_path: PathBuf, project_root: PathBuf) -> CommResult<Self> {
        reject_symlink_components(&mailbox_path, "communication_mailbox")?;
        if let Some(parent) = mailbox_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                CommError::new(
                    "mailbox_directory_create_failed",
                    format!("{}: {error}", parent.display()),
                )
            })?;
        }
        reject_symlink_components(&mailbox_path, "communication_mailbox")?;
        let lock = CommunicationLock::acquire(&mailbox_path)?;
        let mut store = Self {
            mailbox_path,
            project_root,
            projection: Projection::default(),
            _lock: lock,
        };
        // The built-in mailbox adapter is part of the replay baseline. Events
        // created through the default adapter must validate against it while
        // replaying, before any journal-sourced adapter registrations apply.
        store
            .projection
            .adapters
            .insert("mailbox".into(), default_mailbox_adapter());
        store.replay()?;
        store.reconcile_discovery_pending()?;
        Ok(store)
    }

    pub fn status(&self) -> Value {
        let mut active_bugs: Vec<BugRecord> = self
            .projection
            .bugs
            .values()
            .filter(|bug| bug.status == "active")
            .cloned()
            .collect();
        active_bugs.sort_by(priority_then_time_bug);
        let mut loops: Vec<LoopRecord> = self.projection.loops.values().cloned().collect();
        loops.sort_by(|left, right| {
            if left.status == "active" && right.status != "active" {
                Ordering::Less
            } else if left.status != "active" && right.status == "active" {
                Ordering::Greater
            } else {
                self.loop_priority(left)
                    .cmp(&self.loop_priority(right))
                    .then_with(|| left.updated_at.cmp(&right.updated_at))
                    .then_with(|| left.loop_id.cmp(&right.loop_id))
            }
        });
        let pending: Vec<NotificationRecord> = self
            .projection
            .notifications
            .values()
            .filter(|notification| notification.status == "pending")
            .cloned()
            .collect();
        let emitted: Vec<NotificationRecord> = self
            .projection
            .notifications
            .values()
            .filter(|notification| notification.status == "emitted")
            .cloned()
            .collect();
        let unknown: Vec<NotificationRecord> = self
            .projection
            .notifications
            .values()
            .filter(|notification| notification.status == "unknown")
            .cloned()
            .collect();
        json!({
            "protocol": PROTOCOL,
            "mailboxPath": self.mailbox_path,
            "scopes": self.projection.scopes.values().collect::<Vec<_>>(),
            "agents": self.projection.agents.values().collect::<Vec<_>>(),
            "agentTombstones": self.projection.agent_tombstones.values().collect::<Vec<_>>(),
            "discoveryPending": self.projection.discovery_pending.values().collect::<Vec<_>>(),
            "messages": self.projection.messages.values().collect::<Vec<_>>(),
            "messageDeliveryAttempts": self
                .projection
                .message_delivery_attempts
                .values()
                .collect::<Vec<_>>(),
            "adapters": self.projection.adapters.values().collect::<Vec<_>>(),
            "activeBugs": active_bugs,
            "bugs": self.projection.bugs.values().collect::<Vec<_>>(),
            "loops": loops,
            "wakeup": self.projection.wakeup.values().collect::<Vec<_>>(),
            "masterWake": self.projection.master_wake.values().collect::<Vec<_>>(),
            "notificationProjection": {
                "pending": pending,
                "emitted": emitted,
                "unknown": unknown,
                "batches": self.projection.batches
            }
        })
    }

    fn loop_priority(&self, loop_record: &LoopRecord) -> Priority {
        loop_record
            .loop_id
            .strip_prefix("bug-loop-")
            .and_then(|bug_id| self.projection.bugs.get(bug_id))
            .map(|bug| bug.priority.clone())
            .unwrap_or(Priority::P3)
    }

    fn validate_project_root(&self, project_root: &str) -> CommResult<()> {
        let requested = Path::new(project_root);
        if !is_lexically_canonical_absolute(requested) {
            return Err(CommError::new(
                "project_root_not_canonical",
                format!("projectRoot must be an absolute canonical path: {project_root}"),
            ));
        }
        reject_symlink_components(requested, "communication_project_root")?;
        if requested != self.project_root {
            return Err(CommError::new(
                "project_root_mismatch",
                format!(
                    "projectRoot does not match communication root: expected {}, got {}",
                    self.project_root.display(),
                    requested.display()
                ),
            ));
        }
        Ok(())
    }

    fn register_runtime(&mut self, request: RuntimeRequest) -> CommResult<Value> {
        self.validate_project_root(&request.project_root)?;
        let identity = global_registry::RuntimeIdentity {
            runtime_id: request.runtime_id,
            appserver_id: request.appserver_id,
            namespace: request.namespace,
            endpoint: request.endpoint,
            project_root: request.project_root,
            capabilities: request.capabilities,
            tmux_session: request.tmux_session,
            tmux_pane: request.tmux_pane,
            process_id: request.process_id,
        };
        let receipt = global_registry::register_runtime(&identity).map_err(|error| {
            CommError::new(
                if error.starts_with("GLOBAL_RUNTIME_IDENTITY_CONFLICT:") {
                    "runtime_identity_conflict"
                } else if error.starts_with("GLOBAL_RUNTIME_NOT_FOUND:") {
                    "runtime_not_found"
                } else {
                    "runtime_registration_failed"
                },
                error,
            )
        })?;
        Ok(json!({
            "runtime": identity,
            "receipt": serde_json::to_value(receipt).unwrap(),
            "registry": "host"
        }))
    }

    fn require_runtime_for_scope(
        &self,
        request: &ScopeRequest,
    ) -> CommResult<global_registry::RuntimeRecord> {
        let runtime_id = request.runtime_id.as_deref().ok_or_else(|| {
            CommError::new(
                "runtime_registration_required",
                "scope registration requires a host runtimeId registered in ~/.appsdk",
            )
        })?;
        let runtime = global_registry::runtime(runtime_id)
            .map_err(|error| CommError::new("runtime_registration_required", error))?;
        if runtime.identity.appserver_id != request.appserver_id
            || runtime.identity.namespace != request.namespace
            || runtime.identity.endpoint != request.endpoint
            || runtime.identity.project_root != request.project_root
        {
            return Err(CommError::new(
                "runtime_scope_mismatch",
                format!(
                    "runtime {} does not match scope transport identity",
                    runtime_id
                ),
            ));
        }
        Ok(runtime)
    }

    fn require_runtime_for_agent(
        &self,
        scope: &ScopeRecord,
        runtime_id: Option<&str>,
    ) -> CommResult<global_registry::RuntimeRecord> {
        let expected = scope.runtime_id.as_deref().ok_or_else(|| {
            CommError::new(
                "runtime_registration_required",
                "agent registration requires a scope bound to a host runtime",
            )
        })?;
        let runtime_id = runtime_id.ok_or_else(|| {
            CommError::new(
                "runtime_registration_required",
                "agent registration requires runtimeId",
            )
        })?;
        if runtime_id != expected {
            return Err(CommError::new(
                "runtime_agent_mismatch",
                format!("agent runtimeId does not match scope runtimeId: {runtime_id}"),
            ));
        }
        global_registry::runtime(runtime_id)
            .map_err(|error| CommError::new("runtime_registration_required", error))
    }

    fn runtime_for_agent(&self, agent: &AgentRecord) -> CommResult<global_registry::RuntimeRecord> {
        let runtime_id = agent.runtime_id.as_deref().ok_or_else(|| {
            CommError::new(
                "runtime_registration_required",
                format!(
                    "agent has no verified runtime identity: {}",
                    agent.address().key()
                ),
            )
        })?;
        global_registry::runtime(runtime_id)
            .map_err(|error| CommError::new("runtime_registration_required", error))
    }

    fn require_agent_runtime(&self, agent: &AgentRecord) -> CommResult<()> {
        self.runtime_for_agent(agent).map(|_| ())
    }

    fn global_address(address: &Address) -> global_registry::CommunicationAddress {
        global_registry::CommunicationAddress {
            scope_id: address.scope_id.clone(),
            session_id: address.session_id.clone(),
        }
    }

    fn discovery_registration_error(error: String) -> CommError {
        CommError::new(
            "communication_discovery_registration_failed",
            format!("global communication discovery registration failed: {error}"),
        )
    }

    fn discovery_pending_error(mut error: CommError, pending_id: &str) -> CommError {
        let cause = error.context;
        error.context = json!({
            "pendingId": pending_id,
            "cause": cause
        });
        error
    }

    fn discovery_recovery_error(
        pending: &DiscoveryPendingRecord,
        error: impl Into<String>,
    ) -> CommError {
        let mut failure = CommError::new(
            "communication_discovery_recovery_failed",
            format!(
                "unable to reconcile host communication discovery for pending operation {}: {}",
                pending.pending_id,
                error.into()
            ),
        );
        failure.context = json!({
            "pendingId": &pending.pending_id,
            "operation": &pending.operation,
        });
        failure
    }

    fn begin_discovery(&mut self, operation: DiscoveryOperation) -> CommResult<String> {
        let pending = DiscoveryPendingRecord {
            pending_id: new_id("discovery"),
            operation,
            created_at: now(),
        };
        let pending_id = pending.pending_id.clone();
        self.commit(
            "discovery.pending",
            serde_json::to_value(&pending).expect("discovery pending record is serializable"),
        )?;
        Ok(pending_id)
    }

    fn publish_discovery(&self, operation: &DiscoveryOperation) -> Result<(), String> {
        match operation {
            DiscoveryOperation::Scope { record } => {
                global_registry::register_communication_scope(
                    &record.scope_id,
                    Path::new(&record.project_root),
                )?;
            }
            DiscoveryOperation::Agent { record } => {
                let scope = self
                    .projection
                    .scopes
                    .get(&record.scope_id)
                    .ok_or_else(|| "scope is missing while publishing agent".to_string())?;
                global_registry::register_communication_agent(
                    &record.scope_id,
                    &record.session_id,
                    Path::new(&scope.project_root),
                )?;
            }
            DiscoveryOperation::Rebind { event } => {
                let scope = self
                    .projection
                    .scopes
                    .get(&event.to.scope_id)
                    .ok_or_else(|| "scope is missing while publishing rebind".to_string())?;
                global_registry::rebind_communication_agent(
                    &event.from.scope_id,
                    &event.from.session_id,
                    &event.to.session_id,
                    Path::new(&scope.project_root),
                )?;
            }
        }
        Ok(())
    }

    fn finish_discovery(&mut self, pending_id: &str) -> CommResult<()> {
        if !self.projection.discovery_pending.contains_key(pending_id) {
            return Err(CommError::new(
                "discovery_pending_missing",
                format!("discovery pending operation is missing: {pending_id}"),
            ));
        }
        self.commit("discovery.reconciled", json!({ "pendingId": pending_id }))
            .map(|_| ())
    }

    fn publish_and_finish(
        &mut self,
        pending_id: &str,
        operation: &DiscoveryOperation,
    ) -> CommResult<()> {
        if let Err(error) = self.publish_discovery(operation) {
            return Err(Self::discovery_pending_error(
                Self::discovery_registration_error(error),
                pending_id,
            ));
        }
        self.finish_discovery(pending_id)
            .map_err(|error| Self::discovery_pending_error(error, pending_id))
    }

    fn ensure_local_discovery_operation(
        &mut self,
        operation: &DiscoveryOperation,
    ) -> CommResult<()> {
        match operation {
            DiscoveryOperation::Scope { record } => {
                match self.projection.scopes.get(&record.scope_id) {
                    None => {
                        self.commit(
                            "scope.registered",
                            serde_json::to_value(record).expect("scope record is serializable"),
                        )?;
                    }
                    Some(existing) if existing == record => {}
                    Some(_) => {
                        return Err(CommError::new(
                            "discovery_recovery_conflict",
                            format!(
                                "scope changed while discovery was pending: {}",
                                record.scope_id
                            ),
                        ));
                    }
                }
            }
            DiscoveryOperation::Agent { record } => {
                let key = record.address().key();
                match self.projection.agents.get(&key) {
                    None if self.projection.agent_tombstones.contains_key(&key) => {
                        return Err(CommError::new(
                            "discovery_recovery_conflict",
                            format!("agent address is already rebound: {key}"),
                        ));
                    }
                    None => {
                        self.require_scope(&record.scope_id)?;
                        self.commit("agent.registered", json!({ "agent": record }))?;
                    }
                    Some(existing) if existing == record => {}
                    Some(_) => {
                        return Err(CommError::new(
                            "discovery_recovery_conflict",
                            format!("agent changed while discovery was pending: {key}"),
                        ));
                    }
                }
            }
            DiscoveryOperation::Rebind { event } => {
                let from_key = event.from.address().key();
                let to_key = event.to.address().key();
                let local_rebound = self
                    .projection
                    .agents
                    .get(&to_key)
                    .is_some_and(|agent| agent == &event.to)
                    && self
                        .projection
                        .agent_tombstones
                        .get(&from_key)
                        .is_some_and(|tombstone| tombstone == &event.tombstone);
                if local_rebound {
                    return Ok(());
                }
                if self.projection.agents.get(&from_key) != Some(&event.from)
                    || self.projection.agents.contains_key(&to_key)
                    || self.projection.agent_tombstones.contains_key(&from_key)
                    || self.projection.agent_tombstones.contains_key(&to_key)
                {
                    return Err(CommError::new(
                        "discovery_recovery_conflict",
                        format!(
                            "agent rebind state changed while discovery was pending: {from_key}"
                        ),
                    ));
                }
                self.commit(
                    "agent.rebound",
                    serde_json::to_value(event).expect("agent rebound event is serializable"),
                )?;
            }
        }
        Ok(())
    }

    fn reconcile_discovery_pending(&mut self) -> CommResult<()> {
        let pending: Vec<DiscoveryPendingRecord> = self
            .projection
            .discovery_pending
            .values()
            .cloned()
            .collect();
        for record in pending {
            if let Err(error) = self.ensure_local_discovery_operation(&record.operation) {
                return Err(Self::discovery_recovery_error(&record, error.to_string()));
            }
            if let Err(error) = self.publish_discovery(&record.operation) {
                return Err(Self::discovery_recovery_error(&record, error));
            }
            self.finish_discovery(&record.pending_id)
                .map_err(|error| Self::discovery_recovery_error(&record, error.to_string()))?;
        }
        Ok(())
    }

    fn discovery_lookup_error(error: String) -> CommError {
        CommError::new(
            "communication_discovery_failed",
            format!("global communication discovery lookup failed: {error}"),
        )
    }

    fn target_mailbox_root(target: &global_registry::CommunicationTarget) -> CommResult<PathBuf> {
        let mailbox = target
            .project_root
            .join(".appsdk-control/communication/mailbox.jsonl");
        if !mailbox.is_file() {
            return Err(CommError::new(
                "communication_target_mailbox_missing",
                format!(
                    "registered target mailbox is missing: {}",
                    mailbox.display()
                ),
            ));
        }
        Ok(target.project_root.clone())
    }

    fn resolve_agent(&self, address: &Address) -> CommResult<AgentRecord> {
        let key = address.key();
        if self.projection.agents.contains_key(&key)
            || self.projection.agent_tombstones.contains_key(&key)
        {
            return self.require_agent(address).cloned();
        }

        let target = global_registry::communication_target(&Self::global_address(address))
            .map_err(Self::discovery_lookup_error)?
            .ok_or_else(|| {
                CommError::new(
                    "agent_not_registered",
                    format!("agent not registered: {}", address.key()),
                )
            })?;
        if let Some(rebound_from) = target.rebound_from.as_ref() {
            let mut error = CommError::new(
                "agent_address_rebound",
                format!("agent address was rebound to {}", target.address.session_id),
            );
            error.context = json!({
                "oldAddress": rebound_from,
                "newAddress": target.address,
                "projectRoot": target.project_root,
            });
            return Err(error);
        }
        let project_root = Self::target_mailbox_root(&target)?;
        if project_root == self.project_root {
            return Err(CommError::new(
                "agent_not_registered",
                format!("agent not registered: {}", address.key()),
            ));
        }
        let external = Self::open_mailbox_read_only(
            project_root.join(".appsdk-control/communication/mailbox.jsonl"),
        )
        .map_err(|mut error| {
            error.context = json!({
                "targetAddress": address,
                "targetProjectRoot": project_root,
                "cause": error.context,
            });
            error
        })?;
        let external_address = Address {
            scope_id: target.address.scope_id.clone(),
            session_id: target.address.session_id.clone(),
        };
        external.require_agent(&external_address).cloned()
    }

    fn resolve_live_agent(&self, address: &Address) -> CommResult<AgentRecord> {
        let agent = self.resolve_agent(address)?;
        if !agent.live_at(&now()) {
            return Err(CommError::new(
                "agent_lease_expired",
                format!("agent lease expired: {}", address.key()),
            ));
        }
        Ok(agent)
    }

    fn resolve_scope_for_agent(&self, agent: &AgentRecord) -> CommResult<ScopeRecord> {
        if let Some(scope) = self.projection.scopes.get(&agent.scope_id) {
            return Ok(scope.clone());
        }
        let target = global_registry::communication_target(&Self::global_address(&agent.address()))
            .map_err(Self::discovery_lookup_error)?
            .ok_or_else(|| {
                CommError::new(
                    "scope_not_found",
                    format!("scope not found: {}", agent.scope_id),
                )
            })?;
        if target.rebound_from.is_some() {
            return Err(CommError::new(
                "agent_address_rebound",
                format!("agent address was rebound: {}", agent.address().key()),
            ));
        }
        let project_root = Self::target_mailbox_root(&target)?;
        if project_root == self.project_root {
            return Err(CommError::new(
                "scope_not_found",
                format!("scope not found: {}", agent.scope_id),
            ));
        }
        let external = Self::open_mailbox_read_only(
            project_root.join(".appsdk-control/communication/mailbox.jsonl"),
        )
        .map_err(|mut error| {
            error.context = json!({
                "targetAddress": agent.address(),
                "targetProjectRoot": project_root,
                "cause": error.context,
            });
            error
        })?;
        external.require_scope(&agent.scope_id).cloned()
    }

    fn register_adapter(&mut self, request: AdapterRequest) -> CommResult<Value> {
        validate_non_empty(&request.adapter_id, "adapterId")?;
        if !matches!(request.kind.as_str(), "mailbox" | "tmux" | "appserver") {
            return Err(CommError::new(
                "invalid_adapter_kind",
                format!(
                    "adapter kind must be mailbox, tmux or appserver: {}",
                    request.kind
                ),
            ));
        }
        if request.kind == "tmux" && request.target.as_deref().unwrap_or("").trim().is_empty() {
            return Err(CommError::new(
                "tmux_target_required",
                "tmux adapter requires target pane",
            ));
        }
        if request.kind == "appserver" && request.target.as_deref().unwrap_or("").trim().is_empty()
        {
            return Err(CommError::new(
                "appserver_target_required",
                "appserver adapter requires endpoint",
            ));
        }
        if request.kind != "mailbox" && request.recipient.is_none() {
            return Err(CommError::new(
                "adapter_recipient_required",
                "tmux and appserver adapters require a registered recipient address",
            ));
        }
        if let Some(recipient) = request.recipient.as_ref() {
            let agent = self.resolve_live_agent(recipient)?;
            if request.kind != "mailbox" {
                let runtime = self.runtime_for_agent(&agent)?;
                let target = request.target.as_deref().ok_or_else(|| {
                    CommError::new(
                        if request.kind == "tmux" {
                            "tmux_target_required"
                        } else {
                            "appserver_target_required"
                        },
                        if request.kind == "tmux" {
                            "tmux adapter requires target pane"
                        } else {
                            "appserver adapter requires endpoint"
                        },
                    )
                })?;
                validate_adapter_runtime_target(&request.kind, target, &runtime, false)?;
            }
        }
        let record = AdapterRecord {
            adapter_id: request.adapter_id.clone(),
            kind: request.kind,
            target: request.target,
            enabled: request.enabled.unwrap_or(true),
            execute: request.execute.unwrap_or(false),
            recipient: request.recipient,
            registered_at: now(),
        };
        if let Some(existing) = self.projection.adapters.get(&record.adapter_id) {
            if existing.kind == record.kind
                && existing.target == record.target
                && existing.enabled == record.enabled
                && existing.execute == record.execute
                && existing.recipient == record.recipient
            {
                return Ok(json!({ "adapter": existing, "idempotent": true }));
            }
            return Err(CommError::new(
                "adapter_conflict",
                format!("adapter already registered: {}", record.adapter_id),
            ));
        }
        self.commit("adapter.registered", serde_json::to_value(&record).unwrap())?;
        Ok(json!({ "adapter": record, "idempotent": false }))
    }

    fn register_scope(&mut self, request: ScopeRequest) -> CommResult<Value> {
        validate_scope_request(&request)?;
        self.validate_project_root(&request.project_root)?;
        let _runtime = self.require_runtime_for_scope(&request)?;
        let at = now();
        if let Some(existing) = self.projection.scopes.get(&request.scope_id).cloned() {
            if existing.appserver_id == request.appserver_id
                && existing.namespace == request.namespace
                && existing.endpoint == request.endpoint
                && existing.project_root == request.project_root
                && existing.session_ids == request.session_ids
                && existing.runtime_id.as_deref() == request.runtime_id.as_deref()
            {
                let operation = DiscoveryOperation::Scope {
                    record: existing.clone(),
                };
                let pending_id = self.begin_discovery(operation.clone())?;
                self.publish_and_finish(&pending_id, &operation)?;
                return Ok(json!({ "scope": existing, "idempotent": true }));
            }
            return Err(CommError::new(
                "scope_conflict",
                format!(
                    "scope already registered with different identity: {}",
                    request.scope_id
                ),
            ));
        }
        let record = ScopeRecord {
            scope_id: request.scope_id.clone(),
            appserver_id: request.appserver_id,
            namespace: request.namespace,
            endpoint: request.endpoint,
            project_root: request.project_root,
            session_ids: request.session_ids,
            registered_at: at.clone(),
            last_observed_at: at,
            master_session_id: None,
            runtime_id: request.runtime_id,
        };
        let operation = DiscoveryOperation::Scope {
            record: record.clone(),
        };
        let pending_id = self.begin_discovery(operation.clone())?;
        if let Err(error) = self.commit("scope.registered", serde_json::to_value(&record).unwrap())
        {
            return Err(Self::discovery_pending_error(error, &pending_id));
        }
        self.publish_and_finish(&pending_id, &operation)?;
        Ok(json!({ "scope": record, "idempotent": false }))
    }

    fn register_agent(&mut self, request: AgentRequest) -> CommResult<Value> {
        validate_non_empty(&request.scope_id, "scopeId")?;
        validate_non_empty(&request.session_id, "sessionId")?;
        validate_non_empty(&request.agent_id, "agentId")?;
        let scope = self.require_scope(&request.scope_id)?.clone();
        let role = request.role.clone().unwrap_or_else(|| "peer".into());
        if role == "auto" {
            return Err(CommError::new(
                "role_auto_forbidden",
                "agent role auto is forbidden; register peer or provide explicit masterGrant",
            ));
        }
        if !matches!(role.as_str(), "master" | "peer" | "subagent") {
            return Err(CommError::new(
                "invalid_agent_role",
                format!("unsupported agent role: {role}"),
            ));
        }
        self.require_runtime_for_agent(&scope, request.runtime_id.as_deref())?;
        if !scope.session_ids.is_empty() && !scope.session_ids.contains(&request.session_id) {
            return Err(CommError::new(
                "session_not_declared",
                format!(
                    "session is not declared in scope: {}/{}",
                    request.scope_id, request.session_id
                ),
            ));
        }
        if role == "master"
            && request
                .master_grant
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            return Err(CommError::new(
                "master_grant_required",
                "master registration requires non-empty user masterGrant",
            ));
        }
        if role == "subagent" {
            let parent = request.parent.as_ref().ok_or_else(|| {
                CommError::new(
                    "subagent_parent_required",
                    "subagent registration requires parent",
                )
            })?;
            let parent_agent = self.require_agent(parent)?.clone();
            if parent.scope_id != request.scope_id {
                return Err(CommError::new(
                    "subagent_parent_scope_mismatch",
                    "subagent parent must be in the same scope",
                ));
            }
            if !parent_agent.live_at(&now()) {
                return Err(CommError::new(
                    "subagent_parent_expired",
                    "subagent parent lease is expired",
                ));
            }
        } else if request.parent.is_some() {
            return Err(CommError::new(
                "parent_only_for_subagent",
                "parent is only valid for a subagent",
            ));
        }
        let key = Address {
            scope_id: request.scope_id.clone(),
            session_id: request.session_id.clone(),
        }
        .key();
        if self.projection.agent_tombstones.contains_key(&key) {
            let mut error = CommError::new(
                "agent_address_rebound",
                format!("agent address is a read-only rebound tombstone: {key}"),
            );
            error.context = json!({
                "oldAddress": {
                    "scopeId": request.scope_id,
                    "sessionId": request.session_id
                },
                "tombstone": self.projection.agent_tombstones.get(&key)
            });
            return Err(error);
        }
        if let Some(existing) = self.projection.agents.get(&key).cloned() {
            if existing.role == role
                && existing.agent_id == request.agent_id
                && existing.parent == request.parent
                && existing.runtime_id.as_deref() == request.runtime_id.as_deref()
            {
                let operation = DiscoveryOperation::Agent {
                    record: existing.clone(),
                };
                let pending_id = self.begin_discovery(operation.clone())?;
                self.publish_and_finish(&pending_id, &operation)?;
                let reconciled_idle = if existing.role == "master" {
                    self.reconcile_idle_workers_for_master(&existing)?
                } else {
                    Vec::new()
                };
                return Ok(json!({
                    "agent": existing,
                    "idempotent": true,
                    "reconciledIdle": reconciled_idle
                }));
            }
            return Err(CommError::new(
                "agent_conflict",
                format!("agent address already registered: {key}"),
            ));
        }
        if role == "master"
            && scope
                .master_session_id
                .as_deref()
                .is_some_and(|master| master != request.session_id)
        {
            return Err(CommError::new(
                "master_already_registered",
                format!(
                    "scope already has a master: {}",
                    scope.master_session_id.unwrap()
                ),
            ));
        }
        let at = now();
        let lease_ms = request
            .lease_ms
            .unwrap_or(DEFAULT_AGENT_LEASE_MS)
            .max(1_000);
        let expires_at = add_millis(&at, lease_ms as i64)?;
        let record = AgentRecord {
            scope_id: request.scope_id.clone(),
            session_id: request.session_id.clone(),
            agent_id: request.agent_id,
            role,
            master_grant: request.master_grant,
            parent: request.parent,
            lease_ms,
            registered_at: at.clone(),
            last_observed_at: at.clone(),
            expires_at,
            state: AgentState::Working,
            last_state_at: at,
            runtime_id: request.runtime_id,
        };
        let operation = DiscoveryOperation::Agent {
            record: record.clone(),
        };
        let pending_id = self.begin_discovery(operation.clone())?;
        if let Err(error) = self.commit("agent.registered", serde_json::to_value(&record).unwrap())
        {
            return Err(Self::discovery_pending_error(error, &pending_id));
        }
        self.publish_and_finish(&pending_id, &operation)?;
        let reconciled_idle = if record.role == "master" {
            self.reconcile_idle_workers_for_master(&record)?
        } else {
            Vec::new()
        };
        Ok(json!({
            "agent": record,
            "idempotent": false,
            "reconciledIdle": reconciled_idle
        }))
    }

    fn reconcile_idle_workers_for_master(
        &mut self,
        master: &AgentRecord,
    ) -> CommResult<Vec<Value>> {
        let observed_at = now();
        if !master.live_at(&observed_at) {
            return Ok(Vec::new());
        }
        let workers: Vec<AgentRecord> = self
            .projection
            .agents
            .values()
            .filter(|agent| {
                agent.scope_id == master.scope_id
                    && agent.role != "master"
                    && agent.state == AgentState::Idle
                    && agent.live_at(&observed_at)
            })
            .cloned()
            .collect();
        let master_address = master.address();
        let mut reconciled = Vec::new();
        for worker in workers {
            let signal_key = worker_idle_signal_key(&worker.address());
            let message =
                worker_idle_message(&worker, master_address.clone(), &worker.last_state_at);
            let message_id = message
                .message_id
                .as_deref()
                .expect("worker idle message must have a deterministic message id");
            let notification_key = structured_key(&[
                &worker.address().key(),
                &master_address.key(),
                "mailbox",
                &format!("idle:{}", worker.address().key()),
            ]);
            let signal_recorded =
                self.master_wake_signal_recorded(&master_address, &signal_key, message_id);
            let signal_consumed =
                self.master_wake_signal_consumed(&master_address, &signal_key, message_id);
            let notification_matches_message = self
                .projection
                .notifications
                .get(&notification_key)
                .is_some_and(|notification| notification.message_id == message_id);
            if !signal_recorded {
                self.accumulate_worker_idle(&worker, &master_address, &worker.last_state_at)?;
            }
            if !signal_consumed
                && (!self.projection.messages.contains_key(message_id)
                    || !notification_matches_message)
            {
                // send() persists the deterministic message and its
                // notification.  It is safe to call after a partial prefix:
                // messageId and the idle edge are the idempotency boundary.
                reconciled.push(self.send(message)?);
            }
        }
        Ok(reconciled)
    }

    pub fn refresh_agent(&mut self, address: Address, at: Option<&str>) -> CommResult<Value> {
        let at = at.map(validate_time).transpose()?.unwrap_or_else(now);
        let current = self.require_agent(&address)?.clone();
        let refreshed = AgentRecord {
            last_observed_at: at.clone(),
            expires_at: add_millis(&at, current.lease_ms as i64)?,
            ..current
        };
        self.commit("agent.refreshed", serde_json::to_value(&refreshed).unwrap())?;
        Ok(json!({ "agent": refreshed, "observedAt": at }))
    }

    fn rebind_agent(&mut self, request: RebindAgentRequest) -> CommResult<Value> {
        validate_address(&request.from)?;
        validate_address(&request.to)?;
        validate_non_empty(&request.runtime_id, "runtimeId")?;
        if request.from.scope_id != request.to.scope_id {
            return Err(CommError::new(
                "agent_rebind_scope_mismatch",
                "agent rebind must keep the same scope",
            ));
        }
        if request.from == request.to {
            return Err(CommError::new(
                "agent_address_occupied",
                "agent rebind target must use a new session address",
            ));
        }

        let current = self.require_live_agent(&request.from)?.clone();
        if current.runtime_id.as_deref() != Some(request.runtime_id.as_str()) {
            let mut error = CommError::new(
                "agent_rebind_runtime_mismatch",
                format!(
                    "agent runtimeId does not match rebind runtimeId: {}",
                    request.runtime_id
                ),
            );
            error.context = json!({
                "address": request.from,
                "agentId": current.agent_id,
                "agentRuntimeId": current.runtime_id,
                "requestedRuntimeId": request.runtime_id
            });
            return Err(error);
        }
        let scope = self.require_scope(&request.from.scope_id)?.clone();
        self.require_runtime_for_agent(&scope, Some(request.runtime_id.as_str()))?;
        if !scope.session_ids.is_empty() && !scope.session_ids.contains(&request.to.session_id) {
            return Err(CommError::new(
                "session_not_declared",
                format!(
                    "session is not declared in scope: {}/{}",
                    request.to.scope_id, request.to.session_id
                ),
            ));
        }
        if self.projection.agents.contains_key(&request.to.key())
            || self
                .projection
                .agent_tombstones
                .contains_key(&request.to.key())
        {
            let mut error = CommError::new(
                "agent_address_occupied",
                format!(
                    "agent rebind target is already occupied: {}",
                    request.to.key()
                ),
            );
            error.context = json!({ "address": request.to });
            return Err(error);
        }
        if current.role == "master"
            && scope.master_session_id.as_deref() != Some(current.session_id.as_str())
        {
            return Err(CommError::new(
                "master_registration_state_invalid",
                "registered master address does not match the scope master session",
            ));
        }

        let at = request
            .at
            .map(|value| validate_time(&value))
            .transpose()?
            .unwrap_or_else(now);
        let rebound = AgentRecord {
            session_id: request.to.session_id.clone(),
            last_observed_at: at.clone(),
            expires_at: add_millis(&at, current.lease_ms as i64)?,
            ..current.clone()
        };
        let tombstone = AgentTombstone {
            address: request.from.clone(),
            agent_id: current.agent_id.clone(),
            runtime_id: request.runtime_id,
            rebound_to: rebound.address(),
            rebound_at: at,
        };
        let event = AgentReboundEvent {
            from: current,
            to: rebound.clone(),
            tombstone: tombstone.clone(),
        };
        let operation = DiscoveryOperation::Rebind {
            event: event.clone(),
        };
        let pending_id = self.begin_discovery(operation.clone())?;
        if let Err(error) = self.commit("agent.rebound", serde_json::to_value(&event).unwrap()) {
            return Err(Self::discovery_pending_error(error, &pending_id));
        }
        self.publish_and_finish(&pending_id, &operation)?;
        Ok(json!({
            "agent": rebound,
            "tombstone": tombstone,
            "idempotent": false
        }))
    }

    fn send(&mut self, request: MessageRequest) -> CommResult<Value> {
        validate_message_request(&request)?;
        let source = self.require_live_agent(&request.from)?.clone();
        let target = self.resolve_live_agent(&request.to)?;
        self.require_agent_runtime(&source)?;
        self.require_agent_runtime(&target)?;
        let route = self.resolve_route(&source, &target)?;
        self.enqueue_message(request, route, None)
    }

    fn record_delivery(&mut self, request: DeliveryRequest) -> CommResult<Value> {
        validate_non_empty(&request.message_id, "messageId")?;
        validate_non_empty(&request.attempt_id, "attemptId")?;
        validate_non_empty(&request.nonce, "nonce")?;
        validate_non_empty(&request.runtime_id, "runtimeId")?;
        validate_non_empty(&request.state, "state")?;
        if request
            .evidence
            .as_object()
            .is_none_or(|evidence| evidence.is_empty())
        {
            return Err(CommError::new(
                "delivery_evidence_required",
                "delivery evidence must be a non-empty object",
            ));
        }
        let state = request.state.trim().to_ascii_lowercase();
        if !matches!(
            state.as_str(),
            "delivered" | "executed" | "replied" | "read" | "consumed" | "unknown"
        ) {
            return Err(CommError::new(
                "invalid_delivery_state",
                format!("unsupported delivery state: {}", request.state),
            ));
        }
        let message = self
            .projection
            .messages
            .get(&request.message_id)
            .cloned()
            .ok_or_else(|| {
                CommError::new(
                    "message_not_found",
                    format!("message not found: {}", request.message_id),
                )
            })?;
        let adapter = self.require_adapter(&message.adapter_id)?;
        validate_adapter_delivery_receipt(
            &adapter.kind,
            &request.evidence,
            &request.runtime_id,
            &state,
        )?;
        // An exact replay of an already committed receipt is idempotent even
        // when the runtime has since refreshed its volatile transport fields.
        // It creates no new fact; a new state still goes through the current
        // runtime and attempt validation below.
        if message.evidence.iter().any(|evidence| {
            evidence.state == state
                && evidence.details.get("runtimeId").and_then(Value::as_str)
                    == Some(request.runtime_id.as_str())
                && evidence.details.get("receipt") == Some(&request.evidence)
                && evidence.details.get("attemptId").and_then(Value::as_str)
                    == Some(request.attempt_id.as_str())
                && evidence.details.get("nonce").and_then(Value::as_str)
                    == Some(request.nonce.as_str())
        }) {
            return Ok(json!({ "message": message, "idempotent": true }));
        }
        let target = self.resolve_live_agent(&message.to)?;
        let target_runtime_id = target.runtime_id.as_deref().ok_or_else(|| {
            CommError::new(
                "runtime_registration_required",
                format!(
                    "message target has no runtime identity: {}",
                    message.to.key()
                ),
            )
        })?;
        if target_runtime_id != request.runtime_id {
            return Err(CommError::new(
                "delivery_runtime_mismatch",
                format!(
                    "delivery runtimeId does not match target runtime: {}",
                    request.runtime_id
                ),
            ));
        }
        let runtime = global_registry::runtime(&request.runtime_id)
            .map_err(|error| CommError::new("runtime_registration_required", error))?;
        let attempt = self
            .projection
            .message_delivery_attempts
            .get(&request.message_id)
            .cloned()
            .ok_or_else(|| {
                CommError::new(
                    "delivery_attempt_required",
                    format!(
                        "message has no persisted delivery attempt: {}",
                        request.message_id
                    ),
                )
            })?;
        validate_message_delivery_attempt(
            &attempt,
            &message,
            &target,
            &runtime,
            self.require_adapter(&message.adapter_id)?,
            false,
        )?;
        if attempt.attempt_id != request.attempt_id {
            return Err(CommError::new(
                "delivery_attempt_mismatch",
                format!(
                    "delivery attempt {} does not match persisted attempt {}",
                    request.attempt_id, attempt.attempt_id
                ),
            ));
        }
        if attempt.nonce != request.nonce {
            return Err(CommError::new(
                "delivery_attempt_nonce_mismatch",
                "delivery receipt nonce does not match persisted delivery attempt",
            ));
        }
        let at = request
            .observed_at
            .as_deref()
            .map(validate_time)
            .transpose()?
            .unwrap_or_else(now);
        let details = json!({
            "runtimeId": request.runtime_id,
            "runtimeFingerprint": attempt.runtime_fingerprint,
            "attemptId": request.attempt_id,
            "nonce": request.nonce,
            "adapterId": attempt.adapter_id,
            "target": attempt.target,
            "receipt": request.evidence
        });
        validate_delivery_state_transition(&message.state, &state)?;
        let evidence = DeliveryEvidence {
            state: state.clone(),
            at: at.clone(),
            details,
        };
        self.commit(
            "message.state",
            json!({
                "messageId": request.message_id,
                "state": state,
                "evidence": evidence,
                "attemptId": request.attempt_id,
                "nonce": request.nonce
            }),
        )?;
        Ok(json!({
            "message": self.projection.messages.get(&request.message_id),
            "idempotent": false,
            "observedAt": at
        }))
    }

    fn ensure_message_delivery_attempt(
        &mut self,
        message: &MessageRecord,
    ) -> CommResult<MessageDeliveryAttempt> {
        if let Some(existing) = self
            .projection
            .message_delivery_attempts
            .get(&message.message_id)
            .cloned()
        {
            let target = self.resolve_live_agent(&message.to)?;
            let runtime_id = target.runtime_id.as_deref().ok_or_else(|| {
                CommError::new(
                    "runtime_registration_required",
                    format!(
                        "message target has no runtime identity: {}",
                        message.to.key()
                    ),
                )
            })?;
            let runtime = global_registry::runtime(runtime_id)
                .map_err(|error| CommError::new("runtime_registration_required", error))?;
            validate_message_delivery_attempt(
                &existing,
                message,
                &target,
                &runtime,
                self.require_adapter(&message.adapter_id)?,
                false,
            )?;
            return Ok(existing);
        }

        let target = self.resolve_live_agent(&message.to)?;
        let runtime_id = target.runtime_id.clone().ok_or_else(|| {
            CommError::new(
                "runtime_registration_required",
                format!(
                    "message target has no runtime identity: {}",
                    message.to.key()
                ),
            )
        })?;
        let runtime = global_registry::runtime(&runtime_id)
            .map_err(|error| CommError::new("runtime_registration_required", error))?;
        let adapter = self.require_adapter(&message.adapter_id)?;
        let attempt = MessageDeliveryAttempt {
            attempt_id: new_id("attempt"),
            message_id: message.message_id.clone(),
            operation: "message.delivery".into(),
            adapter_id: message.adapter_id.clone(),
            runtime_id,
            runtime_fingerprint: runtime.fingerprint.clone(),
            target: adapter.target.clone(),
            nonce: new_id("nonce"),
            started_at: now(),
        };
        validate_message_delivery_attempt(&attempt, message, &target, &runtime, adapter, false)?;
        self.commit(
            "message.delivery_attempt",
            json!({
                "messageId": message.message_id,
                "attempt": attempt
            }),
        )?;
        Ok(self
            .projection
            .message_delivery_attempts
            .get(&message.message_id)
            .cloned()
            .expect("message delivery attempt committed"))
    }

    pub fn set_agent_state(
        &mut self,
        address: Address,
        state: &str,
        at: Option<&str>,
    ) -> CommResult<Value> {
        let next = AgentState::parse(state)?;
        let at = at.map(validate_time).transpose()?.unwrap_or_else(now);
        let current = self.require_agent(&address)?.clone();
        if current.state == next {
            if current.role == "master" {
                self.repair_master_wakeup(&current, &next)?;
                self.sync_master_wake_schedule(&current.address(), &next, &at)?;
            } else if current.role != "master" && next == AgentState::Idle {
                let master_address = self.scope_master_address(&current.scope_id)?;
                let signal_key = worker_idle_signal_key(&current.address());
                let message =
                    worker_idle_message(&current, master_address.clone(), &current.last_state_at);
                let message_id = message
                    .message_id
                    .as_deref()
                    .expect("worker idle message must have a deterministic message id");
                let signal_recorded =
                    self.master_wake_signal_recorded(&master_address, &signal_key, message_id);
                if !signal_recorded {
                    // The state edge was persisted before its wake signal (for example when
                    // the scope had no master). Repair only that missing step. A consumed edge
                    // remains consumed and must never be reactivated by observing idle again.
                    self.accumulate_worker_idle(&current, &master_address, &current.last_state_at)?;
                }
                if !self.master_wake_signal_consumed(&master_address, &signal_key, message_id) {
                    let notification_key = structured_key(&[
                        &current.address().key(),
                        &master_address.key(),
                        "mailbox",
                        &format!("idle:{}", current.address().key()),
                    ]);
                    let notification_matches_message = self
                        .projection
                        .notifications
                        .get(&notification_key)
                        .is_some_and(|notification| notification.message_id == message_id);
                    if !self.projection.messages.contains_key(message_id)
                        || !notification_matches_message
                    {
                        let notification = self.send(message)?;
                        return Ok(json!({
                            "agent": current,
                            "idempotent": true,
                            "notification": notification
                        }));
                    }
                }
            }
            return Ok(json!({
                "agent": current,
                "idempotent": true,
                "notification": Value::Null
            }));
        }
        self.commit(
            "agent.state",
            json!({ "address": address, "state": next, "at": at }),
        )?;

        if current.role == "master" {
            let wakeup = if next == AgentState::Idle {
                WakeupRecord {
                    address: current.address(),
                    idle_since: Some(at.clone()),
                    reminders_sent: 0,
                    next_due_at: Some(add_seconds(&at, DEFAULT_BATCH_WINDOW_SECONDS)?),
                    stopped: false,
                    last_reminder_at: None,
                }
            } else {
                WakeupRecord {
                    address: current.address(),
                    idle_since: None,
                    reminders_sent: 0,
                    next_due_at: None,
                    stopped: false,
                    last_reminder_at: None,
                }
            };
            self.commit("wakeup.updated", serde_json::to_value(&wakeup).unwrap())?;
            self.sync_master_wake_schedule(&current.address(), &next, &at)?;
            return Ok(json!({
                "agent": self.require_agent(&address)?,
                "idempotent": false,
                "notification": Value::Null
            }));
        }

        if next == AgentState::Idle {
            let master_address = self.scope_master_address(&current.scope_id)?;
            self.accumulate_worker_idle(&current, &master_address, &at)?;
            let message = worker_idle_message(&current, master_address, &at);
            let notification = match self.send(message) {
                Ok(value) => value,
                Err(error) => return Err(error),
            };
            return Ok(json!({
                "agent": self.require_agent(&address)?,
                "idempotent": false,
                "notification": notification
            }));
        }

        if next == AgentState::Working {
            self.clear_worker_idle(&current.address(), &at)?;
        }

        Ok(json!({
            "agent": self.require_agent(&address)?,
            "idempotent": false,
            "notification": Value::Null
        }))
    }

    fn accumulate_worker_idle(
        &mut self,
        worker: &AgentRecord,
        master: &Address,
        at: &str,
    ) -> CommResult<MasterWakeAccumulator> {
        let source = worker.address();
        let key = worker_idle_signal_key(&source);
        let signal = MasterWakeSignal {
            signal_id: worker_idle_message_id(worker, at),
            key,
            kind: "worker_idle".into(),
            title: format!("worker idle: {}", worker.agent_id),
            priority: Priority::P2,
            summary: format!(
                "{} entered idle; inspect the JSONL facts for its latest result",
                worker.agent_id
            ),
            issue_id: None,
            source: Some(source),
            observed_at: validate_time(at)?,
            direct_dispatched: false,
        };
        self.accumulate_master_wake(master, signal)
    }

    fn clear_worker_idle(&mut self, worker: &Address, at: &str) -> CommResult<()> {
        let Some(master) = self.scope_master_address_unchecked(&worker.scope_id) else {
            return Ok(());
        };
        self.clear_master_wake_signal(&master, &worker_idle_signal_key(worker), at)
    }

    fn accumulate_master_wake(
        &mut self,
        master: &Address,
        signal: MasterWakeSignal,
    ) -> CommResult<MasterWakeAccumulator> {
        validate_address(master)?;
        validate_non_empty(&signal.signal_id, "signalId")?;
        validate_non_empty(&signal.key, "key")?;
        validate_non_empty(&signal.kind, "kind")?;
        validate_non_empty(&signal.title, "title")?;
        validate_non_empty(&signal.summary, "summary")?;
        if signal.title.chars().count() > 200 {
            return Err(CommError::new(
                "master_wake_title_too_long",
                "master wake signal title must be at most 200 characters",
            ));
        }
        let master_agent = self.require_agent(master)?.clone();
        if master_agent.role != "master"
            || self
                .projection
                .scopes
                .get(&master.scope_id)
                .and_then(|scope| scope.master_session_id.as_deref())
                != Some(master.session_id.as_str())
        {
            return Err(CommError::new(
                "master_wake_target_required",
                "master wake accumulator requires the registered scope master",
            ));
        }

        let observed_at = validate_time(&signal.observed_at)?;
        let key = master.key();
        let existing = self.projection.master_wake.get(&key).cloned();
        if let Some(existing_signal) = existing
            .as_ref()
            .and_then(|accumulator| accumulator.signals.get(&signal.key))
        {
            if master_wake_signal_identity_matches(existing_signal, &signal) {
                return Ok(existing.expect("master wake accumulator exists"));
            }
            if existing_signal.signal_id == signal.signal_id {
                return Err(CommError::new(
                    "master_wake_signal_conflict",
                    format!("master wake signal id conflicts: {}", signal.signal_id),
                ));
            }
        }
        if let Some(existing_signal) = existing
            .as_ref()
            .and_then(|accumulator| accumulator.consumed_signals.get(&signal.key))
        {
            if master_wake_signal_identity_matches(existing_signal, &signal) {
                return Ok(existing.expect("master wake accumulator exists"));
            }
            if existing_signal.signal_id == signal.signal_id {
                return Err(CommError::new(
                    "master_wake_signal_conflict",
                    format!(
                        "consumed master wake signal id conflicts: {}",
                        signal.signal_id
                    ),
                ));
            }
        }

        let mut accumulator = existing.unwrap_or_else(|| MasterWakeAccumulator {
            address: master.clone(),
            generation: 0,
            pending: false,
            first_observed_at: None,
            last_observed_at: None,
            next_due_at: None,
            reminders_sent: 0,
            stopped: false,
            last_briefing_generation: None,
            last_briefing_at: None,
            held: false,
            signals: BTreeMap::new(),
            consumed_signals: BTreeMap::new(),
        });
        let cycle_start = if accumulator.pending {
            accumulator
                .first_observed_at
                .clone()
                .unwrap_or_else(|| observed_at.clone())
        } else {
            observed_at.clone()
        };
        accumulator.generation = accumulator.generation.checked_add(1).ok_or_else(|| {
            CommError::new(
                "master_wake_generation_exhausted",
                format!("master wake generation exhausted: {}", master.key()),
            )
        })?;
        accumulator.first_observed_at = Some(cycle_start.clone());
        accumulator.last_observed_at = Some(observed_at.clone());
        accumulator
            .signals
            .insert(signal.key.clone(), signal.clone());
        accumulator.consumed_signals.remove(&signal.key);
        accumulator.pending = accumulator
            .signals
            .values()
            .any(|value| !value.direct_dispatched);
        accumulator.reminders_sent = 0;
        accumulator.stopped = false;
        accumulator.held = false;
        accumulator.last_briefing_generation = None;
        accumulator.last_briefing_at = None;
        accumulator.next_due_at = if accumulator.pending {
            if signal.priority.is_breakthrough() {
                Some(observed_at)
            } else {
                Some(add_seconds(&cycle_start, DEFAULT_BATCH_WINDOW_SECONDS)?)
            }
        } else {
            None
        };
        self.commit(
            "master_wake.updated",
            serde_json::to_value(&accumulator).unwrap(),
        )?;
        Ok(accumulator)
    }

    fn sync_master_wake_schedule(
        &mut self,
        master: &Address,
        state: &AgentState,
        _at: &str,
    ) -> CommResult<()> {
        let key = master.key();
        let Some(existing) = self.projection.master_wake.get(&key).cloned() else {
            return Ok(());
        };
        let mut updated = existing.clone();
        match state {
            AgentState::Working => {}
            AgentState::Idle => {
                if updated.pending
                    && !updated.stopped
                    && !updated.held
                    && updated.next_due_at.is_none()
                {
                    let due_origin = updated
                        .last_briefing_at
                        .as_deref()
                        .or(updated.first_observed_at.as_deref())
                        .ok_or_else(|| {
                            CommError::new(
                                "master_wake_schedule_origin_missing",
                                format!("master wake has no durable schedule origin: {key}"),
                            )
                        })?;
                    updated.next_due_at =
                        Some(add_seconds(due_origin, DEFAULT_BATCH_WINDOW_SECONDS)?);
                }
            }
        }
        if master_wake_accumulator_matches(&existing, &updated) {
            return Ok(());
        }
        self.commit(
            "master_wake.updated",
            serde_json::to_value(&updated).unwrap(),
        )?;
        Ok(())
    }

    fn clear_master_wake_signal(
        &mut self,
        master: &Address,
        signal_key: &str,
        at: &str,
    ) -> CommResult<()> {
        let key = master.key();
        let Some(existing) = self.projection.master_wake.get(&key).cloned() else {
            return Ok(());
        };
        if !existing.signals.contains_key(signal_key) {
            return Ok(());
        }
        let mut updated = existing.clone();
        if let Some(signal) = updated.signals.remove(signal_key) {
            updated.consumed_signals.insert(signal_key.into(), signal);
        }
        updated.generation = updated.generation.checked_add(1).ok_or_else(|| {
            CommError::new(
                "master_wake_generation_exhausted",
                format!("master wake generation exhausted: {key}"),
            )
        })?;
        updated.last_observed_at = Some(validate_time(at)?);
        updated.pending = updated
            .signals
            .values()
            .any(|value| !value.direct_dispatched);
        updated.next_due_at = if updated.pending && !updated.held {
            updated.next_due_at.clone()
        } else {
            None
        };
        self.commit(
            "master_wake.updated",
            serde_json::to_value(&updated).unwrap(),
        )?;
        Ok(())
    }

    fn record_master_wake(
        &mut self,
        master: Address,
        request: MasterWakeSignalRequest,
    ) -> CommResult<Value> {
        validate_master_wake_signal_request(&request)?;
        let observed_at = request
            .observed_at
            .as_deref()
            .map(validate_time)
            .transpose()?
            .unwrap_or_else(now);
        let priority = Priority::parse(&request.priority)?;
        let key = request.key.clone();
        let signal = MasterWakeSignal {
            signal_id: request
                .signal_id
                .unwrap_or_else(|| structured_key(&[&request.kind, &request.key, &observed_at])),
            key,
            kind: request.kind,
            title: request.title,
            priority,
            summary: request.summary,
            issue_id: request.issue_id,
            source: request.source,
            observed_at,
            direct_dispatched: false,
        };
        let idempotent =
            self.projection
                .master_wake
                .get(&master.key())
                .is_some_and(|accumulator| {
                    accumulator
                        .signals
                        .get(&signal.key)
                        .or_else(|| accumulator.consumed_signals.get(&signal.key))
                        .is_some_and(|existing| {
                            master_wake_signal_identity_matches(existing, &signal)
                        })
                });
        let accumulator = self.accumulate_master_wake(&master, signal.clone())?;
        let accumulator = if signal.priority.is_breakthrough() {
            self.dispatch_breakthrough_master_wake(&master, &signal.key)?
        } else {
            accumulator
        };
        Ok(json!({
            "masterWake": accumulator,
            "idempotent": idempotent
        }))
    }

    fn dispatch_breakthrough_master_wake(
        &mut self,
        master: &Address,
        signal_key: &str,
    ) -> CommResult<MasterWakeAccumulator> {
        let Some(accumulator) = self.projection.master_wake.get(&master.key()).cloned() else {
            return Err(CommError::new(
                "master_wake_not_found",
                format!("master wake not found: {}", master.key()),
            ));
        };
        let Some(signal) = accumulator.signals.get(signal_key).cloned() else {
            return Ok(accumulator);
        };
        if signal.direct_dispatched {
            return Ok(accumulator);
        }
        let Some(agent) = self.require_live_agent(master).ok().cloned() else {
            return Ok(accumulator);
        };
        let (message_id, conversation_id) = master_wake_direct_message_identity(master, &signal);
        let message = if let Some(existing) = self.projection.messages.get(&message_id).cloned() {
            if existing.conversation_id != conversation_id
                || existing.to != agent.address()
                || existing.from
                    != (Address {
                        scope_id: "appsdk".into(),
                        session_id: "daemon".into(),
                    })
                || existing.delivery_mode != DeliveryMode::Direct
                || existing.coalesce_key.as_deref() != Some("master-wake-direct")
                || existing.adapter_id != "mailbox"
                || existing.issue_id.is_some()
            {
                return Err(CommError::new(
                    "wakeup_message_conflict",
                    format!("direct master wake message id identifies a different message: {message_id}"),
                ));
            }
            self.prepare_wakeup_message(existing)?
        } else {
            let mut message = self.system_message(
                &agent.address(),
                signal.title.clone(),
                &format_priority(&signal.priority),
                &signal.summary,
                "master-wake-direct",
                &signal.observed_at,
                "mailbox",
            )?;
            message.message_id = message_id;
            message.conversation_id = conversation_id;
            message.delivery_mode = DeliveryMode::Direct;
            self.prepare_wakeup_message(message)?
        };
        let notification =
            self.notification_for(&message, &signal.observed_at, Some(&signal.observed_at))?;
        let Some(notification) = notification else {
            return Ok(accumulator);
        };
        if notification.status != "emitted" {
            return Ok(accumulator);
        }
        let mut updated = accumulator;
        if let Some(stored) = updated.signals.get_mut(signal_key) {
            stored.direct_dispatched = true;
        }
        updated.pending = updated
            .signals
            .values()
            .any(|value| !value.direct_dispatched);
        updated.next_due_at = if updated.pending && !updated.held {
            updated.next_due_at.clone()
        } else {
            None
        };
        self.commit(
            "master_wake.updated",
            serde_json::to_value(&updated).unwrap(),
        )?;
        Ok(updated)
    }

    fn decide_master_wake(
        &mut self,
        master: Address,
        generation: u64,
        action: &str,
        at: Option<&str>,
    ) -> CommResult<Value> {
        validate_address(&master)?;
        validate_non_empty(action, "action")?;
        let actor = self.require_live_agent(&master)?.clone();
        if actor.role != "master"
            || self
                .projection
                .scopes
                .get(&master.scope_id)
                .and_then(|scope| scope.master_session_id.as_deref())
                != Some(master.session_id.as_str())
        {
            return Err(CommError::new(
                "master_wake_actor_required",
                "only the registered scope master may decide its wake",
            ));
        }
        let key = master.key();
        let existing = self
            .projection
            .master_wake
            .get(&key)
            .cloned()
            .ok_or_else(|| {
                CommError::new(
                    "master_wake_not_found",
                    format!("master wake not found: {key}"),
                )
            })?;
        if existing.generation != generation {
            return Err(CommError::new(
                "master_wake_generation_conflict",
                format!(
                    "master wake generation is {}, not {}",
                    existing.generation, generation
                ),
            ));
        }
        let at = at.map(validate_time).transpose()?.unwrap_or_else(now);
        let mut updated = existing;
        let action = action.trim().to_ascii_lowercase();
        let terminal_decision = matches!(
            action.as_str(),
            "dispatch" | "handled" | "complete" | "completed"
        );
        let superseded_keys = if terminal_decision {
            self.pending_notification_keys_for_master_wake(&updated)
        } else {
            Vec::new()
        };
        match action.as_str() {
            "hold" => {
                updated.held = true;
                updated.next_due_at = None;
            }
            "dispatch" | "handled" | "complete" | "completed" => {
                for (signal_key, signal) in updated.signals.clone() {
                    updated.consumed_signals.insert(signal_key, signal);
                }
                updated.pending = false;
                updated.next_due_at = None;
                updated.last_briefing_generation = Some(generation);
                updated.last_briefing_at = Some(at.clone());
                updated.reminders_sent = 0;
                updated.stopped = false;
                updated.held = false;
                updated.signals.clear();
            }
            "schedule" => {
                if updated.pending {
                    updated.generation = updated.generation.checked_add(1).ok_or_else(|| {
                        CommError::new(
                            "master_wake_generation_exhausted",
                            format!("master wake generation exhausted: {key}"),
                        )
                    })?;
                }
                updated.held = false;
                updated.stopped = false;
                updated.reminders_sent = 0;
                updated.last_briefing_generation = None;
                updated.last_briefing_at = None;
                updated.next_due_at = if updated.pending {
                    if actor.state == AgentState::Idle {
                        Some(at.clone())
                    } else {
                        Some(add_seconds(&at, DEFAULT_BATCH_WINDOW_SECONDS)?)
                    }
                } else {
                    None
                };
            }
            other => {
                return Err(CommError::new(
                    "master_wake_action_invalid",
                    format!("unsupported master wake action: {other}"),
                ))
            }
        }
        if !superseded_keys.is_empty() {
            self.commit(
                "notification.superseded",
                json!({
                    "keys": superseded_keys,
                    "generation": generation,
                    "reason": "master_wake_decision"
                }),
            )?;
        }
        self.commit(
            "master_wake.decided",
            json!({ "accumulator": updated, "action": action, "at": at }),
        )?;
        if let Some(wakeup) = self.projection.wakeup.get(&key).cloned() {
            let synchronized = synchronize_master_wakeup(wakeup, &action, &updated);
            self.commit(
                "wakeup.updated",
                serde_json::to_value(synchronized).unwrap(),
            )?;
        }
        Ok(json!({
            "masterWake": self.projection.master_wake.get(&key),
            "action": action,
            "generation": generation
        }))
    }

    fn scope_master_address_unchecked(&self, scope_id: &str) -> Option<Address> {
        self.projection
            .scopes
            .get(scope_id)
            .and_then(|scope| scope.master_session_id.as_ref())
            .map(|session_id| Address {
                scope_id: scope_id.into(),
                session_id: session_id.clone(),
            })
    }

    fn master_wake_signal_recorded(
        &self,
        master: &Address,
        signal_key: &str,
        signal_id: &str,
    ) -> bool {
        self.projection
            .master_wake
            .get(&master.key())
            .is_some_and(|accumulator| {
                accumulator
                    .signals
                    .get(signal_key)
                    .is_some_and(|signal| signal.signal_id == signal_id)
                    || accumulator
                        .consumed_signals
                        .get(signal_key)
                        .is_some_and(|signal| signal.signal_id == signal_id)
            })
    }

    fn master_wake_signal_consumed(
        &self,
        master: &Address,
        signal_key: &str,
        signal_id: &str,
    ) -> bool {
        self.projection
            .master_wake
            .get(&master.key())
            .and_then(|accumulator| accumulator.consumed_signals.get(signal_key))
            .is_some_and(|signal| signal.signal_id == signal_id)
    }

    fn master_wake_signal_delivery_unknown(
        &self,
        master: &Address,
        signal: &MasterWakeSignal,
    ) -> bool {
        let (message_id, _) = master_wake_direct_message_identity(master, signal);
        self.projection.notifications.values().any(|notification| {
            notification.message_id == message_id && notification.status == "unknown"
        })
    }

    fn master_wake_signal_delivery_emitted(
        &self,
        master: &Address,
        signal: &MasterWakeSignal,
    ) -> bool {
        let (message_id, _) = master_wake_direct_message_identity(master, signal);
        self.projection.notifications.values().any(|notification| {
            notification.message_id == message_id && notification.status == "emitted"
        })
    }

    fn reconcile_breakthrough_master_wake(
        &mut self,
        accumulator: &MasterWakeAccumulator,
    ) -> CommResult<MasterWakeAccumulator> {
        let mut updated = accumulator.clone();
        let mut changed = false;
        for (signal_key, signal) in accumulator.signals.iter() {
            if signal.priority.is_breakthrough()
                && !signal.direct_dispatched
                && self.master_wake_signal_delivery_emitted(&accumulator.address, signal)
            {
                if let Some(stored) = updated.signals.get_mut(signal_key) {
                    stored.direct_dispatched = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return Ok(updated);
        }
        updated.pending = updated
            .signals
            .values()
            .any(|value| !value.direct_dispatched);
        updated.next_due_at = if updated.pending && !updated.held {
            updated.next_due_at.clone()
        } else {
            None
        };
        self.commit(
            "master_wake.updated",
            serde_json::to_value(&updated).unwrap(),
        )?;
        Ok(updated)
    }

    fn validate_master_wake_message_identity(
        &self,
        message: &MessageRecord,
        accumulator: &MasterWakeAccumulator,
        reminder: u8,
        target: &Address,
        conversation_id: &str,
    ) -> CommResult<()> {
        let (message_id, _) = master_wake_message_identity(accumulator, reminder);
        if message.message_id != message_id
            || message.conversation_id != conversation_id
            || message.from
                != (Address {
                    scope_id: "appsdk".into(),
                    session_id: "daemon".into(),
                })
            || message.to != *target
            || message.delivery_mode != DeliveryMode::Direct
            || message.coalesce_key.as_deref() != Some("master-wake")
            || message.issue_id.is_some()
            || message.adapter_id != "mailbox"
        {
            return Err(CommError::new(
                "wakeup_message_conflict",
                format!("wakeup message id identifies a different master wake: {message_id}"),
            ));
        }
        Ok(())
    }

    fn process_master_wake(
        &mut self,
        accumulator: &MasterWakeAccumulator,
        at: &str,
    ) -> CommResult<Option<MasterWakeAccumulator>> {
        let accumulator = self.reconcile_breakthrough_master_wake(accumulator)?;
        if !accumulator.pending
            || accumulator.held
            || accumulator.stopped
            || accumulator.reminders_sent >= DEFAULT_MASTER_REMINDER_LIMIT
        {
            return Ok(None);
        }
        if !accumulator.signals.values().any(|signal| {
            !signal.direct_dispatched
                && !self.master_wake_signal_delivery_unknown(&accumulator.address, signal)
        }) {
            // An uncertain direct delivery is never replayed as a briefing. Keep the
            // signal in the accumulator for explicit recovery or a new signal generation.
            return Ok(None);
        }
        let Some(agent) = self.live_idle_master_at(&accumulator.address, at) else {
            return Ok(None);
        };
        let Some(next_due_at) = accumulator.next_due_at.as_deref() else {
            return Ok(None);
        };
        if parse_time(at)? < parse_time(next_due_at)? {
            return Ok(None);
        }
        let reminder = accumulator.reminders_sent + 1;
        let (message_id, conversation_id) = master_wake_message_identity(&accumulator, reminder);
        let message = if let Some(existing) = self.projection.messages.get(&message_id).cloned() {
            self.validate_master_wake_message_identity(
                &existing,
                &accumulator,
                reminder,
                &agent.address(),
                &conversation_id,
            )?;
            self.prepare_wakeup_message(existing)?
        } else {
            let (title, body, priority) = self.master_wake_briefing(&accumulator, reminder);
            let mut message = self.system_message(
                &agent.address(),
                title,
                &format_priority(&priority),
                &body,
                "master-wake",
                at,
                "mailbox",
            )?;
            message.message_id = message_id;
            message.conversation_id = conversation_id;
            message.delivery_mode = DeliveryMode::Direct;
            self.prepare_wakeup_message(message)?
        };
        let notification = self.notification_for(&message, at, Some(at))?;
        let Some(notification) = notification else {
            return Ok(None);
        };
        if notification.status != "emitted" {
            return Ok(None);
        }

        let mut next = accumulator.clone();
        next.reminders_sent = reminder;
        next.last_briefing_generation = Some(accumulator.generation);
        next.last_briefing_at = Some(at.into());
        next.stopped = reminder >= DEFAULT_MASTER_REMINDER_LIMIT;
        next.next_due_at = if next.stopped {
            None
        } else {
            Some(add_seconds(at, DEFAULT_BATCH_WINDOW_SECONDS)?)
        };
        let superseded_keys = self.pending_notification_keys_for_master_wake(&accumulator);
        if !superseded_keys.is_empty() {
            self.commit(
                "notification.superseded",
                json!({
                    "keys": superseded_keys,
                    "generation": accumulator.generation,
                    "reason": "master_wake_briefing"
                }),
            )?;
        }
        self.commit(
            "master_wake.briefing",
            json!({
                "accumulator": next,
                "message": message,
                "notification": notification,
                "generation": accumulator.generation,
                "reminder": reminder
            }),
        )?;
        if let Some(wakeup) = self
            .projection
            .wakeup
            .get(&accumulator.address.key())
            .cloned()
        {
            let mut synchronized = wakeup;
            synchronized.reminders_sent = reminder;
            synchronized.last_reminder_at = Some(at.into());
            synchronized.next_due_at = next.next_due_at.clone();
            synchronized.stopped = next.stopped;
            self.commit(
                "wakeup.updated",
                serde_json::to_value(synchronized).unwrap(),
            )?;
        }
        Ok(Some(next))
    }

    fn pending_notification_keys_for_master_wake(
        &self,
        accumulator: &MasterWakeAccumulator,
    ) -> Vec<String> {
        self.projection
            .notifications
            .iter()
            .filter(|(_, notification)| {
                notification.status == "pending"
                    && notification.recipient == accumulator.address
                    && accumulator
                        .signals
                        .values()
                        .any(|signal| master_wake_covers_notification(signal, notification))
            })
            .map(|(key, _)| key.clone())
            .collect()
    }

    fn notification_held_for_master_wake(
        &self,
        notification: &NotificationRecord,
    ) -> CommResult<bool> {
        let Some(master) = self.scope_master_address_unchecked(&notification.recipient.scope_id)
        else {
            return Ok(false);
        };
        if master != notification.recipient {
            return Ok(false);
        }
        let Some(agent) = self.projection.agents.get(&master.key()) else {
            return Ok(false);
        };
        if agent.role != "master" {
            return Ok(false);
        }
        Ok(self
            .projection
            .master_wake
            .get(&master.key())
            .is_some_and(|accumulator| {
                accumulator.pending
                    && accumulator
                        .signals
                        .values()
                        .any(|signal| master_wake_covers_notification(signal, notification))
            }))
    }

    fn master_wake_briefing(
        &self,
        accumulator: &MasterWakeAccumulator,
        reminder: u8,
    ) -> (String, String, Priority) {
        let mut signals: Vec<&MasterWakeSignal> = accumulator
            .signals
            .values()
            .filter(|signal| {
                !signal.direct_dispatched
                    && !self.master_wake_signal_delivery_unknown(&accumulator.address, signal)
            })
            .collect();
        signals.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left.observed_at.cmp(&right.observed_at))
                .then_with(|| left.key.cmp(&right.key))
        });
        let priority = signals
            .first()
            .map(|signal| signal.priority.clone())
            .unwrap_or(Priority::P1);
        let title = format!(
            "master wake: {} update{} (reminder {}/{})",
            signals.len(),
            if signals.len() == 1 { "" } else { "s" },
            reminder,
            DEFAULT_MASTER_REMINDER_LIMIT
        );
        let mut lines = vec![format!(
            "Generation {} observed {}; inspect mailbox JSONL before dispatching the next loop.",
            accumulator.generation,
            accumulator.last_observed_at.as_deref().unwrap_or("unknown")
        )];
        if signals.is_empty() {
            lines.push(
                "No undelivered signal remains; the direct signal is retained as history.".into(),
            );
        } else {
            lines.push("Signals:".into());
            for signal in signals.iter().take(12) {
                lines.push(format!(
                    "- [{}] {}: {} ({})",
                    format_priority(&signal.priority),
                    signal.title,
                    signal.summary,
                    signal.kind
                ));
            }
            if signals.len() > 12 {
                lines.push(format!(
                    "- ... {} more signals in JSONL",
                    signals.len() - 12
                ));
            }
        }

        let mut idle_workers: Vec<&AgentRecord> = self
            .projection
            .agents
            .values()
            .filter(|agent| {
                agent.scope_id == accumulator.address.scope_id
                    && agent.role != "master"
                    && agent.state == AgentState::Idle
            })
            .collect();
        idle_workers.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        lines.push(format!("Idle workers: {}", idle_workers.len()));
        for worker in idle_workers.iter().take(12) {
            lines.push(format!(
                "- {} ({})",
                worker.agent_id,
                worker.address().key()
            ));
        }
        if idle_workers.len() > 12 {
            lines.push(format!(
                "- ... {} more idle workers in status",
                idle_workers.len() - 12
            ));
        }

        let mut active_bugs: Vec<&BugRecord> = self
            .projection
            .bugs
            .values()
            .filter(|bug| bug.scope_id == accumulator.address.scope_id && bug.status == "active")
            .collect();
        active_bugs.sort_by(|left, right| priority_then_time_bug(left, right));
        lines.push(format!("Active bugs: {}", active_bugs.len()));
        for bug in active_bugs.iter().take(12) {
            lines.push(format!(
                "- [{}] {}: {} ({})",
                format_priority(&bug.priority),
                bug.bug_id,
                bug.title,
                bug.loop_id
            ));
        }
        if active_bugs.len() > 12 {
            lines.push(format!(
                "- ... {} more active bugs in status",
                active_bugs.len() - 12
            ));
        }

        let mut active_loops: Vec<&LoopRecord> = self
            .projection
            .loops
            .values()
            .filter(|loop_record| {
                loop_record.owner.scope_id == accumulator.address.scope_id
                    && loop_record.status == "active"
            })
            .collect();
        active_loops.sort_by(|left, right| left.updated_at.cmp(&right.updated_at));
        lines.push(format!("Active loops: {}", active_loops.len()));
        for loop_record in active_loops.iter().take(12) {
            lines.push(format!(
                "- {} phase={} iteration={}",
                loop_record.loop_id, loop_record.phase, loop_record.iteration
            ));
        }
        if active_loops.len() > 12 {
            lines.push(format!(
                "- ... {} more loops in status",
                active_loops.len() - 12
            ));
        }
        lines.push(
            "Action: consume this generation, prioritize P0/P1, and dispatch only within ownership; use master_wake_decide after a scheduling decision.".into(),
        );
        (title, truncate_chars(&lines.join("\n"), 3_600), priority)
    }

    fn repair_master_wakeup(
        &mut self,
        current: &AgentRecord,
        state: &AgentState,
    ) -> CommResult<()> {
        let key = current.address().key();
        let existing = self.projection.wakeup.get(&key).cloned();
        let wakeup = match state {
            AgentState::Working => {
                let valid = existing.as_ref().is_some_and(|wakeup| {
                    wakeup.idle_since.is_none()
                        && wakeup.next_due_at.is_none()
                        && wakeup.reminders_sent == 0
                        && !wakeup.stopped
                        && wakeup.last_reminder_at.is_none()
                });
                if valid {
                    return Ok(());
                }
                WakeupRecord {
                    address: current.address(),
                    idle_since: None,
                    reminders_sent: 0,
                    next_due_at: None,
                    stopped: false,
                    last_reminder_at: None,
                }
            }
            AgentState::Idle => {
                let idle_since = current.last_state_at.clone();
                let same_cycle = existing.as_ref().is_some_and(|wakeup| {
                    wakeup.idle_since.as_deref() == Some(idle_since.as_str())
                });
                if same_cycle
                    && existing
                        .as_ref()
                        .is_some_and(|wakeup| wakeup.stopped && wakeup.next_due_at.is_none())
                {
                    return Ok(());
                }
                let (reminders_sent, stopped, last_reminder_at) = if same_cycle {
                    let wakeup = existing.as_ref().expect("same cycle wakeup exists");
                    (
                        wakeup.reminders_sent,
                        wakeup.reminders_sent >= DEFAULT_MASTER_REMINDER_LIMIT,
                        if wakeup.reminders_sent == 0 {
                            None
                        } else {
                            wakeup.last_reminder_at.clone()
                        },
                    )
                } else {
                    (0, false, None)
                };
                let next_due_at = if reminders_sent == 0 {
                    add_seconds(&idle_since, DEFAULT_BATCH_WINDOW_SECONDS)?
                } else if let Some(last_reminder_at) = last_reminder_at.as_deref() {
                    add_seconds(last_reminder_at, DEFAULT_BATCH_WINDOW_SECONDS)?
                } else {
                    existing
                        .as_ref()
                        .and_then(|wakeup| wakeup.next_due_at.clone())
                        .ok_or_else(|| {
                            CommError::new(
                                "wakeup_schedule_missing",
                                format!("master wakeup has no next due time: {key}"),
                            )
                        })?
                };
                let valid = same_cycle
                    && existing.as_ref().is_some_and(|wakeup| {
                        wakeup.next_due_at.as_deref() == Some(next_due_at.as_str())
                            && wakeup.reminders_sent == reminders_sent
                            && wakeup.stopped == stopped
                            && wakeup.last_reminder_at == last_reminder_at
                    });
                if valid {
                    return Ok(());
                }
                WakeupRecord {
                    address: current.address(),
                    idle_since: Some(idle_since),
                    reminders_sent,
                    next_due_at: Some(next_due_at),
                    stopped,
                    last_reminder_at,
                }
            }
        };
        self.commit("wakeup.updated", serde_json::to_value(&wakeup).unwrap())?;
        Ok(())
    }

    pub fn tick(&mut self, at: Option<&str>) -> CommResult<Value> {
        let at = at.map(validate_time).transpose()?.unwrap_or_else(now);
        let mut addresses = BTreeMap::new();
        for wakeup in self.projection.wakeup.values() {
            addresses.insert(wakeup.address.key(), wakeup.address.clone());
        }
        for accumulator in self.projection.master_wake.values() {
            addresses.insert(accumulator.address.key(), accumulator.address.clone());
        }
        let mut changed = Vec::new();
        let mut master_wake_changed = Vec::new();
        for (_, address) in addresses {
            if let Some(accumulator) = self.projection.master_wake.get(&address.key()).cloned() {
                if accumulator.pending {
                    if let Some(updated) = self.process_master_wake(&accumulator, &at)? {
                        master_wake_changed.push(updated);
                    }
                    continue;
                }
            }
            let Some(wakeup) = self.projection.wakeup.get(&address.key()).cloned() else {
                continue;
            };
            if wakeup.stopped || wakeup.reminders_sent >= DEFAULT_MASTER_REMINDER_LIMIT {
                continue;
            }
            let agent = match self.live_idle_master_at(&wakeup.address, &at) {
                Some(agent) => agent,
                None => continue,
            };
            let due = match wakeup.next_due_at.as_deref() {
                Some(next_due) => parse_time(&at)? >= parse_time(next_due)?,
                None => false,
            };
            if !due {
                continue;
            }
            if self.wakeup_delivery_in_flight(&wakeup)? {
                continue;
            }
            let reminders_sent = wakeup.reminders_sent + 1;
            let stopped = reminders_sent >= DEFAULT_MASTER_REMINDER_LIMIT;
            let next_wakeup = WakeupRecord {
                address: wakeup.address.clone(),
                idle_since: wakeup.idle_since.clone(),
                reminders_sent,
                next_due_at: if stopped {
                    None
                } else {
                    Some(add_seconds(&at, DEFAULT_BATCH_WINDOW_SECONDS)?)
                },
                stopped,
                last_reminder_at: Some(at.clone()),
            };
            let (message_id, conversation_id) = wakeup_message_identity(&wakeup, reminders_sent)?;
            let mut message = self.system_message(
                &agent.address(),
                format!(
                    "master idle reminder {reminders_sent}/{}",
                    DEFAULT_MASTER_REMINDER_LIMIT
                ),
                "p1",
                "Master remains idle. Inspect active Bugs and Loop state before the next scheduled round.",
                "master-idle",
                &at,
                "mailbox",
            )?;
            message.message_id = message_id;
            message.conversation_id = conversation_id;
            let message = self.prepare_wakeup_message(message)?;
            let mut notification =
                self.build_notification(&message, &message.created_at, Some(&message.created_at))?;
            notification.notification_id = format!("wakeup-notification-{}", message.message_id);
            let notification_key = self.notification_key(&message, &notification, true);
            if let Some(existing) = self
                .projection
                .notifications
                .get(&notification_key)
                .cloned()
            {
                if existing.message_id == message.message_id {
                    notification = existing;
                } else {
                    self.commit(
                        "notification.queued",
                        json!({
                            "key": notification_key,
                            "notification": notification.clone()
                        }),
                    )?;
                }
            } else {
                self.commit(
                    "notification.queued",
                    json!({
                        "key": notification_key,
                        "notification": notification.clone()
                    }),
                )?;
            }
            if notification.message_id == message.message_id && notification.status == "emitted" {
                let completed_attempt_id = self
                    .projection
                    .completed_attempts
                    .get(&notification_key)
                    .cloned();
                if let Some(attempt_id) = completed_attempt_id.as_deref() {
                    self.commit_wakeup_reminder(
                        &next_wakeup,
                        &message,
                        &notification_key,
                        &notification,
                        Some(attempt_id),
                        notification.transport_receipt.as_ref(),
                    )?;
                    let updated = self
                        .projection
                        .wakeup
                        .get(&wakeup.address.key())
                        .cloned()
                        .ok_or_else(|| {
                            CommError::new("wakeup_not_found", "wakeup update was not projected")
                        })?;
                    changed.push(updated);
                }
                continue;
            }
            if notification.message_id == message.message_id
                && notification.status == "pending"
                && notification.delivery_attempt.is_none()
                && notification.last_error.is_some()
            {
                let completed_attempt_id = self
                    .projection
                    .completed_attempts
                    .get(&notification_key)
                    .cloned();
                if let Some(attempt_id) = completed_attempt_id.as_deref() {
                    self.commit_wakeup_reminder(
                        &next_wakeup,
                        &message,
                        &notification_key,
                        &notification,
                        Some(attempt_id),
                        None,
                    )?;
                    let updated = self
                        .projection
                        .wakeup
                        .get(&wakeup.address.key())
                        .cloned()
                        .ok_or_else(|| {
                            CommError::new("wakeup_not_found", "wakeup update was not projected")
                        })?;
                    changed.push(updated);
                }
                continue;
            }
            if notification.status == "unknown" || notification.delivery_attempt.is_some() {
                continue;
            }
            let adapter = match self.adapter_for(&message.adapter_id, Some(&agent.address())) {
                Ok(adapter) => adapter,
                Err(error) => {
                    let error = adapter_error(&error, &message.adapter_id, "wakeup.reminder");
                    notification.last_error = Some(adapter_error_record(
                        &error,
                        &message.adapter_id,
                        "wakeup.reminder",
                    ));
                    self.commit_wakeup_reminder(
                        &next_wakeup,
                        &message,
                        &notification_key,
                        &notification,
                        None,
                        None,
                    )?;
                    return Err(error);
                }
            };
            let attempt =
                new_delivery_attempt_at(&message.adapter_id, "notification.emitted", None, &at);
            let attempt_id = attempt.attempt_id.clone();
            let notification_id = notification.notification_id.clone();
            self.commit(
                "notification.delivery_attempt",
                json!({
                    "attemptId": attempt_id.clone(),
                    "keys": [notification_key.clone()],
                    "attempt": attempt
                }),
            )?;
            let receipt = match adapter.deliver(&message) {
                Ok(receipt) => receipt,
                Err(error) => {
                    let error = adapter_error(&error, &message.adapter_id, "wakeup.reminder");
                    if let Err(record_error) = self.record_notification_failure(
                        std::slice::from_ref(&notification_key),
                        &message.adapter_id,
                        &error,
                        "notification.emitted",
                        json!({
                            "messageId": message.message_id,
                            "notificationKey": notification_key.clone(),
                            "notificationId": notification_id,
                            "attemptId": attempt_id.clone()
                        }),
                    ) {
                        return Err(with_secondary_error(
                            error,
                            record_error,
                            "notification.delivery_failed",
                        ));
                    }
                    let notification = self
                        .projection
                        .notifications
                        .get(&notification_key)
                        .cloned()
                        .ok_or_else(|| {
                            CommError::new(
                                "notification_recovery_failed",
                                format!("notification disappeared during wakeup failure: {notification_key}"),
                            )
                        })?;
                    self.commit_wakeup_reminder(
                        &next_wakeup,
                        &message,
                        &notification_key,
                        &notification,
                        Some(&attempt_id),
                        None,
                    )?;
                    return Err(error);
                }
            };
            self.commit(
                "notification.emitted",
                json!({
                    "attemptId": attempt_id.clone(),
                    "keys": [notification_key.clone()],
                    "at": at.clone(),
                    "receipt": receipt.clone()
                }),
            )?;
            let notification = self
                .projection
                .notifications
                .get(&notification_key)
                .cloned()
                .ok_or_else(|| {
                    CommError::new(
                        "notification_recovery_failed",
                        format!("notification disappeared during wakeup: {notification_key}"),
                    )
                })?;
            self.commit_wakeup_reminder(
                &next_wakeup,
                &message,
                &notification_key,
                &notification,
                Some(&attempt_id),
                Some(&receipt),
            )?;
            let updated = self
                .projection
                .wakeup
                .get(&wakeup.address.key())
                .cloned()
                .ok_or_else(|| {
                    CommError::new("wakeup_not_found", "wakeup update was not projected")
                })?;
            changed.push(updated);
        }
        let wakeup = self.projection.wakeup.values().cloned().collect::<Vec<_>>();
        let master_wake = self
            .projection
            .master_wake
            .values()
            .cloned()
            .collect::<Vec<_>>();
        Ok(json!({
            "at": at,
            "wakeup": wakeup,
            "masterWake": master_wake,
            "changed": changed,
            "masterWakeChanged": master_wake_changed
        }))
    }

    pub fn flush_notifications(&mut self, at: Option<&str>) -> CommResult<Value> {
        let at = at.map(validate_time).transpose()?.unwrap_or_else(now);
        let now_time = parse_time(&at)?;
        let mut groups: BTreeMap<String, (Address, String, Vec<(String, NotificationRecord)>)> =
            BTreeMap::new();
        for (key, notification) in &self.projection.notifications {
            if notification.status != "pending" {
                continue;
            }
            let message = self
                .projection
                .messages
                .get(&notification.message_id)
                .ok_or_else(|| {
                    CommError::new(
                        "notification_message_missing",
                        format!(
                            "pending notification {} references missing message {}",
                            key, notification.message_id
                        ),
                    )
                })?;
            if matches!(message.delivery_mode, DeliveryMode::Direct)
                || message.priority.is_breakthrough()
                || notification.priority.is_breakthrough()
            {
                // Direct and P0 notifications have their own delivery retry
                // path.  Keeping them out of this idle batch is essential:
                // an adapter failure must never be silently demoted to a
                // lower urgency transport.
                continue;
            }
            if self.notification_held_for_master_wake(notification)? {
                continue;
            }
            if parse_time(&notification.available_at)? > now_time {
                continue;
            }
            groups
                .entry(structured_key(&[
                    &notification.recipient.key(),
                    &notification.adapter_id,
                ]))
                .or_insert_with(|| {
                    (
                        notification.recipient.clone(),
                        notification.adapter_id.clone(),
                        Vec::new(),
                    )
                })
                .2
                .push((key.clone(), notification.clone()));
        }
        let mut batches = Vec::new();
        for (_, (recipient, adapter_id, mut notifications)) in groups {
            notifications.sort_by(|left, right| {
                left.1
                    .priority
                    .cmp(&right.1.priority)
                    .then_with(|| left.1.created_at.cmp(&right.1.created_at))
                    .then_with(|| left.1.notification_id.cmp(&right.1.notification_id))
            });
            let batch = NotificationBatch {
                batch_id: new_id("batch"),
                recipient: recipient.clone(),
                created_at: at.clone(),
                adapter_id: adapter_id.clone(),
                items: notifications
                    .iter()
                    .map(|(_, notification)| notification.summary())
                    .collect(),
            };
            let keys: Vec<String> = notifications.iter().map(|(key, _)| key.clone()).collect();
            let adapter = match self.adapter_for(&adapter_id, Some(&recipient)) {
                Ok(adapter) => adapter,
                Err(error) => {
                    let error = adapter_error(&error, &adapter_id, "notification.batch_emitted");
                    if let Err(record_error) = self.record_notification_failure(
                        &keys,
                        &adapter_id,
                        &error,
                        "notification.batch_emitted",
                        json!({
                            "batchId": batch.batch_id,
                            "recipient": batch.recipient,
                            "notificationKeys": keys
                        }),
                    ) {
                        return Err(with_secondary_error(
                            error,
                            record_error,
                            "notification.delivery_failed",
                        ));
                    }
                    return Err(error);
                }
            };
            let attempt = new_delivery_attempt(
                &adapter_id,
                "notification.batch_emitted",
                Some(&batch.batch_id),
            );
            let attempt_id = attempt.attempt_id.clone();
            self.commit(
                "notification.delivery_attempt",
                json!({
                    "attemptId": attempt_id.clone(),
                    "keys": keys,
                    "attempt": attempt
                }),
            )?;
            let receipt = match adapter.emit_batch(&batch) {
                Ok(receipt) => receipt,
                Err(error) => {
                    let error = adapter_error(&error, &adapter_id, "notification.batch_emitted");
                    if let Err(record_error) = self.record_notification_failure(
                        &keys,
                        &adapter_id,
                        &error,
                        "notification.batch_emitted",
                        json!({
                            "batchId": batch.batch_id,
                            "recipient": batch.recipient,
                            "notificationKeys": keys,
                            "attemptId": attempt_id.clone()
                        }),
                    ) {
                        return Err(with_secondary_error(
                            error,
                            record_error,
                            "notification.delivery_failed",
                        ));
                    }
                    return Err(error);
                }
            };
            self.commit(
                "notification.batch_emitted",
                json!({
                    "batch": batch,
                    "notificationKeys": keys,
                    "at": at,
                    "attemptId": attempt_id.clone(),
                    "receipt": receipt
                }),
            )?;
            batches.push(batch);
        }
        Ok(json!({ "flushedAt": at, "batches": batches }))
    }

    fn report_bug(&mut self, request: BugRequest) -> CommResult<Value> {
        validate_non_empty(&request.bug_id, "bugId")?;
        validate_non_empty(&request.scope_id, "scopeId")?;
        validate_non_empty(&request.title, "title")?;
        validate_non_empty(&request.description, "description")?;
        if request.reporter.scope_id != request.scope_id {
            return Err(CommError::new(
                "bug_reporter_scope_mismatch",
                "bug reporter must belong to the bug scope",
            ));
        }
        self.require_live_agent(&request.reporter)?;
        let priority = Priority::parse(&request.priority)?;
        let at = now();
        let loop_id = format!("bug-loop-{}", request.bug_id);
        let owner = self.scope_master_address(&request.scope_id)?;
        if let Some(existing) = self.projection.bugs.get(&request.bug_id).cloned() {
            if existing.scope_id != request.scope_id || existing.loop_id != loop_id {
                return Err(CommError::new(
                    "bug_loop_conflict",
                    format!(
                        "bug is bound outside deterministic loop: {}",
                        existing.loop_id
                    ),
                ));
            }
            if existing.title == request.title
                && existing.description == request.description
                && existing.priority == priority
                && existing.reporter == request.reporter
                && existing.worktree_id == request.worktree_id
            {
                return self.recover_idempotent_bug(existing);
            }
            return Err(CommError::new(
                "bug_conflict",
                format!("bug already exists: {}", request.bug_id),
            ));
        }
        if let Some(loop_record) = self.projection.loops.get(&loop_id) {
            validate_bug_loop_binding(loop_record, &request.bug_id, &request.scope_id, &owner)?;
        } else {
            let loop_record = LoopRecord {
                loop_id: loop_id.clone(),
                kind: "bug".into(),
                owner: owner.clone(),
                trigger: BUG_LOOP_TRIGGER.into(),
                work: BUG_LOOP_WORK.into(),
                gate: BUG_LOOP_GATE.into(),
                state: BUG_LOOP_STATE.into(),
                stop: BUG_LOOP_STOP.into(),
                max_iterations: 100,
                deadline_at: None,
                phase: "discover".into(),
                status: "active".into(),
                iteration: 0,
                created_at: at.clone(),
                updated_at: at.clone(),
                completion_evidence: None,
            };
            self.commit("loop.created", serde_json::to_value(&loop_record).unwrap())?;
        }
        let bug = BugRecord {
            bug_id: request.bug_id.clone(),
            scope_id: request.scope_id.clone(),
            title: request.title,
            priority: priority.clone(),
            description: request.description,
            reporter: request.reporter.clone(),
            status: "active".into(),
            worktree_id: request.worktree_id,
            loop_id,
            created_at: at.clone(),
            updated_at: at.clone(),
            resolution_evidence: None,
        };
        self.commit("bug.reported", serde_json::to_value(&bug).unwrap())?;
        let notification = self.system_notification(
            &owner,
            format!("bug reported: {}", bug.title),
            &priority,
            &bug.description,
            Some(&bug.bug_id),
            "bug",
            &at,
            None,
            "mailbox",
        )?;
        let signal = bug_wake_signal(&bug, priority.is_breakthrough());
        let master_wake = self.accumulate_master_wake(&owner, signal)?;
        Ok(json!({
            "bug": bug,
            "notification": notification,
            "masterWake": master_wake,
            "idempotent": false
        }))
    }

    fn recover_idempotent_bug(&mut self, bug: BugRecord) -> CommResult<Value> {
        let owner = self.scope_master_address(&bug.scope_id)?;
        let loop_record = self.recover_bug_loop(&bug, &owner)?;
        let notification = self.recover_bug_notification(&bug, &owner)?;
        let master_wake = self.accumulate_master_wake(
            &owner,
            bug_wake_signal(&bug, bug.priority.is_breakthrough()),
        )?;
        Ok(json!({
            "bug": bug,
            "loop": loop_record,
            "notification": notification,
            "masterWake": master_wake,
            "idempotent": true
        }))
    }

    fn recover_bug_loop(&mut self, bug: &BugRecord, owner: &Address) -> CommResult<LoopRecord> {
        if let Some(existing) = self.projection.loops.get(&bug.loop_id).cloned() {
            if !bug_loop_matches(&existing, owner) {
                return Err(CommError::new(
                    "bug_loop_conflict",
                    format!("bug loop is already used by another loop: {}", bug.loop_id),
                ));
            }
            return Ok(existing);
        }
        let at = now();
        let loop_record = LoopRecord {
            loop_id: bug.loop_id.clone(),
            kind: "bug".into(),
            owner: owner.clone(),
            trigger: BUG_LOOP_TRIGGER.into(),
            work: BUG_LOOP_WORK.into(),
            gate: BUG_LOOP_GATE.into(),
            state: BUG_LOOP_STATE.into(),
            stop: BUG_LOOP_STOP.into(),
            max_iterations: 100,
            deadline_at: None,
            phase: "discover".into(),
            status: "active".into(),
            iteration: 0,
            created_at: at.clone(),
            updated_at: at,
            completion_evidence: None,
        };
        self.commit("loop.created", serde_json::to_value(&loop_record).unwrap())?;
        Ok(loop_record)
    }

    fn recover_bug_notification(&mut self, bug: &BugRecord, owner: &Address) -> CommResult<Value> {
        if let Some(notification) = self
            .projection
            .notifications
            .values()
            .find(|notification| {
                notification.issue_id.as_deref() == Some(bug.bug_id.as_str())
                    && notification.recipient == *owner
                    && notification.coalesce_key.as_deref() == Some("bug")
            })
            .cloned()
        {
            let message = self
                .projection
                .messages
                .get(&notification.message_id)
                .cloned();
            if let Some(message) = message.as_ref() {
                self.ensure_message_delivery_attempt(message)?;
            }
            return Ok(json!({
                "message": message,
                "notification": notification.summary()
            }));
        }

        if let Some(message) = self
            .projection
            .messages
            .values()
            .find(|message| {
                message.issue_id.as_deref() == Some(bug.bug_id.as_str())
                    && message.to == *owner
                    && message.coalesce_key.as_deref() == Some("bug")
            })
            .cloned()
        {
            let notification = self.recover_message_notification(&message)?;
            return Ok(json!({
                "message": message,
                "notification": notification.map(|value| value.summary())
            }));
        }

        self.system_notification(
            owner,
            format!("bug reported: {}", bug.title),
            &bug.priority,
            &bug.description,
            Some(&bug.bug_id),
            "bug",
            &bug.created_at,
            None,
            "mailbox",
        )
    }

    pub fn update_bug(
        &mut self,
        bug_id: &str,
        status: &str,
        actor: Address,
        evidence: Option<Value>,
    ) -> CommResult<Value> {
        validate_non_empty(bug_id, "bugId")?;
        let current =
            self.projection.bugs.get(bug_id).cloned().ok_or_else(|| {
                CommError::new("bug_not_found", format!("bug not found: {bug_id}"))
            })?;
        let actor_record = self.require_live_agent(&actor)?.clone();
        if actor.scope_id != current.scope_id {
            return Err(CommError::new(
                "bug_actor_scope_mismatch",
                "bug update actor must belong to the bug scope",
            ));
        }
        if !matches!(status, "active" | "resolved" | "closed") {
            return Err(CommError::new(
                "invalid_bug_status",
                format!("unsupported bug status: {status}"),
            ));
        }
        let is_scope_master = self
            .projection
            .scopes
            .get(&current.scope_id)
            .and_then(|scope| scope.master_session_id.as_deref())
            == Some(actor.session_id.as_str());
        if matches!(status, "resolved" | "closed")
            && (actor_record.role != "master" || !is_scope_master)
        {
            return Err(CommError::new(
                "bug_resolution_master_required",
                "only the scope master may resolve or close a bug",
            ));
        }
        let owner = self.scope_master_address(&current.scope_id)?;
        let expected_loop_id = format!("bug-loop-{}", current.bug_id);
        if current.loop_id != expected_loop_id {
            return Err(CommError::new(
                "bug_loop_conflict",
                format!("bug is bound to an unexpected loop: {}", current.loop_id),
            ));
        }
        let current_loop = self
            .projection
            .loops
            .get(&expected_loop_id)
            .ok_or_else(|| {
                CommError::new(
                    "bug_loop_conflict",
                    format!("deterministic bug loop is missing: {expected_loop_id}"),
                )
            })?;
        validate_bug_loop_binding(current_loop, &current.bug_id, &current.scope_id, &owner)?;
        let resolution_evidence = if matches!(status, "resolved" | "closed") {
            Some(validate_resolution_evidence(evidence.as_ref())?)
        } else {
            None
        };
        let mut updated = current.clone();
        updated.status = status.into();
        updated.updated_at = now();
        updated.resolution_evidence = resolution_evidence.clone();
        let mut updated_loop = Some(current_loop.clone());
        if matches!(status, "resolved" | "closed") {
            if let Some(loop_record) = updated_loop.as_mut() {
                loop_record.status = "completed".into();
                loop_record.phase = "completed".into();
                loop_record.updated_at = updated.updated_at.clone();
                loop_record.completion_evidence = resolution_evidence.clone();
            }
        } else if status == "active" {
            if let Some(loop_record) = updated_loop.as_mut() {
                loop_record.status = "active".into();
                loop_record.phase = "discover".into();
                loop_record.updated_at = updated.updated_at.clone();
                loop_record.completion_evidence = None;
            }
        }
        self.commit(
            "bug.updated",
            json!({ "bug": updated, "loop": updated_loop, "evidence": resolution_evidence }),
        )?;
        let notification = if matches!(status, "resolved" | "closed") {
            Some(self.system_notification(
                &current.reporter,
                format!("bug {}: {}", status, current.title),
                &current.priority,
                "Bug state changed; inspect the JSONL facts for the merge and verification evidence.",
                Some(&current.bug_id),
                "bug-resolution",
                &updated.updated_at,
                None,
                "mailbox",
            )?)
        } else if status == "active" && updated.priority.is_breakthrough() {
            Some(self.system_notification(
                &owner,
                format!("bug active: {}", updated.title),
                &updated.priority,
                &updated.description,
                Some(&updated.bug_id),
                "bug",
                &updated.updated_at,
                Some(&updated.updated_at),
                "mailbox",
            )?)
        } else {
            None
        };
        let master_wake = if updated.status == "active" {
            Some(self.accumulate_master_wake(
                &owner,
                bug_wake_signal(&updated, updated.priority.is_breakthrough()),
            )?)
        } else {
            self.clear_master_wake_signal(
                &owner,
                &bug_wake_signal_key(&updated.bug_id),
                &updated.updated_at,
            )?;
            self.projection.master_wake.get(&owner.key()).cloned()
        };
        Ok(json!({
            "bug": updated,
            "loop": updated_loop,
            "notification": notification,
            "masterWake": master_wake
        }))
    }

    fn create_loop(&mut self, request: LoopRequest) -> CommResult<Value> {
        validate_loop_request(&request)?;
        if request.loop_id.starts_with("bug-loop-") {
            return Err(CommError::new(
                "bug_loop_reserved",
                "bug-loop identifiers are reserved for report_bug",
            ));
        }
        self.require_live_agent(&request.owner)?;
        if self.projection.loops.contains_key(&request.loop_id) {
            return Err(CommError::new(
                "loop_conflict",
                format!("loop already exists: {}", request.loop_id),
            ));
        }
        if let Some(deadline) = request.deadline_at.as_deref() {
            validate_time(deadline)?;
        }
        let at = now();
        let loop_record = LoopRecord {
            loop_id: request.loop_id.clone(),
            kind: request.kind,
            owner: request.owner,
            trigger: request.trigger,
            work: request.work,
            gate: request.gate,
            state: request.state,
            stop: request.stop,
            max_iterations: request.max_iterations,
            deadline_at: request.deadline_at,
            phase: "discover".into(),
            status: "active".into(),
            iteration: 0,
            created_at: at.clone(),
            updated_at: at,
            completion_evidence: None,
        };
        self.commit("loop.created", serde_json::to_value(&loop_record).unwrap())?;
        Ok(json!({ "loop": loop_record }))
    }

    pub fn advance_loop(
        &mut self,
        loop_id: &str,
        complete: bool,
        blocked: bool,
        actor: Option<Address>,
        evidence: Option<Value>,
        at: Option<&str>,
    ) -> CommResult<Value> {
        let at = at.map(validate_time).transpose()?.unwrap_or_else(now);
        let current = self.projection.loops.get(loop_id).cloned().ok_or_else(|| {
            CommError::new("loop_not_found", format!("loop not found: {loop_id}"))
        })?;
        if current.status != "active" {
            return Err(CommError::new(
                "loop_not_active",
                format!("loop is not active: {loop_id}"),
            ));
        }
        let actor = actor.unwrap_or_else(|| current.owner.clone());
        let actor_record = self.require_live_agent(&actor)?.clone();
        if actor.scope_id != current.owner.scope_id {
            return Err(CommError::new(
                "loop_actor_scope_mismatch",
                "loop actor must belong to the loop owner scope",
            ));
        }
        if actor != current.owner && actor_record.role != "master" {
            return Err(CommError::new(
                "loop_actor_forbidden",
                "only the loop owner or scope master may advance a loop",
            ));
        }
        let mut updated = current.clone();
        let deadline_reached = updated
            .deadline_at
            .as_deref()
            .map(|deadline| {
                parse_time(&at).and_then(|at| parse_time(deadline).map(|deadline| at >= deadline))
            })
            .transpose()?
            .unwrap_or(false);
        if deadline_reached {
            updated.status = "stopped".into();
            updated.phase = "deadline".into();
        } else if complete {
            updated.completion_evidence =
                Some(validate_loop_completion_evidence(evidence.as_ref())?);
            updated.status = "completed".into();
            updated.phase = "completed".into();
        } else if blocked {
            updated.status = "blocked".into();
            updated.phase = "blocked".into();
        } else {
            updated.phase = next_loop_phase(&mut updated)?;
        }
        updated.updated_at = at;
        self.commit("loop.updated", serde_json::to_value(&updated).unwrap())?;
        Ok(json!({ "loop": updated }))
    }

    pub fn record_error(
        &mut self,
        code: &str,
        message: &str,
        context: Value,
        loop_id: Option<&str>,
    ) -> CommResult<Value> {
        validate_non_empty(code, "code")?;
        validate_non_empty(message, "message")?;
        let error = ErrorRecord {
            code: code.into(),
            message: message.into(),
            context,
            at: now(),
        };
        let updated_loop = if let Some(loop_id) = loop_id {
            let current = self.projection.loops.get(loop_id).cloned().ok_or_else(|| {
                CommError::new("loop_not_found", format!("loop not found: {loop_id}"))
            })?;
            if current.status != "active" {
                return Err(CommError::new(
                    "loop_not_active",
                    format!("loop is not active: {loop_id}"),
                ));
            }
            let mut updated = current;
            updated.status = "blocked".into();
            updated.phase = "blocked".into();
            updated.updated_at = error.at.clone();
            Some(updated)
        } else {
            None
        };
        let event_id = self.commit(
            "error.recorded",
            json!({ "error": error, "loop": updated_loop }),
        )?;
        let master_wake = if let Some(loop_record) = updated_loop.as_ref() {
            self.scope_master_address_unchecked(&loop_record.owner.scope_id)
                .map(|master| {
                    self.accumulate_master_wake(
                        &master,
                        loop_error_wake_signal(loop_record, &error),
                    )
                })
                .transpose()?
        } else {
            None
        };
        Ok(json!({
            "error": error,
            "loop": updated_loop,
            "masterWake": master_wake,
            "eventId": event_id
        }))
    }

    fn enqueue_message(
        &mut self,
        request: MessageRequest,
        route: RouteRecord,
        available_at_override: Option<&str>,
    ) -> CommResult<Value> {
        validate_message_request(&request)?;
        let priority = Priority::parse(&request.priority)?;
        let delivery_mode = DeliveryMode::parse(request.delivery_mode.as_deref())?;
        let created_at = request
            .created_at
            .as_deref()
            .map(validate_time)
            .transpose()?
            .unwrap_or_else(now);
        let message_id = request.message_id.clone().unwrap_or_else(|| new_id("msg"));
        let adapter_id = request
            .adapter_id
            .clone()
            .unwrap_or_else(|| "mailbox".into());
        if let Some(existing) = self.projection.messages.get(&message_id).cloned() {
            if message_matches_request(&existing, &request, &priority, &delivery_mode, &adapter_id)
            {
                return self.recover_idempotent_message(existing);
            }
            return Err(CommError::new(
                "message_id_conflict",
                format!("messageId already identifies a different message: {message_id}"),
            ));
        }
        self.adapter_for(&adapter_id, Some(&request.to))?;
        let message = MessageRecord {
            protocol: PROTOCOL.into(),
            message_id: message_id.clone(),
            conversation_id: request.conversation_id.unwrap_or_else(|| new_id("conv")),
            from: request.from,
            to: request.to,
            title: request.title,
            priority: priority.clone(),
            body: request.body,
            delivery_mode: delivery_mode.clone(),
            coalesce_key: request.coalesce_key,
            issue_id: request.issue_id,
            adapter_id,
            delivery_attempt_required: true,
            created_at: created_at.clone(),
            state: "created".into(),
            evidence: Vec::new(),
            route,
            last_error: None,
        };
        self.commit("message.created", serde_json::to_value(&message).unwrap())?;
        let accepted = DeliveryEvidence {
            state: "accepted".into(),
            at: created_at.clone(),
            details: json!({ "transport": "appsdk-internal", "durable": true }),
        };
        self.commit(
            "message.state",
            json!({
                "messageId": message_id,
                "state": "accepted",
                "evidence": accepted
            }),
        )?;
        let current = self
            .projection
            .messages
            .get(&message.message_id)
            .cloned()
            .unwrap();
        let delivery_attempt = self.ensure_message_delivery_attempt(&current)?;
        let notification = self.notification_for(&current, &created_at, available_at_override)?;
        let direct = notification
            .as_ref()
            .filter(|notification| notification.status == "emitted")
            .map(NotificationRecord::summary);
        Ok(json!({
            "message": current,
            "route": current.route,
            "deliveryAttempt": delivery_attempt,
            "notification": direct,
            "idempotent": false
        }))
    }

    fn recover_idempotent_message(&mut self, existing: MessageRecord) -> CommResult<Value> {
        let current = self.recover_message_state(existing)?;
        let delivery_attempt = self.ensure_message_delivery_attempt(&current)?;
        let notification = self.recover_message_notification(&current)?;
        let direct = notification
            .as_ref()
            .filter(|notification| notification.status == "emitted")
            .map(NotificationRecord::summary);
        Ok(json!({
            "message": current,
            "route": current.route,
            "deliveryAttempt": delivery_attempt,
            "notification": direct,
            "idempotent": true
        }))
    }

    fn recover_message_state(&mut self, existing: MessageRecord) -> CommResult<MessageRecord> {
        if existing.state == "created"
            && !existing
                .evidence
                .iter()
                .any(|evidence| evidence.state == "accepted")
        {
            let accepted = DeliveryEvidence {
                state: "accepted".into(),
                at: existing.created_at.clone(),
                details: json!({ "transport": "appsdk-internal", "durable": true, "recovered": true }),
            };
            self.commit(
                "message.state",
                json!({
                    "messageId": existing.message_id,
                    "state": "accepted",
                    "evidence": accepted
                }),
            )?;
        }
        self.projection
            .messages
            .get(&existing.message_id)
            .cloned()
            .ok_or_else(|| {
                CommError::new(
                    "message_recovery_failed",
                    format!(
                        "message disappeared during recovery: {}",
                        existing.message_id
                    ),
                )
            })
    }

    fn recover_message_notification(
        &mut self,
        message: &MessageRecord,
    ) -> CommResult<Option<NotificationRecord>> {
        self.ensure_message_delivery_attempt(message)?;
        let immediate = matches!(message.delivery_mode, DeliveryMode::Direct)
            || message.priority.is_breakthrough();
        let existing = if immediate {
            self.projection
                .notifications
                .iter()
                .find(|(_, notification)| notification.message_id == message.message_id)
                .map(|(key, notification)| (key.clone(), notification.clone()))
        } else {
            let key = self.message_notification_key(message);
            self.projection
                .notifications
                .get(&key)
                .cloned()
                .map(|notification| (key, notification))
        };
        let Some((key, notification)) = existing else {
            return self.notification_for(message, &message.created_at, None);
        };

        if !immediate {
            let current_ordinal = self
                .projection
                .message_ordinals
                .get(&notification.message_id)
                .ok_or_else(|| {
                    CommError::new(
                        "journal_corrupt",
                        format!(
                            "notification {key} references message without a durable creation fact: {}",
                            notification.message_id
                        ),
                    )
                })?;
            let requested_ordinal = self
                .projection
                .message_ordinals
                .get(&message.message_id)
                .ok_or_else(|| {
                    CommError::new(
                        "journal_corrupt",
                        format!(
                            "message {} has no durable creation fact for notification recovery",
                            message.message_id
                        ),
                    )
                })?;
            if notification.message_id == message.message_id {
                return Ok(Some(notification));
            }
            // The current projection is the latest state of the coalescing
            // bucket.  Compare replay-established message creation order:
            // unlike timestamps, it distinguishes a same-time new message
            // from a retry of an older message.  Missing order is corruption;
            // generation/time are not allowed to reconstruct this control
            // fact and silently swallow a newer message prefix.
            if current_ordinal > requested_ordinal {
                return Ok(Some(notification));
            }
            return self.notification_for(message, &message.created_at, None);
        }

        if matches!(notification.status.as_str(), "emitted" | "unknown") {
            return Ok(Some(notification));
        }

        self.retry_immediate_notification(message, &key, notification)
            .map(Some)
    }

    fn notification_for(
        &mut self,
        message: &MessageRecord,
        created_at: &str,
        available_at_override: Option<&str>,
    ) -> CommResult<Option<NotificationRecord>> {
        let immediate = matches!(message.delivery_mode, DeliveryMode::Direct)
            || message.priority.is_breakthrough();
        let adapter = if immediate {
            Some(self.adapter_for(&message.adapter_id, Some(&message.to))?)
        } else {
            None
        };
        let mut notification =
            self.build_notification(message, created_at, available_at_override)?;
        let existing_immediate = if immediate {
            self.projection
                .notifications
                .iter()
                .find(|(_, existing)| existing.message_id == message.message_id)
                .map(|(key, existing)| (key.clone(), existing.clone()))
        } else {
            None
        };
        let key = existing_immediate
            .as_ref()
            .map(|(key, _)| key.clone())
            .unwrap_or_else(|| self.notification_key(message, &notification, !immediate));
        let mut reused_pending = false;
        if let Some(existing) = self.projection.notifications.get(&key) {
            if matches!(
                existing.status.as_str(),
                "emitted" | "unknown" | "superseded"
            ) {
                if existing.message_id == message.message_id || immediate {
                    return Ok(Some(existing.clone()));
                }
                notification.generation = existing.generation.checked_add(1).ok_or_else(|| {
                    CommError::new(
                        "notification_generation_exhausted",
                        format!("notification generation exhausted: {key}"),
                    )
                })?;
            }
            if existing.status == "pending" {
                if immediate {
                    notification = existing.clone();
                    reused_pending = true;
                } else {
                    notification.available_at = existing.available_at.clone();
                    notification.generation = existing.generation;
                }
            }
        }
        let attempt = immediate
            .then(|| new_delivery_attempt(&message.adapter_id, "notification.emitted", None));
        let attempt_id = attempt.as_ref().map(|attempt| attempt.attempt_id.clone());
        let notification_id = notification.notification_id.clone();
        if !reused_pending {
            self.commit(
                "notification.queued",
                json!({ "key": key, "notification": notification }),
            )?;
        }
        if let Some(attempt) = attempt.as_ref() {
            self.commit(
                "notification.delivery_attempt",
                json!({
                    "attemptId": attempt_id.clone(),
                    "keys": [key],
                    "attempt": attempt
                }),
            )?;
        }
        if immediate {
            let adapter = adapter.expect("immediate notification adapter is initialized");
            let receipt = match adapter.deliver(message) {
                Ok(receipt) => receipt,
                Err(error) => {
                    let error = adapter_error(&error, &message.adapter_id, "notification.emitted");
                    if let Err(record_error) = self.record_notification_failure(
                        std::slice::from_ref(&key),
                        &message.adapter_id,
                        &error,
                        "notification.emitted",
                        json!({
                            "messageId": message.message_id,
                            "notificationKey": key,
                            "notificationId": notification_id,
                            "attemptId": attempt_id
                        }),
                    ) {
                        return Err(with_secondary_error(
                            error,
                            record_error,
                            "notification.delivery_failed",
                        ));
                    }
                    return Err(error);
                }
            };
            self.commit(
                "notification.emitted",
                json!({
                    "attemptId": attempt_id,
                    "keys": [key],
                    "at": created_at,
                    "receipt": receipt
                }),
            )?;
        }
        Ok(self.projection.notifications.get(&key).cloned())
    }

    fn retry_immediate_notification(
        &mut self,
        message: &MessageRecord,
        key: &str,
        existing: NotificationRecord,
    ) -> CommResult<NotificationRecord> {
        let adapter = match self.adapter_for(&message.adapter_id, Some(&message.to)) {
            Ok(adapter) => adapter,
            Err(error) => {
                let error = adapter_error(&error, &message.adapter_id, "notification.emitted");
                if let Err(record_error) = self.record_notification_failure(
                    std::slice::from_ref(&key.to_string()),
                    &message.adapter_id,
                    &error,
                    "notification.emitted",
                    json!({
                        "messageId": message.message_id,
                        "notificationKey": key,
                        "notificationId": existing.notification_id
                    }),
                ) {
                    return Err(with_secondary_error(
                        error,
                        record_error,
                        "notification.delivery_failed",
                    ));
                }
                return Err(error);
            }
        };
        let attempt = new_delivery_attempt(&message.adapter_id, "notification.emitted", None);
        let attempt_id = attempt.attempt_id.clone();
        let attempt_started_at = attempt.started_at.clone();
        let notification_id = existing.notification_id.clone();
        self.commit(
            "notification.queued",
            json!({ "key": key, "notification": existing }),
        )?;
        self.commit(
            "notification.delivery_attempt",
            json!({
                "attemptId": attempt_id.clone(),
                "keys": [key],
                "attempt": attempt
            }),
        )?;
        let receipt = match adapter.deliver(message) {
            Ok(receipt) => receipt,
            Err(error) => {
                let error = adapter_error(&error, &message.adapter_id, "notification.emitted");
                if let Err(record_error) = self.record_notification_failure(
                    std::slice::from_ref(&key.to_string()),
                    &message.adapter_id,
                    &error,
                    "notification.emitted",
                    json!({
                        "messageId": message.message_id,
                        "notificationKey": key,
                        "notificationId": notification_id,
                        "attemptId": attempt_id.clone()
                    }),
                ) {
                    return Err(with_secondary_error(
                        error,
                        record_error,
                        "notification.delivery_failed",
                    ));
                }
                return Err(error);
            }
        };
        self.commit(
            "notification.emitted",
            json!({
                "attemptId": attempt_id,
                "keys": [key],
                "at": attempt_started_at,
                "receipt": receipt
            }),
        )?;
        self.projection
            .notifications
            .get(key)
            .cloned()
            .ok_or_else(|| {
                CommError::new(
                    "notification_recovery_failed",
                    format!("notification disappeared during recovery: {key}"),
                )
            })
    }

    fn build_notification(
        &self,
        message: &MessageRecord,
        created_at: &str,
        available_at_override: Option<&str>,
    ) -> CommResult<NotificationRecord> {
        let immediate = matches!(message.delivery_mode, DeliveryMode::Direct)
            || message.priority.is_breakthrough();
        let available_at = if let Some(value) = available_at_override {
            validate_time(value)?
        } else if immediate {
            created_at.to_string()
        } else {
            add_seconds(created_at, DEFAULT_BATCH_WINDOW_SECONDS)?
        };
        Ok(NotificationRecord {
            notification_id: new_id("notification"),
            message_id: message.message_id.clone(),
            generation: 0,
            recipient: message.to.clone(),
            title: message.title.clone(),
            priority: message.priority.clone(),
            issue_id: message.issue_id.clone(),
            coalesce_key: message.coalesce_key.clone(),
            body: message.body.clone(),
            created_at: created_at.to_string(),
            available_at,
            status: "pending".into(),
            emitted_at: None,
            adapter_id: message.adapter_id.clone(),
            transport_receipt: None,
            last_error: None,
            delivery_attempt: None,
        })
    }

    fn notification_key(
        &self,
        message: &MessageRecord,
        notification: &NotificationRecord,
        force_coalesce: bool,
    ) -> String {
        if !force_coalesce {
            return notification.notification_id.clone();
        }
        self.message_notification_key(message)
    }

    fn message_notification_key(&self, message: &MessageRecord) -> String {
        structured_key(&[
            &message.from.key(),
            &message.to.key(),
            &message.adapter_id,
            message.coalesce_key.as_deref().unwrap_or("notification"),
        ])
    }

    fn wakeup_delivery_in_flight(&self, wakeup: &WakeupRecord) -> CommResult<bool> {
        let Some(idle_since) = wakeup.idle_since.as_deref() else {
            return Ok(false);
        };
        let idle_since = parse_time(idle_since)?;
        let daemon = Address {
            scope_id: "appsdk".into(),
            session_id: "daemon".into(),
        };
        let key = structured_key(&[
            &daemon.key(),
            &wakeup.address.key(),
            "mailbox",
            "master-idle",
        ]);
        let Some(notification) = self.projection.notifications.get(&key) else {
            return Ok(false);
        };
        if notification.status != "unknown" && notification.delivery_attempt.is_none() {
            return Ok(false);
        }
        Ok(parse_time(&notification.created_at)? >= idle_since)
    }

    fn prepare_wakeup_message(&mut self, expected: MessageRecord) -> CommResult<MessageRecord> {
        if let Some(existing) = self.projection.messages.get(&expected.message_id).cloned() {
            if !wakeup_message_matches(&existing, &expected) {
                return Err(CommError::new(
                    "wakeup_message_conflict",
                    format!(
                        "wakeup message id already identifies a different message: {}",
                        expected.message_id
                    ),
                ));
            }
            if existing.state == "created" {
                let accepted = DeliveryEvidence {
                    state: "accepted".into(),
                    at: existing.created_at.clone(),
                    details: json!({
                        "transport": "appsdk-internal",
                        "durable": true,
                        "recovered": true
                    }),
                };
                self.commit(
                    "message.state",
                    json!({
                        "messageId": existing.message_id,
                        "state": "accepted",
                        "evidence": accepted
                    }),
                )?;
            } else if delivery_state_rank(&existing.state).is_none() {
                return Err(CommError::new(
                    "wakeup_message_state_invalid",
                    format!(
                        "wakeup message {} has unsupported state: {}",
                        existing.message_id, existing.state
                    ),
                ));
            }
            let current = self
                .projection
                .messages
                .get(&expected.message_id)
                .cloned()
                .ok_or_else(|| {
                    CommError::new(
                        "message_recovery_failed",
                        format!(
                            "wakeup message disappeared during recovery: {}",
                            expected.message_id
                        ),
                    )
                })?;
            self.ensure_message_delivery_attempt(&current)?;
            return Ok(current);
        }

        let mut created = expected.clone();
        created.state = "created".into();
        created.evidence.clear();
        self.commit("message.created", serde_json::to_value(&created).unwrap())?;
        let accepted = expected.evidence.first().cloned().ok_or_else(|| {
            CommError::new(
                "message_evidence_missing",
                "system message accepted evidence missing",
            )
        })?;
        self.commit(
            "message.state",
            json!({
                "messageId": expected.message_id,
                "state": "accepted",
                "evidence": accepted
            }),
        )?;
        let current = self
            .projection
            .messages
            .get(&created.message_id)
            .cloned()
            .ok_or_else(|| {
                CommError::new(
                    "message_recovery_failed",
                    format!(
                        "wakeup message disappeared during creation: {}",
                        created.message_id
                    ),
                )
            })?;
        self.ensure_message_delivery_attempt(&current)?;
        Ok(self
            .projection
            .messages
            .get(&created.message_id)
            .cloned()
            .expect("wakeup message remains after delivery attempt"))
    }

    fn commit_wakeup_reminder(
        &mut self,
        wakeup: &WakeupRecord,
        message: &MessageRecord,
        notification_key: &str,
        notification: &NotificationRecord,
        attempt_id: Option<&str>,
        receipt: Option<&TransportReceipt>,
    ) -> CommResult<()> {
        self.commit(
            "wakeup.reminder",
            json!({
                "wakeup": wakeup,
                "message": message,
                "notificationKey": notification_key,
                "notification": notification,
                "attemptId": attempt_id,
                "receipt": receipt
            }),
        )
        .map(|_| ())
    }

    fn system_message(
        &self,
        target: &Address,
        title: String,
        priority: &str,
        body: &str,
        coalesce_key: &str,
        at: &str,
        adapter_id: &str,
    ) -> CommResult<MessageRecord> {
        Ok(MessageRecord {
            protocol: PROTOCOL.into(),
            message_id: new_id("msg"),
            conversation_id: new_id("conv"),
            from: Address {
                scope_id: "appsdk".into(),
                session_id: "daemon".into(),
            },
            to: target.clone(),
            title,
            priority: Priority::parse(priority)?,
            body: body.into(),
            delivery_mode: DeliveryMode::Idle,
            coalesce_key: Some(coalesce_key.into()),
            issue_id: None,
            adapter_id: adapter_id.into(),
            delivery_attempt_required: true,
            created_at: at.into(),
            state: "accepted".into(),
            evidence: vec![DeliveryEvidence {
                state: "accepted".into(),
                at: at.into(),
                details: json!({ "transport": "appsdk-internal", "source": "daemon" }),
            }],
            route: RouteRecord {
                mode: "system".into(),
                same_appserver: false,
                same_project: false,
                source_role: "daemon".into(),
                target_role: "master".into(),
            },
            last_error: None,
        })
    }

    fn system_notification(
        &mut self,
        target: &Address,
        title: String,
        priority: &Priority,
        body: &str,
        issue_id: Option<&str>,
        coalesce_key: &str,
        at: &str,
        available_at_override: Option<&str>,
        adapter_id: &str,
    ) -> CommResult<Value> {
        let mut message = self.system_message(
            target,
            title,
            &format_priority(priority),
            body,
            coalesce_key,
            at,
            adapter_id,
        )?;
        message.issue_id = issue_id.map(str::to_string);
        self.commit("message.created", serde_json::to_value(&message).unwrap())?;
        self.ensure_message_delivery_attempt(&message)?;
        let notification = self.notification_for(&message, at, available_at_override)?;
        Ok(json!({
            "message": message,
            "notification": notification.map(|value| value.summary())
        }))
    }

    fn require_adapter(&self, adapter_id: &str) -> CommResult<&AdapterRecord> {
        self.projection.adapters.get(adapter_id).ok_or_else(|| {
            CommError::new(
                "adapter_not_registered",
                format!("communication adapter not registered: {adapter_id}"),
            )
        })
    }

    fn adapter_for(
        &self,
        adapter_id: &str,
        recipient: Option<&Address>,
    ) -> CommResult<Box<dyn CommunicationAdapter>> {
        let record = self.require_adapter(adapter_id)?;
        if !record.enabled {
            return Err(CommError::new(
                "adapter_disabled",
                format!("communication adapter is disabled: {adapter_id}"),
            ));
        }
        if let Some(bound_recipient) = record.recipient.as_ref() {
            if recipient != Some(bound_recipient) {
                return Err(CommError::new(
                    "adapter_recipient_mismatch",
                    format!(
                        "adapter {adapter_id} is bound to {}, not {}",
                        bound_recipient.key(),
                        recipient
                            .map(Address::key)
                            .unwrap_or_else(|| "<none>".into())
                    ),
                ));
            }
        }
        let runtime = if record.kind == "mailbox" {
            None
        } else {
            let bound_recipient = record.recipient.as_ref().ok_or_else(|| {
                CommError::new(
                    "adapter_recipient_required",
                    format!("adapter {adapter_id} has no registered recipient address"),
                )
            })?;
            let agent = self.require_live_agent(bound_recipient)?.clone();
            let runtime = self.runtime_for_agent(&agent)?;
            let target = record
                .target
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    CommError::new(
                        if record.kind == "tmux" {
                            "tmux_target_required"
                        } else {
                            "appserver_target_required"
                        },
                        if record.kind == "tmux" {
                            "tmux adapter requires target pane"
                        } else {
                            "appserver adapter requires endpoint"
                        },
                    )
                })?;
            validate_adapter_runtime_target(&record.kind, target, &runtime, true)?;
            Some(runtime)
        };
        match record.kind.as_str() {
            "mailbox" => Ok(Box::new(MailboxAdapter {
                adapter_id: record.adapter_id.clone(),
                path: self.mailbox_path.clone(),
            })),
            "tmux" => {
                let target = record
                    .target
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        CommError::new("tmux_target_required", "tmux adapter requires target pane")
                    })?;
                Ok(Box::new(TmuxAdapter {
                    adapter_id: record.adapter_id.clone(),
                    runtime_id: runtime
                        .as_ref()
                        .expect("tmux adapter runtime was validated")
                        .identity
                        .runtime_id
                        .clone(),
                    target,
                    execute: record.execute,
                }))
            }
            "appserver" => {
                let endpoint = record
                    .target
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        CommError::new(
                            "appserver_target_required",
                            "appserver adapter requires endpoint",
                        )
                    })?;
                Ok(Box::new(AppserverAdapter {
                    adapter_id: record.adapter_id.clone(),
                    runtime_id: runtime
                        .as_ref()
                        .expect("appserver adapter runtime was validated")
                        .identity
                        .runtime_id
                        .clone(),
                    endpoint,
                    capability: APPSERVER_SEND_CAPABILITY.into(),
                }))
            }
            other => Err(CommError::new(
                "invalid_adapter_kind",
                format!("adapter kind is unsupported: {other}"),
            )),
        }
    }

    fn record_notification_failure(
        &mut self,
        keys: &[String],
        adapter_id: &str,
        error: &CommError,
        operation: &str,
        identity: Value,
    ) -> CommResult<()> {
        let record = adapter_error_record(error, adapter_id, operation);
        let mut data = json!({ "keys": keys, "adapterId": adapter_id, "operation": operation, "error": record });
        if let (Some(data), Some(identity)) = (data.as_object_mut(), identity.as_object()) {
            for (key, value) in identity {
                data.insert(key.clone(), value.clone());
            }
        }
        self.commit("notification.delivery_failed", data)
            .map(|_| ())
    }

    fn require_scope(&self, scope_id: &str) -> CommResult<&ScopeRecord> {
        self.projection.scopes.get(scope_id).ok_or_else(|| {
            CommError::new("scope_not_found", format!("scope not found: {scope_id}"))
        })
    }

    fn require_agent(&self, address: &Address) -> CommResult<&AgentRecord> {
        validate_address(address)?;
        if let Some(agent) = self.projection.agents.get(&address.key()) {
            return Ok(agent);
        }
        if let Some(tombstone) = self.projection.agent_tombstones.get(&address.key()) {
            let mut error = CommError::new(
                "agent_address_rebound",
                format!(
                    "agent address was rebound to {}",
                    tombstone.rebound_to.key()
                ),
            );
            error.context = json!({
                "oldAddress": tombstone.address,
                "newAddress": tombstone.rebound_to,
                "agentId": tombstone.agent_id,
                "runtimeId": tombstone.runtime_id,
                "reboundAt": tombstone.rebound_at
            });
            return Err(error);
        }
        Err(CommError::new(
            "agent_not_registered",
            format!("agent not registered: {}", address.key()),
        ))
    }

    fn require_live_agent(&self, address: &Address) -> CommResult<&AgentRecord> {
        let at = now();
        self.require_live_agent_at(address, &at)
    }

    fn require_live_agent_at(&self, address: &Address, at: &str) -> CommResult<&AgentRecord> {
        let agent = self.require_agent(address)?;
        if !agent.live_at(at) {
            return Err(CommError::new(
                "agent_lease_expired",
                format!("agent lease expired: {}", address.key()),
            ));
        }
        Ok(agent)
    }

    fn live_idle_master_at(&self, address: &Address, at: &str) -> Option<AgentRecord> {
        let agent = self.require_live_agent_at(address, at).ok()?;
        if agent.role != "master" || agent.state != AgentState::Idle {
            return None;
        }
        let scope = self.projection.scopes.get(&agent.scope_id)?;
        if scope.master_session_id.as_deref() != Some(agent.session_id.as_str()) {
            return None;
        }
        Some(agent.clone())
    }

    fn scope_master_address(&self, scope_id: &str) -> CommResult<Address> {
        let scope = self.require_scope(scope_id)?;
        let session_id = scope.master_session_id.clone().ok_or_else(|| {
            CommError::new(
                "master_not_registered",
                format!("scope has no master: {scope_id}"),
            )
        })?;
        let address = Address {
            scope_id: scope_id.into(),
            session_id,
        };
        self.require_live_agent(&address)?;
        Ok(address)
    }

    fn resolve_route(&self, source: &AgentRecord, target: &AgentRecord) -> CommResult<RouteRecord> {
        let source_scope = self.require_scope(&source.scope_id)?;
        let target_scope = self.resolve_scope_for_agent(target)?;
        let same_scope = source.scope_id == target.scope_id;
        let same_appserver = source_scope.appserver_id == target_scope.appserver_id;
        let same_project = source_scope.project_root == target_scope.project_root;
        if !same_scope {
            if source.role != "master"
                || source_scope.master_session_id.as_deref() != Some(source.session_id.as_str())
            {
                return Err(CommError::new(
                    "cross_scope_master_required",
                    "cross-scope communication requires the source scope master",
                ));
            }
            if target.role != "master"
                || target_scope.master_session_id.as_deref() != Some(target.session_id.as_str())
            {
                return Err(CommError::new(
                    "cross_scope_target_must_be_master",
                    "cross-scope target must be the target scope master session",
                ));
            }
            return Ok(RouteRecord {
                mode: "cross-scope-master".into(),
                same_appserver,
                same_project,
                source_role: source.role.clone(),
                target_role: target.role.clone(),
            });
        }
        if source.role == "subagent" && target.role == "subagent" {
            return Err(CommError::new(
                "subagent_to_subagent_forbidden",
                "subagents cannot communicate with another subagent",
            ));
        }
        if source.role == "subagent" && !self.parent_or_master_allows(source, target) {
            return Err(CommError::new(
                "subagent_parent_required",
                "subagent may communicate only with its parent or master ancestor",
            ));
        }
        if target.role == "subagent" && !self.parent_or_master_allows(target, source) {
            return Err(CommError::new(
                "subagent_parent_required",
                "peer may communicate only with its bound subagent",
            ));
        }
        if source.role == "peer" && target.role == "peer" && !(same_appserver && same_project) {
            return Err(CommError::new(
                "peer_scope_forbidden",
                "peers may communicate only inside the same App Server and project",
            ));
        }
        let mode = if source.role == "master" || target.role == "master" {
            "same-scope-master"
        } else if source.role == "subagent" || target.role == "subagent" {
            "same-scope-parent"
        } else {
            "same-scope-peer"
        };
        Ok(RouteRecord {
            mode: mode.into(),
            same_appserver,
            same_project,
            source_role: source.role.clone(),
            target_role: target.role.clone(),
        })
    }

    fn parent_or_master_allows(&self, child: &AgentRecord, other: &AgentRecord) -> bool {
        if child
            .parent
            .as_ref()
            .is_some_and(|parent| self.addresses_match_after_rebind(parent, &other.address()))
        {
            return true;
        }
        if other.role != "master" {
            return false;
        }
        let mut current = child.parent.clone();
        while let Some(parent) = current {
            if self.addresses_match_after_rebind(&parent, &other.address()) {
                return true;
            }
            current = self
                .canonical_address(&parent)
                .and_then(|address| self.projection.agents.get(&address.key()))
                .and_then(|agent| agent.parent.clone());
        }
        self.projection
            .scopes
            .get(&child.scope_id)
            .and_then(|scope| scope.master_session_id.as_ref())
            .is_some_and(|master| master == &other.session_id)
    }

    fn canonical_address(&self, address: &Address) -> Option<Address> {
        let mut current = address.clone();
        let mut visited = BTreeMap::new();
        while let Some(tombstone) = self.projection.agent_tombstones.get(&current.key()) {
            if visited.insert(current.key(), true).is_some() {
                return None;
            }
            current = tombstone.rebound_to.clone();
        }
        Some(current)
    }

    fn addresses_match_after_rebind(&self, left: &Address, right: &Address) -> bool {
        self.canonical_address(left)
            .zip(self.canonical_address(right))
            .is_some_and(|(left, right)| left == right)
    }

    fn commit(&mut self, kind: &str, data: Value) -> CommResult<String> {
        let event = EventRecord {
            protocol: PROTOCOL.into(),
            event_id: new_id("event"),
            at: now(),
            kind: kind.into(),
            data,
        };
        let line = serde_json::to_string(&event)
            .map_err(|error| CommError::new("journal_encode_failed", error.to_string()))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.mailbox_path)
            .map_err(|error| {
                CommError::new(
                    "journal_write_failed",
                    format!("{}: {error}", self.mailbox_path.display()),
                )
            })?;
        let record = format!("{line}\n");
        file.write_all(record.as_bytes())
            .map_err(|error| CommError::new("journal_write_failed", error.to_string()))?;
        file.sync_data()
            .map_err(|error| CommError::new("journal_sync_failed", error.to_string()))?;
        self.apply_event(&event)?;
        Ok(event.event_id)
    }

    fn record_error_event(&mut self, error: &CommError) -> CommResult<()> {
        let record = ErrorRecord {
            code: error.code.clone(),
            message: error.message.clone(),
            context: error.context.clone(),
            at: now(),
        };
        self.commit("error.recorded", serde_json::to_value(record).unwrap())
            .map(|_| ())
    }

    fn replay(&mut self) -> CommResult<()> {
        let text = match fs::read_to_string(&self.mailbox_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(CommError::new(
                    "journal_read_failed",
                    format!("{}: {error}", self.mailbox_path.display()),
                ))
            }
        };
        if !text.is_empty() && !text.ends_with('\n') {
            return Err(CommError::new(
                "journal_corrupt",
                "communication JSONL must end with a newline",
            ));
        }
        let mut event_ids = BTreeMap::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                return Err(CommError::new(
                    "journal_corrupt",
                    format!("empty JSONL line at line {}", index + 1),
                ));
            }
            let event: EventRecord = serde_json::from_str(line).map_err(|error| {
                CommError::new(
                    "journal_corrupt",
                    format!("invalid JSONL at line {}: {error}", index + 1),
                )
            })?;
            validate_event_envelope(&event).map_err(|error| {
                CommError::new(
                    "journal_corrupt",
                    format!("invalid envelope at line {}: {error}", index + 1),
                )
            })?;
            if let Some(previous_line) = event_ids.insert(event.event_id.clone(), index + 1) {
                return Err(CommError::new(
                    "journal_corrupt",
                    format!(
                        "duplicate eventId {} at line {} (already present at line {})",
                        event.event_id,
                        index + 1,
                        previous_line
                    ),
                ));
            }
            if event.protocol != PROTOCOL {
                return Err(CommError::new(
                    "journal_protocol_mismatch",
                    format!("unsupported communication protocol at line {}", index + 1),
                ));
            }
            self.apply_event(&event).map_err(|error| {
                CommError::new(
                    "journal_corrupt",
                    format!("invalid event at line {}: {}", index + 1, error.message),
                )
            })?;
        }
        for notification in self.projection.notifications.values_mut() {
            if notification.delivery_attempt.is_some() {
                notification.status = "unknown".into();
            }
        }
        Ok(())
    }

    fn replay_identity_only(&mut self) -> CommResult<()> {
        let text = fs::read_to_string(&self.mailbox_path).map_err(|error| {
            CommError::new(
                "journal_read_failed",
                format!("{}: {error}", self.mailbox_path.display()),
            )
        })?;
        if !text.is_empty() && !text.ends_with('\n') {
            return Err(CommError::new(
                "journal_corrupt",
                "communication JSONL must end with a newline",
            ));
        }
        let mut event_ids = BTreeMap::new();
        for (index, line) in text.lines().enumerate() {
            let line_number = index + 1;
            if line.trim().is_empty() {
                return Err(CommError::new(
                    "journal_corrupt",
                    format!("empty JSONL line at line {line_number}"),
                ));
            }
            let event: EventRecord = serde_json::from_str(line).map_err(|error| {
                CommError::new(
                    "journal_corrupt",
                    format!("invalid JSONL at line {line_number}: {error}"),
                )
            })?;
            validate_event_envelope(&event).map_err(|error| {
                CommError::new(
                    "journal_corrupt",
                    format!("invalid envelope at line {line_number}: {error}"),
                )
            })?;
            if event.protocol != PROTOCOL {
                return Err(CommError::new(
                    "journal_protocol_mismatch",
                    format!("unsupported communication protocol at line {line_number}"),
                ));
            }
            if event_ids
                .insert(event.event_id.clone(), line_number)
                .is_some()
            {
                return Err(CommError::new(
                    "journal_corrupt",
                    format!("duplicate eventId at line {line_number}"),
                ));
            }
            match event.kind.as_str() {
                "scope.registered" | "scope.unregistered" | "agent.registered" | "agent.state"
                | "agent.refreshed" | "agent.rebound" => {
                    self.apply_event(&event).map_err(|error| {
                        CommError::new(
                            "journal_corrupt",
                            format!(
                                "invalid identity event at line {line_number}: {}",
                                error.message
                            ),
                        )
                    })?
                }
                "discovery.pending" => {
                    let _: DiscoveryPendingRecord = decode(&event.data, "discovery pending")
                        .map_err(|error| {
                            CommError::new(
                                "journal_corrupt",
                                format!(
                                    "invalid discovery pending event at line {line_number}: {}",
                                    error.message
                                ),
                            )
                        })?;
                }
                "discovery.reconciled" => {
                    if event
                        .data
                        .get("pendingId")
                        .and_then(Value::as_str)
                        .is_none_or(|value| value.trim().is_empty())
                    {
                        return Err(CommError::new(
                            "journal_corrupt",
                            format!(
                                "invalid discovery reconciled event at line {line_number}: pendingId missing"
                            ),
                        ));
                    }
                }
                "adapter.registered"
                | "message.created"
                | "message.delivery_attempt"
                | "message.state"
                | "notification.queued"
                | "notification.superseded"
                | "notification.delivery_attempt"
                | "notification.emitted"
                | "notification.batch_emitted"
                | "notification.delivery_failed"
                | "wakeup.updated"
                | "master_wake.updated"
                | "master_wake.decided"
                | "master_wake.briefing"
                | "wakeup.reminder"
                | "bug.reported"
                | "bug.updated"
                | "loop.created"
                | "loop.updated"
                | "error.recorded" => {}
                other => {
                    return Err(CommError::new(
                        "journal_unknown_event",
                        format!("unknown communication event: {other}"),
                    ));
                }
            }
        }
        Ok(())
    }

    fn apply_master_wake_decided_event(&mut self, data: &Value) -> CommResult<()> {
        let accumulator =
            decode_master_wake_accumulator_event(require_event_field(data, "accumulator")?)?;
        let action = require_event_field(data, "action")?
            .as_str()
            .ok_or_else(|| {
                CommError::new("event_data_invalid", "master wake action is not a string")
            })?;
        let at = require_event_field(data, "at")?.as_str().ok_or_else(|| {
            CommError::new(
                "event_data_invalid",
                "master wake decision at is not a string",
            )
        })?;
        validate_time(at)?;
        let previous = self
            .projection
            .master_wake
            .get(&accumulator.address.key())
            .cloned()
            .ok_or_else(|| {
                CommError::new(
                    "event_data_invalid",
                    format!(
                        "master wake decision has no prior accumulator: {}",
                        accumulator.address.key()
                    ),
                )
            })?;
        let action = action.trim().to_ascii_lowercase();
        let generation_is_new_schedule = action == "schedule"
            && previous.pending
            && previous
                .generation
                .checked_add(1)
                .is_some_and(|generation| generation == accumulator.generation);
        let generation_mismatch = if action == "schedule" && previous.pending {
            !generation_is_new_schedule
        } else {
            previous.generation != accumulator.generation
        };
        if generation_mismatch {
            return Err(CommError::new(
                "event_data_invalid",
                "master wake decision generation does not match prior accumulator",
            ));
        }
        match action.as_str() {
            "hold" => {
                if !accumulator.held || accumulator.next_due_at.is_some() {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "held master wake decision must remain held without a due time",
                    ));
                }
            }
            "dispatch" | "handled" | "complete" | "completed" => {
                if accumulator.pending || !accumulator.signals.is_empty() {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "terminal master wake decision must clear active signals",
                    ));
                }
            }
            "schedule" => {
                if accumulator.held
                    || accumulator.stopped
                    || accumulator.reminders_sent != 0
                    || accumulator.last_briefing_generation.is_some()
                    || accumulator.last_briefing_at.is_some()
                    || (previous.pending && !generation_is_new_schedule)
                    || accumulator.pending != previous.pending
                    || accumulator.first_observed_at != previous.first_observed_at
                    || accumulator.last_observed_at != previous.last_observed_at
                    || accumulator.signals != previous.signals
                    || accumulator.consumed_signals != previous.consumed_signals
                    || (!accumulator.pending && accumulator.next_due_at.is_some())
                    || (accumulator.pending && accumulator.next_due_at.is_none())
                {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "scheduled master wake decision has inconsistent state",
                    ));
                }
            }
            other => {
                return Err(CommError::new(
                    "event_data_invalid",
                    format!("unsupported master wake decision action: {other}"),
                ))
            }
        }
        let key = accumulator.address.key();
        self.projection
            .master_wake
            .insert(key.clone(), accumulator.clone());
        if let Some(wakeup) = self.projection.wakeup.get(&key).cloned() {
            let synchronized = synchronize_master_wakeup(wakeup, &action, &accumulator);
            self.projection.wakeup.insert(key, synchronized);
        }
        Ok(())
    }

    fn apply_master_wake_briefing_event(&mut self, data: &Value) -> CommResult<()> {
        let accumulator =
            decode_master_wake_accumulator_event(require_event_field(data, "accumulator")?)?;
        let message: MessageRecord = decode(require_event_field(data, "message")?, "message")?;
        let notification: NotificationRecord =
            decode(require_event_field(data, "notification")?, "notification")?;
        let generation = require_event_field(data, "generation")?
            .as_u64()
            .ok_or_else(|| {
                CommError::new(
                    "event_data_invalid",
                    "master wake generation is not an integer",
                )
            })?;
        let reminder = require_event_field(data, "reminder")?
            .as_u64()
            .ok_or_else(|| {
                CommError::new(
                    "event_data_invalid",
                    "master wake reminder is not an integer",
                )
            })?;
        let reminder = u8::try_from(reminder).map_err(|_| {
            CommError::new("event_data_invalid", "master wake reminder is out of range")
        })?;
        if generation != accumulator.generation
            || reminder == 0
            || reminder > DEFAULT_MASTER_REMINDER_LIMIT
            || accumulator.reminders_sent != reminder
            || accumulator.last_briefing_generation != Some(generation)
        {
            return Err(CommError::new(
                "event_data_invalid",
                "master wake briefing generation or reminder is inconsistent",
            ));
        }
        let (message_id, conversation_id) = master_wake_message_identity(&accumulator, reminder);
        self.validate_master_wake_message_identity(
            &message,
            &accumulator,
            reminder,
            &accumulator.address,
            &conversation_id,
        )?;
        if message.message_id != message_id {
            return Err(CommError::new(
                "event_data_invalid",
                "master wake briefing message identity is inconsistent",
            ));
        }
        let projected_message = self
            .projection
            .messages
            .get(&message.message_id)
            .ok_or_else(|| {
                CommError::new(
                    "event_data_invalid",
                    format!("master wake briefing message is not durable: {message_id}"),
                )
            })?;
        if serde_json::to_value(projected_message).unwrap()
            != serde_json::to_value(&message).unwrap()
        {
            return Err(CommError::new(
                "event_data_invalid",
                "master wake briefing message does not match its durable record",
            ));
        }
        if notification.message_id != message.message_id
            || notification.recipient != accumulator.address
            || notification.status != "emitted"
            || notification.delivery_attempt.is_some()
        {
            return Err(CommError::new(
                "event_data_invalid",
                "master wake briefing notification is not terminal for its message",
            ));
        }
        let (notification_key, projected_notification) = self
            .projection
            .notifications
            .iter()
            .find(|(_, value)| value.notification_id == notification.notification_id)
            .or_else(|| {
                self.projection
                    .notifications
                    .iter()
                    .find(|(_, value)| value.message_id == notification.message_id)
            })
            .ok_or_else(|| {
                CommError::new(
                    "event_data_invalid",
                    format!(
                        "master wake briefing notification is not durable: {}",
                        notification.notification_id
                    ),
                )
            })?;
        if serde_json::to_value(projected_notification).unwrap()
            != serde_json::to_value(&notification).unwrap()
        {
            return Err(CommError::new(
                "event_data_invalid",
                "master wake briefing notification does not match its durable record",
            ));
        }
        if !self
            .projection
            .completed_attempts
            .contains_key(notification_key)
        {
            return Err(CommError::new(
                "event_data_invalid",
                "master wake briefing lacks a completed delivery attempt",
            ));
        }
        self.projection
            .master_wake
            .insert(accumulator.address.key(), accumulator);
        Ok(())
    }

    fn apply_agent_rebound_event(&mut self, data: &Value) -> CommResult<()> {
        let rebound: AgentReboundEvent = decode(data, "agent rebound")?;
        for (field, address) in [
            ("from", rebound.from.address()),
            ("to", rebound.to.address()),
            ("tombstone.address", rebound.tombstone.address.clone()),
            ("tombstone.reboundTo", rebound.tombstone.rebound_to.clone()),
        ] {
            validate_address(&address).map_err(|error| {
                CommError::new(
                    "event_data_invalid",
                    format!(
                        "agent rebound {field} address is invalid: {}",
                        error.message
                    ),
                )
            })?;
        }
        let from_key = rebound.from.address().key();
        let to_key = rebound.to.address().key();

        if from_key == to_key {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound must change the session address",
            ));
        }
        if rebound.from.scope_id != rebound.to.scope_id {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound must keep the same scope",
            ));
        }
        if rebound.from.agent_id != rebound.to.agent_id
            || rebound.from.role != rebound.to.role
            || rebound.from.master_grant != rebound.to.master_grant
            || rebound.from.parent != rebound.to.parent
            || rebound.from.lease_ms != rebound.to.lease_ms
            || rebound.from.registered_at != rebound.to.registered_at
            || rebound.from.state != rebound.to.state
            || rebound.from.last_state_at != rebound.to.last_state_at
            || rebound.from.runtime_id != rebound.to.runtime_id
        {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound changed stable agent identity or logical state",
            ));
        }
        let runtime_id = rebound.from.runtime_id.as_deref().ok_or_else(|| {
            CommError::new(
                "event_data_invalid",
                "agent rebound requires a verified runtime identity",
            )
        })?;
        if rebound.tombstone.address != rebound.from.address()
            || rebound.tombstone.rebound_to != rebound.to.address()
            || rebound.tombstone.agent_id != rebound.from.agent_id
            || rebound.tombstone.runtime_id != runtime_id
            || rebound.tombstone.rebound_at != rebound.to.last_observed_at
        {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound tombstone does not match the before and after records",
            ));
        }
        validate_time(&rebound.to.last_observed_at)?;
        let expected_expires =
            add_millis(&rebound.to.last_observed_at, rebound.to.lease_ms as i64)?;
        if rebound.to.expires_at != expected_expires {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound lease expiry does not match the rebind observation time",
            ));
        }

        let scope = self
            .projection
            .scopes
            .get(&rebound.from.scope_id)
            .ok_or_else(|| {
                CommError::new("event_data_invalid", "agent rebound scope is missing")
            })?;
        if !scope.session_ids.is_empty() && !scope.session_ids.contains(&rebound.to.session_id) {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound target session is not declared in the scope",
            ));
        }
        let current = self.projection.agents.get(&from_key).ok_or_else(|| {
            CommError::new("event_data_invalid", "agent rebound source is missing")
        })?;
        if current != &rebound.from {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound source does not match the current projection",
            ));
        }
        if self.projection.agents.contains_key(&to_key)
            || self.projection.agent_tombstones.contains_key(&to_key)
            || self.projection.agent_tombstones.contains_key(&from_key)
        {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound source or target is already tombstoned or occupied",
            ));
        }
        if rebound.from.role == "master"
            && scope.master_session_id.as_deref() != Some(rebound.from.session_id.as_str())
        {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound master source does not match the scope master",
            ));
        }

        self.projection.agents.remove(&from_key);
        self.projection
            .agents
            .insert(to_key.clone(), rebound.to.clone());
        self.projection
            .agent_tombstones
            .insert(from_key.clone(), rebound.tombstone.clone());

        if rebound.from.role == "master" {
            if let Some(scope) = self.projection.scopes.get_mut(&rebound.from.scope_id) {
                scope.master_session_id = Some(rebound.to.session_id.clone());
            }
            if let Some(mut accumulator) = self.projection.master_wake.remove(&from_key) {
                accumulator.address = rebound.to.address();
                self.projection
                    .master_wake
                    .insert(to_key.clone(), accumulator);
            }
            if let Some(mut wakeup) = self.projection.wakeup.remove(&from_key) {
                wakeup.address = rebound.to.address();
                self.projection.wakeup.insert(to_key, wakeup);
            }
        }
        Ok(())
    }

    fn apply_event(&mut self, event: &EventRecord) -> CommResult<()> {
        self.projection.event_ordinal =
            self.projection
                .event_ordinal
                .checked_add(1)
                .ok_or_else(|| {
                    CommError::new("journal_corrupt", "communication event ordinal exhausted")
                })?;
        let event_ordinal = self.projection.event_ordinal;
        match event.kind.as_str() {
            "scope.registered" => {
                let record: ScopeRecord = decode(&event.data, "scope")?;
                self.validate_project_root(&record.project_root)?;
                self.projection
                    .scopes
                    .insert(record.scope_id.clone(), record);
            }
            "scope.unregistered" => {
                let scope_id = event
                    .data
                    .get("scopeId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("event_data_invalid", "scopeId missing"))?;
                self.projection.scopes.remove(scope_id);
            }
            "adapter.registered" => {
                let record: AdapterRecord = decode(&event.data, "adapter")?;
                self.projection
                    .adapters
                    .insert(record.adapter_id.clone(), record);
            }
            "agent.registered" => {
                let record: AgentRecord = decode(&event.data, "agent")?;
                let key = record.address().key();
                if self.projection.agent_tombstones.contains_key(&key) {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "agent registration attempts to reuse a rebound address",
                    ));
                }
                if record.role == "master" {
                    if let Some(scope) = self.projection.scopes.get_mut(&record.scope_id) {
                        scope.master_session_id = Some(record.session_id.clone());
                    }
                }
                self.projection.agents.insert(key, record);
            }
            "agent.state" => {
                let address: Address =
                    decode(event.data.get("address").unwrap_or(&Value::Null), "address")?;
                let state: AgentState =
                    decode(event.data.get("state").unwrap_or(&Value::Null), "state")?;
                let at = event
                    .data
                    .get("at")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "agent state at missing")
                    })?;
                let agent = self
                    .projection
                    .agents
                    .get_mut(&address.key())
                    .ok_or_else(|| CommError::new("event_data_invalid", "agent not found"))?;
                agent.state = state;
                agent.last_state_at = at.into();
            }
            "agent.refreshed" => {
                let record: AgentRecord = decode(&event.data, "agent")?;
                if self
                    .projection
                    .agent_tombstones
                    .contains_key(&record.address().key())
                {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "agent refresh attempts to update a rebound address",
                    ));
                }
                self.projection
                    .agents
                    .insert(record.address().key(), record);
            }
            "agent.rebound" => self.apply_agent_rebound_event(&event.data)?,
            "discovery.pending" => {
                let pending: DiscoveryPendingRecord = decode(&event.data, "discovery pending")?;
                if self
                    .projection
                    .discovery_pending
                    .insert(pending.pending_id.clone(), pending)
                    .is_some()
                {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "discovery pending operation is duplicated",
                    ));
                }
            }
            "discovery.reconciled" => {
                let pending_id = event
                    .data
                    .get("pendingId")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            "discovery reconciled pendingId missing",
                        )
                    })?;
                if self
                    .projection
                    .discovery_pending
                    .remove(pending_id)
                    .is_none()
                {
                    return Err(CommError::new(
                        "event_data_invalid",
                        format!("discovery pending operation is missing: {pending_id}"),
                    ));
                }
            }
            "message.created" => {
                let record: MessageRecord = decode(&event.data, "message")?;
                self.projection
                    .message_ordinals
                    .entry(record.message_id.clone())
                    .or_insert(event_ordinal);
                self.projection
                    .messages
                    .insert(record.message_id.clone(), record);
            }
            "message.delivery_attempt" => {
                let message_id = event
                    .data
                    .get("messageId")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            "message delivery attempt messageId missing",
                        )
                    })?;
                let attempt: MessageDeliveryAttempt = decode(
                    event.data.get("attempt").unwrap_or(&Value::Null),
                    "message delivery attempt",
                )?;
                if attempt.message_id != message_id {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "message delivery attempt messageId does not match attempt record",
                    ));
                }
                let message = self
                    .projection
                    .messages
                    .get(message_id)
                    .cloned()
                    .ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            format!("message delivery attempt message not found: {message_id}"),
                        )
                    })?;
                let target = self.resolve_agent(&message.to).map_err(|error| {
                    CommError::new(
                        "event_data_invalid",
                        format!(
                            "message delivery attempt target agent is missing: {}",
                            error.message
                        ),
                    )
                })?;
                let runtime_id = target.runtime_id.as_deref().ok_or_else(|| {
                    CommError::new(
                        "event_data_invalid",
                        "message delivery attempt target has no runtime identity",
                    )
                })?;
                let runtime = global_registry::runtime(runtime_id)
                    .map_err(|error| CommError::new("event_data_invalid", error))?;
                let adapter = self.require_adapter(&message.adapter_id)?;
                validate_message_delivery_attempt(
                    &attempt, &message, &target, &runtime, adapter, false,
                )?;
                if let Some(existing) = self.projection.message_delivery_attempts.get(message_id) {
                    if existing != &attempt {
                        return Err(CommError::new(
                            "event_data_invalid",
                            format!("message delivery attempt already differs: {message_id}"),
                        ));
                    }
                    return Ok(());
                }
                self.projection
                    .message_delivery_attempts
                    .insert(message_id.into(), attempt);
            }
            "message.state" => {
                let message_id = event
                    .data
                    .get("messageId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("event_data_invalid", "messageId missing"))?;
                let state = event
                    .data
                    .get("state")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("event_data_invalid", "message state missing"))?;
                let evidence: DeliveryEvidence = decode(
                    event.data.get("evidence").unwrap_or(&Value::Null),
                    "evidence",
                )?;
                if evidence.state != state {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "message state evidence does not match message state",
                    ));
                }
                let message_record = self
                    .projection
                    .messages
                    .get(message_id)
                    .cloned()
                    .ok_or_else(|| CommError::new("event_data_invalid", "message not found"))?;
                let target = self.resolve_agent(&message_record.to).map_err(|error| {
                    CommError::new(
                        "event_data_invalid",
                        format!("message target agent is missing: {}", error.message),
                    )
                })?;
                let adapter = self.require_adapter(&message_record.adapter_id)?.clone();
                let attempt_id_value = event.data.get("attemptId");
                let nonce_value = event.data.get("nonce");
                if attempt_id_value.is_some_and(|value| !value.is_string()) {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "message state attemptId must be a string",
                    ));
                }
                if nonce_value.is_some_and(|value| !value.is_string()) {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "message state nonce must be a string",
                    ));
                }
                let attempt_id = attempt_id_value.and_then(Value::as_str);
                let nonce = nonce_value.and_then(Value::as_str);
                validate_replayed_delivery_evidence(
                    state,
                    &evidence,
                    &target,
                    message_id,
                    &message_record,
                    self.projection.message_delivery_attempts.get(message_id),
                    attempt_id,
                    nonce,
                    &adapter,
                )?;
                let message = self
                    .projection
                    .messages
                    .get_mut(message_id)
                    .ok_or_else(|| CommError::new("event_data_invalid", "message not found"))?;
                validate_delivery_state_transition(&message.state, state)?;
                message.state = state.into();
                message.evidence.push(evidence);
            }
            "notification.queued" => {
                let key = event
                    .data
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification key missing")
                    })?;
                let notification: NotificationRecord = decode(
                    event.data.get("notification").unwrap_or(&Value::Null),
                    "notification",
                )?;
                self.projection
                    .notifications
                    .insert(key.into(), notification);
            }
            "notification.superseded" => {
                let keys = event
                    .data
                    .get("keys")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification superseded keys missing")
                    })?;
                for value in keys {
                    let key = value.as_str().ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            "notification superseded key is not a string",
                        )
                    })?;
                    let notification =
                        self.projection.notifications.get_mut(key).ok_or_else(|| {
                            CommError::new(
                                "event_data_invalid",
                                format!("notification key not found: {key}"),
                            )
                        })?;
                    if notification.status == "pending" {
                        notification.status = "superseded".into();
                        notification.delivery_attempt = None;
                    }
                }
            }
            "notification.delivery_attempt" => {
                let keys = event
                    .data
                    .get("keys")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "delivery attempt keys missing")
                    })?;
                if keys.is_empty() {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "delivery attempt keys must not be empty",
                    ));
                }
                let attempt_id = event
                    .data
                    .get("attemptId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "delivery attempt id missing")
                    })?;
                let attempt: DeliveryAttempt = decode(
                    event.data.get("attempt").unwrap_or(&Value::Null),
                    "delivery attempt",
                )?;
                if attempt_id != attempt.attempt_id {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "delivery attempt id does not match attempt record",
                    ));
                }
                if !matches!(
                    attempt.operation.as_str(),
                    "notification.emitted" | "notification.batch_emitted"
                ) {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "delivery attempt operation is unsupported",
                    ));
                }
                if (attempt.operation == "notification.batch_emitted") != attempt.batch_id.is_some()
                {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "delivery attempt batch id does not match operation",
                    ));
                }
                for value in keys {
                    let key = value.as_str().ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            "delivery attempt notification key is not a string",
                        )
                    })?;
                    let notification =
                        self.projection.notifications.get_mut(key).ok_or_else(|| {
                            CommError::new(
                                "event_data_invalid",
                                format!("notification key not found: {key}"),
                            )
                        })?;
                    if notification.adapter_id != attempt.adapter_id {
                        return Err(CommError::new(
                            "event_data_invalid",
                            format!(
                                "delivery attempt adapter mismatch for notification key: {key}"
                            ),
                        ));
                    }
                    notification.delivery_attempt = Some(attempt.clone());
                    notification.status = "pending".into();
                }
            }
            "notification.emitted" => {
                let keys = event
                    .data
                    .get("keys")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification keys missing")
                    })?;
                let at = event
                    .data
                    .get("at")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification emitted at missing")
                    })?;
                let receipt = event
                    .data
                    .get("receipt")
                    .map(|value| decode::<TransportReceipt>(value, "receipt"))
                    .transpose()?;
                let attempt_id = event
                    .data
                    .get("attemptId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            "notification emitted attempt id missing or is not a string",
                        )
                    })?;
                let completed_attempt_id = attempt_id.to_owned();
                for value in keys {
                    let key = value.as_str().ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification key is not a string")
                    })?;
                    {
                        let notification =
                            self.projection.notifications.get_mut(key).ok_or_else(|| {
                                CommError::new(
                                    "event_data_invalid",
                                    format!("notification key not found: {key}"),
                                )
                            })?;
                        validate_terminal_attempt(
                            notification,
                            Some(attempt_id),
                            "notification.emitted",
                        )?;
                        notification.status = "emitted".into();
                        notification.emitted_at = Some(at.into());
                        notification.delivery_attempt = None;
                        if let Some(receipt) = receipt.clone() {
                            notification.transport_receipt = Some(receipt);
                        }
                    }
                    self.projection
                        .completed_attempts
                        .insert(key.into(), completed_attempt_id.clone());
                }
            }
            "notification.batch_emitted" => {
                let batch: NotificationBatch =
                    decode(event.data.get("batch").unwrap_or(&Value::Null), "batch")?;
                let keys = event
                    .data
                    .get("notificationKeys")
                    .or_else(|| event.data.get("notificationIds"))
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification keys missing")
                    })?;
                let at = event
                    .data
                    .get("at")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification batch at missing")
                    })?;
                let receipt = event
                    .data
                    .get("receipt")
                    .map(|value| decode::<TransportReceipt>(value, "receipt"))
                    .transpose()?;
                let attempt_id = event
                    .data
                    .get("attemptId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            "notification batch attempt id missing or is not a string",
                        )
                    })?;
                let completed_attempt_id = attempt_id.to_owned();
                for value in keys {
                    let key = value.as_str().ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification key is not a string")
                    })?;
                    {
                        let notification = if let Some(notification) =
                            self.projection.notifications.get_mut(key)
                        {
                            notification
                        } else {
                            self.projection
                                .notifications
                                .values_mut()
                                .find(|notification| notification.notification_id == key)
                                .ok_or_else(|| {
                                    CommError::new(
                                        "event_data_invalid",
                                        format!("notification key not found: {key}"),
                                    )
                                })?
                        };
                        validate_terminal_attempt(
                            notification,
                            Some(attempt_id),
                            "notification.batch_emitted",
                        )?;
                        notification.status = "emitted".into();
                        notification.emitted_at = Some(at.into());
                        notification.delivery_attempt = None;
                        if let Some(receipt) = receipt.clone() {
                            notification.transport_receipt = Some(receipt);
                        }
                    }
                    self.projection
                        .completed_attempts
                        .insert(key.into(), completed_attempt_id.clone());
                }
                self.projection.batches.push(batch);
            }
            "wakeup.updated" => {
                let wakeup: WakeupRecord = decode(&event.data, "wakeup")?;
                self.projection.wakeup.insert(wakeup.address.key(), wakeup);
            }
            "master_wake.updated" => {
                let accumulator = decode_master_wake_accumulator_event(&event.data)?;
                self.projection
                    .master_wake
                    .insert(accumulator.address.key(), accumulator);
            }
            "master_wake.decided" => {
                self.apply_master_wake_decided_event(&event.data)?;
            }
            "master_wake.briefing" => {
                self.apply_master_wake_briefing_event(&event.data)?;
            }
            "wakeup.reminder" => {
                let wakeup: WakeupRecord =
                    decode(event.data.get("wakeup").unwrap_or(&Value::Null), "wakeup")?;
                let message: MessageRecord =
                    decode(event.data.get("message").unwrap_or(&Value::Null), "message")?;
                let notification: NotificationRecord = decode(
                    event.data.get("notification").unwrap_or(&Value::Null),
                    "notification",
                )?;
                if let Some(receipt) = event.data.get("receipt").filter(|value| !value.is_null()) {
                    let _: TransportReceipt = decode(receipt, "receipt")?;
                }
                let key = event
                    .data
                    .get("notificationKey")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        format!(
                            "{}",
                            structured_key(&[
                                "appsdk/daemon",
                                &notification.recipient.key(),
                                notification
                                    .coalesce_key
                                    .as_deref()
                                    .unwrap_or("master-idle"),
                            ])
                        )
                    });
                let attempt_id = event
                    .data
                    .get("attemptId")
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        value.as_str().ok_or_else(|| {
                            CommError::new(
                                "event_data_invalid",
                                "wakeup reminder attempt id is not a string",
                            )
                        })
                    })
                    .transpose()?;
                if let Some(attempt_id) = attempt_id {
                    match self.projection.completed_attempts.get(&key) {
                        Some(completed_attempt_id) if completed_attempt_id == attempt_id => {}
                        Some(completed_attempt_id) => {
                            return Err(CommError::new(
                                "delivery_attempt_mismatch",
                                format!(
                                    "wakeup reminder attempt {attempt_id} does not match completed attempt {completed_attempt_id}"
                                ),
                            ));
                        }
                        None => {
                            return Err(CommError::new(
                                "delivery_attempt_mismatch",
                                format!(
                                    "wakeup reminder attempt {attempt_id} has no completed terminal event"
                                ),
                            ));
                        }
                    }
                }
                self.projection.wakeup.insert(wakeup.address.key(), wakeup);
                self.projection
                    .message_ordinals
                    .entry(message.message_id.clone())
                    .or_insert(event_ordinal);
                self.projection
                    .messages
                    .insert(message.message_id.clone(), message);
                self.projection.notifications.insert(key, notification);
            }
            "notification.delivery_failed" => {
                let keys = event
                    .data
                    .get("keys")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification failure keys missing")
                    })?;
                let attempt_id = event
                    .data
                    .get("attemptId")
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        value.as_str().ok_or_else(|| {
                            CommError::new(
                                "event_data_invalid",
                                "notification failure attempt id is not a string",
                            )
                        })
                    })
                    .transpose()?;
                let operation = event
                    .data
                    .get("operation")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            "notification failure operation missing or is not a string",
                        )
                    })?;
                if !matches!(
                    operation,
                    "notification.emitted" | "notification.batch_emitted"
                ) {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "notification failure operation is unsupported",
                    ));
                }
                let error: ErrorRecord = decode(
                    event.data.get("error").unwrap_or(&Value::Null),
                    "notification failure error",
                )?;
                let completed_attempt_id = attempt_id.map(str::to_owned);
                for value in keys {
                    let key = value.as_str().ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification key is not a string")
                    })?;
                    {
                        let notification = if let Some(notification) =
                            self.projection.notifications.get_mut(key)
                        {
                            notification
                        } else {
                            self.projection
                                .notifications
                                .values_mut()
                                .find(|notification| notification.notification_id == key)
                                .ok_or_else(|| {
                                    CommError::new(
                                        "event_data_invalid",
                                        format!("notification key not found: {key}"),
                                    )
                                })?
                        };
                        validate_terminal_attempt(notification, attempt_id, operation)?;
                        notification.last_error = Some(error.clone());
                        notification.status = "pending".into();
                        notification.delivery_attempt = None;
                    }
                    if let Some(attempt_id) = completed_attempt_id.as_ref() {
                        self.projection
                            .completed_attempts
                            .insert(key.into(), attempt_id.clone());
                    }
                }
            }
            "bug.reported" => {
                let bug: BugRecord = decode(&event.data, "bug")?;
                self.projection.bugs.insert(bug.bug_id.clone(), bug);
            }
            "bug.updated" => {
                let bug: BugRecord = decode(event.data.get("bug").unwrap_or(&event.data), "bug")?;
                self.projection.bugs.insert(bug.bug_id.clone(), bug);
                if let Some(loop_value) = event.data.get("loop").filter(|value| !value.is_null()) {
                    let loop_record: LoopRecord = decode(loop_value, "loop")?;
                    self.projection
                        .loops
                        .insert(loop_record.loop_id.clone(), loop_record);
                }
            }
            "loop.created" | "loop.updated" => {
                let loop_record: LoopRecord = decode(&event.data, "loop")?;
                self.projection
                    .loops
                    .insert(loop_record.loop_id.clone(), loop_record);
            }
            "error.recorded" => {
                let _: ErrorRecord =
                    decode(event.data.get("error").unwrap_or(&event.data), "error")?;
                if let Some(loop_value) = event.data.get("loop").filter(|value| !value.is_null()) {
                    let loop_record: LoopRecord = decode(loop_value, "loop")?;
                    self.projection
                        .loops
                        .insert(loop_record.loop_id.clone(), loop_record);
                }
            }
            other => {
                return Err(CommError::new(
                    "journal_unknown_event",
                    format!("unknown communication event: {other}"),
                ))
            }
        }
        Ok(())
    }

    fn dispatch(&mut self, request: &Value) -> CommResult<Value> {
        let op = request
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| CommError::new("operation_missing", "request op is required"))?;
        match op {
            "capabilities" => Ok(capabilities()),
            "status" => Ok(self.status()),
            "register_runtime" | "register-runtime" => self.register_runtime(decode(
                request.get("runtime").unwrap_or(request),
                "runtime",
            )?),
            "register_adapter" | "register-adapter" => self.register_adapter(decode(
                request.get("adapter").unwrap_or(request),
                "adapter",
            )?),
            "register_scope" | "register-scope" => {
                self.register_scope(decode(request.get("scope").unwrap_or(request), "scope")?)
            }
            "register_agent" | "register-agent" => {
                self.register_agent(decode(request.get("agent").unwrap_or(request), "agent")?)
            }
            "refresh_agent" | "refresh-agent" => {
                let address: Address =
                    decode(request.get("address").unwrap_or(&Value::Null), "address")?;
                self.refresh_agent(address, request.get("at").and_then(Value::as_str))
            }
            "rebind_agent" | "rebind-agent" => {
                self.rebind_agent(decode(request.get("rebind").unwrap_or(request), "rebind")?)
            }
            "send" => self.send(decode(
                request.get("message").unwrap_or(request),
                "message",
            )?),
            "record_delivery" | "record-delivery" => self.record_delivery(decode(
                request.get("delivery").unwrap_or(request),
                "delivery",
            )?),
            "set_agent_state" | "set-agent-state" => {
                let address: Address =
                    decode(request.get("address").unwrap_or(&Value::Null), "address")?;
                let state = request
                    .get("state")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("state_missing", "state is required"))?;
                self.set_agent_state(address, state, request.get("at").and_then(Value::as_str))
            }
            "tick" => self.tick(request.get("now").and_then(Value::as_str)),
            "accumulate_wake" | "accumulate-wake" | "record_wake" | "record-wake" => {
                let master: Address = decode(
                    request
                        .get("master")
                        .or_else(|| request.get("address"))
                        .unwrap_or(&Value::Null),
                    "master",
                )?;
                let signal: MasterWakeSignalRequest =
                    decode(request.get("signal").unwrap_or(request), "signal")?;
                self.record_master_wake(master, signal)
            }
            "master_wake_decide" | "master-wake-decide" => {
                let master: Address = decode(
                    request
                        .get("master")
                        .or_else(|| request.get("address"))
                        .unwrap_or(&Value::Null),
                    "master",
                )?;
                let generation = request
                    .get("generation")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| {
                        CommError::new("master_wake_generation_required", "generation is required")
                    })?;
                let action = request
                    .get("action")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new("master_wake_action_required", "action is required")
                    })?;
                self.decide_master_wake(
                    master,
                    generation,
                    action,
                    request.get("at").and_then(Value::as_str),
                )
            }
            "flush_notifications" | "flush-notifications" => {
                self.flush_notifications(request.get("now").and_then(Value::as_str))
            }
            "report_bug" | "report-bug" => {
                self.report_bug(decode(request.get("bug").unwrap_or(request), "bug")?)
            }
            "update_bug" | "update-bug" => {
                let bug_id = request
                    .get("bugId")
                    .or_else(|| request.get("bug_id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("bug_id_missing", "bugId is required"))?;
                let status = request
                    .get("status")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("bug_status_missing", "status is required"))?;
                let actor: Address = decode(request.get("actor").unwrap_or(&Value::Null), "actor")?;
                self.update_bug(bug_id, status, actor, request.get("evidence").cloned())
            }
            "create_loop" | "create-loop" => {
                self.create_loop(decode(request.get("loop").unwrap_or(request), "loop")?)
            }
            "advance_loop" | "advance-loop" => {
                let loop_id = request
                    .get("loopId")
                    .or_else(|| request.get("loop_id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("loop_id_missing", "loopId is required"))?;
                self.advance_loop(
                    loop_id,
                    request
                        .get("complete")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    request
                        .get("blocked")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    request
                        .get("actor")
                        .map(|value| decode(value, "actor"))
                        .transpose()?,
                    request.get("evidence").cloned(),
                    request.get("now").and_then(Value::as_str),
                )
            }
            "record_error" | "record-error" => self.record_error(
                request
                    .get("code")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("error_code_missing", "code is required"))?,
                request
                    .get("message")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new("error_message_missing", "message is required")
                    })?,
                request.get("context").cloned().unwrap_or(Value::Null),
                request
                    .get("loopId")
                    .or_else(|| request.get("loop_id"))
                    .and_then(Value::as_str),
            ),
            other => Err(CommError::new(
                "unknown_operation",
                format!("unknown communication operation: {other}"),
            )),
        }
    }
}

pub fn run_cli(args: Vec<String>) -> CommResult<()> {
    if args.len() == 1 && args[0] == "capabilities" {
        println!("{}", serde_json::to_string_pretty(&capabilities()).unwrap());
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
        "adapters": ["mailbox", "tmux", "appserver"],
        "adapterBinding": "recipient-address",
        "completionEvidence": {
            "bug": ["fix", "verification", "merge"],
            "loop": ["gate", "verification"]
        }
    })
}

fn validate_communication_root_input(root: &Path) -> CommResult<()> {
    if !is_lexically_canonical_absolute(root) {
        return Err(CommError::new(
            "communication_root_not_canonical",
            format!(
                "communication root must be an absolute canonical path: {}",
                root.display()
            ),
        ));
    }
    reject_symlink_components(root, "communication_root")?;
    if root.exists() && !root.is_dir() {
        return Err(CommError::new(
            "communication_root_not_directory",
            format!("communication root is not a directory: {}", root.display()),
        ));
    }
    Ok(())
}

fn validate_communication_root(root: &Path) -> CommResult<PathBuf> {
    validate_communication_root_input(root)?;
    let canonical = root.canonicalize().map_err(|error| {
        CommError::new(
            "communication_root_canonicalize_failed",
            format!("{}: {error}", root.display()),
        )
    })?;
    if canonical != root && !is_platform_root_alias(root, &canonical) {
        return Err(CommError::new(
            "communication_root_not_canonical",
            format!(
                "communication root is not canonical: expected {}, got {}",
                canonical.display(),
                root.display()
            ),
        ));
    }
    Ok(canonical)
}

fn infer_project_root(mailbox_path: &Path) -> CommResult<PathBuf> {
    if !is_lexically_canonical_absolute(mailbox_path) {
        return Err(CommError::new(
            "communication_mailbox_not_canonical",
            format!(
                "communication mailbox must be an absolute canonical path: {}",
                mailbox_path.display()
            ),
        ));
    }
    let communication = mailbox_path.parent().ok_or_else(|| {
        CommError::new(
            "communication_mailbox_layout_invalid",
            "communication mailbox has no parent directory",
        )
    })?;
    let control = communication.parent().ok_or_else(|| {
        CommError::new(
            "communication_mailbox_layout_invalid",
            "communication mailbox has no control directory",
        )
    })?;
    let root = control.parent().ok_or_else(|| {
        CommError::new(
            "communication_mailbox_layout_invalid",
            "communication mailbox has no project root",
        )
    })?;
    if mailbox_path.file_name().and_then(|name| name.to_str()) != Some("mailbox.jsonl")
        || communication.file_name().and_then(|name| name.to_str()) != Some("communication")
        || control.file_name().and_then(|name| name.to_str()) != Some(".appsdk-control")
    {
        return Err(CommError::new(
            "communication_mailbox_layout_invalid",
            format!(
                "communication mailbox must be <project>/.appsdk-control/communication/mailbox.jsonl: {}",
                mailbox_path.display()
            ),
        ));
    }
    validate_communication_root(root)
}

fn reject_symlink_components(path: &Path, label: &str) -> CommResult<()> {
    if !path.is_absolute() {
        return Err(CommError::new(
            "communication_path_not_absolute",
            format!("{label} path must be absolute: {}", path.display()),
        ));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let canonical = fs::canonicalize(&current).ok();
                if canonical
                    .as_deref()
                    .is_none_or(|canonical| !is_platform_root_alias(&current, canonical))
                {
                    return Err(CommError::new(
                        "communication_path_symlink",
                        format!("{label} path contains symlink: {}", current.display()),
                    ));
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(CommError::new(
                    "communication_path_stat_failed",
                    format!("{label} path stat failed at {}: {error}", current.display()),
                ));
            }
        }
    }
    Ok(())
}

fn is_platform_root_alias(path: &Path, canonical: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(relative) = path.strip_prefix("/").ok() else {
            return false;
        };
        return canonical == Path::new("/private").join(relative);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, canonical);
        false
    }
}

fn is_lexically_canonical_absolute(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let Some(raw) = path.to_str() else {
        return false;
    };
    let separator = std::path::MAIN_SEPARATOR_STR;
    if raw.len() > separator.len() && raw.ends_with(separator) {
        return false;
    }
    let mut raw_components = raw.split(separator);
    raw_components.next();
    if raw_components.any(|component| component.is_empty() || component == "." || component == "..")
    {
        return false;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => {
                normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR))
            }
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::CurDir | std::path::Component::ParentDir => return false,
        }
    }
    normalized == path
}

fn validate_event_envelope(event: &EventRecord) -> CommResult<()> {
    if event.event_id.trim().is_empty() {
        return Err(CommError::new(
            "event_envelope_invalid",
            "eventId must be non-empty",
        ));
    }
    validate_time(&event.at).map_err(|error| {
        CommError::new(
            "event_envelope_invalid",
            format!("at must be a valid RFC3339 timestamp: {error}"),
        )
    })?;
    Ok(())
}

fn validate_scope_request(request: &ScopeRequest) -> CommResult<()> {
    validate_non_empty(&request.scope_id, "scopeId")?;
    validate_non_empty(&request.appserver_id, "appserverId")?;
    validate_non_empty(&request.endpoint, "endpoint")?;
    validate_non_empty(&request.project_root, "projectRoot")?;
    if !matches!(request.namespace.as_str(), "codex_app" | "codex_tui") {
        return Err(CommError::new(
            "invalid_namespace",
            format!(
                "namespace must be codex_app or codex_tui: {}",
                request.namespace
            ),
        ));
    }
    for session in &request.session_ids {
        validate_non_empty(session, "sessionIds[]")?;
    }
    if request
        .runtime_id
        .as_deref()
        .is_none_or(|runtime_id| runtime_id.trim().is_empty())
    {
        return Err(CommError::new(
            "runtime_registration_required",
            "scope registration requires runtimeId",
        ));
    }
    Ok(())
}

fn validate_adapter_runtime_target(
    kind: &str,
    target: &str,
    runtime: &global_registry::RuntimeRecord,
    stale: bool,
) -> CommResult<()> {
    match kind {
        "tmux" => {
            let session = runtime
                .identity
                .tmux_session
                .as_deref()
                .filter(|value| !value.trim().is_empty());
            let pane = runtime
                .identity
                .tmux_pane
                .as_deref()
                .filter(|value| !value.trim().is_empty());
            let (Some(session), Some(pane)) = (session, pane) else {
                return Err(CommError::new(
                    "tmux_runtime_target_required",
                    format!(
                        "recipient runtime {} has no registered tmux session and pane",
                        runtime.identity.runtime_id
                    ),
                ));
            };
            let expected = format!("{session}:{pane}");
            if target != expected {
                return Err(CommError::new(
                    if stale {
                        "tmux_target_stale"
                    } else {
                        "tmux_target_mismatch"
                    },
                    format!(
                        "tmux adapter target {target} does not match recipient runtime target {expected}"
                    ),
                ));
            }
        }
        "appserver" => {
            if target != runtime.identity.endpoint {
                return Err(CommError::new(
                    if stale {
                        "appserver_target_stale"
                    } else {
                        "appserver_target_mismatch"
                    },
                    format!(
                        "appserver adapter target {target} does not match recipient runtime endpoint {}",
                        runtime.identity.endpoint
                    ),
                ));
            }
            if !runtime
                .identity
                .capabilities
                .iter()
                .any(|capability| capability == APPSERVER_SEND_CAPABILITY)
            {
                return Err(CommError::new(
                    "appserver_capability_missing",
                    format!(
                        "recipient runtime {} does not declare capability {}",
                        runtime.identity.runtime_id, APPSERVER_SEND_CAPABILITY
                    ),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_message_request(request: &MessageRequest) -> CommResult<()> {
    validate_address(&request.from)?;
    validate_address(&request.to)?;
    validate_non_empty(&request.title, "title")?;
    if request.title.chars().count() > 200 {
        return Err(CommError::new(
            "title_too_long",
            "message title must be at most 200 characters",
        ));
    }
    validate_non_empty(&request.body, "body")?;
    if let Some(key) = request.coalesce_key.as_deref() {
        validate_non_empty(key, "coalesceKey")?;
    }
    Ok(())
}

fn delivery_state_rank(state: &str) -> Option<u8> {
    match state {
        "created" => Some(0),
        "accepted" | "intent" => Some(1),
        "delivered" => Some(2),
        "executed" => Some(3),
        "replied" => Some(4),
        "read" => Some(5),
        "consumed" => Some(6),
        "unknown" => Some(0),
        _ => None,
    }
}

fn validate_delivery_state_transition(current: &str, next: &str) -> CommResult<()> {
    let current_rank = delivery_state_rank(current).ok_or_else(|| {
        CommError::new(
            "delivery_state_invalid",
            format!("message has invalid current state: {current}"),
        )
    })?;
    let next_rank = delivery_state_rank(next).ok_or_else(|| {
        CommError::new(
            "delivery_state_invalid",
            format!("message has invalid next state: {next}"),
        )
    })?;
    let delivered_rank = delivery_state_rank("delivered").expect("known delivery state");
    if (next == "unknown" && current_rank >= delivered_rank)
        || (next != "unknown" && next_rank < current_rank)
    {
        return Err(CommError::new(
            "delivery_state_regression",
            format!("cannot move message from {current} to {next}"),
        ));
    }
    Ok(())
}

fn validate_message_delivery_attempt(
    attempt: &MessageDeliveryAttempt,
    message: &MessageRecord,
    target: &AgentRecord,
    runtime: &global_registry::RuntimeRecord,
    adapter: &AdapterRecord,
    require_current_runtime: bool,
) -> CommResult<()> {
    validate_non_empty(&attempt.attempt_id, "attemptId")?;
    validate_non_empty(&attempt.message_id, "messageId")?;
    validate_non_empty(&attempt.operation, "operation")?;
    validate_non_empty(&attempt.adapter_id, "adapterId")?;
    validate_non_empty(&attempt.runtime_id, "runtimeId")?;
    validate_non_empty(&attempt.runtime_fingerprint, "runtimeFingerprint")?;
    validate_non_empty(&attempt.nonce, "nonce")?;
    validate_time(&attempt.started_at)?;
    if attempt.operation != "message.delivery" {
        return Err(CommError::new(
            "delivery_attempt_operation_invalid",
            format!(
                "unsupported message delivery attempt operation: {}",
                attempt.operation
            ),
        ));
    }
    if attempt.message_id != message.message_id {
        return Err(CommError::new(
            "delivery_attempt_message_mismatch",
            "delivery attempt does not match message",
        ));
    }
    if attempt.adapter_id != message.adapter_id || attempt.adapter_id != adapter.adapter_id {
        return Err(CommError::new(
            "delivery_attempt_adapter_mismatch",
            "delivery attempt does not match message adapter",
        ));
    }
    let target_runtime_id = target.runtime_id.as_deref().ok_or_else(|| {
        CommError::new(
            "runtime_registration_required",
            format!(
                "message target has no runtime identity: {}",
                message.to.key()
            ),
        )
    })?;
    if attempt.runtime_id != target_runtime_id || runtime.identity.runtime_id != attempt.runtime_id
    {
        return Err(CommError::new(
            "delivery_attempt_runtime_mismatch",
            "delivery attempt does not match message target runtime",
        ));
    }
    if require_current_runtime {
        if attempt.runtime_fingerprint != runtime.fingerprint {
            return Err(CommError::new(
                "delivery_attempt_runtime_stale",
                "delivery attempt runtime binding is stale",
            ));
        }
    } else if !global_registry::runtime_fingerprint_known(
        &attempt.runtime_id,
        &attempt.runtime_fingerprint,
    )
    .map_err(|error| CommError::new("runtime_registration_required", error))?
    {
        return Err(CommError::new(
            "delivery_attempt_runtime_unknown",
            "delivery attempt runtime fingerprint is not registered",
        ));
    }
    if attempt.target != adapter.target {
        return Err(CommError::new(
            "delivery_attempt_target_mismatch",
            "delivery attempt target does not match adapter target",
        ));
    }
    Ok(())
}

fn validate_adapter_delivery_receipt(
    kind: &str,
    receipt: &Value,
    runtime_id: &str,
    state: &str,
) -> CommResult<()> {
    let object = receipt.as_object().ok_or_else(|| {
        CommError::new(
            "delivery_evidence_invalid",
            "delivery evidence must match the registered adapter receipt contract",
        )
    })?;
    match kind {
        "mailbox" => {
            if object.get("durable").and_then(Value::as_bool) != Some(true) {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "mailbox delivery evidence must confirm a durable mailbox receipt",
                ));
            }
            if object
                .get("format")
                .and_then(Value::as_str)
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "mailbox delivery evidence must include a non-empty receipt format",
                ));
            }
        }
        "tmux" => {
            if object.get("executed").and_then(Value::as_bool) != Some(true) {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "tmux delivery evidence must confirm an executed tmux send",
                ));
            }
            if object
                .get("preview")
                .and_then(Value::as_str)
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "tmux delivery evidence must include the sent preview text",
                ));
            }
            if object.get("runtimeId").and_then(Value::as_str) != Some(runtime_id) {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "tmux delivery evidence runtimeId does not match delivery request",
                ));
            }
        }
        "appserver" => {
            if object.get("hostMustExecute").and_then(Value::as_bool) != Some(true) {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "appserver delivery evidence must confirm host must execute",
                ));
            }
            if object.get("capability").and_then(Value::as_str) != Some(APPSERVER_SEND_CAPABILITY) {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "appserver delivery evidence must include the send capability contract",
                ));
            }
            if object.get("runtimeId").and_then(Value::as_str) != Some(runtime_id) {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "appserver delivery evidence runtimeId does not match delivery request",
                ));
            }
            if matches!(
                state,
                "delivered" | "executed" | "replied" | "read" | "consumed"
            ) && object.get("hostExecuted").and_then(Value::as_bool) != Some(true)
            {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "appserver terminal delivery evidence must include independent host execution evidence",
                ));
            }
        }
        other => {
            return Err(CommError::new(
                "invalid_adapter_kind",
                format!("adapter kind is unsupported: {other}"),
            ));
        }
    }
    Ok(())
}

fn validate_replayed_delivery_evidence(
    state: &str,
    evidence: &DeliveryEvidence,
    target: &AgentRecord,
    message_id: &str,
    message: &MessageRecord,
    attempt: Option<&MessageDeliveryAttempt>,
    event_attempt_id: Option<&str>,
    event_nonce: Option<&str>,
    adapter: &AdapterRecord,
) -> CommResult<()> {
    if !matches!(
        state,
        "delivered" | "executed" | "replied" | "read" | "consumed" | "unknown"
    ) {
        return Ok(());
    }
    let details = evidence.details.as_object().ok_or_else(|| {
        CommError::new(
            "event_data_invalid",
            "external delivery evidence must be a non-empty object",
        )
    })?;
    if details.is_empty() {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence must be a non-empty object",
        ));
    }
    let runtime_id = details
        .get("runtimeId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CommError::new(
                "event_data_invalid",
                "external delivery evidence runtimeId is missing",
            )
        })?;
    let fingerprint = details
        .get("runtimeFingerprint")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CommError::new(
                "event_data_invalid",
                "external delivery evidence runtimeFingerprint is missing",
            )
        })?;
    if details
        .get("receipt")
        .and_then(Value::as_object)
        .is_none_or(|value| value.is_empty())
    {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence receipt must be a non-empty object",
        ));
    }
    let target_runtime_id = target.runtime_id.as_deref().ok_or_else(|| {
        CommError::new(
            "event_data_invalid",
            "external delivery evidence target has no runtime identity",
        )
    })?;
    if target_runtime_id != runtime_id {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence runtimeId does not match message target",
        ));
    }
    let runtime = global_registry::runtime(runtime_id)
        .map_err(|error| CommError::new("event_data_invalid", error))?;
    let known = global_registry::runtime_fingerprint_known(runtime_id, fingerprint)
        .map_err(|error| CommError::new("event_data_invalid", error))?;
    if !known {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence runtimeFingerprint is not registered",
        ));
    }

    // Messages written before the persisted-attempt contract are identified
    // by the absence of the explicit marker. Their historical receipts remain
    // replayable under the old runtime/fingerprint/receipt contract. Any
    // attempt metadata, or any persisted attempt, moves the event to the
    // strict contract instead of silently treating missing fields as legacy.
    let has_attempt_metadata = event_attempt_id.is_some()
        || event_nonce.is_some()
        || details.contains_key("attemptId")
        || details.contains_key("nonce")
        || details.contains_key("adapterId")
        || details.contains_key("target");
    let receipt = details.get("receipt").expect("receipt was checked above");
    if !message.delivery_attempt_required && attempt.is_none() && !has_attempt_metadata {
        if adapter.kind == "appserver"
            && matches!(
                state,
                "delivered" | "executed" | "replied" | "read" | "consumed"
            )
        {
            validate_adapter_delivery_receipt(&adapter.kind, receipt, runtime_id, state)?;
        }
        return Ok(());
    }
    validate_adapter_delivery_receipt(&adapter.kind, receipt, runtime_id, state)?;

    let attempt_id = event_attempt_id
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CommError::new(
                "event_data_invalid",
                "external delivery evidence attemptId is missing",
            )
        })?;
    let nonce = event_nonce
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CommError::new(
                "event_data_invalid",
                "external delivery evidence nonce is missing",
            )
        })?;
    if details.get("attemptId").and_then(Value::as_str) != Some(attempt_id)
        || details.get("nonce").and_then(Value::as_str) != Some(nonce)
    {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence attempt identity does not match message state",
        ));
    }
    let adapter_id = details
        .get("adapterId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CommError::new(
                "event_data_invalid",
                "external delivery evidence adapterId is missing",
            )
        })?;
    if adapter_id != message.adapter_id {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence adapterId does not match message",
        ));
    }
    let expected_target = serde_json::to_value(&adapter.target).unwrap();
    let observed_target = details.get("target").cloned().unwrap_or(Value::Null);
    if observed_target != expected_target {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence target does not match adapter",
        ));
    }
    let attempt = attempt.ok_or_else(|| {
        CommError::new(
            "event_data_invalid",
            format!("message {message_id} has no persisted delivery attempt"),
        )
    })?;
    if attempt.attempt_id != attempt_id || attempt.nonce != nonce {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence does not match persisted delivery attempt",
        ));
    }
    if attempt.runtime_fingerprint != fingerprint {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence runtimeFingerprint does not match attempt",
        ));
    }
    validate_message_delivery_attempt(attempt, message, target, &runtime, adapter, false)?;
    Ok(())
}

fn worker_idle_message(
    current: &AgentRecord,
    master_address: Address,
    transition_at: &str,
) -> MessageRequest {
    MessageRequest {
        from: current.address(),
        to: master_address,
        title: format!("worker idle: {}", current.agent_id),
        priority: "p2".into(),
        body: format!(
            "{} entered idle; inspect Appsdk mailbox facts for the latest result",
            current.agent_id
        ),
        delivery_mode: Some("idle".into()),
        coalesce_key: Some(format!("idle:{}", current.address().key())),
        issue_id: None,
        conversation_id: None,
        message_id: Some(worker_idle_message_id(current, transition_at)),
        created_at: Some(transition_at.into()),
        adapter_id: None,
    }
}

fn worker_idle_message_id(current: &AgentRecord, transition_at: &str) -> String {
    let address_key = current.address().key();
    format!(
        "worker-idle:{}",
        structured_key(&[&address_key, transition_at])
    )
}

fn worker_idle_signal_key(address: &Address) -> String {
    format!("worker-idle:{}", address.key())
}

fn worker_idle_coalesce_key(address: &Address) -> String {
    format!("idle:{}", address.key())
}

fn master_wake_covers_notification(
    signal: &MasterWakeSignal,
    notification: &NotificationRecord,
) -> bool {
    let worker_idle = signal.kind == "worker_idle"
        && signal.source.as_ref().is_some_and(|source| {
            notification.coalesce_key.as_deref() == Some(worker_idle_coalesce_key(source).as_str())
        });
    let bug = signal.issue_id.as_deref().is_some_and(|issue_id| {
        notification.issue_id.as_deref() == Some(issue_id)
            && notification.coalesce_key.as_deref() == Some("bug")
    });
    worker_idle || bug
}

fn bug_wake_signal_key(bug_id: &str) -> String {
    format!("bug:{}", bug_id)
}

fn bug_wake_signal(bug: &BugRecord, direct_dispatched: bool) -> MasterWakeSignal {
    MasterWakeSignal {
        signal_id: format!("bug:{}:{}:{}", bug.bug_id, bug.status, bug.updated_at),
        key: bug_wake_signal_key(&bug.bug_id),
        kind: "bug".into(),
        title: format!("bug {}: {}", bug.status, bug.title),
        priority: bug.priority.clone(),
        summary: bug.description.clone(),
        issue_id: Some(bug.bug_id.clone()),
        source: Some(bug.reporter.clone()),
        observed_at: bug.updated_at.clone(),
        direct_dispatched,
    }
}

fn loop_error_wake_signal(loop_record: &LoopRecord, error: &ErrorRecord) -> MasterWakeSignal {
    MasterWakeSignal {
        signal_id: format!("loop-error:{}:{}", loop_record.loop_id, error.at),
        key: format!("loop-error:{}", loop_record.loop_id),
        kind: "loop_error".into(),
        title: format!("loop blocked: {}", loop_record.loop_id),
        priority: Priority::P1,
        summary: format!("{}: {}", error.code, error.message),
        issue_id: None,
        source: None,
        observed_at: error.at.clone(),
        direct_dispatched: false,
    }
}

fn validate_master_wake_signal_request(request: &MasterWakeSignalRequest) -> CommResult<()> {
    validate_non_empty(&request.key, "key")?;
    validate_non_empty(&request.kind, "kind")?;
    validate_non_empty(&request.title, "title")?;
    validate_non_empty(&request.summary, "summary")?;
    if request.title.chars().count() > 200 {
        return Err(CommError::new(
            "master_wake_title_too_long",
            "master wake signal title must be at most 200 characters",
        ));
    }
    if let Some(signal_id) = request.signal_id.as_deref() {
        validate_non_empty(signal_id, "signalId")?;
    }
    if let Some(source) = request.source.as_ref() {
        validate_address(source)?;
    }
    if let Some(observed_at) = request.observed_at.as_deref() {
        validate_time(observed_at)?;
    }
    Ok(())
}

fn master_wake_signal_matches(left: &MasterWakeSignal, right: &MasterWakeSignal) -> bool {
    master_wake_signal_identity_matches(left, right)
        && left.direct_dispatched == right.direct_dispatched
}

fn master_wake_signal_identity_matches(left: &MasterWakeSignal, right: &MasterWakeSignal) -> bool {
    left.signal_id == right.signal_id
        && left.key == right.key
        && left.kind == right.kind
        && left.title == right.title
        && left.priority == right.priority
        && left.summary == right.summary
        && left.issue_id == right.issue_id
        && left.source == right.source
        && left.observed_at == right.observed_at
}

fn master_wake_accumulator_matches(
    left: &MasterWakeAccumulator,
    right: &MasterWakeAccumulator,
) -> bool {
    left.address == right.address
        && left.generation == right.generation
        && left.pending == right.pending
        && left.first_observed_at == right.first_observed_at
        && left.last_observed_at == right.last_observed_at
        && left.next_due_at == right.next_due_at
        && left.reminders_sent == right.reminders_sent
        && left.stopped == right.stopped
        && left.last_briefing_generation == right.last_briefing_generation
        && left.last_briefing_at == right.last_briefing_at
        && left.held == right.held
        && left.signals.len() == right.signals.len()
        && left.signals.iter().all(|(key, signal)| {
            right
                .signals
                .get(key)
                .is_some_and(|candidate| master_wake_signal_matches(signal, candidate))
        })
        && left.consumed_signals.len() == right.consumed_signals.len()
        && left.consumed_signals.iter().all(|(key, signal)| {
            right
                .consumed_signals
                .get(key)
                .is_some_and(|candidate| master_wake_signal_matches(signal, candidate))
        })
}

fn master_wake_message_identity(
    accumulator: &MasterWakeAccumulator,
    reminder: u8,
) -> (String, String) {
    let generation = accumulator.generation.to_string();
    let reminder = reminder.to_string();
    let cycle = structured_key(&[&accumulator.address.key(), &generation, &reminder]);
    let conversation = structured_key(&[&accumulator.address.key(), &generation]);
    (
        format!("master-wake-message-{cycle}"),
        format!("master-wake-conversation-{conversation}"),
    )
}

fn master_wake_direct_message_identity(
    master: &Address,
    signal: &MasterWakeSignal,
) -> (String, String) {
    let identity = structured_key(&[&master.key(), &signal.key, &signal.signal_id]);
    let conversation = structured_key(&[&master.key(), &signal.key]);
    (
        format!("master-wake-signal-message-{identity}"),
        format!("master-wake-signal-conversation-{conversation}"),
    )
}

fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn message_matches_request(
    existing: &MessageRecord,
    request: &MessageRequest,
    priority: &Priority,
    delivery_mode: &DeliveryMode,
    adapter_id: &str,
) -> bool {
    existing.from == request.from
        && existing.to == request.to
        && existing.title == request.title
        && existing.priority == *priority
        && existing.body == request.body
        && existing.delivery_mode == *delivery_mode
        && existing.coalesce_key == request.coalesce_key
        && existing.issue_id == request.issue_id
        && existing.adapter_id == adapter_id
        && request
            .conversation_id
            .as_ref()
            .map_or(true, |conversation_id| {
                existing.conversation_id == *conversation_id
            })
        && request
            .created_at
            .as_deref()
            .map(validate_time)
            .transpose()
            .ok()
            .flatten()
            .map_or(true, |created_at| existing.created_at == created_at)
}

fn validate_loop_request(request: &LoopRequest) -> CommResult<()> {
    validate_non_empty(&request.loop_id, "loopId")?;
    validate_non_empty(&request.kind, "kind")?;
    validate_address(&request.owner)?;
    for (value, name) in [
        (&request.trigger, "trigger"),
        (&request.work, "work"),
        (&request.gate, "gate"),
        (&request.state, "state"),
        (&request.stop, "stop"),
    ] {
        validate_non_empty(value, name)?;
    }
    if request.max_iterations == 0 {
        return Err(CommError::new(
            "loop_max_iterations_invalid",
            "maxIterations must be positive",
        ));
    }
    Ok(())
}

fn validate_resolution_evidence(evidence: Option<&Value>) -> CommResult<Value> {
    let evidence = evidence.ok_or_else(|| {
        CommError::new(
            "bug_resolution_evidence_required",
            "resolving or closing a bug requires fix, verification and merge evidence",
        )
    })?;
    let object = evidence.as_object().ok_or_else(|| {
        CommError::new(
            "bug_resolution_evidence_invalid",
            "bug resolution evidence must be a JSON object",
        )
    })?;
    if object.is_empty() {
        return Err(CommError::new(
            "bug_resolution_evidence_invalid",
            "bug resolution evidence must contain fix, verification and merge evidence",
        ));
    }
    for field in ["fix", "verification", "merge"] {
        let value = object.get(field).ok_or_else(|| {
            CommError::new(
                "bug_resolution_evidence_required",
                format!("bug resolution evidence is missing {field}"),
            )
        })?;
        if value.is_null() || value.as_str().is_some_and(|text| text.trim().is_empty()) {
            return Err(CommError::new(
                "bug_resolution_evidence_invalid",
                format!("bug resolution evidence {field} must be non-empty"),
            ));
        }
        if !is_valid_loop_evidence_value(value) {
            return Err(CommError::new(
                "bug_resolution_evidence_invalid",
                format!("bug resolution evidence {field} must identify a recognized result"),
            ));
        }
    }
    Ok(evidence.clone())
}

fn validate_bug_loop_binding(
    loop_record: &LoopRecord,
    bug_id: &str,
    scope_id: &str,
    owner: &Address,
) -> CommResult<()> {
    let expected_loop_id = format!("bug-loop-{bug_id}");
    let semantic_match = loop_record.kind == "bug"
        && loop_record.owner == *owner
        && loop_record.owner.scope_id == scope_id
        && loop_record.trigger == BUG_LOOP_TRIGGER
        && loop_record.work == BUG_LOOP_WORK
        && loop_record.gate == BUG_LOOP_GATE
        && loop_record.state == BUG_LOOP_STATE
        && loop_record.stop == BUG_LOOP_STOP;
    if loop_record.loop_id != expected_loop_id || !semantic_match {
        return Err(CommError::new(
            "bug_loop_conflict",
            format!(
                "loop is not the deterministic bug loop: {}",
                loop_record.loop_id
            ),
        ));
    }
    Ok(())
}

fn validate_loop_completion_evidence(evidence: Option<&Value>) -> CommResult<Value> {
    let evidence = evidence.ok_or_else(|| {
        CommError::new(
            "loop_gate_evidence_required",
            "completing a loop requires gate evidence",
        )
    })?;
    let object = evidence.as_object().ok_or_else(|| {
        CommError::new(
            "loop_gate_evidence_invalid",
            "loop gate evidence must be a JSON object",
        )
    })?;
    let mut found_result = false;
    for field in ["gate", "verification"] {
        let Some(value) = object.get(field) else {
            continue;
        };
        found_result = true;
        if !is_valid_loop_evidence_value(value) {
            return Err(CommError::new(
                "loop_gate_evidence_invalid",
                "loop gate evidence must identify a recognized passing gate and verification results",
            ));
        }
    }
    if !found_result {
        return Err(CommError::new(
            "loop_gate_evidence_required",
            "loop gate evidence must identify a non-empty gate or verification result",
        ));
    }
    Ok(evidence.clone())
}

fn is_valid_loop_evidence_value(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
        Value::String(text) => is_valid_descriptive_evidence_string(text),
        Value::Array(values) => {
            !values.is_empty() && values.iter().all(is_valid_loop_evidence_value)
        }
        Value::Object(values) => {
            if values.is_empty() {
                return false;
            }
            if !values.keys().any(|key| is_loop_evidence_result_field(key)) {
                return false;
            }
            values.iter().all(|(key, value)| {
                if is_loop_evidence_result_field(key) {
                    is_valid_loop_evidence_result_field(key, value)
                } else {
                    is_valid_loop_evidence_metadata(value)
                }
            })
        }
    }
}

fn is_loop_evidence_result_field(key: &str) -> bool {
    matches!(
        key,
        "status" | "result" | "outcome" | "state" | "passed" | "success" | "ok" | "verified"
    )
}

fn is_valid_loop_evidence_result_field(key: &str, value: &Value) -> bool {
    match key {
        "passed" | "success" | "ok" | "verified" => matches!(value, Value::Bool(true)),
        "status" | "result" | "outcome" | "state" => is_valid_loop_evidence_result_value(value),
        _ => false,
    }
}

fn is_valid_loop_evidence_result_value(value: &Value) -> bool {
    match value {
        Value::String(text) => {
            let normalized = text.trim().to_ascii_lowercase();
            matches!(
                normalized.as_str(),
                "passed" | "pass" | "success" | "ok" | "verified" | "true"
            )
        }
        Value::Array(values) => {
            !values.is_empty() && values.iter().all(is_valid_loop_evidence_result_value)
        }
        Value::Object(values) => {
            !values.is_empty()
                && values.keys().any(|key| is_loop_evidence_result_field(key))
                && values.iter().all(|(key, value)| {
                    if is_loop_evidence_result_field(key) {
                        is_valid_loop_evidence_result_field(key, value)
                    } else {
                        is_valid_loop_evidence_metadata(value)
                    }
                })
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn is_valid_loop_evidence_metadata(value: &Value) -> bool {
    match value {
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(values) => {
            !values.is_empty() && values.iter().all(is_valid_loop_evidence_metadata)
        }
        Value::Object(values) => {
            !values.is_empty()
                && values.iter().all(|(key, value)| {
                    if is_loop_evidence_result_field(key) {
                        is_valid_loop_evidence_result_field(key, value)
                    } else {
                        is_valid_loop_evidence_metadata(value)
                    }
                })
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
    }
}

fn is_valid_descriptive_evidence_string(text: &str) -> bool {
    let normalized = text.trim().to_ascii_lowercase();
    !normalized.is_empty()
        && !matches!(
            normalized.as_str(),
            "unknown"
                | "pending"
                | "failed"
                | "failure"
                | "fail"
                | "error"
                | "invalid"
                | "false"
                | "null"
                | "unverified"
                | "not_run"
                | "not run"
                | "timeout"
                | "blocked"
        )
}

fn validate_address(address: &Address) -> CommResult<()> {
    validate_non_empty(&address.scope_id, "address.scopeId")?;
    validate_non_empty(&address.session_id, "address.sessionId")
}

fn validate_non_empty(value: &str, name: &str) -> CommResult<()> {
    if value.trim().is_empty() {
        return Err(CommError::new(
            "invalid_request",
            format!("{name} must be non-empty"),
        ));
    }
    Ok(())
}

fn decode<T: DeserializeOwned>(value: &Value, name: &str) -> CommResult<T> {
    serde_json::from_value(value.clone())
        .map_err(|error| CommError::new("invalid_request", format!("{name}: {error}")))
}

fn require_event_field<'a>(data: &'a Value, field: &str) -> CommResult<&'a Value> {
    data.get(field).ok_or_else(|| {
        CommError::new(
            "event_data_invalid",
            format!("master wake event field is missing: {field}"),
        )
    })
}

fn decode_master_wake_accumulator_event(data: &Value) -> CommResult<MasterWakeAccumulator> {
    for field in [
        "address",
        "generation",
        "pending",
        "firstObservedAt",
        "lastObservedAt",
        "nextDueAt",
        "remindersSent",
        "stopped",
        "lastBriefingGeneration",
        "lastBriefingAt",
        "held",
        "signals",
        "consumedSignals",
    ] {
        require_event_field(data, field)?;
    }
    decode(data, "master wake")
}

fn validate_time(value: &str) -> CommResult<String> {
    let parsed = parse_time(value)?;
    Ok(parsed.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn parse_time(value: &str) -> CommResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|error| CommError::new("invalid_timestamp", error.to_string()))
}

fn add_seconds(value: &str, seconds: i64) -> CommResult<String> {
    Ok((parse_time(value)? + Duration::seconds(seconds))
        .to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn add_millis(value: &str, millis: i64) -> CommResult<String> {
    Ok((parse_time(value)? + Duration::milliseconds(millis))
        .to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn priority_then_time_bug(left: &BugRecord, right: &BugRecord) -> Ordering {
    left.priority
        .cmp(&right.priority)
        .then_with(|| left.created_at.cmp(&right.created_at))
        .then_with(|| left.bug_id.cmp(&right.bug_id))
}

fn bug_loop_matches(loop_record: &LoopRecord, owner: &Address) -> bool {
    loop_record.kind == "bug"
        && loop_record.owner == *owner
        && loop_record.trigger == "event:bug.reported"
        && loop_record.work == "triage -> fix in an independent worktree"
        && loop_record.gate == "project verification and review"
        && loop_record.state == "persist bug evidence and next action"
        && loop_record.stop == "resolved, merged, and reporter notified"
}

fn wakeup_message_identity(
    wakeup: &WakeupRecord,
    reminder_number: u8,
) -> CommResult<(String, String)> {
    let idle_since = wakeup.idle_since.as_deref().ok_or_else(|| {
        CommError::new(
            "wakeup_cycle_missing",
            format!("master wakeup has no idle cycle: {}", wakeup.address.key()),
        )
    })?;
    let reminder_number = reminder_number.to_string();
    let cycle = structured_key(&[&wakeup.address.key(), idle_since, &reminder_number]);
    let conversation = structured_key(&[&wakeup.address.key(), idle_since]);
    Ok((
        format!("wakeup-message-{cycle}"),
        format!("wakeup-conversation-{conversation}"),
    ))
}

fn synchronize_master_wakeup(
    wakeup: WakeupRecord,
    action: &str,
    accumulator: &MasterWakeAccumulator,
) -> WakeupRecord {
    let mut synchronized = wakeup;
    if action == "schedule" {
        synchronized.next_due_at = accumulator.next_due_at.clone();
        synchronized.stopped = false;
        synchronized.reminders_sent = 0;
        synchronized.last_reminder_at = None;
    } else {
        synchronized.next_due_at = None;
        synchronized.stopped = true;
    }
    synchronized
}

fn wakeup_message_matches(existing: &MessageRecord, expected: &MessageRecord) -> bool {
    existing.protocol == expected.protocol
        && existing.message_id == expected.message_id
        && existing.conversation_id == expected.conversation_id
        && existing.from == expected.from
        && existing.to == expected.to
        && existing.title == expected.title
        && existing.priority == expected.priority
        && existing.body == expected.body
        && existing.delivery_mode == expected.delivery_mode
        && existing.coalesce_key == expected.coalesce_key
        && existing.issue_id == expected.issue_id
        && existing.adapter_id == expected.adapter_id
        && existing.route.mode == expected.route.mode
        && existing.route.same_appserver == expected.route.same_appserver
        && existing.route.same_project == expected.route.same_project
        && existing.route.source_role == expected.route.source_role
        && existing.route.target_role == expected.route.target_role
}

fn next_loop_phase(loop_record: &mut LoopRecord) -> CommResult<String> {
    let phase = match loop_record.phase.as_str() {
        "discover" => "hand_off",
        "hand_off" => "verify",
        "verify" => "persist",
        "persist" => "schedule",
        "schedule" => {
            loop_record.iteration += 1;
            if loop_record.iteration >= loop_record.max_iterations {
                loop_record.status = "stopped".into();
                "max_iterations"
            } else {
                "discover"
            }
        }
        other => {
            return Err(CommError::new(
                "loop_phase_invalid",
                format!("invalid loop phase: {other}"),
            ))
        }
    };
    Ok(phase.into())
}

fn default_mailbox_adapter() -> AdapterRecord {
    AdapterRecord {
        adapter_id: "mailbox".into(),
        kind: "mailbox".into(),
        target: None,
        enabled: true,
        execute: false,
        recipient: None,
        registered_at: "built-in".into(),
    }
}

fn adapter_error(error: &CommError, adapter_id: &str, operation: &str) -> CommError {
    let mut enriched = error.clone();
    enriched.context = json!({
        "adapterId": adapter_id,
        "operation": operation,
        "cause": error.context
    });
    enriched
}

fn with_secondary_error(mut primary: CommError, secondary: CommError, stage: &str) -> CommError {
    primary.context = json!({
        "cause": primary.context,
        "stage": stage,
        "secondaryError": {
            "code": secondary.code,
            "message": secondary.message,
            "context": secondary.context
        }
    });
    primary
}

fn adapter_error_record(error: &CommError, adapter_id: &str, operation: &str) -> ErrorRecord {
    ErrorRecord {
        code: error.code.clone(),
        message: error.message.clone(),
        context: json!({
            "adapterId": adapter_id,
            "operation": operation,
            "cause": error.context
        }),
        at: now(),
    }
}

fn new_delivery_attempt(
    adapter_id: &str,
    operation: &str,
    batch_id: Option<&str>,
) -> DeliveryAttempt {
    new_delivery_attempt_at(adapter_id, operation, batch_id, &now())
}

fn new_delivery_attempt_at(
    adapter_id: &str,
    operation: &str,
    batch_id: Option<&str>,
    started_at: &str,
) -> DeliveryAttempt {
    DeliveryAttempt {
        attempt_id: new_id("attempt"),
        operation: operation.into(),
        adapter_id: adapter_id.into(),
        started_at: started_at.into(),
        batch_id: batch_id.map(str::to_owned),
    }
}

fn validate_terminal_attempt(
    notification: &NotificationRecord,
    event_attempt_id: Option<&str>,
    operation: &str,
) -> CommResult<()> {
    match (notification.delivery_attempt.as_ref(), event_attempt_id) {
        (None, None) => Ok(()),
        (Some(attempt), Some(attempt_id))
            if attempt.attempt_id == attempt_id && attempt.operation == operation =>
        {
            Ok(())
        }
        (Some(attempt), Some(attempt_id)) => Err(CommError::new(
            "delivery_attempt_mismatch",
            format!(
                "{operation} attempt {attempt_id} does not match pending attempt {}",
                attempt.attempt_id
            ),
        )),
        (Some(attempt), None) => Err(CommError::new(
            "delivery_attempt_mismatch",
            format!(
                "{operation} is missing pending attempt {}",
                attempt.attempt_id
            ),
        )),
        (None, Some(attempt_id)) => Err(CommError::new(
            "delivery_attempt_mismatch",
            format!("{operation} references unknown attempt {attempt_id}"),
        )),
    }
}

fn bounded_preview(message: &MessageRecord) -> String {
    let mut preview = format!(
        "[appsdk][{}] {}",
        format_priority(&message.priority),
        message.title
    );
    if !message.body.trim().is_empty() {
        preview.push_str(": ");
        preview.push_str(message.body.trim());
    }
    preview
        .chars()
        .map(|character| match character {
            '\n' | '\r' | '\t' => ' ',
            other => other,
        })
        .take(240)
        .collect()
}

fn format_priority(priority: &Priority) -> String {
    match priority {
        Priority::P0 => "p0",
        Priority::P1 => "p1",
        Priority::P2 => "p2",
        Priority::P3 => "p3",
    }
    .into()
}

fn structured_key(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| format!("{}#{}", part.len(), part))
        .collect::<Vec<_>>()
        .join("|")
}

fn default_max_iterations() -> u32 {
    100
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn new_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = ID_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    format!("{prefix}-{nanos:x}-{counter:x}")
}
