pub(crate) struct IsolatedTmux {
    socket: PathBuf,
    owner_root: PathBuf,
    pane_command: String,
    sessions: RefCell<Vec<String>>,
    server_pid: RefCell<Option<(u32, String)>>,
    pane_pids: RefCell<Vec<(u32, String)>>,
    cleanup_errors: RefCell<Vec<String>>,
}

const FIXTURE_PANE_READY_TIMEOUT: Duration = Duration::from_secs(5);
const FIXTURE_PANE_READY_POLL: Duration = Duration::from_millis(10);

impl IsolatedTmux {
    pub(crate) fn start(root: &std::path::Path) -> Self {
        Self::start_with_spare_pane(root, true, "sleep 60")
    }

    pub(crate) fn start_single(root: &std::path::Path) -> Self {
        Self::start_with_spare_pane(root, false, "sleep 60")
    }

    pub(super) fn start_online(root: &Path) -> Self {
        let command = crate::client::adapters::tmux::tests::foreground_agent_command(root);
        Self::start_with_spare_pane(root, true, &command)
    }

    pub(super) fn start_with_spare_pane(root: &std::path::Path, spare_pane: bool, pane_command: &str) -> Self {
        static SOCKET_SEQ: AtomicU64 = AtomicU64::new(0);
        let socket = std::env::temp_dir().join(format!(
            "ctmux-{}-{}.sock",
            std::process::id(),
            SOCKET_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let output = with_tmux(|tmux| {
            tmux.args([
                "-S",
                socket.to_str().unwrap(),
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{session_id}|#{pane_id}|#{pane_pid}|#{pid}",
                "-c",
                root.to_str().unwrap(),
                "-s",
                "worker-snapshot-test",
                pane_command,
            ])
            .output()
            .unwrap()
        });
        assert!(
            output.status.success(),
            "start isolated tmux: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let fixture = Self {
            socket: socket.clone(),
            owner_root: root.to_path_buf(),
            pane_command: pane_command.into(),
            sessions: RefCell::new(Vec::new()),
            server_pid: RefCell::new(None),
            pane_pids: RefCell::new(Vec::new()),
            cleanup_errors: RefCell::new(Vec::new()),
        };
        // Record ownership as soon as the first session exists. A later split
        // or readiness check may fail before endpoints() is called.
        fixture.record_allocation(&String::from_utf8(output.stdout).unwrap());
        if spare_pane {
            let split = with_tmux(|tmux| {
                tmux.args([
                    "-S",
                    socket.to_str().unwrap(),
                    "split-window",
                    "-d",
                    "-P",
                    "-F",
                    "#{session_id}|#{pane_id}|#{pane_pid}|#{pid}",
                    "-c",
                    root.to_str().unwrap(),
                    "-t",
                    "worker-snapshot-test:0",
                    pane_command,
                ])
                .output()
                .unwrap()
            });
            assert!(
                split.status.success(),
                "split isolated tmux: {}",
                String::from_utf8_lossy(&split.stderr)
            );
            fixture.record_allocation(&String::from_utf8(split.stdout).unwrap());
            let endpoints = fixture.endpoints();
            for endpoint in endpoints {
                fixture.record_server(endpoint.server_pid);
                fixture.record_process(endpoint.pane_pid);
                fixture.record_fixture_processes(&endpoint);
            }
        }
        fixture
    }

    pub(crate) fn endpoints(&self) -> Vec<crate::proto::TmuxEndpoint> {
        let panes = with_tmux(|tmux| {
            tmux.args([
                "-S",
                self.socket.to_str().unwrap(),
                "list-panes",
                "-a",
                "-F",
                "#{pane_id}",
            ])
            .output()
            .unwrap()
        });
        assert!(
            panes.status.success(),
            "list isolated tmux panes: {}",
            String::from_utf8_lossy(&panes.stderr)
        );
        let endpoints = String::from_utf8(panes.stdout)
            .unwrap()
            .lines()
            .map(|pane_id| {
                let fields = self
                    .wait_for_pane_cwd(pane_id, &self.owner_root)
                    .unwrap_or_else(|error| panic!("{error}"));
                let endpoint = crate::proto::TmuxEndpoint {
                    socket_path: self.socket.to_string_lossy().into_owned(),
                    server_pid: fields[0].parse().unwrap(),
                    tmux_session_id: fields[1].clone(),
                    pane_id: fields[2].clone(),
                    pane_pid: fields[3].parse().unwrap(),
                    codex_session_id: None,
                    codex_thread_id: None,
                };
                self.record_process(endpoint.pane_pid);
                self.record_server(endpoint.server_pid);
                endpoint
            })
            .collect::<Vec<_>>();
        for endpoint in &endpoints {
            self.record_fixture_processes(endpoint);
        }
        endpoints
    }

    pub(crate) fn add_session(&self) -> crate::proto::TmuxEndpoint {
        self.add_session_at(&self.owner_root)
    }

    pub(crate) fn register_session(&self, session: &str) {
        {
            let mut sessions = self.sessions.borrow_mut();
            if !sessions.iter().any(|owned| owned == session) {
                sessions.push(session.to_owned());
            }
        }
        let output = self.display_message(session);
        assert!(
            output.status.success(),
            "register allocated tmux session {session}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let fields = stdout.trim_end().split('|').collect::<Vec<_>>();
        assert_eq!(fields.len(), 5, "allocated tmux session metadata: {stdout:?}");
        assert_eq!(fields[1], session, "registered session identity changed");
        self.record_server(fields[0].parse().expect("allocated server PID"));
        let pane_pid = fields[3].parse().expect("allocated pane PID");
        self.record_process(pane_pid);
        self.record_descendants(pane_pid);
    }

    pub(crate) fn add_session_at(&self, root: &Path) -> crate::proto::TmuxEndpoint {
        static SESSION_SEQ: AtomicU64 = AtomicU64::new(0);
        let session = format!(
            "worker-snapshot-test-{}-{}",
            std::process::id(),
            SESSION_SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let created = with_tmux(|tmux| {
            tmux.args([
                "-S",
                self.socket.to_str().unwrap(),
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{session_id}|#{pane_id}|#{pane_pid}|#{pid}",
                "-c",
                root.to_str().unwrap(),
                "-s",
                &session,
                &self.pane_command,
            ])
            .output()
            .unwrap()
        });
        assert!(
            created.status.success(),
            "add isolated tmux session: {}",
            String::from_utf8_lossy(&created.stderr)
        );
        let created_text = String::from_utf8(created.stdout).unwrap();
        self.record_allocation(&created_text);
        let created_fields = created_text
            .trim_end()
            .split('|')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(
            created_fields.len(),
            4,
            "inspect created isolated tmux session: {created_fields:?}"
        );
        let fields = self
            .wait_for_pane_cwd(&created_fields[1], root)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            fields[1], created_fields[0],
            "readiness queried a different tmux session"
        );
        assert_eq!(
            fields[2], created_fields[1],
            "readiness queried a different tmux pane"
        );

        let endpoint = crate::proto::TmuxEndpoint {
            socket_path: self.socket.to_string_lossy().into_owned(),
            server_pid: fields[0].parse().unwrap(),
            tmux_session_id: fields[1].clone(),
            pane_id: fields[2].clone(),
            pane_pid: fields[3].parse().unwrap(),
            codex_session_id: None,
            codex_thread_id: None,
        };
        self.record_process(endpoint.pane_pid);
        self.record_fixture_processes(&endpoint);
        endpoint
    }

    fn record_allocation(&self, allocation: &str) {
        let fields = allocation.trim_end().split('|').collect::<Vec<_>>();
        if let Some(session) = fields.first().filter(|session| !session.is_empty()) {
            let mut sessions = self.sessions.borrow_mut();
            if !sessions.iter().any(|owned| owned == session) {
                sessions.push((*session).to_owned());
            }
        }
        assert_eq!(fields.len(), 4, "tmux allocation metadata: {allocation:?}");
        self.record_server(fields[3].parse().expect("allocated server PID"));
        let pane_pid = fields[2].parse().expect("allocated pane PID");
        self.record_process(pane_pid);
        self.record_descendants(pane_pid);
    }

    fn record_server(&self, pid: u32) {
        if let Some(start_id) = process_start_identity(pid) {
            *self.server_pid.borrow_mut() = Some((pid, start_id));
        }
    }

    fn record_process(&self, pid: u32) {
        if pid == 0 {
            return;
        }
        let Some(start_id) = process_start_identity(pid) else {
            return;
        };
        let mut processes = self.pane_pids.borrow_mut();
        if !processes.iter().any(|(existing, _)| *existing == pid) {
            processes.push((pid, start_id));
        }
    }

    fn record_process_from_pane(&self, pane_id: &str) {
        let output = self.display_message(pane_id);
        if !output.status.success() {
            return;
        }
        if let Ok(text) = String::from_utf8(output.stdout) {
            if let Some(pid) = text.trim_end().split('|').nth(3).and_then(|value| value.parse().ok()) {
                self.record_process(pid);
                self.record_descendants(pid);
            }
        }
    }

    fn record_fixture_processes(&self, endpoint: &crate::proto::TmuxEndpoint) {
        self.record_descendants(endpoint.pane_pid);
    }

    fn record_descendants(&self, parent: u32) {
        let identity = self.pane_pids.borrow().iter()
            .find(|(pid, _)| *pid == parent).cloned();
        let Some((_, start)) = identity else { return; };
        if !process_identity_matches(parent, &start) {
            return;
        }
        let children = match Command::new("pgrep").args(["-P", &parent.to_string()]).output() {
            Ok(output) => output,
            Err(error) => {
                self.cleanup_errors.borrow_mut().push(format!(
                    "inspect descendants of owned PID {parent}: {error}"
                ));
                return;
            }
        };
        if !children.status.success() && children.status.code() != Some(1) {
            self.cleanup_errors.borrow_mut().push(format!(
                "inspect descendants of owned PID {parent}: {}",
                String::from_utf8_lossy(&children.stderr)
            ));
            return;
        }
        if !process_identity_matches(parent, &start) {
            return;
        }
        for child in String::from_utf8_lossy(&children.stdout).lines() {
            if let Ok(pid) = child.trim().parse() {
                let Some(start_id) = process_start_identity(pid) else { continue; };
                if self.process_descends_from(pid, parent)
                    && process_identity_matches(parent, &start)
                    && process_identity_matches(pid, &start_id)
                {
                    self.record_process(pid);
                    self.record_descendants(pid);
                }
            }
        }
    }

    fn process_descends_from(&self, pid: u32, root: u32) -> bool {
        let mut current = pid;
        for _ in 0..64 {
            if current == root {
                return true;
            }
            let output = match Command::new("ps")
                .args(["-o", "ppid=", "-p", &current.to_string()])
                .output()
            {
                Ok(output) => output,
                Err(error) => {
                    self.cleanup_errors.borrow_mut().push(format!(
                        "inspect ancestry of fixture PID {current}: {error}"
                    ));
                    return false;
                }
            };
            if !output.status.success() {
                return false;
            }
            let Ok(parent) = String::from_utf8_lossy(&output.stdout).trim().parse::<u32>() else {
                return false;
            };
            if parent == 0 || parent == current {
                return false;
            }
            current = parent;
        }
        false
    }

    fn terminate_processes(&self, owned: &[(u32, String)]) {
        for (pid, start_id) in owned.iter().rev() {
            terminate_fixture_pid(*pid, start_id, libc::SIGTERM);
            // Keep the parent alive while it reaps its child before signaling
            // that parent. Signaling the whole tree together can orphan it.
            let deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < deadline && process_identity_matches(*pid, start_id) {
                std::thread::sleep(FIXTURE_PANE_READY_POLL);
            }
            if process_identity_matches(*pid, start_id) {
                terminate_fixture_pid(*pid, start_id, libc::SIGKILL);
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline && process_identity_matches(*pid, start_id) {
                std::thread::sleep(FIXTURE_PANE_READY_POLL);
            }
        }
        for (pid, start_id) in owned {
            if process_identity_matches(*pid, start_id) {
                self.cleanup_errors
                    .borrow_mut()
                    .push(format!("owned fixture PID {pid} survived teardown"));
            }
        }
    }

    fn terminate_owned_processes(&self) {
        let parents = self.pane_pids.borrow().clone();
        for (pid, _) in parents {
            self.record_descendants(pid);
        }
        let owned = self.pane_pids.borrow().clone();
        self.terminate_processes(&owned);
    }

    fn wait_for_pane_cwd(
        &self,
        target: &str,
        expected_root: &Path,
    ) -> Result<Vec<String>, String> {
        let expected = std::fs::canonicalize(expected_root).map_err(|error| {
            format!(
                "canonicalize fixture root {}: {error}",
                expected_root.display()
            )
        })?;
        let deadline = Instant::now() + FIXTURE_PANE_READY_TIMEOUT;
        loop {
            let output = self.display_message(target);
            if !output.status.success() {
                return Err(format!(
                    "inspect isolated tmux session failed: root={} socket={} target={} exit_code={:?} stdout={:?} stderr={:?}",
                    expected_root.display(),
                    self.socket.display(),
                    target,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                ));
            }
            let stdout = String::from_utf8(output.stdout).map_err(|error| {
                format!(
                    "isolated tmux session output is not UTF-8: root={} socket={} target={} bytes={:?}",
                    expected_root.display(),
                    self.socket.display(),
                    target,
                    error.as_bytes(),
                )
            })?;
            let fields = stdout
                .trim_end()
                .split('|')
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if fields.len() != 5 || fields.iter().any(|field| field.is_empty()) {
                return Err(format!(
                    "inspect isolated tmux session returned {} fields: root={} socket={} target={} stdout={:?}",
                    fields.len(),
                    expected_root.display(),
                    self.socket.display(),
                    target,
                    stdout,
                ));
            }
            let actual_cwd = &fields[4];
            let actual = std::fs::canonicalize(actual_cwd).map_err(|error| {
                format!("canonicalize observed pane cwd {actual_cwd}: {error}")
            })?;
            if actual == expected {
                return Ok(fields);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "cwd did not reach fixture root before timeout: root={} actual={} socket={} target={} exit_code={:?} stdout={:?} stderr={:?}",
                    expected_root.display(),
                    actual_cwd,
                    self.socket.display(),
                    target,
                    output.status.code(),
                    stdout,
                    String::from_utf8_lossy(&output.stderr),
                ));
            }
            std::thread::sleep(FIXTURE_PANE_READY_POLL);
        }
    }

    fn display_message(&self, target: &str) -> std::process::Output {
        with_tmux(|tmux| {
            tmux.args([
                "-S",
                self.socket.to_str().unwrap(),
                "display-message",
                "-p",
                "-t",
                target,
                "#{pid}|#{session_id}|#{pane_id}|#{pane_pid}|#{pane_current_path}",
            ])
            .output()
            .unwrap()
        })
    }

    fn kill_pane(&self, pane_id: &str) {
        self.record_process_from_pane(pane_id);
        if let Some(endpoint) = self
            .endpoints()
            .into_iter()
            .find(|endpoint| endpoint.pane_id == pane_id)
        {
            self.record_fixture_processes(&endpoint);
            let owned = self
                .pane_pids
                .borrow()
                .iter()
                .filter(|(pid, _)| self.process_descends_from(*pid, endpoint.pane_pid))
                .cloned()
                .collect::<Vec<_>>();
            self.terminate_processes(&owned);
        }
        let output = with_tmux(|tmux| {
            tmux.args([
                "-S",
                self.socket.to_str().unwrap(),
                "kill-pane",
                "-t",
                pane_id,
            ])
            .output()
            .unwrap()
        });
        if !output.status.success() {
            let listed = with_tmux(|tmux| {
                tmux.args([
                    "-S",
                    self.socket.to_str().unwrap(),
                    "list-panes",
                    "-a",
                    "-F",
                    "#{pane_id}",
                ])
                .output()
            });
            let still_listed = listed.as_ref().is_ok_and(|out| {
                out.status.success()
                    && String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .any(|line| line == pane_id)
            });
            assert!(
                !still_listed,
                "kill test-owned tmux pane {pane_id}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn close_owned_sessions(&self) {
        self.terminate_owned_processes();
        let sessions = self.sessions.borrow_mut().drain(..).collect::<Vec<_>>();
        for session in sessions {
            let output = with_tmux(|tmux| {
                tmux.args([
                    "-S",
                    self.socket.to_str().unwrap(),
                    "kill-session",
                    "-t",
                    &session,
                ])
                .output()
            });
            if output.as_ref().is_ok_and(|output| !output.status.success()) {
                let listed = with_tmux(|tmux| {
                    tmux.args([
                        "-S",
                        self.socket.to_str().unwrap(),
                        "list-sessions",
                        "-F",
                        "#{session_id}",
                    ])
                    .output()
                    .unwrap()
                });
                let still_listed = listed.status.success()
                    && String::from_utf8_lossy(&listed.stdout)
                        .lines()
                        .any(|line| line == session);
                if still_listed {
                    self.cleanup_errors.borrow_mut().push(format!(
                        "close owned tmux session {session}: {}",
                        String::from_utf8_lossy(&output.as_ref().unwrap().stderr)
                    ));
                }
            }
        }
        if let Some((pid, start_id)) = self.server_pid.borrow().clone() {
            let deadline = Instant::now() + Duration::from_secs(2);
            while process_identity_matches(pid, &start_id) && Instant::now() < deadline {
                std::thread::sleep(FIXTURE_PANE_READY_POLL);
            }
            if process_identity_matches(pid, &start_id) {
                self.cleanup_errors
                    .borrow_mut()
                    .push(format!("private tmux server PID {pid} survived session teardown"));
            }
        }
        let server_alive = self
            .server_pid
            .borrow()
            .as_ref()
            .is_some_and(|(pid, start)| process_identity_matches(*pid, start));
        if self.socket.exists() && !server_alive {
            if let Err(error) = std::fs::remove_file(&self.socket) {
                if self.socket.exists() {
                    self.cleanup_errors.borrow_mut().push(format!(
                        "remove stale private tmux socket {}: {error}",
                        self.socket.display()
                    ));
                }
            }
        }
        if self.socket.exists() {
            self.cleanup_errors.borrow_mut().push(format!(
                "private tmux socket {} survived natural server exit",
                self.socket.display()
            ));
        }
    }
}

fn process_start_identity(pid: u32) -> Option<String> {
    crate::host_runtime::exact_process_start_id(pid)
        .ok()
        .flatten()
}

fn process_identity_matches(pid: u32, expected: &str) -> bool {
    process_start_identity(pid).as_deref() == Some(expected)
}

fn terminate_fixture_pid(pid: u32, expected_start_id: &str, signal: libc::c_int) {
    if !process_identity_matches(pid, expected_start_id) {
        return;
    }
    unsafe {
        libc::kill(pid as libc::pid_t, signal);
    }
}

fn with_tmux<R>(run: impl FnOnce(&mut Command) -> R) -> R {
    let _guard = crate::scope::TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    run(&mut Command::new("tmux"))
}

thread_local! {
    static REGISTERED_TEST_TMUX: RefCell<Option<(IsolatedTmux, usize)>> = const { RefCell::new(None) };
    static ONLINE_TEST_TMUX: RefCell<Option<(IsolatedTmux, usize)>> = const { RefCell::new(None) };
}

impl Drop for IsolatedTmux {
    fn drop(&mut self) {
        self.close_owned_sessions();
        if self.socket.exists()
            && !self
                .server_pid
                .borrow()
                .as_ref()
                .is_some_and(|(pid, start)| process_identity_matches(*pid, start))
        {
            let _ = std::fs::remove_file(&self.socket);
        }
        if !std::thread::panicking() && !self.cleanup_errors.borrow().is_empty() {
            panic!("isolated tmux cleanup failed: {}", self.cleanup_errors.borrow().join("; "));
        }
    }
}

struct TestTmuxFixtureScope {
    root: Option<PathBuf>,
}

impl TestTmuxFixtureScope {
    fn enter() -> Self {
        crate::client::adapters::tmux::tests::reap_orphan_test_tmux_artifacts();
        clear_test_tmux_pools();
        Self { root: None }
    }

    fn for_root(root: &Path) -> Self {
        let mut scope = Self::enter();
        scope.root = Some(root.to_owned());
        scope
    }
}

impl Drop for TestTmuxFixtureScope {
    fn drop(&mut self) {
        clear_test_tmux_pools();
        if let Some(root) = &self.root {
            assert!(REGISTERED_TEST_TMUX.with(|fixture| fixture.borrow().is_none()));
            assert!(ONLINE_TEST_TMUX.with(|fixture| fixture.borrow().is_none()));
            std::fs::remove_dir_all(root).expect("remove root after pooled tmux cleanup");
        }
    }
}

fn clear_test_tmux_pools() {
    let registered = REGISTERED_TEST_TMUX.with(|fixture| fixture.borrow_mut().take());
    let online = ONLINE_TEST_TMUX.with(|fixture| fixture.borrow_mut().take());
    drop((registered, online));
}

#[test]
fn isolated_tmux_scope_releases_both_pools_before_root_removal() {
    for unwind in [false, true] {
        let (server, root) = test_server();
        let mut resources = Vec::new();
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _scope = TestTmuxFixtureScope::for_root(&root);
            assert!(register(&server, "scope-offline", "%scope-offline").ok);
            assert!(register_online(&server, "scope-online", "%scope-online").ok);
            for pool in [&REGISTERED_TEST_TMUX, &ONLINE_TEST_TMUX] {
                pool.with(|fixture| {
                    let fixture = fixture.borrow();
                    let tmux = &fixture.as_ref().expect("both pools allocated").0;
                    resources.push((
                        tmux.socket.clone(),
                        tmux.pane_pids.borrow().clone(),
                        tmux.server_pid.borrow().clone().unwrap(),
                    ));
                });
            }
            assert!(root.exists(), "root must remain available during the test");
            if unwind {
                std::panic::panic_any("exercise both-pool root teardown");
            }
        }));
        if unwind {
            let payload = caught.expect_err("test body must unwind");
            assert_eq!(payload.downcast_ref::<&str>(), Some(&"exercise both-pool root teardown"));
        } else {
            caught.expect("normal return must clean up");
        }
        assert!(REGISTERED_TEST_TMUX.with(|fixture| fixture.borrow().is_none()));
        assert!(ONLINE_TEST_TMUX.with(|fixture| fixture.borrow().is_none()));
        assert!(!root.exists(), "guard must remove its root on return and unwind");
        for (socket, processes, (server_pid, server_start)) in resources {
            assert!(!socket.exists(), "pooled socket survived: {}", socket.display());
            assert!(!process_identity_matches(server_pid, &server_start), "pooled server survived");
            for (pid, start) in processes {
                assert!(!process_identity_matches(pid, &start), "pooled process survived: {pid}");
            }
        }
    }
}

#[test]
fn isolated_tmux_teardown_reaps_foreground_processes() {
    let _scope = TestTmuxFixtureScope::enter();
    let (server, root) = test_server();
    let tmux = IsolatedTmux::start_online(&root);
    let endpoint = tmux.endpoints().remove(0);
    wait_for_online_fixture(&endpoint);
    let pid_deadline = std::time::Instant::now() + FIXTURE_PANE_READY_TIMEOUT;
    let foreground_pid_file = loop {
        if let Some(found) = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("codex.pid-"))
            })
        {
            break found;
        }
        assert!(
            std::time::Instant::now() < pid_deadline,
            "foreground fixture pid file"
        );
        std::thread::sleep(FIXTURE_PANE_READY_POLL);
    };
    let foreground_pid = std::fs::read_to_string(foreground_pid_file)
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    let server_pid = endpoint.server_pid;
    let socket = tmux.socket.clone();

    drop(tmux);

    let deadline = Instant::now() + Duration::from_secs(2);
    while (crate::host_runtime::exact_process_start_id(foreground_pid).unwrap().is_some()
        || crate::host_runtime::exact_process_start_id(server_pid).unwrap().is_some()
        || socket.exists())
        && Instant::now() < deadline
    {
        std::thread::sleep(FIXTURE_PANE_READY_POLL);
    }
    assert_eq!(
        crate::host_runtime::exact_process_start_id(foreground_pid).unwrap(),
        None,
        "foreground fixture process survived teardown: pid={foreground_pid}"
    );
    assert_eq!(
        crate::host_runtime::exact_process_start_id(server_pid).unwrap(),
        None,
        "private tmux server survived teardown: pid={server_pid}"
    );
    assert!(!socket.exists(), "private tmux socket survived teardown");
    drop(server);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn isolated_tmux_teardown_reaps_descendant_and_ignores_foreign_pid_file() {
    let _scope = TestTmuxFixtureScope::enter();
    let (server, root) = test_server();
    let sentinel_root = root.with_extension("descendant-sentinel");
    std::fs::create_dir_all(&sentinel_root).unwrap();
    let sentinel = IsolatedTmux::start_single(&sentinel_root);
    let sentinel_endpoint = sentinel.endpoints().remove(0);
    let sentinel_start = process_start_identity(sentinel_endpoint.pane_pid).unwrap();
    std::fs::write(root.join("codex.pid-foreign"), sentinel_endpoint.pane_pid.to_string()).unwrap();
    let tmux = IsolatedTmux::start_with_spare_pane(
        &root,
        false,
        "sleep 60 & child=$!; printf '%s\\n' \"$child\" > codex.pid-child; wait \"$child\"",
    );
    let endpoint = tmux.endpoints().remove(0);
    let deadline = Instant::now() + FIXTURE_PANE_READY_TIMEOUT;
    let child = loop {
        if let Some(pid) = std::fs::read_to_string(root.join("codex.pid-child"))
            .ok().and_then(|text| text.trim().parse::<u32>().ok())
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "descendant PID file was not created");
        std::thread::sleep(FIXTURE_PANE_READY_POLL);
    };
    assert_ne!(child, endpoint.pane_pid, "fixture must have a distinct descendant");
    tmux.record_fixture_processes(&endpoint);
    let processes = tmux.pane_pids.borrow().clone();
    assert!(processes.iter().any(|(pid, _)| *pid == child), "descendant not recorded");
    assert!(!processes.iter().any(|(pid, _)| *pid == sentinel_endpoint.pane_pid));
    let socket = tmux.socket.clone();
    let (server_pid, server_start) = tmux.server_pid.borrow().clone().unwrap();
    drop(tmux);
    for (pid, start) in processes {
        assert!(!process_identity_matches(pid, &start), "owned process survived: {pid}");
    }
    assert!(!process_identity_matches(server_pid, &server_start), "private server survived");
    assert!(!socket.exists(), "private socket survived");
    assert!(process_identity_matches(sentinel_endpoint.pane_pid, &sentinel_start));
    assert!(sentinel_root.exists(), "foreign root was removed");
    assert_eq!(sentinel.endpoints()[0].pane_id, sentinel_endpoint.pane_id);
    drop(sentinel);
    std::fs::remove_dir_all(sentinel_root).unwrap();
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn isolated_tmux_scope_cleans_pooled_fixtures_on_unwind() {
    let (server, root) = test_server();
    let sentinel_root = root.with_extension("sentinel");
    std::fs::create_dir_all(&sentinel_root).unwrap();
    let sentinel = IsolatedTmux::start_single(&sentinel_root);
    let sentinel_endpoint = sentinel.endpoints().remove(0);
    let sentinel_pid = sentinel_endpoint.pane_pid;
    let sentinel_start = process_start_identity(sentinel_pid).expect("sentinel start identity");
    let mut captured = None;
    let panic_payload = "fixture unwind after setup";
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _scope = TestTmuxFixtureScope::enter();
        register(&server, "unwind-peer", "thread-unwind-peer");
        let facts = REGISTERED_TEST_TMUX.with(|fixture| {
            let fixture = fixture.borrow();
            let tmux = &fixture.as_ref().expect("registered fixture").0;
            let endpoints = tmux.endpoints();
            let sessions = tmux.sessions.borrow().clone();
            let processes = tmux.pane_pids.borrow().clone();
            let server = tmux.server_pid.borrow().clone().expect("recorded fixture server");
            assert_eq!(endpoints.len(), 2, "spare pane must exist before unwind");
            assert!(processes.len() >= 2, "both pane leaders must be recorded");
            assert!(!sessions.is_empty(), "fixture session must exist before unwind");
            (tmux.socket.clone(), sessions, processes, server)
        });
        assert!(facts.0.exists(), "fixture socket is missing before unwind");
        captured = Some(facts);
        std::panic::panic_any(panic_payload);
    }));
    let payload = caught.expect_err("test body must unwind");
    assert_eq!(payload.downcast_ref::<&str>(), Some(&panic_payload));
    let (socket, sessions, processes, (server_pid, server_start)) = captured.unwrap();
    assert_eq!(
        REGISTERED_TEST_TMUX.with(|fixture| fixture.borrow().is_some()),
        false,
        "pooled fixture survived a test unwind"
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while (processes.iter().any(|(pid, start)| process_identity_matches(*pid, start))
        || process_identity_matches(server_pid, &server_start)
        || socket.exists())
        && Instant::now() < deadline
    {
        std::thread::sleep(FIXTURE_PANE_READY_POLL);
    }
    for (pid, start) in processes {
        assert!(
            !process_identity_matches(pid, &start),
            "fixture process survived unwind: pid={pid}"
        );
    }
    assert!(
        !process_identity_matches(server_pid, &server_start),
        "fixture server survived unwind: pid={server_pid}"
    );
    assert!(!socket.exists(), "fixture socket survived unwind");
    let listed = Command::new("tmux")
        .args(["-S", socket.to_str().unwrap(), "list-sessions", "-F", "#{session_id}"])
        .output()
        .unwrap();
    assert!(!listed.status.success(), "fixture sessions survived unwind: {sessions:?}");
    assert!(
        process_identity_matches(sentinel_pid, &sentinel_start),
        "unrelated sentinel died during fixture cleanup: pid={sentinel_pid}"
    );
    assert!(sentinel.socket.exists(), "unrelated sentinel socket was removed");
    assert!(sentinel_root.exists(), "unrelated sentinel root was removed");
    assert_eq!(sentinel.endpoints()[0].pane_id, sentinel_endpoint.pane_id);
    drop(sentinel);
    std::fs::remove_dir_all(sentinel_root).unwrap();
    drop(server);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn isolated_tmux_partial_setup_failure_releases_resources() {
    let _scope = TestTmuxFixtureScope::enter();
    let root = std::env::temp_dir().join(format!(
        "collab-peer-partial-setup-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let socket = std::env::temp_dir().join(format!("ctmux-partial-{}.sock", std::process::id()));
    let owner = IsolatedTmux {
        socket: socket.clone(),
        owner_root: root.clone(),
        pane_command: "sleep 60".into(),
        sessions: RefCell::new(Vec::new()),
        server_pid: RefCell::new(None),
        pane_pids: RefCell::new(Vec::new()),
        cleanup_errors: RefCell::new(Vec::new()),
    };
    let output = with_tmux(|tmux| {
        tmux.args([
            "-S",
            socket.to_str().unwrap(),
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{session_id}|#{pane_id}|#{pane_pid}|#{pid}",
            "-c",
            root.to_str().unwrap(),
            "-s",
            "partial-setup",
            "sleep 60",
        ])
        .output()
        .unwrap()
    });
    assert!(output.status.success(), "partial setup session creation failed");
    owner.record_allocation(&String::from_utf8(output.stdout).unwrap());
    let split = with_tmux(|tmux| {
        tmux.args([
            "-S",
            socket.to_str().unwrap(),
            "split-window",
            "-d",
            "-P",
            "-F",
            "#{session_id}|#{pane_id}|#{pane_pid}|#{pid}",
            "-c",
            root.to_str().unwrap(),
            "-t",
            "partial-setup:0",
            "sleep 60",
        ])
        .output()
        .unwrap()
    });
    assert!(split.status.success(), "partial setup spare allocation failed");
    owner.record_allocation(&String::from_utf8(split.stdout).unwrap());
    let error = owner
        .wait_for_pane_cwd("partial-setup:0", &root.join("missing-readiness-root"))
        .unwrap_err();
    assert!(error.contains("canonicalize fixture root"), "{error}");
    let processes = owner.pane_pids.borrow().clone();
    let (server_pid, server_start) = owner.server_pid.borrow().clone().unwrap();
    assert!(processes.len() >= 2, "spare allocation must be recorded before failure");
    drop(owner);
    let deadline = Instant::now() + Duration::from_secs(2);
    while (processes.iter().any(|(pid, start)| process_identity_matches(*pid, start))
        || process_identity_matches(server_pid, &server_start)
        || socket.exists())
        && Instant::now() < deadline
    {
        std::thread::sleep(FIXTURE_PANE_READY_POLL);
    }
    for (pid, start) in processes {
        assert!(
            !process_identity_matches(pid, &start),
            "partial setup process survived cleanup: pid={pid}"
        );
    }
    assert!(
        !process_identity_matches(server_pid, &server_start),
        "partial setup server survived cleanup: pid={server_pid}"
    );
    assert!(!socket.exists(), "partial setup socket survived cleanup");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn isolated_tmux_readiness_rejects_a_persistent_cwd_mismatch() {
    let _scope = TestTmuxFixtureScope::enter();
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let id = SEQ.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "collab-peer-readiness-cwd-mismatch-{}-{id}",
        std::process::id()
    ));
    let other = root.with_file_name(format!(
        "{}-other",
        root.file_name().unwrap().to_string_lossy()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let tmux = IsolatedTmux::start_single(&root);
    let endpoint = tmux.endpoints().remove(0);
    let error = tmux
        .wait_for_pane_cwd(&endpoint.pane_id, &other)
        .unwrap_err();
    assert!(
        error.contains("cwd did not reach fixture root before timeout"),
        "{error}"
    );
    assert!(error.contains(&root.display().to_string()), "{error}");
    assert!(error.contains(&other.display().to_string()), "{error}");
    drop(tmux);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(other).unwrap();
}

#[test]
fn isolated_tmux_readiness_exposes_query_errors() {
    let _scope = TestTmuxFixtureScope::enter();
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let id = SEQ.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "collab-peer-readiness-query-errors-{}-{id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let tmux = IsolatedTmux::start_single(&root);
    let error = tmux
        .wait_for_pane_cwd("missing-readiness-target", &root)
        .unwrap_err();
    assert!(
        error.contains("inspect isolated tmux session returned"),
        "{error}"
    );
    assert!(error.contains("missing-readiness-target"), "{error}");
    drop(tmux);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn real_pane_cwd_is_rejected_by_the_production_registration_gate() {
    let _scope = TestTmuxFixtureScope::enter();
    let (server, root) = test_server();
    let other = root.with_file_name(format!(
        "{}-other",
        root.file_name().unwrap().to_string_lossy()
    ));
    std::fs::create_dir_all(&other).unwrap();
    let tmux = IsolatedTmux::start_single(&other);
    let endpoint = tmux.endpoints().remove(0);
    let observed = Command::new("tmux")
        .args([
            "-S",
            &endpoint.socket_path,
            "display-message",
            "-p",
            "-t",
            &endpoint.pane_id,
            "#{pane_current_path}",
        ])
        .output()
        .unwrap();
    assert!(
        observed.status.success(),
        "inspect real pane cwd: {}",
        String::from_utf8_lossy(&observed.stderr)
    );
    let candidate_cwd = String::from_utf8(observed.stdout)
        .unwrap()
        .trim()
        .to_owned();
    let error = validate_transport_candidates(
        &server,
        &TransportCandidates {
            desktop_bridge: None,
            appserver: None,
            dsh: None,
            rex: None,
            tmux: Some(crate::proto::TmuxCandidate {
                endpoint,
                cwd: candidate_cwd,
            }),
        },
        root.to_str().unwrap(),
    )
    .unwrap_err();
    assert!(error.starts_with("RUNTIME_BINDING_REJECTED:"), "{error}");
    assert!(error.contains("does not match project root"), "{error}");
    drop(tmux);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(other).unwrap();
}

/// Defined committed owner for tests that begin with an existing peer. This
/// seeds typed state; it does not test public initial participant admission.
pub(super) fn register_tmux(
    server: &Server,
    id: &str,
    endpoint: crate::proto::TmuxEndpoint,
) -> Resp {
    let app = AppServerId::new("tui-default").unwrap();
    let cwd = server.root.display().to_string();
    let transport = SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some(endpoint.socket_path.clone()),
        namespace: Some(endpoint.tmux_session_id.clone()),
        session_id: endpoint
            .codex_session_id
            .clone()
            .or_else(|| Some(endpoint.tmux_session_id.clone())),
        thread_id: endpoint
            .codex_thread_id
            .clone()
            .or_else(|| Some(endpoint.pane_id.clone())),
        tmux_endpoint: Some(endpoint),
        rex_endpoint: None,
        capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
        self_check: "prior committed peer fixture".into(),
    };
    let response = seed_committed_owner(
        server,
        id,
        &format!("token-{id}"),
        &transport,
        &cwd,
        Some(app.clone()),
    );
    if response.ok {
        commit_current_thread_route_for_runtime(server, server, id, &cwd, Some(&app)).unwrap();
        restore_registered_peer_default_leases(server);
    }
    response
}

/// Seed a defined prior typed owner. This is fixture state, not evidence that
/// a public initial Register passed participant admission.
pub(super) fn seed_committed_owner(
    server: &Server,
    worker_id: &str,
    token: &str,
    transport: &SelectedTransport,
    cwd: &str,
    app_scope: Option<AppServerId>,
) -> Resp {
    let prepared = prepare_register_typed(
        server,
        worker_id,
        token,
        transport,
        cwd,
        Some(GlobalState::canonical_project_scope(&server.root).unwrap()),
        app_scope,
        false,
    )
    .unwrap();
    let TypedCommand::RegisterWorker { binding, .. } = &prepared.typed.command;
    let project = binding.project_scope.as_str().to_owned();
    let session = binding.session_id.as_ref().unwrap().to_string();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let name = control
        .key_for_worker(&binding.project_scope, &binding.app_scope_id, worker_id)
        .unwrap()
        .map(|key| key.name.clone())
        .unwrap_or_else(|| {
            control
                .ordinary_name(&binding.project_scope, &binding.app_scope_id, worker_id)
                .unwrap()
        });
    store
        .commit(binding_store::ControlChange::Commit {
            key: binding_slot::IdentityBindingKey::new(
                binding.project_scope.clone(),
                binding.app_scope_id.clone(),
                name.clone(),
            ),
            value: binding_slot::BindingValue::new(binding.clone(), transport.clone()),
            expected_generation: None,
        })
        .unwrap();
    let response = consume_register_typed(server, worker_id, transport, prepared, None, None, None);
    assert!(response.ok, "committed registration fixture: {response:?}");
    let mut state = server.state.lock().unwrap();
    let previous = state.name_occupancy(&project, &name);
    let occupancy = previous
        .filter(|old| old.worker_id == worker_id && old.session_id == session)
        .cloned()
        .unwrap_or_else(|| name_state::NameOccupancy {
            name,
            worker_id: worker_id.into(),
            session_id: session,
            occupancy_revision: previous.map_or(1, |old| old.occupancy_revision + 1),
            updated_ms: now_ms(),
            pane_anchor: None,
        });
    let events = name_resolution::build_name_resolution_events_with_rename_from(
        &state,
        &project,
        &occupancy,
        &[],
        previous,
    );
    server.commit_locked_checked(&mut state, &events).unwrap();
    response
}

fn retire_registered_test_pane(binding: &crate::server::global_state::RuntimeBinding) {
    let pane_id = binding
        .tmux_endpoint
        .as_ref()
        .expect("test peer has a tmux endpoint")
        .pane_id
        .clone();
    REGISTERED_TEST_TMUX.with(|fixture| {
        fixture
            .borrow()
            .as_ref()
            .expect("test tmux fixture exists")
            .0
            .kill_pane(&pane_id)
    });
}

pub(super) fn kill_registered_worker_pane(server: &Server, worker_id: &str) {
    let endpoint = server
        .state
        .lock()
        .unwrap()
        .workers
        .get(worker_id)
        .and_then(|worker| worker.transport.as_ref())
        .and_then(|transport| transport.tmux_endpoint.as_ref())
        .expect("test worker has a registered tmux endpoint")
        .clone();
    REGISTERED_TEST_TMUX.with(|fixture| {
        fixture
            .borrow()
            .as_ref()
            .expect("test tmux fixture exists")
            .0
            .kill_pane(&endpoint.pane_id)
    });
    eprintln!(
        "owned killed pane worker={worker_id} endpoint_probe={:?}",
        crate::client::adapters::tmux::probe(&endpoint)
    );
}

fn stop_registered_test_tmux_server() {
    REGISTERED_TEST_TMUX.with(|fixture| {
        let mut fixture = fixture.borrow_mut();
        fixture
            .as_ref()
            .expect("test tmux fixture exists")
            .0
            .close_owned_sessions();
        *fixture = None;
    });
}

#[test]
fn handoff_to_unregistered_recipient_fails_typed_before_recording() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "operator".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        "candidate awaits routing".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok);
    let error = response.error.unwrap_or_default();
    assert!(
        error.contains("HANDOFF_TARGET_UNRESOLVED") && error.contains("operator"),
        "unexpected error: {error}"
    );
    assert_eq!(response.data["error_code"], "HANDOFF_TARGET_UNRESOLVED");
    assert_eq!(response.data["unresolved"]["kind"], "recipient");
    assert_eq!(response.data["unresolved"]["value"], "operator");
    assert_eq!(response.data["reason"], "recipient_not_registered");
    assert_eq!(response.data["recorded"], false);
    assert!(
        server.state.lock().unwrap().msgs.is_empty(),
        "an unresolved handoff target must not create a durable message"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_missing_worktree_path_fails_typed_before_recording() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let missing = root.join("playground/does-not-exist").display().to_string();
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        format!("preflight implementation lives at {missing}"),
        None,
        "immediate".into(),
    );
    assert!(!response.ok);
    let error = response.error.unwrap_or_default();
    assert!(
        error.contains("HANDOFF_TARGET_UNRESOLVED") && error.contains("does-not-exist"),
        "unexpected error: {error}"
    );
    assert_eq!(response.data["unresolved"]["kind"], "worktree");
    assert_eq!(response.data["reason"], "worktree_path_missing");
    assert_eq!(response.data["recorded"], false);
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_registered_recipient_and_existing_path_succeeds() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let sender_endpoint = registered_binding(&server, "handoff-sender")
        .tmux_endpoint
        .expect("sender has a tmux binding");
    let recipient_endpoint = registered_binding(&server, "handoff-owner")
        .tmux_endpoint
        .expect("recipient has a tmux binding");
    assert_eq!(sender_endpoint.socket_path, recipient_endpoint.socket_path);
    assert_ne!(
        sender_endpoint.tmux_session_id,
        recipient_endpoint.tmux_session_id
    );
    assert_ne!(sender_endpoint.pane_id, recipient_endpoint.pane_id);
    let worktree = root.join("playground/handoff-live");
    std::fs::create_dir_all(&worktree).unwrap();
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        format!("preflight implementation lives at {}", worktree.display()),
        None,
        "immediate".into(),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["durable"], true);
    assert!(response.data["msg_id"].is_string());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_relative_missing_worktree_fails_typed() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    std::fs::create_dir_all(root.join("playground")).unwrap();
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        "preflight implementation lives at playground/gone-relative".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok, "{response:?}");
    let error = response.error.unwrap_or_default();
    assert!(
        error.contains("HANDOFF_TARGET_UNRESOLVED") && error.contains("gone-relative"),
        "unexpected error: {error}"
    );
    assert_eq!(response.data["reason"], "worktree_path_missing");
    assert_eq!(response.data["recorded"], false);
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_unrelated_absolute_worktree_leaf_still_delivers() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let unrelated = "/Volumes/extension/code/other-project/worktree";
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("unrelated worktree-leaf path".into()),
        format!("see {unrelated} for reference"),
        None,
        "immediate".into(),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["durable"], true);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_prose_url_worktree_segment_still_delivers() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("prose url".into()),
        "reference https://example.com/docs/worktree for background".into(),
        None,
        "immediate".into(),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["durable"], true);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_dot_prefixed_missing_worktree_fails_typed() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    std::fs::create_dir_all(root.join("playground")).unwrap();
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("dot-prefixed worktree reference".into()),
        "preflight implementation lives at ./playground/gone-wt".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok, "{response:?}");
    let error = response.error.unwrap_or_default();
    assert!(
        error.contains("HANDOFF_TARGET_UNRESOLVED") && error.contains("gone-wt"),
        "unexpected error: {error}"
    );
    assert_eq!(response.data["reason"], "worktree_path_missing");
    assert_eq!(response.data["recorded"], false);
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_registered_task_worktree_outside_playground_fails_typed() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let registered = root.join("outside/wt");
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "task-handoff-registered-wt".into(),
            owner: "handoff-owner".into(),
            created_by: "handoff-owner".into(),
            feature_id: None,
            worktree_path: Some(registered.display().to_string()),
            branch: None,
            base_commit: None,
            priority: "p1".into(),
            status: "working".into(),
            next_step: None,
            wait: None,
            isolation_mode: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        },
    }]);
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("registered worktree reference".into()),
        format!("preflight implementation lives at {}", registered.display()),
        None,
        "immediate".into(),
    );
    assert!(!response.ok, "{response:?}");
    assert_eq!(response.data["reason"], "worktree_path_missing");
    assert_eq!(response.data["recorded"], false);
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_with_unrelated_playground_path_still_delivers() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let response = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("unrelated path mention".into()),
        "see /Volumes/extension/code/other-project/playground/other-worktree for reference".into(),
        None,
        "immediate".into(),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["durable"], true);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn handoff_dedup_resend_survives_a_path_that_disappeared_after_delivery() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let worktree = root.join("playground/handoff-dedup");
    std::fs::create_dir_all(&worktree).unwrap();
    let body = format!("preflight implementation lives at {}", worktree.display());
    let first = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        body.clone(),
        None,
        "immediate".into(),
    );
    assert!(first.ok, "{first:?}");
    let msg_id = first.data["msg_id"].as_str().unwrap().to_owned();
    std::fs::remove_dir_all(&worktree).unwrap();
    let resend = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("preflight candidate handoff".into()),
        body,
        None,
        "immediate".into(),
    );
    assert_eq!(resend.data["deduplicated"], true);
    assert_eq!(resend.data["msg_id"], msg_id.as_str());
    std::fs::remove_dir_all(root).unwrap();
}

