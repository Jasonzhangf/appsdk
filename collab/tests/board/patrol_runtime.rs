use super::*;
use std::collections::BTreeSet;

const CUE_TIMEOUT: Duration = Duration::from_secs(10);
const SKIP_TIMEOUT: Duration = Duration::from_secs(4);
const NEGATIVE_WAIT: Duration = Duration::from_millis(2300);
const DUPLICATE_WAIT: Duration = Duration::from_millis(1200);

#[derive(Clone)]
struct CueBaseline {
    wakes: BTreeSet<String>,
    notices: BTreeSet<String>,
    inputs: usize,
}

fn journal_events(fixture: &AppFixture) -> Vec<Value> {
    std::fs::read_to_string(fixture.name_journal())
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn wake_ids(events: &[Value]) -> BTreeSet<String> {
    events
        .iter()
        .filter(|event| event["ev"] == "WakeBound")
        .filter_map(|event| event["message_id"].as_str().map(str::to_owned))
        .collect()
}

fn notice_ids(events: &[Value]) -> BTreeSet<String> {
    events
        .iter()
        .filter(|event| event["ev"] == "BoardPatrolNoticeBound")
        .filter_map(|event| event["message_id"].as_str().map(str::to_owned))
        .collect()
}

fn message_target(events: &[Value], message_id: &str) -> Option<String> {
    events.iter().find_map(|event| {
        (event["ev"] == "Sent" && event["msg"]["id"].as_str() == Some(message_id))
            .then(|| event["msg"]["to"].as_str().map(str::to_owned))
            .flatten()
    })
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

fn native_input_for(fixture: &AppFixture, thread: &str, message_id: &str) -> bool {
    fixture
        .state
        .lock()
        .unwrap()
        .get(thread)
        .is_some_and(|state| {
            state
                .inputs
                .iter()
                .any(|input| input_has_message(input, message_id))
        })
}

fn cue_baseline(fixture: &AppFixture, thread: &str) -> CueBaseline {
    let events = journal_events(fixture);
    CueBaseline {
        wakes: wake_ids(&events),
        notices: notice_ids(&events),
        inputs: fixture
            .state
            .lock()
            .unwrap()
            .get(thread)
            .map_or(0, |state| state.inputs.len()),
    }
}

fn set_native_idle(fixture: &AppFixture, thread: &str) {
    let mut states = fixture.state.lock().unwrap();
    let state = states
        .get_mut(thread)
        .unwrap_or_else(|| panic!("native thread {thread} must exist"));
    state.status = "idle".into();
    state.active_turn = None;
    for turn in &mut state.turns {
        if let Some(turn) = turn.as_object_mut() {
            turn.insert("status".into(), json!("completed"));
        }
    }
}

fn set_native_status(fixture: &AppFixture, thread: &str, status: &str) {
    fixture
        .state
        .lock()
        .unwrap()
        .get_mut(thread)
        .unwrap_or_else(|| panic!("native thread {thread} must exist"))
        .status = status.into();
}

fn advance_gate(fixture: &mut AppFixture, context: &Value, last_wake_tasks: &[&str]) -> usize {
    let start = journal_events(fixture).len();
    let worker_id = context["identity"]["worker_id"]
        .as_str()
        .expect("context has worker id");
    let owner_key = context["name"].as_str().expect("context has stable name");
    fixture.append_name_events(&[json!({
        "ev": "BoardPatrolGateUpdated",
        "worker_id": worker_id,
        "gate": {
            "owner_key": owner_key,
            "last_wake_ms": 0,
            "last_realtime_idle_since_ms": i64::MIN,
            "last_wake_tasks": last_wake_tasks,
            "next_due_ms": 0
        }
    })]);
    start
}

fn wait_for_new_cue(
    fixture: &AppFixture,
    thread: &str,
    worker_id: &str,
    baseline: &CueBaseline,
    timeout: Duration,
) -> Result<String, String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let events = journal_events(fixture);
        for message_id in notice_ids(&events).difference(&baseline.notices) {
            if message_target(&events, message_id).as_deref() != Some(worker_id) {
                continue;
            }
            let bound = events.iter().any(|event| {
                event["ev"] == "WakeBound" && event["message_id"] == message_id.as_str()
            });
            let accepted = events.iter().any(|event| {
                event["ev"] == "NotificationDeliveryAccepted"
                    && event["message_id"] == message_id.as_str()
                    && event["evidence"].is_object()
            });
            if bound && accepted && native_input_for(fixture, thread, message_id) {
                return Ok(message_id.clone());
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(format!(
        "board cue did not reach {thread}/{worker_id}; new_notices={:?}",
        notice_ids(&journal_events(fixture))
            .difference(&baseline.notices)
            .collect::<Vec<_>>()
    ))
}

fn assert_no_new_cue(fixture: &AppFixture, thread: &str, worker_id: &str, baseline: &CueBaseline) {
    let events = journal_events(fixture);
    let new_notices: Vec<_> = notice_ids(&events)
        .difference(&baseline.notices)
        .cloned()
        .collect();
    let delivered_to_worker: Vec<_> = new_notices
        .iter()
        .filter(|message_id| message_target(&events, message_id).as_deref() == Some(worker_id))
        .cloned()
        .collect();
    assert!(
        delivered_to_worker.is_empty(),
        "native status must not produce a board cue for {worker_id}: {delivered_to_worker:?}"
    );
    let new_wakes: Vec<_> = wake_ids(&events)
        .difference(&baseline.wakes)
        .filter(|message_id| message_target(&events, message_id).as_deref() == Some(worker_id))
        .cloned()
        .collect();
    assert!(
        new_wakes.is_empty(),
        "native status must not bind a new wake for {worker_id}: {new_wakes:?}"
    );
    let input_count = fixture
        .state
        .lock()
        .unwrap()
        .get(thread)
        .map_or(0, |state| state.inputs.len());
    assert_eq!(
        input_count, baseline.inputs,
        "native status must not submit an input to {thread}"
    );
}

fn latest_gate_after(fixture: &AppFixture, worker_id: &str, start: usize) -> Option<Value> {
    journal_events(fixture)
        .into_iter()
        .skip(start)
        .filter(|event| event["ev"] == "BoardPatrolGateUpdated" && event["worker_id"] == worker_id)
        .last()
        .map(|event| event["gate"].clone())
}

fn wait_for_skipped_gate(fixture: &AppFixture, worker_id: &str, start: usize) -> Value {
    let deadline = Instant::now() + SKIP_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(gate) = latest_gate_after(fixture, worker_id, start) {
            if gate["next_due_ms"].as_i64().unwrap_or_default() > 0 {
                return gate;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("board patrol did not persist a skipped gate for {worker_id}");
}

fn assert_direct_message_subscription(status: &Value) {
    assert!(
        status["subscriptions"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|subscription| subscription["event"] == "direct-message"),
        "board patrol cancellation must preserve the direct-message lease: {status}"
    );
}

fn board_revision(board: &Value, task_id: &str) -> u64 {
    board["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|task| task["id"] == task_id)
        .unwrap_or_else(|| panic!("board must list {task_id}: {board}"))["revision"]
        .as_u64()
        .expect("board task has a numeric revision")
}

#[test]
fn board_idle_patrol_runtime_idle_carrier_receives_new_cue() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(alice["name"], "Alice");
    assert_eq!(alice["identity"]["transport"]["kind"], "appserver");
    let worker_id = alice["identity"]["worker_id"].as_str().unwrap().to_owned();

    set_native_status(&fixture, THREAD_A, "active");
    fixture.run_public(&["task", "register", "T1"], THREAD_A);
    set_native_idle(&fixture, THREAD_A);
    let baseline = cue_baseline(&fixture, THREAD_A);

    let gate_start = advance_gate(&mut fixture, &alice, &[]);
    let message_id = wait_for_new_cue(&fixture, THREAD_A, &worker_id, &baseline, CUE_TIMEOUT)
        .expect("idle task owner receives one new board cue");
    let events = journal_events(&fixture);
    assert!(events.iter().any(|event| {
        event["ev"] == "BoardPatrolNoticeBound" && event["message_id"] == message_id.as_str()
    }));
    assert_eq!(
        message_target(&events, &message_id).as_deref(),
        Some(worker_id.as_str())
    );
    assert!(native_input_for(&fixture, THREAD_A, &message_id));

    let gate = latest_gate_after(&fixture, &worker_id, gate_start)
        .expect("idle patrol persists the wake gate");
    assert_eq!(gate["last_wake_tasks"], json!(["T1"]));
    assert_eq!(gate["owner_key"], "Alice");

    let status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(status["board_patrol"]["eligible_task_ids"], json!(["T1"]));
    assert_eq!(status["board_patrol"]["cancelled_tasks"], json!([]));
}

#[test]
fn board_idle_patrol_runtime_busy_carrier_gets_no_cue() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let worker_id = alice["identity"]["worker_id"].as_str().unwrap().to_owned();
    set_native_status(&fixture, THREAD_A, "active");
    fixture.run_public(&["task", "register", "T1"], THREAD_A);
    let baseline = cue_baseline(&fixture, THREAD_A);

    let gate_start = advance_gate(&mut fixture, &alice, &[]);
    let gate = wait_for_skipped_gate(&fixture, &worker_id, gate_start);
    assert_eq!(gate["last_wake_tasks"], json!([]));
    thread::sleep(NEGATIVE_WAIT);
    assert_no_new_cue(&fixture, THREAD_A, &worker_id, &baseline);
}

#[test]
fn board_idle_patrol_runtime_unknown_carrier_gets_no_cue() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let worker_id = alice["identity"]["worker_id"].as_str().unwrap().to_owned();
    set_native_status(&fixture, THREAD_A, "notLoaded");
    fixture.run_public(&["task", "register", "T1"], THREAD_A);
    let baseline = cue_baseline(&fixture, THREAD_A);

    let gate_start = advance_gate(&mut fixture, &alice, &[]);
    let gate = wait_for_skipped_gate(&fixture, &worker_id, gate_start);
    assert_eq!(gate["last_wake_tasks"], json!([]));
    thread::sleep(NEGATIVE_WAIT);
    assert_no_new_cue(&fixture, THREAD_A, &worker_id, &baseline);
}

#[test]
fn board_idle_patrol_runtime_cancel_before_due_keeps_direct_message() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);

    set_native_status(&fixture, THREAD_A, "active");
    fixture.run_public(&["task", "register", "T1"], THREAD_A);
    let cancelled = fixture.run_public(
        &[
            "notify",
            "cancel",
            "--task-id",
            "T1",
            "--reason",
            "waiting for review",
            "--resume-condition",
            "review accepted",
        ],
        THREAD_A,
    );
    assert_eq!(cancelled["cancelled_task_ids"], json!(["T1"]));
    set_native_idle(&fixture, THREAD_A);

    let worker_id = alice["identity"]["worker_id"].as_str().unwrap().to_owned();
    let baseline = cue_baseline(&fixture, THREAD_A);
    advance_gate(&mut fixture, &alice, &["T1"]);
    thread::sleep(NEGATIVE_WAIT);
    assert_no_new_cue(&fixture, THREAD_A, &worker_id, &baseline);

    let status = fixture.run_public(&["notify", "status"], THREAD_A);
    assert_eq!(status["board_patrol"]["cancelled_tasks"], json!(["T1"]));
    assert_eq!(status["board_patrol"]["eligible_task_ids"], json!([]));
    assert_direct_message_subscription(&status);

    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "direct after cancel",
            "ordinary direct message",
        ],
        THREAD_B,
    );
    let direct_id = sent["msg_id"].as_str().unwrap();
    assert_eq!(sent["notification"], "appserver-input-submitted");
    let received = fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "direct-after-patrol-cancel",
            "--timeout",
            "0",
        ],
        THREAD_A,
    );
    assert_eq!(received["count"], 1);
    assert_eq!(received["messages"][0]["id"], direct_id);
    assert_eq!(
        fixture.run_public(&["msg", direct_id], THREAD_A)["state"],
        "read"
    );
}

