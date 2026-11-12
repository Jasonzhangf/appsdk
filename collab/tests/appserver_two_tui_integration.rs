use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[path = "support/communication_receiver.rs"]
mod communication_receiver;
#[path = "support/daemon_readiness.rs"]
mod daemon_readiness;

#[path = "board/block_approval.rs"]
mod block_approval;
#[path = "board/cli.rs"]
mod board_cli;
#[path = "board/central_storage.rs"]
mod central_storage;
#[path = "board/ceo_patrol_runtime.rs"]
mod ceo_patrol_runtime;
#[path = "board/channel_inbound_cli.rs"]
mod channel_inbound_cli;
#[path = "board/context_minimality.rs"]
mod context_minimality;
#[path = "board/degraded_user_maintenance.rs"]
mod degraded_user_maintenance;
#[path = "board/directory_unknown_cli.rs"]
mod directory_unknown_cli;
#[path = "board/dispatch_gap_reminder.rs"]
mod dispatch_gap_reminder;
#[path = "board/manual_communication.rs"]
mod manual_communication;
#[path = "board/name_adjudication.rs"]
mod name_adjudication;
#[path = "board/name_confirmed_takeover.rs"]
mod name_confirmed_takeover;
#[path = "board/name_runtime.rs"]
mod name_runtime_cli;
#[path = "board/name_subscription_lifecycle.rs"]
mod name_subscription_lifecycle;
#[path = "board/patrol_runtime.rs"]
mod patrol_runtime;
#[path = "board/patrol_subscriptions.rs"]
mod patrol_subscriptions;
#[path = "board/project_patrol_subscribe.rs"]
mod project_patrol_subscribe;
#[path = "board/reply_obligations.rs"]
mod reply_obligations_cli;
#[path = "board/work_roles.rs"]
mod work_roles_cli;
#[path = "board/workflow_templates.rs"]
mod workflow_templates;

const THREAD_A: &str = "01a0d637-75e2-71b2-8634-2be0c08a9adc";
const THREAD_B: &str = "01a0d639-dafc-7571-a0aa-bbc2bbbc04ce";

#[test]
fn runtime_restart_public_status_waits_for_a_current_observation() {
    let mut fixture = AppFixture::new();
    let context = fixture.run_public(&["context"], THREAD_A);
    let worker = context["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let project = context["binding"]["project_scope"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture.update_current_state(|state| {
        state["agent_presence"][&project][&worker] = json!({
            "project_scope":project, "worker_id":worker, "session_id":THREAD_A,
            "presence":"online", "reason":"persisted sample before restart", "observed_ms":1
        });
        state["agent_activity"][&project][&worker] = json!({
            "project_scope":project, "worker_id":worker, "session_id":THREAD_A,
            "activity":"idle", "background":false, "reason":"persisted sample before restart",
            "observed_ms":1, "idle_since_ms":1
        });
    });
    let status = fixture.run_public(&["worker", "status", &worker], THREAD_A);
    let row = status["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == worker)
        .unwrap();
    assert_eq!(row["presence"], "unknown", "{status}");
    assert_eq!(row["agent_state"], "unknown", "{status}");
    let state = fixture.current_state();
    assert_eq!(
        state["agent_presence"][&project][&worker]["presence"],
        "unknown"
    );
    assert_eq!(
        state["agent_activity"][&project][&worker]["idle_since_ms"],
        Value::Null
    );
    let recovered = fixture.run_public(&["context", "--query"], THREAD_A);
    assert_eq!(recovered["identity"]["worker_id"], worker, "{recovered}");
    assert_eq!(recovered["name"], context["name"], "{recovered}");
}

#[test]
fn name_mailbox_public_repeated_release_reallocation_keeps_current_carrier() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    let master = r#"{"action":"restore_master","authorization":"fixture requests Jenny"}"#;
    let peer = r#"{"action":"restore_peer","authorization":"fixture releases Jenny"}"#;
    fixture.run_public(&["context", "--approve-grant", master], THREAD_A);
    let history = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Jenny",
            "--subject",
            "history",
            "consumed history",
        ],
        THREAD_B,
    );
    let old_receipt = fixture.run_public(
        &["recv", "--receive-id", "cycle-history", "--timeout", "0"],
        THREAD_A,
    );
    let pending = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Jenny",
            "--subject",
            "cycles",
            "same unread body",
        ],
        THREAD_B,
    );
    let id = pending["msg_id"].as_str().unwrap();
    let mut previous = THREAD_A;
    for current in [THREAD_B, THREAD_A, THREAD_B] {
        fixture.run_public(&["context", "--approve-grant", peer], previous);
        // Replay while Jenny is vacant, when a current-occupancy-only counter is lost.
        fixture.restart_daemon();
        let wake_start = fixture
            .state
            .lock()
            .unwrap()
            .get(current)
            .map_or(0, |s| s.inputs.len());
        let next = fixture.run_public(&["context", "--approve-grant", master], current);
        assert_eq!(next["name"], "Jenny");
        let moved = fixture.run_public(&["msg", id], current);
        assert_eq!(
            moved["to"], next["identity"]["worker_id"],
            "cycle to {current}: {moved}"
        );
        assert_eq!(moved["body"], "same unread body");
        assert_eq!(moved["state"], "pending");
        fixture.restart_daemon();
        assert_eq!(fixture.run_public(&["msg", id], current)["to"], moved["to"]);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let woke = fixture.state.lock().unwrap().get(current).is_some_and(|s| {
                s.inputs[wake_start..]
                    .iter()
                    .any(|input| input.to_string().contains(id))
            });
            if woke {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "message {id} did not wake current carrier {current}"
            );
            thread::sleep(Duration::from_millis(25));
        }
        previous = current;
    }
    let read = fixture.run_public(&["msg", history["msg_id"].as_str().unwrap()], THREAD_B);
    assert_eq!(read["to"], alice["identity"]["worker_id"]);
    assert_eq!(read["state"], "read");
    let replay = fixture.run_public(
        &["recv", "--receive-id", "cycle-history", "--timeout", "0"],
        THREAD_A,
    );
    assert_eq!(replay["messages"], old_receipt["messages"]);
    let received = fixture.run_public(
        &["recv", "--receive-id", "cycle-final", "--timeout", "0"],
        THREAD_B,
    );
    assert_eq!(received["count"], 1);
    assert_eq!(received["messages"][0]["id"], id);
    fixture.restart_daemon();
    assert_eq!(fixture.run_public(&["msg", id], THREAD_B)["state"], "read");
}

#[test]
fn context_query_reports_published_registration_with_pending_business_commit() {
    use std::os::unix::fs::PermissionsExt;
    const THREAD_C: &str = "01a0d63b-2f2c-72cf-93c1-2a4a1e2cf1cc";
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    let path = fixture.project_state_for(&fixture.project, "appserver-cli");
    let directory = path.parent().unwrap();
    let permissions = std::fs::metadata(directory).unwrap().permissions();
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o500)).unwrap();
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let attempted =
            fixture.command_public(&["context", "--op", "register-pending-query"], THREAD_C);
        assert!(!attempted.status.success());
        let map: Value = serde_json::from_slice(
            &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        )
        .unwrap();
        let current = map["slots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|slot| slot["identity_key"]["name"] == "Carol")
            .unwrap();
        assert!(
            !current["current"].is_null(),
            "the name publication must precede this failure: {map}"
        );
        let worker = current["current"]["binding"]["agent_id"].as_str().unwrap();
        let generation = current["generation"].as_u64().unwrap();
        assert!(fixture.current_state()["workers"][worker].is_null());
        let map_before_query = fixture.host_state.join("identity-map.json");
        let map_bytes = std::fs::read(&map_before_query).unwrap();
        let query = fixture.command_public(
            &["context", "--query", "--op", "register-pending-query"],
            THREAD_C,
        );
        assert!(
            query.status.success(),
            "query must show the published current and pending effects: stdout={} stderr={}",
            String::from_utf8_lossy(&query.stdout),
            String::from_utf8_lossy(&query.stderr)
        );
        let result: Value = serde_json::from_slice(&query.stdout).unwrap();
        assert_eq!(result["result"]["snapshot"]["name"], "Carol", "{result}");
        assert_eq!(
            result["result"]["snapshot"]["identity"]["worker_id"], worker,
            "{result}"
        );
        assert_eq!(result["result"]["outcome"], "partial", "{result}");
        assert_eq!(
            result["result"]["phase"], "business_effects_pending",
            "{result}"
        );
        let pending = result["result"]["owner_readback"]["pending_effects"]
            .as_array()
            .unwrap();
        assert!(
            pending
                .iter()
                .any(|effect| effect["kind"] == "register" && effect["target_worker_id"] == worker),
            "{result}"
        );
        let unrelated =
            fixture.command_public(&["context", "--query"], "pending-query-unrelated-caller");
        assert!(
            !unrelated.status.success(),
            "the partial projection must stay scoped to the mapped caller: stdout={}",
            String::from_utf8_lossy(&unrelated.stdout)
        );
        assert!(
            String::from_utf8_lossy(&unrelated.stderr).contains("IDENTITY_MAP_NOT_FOUND"),
            "an unrelated caller keeps the existing rejection: stderr={}",
            String::from_utf8_lossy(&unrelated.stderr)
        );
        assert_eq!(
            std::fs::read(&map_before_query).unwrap(),
            map_bytes,
            "the read-only query must not rewrite the published map"
        );
        assert!(
            fixture.current_state()["workers"][worker].is_null(),
            "the read-only query must not invent a State worker"
        );
        (worker.to_owned(), generation)
    }));
    std::fs::set_permissions(directory, permissions).unwrap();
    let (worker, generation) = failed.unwrap();
    let recovered = fixture.run_public(&["context", "--op", "register-pending-query"], THREAD_C);
    assert_eq!(recovered["name"], "Carol");
    assert_eq!(recovered["identity"]["worker_id"], worker);
    let map_after: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let current_after = map_after["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["identity_key"]["name"] == "Carol")
        .unwrap();
    assert_eq!(
        current_after["generation"], generation,
        "the retry must preserve the published map generation"
    );
}

#[test]
fn name_mailbox_public_distinct_names_follow_address_after_release() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    let old_worker = alice["identity"]["worker_id"].clone();
    let jenny_owner = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"isolated fixture requests Jenny"}"#,
        ],
        THREAD_A,
    );
    assert_eq!(jenny_owner["name"], "Jenny");
    let history = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Jenny",
            "--subject",
            "history",
            "consumed history",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &["recv", "--receive-id", "before-rename", "--timeout", "0"],
        THREAD_A,
    );
    let jenny = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Jenny",
            "--subject",
            "same",
            "same body",
        ],
        THREAD_B,
    );
    let peer = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_peer","authorization":"isolated fixture returns to peer"}"#,
        ],
        THREAD_A,
    );
    assert_eq!(peer["name"], "Alice");
    let alice_message = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "same",
            "same body",
        ],
        THREAD_B,
    );
    assert_ne!(jenny["msg_id"], alice_message["msg_id"]);
    // Only this fixture's child restarts; the name association must replay.
    fixture.restart_daemon();
    let next = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"isolated fixture requests Jenny"}"#,
        ],
        THREAD_B,
    );
    assert_eq!(next["name"], "Jenny");
    let id = jenny["msg_id"].as_str().unwrap();
    let moved = fixture.run_public(&["msg", id], THREAD_B);
    assert_eq!(moved["to"], next["identity"]["worker_id"]);
    assert_eq!(moved["body"], "same body");
    assert_eq!(
        moved["from"],
        fixture.run_public(
            &["msg", alice_message["msg_id"].as_str().unwrap()],
            THREAD_B
        )["from"]
    );
    let read = fixture.run_public(&["msg", history["msg_id"].as_str().unwrap()], THREAD_B);
    assert_eq!(read["to"], old_worker);
    assert_eq!(read["state"], "read");
    let received = fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "jenny-after-release",
            "--timeout",
            "0",
        ],
        THREAD_B,
    );
    assert_eq!(received["count"], 1);
    assert_eq!(received["messages"][0]["id"], id);
    let replay = fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "jenny-after-release",
            "--timeout",
            "0",
        ],
        THREAD_B,
    );
    assert_eq!(replay["messages"], received["messages"]);
    let retained = fixture.run_public(
        &["recv", "--receive-id", "alice-retained", "--timeout", "0"],
        THREAD_A,
    );
    assert_eq!(retained["count"], 1);
    assert_eq!(retained["messages"][0]["id"], alice_message["msg_id"]);
    assert_eq!(fixture.run_public(&["msg", id], THREAD_B)["state"], "read");
}

#[test]
fn name_mailbox_public_handoff_recv_preserves_old_consumed_history() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    let read = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "already read",
            "history",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &["recv", "--receive-id", "old-read", "--timeout", "0"],
        THREAD_A,
    );
    let pending = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "handoff",
            "keep this body",
        ],
        THREAD_B,
    );
    let id = pending["msg_id"].as_str().unwrap();
    assert_eq!(pending["consumed"], false);
    let project = std::fs::canonicalize(&fixture.project)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    fixture.update_current_state(|state| state["agent_presence"][&project][alice["identity"]["worker_id"].as_str().unwrap()] = json!({
        "project_scope":project,
        "worker_id":alice["identity"]["worker_id"], "session_id":THREAD_A,
        "presence":"online", "reason":"isolated fixture", "observed_ms":1
    }));
    let next = fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Alice","authorization":"fixture confirmed handoff","replace_confirmed":true}"#], THREAD_B);
    let moved = fixture.run_public(&["msg", id], THREAD_B);
    assert_eq!(moved["to"], next["identity"]["worker_id"]);
    assert_eq!(moved["body"], "keep this body");
    assert_eq!(moved["state"], "pending");
    assert_eq!(
        fixture.run_public(&["msg", read["msg_id"].as_str().unwrap()], THREAD_B)["to"],
        alice["identity"]["worker_id"]
    );
    let received = fixture.run_public(
        &["recv", "--receive-id", "new-read", "--timeout", "0"],
        THREAD_B,
    );
    assert_eq!(received["count"], 1);
    assert_eq!(received["messages"][0]["id"], id);
    assert_eq!(fixture.run_public(&["msg", id], THREAD_B)["state"], "read");
    let replay = fixture.run_public(
        &["recv", "--receive-id", "new-read", "--timeout", "0"],
        THREAD_B,
    );
    assert_eq!(replay["messages"], received["messages"]);
    assert_eq!(
        fixture.run_public(
            &[
                "recv",
                "--receive-id",
                "old-after-handoff",
                "--timeout",
                "0"
            ],
            THREAD_A
        )["count"],
        0
    );
}

#[test]
fn name_address_public_send_recv_reply() {
    let fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(alice["name"], "Alice");
    let jenny = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"fixture user requests Jenny"}"#,
        ],
        THREAD_B,
    );
    assert_eq!(jenny["name"], "Jenny");

    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "named delivery",
            "hello Alice",
        ],
        THREAD_B,
    );
    let message_id = sent["msg_id"].as_str().unwrap();
    assert_eq!(sent["notification"], "appserver-input-submitted");
    assert_eq!(sent["consumed"], false);
    let durable = fixture.run_public(&["msg", message_id], THREAD_B);
    assert_eq!(durable["to"], alice["identity"]["worker_id"]);
    assert_eq!(durable["state"], "pending");
    let received = fixture.run_public(
        &["recv", "--receive-id", "named-alice", "--timeout", "0"],
        THREAD_A,
    );
    assert_eq!(received["count"], 1);
    assert_eq!(received["messages"][0]["id"], message_id);
    assert_eq!(
        fixture.run_public(&["msg", message_id], THREAD_B)["state"],
        "read"
    );

    let reply = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Jenny",
            "--subject",
            "named reply",
            "--in-reply-to",
            message_id,
            "hello Jenny",
        ],
        THREAD_A,
    );
    assert_eq!(reply["notification"], "appserver-input-submitted");
    assert_eq!(reply["consumed"], false);
    let received = fixture.run_public(
        &["recv", "--receive-id", "named-jenny", "--timeout", "0"],
        THREAD_B,
    );
    assert_eq!(received["count"], 1);
    assert_eq!(received["messages"][0]["id"], reply["msg_id"]);
    assert_eq!(
        received["messages"][0]["to"],
        jenny["identity"]["worker_id"]
    );
    assert_eq!(received["messages"][0]["in_reply_to"], message_id);
    assert_eq!(
        fixture.run_public(&["msg", reply["msg_id"].as_str().unwrap()], THREAD_A)["state"],
        "read"
    );
}

#[test]
fn context_names_allocate_idempotently_and_restore_jenny_with_authorization() {
    let fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(first["name"], "Alice", "{first}");
    let repeat = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(repeat["name"], "Alice");
    assert_eq!(
        repeat["identity"]["worker_id"],
        first["identity"]["worker_id"]
    );
    let second = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(second["name"], "Bob");
    let master = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"user asked to restore Jenny"}"#,
        ],
        THREAD_A,
    );
    assert_eq!(master["name"], "Jenny", "{master}");
    assert_eq!(
        master["master"]["worker_id"],
        first["identity"]["worker_id"]
    );
}

