//! Pure policy for the board idle patrol.
//!
//! The authenticated runtime caller supplies every fact used here: normalized
//! unclosed board task IDs it owns, its stable owner key, per-task cancellation
//! records, the typed host activity facts, the current instant and the previous
//! gate. This module has no clock, no I/O, no `State` access and no board or
//! session enumeration. Status enumeration, delivery, pending-notice handling
//! and persistence of the gate belong to the runtime owner.
//!
//! Two triggers share one gate: a genuine realtime event may wake independently
//! of the periodic deadline, while the periodic poll only wakes when its own
//! deadline has arrived.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Periodic fallback interval for the board idle patrol.
pub const BOARD_IDLE_PATROL_INTERVAL_MS: i64 = 900_000;

/// Error returned when a cancellation request names a task the caller does not own.
pub const BOARD_PATROL_TASK_OWNER_REQUIRED: &str = "BOARD_PATROL_TASK_OWNER_REQUIRED";

/// Which trigger produced one evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatrolTriggerMode {
    /// A genuine host or connection event, or a newly claimed task while idle.
    RealtimeEvent,
    /// The 15-minute fallback poll.
    PeriodicPoll,
}

/// Per-task patrol cancellation, bound to the stable owner that cancelled it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPatrolCancelRecord {
    pub task_id: String,
    /// Stable owner key, for example a stable name rather than a session carrier.
    pub owner_key: String,
    pub reason: Option<String>,
    pub resume_condition: Option<String>,
    pub cancelled_ms: i64,
}

/// Prior patrol episode and the next periodic deadline for one owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerBoardPatrolGate {
    pub owner_key: String,
    pub last_wake_ms: i64,
    pub last_realtime_idle_since_ms: i64,
    pub last_wake_tasks: Vec<String>,
    pub next_due_ms: i64,
}

/// Result of one patrol evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatrolDecision {
    /// The owner holds no unclosed, uncancelled task.
    NoEligibleTasks,
    /// Nothing new since the last wake, or the periodic deadline is not reached.
    NotDueYet,
    /// Activity is not positively idle, so no wake is allowed.
    SkipUnknown { gate: PeerBoardPatrolGate },
    /// The host is busy, so this round is skipped without backfill.
    SkipBusy { gate: PeerBoardPatrolGate },
    /// Wake the owner for exactly these tasks.
    WakeDue {
        tasks: Vec<String>,
        gate: PeerBoardPatrolGate,
    },
}

/// Whether a board status is still open for patrol purposes.
///
/// The caller owns the canonical status enumeration; this predicate only
/// recognizes the two closed forms.
pub fn is_unclosed_board_status(status: &str) -> bool {
    !matches!(status, "closed" | "cancelled")
}

/// Task IDs eligible for patrol after applying owner-scoped cancellation.
///
/// A cancellation record suppresses a task only when its `owner_key` equals the
/// current owner. A task transferred to another owner therefore ignores the old
/// owner's record, while the same stable owner keeps its cancellation across a
/// new session carrier. Input order is preserved.
pub fn eligible_patrol_task_ids(
    unclosed_owned_task_ids: &[String],
    current_owner_key: &str,
    cancelled: &BTreeMap<String, TaskPatrolCancelRecord>,
) -> Vec<String> {
    unclosed_owned_task_ids
        .iter()
        .filter(|task_id| {
            !cancelled
                .get(*task_id)
                .is_some_and(|record| record.owner_key == current_owner_key)
        })
        .cloned()
        .collect()
}

/// Record a cancellation for every requested task owned by `owner_key`.
///
/// An empty `requested_ids` cancels all owned unclosed tasks. All requested IDs
/// are validated before any record is written, so an unowned ID leaves the map
/// unchanged. Returns the cancelled task IDs in request order.
pub fn apply_cancel(
    cancelled: &mut BTreeMap<String, TaskPatrolCancelRecord>,
    owned_unclosed_ids: &[String],
    requested_ids: &[String],
    owner_key: &str,
    reason: Option<String>,
    resume_condition: Option<String>,
    now_ms: i64,
) -> Result<Vec<String>, &'static str> {
    let targets: Vec<String> = if requested_ids.is_empty() {
        owned_unclosed_ids.to_vec()
    } else {
        requested_ids.to_vec()
    };

    let all_owned = targets
        .iter()
        .all(|task_id| owned_unclosed_ids.iter().any(|owned| owned == task_id));
    if !all_owned {
        return Err(BOARD_PATROL_TASK_OWNER_REQUIRED);
    }

    for task_id in &targets {
        cancelled.insert(
            task_id.clone(),
            TaskPatrolCancelRecord {
                task_id: task_id.clone(),
                owner_key: owner_key.to_string(),
                reason: reason.clone(),
                resume_condition: resume_condition.clone(),
                cancelled_ms: now_ms,
            },
        );
    }
    Ok(targets)
}

