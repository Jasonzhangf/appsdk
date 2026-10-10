use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Fixture roots live under `/tmp` so the AF_UNIX socket stays inside
/// `SUN_LEN`. The macOS default temp dir is long enough to make every daemon
/// start fail with `path must be shorter than SUN_LEN` before any product
/// behavior is reachable.
const FIXTURE_ROOT_PREFIX: &str = "/tmp/v23f";

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
pub static TMUX_FIXTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub const PROJECT_JOURNAL: &str = ".agent-collab/server/journal.jsonl";
pub const ROUTES_JOURNAL: &str = "routes.jsonl";
pub const HOST_JOURNAL: &str = "journal.jsonl";
pub const HOST_LOG: &str = "log.txt";
pub const HOST_EVENTS: &str = "events.jsonl";

pub fn collab_test_binary() -> PathBuf {
    std::env::var_os("COLLAB_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_collab")))
}

pub fn collab_test_mcp_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("COLLAB_TEST_MCP_BINARY") {
        return PathBuf::from(path);
    }
    let cli = std::env::var_os("COLLAB_TEST_BINARY").map(PathBuf::from);
    if let Some(cli) = cli {
        return cli
            .parent()
            .expect("COLLAB_TEST_BINARY has a parent")
            .join("collab-mcp");
    }
    PathBuf::from(env!("CARGO_BIN_EXE_collab-mcp"))
}