pub(crate) fn test_server() -> (Server, PathBuf) {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("collab-peer-{}-{n}", std::process::id()));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let state_path = server_dir.join("state.json");
    current_state::save(&state_path, &State::default()).unwrap();
    (
        Server {
            config: crate::config::Config::default(),
            root: root.clone(),
            storage_root: root.clone(),
            state_path,
            host_paths: HostPaths::for_state_root(root.join("host-state")).unwrap(),
            register_gate: Arc::new(Mutex::new(())),
            state: Mutex::new(State::default()),
            appserver_candidate_check: Arc::new(|candidate| {
                Ok(test_appserver_transport(&candidate.thread_id))
            }),
            appserver_notification_sink: Arc::new(|_, _, _, _, _, _| {
                Ok(serde_json::json!({"accepted": true}))
            }),
            appserver_thread_status: Arc::new(|_, thread_id| {
                Ok(serde_json::json!({
                    "thread": {
                        "id": thread_id,
                        "status": {"type": "idle"},
                        "canAcceptDirectInput": true
                    }
                }))
            }),
            appserver_thread_archive: Arc::new(|_, _| Ok(serde_json::json!({"archived": true}))),
            rex_monitor: Default::default(),
            mailbox_notify: tokio::sync::Notify::new(),
        },
        root,
    )
}

