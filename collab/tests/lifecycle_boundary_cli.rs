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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct IsolatedDaemon {
    binary: PathBuf,
    root: PathBuf,
    host: PathBuf,
    home: PathBuf,
    child: Option<Child>,
    tmux: Option<(PathBuf, String)>,
}

impl IsolatedDaemon {
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

    fn start(password: Option<&str>) -> Self {
        Self::start_scenario(password, None, true)
    }

    fn start_scenario(
        password: Option<&str>,
        startup_subdir: Option<&str>,
        project_marker: bool,
    ) -> Self {
        Self::start_scenario_mode(password, startup_subdir, project_marker, false)
    }

    fn start_scenario_mode(
        password: Option<&str>,
        startup_subdir: Option<&str>,
        project_marker: bool,
        service: bool,
    ) -> Self {
        Self::start_scenario_with(
            password,
            startup_subdir,
            project_marker,
            service,
            |_, _, _| {},
        )
    }

    fn start_scenario_with(
        password: Option<&str>,
        startup_subdir: Option<&str>,
        project_marker: bool,
        service: bool,
        prepare: impl FnOnce(&Path, &Path, &Path),
    ) -> Self {
        let root = unique_root("lifecycle");
        let host = root.join("host");
        let home = root.join("home");
        let startup = startup_subdir
            .map(|name| root.join(name))
            .unwrap_or_else(|| root.clone());
        if project_marker {
            std::fs::create_dir_all(startup.join(".agent-collab/server")).unwrap();
        } else {
            std::fs::create_dir_all(&startup).unwrap();
        }
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
        prepare(&root, &host, &home);

        let binary = std::env::var_os("COLLAB_TEST_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_collab")));
        let child = spawn_isolated_daemon(&binary, &startup, &host, &home, service);
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

    fn restart(&mut self, startup: &Path) {
        self.child = Some(spawn_isolated_daemon(
            &self.binary,
            startup,
            &self.host,
            &self.home,
            true,
        ));
        self.wait_until_alive();
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

    fn crash(&mut self) {
        let mut child = self.child.take().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
    }

    fn log_text(&self) -> String {
        std::fs::read_to_string(self.host.join("test-daemon.log")).unwrap_or_default()
    }
}

#[test]
fn serve_startup_cwd_does_not_select_a_project() {
    for (name, marker) in [
        ("project-a", true),
        ("project-b", true),
        ("plain-dir", false),
    ] {
        let fixture = IsolatedDaemon::start_scenario(None, Some(name), marker);
        let readiness = ping(&fixture.socket()).expect("host readiness");
        assert_eq!(readiness["host_ready"], true, "{readiness}");
        assert!(readiness.get("workers").is_none(), "{readiness}");
        assert!(readiness.get("messages").is_none(), "{readiness}");
        assert!(readiness.get("tasks").is_none(), "{readiness}");
        assert!(
            !fixture.host.join("projects").exists(),
            "startup cwd selected a project runtime: {readiness}"
        );
    }
}

#[test]
fn foundation_host_ablation_retains_public_up_recovery_of_owned_child() {
    let mut fixture = IsolatedDaemon::start(None);
    let old_pid = fixture.child.as_ref().unwrap().id();
    assert_eq!(
        std::fs::read_to_string(fixture.host.join("server.pid"))
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap(),
        old_pid
    );
    // Only this test's own child is stopped. The listening socket remains
    // open, but the process can no longer answer a request.
    assert_eq!(
        unsafe { libc::kill(old_pid as libc::pid_t, libc::SIGSTOP) },
        0
    );
    assert!(ping(&fixture.socket()).is_err());
    assert!(UnixStream::connect(fixture.socket()).is_ok());

    let mut terminal = Command::new("script");
    terminal.arg("-q");
    if cfg!(target_os = "linux") {
        let executable = fixture.binary.to_string_lossy().replace('\'', "'\\''");
        terminal.args(["-e", "-c", &format!("exec '{executable}' up"), "/dev/null"]);
    } else {
        terminal.arg("/dev/null").arg(&fixture.binary).arg("up");
    }
    let output = terminal
        .current_dir(&fixture.root)
        .env("HOME", &fixture.home)
        .env("CODEX_HOME", &fixture.home)
        .env("COLLAB_STATE_DIR", &fixture.host)
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_INTERNAL_ORIGINATOR_OVERRIDE")
        .env_remove("DSH_SESSION_ID")
        .env_remove("AGY_SESSION_ID")
        .env_remove("REX_SERVER")
        .env_remove("REX_SESSION")
        .env_remove("REX_BLOCK")
        .env_remove("REX_TRANSPORT")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("COLLAB_SOCKET_PATH")
        .env_remove("COLLAB_HOST_SOCKET")
        .env_remove("COLLAB_AUTOSTART_LABEL")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = output_text(&output);
    let reachable = ping(&fixture.socket()).is_ok();
    let replacement_pid = std::fs::read_to_string(fixture.host.join("server.pid"))
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok());
    let exited = fixture
        .child
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_some();
    // The replacement belongs to this isolated state root. Shut it down
    // through its public socket even when a later assertion fails.
    if reachable {
        let shutdown =
            wire_request_with_context(&fixture.socket(), json!({"op":"Shutdown","password":null}));
        assert_eq!(shutdown["ok"], true, "{shutdown}");
        let deadline = Instant::now() + Duration::from_secs(10);
        while fixture.socket().exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
        }
        assert!(
            !fixture.socket().exists(),
            "isolated replacement did not exit"
        );
    }
    assert!(output.status.success(), "public up failed: {text}");
    assert!(!text.contains("DAEMON_UNKNOWN"), "{text}");
    assert!(text.contains(&format!("\"pid\": {old_pid}")), "{text}");
    assert!(text.contains("\"signal\": \"SIG"), "{text}");
    assert!(exited, "owned unresponsive daemon was not replaced: {text}");
    assert!(reachable, "replacement does not answer Ping: {text}");
    assert!(replacement_pid.is_some_and(|pid| pid != old_pid), "{text}");
}

