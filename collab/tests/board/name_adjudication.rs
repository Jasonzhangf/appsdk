use super::*;

const THREAD_C: &str = "01a0d639-dafc-7571-a0aa-bbc2bbbc04cf";

fn setup(fixture: &mut AppFixture) -> Value {
    let alice = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"fixture Jenny"}"#,
        ],
        THREAD_B,
    );
    alice
}

#[test]
fn explicit_name_takeover_commits_and_is_idempotent() {
    let mut fixture = AppFixture::new();
    setup(&mut fixture);
    let op = "explicit-name-takeover";
    let resumed = fixture.run_public(&["context", "--op", op, "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Alice","authorization":"fixture inheritance"}"#], THREAD_C);
    assert_eq!(resumed["name"], "Alice", "{resumed}");
    assert_eq!(
        resumed["name_resolution"]["outcome"], "committed",
        "{resumed}"
    );
    let repeat = fixture.run_public(&["context", "--op", op, "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Alice","authorization":"fixture inheritance"}"#], THREAD_C);
    assert_eq!(
        repeat["identity"]["worker_id"],
        resumed["identity"]["worker_id"]
    );
}

#[test]
fn explicit_name_takeover_preserves_unread_message() {
    let mut fixture = AppFixture::new();
    setup(&mut fixture);
    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "handoff",
            "unread message",
        ],
        THREAD_B,
    );
    let id = sent["msg_id"].as_str().unwrap();
    let next = fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Alice","authorization":"confirmed handoff","replace_confirmed":true}"#], THREAD_C);
    assert_eq!(next["name_resolution"]["outcome"], "committed");
    let message = fixture.run_public(&["msg", id], THREAD_C);
    assert_eq!(message["to"], next["identity"]["worker_id"]);
    assert_eq!(message["body"], "unread message");
    assert_eq!(message["state"], "pending");
}

#[test]
fn context_adjudication_uses_current_session_and_inherits_open_tasks_after_replay() {
    let mut fixture = AppFixture::new();
    let alice = setup(&mut fixture);
    let worker = alice["identity"]["worker_id"].as_str().unwrap();
    let mut events = Vec::new();
    for (id, status) in [
        ("inherit-working", "working"),
        ("inherit-accepted", "accepted"),
        ("retain-closed", "closed"),
    ] {
        events.push(json!({"ev":"TaskCreated", "task":{
            "id":id,"owner":worker,"created_by":worker,"status":status,
            "priority":"p2", "created_ms":1,"updated_ms":1
        }}));
        events.push(json!({"ev":"TaskAssignmentUpdated", "assignment":{
            "task_id":id,"name":"Alice","session_id":THREAD_A,"assignment_revision":1,"updated_ms":1
        }}));
    }
    fixture.append_name_events(&events);
    let session = "01a0d639-dafc-7571-a0aa-bbc2bbbc04d0";
    fixture.run_public(&["context"], THREAD_C);
    fixture
        .state
        .lock()
        .unwrap()
        .get_mut(THREAD_C)
        .unwrap()
        .session_id = Some(session.into());
    let output = fixture.command_public_with_session(&["context", "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Alice","authorization":"fixture inheritance"}"#], session, THREAD_C);
    assert!(output.status.success(), "{:?}", output);
    let takeover: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        takeover["result"]["snapshot"]["name_resolution"]["outcome"],
        "committed"
    );
    fixture.restart_daemon();
    let output = fixture.command_public_with_session(&["context"], session, THREAD_C);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result"]["snapshot"]["name"], "Alice", "{value}");
    assert_eq!(value["result"]["snapshot"]["session_id"], session);
    let board = fixture.run_public(&["board", "show"], THREAD_B);
    for id in ["inherit-working", "inherit-accepted"] {
        let task = board["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == id)
            .unwrap();
        assert_ne!(task["owner"], worker, "{task}");
        assert_eq!(task["owner_session_id"], session, "{task}");
    }
    assert!(!board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|task| task["id"] == "retain-closed"));
    let closed = fixture.run_public(&["task", "status", "retain-closed"], THREAD_B);
    assert_eq!(closed["owner"], worker);
    assert_eq!(closed["status"], "closed");
    let journal = std::fs::read_to_string(fixture.name_journal()).unwrap();
    let assignment: Value = journal
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| {
            event["ev"] == "TaskAssignmentUpdated"
                && event["assignment"]["task_id"] == "retain-closed"
        })
        .last()
        .unwrap();
    assert_eq!(assignment["assignment"]["session_id"], THREAD_A);
    let inherited: Value = journal
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["ev"] == "NameResolutionCommitted")
        .last()
        .unwrap();
    assert!(!inherited["assignments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|assignment| assignment["task_id"] == "retain-closed"));
}
