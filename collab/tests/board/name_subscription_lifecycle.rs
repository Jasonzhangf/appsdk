use super::*;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

const SUCCESSOR_THREAD: &str = "01a0d642-4a8f-7c42-9c93-641f1e2d3a4b";
const FILLER_THREAD: &str = "01a0d645-7b10-74aa-8d11-6b2f0c9e4d21";
const TASK_ID: &str = "public-name-task-subscription";
const INTERVAL_MS: i64 = 700;
const REPEAT_COUNT: u32 = 5;
const TTL_SECONDS: u64 = 300;

fn task_subscription(status: &Value, subscription_id: &str) -> Value {
    status["subscriptions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|subscription| subscription["id"] == subscription_id)
        .cloned()
        .unwrap_or_else(|| panic!("subscription {subscription_id} is absent: {status}"))
}

fn has_task_subscription(status: &Value, subscription_id: &str) -> bool {
    status["subscriptions"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|subscription| subscription["id"] == subscription_id)
}

fn task_subscription_from_state(current: &Value, subscription_id: &str) -> Value {
    current["notification_subscriptions"][subscription_id].clone()
}

fn wakes_for_subscription(current: &Value, subscription_id: &str) -> BTreeSet<String> {
    current["wake_bindings"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, bound)| *bound == subscription_id)
        .map(|(message_id, _)| message_id.clone())
        .collect()
}

fn input_has_message(input: &Value, message_id: &str) -> bool {
    input["input"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["text"].as_str())
        .chain(input["toolOutput"]["output"].as_str())
        .any(|text| {
            text.split(|character: char| !character.is_ascii_alphanumeric() && character != '-')
                .any(|token| token == message_id)
        })
}

