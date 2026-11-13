use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const REX_PARENT_TIMEOUT: Duration = Duration::from_secs(30);
static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn collab_test_binary() -> PathBuf {
    std::env::var_os("COLLAB_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_collab")))
}

fn official_rex_cli() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        return PathBuf::from(
            "/Applications/Rex Beta.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex",
        );
    }
    #[cfg(target_os = "linux")]
    {
        return PathBuf::from("/usr/local/bin/rex");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        panic!("the Rex live test has no official CLI path for this platform")
    }
}

fn python_executable() -> PathBuf {
    let output = Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .expect("python3 is required by the Rex live receiver");
    assert!(
        output.status.success(),
        "python3 lookup failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let path = String::from_utf8(output.stdout)
        .expect("python executable path is utf8")
        .trim()
        .to_owned();
    assert!(
        Path::new(&path).is_absolute(),
        "python3 returned a non-absolute executable: {path}"
    );
    PathBuf::from(path)
}

fn unique_root(label: &str) -> PathBuf {
    PathBuf::from(format!(
        "/tmp/collab-rex-live-{}-{}-{}",
        label,
        std::process::id(),
        FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn wait_for_ping(child: &mut Child, socket: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("check isolated Collab daemon") {
            panic!("isolated Collab daemon exited before Ping: {status}");
        }
        if let Ok(mut stream) = UnixStream::connect(socket) {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
            if stream.write_all(b"{\"op\":\"Ping\"}\n").is_ok() {
                let mut text = String::new();
                let _ = stream.read_to_string(&mut text);
                if serde_json::from_str::<Value>(&text).is_ok_and(|value| value["ok"] == true) {
                    return;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "isolated Collab daemon did not answer Ping at {}",
            socket.display()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub struct RexCliOutput {
    pub success: bool,
    pub timed_out: bool,
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl RexCliOutput {
    fn detail(&self, operation: &str) -> String {
        format!(
            "{operation} timed_out={} status={} stdout={} stderr={}",
            self.timed_out,
            self.status,
            self.stdout.trim(),
            self.stderr.trim()
        )
    }
}

fn run_child(mut command: Command, stdin_body: Option<String>, timeout: Duration) -> RexCliOutput {
    command
        .stdin(if stdin_body.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn test-owned child process");
    let stdout_reader = child.stdout.take().expect("capture child stdout");
    let stderr_reader = child.stderr.take().expect("capture child stderr");
    let stdout_thread = std::thread::spawn(move || {
        let mut reader = stdout_reader;
        let mut bytes = Vec::new();
        let _ = reader.read_to_end(&mut bytes);
        bytes
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut reader = stderr_reader;
        let mut bytes = Vec::new();
        let _ = reader.read_to_end(&mut bytes);
        bytes
    });
    if let Some(body) = stdin_body {
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(body.as_bytes())
                .expect("write test-owned child stdin");
        }
    }

    let deadline = Instant::now() + timeout;
    let (success, status, timed_out) = loop {
        if let Some(status) = child.try_wait().expect("poll test-owned child") {
            break (status.success(), status.code().unwrap_or(-1), false);
        }
        if Instant::now() >= deadline {
            child.kill().expect("terminate test-owned child");
            child.wait().expect("reap test-owned child");
            break (false, -1, true);
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let stdout = String::from_utf8_lossy(
        &stdout_thread
            .join()
            .expect("join test-owned child stdout reader"),
    )
    .into_owned();
    let stderr = String::from_utf8_lossy(
        &stderr_thread
            .join()
            .expect("join test-owned child stderr reader"),
    )
    .into_owned();
    RexCliOutput {
        success,
        timed_out,
        status,
        stdout,
        stderr,
    }
}

#[derive(Clone)]
pub struct OwnedRexSession {
    pub label: String,
    pub run_id: String,
    pub agent_session_id: String,
    pub session_id: String,
    pub block_id: String,
    pub cwd: PathBuf,
    pub ack_file: PathBuf,
    pub creation_result: Value,
    destroyed: bool,
}

pub struct RexLiveFixture {
    pub binary: PathBuf,
    pub root: PathBuf,
    pub host_state: PathBuf,
    pub rex_server: String,
    pub evidence_path: PathBuf,
    rex_cli: PathBuf,
    python: PathBuf,
    receiver_script: PathBuf,
    daemon: Option<Child>,
    sessions: Vec<OwnedRexSession>,
}

impl RexLiveFixture {
    pub fn new(label: &str) -> Self {
        let rex_server = std::env::var("COLLAB_REX_TEST_SERVER")
            .expect("COLLAB_REX_TEST_SERVER must name an explicitly selected Rex server");
        assert!(
            !rex_server.trim().is_empty(),
            "COLLAB_REX_TEST_SERVER must not be empty"
        );
        let root = unique_root(label);
        let host_state = root.join("h");
        fs::create_dir_all(root.join(".agent-collab")).expect("create project baseline");
        fs::create_dir_all(&host_state).expect("create isolated host state");
        fs::create_dir_all(root.join("home")).expect("create isolated home");
        let receiver_script = root.join("rex_live_receiver.py");
        fs::write(&receiver_script, include_str!("rex_live_receiver.py"))
            .expect("write test-only Rex receiver");
        let rex_cli = official_rex_cli();
        assert!(
            rex_cli.is_file(),
            "official Rex CLI is missing at {}",
            rex_cli.display()
        );
        let evidence_path = root.join("live-evidence.jsonl");
        fs::write(&evidence_path, "").expect("create live evidence file");
        Self {
            binary: collab_test_binary(),
            root,
            host_state,
            rex_server,
            evidence_path,
            rex_cli,
            python: python_executable(),
            receiver_script,
            daemon: None,
            sessions: Vec::new(),
        }
    }

    pub fn unique_token(&self) -> String {
        format!(
            "{}-{}",
            std::process::id(),
            FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )
    }

    pub fn evidence(&self, kind: &str, value: Value) {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&self.evidence_path)
            .expect("open live evidence file");
        let line = json!({"kind": kind, "data": value});
        writeln!(file, "{line}").expect("append live evidence");
    }

    pub fn command(&self, args: &[&str], agent_session_id: Option<&str>) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("LC_ALL", "en_US.UTF-8")
            .env("HOME", self.root.join("home"))
            .env("CODEX_HOME", self.root.join("home"))
            .env("COLLAB_STATE_DIR", &self.host_state);
        if let Some(agent_session_id) = agent_session_id {
            command
                .env("CODEX_SESSION_ID", agent_session_id)
                .env("CODEX_THREAD_ID", format!("thread-{agent_session_id}"));
        }
        command
    }

    pub fn run_ok(&self, args: &[&str], agent_session_id: Option<&str>) -> Value {
        let output = self
            .command(args, agent_session_id)
            .output()
            .expect("run public Collab CLI");
        assert!(
            output.status.success(),
            "collab {args:?} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("collab CLI emits JSON")
    }

    pub fn start_daemon(&mut self) {
        assert!(
            self.daemon.is_none(),
            "isolated Collab daemon already owned"
        );
        let mut command = self.command(&["serve"], None);
        let log = fs::File::create(self.root.join("test-daemon.log"))
            .expect("create isolated Collab daemon log");
        self.daemon = Some(
            command
                .stdin(Stdio::null())
                .stdout(log.try_clone().expect("clone daemon stdout log"))
                .stderr(log)
                .spawn()
                .expect("spawn isolated Collab daemon"),
        );
        wait_for_ping(
            self.daemon.as_mut().expect("daemon was just spawned"),
            &self.host_state.join("server.sock"),
            Duration::from_secs(10),
        );
    }

    pub fn stop_daemon(&mut self) {
        let socket = self.host_state.join("server.sock");
        if let Some(mut child) = self.daemon.take() {
            if child.try_wait().expect("poll isolated daemon").is_none() {
                let result = unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
                assert_eq!(result, 0, "terminate only the test-owned Collab daemon");
            }
            child.wait().expect("reap isolated Collab daemon");
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while socket.exists() {
            assert!(
                Instant::now() < deadline,
                "test-owned Collab daemon still owns {}",
                socket.display()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn restart_daemon(&mut self) {
        self.stop_daemon();
        self.start_daemon();
    }

    fn rex_command(&self, args: &[String]) -> Command {
        let mut command = Command::new(&self.rex_cli);
        command
            .args([
                "--server",
                self.rex_server.as_str(),
                "--autostart=false",
                "--timeout",
                "20s",
            ])
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("LC_ALL", "en_US.UTF-8")
            .env("HOME", self.root.join("home"));
        command
    }

    fn run_rex(&self, args: &[String], stdin_body: Option<&str>) -> RexCliOutput {
        run_child(
            self.rex_command(args),
            stdin_body.map(str::to_owned),
            REX_PARENT_TIMEOUT,
        )
    }

    fn rex_json(&self, args: &[String], stdin_body: Option<&str>) -> Value {
        self.try_rex_json(args, stdin_body)
            .unwrap_or_else(|error| panic!("official Rex CLI failed: {error}"))
    }

    fn try_rex_json(&self, args: &[String], stdin_body: Option<&str>) -> Result<Value, String> {
        let output = self.run_rex(args, stdin_body);
        if !output.success || output.timed_out {
            return Err(output.detail("official Rex CLI"));
        }
        serde_json::from_str(output.stdout.trim()).map_err(|error| {
            format!(
                "official Rex CLI did not return JSON: {error}; {}",
                output.detail("official Rex CLI")
            )
        })
    }

    fn capture(&self, session: &OwnedRexSession) -> Result<String, String> {
        let args = vec![
            "capture".to_owned(),
            "-s".to_owned(),
            session.session_id.clone(),
            "-b".to_owned(),
            session.block_id.clone(),
            "--unwrap".to_owned(),
        ];
        let output = self.run_rex(&args, None);
        if !output.success || output.timed_out {
            return Err(output.detail("official Rex capture"));
        }
        Ok(output.stdout)
    }

    pub fn create_receiver_session(&mut self, role: &str) -> OwnedRexSession {
        let token = self.unique_token();
        let run_id = format!("rex-live-{role}-{token}");
        let label = format!("rex-api-foundations-20261108-live-{role}-{token}");
        let agent_session_id = format!("rex-live-agent-{role}-{token}");
        let ack_file = self.root.join(format!("receiver-{role}-{token}.jsonl"));
        let command = vec![
            self.python.to_string_lossy().into_owned(),
            "-u".to_owned(),
            self.receiver_script.to_string_lossy().into_owned(),
            "--run-id".to_owned(),
            run_id.clone(),
            "--ack-file".to_owned(),
            ack_file.to_string_lossy().into_owned(),
        ];
        let request = json!({
            "label": label,
            "initial_windows": [{
                "window_label": "receiver",
                "layout": {
                    "block": {
                        "flavor": "com.superlogical.terminal.shell",
                        "options": {
                            "shell": "none",
                            "login": false,
                            "command": command,
                            "cwd": self.root.to_string_lossy(),
                        }
                    }
                }
            }]
        });
        let args = vec![
            "api".to_owned(),
            "call".to_owned(),
            "session.create".to_owned(),
            "-".to_owned(),
        ];
        let creation = match self.try_rex_json(&args, Some(&request.to_string())) {
            Ok(creation) => creation,
            Err(error) => {
                self.evidence(
                    "session_create_failed",
                    json!({"label": label, "run_id": run_id, "error": error}),
                );
                panic!("session.create failed for an unknown session: {error}");
            }
        };
        self.evidence(
            "session_create_response",
            json!({
                "label": label,
                "run_id": run_id,
                "response": creation.clone(),
            }),
        );
        let session_id = creation["session_id"]
            .as_str()
            .filter(|value| value.starts_with("session:"))
            .unwrap_or_else(|| {
                panic!("session.create did not return one exact session id: {creation}")
            })
            .to_owned();
        let windows = creation["initial_windows"]
            .as_array()
            .unwrap_or_else(|| panic!("session.create did not return initial_windows: {creation}"));
        assert_eq!(windows.len(), 1, "one initial window: {creation}");
        let block_ids = windows[0]["block_ids"]
            .as_array()
            .unwrap_or_else(|| panic!("session.create did not return block_ids: {creation}"));
        assert_eq!(block_ids.len(), 1, "one initial block: {creation}");
        let block_id = block_ids[0]
            .as_str()
            .filter(|value| value.starts_with("block:"))
            .unwrap_or_else(|| {
                panic!("session.create did not return one exact block id: {creation}")
            })
            .to_owned();
        let owned = OwnedRexSession {
            label,
            run_id,
            agent_session_id,
            session_id,
            block_id,
            cwd: self.root.clone(),
            ack_file,
            creation_result: creation,
            destroyed: false,
        };
        self.evidence(
            "session_created",
            json!({
                "label": owned.label,
                "run_id": owned.run_id,
                "agent_session_id": owned.agent_session_id,
                "session_id": owned.session_id,
                "block_id": owned.block_id,
                "cwd": owned.cwd.to_string_lossy(),
                "creation_result": owned.creation_result,
            }),
        );
        self.sessions.push(owned.clone());
        owned
    }

    pub fn provider_facts(&self, owned: &OwnedRexSession) -> Value {
        json!({
            "session_id": owned.agent_session_id,
            "rex": {
                "server": self.rex_server,
                "session_id": owned.session_id,
                "block_id": owned.block_id,
                "cwd": self.root.to_string_lossy(),
            }
        })
    }

    pub fn context_provide(&self, owned: &OwnedRexSession) -> Value {
        let facts = self.provider_facts(owned).to_string();
        let operation = format!("ctx-rex-live-{}-{}", owned.run_id, self.unique_token());
        self.run_ok(
            &[
                "context",
                "--op",
                operation.as_str(),
                "--provide",
                facts.as_str(),
            ],
            Some(&owned.agent_session_id),
        )
    }

    /// Facts for `owned`'s agent session that target `target`'s actual Rex
    /// block. A later successful binding with these facts must replace the
    /// current block owner without transferring its identity or work.
    pub fn provider_facts_on_block(
        &self,
        owned: &OwnedRexSession,
        target: &OwnedRexSession,
    ) -> Value {
        json!({
            "session_id": owned.agent_session_id,
            "rex": {
                "server": self.rex_server,
                "session_id": target.session_id,
                "block_id": target.block_id,
                "cwd": self.root.to_string_lossy(),
            }
        })
    }

    pub fn context_provide_on_block(
        &self,
        owned: &OwnedRexSession,
        target: &OwnedRexSession,
    ) -> Value {
        let facts = self.provider_facts_on_block(owned, target).to_string();
        let operation = format!(
            "ctx-rex-live-replace-{}-{}",
            owned.run_id,
            self.unique_token()
        );
        self.run_ok(
            &[
                "context",
                "--op",
                operation.as_str(),
                "--provide",
                facts.as_str(),
            ],
            Some(&owned.agent_session_id),
        )
    }

    /// Read the test-owned identity map. Assertions must treat this as the
    /// single control source for current ownership.
    pub fn identity_map(&self) -> Value {
        let path = self.host_state.join("identity-map.json");
        let bytes = fs::read(&path).unwrap_or_else(|error| {
            panic!("read test-owned identity map {}: {error}", path.display())
        });
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!("decode test-owned identity map {}: {error}", path.display())
        })
    }

    /// Every Rex block mapped to its currently published identity names. Only
    /// `active current` rows participate; previous relations are history.
    pub fn rex_block_current_owners(&self) -> std::collections::BTreeMap<String, Vec<String>> {
        let map = self.identity_map();
        let mut owners: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        let Some(slots) = map["slots"].as_array() else {
            return owners;
        };
        for slot in slots {
            let Some(current) = slot.get("current").filter(|value| !value.is_null()) else {
                continue;
            };
            if current["transport"]["kind"] != "rex" {
                continue;
            }
            let Some(block) = current["transport"]["rex_endpoint"]["block_id"].as_str() else {
                continue;
            };
            let name = slot["identity_key"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            owners.entry(block.to_owned()).or_default().push(name);
        }
        owners
    }

    pub fn wait_agent_state(&self, owned: &OwnedRexSession, expected: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let value = self.run_ok(&["context", "--query"], Some(&owned.agent_session_id));
            let current = value
                .get("result")
                .and_then(|result| result.get("snapshot"))
                .unwrap_or(&value);
            if current["agent"]["thread_state"] == expected {
                self.evidence(
                    "agent_status",
                    json!({"expected": expected, "agent": current["agent"]}),
                );
                return current["agent"].clone();
            }
            assert!(
                Instant::now() < deadline,
                "Rex status did not reach {expected}: {value}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn wait_receiver_ready(&self, owned: &OwnedRexSession, timeout: Duration) {
        self.wait_receiver_document(owned, timeout, |value| {
            value["kind"] == "test_receiver_ready" && value["run_id"] == owned.run_id
        });
    }

    pub fn wait_receiver_document<F>(
        &self,
        owned: &OwnedRexSession,
        timeout: Duration,
        predicate: F,
    ) -> Value
    where
        F: Fn(&Value) -> bool,
    {
        let deadline = Instant::now() + timeout;
        let mut last_capture = String::new();
        loop {
            if let Ok(text) = fs::read_to_string(&owned.ack_file) {
                for line in text.lines() {
                    let Ok(value) = serde_json::from_str::<Value>(line) else {
                        continue;
                    };
                    if predicate(&value) {
                        return value;
                    }
                }
            }
            if let Ok(capture) = self.capture(owned) {
                last_capture = capture;
            }
            if Instant::now() >= deadline {
                panic!(
                    "receiver document did not arrive for {}; ack_file={} capture={last_capture}",
                    owned.run_id,
                    owned.ack_file.display()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn wait_capture_contains(&self, owned: &OwnedRexSession, needle: &str, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut last_capture = String::new();
        loop {
            if let Ok(capture) = self.capture(owned) {
                if capture.contains(needle) {
                    return;
                }
                last_capture = capture;
            }
            if Instant::now() >= deadline {
                panic!(
                    "capture for {} did not contain {needle:?}: {last_capture}",
                    owned.run_id
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn write_frame_and_enter(&self, owned: &OwnedRexSession, case_id: &str, payload: &str) {
        let frame = format!(
            "\u{1e}REXFRAME:{case_id}\t{}\n{payload}\u{1e}",
            payload.len()
        );
        let write_args = vec![
            "api".to_owned(),
            "call".to_owned(),
            "-s".to_owned(),
            owned.session_id.clone(),
            "com.superlogical.terminal.write".to_owned(),
            "-".to_owned(),
        ];
        let request = json!({
            "session_id": owned.session_id,
            "block_id": owned.block_id,
            "args": {"data": frame},
        });
        let write = self.rex_json(&write_args, Some(&request.to_string()));
        assert_eq!(
            write["bytes_written"].as_u64(),
            Some(frame.len() as u64),
            "the official write must accept the complete frame: {write}"
        );
        let enter_args = vec![
            "send-key".to_owned(),
            "-s".to_owned(),
            owned.session_id.clone(),
            "-b".to_owned(),
            owned.block_id.clone(),
            "enter".to_owned(),
        ];
        let enter = self.run_rex(&enter_args, None);
        assert!(
            enter.success && !enter.timed_out,
            "{}",
            enter.detail("official Rex send-key")
        );
    }

    fn session_list(&self) -> Result<Value, String> {
        let args = vec![
            "api".to_owned(),
            "call".to_owned(),
            "session.list".to_owned(),
            "-".to_owned(),
        ];
        self.try_rex_json(&args, Some("{}"))
    }

    fn destroy_session(&self, owned: &OwnedRexSession) -> Result<(), String> {
        let args = vec![
            "api".to_owned(),
            "call".to_owned(),
            "-s".to_owned(),
            owned.session_id.clone(),
            "session.destroy".to_owned(),
            "-".to_owned(),
        ];
        let body = json!({"session_id": owned.session_id}).to_string();
        self.try_rex_json(&args, Some(&body))?;
        let sessions = self.session_list()?;
        let still_present = sessions["sessions"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|item| item["session_id"].as_str() == Some(owned.session_id.as_str()))
        });
        if still_present {
            return Err(format!(
                "session.destroy returned success but {} remains in session.list",
                owned.session_id
            ));
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.daemon.is_some() {
            self.stop_daemon();
        }
        for index in (0..self.sessions.len()).rev() {
            if self.sessions[index].destroyed {
                continue;
            }
            let owned = self.sessions[index].clone();
            match self.destroy_session(&owned) {
                Ok(()) => {
                    self.sessions[index].destroyed = true;
                    self.evidence(
                        "session_destroyed",
                        json!({
                            "session_id": owned.session_id,
                            "block_id": owned.block_id,
                        }),
                    );
                }
                Err(error) => {
                    self.evidence(
                        "session_destroy_failed",
                        json!({
                            "session_id": owned.session_id,
                            "block_id": owned.block_id,
                            "error": error,
                        }),
                    );
                    errors.push(error);
                }
            }
        }
        errors
    }
}

impl Drop for RexLiveFixture {
    fn drop(&mut self) {
        let errors = self.finish();
        if errors.is_empty() {
            eprintln!(
                "Rex live evidence retained at {}",
                self.evidence_path.display()
            );
        } else {
            eprintln!(
                "preserving Rex live evidence after cleanup errors at {}: {errors:?}",
                self.evidence_path.display()
            );
        }
    }
}
