use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// The single binary-selection seam for this consumer fixture. `cargo test`
/// builds and runs the debug binary by default; a canonical installed binary
/// can be substituted with `COLLAB_TEST_BINARY` so the same public entry point
/// drives identical installed bytes. The fallback is test-only and never a
/// product behavior.
fn collab_test_binary() -> PathBuf {
    std::env::var_os("COLLAB_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_collab")))
}

struct Fixture {
    binary: PathBuf,
    root: PathBuf,
    host_state: PathBuf,
    tmux_socket: PathBuf,
    initialized: bool,
}

impl Fixture {
    fn command(&self, args: &[&str], pane: Option<&Pane>) -> Output {
        self.command_with_originator(args, pane, "Codex TUI")
    }

    fn command_with_originator(
        &self,
        args: &[&str],
        pane: Option<&Pane>,
        originator: &str,
    ) -> Output {
        self.configured_command(args, pane, originator)
            .output()
            .expect("run collab CLI")
    }

    /// Build (but do not run) the exact isolated-environment CLI invocation.
    /// Long-running subcommands such as `dashboard` need a spawned `Child`, not
    /// a captured `Output`.
    fn configured_command(&self, args: &[&str], pane: Option<&Pane>, originator: &str) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .args(args)
            .current_dir(&self.root)
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("CODEX_HOME", self.root.join("home"))
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("COLLAB_WORKER")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", originator);
        if let Some(pane) = pane {
            command
                .env(
                    "TMUX",
                    format!("{},{},0", self.tmux_socket.display(), pane.server_pid),
                )
                .env("TMUX_PANE", &pane.pane_id)
                .env("CODEX_SESSION_ID", &pane.session_anchor)
                .env("CODEX_THREAD_ID", &pane.thread_anchor);
        }
        command
    }

    fn run_ok(&self, args: &[&str], pane: Option<&Pane>) -> Value {
        let output = self.command(args, pane);
        assert!(
            output.status.success(),
            "collab {:?} failed: stdout={} stderr={}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("collab CLI emits JSON")
    }

    fn command_without_pane(&self, args: &[&str]) -> Output {
        self.command_with_originator(args, None, "Codex TUI")
    }

    fn run_context(&self, args: &[&str], pane: Option<&Pane>) -> Value {
        self.run_ok(args, pane)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.initialized {
            let _ = self.command(&["down"], None);
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && self.host_state.join("server.sock").exists() {
                thread::sleep(Duration::from_millis(50));
            }
        }
        let _ = Command::new("tmux")
            .args(["-S", self.tmux_socket.to_str().unwrap(), "kill-server"])
            .output();
        if std::thread::panicking() {
            eprintln!(
                "preserving failed e2e fixture for diagnosis: root={} host_state={}",
                self.root.display(),
                self.host_state.display()
            );
        } else {
            let _ = std::fs::remove_dir_all(&self.root);
            let _ = std::fs::remove_dir_all(&self.host_state);
        }
    }
}

#[derive(Clone)]
struct Pane {
    server_pid: u32,
    pane_id: String,
    session_anchor: String,
    thread_anchor: String,
}