fn wait_for_native_wake(
    fixture: &AppFixture,
    thread: &str,
    worker_id: &str,
    subscription_id: &str,
    timeout: Duration,
) -> String {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let current = fixture.current_state();
        for message_id in wakes_for_subscription(&current, subscription_id) {
            if current["msgs"][&message_id]["to"] != worker_id {
                continue;
            }
            let accepted = !current["notification_delivery_accepted"][&message_id].is_null()
                && current["notification_delivery_evidence"][&message_id].is_object();
            let native_input = fixture
                .state
                .lock()
                .unwrap()
                .get(thread)
                .is_some_and(|state| {
                    state
                        .inputs
                        .iter()
                        .any(|input| input_has_message(input, &message_id))
                });
            if accepted && native_input {
                return message_id;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("subscription {subscription_id} did not wake {thread}/{worker_id}");
}

fn board_task_revision(fixture: &AppFixture, caller: &str) -> u64 {
    let board = fixture.run_public(&["board", "show"], caller);
    board["tasks"]
        .as_array()
        .unwrap_or_else(|| panic!("board show lists tasks: {board}"))
        .iter()
        .find(|task| task["id"] == TASK_ID)
        .and_then(|task| task["revision"].as_u64())
        .unwrap_or_else(|| panic!("board task {TASK_ID} has a revision: {board}"))
}

fn native_input_count(fixture: &AppFixture, thread: &str) -> usize {
    fixture
        .state
        .lock()
        .unwrap()
        .get(thread)
        .map_or(0, |state| state.inputs.len())
}

fn start_task_with_schedule() -> (AppFixture, Value, Value, String, String) {
    let mut fixture = AppFixture::new();
    let master = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "fixture master creates the task schedule lifecycle",
        ],
        THREAD_A,
    );
    let filler = fixture.run_public(&["context"], FILLER_THREAD);
    assert_eq!(filler["name"], "Alice", "{filler}");
    let bob = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(bob["name"], "Bob", "{bob}");

    fixture.run_public(
        &[
            "task",
            "register",
            TASK_ID,
            "--next",
            "publish a task-linked subscription",
        ],
        THREAD_A,
    );
    let revision = board_task_revision(&fixture, THREAD_A);
    fixture.run_public(
        &[
            "board",
            "describe",
            TASK_ID,
            "--expected-revision",
            &revision.to_string(),
            "--title",
            "Task-linked subscription lifecycle",
            "--description",
            "The committed owner receives the finite task wake",
            "--delivery-condition",
            "The task remains with its current stable owner",
            "--test-condition",
            "public board, context and notify status receipts",
            "--long-horizon-content",
            "Continue the task under its committed current owner",
            "--long-horizon-every-ms",
            &INTERVAL_MS.to_string(),
            "--long-horizon-repeat",
            &REPEAT_COUNT.to_string(),
            "--long-horizon-ttl-seconds",
            &TTL_SECONDS.to_string(),
        ],
        THREAD_A,
    );
    let revision = board_task_revision(&fixture, THREAD_A);
    let admitted = fixture.run_public(
        &[
            "board",
            "backfill-execute",
            TASK_ID,
            "--expected-revision",
            &revision.to_string(),
            "--admit-legacy",
        ],
        THREAD_A,
    );
    assert_eq!(admitted["ticket"]["phases"][0]["status"], "claimed");
    let revision = board_task_revision(&fixture, THREAD_A);
    let started = fixture.run_public(
        &[
            "board",
            "start",
            TASK_ID,
            "--expected-revision",
            &revision.to_string(),
            "--attempt",
            "1",
        ],
        THREAD_A,
    );
    assert_eq!(started["task"]["execute"]["status"], "running");
    let status = fixture.run_public(&["notify", "status"], THREAD_A);
    let subscription = status["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|subscription| subscription["subject"] == format!("goal:task:{TASK_ID}"))
        .cloned()
        .unwrap_or_else(|| panic!("published task has a linked schedule: {status}"));
    let subscription_id = subscription["id"].as_str().unwrap().to_owned();
    let revision = board_task_revision(&fixture, THREAD_A);
    fixture.run_public(
        &[
            "board",
            "invite",
            TASK_ID,
            "--to",
            bob["name"].as_str().unwrap(),
            "--expected-revision",
            &revision.to_string(),
        ],
        THREAD_A,
    );
    let revision = board_task_revision(&fixture, THREAD_A);
    let accepted = fixture.run_public(
        &[
            "board",
            "respond",
            TASK_ID,
            "--accept",
            "--expected-revision",
            &revision.to_string(),
        ],
        THREAD_B,
    );
    let accepted_revision = accepted["task"]["revision"]
        .as_u64()
        .expect("accepted task revision")
        .to_string();
    let attempt = accepted["task"]["execute"]["attempt_generation"]
        .as_u64()
        .expect("accepted task attempt")
        .to_string();
    let started = fixture.run_public(
        &[
            "board",
            "start",
            TASK_ID,
            "--expected-revision",
            &accepted_revision,
            "--attempt",
            &attempt,
        ],
        THREAD_B,
    );
    assert_eq!(started["task"]["execute"]["status"], "running");
    (fixture, master, bob, subscription_id, attempt)
}

#[test]
fn public_completed_same_name_recovery_carries_the_exact_held_row() {
    let (mut fixture, master, bob, subscription_id, attempt) = start_task_with_schedule();
    let wake_id = wait_for_native_wake(
        &fixture,
        THREAD_B,
        bob["identity"]["worker_id"].as_str().unwrap(),
        &subscription_id,
        Duration::from_secs(5),
    );
    fixture.run_public(&["recv", "--timeout", "1"], THREAD_B);
    assert!(!wake_id.is_empty());
    fixture.update_current_state(|state| {
        let row = &mut state["notification_subscriptions"][&subscription_id];
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        row["trigger_ms"] = json!(now + 120_000);
        row["expires_ms"] = json!(now + 100_000);
    });
    let delivered = fixture.run_public(
        &[
            "task",
            "deliver",
            TASK_ID,
            "--attempt",
            &attempt,
            "--no-candidate",
            "--evidence",
            "deliver before the stable name is retired",
        ],
        THREAD_B,
    );
    assert_eq!(delivered["status"], "delivered", "{delivered}");
    let before_retire = task_subscription_from_state(&fixture.current_state(), &subscription_id);
    assert_eq!(before_retire["status"], "armed", "{before_retire}");
    let original_target = before_retire["target"].clone();

    let retired = fixture.run_public(
        &[
            "context",
            "--op",
            "review3-retire-completed-owner",
            "--approve-grant",
            r#"{"action":"retire_name","requested_name":"Bob","authorization":"fixture retires the delivered task owner"}"#,
        ],
        THREAD_A,
    );
    assert_eq!(retired["identity"]["status"], "released", "{retired}");
    let waiting = task_subscription_from_state(&fixture.current_state(), &subscription_id);
    assert_eq!(waiting["status"], "waiting-owner", "{waiting}");
    assert_eq!(
        waiting["worker_id"], bob["identity"]["worker_id"],
        "{waiting}"
    );

    let successor = fixture.run_public(&["context"], SUCCESSOR_THREAD);
    assert_eq!(successor["name"], "Bob", "{successor}");
    assert_ne!(
        successor["identity"]["worker_id"], master["identity"]["worker_id"],
        "the successor must be a distinct current holder"
    );
    let successor_worker = successor["identity"]["worker_id"].as_str().unwrap();
    let recovered = fixture.current_state();
    assert_eq!(
        recovered["tasks"][TASK_ID]["owner"], successor_worker,
        "the delivered task follows its successor name"
    );
    let held = task_subscription_from_state(&recovered, &subscription_id);
    assert_eq!(held["id"], subscription_id, "{held}");
    assert_eq!(held["status"], "waiting-owner", "{held}");
    assert_eq!(held["worker_id"], successor_worker, "{held}");
    assert_eq!(held["interval_ms"], INTERVAL_MS, "{held}");
    assert_eq!(held["repeat_count"], REPEAT_COUNT, "{held}");
    assert_eq!(held["fired_count"], before_retire["fired_count"], "{held}");
    assert_eq!(held["trigger_ms"], before_retire["trigger_ms"], "{held}");
    assert_eq!(held["expires_ms"], before_retire["expires_ms"], "{held}");
    assert_eq!(
        held["goal_content"], before_retire["goal_content"],
        "{held}"
    );
    assert_ne!(held["target"], original_target, "{held}");
    let direct_id = format!("sub-default-direct-message-{successor_worker}");
    let direct = &recovered["notification_subscriptions"][direct_id];
    assert_eq!(held["method"], direct["method"], "{held}");
    assert_eq!(held["target"], direct["target"], "{held}");
    let before_reconcile = fixture.current_state();
    let reconciled = fixture.command_public(
        &[
            "goal", "reconcile", "--goal-id", &format!("task:{TASK_ID}"),
            "--owner", "Bob", "--desired", "active", "--every-ms", "7000",
            "--repeat", "4", "--ttl-seconds", "300",
        ],
        SUCCESSOR_THREAD,
    );
    assert!(!reconciled.status.success(), "{reconciled:?}");
    assert!(
        String::from_utf8_lossy(&reconciled.stderr)
            .contains("GOAL_SCHEDULE_RECOVERY_FIELDS_INVALID"),
        "{reconciled:?}"
    );
    let after_reconcile = fixture.current_state();
    assert_eq!(after_reconcile["notification_subscriptions"], before_reconcile["notification_subscriptions"]);
    assert_eq!(after_reconcile["board_details"][TASK_ID]["long_horizon"], before_reconcile["board_details"][TASK_ID]["long_horizon"]);
    assert_eq!(wakes_for_subscription(&after_reconcile, &subscription_id), wakes_for_subscription(&before_reconcile, &subscription_id));
}

#[test]
fn public_review_rework_keeps_the_progressed_live_row_until_typed_start() {
    let (mut fixture, _master, bob, subscription_id, attempt) = start_task_with_schedule();
    let wake_id = wait_for_native_wake(
        &fixture,
        THREAD_B,
        bob["identity"]["worker_id"].as_str().unwrap(),
        &subscription_id,
        Duration::from_secs(5),
    );
    fixture.run_public(&["recv", "--timeout", "1"], THREAD_B);
    assert!(!wake_id.is_empty());
    fixture.update_current_state(|state| {
        let row = &mut state["notification_subscriptions"][&subscription_id];
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        row["trigger_ms"] = json!(now + 120_000);
        row["expires_ms"] = json!(now + 100_000);
    });
    let before_rework = task_subscription_from_state(&fixture.current_state(), &subscription_id);
    assert!(
        before_rework["fired_count"].as_u64().unwrap_or_default() > 0,
        "{before_rework}"
    );
    fixture.run_public(
        &[
            "task",
            "deliver",
            TASK_ID,
            "--attempt",
            &attempt,
            "--no-candidate",
            "--evidence",
            "deliver the progressed batch before review rework",
        ],
        THREAD_B,
    );
    let status = fixture.run_public(&["task", "status", TASK_ID], THREAD_B);
    let subject_revision = status["review"]["subject_revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let reworked = fixture.run_public(
        &[
            "task",
            "review",
            TASK_ID,
            "--rework",
            "--expected-subject-revision",
            &subject_revision,
            "--evidence",
            "fixture requests the replacement execute batch",
        ],
        THREAD_B,
    );
    assert_eq!(reworked["status"], "rework", "{reworked}");
    let board = fixture.run_public(&["board", "show"], THREAD_B);
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == TASK_ID)
        .unwrap();
    assert_eq!(task["execute"]["status"], "claimed", "{task}");
    let revision = task["revision"].as_u64().unwrap().to_string();
    let attempt = task["execute"]["attempt_generation"]
        .as_u64()
        .unwrap()
        .to_string();
    let retained = task_subscription_from_state(&fixture.current_state(), &subscription_id);
    assert_eq!(retained["id"], subscription_id, "{retained}");
    assert_eq!(retained["status"], before_rework["status"], "{retained}");
    assert_eq!(
        retained["worker_id"], before_rework["worker_id"],
        "{retained}"
    );
    assert_eq!(
        retained["interval_ms"], before_rework["interval_ms"],
        "{retained}"
    );
    assert_eq!(
        retained["repeat_count"], before_rework["repeat_count"],
        "{retained}"
    );
    assert_eq!(
        retained["fired_count"], before_rework["fired_count"],
        "{retained}"
    );
    assert_eq!(
        retained["trigger_ms"], before_rework["trigger_ms"],
        "{retained}"
    );
    assert_eq!(
        retained["expires_ms"], before_rework["expires_ms"],
        "{retained}"
    );
    assert_eq!(
        retained["goal_content"], before_rework["goal_content"],
        "{retained}"
    );
    let wakes = wakes_for_subscription(&fixture.current_state(), &subscription_id);
    thread::sleep(Duration::from_millis((INTERVAL_MS as u64) * 2 + 200));
    let quiet = fixture.current_state();
    assert_eq!(
        wakes_for_subscription(&quiet, &subscription_id),
        wakes,
        "a Claimed replacement Execute must not issue a task wake"
    );
    assert_eq!(
        task_subscription_from_state(&quiet, &subscription_id),
        retained,
        "the original live row must survive the Claimed interval"
    );

    fixture.run_public(
        &[
            "board",
            "start",
            TASK_ID,
            "--expected-revision",
            &revision,
            "--attempt",
            &attempt,
        ],
        THREAD_B,
    );
    let resumed = task_subscription_from_state(&fixture.current_state(), &subscription_id);
    assert_eq!(resumed["id"], subscription_id, "{resumed}");
    assert_eq!(resumed["status"], "armed", "{resumed}");
    assert_eq!(
        resumed["fired_count"], before_rework["fired_count"],
        "{resumed}"
    );
    assert_eq!(
        resumed["trigger_ms"], before_rework["trigger_ms"],
        "{resumed}"
    );
    assert_eq!(
        resumed["expires_ms"], before_rework["expires_ms"],
        "{resumed}"
    );
    assert_eq!(
        resumed["interval_ms"], before_rework["interval_ms"],
        "{resumed}"
    );
    assert_eq!(
        resumed["repeat_count"], before_rework["repeat_count"],
        "{resumed}"
    );
    assert_eq!(
        resumed["goal_content"], before_rework["goal_content"],
        "{resumed}"
    );

    let closed = fixture.run_public(
        &[
            "task",
            "close",
            TASK_ID,
            "--force",
            "--reason",
            "fixture terminates the review-rework lifecycle",
        ],
        THREAD_A,
    );
    assert_eq!(closed["status"], "closed", "{closed}");
    assert_eq!(
        task_subscription_from_state(&fixture.current_state(), &subscription_id)["status"],
        "cancelled"
    );
}

