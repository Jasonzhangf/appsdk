//! Independent public consumer of the pure board patrol policy.
//!
//! The source module is included directly so this contract compiles before any
//! binary module wiring exists. All facts are constructed by the test; no
//! clock, `State`, board file or session is read.

#[path = "../src/board_patrol_policy.rs"]
mod board_patrol_policy;

use std::collections::BTreeMap;

use serde_json::json;

use board_patrol_policy::{
    apply_cancel, apply_rearm, eligible_patrol_task_ids, evaluate_board_patrol,
    is_unclosed_board_status, PatrolDecision, PatrolTriggerMode, PeerBoardPatrolGate,
    TaskPatrolCancelRecord, BOARD_IDLE_PATROL_INTERVAL_MS, BOARD_PATROL_TASK_OWNER_REQUIRED,
};

fn id(value: &str) -> String {
    value.to_string()
}

fn record(task_id: &str, owner_key: &str, cancelled_ms: i64) -> TaskPatrolCancelRecord {
    TaskPatrolCancelRecord {
        task_id: task_id.to_string(),
        owner_key: owner_key.to_string(),
        reason: None,
        resume_condition: None,
        cancelled_ms,
    }
}

fn gate(owner_key: &str, next_due_ms: i64) -> PeerBoardPatrolGate {
    PeerBoardPatrolGate {
        owner_key: owner_key.to_string(),
        last_wake_ms: 0,
        last_realtime_idle_since_ms: 100,
        last_wake_tasks: Vec::new(),
        next_due_ms,
        idle_episode_start_ms: None,
        last_poll_idle: false,
    }
}

fn assert_wake(decision: &PatrolDecision, expected_tasks: &[String], expected_next_due_ms: i64) {
    let PatrolDecision::WakeDue { tasks, gate } = decision else {
        panic!("expected WakeDue, got {decision:?}");
    };
    assert_eq!(tasks, expected_tasks);
    assert_eq!(gate.next_due_ms, expected_next_due_ms);
    assert_eq!(gate.last_wake_tasks, expected_tasks);
}

fn assert_no_wake(decision: &PatrolDecision) {
    assert!(
        !matches!(decision, PatrolDecision::WakeDue { .. }),
        "expected no wake, got {decision:?}"
    );
}

const REMINDER_INTERVAL_MS: i64 = 1_800_000;

#[test]
fn board_idle_requires_trusted_30m_and_30m_between_pushes() {
    let tasks = vec![id("T1")];
    let idle_since = 100_000;
    let open_gate = gate("jenny", 0);

    for now in [idle_since + 1_799_999, idle_since + REMINDER_INTERVAL_MS] {
        let decision = evaluate_board_patrol(
            &tasks,
            "jenny",
            PatrolTriggerMode::PeriodicPoll,
            true,
            false,
            idle_since,
            now,
            Some(&open_gate),
        );
        assert_no_wake(&decision);
    }

    let first = evaluate_board_patrol(
        &tasks,
        "jenny",
        PatrolTriggerMode::PeriodicPoll,
        true,
        false,
        idle_since,
        idle_since + REMINDER_INTERVAL_MS + 1,
        Some(&open_gate),
    );
    let PatrolDecision::WakeDue {
        gate: pushed_gate, ..
    } = &first
    else {
        panic!("30m00s+1ms must be the first eligible push: {first:?}");
    };

    let just_before_next = evaluate_board_patrol(
        &tasks,
        "jenny",
        PatrolTriggerMode::PeriodicPoll,
        true,
        false,
        idle_since,
        idle_since + REMINDER_INTERVAL_MS + 1 + REMINDER_INTERVAL_MS - 1,
        Some(pushed_gate),
    );
    assert_no_wake(&just_before_next);

    let next = evaluate_board_patrol(
        &tasks,
        "jenny",
        PatrolTriggerMode::PeriodicPoll,
        true,
        false,
        idle_since,
        idle_since + REMINDER_INTERVAL_MS + 1 + REMINDER_INTERVAL_MS,
        Some(pushed_gate),
    );
    assert_wake(
        &next,
        &tasks,
        idle_since
            + REMINDER_INTERVAL_MS
            + 1
            + REMINDER_INTERVAL_MS
            + BOARD_IDLE_PATROL_INTERVAL_MS,
    );
}