#[test]
fn host_registration_rows_do_not_create_or_schedule_project_routes() {
    let fixture = IsolatedDaemon::start_scenario_with(
        None,
        Some("plain-dir"),
        false,
        false,
        |root, host, _| {
            let project = root.join("project-without-route");
            std::fs::create_dir_all(project.join(".agent-collab/server")).unwrap();
            let scope = project
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            std::fs::write(
                host.join("host-state.json"),
                serde_json::to_vec(&json!({
                    "schema_version": 1,
                    "state": {
                        "master_wake": {
                            "generation": 1, "goal_due": true,
                            "scheduling_decision_revision": 0,
                            "first_pending_ms": 0, "last_updated_ms": 0
                        },
                        "global": {
                            "epoch": 1, "sequence": 0, "revision": 0,
                            "projects": {(scope.clone()): {
                                "project_scope": scope,
                                "registrations": {"appserver-cli": {
                                    "project_scope": scope,
                                    "app_scope_id": "appserver-cli",
                                    "registered_at_ms": 17
                                }}
                            }},
                            "current_thread_routes": [], "legacy_thread_routes": [],
                            "current_thread_route_tombstones": {}, "command_receipts": {}
                        }
                    }
                }))
                .unwrap(),
            )
            .unwrap();
        },
    );
    assert_eq!(ping(&fixture.socket()).unwrap()["host_ready"], true);
    let project = fixture.root.join("project-without-route");
    let before = std::fs::read(fixture.host.join("host-state.json")).unwrap();
    let host_state: Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(host_state["state"]["master_wake"]["goal_due"], true);
    let status = fixture
        .base_command("status")
        .arg("--all")
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(!status.status.success(), "{}", output_text(&status));
    assert!(
        output_text(&status).contains("PROJECT_SCOPE_UNKNOWN"),
        "{}",
        output_text(&status)
    );
    assert!(!fixture.host.join("routes.json").exists());
    assert!(!fixture.host.join("projects").exists());
    assert!(!project.join(".agent-collab/server/state.json").exists());
    assert_eq!(
        std::fs::read(fixture.host.join("host-state.json")).unwrap(),
        before
    );
    assert_eq!(ping(&fixture.socket()).unwrap()["host_ready"], true);
}

#[test]
fn supervised_service_startup_does_not_select_a_project() {
    let fixture = IsolatedDaemon::start_scenario_mode(None, Some("project-a"), true, true);
    let readiness = ping(&fixture.socket()).expect("host readiness");
    assert_eq!(readiness["host_ready"], true, "{readiness}");
    assert!(readiness.get("workers").is_none(), "{readiness}");
    assert!(!fixture.host.join("projects").exists(), "{readiness}");
    assert!(!fixture.host.join("host-control.json").exists());
    assert!(!fixture.host.join("host-state.json").exists());
    assert!(!fixture.host.join("service-record.json").exists());
}