fn configured_test_worktree(server: &mut Server, root: &Path, slug: &str) -> PathBuf {
    let base = root.with_file_name(format!(
        "{}-external",
        root.file_name().unwrap().to_string_lossy()
    ));
    let path = base.join(configured_project_key(root).unwrap()).join(slug);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    server.config.worktree.base = Some(base);
    path
}

pub(crate) fn test_appserver_transport(thread_id: &str) -> SelectedTransport {
    SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some(format!("session-{thread_id}")),
        thread_id: Some(thread_id.into()),
        tmux_endpoint: None,
        rex_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    }
}

fn test_tmux_transport(thread_id: &str) -> SelectedTransport {
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: "/tmp/collab-test-tmux.sock".into(),
        server_pid: 1,
        tmux_session_id: "$test".into(),
        pane_id: "%0".into(),
        pane_pid: 2,
        codex_session_id: Some(format!("session-{thread_id}")),
        codex_thread_id: Some(thread_id.into()),
    };
    SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some(endpoint.socket_path.clone()),
        namespace: Some(endpoint.tmux_session_id.clone()),
        session_id: endpoint.codex_session_id.clone(),
        thread_id: endpoint.codex_thread_id.clone(),
        tmux_endpoint: Some(endpoint),
        rex_endpoint: None,
        capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
        self_check: "test tmux transport".into(),
    }
}