#[test]
fn closed_and_cancelled_statuses_are_ineligible() {
    assert!(!is_unclosed_board_status("closed"));
    assert!(!is_unclosed_board_status("cancelled"));
    assert!(is_unclosed_board_status("open"));
    assert!(is_unclosed_board_status("in_progress"));
    assert!(is_unclosed_board_status("waiting_user"));
}

#[test]
fn cancellation_filters_only_same_owner() {
    let tasks = vec![id("T1"), id("T2"), id("T3")];
    let mut cancelled = BTreeMap::new();
    cancelled.insert(id("T1"), record("T1", "jenny", 1));
    cancelled.insert(id("T2"), record("T2", "other", 1));

    let eligible = eligible_patrol_task_ids(&tasks, "jenny", &cancelled);
    assert_eq!(eligible, vec![id("T2"), id("T3")]);
}

#[test]
fn same_stable_owner_keeps_cancellation_across_new_carrier() {
    let tasks = vec![id("T1")];
    let mut cancelled = BTreeMap::new();
    cancelled.insert(id("T1"), record("T1", "jenny", 10));

    // A new session carrier still uses the same stable owner key, so the
    // cancellation survives.
    let eligible = eligible_patrol_task_ids(&tasks, "jenny", &cancelled);
    assert!(eligible.is_empty());
}

#[test]
fn different_owner_ignores_old_cancellation() {
    let tasks = vec![id("T1")];
    let mut cancelled = BTreeMap::new();
    cancelled.insert(id("T1"), record("T1", "jenny", 10));

    let eligible = eligible_patrol_task_ids(&tasks, "other", &cancelled);
    assert_eq!(eligible, vec![id("T1")]);
}

#[test]
fn cancelled_t1_does_not_hide_new_t2() {
    let tasks = vec![id("T1"), id("T2")];
    let mut cancelled = BTreeMap::new();
    cancelled.insert(id("T1"), record("T1", "jenny", 10));

    let eligible = eligible_patrol_task_ids(&tasks, "jenny", &cancelled);
    assert_eq!(eligible, vec![id("T2")]);
}

#[test]
fn cancel_rejects_unowned_id_atomically() {
    let owned = vec![id("T1")];
    let mut cancelled = BTreeMap::new();
    cancelled.insert(id("T1"), record("T1", "jenny", 10));

    let result = apply_cancel(
        &mut cancelled,
        &owned,
        &[id("T1"), id("T2")],
        "jenny",
        None,
        None,
        20,
    );

    assert_eq!(result, Err(BOARD_PATROL_TASK_OWNER_REQUIRED));
    assert_eq!(
        cancelled.get("T1"),
        Some(&record("T1", "jenny", 10)),
        "existing record must stay unchanged"
    );
    assert!(
        !cancelled.contains_key("T2"),
        "unowned pending id must not be written"
    );
}

#[test]
fn cancel_without_request_cancels_all_owned() {
    let owned = vec![id("T1"), id("T2")];
    let mut cancelled = BTreeMap::new();

    let result = apply_cancel(&mut cancelled, &owned, &[], "jenny", None, None, 5);

    assert_eq!(result, Ok(vec![id("T1"), id("T2")]));
    assert_eq!(cancelled.len(), 2);
    assert!(cancelled.get("T1").is_some_and(|r| r.owner_key == "jenny"));
    assert!(cancelled.get("T2").is_some_and(|r| r.owner_key == "jenny"));
}