/// Remove only this owner's cancellation records for the requested tasks.
///
/// Records written by another owner are never removed. Returns the task IDs
/// whose records were actually removed, in request order.
pub fn apply_rearm(
    cancelled: &mut BTreeMap<String, TaskPatrolCancelRecord>,
    requested_ids: &[String],
    owner_key: &str,
) -> Vec<String> {
    let mut restored = Vec::new();
    for task_id in requested_ids {
        let owns_record = cancelled
            .get(task_id)
            .is_some_and(|record| record.owner_key == owner_key);
        if owns_record {
            cancelled.remove(task_id);
            restored.push(task_id.clone());
        }
    }
    restored
}

/// Evaluate one patrol activation.
///
/// `is_positively_idle` must come from a current host fact. `is_busy` is only
/// consulted for a positively known busy host; waiting, offline and unknown
/// activity are all represented as `is_positively_idle == false` and never wake.
pub fn evaluate_board_patrol(
    eligible_tasks: &[String],
    owner_key: &str,
    mode: PatrolTriggerMode,
    is_positively_idle: bool,
    is_busy: bool,
    idle_since_ms: i64,
    now_ms: i64,
    prev_gate: Option<&PeerBoardPatrolGate>,
) -> PatrolDecision {
    if eligible_tasks.is_empty() {
        return PatrolDecision::NoEligibleTasks;
    }

    let mut gate = match prev_gate {
        Some(gate) if gate.owner_key == owner_key => gate.clone(),
        _ => PeerBoardPatrolGate {
            owner_key: owner_key.to_string(),
            last_wake_ms: 0,
            // No prior episode yet; any realtime idle episode is new.
            last_realtime_idle_since_ms: i64::MIN,
            last_wake_tasks: Vec::new(),
            next_due_ms: now_ms,
        },
    };

    match mode {
        PatrolTriggerMode::PeriodicPoll => {
            if now_ms < gate.next_due_ms {
                return PatrolDecision::NotDueYet;
            }
            if is_busy {
                gate.next_due_ms = now_ms + BOARD_IDLE_PATROL_INTERVAL_MS;
                return PatrolDecision::SkipBusy { gate };
            }
            if !is_positively_idle {
                // Advance the poll window so an unknown host is not probed on
                // every tick, and the deadline cannot stay stale forever.
                gate.next_due_ms = now_ms + BOARD_IDLE_PATROL_INTERVAL_MS;
                return PatrolDecision::SkipUnknown { gate };
            }
            gate.last_wake_ms = now_ms;
            gate.last_realtime_idle_since_ms = idle_since_ms;
            gate.last_wake_tasks = eligible_tasks.to_vec();
            gate.next_due_ms = now_ms + BOARD_IDLE_PATROL_INTERVAL_MS;
            PatrolDecision::WakeDue {
                tasks: eligible_tasks.to_vec(),
                gate,
            }
        }
        PatrolTriggerMode::RealtimeEvent => {
            if is_busy {
                return PatrolDecision::SkipBusy { gate };
            }
            if !is_positively_idle {
                return PatrolDecision::SkipUnknown { gate };
            }
            let is_new_episode = idle_since_ms > gate.last_realtime_idle_since_ms;
            let has_new_task = eligible_tasks
                .iter()
                .any(|task_id| !gate.last_wake_tasks.contains(task_id));
            if !is_new_episode && !has_new_task {
                return PatrolDecision::NotDueYet;
            }
            gate.last_wake_ms = now_ms;
            gate.last_realtime_idle_since_ms = idle_since_ms;
            gate.last_wake_tasks = eligible_tasks.to_vec();
            gate.next_due_ms = now_ms + BOARD_IDLE_PATROL_INTERVAL_MS;
            PatrolDecision::WakeDue {
                tasks: eligible_tasks.to_vec(),
                gate,
            }
        }
    }
}
