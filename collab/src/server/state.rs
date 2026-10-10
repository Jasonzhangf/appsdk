use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use super::global_state::{GlobalState, ProjectRegistration, RuntimeBinding, StateError};
use crate::proto::{
    CommandEnvelope, PeerLifecycleAction, PeerLifecycleClose, PeerLifecycleCreate,
    PeerLifecyclePhase, PeerLifecycleTarget, PeerLifecycleUpdate, SelectedTransport, TransportKind,
};

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn default_task_status() -> String {
    "working".into()
}

pub fn default_priority() -> String {
    "p2".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitSpec {
    #[serde(default)]
    pub waiter: String,
    pub waiting_for: String,
    pub responsible_actor: String,
    pub reason: String,
    pub deadline_ms: i64,
    pub resume_on: Vec<String>,
    pub escalation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationRecord {
    pub id: String,
    pub from_version: String,
    pub to_version: String,
    pub phase: String,
    pub admission_frozen: bool,
    pub snapshot_hash: Option<String>,
    pub worker_count: usize,
    pub task_count: usize,
    pub message_count: usize,
    pub operator: String,
    pub issues: Vec<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerRec {
    pub id: String,
    pub token: String,
    pub cwd: String,
    pub registered_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<SelectedTransport>,
}

impl WorkerRec {
    pub fn transport_kind(&self) -> Option<TransportKind> {
        self.transport
            .as_ref()
            .map(|transport| transport.kind.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeBinding {
    pub worktree_root: String,
    pub owning_project_scope: String,
    pub task_id: String,
    pub owner_agent_id: String,
    pub binding_id: String,
    pub base_commit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandReceipt {
    pub operation_id: String,
    pub outcome: serde_json::Value,
    #[serde(default)]
    pub sequence: u64,
    #[serde(default)]
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "cmd")]
pub enum TypedCommand {
    RegisterWorker {
        registration: ProjectRegistration,
        binding: RuntimeBinding,
        worker: WorkerRec,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "gr")]
pub enum GlobalEvent {
    ProjectRegistered {
        registration: ProjectRegistration,
    },
    RuntimeBound {
        binding: RuntimeBinding,
    },
    MasterGranted {
        grant: super::global_state::MasterGrant,
    },
    MasterRevoked {
        project_scope: crate::scope::ProjectScopeId,
        binding_id: crate::identity::BindingId,
    },
    MigrationCommitEvidence {
        evidence: super::global_state::MigrationCommitEvidence,
    },
    RuntimeBindingLedgerClassified {
        record: super::global_state::RuntimeBindingLedgerRecord,
    },
    LedgerScanReceiptRecorded {
        receipt: super::global_state::LedgerScanReceipt,
    },
    MasterGrantReplacementStarted {
        intent: super::global_state::MasterGrantReplacementIntent,
    },
    MasterGrantReplacementCompleted {
        receipt: super::global_state::MasterGrantReplacementReceipt,
    },
}

impl GlobalEvent {
    pub fn apply(self, global: &mut GlobalState) -> Result<(), StateError> {
        match self {
            Self::ProjectRegistered { registration } => {
                global.register_project(registration).map(|_| ())
            }
            Self::RuntimeBound { binding } => global.bind_runtime(binding).map(|_| ()),
            Self::MasterGranted { grant } => global.grant_master(grant).map(|_| ()),
            Self::MasterRevoked {
                project_scope,
                binding_id,
            } => global
                .revoke_master(&project_scope, &binding_id)
                .map(|_| ()),
            Self::MigrationCommitEvidence { evidence } => {
                global.record_migration_commit_evidence(evidence)
            }
            Self::RuntimeBindingLedgerClassified { record } => {
                global.classify_runtime_binding_ledger(record).map(|_| ())
            }
            Self::LedgerScanReceiptRecorded { receipt } => {
                global.record_ledger_scan_receipt(receipt).map(|_| ())
            }
            Self::MasterGrantReplacementStarted { intent } => global
                .record_master_grant_replacement_intent(intent)
                .map(|_| ()),
            Self::MasterGrantReplacementCompleted { receipt } => global
                .record_master_grant_replacement_receipt(receipt)
                .map(|_| ()),
        }
    }
}

impl From<TypedCommand> for GlobalEvent {
    fn from(command: TypedCommand) -> Self {
        match command {
            TypedCommand::RegisterWorker { registration, .. } => {
                GlobalEvent::ProjectRegistered { registration }
            }
        }
    }
}

impl TypedCommand {
    pub fn global_events(&self) -> Vec<GlobalEvent> {
        match self {
            Self::RegisterWorker {
                registration,
                binding,
                ..
            } => vec![
                GlobalEvent::ProjectRegistered {
                    registration: registration.clone(),
                }
                .into(),
                GlobalEvent::RuntimeBound {
                    binding: binding.clone(),
                }
                .into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TypedEnvelope {
    pub command: TypedCommand,
    pub envelope: CommandEnvelope,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedApplyError(pub StateError);

impl From<StateError> for TypedApplyError {
    fn from(value: StateError) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for TypedApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for TypedApplyError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypedOutcome {
    pub receipt: super::global_state::CommandReceipt,
    pub replayed: bool,
}

pub const MAX_WAKE_ATTEMPTS: u32 = 1;
pub const MAX_NOTIFICATION_REPEATS: u32 = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationSubscription {
    pub id: String,
    pub worker_id: String,
    pub event: String,
    pub subject: Option<String>,
    pub target: String,
    pub method: String,
    pub trigger_ms: Option<i64>,
    #[serde(default)]
    pub trigger_times_ms: Vec<i64>,
    #[serde(default)]
    pub interval_ms: Option<i64>,
    #[serde(default = "default_repeat_count")]
    pub repeat_count: u32,
    #[serde(default)]
    pub fired_count: u32,
    pub expires_ms: i64,
    pub status: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    #[serde(default)]
    pub status_reason: Option<String>,
}

pub fn default_repeat_count() -> u32 {
    1
}

impl NotificationSubscription {
    pub fn matches(&self, worker_id: &str, event: &str, subject: Option<&str>, now: i64) -> bool {
        self.worker_id == worker_id
            && self.event == event
            && self.subject.as_deref() == subject
            && crate::proto::TransportKind::from_method(self.method.as_str()).is_some()
            && self.status == "armed"
            && self.expires_ms > now
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub mtype: String,
    #[serde(default)]
    pub subject: Option<String>,
    pub body: String,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    pub created_ms: i64,
    /// pending -> delivered -> read; replies may also become superseded.
    pub state: String,
    #[serde(default, alias = "nudge_count")]
    pub wake_attempt_count: u32,
    #[serde(default, alias = "last_nudge_ms")]
    pub last_wake_attempt_ms: i64,
    /// One explicit recovery attempt may follow a known-undelivered attempt;
    /// this marker makes that budget durable and replayable.
    #[serde(default)]
    pub retry_attempted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationDeliveryFailure {
    pub message_id: String,
    pub operation: String,
    pub error: String,
    pub failed_ms: i64,
    #[serde(default)]
    pub retryable: bool,
}

/// Durable identity of one committed receive batch. The receive identity is
/// caller-owned and persisted with the consumption, so a lost socket response
/// can replay the exact batch without inventing a second mailbox. Only message
/// identities are stored; retention still owns message bodies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReceiveReceipt {
    pub receive_id: String,
    pub worker_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_scope: Option<crate::scope::RouteScope>,
    pub message_ids: Vec<String>,
    pub received_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentSnapshotReceipt {
    pub subagent_id: String,
    pub thread_id: String,
    pub captured_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerSnapshotReceipt {
    pub worker_id: String,
    pub thread_id: String,
    pub captured_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerCloseReceipt {
    pub worker_id: String,
    pub closed_by: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_captured_ms: Option<i64>,
    pub at_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResponsibilitySnapshot {
    #[serde(default)]
    pub task_ids: Vec<String>,
    #[serde(default)]
    pub scheduler_request_ids: Vec<String>,
    #[serde(default)]
    pub managed_subagent_ids: Vec<String>,
    #[serde(default)]
    pub worktree_binding_ids: Vec<String>,
    #[serde(default)]
    pub subscription_ids: Vec<String>,
    #[serde(default)]
    pub unread_message_ids: Vec<String>,
}

/// Durable close-admission fence for one exact peer generation.
///
/// The snapshot records only responsibility identities. It deliberately does
/// not carry message bodies, credentials, or host payloads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResponsibilityFence {
    pub operation_id: String,
    pub worker_id: String,
    pub project_scope: String,
    pub app_scope: String,
    pub binding_id: String,
    pub endpoint_generation: u64,
    pub snapshot: ResponsibilitySnapshot,
    pub state: String,
    pub created_ms: i64,
}

impl ResponsibilityFence {
    /// Any durable fence blocks new responsibility for its exact target.
    /// B23-F has no release path; `partial` and `unknown` lifecycle phases stay
    /// fenced until a later lifecycle owner adds a proof-backed release.
    pub fn is_active(&self) -> bool {
        matches!(self.state.as_str(), "closing" | "partial" | "unknown")
    }

    pub fn matches_binding(
        &self,
        worker_id: &str,
        project_scope: &str,
        app_scope: &str,
        binding_id: &str,
        endpoint_generation: u64,
    ) -> bool {
        self.worker_id == worker_id
            && self.project_scope == project_scope
            && self.app_scope == app_scope
            && self.binding_id == binding_id
            && self.endpoint_generation == endpoint_generation
    }
}

/// Durable lifecycle operation projection.
///
/// The immutable intent is written before any host effect. Later events may
/// advance only the mutable phase and owner readback; replay rejects any
/// attempt to change the frozen target, actor, capability hash, or digest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PeerLifecycleOperationRecord {
    pub operation_id: String,
    pub action: PeerLifecycleAction,
    pub actor_id: String,
    pub project_scope: crate::scope::ProjectScopeId,
    pub app_scope_id: crate::identity::AppServerId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<PeerLifecycleTarget>,
    pub intent_digest: String,
    pub query_capability_hash: String,
    /// Close captures the exact responsibility set with the operation event.
    /// The reducer installs the matching fence from this same record so a
    /// restart can never recover one without the other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responsibility_snapshot: Option<ResponsibilitySnapshot>,
    pub phase: PeerLifecyclePhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<PeerLifecycleUpdate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close: Option<PeerLifecycleClose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create: Option<PeerLifecycleCreate>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

impl PeerLifecycleOperationRecord {
    // Only R/U/C callers use this accessor; the reducer requires their target.
    pub fn exact_target(&self) -> &PeerLifecycleTarget {
        self.target
            .as_ref()
            .expect("validated non-Create lifecycle target")
    }
}

fn close_fence_state(phase: &PeerLifecyclePhase) -> Option<&'static str> {
    match phase {
        PeerLifecyclePhase::IntentPersisted
        | PeerLifecyclePhase::HostDispatchClaimed
        | PeerLifecyclePhase::HostDispatched
        | PeerLifecyclePhase::ReadbackPending => Some("closing"),
        PeerLifecyclePhase::Partial | PeerLifecyclePhase::CleanupOpen => Some("partial"),
        PeerLifecyclePhase::Unknown => Some("unknown"),
        PeerLifecyclePhase::Complete
        | PeerLifecyclePhase::Refused
        | PeerLifecyclePhase::Cancelled => None,
    }
}

/// Build the exact fence represented by a Close lifecycle operation.
pub fn close_fence_for_operation(
    operation: &PeerLifecycleOperationRecord,
) -> Option<ResponsibilityFence> {
    let snapshot = operation.responsibility_snapshot.as_ref()?;
    let state = close_fence_state(&operation.phase)?;
    Some(ResponsibilityFence {
        operation_id: operation.operation_id.clone(),
        worker_id: operation.exact_target().worker_id.clone(),
        project_scope: operation.exact_target().project_scope.as_str().to_owned(),
        app_scope: operation.exact_target().app_scope_id.as_str().to_owned(),
        binding_id: operation.exact_target().binding_id.as_str().to_owned(),
        endpoint_generation: operation.exact_target().endpoint_generation,
        snapshot: snapshot.clone(),
        state: state.to_owned(),
        created_ms: operation.created_ms,
    })
}

fn sorted_unique(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

/// Responsibility snapshot captured atomically with a close admission.
///
/// Only identities are persisted. `TaskRec`, messages, subscriptions and
/// worktree records remain the original owners of their payloads.
pub fn responsibility_snapshot(state: &State, worker_id: &str) -> ResponsibilitySnapshot {
    let task_ids = sorted_unique(
        state
            .tasks
            .values()
            .filter(|task| {
                task.owner == worker_id && !matches!(task.status.as_str(), "closed" | "cancelled")
            })
            .map(|task| task.id.clone())
            .collect(),
    );
    let scheduler_request_ids = sorted_unique(
        state
            .scheduler_admissions
            .values()
            .filter(|admission| {
                admission.worker_id == worker_id
                    && matches!(admission.status.as_str(), "pending" | "notifying")
            })
            .map(|admission| admission.request_id.clone())
            .collect(),
    );
    let managed_subagent_ids = sorted_unique(
        state
            .subagents
            .values()
            .filter(|subagent| {
                subagent.peer == worker_id
                    && !matches!(subagent.status.as_str(), "closed" | "failed")
            })
            .map(|subagent| subagent.id.clone())
            .collect(),
    );
    let worktree_binding_ids = sorted_unique(
        state
            .worktree_bindings
            .values()
            .filter(|binding| {
                binding.owner_agent_id == worker_id
                    && state
                        .tasks
                        .get(&binding.task_id)
                        .is_none_or(|task| !matches!(task.status.as_str(), "closed" | "cancelled"))
            })
            .map(|binding| binding.binding_id.clone())
            .collect(),
    );
    let subscription_ids = sorted_unique(
        state
            .notification_subscriptions
            .values()
            .filter(|subscription| {
                subscription.worker_id == worker_id
                    && matches!(subscription.status.as_str(), "armed" | "notifying")
            })
            .map(|subscription| subscription.id.clone())
            .collect(),
    );
    let unread_message_ids = sorted_unique(
        state
            .msgs
            .values()
            .filter(|message| {
                message.to == worker_id && matches!(message.state.as_str(), "pending" | "delivered")
            })
            .map(|message| message.id.clone())
            .collect(),
    );
    ResponsibilitySnapshot {
        task_ids,
        scheduler_request_ids,
        managed_subagent_ids,
        worktree_binding_ids,
        subscription_ids,
        unread_message_ids,
    }
}

/// A task status that no longer carries responsibility for its owner. This
/// matches [`responsibility_snapshot`], which keeps every other status.
fn task_status_is_terminal(status: &str) -> bool {
    matches!(status, "closed" | "cancelled")
}

/// A subscription status that still binds an obligation to its owner.
fn subscription_status_is_active(status: &str) -> bool {
    matches!(status, "armed" | "notifying")
}

/// A subscription status that only retires or consumes the obligation. These
/// are safe under an active fence because they reduce rather than add
/// responsibility. `rebound` is deliberately not here: it re-points a wake and
/// is treated as a mutation.
fn subscription_status_retires(status: &str) -> bool {
    matches!(
        status,
        "cancelled" | "consumed" | "expired" | "transport-lost" | "suppressed"
    )
}

/// The fence-relevant decision for one event.
///
/// `Mutates` means the event adds or changes responsibility owned by the
/// named target; `Safe` covers retirement, consumption, history, and unrelated
/// events. The distinction is drawn from the same statuses as the snapshot so
/// a fence cannot be bypassed by a writer whose event shape was not listed.
enum ResponsibilityEffect<'a> {
    Mutates(&'a str),
    Safe,
}

#[derive(Clone, Copy)]
enum SubscriptionConsumptionStatus {
    Unchanged,
    Consumed,
    Armed,
}

#[derive(Clone, Copy)]
struct SubscriptionConsumption {
    fired_count: u32,
    status: SubscriptionConsumptionStatus,
}

/// A retired runtime binding carries no live native thread or tmux pane, so it
/// is an exact control-plane retirement record rather than a new
/// responsibility. The session id is intentionally not required to be absent:
/// the route owners retain it across retirement.
fn runtime_endpoint_is_retired(binding: &crate::server::global_state::RuntimeBinding) -> bool {
    binding.native_thread_id.is_none() && binding.tmux_endpoint.is_none()
}

fn subscription_consumption_total(subscription: &NotificationSubscription) -> u32 {
    if subscription.interval_ms.is_some() {
        subscription.repeat_count
    } else {
        subscription.trigger_times_ms.len().max(1) as u32
    }
}

fn consume_subscription_for_preflight(
    subscription: &NotificationSubscription,
    consumed_ms: Option<i64>,
) -> SubscriptionConsumption {
    if subscription.status == "cancelled" {
        return SubscriptionConsumption {
            fired_count: subscription.fired_count,
            status: SubscriptionConsumptionStatus::Unchanged,
        };
    }

    let fired_count = subscription.fired_count.saturating_add(1);
    if is_goal_deadline(subscription) {
        let _ = consumed_ms;
        return SubscriptionConsumption {
            fired_count,
            status: SubscriptionConsumptionStatus::Consumed,
        };
    }
    if fired_count >= subscription_consumption_total(subscription) {
        return SubscriptionConsumption {
            fired_count,
            status: SubscriptionConsumptionStatus::Consumed,
        };
    }
    let _ = consumed_ms;
    SubscriptionConsumption {
        fired_count,
        status: SubscriptionConsumptionStatus::Armed,
    }
}

/// Classify how one event relates to responsibility for a closing peer.
///
/// This is intentionally a single table over every durable event shape used by
/// the responsibility writers. New events default to [`Safe`] only when they
/// cannot add or change an owner; every event this call cannot prove safe is
/// classified as a mutation for its exact target.
fn responsibility_effect<'a>(state: &'a State, event: &'a Event) -> ResponsibilityEffect<'a> {
    match event {
        Event::TaskCreated { task } => ResponsibilityEffect::Mutates(task.owner.as_str()),
        Event::TaskUpdated { task } => {
            if task_status_is_terminal(&task.status) {
                ResponsibilityEffect::Safe
            } else {
                ResponsibilityEffect::Mutates(task.owner.as_str())
            }
        }
        Event::SchedulerAdmission { admission } => {
            if matches!(admission.status.as_str(), "pending" | "notifying") {
                ResponsibilityEffect::Mutates(admission.worker_id.as_str())
            } else {
                ResponsibilityEffect::Safe
            }
        }
        Event::SchedulerAdmissionStatus {
            request_id, status, ..
        } => match state
            .scheduler_admissions
            .get(request_id)
            .map(|admission| admission.worker_id.as_str())
        {
            Some(worker_id) => match status.as_str() {
                "pending" | "notifying" | "succeeded" => ResponsibilityEffect::Mutates(worker_id),
                _ => ResponsibilityEffect::Safe,
            },
            None => ResponsibilityEffect::Safe,
        },
        Event::SubagentUpdated { subagent } => match subagent.status.as_str() {
            "closed" | "failed" => ResponsibilityEffect::Safe,
            _ => ResponsibilityEffect::Mutates(subagent.peer.as_str()),
        },
        Event::WorktreeBound { binding } => {
            ResponsibilityEffect::Mutates(binding.owner_agent_id.as_str())
        }
        Event::NotificationSubscribed { subscription } => {
            if subscription_status_is_active(&subscription.status) {
                ResponsibilityEffect::Mutates(subscription.worker_id.as_str())
            } else {
                ResponsibilityEffect::Safe
            }
        }
        Event::NotificationStatus {
            subscription_id,
            status,
            ..
        } => match state.notification_subscriptions.get(subscription_id) {
            Some(subscription) => {
                if subscription_status_retires(status) {
                    ResponsibilityEffect::Safe
                } else {
                    ResponsibilityEffect::Mutates(subscription.worker_id.as_str())
                }
            }
            None => ResponsibilityEffect::Safe,
        },
        Event::NotificationRebound {
            subscription_id, ..
        } => match state.notification_subscriptions.get(subscription_id) {
            Some(subscription) => ResponsibilityEffect::Mutates(subscription.worker_id.as_str()),
            None => ResponsibilityEffect::Safe,
        },
        Event::NotificationSuppressed {
            subscription_id,
            status,
            ..
        } => match state.notification_subscriptions.get(subscription_id) {
            Some(subscription) => {
                if subscription_status_retires(status) {
                    ResponsibilityEffect::Safe
                } else {
                    ResponsibilityEffect::Mutates(subscription.worker_id.as_str())
                }
            }
            None => ResponsibilityEffect::Safe,
        },
        Event::NotificationSkipped {
            subscription_id, ..
        } => match state.notification_subscriptions.get(subscription_id) {
            Some(subscription) => ResponsibilityEffect::Mutates(subscription.worker_id.as_str()),
            None => ResponsibilityEffect::Safe,
        },
        Event::NotificationConsumed {
            subscription_id, ..
        } => match state.notification_subscriptions.get(subscription_id) {
            Some(subscription) if subscription_status_is_active(&subscription.status) => {
                ResponsibilityEffect::Mutates(subscription.worker_id.as_str())
            }
            Some(_) | None => ResponsibilityEffect::Safe,
        },
        Event::Acked { .. } | Event::ReceiveCommitted { .. } => ResponsibilityEffect::Safe,
        // Any `Sent` to the target adds an unread message, and
        // `responsibility_snapshot` counts unread messages as responsibility.
        // Replies, acks, and system notifications still land in the target's
        // mailbox, so all of them are additions for the receiving peer.
        Event::Sent { msg } => ResponsibilityEffect::Mutates(msg.to.as_str()),
        Event::WakeBound { message_id, .. } => match state.msgs.get(message_id) {
            Some(msg) => ResponsibilityEffect::Mutates(msg.to.as_str()),
            None => ResponsibilityEffect::Safe,
        },
        Event::Registered { worker } => ResponsibilityEffect::Mutates(worker.id.as_str()),
        Event::MergeRequested { request } => ResponsibilityEffect::Mutates(request.owner.as_str()),
        Event::MasterAssigned { worker_id, .. } => {
            ResponsibilityEffect::Mutates(worker_id.as_str())
        }
        Event::GlobalCurrentThreadRouteSet { binding } => {
            ResponsibilityEffect::Mutates(binding.agent_id.as_str())
        }
        // A `GlobalRuntimeBound` that no longer carries a live endpoint is the
        // control-plane retirement record for the retired generation; it must
        // stay committable while the fence holds.
        Event::GlobalRuntimeBound { binding } => {
            if runtime_endpoint_is_retired(binding) {
                ResponsibilityEffect::Safe
            } else {
                ResponsibilityEffect::Mutates(binding.agent_id.as_str())
            }
        }
        Event::GlobalMasterGranted { grant } => {
            ResponsibilityEffect::Mutates(grant.agent_id.as_str())
        }
        // A rollback can restore a worker, its route, and its subscriptions.
        // Refuse it while the exact failed target is fenced.
        Event::GlobalRuntimeBindingRollback { failed, .. } => {
            ResponsibilityEffect::Mutates(failed.agent_id.as_str())
        }
        Event::PeerLifecycleOperationRecorded { operation }
            if operation.action == PeerLifecycleAction::Update
                && operation.phase.is_in_flight() =>
        {
            ResponsibilityEffect::Mutates(operation.exact_target().worker_id.as_str())
        }
        Event::ReducerSnapshot { .. } => ResponsibilityEffect::Safe,
        _ => ResponsibilityEffect::Safe,
    }
}

/// Batch-level preflight used before any journal bytes are written.
///
/// The fence is evaluated against the fence set as it stands before the batch
/// plus any fence set earlier in the same batch, so a close admission
/// that writes its own fence can never be combined with a responsibility
/// addition for the same target. Safe retirement events owned by the exact
/// closing target stay allowed.
pub fn responsibility_preflight(st: &State, evs: &[Event]) -> Result<(), String> {
    let mut fences = st.responsibility_fences.clone();
    let mut simulated_msgs = st.msgs.clone();
    let mut simulated_subscriptions = st.notification_subscriptions.clone();
    let mut simulated_wake_bindings = st.wake_bindings.clone();
    for event in evs {
        match event {
            Event::ReducerSnapshot { .. } => {
                return Err("reducer snapshot is replay-only".into());
            }
            Event::ResponsibilityFenceSet { fence } => {
                fences.insert(fence.operation_id.clone(), fence.clone());
            }
            Event::PeerLifecycleOperationRecorded { operation } => {
                if operation.action == PeerLifecycleAction::Close {
                    if operation.phase == PeerLifecyclePhase::IntentPersisted
                        && st.peer_lifecycle_operations.values().any(|existing| {
                            existing.action == PeerLifecycleAction::Update
                                && existing.phase.is_in_flight()
                                && existing.exact_target().worker_id
                                    == operation.exact_target().worker_id
                        })
                    {
                        return Err(format!(
                            "target_updating: close for {} is excluded by an in-flight update",
                            operation.exact_target().worker_id
                        ));
                    }
                    match close_fence_for_operation(operation) {
                        Some(fence) => {
                            fences.insert(fence.operation_id.clone(), fence);
                        }
                        None => {
                            fences.remove(&operation.operation_id);
                        }
                    }
                }
            }
            Event::Sent { msg } => {
                simulated_msgs.insert(msg.id.clone(), msg.clone());
            }
            Event::Delivered { ids } => {
                for id in ids {
                    if let Some(message) = simulated_msgs.get_mut(id) {
                        if message.state == "pending" {
                            message.state = "delivered".into();
                        }
                    }
                }
            }
            Event::Acked { ids } | Event::ReceiveCommitted { ids, .. } => {
                for id in ids {
                    if let Some(message) = simulated_msgs.get_mut(id) {
                        message.state = "read".into();
                    }
                    let Some(subscription_id) = simulated_wake_bindings.get(id).cloned() else {
                        continue;
                    };
                    let Some(subscription) = simulated_subscriptions.get(&subscription_id).cloned()
                    else {
                        continue;
                    };
                    let consumes_on_read = subscription.event != "direct-message"
                        || subscription.trigger_ms.is_some()
                        || !subscription.trigger_times_ms.is_empty()
                        || subscription.interval_ms.is_some();
                    if !consumes_on_read {
                        continue;
                    }
                    let read_count = simulated_wake_bindings
                        .iter()
                        .filter(|(_, bound)| *bound == &subscription_id)
                        .filter(|(message_id, _)| {
                            simulated_msgs
                                .get(*message_id)
                                .is_some_and(|message| message.state == "read")
                        })
                        .count() as u32;
                    if read_count > subscription.fired_count {
                        let consumed = consume_subscription_for_preflight(
                            &subscription,
                            if subscription.event == "master-idle" {
                                simulated_msgs.get(id).and_then(|message| {
                                    [message.last_wake_attempt_ms, message.created_ms]
                                        .into_iter()
                                        .find(|timestamp| *timestamp > 0)
                                })
                            } else {
                                None
                            },
                        );
                        if let Some(subscription) =
                            simulated_subscriptions.get_mut(&subscription_id)
                        {
                            subscription.fired_count = consumed.fired_count;
                            subscription.status = match consumed.status {
                                SubscriptionConsumptionStatus::Unchanged => {
                                    subscription.status.clone()
                                }
                                SubscriptionConsumptionStatus::Consumed => "consumed".into(),
                                SubscriptionConsumptionStatus::Armed => "armed".into(),
                            };
                        }
                    }
                }
            }
            Event::NotificationConsumed {
                subscription_id,
                consumed_ms,
                ..
            } => {
                if let Some(subscription) = simulated_subscriptions.get(subscription_id).cloned() {
                    let consumed =
                        consume_subscription_for_preflight(&subscription, Some(*consumed_ms));
                    if let Some(subscription) = simulated_subscriptions.get_mut(subscription_id) {
                        subscription.fired_count = consumed.fired_count;
                        subscription.status = match consumed.status {
                            SubscriptionConsumptionStatus::Unchanged => subscription.status.clone(),
                            SubscriptionConsumptionStatus::Consumed => "consumed".into(),
                            SubscriptionConsumptionStatus::Armed => "armed".into(),
                        };
                    }
                }
            }
            Event::NotificationStatus {
                subscription_id,
                status,
                ..
            }
            | Event::NotificationSuppressed {
                subscription_id,
                status,
                ..
            } => {
                if let Some(subscription) = simulated_subscriptions.get_mut(subscription_id) {
                    subscription.status = status.clone();
                }
            }
            _ => {}
        }

        let ResponsibilityEffect::Mutates(target) = responsibility_effect(st, event) else {
            continue;
        };
        if let Some(fence) = fences
            .values()
            .find(|fence| fence.is_active() && fence.worker_id == target)
        {
            return Err(format!(
                "target_closing: responsibility for {} is fenced by operation {}",
                fence.worker_id, fence.operation_id
            ));
        }
        let current_update_operation_id = match event {
            Event::PeerLifecycleOperationRecorded { operation }
                if operation.action == PeerLifecycleAction::Update =>
            {
                Some(operation.operation_id.as_str())
            }
            _ => None,
        };
        if let Some(operation) = st.peer_lifecycle_operations.values().find(|operation| {
            operation.action == PeerLifecycleAction::Update
                && operation.phase.is_in_flight()
                && operation.exact_target().worker_id == target
                && Some(operation.operation_id.as_str()) != current_update_operation_id
        }) {
            return Err(format!(
                "target_updating: responsibility for {} is excluded by operation {}",
                target, operation.operation_id
            ));
        }
    }

    for fence in fences.values().filter(|fence| fence.is_active()) {
        for (id, subscription) in &simulated_subscriptions {
            if subscription.worker_id != fence.worker_id
                || !subscription_status_is_active(&subscription.status)
                || fence.snapshot.subscription_ids.contains(id)
            {
                continue;
            }
            let was_active = st
                .notification_subscriptions
                .get(id)
                .is_some_and(|existing| subscription_status_is_active(&existing.status));
            if !was_active {
                return Err(format!(
                    "target_closing: responsibility for {} is fenced by operation {}",
                    fence.worker_id, fence.operation_id
                ));
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerAdmissionRecord {
    pub request_id: String,
    pub decision: String,
    pub worker_id: String,
    #[serde(default)]
    pub managed_subagent_id: Option<String>,
    pub message_id: String,
    pub task_id: String,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

pub const REQUEST_COOLDOWN_MS: i64 = 5 * 60 * 1000;

pub fn is_goal_deadline(subscription: &NotificationSubscription) -> bool {
    subscription.event == "deadline"
        && subscription
            .subject
            .as_deref()
            .is_some_and(|subject| subject.starts_with("goal:"))
}

/// Canonical identity for one goal deadline occurrence. Registrations and the
/// scheduler share this key so a new goal revision cannot shadow an existing
/// goal merely because its deadline happens to be the same.
pub fn goal_deadline_key(subscription: &NotificationSubscription) -> Option<(String, String, i64)> {
    if !is_goal_deadline(subscription) {
        return None;
    }
    let trigger = subscription
        .interval_ms
        .map(|interval| {
            subscription
                .trigger_ms
                .unwrap_or(subscription.created_ms.saturating_add(interval))
        })
        .or_else(|| {
            subscription
                .trigger_times_ms
                .get(subscription.fired_count as usize)
                .copied()
        })
        .or(subscription.trigger_ms)?;
    Some((
        subscription.worker_id.clone(),
        subscription.subject.clone()?,
        trigger,
    ))
}

pub use super::notification_state::{MasterWakeAccumulator, MasterWakeSignal};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRec {
    pub id: String,
    pub owner: String,
    pub created_by: String,
    #[serde(default)]
    pub feature_id: Option<String>,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub base_commit: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default = "default_task_status")]
    pub status: String,
    #[serde(default)]
    pub next_step: Option<String>,
    #[serde(default)]
    pub wait: Option<WaitSpec>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

/// Lifecycle evidence is separate from TaskRec so older producers and journal
/// events remain replayable as the task contract gains new milestones.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskLifecycleRecord {
    #[serde(default)]
    pub delivery_evidence: Option<String>,
    #[serde(default)]
    pub delivered_ms: Option<i64>,
    /// The exact candidate commit recorded at deliver time (branch head or
    /// worktree HEAD when resolvable). PendingMerge.candidate_commit is copied
    /// from this so integration can prove the candidate itself was merged.
    #[serde(default)]
    pub delivery_commit: Option<String>,
    #[serde(default)]
    pub review_evidence: Option<String>,
    #[serde(default)]
    pub reviewer: Option<String>,
    #[serde(default)]
    pub reviewed_ms: Option<i64>,
    #[serde(default)]
    pub integration_commit: Option<String>,
    #[serde(default)]
    pub integration_evidence: Option<String>,
    #[serde(default)]
    pub integrated_ms: Option<i64>,
}

/// Daemon-owned registration that an accepted task is awaiting a merge on
/// `refs/heads/main`. Created when a task review is accepted; resolved when
/// `collab task integrated` records a main-reachable commit, or when the task
/// leaves the mergeable lifecycle (rework/cancel/manual close). The daemon is
/// the single owner so a busy master cannot lose the obligation in chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMerge {
    pub task_id: String,
    pub owner: String,
    pub requested_by: String,
    pub requested_ms: i64,
    #[serde(default)]
    pub candidate_commit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "CleanupReceiptRepr")]
pub struct CleanupReceipt {
    pub id: String,
    pub task_id: String,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub verified_ms: i64,
    #[serde(default)]
    pub verification: CleanupVerification,
    #[serde(default)]
    pub manual_reason: Option<String>,
}

#[derive(Deserialize)]
struct CleanupReceiptRepr {
    id: String,
    task_id: String,
    worktree_path: Option<String>,
    branch: Option<String>,
    verified_ms: i64,
    #[serde(default)]
    verification: Option<CleanupVerification>,
    #[serde(default)]
    manual_reason: Option<String>,
}

impl From<CleanupReceiptRepr> for CleanupReceipt {
    fn from(receipt: CleanupReceiptRepr) -> Self {
        let verification = receipt
            .verification
            .unwrap_or_else(|| CleanupVerification::legacy(&receipt.manual_reason));
        Self {
            id: receipt.id,
            task_id: receipt.task_id,
            worktree_path: receipt.worktree_path,
            branch: receipt.branch,
            verified_ms: receipt.verified_ms,
            verification,
            manual_reason: receipt.manual_reason,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CleanupVerification {
    #[default]
    Verified,
    Unverified,
}

impl CleanupVerification {
    fn legacy(manual_reason: &Option<String>) -> Self {
        if manual_reason.is_some() {
            Self::Unverified
        } else {
            Self::Verified
        }
    }
}

pub fn task_resource_active(status: &str) -> bool {
    !matches!(
        status,
        "pending" | "invited" | "waiting" | "merged" | "closed" | "cancelled"
    )
}

pub fn wait_cycle(tasks: &HashMap<String, TaskRec>, task_id: &str, waiting_for: &str) -> bool {
    let mut current = waiting_for;
    let mut seen = std::collections::HashSet::new();
    while seen.insert(current.to_string()) {
        if current == task_id {
            return true;
        }
        let Some(task) = tasks.get(current) else {
            return false;
        };
        let Some(wait) = task.wait.as_ref() else {
            return false;
        };
        current = &wait.waiting_for;
    }
    true
}

/// Journal events. Every mutation is an event: live path applies + appends,
/// replay applies only. This is what makes restart recovery deterministic.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "ev")]
pub enum Event {
    MasterWakeSignal {
        signal: MasterWakeSignal,
        at_ms: i64,
    },
    MasterWakeUpdated {
        accumulator: MasterWakeAccumulator,
    },
    KeepaliveUpdated {
        worker_id: String,
        record: super::keepalive::Record,
    },
    SubagentUpdated {
        subagent: crate::subagent::Record,
    },
    SubagentSnapshotCaptured {
        subagent_id: String,
        thread_id: String,
        captured_ms: i64,
    },
    WorkerSnapshotCaptured {
        worker_id: String,
        thread_id: String,
        captured_ms: i64,
    },
    Registered {
        worker: WorkerRec,
    },
    #[serde(rename = "WorkerRemoved")]
    LegacyWorkerRemoved {
        worker_id: String,
    },
    /// Master-authorized retirement of a worker registration. Unlike the legacy
    /// remove-worker path this records who closed it and why.
    WorkerClosed {
        worker_id: String,
        closed_by: String,
        reason: String,
        #[serde(default)]
        snapshot_captured_ms: Option<i64>,
        at_ms: i64,
    },
    /// Durable close-admission fence. The fence and the exact responsibility
    /// snapshot commit together before any close effect is attempted.
    ResponsibilityFenceSet {
        fence: ResponsibilityFence,
    },
    /// Durable peer lifecycle operation projection. The immutable intent is
    /// committed before any selected-host effect.
    PeerLifecycleOperationRecorded {
        operation: PeerLifecycleOperationRecord,
    },
    #[serde(rename = "MasterTransferred")]
    LegacyMasterTransferred {
        from: String,
        to: String,
    },
    Sent {
        msg: Message,
    },
    DeliveryMode {
        msg_id: String,
        mode: String,
        #[serde(default)]
        source_thread_id: Option<String>,
    },
    WakeAttempted {
        ids: Vec<String>,
        #[serde(default)]
        attempted_ms: i64,
        #[serde(default)]
        retry: bool,
    },
    NotificationDeliveryFailed {
        message_id: String,
        operation: String,
        error: String,
        failed_ms: i64,
        #[serde(default)]
        retryable: bool,
    },
    /// The native transport accepted the notification for this message. A
    /// later explicit recovery must refuse to resend it even though the
    /// mailbox row still reads `pending` until the recipient consumes it.
    NotificationDeliveryAccepted {
        message_id: String,
        accepted_ms: i64,
        #[serde(default)]
        evidence: Option<serde_json::Value>,
    },
    NotificationSubscribed {
        subscription: NotificationSubscription,
    },
    NotificationStatus {
        subscription_id: String,
        status: String,
        updated_ms: i64,
    },
    NotificationRebound {
        subscription_id: String,
        target: String,
        updated_ms: i64,
    },
    NotificationSuppressed {
        subscription_id: String,
        status: String,
        reason: String,
        updated_ms: i64,
    },
    NotificationSkipped {
        subscription_id: String,
        reason: String,
        due_ms: i64,
        skipped_ms: i64,
    },
    NotificationConsumed {
        subscription_id: String,
        message_id: String,
        consumed_ms: i64,
    },
    WakeBound {
        message_id: String,
        subscription_id: String,
    },
    ReceiveCommitted {
        receipt: ReceiveReceipt,
        /// Consumption applied by the same event: a journal truncated after
        /// this row replays a delivered batch, never an unread one.
        ids: Vec<String>,
    },
    Delivered {
        ids: Vec<String>,
    },
    Acked {
        ids: Vec<String>,
    },
    #[serde(rename = "Nudged")]
    LegacyNudged {
        msg_id: String,
    },
    Superseded {
        ids: Vec<String>,
    },
    BoardDetailsChanged {
        task_id: String,
        details: crate::board::BoardTaskDetails,
    },
    /// Compacted journal baseline. The payload is the existing snapshot event
    /// set, and `sequence`/`revision` are the exact pre-compaction counters.
    /// This record is generated only by the journal rewrite owner.
    ReducerSnapshot {
        sequence: u64,
        revision: u64,
        events: Vec<Event>,
    },
    TaskCreated {
        task: TaskRec,
    },
    SchedulerAdmission {
        admission: SchedulerAdmissionRecord,
    },
    SchedulerAdmissionStatus {
        request_id: String,
        status: String,
        error: Option<String>,
        updated_ms: i64,
    },
    TaskUpdated {
        task: TaskRec,
    },
    TaskLifecycleUpdated {
        task_id: String,
        record: TaskLifecycleRecord,
    },
    MergeRequested {
        request: PendingMerge,
    },
    MergeResolved {
        task_id: String,
        resolved_by: String,
        #[serde(default)]
        reason: Option<String>,
        at_ms: i64,
    },
    CleanupVerified {
        receipt: CleanupReceipt,
    },
    MigrationUpdated {
        migration: MigrationRecord,
    },
    ReducerCheckpoint {
        sequence: u64,
        revision: u64,
    },
    CommandStarted {
        command_id: String,
        operation_id: String,
    },
    CommandRecorded {
        command_id: String,
        receipt: CommandReceipt,
    },
    CommandCompleted {
        command_id: String,
        operation_id: String,
        receipt: CommandReceipt,
    },
    #[serde(alias = "RootAssigned")]
    MasterAssigned {
        worker_id: String,
        assigned_by: String,
        approval: Option<String>,
        assigned_ms: i64,
    },
    WorktreeBound {
        binding: WorktreeBinding,
    },
    GlobalProjectRegistered {
        registration: super::global_state::ProjectRegistration,
    },
    GlobalRuntimeBound {
        binding: super::global_state::RuntimeBinding,
    },
    GlobalMasterGranted {
        grant: super::global_state::MasterGrant,
    },
    GlobalMasterRevoked {
        project_scope: crate::scope::ProjectScopeId,
        binding_id: crate::identity::BindingId,
    },
    GlobalRuntimeBindingRollback {
        failed: super::global_state::RuntimeBinding,
        previous: Option<super::global_state::RuntimeBinding>,
        previous_grant: Option<super::global_state::MasterGrant>,
        #[serde(default)]
        previous_worker: Option<WorkerRec>,
        #[serde(default)]
        previous_subscriptions: Vec<NotificationSubscription>,
    },
    GlobalCurrentThreadRouteSet {
        binding: super::global_state::RuntimeBinding,
    },
    GlobalCurrentThreadRouteRetired {
        binding: super::global_state::RuntimeBinding,
    },
    GlobalCurrentThreadRouteTombstoneSet {
        tombstone: super::global_state::RuntimeBindingTombstone,
    },
    GlobalMigrationCommitEvidence {
        evidence: super::global_state::MigrationCommitEvidence,
    },
    GlobalRuntimeBindingLedgerClassified {
        record: super::global_state::RuntimeBindingLedgerRecord,
    },
    GlobalLedgerScanReceiptRecorded {
        receipt: super::global_state::LedgerScanReceipt,
    },
    GlobalMasterGrantReplacementStarted {
        intent: super::global_state::MasterGrantReplacementIntent,
    },
    GlobalMasterGrantReplacementCompleted {
        receipt: super::global_state::MasterGrantReplacementReceipt,
    },
}

#[derive(Debug, Default)]
pub struct State {
    /// Monotonic in-memory reducer revision and journal sequence.  These are
    /// not business payload and are advanced only by the resident writer.
    pub revision: u64,
    pub sequence: u64,
    /// A failed journal write makes the in-memory reducer unsafe to mutate.
    /// Keep the exact first failure so admission can fail closed.
    pub journal_poison: Option<String>,
    pub master_wake: MasterWakeAccumulator,
    pub keepalives: HashMap<String, super::keepalive::Record>,
    pub subagents: HashMap<String, crate::subagent::Record>,
    pub subagent_snapshots: HashMap<String, SubagentSnapshotReceipt>,
    pub worker_snapshots: HashMap<String, WorkerSnapshotReceipt>,
    pub worker_closures: HashMap<String, WorkerCloseReceipt>,
    pub responsibility_fences: HashMap<String, ResponsibilityFence>,
    pub peer_lifecycle_operations: HashMap<String, PeerLifecycleOperationRecord>,
    pub workers: HashMap<String, WorkerRec>,
    pub msgs: HashMap<String, Message>,
    pub notification_delivery_failures: HashMap<String, NotificationDeliveryFailure>,
    /// Last native attempt accepted for a durable message, if any.
    pub notification_delivery_accepted: HashMap<String, i64>,
    pub notification_delivery_evidence: HashMap<String, serde_json::Value>,
    pub receive_receipts: HashMap<String, ReceiveReceipt>,
    pub tasks: HashMap<String, TaskRec>,
    pub board_details: HashMap<String, crate::board::BoardTaskDetails>,
    pub scheduler_admissions: HashMap<String, SchedulerAdmissionRecord>,
    pub task_lifecycle: HashMap<String, TaskLifecycleRecord>,
    /// Accepted tasks awaiting a main merge, keyed by task id. Daemon-owned so
    /// the merge obligation survives a busy master, session change, and
    /// restart.
    pub pending_merges: HashMap<String, PendingMerge>,
    pub cleanup_receipts: HashMap<String, CleanupReceipt>,
    pub delivery_modes: HashMap<String, String>,
    pub delivery_source_threads: HashMap<String, String>,
    pub notification_subscriptions: HashMap<String, NotificationSubscription>,
    pub wake_bindings: HashMap<String, String>,
    pub migration: Option<MigrationRecord>,
    /// Legacy journal projection kept for wire/replay compatibility.  Typed
    /// command idempotency is owned by `global.command_receipts`; this map is
    /// updated from the same committed event and is never consulted first.
    pub command_receipts: HashMap<String, CommandReceipt>,
    /// Keep the legacy event shape when compacting an old journal receipt.
    /// Modern command transactions retain their Started/Completed framing.
    legacy_command_ids: HashSet<String>,
    pub global: super::global_state::GlobalState,
    pub master_worker_id: Option<String>,
    pub master_assigned_by: Option<String>,
    pub master_approval: Option<String>,
    pub master_assigned_ms: Option<i64>,
    pub worktree_bindings: HashMap<String, WorktreeBinding>,
}

fn has_current_master_grant(state: &State, worker_id: &str, target: &str) -> bool {
    state.global.projects.values().any(|project| {
        project.master_grants.values().any(|grant| {
            grant.agent_id.as_str() == worker_id
                && state
                    .global
                    .lookup_master_grant(&grant.project_scope, &grant.binding_id)
                    .is_some_and(|current| current == grant)
                && state
                    .global
                    .lookup_binding_for(
                        &crate::scope::RouteScope {
                            app_scope_id: grant.app_scope_id.clone(),
                            project_scope_id: grant.project_scope.clone(),
                        },
                        &grant.binding_id,
                    )
                    .is_some_and(|binding| {
                        binding.endpoint_generation == grant.endpoint_generation
                            && binding.agent_id == grant.agent_id
                            && binding
                                .native_thread_id
                                .as_ref()
                                .is_some_and(|thread_id| thread_id.as_str() == target)
                    })
        })
    })
}

fn subscription_affects_master_wake(
    state: &State,
    subscription: &NotificationSubscription,
) -> bool {
    matches!(subscription.event.as_str(), "deadline" | "master-idle")
        && (is_goal_deadline(subscription)
            || state.master_worker_id.as_deref() == Some(subscription.worker_id.as_str())
            || has_current_master_grant(state, &subscription.worker_id, &subscription.target))
}

#[path = "state_impl.rs"]
mod state_impl;

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