fn tmux(socket: &Path, args: &[&str]) -> Output {
    let output = Command::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(args)
        .output()
        .expect("run isolated tmux command");
    assert!(
        output.status.success(),
        "tmux {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn unique_root() -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "ct{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Seed the project baseline marker. `collab context` resolves its project root
/// from the `.agent-collab` baseline or a git root; a temp fixture is neither,
/// so it seeds the marker instead of reaching into daemon state.
fn seed_baseline(root: &Path) {
    std::fs::create_dir_all(root.join(".agent-collab")).expect("seed .agent-collab baseline");
}

/// Build an isolated fixture that owns exactly one tmux pane. The pane carries
/// a Codex session/thread anchor so a tmux registration records both ids, which
/// is exactly the shape whose runtime liveness a tmux transport cannot answer.
fn single_pane_fixture(label: &str) -> (Fixture, Pane) {
    let root = unique_root();
    let host_state = root.join("h");
    let tmux_socket = root.join("t.sock");
    std::fs::create_dir_all(&host_state).expect("create isolated host state root");
    seed_baseline(&root);
    let mut fixture = Fixture {
        binary: collab_test_binary(),
        root: root.clone(),
        host_state,
        tmux_socket: tmux_socket.clone(),
        initialized: false,
    };
    tmux(
        &tmux_socket,
        &["new-session", "-d", "-s", label, "sleep 600"],
    );
    let server_pid =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pid}"]).stdout)
            .expect("tmux server pid is utf8")
            .trim()
            .parse::<u32>()
            .expect("tmux server pid is numeric");
    let pane_id =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pane_id}"]).stdout)
            .expect("pane id is utf8")
            .trim()
            .to_owned();
    fixture.initialized = true;
    let pane = Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: format!("session-{pane_id}"),
        thread_anchor: format!("thread-{pane_id}"),
    };
    (fixture, pane)
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

#[test]
fn collab_recv_cli_subprocess_commits_queryable_receipt_over_isolated_daemon() {
    let root = unique_root();
    let host_state = root.join("h");
    let tmux_socket = root.join("t.sock");
    std::fs::create_dir_all(&root).expect("create isolated project root");
    std::fs::create_dir_all(&host_state).expect("create isolated host state root");
    seed_baseline(&root);
    let mut fixture = Fixture {
        binary: collab_test_binary(),
        root: root.clone(),
        host_state,
        tmux_socket: tmux_socket.clone(),
        initialized: false,
    };

    tmux(
        &tmux_socket,
        &[
            "new-session",
            "-d",
            "-s",
            "collab-tmux-recv-e2e",
            "sleep 600",
        ],
    );
    tmux(
        &tmux_socket,
        &[
            "split-window",
            "-d",
            "-t",
            "collab-tmux-recv-e2e:0",
            "sleep 600",
        ],
    );
    let server_pid =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pid}"]).stdout)
            .expect("tmux server pid is utf8")
            .trim()
            .parse::<u32>()
            .expect("tmux server pid is numeric");
    let pane_ids =
        String::from_utf8(tmux(&tmux_socket, &["list-panes", "-a", "-F", "#{pane_id}"]).stdout)
            .expect("pane list is utf8");
    let pane_ids = pane_ids.lines().collect::<Vec<_>>();
    assert_eq!(pane_ids.len(), 2, "fixture owns exactly two panes");
    let pane = |pane_id: &str| Pane {
        server_pid,
        pane_id: pane_id.to_owned(),
        session_anchor: format!("session-{pane_id}"),
        thread_anchor: format!("thread-{pane_id}"),
    };
    let sender = pane(pane_ids[0]);
    let receiver = pane(pane_ids[1]);

    fixture.initialized = true;
    let unknown_host =
        fixture.command_with_originator(&["context"], Some(&sender), "Codex future host");
    assert!(
        unknown_host.status.success(),
        "context with a verified tmux candidate must return a snapshot or classified terminal: stdout={} stderr={}",
        String::from_utf8_lossy(&unknown_host.stdout),
        String::from_utf8_lossy(&unknown_host.stderr)
    );
    let unknown_host_registration: Value =
        serde_json::from_slice(&unknown_host.stdout).expect("collab CLI emits JSON");
    assert_eq!(
        unknown_host_registration["identity"]["transport"]["kind"],
        "tmux"
    );
    let receiver_context = fixture.run_context(&["context"], Some(&receiver));
    let sender_context = fixture.run_context(&["context"], Some(&sender));
    let receiver_id = receiver_context["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names receiver worker")
        .to_owned();
    let sender_id = sender_context["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names sender worker")
        .to_owned();

    let route = fixture.run_ok(
        &["route", "resolve", "--pane-id", &sender.pane_id],
        Some(&sender),
    );
    assert_eq!(
        route["tmux_endpoint"]["socket_path"],
        tmux_socket.to_string_lossy().as_ref()
    );
    assert_eq!(route["tmux_endpoint"]["server_pid"], server_pid);
    assert_eq!(route["tmux_endpoint"]["pane_id"], sender.pane_id);
    assert_eq!(route["session_id"], sender.session_anchor);
    assert_eq!(route["native_thread_id"], sender.thread_anchor);

    let sent = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &receiver_id,
            "--subject",
            "tmux recv e2e",
            "persist before consume",
        ],
        Some(&sender),
    );
    let message_id = sent["msg_id"]
        .as_str()
        .or_else(|| sent["message_id"].as_str())
        .expect("send response has durable message id")
        .to_owned();
    assert_eq!(
        sent["consumed"], false,
        "wake acceptance is not consumption"
    );

    let received = fixture.run_ok(
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "tmux-e2e-receive-1",
        ],
        Some(&receiver),
    );
    assert_eq!(received["count"], 1);
    assert_eq!(received["messages"][0]["id"], message_id);
    assert_eq!(received["receive_id"], "tmux-e2e-receive-1");

    let status = fixture.run_ok(&["msg", &message_id], Some(&sender));
    assert_eq!(status["consumed_by_recv"], true);
    assert_eq!(status["state"], "read");
    assert_eq!(sender_context["identity"]["worker_id"], sender_id);
    assert_eq!(receiver_context["identity"]["worker_id"], receiver_id);

    // Register an owned task and leave one durable message unread so the
    // restart proves tasks and mailbox survive, not only the identity.
    let task = fixture.run_ok(
        &[
            "task",
            "register",
            "tmux-e2e-task",
            "--next",
            "survive restart",
        ],
        Some(&sender),
    );
    assert_eq!(task["task"], "tmux-e2e-task");
    let preserved = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &receiver_id,
            "--subject",
            "preserve across restart",
            "durable mailbox",
        ],
        Some(&sender),
    );
    let preserved_id = preserved["msg_id"]
        .as_str()
        .or_else(|| preserved["message_id"].as_str())
        .expect("preserved send has a durable message id")
        .to_owned();

    // Daemon restart and lost local receipt: the same actual tmux anchor must
    // return the same worker and generation without a fresh identity.
    let sender_identity = sender_context["identity"].clone();
    let sender_generation = sender_context["binding"]["endpoint_generation"].clone();
    let down = fixture.command_without_pane(&["down"]);
    assert!(down.status.success(), "isolated fixture down must succeed");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && fixture.host_state.join("server.sock").exists() {
        thread::sleep(Duration::from_millis(50));
    }
    std::fs::remove_dir_all(fixture.host_state.join("identities")).unwrap();
    let up = fixture.command_without_pane(&["up"]);
    assert!(up.status.success(), "isolated fixture up must succeed");
    let restart_context = fixture.run_context(&["context"], Some(&sender));
    assert_eq!(restart_context["identity"]["worker_id"], sender_id);
    assert_eq!(
        restart_context["binding"]["endpoint_generation"],
        sender_generation
    );
    assert_eq!(
        restart_context["identity"], sender_identity,
        "same tmux anchor must restore the same identity"
    );
    assert!(
        restart_context["subscriptions"]
            .as_array()
            .expect("context lists the caller's subscriptions")
            .iter()
            .any(|subscription| subscription["event"] == "direct-message"
                && subscription["status"] == "armed"),
        "the default direct-message lease must be armed again after restart: {restart_context}"
    );

    // Mailbox and tasks are daemon state, not receipt state: they survive the
    // restart together with the recovered identity.
    let tasks = restart_context["tasks"]
        .as_array()
        .expect("context lists the caller's tasks");
    assert!(
        tasks
            .iter()
            .any(|task| task["id"] == "tmux-e2e-task" && task["owner"] == sender_id),
        "the owned task must survive the daemon restart: {restart_context}"
    );
    let preserved_recv = fixture.run_ok(
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "tmux-e2e-receive-2",
        ],
        Some(&receiver),
    );
    let preserved_ids = preserved_recv["messages"]
        .as_array()
        .expect("recv returns messages")
        .iter()
        .filter_map(|message| message["id"].as_str())
        .collect::<Vec<_>>();
    assert!(
        preserved_ids.contains(&preserved_id.as_str()),
        "unread mailbox must survive the daemon restart: {preserved_recv}"
    );

    // No `collab context` display path may leak the private credential or the
    // internal receipt: not the first registration, not the receipt-derived
    // reads, and not the lost-receipt restart rebuild.
    assert_context_display_is_public("unknown-host context", &unknown_host);
    assert_context_display_is_public(
        "sender context",
        &fixture.command(&["context"], Some(&sender)),
    );
    assert_context_display_is_public(
        "receiver context",
        &fixture.command(&["context"], Some(&receiver)),
    );
    assert_context_display_is_public(
        "restart context",
        &fixture.command(&["context"], Some(&sender)),
    );
}

#[test]
fn context_recovers_archived_master_in_the_same_live_pane() {
    let root = unique_root();
    let host_state = root.join("h");
    let tmux_socket = root.join("t.sock");
    std::fs::create_dir_all(&host_state).unwrap();
    seed_baseline(&root);
    let mut fixture = Fixture {
        binary: collab_test_binary(),
        root: root.clone(),
        host_state,
        tmux_socket: tmux_socket.clone(),
        initialized: false,
    };
    tmux(
        &tmux_socket,
        &[
            "new-session",
            "-d",
            "-s",
            "collab-pane-recovery",
            "sleep 600",
        ],
    );
    let server_pid =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pid}"]).stdout)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
    let pane_id =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pane_id}"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
    let old = Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: "session-before-restart".into(),
        thread_anchor: "thread-before-restart".into(),
    };
    fixture.initialized = true;
    // The daemon owns the worker id; the fixture must not assume a naming
    // scheme. Read the concrete worker from the registration receipt.
    let init = fixture.run_ok(&["init"], Some(&old));
    let worker_id = init["worker_id"]
        .as_str()
        .expect("init receipt names the daemon-owned worker")
        .to_owned();
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated master",
        ],
        Some(&old),
    );
    let archive = fixture
        .host_state
        .join("archives/identities-retired-1")
        .join(&worker_id);
    std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
    std::fs::rename(
        fixture.host_state.join("identities").join(&worker_id),
        &archive,
    )
    .unwrap();
    let new = Pane {
        session_anchor: "session-after-restart".into(),
        thread_anchor: "thread-after-restart".into(),
        ..old
    };
    let recovered = fixture.run_ok(&["context"], Some(&new));
    assert_eq!(recovered["identity"]["worker_id"], worker_id);
    assert_eq!(recovered["identity"]["role"], "master");
    assert_eq!(recovered["registered"], true);
    let again = fixture.run_ok(&["context"], Some(&new));
    assert_eq!(again["identity"]["worker_id"], worker_id);
    assert_eq!(
        again["binding"]["endpoint_generation"],
        recovered["binding"]["endpoint_generation"]
    );

    let sent = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &worker_id,
            "--subject",
            "recovery",
            "consume me",
        ],
        Some(&new),
    );
    let message_id = sent["msg_id"]
        .as_str()
        .or_else(|| sent["message_id"].as_str())
        .unwrap();
    let received = fixture.run_ok(
        &["recv", "--timeout", "0", "--receive-id", "recovery-recv-1"],
        Some(&new),
    );
    assert_eq!(received["messages"][0]["id"], message_id);
    let status = fixture.run_ok(&["msg", message_id], Some(&new));
    assert_eq!(status["consumed_by_recv"], true);
}