#[test]
fn rearm_removes_only_callers_records() {
    let mut cancelled = BTreeMap::new();
    cancelled.insert(id("T1"), record("T1", "jenny", 1));
    cancelled.insert(id("T2"), record("T2", "other", 1));

    let restored = apply_rearm(&mut cancelled, &[id("T1"), id("T2")], "jenny");
    assert_eq!(restored, vec![id("T1")]);
    assert!(!cancelled.contains_key("T1"));
    assert!(
        cancelled.get("T2").is_some_and(|r| r.owner_key == "other"),
        "another owner's record survives"
    );
}

#[test]
fn periodic_wakes_on_positive_idle_at_due() {
    let now = 2_000_000;
    let decision = evaluate_board_patrol(
        &[id("T1")],
        "jenny",
        PatrolTriggerMode::PeriodicPoll,
        true,
        false,
        now - REMINDER_INTERVAL_MS - 1,
        now,
        None,
    );

    assert_wake(&decision, &[id("T1")], now + BOARD_IDLE_PATROL_INTERVAL_MS);
}

#[test]
fn periodic_busy_skip_advances_deadline_and_never_wakes_early() {
    let due = evaluate_board_patrol(
        &[id("T1")],
        "jenny",
        PatrolTriggerMode::PeriodicPoll,
        true,
        true,
        100,
        1_000,
        None,
    );
    let PatrolDecision::SkipBusy { gate } = due else {
        panic!("expected SkipBusy, got {due:?}");
    };
    assert_eq!(gate.next_due_ms, 1_000 + BOARD_IDLE_PATROL_INTERVAL_MS);

    // A later tick sees a fresh idle episode, but the periodic mode must not
    // backfill the skipped busy round before its advanced deadline.
    let early = evaluate_board_patrol(
        &[id("T1")],
        "jenny",
        PatrolTriggerMode::PeriodicPoll,
        true,
        false,
        50_000,
        1_000 + 60_000,
        Some(&gate),
    );
    assert!(matches!(early, PatrolDecision::NotDueYet { .. }));
}

#[test]
fn periodic_due_wakes_again_with_unchanged_idle_since() {
    let now = 2_000_000;
    let idle_since = now - REMINDER_INTERVAL_MS - 1;
    let due = evaluate_board_patrol(
        &[id("T1")],
        "jenny",
        PatrolTriggerMode::PeriodicPoll,
        true,
        false,
        idle_since,
        now,
        None,
    );
    let PatrolDecision::WakeDue { gate, .. } = due else {
        panic!("expected WakeDue, got {due:?}");
    };

    // The next periodic deadline is only 15 minutes later, which must not
    // produce a second push inside the 30-minute interval.
    let early = evaluate_board_patrol(
        &[id("T1")],
        "jenny",
        PatrolTriggerMode::PeriodicPoll,
        true,
        false,
        idle_since,
        gate.next_due_ms,
        Some(&gate),
    );
    assert!(matches!(early, PatrolDecision::NotDueYet { .. }));

    // A full 30 minutes after the last committed push may push again.
    let again_at = gate.last_wake_ms + REMINDER_INTERVAL_MS;
    let again = evaluate_board_patrol(
        &[id("T1")],
        "jenny",
        PatrolTriggerMode::PeriodicPoll,
        true,
        false,
        idle_since,
        again_at,
        Some(&gate),
    );
    assert_wake(
        &again,
        &[id("T1")],
        again_at + BOARD_IDLE_PATROL_INTERVAL_MS,
    );
}

#[test]
fn periodic_unknown_advances_poll_window() {
    let unknown = evaluate_board_patrol(
        &[id("T1")],
        "jenny",
        PatrolTriggerMode::PeriodicPoll,
        false,
        false,
        100,
        1_000,
        None,
    );
    let PatrolDecision::SkipUnknown { gate } = unknown else {
        panic!("expected SkipUnknown, got {unknown:?}");
    };
    assert_eq!(gate.next_due_ms, 1_000 + BOARD_IDLE_PATROL_INTERVAL_MS);

    let next_tick = evaluate_board_patrol(
        &[id("T1")],
        "jenny",
        PatrolTriggerMode::PeriodicPoll,
        false,
        false,
        100,
        1_001,
        Some(&gate),
    );
    assert!(matches!(next_tick, PatrolDecision::NotDueYet { .. }));
}

