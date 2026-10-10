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

#[path = "../state_impl.rs"]
mod state_impl;

#[cfg(test)]
#[path = "../state_tests.rs"]
mod tests;