#[test]
fn context_names_explicit_restore_commits_directly_and_query_is_read_only() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);
    let output = fixture.command_public(&["context", "--op", "names-restore",
        "--approve-grant", r#"{"action":"restore_name","requested_name":"Alice","authorization":"restore my named work"}"#], THREAD_B);
    assert!(
        output.status.success(),
        "authorized name handoff: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // An explicit authorized name change commits directly even when another
    // carrier holds the target online; there is no second confirmation.
    let restored: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(restored["ok"], true, "{restored}");
    assert_eq!(restored["result"]["snapshot"]["name"], "Alice");
    assert_eq!(
        restored["result"]["snapshot"]["identity"]["worker_id"],
        bob["identity"]["worker_id"]
    );
    let control: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let slots = control["slots"].as_array().unwrap();
    let current = slots
        .iter()
        .find(|slot| slot["identity_key"]["name"] == "Alice" && !slot["current"].is_null())
        .expect("Alice has a current binding");
    assert_eq!(
        current["current"]["binding"]["agent_id"], bob["identity"]["worker_id"],
        "control must follow the explicitly selected name: {current}"
    );
    assert!(
        slots
            .iter()
            .filter(|slot| !slot["current"].is_null())
            .all(|slot| slot["identity_key"]["name"] == "Alice"
                || slot["current"]["binding"]["agent_id"] != bob["identity"]["worker_id"]),
        "the same carrier must not retain its previous active name: {slots:?}"
    );
    fixture.restart_daemon();
    let current_map = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let current_routes = std::fs::read(fixture.host_state.join("routes.json")).unwrap();
    let current_workers = fixture.current_state()["workers"].clone();
    let query = fixture.command_public(&["context", "--op", "names-restore", "--query"], THREAD_B);
    assert!(query.status.success());
    let query: Value = serde_json::from_slice(&query.stdout).unwrap();
    assert_eq!(query["result"]["outcome"], "completed", "{query}");
    assert_eq!(query["result"]["snapshot"]["name"], "Alice");
    assert_eq!(
        std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        current_map
    );
    assert_eq!(
        std::fs::read(fixture.host_state.join("routes.json")).unwrap(),
        current_routes
    );
    assert_eq!(fixture.current_state()["workers"], current_workers);
    // The displaced carrier registers normally when it returns. Its previous
    // worker remains historical business state while Alice stays with Bob's
    // explicitly approved successor.
    let after = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(after["name"], "Bob", "{after}");
    assert_ne!(after["identity"]["worker_id"], bob["identity"]["worker_id"]);
    let old_worker = alice["identity"]["worker_id"].as_str().unwrap();
    assert!(!fixture.current_state()["workers"][old_worker].is_null());
    let successor = fixture.run_public(&["context", "--query"], THREAD_B);
    assert_eq!(successor["name"], "Alice", "{successor}");
    assert_eq!(
        successor["identity"]["worker_id"],
        bob["identity"]["worker_id"]
    );
}

#[test]
fn context_names_authorized_new_session_continues_named_open_tasks_and_preserves_closed_assignment()
{
    let mut fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    let worker = first["identity"]["worker_id"].as_str().unwrap();
    for id in ["open-a", "open-b", "closed"] {
        fixture.run_public(
            &["task", "register", id, "--next", "fixture continuation"],
            THREAD_A,
        );
    }
    fixture.update_current_state(|state| {
    for (id, status) in [
        ("open-a", "working"),
        ("open-b", "accepted"),
        ("closed", "closed"),
    ] {
        state["tasks"][id]["status"] = json!(status);
        state["task_assignments"][id] = json!({
            "task_id":id,"name":"Alice","session_id":THREAD_A,"assignment_revision":1,"updated_ms":1
        });
    }
    });
    let closed = fixture.current_state()["task_assignments"]["closed"].clone();
    let closed_task = fixture.current_state()["tasks"]["closed"].clone();
    let second = fixture.run_public(&["context"], THREAD_B);
    assert_ne!(second["identity"]["worker_id"], worker);
    assert_ne!(second["name"], "Alice");
    for id in ["open-a", "open-b"] {
        assert_eq!(fixture.current_state()["tasks"][id]["owner"], worker);
    }
    let output = fixture.command_public(&["context", "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Alice","authorization":"user continues named work in the new session"}"#], THREAD_B);
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["ok"], true, "{result}");
    let snapshot = &result["result"]["snapshot"];
    assert_eq!(
        snapshot["identity"]["worker_id"],
        second["identity"]["worker_id"]
    );
    // The authorized current name owns open work across different carriers.
    assert_eq!(snapshot["name"], "Alice", "{snapshot}");
    assert_eq!(snapshot["session_id"], THREAD_B, "{snapshot}");
    assert_eq!(snapshot["binding"]["session_id"], THREAD_B);
    let state = fixture.current_state();
    for id in ["open-a", "open-b"] {
        let assignment = &state["task_assignments"][id];
        assert_eq!(assignment["name"], "Alice");
        assert_eq!(assignment["session_id"], THREAD_B);
        assert_eq!(assignment["assignment_revision"], 2);
        assert_eq!(state["tasks"][id]["owner"], second["identity"]["worker_id"]);
    }
    assert_eq!(state["task_assignments"]["closed"], closed);
    assert_eq!(state["tasks"]["closed"], closed_task);
    let displaced = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(displaced["identity"]["worker_id"], worker);
    assert_ne!(displaced["name"], "Alice");
    fixture.restart_daemon();
    let restored = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(restored["name"], "Alice");
    let after = fixture.current_state();
    for id in ["open-a", "open-b"] {
        assert_eq!(after["task_assignments"][id], state["task_assignments"][id]);
        assert_eq!(after["tasks"][id], state["tasks"][id]);
    }
    assert_eq!(after["task_assignments"]["closed"], closed);
    assert_eq!(after["tasks"]["closed"], closed_task);
}

#[test]
fn context_names_same_name_new_session_continues_open_task_and_old_carrier_cannot_commit() {
    let mut fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(first["name"], "Alice", "{first}");
    let task_id = "same-name-continuation";
    fixture.run_public(
        &["task", "register", task_id, "--next", "continue named work"],
        THREAD_A,
    );
    let second = fixture.run_public(&["context"], THREAD_B);
    assert_ne!(second["name"], "Alice", "{second}");
    let restored = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_name","requested_name":"Alice","authorization":"user continues named work in the new session"}"#,
        ],
        THREAD_B,
    );
    assert_eq!(restored["name"], "Alice", "{restored}");
    assert_eq!(
        restored["identity"]["worker_id"],
        second["identity"]["worker_id"]
    );
    let state = fixture.current_state();
    assert_eq!(state["task_assignments"][task_id]["name"], "Alice");
    assert_eq!(
        state["task_assignments"][task_id]["session_id"], THREAD_B,
        "the open task follows the current name holder's session"
    );
    assert_eq!(
        state["tasks"][task_id]["owner"], second["identity"]["worker_id"],
        "the current name holder becomes the task owner"
    );
    let continued = fixture.run_public(
        &[
            "task",
            "update",
            task_id,
            "--next",
            "continue in the new session",
        ],
        THREAD_B,
    );
    assert_eq!(continued["owner"], second["identity"]["worker_id"]);
    let baseline = fixture.current_state();
    let rejected = fixture.command_public(
        &[
            "task",
            "update",
            task_id,
            "--next",
            "the displaced carrier must not commit work",
        ],
        THREAD_A,
    );
    assert!(
        !rejected.status.success(),
        "the displaced carrier must not continue the retained task: stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    let after = fixture.current_state();
    assert_eq!(
        after["tasks"][task_id], baseline["tasks"][task_id],
        "a rejected old-carrier update must not mutate the task"
    );
    assert_eq!(
        after["task_assignments"][task_id], baseline["task_assignments"][task_id],
        "a rejected old-carrier update must not move the assignment"
    );
}

#[test]
fn context_names_map_first_task_mutation_denies_old_holder_before_effect_commit() {
    use std::os::unix::fs::PermissionsExt;

    let mut fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(first["name"], "Alice", "{first}");
    let task_id = "map-first-task";
    fixture.run_public(
        &[
            "task",
            "register",
            task_id,
            "--next",
            "continue after map publication",
        ],
        THREAD_A,
    );
    let second = fixture.run_public(&["context"], THREAD_B);
    assert_ne!(second["name"], "Alice", "{second}");
    let before = fixture.current_state();
    let state_path = fixture.project_state_for(&fixture.project, "appserver-cli");
    let directory = state_path.parent().unwrap();
    let permissions = std::fs::metadata(directory).unwrap().permissions();
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o500)).unwrap();
    let window_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let attempted = fixture.command_public(
            &[
                "context",
                "--op",
                "task-map-first-pending",
                "--approve-grant",
                r#"{"action":"restore_name","requested_name":"Alice","authorization":"fixture user authorizes Alice takeover"}"#,
            ],
            THREAD_B,
        );
        assert!(!attempted.status.success());
        assert!(
            String::from_utf8_lossy(&attempted.stderr)
                .contains("CONTROL_COMMITTED_EFFECTS_PENDING"),
            "the interrupted context must leave a real pending effect: {}",
            String::from_utf8_lossy(&attempted.stderr)
        );
        let map: Value = serde_json::from_slice(
            &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        )
        .unwrap();
        let current = map["slots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|slot| slot["identity_key"]["name"] == "Alice")
            .unwrap();
        assert_eq!(
            current["current"]["binding"]["agent_id"], second["identity"]["worker_id"],
            "the current map publishes before the business effect commits"
        );
        assert!(map["effects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|effect| effect["operation_id"] == "task-map-first-pending"));
        let failed_state = fixture.current_state();
        assert_eq!(
            failed_state["tasks"][task_id], before["tasks"][task_id],
            "the interrupted context must not update the task before recovery"
        );
        assert_eq!(
            failed_state["task_assignments"][task_id], before["task_assignments"][task_id],
            "the interrupted context must not rebase the assignment before recovery"
        );
        let old_holder = fixture.command_public(
            &[
                "task",
                "update",
                task_id,
                "--next",
                "the old holder must lose authority as soon as the map publishes",
            ],
            THREAD_A,
        );
        assert!(
            !old_holder.status.success(),
            "the old holder must be denied before the pending effect completes: stdout={} stderr={}",
            String::from_utf8_lossy(&old_holder.stdout),
            String::from_utf8_lossy(&old_holder.stderr)
        );
        let after_denial = fixture.current_state();
        assert_eq!(
            after_denial["tasks"][task_id], before["tasks"][task_id],
            "the denied old-holder mutation must not change the task"
        );
        assert_eq!(
            after_denial["task_assignments"][task_id], before["task_assignments"][task_id],
            "the denied old-holder mutation must not move the assignment"
        );
    }));
    std::fs::set_permissions(directory, permissions).unwrap();
    window_result.unwrap();

    let resumed = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(resumed["name"], "Alice", "{resumed}");
    let recovered = fixture.current_state();
    assert_eq!(
        recovered["task_assignments"][task_id]["session_id"],
        THREAD_B
    );
    assert_eq!(
        recovered["tasks"][task_id]["owner"],
        second["identity"]["worker_id"]
    );
    let continued = fixture.run_public(
        &[
            "task",
            "update",
            task_id,
            "--next",
            "new holder continues after recovery",
        ],
        THREAD_B,
    );
    assert_eq!(continued["owner"], second["identity"]["worker_id"]);
}

#[test]
fn task_register_current_owner_records_stable_name_and_session() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let task_id = "task-register-current-owner";
    let registered = fixture.run_public(
        &[
            "task",
            "register",
            task_id,
            "--next",
            "fixture task register assignment",
        ],
        THREAD_A,
    );
    assert_eq!(registered["task"], task_id);
    let state = fixture.current_state();
    assert_eq!(
        state["tasks"][task_id]["owner"],
        alice["identity"]["worker_id"]
    );
    assert_eq!(state["task_assignments"][task_id]["name"], alice["name"]);
    assert_eq!(state["task_assignments"][task_id]["session_id"], THREAD_A);

    let map: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let slot = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| {
            slot["identity_status"] == "active" && slot["identity_key"]["name"] == alice["name"]
        })
        .expect("registered stable name has a current slot");
    assert_eq!(
        slot["current"]["binding"]["agent_id"],
        alice["identity"]["worker_id"]
    );
    assert_eq!(slot["current"]["binding"]["session_id"], THREAD_A);
    drop(fixture);
}

#[test]
fn context_names_online_replacement_approves_directly() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    let project = std::fs::canonicalize(&fixture.project)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    fixture.update_current_state(|state| state["agent_presence"][&project][alice["identity"]["worker_id"].as_str().unwrap()] = json!({
        "project_scope":project,
        "worker_id":alice["identity"]["worker_id"], "session_id":THREAD_A,
        "presence":"online", "reason":"isolated current agent fact", "observed_ms":1
    }));
    let before = fixture.current_state();
    let output = fixture.command_public(&["context", "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Alice","authorization":"restore my work"}"#], THREAD_B);
    assert!(
        output.status.success(),
        "online name handoff: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let restored: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(restored["ok"], true, "{restored}");
    assert_eq!(restored["result"]["snapshot"]["name"], "Alice");
    let restored = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(restored["name"], "Alice", "{restored}");
    let after = fixture.current_state();
    let old_worker = alice["identity"]["worker_id"].as_str().unwrap();
    assert_eq!(after["workers"][old_worker], before["workers"][old_worker]);
    assert_eq!(after["global"]["projects"], before["global"]["projects"]);
}

#[test]
fn context_names_full_pool_allocates_unique_generated_names_and_reloads_them() {
    let mut fixture = AppFixture::new();
    let mut contexts = Vec::new();
    for index in 0..20 {
        let thread = format!("ordinary-pool-{index}");
        let context = fixture.run_public(&["context"], &thread);
        contexts.push((thread, context));
    }
    let first_worker = contexts[0].1["identity"]["worker_id"].clone();
    let incoming_worker = contexts[1].1["identity"]["worker_id"].clone();
    let target = contexts[0].1["name"].as_str().unwrap();
    let released = contexts[1].1["name"].clone();
    let approval = json!({"action":"restore_name","requested_name":target,
        "authorization":"user continues the selected name with the full pool"})
    .to_string();
    let restored = fixture.run_public(&["context", "--approve-grant", &approval], &contexts[1].0);
    assert_eq!(restored["identity"]["worker_id"], incoming_worker);
    assert_eq!(restored["name"], target);
    let displaced = fixture.run_public(&["context"], &contexts[0].0);
    assert_eq!(displaced["identity"]["worker_id"], first_worker);
    assert_eq!(displaced["name"], released);
    let map_path = fixture.host_state.join("identity-map.json");
    let map: Value = serde_json::from_slice(&std::fs::read(&map_path).unwrap()).unwrap();
    assert_eq!(
        map["slots"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|slot| slot["identity_status"] == "active")
            .count(),
        20
    );
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "user assigns the pool master",
        ],
        &contexts[19].0,
    );
    fixture.run_public(&["context"], "ordinary-pool-extra");
    let before_generation: Value =
        serde_json::from_slice(&std::fs::read(&map_path).unwrap()).unwrap();
    let active_before_generation = before_generation["slots"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|slot| slot["identity_status"] == "active")
        .count();
    assert_eq!(active_before_generation, 21);
    let generated_a = fixture.spawn_public(&["context"], "ordinary-pool-generated-a");
    let generated_b = fixture.spawn_public(&["context"], "ordinary-pool-generated-b");
    let output_a = generated_a.wait_with_output().unwrap();
    let output_b = generated_b.wait_with_output().unwrap();
    assert!(
        output_a.status.success(),
        "{}",
        String::from_utf8_lossy(&output_a.stderr)
    );
    assert!(
        output_b.status.success(),
        "{}",
        String::from_utf8_lossy(&output_b.stderr)
    );
    let generated_a: Value = serde_json::from_slice(&output_a.stdout).unwrap();
    let generated_a = generated_a["result"]["snapshot"].clone();
    let generated_b: Value = serde_json::from_slice(&output_b.stdout).unwrap();
    let generated_b = generated_b["result"]["snapshot"].clone();
    let mut generated_names = [
        generated_a["name"].as_str().unwrap(),
        generated_b["name"].as_str().unwrap(),
    ];
    generated_names.sort_unstable();
    assert_eq!(generated_names, ["peer-1", "peer-2"]);
    assert_ne!(
        generated_a["identity"]["worker_id"],
        generated_b["identity"]["worker_id"]
    );

    fixture.restart_daemon();
    let recovered_a = fixture.run_public(&["context"], "ordinary-pool-generated-a");
    let recovered_b = fixture.run_public(&["context"], "ordinary-pool-generated-b");
    for (before, after) in [(&generated_a, &recovered_a), (&generated_b, &recovered_b)] {
        assert_eq!(after["name"], before["name"]);
        assert_eq!(
            after["identity"]["worker_id"],
            before["identity"]["worker_id"]
        );
    }
    let master = fixture.run_public(&["master", "status"], &contexts[19].0);
    assert_eq!(
        master["master"]["worker_id"],
        contexts[19].1["identity"]["worker_id"]
    );
    let map: Value = serde_json::from_slice(&std::fs::read(&map_path).unwrap()).unwrap();
    let entries = map["slots"].as_array().unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|slot| slot["identity_status"] == "active")
            .count(),
        active_before_generation + 2
    );
    for (name, session_id) in [
        (target, &contexts[1].0),
        (released.as_str().unwrap(), &contexts[0].0),
    ] {
        let slot = entries
            .iter()
            .find(|slot| {
                slot["identity_status"] == "active" && slot["identity_key"]["name"] == name
            })
            .unwrap_or_else(|| panic!("missing active identity {name}"));
        assert_eq!(&slot["current"]["binding"]["session_id"], session_id);
    }
    for name in ["Alice", "Tina", "peer-1", "peer-2"] {
        let slot = entries
            .iter()
            .find(|slot| {
                slot["identity_status"] == "active" && slot["identity_key"]["name"] == name
            })
            .unwrap_or_else(|| panic!("missing active identity {name}"));
        assert_eq!(slot["display_slot"].is_null(), name.starts_with("peer-"));
        assert_eq!(
            entries
                .iter()
                .filter(|slot| slot["identity_status"] == "active"
                    && slot["identity_key"]["name"] == name)
                .count(),
            1,
            "{name}"
        );
    }
}

