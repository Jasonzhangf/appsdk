use super::*;
use std::sync::mpsc::{self, Receiver};

const RECEIVER_TIMEOUT: Duration = Duration::from_secs(10);

struct NativeReceiver {
    process: communication_receiver::ReceivingProcess,
    native: Receiver<Value>,
}

impl NativeReceiver {
    fn start(fixture: &AppFixture, thread: &str) -> Self {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .current_dir(&fixture.project)
            .env("PATH", fixture.fixture_path())
            .env("COLLAB_STATE_DIR", &fixture.host_state)
            .env("HOME", fixture.root.join("home"))
            .env("CODEX_HOME", fixture.root.join("home"))
            .env("COLLAB_APPSERVER_SOCKET", &fixture.app_socket)
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
            .env("CODEX_SESSION_ID", thread)
            .env("CODEX_THREAD_ID", thread)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("COLLAB_WORKER");
        let (tx, native) = mpsc::channel();
        fixture
            .state
            .lock()
            .unwrap()
            .get_mut(thread)
            .unwrap()
            .receiver = Some(tx);
        Self {
            process: communication_receiver::ReceivingProcess::start(command, &binary()),
            native,
        }
    }

    fn event(&mut self, expected: &str) -> Value {
        self.process.event(expected)
    }

    fn release_native(&mut self) -> Value {
        let native = self
            .native
            .recv_timeout(RECEIVER_TIMEOUT)
            .expect("actual accepted native input");
        self.process.submit(&native);
        native
    }
}

#[test]
fn ordinary_self_message_runs_an_independent_receiver_and_real_reply() {
    let fixture = AppFixture::new();
    let identity = fixture.run_public(&["context"], THREAD_A);
    let principal = identity["identity"]["worker_id"].as_str().unwrap();
    let mut receiver = NativeReceiver::start(&fixture, THREAD_A);
    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            principal,
            "--subject",
            "communication-recovery",
            "ordinary real receiver observation",
        ],
        THREAD_A,
    );
    let message_id = sent["msg_id"].as_str().unwrap();
    assert_eq!(
        fixture.run_public(&["msg", message_id], THREAD_A)["consumed_by_recv"],
        false
    );
    let native = receiver.release_native();
    assert_eq!(
        native["params"]["clientUserMessageId"],
        format!("collab-notification-{message_id}")
    );
    let received = receiver.event("received");
    assert_eq!(
        received["receipt"]["receive_id"],
        format!("recover-{message_id}")
    );
    let replied = receiver.event("replied");
    let reply_id = replied["receipt"]["msg_id"].as_str().unwrap();
    let reply = fixture.run_public(&["msg", reply_id], THREAD_A);
    assert_eq!(reply["from"], principal);
    assert_eq!(reply["to"], principal);
    assert_eq!(
        fixture.current_state()["msgs"][reply_id]["in_reply_to"],
        message_id
    );
    assert_eq!(
        fixture.run_public(&["msg", message_id], THREAD_A)["consumed_by_recv"],
        true
    );
    let second_wake = receiver.native.recv_timeout(RECEIVER_TIMEOUT).unwrap();
    assert_eq!(
        second_wake["params"]["clientUserMessageId"],
        format!("collab-notification-{reply_id}")
    );
}

#[test]
fn manual_recovery_presence_only_unknown() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let before = fixture.current_state();
    let response = fixture.command_public(&["context", "--recover-communication"], THREAD_A);
    let response: Value = serde_json::from_slice(&response.stdout).unwrap();
    assert_eq!(
        response["result"]["owner_readback"]["communication"]["state"],
        "unknown"
    );
    assert_eq!(response["result"]["outcome"], "unknown");
    assert_eq!(fixture.current_state()["msgs"], before["msgs"]);
}

fn fail_selected_send(fixture: &AppFixture, principal: &str, known: bool) {
    {
        let mut state = fixture.state.lock().unwrap();
        let thread = state.get_mut(THREAD_A).unwrap();
        thread.reject_input = !known;
        thread.reject_as_missing = known;
    }
    let output = fixture.command_public(
        &[
            "sendmessage",
            "--to",
            principal,
            "--subject",
            "selected-failure-control",
            "retain the original failure",
        ],
        THREAD_A,
    );
    assert!(!output.status.success());
    let current = fixture.current_state();
    let message = current["msgs"]
        .as_object()
        .unwrap()
        .values()
        .find(|message| message["subject"] == "selected-failure-control")
        .unwrap();
    let failure = &current["notification_delivery_failures"][message["id"].as_str().unwrap()];
    assert!(failure.is_object());
    assert_eq!(
        failure["retryable"], known,
        "fixture must establish the typed producer classification"
    );
    let mut state = fixture.state.lock().unwrap();
    let thread = state.get_mut(THREAD_A).unwrap();
    thread.reject_input = false;
    thread.reject_as_missing = false;
}