#[test]
fn host_dispatch_cross_project_mail_stays_in_selected_runtime() {
    let mut fixture = IsolatedDaemon::start_scenario_with(
        None,
        Some("neutral-cwd"),
        false,
        false,
        |root, host, _| {
            std::fs::write(
                host.join("host-state.json"),
                serde_json::to_vec(&json!({"schema_version":1,"state":{}})).unwrap(),
            )
            .unwrap();
            let mut routes = Vec::new();
            let mut slots = Vec::new();
            for name in ["source", "target"] {
                let project = root.join(name);
                std::fs::create_dir_all(project.join(".agent-collab/server")).unwrap();
                let project = project.canonicalize().unwrap();
                let worker = format!("{name}-worker");
                let binding = json!({"project_scope":project,"app_scope_id":"appserver-cli",
                    "agent_id":worker,"runtime_id":format!("{name}-runtime"),
                    "binding_id":format!("{name}-binding"),"session_id":format!("{name}-session"),
                    "native_thread_id":format!("{name}-thread")});
                let transport = json!({"kind":"appserver","endpoint":"unix:///tmp/unused-dispatch-fixture.sock",
                    "namespace":"codex_tui","session_id":format!("{name}-session"),
                    "thread_id":format!("{name}-thread"),"capabilities":[],"self_check":"static non-tmux fixture"});
                std::fs::write(project.join(".agent-collab/server/state.json"),
                    serde_json::to_vec(&json!({"schema_version":1,"state":{
                        "workers":{(worker.clone()):{"id":worker,"token":format!("{name}-token"),
                            "cwd":project,"registered_ms":1,"transport":transport}},
                        "name_occupancies":{(project.to_string_lossy().into_owned()):{
                            "Alice":{"name":"Alice","worker_id":worker,
                                "session_id":format!("{name}-session"),"occupancy_revision":1,"updated_ms":1}}},
                        "global":{"epoch":1,"sequence":0,"revision":0,"projects":{
                            (project.to_string_lossy().into_owned()):{"project_scope":project,
                                "registrations":{"appserver-cli":{"project_scope":project,"app_scope_id":"appserver-cli"}},
                                "runtime_bindings":{(format!("{name}-binding")):binding}}
                        }}
                    }})).unwrap()).unwrap();
                routes.push(json!({"project_scope":project,"canonical_root":project,
                    "app_scope_id":"appserver-cli","storage_root":project,"registered_ms":1}));
                slots.push(json!({"identity_key":{"project_scope":project,"app_scope_id":"appserver-cli","name":"Alice"},
                    "display_slot":2,"identity_status":"active","generation":1,"previous":null,
                    "current":{"binding":binding,"transport":transport,
                        "tab_id":{"project_scope":project,"app_scope_id":"appserver-cli"},
                        "host":{"host_type":"native","agent_kind":null,"profile":null},
                        "invocation_cwd":project,"physical_name":null}}));
            }
            std::fs::write(
                host.join("routes.json"),
                serde_json::to_vec(&routes).unwrap(),
            )
            .unwrap();
            std::fs::write(
                host.join("identity-map.json"),
                serde_json::to_vec(&json!({"schema_version":1,"slots":slots})).unwrap(),
            )
            .unwrap();
        },
    );
    let source = fixture.root.join("source").canonicalize().unwrap();
    let target = fixture.root.join("target").canonicalize().unwrap();
    let before = std::fs::read(fixture.host.join("host-state.json")).unwrap();
    assert!(!fixture.host.join("host-control.json").exists());
    let bad_grant = wire_request(
        &fixture.socket(),
        &target,
        json!({"op":"Ceo",
        "worker_id":"target-worker","token":"wrong-token",
        "command":{"action":"promote","approval":"isolated host control","workspace":target}}),
    );
    assert_eq!(bad_grant["ok"], false, "{bad_grant}");
    assert!(!fixture.host.join("host-control.json").exists());
    let grant = wire_request(
        &fixture.socket(),
        &target,
        json!({"op":"Ceo",
        "worker_id":"target-worker","token":"target-token",
        "command":{"action":"promote","approval":"isolated host control","workspace":target}}),
    );
    assert_eq!(grant["ok"], true, "{grant}");
    assert_eq!(grant["is_current"], true, "{grant}");
    assert!(fixture.host.join("host-control.json").is_file());
    let send = |token: &str, reply: Option<&str>| {
        json!({"op":"CrossProjectSend",
        "from":"forged","from_project":"ignored","worker_id":"source-worker","token":token,
        "command":{"command_id":"public-dispatch-command","operation_id":"public-dispatch-operation",
            "actor_binding_id":"source-binding","scope":{"project_scope_id":source,"app_scope_id":"appserver-cli"}},
        "to":"target-worker","subject":"public dispatch","body":"durable target mail","in_reply_to":reply})
    };
    let rejected = wire_request(&fixture.socket(), &target, send("wrong-token", None));
    assert_eq!(rejected["ok"], false, "{rejected}");
    assert!(rejected["error"]
        .as_str()
        .unwrap()
        .contains("token mismatch"));
    let sent = wire_request(&fixture.socket(), &target, send("source-token", None));
    assert_eq!(sent["ok"], true, "{sent}");
    assert_eq!(sent["consumed"], false, "{sent}");
    let id = sent["msg_id"].as_str().unwrap();
    let target_state = fixture.runtime_state(&target);
    assert_eq!(target_state["msgs"][id]["body"], "durable target mail");
    assert_eq!(
        target_state["msgs"][id]["from"],
        format!("source-worker@{}", source.display())
    );
    assert!(fixture.runtime_state(&source)["msgs"][id].is_null());
    let poll = json!({"op":"Poll", "worker_id":"target-worker","token":"target-token",
        "timeout_ms":0,"receive_id":"dispatch-consume", "project_context":{
            "project_scope":target,"canonical_root":target,"app_scope_id":"appserver-cli",
            "runtime_context":{"agent_id":"target-worker","runtime_id":"target-runtime",
                "appserver_id":"appserver-cli","binding_id":"target-binding",
                "session_id":"target-session","native_thread_id":"target-thread"}}});
    let received = wire_request_with_context(&fixture.socket(), poll.clone());
    assert_eq!(received["ok"], true, "{received}");
    assert!(received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["id"] == id));
    let replayed = wire_request_with_context(&fixture.socket(), poll);
    assert_eq!(replayed["messages"], received["messages"]);
    let reply = wire_request(
        &fixture.socket(),
        &source,
        json!({"op":"CrossProjectSend",
        "from":"ignored","from_project":"ignored","worker_id":"target-worker","token":"target-token",
        "command":{"command_id":"public-reply-command","operation_id":"public-reply-operation",
            "actor_binding_id":"target-binding","scope":{"project_scope_id":target,"app_scope_id":"appserver-cli"}},
        "to":"source-worker","subject":"reply","body":"linked reply","in_reply_to":id}),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let reply_id = reply["msg_id"].as_str().unwrap();
    assert_eq!(
        fixture.runtime_state(&source)["msgs"][reply_id]["in_reply_to"],
        id
    );
    assert!(fixture.runtime_state(&target)["msgs"][reply_id].is_null());
    for project in [&source, &target] {
        let status = fixture
            .base_command("status")
            .arg("--all")
            .current_dir(project)
            .output()
            .unwrap();
        assert!(status.status.success(), "{}", output_text(&status));
        let state: Value = serde_json::from_slice(&status.stdout).unwrap();
        assert_eq!(state["summary"]["workers"], 1, "{state}");
    }
    let status = fixture.run("status");
    assert!(status.status.success(), "{}", output_text(&status));
    let listed = fixture
        .base_command("list")
        .current_dir(fixture.root.join("neutral-cwd"))
        .output()
        .unwrap();
    assert!(listed.status.success(), "{}", output_text(&listed));
    let ceo = fixture.base_command("ceo").arg("status").output().unwrap();
    assert!(ceo.status.success(), "{}", output_text(&ceo));
    let ceo: Value = serde_json::from_slice(&ceo.stdout).unwrap();
    assert_eq!(ceo["ceo"]["approval"], "isolated host control", "{ceo}");
    assert!(fixture.runtime_state(&target)["ceo_grant"].is_null());
    let inbound = json!({"op":"ChannelInbound","from":"external-fixture","to":"",
        "subject":"host CEO channel","body":"[ceo] control route body",
        "envelope":{"source_channel":"qqbot","channel_instance":"qqbot@fixture",
            "external_user":"fixture-user","external_conversation":"fixture-conversation",
            "platform_message_id":"host-control-event","reply_route":"fixture-only",
            "recipient_address":"ceo","attachments":[]}});
    let rejected = wire_request(&fixture.socket(), &source, inbound.clone());
    assert_eq!(rejected["ok"], false, "{rejected}");
    assert!(rejected["error"]
        .as_str()
        .unwrap()
        .contains("CHANNEL_CEO_STALE_WORKSPACE"));
    let delivered = wire_request(&fixture.socket(), &target, inbound.clone());
    // The fixture has no transport server. Mailbox commit and unavailable
    // notification remain separate public results.
    assert_eq!(delivered["ok"], false, "{delivered}");
    assert_eq!(delivered["durable"], true, "{delivered}");
    assert_eq!(
        delivered["failure"], "notification_delivery_failed",
        "{delivered}"
    );
    assert!(delivered["notification_error"]
        .as_str()
        .unwrap()
        .contains("ADAPTER_ROUTE_UNAVAILABLE"));
    let channel_id = delivered["msg_id"].as_str().unwrap();
    let committed = fixture.runtime_state(&target);
    assert_eq!(committed["msgs"][channel_id]["to"], "target-worker");
    assert_eq!(
        committed["msgs"][channel_id]["body"],
        "来源：QQBot\n消息：control route body"
    );
    assert!(fixture.runtime_state(&source)["msgs"][channel_id].is_null());
    let duplicate = wire_request(&fixture.socket(), &target, inbound);
    assert_eq!(duplicate["ok"], true, "{duplicate}");
    assert_eq!(duplicate["msg_id"], channel_id);
    assert_eq!(
        std::fs::read(fixture.host.join("host-state.json")).unwrap(),
        before
    );
    // Opposing public requests must complete without a host-wide state lock.
    let socket = fixture.socket();
    let (forward, reverse) = thread::scope(|scope| {
        let forward = scope.spawn(|| wire_request(&socket, &target, json!({"op":"CrossProjectSend",
            "from":"ignored","from_project":"ignored","worker_id":"source-worker","token":"source-token",
            "command":{"command_id":"concurrent-forward","operation_id":"concurrent-forward-op",
                "actor_binding_id":"source-binding","scope":{"project_scope_id":source,"app_scope_id":"appserver-cli"}},
            "to":"target-worker","subject":"concurrent","body":"forward"})));
        let reverse = scope.spawn(|| wire_request(&socket, &source, json!({"op":"CrossProjectSend",
            "from":"ignored","from_project":"ignored","worker_id":"target-worker","token":"target-token",
            "command":{"command_id":"concurrent-reverse","operation_id":"concurrent-reverse-op",
                "actor_binding_id":"target-binding","scope":{"project_scope_id":target,"app_scope_id":"appserver-cli"}},
            "to":"source-worker","subject":"concurrent","body":"reverse"})));
        (forward.join().unwrap(), reverse.join().unwrap())
    });
    for (response, project, body) in [
        (&forward, &target, "forward"),
        (&reverse, &source, "reverse"),
    ] {
        assert_eq!(response["ok"], true, "{response}");
        assert_eq!(response["consumed"], false, "{response}");
        let id = response["msg_id"].as_str().unwrap();
        assert_eq!(fixture.runtime_state(project)["msgs"][id]["body"], body);
    }
    let clear = wire_request(
        &socket,
        &target,
        json!({"op":"Ceo","worker_id":"target-worker",
        "token":"target-token","command":{"action":"clear","approval":"isolated revoke"}}),
    );
    assert_eq!(clear["ok"], true, "{clear}");
    let actor_request = |project: &Path, name: &str, mut request: Value| {
        request["project_context"] = json!({"project_scope":project,"canonical_root":project,
            "app_scope_id":"appserver-cli","runtime_context":{
                "agent_id":format!("{name}-worker"),"runtime_id":format!("{name}-runtime"),
                "appserver_id":"appserver-cli","binding_id":format!("{name}-binding"),
                "session_id":format!("{name}-session"),"native_thread_id":format!("{name}-thread")}});
        wire_request_with_context(&socket, request)
    };
    let promote = actor_request(
        &source,
        "source",
        json!({"op":"MasterPromote",
        "worker_id":"source-worker","token":"source-token","approval":"isolated reset master"}),
    );
    assert_eq!(promote["ok"], true, "{promote}");
    let denied = actor_request(
        &target,
        "target",
        json!({"op":"ProjectReset",
        "worker_id":"target-worker","token":"target-token"}),
    );
    assert_eq!(denied["ok"], false, "{denied}");
    assert!(denied["error"]
        .as_str()
        .unwrap()
        .contains("RESET_MASTER_REQUIRED"));
    let reset = actor_request(
        &source,
        "source",
        json!({"op":"ProjectReset",
        "worker_id":"source-worker","token":"source-token"}),
    );
    assert_eq!(reset["ok"], true, "{reset}");
    assert_eq!(reset["status"], "complete", "{reset}");
    assert!(!source.join(".agent-collab").exists());
    assert_eq!(
        fixture.runtime_state(&target)["msgs"][channel_id]["body"],
        "来源：QQBot\n消息：control route body"
    );
    std::fs::create_dir_all(source.join(".agent-collab")).unwrap();
    std::fs::write(source.join(".agent-collab/after-reset"), "new data").unwrap();
    fixture.crash();
    fixture.restart(&fixture.root.join("neutral-cwd"));
    assert_eq!(
        std::fs::read_to_string(source.join(".agent-collab/after-reset")).unwrap(),
        "new data"
    );
    let control: Value =
        serde_json::from_slice(&std::fs::read(fixture.host.join("host-control.json")).unwrap())
            .unwrap();
    assert!(control["state"]["ceo_grant"].is_null());
    assert_eq!(control["state"]["pending_project_resets"], json!({}));
}

