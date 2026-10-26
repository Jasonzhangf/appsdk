//! Independent public consumer of the CEO report time policy.
//!
//! The source module is included directly so this contract compiles before any
//! binary module wiring exists. All instants are deterministic epoch values for
//! Asia/Shanghai (UTC+8); no host timezone or system clock is used.

#[path = "../src/ceo_reporting_policy.rs"]
mod ceo_reporting_policy;

use ceo_reporting_policy::{
    CeoReportDecision, CeoReportPolicy, CeoReportPolicyError, CeoReportSuppression,
    DEFAULT_MIN_INTERVAL_MS,
};

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 86_400_000;

// 2026-10-25 00:00:00 Beijing (2026-10-24T16:00:00Z).
const BJ_1025_0000: u64 = 1_792_857_600_000;
const BJ_1025_0800: u64 = BJ_1025_0000 + 8 * HOUR_MS;
const BJ_1026_0800: u64 = BJ_1025_0800 + DAY_MS;

fn default_policy() -> CeoReportPolicy {
    CeoReportPolicy::default()
}

fn assert_blocked(decision: CeoReportDecision, reason: CeoReportSuppression, next_ms: u64) {
    assert!(!decision.allowed, "expected blocked, got {decision:?}");
    assert_eq!(
        decision.suppression, reason,
        "unexpected suppression reason: {decision:?}"
    );
    assert_eq!(
        decision.next_eligible_ms, next_ms,
        "unexpected next eligibility: {decision:?}"
    );
}

fn assert_allowed(decision: CeoReportDecision, now_ms: u64) {
    assert_eq!(
        decision,
        CeoReportDecision {
            allowed: true,
            suppression: CeoReportSuppression::Allowed,
            next_eligible_ms: now_ms,
        }
    );
}

#[test]
fn default_policy_is_hourly_with_beijing_midnight_to_eight() {
    let policy = default_policy();
    assert_eq!(DEFAULT_MIN_INTERVAL_MS, 3_600_000);
    assert_eq!(policy.min_interval_ms(), 3_600_000);
    assert_eq!(policy.quiet_window(), (0, 480));
}

#[test]
fn two_am_is_quiet_and_defers_to_eight_am() {
    let policy = default_policy();
    let two_am = BJ_1025_0000 + 2 * HOUR_MS;

    // No prior report: only the quiet window blocks.
    assert_blocked(
        policy.evaluate(two_am, None, false),
        CeoReportSuppression::QuietHours,
        BJ_1025_0800,
    );
    // Hourly limit already satisfied: the quiet window still blocks.
    assert_blocked(
        policy.evaluate(two_am, Some(two_am - 3 * HOUR_MS), false),
        CeoReportSuppression::QuietHours,
        BJ_1025_0800,
    );
}

#[test]
fn midnight_start_is_quiet_and_eight_am_is_allowed() {
    let policy = default_policy();
    assert_blocked(
        policy.evaluate(BJ_1025_0000, None, false),
        CeoReportSuppression::QuietHours,
        BJ_1025_0800,
    );
    assert_allowed(policy.evaluate(BJ_1025_0800, None, false), BJ_1025_0800);
}

#[test]
fn fifty_nine_minutes_is_blocked_and_sixty_minutes_is_allowed() {
    let policy = default_policy();
    let last = BJ_1025_0000 + 10 * HOUR_MS;
    let due = last + DEFAULT_MIN_INTERVAL_MS;

    assert_blocked(
        policy.evaluate(due - MINUTE_MS, Some(last), false),
        CeoReportSuppression::Cooldown,
        due,
    );
    assert_allowed(policy.evaluate(due, Some(last), false), due);
}

#[test]
fn hourly_deadline_inside_quiet_window_defers_to_eight_am() {
    let policy = default_policy();
    let last = BJ_1025_0000 + 23 * HOUR_MS + 10 * MINUTE_MS;
    let now = last + 20 * MINUTE_MS;

    // Hourly deadline is 00:10 the next day, inside the quiet window.
    assert_blocked(
        policy.evaluate(now, Some(last), false),
        CeoReportSuppression::Cooldown,
        BJ_1026_0800,
    );
}

