use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

struct Fixture {
    binary: PathBuf,
    root: PathBuf,
    host_state: PathBuf,
    tmux_socket: PathBuf,
    initialized: bool,
}

impl Fixture {
    fn command(&self, args: &[&str], pane: Option<&Pane>) -> Output {
        let mut command = Command::new(&self.binary);
        command
            .args(args)
            .current_dir(&self.root)
            .env("COLLAB_STATE_DIR", &self.host_state);
        if let Some(pane) = pane {
            command
                .env(
                    "TMUX",
                    format!("{},{},0", self.tmux_socket.display(), pane.server_pid),
                )
                .env("TMUX_PANE", &pane.pane_id)
                .env("CODEX_SESSION_ID", &pane.session_anchor)
                .env("CODEX_THREAD_ID", &pane.thread_anchor)
                .env("COLLAB_WORKER", &pane.worker_id);
        }
        command.output().expect("run collab CLI")
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

struct Pane {
    server_pid: u32,
    pane_id: String,
    session_anchor: String,
    thread_anchor: String,
    worker_id: String,
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

#[test]
fn collab_recv_cli_subprocess_commits_queryable_receipt_over_isolated_daemon() {
    let root = unique_root();
    let host_state = root.join("h");
    let tmux_socket = root.join("t.sock");
    std::fs::create_dir_all(&root).expect("create isolated project root");
    std::fs::create_dir_all(&host_state).expect("create isolated host state root");
    let mut fixture = Fixture {
        binary: PathBuf::from(env!("CARGO_BIN_EXE_collab")),
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
    let pane = |pane_id: &str, worker_id: &str| Pane {
        server_pid,
        pane_id: pane_id.to_owned(),
        session_anchor: format!("session-{worker_id}"),
        thread_anchor: format!("thread-{worker_id}"),
        worker_id: worker_id.to_owned(),
    };
    let sender = pane(pane_ids[0], "tmux-e2e-a");
    let receiver = pane(pane_ids[1], "tmux-e2e-b");

    fixture.initialized = true;
    fixture.run_ok(&["init"], Some(&sender));
    fixture.run_ok(&["init"], Some(&receiver));

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
            &receiver.worker_id,
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
            "--worker",
            &receiver.worker_id,
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
}

#[test]
fn context_recovers_archived_master_in_the_same_live_pane() {
    let root = unique_root();
    let host_state = root.join("h");
    let tmux_socket = root.join("t.sock");
    std::fs::create_dir_all(&host_state).unwrap();
    let mut fixture = Fixture {
        binary: PathBuf::from(env!("CARGO_BIN_EXE_collab")),
        root: root.clone(),
        host_state,
        tmux_socket: tmux_socket.clone(),
        initialized: false,
    };
    tmux(&tmux_socket, &["new-session", "-d", "-s", "collab-pane-recovery", "sleep 600"]);
    let server_pid = String::from_utf8(
        tmux(&tmux_socket, &["display-message", "-p", "#{pid}"]).stdout,
    )
    .unwrap()
    .trim()
    .parse::<u32>()
    .unwrap();
    let pane_id = String::from_utf8(
        tmux(&tmux_socket, &["display-message", "-p", "#{pane_id}"]).stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    let worker_id = format!("codex-{pane_id}");
    let old = Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: "session-before-restart".into(),
        thread_anchor: "thread-before-restart".into(),
        worker_id: worker_id.clone(),
    };
    fixture.initialized = true;
    fixture.run_ok(&["init"], Some(&old));
    fixture.run_ok(
        &["master", "promote", "--approval", "user approved isolated master"],
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
    assert_eq!(recovered["bootstrap"]["identity"], "recovered");
    let again = fixture.run_ok(&["context"], Some(&new));
    assert_eq!(again["bootstrap"]["identity"], "reused");
    assert_eq!(again["identity"]["endpoint_generation"], recovered["identity"]["endpoint_generation"]);

    let sent = fixture.run_ok(
        &["sendmessage", "--to", &worker_id, "--subject", "recovery", "consume me"],
        Some(&new),
    );
    let message_id = sent["msg_id"]
        .as_str()
        .or_else(|| sent["message_id"].as_str())
        .unwrap();
    let received = fixture.run_ok(
        &["recv", "--worker", &worker_id, "--timeout", "0", "--receive-id", "recovery-recv-1"],
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
    let mut fixture = Fixture {
        binary: PathBuf::from(env!("CARGO_BIN_EXE_collab")),
        root: root.clone(),
        host_state: host_state.clone(),
        tmux_socket: tmux_socket.clone(),
        initialized: false,
    };
    tmux(
        &tmux_socket,
        &["new-session", "-d", "-s", "collab-implicit-context", "sleep 600"],
    );
    let server_pid = String::from_utf8(
        tmux(&tmux_socket, &["display-message", "-p", "#{pid}"]).stdout,
    )
    .unwrap()
    .trim()
    .parse::<u32>()
    .unwrap();
    let pane_id = String::from_utf8(
        tmux(&tmux_socket, &["display-message", "-p", "#{pane_id}"]).stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    let worker_id = format!("codex-{pane_id}");
    let pane = Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: "session-implicit".into(),
        thread_anchor: "thread-implicit".into(),
        worker_id: worker_id.clone(),
    };
    fixture.initialized = true;
    fixture.run_ok(&["init"], Some(&pane));

    // Durable identity exists and is registered; now make its token unusable so
    // the daemon rejects the first authenticated call.
    let identity_path = host_state.join("identities").join(&worker_id).join("identity.json");
    let mut identity: Value =
        serde_json::from_slice(&std::fs::read(&identity_path).unwrap()).unwrap();
    identity["token"] = Value::String("not-the-recorded-token".into());
    std::fs::write(&identity_path, serde_json::to_vec_pretty(&identity).unwrap()).unwrap();

    // Implicit: no `--worker`, and COLLAB_WORKER is deliberately removed while
    // the tmux anchor stays, so the identity is resolved from the anchor alone.
    let output = Command::new(&fixture.binary)
        .arg("context")
        .current_dir(&fixture.root)
        .env("COLLAB_STATE_DIR", &fixture.host_state)
        .env("TMUX", format!("{},{},0", tmux_socket.display(), pane.server_pid))
        .env("TMUX_PANE", &pane.pane_id)
        .env("CODEX_SESSION_ID", &pane.session_anchor)
        .env("CODEX_THREAD_ID", &pane.thread_anchor)
        .env_remove("COLLAB_WORKER")
        .output()
        .expect("run implicit collab context");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "a classified identity failure must still exit 0: stdout={stdout} stderr={stderr}"
    );
    let snapshot: Value = serde_json::from_str(&stdout).expect("context emits JSON");
    let update = &snapshot["requires_identity_update"];
    assert_eq!(
        snapshot["registered"], false,
        "the terminal is not a registered snapshot: {snapshot}"
    );
    assert_eq!(
        update["reason"], "TOKEN_MISMATCH",
        "the daemon rejection must be classified: {snapshot}"
    );
    assert_eq!(
        update["worker_id"], worker_id,
        "the implicit path must name the identity it resolved and had rejected: {snapshot}"
    );
    let action = update["action"].as_str().unwrap();
    assert!(
        action.contains(&worker_id),
        "the escalation must carry the concrete worker: {snapshot}"
    );
    assert!(
        !action.contains("<worker_id>"),
        "no placeholder on the resolved path: {snapshot}"
    );
}