#[test]
fn public_completed_eligibility_excludes_merged_and_requires_a_live_row() {
    let (mut fixture, _master, _bob, subscription_id, attempt) = start_task_with_schedule();
    fixture.run_public(
        &[
            "task",
            "deliver",
            TASK_ID,
            "--attempt",
            &attempt,
            "--no-candidate",
            "--evidence",
            "retain the live row for completed eligibility checks",
        ],
        THREAD_B,
    );
    let armed = task_subscription_from_state(&fixture.current_state(), &subscription_id);
    assert_eq!(armed["status"], "armed", "{armed}");

    let before_delivery: BTreeSet<_> =
        wakes_for_subscription(&fixture.current_state(), &subscription_id);
    thread::sleep(Duration::from_millis((INTERVAL_MS as u64) * 2 + 200));
    assert!(
        wakes_for_subscription(&fixture.current_state(), &subscription_id).len()
            > before_delivery.len(),
        "an actionable delivered lifecycle must remain eligible for a due wake"
    );

    fixture.update_current_state(|state| {
        state["tasks"][TASK_ID]["status"] = json!("merged");
    });
    let before: BTreeSet<_> = wakes_for_subscription(&fixture.current_state(), &subscription_id);
    thread::sleep(Duration::from_millis((INTERVAL_MS as u64) * 2 + 200));
    let after_merged = fixture.current_state();
    assert_eq!(
        wakes_for_subscription(&after_merged, &subscription_id),
        before,
        "a merged Completed task must not receive a regular due wake"
    );
    assert_eq!(
        task_subscription_from_state(&after_merged, &subscription_id)["status"],
        "armed",
        "retention remains distinct from wake eligibility"
    );

    fixture.update_current_state(|state| {
        state["tasks"][TASK_ID]["status"] = json!("delivered");
        state["tasks"][TASK_ID]["wait"] = json!({
            "waiter": state["tasks"][TASK_ID]["owner"].clone(),
            "waiting_for": "typed-dependency-holder",
            "responsible_actor": state["tasks"][TASK_ID]["owner"].clone(),
            "reason": "typed_dependency",
            "deadline_ms": 9_999_999_999_999_i64,
            "resume_on": ["typed_release"],
            "escalation": "owner_recheck"
        });
    });
    let before_dependency: BTreeSet<_> =
        wakes_for_subscription(&fixture.current_state(), &subscription_id);
    thread::sleep(Duration::from_millis((INTERVAL_MS as u64) * 2 + 200));
    let after_dependency = fixture.current_state();
    assert_eq!(
        wakes_for_subscription(&after_dependency, &subscription_id),
        before_dependency,
        "a typed dependency must not receive a regular due wake"
    );
    assert_eq!(
        task_subscription_from_state(&after_dependency, &subscription_id)["status"],
        "armed",
        "the live row remains retained while the dependency blocks action"
    );
}