fn manual_context(fixture: &AppFixture, args: &[&str]) -> Value {
    let output = fixture.command_public(args, THREAD_A);
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "manual Context JSON: {error}; {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn manual_recovery_broken_selected_path_real_receiver() {
    let fixture = AppFixture::new();
    let identity = fixture.run_public(&["context"], THREAD_A);
    let principal = identity["identity"]["worker_id"].as_str().unwrap();
    fail_selected_send(&fixture, principal, true);
    let before_map = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let credential = fixture
        .host_state
        .join("identities")
        .join(principal)
        .join("identity.json");
    let before_credential = std::fs::read(&credential).unwrap();
    let mut receiver = NativeReceiver::start(&fixture, THREAD_A);
    let pending = manual_context(&fixture, &["context", "--recover-communication"]);
    let communication = &pending["result"]["owner_readback"]["communication"];
    assert_eq!(communication["classification"], "broken");
    assert_eq!(communication["state"], "pending");
    assert_eq!(communication["delivery"], "accepted");
    assert!(communication["receive_id"].is_null());
    assert!(communication["reply_message_id"].is_null());
    let id = communication["message_id"].as_str().unwrap();
    let held = manual_context(&fixture, &["context", "--query"]);
    assert_eq!(
        held["result"]["owner_readback"]["communication"]["state"],
        "pending"
    );
    let repeated = manual_context(&fixture, &["context", "--recover-communication"]);
    assert_eq!(
        repeated["result"]["owner_readback"]["communication"]["message_id"],
        id
    );
    assert_eq!(
        std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        before_map
    );
    assert!(
        std::fs::read(&credential).unwrap() == before_credential,
        "manual recovery must preserve exact credentials"
    );
    let native = receiver.release_native();
    assert_eq!(
        native["params"]["clientUserMessageId"],
        format!("collab-notification-{id}")
    );
    receiver.event("received");
    let reply = receiver.event("replied");
    let recovered = manual_context(&fixture, &["context", "--query"]);
    let recovered = &recovered["result"]["owner_readback"]["communication"];
    assert_eq!(recovered["state"], "recovered");
    assert_eq!(recovered["message_id"], id);
    assert_eq!(recovered["receive_id"], format!("recover-{id}"));
    assert_eq!(recovered["reply_message_id"], reply["receipt"]["msg_id"]);
    let current = fixture.current_state();
    assert_eq!(
        current["msgs"]
            .as_object()
            .unwrap()
            .values()
            .filter(|message| message["subject"] == "communication-recovery")
            .count(),
        1
    );
}

#[test]
fn manual_recovery_healthy_no_loop() {
    let fixture = AppFixture::new();
    let identity = fixture.run_public(&["context"], THREAD_A);
    let principal = identity["identity"]["worker_id"].as_str().unwrap();
    let mut receiver = NativeReceiver::start(&fixture, THREAD_A);
    fixture.run_public(
        &[
            "sendmessage",
            "--to",
            principal,
            "--subject",
            "healthy-control",
            "real current delivery and receive",
        ],
        THREAD_A,
    );
    receiver.release_native();
    receiver.event("received");
    let before = fixture.current_state();
    let response = manual_context(&fixture, &["context", "--recover-communication"]);
    assert_eq!(
        response["result"]["owner_readback"]["communication"]["state"],
        "not_needed"
    );
    assert_eq!(response["result"]["outcome"], "completed");
    assert_eq!(fixture.current_state()["msgs"], before["msgs"]);
    assert!(
        receiver.native.try_recv().is_err(),
        "healthy recovery must submit no native input"
    );
}

#[test]
fn manual_recovery_uncertain_failure_does_not_submit() {
    let fixture = AppFixture::new();
    let identity = fixture.run_public(&["context"], THREAD_A);
    let principal = identity["identity"]["worker_id"].as_str().unwrap();
    fail_selected_send(&fixture, principal, false);
    let before = fixture.current_state();
    let response = manual_context(&fixture, &["context", "--recover-communication"]);
    assert_eq!(
        response["result"]["owner_readback"]["communication"]["state"],
        "unknown"
    );
    assert_eq!(fixture.current_state()["msgs"], before["msgs"]);
}

