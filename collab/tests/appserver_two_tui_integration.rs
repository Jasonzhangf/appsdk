use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[path = "board/cli.rs"]
mod board_cli;
#[path = "board/central_storage.rs"]
mod central_storage;
#[path = "board/ceo_patrol_runtime.rs"]
mod ceo_patrol_runtime;
#[path = "board/context_minimality.rs"]
mod context_minimality;
#[path = "board/degraded_user_maintenance.rs"]
mod degraded_user_maintenance;
#[path = "board/name_adjudication.rs"]
mod name_adjudication;
#[path = "board/name_confirmed_takeover.rs"]
mod name_confirmed_takeover;
#[path = "board/name_runtime.rs"]
mod name_runtime_cli;
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
    fixture.append_name_events(&[json!({"ev":"AgentPresenceObserved", "presence":{
        "project_scope":std::fs::canonicalize(&fixture.project).unwrap(),
        "worker_id":alice["identity"]["worker_id"], "session_id":THREAD_A,
        "presence":"online", "reason":"isolated fixture", "observed_ms":1
    }})]);
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
    assert_eq!(first["name_resolution"]["outcome"], "committed");
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
    assert_eq!(master["name_resolution"]["outcome"], "committed");
}

#[test]
fn context_names_unknown_keeps_peer_and_query_is_read_only() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);
    let output = fixture.command_public(&["context", "--op", "names-pending",
        "--approve-grant", r#"{"action":"restore_name","requested_name":"Alice","authorization":"restore my named work"}"#], THREAD_B);
    let pending: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(pending["ok"], false, "{pending}");
    assert_eq!(
        pending["result"]["snapshot"]["name_resolution"]["outcome"], "pending",
        "{pending}"
    );
    assert_eq!(pending["result"]["snapshot"]["name"], "Bob");
    assert_eq!(
        pending["result"]["snapshot"]["identity"]["worker_id"],
        bob["identity"]["worker_id"]
    );
    fixture.restart_daemon();
    let query = fixture.command_public(&["context", "--op", "names-pending", "--query"], THREAD_B);
    let query: Value = serde_json::from_slice(&query.stdout).unwrap();
    assert_eq!(
        query["result"]["queried_operation"]["name_resolution"]["outcome"], "pending",
        "{query}"
    );
    let after = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(
        after["identity"]["worker_id"],
        alice["identity"]["worker_id"]
    );
    assert_eq!(after["name"], "Alice");
}

#[test]
fn context_names_current_worker_session_continues_without_another_approval() {
    let mut fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    let worker = first["identity"]["worker_id"].as_str().unwrap();
    let mut events = Vec::new();
    for (id, status) in [
        ("open-a", "working"),
        ("open-b", "accepted"),
        ("closed", "closed"),
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
    fixture
        .state
        .lock()
        .unwrap()
        .get_mut(THREAD_A)
        .unwrap()
        .session_id = Some(THREAD_B.into());
    let output = fixture.command_public_with_session(&["context"], THREAD_B, THREAD_A);
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["ok"], true, "{result}");
    let snapshot = &result["result"]["snapshot"];
    assert_eq!(
        snapshot["identity"]["worker_id"],
        first["identity"]["worker_id"]
    );
    assert_eq!(snapshot["name"], "Alice");
    assert_eq!(snapshot["session_id"], THREAD_B, "{snapshot}");
    assert_eq!(snapshot["binding"]["session_id"], THREAD_B);
    let events = std::fs::read_to_string(fixture.name_journal()).unwrap();
    let resolution: Value = events
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["ev"] == "NameResolutionCommitted")
        .last()
        .unwrap();
    let assignments = resolution["assignments"].as_array().unwrap();
    assert_eq!(assignments.len(), 2, "{resolution}");
    for assignment in assignments {
        assert_ne!(assignment["task_id"], "closed");
        assert_eq!(assignment["session_id"], THREAD_B);
        assert_eq!(assignment["assignment_revision"], 2);
    }
}