#[test]
fn current_map_session_routes_without_host_address_publication() {
    let fixture = IsolatedDaemon::start_scenario_with(
        None,
        Some("plain-dir"),
        false,
        false,
        |root, host, _| {
            std::fs::write(
                host.join("host-state.json"),
                serde_json::to_vec(&json!({"schema_version":1,"state":{}})).unwrap(),
            )
            .unwrap();
            let project = root.join("mapped-project");
            std::fs::create_dir_all(project.join(".agent-collab/server")).unwrap();
            let project = project.canonicalize().unwrap();
            let binding = json!({
                "project_scope": project, "app_scope_id": "appserver-cli",
                "agent_id": "mapped-worker", "runtime_id": "mapped-runtime",
                "binding_id": "mapped-binding", "session_id": "mapped-session",
                "native_thread_id": "mapped-thread"
            });
            let transport = json!({
                "kind":"appserver", "endpoint":"unix:///tmp/unused-map-fixture.sock",
                "namespace":"codex_tui", "session_id":"mapped-session",
                "thread_id":"mapped-thread", "capabilities":[], "self_check":"committed fixture"
            });
            std::fs::write(project.join(".agent-collab/server/state.json"),
                serde_json::to_vec(&json!({"schema_version":1,"state":{
                    "workers":{"mapped-worker":{"id":"mapped-worker","token":"fixture-token",
                        "cwd":project,"registered_ms":1,"transport":transport}},
                    "global":{"epoch":1,"sequence":0,"revision":0,"projects":{
                        (project.to_string_lossy().into_owned()):{
                            "project_scope":project,
                            "registrations":{"appserver-cli":{"project_scope":project,"app_scope_id":"appserver-cli"}},
                            "runtime_bindings":{"mapped-binding":binding}
                        }
                    }},
                    "target_sessions":{"unread-mail-marker":"unchanged"}
                }})).unwrap()).unwrap();
            std::fs::write(
                host.join("routes.json"),
                serde_json::to_vec(&json!([{
                    "project_scope":project,"canonical_root":project,"app_scope_id":"appserver-cli",
                    "storage_root":project,"registered_ms":1
                }]))
                .unwrap(),
            )
            .unwrap();
            std::fs::write(host.join("identity-map.json"), serde_json::to_vec(&json!({
                "schema_version":1,"slots":[{
                    "identity_key":{"project_scope":project,"app_scope_id":"appserver-cli","name":"Alice"},
                    "display_slot":2,"identity_status":"active","generation":1,"previous":null,
                    "current":{"binding":binding,"transport":transport,
                        "tab_id":{"project_scope":project,"app_scope_id":"appserver-cli"},
                        "host":{"host_type":"native","agent_kind":null,"profile":null},
                        "invocation_cwd":project,"physical_name":null}
                }]
            })).unwrap()).unwrap();
        },
    );
    let project = fixture.root.join("mapped-project");
    let legacy_path = fixture.host.join("host-state.json");
    let before = std::fs::read(&legacy_path).unwrap();
    let resolved = wire_request(
        &fixture.socket(),
        &project,
        json!({"op":"RouteResolveNative","session_id":"mapped-session"}),
    );
    assert_eq!(resolved["ok"], true, "{resolved}");
    assert_eq!(resolved["agent_id"], "mapped-worker", "{resolved}");
    let missing = wire_request(
        &fixture.socket(),
        &project,
        json!({"op":"RouteResolveNative","session_id":"old-session"}),
    );
    assert_eq!(missing["ok"], false, "{missing}");
    assert!(missing["error"]
        .as_str()
        .unwrap()
        .starts_with("ROUTE_RESOLVE_NOT_FOUND"));
    let status = fixture
        .base_command("status")
        .arg("--all")
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(status.status.success(), "{}", output_text(&status));
    assert_eq!(
        std::fs::read(&legacy_path).unwrap(),
        before,
        "startup and public route reads must not publish a legacy host address index"
    );
    let state: Value = serde_json::from_slice(
        &std::fs::read(project.join(".agent-collab/server/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        state["state"]["target_sessions"]["unread-mail-marker"],
        "unchanged"
    );
}

#[test]
fn host_control_reset_handoff_finishes_before_public_readiness() {
    let fixture = IsolatedDaemon::start_scenario_with(
        None,
        Some("plain-dir"),
        false,
        false,
        |root, host, _| {
            let project = root.join("reset-project");
            let foreign = root.join("foreign-project/.agent-collab/mailbox");
            std::fs::create_dir_all(project.join(".agent-collab/mailbox")).unwrap();
            std::fs::create_dir_all(&foreign).unwrap();
            std::fs::write(project.join(".agent-collab/mailbox/remove"), "owned").unwrap();
            std::fs::write(foreign.join("keep"), "foreign unread mail").unwrap();
            let scope = project
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            std::fs::write(
                host.join("host-state.json"),
                serde_json::to_vec(&json!({
                    "schema_version": 1, "state": {"pending_project_resets": {(scope.clone()): true}}
                }))
                .unwrap(),
            )
            .unwrap();
            std::fs::write(
                host.join("host-control.json"),
                serde_json::to_vec(&json!({
                    "schema_version":1,"state":{"pending_project_resets":{(scope):true},
                        "ceo_grant":null,"ceo_bootstraps":{},"ceo_report_state":{},
                        "commands":{"epoch":1,"sequence":0,"revision":0,"receipts":{}}}
                }))
                .unwrap(),
            )
            .unwrap();
        },
    );
    assert!(!fixture.root.join("reset-project/.agent-collab").exists());
    assert_eq!(
        std::fs::read_to_string(
            fixture
                .root
                .join("foreign-project/.agent-collab/mailbox/keep")
        )
        .unwrap(),
        "foreign unread mail"
    );
    let control: Value =
        serde_json::from_slice(&std::fs::read(fixture.host.join("host-control.json")).unwrap())
            .unwrap();
    assert_eq!(control["state"]["pending_project_resets"], json!({}));
    assert!(control["state"].get("workers").is_none());
    assert!(control["state"].get("global").is_none());
    let legacy: Value =
        serde_json::from_slice(&std::fs::read(fixture.host.join("host-state.json")).unwrap())
            .unwrap();
    assert_eq!(
        legacy["state"]["pending_project_resets"],
        json!({
            (fixture.root.join("reset-project").canonicalize().unwrap().to_string_lossy().into_owned()): true
        })
    );
    assert!(control["state"].get("reset_owner_initialized").is_none());
    assert!(control["state"].get("legacy_migration_completed").is_none());
    let status = fixture.run("status");
    assert!(status.status.success(), "{}", output_text(&status));
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["host_ready"], true);
}

#[test]
fn legacy_project_state_is_preserved_without_startup_projection() {
    let fixture = IsolatedDaemon::start_scenario_with(
        None,
        Some("plain-dir"),
        false,
        false,
        |root, host, _| {
            let project = root.join("legacy-project");
            let server = project.join(".agent-collab/server");
            std::fs::create_dir_all(&server).unwrap();
            std::fs::write(
                server.join("state.json"),
                serde_json::to_vec(&json!({
                    "schema_version": 1,
                    "state": {
                        "pending_project_resets": {"reset-marker": true},
                        "ceo_grant": {
                            "workspace": project.to_string_lossy(),
                            "app_scope_id": "appserver-cli",
                            "name": "CEO",
                            "granted_by": "user",
                            "approval": "legacy-approval",
                            "granted_ms": 1
                        },
                        "ceo_report_state": {
                            "notice_gate": {"next_due_ms": 42}
                        },
                        "target_sessions": {"mail-marker": "target"}
                    }
                }))
                .unwrap(),
            )
            .unwrap();
            std::fs::write(
                host.join("service-record.json"),
                serde_json::to_vec(&json!({
                    "service_scope_root": project,
                }))
                .unwrap(),
            )
            .unwrap();
        },
    );

    let readiness = ping(&fixture.socket()).expect("host readiness");
    assert_eq!(readiness["host_ready"], true, "{readiness}");
    assert!(!fixture.host.join("host-control.json").exists());
    let status = fixture.base_command("ceo").arg("status").output().unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert!(status["ceo"].is_null(), "{status}");
    assert_eq!(status["is_current"], false, "{status}");
    assert!(!fixture.host.join("host-state.json").exists());

    let project = fixture.root.join("legacy-project");
    let legacy: Value = serde_json::from_slice(
        &std::fs::read(project.join(".agent-collab/server/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(legacy["state"]["ceo_grant"]["approval"], "legacy-approval");
    assert_eq!(
        legacy["state"]["pending_project_resets"]["reset-marker"],
        true
    );
    assert_eq!(
        legacy["state"]["ceo_report_state"]["notice_gate"]["next_due_ms"],
        42
    );
    assert_eq!(legacy["state"]["target_sessions"]["mail-marker"], "target");
    assert!(!fixture.host.join("routes.json").exists());
    let record: Value =
        serde_json::from_slice(&std::fs::read(fixture.host.join("service-record.json")).unwrap())
            .unwrap();
    assert_eq!(record["service_scope_root"], json!(project), "{record}");
}

#[test]
fn typed_owner_restarts_without_reimporting_conflicting_legacy_authority() {
    let mut fixture = IsolatedDaemon::start_scenario_with(
        None,
        Some("plain-dir"),
        false,
        false,
        |root, host, _| {
            let project = root.join("legacy-project");
            let server = project.join(".agent-collab/server");
            std::fs::create_dir_all(&server).unwrap();
            std::fs::write(
                server.join("state.json"),
                serde_json::to_vec(&json!({
                    "schema_version": 1,
                    "state": {
                        "ceo_grant": {
                            "workspace": project.to_string_lossy(),
                            "app_scope_id": "appserver-cli",
                            "name": "CEO",
                            "granted_by": "user",
                            "approval": "source-approval",
                            "granted_ms": 1
                        },
                        "target_sessions": {"mail-marker": "target"}
                    }
                }))
                .unwrap(),
            )
            .unwrap();
            std::fs::write(
                host.join("host-state.json"),
                serde_json::to_vec(&json!({
                    "schema_version": 1,
                    "state": {
                        "ceo_grant": {
                            "workspace": project.to_string_lossy(),
                            "app_scope_id": "appserver-cli",
                            "name": "CEO",
                            "granted_by": "user",
                            "approval": "host-approval",
                            "granted_ms": 2
                        },
                        "legacy_migration_completed":true,
                        "ceo_bootstraps":{},"ceo_report_state":{},
                        "commands":{"epoch":1,"sequence":0,"revision":0,"receipts":{}}
                    }
                }))
                .unwrap(),
            )
            .unwrap();
            std::fs::copy(host.join("host-state.json"), host.join("host-control.json")).unwrap();
            std::fs::write(
                host.join("service-record.json"),
                serde_json::to_vec(&json!({
                    "service_scope_root": project,
                }))
                .unwrap(),
            )
            .unwrap();
        },
    );
    let plain = fixture.root.join("plain-dir");
    let project = fixture.root.join("legacy-project");
    let legacy_host_before = std::fs::read(fixture.host.join("host-state.json")).unwrap();
    let legacy_host: Value = serde_json::from_slice(&legacy_host_before).unwrap();
    assert_eq!(
        legacy_host["state"]["ceo_grant"]["approval"],
        "host-approval"
    );

    let host_state: Value =
        serde_json::from_slice(&std::fs::read(fixture.host.join("host-control.json")).unwrap())
            .unwrap();
    assert_eq!(
        host_state["state"]["ceo_grant"]["approval"],
        "host-approval"
    );
    assert!(!fixture.host.join("routes.json").exists());
    let storage_root = project.clone();
    let project_state = || -> Value {
        serde_json::from_slice(
            &std::fs::read(storage_root.join(".agent-collab/server/state.json")).unwrap(),
        )
        .unwrap()
    };
    assert_eq!(
        project_state()["state"]["target_sessions"]["mail-marker"],
        "target"
    );

    fixture.crash();
    // Reopening a fenced host input would now fail decoding. A stale service
    // record must not cause historical project discovery after completion.
    std::fs::write(fixture.host.join("host-state.json"), b"retired legacy host").unwrap();
    std::fs::write(
        fixture.host.join("service-record.json"),
        serde_json::to_vec(&json!({"service_scope_root": fixture.root.join("missing")})).unwrap(),
    )
    .unwrap();
    fixture.restart(&plain);

    let host_state: Value =
        serde_json::from_slice(&std::fs::read(fixture.host.join("host-control.json")).unwrap())
            .unwrap();
    assert_eq!(
        host_state["state"]["ceo_grant"]["approval"],
        "host-approval"
    );
    assert_eq!(
        project_state()["state"]["target_sessions"]["mail-marker"],
        "target"
    );
    let legacy: Value = serde_json::from_slice(
        &std::fs::read(project.join(".agent-collab/server/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(legacy["state"]["ceo_grant"]["approval"], "source-approval");
    assert!(!legacy_host_before.is_empty());
    assert_eq!(
        std::fs::read(fixture.host.join("host-state.json")).unwrap(),
        b"retired legacy host"
    );
    assert_eq!(host_state["state"]["legacy_migration_completed"], true);
}

#[test]
fn fixed_routes_preserve_two_project_mailboxes_without_project_ledger_import() {
    let message_created_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .min(i64::MAX as u128) as i64;
    let fixture = IsolatedDaemon::start_scenario_with(
        None,
        Some("plain-dir"),
        false,
        false,
        |root, host, _| {
            let project_a = root.join("project-a");
            let project_b = root.join("project-b");
            for project in [&project_a, &project_b] {
                std::fs::create_dir_all(project.join(".agent-collab/server")).unwrap();
            }
            let project_a = std::fs::canonicalize(project_a).unwrap();
            let project_b = std::fs::canonicalize(project_b).unwrap();
            let scope_a = project_a.to_string_lossy().into_owned();
            let scope_b = project_b.to_string_lossy().into_owned();
            let registration = |scope: &str, registered_ms: i64| {
                json!({
                    "project_scope": scope,
                    "app_scope_id": "appserver-cli",
                    "registered_at_ms": registered_ms,
                })
            };
            let project_state = |scope: &str, task_id: &str, message_id: &str, body: &str| {
                json!({
                    "schema_version": 1,
                    "state": {
                        "workers": {
                            "legacy-peer": {
                                "id": "legacy-peer",
                                "token": "legacy-token",
                                "cwd": scope,
                                "registered_ms": 1
                            }
                        },
                        "tasks": {
                            (task_id): {
                                "id": task_id,
                                "owner": "legacy-peer",
                                "created_by": "user",
                                "status": "working",
                                "created_ms": 1,
                                "updated_ms": 2
                            }
                        },
                        "msgs": {
                            (message_id): {
                                "id": message_id,
                                "from": "Alice",
                                "to": "legacy-peer",
                                "type": "notify",
                                "body": body,
                                "created_ms": message_created_ms,
                                "state": "pending"
                            }
                        },
                        "global": {
                            "epoch": 1,
                            "sequence": 0,
                            "revision": 0,
                            "projects": {
                                (scope): {
                                    "project_scope": scope,
                                    "registrations": {
                                        "appserver-cli": registration(scope, 11)
                                    }
                                }
                            },
                            "current_thread_routes": [],
                            "legacy_thread_routes": [],
                            "current_thread_route_tombstones": {},
                            "command_receipts": {}
                        }
                    }
                })
            };

            let mut legacy_a = project_state(&scope_a, "task-a", "mail-a", "mailbox-a");
            std::fs::write(host.join("routes.json"), serde_json::to_vec(&json!([
                {"project_scope":scope_a,"canonical_root":project_a,"app_scope_id":"appserver-cli",
                    "storage_root":project_a,"registered_ms":11},
                {"project_scope":scope_b,"canonical_root":project_b,"app_scope_id":"appserver-cli",
                    "storage_root":project_b,"registered_ms":22}
            ])).unwrap()).unwrap();
            legacy_a["state"]["sequence"] = json!(8);
            legacy_a["state"]["revision"] = json!(6);
            legacy_a["state"]["global"]["epoch"] = json!(7);
            legacy_a["state"]["global"]["sequence"] = json!(8);
            legacy_a["state"]["global"]["revision"] = json!(6);
            legacy_a["state"]["global"]["command_receipts"] = json!({
                "legacy-command": {
                    "command_id": "legacy-command",
                    "operation_id": "legacy-operation",
                    "epoch": 7,
                    "sequence": 8,
                    "revision": 6,
                    "outcome": {"project": scope_b, "accepted": true}
                }
            });
            legacy_a["state"]["global"]["projects"][&scope_b] = json!({
                "project_scope": scope_b,
                "registrations": {
                    "appserver-cli": registration(&scope_b, 22)
                }
            });
            std::fs::write(
                project_a.join(".agent-collab/server/state.json"),
                serde_json::to_vec(&legacy_a).unwrap(),
            )
            .unwrap();
            std::fs::write(
                project_b.join(".agent-collab/server/state.json"),
                serde_json::to_vec(&project_state(&scope_b, "task-b", "mail-b", "mailbox-b"))
                    .unwrap(),
            )
            .unwrap();
            std::fs::write(
                host.join("service-record.json"),
                serde_json::to_vec(&json!({"service_scope_root": project_a})).unwrap(),
            )
            .unwrap();
        },
    );
    let project_a = fixture.root.join("project-a");
    let project_b = fixture.root.join("project-b");

    assert!(!fixture.host.join("host-state.json").exists());
    assert!(!fixture.host.join("host-control.json").exists());
    let original: Value = serde_json::from_slice(
        &std::fs::read(project_a.join(".agent-collab/server/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(original["state"]["global"]["epoch"], 7);
    assert_eq!(
        original["state"]["global"]["command_receipts"]["legacy-command"],
        json!({
            "command_id": "legacy-command", "operation_id": "legacy-operation",
            "epoch": 7, "sequence": 8, "revision": 6,
            "outcome": {"project": std::fs::canonicalize(&project_b).unwrap(), "accepted": true}
        })
    );

    let routes: Vec<Value> =
        serde_json::from_slice(&std::fs::read(fixture.host.join("routes.json")).unwrap()).unwrap();
    assert_eq!(routes.len(), 2, "{routes:?}");
    for project in [&project_a, &project_b] {
        let scope = std::fs::canonicalize(project)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let matched: Vec<_> = routes
            .iter()
            .filter(|route| route["project_scope"] == scope)
            .collect();
        assert_eq!(matched.len(), 1, "route for {scope}: {routes:?}");
        let registered_ms = if project == &project_a { 11 } else { 22 };
        assert_eq!(matched[0]["registered_ms"], registered_ms, "{routes:?}");
        assert_eq!(
            std::fs::canonicalize(matched[0]["storage_root"].as_str().unwrap()).unwrap(),
            std::fs::canonicalize(project).unwrap(),
            "{routes:?}"
        );
    }

    for (project, task_id, message_id, body) in [
        (&project_a, "task-a", "mail-a", "mailbox-a"),
        (&project_b, "task-b", "mail-b", "mailbox-b"),
    ] {
        let status = fixture
            .base_command("status")
            .arg("--all")
            .current_dir(project)
            .output()
            .unwrap();
        assert!(status.status.success(), "{}", output_text(&status));
        let status: Value = serde_json::from_slice(&status.stdout).unwrap();
        assert!(
            status["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|task| task["id"] == task_id),
            "{status}"
        );

        let mailbox = fixture
            .base_command("mailbox")
            .args(["read", "--all"])
            .current_dir(project)
            .output()
            .unwrap();
        assert!(mailbox.status.success(), "{}", output_text(&mailbox));
        let mailbox: Value = serde_json::from_slice(&mailbox.stdout).unwrap();
        assert!(
            mailbox["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["id"] == message_id && message["body"] == body),
            "{mailbox}"
        );
    }
}

#[test]
fn conflicting_fixed_storage_owners_fail_before_state_replace() {
    let root = unique_root("ambiguous-storage");
    let host = root.join("host");
    let home = root.join("home");
    let startup = root.join("plain-dir");
    std::fs::create_dir_all(&host).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&startup).unwrap();
    let project_a = root.join("project-a");
    let project_b = root.join("project-b");
    for project in [&project_a, &project_b] {
        std::fs::create_dir_all(project.join(".agent-collab/server")).unwrap();
    }
    let project_a = std::fs::canonicalize(project_a).unwrap();
    let project_b = std::fs::canonicalize(project_b).unwrap();
    let scope_a = project_a.to_string_lossy().into_owned();
    let scope_b = project_b.to_string_lossy().into_owned();
    let registration = |scope: &str, registered_ms: i64| {
        json!({
            "project_scope": scope,
            "app_scope_id": "appserver-cli",
            "registered_at_ms": registered_ms,
        })
    };
    let legacy = json!({
        "schema_version": 1,
        "state": {
            "global": {
                "epoch": 1,
                "sequence": 0,
                "revision": 0,
                "projects": {
                    (&scope_a): {
                        "project_scope": scope_a,
                        "registrations": {
                            "appserver-cli": registration(&scope_a, 11)
                        }
                    },
                    (&scope_b): {
                        "project_scope": scope_b,
                        "registrations": {
                            "appserver-cli": registration(&scope_b, 22)
                        }
                    }
                },
                "current_thread_routes": [],
                "legacy_thread_routes": [],
                "current_thread_route_tombstones": {},
                "command_receipts": {}
            }
        }
    });
    std::fs::write(
        project_a.join(".agent-collab/server/state.json"),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    std::fs::write(
        project_b.join(".agent-collab/server/state.json"),
        serde_json::to_vec(&json!({"schema_version": 1, "state": {"revision": 7}})).unwrap(),
    )
    .unwrap();
    let central_b = {
        let projects = encoded_storage_path(&host.join("projects"), &scope_b);
        encoded_storage_path(&projects.join("apps"), "appserver-cli")
    };
    std::fs::create_dir_all(central_b.join(".agent-collab/server")).unwrap();
    std::fs::write(
        central_b.join(".agent-collab/server/state.json"),
        serde_json::to_vec(&json!({"schema_version": 1, "state": {"revision": 9}})).unwrap(),
    )
    .unwrap();
    std::fs::write(
        host.join("service-record.json"),
        serde_json::to_vec(&json!({"service_scope_root": project_a})).unwrap(),
    )
    .unwrap();
    let project_b_before =
        std::fs::read(project_b.join(".agent-collab/server/state.json")).unwrap();
    let central_before = std::fs::read(central_b.join(".agent-collab/server/state.json")).unwrap();
    let routes = serde_json::to_vec(&json!([
        {"project_scope":scope_a,"canonical_root":project_a,"app_scope_id":"appserver-cli",
            "storage_root":project_a,"registered_ms":11},
        {"project_scope":scope_b,"canonical_root":project_b,"app_scope_id":"appserver-cli",
            "storage_root":project_a,"registered_ms":22}
    ]))
    .unwrap();
    std::fs::write(host.join("routes.json"), &routes).unwrap();

    let binary = std::env::var_os("COLLAB_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_collab")));
    let mut child = spawn_isolated_daemon(&binary, &startup, &host, &home, false);
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        match child.try_wait().unwrap() {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!(
                    "conflicting fixed storage owners must fail startup: {}",
                    std::fs::read_to_string(host.join("test-daemon.log")).unwrap_or_default()
                );
            }
            None => thread::sleep(Duration::from_millis(25)),
        }
    };
    assert!(!status.success(), "{status}");
    let log = std::fs::read_to_string(host.join("test-daemon.log")).unwrap();
    assert!(
        log.contains("HOST_ROUTE_INVALID: duplicate runtime storage root"),
        "{log}"
    );
    assert_eq!(std::fs::read(host.join("routes.json")).unwrap(), routes);
    assert_eq!(
        std::fs::read(project_b.join(".agent-collab/server/state.json")).unwrap(),
        project_b_before
    );
    assert_eq!(
        std::fs::read(central_b.join(".agent-collab/server/state.json")).unwrap(),
        central_before
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ordinary_status_reads_the_host_from_a_non_project_directory() {
    let fixture = IsolatedDaemon::start_scenario(None, Some("plain-dir"), false);
    let output = fixture
        .base_command("status")
        .current_dir(fixture.root.join("plain-dir"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["host_ready"], true, "{value}");
    assert!(value.get("workers").is_none(), "{value}");
}

#[test]
fn host_config_is_global_from_project_and_non_project_directories() {
    let fixture = IsolatedDaemon::start_scenario(None, Some("project-a"), true);
    let project_a = fixture.root.join("project-a").canonicalize().unwrap();
    let project_b = fixture.root.join("project-b");
    let plain = fixture.root.join("plain-dir");
    std::fs::create_dir_all(project_b.join(".agent-collab")).unwrap();
    std::fs::create_dir_all(&plain).unwrap();
    let config = fixture.home.join(".appsdk/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        format!(
            "[timers]\ntick_interval_ms = 5000\n\n[[projects]]\nroot = {:?}\n[projects.timers]\ntick_interval_ms = 10000\n",
            project_a.to_string_lossy()
        ),
    )
    .unwrap();

    for cwd in [&project_a, &project_b, &plain] {
        let output = fixture
            .base_command("config")
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", output_text(&output));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["timers"]["tick_interval_ms"], 5000, "{value}");
    }
}

#[test]
fn two_projects_share_one_host_and_keep_their_runtime_state_separate() {
    let mut fixture = IsolatedDaemon::start_scenario(None, Some("project-a"), true);
    let project_a = fixture.root.join("project-a");
    let project_b = fixture.root.join("project-b");
    std::fs::create_dir_all(project_b.join(".agent-collab")).unwrap();

    let pane_a = fixture.start_tmux_pane();
    let context_a = fixture.command_at(&project_a, &pane_a, &["context"]);
    assert!(context_a.status.success(), "{}", output_text(&context_a));
    let context_a: Value = serde_json::from_slice(&context_a.stdout).unwrap();
    let binding_a = context_a["result"]["snapshot"]["binding"].clone();

    let window = tmux(
        &pane_a.socket,
        &["new-window", "-d", "-P", "-F", "#{pane_id}", "sh -i"],
    );
    assert!(window.status.success());
    let pane_b = TmuxPane {
        socket: pane_a.socket.clone(),
        server_pid: pane_a.server_pid,
        pane_id: String::from_utf8(window.stdout).unwrap().trim().to_owned(),
        session_anchor: format!("session-b-{}", std::process::id()),
        thread_anchor: format!("thread-b-{}", std::process::id()),
    };
    let context_b = fixture.command_at(&project_b, &pane_b, &["context"]);
    assert!(context_b.status.success(), "{}", output_text(&context_b));
    let context_b: Value = serde_json::from_slice(&context_b.stdout).unwrap();
    let binding_b = context_b["result"]["snapshot"]["binding"].clone();

    assert_ne!(binding_a["project_scope"], binding_b["project_scope"]);
    assert_ne!(
        fixture.runtime_state_path(&project_a),
        fixture.runtime_state_path(&project_b)
    );
    let host_as_project = wire_request(
        &fixture.socket(),
        &fixture.host,
        json!({"op":"MasterStatus"}),
    );
    assert_eq!(host_as_project["ok"], false, "{host_as_project}");
    assert!(
        host_as_project["error"]
            .as_str()
            .is_some_and(|error| error.starts_with("PROJECT_SCOPE_UNKNOWN")),
        "{host_as_project}"
    );
    for (project, binding) in [(&project_a, &binding_a), (&project_b, &binding_b)] {
        let output = fixture
            .base_command("status")
            .arg("--all")
            .current_dir(project)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", output_text(&output));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["summary"]["workers"], 1, "{value}");
        assert!(
            value["workers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|worker| worker["id"] == binding["agent_id"]),
            "{value}"
        );
    }

    let plain = fixture.root.join("plain-dir");
    std::fs::create_dir_all(&plain).unwrap();
    for cwd in [&project_a, &project_b, &plain] {
        let output = fixture
            .base_command("status")
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", output_text(&output));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["host_ready"], true, "{value}");
        assert!(value.get("workers").is_none(), "{value}");
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

fn encoded_storage_path(base: &Path, value: &str) -> PathBuf {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = value.as_bytes();
    let mut path = base.to_path_buf();
    for (index, chunk) in bytes.chunks(96).enumerate() {
        let prefix = if index == 0 {
            format!("v1-{}-", bytes.len())
        } else {
            String::new()
        };
        let mut component = String::with_capacity(prefix.len() + chunk.len() * 2);
        component.push_str(&prefix);
        for byte in chunk {
            component.push(HEX[(byte >> 4) as usize] as char);
            component.push(HEX[(byte & 0x0f) as usize] as char);
        }
        path.push(component);
    }
    path
}

fn spawn_isolated_daemon(
    binary: &Path,
    startup: &Path,
    host: &Path,
    home: &Path,
    service: bool,
) -> Child {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(host.join("test-daemon.log"))
        .unwrap();
    let mut command = Command::new(binary);
    command.arg("serve");
    if service {
        command.arg("--service");
    }
    command
        .current_dir(startup)
        .env("HOME", home)
        .env("COLLAB_STATE_DIR", host)
        .env("CODEX_HOME", home)
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
        .stderr(log)
        .spawn()
        .expect("spawn isolated collab serve")
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
    wire_request_with_context(socket, Value::Object(envelope))
}

fn wire_request_with_context(socket: &Path, request: Value) -> Value {
    let line = serde_json::to_vec(&request).unwrap();
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

#[test]
fn context_admits_the_invocation_root_before_rex_target_validation() {
    let fixture = IsolatedDaemon::start(None);
    let project_b = unique_root("rex-root-preflight-project-b");
    std::fs::create_dir_all(project_b.join(".agent-collab")).unwrap();
    let provide = json!({
        "session_id": "agent-session-rex-root-preflight",
        "rex": {
            "server": "unix:///tmp/rex-root-preflight-not-contacted.sock",
            "session_id": "session:invalid",
            "block_id": "block:invalid",
            "cwd": project_b.display().to_string(),
        }
    })
    .to_string();
    let output = fixture
        .base_command("context")
        .current_dir(&project_b)
        .args(["--op", "rex-root-preflight-cli", "--provide", &provide])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", output_text(&output));
    let text = output_text(&output);
    assert!(
        !text.contains("does not match project root"),
        "the daemon resident root must not reject the invocation project: {text}"
    );
    assert!(
        text.contains("REX_"),
        "the request must continue to real Rex target validation: {text}"
    );
    let _ = std::fs::remove_dir_all(project_b);
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
    // Host startup no longer materializes a project. This offline admission
    // fixture explicitly owns its index; it must still refuse the live daemon.
    let storage_root = fixture.root.clone();
    std::fs::write(
        storage_root.join(".agent-collab/server/state.json"),
        serde_json::to_vec(&json!({"schema_version": 1, "state": {}})).unwrap(),
    )
    .unwrap();
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
