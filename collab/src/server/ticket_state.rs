//! Ticket phase model and pure queue-selection helpers.
//!
//! The queue projection is derived from phase records; this module never owns a
//! second writable queue. Slot keys are the stable identity names.

use serde::{Deserialize, Serialize};

/// Which lifecycle phase one phase record belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketPhaseKind {
    Plan,
    PlanReview,
    Execute,
    FinalReview,
}

/// Where a phase is in its slot lifecycle. Waiting dependencies and resources
/// stay `Queued` in their slot; only assignment absence is `WaitingAssignment`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketPhaseStatus {
    WaitingAssignment,
    Queued,
    Claimed,
    Running,
    Completed,
    Rework,
    Revoked,
}

/// Submitted conclusion of one phase, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketPhaseResult {
    pub outcome: String, // "pass" | "fail"
    pub subject_revision: u64,
    pub submitted_by: String,
    pub submitted_ms: i64,
    #[serde(default)]
    pub evidence_path: Option<String>,
}

/// One durable phase batch of a ticket. Keyed by (task_id, phase, batch).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketPhaseRecord {
    pub task_id: String,
    pub phase: TicketPhaseKind,
    pub batch: u64,
    pub priority: String,
    pub status: TicketPhaseStatus,
    /// Stable slot name (identity name) currently holding this phase, if any.
    #[serde(default)]
    pub slot_name: Option<String>,
    /// Identity-control generation observed when this attempt claimed the slot.
    /// A later binding/session handoff makes the old attempt stale.
    #[serde(default)]
    pub binding_generation: u64,
    /// Monotonic enqueue order; FIFO tie-break within one priority.
    #[serde(default)]
    pub enqueue_seq: u64,
    /// Increments on rework/reclaim; old attempts must be rejected.
    #[serde(default)]
    pub attempt_generation: u64,
    /// Dependency task IDs that block execution, not enqueue.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Review route group selected for a plan/final review phase.
    #[serde(default)]
    pub route_group: Option<String>,
    #[serde(default)]
    pub wait_reason: Option<String>,
    #[serde(default)]
    pub result: Option<TicketPhaseResult>,
}

/// One eligible slot candidate after scope/role filtering. Names are stable
/// slot keys; candidates are supplied in stable key order by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketCandidate {
    pub slot_name: String,
    pub worker_id: String,
    pub matched_preferred_role: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewRouteGroup {
    Reviewer,
    ArchitectExpert,
    IndependentPeer,
}

/// One eligible review receiver after role, project and participation checks.
/// The caller supplies stable name order. `participated` means the stable name
/// or its current actor participated in the subject, so the route must exclude
/// it even after a session or role change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewCandidate {
    pub slot_name: String,
    pub worker_id: String,
    pub group: ReviewRouteGroup,
    pub participated: bool,
}

pub fn priority_rank(priority: &str) -> u8 {
    match priority {
        "p0" => 0,
        "p1" => 1,
        "p2" => 2,
        "p3" => 3,
        _ => 4,
    }
}

/// Review routing order is Reviewer, then architect/expert, then another
/// independent peer. The first group with any eligible receiver wins; busy
/// capacity never lowers the group. Within the chosen group, the shortest
/// queue wins and equal watermarks rotate.
pub fn select_review_route<'a>(
    candidates: &'a [ReviewCandidate],
    queue_len: impl Fn(&str) -> usize,
    cursor: Option<&str>,
) -> Option<&'a ReviewCandidate> {
    for group in [
        ReviewRouteGroup::Reviewer,
        ReviewRouteGroup::ArchitectExpert,
        ReviewRouteGroup::IndependentPeer,
    ] {
        let eligible: Vec<TicketCandidate> = candidates
            .iter()
            .filter(|candidate| candidate.group == group && !candidate.participated)
            .map(|candidate| TicketCandidate {
                slot_name: candidate.slot_name.clone(),
                worker_id: candidate.worker_id.clone(),
                matched_preferred_role: true,
            })
            .collect();
        let Some(selected) = select_shortest_queue(&eligible, &queue_len, cursor) else {
            continue;
        };
        return candidates.iter().find(|candidate| {
            candidate.group == group && candidate.slot_name == selected.slot_name
        });
    }
    None
}