#[test]
fn periodic_unknown_waiting_and_offline_never_wake() {
    for (is_idle, is_busy, label) in [(false, false, "waiting_or_offline"), (false, true, "busy")] {
        let decision = evaluate_board_patrol(
            &[id("T1")],
            "jenny",
            PatrolTriggerMode::PeriodicPoll,
            is_idle,
            is_busy,
            100,
            1_000,
            None,
        );
        assert_no_wake(&decision);
        assert!(
            matches!(
                decision,
                PatrolDecision::SkipUnknown { .. } | PatrolDecision::SkipBusy { .. }
            ),
            "{label} must not wake, got {decision:?}"
        );
    }
}

#[test]
fn periodic_different_owner_discards_old_gate_identity() {
    let now = 2_000_000;
    let old_gate = gate("other", 1_000 + BOARD_IDLE_PATROL_INTERVAL_MS);
    let decision = evaluate_board_patrol(
        &[id("T1")],
        "jenny",
        PatrolTriggerMode::PeriodicPoll,
        true,
        false,
        now - REMINDER_INTERVAL_MS - 1,
        now,
        Some(&old_gate),
    );
    assert_wake(&decision, &[id("T1")], now + BOARD_IDLE_PATROL_INTERVAL_MS);
}

#[test]
fn realtime_wakes_independently_of_periodic_deadline() {
    // The periodic deadline is far away, but a genuine realtime event with a
    // trusted idle episode older than 30 minutes still wakes.
    let now = 2_000_000;
    let old_gate = gate("jenny", 1_000 + BOARD_IDLE_PATROL_INTERVAL_MS);
    let decision = evaluate_board_patrol(
        &[id("T1")],
        "jenny",
        PatrolTriggerMode::RealtimeEvent,
        true,
        false,
        now - REMINDER_INTERVAL_MS - 1,
        now,
        Some(&old_gate),
    );
    assert_wake(&decision, &[id("T1")], now + BOARD_IDLE_PATROL_INTERVAL_MS);
}

#[test]
fn realtime_duplicate_same_idle_and_tasks_coalesces() {
    let now = 2_000_000;
    let idle_since = now - REMINDER_INTERVAL_MS - 1;
    let first = evaluate_board_patrol(
        &[id("T1")],
        "jenny",
        PatrolTriggerMode::RealtimeEvent,
        true,
        false,
        idle_since,
        now,
        None,
    );
    let PatrolDecision::WakeDue { gate, .. } = first else {
        panic!("expected WakeDue, got {first:?}");
    };

    let duplicate = evaluate_board_patrol(
        &[id("T1")],
        "jenny",
        PatrolTriggerMode::RealtimeEvent,
        true,
        false,
        idle_since,
        now + 1,
        Some(&gate),
    );
    assert!(matches!(duplicate, PatrolDecision::NotDueYet { .. }));
}

#[test]
fn realtime_new_task_on_positive_idle_wakes() {
    let now = 2_000_000;
    let idle_since = now - REMINDER_INTERVAL_MS - 1;
    let first = evaluate_board_patrol(
        &[id("T1")],
        "jenny",
        PatrolTriggerMode::RealtimeEvent,
        true,
        false,
        idle_since,
        now,
        None,
    );
    let PatrolDecision::WakeDue { gate, .. } = first else {
        panic!("expected WakeDue, got {first:?}");
    };

    // A new task does not bypass the 30-minute interval after the last push.
    let immediate = evaluate_board_patrol(
        &[id("T1"), id("T2")],
        "jenny",
        PatrolTriggerMode::RealtimeEvent,
        true,
        false,
        idle_since,
        now + 1,
        Some(&gate),
    );
    assert!(matches!(immediate, PatrolDecision::NotDueYet { .. }));

    // Once the interval elapses the new task becomes eligible.
    let due_at = gate.last_wake_ms + REMINDER_INTERVAL_MS;
    let new_task = evaluate_board_patrol(
        &[id("T1"), id("T2")],
        "jenny",
        PatrolTriggerMode::RealtimeEvent,
        true,
        false,
        idle_since,
        due_at,
        Some(&gate),
    );
    assert_wake(
        &new_task,
        &[id("T1"), id("T2")],
        due_at + BOARD_IDLE_PATROL_INTERVAL_MS,
    );
}