fn fixture_peer_endpoint(server: &Server, thread_id: &str) -> crate::proto::TmuxEndpoint {
    fixture_peer_endpoint_with_presence(server, thread_id, false)
}

fn fixture_peer_endpoint_with_presence(
    server: &Server,
    thread_id: &str,
    online: bool,
) -> crate::proto::TmuxEndpoint {
    let thread_id = thread_id
        .strip_prefix('%')
        .map(|legacy_fixture| format!("thread-{legacy_fixture}"))
        .unwrap_or_else(|| thread_id.to_string());
    let fixture = if online { &ONLINE_TEST_TMUX } else { &REGISTERED_TEST_TMUX };
    let mut endpoint = fixture.with(|fixture| {
        let mut fixture = fixture.borrow_mut();
        // A test can create multiple owned roots. The participant script and
        // pane cwd belong to one root and must not outlive its fixture.
        if fixture
            .as_ref()
            .is_some_and(|(tmux, _)| tmux.owner_root != server.root)
        {
            *fixture = None;
        }
        let (tmux, endpoint_index) = fixture.get_or_insert_with(|| {
            let tmux = if online {
                IsolatedTmux::start_online(&server.root)
            } else {
                IsolatedTmux::start(&server.root)
            };
            (tmux, 0)
        });
        let endpoint = if *endpoint_index == 0 {
            tmux.endpoints().remove(0)
        } else {
            tmux.add_session()
        };
        *endpoint_index += 1;
        endpoint
    });
    endpoint.codex_session_id = Some(format!("session-{thread_id}"));
    endpoint.codex_thread_id = Some(thread_id);
    if online {
        wait_for_online_fixture(&endpoint);
    }
    endpoint
}