#[test]
fn context_names_takeover_keeps_source_name_work_and_continues_target_name_work() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(
        &[
            "task",
            "register",
            "source-alice-work",
            "--next",
            "keep Alice work",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "task",
            "register",
            "target-bob-work",
            "--next",
            "continue Bob work",
        ],
        THREAD_B,
    );
    fixture.update_current_state(|state| {
        for (id, name, session) in [
            ("source-alice-work", "Alice", THREAD_A),
            ("target-bob-work", "Bob", THREAD_B),
        ] {
            state["task_assignments"][id] = json!({
                "task_id": id,
                "name": name,
                "session_id": session,
                "assignment_revision": 1,
                "updated_ms": 1
            });
        }
    });
    let before = fixture.current_state();
    assert_eq!(
        before["task_assignments"]["source-alice-work"]["name"],
        "Alice"
    );
    assert_eq!(before["task_assignments"]["target-bob-work"]["name"], "Bob");

    let takeover = fixture.run_public(
        &["context", "--op", "r8-source-work-stays", "--approve-grant",
            r#"{"action":"restore_name","requested_name":"Bob","authorization":"fixture user authorizes Bob takeover"}"#],
        THREAD_A,
    );
    assert_eq!(takeover["name"], "Bob");
    assert_eq!(
        takeover["identity"]["worker_id"],
        alice["identity"]["worker_id"]
    );
    assert!(takeover["master"].is_null());
    let after = fixture.current_state();
    let source_assignment = &after["task_assignments"]["source-alice-work"];
    let before_source_assignment = &before["task_assignments"]["source-alice-work"];
    assert_eq!(
        source_assignment["task_id"], before_source_assignment["task_id"],
        "source-name work stays with its original task when the carrier takes Bob"
    );
    assert_eq!(
        source_assignment["name"], before_source_assignment["name"],
        "the source assignment keeps the Alice name"
    );
    assert_eq!(
        source_assignment["session_id"], before_source_assignment["session_id"],
        "the source assignment keeps its original execution carrier"
    );
    assert_eq!(
        after["tasks"]["source-alice-work"]["owner"],
        alice["identity"]["worker_id"]
    );
    assert_eq!(
        after["tasks"]["source-alice-work"]["status"],
        before["tasks"]["source-alice-work"]["status"]
    );
    assert_eq!(after["task_assignments"]["target-bob-work"]["name"], "Bob");
    assert_eq!(
        after["tasks"]["target-bob-work"]["owner"],
        alice["identity"]["worker_id"]
    );
    assert_ne!(
        after["tasks"]["target-bob-work"]["owner"],
        bob["identity"]["worker_id"]
    );
    let source_update = fixture.command_public(
        &[
            "task",
            "update",
            "source-alice-work",
            "--status",
            "working",
            "--next",
            "must stay with Alice",
        ],
        THREAD_A,
    );
    assert!(
        !source_update.status.success(),
        "the current Bob holder cannot execute Alice's retained work: {}",
        String::from_utf8_lossy(&source_update.stdout)
    );
    assert_eq!(
        fixture.current_state()["tasks"]["source-alice-work"],
        after["tasks"]["source-alice-work"]
    );
    let source_next = fixture.command_public(
        &[
            "task",
            "update",
            "source-alice-work",
            "--next",
            "next-only must stay with Alice",
        ],
        THREAD_A,
    );
    assert!(
        !source_next.status.success(),
        "a next-step-only update must use the same current-name owner gate: stdout={} stderr={}",
        String::from_utf8_lossy(&source_next.stdout),
        String::from_utf8_lossy(&source_next.stderr)
    );
    assert!(
        String::from_utf8_lossy(&source_next.stderr).contains("TASK_OWNER_REQUIRED"),
        "next-only rejection must be the owner gate: {}",
        String::from_utf8_lossy(&source_next.stderr)
    );
    assert_eq!(
        fixture.current_state()["tasks"]["source-alice-work"],
        after["tasks"]["source-alice-work"]
    );
    let source_block = fixture.command_public(
        &[
            "task",
            "update",
            "source-alice-work",
            "--status",
            "blocked",
            "--next",
            "block application must stay with Alice",
        ],
        THREAD_A,
    );
    assert!(!source_block.status.success(),
        "a source-name block application must be rejected before the block side effect: stdout={} stderr={}",
        String::from_utf8_lossy(&source_block.stdout),
        String::from_utf8_lossy(&source_block.stderr));
    assert!(
        String::from_utf8_lossy(&source_block.stderr).contains("TASK_OWNER_REQUIRED"),
        "block application rejection must be the owner gate: {}",
        String::from_utf8_lossy(&source_block.stderr)
    );
    let after_rejections = fixture.current_state();
    assert_eq!(
        after_rejections["tasks"]["source-alice-work"],
        after["tasks"]["source-alice-work"]
    );
    assert!(
        after_rejections["task_block_requests"]
            .get("source-alice-work")
            .is_none(),
        "source-name block application must not create a pending request: {after_rejections}"
    );
    for (label, expected, args) in [
        (
            "relocate",
            "TASK_OWNER_REQUIRED",
            vec![
                "task",
                "relocate",
                "source-alice-work",
                "--worktree",
                "/tmp/source-alice-relocate-must-stay",
            ],
        ),
        (
            "accept",
            "TASK_OWNER_REQUIRED",
            vec!["task", "accept", "source-alice-work"],
        ),
        (
            "wait",
            "TASK_OWNER_REQUIRED",
            vec![
                "task",
                "wait",
                "source-alice-work",
                "--for",
                "target-bob-work",
            ],
        ),
        (
            "deliver",
            "TASK_OWNER_REQUIRED",
            vec![
                "task",
                "deliver",
                "source-alice-work",
                "--evidence",
                "source delivery must stay with Alice",
                "--worktree",
                "/tmp/source-alice-deliver-must-stay",
            ],
        ),
        (
            "close",
            "task close requires task owner or current master authority",
            vec!["task", "close", "source-alice-work"],
        ),
    ] {
        let rejected = fixture.command_public(&args, THREAD_A);
        assert!(
            !rejected.status.success(),
            "{label} from the old name must be rejected: {}",
            String::from_utf8_lossy(&rejected.stdout)
        );
        let stderr = String::from_utf8_lossy(&rejected.stderr);
        assert!(
            stderr.contains(expected),
            "{label} must be rejected by the current-name owner gate ({expected}): {stderr}"
        );
        let after_mutation = fixture.current_state();
        assert_eq!(
            after_mutation["tasks"]["source-alice-work"], after["tasks"]["source-alice-work"],
            "{label} must not mutate the source task"
        );
        assert_eq!(
            after_mutation["task_assignments"]["source-alice-work"],
            after["task_assignments"]["source-alice-work"],
            "{label} must not move the source assignment"
        );
    }
    // Closing is not the last owner-only entry. A closed task still refuses
    // cleanup finalization by the name that lost the assignment.
    fixture.update_current_state(|state| {
        state["tasks"]["source-alice-work"]["status"] = json!("closed");
    });
    let closed_source = fixture.current_state();
    let source_finalize = fixture.command_public(
        &[
            "task",
            "finalize-cleanup",
            "source-alice-work",
            "--commit",
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        ],
        THREAD_A,
    );
    assert!(
        !source_finalize.status.success(),
        "finalizing cleanup for the retained source task must stay with its current name holder: stdout={} stderr={}",
        String::from_utf8_lossy(&source_finalize.stdout),
        String::from_utf8_lossy(&source_finalize.stderr)
    );
    assert!(
        String::from_utf8_lossy(&source_finalize.stderr)
            .contains("cleanup finalization is not authorized for this caller"),
        "finalize rejection must come from the owner admission, not a fixture precondition: {}",
        String::from_utf8_lossy(&source_finalize.stderr)
    );
    assert_eq!(
        fixture.current_state()["tasks"]["source-alice-work"],
        closed_source["tasks"]["source-alice-work"],
        "rejected cleanup finalization must not mutate the closed source task"
    );
    fixture.run_public(
        &[
            "task",
            "update",
            "target-bob-work",
            "--status",
            "working",
            "--next",
            "continue current Bob work",
        ],
        THREAD_A,
    );
    let target_block = fixture.run_public(
        &[
            "task",
            "block",
            "target-bob-work",
            "--next",
            "current holder may apply a block",
        ],
        THREAD_A,
    );
    assert_eq!(
        target_block["block_request"]["requester"],
        alice["identity"]["worker_id"],
        "a block request belongs to the current stable-name holder, not the legacy owner: {target_block}"
    );
    assert_eq!(
        target_block["block_request"]["state"], "pending",
        "{target_block}"
    );
    assert_eq!(target_block["status"], "working", "{target_block}");
}

#[test]
fn context_names_takeover_preserves_legacy_source_task_attribution() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(
        &[
            "task",
            "register",
            "legacy-alice-work",
            "--next",
            "preserve source attribution",
        ],
        THREAD_A,
    );
    fixture.update_current_state(|state| {
        state["task_assignments"]
            .as_object_mut()
            .unwrap()
            .remove("legacy-alice-work");
    });
    let before = fixture.current_state();
    assert!(before["task_assignments"]["legacy-alice-work"].is_null());
    let takeover = fixture.run_public(
        &["context", "--op", "r8-legacy-source-work", "--approve-grant",
            r#"{"action":"restore_name","requested_name":"Bob","authorization":"fixture user authorizes Bob takeover"}"#],
        THREAD_A,
    );
    assert_eq!(takeover["name"], "Bob");
    let after = fixture.current_state();
    assert_eq!(
        after["task_assignments"]["legacy-alice-work"]["name"],
        "Alice"
    );
    assert_eq!(
        after["task_assignments"]["legacy-alice-work"]["session_id"],
        THREAD_A
    );
    assert_eq!(
        after["tasks"]["legacy-alice-work"]["owner"],
        alice["identity"]["worker_id"]
    );
    assert_eq!(
        after["tasks"]["legacy-alice-work"]["status"],
        before["tasks"]["legacy-alice-work"]["status"]
    );
}

#[test]
fn role_new_master_delegation_requires_user_authorization() {
    let fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "user assigns initial master",
        ],
        THREAD_A,
    );
    let target = bob["identity"]["worker_id"].as_str().unwrap();
    let _attempted = fixture.command_public(&["master", "delegate", target], THREAD_A);
    let current = fixture.run_public(&["master", "status"], THREAD_A);
    assert_eq!(
        current["master"]["worker_id"], alice["identity"]["worker_id"],
        "a current grant does not supply the user's new-master instruction"
    );
    let accepted = fixture.command_public(
        &[
            "master",
            "delegate",
            target,
            "--approval",
            "user explicitly assigns this peer as master",
        ],
        THREAD_A,
    );
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let current = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(current["name"], "Jenny");
    assert_eq!(current["identity"]["role"], "master");
}

#[test]
fn name_mailbox_public_map_published_effect_pending_denies_old_holder_inbox() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    let second = fixture.run_public(&["context"], THREAD_B);
    let worker = first["identity"]["worker_id"].as_str().unwrap();
    let credential: Value = serde_json::from_slice(
        &std::fs::read(
            fixture
                .host_state
                .join("identities")
                .join(worker)
                .join("identity.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let root = fixture.project.canonicalize().unwrap();
    let request = json!({
        "op": "Inbox",
        "worker_id": worker,
        "token": credential["token"],
        "project_context": {
            "canonical_root": root,
            "project_scope": root,
            "app_scope_id": "appserver-cli",
            "runtime_context": credential["runtime"]
        }
    });
    let inbox = || {
        let mut stream = UnixStream::connect(fixture.host_state.join("server.sock")).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        writeln!(stream, "{request}").unwrap();
        let mut line = String::new();
        std::io::BufReader::new(stream)
            .read_line(&mut line)
            .unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "map-first-inbox",
            "map-first inbox sentinel",
        ],
        THREAD_B,
    );
    let message_id = sent["msg_id"].as_str().unwrap();
    let before = inbox();
    assert_eq!(before["ok"], true, "{before}");
    assert!(before["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["id"] == message_id));

    let state_before = fixture.current_state();
    let state_path = fixture.project_state_for(&fixture.project, "appserver-cli");
    let directory = state_path.parent().unwrap();
    let permissions = std::fs::metadata(directory).unwrap().permissions();
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o500)).unwrap();
    let window_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let attempted = fixture.command_public(
            &["context", "--op", "mailbox-map-first-pending", "--approve-grant",
                r#"{"action":"restore_name","requested_name":"Alice","authorization":"fixture user authorizes Alice takeover"}"#],
            THREAD_B,
        );
        assert!(!attempted.status.success());
        assert!(String::from_utf8_lossy(&attempted.stderr)
            .contains("CONTROL_COMMITTED_EFFECTS_PENDING"));
        let map: Value = serde_json::from_slice(
            &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        )
        .unwrap();
        let current = map["slots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|slot| slot["identity_key"]["name"] == "Alice")
            .unwrap();
        assert_eq!(
            current["current"]["binding"]["agent_id"],
            second["identity"]["worker_id"]
        );
        assert!(map["effects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|effect| effect["operation_id"] == "mailbox-map-first-pending"));
        let failed_state = fixture.current_state();
        assert_eq!(
            failed_state["name_occupancies"],
            state_before["name_occupancies"]
        );
        assert_eq!(failed_state["msgs"], state_before["msgs"]);

        let after = inbox();
        if after["ok"] == false {
            let error = after["error"].as_str().unwrap();
            assert!(
                error.contains("CURRENT") || error.contains("IDENTITY_MAP"),
                "{after}"
            );
        } else {
            assert!(
                after["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|message| message["id"] != message_id),
                "old holder must not see target-name mail after current map publication: {after}"
            );
        }
    }));
    std::fs::set_permissions(directory, permissions).unwrap();
    window_result.unwrap();

    let resumed = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(resumed["name"], "Alice");
    let received = fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "map-first-current-receive",
            "--timeout",
            "0",
        ],
        THREAD_B,
    );
    assert!(received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |message| message["id"] == message_id && message["body"] == "map-first inbox sentinel"
        ));
}

#[test]
fn context_names_primary_effect_failure_keeps_both_identities_and_recovers_named_work_once() {
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    let second = fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(
        &[
            "task",
            "register",
            "target-name-work",
            "--next",
            "keep target work",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "task",
            "register",
            "source-name-work",
            "--next",
            "keep source work",
        ],
        THREAD_B,
    );
    let state_before = fixture.current_state();
    let path = fixture.project_state_for(&fixture.project, "appserver-cli");
    let directory = path.parent().unwrap();
    let permissions = std::fs::metadata(directory).unwrap().permissions();
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o500)).unwrap();
    let attempted = fixture.command_public(&["context", "--op", "ordinary-name-effects-fail", "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Alice","authorization":"user explicitly continues named work"}"#], THREAD_B);
    std::fs::set_permissions(directory, permissions).unwrap();
    assert!(!attempted.status.success());
    assert!(
        String::from_utf8_lossy(&attempted.stderr).contains("CONTROL_COMMITTED_EFFECTS_PENDING")
    );
    let map: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let active: Vec<_> = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|slot| slot["identity_status"] == "active")
        .collect();
    assert_eq!(active.len(), 2);
    for worker in [
        &first["identity"]["worker_id"],
        &second["identity"]["worker_id"],
    ] {
        assert_eq!(
            active
                .iter()
                .filter(|slot| &slot["current"]["binding"]["agent_id"] == worker)
                .count(),
            1
        );
    }
    let pending: Vec<_> = map["effects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|effect| effect["operation_id"] == "ordinary-name-effects-fail")
        .collect();
    assert_eq!(pending.len(), 2);
    let state_failed = fixture.current_state();
    assert_eq!(state_failed["tasks"], state_before["tasks"]);
    assert_eq!(
        state_failed["task_assignments"],
        state_before["task_assignments"]
    );
    assert_eq!(
        state_failed["name_occupancies"],
        state_before["name_occupancies"]
    );
    fixture.restart_daemon();
    let restored = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(restored["name"], "Alice");
    let state = fixture.current_state();
    for id in ["target-name-work", "source-name-work"] {
        assert_eq!(state["tasks"][id]["owner"], second["identity"]["worker_id"]);
    }
    assert_eq!(
        state["task_assignments"]["target-name-work"]["name"],
        "Alice"
    );
    assert_eq!(
        state["task_assignments"]["source-name-work"],
        state_before["task_assignments"]["source-name-work"]
    );
    let displaced = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(
        displaced["identity"]["worker_id"],
        first["identity"]["worker_id"]
    );
    assert_eq!(displaced["name"], "Bob");
    assert_eq!(fixture.current_state()["tasks"], state["tasks"]);
    assert_eq!(
        fixture.current_state()["task_assignments"],
        state["task_assignments"]
    );
}

#[test]
fn context_names_master_commands_and_peer_restore_keep_jenny_authority_consistent() {
    let fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "user assigned project goal",
        ],
        THREAD_A,
    );
    let still_master = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(still_master["name"], "Jenny");
    assert_eq!(
        still_master["master"]["worker_id"],
        alice["identity"]["worker_id"]
    );
    let peer = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_peer","authorization":"continue as peer"}"#,
        ],
        THREAD_A,
    );
    assert_ne!(peer["name"], "Jenny", "{peer}");
    assert!(peer["master"].is_null(), "{peer}");
    fixture.run_public(
        &["master", "promote", "--approval", "restore project goal"],
        THREAD_A,
    );
    fixture.run_public(
        &["master", "clear", "--approval", "clear project role"],
        THREAD_A,
    );
    let cleared = fixture.run_public(&["context"], THREAD_A);
    assert_ne!(cleared["name"], "Jenny");
    assert!(cleared["master"].is_null(), "{cleared}");
}