#[test]
fn public_completed_reconciliation_requires_the_original_live_row() {
    let (mut fixture, _master, _bob, subscription_id, attempt) = start_task_with_schedule();
    fixture.run_public(
        &[
            "task",
            "deliver",
            TASK_ID,
            "--attempt",
            &attempt,
            "--no-candidate",
            "--evidence",
            "deliver before testing completed reconciliation",
        ],
        THREAD_B,
    );
    fixture.update_current_state(|state| {
        state["notification_subscriptions"]
            .as_object_mut()
            .unwrap()
            .remove(&subscription_id);
        state["tasks"][TASK_ID]["wait"] = json!({
            "waiter": state["tasks"][TASK_ID]["owner"].clone(),
            "waiting_for": "typed-dependency-holder",
            "responsible_actor": state["tasks"][TASK_ID]["owner"].clone(),
            "reason": "typed_dependency",
            "deadline_ms": 9_999_999_999_999_i64,
            "resume_on": ["typed_release"],
            "escalation": "owner_recheck"
        });
    });
    let before = fixture.current_state();
    let reconcile = fixture.command_public(
        &[
            "goal",
            "reconcile",
            "--goal-id",
            &format!("task:{TASK_ID}"),
            "--owner",
            "Bob",
            "--desired",
            "active",
        ],
        THREAD_B,
    );
    assert!(
        !reconcile.status.success(),
        "ordinary reconciliation must not create a batch without a live row: {reconcile:?}"
    );
    assert!(
        String::from_utf8_lossy(&reconcile.stderr)
            .contains("GOAL_SCHEDULE_TASK_REQUIRES_LIVE_BATCH"),
        "{reconcile:?}"
    );
    let after = fixture.current_state();
    assert_eq!(
        after["notification_subscriptions"], before["notification_subscriptions"],
        "no replacement row may be created under the original or a new ID"
    );
}

fn completed_transport_lost_fixture() -> (AppFixture, String, String) {
    let (mut fixture, _master, bob, subscription_id, attempt) = start_task_with_schedule();
    let wake_id = wait_for_native_wake(
        &fixture,
        THREAD_B,
        bob["identity"]["worker_id"].as_str().unwrap(),
        &subscription_id,
        Duration::from_secs(5),
    );
    fixture.run_public(&["recv", "--timeout", "1"], THREAD_B);
    assert!(!wake_id.is_empty());
    fixture.run_public(
        &[
            "task",
            "deliver",
            TASK_ID,
            "--attempt",
            &attempt,
            "--no-candidate",
            "--evidence",
            "deliver before transport recovery",
        ],
        THREAD_B,
    );

    let worker = bob["identity"]["worker_id"].as_str().unwrap().to_owned();
    fixture.update_current_state(|state| {
        let row = &mut state["notification_subscriptions"][&subscription_id];
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        row["status"] = json!("transport-lost");
        row["status_reason"] = json!("fixture transport unavailable");
        row["method"] = json!("fixture-missing-method");
        row["target"] = json!("fixture-missing-target");
        row["trigger_ms"] = json!(now + 120_000);
        // The retained batch is authoritative even when Board policy differs.
        row["interval_ms"] = json!(90_000);
        row["repeat_count"] = json!(8);
        row["goal_content"]["ttl_seconds"] = json!(240);
    });
    (fixture, worker, subscription_id)
}

#[test]
fn public_completed_transport_lost_reconciliation_restores_the_exact_live_row() {
    let (fixture, worker, subscription_id) = completed_transport_lost_fixture();
    let before = fixture.current_state();
    assert_eq!(before["tasks"][TASK_ID]["status"], "delivered");
    let held = task_subscription_from_state(&before, &subscription_id);
    assert_eq!(held["status"], "transport-lost", "{held}");
    assert_eq!(held["worker_id"], worker, "{held}");
    let direct =
        &before["notification_subscriptions"][format!("sub-default-direct-message-{worker}")];
    assert!(!direct["method"].as_str().unwrap_or_default().is_empty());
    assert!(!direct["target"].as_str().unwrap_or_default().is_empty());
    let public_before = fixture.run_public(&["notify", "status"], THREAD_B);
    let public_held = task_subscription(&public_before, &subscription_id);
    assert_eq!(public_held["status"], "transport-lost", "{public_before}");
    let public_direct_id = format!("sub-default-direct-message-{worker}");
    let public_direct = task_subscription(&public_before, &public_direct_id);
    assert!(!public_direct["method"]
        .as_str()
        .unwrap_or_default()
        .is_empty());
    assert!(!public_direct["target"]
        .as_str()
        .unwrap_or_default()
        .is_empty());

    let reconciled = fixture.command_public(
        &[
            "goal",
            "reconcile",
            "--goal-id",
            &format!("task:{TASK_ID}"),
            "--owner",
            "Bob",
            "--desired",
            "active",
        ],
        THREAD_B,
    );
    assert!(
        reconciled.status.success(),
        "stdout: {}; stderr: {}",
        String::from_utf8_lossy(&reconciled.stdout),
        String::from_utf8_lossy(&reconciled.stderr)
    );
    let receipt: Value = serde_json::from_slice(&reconciled.stdout).expect("reconcile emits JSON");
    let receipt_subscription = &receipt["subscription"];
    assert_eq!(receipt_subscription["id"], subscription_id, "{receipt}");
    assert_eq!(receipt_subscription["status"], "armed", "{receipt}");
    assert_eq!(
        receipt_subscription["worker_id"], public_held["worker_id"],
        "{receipt}"
    );
    assert_eq!(
        receipt_subscription["method"], public_direct["method"],
        "{receipt}"
    );
    assert_eq!(
        receipt_subscription["target"], public_direct["target"],
        "{receipt}"
    );
    for field in [
        "subject",
        "interval_ms",
        "repeat_count",
        "fired_count",
        "trigger_ms",
        "expires_ms",
        "goal_content",
    ] {
        assert_eq!(
            receipt_subscription[field], public_held[field],
            "receipt changed {field}: {receipt}"
        );
    }

    let recovered = fixture.current_state();
    let resumed = task_subscription_from_state(&recovered, &subscription_id);
    assert_eq!(resumed["id"], subscription_id, "{resumed}");
    assert_eq!(resumed["status"], "armed", "{resumed}");
    assert_eq!(resumed["worker_id"], held["worker_id"], "{resumed}");
    assert_eq!(resumed["interval_ms"], held["interval_ms"], "{resumed}");
    assert_eq!(resumed["repeat_count"], held["repeat_count"], "{resumed}");
    assert_eq!(resumed["fired_count"], held["fired_count"], "{resumed}");
    assert_eq!(resumed["trigger_ms"], held["trigger_ms"], "{resumed}");
    assert_eq!(resumed["expires_ms"], held["expires_ms"], "{resumed}");
    assert_eq!(resumed["goal_content"], held["goal_content"], "{resumed}");
    assert_eq!(
        resumed["goal_content"]["source"]["task_id"], TASK_ID,
        "{resumed}"
    );
    assert_eq!(
        resumed["goal_content"]["ttl_seconds"], 240,
        "{resumed}"
    );
    assert_eq!(resumed["method"], direct["method"], "{resumed}");
    assert_eq!(resumed["target"], direct["target"], "{resumed}");
    assert!(resumed["status_reason"].is_null(), "{resumed}");
    let public_status = fixture.run_public(&["notify", "status"], THREAD_B);
    let public_resumed = task_subscription(&public_status, &subscription_id);
    for field in ["id", "worker_id"] {
        assert_eq!(
            public_resumed[field], public_held[field],
            "notify status differs for {field}: {public_status}"
        );
    }
    assert_eq!(public_resumed["status"], "armed", "{public_status}");
    assert_eq!(
        public_resumed["method"], public_direct["method"],
        "{public_status}"
    );
    assert_eq!(
        public_resumed["target"], public_direct["target"],
        "{public_status}"
    );
    for field in [
        "subject",
        "interval_ms",
        "repeat_count",
        "fired_count",
        "trigger_ms",
        "expires_ms",
        "goal_content",
    ] {
        assert_eq!(
            public_resumed[field], public_held[field],
            "notify status changed {field}: {public_status}"
        );
    }
    assert!(public_resumed["status_reason"].is_null(), "{public_status}");
    let linked_rows = recovered["notification_subscriptions"]
        .as_object()
        .unwrap()
        .values()
        .filter(|row| row["goal_content"]["source"]["task_id"] == TASK_ID)
        .count();
    assert_eq!(linked_rows, 1, "recovery must retain one exact task row");
    assert_eq!(recovered["board_details"], before["board_details"]);
    assert_eq!(recovered["task_lifecycle"], before["task_lifecycle"]);
}