pub(super) fn wait_for_online_fixture(endpoint: &crate::proto::TmuxEndpoint) {
    let deadline = std::time::Instant::now() + FIXTURE_PANE_READY_TIMEOUT;
    while !crate::client::adapters::tmux::agent_is_online(endpoint) {
        assert!(std::time::Instant::now() < deadline, "test Codex foreground unavailable: {endpoint:?}");
        std::thread::sleep(FIXTURE_PANE_READY_POLL);
    }
}

/// Prior-state fixture for task, notification and readback tests. Explicit
/// registration effects use register_public and its real participant return.
pub(crate) fn register(server: &Server, id: &str, thread_id: &str) -> Resp {
    register_tmux(server, id, fixture_peer_endpoint(server, thread_id))
}

/// Delivery fixtures must prove the recipient is online. Presence-projection
/// fixtures keep `register` offline so a killed pane stays observably absent.
pub(crate) fn register_online(server: &Server, id: &str, thread_id: &str) -> Resp {
    let response = register_tmux(
        server, id, fixture_peer_endpoint_with_presence(server, thread_id, true),
    );
    if response.ok {
        let (project_scope, session_id) = {
            let state = server.state.lock().unwrap();
            let worker = &state.workers[id];
            let transport = worker.transport.as_ref().expect("tmux transport");
            (
                GlobalState::canonical_project_scope(&server.root)
                    .unwrap()
                    .as_str()
                    .to_owned(),
                transport.session_id.clone().unwrap_or_default(),
            )
        };
        server.commit(&[Event::AgentPresenceObserved {
            presence: name_state::AgentPresenceRecord {
                project_scope,
                worker_id: id.into(),
                session_id,
                presence: name_state::AgentPresence::Online,
                reason: "live peer fixture is online".into(),
                observed_ms: now_ms(),
            },
        }]);
    }
    response
}