pub fn unique_root(label: &str) -> PathBuf {
    PathBuf::from(format!(
        "{FIXTURE_ROOT_PREFIX}-{label}-{}-{}",
        std::process::id(),
        FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

pub fn seed_baseline(root: &Path) {
    std::fs::create_dir_all(root.join(".agent-collab")).expect("seed project baseline");
}

#[derive(Clone)]
pub struct Pane {
    pub server_pid: u32,
    pub pane_id: String,
    pub session_anchor: String,
    pub thread_anchor: String,
}

pub struct Fixture {
    pub binary: PathBuf,
    pub root: PathBuf,
    pub host_state: PathBuf,
    pub tmux_socket: PathBuf,
    active_daemon: bool,
    hidden_project_journal: Option<PathBuf>,
    hook_socket: Option<PathBuf>,
}

impl Fixture {
    pub fn isolated(label: &str) -> (Self, Pane) {
        let root = unique_root(label);
        let host_state = root.join("h");
        let tmux_socket = root.join("t.sock");
        std::fs::create_dir_all(&host_state).expect("create isolated host state");
        seed_baseline(&root);
        let mut fixture = Self {
            binary: collab_test_binary(),
            root,
            host_state,
            tmux_socket: tmux_socket.clone(),
            active_daemon: false,
            hidden_project_journal: None,
            hook_socket: None,
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
        fixture.active_daemon = true;
        let pane = Pane {
            server_pid,
            pane_id: pane_id.clone(),
            session_anchor: format!("session-{pane_id}"),
            thread_anchor: format!("thread-{pane_id}"),
        };
        (fixture, pane)
    }

    pub fn without_tmux(label: &str) -> Self {
        let root = unique_root(label);
        let host_state = root.join("h");
        std::fs::create_dir_all(&host_state).expect("create isolated host state");
        seed_baseline(&root);
        Self {
            binary: collab_test_binary(),
            root: root.clone(),
            host_state,
            tmux_socket: root.join("unused.sock"),
            // A plain `context` invocation may auto-start the daemon even
            // without a tmux identity anchor, so Drop owns that lifecycle.
            active_daemon: true,
            hidden_project_journal: None,
            hook_socket: None,
        }
    }

    pub fn configured_command(&self, args: &[&str], pane: Option<&Pane>) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .args(args)
            .current_dir(&self.root)
            .env("COLLAB_STATE_DIR", &self.host_state)
            .env("CODEX_HOME", self.root.join("home"))
            .env_remove("COLLAB_SOCKET_PATH")
            .env_remove("COLLAB_HOST_SOCKET")
            .env_remove("COLLAB_LOCK_PATH")
            .env_remove("COLLAB_HOST_LOCK")
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env_remove("COLLAB_WORKER")
            .env_remove("DSH_SESSION_ID")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI");
        if let Some(hook_socket) = &self.hook_socket {
            command.env("COLLAB_CONTEXT_CANCEL_HOOK_SOCKET", hook_socket);
        }
        match pane {
            Some(pane) => {
                command
                    .env(
                        "TMUX",
                        format!("{},{},0", self.tmux_socket.display(), pane.server_pid),
                    )
                    .env("TMUX_PANE", &pane.pane_id)
                    .env("CODEX_SESSION_ID", &pane.session_anchor)
                    .env("CODEX_THREAD_ID", &pane.thread_anchor);
            }
            None => {
                command
                    .env_remove("TMUX")
                    .env_remove("TMUX_PANE")
                    .env_remove("CODEX_SESSION_ID")
                    .env_remove("CODEX_THREAD_ID");
            }
        }
        command
    }

    pub fn command(&self, args: &[&str], pane: Option<&Pane>) -> Output {
        self.configured_command(args, pane)
            .output()
            .expect("run public collab CLI")
    }

    pub fn spawn_command(&self, args: &[&str], pane: Option<&Pane>) -> Child {
        self.configured_command(args, pane)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn public collab CLI")
    }

    pub fn command_status(&self, args: &[&str], pane: Option<&Pane>) -> i32 {
        self.command(args, pane).status.code().unwrap_or(-1)
    }

    pub fn run_ok(&self, args: &[&str], pane: Option<&Pane>) -> Value {
        let output = self.command(args, pane);
        assert!(
            output.status.success(),
            "collab {args:?} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("collab CLI emits JSON")
    }

    pub fn context(&self, operation_id: Option<&str>, pane: Option<&Pane>) -> Value {
        let mut args = vec!["context"];
        if let Some(operation_id) = operation_id {
            args.extend(["--op", operation_id]);
        }
        self.run_ok(&args, pane)
    }

    pub fn context_query(&self, operation_id: &str, pane: Option<&Pane>) -> Value {
        self.run_ok(&["context", "--op", operation_id, "--query"], pane)
    }

    pub fn context_provide(
        &self,
        operation_id: &str,
        provide: &Value,
        pane: Option<&Pane>,
    ) -> Value {
        let provide = serde_json::to_string(provide).expect("serialize provide facts");
        self.run_ok(
            &["context", "--op", operation_id, "--provide", &provide],
            pane,
        )
    }

    pub fn project_journal(&self) -> PathBuf {
        self.root.join(PROJECT_JOURNAL)
    }

    pub fn host_journal(&self) -> PathBuf {
        self.host_state.join(HOST_JOURNAL)
    }

    pub fn host_routes(&self) -> PathBuf {
        self.host_state.join(ROUTES_JOURNAL)
    }

    pub fn host_log(&self) -> PathBuf {
        self.host_state.join(HOST_LOG)
    }

    pub fn host_events(&self) -> PathBuf {
        self.host_state.join(HOST_EVENTS)
    }

    pub fn proof_path(&self, operation_id: &str) -> PathBuf {
        self.host_state
            .join("context-operations")
            .join(format!("{operation_id}.json"))
    }

    pub fn proof(&self, operation_id: &str) -> Value {
        serde_json::from_slice(
            &std::fs::read(self.proof_path(operation_id)).expect("read public context proof"),
        )
        .expect("context proof is JSON")
    }

    pub fn set_proof_capability(&self, operation_id: &str, capability: &str) {
        let path = self.proof_path(operation_id);
        let mut proof = self.proof(operation_id);
        proof["query_capability"] = Value::String(capability.to_owned());
        let bytes = serde_json::to_vec_pretty(&proof).expect("serialize public proof");
        std::fs::write(&path, bytes).expect("update public proof capability");
    }

    /// Move the real daemon-written project journal aside so the inner owner is
    /// genuinely unavailable. The bytes and path are restored on drop.
    pub fn hide_project_journal(&mut self) {
        let journal = self.project_journal();
        assert!(
            journal.exists(),
            "B05 requires a project journal produced by public operations"
        );
        let hidden = self.host_state.join("journal.hidden.jsonl");
        std::fs::rename(&journal, &hidden).expect("preserve real project journal");
        std::fs::create_dir(&journal).expect("make project journal unavailable");
        self.hidden_project_journal = Some(hidden);
    }

    /// Stop a fixture daemon whose project replay failed and whose degraded
    /// server intentionally accepts only operation queries. The PID comes
    /// from this fixture's private state directory and is rechecked exactly.
    pub fn stop_degraded_daemon(&mut self) {
        let pid_path = self.host_state.join("server.pid");
        let pid = std::fs::read_to_string(&pid_path)
            .expect("read fixture daemon pid")
            .trim()
            .parse::<i32>()
            .expect("fixture daemon pid is numeric");
        assert!(pid > 1, "fixture daemon pid is valid");
        let result = unsafe { libc::kill(pid, libc::SIGTERM) };
        assert_eq!(result, 0, "terminate only fixture daemon pid {pid}");
        assert!(
            wait_until(Duration::from_secs(15), || {
                !self.host_state.join("server.sock").exists()
            }),
            "degraded fixture daemon still owns its socket"
        );
        self.active_daemon = false;
    }

    pub fn restore_project_journal(&mut self) {
        if let Some(hidden) = self.hidden_project_journal.take() {
            let journal = self.project_journal();
            if hidden.exists() {
                if journal.is_dir() {
                    std::fs::remove_dir_all(&journal).expect("remove unavailable journal marker");
                } else if journal.exists() {
                    std::fs::remove_file(&journal).expect("remove replacement project journal");
                }
                std::fs::rename(&hidden, &journal).expect("restore real project journal");
            }
        }
    }

    pub fn stop_daemon(&mut self) {
        if self.active_daemon {
            let output = self.command(&["down"], None);
            let host_log_tail = std::fs::read_to_string(self.host_log())
                .unwrap_or_default()
                .lines()
                .rev()
                .take(12)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                output.status.success(),
                "fixture daemon down failed: stdout={} stderr={} host_log_tail={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
                host_log_tail
            );
            assert!(
                wait_until(Duration::from_secs(15), || {
                    !self.host_state.join("server.sock").exists()
                }),
                "fixture daemon still owns its socket after down"
            );
            self.active_daemon = false;
        }
    }

    pub fn daemon_pid(&self) -> i32 {
        std::fs::read_to_string(self.host_state.join("server.pid"))
            .expect("read fixture daemon pid")
            .trim()
            .parse::<i32>()
            .expect("fixture daemon pid is numeric")
    }

    /// Pause only this fixture's daemon so a cancellation signal can be
    /// observed before or at a durable boundary. The guard always resumes the
    /// same exact PID on drop, including during a panic.
    pub fn pause_daemon(&self) -> DaemonPause {
        let pid = self.daemon_pid();
        assert!(pid > 1, "fixture daemon pid is valid");
        let result = unsafe { libc::kill(pid, libc::SIGSTOP) };
        assert_eq!(result, 0, "pause only fixture daemon pid {pid}");
        DaemonPause { pid }
    }

    pub fn restart_daemon(&mut self) {
        self.stop_daemon();
        std::fs::remove_file(self.host_state.join("DOWN")).ok();
        client_ensure_server(
            &self.binary,
            &self.root,
            &self.host_state,
            self.hook_socket.as_deref(),
        );
        self.active_daemon = true;
    }

    /// Restart only this fixture daemon with one compile-time gated fault
    /// injection variable, used to prove public journal failure projections.
    pub fn restart_daemon_with_env(&mut self, key: &str, value: &str) {
        self.stop_daemon();
        std::fs::remove_file(self.host_state.join("DOWN")).ok();
        let mut command = self.configured_command(&["up"], None);
        command.env(key, value);
        let output = command.output().expect("restart daemon with test fault");
        assert!(
            output.status.success(),
            "fault-injected daemon up failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        self.active_daemon = true;
    }

    /// Stop this fixture's daemon by exact PID so a request handler can be
    /// interrupted at a durable boundary without using the normal down path.
    pub fn kill_daemon(&self) {
        let pid = self.daemon_pid();
        assert!(pid > 1, "fixture daemon pid is valid");
        let result = unsafe { libc::kill(pid, libc::SIGTERM) };
        assert_eq!(result, 0, "terminate only fixture daemon pid {pid}");
        assert!(
            wait_until(Duration::from_secs(15), || {
                !self.host_state.join("server.sock").exists()
            }),
            "fixture daemon still owns its socket after kill"
        );
    }

    pub fn restart_after_external_stop(&mut self) {
        self.active_daemon = false;
        std::fs::remove_file(self.host_state.join("DOWN")).ok();
        client_ensure_server(
            &self.binary,
            &self.root,
            &self.host_state,
            self.hook_socket.as_deref(),
        );
        self.active_daemon = true;
    }

    /// Point this fixture's CLI, MCP and daemon processes at the private
    /// acknowledged-barrier socket. Must be called before the first public
    /// operation; the isolated daemon reads the same variable at startup.
    pub fn enable_cancellation_hooks(&mut self, socket: &Path) {
        self.hook_socket = Some(socket.to_path_buf());
    }

    pub fn restart_daemon_with_file_limit(&mut self, bytes: u64) {
        self.stop_daemon();
        std::fs::remove_file(self.host_state.join("DOWN")).ok();
        let mut command = self.configured_command(&["up"], None);
        unsafe {
            command.pre_exec(move || {
                if libc::signal(libc::SIGXFSZ, libc::SIG_IGN) == libc::SIG_ERR {
                    return Err(std::io::Error::last_os_error());
                }
                let limit = libc::rlimit {
                    rlim_cur: bytes,
                    rlim_max: bytes,
                };
                if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let output = command.output().expect("restart daemon with file-size cap");
        assert!(
            output.status.success(),
            "capped daemon up failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        self.active_daemon = true;
    }

    pub fn snapshot_surfaces(&self) -> Vec<(String, Vec<u8>)> {
        let mut surfaces = Vec::new();
        for (label, path) in [
            ("host_journal", self.host_journal()),
            ("host_routes", self.host_routes()),
            ("host_log", self.host_log()),
            ("host_events", self.host_events()),
            ("project_journal", self.project_journal()),
        ] {
            if let Ok(bytes) = std::fs::read(path) {
                surfaces.push((label.to_owned(), bytes));
            }
        }
        surfaces
    }
}

pub struct DaemonPause {
    pid: i32,
}

impl Drop for DaemonPause {
    fn drop(&mut self) {
        let _ = unsafe { libc::kill(self.pid, libc::SIGCONT) };
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.restore_project_journal();
        self.stop_daemon();
        let _ = Command::new("tmux")
            .args([
                "-S",
                self.tmux_socket.to_str().unwrap_or_default(),
                "kill-server",
            ])
            .output();
        if std::thread::panicking() {
            eprintln!(
                "preserving failed public fixture: root={} host_state={}",
                self.root.display(),
                self.host_state.display()
            );
        } else {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

/// A narrow native AppServer endpoint for testing the `context --provide`
/// public path. Registration and durable identity state remain owned by the
/// real Collab daemon; this endpoint only answers initialize and thread/read.
pub struct TestAppServer {
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    state: Arc<Mutex<TestAppServerState>>,
    gate: Arc<ArchiveGate>,
}

#[derive(Clone)]
struct TestAppServerState {
    created_thread: Option<String>,
    created_cwd: String,
    created_turns: Vec<Value>,
    start_requests: usize,
    cwd: String,
    turns: Vec<Value>,
    effect: String,
    settings_requests: usize,
    archived: bool,
    archive_requests: usize,
    readback_requests: usize,
    /// Marker file name extracted from the challenge prompt, retained so a
    /// delayed completion can read the operation-owned marker.
    challenge_marker: Option<String>,
    /// Durable identity of the pending challenge turn for the seeded thread.
    /// Number of post-dispatch history reads observed by the fixture.
    challenge_reads: usize,
    /// Number of Create readiness challenge turns dispatched.
    readiness_requests: usize,
    /// Number of Update challenge turns dispatched.
    challenge_requests: usize,
}

#[derive(Default)]
struct ArchiveGate {
    inner: Mutex<ArchiveGateState>,
    changed: std::sync::Condvar,
}

#[derive(Default)]
struct ArchiveGateState {
    archive_requests: usize,
    readback_requests: usize,
    release_archive: bool,
    release_readback: bool,
}

impl TestAppServer {
    pub fn start(root: &Path, session_id: &str, thread_id: &str) -> Self {
        Self::start_with_effect(root, session_id, thread_id, "ack_only")
    }

    pub fn start_with_effect(root: &Path, session_id: &str, thread_id: &str, effect: &str) -> Self {
        Self::start_with_effect_at(root, "test-appserver.sock", session_id, thread_id, effect)
    }

    pub fn start_with_effect_at(
        root: &Path,
        socket_name: &str,
        session_id: &str,
        thread_id: &str,
        effect: &str,
    ) -> Self {
        let socket = root.join(socket_name);
        let listener = UnixListener::bind(&socket).expect("bind test AppServer socket");
        listener
            .set_nonblocking(true)
            .expect("set test AppServer listener nonblocking");
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_root = root.to_path_buf();
        let worker_session_id = session_id.to_owned();
        let worker_thread_id = thread_id.to_owned();
        let state = Arc::new(Mutex::new(TestAppServerState {
            created_thread: None,
            created_cwd: String::new(),
            created_turns: Vec::new(),
            start_requests: 0,
            cwd: root.to_string_lossy().into_owned(),
            turns: Vec::new(),
            effect: effect.to_owned(),
            settings_requests: 0,
            archived: false,
            archive_requests: 0,
            readback_requests: 0,
            challenge_marker: None,
            challenge_reads: 0,
            readiness_requests: 0,
            challenge_requests: 0,
        }));
        let worker_state = state.clone();
        let gate = Arc::new(ArchiveGate::default());
        let worker_gate = gate.clone();
        let worker = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => serve_appserver_connection(
                        stream,
                        &worker_root,
                        &worker_session_id,
                        &worker_thread_id,
                        &worker_state,
                        &worker_gate,
                    ),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("test AppServer accept failed: {error}"),
                }
            }
        });
        Self {
            socket,
            stop,
            worker: Some(worker),
            state,
            gate,
        }
    }

    pub fn endpoint(&self) -> String {
        format!("unix://{}", self.socket.display())
    }

    pub fn settings_request_count(&self) -> usize {
        self.state.lock().unwrap().settings_requests
    }

    pub fn readiness_request_count(&self) -> usize {
        self.state.lock().unwrap().readiness_requests
    }

    pub fn challenge_request_count(&self) -> usize {
        self.state.lock().unwrap().challenge_requests
    }

    pub fn set_effect(&self, effect: &str) {
        self.state.lock().unwrap().effect = effect.to_owned();
    }

    pub fn start_request_count(&self) -> usize {
        self.state.lock().unwrap().start_requests
    }

    pub fn archive_request_count(&self) -> usize {
        self.gate.inner.lock().unwrap().archive_requests
    }

    pub fn readback_request_count(&self) -> usize {
        self.gate.inner.lock().unwrap().readback_requests
    }

    pub fn release_archive(&self) {
        let mut inner = self.gate.inner.lock().unwrap();
        inner.release_archive = true;
        self.gate.changed.notify_all();
    }

    pub fn release_readback(&self) {
        let mut inner = self.gate.inner.lock().unwrap();
        inner.release_readback = true;
        self.gate.changed.notify_all();
    }
}

impl Drop for TestAppServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = UnixStream::connect(&self.socket);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("join test AppServer worker");
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

const CHALLENGE_MARKER_TOKEN: &str = "[[marker:";
/// Post-dispatch reads before a delayed challenge completes. The lifecycle
/// owner's initial window spends fewer reads, so its first response is pending
/// and the later same-operation readback is the one that observes completion.
const DELAYED_CHALLENGE_READS: usize = 4;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn challenge_marker_from_prompt(prompt: &str) -> Option<String> {
    let start = prompt.find(CHALLENGE_MARKER_TOKEN)? + CHALLENGE_MARKER_TOKEN.len();
    let rest = &prompt[start..];
    let end = rest.find("]]")?;
    Some(rest[..end].to_owned())
}

fn read_challenge_marker(cwd: &str, marker_file: &str) -> Option<String> {
    std::fs::read_to_string(Path::new(cwd).join(marker_file)).ok()
}

fn challenge_envelope(cwd: &str, marker: &str) -> String {
    format!("post-update:{cwd}:{marker}")
}

/// Complete a delayed challenge only after enough post-dispatch reads. The
/// marker is read from the intended execution directory, so the fixture never
/// invents the secret.
fn complete_delayed_challenge(state: &mut TestAppServerState, created: bool) {
    state.challenge_reads = state.challenge_reads.saturating_add(1);
    if state.challenge_reads < DELAYED_CHALLENGE_READS {
        return;
    }
    let Some(marker_file) = state.challenge_marker.clone() else {
        return;
    };
    if created {
        let cwd = state.created_cwd.clone();
        let Some(marker) = read_challenge_marker(&cwd, &marker_file) else {
            return;
        };
        if let Some(turn) = state
            .created_turns
            .iter_mut()
            .find(|turn| turn["status"] == "inProgress")
        {
            turn["status"] = json!("completed");
            turn["completedAt"] = json!(now_secs());
            turn["items"] = json!([{
                "id": "create-challenge", "type": "agentMessage",
                "text": challenge_envelope(&cwd, &marker),
            }]);
        }
    } else {
        let cwd = state.cwd.clone();
        let Some(marker) = read_challenge_marker(&cwd, &marker_file) else {
            return;
        };
        if let Some(turn) = state
            .turns
            .iter_mut()
            .find(|turn| turn["status"] == "inProgress")
        {
            turn["status"] = json!("completed");
            turn["completedAt"] = json!(now_secs());
            turn["items"] = json!([{
                "id": "update-challenge", "type": "agentMessage",
                "text": challenge_envelope(&cwd, &marker),
            }]);
        }
    }
}

fn serve_appserver_connection(
    mut stream: UnixStream,
    root: &Path,
    session_id: &str,
    thread_id: &str,
    state: &Arc<Mutex<TestAppServerState>>,
    gate: &Arc<ArchiveGate>,
) {
    // macOS carries O_NONBLOCK from a nonblocking listener to accepted
    // sockets. Keep only accept() nonblocking; frame reads need to wait for
    // the peer's next request after the HTTP upgrade.
    stream
        .set_nonblocking(false)
        .expect("set accepted AppServer stream blocking");
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        if stream.read_exact(&mut byte).is_err() {
            return;
        }
        request.push(byte[0]);
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    if stream
        .write_all(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
        )
        .is_err()
    {
        return;
    }
    loop {
        let payload = match read_appserver_frame(&mut stream) {
            Ok(Some(payload)) => payload,
            Ok(None) => return,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                return;
            }
            Err(error) => {
                eprintln!("test AppServer websocket read failed: {error}");
                return;
            }
        };
        let Ok(request) = serde_json::from_slice::<Value>(&payload) else {
            eprintln!("test AppServer received invalid JSON frame");
            return;
        };
        eprintln!(
            "test AppServer request method={:?} id={:?}",
            request.get("method"),
            request.get("id")
        );
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request.get("method").and_then(Value::as_str);
        let response = match method {
            Some("thread/start") => {
                let mut state = state.lock().unwrap();
                state.start_requests += 1;
                let created = format!("created-thread-{}", state.start_requests);
                state.created_thread = Some(created.clone());
                state.created_cwd = request
                    .pointer("/params/cwd")
                    .and_then(Value::as_str)
                    .unwrap()
                    .to_owned();
                if state.effect == "create_response_loss" {
                    return;
                }
                // A notification with another id must never be accepted as the start response.
                let unrelated = json!({"id":999999,"result":{"thread":{"id":"unrelated-thread"}}});
                write_appserver_frame(&mut stream, &serde_json::to_vec(&unrelated).unwrap())
                    .unwrap();
                json!({"id":id,"result":{"thread":{"id":created}}})
            }
            Some("thread/resume") => {
                let requested = request
                    .pointer("/params/threadId")
                    .and_then(Value::as_str)
                    .unwrap_or(thread_id);
                json!({"id":id,"result":{"thread":{"id":requested,"status":{"type":"idle"}}}})
            }
            Some("turn/start") => {
                let mut state = state.lock().unwrap();
                let requested = request
                    .pointer("/params/threadId")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                // `immediate_notify` carries the body inside the delegated
                // tool output for a fresh turn and inside `input` when
                // steering; accept either carrier.
                let input_text = request
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let tool_output = request
                    .pointer("/params/toolOutput/output")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let prompt = format!("{input_text}\n{tool_output}");
                let marker_file = challenge_marker_from_prompt(&prompt);
                let client_message_id = request
                    .pointer("/params/clientUserMessageId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let is_create_readiness = client_message_id.starts_with("create-ready-");
                let is_update_challenge = client_message_id.starts_with("update-challenge-");
                let now = now_secs();
                let effect = state.effect.clone();
                if is_create_readiness {
                    state.readiness_requests += 1;
                    let turn_id = "create-readiness-turn";
                    let cwd = state.created_cwd.clone();
                    let marker = marker_file
                        .as_deref()
                        .and_then(|file| read_challenge_marker(&cwd, file));
                    match effect.as_str() {
                        "create_ready" | "ready_and_update_ok" | "reject_generic_turn_start" => {
                            if let Some(marker) = marker.as_deref() {
                                state.created_turns.push(json!({
                                    "id": turn_id, "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "create-challenge", "type": "agentMessage",
                                        "text": challenge_envelope(&cwd, marker),
                                    }],
                                }));
                            }
                        }
                        "create_ready_delayed" => {
                            state.challenge_marker = marker_file.clone();
                            state.challenge_reads = 0;
                            state.created_turns.push(json!({
                                "id": turn_id, "status": "inProgress", "items": [],
                            }));
                        }
                        // Producer-side correlation failures: the turn is
                        // dispatched, but the history cannot prove the exact
                        // marker, turn, or thread, so it must never finalize.
                        "create_ready_wrong_marker" => {
                            if marker.is_some() {
                                state.created_turns.push(json!({
                                    "id": turn_id, "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "create-wrong-marker", "type": "agentMessage",
                                        "text": format!("post-update:{cwd}:wrong-marker"),
                                    }],
                                }));
                            }
                        }
                        "create_ready_wrong_turn" => {
                            if let Some(marker) = marker.as_deref() {
                                state.created_turns.push(json!({
                                    "id": "wrong-create-turn", "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "create-wrong-turn", "type": "agentMessage",
                                        "text": challenge_envelope(&cwd, marker),
                                    }],
                                }));
                            }
                        }
                        "create_ready_wrong_thread" => {
                            if let Some(marker) = marker.as_deref() {
                                state.created_turns.push(json!({
                                    "id": turn_id, "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "create-wrong-thread", "type": "agentMessage",
                                        "text": challenge_envelope(&cwd, marker),
                                    }],
                                }));
                            }
                        }
                        _ => {}
                    }
                    json!({"id": id, "result": {"turn": {"id": turn_id, "status": "inProgress"}}})
                } else if is_update_challenge {
                    state.challenge_requests += 1;
                    let turn_id = "update-challenge-turn";
                    let cwd = state.cwd.clone();
                    let marker = marker_file
                        .as_deref()
                        .and_then(|file| read_challenge_marker(&cwd, file));
                    let mut new_turns: Vec<Value> = Vec::new();
                    match effect.as_str() {
                        "completed_turn" | "update_challenge_ok" | "ready_and_update_ok" => {
                            if let Some(marker) = marker.as_deref() {
                                new_turns.push(json!({
                                    "id": turn_id, "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "update-challenge", "type": "agentMessage",
                                        "text": challenge_envelope(&cwd, marker),
                                    }],
                                }));
                            }
                        }
                        "update_challenge_delayed" => {
                            state.challenge_marker = marker_file.clone();
                            state.challenge_reads = 0;
                            new_turns.push(json!({
                                "id": turn_id, "status": "inProgress", "items": [],
                            }));
                        }
                        "update_challenge_quoted" => {
                            if let Some(marker) = marker.as_deref() {
                                new_turns.push(json!({
                                    "id": turn_id, "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "update-quoted", "type": "agentMessage",
                                        "text": format!("\"{}\"", challenge_envelope(&cwd, marker)),
                                    }],
                                }));
                            }
                        }
                        "update_challenge_failed" => {
                            new_turns.push(json!({
                                "id": turn_id, "status": "completed", "completedAt": now,
                                "items": [{
                                    "id": "update-failed", "type": "agentMessage",
                                    "text": "execution failed: cat: no such file or directory",
                                }],
                            }));
                        }
                        "update_challenge_prefix" => {
                            if let Some(marker) = marker.as_deref() {
                                new_turns.push(json!({
                                    "id": turn_id, "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "update-prefix", "type": "agentMessage",
                                        "text": format!("post-update:{cwd}-longer:{marker}"),
                                    }],
                                }));
                            }
                        }
                        "update_challenge_stale" => {
                            if let Some(marker) = marker.as_deref() {
                                // A stale turn completed in the same second but
                                // is not the dispatched challenge turn.
                                new_turns.push(json!({
                                    "id": "stale-preupdate-turn", "status": "completed",
                                    "completedAt": now,
                                    "items": [{
                                        "id": "update-stale", "type": "agentMessage",
                                        "text": challenge_envelope(&cwd, marker),
                                    }],
                                }));
                            }
                            new_turns.push(json!({
                                "id": turn_id, "status": "inProgress", "items": [],
                            }));
                        }
                        "update_challenge_wrong_thread" => {
                            if let Some(marker) = marker.as_deref() {
                                new_turns.push(json!({
                                    "id": turn_id, "status": "completed", "completedAt": now,
                                    "items": [{
                                        "id": "update-wrong-thread", "type": "agentMessage",
                                        "text": challenge_envelope(&cwd, marker),
                                    }],
                                }));
                            }
                        }
                        _ => {}
                    }
                    if requested.as_deref() == state.created_thread.as_deref() {
                        state.created_turns.extend(new_turns);
                    } else {
                        state.turns.extend(new_turns);
                    }
                    json!({"id": id, "result": {"turn": {"id": turn_id, "status": "inProgress"}}})
                } else {
                    if state.effect == "reject_generic_turn_start" {
                        json!({
                            "id": id,
                            "error": {
                                "code": -32000,
                                "message": "APPSERVER_NOTIFICATION_REJECTED: fixture refused generic notification",
                                "data": {
                                    "notification_error": "fixture refused generic notification",
                                    "repair_required": true
                                }
                            }
                        })
                    } else {
                        json!({"id": id, "result": {"turn": {"id": "unrelated-turn", "status": "completed"}}})
                    }
                }
            }
            Some("thread/read") => {
                let include_turns = request
                    .pointer("/params/includeTurns")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let effect = state.lock().unwrap().effect.clone();
                let archived = state.lock().unwrap().archived;
                if effect == "archive_hold_readback" && archived {
                    let mut inner = gate.inner.lock().unwrap();
                    inner.readback_requests += 1;
                    while !inner.release_readback {
                        inner = gate.changed.wait(inner).unwrap();
                    }
                }
                let mut state = state.lock().unwrap();
                let requested = request
                    .pointer("/params/threadId")
                    .and_then(Value::as_str)
                    .unwrap_or(thread_id);
                let created = state.created_thread.as_deref() == Some(requested);
                if created && state.effect == "create_register_failure" {
                    // The first exact read establishes the session; formal Register then fails its probe.
                    if !include_turns {
                        return;
                    }
                }
                if include_turns {
                    complete_delayed_challenge(&mut state, created);
                }
                let mut thread = json!({
                    "id": if created { requested } else { thread_id },
                    "sessionId": if created {
                        if state.start_requests == 1 {
                            "created-session".to_owned()
                        } else {
                            format!("created-session-{}", state.start_requests)
                        }
                    } else {
                        session_id.to_owned()
                    },
                    "cwd": if created { &state.created_cwd } else { &state.cwd },
                    "status": {"type": if state.archived { "notLoaded" } else { "idle" }}
                });
                // Simulate a completed challenge that landed on a different
                // thread: the exact-thread correlation must reject it. The
                // post-start read must still see the exact created thread so
                // registration can complete; only post-challenge reads are
                // redirected.
                if created
                    && include_turns
                    && state.effect == "create_ready_wrong_thread"
                    && state.readiness_requests > 0
                {
                    thread["id"] = json!("wrong-thread");
                }
                if !created
                    && include_turns
                    && state.effect == "update_challenge_wrong_thread"
                {
                    thread["id"] = json!("wrong-thread");
                }
                if include_turns {
                    thread["turns"] = if created {
                        json!(state.created_turns)
                    } else {
                        json!(state.turns)
                    };
                }
                json!({"id": id, "result": {"thread": thread}})
            }
            Some("thread/archive") => {
                let effect = state.lock().unwrap().effect.clone();
                {
                    let mut inner = gate.inner.lock().unwrap();
                    inner.archive_requests += 1;
                    if effect == "archive_hold_readback" {
                        while !inner.release_archive {
                            inner = gate.changed.wait(inner).unwrap();
                        }
                    }
                }
                match effect.as_str() {
                    "archive_unknown" => return,
                    "archive_missing" => {
                        json!({"id": id, "error": {"code": -32000, "message": format!("thread {thread_id} not found")}})
                    }
                    "archive_refused" => {
                        json!({"id": id, "error": {"code": -32000, "message": "archive refused"}})
                    }
                    _ => {
                        state.lock().unwrap().archived = true;
                        json!({"id": id, "result": {"archived": true}})
                    }
                }
            }
            Some("thread/settings/update") => {
                let effect = state.lock().unwrap().effect.clone();
                if effect == "unknown" {
                    return;
                }
                if effect == "refused" {
                    json!({"id": id, "error": {"code": -32000, "message": "settings refused"}})
                } else {
                    let cwd = request
                        .pointer("/params/cwd")
                        .and_then(Value::as_str)
                        .unwrap_or(root.to_string_lossy().as_ref())
                        .to_owned();
                    let mut state = state.lock().unwrap();
                    state.settings_requests += 1;
                    state.cwd = cwd.clone();
                    json!({"id": id, "result": {"thread": {"id": thread_id, "cwd": cwd}}})
                }
            }
            _ => json!({"id": id, "result": {}}),
        };
        let Ok(encoded) = serde_json::to_vec(&response) else {
            return;
        };
        if let Err(error) = write_appserver_frame(&mut stream, &encoded) {
            eprintln!("test AppServer websocket write failed: {error}");
            return;
        }
    }
}