/// The implicit bootstrap path — `collab context` run from a pane with no
/// `--worker` and no `COLLAB_WORKER` — resolves its durable identity from the
/// live anchor and can then be rejected by the daemon. The terminal must name
/// that resolved identity: before, it echoed the caller's optional `--worker`,
/// which is absent on this path, so the agent got `worker_id: null` plus a
/// `<worker_id>` placeholder and could not run the repair it was handed.
#[test]
fn implicit_context_names_the_resolved_identity_when_the_daemon_rejects_its_token() {
    let root = unique_root();
    let host_state = root.join("h");
    let tmux_socket = root.join("t.sock");
    std::fs::create_dir_all(&root).expect("create isolated project root");
    std::fs::create_dir_all(&host_state).expect("create isolated host state root");
    seed_baseline(&root);
    let mut fixture = Fixture {
        binary: collab_test_binary(),
        root: root.clone(),
        host_state: host_state.clone(),
        tmux_socket: tmux_socket.clone(),
        initialized: false,
    };
    tmux(
        &tmux_socket,
        &[
            "new-session",
            "-d",
            "-s",
            "collab-implicit-context",
            "sleep 600",
        ],
    );
    let server_pid =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pid}"]).stdout)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
    let pane_id =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pane_id}"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
    let pane = Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: "session-implicit".into(),
        thread_anchor: "thread-implicit".into(),
    };
    fixture.initialized = true;
    // The daemon owns the worker id; read the concrete value from the init
    // receipt instead of re-deriving the old naming scheme.
    let init = fixture.run_ok(&["init"], Some(&pane));
    let worker_id = init["worker_id"]
        .as_str()
        .expect("init receipt names the daemon-owned worker")
        .to_owned();

    // Durable identity exists and is registered; now make its token unusable so
    // the daemon rejects the first authenticated call.
    let identity_path = host_state
        .join("identities")
        .join(&worker_id)
        .join("identity.json");
    let mut identity: Value =
        serde_json::from_slice(&std::fs::read(&identity_path).unwrap()).unwrap();
    identity["token"] = Value::String("not-the-recorded-token".into());
    std::fs::write(
        &identity_path,
        serde_json::to_vec_pretty(&identity).unwrap(),
    )
    .unwrap();

    // Implicit: no `--worker`, and COLLAB_WORKER is deliberately removed while
    // the tmux anchor stays, so the identity is resolved from the anchor alone.
    let output = Command::new(&fixture.binary)
        .arg("context")
        .current_dir(&fixture.root)
        .env("COLLAB_STATE_DIR", &fixture.host_state)
        .env(
            "TMUX",
            format!("{},{},0", tmux_socket.display(), pane.server_pid),
        )
        .env("TMUX_PANE", &pane.pane_id)
        .env("CODEX_SESSION_ID", &pane.session_anchor)
        .env("CODEX_THREAD_ID", &pane.thread_anchor)
        .output()
        .expect("run implicit collab context");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "a rejected stored credential must fail: stdout={stdout} stderr={stderr}"
    );
    assert!(stderr.contains("TOKEN_MISMATCH:"), "{stderr}");
    assert!(
        stderr.contains(&worker_id),
        "the error must name the rejected identity: {stderr}"
    );
    assert!(
        !stderr.contains("IDENTITY_INFORMATION_REQUIRED"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("not-the-recorded-token"),
        "the rejected credential must not leak"
    );
    let after: Value = serde_json::from_slice(&std::fs::read(&identity_path).unwrap()).unwrap();
    assert_eq!(
        after["token"], identity["token"],
        "the daemon must not replace a rejected credential"
    );
}

/// `collab context` is the bootstrap read, so it is also where a peer reads
/// back its own runtime binding. A peer that loses the registration receipt
/// otherwise cannot address its route again: the endpoint generation then
/// appears only inside a rejection message, and recovering from there would
/// mean inferring a control value from an error.
#[test]
fn context_returns_the_binding_receipt_that_addresses_the_route() {
    let root = unique_root();
    let host_state = root.join("h");
    let tmux_socket = root.join("t.sock");
    std::fs::create_dir_all(&host_state).unwrap();
    seed_baseline(&root);
    let mut fixture = Fixture {
        binary: collab_test_binary(),
        root: root.clone(),
        host_state,
        tmux_socket: tmux_socket.clone(),
        initialized: false,
    };
    tmux(
        &tmux_socket,
        &[
            "new-session",
            "-d",
            "-s",
            "collab-context-binding",
            "sleep 600",
        ],
    );
    let server_pid =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pid}"]).stdout)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
    let pane_id =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pane_id}"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
    let pane = Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: "session-binding".into(),
        thread_anchor: "thread-binding".into(),
    };
    fixture.initialized = true;
    let registered = fixture.run_ok(&["context"], Some(&pane));
    let worker_id = registered["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names the registered worker")
        .to_owned();

    let context = fixture.run_ok(&["context"], Some(&pane));
    let binding = &context["binding"];
    let canonical_root = root.canonicalize().unwrap().to_string_lossy().into_owned();
    assert_eq!(binding["agent_id"], worker_id);
    assert_eq!(binding["project_scope"], canonical_root);
    assert!(
        binding["app_scope_id"]
            .as_str()
            .is_some_and(|scope| !scope.is_empty()),
        "the receipt must name its app scope: {context}"
    );
    assert_eq!(binding["session_id"], pane.session_anchor);
    assert_eq!(binding["native_thread_id"], pane.thread_anchor);
    assert!(
        binding["runtime_id"]
            .as_str()
            .is_some_and(|runtime_id| !runtime_id.is_empty()),
        "the receipt must carry a runtime id"
    );
    assert_eq!(registered["binding"]["runtime_id"], binding["runtime_id"]);
    assert!(
        binding["binding_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "the receipt must name its binding: {context}"
    );
    assert!(
        binding["endpoint_generation"]
            .as_u64()
            .is_some_and(|generation| generation >= 1),
        "the receipt must carry a usable endpoint generation: {context}"
    );

    // The receipt is the daemon's record, not a copy of the caller's local
    // state: reading it again returns the same binding.
    let again = fixture.run_ok(&["context"], Some(&pane));
    assert_eq!(again["binding"], *binding);
}

