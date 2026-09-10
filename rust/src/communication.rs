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

pub const PROTOCOL: &str = "appsdk-comm/v1";
pub const DEFAULT_BATCH_WINDOW_SECONDS: i64 = 120;
pub const DEFAULT_MASTER_REMINDER_LIMIT: u8 = 3;
pub const DEFAULT_AGENT_LEASE_MS: u64 = 7 * 24 * 60 * 60 * 1000;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
            evidence: json!({ "preview": preview, "executed": self.execute }),
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
            evidence: json!({ "preview": preview, "executed": self.execute }),
        })
    }
}

struct AppserverAdapter {
    adapter_id: String,
    endpoint: String,
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
                "hostMustExecute": true
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
                "hostMustExecute": true
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NotificationSummary {
    #[serde(rename = "notificationId")]
    notification_id: String,
    #[serde(rename = "messageId")]
    message_id: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Projection {
    scopes: BTreeMap<String, ScopeRecord>,
    agents: BTreeMap<String, AgentRecord>,
    messages: BTreeMap<String, MessageRecord>,
    notifications: BTreeMap<String, NotificationRecord>,
    adapters: BTreeMap<String, AdapterRecord>,
    wakeup: BTreeMap<String, WakeupRecord>,
    bugs: BTreeMap<String, BugRecord>,
    loops: BTreeMap<String, LoopRecord>,
    #[serde(default)]
    batches: Vec<NotificationBatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    projection: Projection,
    _lock: CommunicationLock,
}

struct CommunicationLock {
    _file: File,
}

impl CommunicationLock {
    fn acquire(mailbox_path: &Path) -> CommResult<Self> {
        let lock_path = mailbox_path.with_extension("jsonl.lock");
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

        Ok(Self { _file: file })
    }
}

impl CommunicationStore {
    pub fn open(root: &Path) -> CommResult<Self> {
        if root.exists() && !root.is_dir() {
            return Err(CommError::new(
                "communication_root_not_directory",
                format!("communication root is not a directory: {}", root.display()),
            ));
        }
        if !root.exists() {
            fs::create_dir_all(root).map_err(|error| {
                CommError::new(
                    "communication_root_create_failed",
                    format!("{}: {error}", root.display()),
                )
            })?;
        }
        Self::open_mailbox(root.join(".appsdk-control/communication/mailbox.jsonl"))
    }

    pub fn open_mailbox(mailbox_path: PathBuf) -> CommResult<Self> {
        if let Some(parent) = mailbox_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                CommError::new(
                    "mailbox_directory_create_failed",
                    format!("{}: {error}", parent.display()),
                )
            })?;
        }
        let lock = CommunicationLock::acquire(&mailbox_path)?;
        let mut store = Self {
            mailbox_path,
            projection: Projection::default(),
            _lock: lock,
        };
        store.replay()?;
        store
            .projection
            .adapters
            .entry("mailbox".into())
            .or_insert_with(default_mailbox_adapter);
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
        json!({
            "protocol": PROTOCOL,
            "mailboxPath": self.mailbox_path,
            "scopes": self.projection.scopes.values().collect::<Vec<_>>(),
            "agents": self.projection.agents.values().collect::<Vec<_>>(),
            "messages": self.projection.messages.values().collect::<Vec<_>>(),
            "adapters": self.projection.adapters.values().collect::<Vec<_>>(),
            "activeBugs": active_bugs,
            "bugs": self.projection.bugs.values().collect::<Vec<_>>(),
            "loops": loops,
            "wakeup": self.projection.wakeup.values().collect::<Vec<_>>(),
            "notificationProjection": {
                "pending": pending,
                "emitted": emitted,
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
            self.require_live_agent(recipient)?;
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
        let at = now();
        if let Some(existing) = self.projection.scopes.get(&request.scope_id) {
            if existing.appserver_id == request.appserver_id
                && existing.namespace == request.namespace
                && existing.endpoint == request.endpoint
                && existing.project_root == request.project_root
                && existing.session_ids == request.session_ids
            {
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
        };
        self.commit("scope.registered", serde_json::to_value(&record).unwrap())?;
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
        if let Some(existing) = self.projection.agents.get(&key) {
            if existing.role == role
                && existing.agent_id == request.agent_id
                && existing.parent == request.parent
            {
                return Ok(json!({ "agent": existing, "idempotent": true }));
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
        };
        self.commit("agent.registered", serde_json::to_value(&record).unwrap())?;
        Ok(json!({ "agent": record, "idempotent": false }))
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

    fn send(&mut self, request: MessageRequest) -> CommResult<Value> {
        validate_message_request(&request)?;
        let source = self.require_live_agent(&request.from)?.clone();
        let target = self.require_live_agent(&request.to)?.clone();
        let route = self.resolve_route(&source, &target)?;
        self.enqueue_message(request, route, None)
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
            if current.role == "master" && next == AgentState::Idle {
                if !self.projection.wakeup.contains_key(&address.key()) {
                    let wakeup = WakeupRecord {
                        address: current.address(),
                        idle_since: Some(at.clone()),
                        reminders_sent: 0,
                        next_due_at: Some(add_seconds(&at, DEFAULT_BATCH_WINDOW_SECONDS)?),
                        stopped: false,
                        last_reminder_at: None,
                    };
                    self.commit("wakeup.updated", serde_json::to_value(&wakeup).unwrap())?;
                }
            } else if current.role != "master" && next == AgentState::Idle {
                let master_address = self.scope_master_address(&current.scope_id)?;
                let notification_key = structured_key(&[
                    &current.address().key(),
                    &master_address.key(),
                    "mailbox",
                    &format!("idle:{}", current.address().key()),
                ]);
                if !self
                    .projection
                    .notifications
                    .contains_key(&notification_key)
                {
                    let message = worker_idle_message(&current, master_address, &at);
                    let notification = self.send(message)?;
                    return Ok(json!({
                        "agent": current,
                        "idempotent": true,
                        "notification": notification
                    }));
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
            return Ok(json!({
                "agent": self.require_agent(&address)?,
                "idempotent": false,
                "notification": Value::Null
            }));
        }

        if next == AgentState::Idle {
            let master_address = self.scope_master_address(&current.scope_id)?;
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

        Ok(json!({
            "agent": self.require_agent(&address)?,
            "idempotent": false,
            "notification": Value::Null
        }))
    }

    pub fn tick(&mut self, at: Option<&str>) -> CommResult<Value> {
        let at = at.map(validate_time).transpose()?.unwrap_or_else(now);
        let wakeups: Vec<WakeupRecord> = self.projection.wakeup.values().cloned().collect();
        let mut changed = Vec::new();
        for wakeup in wakeups {
            if wakeup.stopped || wakeup.reminders_sent >= DEFAULT_MASTER_REMINDER_LIMIT {
                continue;
            }
            let agent = match self.projection.agents.get(&wakeup.address.key()) {
                Some(agent) if agent.role == "master" && agent.state == AgentState::Idle => {
                    agent.clone()
                }
                _ => continue,
            };
            let due = match wakeup.next_due_at.as_deref() {
                Some(next_due) => parse_time(&at)? >= parse_time(next_due)?,
                None => false,
            };
            if !due {
                continue;
            }
            let reminders_sent = wakeup.reminders_sent + 1;
            let next_wakeup = WakeupRecord {
                address: wakeup.address.clone(),
                idle_since: wakeup.idle_since.clone(),
                reminders_sent,
                next_due_at: Some(add_seconds(&at, DEFAULT_BATCH_WINDOW_SECONDS)?),
                stopped: reminders_sent >= DEFAULT_MASTER_REMINDER_LIMIT,
                last_reminder_at: Some(at.clone()),
            };
            let message = self.system_message(
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
            let mut notification = self.build_notification(&message, &at, Some(&at))?;
            let notification_key = self.notification_key(&message, &notification, true);
            let adapter = match self.adapter_for(&message.adapter_id, Some(&agent.address())) {
                Ok(adapter) => adapter,
                Err(error) => {
                    let error = adapter_error(&error, &message.adapter_id, "wakeup.reminder");
                    notification.last_error = Some(adapter_error_record(
                        &error,
                        &message.adapter_id,
                        "wakeup.reminder",
                    ));
                    self.commit(
                        "wakeup.reminder",
                        json!({
                            "wakeup": next_wakeup,
                            "message": message,
                            "notificationKey": notification_key,
                            "notification": notification,
                            "receipt": Value::Null
                        }),
                    )?;
                    return Err(error);
                }
            };
            let receipt = match adapter.deliver(&message) {
                Ok(receipt) => receipt,
                Err(error) => {
                    let error = adapter_error(&error, &message.adapter_id, "wakeup.reminder");
                    notification.last_error = Some(adapter_error_record(
                        &error,
                        &message.adapter_id,
                        "wakeup.reminder",
                    ));
                    self.commit(
                        "wakeup.reminder",
                        json!({
                            "wakeup": next_wakeup,
                            "message": message,
                            "notificationKey": notification_key,
                            "notification": notification,
                            "receipt": Value::Null
                        }),
                    )?;
                    return Err(error);
                }
            };
            notification.status = "emitted".into();
            notification.emitted_at = Some(at.clone());
            notification.transport_receipt = Some(receipt.clone());
            self.commit(
                "wakeup.reminder",
                json!({
                    "wakeup": next_wakeup,
                    "message": message,
                    "notificationKey": notification_key,
                    "notification": notification,
                    "receipt": receipt
                }),
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
        Ok(json!({ "at": at, "wakeup": wakeup, "changed": changed }))
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
                    if let Err(record_error) =
                        self.record_notification_failure(&keys, &adapter_id, &error)
                    {
                        return Err(with_secondary_error(
                            error,
                            record_error,
                            "notification.delivery_failed",
                        ));
                    }
                    return Err(error);
                }
            };
            let receipt = match adapter.emit_batch(&batch) {
                Ok(receipt) => receipt,
                Err(error) => {
                    let error = adapter_error(&error, &adapter_id, "notification.batch_emitted");
                    if let Err(record_error) =
                        self.record_notification_failure(&keys, &adapter_id, &error)
                    {
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
        if let Some(existing) = self.projection.bugs.get(&request.bug_id) {
            if existing.title == request.title
                && existing.description == request.description
                && existing.priority == priority
                && existing.reporter == request.reporter
                && existing.worktree_id == request.worktree_id
            {
                return Ok(json!({ "bug": existing, "idempotent": true }));
            }
            return Err(CommError::new(
                "bug_conflict",
                format!("bug already exists: {}", request.bug_id),
            ));
        }
        let at = now();
        let loop_id = format!("bug-loop-{}", request.bug_id);
        let owner = self.scope_master_address(&request.scope_id)?;
        if !self.projection.loops.contains_key(&loop_id) {
            let loop_record = LoopRecord {
                loop_id: loop_id.clone(),
                kind: "bug".into(),
                owner: owner.clone(),
                trigger: "event:bug.reported".into(),
                work: "triage -> fix in an independent worktree".into(),
                gate: "project verification and review".into(),
                state: "persist bug evidence and next action".into(),
                stop: "resolved, merged, and reporter notified".into(),
                max_iterations: 100,
                deadline_at: None,
                phase: "discover".into(),
                status: "active".into(),
                iteration: 0,
                created_at: at.clone(),
                updated_at: at.clone(),
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
        Ok(json!({ "bug": bug, "notification": notification, "idempotent": false }))
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
        let resolution_evidence = if matches!(status, "resolved" | "closed") {
            Some(validate_resolution_evidence(evidence.as_ref())?)
        } else {
            None
        };
        let mut updated = current.clone();
        updated.status = status.into();
        updated.updated_at = now();
        updated.resolution_evidence = resolution_evidence.clone();
        let mut updated_loop = self.projection.loops.get(&current.loop_id).cloned();
        if matches!(status, "resolved" | "closed") {
            if let Some(loop_record) = updated_loop.as_mut() {
                loop_record.status = "completed".into();
                loop_record.phase = "completed".into();
                loop_record.updated_at = updated.updated_at.clone();
            }
        } else if status == "active" {
            if let Some(loop_record) = updated_loop.as_mut() {
                loop_record.status = "active".into();
                loop_record.phase = "discover".into();
                loop_record.updated_at = updated.updated_at.clone();
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
        } else {
            None
        };
        Ok(json!({ "bug": updated, "loop": updated_loop, "notification": notification }))
    }

    fn create_loop(&mut self, request: LoopRequest) -> CommResult<Value> {
        validate_loop_request(&request)?;
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
            validate_loop_completion_evidence(evidence.as_ref())?;
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
        Ok(json!({ "error": error, "loop": updated_loop, "eventId": event_id }))
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
        if self.projection.messages.contains_key(&message_id) {
            let existing = self.projection.messages.get(&message_id).unwrap();
            if message_matches_request(existing, &request, &priority, &delivery_mode, &adapter_id) {
                return Ok(json!({
                    "message": existing,
                    "route": existing.route,
                    "idempotent": true
                }));
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
        let notification = self.notification_for(&current, &created_at, available_at_override)?;
        let direct = notification
            .as_ref()
            .filter(|notification| notification.status == "emitted")
            .map(NotificationRecord::summary);
        Ok(json!({
            "message": current,
            "route": current.route,
            "notification": direct,
            "idempotent": false
        }))
    }

    fn notification_for(
        &mut self,
        message: &MessageRecord,
        created_at: &str,
        available_at_override: Option<&str>,
    ) -> CommResult<Option<NotificationRecord>> {
        let immediate = matches!(message.delivery_mode, DeliveryMode::Direct)
            || message.priority.is_breakthrough();
        let mut notification =
            self.build_notification(message, created_at, available_at_override)?;
        let key = self.notification_key(message, &notification, !immediate);
        if let Some(existing) = self.projection.notifications.get(&key) {
            if existing.status == "pending" {
                notification.available_at = existing.available_at.clone();
            }
        }
        self.commit(
            "notification.queued",
            json!({ "key": key, "notification": notification }),
        )?;
        if immediate {
            let adapter = match self.adapter_for(&message.adapter_id, Some(&message.to)) {
                Ok(adapter) => adapter,
                Err(error) => {
                    let error = adapter_error(&error, &message.adapter_id, "notification.emitted");
                    if let Err(record_error) = self.record_notification_failure(
                        std::slice::from_ref(&key),
                        &message.adapter_id,
                        &error,
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
            let receipt = match adapter.deliver(message) {
                Ok(receipt) => receipt,
                Err(error) => {
                    let error = adapter_error(&error, &message.adapter_id, "notification.emitted");
                    if let Err(record_error) = self.record_notification_failure(
                        std::slice::from_ref(&key),
                        &message.adapter_id,
                        &error,
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
                json!({ "keys": [key], "at": created_at, "receipt": receipt }),
            )?;
        }
        Ok(self.projection.notifications.get(&key).cloned())
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
        structured_key(&[
            &message.from.key(),
            &message.to.key(),
            &message.adapter_id,
            notification
                .coalesce_key
                .as_deref()
                .unwrap_or("notification"),
        ])
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
                    endpoint,
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
    ) -> CommResult<()> {
        let record = adapter_error_record(error, adapter_id, "notification.delivery");
        self.commit(
            "notification.delivery_failed",
            json!({ "keys": keys, "adapterId": adapter_id, "error": record }),
        )
        .map(|_| ())
    }

    fn require_scope(&self, scope_id: &str) -> CommResult<&ScopeRecord> {
        self.projection.scopes.get(scope_id).ok_or_else(|| {
            CommError::new("scope_not_found", format!("scope not found: {scope_id}"))
        })
    }

    fn require_agent(&self, address: &Address) -> CommResult<&AgentRecord> {
        validate_address(address)?;
        self.projection.agents.get(&address.key()).ok_or_else(|| {
            CommError::new(
                "agent_not_registered",
                format!("agent not registered: {}", address.key()),
            )
        })
    }

    fn require_live_agent(&self, address: &Address) -> CommResult<&AgentRecord> {
        let agent = self.require_agent(address)?;
        if !agent.live_at(&now()) {
            return Err(CommError::new(
                "agent_lease_expired",
                format!("agent lease expired: {}", address.key()),
            ));
        }
        Ok(agent)
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
        let target_scope = self.require_scope(&target.scope_id)?;
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
            .is_some_and(|parent| parent.key() == other.address().key())
        {
            return true;
        }
        if other.role != "master" {
            return false;
        }
        let mut current = child.parent.clone();
        while let Some(parent) = current {
            if parent.key() == other.address().key() {
                return true;
            }
            current = self
                .projection
                .agents
                .get(&parent.key())
                .and_then(|agent| agent.parent.clone());
        }
        self.projection
            .scopes
            .get(&child.scope_id)
            .and_then(|scope| scope.master_session_id.as_ref())
            .is_some_and(|master| master == &other.session_id)
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
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let event: EventRecord = serde_json::from_str(line).map_err(|error| {
                CommError::new(
                    "journal_corrupt",
                    format!("invalid JSONL at line {}: {error}", index + 1),
                )
            })?;
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
        Ok(())
    }

    fn apply_event(&mut self, event: &EventRecord) -> CommResult<()> {
        match event.kind.as_str() {
            "scope.registered" => {
                let record: ScopeRecord = decode(&event.data, "scope")?;
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
                self.projection
                    .agents
                    .insert(record.address().key(), record);
            }
            "message.created" => {
                let record: MessageRecord = decode(&event.data, "message")?;
                self.projection
                    .messages
                    .insert(record.message_id.clone(), record);
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
                let message = self
                    .projection
                    .messages
                    .get_mut(message_id)
                    .ok_or_else(|| CommError::new("event_data_invalid", "message not found"))?;
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
                for value in keys {
                    let key = value.as_str().ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification key is not a string")
                    })?;
                    let notification =
                        self.projection.notifications.get_mut(key).ok_or_else(|| {
                            CommError::new(
                                "event_data_invalid",
                                format!("notification key not found: {key}"),
                            )
                        })?;
                    notification.status = "emitted".into();
                    notification.emitted_at = Some(at.into());
                    if let Some(receipt) = receipt.clone() {
                        notification.transport_receipt = Some(receipt);
                    }
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
                for value in keys {
                    let key = value.as_str().ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification key is not a string")
                    })?;
                    let notification =
                        if let Some(notification) = self.projection.notifications.get_mut(key) {
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
                    notification.status = "emitted".into();
                    notification.emitted_at = Some(at.into());
                    if let Some(receipt) = receipt.clone() {
                        notification.transport_receipt = Some(receipt);
                    }
                }
                self.projection.batches.push(batch);
            }
            "wakeup.updated" => {
                let wakeup: WakeupRecord = decode(&event.data, "wakeup")?;
                self.projection.wakeup.insert(wakeup.address.key(), wakeup);
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
                self.projection.wakeup.insert(wakeup.address.key(), wakeup);
                self.projection
                    .messages
                    .insert(message.message_id.clone(), message);
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
                let error: ErrorRecord = decode(
                    event.data.get("error").unwrap_or(&Value::Null),
                    "notification failure error",
                )?;
                for value in keys {
                    let key = value.as_str().ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification key is not a string")
                    })?;
                    let notification =
                        if let Some(notification) = self.projection.notifications.get_mut(key) {
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
                    notification.last_error = Some(error.clone());
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
            "send" => self.send(decode(
                request.get("message").unwrap_or(request),
                "message",
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
            "register_adapter", "register_scope", "register_agent", "refresh_agent",
            "send", "set_agent_state", "tick", "flush_notifications", "report_bug",
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
            "p0Breakthrough": true
        },
        "facts": {
            "format": "jsonl",
            "path": ".appsdk-control/communication/mailbox.jsonl",
            "projection": "replayed",
            "lock": "exclusive-command-lifecycle"
        },
        "adapters": ["mailbox", "tmux", "appserver"],
        "adapterBinding": "recipient-address",
        "completionEvidence": {
            "bug": ["fix", "verification", "merge"],
            "loop": ["gate", "verification"]
        }
    })
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

fn worker_idle_message(current: &AgentRecord, master_address: Address, at: &str) -> MessageRequest {
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
        message_id: None,
        created_at: Some(at.into()),
        adapter_id: None,
    }
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
    }
    Ok(evidence.clone())
}

fn validate_loop_completion_evidence(evidence: Option<&Value>) -> CommResult<()> {
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
    let gate = object.get("gate").or_else(|| object.get("verification"));
    if gate.is_none()
        || gate.is_some_and(|value| {
            value.as_str().is_some_and(|text| text.trim().is_empty())
                || value.as_object().is_some_and(|object| object.is_empty())
        })
    {
        return Err(CommError::new(
            "loop_gate_evidence_required",
            "loop gate evidence must identify a non-empty gate or verification result",
        ));
    }
    Ok(())
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