fn read_appserver_frame(stream: &mut UnixStream) -> std::io::Result<Option<Vec<u8>>> {
    let mut header = [0_u8; 2];
    match stream.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let mut length = (header[1] & 0x7f) as u64;
    if length == 126 {
        let mut bytes = [0_u8; 2];
        stream.read_exact(&mut bytes)?;
        length = u16::from_be_bytes(bytes) as u64;
    } else if length == 127 {
        let mut bytes = [0_u8; 8];
        stream.read_exact(&mut bytes)?;
        length = u64::from_be_bytes(bytes);
    }
    let length = usize::try_from(length).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "frame length overflow")
    })?;
    let masked = header[1] & 0x80 != 0;
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
    Ok(Some(payload))
}

fn write_appserver_frame(stream: &mut UnixStream, payload: &[u8]) -> std::io::Result<()> {
    let mut frame = Vec::with_capacity(payload.len() + 10);
    frame.push(0x81);
    match payload.len() {
        length if length < 126 => frame.push(length as u8),
        length if length <= u16::MAX as usize => {
            frame.push(126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        }
        length => {
            frame.push(127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(payload);
    stream.write_all(&frame)
}

pub struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<Result<Value, String>>,
    reader: Option<JoinHandle<()>>,
}

impl Mcp {
    pub fn start(fixture: &Fixture, pane: Option<&Pane>) -> Self {
        let mut command = Command::new(collab_test_mcp_binary());
        command
            .current_dir(&fixture.root)
            .env("COLLAB_STATE_DIR", &fixture.host_state)
            .env("CODEX_HOME", fixture.root.join("home"))
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env_remove("COLLAB_WORKER")
            .env_remove("DSH_SESSION_ID")
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
            .env("COLLAB_BIN", &fixture.binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(pane) = pane {
            command
                .env(
                    "TMUX",
                    format!("{},{},0", fixture.tmux_socket.display(), pane.server_pid),
                )
                .env("TMUX_PANE", &pane.pane_id)
                .env("CODEX_SESSION_ID", &pane.session_anchor)
                .env("CODEX_THREAD_ID", &pane.thread_anchor);
        } else {
            command
                .env_remove("TMUX")
                .env_remove("TMUX_PANE")
                .env_remove("CODEX_SESSION_ID")
                .env_remove("CODEX_THREAD_ID");
        }
        if let Some(hook_socket) = &fixture.hook_socket {
            command.env("COLLAB_CONTEXT_CANCEL_HOOK_SOCKET", hook_socket);
        }
        let mut child = command.spawn().expect("launch real collab-mcp");
        let stdin = child.stdin.take().expect("MCP stdin");
        let stdout = child.stdout.take().expect("MCP stdout");
        let (sender, replies) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let reply = line.map_err(|error| error.to_string()).and_then(|line| {
                    serde_json::from_str(&line).map_err(|error| error.to_string())
                });
                if sender.send(reply).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            replies,
            reader: Some(reader),
        }
    }

    pub fn send_context(&mut self, id: u64, arguments: Value) {
        let stdin = self.stdin.as_mut().expect("MCP stdin is open");
        serde_json::to_writer(
            &mut *stdin,
            &json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":"tools/call",
                "params":{"name":"collab_context","arguments":arguments}
            }),
        )
        .expect("serialize MCP request");
        writeln!(stdin).expect("write MCP request");
        stdin.flush().expect("flush MCP request");
    }

    pub fn cancel(&mut self, id: u64) {
        let stdin = self.stdin.as_mut().expect("MCP stdin is open");
        serde_json::to_writer(
            &mut *stdin,
            &json!({
                "jsonrpc":"2.0",
                "method":"notifications/cancelled",
                "params":{
                    "requestId":id,
                    "reason":"fixture cancellation boundary"
                }
            }),
        )
        .expect("serialize MCP cancellation");
        writeln!(stdin).expect("write MCP cancellation");
        stdin.flush().expect("flush MCP cancellation");
    }

    pub fn recv_reply(&mut self, timeout: Duration) -> Result<Value, String> {
        let reply = self
            .replies
            .recv_timeout(timeout)
            .map_err(|error| format!("MCP reply timeout: {error}"))?
            .map_err(|error| format!("MCP reply read failed: {error}"))?;
        Ok(reply)
    }

    pub fn call_context(&mut self, id: u64, arguments: Value) -> Value {
        self.send_context(id, arguments);
        let reply = self
            .recv_reply(Duration::from_secs(30))
            .expect("MCP returns a bounded response");
        assert_eq!(reply["id"], id, "{reply}");
        reply["result"].clone()
    }

    pub fn finish(mut self) {
        self.stdin.take();
        let status = self.child.wait().expect("wait for MCP process");
        assert!(status.success(), "MCP process must exit cleanly: {status}");
        if let Some(reader) = self.reader.take() {
            reader.join().expect("join MCP stdout reader");
        }
    }
}

fn client_ensure_server(binary: &Path, root: &Path, host_state: &Path, hook_socket: Option<&Path>) {
    let mut command = Command::new(binary);
    command
        .args(["up"])
        .current_dir(root)
        .env("COLLAB_STATE_DIR", host_state)
        .env("CODEX_HOME", root.join("home"))
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID");
    if let Some(hook_socket) = hook_socket {
        command.env("COLLAB_CONTEXT_CANCEL_HOOK_SOCKET", hook_socket);
    }
    let output = command.output().expect("start isolated daemon");
    assert!(
        output.status.success(),
        "isolated daemon up failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn tmux(socket: &Path, args: &[&str]) -> Output {
    let output = Command::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(args)
        .output()
        .expect("run isolated tmux");
    assert!(
        output.status.success(),
        "tmux {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

pub fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    predicate()
}

/// One acknowledged production barrier. `release` unblocks the paused real
/// process so the test can assert the durable state before continuing.
pub struct Barrier {
    pub operation_id: String,
    pub boundary: String,
    pub pid: u32,
    pub nested_command_id: Option<String>,
    pub nested_operation_id: Option<String>,
    release: std::os::unix::net::UnixStream,
}

impl Barrier {
    pub fn release(self) {
        self.respond("release");
    }

    pub fn respond(mut self, reply: &str) {
        let _ = writeln!(self.release, "{reply}");
        let _ = self.release.flush();
    }
}

/// Private acknowledged-barrier control socket for the `context-cancel-test-hooks`
/// feature. It accepts only this fixture's exact barrier connections and never
/// appears in a default or release build.

include!("context_operation_fixture/cancellation.rs");