pub(super) fn register_appserver_peer(server: &Server, id: &str, transport: &SelectedTransport) {
    let app = AppServerId::new("tui-default").unwrap();
    let cwd = server.root.display().to_string();
    let response = seed_committed_owner(
        server,
        id,
        &format!("token-{id}"),
        transport,
        &cwd,
        Some(app.clone()),
    );
    assert!(response.ok, "{response:?}");
    commit_current_thread_route_for_runtime(server, server, id, &cwd, Some(&app)).unwrap();
    restore_registered_peer_default_leases(server);
}

/// Seed a committed peer without starting a tmux server. Role projection and
/// scheduling contracts do not need a live pane.
pub(crate) fn register_synthetic_peer(server: &Server, id: &str, thread_id: &str) -> Resp {
    static PANE_SEQ: AtomicU64 = AtomicU64::new(0);
    let pane = PANE_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    register_tmux(
        server,
        id,
        crate::proto::TmuxEndpoint {
            socket_path: format!("/tmp/collab-synthetic-{}-{pane}.sock", std::process::id()),
            server_pid: 1,
            tmux_session_id: format!("${pane}"),
            pane_id: format!("%{pane}"),
            pane_pid: 2,
            codex_session_id: Some(format!("session-{thread_id}")),
            codex_thread_id: Some(thread_id.into()),
        },
    )
}