fn refused_transport_recovery(fixture: &AppFixture, thread: &str, extra: &[&str], error: &str) {
    let before = fixture.current_state();
    let goal = format!("task:{TASK_ID}");
    let mut args = vec![
        "goal",
        "reconcile",
        "--goal-id",
        &goal,
        "--owner",
        "Bob",
        "--desired",
        "active",
    ];
    args.extend_from_slice(extra);
    let output = fixture.command_public(&args, thread);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(error),
        "{output:?}"
    );
    let after = fixture.current_state();
    for field in [
        "notification_subscriptions",
        "board_details",
        "task_lifecycle",
    ] {
        assert_eq!(after[field], before[field], "refusal changed {field}");
    }
}

#[test]
fn public_completed_transport_lost_reconciliation_refuses_policy_overrides() {
    let (fixture, _, _) = completed_transport_lost_fixture();
    for extra in [
        vec!["--repeat", "1"],
        vec!["--every-ms", "90001"],
        vec!["--ttl-seconds", "200"],
        vec!["--content", "unauthorized recovery content"],
        vec!["--at-ms", "9999999999999"],
    ] {
        refused_transport_recovery(
            &fixture,
            THREAD_B,
            &extra,
            "GOAL_SCHEDULE_RECOVERY_FIELDS_INVALID",
        );
    }
}

#[test]
fn public_completed_transport_lost_reconciliation_requires_authenticated_owner() {
    let (fixture, _, _) = completed_transport_lost_fixture();
    refused_transport_recovery(
        &fixture,
        THREAD_A,
        &[],
        "GOAL_SCHEDULE_RECOVERY_OWNER_MISMATCH",
    );
}

#[test]
fn public_completed_transport_lost_reconciliation_requires_actionable_owner() {
    let (mut fixture, _, _) = completed_transport_lost_fixture();
    fixture.update_current_state(|state| {
        state["tasks"][TASK_ID]["status"] = json!("merged");
    });
    refused_transport_recovery(
        &fixture,
        THREAD_B,
        &[],
        "GOAL_SCHEDULE_RECOVERY_NOT_ACTIONABLE",
    );
    fixture.update_current_state(|state| {
        state["tasks"][TASK_ID]["status"] = json!("delivered");
        state["tasks"][TASK_ID]["wait"] = json!({
            "waiter": state["tasks"][TASK_ID]["owner"].clone(),
            "waiting_for": "typed-dependency-holder",
            "responsible_actor": state["tasks"][TASK_ID]["owner"].clone(),
            "reason": "typed_dependency", "deadline_ms": 9_999_999_999_999_i64,
            "resume_on": ["typed_release"], "escalation": "owner_recheck"
        });
    });
    refused_transport_recovery(
        &fixture,
        THREAD_B,
        &[],
        "GOAL_SCHEDULE_RECOVERY_NOT_ACTIONABLE",
    );
}

#[test]
fn public_completed_transport_lost_reconciliation_requires_current_route() {
    let (mut fixture, worker, _) = completed_transport_lost_fixture();
    fixture.update_current_state(|state| {
        state["workers"][&worker]["transport"] = Value::Null;
    });
    refused_transport_recovery(
        &fixture,
        THREAD_B,
        &[],
        "GOAL_SCHEDULE_RECOVERY_ROUTE_UNAVAILABLE",
    );
}