#[test]
fn human_retirement_releases_ordinary_name_and_allows_reallocation() {
    const THREAD_C: &str = "01a0d63b-2f2c-72cf-93c1-2a4a1e2cf1cc";
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(alice["name"], "Alice", "{alice}");
    assert_eq!(bob["name"], "Bob", "{bob}");
    let bob_worker = bob["identity"]["worker_id"].as_str().unwrap();

    // Legal task assignment and a queued message belong to Bob's name.
    fixture.run_public(
        &[
            "task",
            "register",
            "bob-retire-keeps-work",
            "--next",
            "survive explicit retirement",
        ],
        THREAD_B,
    );
    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Bob",
            "--subject",
            "retire-keep",
            "queued for Bob",
        ],
        THREAD_A,
    );
    let msg_id = sent["msg_id"].as_str().unwrap().to_owned();

    // Without the explicit human authorization the release is refused.
    let refused = fixture.command_public(
        &[
            "context",
            "--op",
            "retire-bob-noauth",
            "--approve-grant",
            r#"{"action":"retire_name","requested_name":"Bob"}"#,
        ],
        THREAD_A,
    );
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("IDENTITY_AUTHORIZATION_REQUIRED"),
        "stderr={}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let map_before: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let bob_before = map_before["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["identity_key"]["name"] == "Bob")
        .expect("Bob slot exists");
    assert_eq!(
        bob_before["current"]["binding"]["agent_id"], bob_worker,
        "a refused retirement must not release Bob: {bob_before}"
    );

    // Explicit human retirement releases the ordinary name.
    let retired = fixture.command_public(
        &[
            "context",
            "--op",
            "retire-bob",
            "--approve-grant",
            r#"{"action":"retire_name","requested_name":"Bob","authorization":"本次人类明确指令"}"#,
        ],
        THREAD_A,
    );
    assert!(
        retired.status.success(),
        "explicit retirement: stdout={} stderr={}",
        String::from_utf8_lossy(&retired.stdout),
        String::from_utf8_lossy(&retired.stderr)
    );
    let retired: Value = serde_json::from_slice(&retired.stdout).unwrap();
    assert_eq!(retired["ok"], true, "{retired}");
    assert_eq!(retired["result"]["action"], "retire_name", "{retired}");
    assert_eq!(retired["result"]["outcome"], "completed", "{retired}");
    let map_after: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let bob_after = map_after["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["identity_key"]["name"] == "Bob")
        .expect("Bob slot exists");
    assert!(
        bob_after["current"].is_null(),
        "Bob current released: {bob_after}"
    );
    assert_eq!(
        bob_after["previous"]["binding"]["agent_id"], bob_worker,
        "retired binding is retained as history, not erased: {bob_after}"
    );
    assert!(
        map_after["effects"]
            .as_array()
            .map_or(true, |effects| effects.is_empty()),
        "the bounded human-retire effect must be consumed: {map_after}"
    );

    // Task, message and host are preserved; only the current name is released.
    let state = fixture.current_state();
    assert!(
        state["tasks"]["bob-retire-keeps-work"].is_object(),
        "the unfinished task survives retirement: {state}"
    );
    assert_eq!(
        state["task_assignments"]["bob-retire-keeps-work"]["name"], "Bob",
        "the task stays attributed to the retired name: {state}"
    );
    let message = fixture.run_public(&["msg", &msg_id], THREAD_A);
    assert_eq!(message["state"], "pending", "{message}");
    assert_eq!(
        message["to"], bob_worker,
        "the queued message is not re-addressed: {message}"
    );

    // A later ordinary context acquires the freed ordinary name.
    let reallocated = fixture.run_public(&["context"], THREAD_C);
    assert_eq!(reallocated["name"], "Bob", "{reallocated}");
    assert_ne!(
        reallocated["identity"]["worker_id"], bob_worker,
        "the free name goes to a new carrier: {reallocated}"
    );

    // Replaying the original operation does not retire the successor.
    let replay = fixture.command_public(
        &[
            "context",
            "--op",
            "retire-bob",
            "--approve-grant",
            r#"{"action":"retire_name","requested_name":"Bob","authorization":"本次人类明确指令"}"#,
        ],
        THREAD_A,
    );
    assert!(
        replay.status.success(),
        "retire replay: stdout={} stderr={}",
        String::from_utf8_lossy(&replay.stdout),
        String::from_utf8_lossy(&replay.stderr)
    );
    let map_replayed: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let bob_replayed = map_replayed["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["identity_key"]["name"] == "Bob")
        .expect("Bob slot exists");
    assert_eq!(
        bob_replayed["current"]["binding"]["agent_id"], reallocated["identity"]["worker_id"],
        "a retry must not retire the later holder: {bob_replayed}"
    );
}

#[test]
fn human_retirement_jenny_revokes_exact_grant_and_replay_preserves_successor() {
    const THREAD_C: &str = "01a0d63f-5d9a-72cf-93c1-2a4a1e2cf2dd";
    let mut fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);
    let jenny = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"fixture explicitly requests Jenny"}"#,
        ],
        THREAD_A,
    );
    assert_eq!(jenny["name"], "Jenny", "{jenny}");
    let jenny_worker = jenny["identity"]["worker_id"].as_str().unwrap().to_owned();
    let bob_worker = bob["identity"]["worker_id"].as_str().unwrap().to_owned();
    let project_scope = jenny["binding"]["project_scope"]
        .as_str()
        .unwrap()
        .to_owned();
    let published = fixture.run_public(
        &[
            "board",
            "publish",
            "jenny-retire-keeps-work",
            "--title",
            "Jenny retirement",
            "--description",
            "preserve work through explicit retirement",
            "--delivery-condition",
            "work remains attachable",
            "--test-condition",
            "public retirement regression",
            "--priority",
            "p1",
            "--long-horizon-content",
            "Continue the retired Jenny task from its preserved board state",
            "--long-horizon-every-ms",
            "3600000",
        ],
        THREAD_A,
    );
    assert_eq!(
        published["task"]["owner_display_name"], "Jenny",
        "{published}"
    );
    // Publishing alone does not create an owned unfinished task.
    fixture.run_public(
        &[
            "board",
            "claim",
            "jenny-retire-keeps-work",
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            "jenny-retire-keeps-work",
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        THREAD_A,
    );
    let owned = fixture.run_public(&["task", "status", "jenny-retire-keeps-work"], THREAD_A);
    assert_eq!(owned["owner"], jenny_worker, "{owned}");

    let unauthorized = fixture.command_public(
        &[
            "context",
            "--op",
            "retire-jenny-noauth",
            "--approve-grant",
            r#"{"action":"retire_name","requested_name":"Jenny"}"#,
        ],
        THREAD_B,
    );
    assert!(!unauthorized.status.success());
    assert!(
        String::from_utf8_lossy(&unauthorized.stderr).contains("IDENTITY_AUTHORIZATION_REQUIRED"),
        "stderr={}",
        String::from_utf8_lossy(&unauthorized.stderr)
    );

    let retired = fixture.command_public(
        &[
            "context",
            "--op",
            "retire-jenny",
            "--approve-grant",
            r#"{"action":"retire_name","requested_name":"Jenny","authorization":"human approved Jenny retirement"}"#,
        ],
        THREAD_B,
    );
    assert!(
        retired.status.success(),
        "explicit Jenny retirement: stdout={} stderr={}",
        String::from_utf8_lossy(&retired.stdout),
        String::from_utf8_lossy(&retired.stderr)
    );
    let retired: Value = serde_json::from_slice(&retired.stdout).unwrap();
    assert_eq!(retired["result"]["action"], "retire_name", "{retired}");
    assert_eq!(retired["result"]["outcome"], "completed", "{retired}");
    assert_eq!(
        retired["result"]["snapshot"]["identity"]["status"], "released",
        "{retired}"
    );

    let map: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let jenny_slot = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["identity_key"]["name"] == "Jenny")
        .expect("Jenny slot exists");
    assert!(jenny_slot["current"].is_null(), "{jenny_slot}");
    assert_eq!(
        jenny_slot["previous"]["binding"]["agent_id"], jenny_worker,
        "only the exact retired binding is retained: {jenny_slot}"
    );
    let no_master = fixture.run_public(&["master", "status"], THREAD_B);
    assert!(no_master["master"].is_null(), "{no_master}");
    assert_eq!(
        fixture.current_state()["global"]["master_grants"]
            .as_array()
            .map_or(0, Vec::len),
        0,
        "explicit Jenny retirement revokes its exact old grant"
    );

    let board = fixture.run_public(&["board", "show"], THREAD_B);
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "jenny-retire-keeps-work")
        .unwrap();
    assert_eq!(task["owner"], "", "{task}");
    assert_eq!(task["owner_name"], "Jenny", "{task}");
    assert_eq!(task["needs_reassignment"], true, "{task}");
    let state = fixture.current_state();
    let schedule = &state["name_reassignments"][&project_scope]["Jenny"];
    assert_eq!(schedule["kind"], "eligible-after-retirement", "{schedule}");

    fixture.restart_daemon();
    let no_master_after_restart = fixture.run_public(&["master", "status"], THREAD_B);
    assert!(
        no_master_after_restart["master"].is_null(),
        "{no_master_after_restart}"
    );
    let board_after_restart = fixture.run_public(&["board", "show"], THREAD_B);
    let task_after_restart = board_after_restart["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "jenny-retire-keeps-work")
        .unwrap();
    assert_eq!(
        task_after_restart["needs_reassignment"], true,
        "{task_after_restart}"
    );
    let restarted_state = fixture.current_state();
    let restarted_schedule = &restarted_state["name_reassignments"][&project_scope]["Jenny"];
    assert_eq!(
        restarted_schedule["kind"], "eligible-after-retirement",
        "no master and a restart must preserve immediate eligibility: {restarted_schedule}"
    );

    let ordinary = fixture.run_public(&["context"], THREAD_C);
    assert_ne!(ordinary["name"], "Jenny", "{ordinary}");
    let successor = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"human approved the successor Jenny"}"#,
        ],
        THREAD_B,
    );
    assert_eq!(successor["name"], "Jenny", "{successor}");
    assert_eq!(
        successor["identity"]["worker_id"], bob_worker,
        "{successor}"
    );

    let replay = fixture.command_public(
        &[
            "context",
            "--op",
            "retire-jenny",
            "--approve-grant",
            r#"{"action":"retire_name","requested_name":"Jenny","authorization":"human approved Jenny retirement"}"#,
        ],
        THREAD_A,
    );
    assert!(
        replay.status.success(),
        "retire replay: stdout={} stderr={}",
        String::from_utf8_lossy(&replay.stdout),
        String::from_utf8_lossy(&replay.stderr)
    );
    let replay: Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(replay["result"]["outcome"], "completed", "{replay}");
    let map_after_replay: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let successor_slot = map_after_replay["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["identity_key"]["name"] == "Jenny")
        .expect("successor Jenny slot exists");
    assert_eq!(
        successor_slot["current"]["binding"]["agent_id"], bob_worker,
        "old operation replay must not retire the successor: {successor_slot}"
    );
    let successor_master = fixture.run_public(&["master", "status"], THREAD_B);
    assert_eq!(
        successor_master["master"]["worker_id"], bob_worker,
        "old operation replay must not revoke the successor grant: {successor_master}"
    );
}

#[test]
fn human_retirement_jenny_requires_authorization_and_a_current_holder() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let jenny = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"fixture explicitly requests Jenny"}"#,
        ],
        THREAD_A,
    );
    let jenny_worker = jenny["identity"]["worker_id"].as_str().unwrap().to_owned();

    let missing_authorization = fixture.command_public(
        &[
            "context",
            "--op",
            "retire-jenny-negative-noauth",
            "--approve-grant",
            r#"{"action":"retire_name","requested_name":"Jenny"}"#,
        ],
        THREAD_B,
    );
    assert!(!missing_authorization.status.success());
    assert!(
        String::from_utf8_lossy(&missing_authorization.stderr)
            .contains("IDENTITY_AUTHORIZATION_REQUIRED"),
        "stderr={}",
        String::from_utf8_lossy(&missing_authorization.stderr)
    );

    let missing_holder = fixture.command_public(
        &[
            "context",
            "--op",
            "retire-tina-not-held",
            "--approve-grant",
            r#"{"action":"retire_name","requested_name":"Tina","authorization":"human approved Tina retirement"}"#,
        ],
        THREAD_A,
    );
    assert!(!missing_holder.status.success());
    assert!(
        String::from_utf8_lossy(&missing_holder.stderr).contains("IDENTITY_RETIRE_NAME_NOT_HELD"),
        "stderr={}",
        String::from_utf8_lossy(&missing_holder.stderr)
    );

    let map: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let jenny_slot = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["identity_key"]["name"] == "Jenny")
        .expect("Jenny slot exists");
    assert_eq!(
        jenny_slot["current"]["binding"]["agent_id"], jenny_worker,
        "failed retirement attempts must not release Jenny: {jenny_slot}"
    );
    let master = fixture.run_public(&["master", "status"], THREAD_A);
    assert_eq!(master["master"]["worker_id"], jenny_worker, "{master}");
}

#[test]
fn manager_can_invite_during_identity_return_wait_before_deadline() {
    const THREAD_C: &str = "01a0d63e-5c31-7f30-8661-339018e7c2a3";
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let owner = fixture.run_public(&["context"], THREAD_B);
    let recipient = fixture.run_public(&["context"], THREAD_C);
    let owner_worker = owner["identity"]["worker_id"].as_str().unwrap().to_owned();
    let recipient_worker = recipient["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let owner_name = owner["name"].as_str().unwrap().to_owned();
    let project_scope = owner["binding"]["project_scope"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "fixture approves the waiting-period manager",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "board",
            "publish",
            "manager-before-timeout",
            "--title",
            "manager early invitation",
            "--description",
            "manager invitation is independent from automatic deadline",
            "--delivery-condition",
            "invite then accept",
            "--test-condition",
            "public board regression",
            "--priority",
            "p1",
            "--long-horizon-content",
            "Continue the task after the manager invitation is accepted",
            "--long-horizon-every-ms",
            "3600000",
        ],
        THREAD_A,
    );
    let claimed = fixture.run_public(
        &[
            "board",
            "claim",
            "manager-before-timeout",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    assert_eq!(claimed["task"]["owner"], owner_worker, "{claimed}");
    fixture.run_public(
        &[
            "board",
            "start",
            "manager-before-timeout",
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    fixture
        .state
        .lock()
        .unwrap()
        .get_mut(THREAD_B)
        .unwrap()
        .reject_as_missing = true;
    let missing = fixture.run_public(&["worker", "snapshot", &owner_worker], THREAD_A);
    assert_eq!(missing["judgment_status"], "satisfied", "{missing}");
    fixture.run_public(&["worker", "snapshot", &recipient_worker], THREAD_A);

    let deadline = Instant::now() + Duration::from_secs(10);
    let waiting = loop {
        let state = fixture.current_state();
        let schedule = &state["name_reassignments"][&project_scope][&owner_name];
        if schedule["kind"] == "waiting-return" {
            break schedule.clone();
        }
        assert!(
            Instant::now() < deadline,
            "no real loss schedule: {schedule}"
        );
        thread::sleep(Duration::from_millis(50));
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!(waiting["deadline_ms"].as_i64().unwrap() > now, "{waiting}");

    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "manager-before-timeout")
        .unwrap();
    let revision = task["revision"].as_u64().unwrap();
    assert_eq!(task["needs_reassignment"], true, "{task}");
    assert_eq!(task["owner_name"], owner_name, "{task}");
    assert!(task["invited_peer"].is_null(), "{task}");

    let durable_owner = fixture.run_public(&["task", "status", "manager-before-timeout"], THREAD_A);
    assert_eq!(durable_owner["owner"], owner_worker, "{durable_owner}");

    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            "manager-before-timeout",
            "--to",
            &recipient_worker,
            "--expected-revision",
            &revision.to_string(),
        ],
        THREAD_A,
    );
    assert_eq!(
        invited["task"]["invited_peer"], recipient_worker,
        "{invited}"
    );
    let before_accept = fixture.run_public(&["task", "status", "manager-before-timeout"], THREAD_A);
    assert_eq!(
        before_accept["owner"], owner_worker,
        "an invitation must not transfer ownership: {before_accept}"
    );

    let accepted = fixture.run_public(
        &[
            "board",
            "respond",
            "manager-before-timeout",
            "--accept",
            "--expected-revision",
            &invited["task"]["revision"].to_string(),
        ],
        THREAD_C,
    );
    assert_eq!(
        accepted["task"]["owner"], recipient_worker,
        "acceptance commits the new owner: {accepted}"
    );
    let after_accept = fixture.run_public(&["task", "status", "manager-before-timeout"], THREAD_A);
    assert_eq!(after_accept["owner"], recipient_worker, "{after_accept}");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!(
        waiting["deadline_ms"].as_i64().unwrap() > now,
        "acceptance occurred before the automatic deadline: {waiting}"
    );
}

#[test]
fn human_retirement_legacy_self_close_does_not_enter_explicit_path() {
    const THREAD_D: &str = "01a0d63c-1a3e-7a55-8f47-5b6c7d8e9f10";
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let carol = fixture.run_public(&["context"], THREAD_D);
    assert_eq!(carol["name"], "Bob", "{carol}");
    let close = fixture.command_public(&["worker", "close", "--self"], THREAD_D);
    assert!(
        close.status.success(),
        "legacy self close: stdout={} stderr={}",
        String::from_utf8_lossy(&close.stdout),
        String::from_utf8_lossy(&close.stderr)
    );
    // The legacy self-unregister path is not the explicit human retirement:
    // it records no `retire_name:` command receipt and leaves no HumanRetire
    // effect behind.
    let state = fixture.current_state();
    let receipts = state["global"]["command_receipts"].clone();
    assert!(
        receipts
            .as_object()
            .is_none_or(|receipts| receipts.keys().all(|key| !key.starts_with("retire_name:"))),
        "legacy self close must not record an explicit retirement receipt: {receipts}"
    );
    let map: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    assert!(
        map["effects"].as_array().is_none_or(|effects| effects
            .iter()
            .all(|effect| { effect["kind"] != "human_retire" })),
        "legacy self close must not publish a HumanRetire effect: {map}"
    );
}

#[test]
fn human_retirement_rejects_operation_id_owned_by_another_command() {
    const THREAD_C: &str = "01a0d63e-dafc-7571-a0aa-bbc2bbbc04ce";
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(bob["name"], "Bob", "{bob}");
    let bob_worker = bob["identity"]["worker_id"].as_str().unwrap().to_owned();
    // A first registration records a durable register receipt. Context does
    // not persist its own outer operation receipt, so reuse the real receipt
    // that the public registration committed.
    let context = fixture.command_public(&["context"], THREAD_C);
    assert!(
        context.status.success(),
        "context receipt: stdout={} stderr={}",
        String::from_utf8_lossy(&context.stdout),
        String::from_utf8_lossy(&context.stderr)
    );
    let state = fixture.current_state();
    let operation = state["global"]["command_receipts"]
        .as_object()
        .and_then(|receipts| {
            receipts.values().find_map(|receipt| {
                receipt["command_id"]
                    .as_str()
                    .is_some_and(|command| command.starts_with("register-"))
                    .then(|| receipt["operation_id"].as_str())
                    .flatten()
            })
        })
        .expect("the public registration recorded an operation receipt")
        .to_owned();

    let refused = fixture.command_public(
        &[
            "context",
            "--op",
            operation.as_str(),
            "--approve-grant",
            r#"{"action":"retire_name","requested_name":"Bob","authorization":"本次人类明确指令"}"#,
        ],
        THREAD_A,
    );
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("IDENTITY_OPERATION_CONFLICT"),
        "stderr={}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let map: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let bob_after = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["identity_key"]["name"] == "Bob")
        .expect("Bob slot exists");
    assert_eq!(
        bob_after["current"]["binding"]["agent_id"], bob_worker,
        "an operation conflict must not release Bob: {bob_after}"
    );
    assert!(
        map["effects"]
            .as_array()
            .is_none_or(|effects| effects.is_empty()),
        "an operation conflict must not publish a retirement effect: {map}"
    );
}

fn unique_root() -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let root = std::path::Path::new("/tmp").join(format!(
        "ct-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// The single binary-selection seam for this consumer fixture. `cargo test`
/// builds and runs the debug binary by default; a canonical installed binary
/// can be substituted with `COLLAB_TEST_BINARY` so the same public entry point
/// drives identical installed bytes. The fallback is test-only and never a
/// product behavior.
fn binary() -> PathBuf {
    std::env::var_os("COLLAB_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_collab")))
}

/// Seed the project baseline marker. `collab context` resolves its project root
/// from the `.agent-collab` baseline or a git root; a temp fixture is neither,
/// so it seeds the marker instead of reaching into daemon state.
fn seed_baseline(project: &std::path::Path) {
    std::fs::create_dir_all(project.join(".agent-collab")).expect("seed .agent-collab baseline");
}

/// Walk a parsed context snapshot and fail if any object key carries a secret.
fn assert_no_secret_surface(label: &str, value: &Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                assert_ne!(key, "token", "{label}: raw token key leaked");
                assert_ne!(key, "identity_receipt", "{label}: private receipt leaked");
                assert_no_secret_surface(label, child);
            }
        }
        Value::Array(items) => {
            for item in items {
                assert_no_secret_surface(label, item);
            }
        }
        _ => {}
    }
}

