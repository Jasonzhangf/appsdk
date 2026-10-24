use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

struct IsolatedDaemon {
    binary: PathBuf,
    root: PathBuf,
    host: PathBuf,
    home: PathBuf,
    child: Option<Child>,
    tmux: Option<(PathBuf, String)>,
}

impl IsolatedDaemon {
    fn start(password: Option<&str>) -> Self {
        let root = unique_root("lifecycle");
        let host = root.join("host");
        let home = root.join("home");
        std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
        std::fs::create_dir_all(&host).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        if let Some(password) = password {
            let config = home.join(".appsdk");
            std::fs::create_dir_all(&config).unwrap();
            std::fs::write(
                config.join("config.toml"),
                format!("[maintenance]\npassword = \"{password}\"\n"),
            )
            .unwrap();
        }

        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(host.join("test-daemon.log"))
            .unwrap();
        let binary = std::env::var_os("COLLAB_TEST_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_collab")));
        let mut command = Command::new(&binary);
        command
            .arg("serve")
            .current_dir(&root)
            .env("HOME", &home)
            .env("COLLAB_STATE_DIR", &host)
            .env("CODEX_HOME", &home)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("COLLAB_SOCKET_PATH")
            .env_remove("COLLAB_HOST_SOCKET")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        let child = command.spawn().expect("spawn isolated collab serve");
        let mut fixture = Self {
            binary,
            root,
            host,
            home,
            child: Some(child),
            tmux: None,
        };
        fixture.wait_until_alive();
        fixture
    }

    fn socket(&self) -> PathBuf {
        self.host.join("server.sock")
    }

    fn base_command(&self, arg: &str) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .arg(arg)
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("COLLAB_STATE_DIR", &self.host)
            .env("CODEX_HOME", &self.home)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("COLLAB_SOCKET_PATH")
            .env_remove("COLLAB_HOST_SOCKET")
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("COLLAB_APPSERVER_NAMESPACE");
        command
    }

    fn run(&self, arg: &str) -> Output {
        self.base_command(arg).output().expect("run collab command")
    }

    fn start_tmux_pane(&mut self) -> TmuxPane {
        let socket = self.root.join("owned-tmux.sock");
        let session = format!("lifecycle-{}", std::process::id());
        let output = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args(["new-session", "-d", "-s", &session, "sh -i"])
            .output()
            .expect("start owned tmux session");
        assert!(
            output.status.success(),
            "tmux failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        self.tmux = Some((socket.clone(), session.clone()));
        std::fs::write(
            self.host.join("tmux-endpoints.json"),
            serde_json::to_vec(&json!({"endpoints": [socket]})).unwrap(),
        )
        .unwrap();
        let server_pid =
            String::from_utf8(tmux(&socket, &["display-message", "-p", "#{pid}"]).stdout)
                .unwrap()
                .trim()
                .parse::<u32>()
                .unwrap();
        let pane_id =
            String::from_utf8(tmux(&socket, &["display-message", "-p", "#{pane_id}"]).stdout)
                .unwrap()
                .trim()
                .to_owned();
        TmuxPane {
            socket,
            server_pid,
            pane_id,
            session_anchor: format!("session-{session}"),
            thread_anchor: format!("thread-{session}"),
        }
    }

    fn context_with_pane(&self, pane: &TmuxPane) -> Output {
        self.command_with_pane(pane, &["context"])
    }

    fn command_with_pane(&self, pane: &TmuxPane, args: &[&str]) -> Output {
        self.command_at(&self.root, pane, args)
    }

    fn command_at(&self, root: &Path, pane: &TmuxPane, args: &[&str]) -> Output {
        static COMMAND: AtomicU64 = AtomicU64::new(0);
        let sequence = COMMAND.fetch_add(1, Ordering::Relaxed);
        let prefix = self.root.join(format!("pane-command-{sequence}"));
        let script = prefix.with_extension("sh");
        let stdout = prefix.with_extension("stdout");
        let stderr = prefix.with_extension("stderr");
        let status = prefix.with_extension("status");
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
        let command = args
            .iter()
            .map(|value| quote(value))
            .collect::<Vec<_>>()
            .join(" ");
        std::fs::write(&script, format!(
            "cd {root}\nunset COLLAB_SOCKET_PATH COLLAB_HOST_SOCKET COLLAB_LOCK_PATH COLLAB_HOST_LOCK COLLAB_APPSERVER_SOCKET CODEX_APP_SERVER_SOCKET COLLAB_APPSERVER_NAMESPACE\nexport HOME={home} CODEX_HOME={home} COLLAB_STATE_DIR={host} TMUX_PANE={pane} CODEX_SESSION_ID={session} CODEX_THREAD_ID={thread}\n{binary} {command} >{stdout} 2>{stderr}\nprintf '%s' \"$?\" >{status}\n",
            root=quote(&root.to_string_lossy()), home=quote(&self.home.to_string_lossy()),
            host=quote(&self.host.to_string_lossy()), pane=quote(&pane.pane_id),
            session=quote(&pane.session_anchor), thread=quote(&pane.thread_anchor),
            binary=quote(&self.binary.to_string_lossy()), stdout=quote(&stdout.to_string_lossy()),
            stderr=quote(&stderr.to_string_lossy()), status=quote(&status.to_string_lossy()),
        )).unwrap();
        let start = format!("sh {}", quote(&script.to_string_lossy()));
        assert!(tmux(
            &pane.socket,
            &["send-keys", "-t", &pane.pane_id, "-l", &start]
        )
        .status
        .success());
        assert!(
            tmux(&pane.socket, &["send-keys", "-t", &pane.pane_id, "Enter"])
                .status
                .success()
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        while std::fs::read_to_string(&status)
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .is_none()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(25));
        }
        assert!(
            status.exists(),
            "owned pane command did not finish: {args:?}"
        );
        let code: i32 = std::fs::read_to_string(status).unwrap().parse().unwrap();
        Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: std::fs::read(stdout).unwrap(),
            stderr: std::fs::read(stderr).unwrap(),
        }
    }

    fn runtime_journal(&self, project: &Path) -> PathBuf {
        let canonical = std::fs::canonicalize(project).unwrap();
        std::fs::read_to_string(self.host.join("routes.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|record| record["project_scope"] == canonical.to_string_lossy().as_ref())
            .map(|record| {
                PathBuf::from(record["storage_root"].as_str().unwrap())
                    .join(".agent-collab/server/journal.jsonl")
            })
            .expect("registered project has a centralized runtime journal")
    }

    fn wait_until_alive(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
                panic!(
                    "isolated daemon exited before readiness: {status}; log={}",
                    self.log_text()
                );
            }
            if ping(&self.socket()).is_ok() {
                return;
            }
            if Instant::now() >= deadline {
                panic!("isolated daemon did not become ready: {}", self.log_text());
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn wait_for_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.child.as_mut().unwrap().try_wait().unwrap().is_some() {
                let mut child = self.child.take().unwrap();
                let _ = child.wait();
                return;
            }
            if Instant::now() >= deadline {
                panic!(
                    "isolated daemon did not exit after authenticated shutdown: {}",
                    self.log_text()
                );
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn log_text(&self) -> String {
        std::fs::read_to_string(self.host.join("test-daemon.log")).unwrap_or_default()
    }
}

impl Drop for IsolatedDaemon {
    fn drop(&mut self) {
        if let Some((socket, session)) = self.tmux.take() {
            let _ = Command::new("tmux")
                .arg("-S")
                .arg(&socket)
                .args(["kill-session", "-t", &session])
                .output();
        }
        if let Some(mut child) = self.child.take() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        if std::thread::panicking() {
            eprintln!(
                "preserving failed lifecycle fixture: root={} host={} home={}",
                self.root.display(),
                self.host.display(),
                self.home.display()
            );
        } else {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

struct TmuxPane {
    socket: PathBuf,
    server_pid: u32,
    pane_id: String,
    session_anchor: String,
    thread_anchor: String,
}

fn unique_root(label: &str) -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    PathBuf::from("/tmp").join(format!(
        "collab-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn tmux(socket: &Path, args: &[&str]) -> Output {
    let output = Command::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(args)
        .output()
        .expect("run owned tmux command");
    assert!(
        output.status.success(),
        "tmux {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn ping(socket: &Path) -> std::io::Result<Value> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_millis(300)))?;
    stream.write_all(b"{\"op\":\"Ping\"}\n")?;
    stream.flush()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let value: Value = serde_json::from_str(line.trim())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if value["ok"] != true {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Ping failed: {value}"),
        ));
    }
    Ok(value)
}

fn project_context(root: &Path) -> Value {
    let root = root.canonicalize().unwrap();
    let root = root.to_string_lossy();
    json!({
        "app_scope_id": "appserver-cli",
        "canonical_root": root,
        "project_scope": root,
    })
}

fn wire_request(socket: &Path, root: &Path, request: Value) -> Value {
    let mut envelope = request.as_object().unwrap().clone();
    envelope.insert("project_context".into(), project_context(root));
    let line = serde_json::to_vec(&envelope).unwrap();
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream.write_all(&line).unwrap();
    stream.write_all(b"\n").unwrap();
    stream.flush().unwrap();
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response).unwrap();
    serde_json::from_str(response.trim()).unwrap()
}

fn run_pty(fixture: &IsolatedDaemon, input: &[u8]) -> Output {
    let mut terminal = Command::new("script");
    terminal.arg("-q");
    if cfg!(target_os = "linux") {
        let executable = fixture.binary.to_string_lossy().replace('\'', "'\\''");
        terminal.args([
            "-e",
            "-c",
            &format!("exec '{executable}' down"),
            "/dev/null",
        ]);
    } else {
        terminal.arg("/dev/null").arg(&fixture.binary).arg("down");
    }
    let mut child = terminal
        .current_dir(&fixture.root)
        .env("HOME", &fixture.home)
        .env("COLLAB_STATE_DIR", &fixture.host)
        .env("CODEX_HOME", &fixture.home)
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_INTERNAL_ORIGINATOR_OVERRIDE")
        .env_remove("DSH_SESSION_ID")
        .env_remove("AGY_SESSION_ID")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("COLLAB_SOCKET_PATH")
        .env_remove("COLLAB_HOST_SOCKET")
        .env_remove("COLLAB_APPSERVER_SOCKET")
        .env_remove("CODEX_APP_SERVER_SOCKET")
        .env_remove("COLLAB_APPSERVER_NAMESPACE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start pty down command");
    let mut stdout = child.stdout.take().unwrap();
    let mut prefix = Vec::new();
    while !prefix.ends_with(b"Password: ") {
        let mut byte = [0];
        if stdout.read(&mut byte).unwrap() == 0 {
            break;
        }
        prefix.push(byte[0]);
    }
    {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input).unwrap();
        stdin.flush().unwrap();
    }
    child.stdout = Some(stdout);
    let mut output = child.wait_with_output().unwrap();
    prefix.extend_from_slice(&output.stdout);
    output.stdout = prefix;
    output
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_no_text(output: &Output, forbidden: &[&str]) {
    let text = output_text(output);
    for value in forbidden {
        assert!(
            !text.contains(value),
            "output leaked {value:?}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, current: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                out.insert(relative, Vec::new());
                visit(root, &path, out);
            } else if path.is_file() {
                out.insert(relative, std::fs::read(&path).unwrap_or_default());
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

fn config_file(home: &Path) -> PathBuf {
    home.join(".appsdk/config.toml")
}

#[test]
fn help_and_mcp_hide_maintenance_commands_and_credentials() {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_collab"));
    let help = Command::new(&binary)
        .arg("--help")
        .output()
        .expect("run collab help");
    assert!(help.status.success());
    let help_text = output_text(&help).to_lowercase();
    for command in ["up", "down"] {
        assert!(
            !help_text
                .lines()
                .any(|line| line.trim_start().starts_with(&format!("{command} "))),
            "agent help exposed {command}: {help_text}"
        );
    }
    for secret in ["maintenance password", "maintenance_password"] {
        assert!(
            !help_text.contains(secret),
            "agent help exposed {secret}: {help_text}"
        );
    }
    let reset_help = Command::new(&binary)
        .args(["reset", "--help"])
        .output()
        .unwrap();
    let reset_help = output_text(&reset_help);
    for hidden in ["--host", "--storage-root", "--approval", "--discard-legacy"] {
        assert!(!reset_help.contains(hidden), "{reset_help}");
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_collab-mcp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start collab-mcp");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    let tools = response["result"]["tools"].as_array().unwrap();
    let serialized = serde_json::to_string(tools).unwrap().to_lowercase();
    assert!(!serialized.contains("maintenance password"));
    assert!(!serialized.contains("maintenance_password"));
    for tool in tools {
        let name = tool["name"].as_str().unwrap_or_default();
        assert_ne!(name, "collab_up");
        assert_ne!(name, "collab_down");
        assert_ne!(name, "collab_config");
    }
}

#[test]
fn agent_runtime_cannot_manage_daemon_even_with_a_terminal() {
    let fixture = IsolatedDaemon::start(None);
    let before = std::fs::read(fixture.host.join("server.pid")).unwrap();
    for action in ["up", "down"] {
        let mut terminal = Command::new("script");
        terminal.arg("-q");
        if cfg!(target_os = "linux") {
            let executable = fixture.binary.to_string_lossy().replace('\'', "'\\''");
            terminal.args([
                "-e",
                "-c",
                &format!("exec '{executable}' {action}"),
                "/dev/null",
            ]);
        } else {
            terminal.arg("/dev/null").arg(&fixture.binary).arg(action);
        }
        let result = terminal
            .current_dir(&fixture.root)
            .env("HOME", &fixture.home)
            .env("COLLAB_STATE_DIR", &fixture.host)
            .env("CODEX_THREAD_ID", "agent-runtime")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output_text(&result).contains("USER_MAINTENANCE_REQUIRED"));
        assert!(!output_text(&result).contains("Password:"));
        assert_eq!(
            std::fs::read(fixture.host.join("server.pid")).unwrap(),
            before
        );
        assert!(!fixture.host.join("DOWN").exists());
        assert!(ping(&fixture.socket()).is_ok());
    }
}

#[test]
fn noninteractive_up_and_down_have_no_side_effects() {
    let root = unique_root("noninteractive");
    let host = root.join("host");
    let home = root.join("home");
    std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
    std::fs::create_dir_all(&host).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_collab"));
    let before_root = tree(&root);
    let before_host = tree(&host);

    for arg in ["up", "down"] {
        let output = Command::new(&binary)
            .arg(arg)
            .current_dir(&root)
            .env("HOME", &home)
            .env("COLLAB_STATE_DIR", &host)
            .env("CODEX_HOME", &home)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .output()
            .unwrap();
        assert!(!output.status.success(), "{arg} must require a terminal");
        assert_no_text(&output, &["admin", "maintenance password"]);
    }

    assert_eq!(tree(&root), before_root);
    assert_eq!(tree(&host), before_host);
    assert!(!config_file(&home).exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn context_down_marker_fails_without_startup_or_maintenance_hint() {
    let root = unique_root("context-down");
    let host = root.join("host");
    let home = root.join("home");
    std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
    std::fs::create_dir_all(&host).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(host.join("DOWN"), b"explicitly stopped\n").unwrap();
    let before = tree(&root);
    let output = Command::new(env!("CARGO_BIN_EXE_collab"))
        .arg("context")
        .current_dir(&root)
        .env("HOME", &home)
        .env("COLLAB_STATE_DIR", &host)
        .env("CODEX_HOME", &home)
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let text = output_text(&output).to_lowercase();
    assert!(text.contains("daemon_unavailable"), "{text}");
    assert!(!text.contains("collab up"), "{text}");
    assert!(!text.contains("collab down"), "{text}");
    assert_eq!(tree(&root), before);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn context_does_not_start_for_absent_or_stale_endpoints() {
    for stale in [false, true] {
        let root = unique_root(if stale {
            "context-stale"
        } else {
            "context-absent"
        });
        let host = root.join("host");
        let home = root.join("home");
        std::fs::create_dir_all(&host).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        if stale {
            std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
            let listener = UnixListener::bind(host.join("server.sock")).unwrap();
            drop(listener);
        } else {
            let git = Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(git.status.success());
        }
        let before = tree(&root);
        let output = Command::new(env!("CARGO_BIN_EXE_collab"))
            .arg("context")
            .current_dir(&root)
            .env("HOME", &home)
            .env("COLLAB_STATE_DIR", &host)
            .env("CODEX_HOME", &home)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .output()
            .unwrap();
        assert!(!output.status.success(), "stale={stale}");
        let text = output_text(&output).to_lowercase();
        assert!(text.contains("daemon_unavailable"), "{text}");
        assert!(!text.contains("collab up"), "{text}");
        assert_eq!(tree(&root), before, "stale={stale}");
        let _ = std::fs::remove_dir_all(root);
    }
}

#[test]
fn legacy_or_unauthenticated_shutdown_is_rejected_without_events_or_marker() {
    let fixture = IsolatedDaemon::start(None);
    let event_path = fixture.host.join("events.jsonl");
    let before_events = std::fs::read(&event_path).unwrap_or_default();
    for request in [
        json!({"op":"Shutdown","operator":true}),
        json!({"op":"Shutdown"}),
        json!({"op":"Shutdown","password":"wrong-password"}),
    ] {
        let response = wire_request(&fixture.socket(), &fixture.root, request);
        assert_eq!(response["ok"], false, "{response}");
        assert!(!fixture.host.join("DOWN").exists());
        assert_eq!(
            std::fs::read(&event_path).unwrap_or_default(),
            before_events
        );
        ping(&fixture.socket()).expect("daemon remains alive after rejected shutdown");
    }
}

#[test]
fn pty_password_rejects_default_for_custom_config_and_never_echoes_it() {
    const CUSTOM: &str = "custom-lifecycle-secret";
    let mut fixture = IsolatedDaemon::start(Some(CUSTOM));

    let wrong = run_pty(&fixture, b"admin\n");
    assert!(!wrong.status.success(), "{}", output_text(&wrong));
    assert_no_text(&wrong, &[CUSTOM]);
    assert!(!fixture.host.join("DOWN").exists());
    ping(&fixture.socket()).expect("wrong password must not stop the daemon");

    let correct = run_pty(&fixture, format!("{CUSTOM}\n").as_bytes());
    assert!(correct.status.success(), "{}", output_text(&correct));
    assert_no_text(&correct, &[CUSTOM]);
    fixture.wait_for_exit();
    assert!(fixture.host.join("DOWN").is_file());
    assert!(!std::fs::read_to_string(fixture.host.join("events.jsonl"))
        .unwrap_or_default()
        .contains(CUSTOM));
    assert!(!fixture.log_text().contains(CUSTOM));
}

#[test]
fn pty_default_user_password_stops_only_the_owned_daemon() {
    let mut fixture = IsolatedDaemon::start(None);
    let output = run_pty(&fixture, b"admin\n");
    assert!(output.status.success(), "{}", output_text(&output));
    assert_no_text(&output, &["admin"]);
    fixture.wait_for_exit();
    assert!(fixture.host.join("DOWN").is_file());
}

#[test]
fn pty_eof_and_cancel_do_not_stop_or_mutate_the_daemon() {
    for input in [b"\x04".to_vec(), b"\x03".to_vec()] {
        let fixture = IsolatedDaemon::start(None);
        let before_events = std::fs::read(fixture.host.join("events.jsonl")).unwrap_or_default();
        let output = run_pty(&fixture, &input);
        assert!(!output.status.success(), "{}", output_text(&output));
        assert!(!fixture.host.join("DOWN").exists());
        assert_eq!(
            std::fs::read(fixture.host.join("events.jsonl")).unwrap_or_default(),
            before_events
        );
        ping(&fixture.socket()).expect("EOF or cancel must not stop the daemon");
    }
}

#[test]
fn available_context_restores_identity_without_register_or_pid_change() {
    let mut fixture = IsolatedDaemon::start(None);
    let pane = fixture.start_tmux_pane();
    let first = fixture.context_with_pane(&pane);
    assert!(
        first.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first["ok"], true, "{first}");
    let worker = first["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let binding = first["result"]["snapshot"]["binding"].clone();
    let journal = fixture.runtime_journal(&fixture.root);
    let before_journal = std::fs::read(&journal).unwrap_or_default();
    let before_pid = std::fs::read_to_string(fixture.host.join("server.pid")).unwrap();

    let second = fixture.context_with_pane(&pane);
    assert!(
        second.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    let second: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(second["ok"], true, "{second}");
    assert_eq!(
        second["result"]["snapshot"]["identity"]["worker_id"],
        worker
    );
    assert_eq!(second["result"]["snapshot"]["binding"], binding);
    assert_eq!(
        std::fs::read_to_string(fixture.host.join("server.pid")).unwrap(),
        before_pid
    );
    assert_eq!(
        registration_events(&std::fs::read(&journal).unwrap_or_default()),
        registration_events(&before_journal),
        "ordinary restore must not write a Register event"
    );
}

// Presence and ledger timers may append observations while context is read.
// Compare the registration owner events that this test promises not to repeat.
fn registration_events(bytes: &[u8]) -> Vec<Value> {
    std::str::from_utf8(bytes).unwrap().lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| !matches!(event["ev"].as_str(),
            Some("AgentPresenceObserved" | "AgentActivityObserved"
                | "GlobalRuntimeBindingLedgerClassified" | "GlobalLedgerScanReceiptRecorded")))
        .collect()
}

#[test]
fn self_request_cannot_unbind_another_projects_pane() {
    let mut fixture = IsolatedDaemon::start(None);
    let pane = fixture.start_tmux_pane();
    let own = fixture.context_with_pane(&pane);
    let own: Value = serde_json::from_slice(&own.stdout).unwrap();
    let own_id = own["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap();
    let credential: Value = serde_json::from_slice(
        &std::fs::read(
            fixture
                .host
                .join("identities")
                .join(own_id)
                .join("identity.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let other = fixture.root.join("other-project");
    std::fs::create_dir_all(other.join(".agent-collab")).unwrap();
    let window = tmux(
        &pane.socket,
        &["new-window", "-d", "-P", "-F", "#{pane_id}", "sh -i"],
    );
    let other_pane = TmuxPane {
        socket: pane.socket.clone(),
        server_pid: pane.server_pid,
        pane_id: String::from_utf8(window.stdout).unwrap().trim().to_owned(),
        session_anchor: "other-project-session".into(),
        thread_anchor: "other-project-thread".into(),
    };
    let other_context = fixture.command_at(&other, &other_pane, &["context"]);
    assert!(
        other_context.status.success(),
        "{}",
        output_text(&other_context)
    );
    let other_context: Value = serde_json::from_slice(&other_context.stdout).unwrap();
    let other_binding = &other_context["result"]["snapshot"]["binding"];
    assert!(fixture
        .command_at(
            &other,
            &other_pane,
            &["master", "promote", "--approval", "isolated other master"]
        )
        .status
        .success());
    let journal = fixture.runtime_journal(&other);
    let before = std::fs::read(&journal).unwrap();
    let request = json!({"op":"UnregisterSelf", "worker_id":own_id,
        "token":credential["token"], "facts":{"tmux":{
            "endpoint":other_binding["tmux_endpoint"], "cwd":other_binding["project_scope"]}}});
    let _response = wire_request(&fixture.socket(), &fixture.root, request);
    assert_eq!(
        std::fs::read(&journal).unwrap(),
        before,
        "supplied foreign pane facts must not authorize retirement of its identity or grant"
    );
    let route = wire_request(
        &fixture.socket(),
        &other,
        json!({"op":"RouteResolve", "tmux_endpoint":other_binding["tmux_endpoint"]}),
    );
    assert_eq!(route["ok"], true, "{route}");
}

#[test]
fn current_pane_can_remove_own_binding_with_unfinished_work_and_keep_its_host() {
    let mut fixture = IsolatedDaemon::start(None);
    let pane = fixture.start_tmux_pane();
    let first = fixture.context_with_pane(&pane);
    assert!(first.status.success(), "{}", output_text(&first));
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    let binding = &first["result"]["snapshot"]["binding"];
    let daemon_pid = std::fs::read(fixture.host.join("server.pid")).unwrap();
    let published = fixture.command_with_pane(
        &pane,
        &[
            "board",
            "publish",
            "unfinished-self-work",
            "--title",
            "continue later",
            "--description",
            "must survive self unbind",
            "--delivery-condition",
            "later",
            "--test-condition",
            "this test",
        ],
    );
    assert!(published.status.success(), "{}", output_text(&published));
    let published: Value = serde_json::from_slice(&published.stdout).unwrap();
    let revision = published["task"]["revision"].as_u64().unwrap().to_string();
    let claimed = fixture.command_with_pane(
        &pane,
        &[
            "board",
            "claim",
            "unfinished-self-work",
            "--expected-revision",
            &revision,
        ],
    );
    assert!(claimed.status.success(), "{}", output_text(&claimed));
    let journal = fixture.runtime_journal(&fixture.root);
    let before = std::fs::read_to_string(&journal).unwrap();
    let worker = first["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap();
    let identity_path = fixture
        .host
        .join("identities")
        .join(worker)
        .join("identity.json");
    let mut stale: Value = serde_json::from_slice(&std::fs::read(&identity_path).unwrap()).unwrap();
    stale["token"] = json!("stale-local-self-token");
    std::fs::write(&identity_path, serde_json::to_vec(&stale).unwrap()).unwrap();
    let removed = fixture.command_with_pane(&pane, &["worker", "close", "--self"]);
    assert!(removed.status.success(), "{}", output_text(&removed));
    let removed: Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(removed["ok"], true, "{removed}");
    assert_eq!(removed["result"]["outcome"], "complete", "{removed}");
    assert_eq!(
        std::fs::read(fixture.host.join("server.pid")).unwrap(),
        daemon_pid
    );
    assert!(ping(&fixture.socket()).is_ok());
    assert!(tmux(
        &pane.socket,
        &["display-message", "-p", "-t", &pane.pane_id, "#{pane_id}"]
    )
    .status
    .success());
    let route = wire_request(
        &fixture.socket(),
        &fixture.root,
        json!({"op":"RouteResolve", "tmux_endpoint":binding["tmux_endpoint"]}),
    );
    assert_eq!(route["ok"], false, "old route must be unusable: {route}");
    let after = std::fs::read_to_string(journal).unwrap();
    assert!(
        after.starts_with(&before),
        "self removal must retain task history"
    );
    let repeated = fixture.command_with_pane(&pane, &["worker", "close", "--self"]);
    assert!(repeated.status.success(), "{}", output_text(&repeated));
}

#[test]
fn project_master_resets_online_and_erases_its_history_without_touching_another_project() {
    let mut fixture = IsolatedDaemon::start(None);
    let pane = fixture.start_tmux_pane();
    let registered = fixture.context_with_pane(&pane);
    assert!(registered.status.success(), "{}", output_text(&registered));
    let promoted = fixture.command_with_pane(
        &pane,
        &[
            "master",
            "promote",
            "--approval",
            "user authorizes isolated project owner",
        ],
    );
    assert!(promoted.status.success(), "{}", output_text(&promoted));
    let published = fixture.command_with_pane(
        &pane,
        &[
            "board",
            "publish",
            "erase-project-history",
            "--title",
            "erase-project-history",
            "--description",
            "erase-project-history",
            "--delivery-condition",
            "reset clears this",
            "--test-condition",
            "this test",
        ],
    );
    assert!(published.status.success(), "{}", output_text(&published));
    let asset = fixture
        .root
        .join(".agent-collab/mailboxes/reset-owned-asset");
    std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
    std::fs::write(&asset, b"erase-project-history").unwrap();

    let other = fixture.root.join("other-project");
    std::fs::create_dir_all(other.join(".agent-collab")).unwrap();
    let window = tmux(
        &pane.socket,
        &[
            "new-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-n",
            "other-project",
            "sh -i",
        ],
    );
    let other_pane = TmuxPane {
        socket: pane.socket.clone(),
        server_pid: pane.server_pid,
        pane_id: String::from_utf8(window.stdout).unwrap().trim().to_owned(),
        session_anchor: "reset-other-project-session".into(),
        thread_anchor: "thread-other-project".into(),
    };
    let other_context = fixture.command_at(&other, &other_pane, &["context"]);
    assert!(
        other_context.status.success(),
        "{}",
        output_text(&other_context)
    );
    let other_task = fixture.command_at(
        &other,
        &other_pane,
        &[
            "board",
            "publish",
            "retain-other-history",
            "--title",
            "retain-other-history",
            "--description",
            "preserve",
            "--delivery-condition",
            "keep",
            "--test-condition",
            "this test",
        ],
    );
    assert!(other_task.status.success(), "{}", output_text(&other_task));
    let other_journal = fixture.runtime_journal(&other);
    let other_before = std::fs::read(&other_journal).unwrap();
    let denied = fixture.command_at(&other, &other_pane, &["reset", "--project"]);
    assert!(
        !denied.status.success(),
        "a peer must not reset project assets"
    );
    assert_eq!(std::fs::read(&other_journal).unwrap(), other_before);
    let pid_before = std::fs::read(fixture.host.join("server.pid")).unwrap();

    let resident = fixture.runtime_journal(&fixture.root);
    let reset = fixture.command_with_pane(&pane, &["reset", "--project"]);
    assert!(reset.status.success(), "{}", output_text(&reset));
    let reset: Value = serde_json::from_slice(&reset.stdout).unwrap();
    assert_eq!(reset["status"], "complete", "{reset}");
    let proofs: Vec<Value> = std::fs::read_dir(fixture.host.join("context-operations"))
        .unwrap()
        .map(|entry| {
            serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect();
    let removed_scope = std::fs::canonicalize(&fixture.root)
        .unwrap()
        .display()
        .to_string();
    let retained_scope = std::fs::canonicalize(&other).unwrap().display().to_string();
    assert!(!proofs
        .iter()
        .any(|proof| proof["project_scope"] == removed_scope));
    assert!(proofs
        .iter()
        .any(|proof| proof["project_scope"] == retained_scope));
    assert!(!asset.exists());
    assert_eq!(
        std::fs::read(fixture.host.join("server.pid")).unwrap(),
        pid_before
    );
    assert_eq!(std::fs::read(&other_journal).unwrap(), other_before);
    let other_board = fixture.command_at(&other, &other_pane, &["board", "show"]);
    assert!(
        other_board.status.success(),
        "{}",
        output_text(&other_board)
    );
    assert!(String::from_utf8_lossy(&other_board.stdout).contains("retain-other-history"));
    assert!(
        !std::fs::read_to_string(&resident)
            .unwrap_or_default()
            .contains("erase-project-history"),
        "reset must erase resident project history, not only its live projection"
    );
    let restored = fixture.context_with_pane(&pane);
    assert!(restored.status.success(), "{}", output_text(&restored));
    let restored: Value = serde_json::from_slice(&restored.stdout).unwrap();
    assert_eq!(
        restored["result"]["snapshot"]["identity"]["role"], "worker",
        "{restored}"
    );
    assert!(
        restored["result"]["snapshot"]["master"].is_null(),
        "{restored}"
    );
}