fn register_public(server: &Arc<Server>, id: &str, thread_id: &str) -> Resp {
    register_public_with_presence(server, id, thread_id, false)
}

fn register_public_with_presence(
    server: &Arc<Server>,
    id: &str,
    thread_id: &str,
    online: bool,
) -> Resp {
    let app = AppServerId::new("tui-default").unwrap();
    let prior = server
        .state
        .lock()
        .unwrap()
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| binding.agent_id.as_str() == id && binding.app_scope_id == app)
        .cloned();
    let mut endpoint = prior
        .as_ref()
        .and_then(|binding| binding.tmux_endpoint.clone())
        .unwrap_or_else(|| fixture_peer_endpoint_with_presence(server, thread_id, online));
    endpoint.codex_session_id = Some(format!("session-{thread_id}"));
    endpoint.codex_thread_id = Some(thread_id.into());
    let context = match prior {
        Some(binding) => ProjectContext::for_registered_route(
            &server.root,
            &crate::identity::RuntimeIdentity {
                agent_id: binding.agent_id,
                runtime_id: binding.runtime_id,
                appserver_id: binding.app_scope_id,
                binding_id: binding.binding_id,
                session_id: binding.session_id,
                native_thread_id: binding.native_thread_id,
            },
        )
        .unwrap(),
        None => ProjectContext::for_registered_root_with_app(&server.root, app).unwrap(),
    };
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    super::host_route_registry_tests::register_host_fixture(
        &manager,
        Some(context),
        Req::register(
            id.into(),
            format!("token-{id}"),
            server.root.display().to_string(),
            Some(TransportCandidates {
                desktop_bridge: None,
                appserver: None,
                dsh: None,
                rex: None,
                tmux: Some(crate::proto::TmuxCandidate {
                    endpoint,
                    cwd: server.root.display().to_string(),
                }),
            }),
        ),
    )
    .1
}

/// Model the provenance committed by an explicit managed-child Bind after
/// the child has already been registered in this test server.
pub(crate) fn bind_test_subagent_record(
    server: &Server,
    mut record: crate::subagent::Record,
) -> crate::subagent::Record {
    let state = server.state.lock().unwrap();
    let binding = state
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| {
            binding.agent_id.as_str() == record.peer
                && binding
                    .native_thread_id
                    .as_ref()
                    .map(|thread| thread.as_str())
                    == record.thread_id.as_deref()
        })
        .unwrap_or_else(|| panic!("registered child binding missing for {}", record.peer));
    record.create_operation_id = Some(format!("test-create-{}", record.id));
    record.binding_id = Some(binding.binding_id.as_str().to_owned());
    record
}

pub(crate) fn promote_master(server: &Server, worker_id: &str, approval: &str) {
    let binding = registered_binding(server, worker_id);
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let source = control
        .key_for_worker(&binding.project_scope, &binding.app_scope_id, worker_id)
        .unwrap()
        .expect("registered fixture has a current name");
    let value = control
        .slot(source)
        .unwrap()
        .active_binding()
        .unwrap()
        .clone();
    store
        .commit(binding_store::ControlChange::MoveName {
            target: binding_slot::IdentityBindingKey::new(
                binding.project_scope.clone(),
                binding.app_scope_id.clone(),
                "Jenny",
            ),
            value,
            source_names: vec![source.name.clone()],
            expected_generation: None,
        })
        .unwrap();
    let grant = global_state::MasterGrant::new(
        binding.project_scope.clone(),
        binding.app_scope_id,
        binding.agent_id,
        "project",
        "user",
        approval,
        binding.binding_id,
        now_ms(),
    )
    .unwrap();
    let mut state = server.state.lock().unwrap();
    let previous = state.name_occupancy(binding.project_scope.as_str(), "Jenny");
    let occupancy = name_state::NameOccupancy {
        name: "Jenny".into(),
        worker_id: worker_id.into(),
        session_id: binding.session_id.as_ref().unwrap().to_string(),
        occupancy_revision: previous.map_or(1, |old| old.occupancy_revision + 1),
        updated_ms: now_ms(),
        pane_anchor: None,
    };
    let mut events = vec![Event::GlobalMasterGranted { grant }];
    events.extend(
        name_resolution::build_name_resolution_events_with_rename_from(
            &state,
            binding.project_scope.as_str(),
            &occupancy,
            &[source.name.clone()],
            previous,
        ),
    );
    server.commit_locked_checked(&mut state, &events).unwrap();
}

fn send_command(root: &Path, id: &str) -> crate::proto::CommandEnvelope {
    use crate::identity::{AppServerId, BindingId, CommandId, OperationId};
    use crate::proto::CommandEnvelope;
    let app = AppServerId::new("tui-default").unwrap();
    let scope = crate::scope::RouteScope::for_registered_project(app, root).unwrap();
    CommandEnvelope::new(
        CommandId::new(format!("command-{id}")).unwrap(),
        OperationId::new(format!("operation-{id}")).unwrap(),
        BindingId::new(format!("binding-{id}")).unwrap(),
        scope,
        None,
        None,
        None,
        None,
    )
}