#[test]
fn realtime_busy_and_unknown_never_wake() {
    for (is_idle, is_busy, label) in [(false, false, "waiting_or_offline"), (false, true, "busy")] {
        let decision = evaluate_board_patrol(
            &[id("T1")],
            "jenny",
            PatrolTriggerMode::RealtimeEvent,
            is_idle,
            is_busy,
            100,
            1_000,
            None,
        );
        assert_no_wake(&decision);
        assert!(
            matches!(
                decision,
                PatrolDecision::SkipUnknown { .. } | PatrolDecision::SkipBusy { .. }
            ),
            "{label} must not wake, got {decision:?}"
        );
    }
}

#[test]
fn realtime_different_owner_discards_old_gate() {
    let now = 2_000_000;
    let old_gate = gate("other", 1_000 + BOARD_IDLE_PATROL_INTERVAL_MS);
    let decision = evaluate_board_patrol(
        &[id("T1")],
        "jenny",
        PatrolTriggerMode::RealtimeEvent,
        true,
        false,
        now - REMINDER_INTERVAL_MS - 1,
        now,
        Some(&old_gate),
    );
    assert_wake(&decision, &[id("T1")], now + BOARD_IDLE_PATROL_INTERVAL_MS);
}

#[test]
fn no_eligible_tasks_never_wakes() {
    let decision = evaluate_board_patrol(
        &[],
        "jenny",
        PatrolTriggerMode::RealtimeEvent,
        true,
        false,
        100,
        1_000,
        None,
    );
    assert_eq!(decision, PatrolDecision::NoEligibleTasks);
}

#[test]
fn serde_roundtrips_records_gate_and_decisions() {
    let cancelled = BTreeMap::from([
        (id("T1"), record("T1", "jenny", 10)),
        (id("T2"), record("T2", "other", 11)),
    ]);

    let json_cancelled = serde_json::to_value(&cancelled).expect("serialize cancelled");
    let decoded: BTreeMap<String, TaskPatrolCancelRecord> =
        serde_json::from_value(json_cancelled).expect("deserialize cancelled");
    assert_eq!(decoded, cancelled);

    let decoded_record: TaskPatrolCancelRecord = serde_json::from_value(json!({
        "task_id": "T1",
        "owner_key": "jenny",
        "reason": null,
        "resume_condition": "after_user_ok",
        "cancelled_ms": 10,
    }))
    .expect("deserialize record");
    assert_eq!(
        decoded_record.resume_condition.as_deref(),
        Some("after_user_ok")
    );

    let value = json!({
        "owner_key": "jenny",
        "last_wake_ms": 10,
        "last_realtime_idle_since_ms": 100,
        "last_wake_tasks": ["T1"],
        "next_due_ms": 20,
    });
    let decoded_gate: PeerBoardPatrolGate =
        serde_json::from_value(value).expect("deserialize gate");
    assert_eq!(decoded_gate.last_wake_tasks, vec![id("T1")]);

    let json_decision = serde_json::to_value(PatrolDecision::NotDueYet {
        gate: decoded_gate.clone(),
    })
    .expect("serialize decision");
    let decoded_decision: PatrolDecision =
        serde_json::from_value(json_decision).expect("deserialize decision");
    assert_eq!(
        decoded_decision,
        PatrolDecision::NotDueYet { gate: decoded_gate }
    );
}