/// The public `collab context` display must never expose the private credential
/// or the internal identity receipt, in raw text or in any parsed object.
fn assert_context_display_is_public(label: &str, output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("identity_receipt"),
        "{label}: context printed the private identity receipt: {stdout}"
    );
    assert!(
        !stdout.contains("\"token\""),
        "{label}: context printed a token field: {stdout}"
    );
    let parsed: Value = serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!("{label}: context emits JSON: {error}: stdout={stdout} stderr={stderr}")
    });
    assert_no_secret_surface(label, &parsed);
}

struct AppFixture {
    root: PathBuf,
    project: PathBuf,
    host_state: PathBuf,
    app_socket: PathBuf,
    path_prepend: PathBuf,
    state: Arc<Mutex<HashMap<String, ThreadState>>>,
    appserver_connections: Arc<AtomicU64>,
    daemon: Option<Child>,
    _server_thread: thread::JoinHandle<()>,
}

impl AppFixture {
    fn project_state_for(&self, project: &Path, app: &str) -> PathBuf {
        let project = project.canonicalize().unwrap();
        let routes: Vec<Value> =
            serde_json::from_slice(&std::fs::read(self.host_state.join("routes.json")).unwrap())
                .unwrap();
        let matches: Vec<_> = routes
            .iter()
            .filter(|route| {
                route["project_scope"].as_str() == project.to_str() && route["app_scope_id"] == app
            })
            .collect();
        assert_eq!(matches.len(), 1, "one exact current project/app route");
        PathBuf::from(matches[0]["storage_root"].as_str().unwrap())
            .join(".agent-collab/server/state.json")
    }

    fn current_state_for(&self, project: &Path, app: &str) -> Value {
        let document: Value =
            serde_json::from_slice(&std::fs::read(self.project_state_for(project, app)).unwrap())
                .unwrap();
        assert_eq!(document["schema_version"], 1);
        assert!(document["state"].is_object());
        document["state"].clone()
    }

    fn current_state(&self) -> Value {
        self.current_state_for(&self.project, "appserver-cli")
    }

    fn update_current_state(&mut self, update: impl FnOnce(&mut Value)) {
        self.stop_daemon();
        let path = self.project_state_for(&self.project, "appserver-cli");
        let mut document: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(document["schema_version"], 1);
        assert!(document["state"].is_object());
        update(&mut document["state"]);
        std::fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
        self.start_daemon();
    }

    fn new() -> Self {
        Self::with_managed_socket(false)
    }

    fn new_desktop_managed() -> Self {
        Self::with_managed_socket(true)
    }

