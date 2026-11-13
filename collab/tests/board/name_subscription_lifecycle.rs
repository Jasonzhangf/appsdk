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
    fixture.run_public(
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
            "--no-candidate",
            "--evidence",
            "the resumed task schedule delivered its public wake and progress receipts",
        ],
        SUCCESSOR_THREAD,
    );
    assert_eq!(delivered["status"], "delivered", "{delivered}");
    let reviewed = fixture.run_public(
        &[
            "task",
            "review",
            TASK_ID,
            "--accept",
            "--evidence",
            "another registered peer accepted the resumed task subscription evidence",
        ],
        THREAD_A,
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