#[test]
fn context_missing_facts_and_invalid_supplement_have_no_identity_side_effect() {
    let root = unique_root();
    let host_state = root.join("h");
    std::fs::create_dir_all(&host_state).unwrap();
    seed_baseline(&root);
    let mut fixture = Fixture {
        binary: collab_test_binary(),
        root: root.clone(),
        host_state: host_state.clone(),
        tmux_socket: root.join("unused.sock"),
        initialized: false,
    };
    fixture.initialized = true;

    // A caller with no anchor at all: no tmux pane, no session/thread, and an
    // unsupported originator so no namespace is observed either. The daemon
    // must name every absent factual field and must not guess a worker.
    let no_anchor = fixture.command_with_originator(&["context"], None, "Codex future host");
    assert!(
        no_anchor.status.success(),
        "missing facts are a classified success terminal: stdout={} stderr={}",
        String::from_utf8_lossy(&no_anchor.stdout),
        String::from_utf8_lossy(&no_anchor.stderr)
    );
    let no_anchor_snapshot: Value = serde_json::from_slice(&no_anchor.stdout).unwrap();
    assert_eq!(no_anchor_snapshot["registered"], false);
    let update = &no_anchor_snapshot["requires_identity_update"];
    assert_eq!(update["reason"], "IDENTITY_INFORMATION_REQUIRED");
    assert_eq!(
        update["required_fields"],
        json!(["session_id", "thread_id", "endpoint", "namespace"]),
        "no-anchor context returns every exact missing native fact: {no_anchor_snapshot}"
    );
    assert!(
        update["worker_id"].is_null(),
        "missing-facts terminal must not guess a worker"
    );
    assert_eq!(
        update["action"],
        json!("collab context --provide '<JSON containing required_fields>'")
    );

    // A partial anchor: the runtime is a recognized TUI, so namespace is
    // observed from the originator. The daemon must not re-request observed
    // facts; it asks only for the session, thread and endpoint it still lacks.
    let partial = fixture.command(&["context"], None);
    assert!(
        partial.status.success(),
        "partial native facts are a classified success terminal: stdout={} stderr={}",
        String::from_utf8_lossy(&partial.stdout),
        String::from_utf8_lossy(&partial.stderr)
    );
    let partial_snapshot: Value = serde_json::from_slice(&partial.stdout).unwrap();
    assert_eq!(partial_snapshot["registered"], false);
    assert_eq!(
        partial_snapshot["requires_identity_update"]["required_fields"],
        json!(["session_id", "thread_id", "endpoint"]),
        "observed namespace must not be re-requested: {partial_snapshot}"
    );
    assert!(
        partial_snapshot["requires_identity_update"]["worker_id"].is_null(),
        "partial terminal must not guess a worker: {partial_snapshot}"
    );

    // Unknown, duplicate and empty supplements are rejected before the daemon
    // creates any identity, so a bad claim can never become a registered peer.
    for (label, provide) in [
        ("unknown field", "{\"token\":\"x\"}"),
        (
            "duplicate field",
            "{\"session_id\":\"a\",\"session_id\":\"b\"}",
        ),
        ("empty value", "{\"session_id\":\"\"}"),
    ] {
        let invalid = fixture.command_without_pane(&["context", "--provide", provide]);
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&invalid.stdout),
            String::from_utf8_lossy(&invalid.stderr)
        );
        assert!(
            !invalid.status.success(),
            "{label} supplement must fail closed: {combined}"
        );
        assert!(
            combined.contains("IDENTITY_FACT_INVALID"),
            "{label} supplement must be rejected explicitly: {combined}"
        );
    }

    let identities = host_state.join("identities");
    let created = identities.exists()
        && std::fs::read_dir(&identities)
            .expect("read isolated identities dir")
            .next()
            .is_some();
    assert!(
        !created,
        "rejected supplements must not create an identity: {identities:?}"
    );
}

/// The identity selector and the client-side bootstrap/mint surface are gone.
/// Each removed command or flag must fail at argument parsing, before any
/// daemon or identity side effect.
#[test]
fn removed_identity_commands_and_flags_are_rejected_by_the_cli() {
    let root = unique_root();
    std::fs::create_dir_all(&root).unwrap();
    let binary = collab_test_binary();
    let cases: &[&[&str]] = &[
        &["context", "--worker", "some-worker"],
        &["init", "--worker-id", "some-worker"],
        &["whoami"],
        &["worker", "recover"],
        &["recv", "--worker", "some-worker"],
        &[
            "send",
            "--from",
            "some-worker",
            "--to",
            "x",
            "--subject",
            "y",
            "body",
        ],
        &["inbox", "--worker", "some-worker"],
        &["ack", "m1", "--worker", "some-worker"],
        &["mailbox", "read", "--worker", "some-worker"],
    ];
    for case in cases {
        let output = Command::new(&binary)
            .args(*case)
            .current_dir(&root)
            .env("COLLAB_STATE_DIR", root.join("h"))
            .env("CODEX_HOME", root.join("home"))
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .output()
            .expect("run collab CLI");
        assert!(
            !output.status.success(),
            "removed identity surface must be rejected: {case:?}"
        );
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            combined.contains("error")
                || combined.contains("unexpected")
                || combined.contains("unrecognized"),
            "removed identity surface must produce a parse rejection: {case:?} => {combined}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// B01: a tmux registration records a Codex session/thread anchor, but a tmux
/// pane address cannot answer whether the agent is resident. The recorded grant
/// holder must still be able to act, and every public projection must agree on
/// that holder while the transport observation stays independent.
#[test]
fn master_authority_tmux_unknown_holder_keeps_control_and_projection() {
    let (fixture, pane) = single_pane_fixture("collab-master-authority-b01");
    let registered = fixture.run_context(&["context"], Some(&pane));
    let worker_id = registered["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names the registered worker")
        .to_owned();
    let project_scope = registered["binding"]["project_scope"]
        .as_str()
        .expect("context receipt names the project scope")
        .to_owned();
    let app_scope_id = registered["binding"]["app_scope_id"]
        .as_str()
        .expect("context receipt names the app scope")
        .to_owned();

    let promoted = fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated tmux master",
        ],
        Some(&pane),
    );
    assert_eq!(promoted["master"], worker_id);
    assert_scope("promote receipt", &promoted, &project_scope, &app_scope_id);

    let status = fixture.run_ok(&["master", "status"], Some(&pane));
    assert_eq!(
        status["master"]["worker_id"], worker_id,
        "status must project the current grant holder: {status}"
    );
    assert_scope("assigned status", &status, &project_scope, &app_scope_id);
    assert!(
        status["recorded_unusable"].is_null(),
        "recorded_unusable must not carry a second authority model: {status}"
    );

    let context = fixture.run_context(&["context"], Some(&pane));
    assert_eq!(
        context["master"]["worker_id"], worker_id,
        "context must project the current grant holder: {context}"
    );
    assert_eq!(context["identity"]["role"], "master");
    assert_scope("assigned context", &context, &project_scope, &app_scope_id);
    assert!(context["recorded_unusable"].is_null(), "{context}");
    assert_ne!(
        context["liveness"]["presence"], "present",
        "a tmux pane address is not an online credential: {context}"
    );

    let published = fixture.run_ok(
        &[
            "board",
            "publish",
            "tmux-master-task",
            "--title",
            "tmux master control",
            "--description",
            "control without a transport liveness probe",
            "--delivery-condition",
            "the board accepts the publish",
            "--test-condition",
            "this test",
            "--priority",
            "p2",
        ],
        Some(&pane),
    );
    assert_eq!(published["task"]["id"], "tmux-master-task");

    let board = fixture.run_ok(&["board", "show"], Some(&pane));
    assert_scope("assigned board", &board, &project_scope, &app_scope_id);
    let master_rows: Vec<&Value> = board["workers"]
        .as_array()
        .expect("board lists workers")
        .iter()
        .filter(|worker| worker["role"] == "master")
        .collect();
    assert_eq!(master_rows.len(), 1, "exactly one master role: {board}");
    assert_eq!(master_rows[0]["id"], worker_id);
    assert_ne!(
        master_rows[0]["status"], "online",
        "a pane address must not make the agent online: {board}"
    );
}