#[test]
fn board_idle_patrol_runtime_same_name_restoration_routes_only_current_carrier() {
    let mut fixture = AppFixture::new();
    let original = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(original["name"], "Alice");
    fixture.run_public(&["context"], THREAD_B);
    let old_worker = original["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();

    fixture.run_public(
        &[
            "board",
            "publish",
            "T1",
            "--title",
            "Cancelled stable-name work",
            "--description",
            "The same-name carrier must retain this cancellation",
            "--delivery-condition",
            "The task remains cancelled across carrier restoration",
            "--test-condition",
            "The current carrier alone receives the next eligible cue",
        ],
        THREAD_A,
    );
    let published = fixture.run_public(&["board", "show"], THREAD_A);
    let revision = board_revision(&published, "T1").to_string();
    set_native_status(&fixture, THREAD_A, "active");
    fixture.run_public(
        &["board", "claim", "T1", "--expected-revision", &revision],
        THREAD_A,
    );
    fixture.run_public(&["notify", "cancel", "--task-id", "T1"], THREAD_A);

    let restored = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_name","requested_name":"Alice","authorization":"fixture confirmed handoff","replace_confirmed":true}"#,
        ],
        THREAD_B,
    );
    assert_eq!(restored["name"], "Alice");
    assert_eq!(restored["name_resolution"]["outcome"], "committed");
    let current_worker = restored["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(current_worker, old_worker);
    let cancelled_status = fixture.run_public(&["notify", "status"], THREAD_B);
    assert_eq!(
        cancelled_status["board_patrol"]["cancelled_tasks"],
        json!(["T1"])
    );
    assert_eq!(
        cancelled_status["board_patrol"]["eligible_task_ids"],
        json!([])
    );

    set_native_status(&fixture, THREAD_B, "active");
    fixture.run_public(&["task", "register", "T2"], THREAD_B);
    set_native_idle(&fixture, THREAD_B);
    let old_baseline = cue_baseline(&fixture, THREAD_A);
    let current_baseline = cue_baseline(&fixture, THREAD_B);

    advance_gate(&mut fixture, &restored, &["T1"]);
    let message_id = wait_for_new_cue(
        &fixture,
        THREAD_B,
        &current_worker,
        &current_baseline,
        CUE_TIMEOUT,
    )
    .expect("the current same-name carrier receives the eligible task cue");
    let events = journal_events(&fixture);
    assert_eq!(
        message_target(&events, &message_id).as_deref(),
        Some(current_worker.as_str())
    );
    thread::sleep(DUPLICATE_WAIT);
    assert_no_new_cue(&fixture, THREAD_A, &old_worker, &old_baseline);

    let final_status = fixture.run_public(&["notify", "status"], THREAD_B);
    assert_eq!(
        final_status["board_patrol"]["cancelled_tasks"],
        json!(["T1"])
    );
    assert_eq!(
        final_status["board_patrol"]["eligible_task_ids"],
        json!(["T2"])
    );
    assert_eq!(final_status["board_patrol"]["owner_key"], "Alice");
}