/// Count not-yet-started phase items assigned to one slot. Claimed, running
/// and completed items are excluded because they are no longer queued.
pub fn slot_queue_len<'a, I>(phases: I, slot_name: &str) -> usize
where
    I: IntoIterator<Item = &'a TicketPhaseRecord>,
{
    phases
        .into_iter()
        .filter(|phase| {
            phase.status == TicketPhaseStatus::Queued
                && phase.slot_name.as_deref() == Some(slot_name)
        })
        .count()
}

/// Select the shortest queue among candidates. The caller passes candidates in
/// stable key order and the group's last selected slot name (rotation cursor).
/// Ties rotate to the first candidate strictly after the cursor, wrapping.
pub fn select_shortest_queue<'a>(
    candidates: &'a [TicketCandidate],
    queue_len: impl Fn(&str) -> usize,
    cursor: Option<&str>,
) -> Option<&'a TicketCandidate> {
    let mut best: Option<(&TicketCandidate, usize)> = None;
    for candidate in candidates {
        let len = queue_len(&candidate.slot_name);
        if best.is_none() || len < best.expect("best exists").1 {
            best = Some((candidate, len));
        }
    }
    let (winner, min_len) = best?;
    if candidates.len() == 1 || cursor.is_none() {
        return Some(winner);
    }
    let cursor = cursor.expect("cursor checked");
    let tied: Vec<&TicketCandidate> = candidates
        .iter()
        .filter(|c| queue_len(&c.slot_name) == min_len)
        .collect();
    if tied.len() < 2 {
        return Some(winner);
    }
    let after = tied
        .iter()
        .find(|c| c.slot_name.as_str() > cursor)
        .or_else(|| tied.first());
    after.copied()
}

