use super::*;

const THREAD_C: &str = "01a0d639-dafc-7571-a0aa-bbc2bbbc04cf";

fn control(fixture: &AppFixture, thread: &str, op: &str, verdict: &str) -> Value {
    let intent = json!({"action":"adjudicate_name", "operation_id":op,
        "verdict":verdict, "evidence":"isolated snapshot observation"})
    .to_string();
    let output = fixture.command_public(&["context", "--approve-grant", &intent], thread);
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{:?}", output))
}

fn pending(fixture: &mut AppFixture) -> String {
    let alice = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"fixture Jenny"}"#,
        ],
        THREAD_B,
    );
    fixture.append_name_events(&[json!({"ev":"AgentPresenceObserved", "presence":{
        "project_scope":std::fs::canonicalize(&fixture.project).unwrap(),
        "worker_id":alice["identity"]["worker_id"], "session_id":THREAD_A,
        "presence":"unknown", "reason":"fixture unknown", "observed_ms":1
    }})]);
    let output = fixture.command_public(&["context", "--op", "adjudication-pending",
        "--approve-grant", r#"{"action":"restore_name","requested_name":"Alice","authorization":"fixture inheritance"}"#], THREAD_C);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["result"]["snapshot"]["name_resolution"]["outcome"], "pending",
        "{value}"
    );
    "adjudication-pending".into()
}

#[test]
fn context_adjudication_offline_resumes_original_operation_and_is_idempotent() {
    let mut fixture = AppFixture::new();
    let op = pending(&mut fixture);
    let verdict = control(&fixture, THREAD_B, &op, "offline");
    assert_eq!(
        verdict["result"]["snapshot"]["name_resolution"]["outcome"], "applied",
        "{verdict}"
    );
    let repeat = control(&fixture, THREAD_B, &op, "offline");
    assert_eq!(
        repeat["result"]["snapshot"]["name_resolution"]["outcome"], "applied",
        "{repeat}"
    );
    let resumed = fixture.run_public(&["context", "--op", &op, "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Alice","authorization":"fixture inheritance"}"#], THREAD_C);
    assert_eq!(resumed["name"], "Alice", "{resumed}");
    let output = fixture.command_public(&["context", "--op", &op, "--query"], THREAD_C);
    let query: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        query["result"]["queried_operation"]["outcome"], "completed",
        "{query}"
    );
    assert_eq!(
        query["result"]["queried_operation"]["name_resolution"]["outcome"], "applied",
        "{query}"
    );
}

#[test]
fn context_adjudication_denies_peer_retains_unknown_online_and_fences_stale() {
    let mut fixture = AppFixture::new();
    let op = pending(&mut fixture);
    let denied = control(&fixture, THREAD_C, &op, "offline");
    assert_eq!(denied["ok"], false, "{denied}");
    assert_eq!(
        denied["result"]["snapshot"]["name_resolution"]["outcome"],
        "failed"
    );
    assert!(
        denied["result"]["owner_readback"]["name_resolution"]["error"]
            .as_str()
            .unwrap()
            .contains("BOARD_MASTER_REQUIRED")
    );
    let unknown = control(&fixture, THREAD_B, &op, "unknown");
    assert_eq!(
        unknown["result"]["snapshot"]["name_resolution"]["outcome"], "pending",
        "{unknown}"
    );
    let online = control(&fixture, THREAD_B, &op, "online");
    assert_eq!(
        online["result"]["snapshot"]["name_resolution"]["outcome"], "needs_confirmation",
        "{online}"
    );
    assert_eq!(fixture.run_public(&["context"], THREAD_A)["name"], "Alice");
    fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Alice","authorization":"confirmed handoff","replace_confirmed":true}"#], THREAD_C);
    let stale = control(&fixture, THREAD_B, &op, "offline");
    assert_eq!(
        stale["result"]["snapshot"]["name_resolution"]["outcome"], "stale_decision",
        "{stale}"
    );
    assert_eq!(fixture.run_public(&["context"], THREAD_C)["name"], "Alice");
}

#[test]
fn context_adjudication_uses_current_session_and_inherits_open_tasks_after_replay() {
    let mut fixture = AppFixture::new();
    let op = pending(&mut fixture);
    let alice = fixture.run_public(&["context"], THREAD_A);
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
    fixture
        .state
        .lock()
        .unwrap()
        .get_mut(THREAD_C)
        .unwrap()
        .session_id = Some(session.into());
    let output = fixture.command_public_with_session(&["context"], session, THREAD_C);
    assert!(output.status.success(), "{:?}", output);
    let verdict = control(&fixture, THREAD_B, &op, "offline");
    assert_eq!(
        verdict["result"]["snapshot"]["name_resolution"]["outcome"], "applied",
        "{verdict}"
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