/// B03: an authenticated peer clears the current grant without becoming master
/// and without probing the incumbent. A second clear is idempotent, and no
/// task, peer, message or consumption state is deleted.
#[test]
fn master_authority_clear_removes_grant_and_preserves_business_state() {
    let (fixture, pane) = single_pane_fixture("collab-master-authority-b03");
    let registered = fixture.run_context(&["context"], Some(&pane));
    let worker_id = registered["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names the registered worker")
        .to_owned();
    let project_scope = registered["binding"]["project_scope"]
        .as_str()
        .expect("context receipt names the project scope")
        .to_owned();
    let app_scope_id = registered["binding"]["app_scope_id"]
        .as_str()
        .expect("context receipt names the app scope")
        .to_owned();
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated tmux master",
        ],
        Some(&pane),
    );

    fixture.run_ok(
        &[
            "task",
            "register",
            "authority-clear-task",
            "--next",
            "survive clear",
        ],
        Some(&pane),
    );
    let sent = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &worker_id,
            "--subject",
            "authority clear",
            "preserve me",
        ],
        Some(&pane),
    );
    let message_id = sent["msg_id"]
        .as_str()
        .or_else(|| sent["message_id"].as_str())
        .expect("send names the durable message")
        .to_owned();

    let cleared = fixture.run_ok(
        &[
            "master",
            "clear",
            "--approval",
            "user approved isolated clear",
        ],
        Some(&pane),
    );
    assert_eq!(cleared["was_empty"], false, "{cleared}");
    assert!(cleared["master"].is_null(), "{cleared}");
    assert_eq!(cleared["previous_worker_id"], worker_id);
    assert_scope("clear receipt", &cleared, &project_scope, &app_scope_id);

    let status = fixture.run_ok(&["master", "status"], Some(&pane));
    assert!(status["master"].is_null(), "{status}");
    assert_scope("empty status", &status, &project_scope, &app_scope_id);

    let context = fixture.run_context(&["context"], Some(&pane));
    assert!(context["master"].is_null(), "{context}");
    assert_scope("empty context", &context, &project_scope, &app_scope_id);
    assert_eq!(context["identity"]["role"], "worker");
    assert!(
        context["tasks"]
            .as_array()
            .expect("context lists tasks")
            .iter()
            .any(|task| task["id"] == "authority-clear-task"),
        "clear must not delete tasks: {context}"
    );
    assert!(
        context["inbox"]["messages"]
            .as_array()
            .expect("context lists inbox messages")
            .iter()
            .any(|message| message["id"].as_str() == Some(message_id.as_str())),
        "clear must not delete messages: {context}"
    );

    let again = fixture.run_ok(
        &[
            "master",
            "clear",
            "--approval",
            "user approved isolated clear",
        ],
        Some(&pane),
    );
    assert_eq!(
        again["was_empty"], true,
        "second clear is idempotent: {again}"
    );
    assert!(again["master"].is_null(), "{again}");
    assert_scope(
        "second clear receipt",
        &again,
        &project_scope,
        &app_scope_id,
    );

    // A real isolated daemon restart must not resurrect any typed or legacy
    // holder, and the public task/message facts must still be present.
    let down = fixture.command_without_pane(&["down"]);
    assert!(
        down.status.success(),
        "isolated clear fixture down must succeed"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && fixture.host_state.join("server.sock").exists() {
        thread::sleep(Duration::from_millis(50));
    }
    let up = fixture.command_without_pane(&["up"]);
    assert!(
        up.status.success(),
        "isolated clear fixture up must succeed"
    );

    let restarted_status = fixture.run_ok(&["master", "status"], Some(&pane));
    assert!(
        restarted_status["master"].is_null(),
        "clear must not resurrect authority after restart: {restarted_status}"
    );
    assert_scope(
        "restarted empty status",
        &restarted_status,
        &project_scope,
        &app_scope_id,
    );
    let restarted_context = fixture.run_context(&["context"], Some(&pane));
    assert!(restarted_context["master"].is_null(), "{restarted_context}");
    assert!(
        restarted_context["tasks"]
            .as_array()
            .expect("restarted context lists tasks")
            .iter()
            .any(|task| task["id"] == "authority-clear-task"),
        "restart must preserve tasks: {restarted_context}"
    );
    assert!(
        restarted_context["inbox"]["messages"]
            .as_array()
            .expect("restarted context lists inbox messages")
            .iter()
            .any(|message| message["id"].as_str() == Some(message_id.as_str())),
        "restart must preserve the mailbox: {restarted_context}"
    );

    let rejected = fixture.command(
        &[
            "board",
            "publish",
            "after-clear",
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
        Some(&pane),
    );
    assert!(
        !rejected.status.success(),
        "a cleared holder must not keep board control: stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
}

/// B04: a blank approval is not authorization. Both promote and clear must
/// refuse it and leave the recorded authority unchanged.
#[test]
fn master_authority_rejects_blank_approval_without_state_change() {
    let (fixture, pane) = single_pane_fixture("collab-master-authority-b04");
    fixture.run_context(&["context"], Some(&pane));

    let promote = fixture.command(&["master", "promote", "--approval", "   "], Some(&pane));
    assert!(
        !promote.status.success(),
        "blank promote approval must be rejected: stdout={} stderr={}",
        String::from_utf8_lossy(&promote.stdout),
        String::from_utf8_lossy(&promote.stderr)
    );
    let clear = fixture.command(&["master", "clear", "--approval", "  "], Some(&pane));
    assert!(
        !clear.status.success(),
        "blank clear approval must be rejected: stdout={} stderr={}",
        String::from_utf8_lossy(&clear.stdout),
        String::from_utf8_lossy(&clear.stderr)
    );

    let status = fixture.run_ok(&["master", "status"], Some(&pane));
    assert!(status["master"].is_null(), "{status}");
}

/// B10: the same stable authority state must be projected identically by
/// `master status`, `context`, `board show`, and the real dashboard HTTP GET.
/// The dashboard is a read-only loopback consumer: it must serve the current
/// grant holder while keeping the per-worker agent observation separate, and it
/// must not expose any authority write route.
#[test]
fn master_authority_dashboard_get_agrees_with_status_context_and_board() {
    let (fixture, pane) = single_pane_fixture("collab-master-authority-b10");
    let registered = fixture.run_context(&["context"], Some(&pane));
    let worker_id = registered["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names the registered worker")
        .to_owned();
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated dashboard master",
        ],
        Some(&pane),
    );

    let status = fixture.run_ok(&["master", "status"], Some(&pane));
    let context = fixture.run_context(&["context"], Some(&pane));
    let board = fixture.run_ok(&["board", "show"], Some(&pane));
    assert_eq!(status["master"]["worker_id"], worker_id, "{status}");
    assert_eq!(context["master"]["worker_id"], worker_id, "{context}");
    assert_eq!(board["master"]["worker_id"], worker_id, "{board}");
    // The board's master role is the authority projection, while the member
    // status stays a transport observation: a tmux pane is not an online agent.
    let master_rows: Vec<&Value> = board["workers"]
        .as_array()
        .expect("board lists workers")
        .iter()
        .filter(|worker| worker["role"] == "master")
        .collect();
    assert_eq!(master_rows.len(), 1, "exactly one master role: {board}");
    assert_eq!(master_rows[0]["id"], worker_id);
    assert_ne!(
        master_rows[0]["status"], "online",
        "a pane address must not make the agent online: {board}"
    );

    // The dashboard binds an OS-selected loopback port and prints its URL plus
    // the capability fragment. Parse that line, then drive the real HTTP GET
    // with the exact bearer capability the page would use.
    let mut child = fixture
        .configured_command(&["dashboard", "--port", "0"], Some(&pane), "Codex TUI")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn isolated dashboard");
    let stdout = child.stdout.take().expect("dashboard stdout is piped");
    let mut reader = BufReader::new(stdout);
    let mut announcement = String::new();
    reader
        .read_line(&mut announcement)
        .expect("dashboard announces its URL before serving");
    let announcement: Value =
        serde_json::from_str(announcement.trim()).expect("dashboard prints one JSON announcement");
    assert_eq!(announcement["read_only"], true, "{announcement}");
    let url = announcement["url"]
        .as_str()
        .expect("dashboard announces a URL")
        .to_owned();
    let (authority, capability) = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split_once("/#"))
        .expect("dashboard URL carries the loopback authority and capability fragment");
    let capability = capability.to_owned();
    let authority = authority.to_owned();

    let snapshot = dashboard_get(&authority, &capability, None);
    assert!(
        snapshot.starts_with("HTTP/1.1 200"),
        "authorized dashboard GET must succeed: {snapshot}"
    );
    let body = dashboard_body(&snapshot);
    let served: Value = serde_json::from_str(&body).expect("dashboard serves the board snapshot");
    assert_eq!(
        served["master"]["worker_id"], worker_id,
        "dashboard must project the same current grant holder: {served}"
    );
    let panel_master: Vec<&Value> = served["workers"]
        .as_array()
        .expect("dashboard lists workers")
        .iter()
        .filter(|worker| worker["role"] == "master")
        .collect();
    assert_eq!(panel_master.len(), 1, "{served}");
    assert_eq!(panel_master[0]["id"], worker_id);
    assert_ne!(
        panel_master[0]["status"], "online",
        "the panel must keep the agent observation separate from authority: {served}"
    );

    // No dashboard route writes authority: the observation is GET-only and a
    // write method on the same path is refused rather than applied.
    let refused = dashboard_get(&authority, &capability, Some("POST"));
    assert!(
        !refused.starts_with("HTTP/1.1 200"),
        "the dashboard must not accept an authority write: {refused}"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = reader;
}