/// Pick the next item a slot may start: queued on this slot, all dependencies
/// satisfied, ordered by priority then FIFO enqueue sequence. Claimed, running
/// and completed items are never re-selected.
pub fn next_queued_for_slot<'a>(
    phases: &'a [TicketPhaseRecord],
    slot_name: &str,
    dependency_satisfied: impl Fn(&str) -> bool,
) -> Option<&'a TicketPhaseRecord> {
    phases
        .iter()
        .filter(|phase| {
            phase.status == TicketPhaseStatus::Queued
                && phase.slot_name.as_deref() == Some(slot_name)
                && phase.depends_on.iter().all(|dep| dependency_satisfied(dep))
        })
        .min_by(|left, right| {
            priority_rank(&left.priority)
                .cmp(&priority_rank(&right.priority))
                .then(left.enqueue_seq.cmp(&right.enqueue_seq))
                .then(left.task_id.cmp(&right.task_id))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phase(
        task: &str,
        slot: Option<&str>,
        status: TicketPhaseStatus,
        seq: u64,
    ) -> TicketPhaseRecord {
        TicketPhaseRecord {
            task_id: task.into(),
            phase: TicketPhaseKind::Execute,
            batch: 0,
            priority: "p2".into(),
            status,
            slot_name: slot.map(Into::into),
            binding_generation: 0,
            enqueue_seq: seq,
            attempt_generation: 0,
            depends_on: vec![],
            route_group: None,
            wait_reason: None,
            result: None,
        }
    }

    #[test]
    fn queue_len_counts_only_queued_items() {
        let phases = vec![
            phase("a", Some("Alice"), TicketPhaseStatus::Queued, 0),
            phase("b", Some("Alice"), TicketPhaseStatus::Running, 1),
            phase("c", Some("Alice"), TicketPhaseStatus::Completed, 2),
            phase("d", Some("Alice"), TicketPhaseStatus::Claimed, 3),
            phase("e", Some("Alice"), TicketPhaseStatus::Queued, 4),
        ];
        assert_eq!(slot_queue_len(&phases, "Alice"), 2);
        assert_eq!(slot_queue_len(&phases, "Bob"), 0);
    }

    #[test]
    fn shortest_queue_wins_and_rotation_breaks_ties() {
        let candidates = vec![
            TicketCandidate {
                slot_name: "Alice".into(),
                worker_id: "w1".into(),
                matched_preferred_role: false,
            },
            TicketCandidate {
                slot_name: "Bob".into(),
                worker_id: "w2".into(),
                matched_preferred_role: false,
            },
            TicketCandidate {
                slot_name: "Carol".into(),
                worker_id: "w3".into(),
                matched_preferred_role: false,
            },
        ];
        let queues = |slot: &str| match slot {
            "Alice" => 3,
            "Bob" => 1,
            _ => 1,
        };
        let picked = select_shortest_queue(&candidates, queues, Some("Bob")).unwrap();
        assert_eq!(picked.slot_name, "Carol", "ties rotate past cursor");
        let wrapped = select_shortest_queue(&candidates, queues, Some("Carol")).unwrap();
        assert_eq!(wrapped.slot_name, "Bob", "wrap to first tied candidate");
    }

    #[test]
    fn empty_candidates_returns_none() {
        assert!(select_shortest_queue(&[], |_| 0, None).is_none());
    }

    #[test]
    fn review_route_uses_first_eligible_group_and_excludes_participants() {
        let candidates = vec![
            ReviewCandidate {
                slot_name: "Alice".into(),
                worker_id: "w1".into(),
                group: ReviewRouteGroup::Reviewer,
                participated: true,
            },
            ReviewCandidate {
                slot_name: "Bob".into(),
                worker_id: "w2".into(),
                group: ReviewRouteGroup::ArchitectExpert,
                participated: false,
            },
            ReviewCandidate {
                slot_name: "Carol".into(),
                worker_id: "w3".into(),
                group: ReviewRouteGroup::IndependentPeer,
                participated: false,
            },
        ];
        let selected = select_review_route(&candidates, |_| 0, None).unwrap();
        assert_eq!(selected.slot_name, "Bob");
        assert_eq!(selected.group, ReviewRouteGroup::ArchitectExpert);
    }

    #[test]
    fn busy_first_group_does_not_fall_through_to_a_later_group() {
        let candidates = vec![
            ReviewCandidate {
                slot_name: "Alice".into(),
                worker_id: "w1".into(),
                group: ReviewRouteGroup::Reviewer,
                participated: false,
            },
            ReviewCandidate {
                slot_name: "Bob".into(),
                worker_id: "w2".into(),
                group: ReviewRouteGroup::IndependentPeer,
                participated: false,
            },
        ];
        let selected = select_review_route(
            &candidates,
            |slot| if slot == "Alice" { 5 } else { 0 },
            None,
        )
        .unwrap();
        assert_eq!(selected.slot_name, "Alice");
    }

    #[test]
    fn review_route_returns_none_when_all_groups_lack_an_independent_candidate() {
        let candidates = vec![ReviewCandidate {
            slot_name: "Alice".into(),
            worker_id: "w1".into(),
            group: ReviewRouteGroup::Reviewer,
            participated: true,
        }];
        assert!(select_review_route(&candidates, |_| 0, None).is_none());
    }

    #[test]
    fn next_queued_orders_priority_then_fifo_and_skips_unsatisfied_dependencies() {
        let mut phases = vec![
            phase("p2-first", Some("Alice"), TicketPhaseStatus::Queued, 1),
            phase("p0-later", Some("Alice"), TicketPhaseStatus::Queued, 5),
            phase("p0-blocked", Some("Alice"), TicketPhaseStatus::Queued, 2),
            phase("p2-running", Some("Alice"), TicketPhaseStatus::Running, 0),
            phase("other-slot", Some("Bob"), TicketPhaseStatus::Queued, 0),
        ];
        phases[0].priority = "p2".into();
        phases[1].priority = "p0".into();
        phases[2].priority = "p0".into();
        phases[2].depends_on = vec!["gate".into()];
        let ready = |dep: &str| dep == "gate-satisfied";
        assert_eq!(
            next_queued_for_slot(&phases, "Alice", ready)
                .unwrap()
                .task_id,
            "p0-later",
            "p0 wins over p2; blocked p0 is skipped"
        );
        let open = |_: &str| true;
        assert_eq!(
            next_queued_for_slot(&phases, "Alice", open)
                .unwrap()
                .task_id,
            "p0-blocked",
            "with the dependency satisfied the earlier p0 wins FIFO"
        );
        assert_eq!(
            next_queued_for_slot(&phases, "Alice", open)
                .unwrap()
                .enqueue_seq,
            2
        );
        assert!(next_queued_for_slot(&phases, "Nobody", open).is_none());
    }
}
