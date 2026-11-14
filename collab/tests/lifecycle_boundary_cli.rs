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
        Self::start_with_env(password, &[], true)
    }

    fn start_with_env(
        password: Option<&str>,
        envs: &[(&str, &str)],
        wait_until_ready: bool,
    ) -> Self {
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

        let binary = std::env::var_os("COLLAB_TEST_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_collab")));
        let mut fixture = Self {
            binary,
            root,
            host,
            home,
            child: None,
            tmux: None,
        };
        fixture.spawn_child_with_env(envs);
        if wait_until_ready {
            fixture.wait_until_alive();
        }
        fixture
    }

    fn spawn_child(&mut self) {
        self.spawn_child_with_env(&[]);
    }

    fn spawn_child_with_env(&mut self, envs: &[(&str, &str)]) {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.host.join("test-daemon.log"))
            .unwrap();
        let mut command = Command::new(&self.binary);
        command
            .arg("serve")
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
            .env_remove("REX_SERVER")
            .env_remove("REX_SESSION")
            .env_remove("REX_BLOCK")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        for (name, value) in envs {
            command.env(name, value);
        }
        self.child = Some(command.spawn().expect("spawn isolated collab serve"));
    }

    fn spawn_daemon_process_with_env(&self, envs: &[(&str, &str)]) -> Child {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.host.join("test-daemon.log"))
            .unwrap();
        let mut command = Command::new(&self.binary);
        command
            .arg("serve")
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
            .env_remove("REX_SERVER")
            .env_remove("REX_SESSION")
            .env_remove("REX_BLOCK")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        for (name, value) in envs {
            command.env(name, value);
        }
        command
            .spawn()
            .expect("spawn isolated collab serve process")
    }

    fn restart_daemon(&mut self) {
        self.spawn_child();
        self.wait_until_alive();
    }

    fn kill_and_reap_daemon(&mut self) {
        let child = self.child.as_mut().expect("fixture daemon is present");
        let pid = child.id();
        assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) }, 0);
        child.wait().expect("reap fixture daemon after SIGKILL");
        self.child = None;
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
            .env_remove("COLLAB_APPSERVER_NAMESPACE")
            .env_remove("REX_SERVER")
            .env_remove("REX_SESSION")
            .env_remove("REX_BLOCK");
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
            .args([
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{session_id}",
                "-s",
                &session,
                "sh -i",
            ])
            .output()
            .expect("start owned tmux session");
        assert!(
            output.status.success(),
            "tmux failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let session_id = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        assert!(session_id.starts_with('$'));
        self.tmux = Some((socket.clone(), session_id));
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
            "cd {root}\nunset COLLAB_SOCKET_PATH COLLAB_HOST_SOCKET COLLAB_LOCK_PATH COLLAB_HOST_LOCK COLLAB_APPSERVER_SOCKET CODEX_APP_SERVER_SOCKET COLLAB_APPSERVER_NAMESPACE REX_SERVER REX_SESSION REX_BLOCK\nexport HOME={home} CODEX_HOME={home} COLLAB_STATE_DIR={host} TMUX_PANE={pane} CODEX_SESSION_ID={session} CODEX_THREAD_ID={thread}\n{binary} {command} >{stdout} 2>{stderr}\nprintf '%s' \"$?\" >{status}\n",
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

    fn runtime_state_path(&self, project: &Path) -> PathBuf {
        let canonical = std::fs::canonicalize(project).unwrap();
        let routes: Vec<Value> =
            serde_json::from_slice(&std::fs::read(self.host.join("routes.json")).unwrap()).unwrap();
        let matched: Vec<_> = routes
            .iter()
            .filter(|record| {
                record["project_scope"].as_str() == canonical.to_str()
                    && record["app_scope_id"] == "appserver-cli"
            })
            .collect();
        assert_eq!(matched.len(), 1, "one exact current project/app route");
        PathBuf::from(matched[0]["storage_root"].as_str().unwrap())
            .join(".agent-collab/server/state.json")
    }

    fn central_runtime_state_path(&self) -> PathBuf {
        fn collect_state_files(path: &Path, found: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(path) else {
                return;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    collect_state_files(&path, found);
                } else if path.file_name().is_some_and(|name| name == "state.json")
                    && path
                        .parent()
                        .and_then(Path::parent)
                        .and_then(Path::file_name)
                        .is_some_and(|name| name == ".agent-collab")
                {
                    found.push(path);
                }
            }
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut found = Vec::new();
            collect_state_files(&self.host.join("projects"), &mut found);
            if found.len() == 1 {
                return found.pop().unwrap();
            }
            assert!(
                found.is_empty(),
                "expected at most one central runtime current state, found {found:?}"
            );
            assert!(
                Instant::now() < deadline,
                "central runtime current state did not appear"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn runtime_state(&self, project: &Path) -> Value {
        let document: Value =
            serde_json::from_slice(&std::fs::read(self.runtime_state_path(project)).unwrap())
                .unwrap();
        assert_eq!(document["schema_version"], 1);
        document["state"].clone()
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
        .env_remove("REX_SERVER")
        .env_remove("REX_SESSION")
        .env_remove("REX_BLOCK")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start pty down command");
    let mut stdout = child.stdout.take().unwrap();
    let mut prefix = Vec::new();
    let mut prompted = false;
    loop {
        if prefix.ends_with(b"Password: ") {
            prompted = true;
            break;
        }
        let mut byte = [0];
        if stdout.read(&mut byte).unwrap() == 0 {
            break;
        }
        prefix.push(byte[0]);
    }
    // Only feed a password when the daemon actually prompted. With no
    // configured password `down` never prompts, so the process may already be
    // gone and writing to its stdin would fail.
    if prompted {
        let mut stdin = child.stdin.take().unwrap();
        let _ = stdin.write_all(input);
        let _ = stdin.flush();
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

#[cfg(target_os = "macos")]
fn activation_public_up(fail_bootstrap: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = IsolatedDaemon::start(None);
    let bin = fixture.root.join("fake-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = fixture.root.join("activation.log");
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    let bootout = if fail_bootstrap {
        format!("kill -TERM {}", fixture.child.as_ref().unwrap().id())
    } else {
        ":".into()
    };
    let bootstrap = if fail_bootstrap {
        "echo 'fake bootstrap failure' >&2; exit 7"
    } else {
        ":"
    };
    let launchctl = bin.join("launchctl");
    std::fs::write(&launchctl, format!(
        "#!/bin/sh\necho \"$*\" >> {log}\ncase \"$1\" in\nlist) echo '\"PID\" = 123;' ;;\nbootout) {bootout} ;;\nprint) echo \"Could not find service \\\"${{2##*/}}\\\" in domain for user gui: 501\" >&2; exit 113 ;;\nbootstrap) {bootstrap} ;;\nesac\n",
        log = quote(&log.to_string_lossy())
    )).unwrap();
    let tmux = bin.join("tmux");
    std::fs::write(
        &tmux,
        format!(
            "#!/bin/sh\necho unexpected-tmux >> {}\nexit 1\n",
            quote(&log.to_string_lossy())
        ),
    )
    .unwrap();
    for path in [&launchctl, &tmux] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let script = fixture.root.join("up.sh");
    let status = fixture.root.join("up.exit");
    std::fs::write(
        &script,
        format!(
            "{} up\nprintf '%s' \"$?\" > {}\n",
            quote(&fixture.binary.to_string_lossy()),
            quote(&status.to_string_lossy())
        ),
    )
    .unwrap();
    let output = Command::new("/usr/bin/script")
        .arg("-q")
        .arg("/dev/null")
        .arg("/bin/sh")
        .arg(&script)
        .current_dir(&fixture.root)
        .env_clear()
        .env("HOME", &fixture.home)
        .env("CODEX_HOME", &fixture.home)
        .env("COLLAB_STATE_DIR", &fixture.host)
        .env(
            "COLLAB_AUTOSTART_LABEL",
            format!("com.appsdk.collab.e2e.activation.{}", std::process::id()),
        )
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let code: i32 = std::fs::read_to_string(status).unwrap().parse().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if fail_bootstrap {
        assert_ne!(
            code, 0,
            "failed reload must not use prior liveness as success: {text}"
        );
        assert!(text.contains("fake bootstrap failure"), "{text}");
        assert!(!text.contains("\"ok\": true"), "{text}");
    } else {
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("\"supervised\": true"), "{text}");
        fixture.wait_until_alive();
    }
    let calls = std::fs::read_to_string(log).unwrap();
    assert!(!calls.contains("unexpected-tmux"), "{calls}");
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("bootstrap "))
            .count(),
        1,
        "{calls}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn activation_public_up_does_not_report_success_after_losing_old_daemon() {
    activation_public_up(true);
}

#[cfg(target_os = "macos")]
#[test]
fn activation_public_up_requires_current_daemon_after_reload() {
    activation_public_up(false);
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
fn unauthenticated_shutdown_is_allowed_when_no_password_is_configured() {
    // With no `[maintenance] password` set, shutdown requires no credentials.
    // Each request stops the daemon, so give every case its own isolated child.
    for request in [
        json!({"op":"Shutdown","operator":true}),
        json!({"op":"Shutdown"}),
        json!({"op":"Shutdown","password":"wrong-password"}),
    ] {
        let mut fixture = IsolatedDaemon::start(None);
        let event_path = fixture.host.join("events.jsonl");
        let before_events = std::fs::read(&event_path).unwrap_or_default();
        let response = wire_request(&fixture.socket(), &fixture.root, request.clone());
        assert_eq!(response["ok"], true, "{request} -> {response}");
        assert!(
            fixture.host.join("DOWN").exists(),
            "{request} must stop the daemon"
        );
        assert_eq!(
            std::fs::read(&event_path).unwrap_or_default(),
            before_events,
            "shutdown must not emit a lifecycle event"
        );
        fixture.wait_for_exit();
    }
}

#[test]
fn shutdown_with_wrong_password_is_rejected_when_a_password_is_configured() {
    let mut fixture = IsolatedDaemon::start(Some("lifecycle-secret"));
    let pid = std::fs::read(fixture.host.join("server.pid")).unwrap();
    for request in [
        json!({"op":"Shutdown"}),
        json!({"op":"Shutdown","password":"wrong-password"}),
    ] {
        let response = wire_request(&fixture.socket(), &fixture.root, request.clone());
        assert_eq!(response["ok"], false, "{request} -> {response}");
        assert!(!fixture.host.join("DOWN").exists());
        assert_eq!(std::fs::read(fixture.host.join("server.pid")).unwrap(), pid);
        ping(&fixture.socket()).expect("daemon remains alive after rejected shutdown");
    }
    let response = wire_request(
        &fixture.socket(),
        &fixture.root,
        json!({"op":"Shutdown","password":"lifecycle-secret"}),
    );
    assert_eq!(response["ok"], true, "{response}");
    assert!(fixture.host.join("DOWN").exists());
    fixture.wait_for_exit();
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
    assert!(!fixture.log_text().contains(CUSTOM));
}

#[test]
fn pty_down_without_password_stops_the_owned_daemon_without_prompting() {
    let mut fixture = IsolatedDaemon::start(None);
    // No `[maintenance] password` is configured, so `down` must never prompt.
    // Feeding bytes would only reach the child if a prompt appeared; run_pty
    // tolerates the absent prompt and treats the process exit as the outcome.
    let output = run_pty(&fixture, b"\n");
    assert!(output.status.success(), "{}", output_text(&output));
    fixture.wait_for_exit();
    assert!(fixture.host.join("DOWN").is_file());
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn pty_eof_and_cancel_do_not_stop_or_mutate_the_daemon() {
    for input in [b"\x04".to_vec(), b"\x03".to_vec()] {
        // A configured password makes `down` prompt, so EOF / Ctrl-C cancels.
        // With no password there is no prompt and nothing to cancel.
        let fixture = IsolatedDaemon::start(Some("admin"));
        let before_pid = std::fs::read(fixture.host.join("server.pid")).unwrap();
        let output = run_pty(&fixture, &input);
        assert!(!output.status.success(), "{}", output_text(&output));
        assert!(!fixture.host.join("DOWN").exists());
        assert_eq!(
            std::fs::read(fixture.host.join("server.pid")).unwrap(),
            before_pid
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
    let before_state = fixture.runtime_state(&fixture.root);
    let before_map = std::fs::read(fixture.host.join("identity-map.json")).unwrap();
    let before_routes = std::fs::read(fixture.host.join("routes.json")).unwrap();
    let credential_path = fixture
        .host
        .join("identities")
        .join(&worker)
        .join("identity.json");
    let credential = std::fs::read(&credential_path).unwrap();
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
        std::fs::read(fixture.host.join("identity-map.json")).unwrap(),
        before_map
    );
    assert_eq!(
        std::fs::read(fixture.host.join("routes.json")).unwrap(),
        before_routes
    );
    assert_eq!(std::fs::read(credential_path).unwrap(), credential);
    assert_eq!(
        fixture.runtime_state(&fixture.root)["workers"],
        before_state["workers"]
    );
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
    let before = fixture.runtime_state(&other);
    let map_before: Value =
        serde_json::from_slice(&std::fs::read(fixture.host.join("identity-map.json")).unwrap())
            .unwrap();
    let request = json!({"op":"UnregisterSelf", "worker_id":own_id,
        "token":credential["token"], "facts":{"tmux":{
            "endpoint":other_binding["tmux_endpoint"], "cwd":other_binding["project_scope"]}}});
    let _response = wire_request(&fixture.socket(), &fixture.root, request);
    let after = fixture.runtime_state(&other);
    for field in ["workers", "tasks", "board_details", "task_assignments"] {
        assert!(
            after[field] == before[field],
            "foreign self request must preserve current {field}"
        );
    }
    let map_after: Value =
        serde_json::from_slice(&std::fs::read(fixture.host.join("identity-map.json")).unwrap())
            .unwrap();
    let scope = other.canonicalize().unwrap().to_string_lossy().into_owned();
    let foreign_slots = |map: &Value| {
        map["slots"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|slot| slot["identity_key"]["project_scope"] == scope)
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        foreign_slots(&map_after),
        foreign_slots(&map_before),
        "a valid self request may retire itself but cannot change the foreign identity"
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
    let before = fixture.runtime_state(&fixture.root);
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
    let after = fixture.runtime_state(&fixture.root);
    for field in ["tasks", "board_details", "task_lifecycle"] {
        assert_eq!(
            after[field]["unfinished-self-work"], before[field]["unfinished-self-work"],
            "self removal must retain current {field}"
        );
    }
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
    let other_state_path = fixture.runtime_state_path(&other);
    let other_before = fixture.runtime_state(&other);
    let denied = fixture.command_at(&other, &other_pane, &["reset", "--project"]);
    assert!(
        !denied.status.success(),
        "a peer must not reset project assets"
    );
    let after_denial = fixture.runtime_state(&other);
    for field in [
        "workers",
        "tasks",
        "board_details",
        "task_assignments",
        "pending_merges",
    ] {
        assert!(
            after_denial[field] == other_before[field],
            "denied reset must preserve current {field}"
        );
    }
    let pid_before = std::fs::read(fixture.host.join("server.pid")).unwrap();

    let resident = fixture.runtime_state_path(&fixture.root);
    let map_path = fixture.host.join("identity-map.json");
    let before_map: Value = serde_json::from_slice(&std::fs::read(&map_path).unwrap()).unwrap();
    let reset = fixture.command_with_pane(&pane, &["reset", "--project"]);
    assert!(reset.status.success(), "{}", output_text(&reset));
    let reset: Value = serde_json::from_slice(&reset.stdout).unwrap();
    assert_eq!(reset["status"], "complete", "{reset}");
    let after_map: Value = serde_json::from_slice(&std::fs::read(&map_path).unwrap()).unwrap();
    let removed_scope = std::fs::canonicalize(&fixture.root)
        .unwrap()
        .display()
        .to_string();
    let retained_scope = std::fs::canonicalize(&other).unwrap().display().to_string();
    assert!(!after_map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|slot| slot["identity_key"]["project_scope"] == removed_scope));
    let other_slots = |map: &Value| {
        map["slots"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|slot| slot["identity_key"]["project_scope"] == retained_scope)
            .cloned()
            .collect::<Vec<_>>()
    };
    assert!(!other_slots(&before_map).is_empty());
    assert_eq!(other_slots(&after_map), other_slots(&before_map));
    assert!(!asset.exists());
    assert_eq!(
        std::fs::read(fixture.host.join("server.pid")).unwrap(),
        pid_before
    );
    let after_reset = fixture.runtime_state(&other);
    for field in [
        "workers",
        "tasks",
        "board_details",
        "task_assignments",
        "pending_merges",
    ] {
        assert!(
            after_reset[field] == other_before[field],
            "scoped reset must preserve foreign {field}"
        );
    }
    assert!(other_state_path.exists());
    let other_board = fixture.command_at(&other, &other_pane, &["board", "show"]);
    assert!(
        other_board.status.success(),
        "{}",
        output_text(&other_board)
    );
    assert!(String::from_utf8_lossy(&other_board.stdout).contains("retain-other-history"));
    match std::fs::read(&resident) {
        Ok(bytes) => {
            let document: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(
                document["state"]["tasks"]["erase-project-history"].is_null(),
                "reset must erase the scoped current task"
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("read reset current state: {error}"),
    }
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

/// Run the user maintenance entry `collab up` through a pseudo terminal, with
/// the fixture's isolated state root. The isolated root has no login
/// supervisor, so `up` uses its unsupervised start path and never calls
/// launchctl or tmux.
fn up_in_terminal(fixture: &IsolatedDaemon) -> Output {
    let mut terminal = Command::new("script");
    terminal.arg("-q");
    if cfg!(target_os = "linux") {
        let executable = fixture.binary.to_string_lossy().replace('\'', "'\\''");
        terminal.args(["-e", "-c", &format!("exec '{executable}' up"), "/dev/null"]);
    } else {
        terminal.arg("/dev/null").arg(&fixture.binary).arg("up");
    }
    terminal
        .current_dir(&fixture.root)
        .env("HOME", &fixture.home)
        .env("CODEX_HOME", &fixture.home)
        .env("COLLAB_STATE_DIR", &fixture.host)
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("COLLAB_SOCKET_PATH")
        .env_remove("COLLAB_HOST_SOCKET")
        .env_remove("COLLAB_AUTOSTART_LABEL")
        .stdin(Stdio::null())
        .output()
        .expect("run collab up in a terminal")
}

fn recorded_pid(fixture: &IsolatedDaemon) -> u32 {
    std::fs::read_to_string(fixture.host.join("server.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

/// Freeze the fixture-owned daemon: the kernel still accepts connections on
/// its listening socket, but no request is ever answered. This is the
/// "process alive, socket unresponsive" state.
fn freeze_fixture_daemon(fixture: &IsolatedDaemon) -> u32 {
    let pid = fixture.child.as_ref().unwrap().id();
    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGSTOP) }, 0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while ping(&fixture.socket()).is_ok() {
        assert!(Instant::now() < deadline, "frozen daemon still answers");
        thread::sleep(Duration::from_millis(25));
    }
    assert!(
        UnixStream::connect(fixture.socket()).is_ok(),
        "frozen daemon socket must still accept connections"
    );
    pid
}

fn terminate_owned_child(child: &mut Child) {
    if child.try_wait().unwrap().is_none() {
        let pid = child.id() as libc::pid_t;
        let _ = unsafe { libc::kill(pid, libc::SIGCONT) };
        let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
        let _ = child.wait();
    }
}

/// Stop a daemon that `up` launched detached in the isolated state root.
fn shutdown_isolated_daemon(socket: &Path) {
    if let Ok(mut stream) = UnixStream::connect(socket) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.write_all(b"{\"op\":\"Shutdown\",\"password\":null}\n");
        let mut line = String::new();
        let _ = BufReader::new(stream).read_line(&mut line);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while UnixStream::connect(socket).is_ok() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(feature = "lifecycle-test-hooks")]
struct LifecycleHook {
    listener: UnixListener,
    socket: PathBuf,
}

#[cfg(feature = "lifecycle-test-hooks")]
impl LifecycleHook {
    fn new(label: &str) -> Self {
        let socket = unique_root(label).with_extension("sock");
        let listener = UnixListener::bind(&socket).expect("bind lifecycle test hook");
        listener
            .set_nonblocking(true)
            .expect("set lifecycle test hook listener nonblocking");
        Self { listener, socket }
    }

    fn socket(&self) -> &Path {
        &self.socket
    }

    fn wait(&self, expected: &str) -> UnixStream {
        let deadline = Instant::now() + Duration::from_secs(20);
        let (stream, _) = loop {
            match self.listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for lifecycle hook {expected}"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => panic!("receive lifecycle hook {expected}: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), expected, "unexpected lifecycle hook point");
        stream
    }

    fn resume(&self, stream: &mut UnixStream) {
        stream.write_all(b"resume\n").unwrap();
        stream.flush().unwrap();
    }
}

#[cfg(feature = "lifecycle-test-hooks")]
impl Drop for LifecycleHook {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

#[cfg(feature = "lifecycle-test-hooks")]
fn up_in_terminal_with_hook(fixture: &IsolatedDaemon, hook: &LifecycleHook) -> Child {
    let mut terminal = Command::new("script");
    terminal.arg("-q");
    if cfg!(target_os = "linux") {
        let executable = fixture.binary.to_string_lossy().replace('\'', "'\\''");
        terminal.args(["-e", "-c", &format!("exec '{executable}' up"), "/dev/null"]);
    } else {
        terminal.arg("/dev/null").arg(&fixture.binary).arg("up");
    }
    terminal
        .current_dir(&fixture.root)
        .env("HOME", &fixture.home)
        .env("CODEX_HOME", &fixture.home)
        .env("COLLAB_STATE_DIR", &fixture.host)
        .env("COLLAB_LIFECYCLE_TEST_HOOK", hook.socket())
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("COLLAB_SOCKET_PATH")
        .env_remove("COLLAB_HOST_SOCKET")
        .env_remove("COLLAB_AUTOSTART_LABEL")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run collab up with lifecycle hook")
}

#[cfg(feature = "lifecycle-test-hooks")]
fn hook_env(hook: &LifecycleHook) -> (&'static str, String) {
    (
        "COLLAB_LIFECYCLE_TEST_HOOK",
        hook.socket().to_string_lossy().into_owned(),
    )
}

#[cfg(feature = "lifecycle-test-hooks")]
fn resume_spawned_daemon_publication(hook: &LifecycleHook) {
    for expected in [
        "owner-admitted-before-publication",
        "owner-publication-partial",
    ] {
        let mut point = hook.wait(expected);
        hook.resume(&mut point);
    }
}

#[cfg(feature = "lifecycle-test-hooks")]
fn transfer_lock_to_successor(fixture: &mut IsolatedDaemon) -> u32 {
    fixture.kill_and_reap_daemon();
    fixture.restart_daemon();
    let successor = recorded_pid(fixture);
    assert!(
        ping(&fixture.socket()).is_ok(),
        "successor must answer Ping"
    );
    successor
}

#[cfg(feature = "lifecycle-test-hooks")]
fn assert_refused_and_successor_alive(output: Output, text: &str, successor: u32) -> String {
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("DAEMON_REPLACE_REFUSED"), "{text}");
    assert!(!text.contains("\"ok\": true"), "{text}");
    assert!(successor > 1, "successor PID must identify a real process");
    text.to_owned()
}

#[cfg(feature = "lifecycle-test-hooks")]
fn assert_successor_publication_refused(fixture: &IsolatedDaemon, label: &str) {
    static ATTEMPT: AtomicU64 = AtomicU64::new(0);
    let log_path = fixture.root.join(format!(
        "successor-attempt-{label}-{}-{}",
        std::process::id(),
        ATTEMPT.fetch_add(1, Ordering::Relaxed)
    ));
    let log = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&log_path)
        .unwrap();
    let mut command = fixture.base_command("serve");
    command
        .env_remove("COLLAB_LIFECYCLE_TEST_HOOK")
        .env_remove("COLLAB_LIFECYCLE_RELEASE_ADMISSION")
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    let mut child = command.spawn().expect("attempt successor admission");
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            terminate_owned_child(&mut child);
            panic!("successor admission attempt did not finish: {label}");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let text = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        !status.success(),
        "successor admission must be refused: {text}"
    );
    assert!(
        text.contains("DAEMON_OWNER_PUBLICATION_REFUSED"),
        "successor must fail at owner publication before admission: {text}"
    );
}

#[test]
fn up_replaces_a_live_daemon_whose_socket_does_not_answer() {
    let mut fixture = IsolatedDaemon::start(None);
    let frozen = freeze_fixture_daemon(&fixture);
    assert_eq!(recorded_pid(&fixture), frozen);

    let output = up_in_terminal(&fixture);
    let text = output_text(&output);
    let exited = fixture.child.as_mut().unwrap().try_wait().unwrap();
    let replacement_pid = std::fs::read_to_string(fixture.host.join("server.pid"))
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok());
    let reachable = ping(&fixture.socket()).is_ok();
    if reachable {
        shutdown_isolated_daemon(&fixture.socket());
    }
    assert!(output.status.success(), "up must recover service: {text}");
    assert!(!text.contains("DAEMON_UNKNOWN"), "{text}");
    assert!(
        text.contains(&format!("\"pid\": {frozen}")) && text.contains("\"signal\": \"SIG"),
        "up must report the replaced recorded daemon: {text}"
    );
    assert!(
        exited.is_some(),
        "the unresponsive daemon must be replaced: {text}"
    );
    assert!(reachable, "replacement daemon must answer Ping: {text}");
    assert!(
        replacement_pid.is_some_and(|pid| pid != frozen),
        "a new daemon must own the recorded PID: {text}"
    );
}

#[cfg(feature = "lifecycle-test-hooks")]
#[test]
fn up_refuses_owner_publication_while_record_is_partial() {
    let hook = LifecycleHook::new("partial-owner-publication");
    let (hook_name, hook_path) = hook_env(&hook);
    let mut fixture =
        IsolatedDaemon::start_with_env(None, &[(hook_name, hook_path.as_str())], false);
    let mut point = hook.wait("owner-admitted-before-publication");
    hook.resume(&mut point);
    let mut point = hook.wait("owner-publication-partial");
    let daemon_pid = fixture.child.as_ref().unwrap().id();
    let stale_socket = UnixListener::bind(fixture.socket()).unwrap();
    drop(stale_socket);
    std::fs::write(fixture.host.join("server.pid"), daemon_pid.to_string()).unwrap();

    let output = up_in_terminal(&fixture);
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("DAEMON_OWNER_PUBLICATION_REFUSED"),
        "partial owner publication must be refused at the publication lock: {text}"
    );
    assert!(
        fixture
            .child
            .as_mut()
            .unwrap()
            .try_wait()
            .unwrap()
            .is_none(),
        "a partial proof must never authorize a signal"
    );
    hook.resume(&mut point);
    fixture.wait_until_alive();
    fixture.kill_and_reap_daemon();
}

#[cfg(feature = "lifecycle-test-hooks")]
#[test]
fn up_replaces_after_owner_publication_completes() {
    let hook = LifecycleHook::new("complete-owner-publication");
    let (hook_name, hook_path) = hook_env(&hook);
    let mut fixture =
        IsolatedDaemon::start_with_env(None, &[(hook_name, hook_path.as_str())], false);
    let mut point = hook.wait("owner-admitted-before-publication");
    hook.resume(&mut point);
    let mut point = hook.wait("owner-publication-partial");
    let daemon_pid = fixture.child.as_ref().unwrap().id();
    let stale_socket = UnixListener::bind(fixture.socket()).unwrap();
    drop(stale_socket);
    std::fs::write(fixture.host.join("server.pid"), daemon_pid.to_string()).unwrap();
    hook.resume(&mut point);
    fixture.wait_until_alive();
    let frozen = freeze_fixture_daemon(&fixture);

    let output = up_in_terminal(&fixture);
    let text = output_text(&output);
    let reachable = ping(&fixture.socket()).is_ok();
    if reachable {
        shutdown_isolated_daemon(&fixture.socket());
    }
    assert!(output.status.success(), "{text}");
    assert!(
        text.contains(&format!("\"pid\": {frozen}")),
        "completed owner publication must remain replaceable: {text}"
    );
    assert!(reachable, "replacement must answer current Ping: {text}");
    fixture.wait_for_exit();
}

#[cfg(feature = "lifecycle-test-hooks")]
#[test]
fn up_never_accepts_predecessor_proof_after_successor_admission() {
    let mut fixture =
        IsolatedDaemon::start_with_env(None, &[("COLLAB_LIFECYCLE_RELEASE_ADMISSION", "1")], true);
    freeze_fixture_daemon(&fixture);
    let successor_hook = LifecycleHook::new("successor-after-admission");
    let (hook_name, hook_path) = hook_env(&successor_hook);
    let mut successor = fixture.spawn_daemon_process_with_env(&[(hook_name, hook_path.as_str())]);
    let _point = successor_hook.wait("owner-admitted-before-publication");

    let output = up_in_terminal(&fixture);
    let text = output_text(&output);
    let predecessor_alive = fixture
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none();
    let successor_alive = successor.try_wait().unwrap().is_none();
    terminate_owned_child(fixture.child.as_mut().unwrap());
    terminate_owned_child(&mut successor);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("DAEMON_OWNER_PUBLICATION_REFUSED"),
        "successor admission must be excluded by owner publication: {text}"
    );
    assert!(
        predecessor_alive,
        "a predecessor proof must not signal its retained process after successor admission: {text}"
    );
    assert!(
        successor_alive,
        "the successor admission owner must remain alive: {text}"
    );
}

#[test]
fn up_refuses_unsupported_owner_publication_protocol() {
    let mut fixture = IsolatedDaemon::start(None);
    let daemon_pid = freeze_fixture_daemon(&fixture);
    let record_path = fixture.host.join("daemon.lock");
    let current = std::fs::read_to_string(&record_path).unwrap();
    let fields = current.split_whitespace().collect::<Vec<_>>();
    assert_eq!(fields.first(), Some(&"collab-daemon-v3"));
    let start_id = fields[3];
    let records = [
        format!("collab-daemon {daemon_pid} {start_id}\n"),
        current.replacen("collab-daemon-v3 ", "collab-daemon-v2 ", 1),
    ];
    for record in records {
        std::fs::write(&record_path, record).unwrap();
        let output = up_in_terminal(&fixture);
        let text = output_text(&output);
        assert!(!output.status.success(), "{text}");
        assert!(
            text.contains("unsupported owner publication protocol"),
            "old owner proof must be refused as unsupported: {text}"
        );
        assert!(
            fixture
                .child
                .as_mut()
                .unwrap()
                .try_wait()
                .unwrap()
                .is_none(),
            "unsupported evidence must not signal collab PID {daemon_pid}: {text}"
        );
    }
    terminate_owned_child(fixture.child.as_mut().unwrap());
}

#[test]
fn up_keeps_a_healthy_daemon() {
    let mut fixture = IsolatedDaemon::start(None);
    let before = recorded_pid(&fixture);
    let output = up_in_terminal(&fixture);
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    assert!(text.contains("\"replaced\": null"), "{text}");
    assert!(fixture
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
    assert_eq!(recorded_pid(&fixture), before);
    assert!(ping(&fixture.socket()).is_ok());
}

#[test]
fn up_never_signals_a_pid_that_is_not_the_recorded_collab_daemon() {
    let mut fixture = IsolatedDaemon::start(None);
    freeze_fixture_daemon(&fixture);
    let mut bystander = Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn owned bystander process");
    std::fs::write(fixture.host.join("server.pid"), bystander.id().to_string()).unwrap();

    let output = up_in_terminal(&fixture);
    let text = output_text(&output);
    let bystander_alive = bystander.try_wait().unwrap().is_none();
    let daemon_alive = fixture
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none();
    let _ = bystander.kill();
    let _ = bystander.wait();
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("DAEMON_REPLACE_REFUSED"), "{text}");
    assert!(bystander_alive, "a non-collab PID must never be signalled");
    assert!(
        daemon_alive,
        "an unverified PID must not cause any replacement"
    );
}

#[test]
fn up_never_signals_a_live_collab_pid_that_does_not_own_the_lock() {
    let mut fixture = IsolatedDaemon::start(None);
    freeze_fixture_daemon(&fixture);
    let mut stale = fixture
        .base_command("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn a second collab process");
    assert_eq!(
        unsafe { libc::kill(stale.id() as libc::pid_t, libc::SIGSTOP) },
        0
    );
    std::fs::write(fixture.host.join("server.pid"), stale.id().to_string()).unwrap();

    let output = up_in_terminal(&fixture);
    let text = output_text(&output);
    let stale_alive = stale.try_wait().unwrap().is_none();
    let daemon_alive = fixture
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none();
    let _ = unsafe { libc::kill(stale.id() as libc::pid_t, libc::SIGKILL) };
    let _ = stale.wait();
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("DAEMON_REPLACE_REFUSED"), "{text}");
    assert!(
        stale_alive,
        "a live collab PID that does not own daemon.lock must not be signalled"
    );
    assert!(
        daemon_alive,
        "the recorded lock owner must remain untouched"
    );
}

#[test]
fn up_never_signals_a_live_collab_pid_with_a_stale_lock_start_identity() {
    let mut fixture = IsolatedDaemon::start(None);
    let daemon_pid = freeze_fixture_daemon(&fixture);
    std::fs::write(
        fixture.host.join("daemon.lock"),
        format!("collab-daemon {daemon_pid} forged-start-identity\n"),
    )
    .unwrap();

    let output = up_in_terminal(&fixture);
    let text = output_text(&output);
    let daemon_alive = fixture
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none();
    let _ = unsafe { libc::kill(daemon_pid as libc::pid_t, libc::SIGKILL) };
    let _ = fixture.child.as_mut().unwrap().wait();
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("DAEMON_REPLACE_REFUSED"), "{text}");
    assert!(
        daemon_alive,
        "a daemon whose lock start identity is stale must not be signalled"
    );
}

#[test]
fn up_refuses_an_empty_legacy_lock_without_signalling_the_live_owner() {
    let mut fixture = IsolatedDaemon::start(None);
    let daemon_pid = freeze_fixture_daemon(&fixture);
    let lock = OpenOptions::new()
        .write(true)
        .open(fixture.host.join("daemon.lock"))
        .unwrap();
    lock.set_len(0).unwrap();

    let output = up_in_terminal(&fixture);
    let text = output_text(&output);
    let daemon_alive = fixture
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none();
    let _ = unsafe { libc::kill(daemon_pid as libc::pid_t, libc::SIGKILL) };
    let _ = fixture.child.as_mut().unwrap().wait();
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("DAEMON_REPLACE_REFUSED"), "{text}");
    assert!(
        daemon_alive,
        "an empty legacy owner record must not be signalled"
    );
}

#[cfg(feature = "lifecycle-test-hooks")]
#[test]
fn up_refuses_generation_change_before_sigterm() {
    let mut fixture = IsolatedDaemon::start(None);
    freeze_fixture_daemon(&fixture);
    let hook = LifecycleHook::new("owner-before-sigterm");
    let up = up_in_terminal_with_hook(&fixture, &hook);
    let mut point = hook.wait("sigterm-before-owner");
    let successor = transfer_lock_to_successor(&mut fixture);
    hook.resume(&mut point);

    let output = up.wait_with_output().unwrap();
    let text = output_text(&output);
    assert_refused_and_successor_alive(output, &text, successor);
    assert!(text.contains("SIGTERM"), "{text}");
    assert!(
        ping(&fixture.socket()).is_ok(),
        "successor must remain responsive"
    );
}

#[cfg(feature = "lifecycle-test-hooks")]
#[test]
fn up_prevents_successor_admission_between_proof_and_sigterm() {
    let mut fixture = IsolatedDaemon::start(None);
    let frozen = freeze_fixture_daemon(&fixture);
    let hook = LifecycleHook::new("successor-during-sigterm-proof");
    let up = up_in_terminal_with_hook(&fixture, &hook);
    let mut point = hook.wait("sigterm-before-owner");
    hook.resume(&mut point);
    let mut point = hook.wait("sigterm-before-delivery");
    assert_successor_publication_refused(&fixture, "sigterm-delivery");
    assert_eq!(
        unsafe { libc::kill(frozen as libc::pid_t, libc::SIGCONT) },
        0,
        "resume the exact retained fixture before its SIGTERM"
    );
    hook.resume(&mut point);
    resume_spawned_daemon_publication(&hook);

    let output = up.wait_with_output().unwrap();
    let text = output_text(&output);
    let reachable = ping(&fixture.socket()).is_ok();
    if reachable {
        shutdown_isolated_daemon(&fixture.socket());
    }
    assert!(output.status.success(), "{text}");
    assert!(
        text.contains(&format!("\"pid\": {frozen}")) && text.contains("\"signal\": \"SIGTERM\""),
        "replacement must report the retained SIGTERM target: {text}"
    );
    fixture.wait_for_exit();
    assert!(
        reachable,
        "replacement after guarded SIGTERM must answer Ping: {text}"
    );
}

#[cfg(feature = "lifecycle-test-hooks")]
#[test]
fn up_prevents_successor_admission_between_proof_and_sigkill() {
    let mut fixture = IsolatedDaemon::start(None);
    let frozen = freeze_fixture_daemon(&fixture);
    let hook = LifecycleHook::new("successor-during-sigkill-proof");
    let up = up_in_terminal_with_hook(&fixture, &hook);
    let mut point = hook.wait("sigterm-before-owner");
    hook.resume(&mut point);
    let mut point = hook.wait("sigterm-before-delivery");
    hook.resume(&mut point);
    let mut point = hook.wait("sigkill-before-owner");
    hook.resume(&mut point);
    let mut point = hook.wait("sigkill-before-delivery");
    assert_successor_publication_refused(&fixture, "sigkill-delivery");
    hook.resume(&mut point);
    resume_spawned_daemon_publication(&hook);

    let output = up.wait_with_output().unwrap();
    let text = output_text(&output);
    let reachable = ping(&fixture.socket()).is_ok();
    if reachable {
        shutdown_isolated_daemon(&fixture.socket());
    }
    assert!(output.status.success(), "{text}");
    assert!(
        text.contains(&format!("\"pid\": {frozen}")) && text.contains("\"signal\": \"SIGKILL\""),
        "replacement must retain the target through SIGKILL: {text}"
    );
    fixture.wait_for_exit();
    assert!(
        reachable,
        "replacement after guarded SIGKILL must answer Ping: {text}"
    );
}

#[cfg(feature = "lifecycle-test-hooks")]
#[test]
fn up_refuses_sigterm_when_the_proved_instance_disappears() {
    let mut fixture = IsolatedDaemon::start(None);
    freeze_fixture_daemon(&fixture);
    let hook = LifecycleHook::new("gone-after-sigterm-proof");
    let up = up_in_terminal_with_hook(&fixture, &hook);
    let mut point = hook.wait("sigterm-before-owner");
    hook.resume(&mut point);
    let mut point = hook.wait("sigterm-before-delivery");
    fixture.kill_and_reap_daemon();
    hook.resume(&mut point);

    let output = up.wait_with_output().unwrap();
    let text = output_text(&output);
    fixture.restart_daemon();
    let successor = recorded_pid(&fixture);
    assert_refused_and_successor_alive(output, &text, successor);
    assert!(
        text.contains("SIGTERM target disappeared before delivery"),
        "{text}"
    );
    assert!(text.contains("no signal was delivered"), "{text}");
    assert!(!text.contains("SIGKILL target disappeared"), "{text}");
    assert!(
        ping(&fixture.socket()).is_ok(),
        "successor must remain responsive"
    );
}

#[cfg(feature = "lifecycle-test-hooks")]
#[test]
fn up_refuses_sigkill_when_the_lock_owner_changed_after_sigterm() {
    let mut fixture = IsolatedDaemon::start(None);
    freeze_fixture_daemon(&fixture);
    let hook = LifecycleHook::new("owner-before-sigkill");
    let up = up_in_terminal_with_hook(&fixture, &hook);
    let mut point = hook.wait("sigterm-before-owner");
    hook.resume(&mut point);
    let mut point = hook.wait("sigterm-before-delivery");
    hook.resume(&mut point);
    let mut point = hook.wait("sigkill-before-owner");
    let successor = transfer_lock_to_successor(&mut fixture);
    hook.resume(&mut point);

    let output = up.wait_with_output().unwrap();
    let text = output_text(&output);
    assert_refused_and_successor_alive(output, &text, successor);
    assert!(text.contains("SIGKILL"), "{text}");
    assert!(text.contains("SIGTERM was delivered"), "{text}");
    assert!(!text.contains("no process was signalled"), "{text}");
    assert!(
        ping(&fixture.socket()).is_ok(),
        "successor must remain responsive"
    );
}

#[cfg(feature = "lifecycle-test-hooks")]
#[test]
fn up_refuses_sigkill_when_the_proved_instance_disappears() {
    let mut fixture = IsolatedDaemon::start(None);
    freeze_fixture_daemon(&fixture);
    let hook = LifecycleHook::new("gone-after-sigkill-proof");
    let up = up_in_terminal_with_hook(&fixture, &hook);
    let mut point = hook.wait("sigterm-before-owner");
    hook.resume(&mut point);
    let mut point = hook.wait("sigterm-before-delivery");
    hook.resume(&mut point);
    let mut point = hook.wait("sigkill-before-owner");
    hook.resume(&mut point);
    let mut point = hook.wait("sigkill-before-delivery");
    fixture.kill_and_reap_daemon();
    hook.resume(&mut point);

    let output = up.wait_with_output().unwrap();
    let text = output_text(&output);
    fixture.restart_daemon();
    let successor = recorded_pid(&fixture);
    assert_refused_and_successor_alive(output, &text, successor);
    assert!(
        text.contains("SIGKILL target disappeared before delivery"),
        "{text}"
    );
    assert!(text.contains("SIGTERM was delivered"), "{text}");
    assert!(
        ping(&fixture.socket()).is_ok(),
        "successor must remain responsive"
    );
}

#[cfg(feature = "lifecycle-test-hooks")]
#[test]
fn up_refuses_stale_owner_during_offline_admission() {
    let mut fixture =
        IsolatedDaemon::start_with_env(None, &[("COLLAB_LIFECYCLE_RELEASE_ADMISSION", "1")], true);
    let storage_root = fixture
        .central_runtime_state_path()
        .ancestors()
        .nth(3)
        .unwrap()
        .to_path_buf();
    let daemon_pid = freeze_fixture_daemon(&fixture);
    let hook = LifecycleHook::new("offline-admission");
    let (hook_name, hook_path) = hook_env(&hook);
    let reset_log = fixture.root.join("offline-reset.log");
    let reset_output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&reset_log)
        .unwrap();
    let mut command = fixture.base_command("reset");
    let mut reset = command
        .args([
            "--host",
            "--storage-root",
            storage_root.to_str().unwrap(),
            "--approval",
            "isolated offline admission test",
            "--discard-legacy",
        ])
        .env(hook_name, hook_path)
        .env_remove("COLLAB_LIFECYCLE_RELEASE_ADMISSION")
        .stdin(Stdio::null())
        .stdout(reset_output.try_clone().unwrap())
        .stderr(reset_output)
        .spawn()
        .expect("start offline reset admission fixture");
    let mut point = hook.wait("reset-admission-held");

    let output = up_in_terminal(&fixture);
    let text = output_text(&output);
    assert!(!output.status.success(), "{text}");
    assert!(
        text.contains("DAEMON_OWNER_PUBLICATION_REFUSED"),
        "offline admission must own publication while retaining admission: {text}"
    );
    assert!(
        fixture
            .child
            .as_mut()
            .unwrap()
            .try_wait()
            .unwrap()
            .is_none(),
        "stale daemon proof must not signal the live fixture PID {daemon_pid}: {text}"
    );

    hook.resume(&mut point);
    let reset_status = reset.wait().unwrap();
    let reset_text = std::fs::read_to_string(&reset_log).unwrap_or_default();
    terminate_owned_child(fixture.child.as_mut().unwrap());
    assert!(!reset_status.success(), "{reset_text}");
    assert!(
        reset_text.contains("RESET_DAEMON_LIVE"),
        "reset must reach its existing live-daemon refusal after the barrier: {reset_text}"
    );
}