/// One dashboard request. `method` defaults to `GET`; the dashboard rejects any
/// method that is not registered.
fn dashboard_get(authority: &str, capability: &str, method: Option<&str>) -> String {
    let mut stream = TcpStream::connect(authority).expect("connect isolated dashboard");
    let method = method.unwrap_or("GET");
    let request = format!(
        "{method} /api/board HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {capability}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn dashboard_body(response: &str) -> String {
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default()
}

/// The accepted projection contract keeps `{project_scope, app_scope_id}` at
/// the top level of every authority-bearing public response, including Empty.
fn assert_scope(label: &str, value: &Value, project_scope: &str, app_scope_id: &str) {
    assert_eq!(
        value["scope"]["project_scope"], project_scope,
        "{label} must project the exact project scope: {value}"
    );
    assert_eq!(
        value["scope"]["app_scope_id"], app_scope_id,
        "{label} must project the exact app scope: {value}"
    );
}

/// Send one raw public request envelope to the isolated daemon socket and
/// return its parsed response. This is the same wire the CLI uses; the test
/// only bypasses the CLI so it can forge a token, scope, or generation.
fn wire_request(socket: &Path, request: &Value) -> Value {
    let mut stream =
        std::os::unix::net::UnixStream::connect(socket).expect("connect isolated daemon socket");
    stream
        .write_all(serde_json::to_string(request).unwrap().as_bytes())
        .unwrap();
    stream.write_all(b"\n").unwrap();
    stream.flush().unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).expect("daemon emits one JSON response line")
}

/// Build the typed runtime context the CLI attaches, from the caller's own
/// context receipt, so forged variants differ only in the field under test.
fn runtime_context_from_receipt(receipt: &Value) -> Value {
    let binding = &receipt["binding"];
    json!({
        "agent_id": binding["agent_id"],
        "runtime_id": binding["runtime_id"],
        "appserver_id": binding["app_scope_id"],
        "endpoint_generation": binding["endpoint_generation"],
        "binding_id": binding["binding_id"],
        "session_id": binding["session_id"],
        "native_thread_id": binding["native_thread_id"],
    })
}

/// Build a registration request for the public `Register` operation. The
/// fixture supplies its own token and a distinct tmux pane; the daemon owns the
/// binding id and generation.
fn tmux_register_request(
    worker_id: &str,
    token: &str,
    root: &Path,
    pane: &Pane,
    tmux_socket: &Path,
) -> Value {
    let observed = tmux(
        tmux_socket,
        &[
            "display-message",
            "-p",
            "-t",
            &pane.pane_id,
            "#{pid}\t#{session_id}\t#{pane_pid}",
        ],
    );
    let observed = String::from_utf8(observed.stdout).expect("tmux facts are UTF-8");
    let fields: Vec<_> = observed.trim().split('\t').collect();
    let server_pid: u32 = fields[0].parse().expect("observed server PID");
    let session_id = fields[1];
    let pane_pid: u32 = fields[2].parse().expect("observed pane PID");
    let canonical_root = std::fs::canonicalize(root)
        .expect("canonicalize fixture root")
        .to_string_lossy()
        .into_owned();
    json!({
        "op": "Register",
        "worker_id": worker_id,
        "token": token,
        "cwd": canonical_root,
        "candidates": {
            "tmux": {
                "endpoint": {
                    "socket_path": tmux_socket,
                    "server_pid": server_pid,
                    "tmux_session_id": session_id,
                    "pane_id": pane.pane_id,
                    "pane_pid": pane_pid,
                    "codex_session_id": pane.session_anchor,
                    "codex_thread_id": pane.thread_anchor,
                },
                "cwd": canonical_root,
            },
        },
        "project_context": {
            "app_scope_id": "appserver-cli",
            "canonical_root": canonical_root,
            "project_scope": canonical_root,
        },
    })
}

