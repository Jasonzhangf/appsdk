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
    owned_sessions: Vec<String>,
    daemon: Option<std::process::Child>,
}

impl Fixture {
    fn start_daemon(&mut self) {
        std::fs::create_dir_all(&self.host_state).unwrap();
        std::fs::write(
            self.host_state.join("tmux-endpoints.json"),
            serde_json::to_vec(&json!({
                "endpoints": [self.tmux_socket.to_string_lossy()]
            }))
            .unwrap(),
        )
        .unwrap();
        let daemon_log = std::fs::File::create(self.root.join("test-daemon.log")).unwrap();
        self.daemon = Some(
            self.configured_command(&["serve"], None, "Codex TUI")
                .stdin(std::process::Stdio::null())
                .stdout(daemon_log.try_clone().unwrap())
                .stderr(daemon_log)
                .spawn()
                .expect("spawn owned serve child"),
        );
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

    fn stop_daemon(&mut self) {
        if let Some(mut daemon) = self.daemon.take() {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        let _ = std::fs::remove_file(self.host_state.join("server.sock"));
    }

    fn restart_daemon(&mut self) {
        self.stop_daemon();
        self.start_daemon();
    }

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
        let mut command = if let Some(pane) = pane {
            self.ensure_pane_runner(pane);
            let mut command = Command::new("python3");
            command
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/pane_cli.py"))
                .arg("client")
                .arg("--socket")
                .arg(&self.tmux_socket)
                .arg("--pane-id")
                .arg(&pane.pane_id)
                .arg("--root")
                .arg(&self.root)
                .arg("--binary")
                .arg(&self.binary)
                .arg("--");
            command
        } else {
            Command::new(&self.binary)
        };
        command
            .args(args)
            .current_dir(&self.root)
            .env("HOME", self.root.join("home"))
            .env(
                "PANE_CONTROL_PATH",
                std::env::var_os("PATH").unwrap_or_default(),
            )
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("CODEX_HOME", self.root.join("home"))
            .env_remove("COLLAB_SOCKET_PATH")
            .env_remove("COLLAB_HOST_SOCKET")
            .env_remove("COLLAB_LOCK_PATH")
            .env_remove("COLLAB_HOST_LOCK")
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

    fn ensure_pane_runner(&self, pane: &Pane) {
        let ready = self.root.join(format!(
            "pane-runner-{}.ready",
            pane.pane_id.trim_start_matches('%')
        ));
        let observed = tmux(
            &self.tmux_socket,
            &["display-message", "-p", "-t", &pane.pane_id, "#{pane_pid}"],
        );
        let pid = String::from_utf8(observed.stdout).unwrap();
        if std::fs::read_to_string(&ready).is_ok_and(|stored| stored.trim() == pid.trim()) {
            return;
        }
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
        let helper = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/pane_cli.py");
        let start = format!(
            "exec python3 {} runner --socket {} --pane-id {} --root {}",
            quote(&helper.to_string_lossy()),
            quote(&self.tmux_socket.to_string_lossy()),
            quote(&pane.pane_id),
            quote(&self.root.to_string_lossy())
        );
        std::fs::remove_file(&ready).ok();
        tmux(
            &self.tmux_socket,
            &["respawn-pane", "-k", "-t", &pane.pane_id, &start],
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(ready.exists(), "test-owned pane runner did not start");
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
        let value = self.run_ok(args, pane);
        value
            .get("result")
            .and_then(|result| result.get("snapshot"))
            .cloned()
            .unwrap_or(value)
    }

    fn registered_context(&self, pane: &Pane, native_ids: bool) -> Value {
        self.registered_context_with_originator(pane, native_ids, "Codex TUI")
    }

    fn registered_context_with_originator(
        &self,
        pane: &Pane,
        native_ids: bool,
        originator: &str,
    ) -> Value {
        let label = format!(
            "participant-{}-{}",
            pane.server_pid,
            pane.pane_id.trim_start_matches('%')
        );
        let mut command =
            self.configured_command(&["context", "--op", &label], Some(pane), originator);
        if !native_ids {
            command
                .env_remove("CODEX_SESSION_ID")
                .env_remove("CODEX_THREAD_ID");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "participant setup failed: {} {}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["ok"], true, "{value}");
        assert_eq!(value["result"]["outcome"], "completed", "{value}");
        value["result"]["snapshot"].clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop_daemon();
        for session in self.owned_sessions.drain(..) {
            let _ = Command::new("tmux")
                .args([
                    "-S",
                    self.tmux_socket.to_str().unwrap(),
                    "kill-session",
                    "-t",
                    &session,
                ])
                .output();
        }
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
    std::fs::write(
        host_state.join("tmux-endpoints.json"),
        serde_json::to_vec(&json!({
            "endpoints": [tmux_socket.to_string_lossy()]
        }))
        .unwrap(),
    )
    .unwrap();
    seed_baseline(&root);
    let mut fixture = Fixture {
        binary: collab_test_binary(),
        root: root.clone(),
        host_state,
        tmux_socket: tmux_socket.clone(),
        owned_sessions: Vec::new(),
        daemon: None,
    };
    fixture.start_daemon();
    tmux(
        &tmux_socket,
        &["new-session", "-d", "-s", label, "sleep 600"],
    );
    fixture.owned_sessions.push(label.to_owned());
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
