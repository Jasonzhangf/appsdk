use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(from = "MasterWakeSignalRepr")]
pub enum MasterWakeSignal {
    GoalDue {
        revision: u64,
    },
    #[serde(skip_serializing)]
    LegacyIgnored,
}

#[derive(Deserialize)]
struct GoalDueRepr {
    revision: u64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum MasterWakeSignalRepr {
    GoalDue {
        #[serde(rename = "GoalDue")]
        goal_due: GoalDueRepr,
    },
    Legacy(serde::de::IgnoredAny),
}

impl From<MasterWakeSignalRepr> for MasterWakeSignal {
    fn from(repr: MasterWakeSignalRepr) -> Self {
        match repr {
            MasterWakeSignalRepr::GoalDue { goal_due } => Self::GoalDue {
                revision: goal_due.revision,
            },
            MasterWakeSignalRepr::Legacy(_) => Self::LegacyIgnored,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MasterWakeAccumulator {
    pub generation: u64,
    pub goal_due: bool,
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
    /// skip or failed delivery must not move it, and transient gap resolution
    /// preserves the monotonic cooldown timestamp.
    #[serde(default)]
    pub last_dispatch_gap_notice_ms: i64,
    /// `clean`, `delivered` or `delivery_failed`.
    #[serde(default = "default_dispatch_gap_delivery_state")]
    pub dispatch_gap_delivery_state: String,
    pub scheduling_decision_revision: u64,
    pub first_pending_ms: i64,
    pub last_updated_ms: i64,
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
        MasterWakeSignal::LegacyIgnored => false,
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

/// A message belongs to the master goal-notification lifecycle when it is a
/// typed `notification`, keeping unrelated `notify` traffic outside.
pub fn is_master_wake_notice(mtype: &str, _subject: Option<&str>) -> bool {
    mtype == "notification"
}