/// B06/B07: a same-principal registration on a new pane is the only recovery
/// path that may reissue the grant. A different principal on the same or a new
/// pane inherits nothing, and the old generation request is still fenced.
#[test]
fn master_authority_same_principal_recovery_and_new_principal_do_not_inherit() {
    let (fixture, first_pane) = single_pane_fixture("collab-master-authority-b06");
    let registered = fixture.run_context(&["context"], Some(&first_pane));
    let worker_id = registered["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names the registered worker")
        .to_owned();
    let old_binding = registered["binding"].clone();
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated recovery master",
        ],
        Some(&first_pane),
    );

    // A second authenticated principal on its own pane is an independent peer.
    let created = tmux(
        &fixture.tmux_socket,
        &[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            "collab-master-authority-b06-second",
            "sleep 600",
        ],
    );
    let second_pane_id = String::from_utf8(created.stdout).unwrap().trim().to_owned();
    let second_pane = Pane {
        pane_id: second_pane_id.clone(),
        session_anchor: format!("session-{second_pane_id}"),
        thread_anchor: format!("thread-{second_pane_id}"),
        server_pid: first_pane.server_pid,
    };
    let socket = fixture.host_state.join("server.sock");
    let second_registered = wire_request(
        &socket,
        &tmux_register_request(
            "recovery-second-principal",
            "token-recovery-second-principal",
            &fixture.root,
            &second_pane,
            &fixture.tmux_socket,
        ),
    );
    assert_eq!(
        second_registered["ok"], true,
        "the second principal must register as a peer: {second_registered}"
    );
    let second_status = fixture.run_ok(&["master", "status"], Some(&second_pane));
    assert_eq!(
        second_status["master"]["worker_id"], worker_id,
        "a new principal must not inherit authority by registering a pane: {second_status}"
    );

    // Same-principal recovery moves to a new pane. The grant follows only the
    // verified principal and binding generation.
    let created = tmux(
        &fixture.tmux_socket,
        &[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            "collab-master-authority-b06-recovery",
            "sleep 600",
        ],
    );
    let recovery_pane_id = String::from_utf8(created.stdout).unwrap().trim().to_owned();
    assert!(!recovery_pane_id.is_empty(), "{recovery_pane_id}");
    let recovery_pane = Pane {
        pane_id: recovery_pane_id.clone(),
        session_anchor: format!("session-{recovery_pane_id}"),
        thread_anchor: format!("thread-{recovery_pane_id}"),
        server_pid: first_pane.server_pid,
    };
    let identity_file = fixture
        .host_state
        .join("identities")
        .join(&worker_id)
        .join("identity.json");
    let identity: Value =
        serde_json::from_slice(&std::fs::read(&identity_file).expect("read own identity"))
            .expect("identity is JSON");
    let token = identity["token"]
        .as_str()
        .expect("identity carries the credential")
        .to_owned();
    let mut recovery_request = tmux_register_request(
        &worker_id,
        &token,
        &fixture.root,
        &recovery_pane,
        &fixture.tmux_socket,
    );
    recovery_request["project_context"]["runtime_context"] =
        runtime_context_from_receipt(&registered);
    let recovered = wire_request(&socket, &recovery_request);
    assert_eq!(
        recovered["ok"], true,
        "same-principal recovery must succeed: {recovered}"
    );
    let new_binding = recovered["command"]["binding"].clone();
    let new_runtime = runtime_context_from_receipt(&json!({"binding": new_binding}));
    let new_route = json!({
        "app_scope_id": new_binding["app_scope_id"],
        "canonical_root": new_binding["project_scope"],
        "project_scope": new_binding["project_scope"],
        "runtime_context": new_runtime,
    });
    let recovered_context = wire_request(
        &socket,
        &json!({
            "op": "Context", "worker_id": worker_id, "token": token,
            "project_context": new_route,
        }),
    );
    assert_eq!(recovered_context["ok"], true, "{recovered_context}");
    assert_eq!(
        recovered_context["identity"]["worker_id"], worker_id,
        "recovery must keep the principal: {recovered_context}"
    );
    assert_ne!(
        recovered_context["binding"]["endpoint_generation"], old_binding["endpoint_generation"],
        "recovery must advance the generation: {recovered_context}"
    );
    let recovered_status = wire_request(
        &socket,
        &json!({
            "op": "MasterStatus", "project_context": new_route,
        }),
    );
    assert_eq!(
        recovered_status["master"]["worker_id"], worker_id,
        "same-principal recovery must reissue the grant: {recovered_status}"
    );

    // The old generation remains fenced even with the valid credential.
    let stale = wire_request(
        &socket,
        &json!({
            "op": "MasterClear",
            "worker_id": worker_id,
            "token": token,
            "approval": "stale generation clear",
            "project_context": {
                "app_scope_id": old_binding["app_scope_id"],
                "canonical_root": old_binding["project_scope"],
                "project_scope": old_binding["project_scope"],
                "runtime_context": {
                    "agent_id": old_binding["agent_id"],
                    "runtime_id": old_binding["runtime_id"],
                    "appserver_id": old_binding["app_scope_id"],
                    "endpoint_generation": old_binding["endpoint_generation"],
                    "binding_id": old_binding["binding_id"],
                    "session_id": old_binding["session_id"],
                    "native_thread_id": old_binding["native_thread_id"],
                },
            },
        }),
    );
    assert_eq!(
        stale["ok"], false,
        "old generation must be rejected: {stale}"
    );
    let unchanged = wire_request(
        &socket,
        &json!({
            "op": "MasterStatus", "project_context": new_route,
        }),
    );
    assert_eq!(
        unchanged["master"]["worker_id"], worker_id,
        "a stale-generation refusal must not change authority: {unchanged}"
    );
}

/// B09/B15: real routed mutations preserve other project/app scopes, and
/// concurrent clear/promote observations are complete committed states.
#[test]
fn master_authority_scope_isolation_and_concurrent_mutations() {
    let (fixture, first_pane) = single_pane_fixture("collab-master-scopes");
    let initial = fixture.run_context(&["context"], Some(&first_pane));
    let socket = fixture.host_state.join("server.sock");
    let other_root = fixture.root.join("other-project");
    seed_baseline(&other_root);
    let mut routes = Vec::new();
    for (index, (root, app)) in [
        (&fixture.root, "authority-app-a"),
        (&fixture.root, "authority-app-b"),
        (&other_root, "authority-app-a"),
    ]
    .into_iter()
    .enumerate()
    {
        let label = format!("authority-scope-{index}");
        let created = tmux(
            &fixture.tmux_socket,
            &[
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-s",
                &label,
                "sleep 600",
            ],
        );
        let pane_id = String::from_utf8(created.stdout).unwrap().trim().to_owned();
        let pane = Pane {
            server_pid: first_pane.server_pid,
            session_anchor: format!("session-{pane_id}"),
            thread_anchor: format!("thread-{pane_id}"),
            pane_id,
        };
        let worker = format!("scope-worker-{index}");
        let token = format!("scope-token-{index}");
        let mut register =
            tmux_register_request(&worker, &token, root, &pane, &fixture.tmux_socket);
        register["project_context"]["app_scope_id"] = json!(app);
        let registered = wire_request(&socket, &register);
        assert_eq!(registered["ok"], true, "scope registration: {registered}");
        let binding = registered["command"]["binding"].clone();
        let runtime = runtime_context_from_receipt(&json!({"binding": binding}));
        let route = json!({
            "app_scope_id": binding["app_scope_id"],
            "canonical_root": binding["project_scope"],
            "project_scope": binding["project_scope"], "runtime_context": runtime,
        });
        let promote = json!({"op":"MasterPromote", "worker_id":worker,
            "token":token, "approval":"approved scope fixture", "project_context":route});
        let result = wire_request(&socket, &promote);
        assert_eq!(result["ok"], true, "scope promote: {result}");
        routes.push((worker, token, route, promote));
    }
    for (worker, token, route, _) in &routes {
        for request in [
            json!({"op":"MasterStatus", "project_context":route}),
            json!({"op":"Context", "worker_id":worker,"token":token,"project_context":route}),
            json!({"op":"BoardShow", "project_context":route}),
        ] {
            let result = wire_request(&socket, &request);
            assert_eq!(result["ok"], true, "{result}");
            assert_eq!(result["scope"]["project_scope"], route["project_scope"]);
            assert_eq!(result["scope"]["app_scope_id"], route["app_scope_id"]);
            assert_eq!(result["master"]["worker_id"], *worker, "{result}");
        }
    }
    let (worker, token, route, promote) = &routes[0];
    let clear = json!({"op":"MasterClear", "worker_id":worker, "token":token,
        "approval":"approved concurrent fixture", "project_context":route});
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let children: Vec<_> = [clear.clone(), promote.clone()]
        .into_iter()
        .map(|request| {
            let socket = socket.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                wire_request(&socket, &request)
            })
        })
        .collect();
    barrier.wait();
    for _ in 0..12 {
        let observed = wire_request(
            &socket,
            &json!({"op":"MasterStatus", "project_context":route}),
        );
        assert_eq!(observed["ok"], true, "{observed}");
        assert_eq!(observed["scope"]["project_scope"], route["project_scope"]);
        assert_eq!(observed["scope"]["app_scope_id"], route["app_scope_id"]);
        assert!(
            observed["master"].is_null() || observed["master"]["worker_id"] == *worker,
            "partial or foreign authority: {observed}"
        );
    }
    for child in children {
        let result = child.join().unwrap();
        assert_eq!(result["ok"], true, "{result}");
    }
    assert_eq!(wire_request(&socket, &clear)["ok"], true);
    for (index, (worker, _, route, _)) in routes.iter().enumerate() {
        let status = wire_request(
            &socket,
            &json!({"op":"MasterStatus", "project_context":route}),
        );
        assert_eq!(status["ok"], true, "{status}");
        if index == 0 {
            assert!(status["master"].is_null(), "{status}");
        } else {
            assert_eq!(status["master"]["worker_id"], *worker, "{status}");
        }
    }
    let original = wire_request(
        &socket,
        &json!({"op":"MasterStatus",
        "project_context": {
            "app_scope_id": initial["scope"]["app_scope_id"],
            "canonical_root": initial["scope"]["project_scope"],
            "project_scope": initial["scope"]["project_scope"],
        }}),
    );
    assert_eq!(original["ok"], true, "{original}");
    assert!(original["master"].is_null());
    assert_eq!(original["scope"], initial["scope"]);
}