#[test]
fn manual_recovery_ordinary_context_and_restart_do_not_probe() {
    let mut fixture = AppFixture::new();
    let identity = fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_master","authorization":"fixture preserves the current role through manual communication"}"#], THREAD_A);
    let principal = identity["identity"]["worker_id"].as_str().unwrap();
    fixture.run_public(&["task", "register", "communication-owned-work"], THREAD_A);
    fail_selected_send(&fixture, principal, true);
    let before = fixture.current_state();
    let map = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    fixture.run_public(&["context"], THREAD_A);
    fixture.restart_daemon();
    let restored = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(restored["name"], "Jenny");
    assert_eq!(restored["master"]["worker_id"], principal);
    assert_eq!(
        std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        map
    );
    let after = fixture.current_state();
    for field in ["msgs", "tasks", "task_assignments"] {
        assert_eq!(
            after[field], before[field],
            "ordinary restore must preserve current {field}"
        );
    }
    assert!(!after["msgs"]
        .as_object()
        .unwrap()
        .values()
        .any(|message| message["subject"] == "communication-recovery"));
}

#[test]
fn manual_recovery_new_binding_cannot_use_the_old_exchange() {
    let fixture = AppFixture::new();
    let identity = fixture.run_public(&["context"], THREAD_A);
    let principal = identity["identity"]["worker_id"].as_str().unwrap();
    fail_selected_send(&fixture, principal, true);
    let pending = manual_context(&fixture, &["context", "--recover-communication"]);
    let id = pending["result"]["owner_readback"]["communication"]["message_id"]
        .as_str()
        .unwrap();
    let mut next = ThreadState::new();
    next.session_id = Some(THREAD_A.into());
    fixture.state.lock().unwrap().insert(THREAD_B.into(), next);
    let changed = fixture.command_public_with_session(&["context"], THREAD_A, THREAD_B);
    assert!(changed.status.success());
    let changed: Value = serde_json::from_slice(&changed.stdout).unwrap();
    assert_eq!(
        changed["result"]["snapshot"]["identity"]["worker_id"],
        principal
    );
    assert_eq!(
        changed["result"]["snapshot"]["binding"]["native_thread_id"],
        THREAD_B
    );
    let before = fixture.current_state();
    let query = fixture.command_public_with_session(&["context", "--query"], THREAD_A, THREAD_B);
    let query: Value = serde_json::from_slice(&query.stdout).unwrap();
    assert_eq!(
        query["result"]["owner_readback"]["communication"]["message_id"],
        id
    );
    assert_eq!(
        query["result"]["owner_readback"]["communication"]["state"],
        "unknown"
    );
    assert_eq!(fixture.current_state()["msgs"], before["msgs"]);
}

#[test]
fn manual_recovery_unrelated_reply_cannot_complete_a_held_exchange() {
    let fixture = AppFixture::new();
    let identity = fixture.run_public(&["context"], THREAD_A);
    let principal = identity["identity"]["worker_id"].as_str().unwrap();
    fail_selected_send(&fixture, principal, true);
    let mut receiver = NativeReceiver::start(&fixture, THREAD_A);
    let pending = manual_context(&fixture, &["context", "--recover-communication"]);
    let id = pending["result"]["owner_readback"]["communication"]["message_id"]
        .as_str()
        .unwrap();
    fixture.run_public(&["context"], THREAD_B);
    let wrong = fixture.command_public(
        &[
            "sendmessage",
            "--to",
            principal,
            "--subject",
            "communication-recovery-reply",
            "--type",
            "reply",
            "--in-reply-to",
            id,
            "communication recovery received",
        ],
        THREAD_B,
    );
    assert!(
        wrong.status.success(),
        "ordinary cross-peer replies remain permitted"
    );
    let wrong: Value = serde_json::from_slice(&wrong.stdout).unwrap();
    let wrong_id = wrong["msg_id"].as_str().unwrap();
    assert_ne!(fixture.current_state()["msgs"][wrong_id]["from"], principal);
    fixture.run_public(
        &[
            "sendmessage",
            "--to",
            principal,
            "--subject",
            "communication-recovery-reply",
            "--type",
            "reply",
            "communication recovery received",
        ],
        THREAD_A,
    );
    let query = manual_context(&fixture, &["context", "--query"]);
    assert_eq!(
        query["result"]["owner_readback"]["communication"]["state"],
        "pending"
    );
    receiver.release_native();
    receiver.event("received");
    receiver.event("replied");
    let query = manual_context(&fixture, &["context", "--query"]);
    assert_eq!(
        query["result"]["owner_readback"]["communication"]["state"],
        "recovered"
    );
}
