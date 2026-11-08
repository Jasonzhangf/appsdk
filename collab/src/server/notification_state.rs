use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MasterWakeSignal {
    GoalDue { revision: u64 },
    WorkerIdle { worker_id: String },
    MasterIdle { worker_id: String },
    WorkerUnresponsive { worker_id: String },
    WorkerRecovered { worker_id: String },
    WorkerWorking { worker_id: String },
    TaskBlocked { task_id: String },
    TaskFreed { task_id: String },
    SubagentStatus { subagent_id: String },
    SubagentWorking { subagent_id: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MasterWakeAccumulator {
    pub generation: u64,
    pub goal_due: bool,
    #[serde(default)]
    pub idle_workers: Vec<String>,
    /// Idle-capacity additions accumulated since the last master-idle batch.
    #[serde(default)]
    pub newly_idle_workers: Vec<String>,
    #[serde(default)]
    pub unresponsive_workers: Vec<String>,
    #[serde(default)]
    pub blocked_or_timed_out_tasks: Vec<String>,
    #[serde(default)]
    pub completed_or_freed_tasks: Vec<String>,
    #[serde(default)]
    pub highest_bug_revision: Option<u64>,
    #[serde(default)]
    pub active_goal_revision: Option<u64>,
    /// Board task set currently forming a dispatch gap: `status == pending`,
    /// still held by the publisher/current master, no active invitation, and
    /// publicly visible in the current master's board scope.
    #[serde(default)]
    pub dispatch_gap_task_ids: Vec<String>,
    /// One-layer direct peers with positive typed idle activity and a current
    /// binding. Unknown, cold, offline, waiting_user and managed children are
    /// never added here.
    #[serde(default)]
    pub dispatch_gap_idle_peer_ids: Vec<String>,
    /// Sorted task+peer condition key of the last durable dispatch-gap
    /// reminder. `None` while the gap is resolved so a new gap notifies.
    #[serde(default)]
    pub last_dispatch_gap_notice_key: Option<String>,
    /// Durable commit time of the last dispatch-gap reminder. A local check,
    /// skip or failed delivery must not move it.
    #[serde(default)]
    pub last_dispatch_gap_notice_ms: i64,
    /// `clean`, `delivered` or `delivery_failed`.
    #[serde(default = "default_dispatch_gap_delivery_state")]
    pub dispatch_gap_delivery_state: String,
    pub scheduling_decision_revision: u64,
    pub first_pending_ms: i64,
    pub last_updated_ms: i64,
    /// Idle set of the last notification actually generated for the master.
    /// A repeated `WorkerIdle` signal for an already idle worker must not
    /// rebuild the batch after [`mark_idle_batch_delivered`] clears
    /// `newly_idle_workers`.
    #[serde(default)]
    pub last_notified_newly_idle: Vec<String>,
    #[serde(default)]
    pub last_notified_live_idle: Vec<String>,
    #[serde(default)]
    pub wake_hold: Option<WakeHold>,
    #[serde(default = "default_delivery_state")]
    pub delivery_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WakeHold {
    pub reason: String,
    pub held_by: String,
    pub held_ms: i64,
    pub until_ms: i64,
}

fn default_delivery_state() -> String {
    "clean".into()
}

fn default_dispatch_gap_delivery_state() -> String {
    "clean".into()
}

/// Replace the dispatch-gap condition projection. Returns true when the
/// materialised task or peer set changed. This is the only writer for
/// `dispatch_gap_task_ids` / `dispatch_gap_idle_peer_ids`.
pub fn record_dispatch_gap_condition(
    accumulator: &mut MasterWakeAccumulator,
    task_ids: Vec<String>,
    idle_peer_ids: Vec<String>,
) -> bool {
    let mut tasks = task_ids;
    tasks.sort();
    tasks.dedup();
    let mut peers = idle_peer_ids;
    peers.sort();
    peers.dedup();
    let changed = accumulator.dispatch_gap_task_ids != tasks
        || accumulator.dispatch_gap_idle_peer_ids != peers;
    accumulator.dispatch_gap_task_ids = tasks;
    accumulator.dispatch_gap_idle_peer_ids = peers;
    changed
}

/// Clear the dispatch-gap condition and idempotence key when the gap resolves.
/// Returns true when persisted state actually changed.
pub fn clear_dispatch_gap_condition(accumulator: &mut MasterWakeAccumulator) -> bool {
    let changed = !accumulator.dispatch_gap_task_ids.is_empty()
        || !accumulator.dispatch_gap_idle_peer_ids.is_empty()
        || accumulator.last_dispatch_gap_notice_key.is_some();
    if changed {
        accumulator.dispatch_gap_task_ids.clear();
        accumulator.dispatch_gap_idle_peer_ids.clear();
        accumulator.last_dispatch_gap_notice_key = None;
        accumulator.dispatch_gap_delivery_state = "clean".into();
    }
    changed
}

/// Advance the dispatch-gap idempotence window. Call only after the reminder
/// message was durably committed.
pub fn mark_dispatch_gap_notice_delivered(
    accumulator: &mut MasterWakeAccumulator,
    condition_key: String,
    delivered_ms: i64,
) {
    accumulator.last_dispatch_gap_notice_key = Some(condition_key);
    accumulator.last_dispatch_gap_notice_ms = delivered_ms;
    accumulator.dispatch_gap_delivery_state = "delivered".into();
}

/// Preserve the original error on a non-durable reminder. This must never
/// advance `last_dispatch_gap_notice_key` or `last_dispatch_gap_notice_ms`.
pub fn mark_dispatch_gap_delivery_failed(accumulator: &mut MasterWakeAccumulator) {
    accumulator.dispatch_gap_delivery_state = "delivery_failed".into();
}

fn add_unique(items: &mut Vec<String>, value: String) -> bool {
    if items.contains(&value) {
        return false;
    }
    items.push(value);
    items.sort();
    true
}

pub fn accumulate_master_wake(
    accumulator: &mut MasterWakeAccumulator,
    signal: &MasterWakeSignal,
    created_ms: i64,
) {
    let changed = match signal {
        MasterWakeSignal::GoalDue { revision } => {
            let changed = accumulator
                .active_goal_revision
                .is_none_or(|current| *revision > current);
            if changed {
                accumulator.goal_due = true;
                accumulator.active_goal_revision = Some(*revision);
            }
            changed
        }
        MasterWakeSignal::WorkerIdle { worker_id } => {
            let already_idle = accumulator.idle_workers.contains(worker_id);
            let added = add_unique(&mut accumulator.idle_workers, worker_id.clone());
            // A worker already in the live idle set is not a new idle edge.
            // Rebuilding `newly_idle_workers` here would re-notify the same
            // idle set after the previous batch was delivered.
            let newly_added = if already_idle {
                false
            } else {
                add_unique(&mut accumulator.newly_idle_workers, worker_id.clone())
            };
            let recovered = accumulator
                .unresponsive_workers
                .iter()
                .position(|id| id == worker_id)
                .map(|index| accumulator.unresponsive_workers.remove(index))
                .is_some();
            added || newly_added || recovered
        }
        MasterWakeSignal::MasterIdle { worker_id } => {
            // The master is the consumer, not idle worker capacity. Its idle
            // edge flushes the accumulated batch instead of joining it.
            accumulator
                .unresponsive_workers
                .iter()
                .position(|id| id == worker_id)
                .map(|index| accumulator.unresponsive_workers.remove(index))
                .is_some()
                || (accumulator.generation == 0 && accumulator.last_updated_ms == 0)
        }
        MasterWakeSignal::WorkerUnresponsive { worker_id } => {
            accumulator.idle_workers.retain(|id| id != worker_id);
            accumulator.newly_idle_workers.retain(|id| id != worker_id);
            add_unique(&mut accumulator.unresponsive_workers, worker_id.clone())
        }
        MasterWakeSignal::WorkerRecovered { worker_id }
        | MasterWakeSignal::WorkerWorking { worker_id } => {
            let idle_removed = accumulator
                .idle_workers
                .iter()
                .position(|id| id == worker_id);
            if let Some(index) = idle_removed {
                accumulator.idle_workers.remove(index);
            }
            accumulator.newly_idle_workers.retain(|id| id != worker_id);
            let unresponsive_removed = accumulator
                .unresponsive_workers
                .iter()
                .position(|id| id == worker_id);
            if let Some(index) = unresponsive_removed {
                accumulator.unresponsive_workers.remove(index);
            }
            idle_removed.is_some() || unresponsive_removed.is_some()
        }
        MasterWakeSignal::TaskBlocked { task_id } => {
            add_unique(&mut accumulator.blocked_or_timed_out_tasks, task_id.clone())
        }
        MasterWakeSignal::TaskFreed { task_id } => {
            add_unique(&mut accumulator.completed_or_freed_tasks, task_id.clone())
        }
        MasterWakeSignal::SubagentStatus { subagent_id } => {
            let id = format!("subagent:{subagent_id}");
            let added = add_unique(&mut accumulator.idle_workers, id.clone());
            let newly_added = add_unique(&mut accumulator.newly_idle_workers, id);
            added || newly_added
        }
        MasterWakeSignal::SubagentWorking { subagent_id } => {
            let id = format!("subagent:{subagent_id}");
            let idle_removed = accumulator
                .idle_workers
                .iter()
                .position(|existing| existing == &id)
                .map(|index| accumulator.idle_workers.remove(index))
                .is_some();
            let newly_removed = accumulator
                .newly_idle_workers
                .iter()
                .position(|existing| existing == &id)
                .map(|index| accumulator.newly_idle_workers.remove(index))
                .is_some();
            idle_removed || newly_removed
        }
    };
    if changed {
        if accumulator.generation == 0 {
            accumulator.generation = 1;
            accumulator.first_pending_ms = created_ms;
        }
        accumulator.last_updated_ms = created_ms;
        accumulator.delivery_state = "pending".into();
    }
}

pub fn mark_master_wake_delivered(accumulator: &mut MasterWakeAccumulator) {
    accumulator.delivery_state = "notified_unconsumed".into();
}

/// Clear the delivered goal-due latch while keeping the notified revision so a
/// same-revision replay does not raise it again.
pub fn clear_delivered_master_goal_due(accumulator: &mut MasterWakeAccumulator) {
    accumulator.goal_due = false;
}

pub fn clear_master_goal_due(accumulator: &mut MasterWakeAccumulator) {
    accumulator.goal_due = false;
    accumulator.active_goal_revision = None;
}

pub fn mark_master_wake_delivery_failed(accumulator: &mut MasterWakeAccumulator) {
    accumulator.delivery_state = "delivery_failed".into();
}

pub fn mark_master_wake_skipped_busy(accumulator: &mut MasterWakeAccumulator) {
    accumulator.delivery_state = "skipped-busy".into();
}

pub fn mark_idle_batch_delivered(accumulator: &mut MasterWakeAccumulator) {
    accumulator.last_notified_newly_idle = accumulator.newly_idle_workers.clone();
    accumulator.last_notified_live_idle = accumulator.idle_workers.clone();
    accumulator.newly_idle_workers.clear();
}

/// A message belongs to the master idle-notification contract when it is a
/// typed `notification` or an idle batch subject. These share one idempotency
/// and delivery-failure lifecycle, so unrelated `notify` traffic such as
/// `reply-pending` stays outside.
pub fn is_master_wake_notice(mtype: &str, subject: Option<&str>) -> bool {
    mtype == "notification" || is_idle_batch_notice(mtype, subject)
}

/// Idle batch notices are selected purely by batch subject, regardless of the
/// carrying mtype. This keeps unrelated typed `notification` traffic such as
/// `goal:plan.md` out of idle-batch idempotency while preserving typed notice
/// gating in state_impl.
pub fn is_idle_batch_notice(_mtype: &str, subject: Option<&str>) -> bool {
    matches!(
        subject,
        Some("worker-idle" | "master-wake-batch" | "subagent-status")
    )
}

pub fn retain_live_idle_workers(
    accumulator: &mut MasterWakeAccumulator,
    live_workers: &BTreeSet<String>,
    live_subagents: &BTreeSet<String>,
) -> bool {
    let before_idle = accumulator.idle_workers.clone();
    let before_newly = accumulator.newly_idle_workers.clone();
    accumulator.idle_workers.retain(|id| {
        if let Some(subagent_id) = id.strip_prefix("subagent:") {
            live_subagents.contains(subagent_id)
        } else {
            live_workers.contains(id)
        }
    });
    accumulator.newly_idle_workers.retain(|id| {
        if let Some(subagent_id) = id.strip_prefix("subagent:") {
            live_subagents.contains(subagent_id)
        } else {
            live_workers.contains(id)
        }
    });
    let changed =
        accumulator.idle_workers != before_idle || accumulator.newly_idle_workers != before_newly;
    if changed {
        accumulator.generation = accumulator.generation.saturating_add(1);
        accumulator.delivery_state = "pending".into();
    }
    changed
}