#[test]
fn urgent_report_during_quiet_still_starts_nonurgent_cooldown() {
    let policy = default_policy();
    let urgent_at = BJ_1025_0000 + 3 * HOUR_MS;
    assert_allowed(policy.evaluate(urgent_at, None, true), urgent_at);

    // Cooldown deadline 04:00 is still quiet, so the next slot is 08:00.
    let nonurgent_at = urgent_at + 30 * MINUTE_MS;
    assert_blocked(
        policy.evaluate(nonurgent_at, Some(urgent_at), false),
        CeoReportSuppression::QuietHours,
        BJ_1025_0800,
    );
    // Both limits are satisfied at 08:00.
    assert_allowed(
        policy.evaluate(BJ_1025_0800, Some(urgent_at), false),
        BJ_1025_0800,
    );
}

#[test]
fn cooldown_outlasting_quiet_window_uses_the_later_deadline() {
    let policy = default_policy();
    let last = BJ_1025_0000 + 7 * HOUR_MS + 45 * MINUTE_MS;
    let now = BJ_1025_0800 + 10 * MINUTE_MS;

    // Quiet hours ended, but the hourly limit runs until 08:45.
    assert_blocked(
        policy.evaluate(now, Some(last), false),
        CeoReportSuppression::Cooldown,
        last + DEFAULT_MIN_INTERVAL_MS,
    );
}

#[test]
fn explicit_urgent_bypasses_both_limits() {
    let policy = default_policy();
    let now = BJ_1025_0000 + 2 * HOUR_MS + 5 * MINUTE_MS;
    let last = now - 5 * MINUTE_MS;

    assert_blocked(
        policy.evaluate(now, Some(last), false),
        CeoReportSuppression::QuietHours,
        BJ_1025_0800,
    );
    assert_allowed(policy.evaluate(now, Some(last), true), now);
}

#[test]
fn body_text_never_influences_the_decision() {
    let policy = default_policy();
    let now = BJ_1025_0000 + 2 * HOUR_MS;
    let subject = "P1 urgent: deploy broken";
    let body = "urgent urgent ack heartbeat P1";
    assert!(!subject.is_empty() && !body.is_empty());

    // evaluate takes no text; the same quiet instant stays blocked.
    assert_blocked(
        policy.evaluate(now, None, false),
        CeoReportSuppression::QuietHours,
        BJ_1025_0800,
    );
}

#[test]
fn evaluate_signature_takes_no_text() {
    let evaluate: fn(&CeoReportPolicy, u64, Option<u64>, bool) -> CeoReportDecision =
        CeoReportPolicy::evaluate;
    let policy = default_policy();
    assert_allowed(evaluate(&policy, BJ_1025_0800, None, false), BJ_1025_0800);
}

#[test]
fn configured_wrapping_window_spans_midnight() {
    let policy = default_policy()
        .with_quiet_window(22 * 60, 6 * 60)
        .expect("valid minute boundaries");

    let late = BJ_1025_0000 + 23 * HOUR_MS;
    assert_blocked(
        policy.evaluate(late, None, false),
        CeoReportSuppression::QuietHours,
        BJ_1025_0000 + DAY_MS + 6 * HOUR_MS,
    );

    let early = BJ_1025_0000 + 5 * HOUR_MS + 59 * MINUTE_MS;
    assert_blocked(
        policy.evaluate(early, None, false),
        CeoReportSuppression::QuietHours,
        BJ_1025_0000 + 6 * HOUR_MS,
    );

    assert_allowed(
        policy.evaluate(BJ_1025_0000 + 6 * HOUR_MS, None, false),
        BJ_1025_0000 + 6 * HOUR_MS,
    );
    assert_allowed(
        policy.evaluate(BJ_1025_0000 + 12 * HOUR_MS, None, false),
        BJ_1025_0000 + 12 * HOUR_MS,
    );
}

#[test]
fn user_supplied_minute_boundaries_are_validated() {
    let policy = default_policy();
    assert_eq!(
        policy.with_quiet_window(1440, 0).unwrap_err(),
        CeoReportPolicyError::QuietMinuteOutOfRange { minute: 1440 }
    );
    assert_eq!(
        policy.with_quiet_window(0, 1440).unwrap_err(),
        CeoReportPolicyError::QuietMinuteOutOfRange { minute: 1440 }
    );
    assert!(policy.with_quiet_window(23 * 60 + 59, 0).is_ok());
}

#[test]
fn equal_boundaries_disable_quiet_window() {
    let policy = default_policy()
        .with_quiet_window(8 * 60, 8 * 60)
        .expect("valid minute boundaries");
    let two_am = BJ_1025_0000 + 2 * HOUR_MS;
    assert_allowed(policy.evaluate(two_am, None, false), two_am);
}