#[test]
fn board_idle_patrol_runtime_unconsumed_notice_is_not_retried_and_next_due_is_new() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let worker_id = alice["identity"]["worker_id"].as_str().unwrap().to_owned();
    set_native_status(&fixture, THREAD_A, "active");
    fixture.run_public(&["task", "register", "T1"], THREAD_A);
    set_native_idle(&fixture, THREAD_A);

    let first_baseline = cue_baseline(&fixture, THREAD_A);
    advance_gate(&mut fixture, &alice, &[]);
    let first_id = wait_for_new_cue(&fixture, THREAD_A, &worker_id, &first_baseline, CUE_TIMEOUT)
        .expect("first due occurrence is accepted without being consumed");
    let first_state = fixture.run_public(&["msg", &first_id], THREAD_A)["state"]
        .as_str()
        .unwrap()
        .to_owned();

    set_native_idle(&fixture, THREAD_A);
    let second_baseline = cue_baseline(&fixture, THREAD_A);
    advance_gate(&mut fixture, &alice, &["T1"]);
    let second_id = wait_for_new_cue(
        &fixture,
        THREAD_A,
        &worker_id,
        &second_baseline,
        CUE_TIMEOUT,
    )
    .expect("a later due occurrence creates a new board cue");
    assert_ne!(second_id, first_id);

    let events = journal_events(&fixture);
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event["ev"] == "WakeBound" && event["message_id"] == first_id.as_str()
            })
            .count(),
        1,
        "an accepted unconsumed notice must not be rebound"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event["ev"] == "NotificationDeliveryAccepted"
                    && event["message_id"] == first_id.as_str()
            })
            .count(),
        1,
        "an accepted unconsumed notice must not be re-accepted"
    );
    assert!(
        !events.iter().any(|event| {
            event["ev"] == "ReceiveCommitted"
                && event["ids"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id == first_id.as_str()))
        }),
        "the test must not consume the first notice"
    );
    assert_eq!(
        notice_ids(&events)
            .difference(&second_baseline.notices)
            .cloned()
            .collect::<Vec<_>>(),
        vec![second_id.clone()]
    );
    assert_eq!(
        fixture.run_public(&["msg", &first_id], THREAD_A)["state"],
        first_state,
        "a later occurrence must preserve the first notice delivery status"
    );
}