fn authenticated_send(root: &Path, from: &str, to: &str, subject: &str) -> Req {
    Req::Send {
        from: from.into(),
        worker_id: Some(from.into()),
        token: Some(format!("token-{from}")),
        command: Some(send_command(root, from)),
        to: to.into(),
        mtype: "notify".into(),
        subject: Some(subject.into()),
        body: "body".into(),
        in_reply_to: None,
        target_session: None,
        delivery: "immediate".into(),
    }
}

#[test]
fn master_idle_subscription_is_restricted_to_the_live_master_and_supported_intervals() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    register(&server, "worker", "%worker");
    promote_master(&server, "master", "user-approved");

    let worker = handle_notification_subscribe(
        &server,
        "worker".into(),
        "token-worker".into(),
        "master-idle".into(),
        Some("master-idle".into()),
        None,
        Vec::new(),
        Some(900_000),
        3,
        86_400,
    );
    assert!(!worker.ok);
    assert_eq!(
        worker.error.as_deref(),
        Some("master-idle subscription requires the current registered master")
    );

    let accepted = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "master-idle".into(),
        Some("master-idle".into()),
        None,
        Vec::new(),
        Some(3_600_000),
        3,
        86_400,
    );
    assert!(accepted.ok, "{}", accepted.error.unwrap_or_default());

    let invalid = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "master-idle".into(),
        Some("master-idle".into()),
        None,
        Vec::new(),
        Some(60_000),
        3,
        86_400,
    );
    assert!(!invalid.ok);
    assert_eq!(
        invalid.error.as_deref(),
        Some("master-idle interval must be project-patrol 1500000, compatibility 900000, or 3600000 ms")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_review_accept_leaves_integration_with_owner_and_sends_no_master_notice() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(
        &server,
        "master",
        "user approved master merge registration test",
    );
    assert!(create_task(&server, "owner", "task", "feature").ok);

    accept_task(&server, "owner", "task");

    let state = server.state.lock().unwrap();
    assert!(!state.pending_merges.contains_key("task"));
    assert_eq!(state.tasks["task"].owner, "owner");
    assert_eq!(state.tasks["task"].status, "accepted");
    let notified = state.msgs.values().any(|message| {
        message.to == "master"
            && message
                .subject
                .as_deref()
                .is_some_and(|subject| subject == "merge-pending:task")
    });
    assert!(!notified);
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_review_accept_does_not_depend_on_master_subscription() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(
        &server,
        "master",
        "user approved master merge registration test",
    );
    assert!(create_task(&server, "owner", "task", "feature").ok);

    // The owner's delivery must proceed even if the master has no lease.
    let drop_ids: Vec<String> = server
        .state
        .lock()
        .unwrap()
        .notification_subscriptions
        .values()
        .filter(|sub| sub.worker_id == "master" && sub.event == "direct-message")
        .map(|sub| sub.id.clone())
        .collect();
    {
        let mut state = server.state.lock().unwrap();
        for id in drop_ids {
            state.notification_subscriptions.remove(&id);
        }
    }
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate commit and gates passed".into()),
            Some("/tmp/candidate".into()),
        )
        .ok
    );
    let response = handle_task_review(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        true,
        false,
        "review passed".into(),
    );
    assert!(response.ok, "{}", response.error.unwrap_or_default());
    assert!(response.data.get("notification").is_none());
    assert!(response.data.get("repair_required").is_none());
    assert_eq!(server.state.lock().unwrap().tasks["task"].status, "accepted");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_integrates_main_without_a_delivery_candidate() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(
        &server,
        "master",
        "user approved candidate-free integration test",
    );
    initialize_main(&root);
    assert!(create_task(&server, "owner", "task", "feature").ok);
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    // Delivery no longer binds an automatic candidate. The supplied integration
    // commit must still be reachable from refs/heads/main, but the absence of a
    // delivery candidate is not a refusal.
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate commit and gates passed".into()),
            Some("/tmp/candidate-nonexistent".into()),
        )
        .ok
    );
    let response = handle_task_review(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        true,
        false,
        "review passed".into(),
    );
    assert!(response.ok, "{}", response.error.unwrap_or_default());
    assert_eq!(
        response.data["merge_pending"],
        serde_json::json!(false)
    );
    {
        let state = server.state.lock().unwrap();
        assert!(!state.pending_merges.contains_key("task"));
        assert_eq!(state.tasks["task"].status, "accepted");
    }

    // The owner may record the integration once the commit is on main.
    let head = current_head(&root);
    let integrated = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        head.clone(),
        "main head integration recorded".into(),
    );
    assert!(integrated.ok, "{}", integrated.error.unwrap_or_default());
    {
        let state = server.state.lock().unwrap();
        assert!(
            !state.pending_merges.contains_key("task"),
            "a valid integration resolves the pending merge"
        );
        assert_eq!(state.tasks["task"].status, "merged");
        assert_eq!(
            state.task_lifecycle["task"].integration_commit.as_deref(),
            Some(head.as_str())
        );
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn review_accept_merge_pending_field_reflects_registered_obligation() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate commit and gates passed".into()),
            Some("/tmp/candidate-nonexistent".into()),
        )
        .ok
    );
    // Masterless accept leaves the owner-local lifecycle intact: no daemon
    // pending merge exists, so merge_pending must be false even though accept
    // succeeded.
    let response = handle_task_review(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        true,
        false,
        "review passed".into(),
    );
    assert!(response.ok, "{}", response.error.unwrap_or_default());
    assert_eq!(response.data["merge_pending"], serde_json::json!(false));
    assert!(!server
        .state
        .lock()
        .unwrap()
        .pending_merges
        .contains_key("task"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_closes_only_after_recording_main_integration() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master close gate test");
    initialize_main(&root);
    assert!(create_task(&server, "owner", "task", "feature").ok);

    accept_task(&server, "owner", "task");

    let refused = handle_task_close(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        false,
        None,
    );
    assert!(!refused.ok, "{refused:?}");
    assert!(refused.error.unwrap().contains("must be merged by its owner"));

    let master_integrated = handle_task_integrated(
        &server,
        "master".into(),
        "token-master".into(),
        "task".into(),
        current_head(&root),
        "master override attempt".into(),
    );
    assert!(
        !master_integrated.ok,
        "an unrelated master must not record the owner's integration"
    );

    let head = current_head(&root);
    assert!(
        handle_task_integrated(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            head,
            "main merged".into(),
        )
        .ok
    );
    {
        let state = server.state.lock().unwrap();
        assert!(
            !state.pending_merges.contains_key("task"),
            "task integrated must resolve the pending merge"
        );
        assert_eq!(state.tasks["task"].status, "merged");
    }
    let closed = handle_task_close(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn integrated_supersedes_stale_merge_pending_notice() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master merge notice test");
    initialize_main(&root);
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");

    let notice_id = "legacy-merge-notice";
    server.commit(&[
        Event::MergeRequested {
            request: crate::server::state::PendingMerge {
                task_id: "task".into(),
                owner: "owner".into(),
                requested_by: "master".into(),
                requested_ms: now_ms(),
                candidate_commit: None,
            },
        },
        Event::Sent {
            msg: Message {
                id: notice_id.into(),
                from: "collab-server".into(),
                to: "master".into(),
                mtype: "notify".into(),
                subject: Some("merge-pending:task".into()),
                body: "historical merge obligation".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
    ]);

    let head = current_head(&root);
    assert!(
        handle_task_integrated(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            head,
            "merged onto main".into(),
        )
        .ok
    );

    let state = server.state.lock().unwrap();
    assert!(!state.pending_merges.contains_key("task"));
    assert_eq!(
        state.msgs[notice_id].state, "superseded",
        "a resolved obligation must not remain readable as an actionable P0 notice"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_review_accept_succeeds_when_master_presence_is_unknown() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master presence test");
    assert!(create_task(&server, "owner", "task", "feature").ok);

    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate commit and gates passed".into()),
            Some("/tmp/candidate".into()),
        )
        .ok
    );

    // Closing the fixture sessions leaves the master's transport Unknown.
    // The owner's authorized transition is independent of that transport.
    stop_registered_test_tmux_server();

    let response = handle_task_review(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        true,
        false,
        "review passed".into(),
    );
    assert!(response.ok, "{response:?}");
    let state = server.state.lock().unwrap();
    assert!(!state.pending_merges.contains_key("task"));
    assert_eq!(state.tasks["task"].status, "accepted");
    assert!(
        !state.msgs.values().any(|message| {
            message.to == "master" && message.subject.as_deref() == Some("merge-pending:task")
        }),
        "the owner's review must not wake the current grant holder for integration"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}