/// B05: an authenticated mutation over the public socket must reject a wrong
/// token, a mismatched project route, a mismatched app scope, and a stale
/// endpoint generation. Every rejection leaves the recorded authority and the
/// business state exactly as they were, and the raw token is never echoed.
#[test]
fn master_authority_public_wire_rejects_bad_token_scope_and_generation_without_state_change() {
    let (fixture, pane) = single_pane_fixture("collab-master-authority-b05");
    let receipt = fixture.run_context(&["context"], Some(&pane));
    let worker_id = receipt["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names the registered worker")
        .to_owned();
    // The public display never carries the credential, so the fixture reads its
    // own persisted identity to build the real (and forged) wire requests.
    let identity_file = fixture
        .host_state
        .join("identities")
        .join(&worker_id)
        .join("identity.json");
    let identity: Value =
        serde_json::from_slice(&std::fs::read(&identity_file).unwrap_or_else(|error| {
            panic!("read own identity {}: {error}", identity_file.display())
        }))
        .expect("persisted identity is JSON");
    let token = identity["token"]
        .as_str()
        .expect("persisted identity carries the credential")
        .to_owned();
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated wire master",
        ],
        Some(&pane),
    );
    fixture.run_ok(
        &[
            "task",
            "register",
            "wire-preserved-task",
            "--next",
            "stay put",
        ],
        Some(&pane),
    );

    let socket = fixture.host_state.join("server.sock");
    let runtime_context = runtime_context_from_receipt(&receipt);
    let context_for = |canonical_root: &str, app_scope: &str, runtime: Value| {
        json!({
            "app_scope_id": app_scope,
            "canonical_root": canonical_root,
            "project_scope": canonical_root,
            "runtime_context": runtime,
        })
    };
    let root = std::fs::canonicalize(&fixture.root)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let app_scope = receipt["binding"]["app_scope_id"]
        .as_str()
        .expect("receipt names the app scope")
        .to_owned();
    let endpoint_generation = runtime_context["endpoint_generation"]
        .as_u64()
        .expect("receipt carries the live generation");

    // The opaque token is the one credential the fixture must forge; every
    // other field is the caller's real context.
    let mut wrong_token_runtime = runtime_context.clone();
    wrong_token_runtime["endpoint_generation"] = json!(endpoint_generation);
    let cases: Vec<(&str, &[&str], Value)> = vec![
        (
            "wrong token",
            &["token mismatch"],
            json!({
                "op": "MasterClear",
                "worker_id": worker_id,
                "token": "not-the-real-token",
                "approval": "forged clear",
                "project_context": context_for(&root, &app_scope, wrong_token_runtime),
            }),
        ),
        (
            "mismatched project route",
            &["RUNTIME_BINDING_REJECTED", "PROJECT_SCOPE_UNKNOWN"],
            json!({
                "op": "MasterClear",
                "worker_id": worker_id,
                "token": token,
                "approval": "forged clear",
                "project_context": context_for(
                    &format!("{root}/other-project"),
                    &app_scope,
                    runtime_context.clone(),
                ),
            }),
        ),
        (
            "mismatched app scope",
            &[
                "RUNTIME_BINDING_REJECTED",
                "PROJECT_SCOPE_UNKNOWN",
                "PROJECT_CONTEXT_INVALID",
            ],
            json!({
                "op": "MasterClear",
                "worker_id": worker_id,
                "token": token,
                "approval": "forged clear",
                "project_context": context_for(
                    &root,
                    "appserver-other-scope",
                    runtime_context.clone(),
                ),
            }),
        ),
        (
            "stale endpoint generation",
            &["SESSION_THREAD_BINDING_MISMATCH"],
            json!({
                "op": "MasterClear",
                "worker_id": worker_id,
                "token": token,
                "approval": "forged clear",
                "project_context": {
                    "app_scope_id": app_scope,
                    "canonical_root": root,
                    "project_scope": root,
                    "runtime_context": {
                        "agent_id": runtime_context["agent_id"],
                        "runtime_id": runtime_context["runtime_id"],
                        "appserver_id": runtime_context["appserver_id"],
                        "endpoint_generation": endpoint_generation + 99,
                        "binding_id": runtime_context["binding_id"],
                        "session_id": runtime_context["session_id"],
                        "native_thread_id": runtime_context["native_thread_id"],
                    },
                },
            }),
        ),
    ];

    for (label, expected_errors, request) in cases {
        let response = wire_request(&socket, &request);
        assert_eq!(
            response["ok"], false,
            "{label} must be rejected: {response}"
        );
        assert!(
            response["error"]
                .as_str()
                .is_some_and(|error| expected_errors.iter().any(|e| error.contains(e))),
            "{label} must fail at one of {expected_errors:?}: {response}"
        );
        let rendered = response.to_string();
        assert!(
            !rendered.contains("not-the-real-token"),
            "{label} must not echo the credential: {response}"
        );
    }

    // The holder is unchanged and the business state is intact.
    let status = fixture.run_ok(&["master", "status"], Some(&pane));
    assert_eq!(
        status["master"]["worker_id"], worker_id,
        "a rejected mutation must not change the holder: {status}"
    );
    let task = fixture.run_ok(&["task", "status", "wire-preserved-task"], Some(&pane));
    assert_eq!(task["owner"], worker_id, "{task}");
    assert_eq!(task["next_step"], "stay put", "{task}");
}