#[test]
fn context_names_online_replacement_needs_confirmation_and_keeps_old_executor() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.append_name_events(&[json!({"ev":"AgentPresenceObserved", "presence":{
        "project_scope":std::fs::canonicalize(&fixture.project).unwrap(),
        "worker_id":alice["identity"]["worker_id"], "session_id":THREAD_A,
        "presence":"online", "reason":"isolated current agent fact", "observed_ms":1
    }})]);
    let output = fixture.command_public(&["context", "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Alice","authorization":"restore my work"}"#], THREAD_B);
    let refused: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(
        refused["result"]["snapshot"]["name_resolution"]["outcome"], "confirmation_required",
        "{refused}"
    );
    assert_eq!(refused["result"]["snapshot"]["name"], "Bob");
    let restored = fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Alice","authorization":"replace online holder","replace_confirmed":true}"#], THREAD_B);
    assert_eq!(restored["name"], "Alice");
    let events = std::fs::read_to_string(fixture.name_journal()).unwrap();
    assert!(!events.contains("WorkerClosed"));
    assert!(!events.contains("GlobalMasterRevoked"));
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
    let attempted = fixture.command_public(
        &["master", "promote", "--approval", "user requested Jenny"],
        THREAD_B,
    );
    assert!(!attempted.status.success(), "{:?}", attempted);
    let still_master = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(still_master["name"], "Jenny");
    assert_eq!(
        still_master["master"]["worker_id"],
        alice["identity"]["worker_id"]
    );
    let legacy = fixture.command_public(&["worker", "name", "Carol"], THREAD_A);
    assert!(!legacy.status.success(), "{:?}", legacy);
    assert!(String::from_utf8_lossy(&legacy.stderr).contains("WORKER_NAME_RESTORE_REQUIRED"));
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
    assert_eq!(peer["name_resolution"]["outcome"], "committed");
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
    state: Arc<Mutex<HashMap<String, ThreadState>>>,
    appserver_connections: Arc<AtomicU64>,
    daemon: Option<Child>,
    _server_thread: thread::JoinHandle<()>,
}