#[test]
fn public_retirement_waits_task_subscription_then_resumes_for_current_bob() {
    let mut fixture = AppFixture::new();
    let master = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "fixture master creates one task-linked long-horizon schedule",
        ],
        THREAD_A,
    );
    // Promotion frees the first peer's ordinary name. Keep that slot occupied
    // so the fresh successor deterministically reacquires the retired name.
    let filler = fixture.run_public(&["context"], FILLER_THREAD);
    assert_eq!(filler["name"], "Alice", "{filler}");
    let bob = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(bob["name"], "Bob", "{bob}");

    fixture.run_public(
        &[
            "task",
            "register",
            TASK_ID,
            "--next",
            "publish a task-linked subscription",
        ],
        THREAD_A,
    );
    let revision = board_task_revision(&fixture, THREAD_A);
    fixture.run_public(
        &[
            "board",
            "describe",
            TASK_ID,
            "--expected-revision",
            &revision.to_string(),
            "--title",
            "Task-linked subscription lifecycle",
            "--description",
            "The committed owner receives the finite task wake",
            "--delivery-condition",
            "The task remains with its current stable owner",
            "--test-condition",
            "public board, context and notify status receipts",
            "--long-horizon-content",
            "Continue the task under its committed current owner",
            "--long-horizon-every-ms",
            &INTERVAL_MS.to_string(),
            "--long-horizon-repeat",
            &REPEAT_COUNT.to_string(),
            "--long-horizon-ttl-seconds",
            &TTL_SECONDS.to_string(),
        ],
        THREAD_A,
    );

    // The task-register path predates typed execution. Admit it through the
    // public repair path, then issue the typed start that activates its
    // declared cadence. This keeps the later name recovery on the supported
    // execution lifecycle instead of treating legacy working state as started.
    let revision = board_task_revision(&fixture, THREAD_A);
    let admitted = fixture.run_public(
        &[
            "board",
            "backfill-execute",
            TASK_ID,
            "--expected-revision",
            &revision.to_string(),
            "--admit-legacy",
        ],
        THREAD_A,
    );
    assert_eq!(admitted["ticket"]["phases"][0]["status"], "claimed");
    let revision = board_task_revision(&fixture, THREAD_A);
    let started = fixture.run_public(
        &[
            "board",
            "start",
            TASK_ID,
            "--expected-revision",
            &revision.to_string(),
            "--attempt",
            "1",
        ],
        THREAD_A,
    );
    assert_eq!(started["task"]["execute"]["status"], "running");

    let master_status = fixture.run_public(&["notify", "status"], THREAD_A);
    let subscription = master_status["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|subscription| subscription["subject"] == format!("goal:task:{TASK_ID}"))
        .unwrap_or_else(|| panic!("published task has a linked schedule: {master_status}"));
    let subscription_id = subscription["id"].as_str().unwrap().to_owned();
    let original_content = subscription["goal_content"].clone();
    assert_eq!(
        subscription["worker_id"], master["identity"]["worker_id"],
        "{subscription}"
    );
    assert_eq!(subscription["interval_ms"], INTERVAL_MS, "{subscription}");
    assert_eq!(subscription["repeat_count"], REPEAT_COUNT, "{subscription}");
    assert_eq!(subscription["fired_count"], 0, "{subscription}");

    let revision = board_task_revision(&fixture, THREAD_A);
    fixture.run_public(
        &[
            "board",
            "invite",
            TASK_ID,
            "--to",
            bob["name"].as_str().unwrap(),
            "--expected-revision",
            &revision.to_string(),
        ],
        THREAD_A,
    );

    // An invitation is not a committed owner transfer.
    assert_eq!(
        task_subscription(
            &fixture.run_public(&["notify", "status"], THREAD_A),
            &subscription_id
        )["worker_id"],
        master["identity"]["worker_id"]
    );
    assert!(!has_task_subscription(
        &fixture.run_public(&["notify", "status"], THREAD_B),
        &subscription_id
    ));

    let revision = board_task_revision(&fixture, THREAD_A);
    let accepted = fixture.run_public(
        &[
            "board",
            "respond",
            TASK_ID,
            "--accept",
            "--expected-revision",
            &revision.to_string(),
        ],
        THREAD_B,
    );
    let accepted_revision = accepted["task"]["revision"]
        .as_u64()
        .expect("accepted task revision")
        .to_string();
    let accepted_attempt = accepted["task"]["execute"]["attempt_generation"]
        .as_u64()
        .expect("accepted task attempt")
        .to_string();
    let started = fixture.run_public(
        &[
            "board",
            "start",
            TASK_ID,
            "--expected-revision",
            &accepted_revision,
            "--attempt",
            &accepted_attempt,
        ],
        THREAD_B,
    );
    assert_eq!(started["task"]["execute"]["status"], "running");
    let bob_status = fixture.run_public(&["notify", "status"], THREAD_B);
    let accepted_subscription = task_subscription(&bob_status, &subscription_id);
    assert_eq!(
        accepted_subscription["worker_id"], bob["identity"]["worker_id"],
        "{accepted_subscription}"
    );
    assert_eq!(
        accepted_subscription["goal_content"], original_content,
        "{accepted_subscription}"
    );

    let first_wake_id = wait_for_native_wake(
        &fixture,
        THREAD_B,
        bob["identity"]["worker_id"].as_str().unwrap(),
        &subscription_id,
        Duration::from_secs(5),
    );
    let received = fixture.run_public(&["recv", "--timeout", "1"], THREAD_B);
    assert!(
        received["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|message| message["id"] == first_wake_id),
        "the current owner must publicly consume the real task wake: {received}"
    );
    let before_retire = task_subscription_from_state(&fixture.current_state(), &subscription_id);
    assert!(
        before_retire["fired_count"].as_u64().unwrap_or_default() >= 1,
        "the real periodic timer must consume progress before retirement: {before_retire}"
    );
    let fired_before_retire = before_retire["fired_count"].clone();
    let trigger_before_retire = before_retire["trigger_ms"].clone();
    let expires_before_retire = before_retire["expires_ms"].clone();
    let inputs_before_retire = native_input_count(&fixture, THREAD_B);
    let wakes_before_retire = wakes_for_subscription(&fixture.current_state(), &subscription_id);

    let retired = fixture.run_public(
        &[
            "context",
            "--op",
            "public-task-subscription-retire-bob",
            "--approve-grant",
            r#"{"action":"retire_name","requested_name":"Bob","authorization":"fixture explicitly retires Bob"}"#,
        ],
        THREAD_A,
    );
    assert_eq!(retired["identity"]["status"], "released", "{retired}");
    assert_eq!(retired["operations"]["receipt"], true, "{retired}");
    assert_eq!(retired["operations"]["pending"], false, "{retired}");
    let waiting = task_subscription_from_state(&fixture.current_state(), &subscription_id);
    assert_eq!(waiting["status"], "waiting-owner", "{waiting}");
    assert_eq!(
        waiting["worker_id"], bob["identity"]["worker_id"],
        "{waiting}"
    );
    assert_eq!(waiting["goal_content"], original_content, "{waiting}");
    assert_eq!(waiting["fired_count"], fired_before_retire, "{waiting}");
    assert_eq!(waiting["repeat_count"], REPEAT_COUNT, "{waiting}");
    assert_eq!(waiting["interval_ms"], INTERVAL_MS, "{waiting}");
    assert_eq!(waiting["trigger_ms"], trigger_before_retire, "{waiting}");
    assert_eq!(waiting["expires_ms"], expires_before_retire, "{waiting}");

    // Let at least two nominal timer periods pass with no legal owner.
    thread::sleep(Duration::from_millis((INTERVAL_MS as u64) * 2 + 600));
    let after_wait = fixture.current_state();
    assert!(
        wakes_for_subscription(&after_wait, &subscription_id).is_subset(&wakes_before_retire),
        "a waiting task schedule must not bind a new wake to the retired holder"
    );
    assert_eq!(
        native_input_count(&fixture, THREAD_B),
        inputs_before_retire,
        "the retired holder must not receive the name-owned task wake"
    );
    assert_eq!(
        task_subscription_from_state(&after_wait, &subscription_id),
        waiting,
        "waiting must preserve the original subscription record while no holder exists"
    );

    let successor = fixture.run_public(&["context"], SUCCESSOR_THREAD);
    assert_eq!(successor["name"], "Bob", "{successor}");
    assert_ne!(
        successor["identity"]["worker_id"], bob["identity"]["worker_id"],
        "{successor}"
    );
    // Restoring the stable name leaves the revoked Execute batch queued.
    // Claim and start it before the held cadence can resume.
    let revision = board_task_revision(&fixture, SUCCESSOR_THREAD);
    let claimed = fixture.run_public(
        &[
            "board",
            "claim",
            TASK_ID,
            "--expected-revision",
            &revision.to_string(),
        ],
        SUCCESSOR_THREAD,
    );
    assert_eq!(claimed["task"]["execute"]["status"], "claimed");
    assert_eq!(claimed["goal_schedule"]["armed"], false);
    let held = &claimed["goal_schedule"]["subscription"];
    assert_eq!(held["id"], subscription_id);
    assert_eq!(held["fired_count"], fired_before_retire);
    assert_eq!(held["trigger_ms"], trigger_before_retire);
    assert_eq!(held["expires_ms"], expires_before_retire);
    let revision = claimed["task"]["revision"].as_u64().unwrap().to_string();
    let attempt = claimed["task"]["execute"]["attempt_generation"]
        .as_u64()
        .unwrap()
        .to_string();
    fixture.run_public(
        &[
            "task",
            "start",
            TASK_ID,
            "--expected-revision",
            &revision,
            "--attempt",
            &attempt,
        ],
        SUCCESSOR_THREAD,
    );
    let successor_status = fixture.run_public(&["notify", "status"], SUCCESSOR_THREAD);
    let resumed = task_subscription(&successor_status, &subscription_id);
    assert_eq!(resumed["status"], "armed", "{resumed}");
    assert_eq!(
        resumed["worker_id"], successor["identity"]["worker_id"],
        "{resumed}"
    );
    assert_eq!(resumed["goal_content"], original_content, "{resumed}");
    assert_eq!(resumed["fired_count"], fired_before_retire, "{resumed}");
    assert_eq!(resumed["repeat_count"], REPEAT_COUNT, "{resumed}");
    assert_eq!(resumed["interval_ms"], INTERVAL_MS, "{resumed}");
    assert_eq!(resumed["trigger_ms"], trigger_before_retire, "{resumed}");
    assert_eq!(resumed["expires_ms"], expires_before_retire, "{resumed}");

    // The schedule owner and its cadence survive a private daemon restart; the
    // next real occurrence must still reach the new holder.
    fixture.restart_daemon();
    let after_restart = task_subscription(
        &fixture.run_public(&["notify", "status"], SUCCESSOR_THREAD),
        &subscription_id,
    );
    assert_eq!(
        after_restart, resumed,
        "restart preserves the task schedule"
    );

    let second_wake_id = wait_for_native_wake(
        &fixture,
        SUCCESSOR_THREAD,
        successor["identity"]["worker_id"].as_str().unwrap(),
        &subscription_id,
        Duration::from_secs(5),
    );
    let received = fixture.run_public(&["recv", "--timeout", "1"], SUCCESSOR_THREAD);
    assert!(
        received["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|message| message["id"] == second_wake_id),
        "the new holder must publicly consume the resumed task wake: {received}"
    );
    let after_resume = task_subscription_from_state(&fixture.current_state(), &subscription_id);
    assert!(
        after_resume["fired_count"].as_u64().unwrap_or_default()
            > fired_before_retire.as_u64().unwrap_or_default(),
        "the resumed schedule must consume its next real occurrence: {after_resume}"
    );

    let delivered = fixture.run_public(
        &[
            "task",
            "deliver",
            TASK_ID,
            "--attempt",
            &attempt,
            "--no-candidate",
            "--evidence",
            "the resumed task schedule delivered its public wake and progress receipts",
        ],
        SUCCESSOR_THREAD,
    );
    assert_eq!(delivered["status"], "delivered", "{delivered}");
    let status = fixture.run_public(&["task", "status", TASK_ID], SUCCESSOR_THREAD);
    let subject_revision = status["review"]["subject_revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let reviewed = fixture.run_public(
        &[
            "task",
            "review",
            TASK_ID,
            "--accept",
            "--expected-subject-revision",
            &subject_revision,
            "--evidence",
            "the owner records independent verification of the resumed subscription",
        ],
        SUCCESSOR_THREAD,
    );
    assert_eq!(reviewed["merge_pending"], false, "{reviewed}");
    assert_eq!(reviewed["disposition"], "no_merge_required", "{reviewed}");
    let closed = fixture.run_public(
        &[
            "task",
            "close",
            TASK_ID,
            "--reason",
            "fixture completed the task-owned schedule",
        ],
        SUCCESSOR_THREAD,
    );
    assert_eq!(closed["status"], "closed", "{closed}");
    let terminal_status = fixture.run_public(&["notify", "status"], SUCCESSOR_THREAD);
    assert_eq!(
        task_subscription(&terminal_status, &subscription_id)["status"],
        "cancelled",
        "closing the task keeps its terminal schedule outcome"
    );
}

#[test]
fn public_legacy_batch_survives_successor_update_and_legacy_admission_until_typed_start() {
    const HELD_ID: &str = "legacy-waiting-subscription";
    const THREAD_C: &str = SUCCESSOR_THREAD;

    let mut fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "fixture master owns the legacy lifecycle setup",
        ],
        THREAD_A,
    );
    let filler = fixture.run_public(&["context"], FILLER_THREAD);
    assert_eq!(filler["name"], "Alice", "{filler}");
    let bob = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(bob["name"], "Bob", "{bob}");
    let successor_before = fixture.run_public(&["context"], THREAD_C);
    assert_eq!(successor_before["name"], "Carol", "{successor_before}");

    fixture.run_public(
        &[
            "task",
            "register",
            TASK_ID,
            "--next",
            "persist a legacy working task before typed execution",
        ],
        THREAD_A,
    );
    let revision = board_task_revision(&fixture, THREAD_A);
    fixture.run_public(
        &[
            "board",
            "describe",
            TASK_ID,
            "--expected-revision",
            &revision.to_string(),
            "--title",
            "Legacy task subscription lifecycle",
            "--description",
            "A held task schedule survives the first typed admission",
            "--delivery-condition",
            "The task remains with its recovered current owner",
            "--test-condition",
            "public admission, start, wake and close receipts",
            "--long-horizon-content",
            "Continue the persisted task after owner recovery",
            "--long-horizon-every-ms",
            &INTERVAL_MS.to_string(),
            "--long-horizon-repeat",
            &REPEAT_COUNT.to_string(),
            "--long-horizon-ttl-seconds",
            &TTL_SECONDS.to_string(),
        ],
        THREAD_A,
    );

    let bob_worker = bob["identity"]["worker_id"].as_str().unwrap().to_owned();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let trigger = now + INTERVAL_MS;
    let expires = now + (TTL_SECONDS as i64) * 1000;
    fixture.update_current_state(|state| {
        let task = state["tasks"][TASK_ID].as_object_mut().unwrap();
        task.insert("owner".into(), json!(bob_worker));
        task.insert("status".into(), json!("working"));
        task.insert("updated_ms".into(), json!(now));
        state["task_assignments"][TASK_ID] = json!({
            "assignment_revision": 1,
            "name": "Bob",
            "session_id": THREAD_B,
            "task_id": TASK_ID,
            "updated_ms": now,
        });
        state["board_details"][TASK_ID]["admitted"] = json!(false);
        state["ticket_phases"]
            .as_array_mut()
            .unwrap()
            .retain(|phase| phase["task_id"] != TASK_ID);
        let direct_subscription = &state["notification_subscriptions"]
            [format!("sub-default-direct-message-{bob_worker}")];
        state["notification_subscriptions"][HELD_ID] = json!({
            "created_ms": now,
            "event": "deadline",
            "expires_ms": expires,
            "fired_count": 2,
            "goal_content": {
                "source": {"kind": "board-task", "task_id": TASK_ID},
                "ttl_seconds": TTL_SECONDS,
            },
            "id": HELD_ID,
            "interval_ms": INTERVAL_MS,
            "method": direct_subscription["method"],
            "patrol_idle_since_ms": null,
            "patrol_last_push_ms": 0,
            "periodic_declared": true,
            "repeat_count": REPEAT_COUNT,
            "status": "waiting-owner",
            "status_reason": "legacy owner retired before typed execution",
            "subject": format!("goal:task:{TASK_ID}"),
            "target": direct_subscription["target"],
            "trigger_ms": trigger,
            "trigger_times_ms": [],
            "updated_ms": now,
            "worker_id": bob_worker,
        });
    });

    let persisted = fixture.current_state();
    assert_eq!(persisted["tasks"][TASK_ID]["status"], "working");
    assert_eq!(
        task_subscription_from_state(&persisted, HELD_ID)["status"],
        "waiting-owner"
    );

    let handed = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_name","requested_name":"Bob","authorization":"fixture confirms the legacy successor","replace_confirmed":true}"#,
        ],
        THREAD_C,
    );
    let successor = handed["identity"]["worker_id"].as_str().unwrap().to_owned();
    assert_ne!(successor, bob_worker);
    let recovered = fixture.current_state();
    assert_eq!(recovered["tasks"][TASK_ID]["owner"], successor);
    let held = task_subscription_from_state(&recovered, HELD_ID);
    assert_eq!(held["status"], "waiting-owner", "{held}");
    assert_eq!(held["worker_id"], successor, "{held}");
    assert_eq!(held["interval_ms"], INTERVAL_MS, "{held}");
    assert_eq!(held["repeat_count"], REPEAT_COUNT, "{held}");
    assert_eq!(held["fired_count"], 2, "{held}");
    assert_eq!(held["trigger_ms"], trigger, "{held}");
    assert_eq!(held["expires_ms"], expires, "{held}");

    let revision = board_task_revision(&fixture, THREAD_C);
    let updated = fixture.run_public(
        &[
            "board",
            "update",
            TASK_ID,
            "--expected-revision",
            &revision.to_string(),
            "--next",
            "recovered owner continues the persisted legacy task",
        ],
        THREAD_C,
    );
    assert_eq!(updated["task"]["status"], "working");
    assert_eq!(
        task_subscription_from_state(&fixture.current_state(), HELD_ID),
        held,
        "an ordinary nonterminal update must not cancel or mutate the held row"
    );

    let revision = board_task_revision(&fixture, THREAD_A);
    let admitted = fixture.run_public(
        &[
            "board",
            "backfill-execute",
            TASK_ID,
            "--expected-revision",
            &revision.to_string(),
            "--admit-legacy",
        ],
        THREAD_A,
    );
    assert_eq!(admitted["ticket"]["phases"][0]["status"], "claimed");
    assert!(wakes_for_subscription(&fixture.current_state(), HELD_ID).is_empty());
    assert_eq!(
        task_subscription_from_state(&fixture.current_state(), HELD_ID),
        held,
        "first legacy admission must leave the exact waiting row available"
    );

    thread::sleep(Duration::from_millis((INTERVAL_MS as u64) * 2 + 200));
    let quiet = fixture.current_state();
    assert!(wakes_for_subscription(&quiet, HELD_ID).is_empty());
    assert_eq!(task_subscription_from_state(&quiet, HELD_ID), held);

    let revision = board_task_revision(&fixture, THREAD_C);
    let started = fixture.run_public(
        &[
            "board",
            "start",
            TASK_ID,
            "--expected-revision",
            &revision.to_string(),
            "--attempt",
            "1",
        ],
        THREAD_C,
    );
    assert_eq!(started["task"]["execute"]["status"], "running");
    let resumed = task_subscription_from_state(&fixture.current_state(), HELD_ID);
    assert_eq!(resumed["id"], HELD_ID);
    assert_eq!(resumed["status"], "armed");
    assert_eq!(resumed["worker_id"], successor);
    assert_eq!(resumed["interval_ms"], INTERVAL_MS);
    assert_eq!(resumed["repeat_count"], REPEAT_COUNT);
    assert_eq!(resumed["fired_count"], 2);
    assert_eq!(resumed["trigger_ms"], trigger);
    assert_eq!(resumed["expires_ms"], expires);

    let wake_id = wait_for_native_wake(
        &fixture,
        THREAD_C,
        &successor,
        HELD_ID,
        Duration::from_secs(5),
    );
    let received = fixture.run_public(&["recv", "--timeout", "1"], THREAD_C);
    assert!(
        received["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|message| message["id"] == wake_id),
        "{received}"
    );

    let delivered = fixture.run_public(
        &[
            "task",
            "deliver",
            TASK_ID,
            "--attempt",
            "1",
            "--no-candidate",
            "--evidence",
            "legacy admission and typed start preserved the recovered subscription",
        ],
        THREAD_C,
    );
    assert_eq!(delivered["status"], "delivered");
    let status = fixture.run_public(&["task", "status", TASK_ID], THREAD_C);
    let subject_revision = status["review"]["subject_revision"]
        .as_u64()
        .unwrap()
        .to_string();
    fixture.run_public(
        &[
            "task",
            "review",
            TASK_ID,
            "--accept",
            "--expected-subject-revision",
            &subject_revision,
            "--evidence",
            "the recovered owner verified the legacy subscription lifecycle",
        ],
        THREAD_C,
    );
    let closed = fixture.run_public(
        &[
            "task",
            "close",
            TASK_ID,
            "--reason",
            "fixture completed the legacy task-owned schedule",
        ],
        THREAD_C,
    );
    assert_eq!(closed["status"], "closed", "{closed}");
    assert_eq!(
        task_subscription_from_state(&fixture.current_state(), HELD_ID)["status"],
        "cancelled"
    );
}