    fn with_managed_socket(managed_socket: bool) -> Self {
        let root = unique_root();
        let project = root.join("project");
        let host_state = root.join("h");
        let app_socket = if managed_socket {
            root.join("home/app-server-control/app-server-control.sock")
        } else {
            root.join("app.sock")
        };
        let path_prepend = root.join("bin");
        std::fs::create_dir_all(&project).expect("create project root");
        std::fs::create_dir_all(&host_state).expect("create host state root");
        std::fs::create_dir_all(&path_prepend).expect("create fixture binary root");
        std::fs::create_dir_all(app_socket.parent().unwrap())
            .expect("create AppServer socket root");
        seed_baseline(&project);
        let project_str = std::fs::canonicalize(&project)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let listener = UnixListener::bind(&app_socket).expect("bind app server fixture");
        let shared = Arc::new(Mutex::new(HashMap::<String, ThreadState>::new()));
        let server_shared = Arc::clone(&shared);
        let connections = Arc::new(AtomicU64::new(0));
        let server_connections = Arc::clone(&connections);
        let server_scope = project_str.clone();
        let server_thread = thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        server_connections.fetch_add(1, Ordering::Relaxed);
                        let shared = Arc::clone(&server_shared);
                        let scope = server_scope.clone();
                        thread::spawn(move || serve_connection(stream, shared, &scope));
                    }
                    Err(_) => break,
                }
            }
        });
        let mut fixture = Self {
            root,
            project,
            host_state,
            app_socket,
            path_prepend,
            state: shared,
            appserver_connections: connections,
            daemon: None,
            _server_thread: server_thread,
        };
        fixture.start_daemon();
        fixture
    }

    fn command_public(&self, args: &[&str], thread: &str) -> Output {
        self.command_public_with_session(args, thread, thread)
    }

    fn command_public_with_session(&self, args: &[&str], session: &str, thread: &str) -> Output {
        Command::new(binary())
            .args(args)
            .current_dir(&self.project)
            .env("PATH", self.fixture_path())
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("HOME", self.root.join("home"))
            .env("COLLAB_APPSERVER_SOCKET", &self.app_socket)
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
            .env("CODEX_SESSION_ID", session)
            .env("CODEX_THREAD_ID", thread)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("COLLAB_WORKER")
            .env("CODEX_HOME", self.root.join("home"))
            .output()
            .expect("run collab CLI")
    }

    /// Start the public CLI without waiting so two real processes can race one
    /// daemon request. The environment matches `command_public`.
    fn spawn_public(&self, args: &[&str], thread: &str) -> std::process::Child {
        Command::new(binary())
            .args(args)
            .current_dir(&self.project)
            .env("PATH", self.fixture_path())
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("HOME", self.root.join("home"))
            .env("COLLAB_APPSERVER_SOCKET", &self.app_socket)
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
            .env("CODEX_SESSION_ID", thread)
            .env("CODEX_THREAD_ID", thread)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("COLLAB_WORKER")
            .env("CODEX_HOME", self.root.join("home"))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn collab CLI")
    }

    fn command_without_thread(&self, args: &[&str], session: &str) -> Output {
        Command::new(binary())
            .args(args)
            .current_dir(&self.project)
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("HOME", self.root.join("home"))
            .env("COLLAB_APPSERVER_SOCKET", &self.app_socket)
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
            .env("CODEX_SESSION_ID", session)
            .env_remove("CODEX_THREAD_ID")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("COLLAB_WORKER")
            .env("CODEX_HOME", self.root.join("home"))
            .output()
            .expect("run collab CLI with a continuously missing thread fact")
    }

    fn command_desktop(&self, args: &[&str], thread: &str) -> Output {
        Command::new(binary())
            .args(args)
            .current_dir(&self.project)
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("HOME", self.root.join("home"))
            .env("COLLAB_APPSERVER_SOCKET", &self.app_socket)
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
            .env("CODEX_SESSION_ID", thread)
            .env("CODEX_THREAD_ID", thread)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("COLLAB_WORKER")
            .env("CODEX_HOME", self.root.join("home"))
            .output()
            .expect("run Desktop collab CLI")
    }

    fn command_desktop_without_explicit_endpoint(&self, args: &[&str], thread: &str) -> Output {
        Command::new(binary())
            .args(args)
            .current_dir(&self.project)
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("HOME", self.root.join("home"))
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env_remove("CODEX_APP_TOOLS_PIPE_PATH")
            .env_remove("CODEX_MCP_NODE_PATH")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
            .env("CODEX_SESSION_ID", thread)
            .env("CODEX_THREAD_ID", thread)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("COLLAB_WORKER")
            .env("CODEX_HOME", self.root.join("home"))
            .output()
            .expect("run Desktop collab CLI without an explicit endpoint")
    }

    fn command_tui_without_explicit_endpoint(&self, args: &[&str], thread: &str) -> Output {
        Command::new(binary())
            .args(args)
            .current_dir(&self.project)
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("HOME", self.root.join("home"))
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
            .env("CODEX_SESSION_ID", thread)
            .env("CODEX_THREAD_ID", thread)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("COLLAB_WORKER")
            .env("CODEX_HOME", self.root.join("home"))
            .output()
            .expect("run TUI collab CLI without an explicit endpoint")
    }

    /// No automatically observed native facts: no AppServer socket, no
    /// session/thread, no tmux, and an originator the runtime cannot map to a
    /// namespace. The caller must supply every fact through `--provide`.
    fn command_without_native_facts(&self, args: &[&str]) -> Output {
        Command::new(binary())
            .args(args)
            .current_dir(&self.project)
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("HOME", self.root.join("home"))
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex future host")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("COLLAB_WORKER")
            .env("CODEX_HOME", self.root.join("home"))
            .output()
            .expect("run collab CLI without native facts")
    }

    fn run_public(&self, args: &[&str], thread: &str) -> Value {
        let output = self.command_public(args, thread);
        assert!(
            output.status.success(),
            "collab {:?} failed: stdout={} stderr={}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).expect("collab CLI emits JSON");
        value
            .get("result")
            .and_then(|result| result.get("snapshot"))
            .cloned()
            .unwrap_or(value)
    }

    fn run_ok_desktop(&self, args: &[&str], thread: &str) -> Value {
        let output = self.command_desktop(args, thread);
        assert!(
            output.status.success(),
            "Desktop collab {:?} failed: stdout={} stderr={}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).expect("collab CLI emits JSON");
        value
            .get("result")
            .and_then(|result| result.get("snapshot"))
            .cloned()
            .unwrap_or(value)
    }

    /// The daemon owns the worker id. Fixtures must read it back from the
    /// registration receipt rather than assume a `COLLAB_WORKER`-derived name.
    fn worker_id(&self, thread: &str) -> String {
        let context = self.run_public(&["context"], thread);
        context["identity"]["worker_id"]
            .as_str()
            .expect("context receipt names the daemon-owned worker")
            .to_owned()
    }

    fn stop_daemon(&mut self) {
        if let Some(mut daemon) = self.daemon.take() {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        let _ = std::fs::remove_file(self.host_state.join("server.sock"));
    }

    fn start_daemon(&mut self) {
        let daemon_log =
            std::fs::File::create(self.root.join("test-daemon.log")).expect("create serve log");
        let mut command = Command::new(binary());
        command
            .arg("serve")
            .current_dir(&self.project)
            .env("PATH", self.fixture_path())
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("HOME", self.root.join("home"))
            .env("CODEX_HOME", self.root.join("home"))
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env_remove("CODEX_APP_TOOLS_PIPE_PATH")
            .env_remove("CODEX_MCP_NODE_PATH")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("COLLAB_WORKER")
            .stdin(std::process::Stdio::null())
            .stdout(daemon_log.try_clone().expect("clone serve log"))
            .stderr(daemon_log);
        self.daemon = Some(command.spawn().expect("spawn owned serve child"));
        daemon_readiness::wait_for_ping(
            self.daemon.as_mut().unwrap(),
            &self.host_state.join("server.sock"),
            Duration::from_secs(5),
        );
    }

    fn restart_daemon(&mut self) {
        self.stop_daemon();
        self.start_daemon();
    }

    fn fixture_path(&self) -> std::ffi::OsString {
        let mut path = std::ffi::OsString::from(&self.path_prepend);
        if let Some(existing) = std::env::var_os("PATH") {
            path.push(":");
            path.push(existing);
        }
        path
    }
}

impl Drop for AppFixture {
    fn drop(&mut self) {
        self.stop_daemon();
        if std::thread::panicking() {
            eprintln!(
                "preserving failed app e2e fixture for diagnosis: root={}",
                self.root.display()
            );
        } else {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

#[derive(Clone, Debug)]
struct ThreadState {
    inputs: Vec<Value>,
    receiver: Option<std::sync::mpsc::Sender<Value>>,
    reject_input: bool,
    reject_as_missing: bool,
    session_id: Option<String>,
    status: String,
    active_turn: Option<String>,
    turns: Vec<Value>,
    tool_namespaces: Vec<Option<String>>,
    seq: u64,
    queued: u64,
}

impl ThreadState {
    fn new() -> Self {
        Self {
            inputs: Vec::new(),
            receiver: None,
            reject_input: false,
            reject_as_missing: false,
            session_id: None,
            status: "idle".into(),
            active_turn: None,
            turns: Vec::new(),
            tool_namespaces: Vec::new(),
            seq: 0,
            queued: 0,
        }
    }

    fn next_turn(&mut self) -> String {
        self.seq += 1;
        format!("turn-{}", self.seq)
    }
}

fn serve_connection(
    mut stream: UnixStream,
    state: Arc<Mutex<HashMap<String, ThreadState>>>,
    project_root: &str,
) {
    if !handshake(&mut stream) {
        return;
    }
    loop {
        let payload = match read_client_frame(&mut stream) {
            Ok(payload) => payload,
            Err(_) => break,
        };
        let Ok(request) = serde_json::from_slice::<Value>(&payload) else {
            continue;
        };
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            continue;
        };
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        let response = dispatch(method, params.clone(), id, &state, project_root);
        if write_frame(&mut stream, &response).is_err() {
            break;
        }
        if response.get("error").is_none()
            && matches!(method, "turn/start" | "turn/steer" | "thread/queue/add")
        {
            let receiver = state
                .lock()
                .unwrap()
                .get(params["threadId"].as_str().unwrap_or_default())
                .and_then(|thread| thread.receiver.clone());
            if let Some(receiver) = receiver {
                let _ = receiver.send(json!({"method":method,"params":params}));
            }
        }
    }
}

fn dispatch(
    method: &str,
    params: Value,
    id: Value,
    state: &Mutex<HashMap<String, ThreadState>>,
    project_root: &str,
) -> Value {
    match method {
        "initialize" => json!({"id": id, "result": {}}),
        "thread/loaded/list" => json!({"id": id, "result": {"data": []}}),
        "thread/resume" => {
            let thread_id = params["threadId"].as_str().unwrap_or_default().to_string();
            if state
                .lock()
                .unwrap()
                .get(&thread_id)
                .is_some_and(|s| s.reject_as_missing)
            {
                return json!({"id":id,"error":{"code":-32000,"message":format!("thread not found: {thread_id}")}});
            }
            json!({"id": id, "result": {"thread": {"id": thread_id}}})
        }
        "thread/read" => thread_read(params, id, state, project_root),
        "thread/items/list" => {
            json!({"id": id, "error": {"code": -32601, "message": "unsupported"}})
        }
        "thread/turns/list" => thread_turns(params, id, state),
        "turn/start" | "turn/steer" | "thread/queue/add" => {
            let thread = params["threadId"].as_str().unwrap_or_default().to_owned();
            if state
                .lock()
                .unwrap()
                .get(&thread)
                .is_some_and(|s| s.reject_input)
            {
                return json!({"id":id,"error":{"code":-32000,"message":"fixture rejects native input"}});
            }
            state
                .lock()
                .unwrap()
                .entry(thread)
                .or_insert_with(ThreadState::new)
                .inputs
                .push(params.clone());
            match method {
                "turn/start" => turn_start(params, id, state),
                "turn/steer" => turn_steer(params, id, state),
                _ => queue_add(params, id, state),
            }
        }
        "thread/archive" => json!({"id": id, "result": {"archived": true}}),
        _ => json!({"id": id, "error": {"code": -32601, "message": "unsupported"}}),
    }
}

fn thread_read(
    params: Value,
    id: Value,
    state: &Mutex<HashMap<String, ThreadState>>,
    project_root: &str,
) -> Value {
    let thread_id = params["threadId"].as_str().unwrap_or_default().to_string();
    let mut locked = state.lock().unwrap();
    let entry = locked
        .entry(thread_id.clone())
        .or_insert_with(ThreadState::new);
    if entry.reject_as_missing {
        return json!({"id":id,"error":{"code":-32000,"message":format!("thread not found: {thread_id}")}});
    }
    json!({
        "id": id,
        "result": {
            "thread": {
                "id": thread_id,
                "sessionId": entry.session_id.as_ref().unwrap_or(&thread_id),
                "cwd": project_root,
                "status": {"type": entry.status},
                "turns": entry.turns
            }
        }
    })
}

fn thread_turns(params: Value, id: Value, state: &Mutex<HashMap<String, ThreadState>>) -> Value {
    let thread_id = params["threadId"].as_str().unwrap_or_default().to_string();
    let data = state
        .lock()
        .unwrap()
        .get(&thread_id)
        .cloned()
        .unwrap_or_else(ThreadState::new)
        .turns;
    json!({"id": id, "result": {"data": data}})
}

fn turn_start(params: Value, id: Value, state: &Mutex<HashMap<String, ThreadState>>) -> Value {
    let thread_id = params["threadId"].as_str().unwrap_or_default().to_string();
    // Admission probes method availability with an empty target. They do not
    // dispatch a participant turn or produce a registration reply.
    if thread_id.is_empty() {
        return json!({"id": id, "result": {}});
    }
    let mut locked = state.lock().unwrap();
    let entry = locked
        .entry(thread_id.clone())
        .or_insert_with(ThreadState::new);
    entry.tool_namespaces.push(
        params["toolOutput"]["namespace"]
            .as_str()
            .map(str::to_owned),
    );
    let turn_id = entry.next_turn();
    entry.status = "active".into();
    entry.active_turn = Some(turn_id.clone());
    entry
        .turns
        .push(json!({"id": turn_id, "status": "inProgress"}));
    json!({
        "id": id,
        "result": {"turn": {"id": turn_id, "status": "inProgress"}}
    })
}

fn turn_steer(params: Value, id: Value, state: &Mutex<HashMap<String, ThreadState>>) -> Value {
    let thread_id = params["threadId"].as_str().unwrap_or_default().to_string();
    let expected = params["expectedTurnId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let mut locked = state.lock().unwrap();
    let entry = locked.entry(thread_id).or_insert_with(ThreadState::new);
    if entry.active_turn.as_deref() != Some(expected.as_str()) {
        return json!({
            "id": id,
            "error": {"code": -32602, "message": "turn identity mismatch"}
        });
    }
    json!({"id": id, "result": {"turnId": expected}})
}

fn queue_add(params: Value, id: Value, state: &Mutex<HashMap<String, ThreadState>>) -> Value {
    let thread_id = params["threadId"].as_str().unwrap_or_default().to_string();
    let mut locked = state.lock().unwrap();
    let entry = locked.entry(thread_id).or_insert_with(ThreadState::new);
    entry.queued += 1;
    json!({
        "id": id,
        "result": {"queuedSubmission": {"id": format!("qs-{}", entry.queued)}}
    })
}

fn handshake(stream: &mut UnixStream) -> bool {
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        match stream.read_exact(&mut byte) {
            Ok(()) => {
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => return false,
        }
    }
    stream
        .write_all(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
        )
        .is_ok()
}

fn read_client_frame(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header)?;
    let masked = header[1] & 0x80 != 0;
    let mut length = (header[1] & 0x7f) as usize;
    if length == 126 {
        let mut bytes = [0_u8; 2];
        stream.read_exact(&mut bytes)?;
        length = u16::from_be_bytes(bytes) as usize;
    }
    let mut mask = [0_u8; 4];
    if masked {
        stream.read_exact(&mut mask)?;
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload)?;
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    Ok(payload)
}

fn write_frame(stream: &mut UnixStream, payload: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(payload).expect("json encode");
    let mut frame = Vec::with_capacity(bytes.len() + 4);
    frame.push(0x81);
    match bytes.len() {
        length if length < 126 => frame.push(length as u8),
        length if length <= u16::MAX as usize => {
            frame.push(126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        }
        _ => unimplemented!("large test frames"),
    }
    frame.extend_from_slice(&bytes);
    stream.write_all(&frame)
}

#[test]
fn two_tui_appserver_receipt_flow() {
    let fixture = AppFixture::new();

    let context_a = fixture.run_public(&["context"], THREAD_A);
    let context_b = fixture.run_public(&["context"], THREAD_B);
    let worker_a = context_a["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names TUI worker A")
        .to_owned();
    let worker_b = context_b["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names TUI worker B")
        .to_owned();
    let mut app_fixture = fixture;

    let context_a = app_fixture.run_public(&["context"], THREAD_A);
    assert_eq!(context_a["identity"]["transport"]["kind"], "appserver");
    assert_eq!(context_a["liveness"]["presence"], "present");
    let context_b = app_fixture.run_public(&["context"], THREAD_B);
    assert_eq!(context_b["identity"]["transport"]["kind"], "appserver");
    assert_eq!(context_b["liveness"]["presence"], "present");
    assert_eq!(context_a["identity"]["worker_id"], worker_a);
    assert_eq!(context_b["identity"]["worker_id"], worker_b);

    let sent = app_fixture.run_public(
        &[
            "send",
            "--to",
            &worker_b,
            "--subject",
            "appserver immediate",
            "immediate body",
        ],
        THREAD_A,
    );
    let immediate_id = sent["msg_id"].as_str().unwrap().to_string();
    assert_eq!(sent["notification"], "appserver-input-submitted");
    assert_eq!(sent["consumed"], false);

    let steered = app_fixture.run_public(
        &[
            "send",
            "--to",
            &worker_b,
            "--subject",
            "appserver steer",
            "steer body",
        ],
        THREAD_A,
    );
    let steered_id = steered["msg_id"].as_str().unwrap().to_string();
    assert_eq!(steered["notification"], "appserver-input-submitted");
    assert_eq!(steered["consumed"], false);

    let queued = app_fixture.run_public(
        &[
            "send",
            "--to",
            &worker_b,
            "--subject",
            "appserver queued",
            "queued body",
        ],
        THREAD_A,
    );
    let queued_id = queued["msg_id"].as_str().unwrap().to_string();
    assert_eq!(queued["notification"], "appserver-input-submitted");
    assert_eq!(queued["consumed"], false);

    let received = app_fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "appserver-receive-1",
            "--timeout",
            "0",
        ],
        THREAD_B,
    );
    assert_eq!(received["count"], 3);
    let ids = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(ids.contains(&immediate_id));
    assert!(ids.contains(&steered_id));
    assert!(ids.contains(&queued_id));
    assert_eq!(received["receive_id"], "appserver-receive-1");

    let msg_state = app_fixture.run_public(&["msg", &immediate_id], THREAD_A);
    assert_eq!(msg_state["state"], "read");

    app_fixture.restart_daemon();
    let replay_context = app_fixture.run_public(&["context"], THREAD_B);
    assert_eq!(replay_context["inbox"]["unread"], 0);
    assert_eq!(replay_context["identity"]["transport"]["kind"], "appserver");
    assert_eq!(replay_context["identity"]["worker_id"], worker_b);
    let replayed_msg = app_fixture.run_public(&["msg", &immediate_id], THREAD_A);
    assert_eq!(replayed_msg["state"], "read");

    drop(app_fixture);
}

/// Count durable `worker-idle` notifications addressed to the master. The
/// count is read from committed state, so it measures one effective delivery
/// per idle state rather than transport attempts.
fn idle_notifications_to(state: &Value, master: &str) -> usize {
    state["msgs"].as_object().map_or(0, |msgs| {
        msgs.values()
            .filter(|message| {
                message["to"].as_str() == Some(master)
                    && message["subject"].as_str() == Some("worker-idle")
            })
            .count()
    })
}

/// Public-entry idempotence and recipient-presence contract for the idle
/// wake. An unknown-presence master keeps the batch pending; once the master
/// is confirmed online the same idle set is delivered exactly once and is not
/// regenerated while that notification stays unconsumed.
#[test]
fn ordinary_idle_wake_is_held_while_master_unknown_and_sent_once_when_online() {
    let fixture = AppFixture::new();
    let context_a = fixture.run_public(&["context"], THREAD_A);
    let context_b = fixture.run_public(&["context"], THREAD_B);
    let worker_a = context_a["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names TUI worker A")
        .to_owned();
    let worker_b = context_b["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names TUI worker B")
        .to_owned();
    let mut app_fixture = fixture;
    let promoted = app_fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"fixture keeps worker B as master"}"#,
        ],
        THREAD_B,
    );
    assert_eq!(promoted["master"]["worker_id"], worker_b);
    let project = std::fs::canonicalize(&app_fixture.project)
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let seed = |state: &mut Value, master_presence: &str| {
        state["agent_presence"][&project][&worker_a] = json!({
            "project_scope": project, "worker_id": worker_a, "session_id": THREAD_A,
            "presence": "online", "reason": "fixture", "observed_ms": 1
        });
        state["agent_activity"][&project][&worker_a] = json!({
            "project_scope": project, "worker_id": worker_a, "session_id": THREAD_A,
            "activity": "idle", "background": false, "reason": "fixture",
            "observed_ms": 1, "idle_since_ms": 1
        });
        state["agent_presence"][&project][&worker_b] = json!({
            "project_scope": project, "worker_id": worker_b, "session_id": THREAD_B,
            "presence": master_presence, "reason": "fixture", "observed_ms": 2
        });
        state["agent_activity"][&project][&worker_b] = json!({
            "project_scope": project, "worker_id": worker_b, "session_id": THREAD_B,
            "activity": "idle", "background": false, "reason": "fixture",
            "observed_ms": 2, "idle_since_ms": 2
        });
        state["master_wake"]["idle_workers"] = json!([worker_a]);
        state["master_wake"]["newly_idle_workers"] = json!([worker_a]);
        state["master_wake"]["generation"] = json!(1);
        state["master_wake"]["delivery_state"] = json!("pending");
    };

    // Unknown recipient presence must not deliver, and the pending idle set
    // must survive repeated daemon ticks.
    app_fixture.update_current_state(|state| seed(state, "unknown"));
    thread::sleep(Duration::from_millis(2500));
    assert_eq!(
        idle_notifications_to(&app_fixture.current_state(), &worker_b),
        0,
        "an unknown-presence master must not receive an idle wake"
    );

    // Confirming the master online delivers the same idle set exactly once.
    app_fixture.update_current_state(|state| seed(state, "online"));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if idle_notifications_to(&app_fixture.current_state(), &worker_b) >= 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "an online master must receive the idle wake"
        );
        thread::sleep(Duration::from_millis(100));
    }

    // The still-unconsumed notification must not be regenerated on later ticks.
    thread::sleep(Duration::from_millis(2500));
    assert_eq!(
        idle_notifications_to(&app_fixture.current_state(), &worker_b),
        1,
        "the same unconsumed idle set must not be delivered twice"
    );

    drop(app_fixture);
}

