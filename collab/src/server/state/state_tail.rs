#[derive(Debug, Default)]
pub struct State {
    pub pending_project_resets: std::collections::BTreeMap<String, bool>,
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
    pub worker_names: HashMap<String, String>,
    pub name_occupancies:
        HashMap<String, HashMap<String, crate::server::name_state::NameOccupancy>>,
    /// Per-project/name durable high watermark, including released names.
    pub name_occupancy_revisions: HashMap<String, HashMap<String, u64>>,
    pub task_assignments: HashMap<String, crate::server::name_state::TaskAssignment>,
    pub name_work_roles: HashMap<String, HashMap<String, crate::board::NameWorkRoles>>,
    /// Master-configured workflow templates keyed by project scope, then target
    /// (`master`, `peer`, or `role:<specialty>`).
    pub workflow_templates:
        HashMap<String, HashMap<String, crate::board::WorkflowTemplateOverride>>,
    /// Project-level default policy keyed by exact `work_kind`.
    pub project_work_policy: HashMap<String, crate::board::WorkPolicy>,
    pub name_recoveries: HashMap<String, crate::server::name_state::NameRecoveryPending>,
    pub agent_presence:
        HashMap<String, HashMap<String, crate::server::name_state::AgentPresenceRecord>>,
    pub agent_activity:
        HashMap<String, HashMap<String, crate::server::name_state::AgentActivityRecord>>,
    pub msgs: HashMap<String, Message>,
    pub target_sessions: HashMap<String, String>,
    /// Reply obligations keyed by the original message id. This is the single
    /// daemon-owned control record; message bodies never carry it.
    pub reply_obligations: HashMap<String, ReplyObligation>,
    pub mailbox_name_addresses: HashMap<String, crate::server::name_mailbox::MailboxNameAddress>,
    pub mailbox_handoffs: Vec<crate::server::name_mailbox::MailboxHandoff>,
    /// Per-external-conversation default delivery target, keyed by
    /// `<channel>:<conversation>`. Control state only; never grants identity.
    pub conversation_defaults:
        std::collections::HashMap<String, crate::server::conversation_default::ConversationDefault>,
    /// Typed channel envelopes keyed by the Collab inbound message id.
    pub channel_envelopes:
        std::collections::HashMap<String, crate::server::channel_envelope::ChannelEnvelope>,
    /// Platform event idempotency index: event key -> Collab message id.
    pub platform_event_index: std::collections::HashMap<String, String>,
    /// Immutable QQ route for each committed agent reply, keyed by the Collab
    /// outbound message id.
    pub channel_reply_routes:
        std::collections::HashMap<String, crate::server::channel_envelope::ChannelReplyRoute>,
    /// Platform send outcome keyed by the Collab outbound message id. Never
    /// projected into `Message.state`.
    pub channel_delivery_results:
        std::collections::HashMap<String, crate::server::channel_delivery::ChannelDeliveryResult>,
    pub notification_delivery_failures: HashMap<String, NotificationDeliveryFailure>,
    /// Last native attempt accepted for a durable message, if any.
    pub notification_delivery_accepted: HashMap<String, i64>,
    pub notification_delivery_evidence: HashMap<String, serde_json::Value>,
    pub receive_receipts: HashMap<String, ReceiveReceipt>,
    pub tasks: HashMap<String, TaskRec>,
    pub board_details: HashMap<String, crate::board::BoardTaskDetails>,
    pub cancelled_board_patrol_tasks:
        std::collections::BTreeMap<String, crate::board_patrol_policy::TaskPatrolCancelRecord>,
    pub board_patrol_gates:
        std::collections::BTreeMap<String, crate::board_patrol_policy::PeerBoardPatrolGate>,
    /// Durable cue marker for daemon-generated board-idle patrol messages.
    /// Delivery revalidation reads this marker, never the body or subject.
    pub board_patrol_notice_ids: std::collections::BTreeSet<String>,
    /// Canonical CEO progress coalescing and report acknowledgement state.
    /// Only the host runtime writes it; authorization, grant eligibility and
    /// clock ownership belong to the later caller. Initialize with the typed
    /// default so replay and snapshot restore the exact helper state.
    pub ceo_report_state: crate::ceo_report_state::CeoReportState,
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
    /// Project-mirrored global CEO grant. `CeoGrantSet` is the only writer;
    /// `None` revokes the grant and never touches bootstrap history.
    pub ceo_grant: Option<crate::ceo::CeoGrant>,
    /// Durable master-bootstrap launch intents keyed by operation id.
    pub ceo_bootstraps: HashMap<String, crate::ceo::CeoBootstrapRecord>,
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
        && (state.master_worker_id.as_deref() == Some(subscription.worker_id.as_str())
            || has_current_master_grant(state, &subscription.worker_id, &subscription.target))
}

#[path = "../state_impl.rs"]
mod state_impl;

pub(crate) use state_impl::board_patrol_current_owner_key;
pub(crate) use state_impl::board_patrol_eligible_task_ids;
pub(crate) use state_impl::board_patrol_notice_is_current;
pub(crate) use state_impl::board_patrol_owned_unclosed_task_ids;
pub(crate) use state_impl::board_patrol_owner_key;

#[cfg(test)]
#[path = "../state_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../name_state_tests.rs"]
mod name_state_tests;