impl AppFixture {
    fn name_journal(&self) -> PathBuf {
        let project = std::fs::canonicalize(&self.project).unwrap();
        let record: Value = std::fs::read_to_string(self.host_state.join("routes.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|record| record["project_scope"] == project.to_string_lossy().as_ref())
            .unwrap();
        PathBuf::from(record["storage_root"].as_str().unwrap())
            .join(".agent-collab/server/journal.jsonl")
    }

    fn append_name_events(&mut self, events: &[Value]) {
        self.stop_daemon();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(self.name_journal())
            .unwrap();
        for event in events {
            writeln!(file, "{event}").unwrap();
        }
        file.sync_all().unwrap();
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
        std::fs::create_dir_all(&project).expect("create project root");
        std::fs::create_dir_all(&host_state).expect("create host state root");
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
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("HOME", self.root.join("home"))
            .env("CODEX_HOME", self.root.join("home"))
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("COLLAB_WORKER")
            .stdin(std::process::Stdio::null())
            .stdout(daemon_log.try_clone().expect("clone serve log"))
            .stderr(daemon_log);
        self.daemon = Some(command.spawn().expect("spawn owned serve child"));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.host_state.join("server.sock").exists() && Instant::now() < deadline {
            if let Some(status) = self.daemon.as_mut().unwrap().try_wait().unwrap() {
                panic!(
                    "test-owned serve exited before readiness: {status}: {}",
                    std::fs::read_to_string(self.root.join("test-daemon.log")).unwrap_or_default()
                );
            }
            thread::sleep(Duration::from_millis(25));
        }
        assert!(
            self.host_state.join("server.sock").exists(),
            "owned serve did not create socket"
        );
    }

    fn restart_daemon(&mut self) {
        self.stop_daemon();
        self.start_daemon();
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
    reject_input: bool,
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
            reject_input: false,
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
        let response = dispatch(method, params, id, &state, project_root);
        if write_frame(&mut stream, &response).is_err() {
            break;
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

    // Restore notifications and replay a historical cancellation of the
    // default lease. The daemon reconciles the lease and binds the pending
    // immediate report; the automatic attempt owner may submit it only once.
    fixture.stop_daemon();
    std::fs::write(&config_path, "[notifications]\nenabled=true\n").unwrap();
    fixture.append_name_events(&[json!({
        "ev": "NotificationSuppressed",
        "subscription_id": default_id,
        "status": "cancelled",
        "reason": "explicit-unsubscribe",
        "updated_ms": 1
    })]);

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let events: Vec<Value> = std::fs::read_to_string(fixture.name_journal())
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let accepted = events.iter().any(|event| {
            event["ev"] == "NotificationDeliveryAccepted"
                && event["message_id"] == message_id
                && event["evidence"].is_object()
        });
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
    let events: Vec<Value> = std::fs::read_to_string(fixture.name_journal())
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let immediate_events = events
        .iter()
        .filter(|event| {
            event["ev"] == "DeliveryMode"
                && event["msg_id"] == message_id
                && event["mode"] == "immediate"
        })
        .count();
    let accepted_events = events
        .iter()
        .filter(|event| {
            event["ev"] == "NotificationDeliveryAccepted" && event["message_id"] == message_id
        })
        .count();
    let attempted_events = events
        .iter()
        .filter(|event| {
            event["ev"] == "WakeAttempted"
                && event["ids"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id == &message_id))
        })
        .count();
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
        immediate_events, 1,
        "public sendmessage must persist immediate mode"
    );
    assert_eq!(
        attempted_events, 1,
        "the recovered report must be attempted once"
    );
    assert_eq!(
        accepted_events, 1,
        "the recovered report must be accepted once"
    );
    assert_eq!(
        observed_events, 1,
        "the recovered report must reach native input once"
    );

    // A later daemon restart and context reconciliation must not resubmit an
    // accepted attempt or spend more of its at-most-once budget.
    fixture.restart_daemon();
    fixture.run_public(&["context"], THREAD_B);
    thread::sleep(Duration::from_millis(1300));
    let events: Vec<Value> = std::fs::read_to_string(fixture.name_journal())
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let accepted_after_replay = events
        .iter()
        .filter(|event| {
            event["ev"] == "NotificationDeliveryAccepted" && event["message_id"] == message_id
        })
        .count();
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
        accepted_after_replay, 1,
        "reconciliation must not replay an accepted report"
    );
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

    fixture.append_name_events(&[json!({"ev":"AgentPresenceObserved", "presence":{
        "project_scope":project_scope, "worker_id":worker_a, "session_id":THREAD_A,
        "presence":"online", "reason":"current online holder fixture", "observed_ms":1
    }})]);
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
    let generation = registered["binding"]["endpoint_generation"].clone();
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

    // The same real native anchor replayed through the automatic path returns
    // the same worker and generation, so the supplement is idempotent.
    let replay = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(replay["identity"]["worker_id"], worker);
    assert_eq!(replay["binding"]["endpoint_generation"], generation);
    assert_eq!(replay["binding"], binding);

    drop(fixture);
}

#[test]
fn every_context_requires_current_thread_when_environment_omits_it() {
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
    assert!(!missing_again.status.success());
    let missing_again: Value = serde_json::from_slice(&missing_again.stdout).unwrap();
    assert_eq!(missing_again["result"]["outcome"], "missing_facts");
    assert_eq!(
        missing_again["result"]["requires"]["fields"],
        json!(["thread_id"])
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
    let routes = std::fs::read(fixture.host_state.join("routes.jsonl")).unwrap();
    let operations = std::fs::read_dir(fixture.host_state.join("context-operations"))
        .unwrap()
        .count();
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
        std::fs::read(fixture.host_state.join("routes.jsonl")).unwrap(),
        routes
    );
    assert_eq!(
        std::fs::read_dir(fixture.host_state.join("context-operations"))
            .unwrap()
            .count(),
        operations
    );
    // The explicit wrong-thread requests retain their old verification path;
    // neither session-only reads nor refused addresses contact AppServer.
    assert_eq!(
        fixture.appserver_connections.load(Ordering::SeqCst),
        connections
    );

    let mut duplicate: Value = std::fs::read_to_string(fixture.name_journal())
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["ev"] == "GlobalCurrentThreadRouteSet")
        .last()
        .expect("current native route event");
    duplicate["binding"]["binding_id"] = json!("binding-conflicting-current");
    duplicate["binding"]["agent_id"] = json!("conflicting-current");
    duplicate["binding"]["runtime_id"] = json!("runtime-conflicting-current");
    duplicate["binding"]["native_thread_id"] = json!("conflicting-current-thread");
    let bound = json!({"ev":"GlobalRuntimeBound","binding":duplicate["binding"]});
    fixture.append_name_events(&[bound, duplicate]);
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
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=Collab Test",
            "-c",
            "user.email=collab-test@example.invalid",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "initial",
        ],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(&canonical)
            .status()
            .unwrap()
            .success());
    }
    let context = fixture.run_public(&["context"], THREAD_A);
    let linked = fixture.root.join("external-linked");
    assert!(Command::new("git")
        .args([
            "worktree",
            "add",
            "-q",
            "-b",
            "linked",
            linked.to_str().unwrap()
        ])
        .current_dir(&canonical)
        .status()
        .unwrap()
        .success());
    fixture.project = linked;
    let replay = fixture.command_public(&["context"], THREAD_A);
    let tasks = fixture.command_without_thread(&["task", "status"], THREAD_A);
    let worktree_baseline = fixture.project.join(".agent-collab").exists();
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
        !worktree_baseline,
        "a worktree must not create a second identity baseline"
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