#[test]
fn idle_capacity_projection_public_board_and_context_share_pruned_capacity() {
    let thread_c = "01a0d63b-7a35-71c6-83b4-6d4eb7b47a25";
    let mut fixture = AppFixture::new();
    let context_a = fixture.run_public(&["context"], THREAD_A);
    let context_c = fixture.run_public(&["context"], thread_c);
    let promoted = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"isolated idle projection fixture"}"#,
        ],
        THREAD_B,
    );
    let busy_worker = context_a["identity"]["worker_id"]
        .as_str()
        .expect("busy fixture worker id")
        .to_owned();
    let idle_worker = context_c["identity"]["worker_id"]
        .as_str()
        .expect("idle fixture worker id")
        .to_owned();
    assert_eq!(
        promoted["master"]["worker_id"],
        promoted["identity"]["worker_id"]
    );

    fixture.update_current_state(|state| {
        for worker_id in [&busy_worker, &idle_worker] {
            state["keepalives"][worker_id] = json!({
                "observed": "idle",
                "idle_since_ms": 1,
                "activity_ms": 1,
                "last_notice_ms": 0,
                "last_notice_id": null,
                "unacked": 0,
                "suspected_offline": false
            });
        }
        state["tasks"]["public-busy"] = json!({
            "id": "public-busy",
            "owner": busy_worker,
            "created_by": promoted["identity"]["worker_id"],
            "feature_id": null,
            "worktree_path": null,
            "branch": null,
            "base_commit": null,
            "priority": "p1",
            "status": "accepted",
            "next_step": null,
            "wait": null,
            "isolation_mode": null,
            "created_ms": 1,
            "updated_ms": 1
        });
        state["master_wake"]["idle_workers"] = json!([busy_worker, idle_worker]);
        state["master_wake"]["newly_idle_workers"] = json!([busy_worker, idle_worker]);
        state["master_wake"]["generation"] = json!(1);
        state["master_wake"]["delivery_state"] = json!("pending");
    });
    let before = fixture.current_state();

    let board_output = fixture.command_public(&["board", "show"], THREAD_B);
    let context_output = fixture.command_public(&["context"], THREAD_B);
    println!("PUBLIC_CLI_FIXTURE {}", fixture.root.display());
    println!(
        "PUBLIC_CLI_COMMAND cwd={} COLLAB_STATE_DIR={} CODEX_SESSION_ID={} collab board show",
        fixture.project.display(),
        fixture.host_state.display(),
        THREAD_B
    );
    println!("PUBLIC_CLI_BOARD_EXIT {:?}", board_output.status.code());
    println!(
        "PUBLIC_CLI_BOARD_STDOUT {}",
        String::from_utf8_lossy(&board_output.stdout)
    );
    println!(
        "PUBLIC_CLI_BOARD_STDERR {}",
        String::from_utf8_lossy(&board_output.stderr)
    );
    println!(
        "PUBLIC_CLI_COMMAND cwd={} COLLAB_STATE_DIR={} CODEX_SESSION_ID={} collab context",
        fixture.project.display(),
        fixture.host_state.display(),
        THREAD_B
    );
    println!("PUBLIC_CLI_CONTEXT_EXIT {:?}", context_output.status.code());
    println!(
        "PUBLIC_CLI_CONTEXT_STDOUT {}",
        String::from_utf8_lossy(&context_output.stdout)
    );
    println!(
        "PUBLIC_CLI_CONTEXT_STDERR {}",
        String::from_utf8_lossy(&context_output.stderr)
    );
    assert!(
        board_output.status.success(),
        "board show failed: {}",
        String::from_utf8_lossy(&board_output.stderr)
    );
    assert!(
        context_output.status.success(),
        "context failed: {}",
        String::from_utf8_lossy(&context_output.stderr)
    );
    let snapshot = |output: &Output| {
        let value: Value = serde_json::from_slice(&output.stdout).expect("public CLI JSON");
        value
            .get("result")
            .and_then(|result| result.get("snapshot"))
            .cloned()
            .unwrap_or(value)
    };
    let board = snapshot(&board_output);
    let context = snapshot(&context_output);
    for wake in [
        &board["master"]["master_wake"],
        &context["master_wake"],
        &context["master"]["master_wake"],
    ] {
        assert!(
            !wake["idle_workers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id.as_str() == Some(busy_worker.as_str())),
            "busy worker leaked into public idle capacity: {wake}"
        );
        assert!(
            wake["idle_workers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id.as_str() == Some(idle_worker.as_str())),
            "genuine idle worker disappeared from public capacity: {wake}"
        );
    }
    assert_eq!(
        context["master_wake"], context["master"]["master_wake"],
        "context must project one master capacity value in both fields"
    );
    assert_eq!(
        fixture.current_state(),
        before,
        "public reads must not mutate control state"
    );
}

fn native_input_texts(input: &Value) -> Vec<String> {
    let mut texts = Vec::new();
    if let Some(items) = input["input"].as_array() {
        for item in items {
            if let Some(text) = item["text"].as_str() {
                texts.push(text.to_owned());
            }
        }
    }
    if let Some(text) = input["toolOutput"]["output"].as_str() {
        texts.push(text.to_owned());
    }
    texts
}

/// Black-box batch-delivery contract at the real App Server entry: the batch
/// body anchors on `entries=` (each entry already carrying its id, title and
/// preview) and no longer repeats `message_ids=` or `last=`.
#[test]
fn appserver_batch_delivery_emits_entries_format_without_removed_fields() {
    let fixture = AppFixture::new();
    let context_a = fixture.run_public(&["context"], THREAD_A);
    let context_b = fixture.run_public(&["context"], THREAD_B);
    let recipient = context_b["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names TUI worker B")
        .to_owned();
    assert!(context_a["identity"]["worker_id"].as_str().is_some());

    let mut app_fixture = fixture;
    app_fixture.run_public(
        &[
            "send",
            "--to",
            &recipient,
            "--subject",
            "batch format one",
            "batch format one body",
        ],
        THREAD_A,
    );
    app_fixture.run_public(
        &[
            "send",
            "--to",
            &recipient,
            "--subject",
            "batch format two",
            "batch format two body",
        ],
        THREAD_A,
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut observed_batch: Option<String> = None;
    loop {
        let batch = {
            let inputs = app_fixture.state.lock().unwrap();
            inputs.get(THREAD_B).map_or(None, |state| {
                state.inputs.iter().find_map(|input| {
                    native_input_texts(input).into_iter().find(|text| {
                        text.contains("[notification-batch]") && text.contains("Batch wake:")
                    })
                })
            })
        };
        if let Some(text) = batch {
            observed_batch = Some(text);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "batch notification never reached worker B native input"
        );
        thread::sleep(Duration::from_millis(50));
    }

    let batch_text = observed_batch.expect("captured a notification-batch delivery");
    let seed_id = batch_text
        .split_once("COLLAB_NOTIFY ")
        .and_then(|(_, rest)| rest.split_once(" [notification-batch]"))
        .map(|(id, _)| id.trim())
        .expect("batch notification names its seed message id");
    assert!(
        batch_text.contains("Batch wake: entries="),
        "batch body must anchor on entries: {batch_text}"
    );
    assert!(
        !batch_text.contains("message_ids="),
        "batch body must not re-list message_ids: {batch_text}"
    );
    assert!(
        !batch_text.contains(" last="),
        "batch body must not repeat the last title: {batch_text}"
    );
    assert!(
        batch_text.contains(&format!("entries={seed_id}:")),
        "the first entry must carry the notification seed id: {batch_text}"
    );

    drop(app_fixture);
}

fn native_input_has_message(input: &Value, message_id: &str) -> bool {
    input["input"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["text"].as_str())
        .chain(input["toolOutput"]["output"].as_str())
        .any(|text| {
            text.split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                .any(|token| token == message_id)
        })
}

#[test]
fn public_default_immediate_recovery_reaches_the_adaptor_exactly_once() {
    let mut fixture = AppFixture::new();
    let context_a = fixture.run_public(&["context"], THREAD_A);
    let context_b = fixture.run_public(&["context"], THREAD_B);
    let worker_b = context_b["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names worker B")
        .to_owned();
    let default_id = fixture.run_public(&["notify", "status"], THREAD_B)["subscriptions"]
        .as_array()
        .expect("notify status lists subscriptions")
        .iter()
        .find(|subscription| {
            subscription["event"] == "direct-message"
                && subscription["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("sub-default-direct-message-"))
        })
        .and_then(|subscription| subscription["id"].as_str())
        .expect("registered worker has the daemon-owned default lease")
        .to_owned();
    assert_eq!(context_a["identity"]["transport"]["kind"], "appserver");

    // Persist a real public sendmessage with delivery=immediate while native
    // notification is disabled. The send owner must not spend the durable
    // attempt budget before the daemon owns the current default lease.
    let config_path = fixture.root.join("home/.appsdk/config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fixture.stop_daemon();
    std::fs::write(&config_path, "[notifications]\nenabled=false\n").unwrap();
    fixture.start_daemon();
    let sent = fixture.command_public(
        &[
            "sendmessage",
            "--to",
            &worker_b,
            "--subject",
            "recovered immediate public",
            "recovered immediate body",
        ],
        THREAD_A,
    );
    let sent_stderr = String::from_utf8_lossy(&sent.stderr);
    let sent_body = sent_stderr
        .rsplit_once("collab response: ")
        .map(|(_, body)| body.trim())
        .unwrap_or_default();
    let sent: Value =
        serde_json::from_str(sent_body).expect("sendmessage emits a JSON result on stderr");
    assert!(
        sent["durable"] == true
            && sent["notification_error"]
                .as_str()
                .is_some_and(|error| error == "notifications are disabled"),
        "the disabled-send case must retain the durable immediate report: {sent}"
    );
    let message_id = sent["msg_id"]
        .as_str()
        .or_else(|| sent["message_id"].as_str())
        .expect("public sendmessage has a durable message id")
        .to_owned();
    let before = fixture.run_public(&["msg", &message_id], THREAD_A);
    assert_eq!(before["state"], "pending");
    assert_eq!(before["wake_attempts"], 0);

    // Restore notifications with the current default lease cancelled.
    // The daemon reconciles the lease and binds the pending
    // immediate report; the automatic attempt owner may submit it only once.
    fixture.stop_daemon();
    std::fs::write(&config_path, "[notifications]\nenabled=true\n").unwrap();
    fixture.update_current_state(|state| {
        let lease = &mut state["notification_subscriptions"][&default_id];
        assert!(lease.is_object());
        lease["status"] = json!("cancelled");
        lease["status_reason"] = json!("explicit-unsubscribe");
        lease["updated_ms"] = json!(1);
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state = fixture.current_state();
        let accepted = state["notification_delivery_accepted"][&message_id].is_i64()
            && state["notification_delivery_evidence"][&message_id].is_object();
        let inputs = fixture.state.lock().unwrap();
        let observed = inputs.get(THREAD_B).is_some_and(|state| {
            state
                .inputs
                .iter()
                .any(|input| native_input_has_message(input, &message_id))
        });
        if accepted && observed {
            break;
        }
        drop(inputs);
        assert!(
            Instant::now() < deadline,
            "public immediate recovery did not reach the adaptor once"
        );
        thread::sleep(Duration::from_millis(50));
    }

    let after = fixture.run_public(&["msg", &message_id], THREAD_A);
    assert_eq!(after["wake_attempts"], 1);
    let state = fixture.current_state();
    assert_eq!(state["delivery_modes"][&message_id], "immediate");
    assert_eq!(state["msgs"][&message_id]["wake_attempt_count"], 1);
    let accepted_before_restart = state["notification_delivery_accepted"][&message_id].clone();
    let evidence_before_restart = state["notification_delivery_evidence"][&message_id].clone();
    assert!(accepted_before_restart.is_i64());
    assert!(evidence_before_restart.is_object());
    let observed_events = {
        let inputs = fixture.state.lock().unwrap();
        inputs.get(THREAD_B).map_or(0, |state| {
            state
                .inputs
                .iter()
                .filter(|input| native_input_has_message(input, &message_id))
                .count()
        })
    };
    assert_eq!(
        observed_events, 1,
        "the recovered report must reach native input once"
    );

    // A later daemon restart and context reconciliation must not resubmit an
    // accepted attempt or spend more of its at-most-once budget.
    fixture.restart_daemon();
    fixture.run_public(&["context"], THREAD_B);
    thread::sleep(Duration::from_millis(1300));
    let state = fixture.current_state();
    assert_eq!(state["msgs"][&message_id]["wake_attempt_count"], 1);
    assert_eq!(
        state["notification_delivery_accepted"][&message_id],
        accepted_before_restart
    );
    assert_eq!(
        state["notification_delivery_evidence"][&message_id],
        evidence_before_restart
    );
    let observed_after_replay = {
        let inputs = fixture.state.lock().unwrap();
        inputs.get(THREAD_B).map_or(0, |state| {
            state
                .inputs
                .iter()
                .filter(|input| native_input_has_message(input, &message_id))
                .count()
        })
    };
    assert_eq!(
        observed_after_replay, 1,
        "replay must not submit the report twice"
    );
    drop(fixture);
}

#[test]
fn desktop_registration_uses_explicit_endpoint_and_codex_app_namespace() {
    let mut fixture = AppFixture::new_desktop_managed();

    let context_a = fixture.run_ok_desktop(&["context"], THREAD_A);
    assert_eq!(context_a["identity"]["transport"]["namespace"], "codex_app");
    assert_eq!(
        context_a["identity"]["transport"]["endpoint"],
        format!("unix://{}", fixture.app_socket.display())
    );
    let context_b = fixture.run_ok_desktop(&["context"], THREAD_B);
    let worker_b = context_b["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names Desktop worker B")
        .to_owned();
    assert_eq!(context_b["identity"]["transport"]["namespace"], "codex_app");

    let context_b = fixture.run_ok_desktop(&["context"], THREAD_B);
    assert_eq!(context_b["identity"]["transport"]["namespace"], "codex_app");
    assert_eq!(
        context_b["identity"]["transport"]["endpoint"],
        format!("unix://{}", fixture.app_socket.display())
    );

    let sent = fixture.run_ok_desktop(
        &[
            "send",
            "--to",
            &worker_b,
            "--subject",
            "desktop namespace",
            "desktop body",
        ],
        THREAD_A,
    );
    assert_eq!(sent["notification"], "appserver-input-submitted");
    assert_eq!(sent["consumed"], false);
    assert_eq!(
        fixture.state.lock().unwrap()[THREAD_B]
            .tool_namespaces
            .last()
            .and_then(Option::as_deref),
        Some("codex_app")
    );

    let received = fixture.run_ok_desktop(
        &[
            "recv",
            "--receive-id",
            "desktop-receive-1",
            "--timeout",
            "0",
        ],
        THREAD_B,
    );
    assert_eq!(received["count"], 1);

    fixture.restart_daemon();
    let replay_context = fixture.run_ok_desktop(&["context"], THREAD_B);
    assert_eq!(
        replay_context["identity"]["transport"]["namespace"],
        "codex_app"
    );
}

#[test]
fn desktop_managed_socket_is_not_inferred_without_explicit_endpoint() {
    let fixture = AppFixture::new_desktop_managed();

    let output = fixture.command_desktop_without_explicit_endpoint(&["context"], THREAD_A);
    assert!(
        !output.status.success(),
        "unenrolled Desktop must report its actual bridge capability gap"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("DESKTOP_BRIDGE_CAPABILITY_REQUIRED"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "no fabricated AppServer requirement projection"
    );
    assert_eq!(
        fixture.appserver_connections.load(Ordering::Relaxed),
        0,
        "the managed Desktop socket must not be probed without a current-runtime endpoint"
    );
    assert!(!fixture.host_state.join("bindings").exists());
}

#[test]
fn tui_does_not_borrow_the_desktop_managed_socket() {
    let mut fixture = AppFixture::new_desktop_managed();

    let output = fixture.command_tui_without_explicit_endpoint(&["init"], THREAD_A);
    assert!(
        !output.status.success(),
        "TUI init must not synthesize a managed Desktop endpoint: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fixture.appserver_connections.load(Ordering::Relaxed),
        0,
        "TUI must not connect to the Desktop managed AppServer socket"
    );
}

#[test]
fn wrong_appserver_endpoint_fails_closed() {
    let fixture = AppFixture::new();
    let root = fixture.root.clone();
    let project = fixture.project.clone();
    let host_state = fixture.host_state.clone();
    let missing = root.join("missing.sock");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&host_state).unwrap();
    let output = Command::new(binary())
        .arg("init")
        .current_dir(&project)
        .env("COLLAB_STATE_DIR", &host_state)
        .env("HOME", root.join("home"))
        .env("COLLAB_APPSERVER_SOCKET", &missing)
        .env_remove("CODEX_APP_SERVER_SOCKET")
        .env_remove("COLLAB_APPSERVER_NAMESPACE")
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
        .env("CODEX_SESSION_ID", THREAD_A)
        .env("CODEX_THREAD_ID", THREAD_A)
        .env("CODEX_HOME", root.join("home"))
        .output()
        .expect("run collab init");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{combined}");
    assert!(
        combined.contains("APPSERVER_ENDPOINT_REJECTED") || combined.contains("endpoint"),
        "{combined}"
    );
}

/// B02/B11: a confirmed online name takeover transfers the typed grant and
/// preserves the old peer and the public AppServer communication path.
#[test]
fn master_authority_appserver_explicit_replacement_keeps_holder_control() {
    let mut fixture = AppFixture::new();
    let context_a = fixture.run_public(&["context"], THREAD_A);
    let context_b = fixture.run_public(&["context"], THREAD_B);
    let worker_a = context_a["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names TUI worker A")
        .to_owned();
    let worker_b = context_b["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names TUI worker B")
        .to_owned();
    let project_scope = context_a["binding"]["project_scope"]
        .as_str()
        .expect("context receipt names the project scope")
        .to_owned();
    let app_scope_id = context_a["binding"]["app_scope_id"]
        .as_str()
        .expect("context receipt names the app scope")
        .to_owned();

    let promoted_a = fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "user approved TUI worker A",
        ],
        THREAD_A,
    );
    assert_eq!(promoted_a["master"], worker_a);
    assert_eq!(promoted_a["scope"]["project_scope"], project_scope);
    assert_eq!(promoted_a["scope"]["app_scope_id"], app_scope_id);

    fixture.update_current_state(|state| {
        state["agent_presence"][project_scope.as_str()][worker_a.as_str()] = json!({
            "project_scope":project_scope, "worker_id":worker_a, "session_id":THREAD_A,
            "presence":"online", "reason":"current online holder fixture", "observed_ms":1
        })
    });
    let promoted_b = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"user approved TUI worker B","replace_confirmed":true}"#,
        ],
        THREAD_B,
    );
    assert_eq!(promoted_b["master"]["worker_id"], worker_b);
    assert_eq!(promoted_b["scope"]["project_scope"], project_scope);
    assert_eq!(promoted_b["scope"]["app_scope_id"], app_scope_id);

    let status = fixture.run_public(&["master", "status"], THREAD_B);
    assert_eq!(status["master"]["worker_id"], worker_b);
    assert_eq!(status["scope"]["project_scope"], project_scope);
    assert_eq!(status["scope"]["app_scope_id"], app_scope_id);

    // The replaced holder remains a peer and may publish its own work.
    let published_by_peer = fixture.run_public(
        &[
            "board",
            "publish",
            "a-after-replacement",
            "--title",
            "t",
            "--description",
            "d",
            "--delivery-condition",
            "c",
            "--test-condition",
            "t",
            "--priority",
            "p2",
        ],
        THREAD_A,
    );
    assert_eq!(published_by_peer["task"]["publisher"], worker_a);
    let published = fixture.run_public(
        &[
            "board",
            "publish",
            "b-after-replacement",
            "--title",
            "t",
            "--description",
            "d",
            "--delivery-condition",
            "c",
            "--test-condition",
            "t",
            "--priority",
            "p2",
            "--long-horizon-content",
            "Complete the replacement master's task under its current board contract",
            "--long-horizon-every-ms",
            "3600000",
        ],
        THREAD_B,
    );
    assert_eq!(published["task"]["id"], "b-after-replacement");
    let rejected = fixture.command_public(
        &[
            "board",
            "invite",
            "b-after-replacement",
            "--to",
            &worker_a,
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("BOARD_MASTER_REQUIRED"));
    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            "b-after-replacement",
            "--to",
            &worker_a,
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    assert_eq!(invited["task"]["status"], "invited");

    // The replaced holder keeps its own peer row, task and mailbox while it
    // loses only control authority.
    fixture.run_public(
        &[
            "task",
            "register",
            "a-preserved-task",
            "--next",
            "survive replacement",
        ],
        THREAD_A,
    );
    let preserved = fixture.run_public(
        &[
            "send",
            "--to",
            &worker_a,
            "--subject",
            "preserve replaced holder",
            "keep the mailbox",
        ],
        THREAD_B,
    );
    let preserved_message = preserved["msg_id"]
        .as_str()
        .or_else(|| preserved["message_id"].as_str())
        .expect("send names the durable message")
        .to_owned();
    let context_a = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(
        context_a["identity"]["worker_id"], worker_a,
        "replacement must preserve the replaced holder's peer identity: {context_a}"
    );
    assert!(
        context_a["tasks"]
            .as_array()
            .expect("context lists tasks")
            .iter()
            .any(|task| task["id"] == "a-preserved-task"),
        "replacement must preserve the replaced holder's task: {context_a}"
    );
    assert!(
        context_a["inbox"]["messages"]
            .as_array()
            .expect("context lists inbox messages")
            .iter()
            .any(|message| message["id"].as_str() == Some(preserved_message.as_str())),
        "replacement must preserve the replaced holder's mailbox: {context_a}"
    );

    // The AppServer delivery path still works after the authority change.
    let sent = fixture.run_public(
        &[
            "send",
            "--to",
            &worker_a,
            "--subject",
            "after replacement",
            "deliver to the replaced peer",
        ],
        THREAD_B,
    );
    assert_eq!(sent["notification"], "appserver-input-submitted");
    assert_eq!(sent["consumed"], false);

    drop(fixture);
}

#[test]
fn repeated_appserver_context_reuses_the_same_identity_and_binding() {
    let mut fixture = AppFixture::new();
    let context_a = fixture.run_public(&["context"], THREAD_A);
    assert!(
        context_a["identity"]["worker_id"].as_str().is_some(),
        "daemon receipt must name the AppServer worker"
    );
    let repeated = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(
        repeated["identity"]["worker_id"],
        context_a["identity"]["worker_id"]
    );
    assert_eq!(repeated["binding"], context_a["binding"]);
    assert_eq!(
        fixture.run_public(&["who"], THREAD_A)["workers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    drop(fixture);
}

#[test]
fn context_partial_native_facts_require_the_missing_endpoint_fields() {
    let mut fixture = AppFixture::new();

    // No explicit AppServer endpoint: the TUI originator still yields a
    // namespace, but the native socket is absent, so the endpoint is the one
    // fact the caller must supply. The already-observed facts are not
    // re-requested.
    let partial = fixture.command_tui_without_explicit_endpoint(&["context"], THREAD_A);
    assert!(
        !partial.status.success(),
        "missing endpoint returns a typed result"
    );
    let partial: Value = serde_json::from_slice(&partial.stdout).unwrap();
    assert_eq!(
        partial["result"]["requires"]["fields"],
        json!(["endpoint"]),
        "observed session/thread facts must not be re-requested: {partial}"
    );
    assert_eq!(partial["result"]["outcome"], "missing_facts");

    // Supplying a conflicting session/thread must be rejected before the daemon
    // creates any identity: a supplement may fill gaps, never override the
    // caller's real anchor.
    let invalid = fixture.command_tui_without_explicit_endpoint(
        &[
            "context",
            "--provide",
            "{\"session_id\":\"different\",\"thread_id\":\"different\"}",
        ],
        THREAD_A,
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&invalid.stdout),
        String::from_utf8_lossy(&invalid.stderr)
    );
    assert!(!invalid.status.success(), "{combined}");
    assert!(
        combined.contains("IDENTITY_FACT_CONFLICT"),
        "supplied facts must not override the observed caller anchor: {combined}"
    );
    let identities = fixture.host_state.join("identities");
    let created = identities.exists()
        && std::fs::read_dir(&identities)
            .expect("read isolated identities dir")
            .next()
            .is_some();
    assert!(
        !created,
        "a conflicting supplement must not create an identity: {identities:?}"
    );
    drop(fixture);
}

/// One complete supplement, supplied through the real CLI to the real daemon,
/// must finish the whole bootstrap: identity, runtime binding and default
/// lease. A plain replay of the same actual anchor must then return the same
/// worker and generation with no second identity.
#[test]
fn one_full_supplement_establishes_and_then_replays_the_same_identity() {
    let mut fixture = AppFixture::new();

    let missing = fixture.command_without_native_facts(&["context"]);
    assert!(
        !missing.status.success(),
        "no-anchor context returns a typed missing-facts result"
    );
    let missing: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(missing["result"]["outcome"], "missing_facts");
    assert_eq!(
        missing["result"]["requires"]["fields"],
        json!(["session_id", "thread_id", "endpoint", "namespace"])
    );

    let endpoint = format!("unix://{}", fixture.app_socket.display());
    let supplement = format!(
        "{{\"session_id\":\"{THREAD_A}\",\"thread_id\":\"{THREAD_A}\",\"endpoint\":\"{endpoint}\",\"namespace\":\"codex_tui\"}}"
    );
    let registered = fixture.command_without_native_facts(&["context", "--provide", &supplement]);
    assert_context_display_is_public("full-supplement context", &registered);
    assert!(
        registered.status.success(),
        "a complete real supplement must register: stdout={} stderr={}",
        String::from_utf8_lossy(&registered.stdout),
        String::from_utf8_lossy(&registered.stderr)
    );
    let registered_response: Value = serde_json::from_slice(&registered.stdout).unwrap();
    let registered = registered_response["result"]["snapshot"].clone();
    assert_eq!(
        registered["registered"], true,
        "complete supplement must register: {registered}"
    );
    let worker = registered["identity"]["worker_id"]
        .as_str()
        .expect("registered receipt names the daemon-owned worker")
        .to_owned();
    assert_eq!(
        registered["identity"]["transport"]["kind"], "appserver",
        "the verified endpoint must be the selected transport: {registered}"
    );
    assert_eq!(
        registered["identity"]["transport"]["namespace"],
        "codex_tui"
    );
    let binding = registered["binding"].clone();
    assert!(
        registered["subscriptions"]
            .as_array()
            .expect("context lists subscriptions")
            .iter()
            .any(|subscription| subscription["event"] == "direct-message"
                && subscription["status"] == "armed"),
        "the default direct-message lease must be armed after registration: {registered}"
    );

    // Repeating current facts retains the same worker and binding.
    let replay = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(replay["identity"]["worker_id"], worker);
    assert_eq!(replay["binding"], binding);

    drop(fixture);
}

#[test]
fn first_context_requires_thread_and_current_map_restores_without_it() {
    let mut fixture = AppFixture::new();
    let missing = fixture.command_without_thread(&["context"], THREAD_A);
    assert!(!missing.status.success());
    let missing: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(
        missing["result"]["requires"]["fields"],
        json!(["thread_id"])
    );
    let supplement = format!("{{\"thread_id\":\"{THREAD_A}\"}}");
    let registered =
        fixture.command_without_thread(&["context", "--provide", &supplement], THREAD_A);
    assert_context_display_is_public("thread supplement", &registered);
    assert!(
        registered.status.success(),
        "{}",
        String::from_utf8_lossy(&registered.stderr)
    );
    let registered_response: Value = serde_json::from_slice(&registered.stdout).unwrap();
    let registered = registered_response["result"]["snapshot"].clone();
    assert_eq!(registered["registered"], true);
    let missing_again = fixture.command_without_thread(&["context"], THREAD_A);
    assert!(missing_again.status.success());
    let missing_again: Value = serde_json::from_slice(&missing_again.stdout).unwrap();
    assert_eq!(missing_again["result"]["outcome"], "completed");
    assert_eq!(
        missing_again["result"]["snapshot"]["binding"],
        registered["binding"]
    );
    assert_eq!(
        missing_again["result"]["snapshot"]["identity"],
        registered["identity"]
    );
    for args in [
        vec!["context", "--provide", &supplement],
        vec!["task", "status"],
    ] {
        let output = fixture.command_without_thread(&args, THREAD_A);
        assert!(
            output.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        if args[0] == "context" {
            let value = value["result"]["snapshot"].clone();
            assert_eq!(value["registered"], true);
            assert_eq!(value["identity"], registered["identity"]);
            assert_eq!(value["binding"], registered["binding"]);
        } else {
            assert_eq!(value["tasks"], json!([]));
        }
    }
    let tasks = fixture.command_tui_without_explicit_endpoint(&["task", "status"], THREAD_A);
    assert!(
        tasks.status.success(),
        "{}{}",
        String::from_utf8_lossy(&tasks.stdout),
        String::from_utf8_lossy(&tasks.stderr)
    );
    let tasks: Value = serde_json::from_slice(&tasks.stdout).unwrap();
    assert_eq!(tasks["tasks"], json!([]));
    let anonymous = fixture.command_without_native_facts(&["context"]);
    assert!(!anonymous.status.success());
    let anonymous: Value = serde_json::from_slice(&anonymous.stdout).unwrap();
    assert_eq!(
        anonymous["result"]["requires"]["fields"],
        json!(["session_id", "thread_id", "endpoint", "namespace"])
    );
    drop(fixture);
}

#[test]
fn session_only_identity_read_is_scoped_pure_and_ignores_historical_credentials() {
    let mut fixture = AppFixture::new();
    let registered = fixture.run_public(&["context"], THREAD_A);
    let worker = registered["identity"]["worker_id"].as_str().unwrap();
    let identity_path = fixture
        .host_state
        .join("identities")
        .join(worker)
        .join("identity.json");
    let credential = std::fs::read(&identity_path).unwrap();
    let mut historical: Value = serde_json::from_slice(&credential).unwrap();
    historical["worker_id"] = json!("historical-reader");
    historical["runtime"]["agent_id"] = json!("historical-reader");
    historical["runtime"]["binding_id"] = json!("binding-historical-reader");
    let history_path = fixture
        .host_state
        .join("identities/historical-reader/identity.json");
    std::fs::create_dir_all(history_path.parent().unwrap()).unwrap();
    std::fs::write(&history_path, serde_json::to_vec(&historical).unwrap()).unwrap();
    let routes = std::fs::read(fixture.host_state.join("routes.json")).unwrap();
    let mapping = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let connections = fixture.appserver_connections.load(Ordering::SeqCst);
    let tasks = fixture.command_without_thread(&["task", "status"], THREAD_A);
    assert!(
        tasks.status.success(),
        "{}{}",
        String::from_utf8_lossy(&tasks.stdout),
        String::from_utf8_lossy(&tasks.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&tasks.stdout).unwrap()["tasks"],
        json!([])
    );
    let with_thread = fixture.command_tui_without_explicit_endpoint(&["task", "status"], THREAD_A);
    assert!(
        with_thread.status.success(),
        "{}",
        String::from_utf8_lossy(&with_thread.stderr)
    );

    let root = std::fs::canonicalize(&fixture.project).unwrap();
    let context =
        json!({"app_scope_id":"appserver-cli", "canonical_root":root, "project_scope":root});
    let daemon_socket = fixture.host_state.join("server.sock");
    let call = |scope: Option<Value>, session: &str, thread: Option<&str>| {
        let mut request = json!({"op":"RouteResolveNative","session_id":session});
        if let Some(scope) = scope {
            request["project_context"] = scope;
        }
        if let Some(thread) = thread {
            request["native_thread_id"] = json!(thread);
        }
        let mut stream = UnixStream::connect(&daemon_socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        writeln!(stream, "{request}").unwrap();
        let mut line = String::new();
        std::io::BufReader::new(stream)
            .read_line(&mut line)
            .unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    let current = call(Some(context.clone()), THREAD_A, None);
    assert_eq!(current["ok"], true, "{current}");
    assert_eq!(current["agent_id"], worker);
    let missing_scope = call(None, THREAD_A, None);
    assert_eq!(missing_scope["ok"], false);
    assert!(missing_scope["error"]
        .as_str()
        .unwrap()
        .contains("PROJECT_CONTEXT_REQUIRED"));
    let mut other_app = context.clone();
    other_app["app_scope_id"] = json!("other-app");
    let other_root = fixture.root.join("other-project");
    std::fs::create_dir_all(&other_root).unwrap();
    let other_root = std::fs::canonicalize(other_root).unwrap();
    let other_project = json!({"app_scope_id":"appserver-cli","canonical_root":other_root,"project_scope":other_root});
    for (scope, session, thread) in [
        (other_app, THREAD_A, None),
        (other_project, THREAD_A, None),
        (context.clone(), "missing-session", None),
        (context.clone(), "", None),
        (context.clone(), THREAD_A, Some("wrong-thread")),
        (context.clone(), THREAD_A, Some("")),
    ] {
        let refused = call(Some(scope), session, thread);
        assert_eq!(refused["ok"], false, "{refused}");
    }
    assert_eq!(std::fs::read(&identity_path).unwrap(), credential);
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&history_path).unwrap()).unwrap(),
        historical
    );
    assert_eq!(
        std::fs::read(fixture.host_state.join("routes.json")).unwrap(),
        routes
    );
    assert_eq!(
        std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        mapping
    );
    // The explicit wrong-thread requests retain their old verification path;
    // neither session-only reads nor refused addresses contact AppServer.
    assert_eq!(
        fixture.appserver_connections.load(Ordering::SeqCst),
        connections
    );

    fixture.stop_daemon();
    let map_path = fixture.host_state.join("identity-map.json");
    let mut map: Value = serde_json::from_slice(&std::fs::read(&map_path).unwrap()).unwrap();
    let mut duplicate = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| {
            slot["identity_status"] == "active" && slot["current"]["binding"]["agent_id"] == worker
        })
        .unwrap()
        .clone();
    duplicate["identity_key"]["name"] = json!("Bob");
    duplicate["display_slot"] = json!(3);
    duplicate["current"]["binding"]["binding_id"] = json!("binding-conflicting-current");
    duplicate["current"]["binding"]["agent_id"] = json!("conflicting-current");
    duplicate["current"]["binding"]["runtime_id"] = json!("runtime-conflicting-current");
    duplicate["current"]["binding"]["native_thread_id"] = json!("conflicting-current-thread");
    duplicate["current"]["transport"]["thread_id"] = json!("conflicting-current-thread");
    duplicate["previous"] = Value::Null;
    let binding = duplicate["current"]["binding"].clone();
    let transport = duplicate["current"]["transport"].clone();
    map["slots"].as_array_mut().unwrap().push(duplicate);
    let state_path = fixture.project_state_for(&fixture.project, "appserver-cli");
    let mut document: Value = serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    let state = &mut document["state"];
    let mut duplicate_worker = state["workers"][worker].clone();
    duplicate_worker["id"] = json!("conflicting-current");
    duplicate_worker["token"] = json!("conflicting-current-token");
    duplicate_worker["transport"] = transport.clone();
    state["workers"]["conflicting-current"] = duplicate_worker;
    state["global"]["projects"][root.to_str().unwrap()]["runtime_bindings"]
        ["binding-conflicting-current"] = binding.clone();
    let mut duplicate_credential: Value = serde_json::from_slice(&credential).unwrap();
    duplicate_credential["worker_id"] = json!("conflicting-current");
    duplicate_credential["token"] = json!("conflicting-current-token");
    duplicate_credential["transport"] = transport;
    for field in [
        "agent_id",
        "runtime_id",
        "binding_id",
        "session_id",
        "native_thread_id",
    ] {
        duplicate_credential["runtime"][field] = binding[field].clone();
    }
    let duplicate_path = fixture
        .host_state
        .join("identities/conflicting-current/identity.json");
    std::fs::create_dir_all(duplicate_path.parent().unwrap()).unwrap();
    std::fs::write(
        duplicate_path,
        serde_json::to_vec(&duplicate_credential).unwrap(),
    )
    .unwrap();
    std::fs::write(state_path, serde_json::to_vec(&document).unwrap()).unwrap();
    std::fs::write(map_path, serde_json::to_vec(&map).unwrap()).unwrap();
    fixture.start_daemon();
    let ambiguous = call(Some(context), THREAD_A, None);
    assert_eq!(ambiguous["ok"], false, "{ambiguous}");
    assert!(
        ambiguous["error"]
            .as_str()
            .unwrap()
            .contains("ROUTE_RESOLVE_AMBIGUOUS"),
        "{ambiguous}"
    );
}

#[test]
fn external_linked_worktree_reuses_the_canonical_identity_without_recovery_calls() {
    let mut fixture = AppFixture::new();
    let canonical = fixture.project.clone();
    let context = fixture.run_public(&["context"], THREAD_A);
    let linked = fixture.root.join("external-linked");
    // A task directory binds to the project through an explicit `.agent-collab`
    // marker, not through git worktree metadata. The marker may be a symlink.
    std::fs::create_dir_all(&linked).unwrap();
    std::os::unix::fs::symlink(
        canonical.join(".agent-collab"),
        linked.join(".agent-collab"),
    )
    .unwrap();
    fixture.project = linked;
    let replay = fixture.command_public(&["context"], THREAD_A);
    let tasks = fixture.command_without_thread(&["task", "status"], THREAD_A);
    let worktree_marker = std::fs::symlink_metadata(fixture.project.join(".agent-collab"))
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    // Restore the daemon's lifecycle cwd before assertions so a failing
    // regression still stops its own isolated daemon in Fixture::drop.
    fixture.project = canonical;
    assert!(
        replay.status.success(),
        "{}{}",
        String::from_utf8_lossy(&replay.stdout),
        String::from_utf8_lossy(&replay.stderr)
    );
    let replay_response: Value = serde_json::from_slice(&replay.stdout).unwrap();
    let replay = replay_response["result"]["snapshot"].clone();
    assert_eq!(replay["project_root"], context["project_root"]);
    assert_eq!(replay["identity"], context["identity"]);
    assert_eq!(replay["binding"], context["binding"]);
    assert!(
        tasks.status.success(),
        "{}{}",
        String::from_utf8_lossy(&tasks.stdout),
        String::from_utf8_lossy(&tasks.stderr)
    );
    let tasks: Value = serde_json::from_slice(&tasks.stdout).unwrap();
    assert_eq!(tasks["tasks"], json!([]));
    assert!(
        worktree_marker,
        "a task directory must use the project marker symlink, not a new baseline"
    );
    drop(fixture);
}

#[test]
fn forged_appserver_endpoint_and_missing_project_context_fail_closed() {
    let fixture = AppFixture::new();
    let root = fixture.root.clone();
    let project = fixture.project.clone();
    let host_state = fixture.host_state.clone();
    let forged = root.join("forged.sock");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&host_state).unwrap();
    seed_baseline(&project);

    let output = Command::new(binary())
        .args([
            "context",
            "--provide",
            &format!(
                "{{\"session_id\":\"{THREAD_A}\",\"thread_id\":\"{THREAD_A}\",\"endpoint\":\"unix://{}\",\"namespace\":\"codex_tui\"}}",
                forged.display()
            ),
        ])
        .current_dir(&project)
        .env("COLLAB_STATE_DIR", &host_state)
        .env("HOME", root.join("home"))
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
        .env("CODEX_HOME", root.join("home"))
        .env_remove("COLLAB_APPSERVER_SOCKET")
        .env_remove("CODEX_APP_SERVER_SOCKET")
        .env_remove("COLLAB_APPSERVER_NAMESPACE")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("COLLAB_WORKER")
        .output()
        .expect("run context with forged endpoint");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{combined}");
    assert!(
        combined.contains("APPSERVER_ENDPOINT_REJECTED")
            || combined.contains("endpoint")
            || combined.contains("RouteUnavailable"),
        "{combined}"
    );
    assert!(
        !host_state.join("identities").exists(),
        "forged endpoint must not mint an identity"
    );

    // A request envelope that omits ProjectContext is rejected by the daemon
    // before identity selection. This mirrors the public wire contract without
    // reaching into daemon state.
    let server_socket = host_state.join("server.sock");
    if server_socket.exists() {
        let mut stream = UnixStream::connect(&server_socket).expect("connect isolated daemon");
        stream
            .write_all(b"{\"op\":\"IdentityContext\",\"facts\":{}}\n")
            .unwrap();
        let mut reader = std::io::BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(
            line.contains("PROJECT_CONTEXT_REQUIRED"),
            "missing ProjectContext must fail explicitly: {line}"
        );
    }
}
