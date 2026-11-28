#![cfg(all(unix, debug_assertions, feature = "lifecycle-test-hooks"))]

use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const WORKER: &str = "collab-restore-fixture";
const BINDING: &str = "binding-collab-restore-fixture";
const SESSION: &str = "session:30000000-0000-0000-0000-000000000001";
const BLOCK: &str = "block:30000000-0000-0000-0000-000000000002";
const WINDOW: &str = "window:30000000-0000-0000-0000-000000000003";
static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
    state: PathBuf,
    project: PathBuf,
    create_lifetime: Mutex<Option<CreateLifetime>>,
    cleanup_receipt: Option<PathBuf>,
    outer_reaped: PathBuf,
    preflight_pid: Mutex<Option<u32>>,
}

struct CreateLifetime {
    supervisor: Option<JoinHandle<()>>,
    pid_path: PathBuf,
    eof_path: PathBuf,
    outer_pid: u32,
    outer_reaped: PathBuf,
}

struct OwnedChild {
    child: Option<std::process::Child>,
    reaped_receipt: Option<PathBuf>,
}

impl OwnedChild {
    fn new(child: std::process::Child, reaped_receipt: Option<PathBuf>) -> Self {
        Self {
            child: Some(child),
            reaped_receipt,
        }
    }

    fn id(&self) -> u32 {
        self.child.as_ref().expect("owned child is active").id()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child
            .as_mut()
            .expect("owned child is active")
            .try_wait()
    }

    fn wait_with_output(
        mut self,
        deadline: Instant,
        stdout_path: &std::path::Path,
        stderr_path: &std::path::Path,
    ) -> std::io::Result<Output> {
        let mut child = self.child.take().expect("owned child is active");
        loop {
            if let Some(status) = child.try_wait()? {
                if let Some(receipt) = &self.reaped_receipt {
                    fs::write(receipt, b"reaped").ok();
                }
                return Ok(Output {
                    status,
                    stdout: fs::read(stdout_path)?,
                    stderr: fs::read(stderr_path)?,
                });
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let status = child.wait()?;
                if let Some(receipt) = &self.reaped_receipt {
                    fs::write(receipt, b"reaped-after-timeout").ok();
                }
                return Ok(Output {
                    status,
                    stdout: fs::read(stdout_path)?,
                    stderr: fs::read(stderr_path)?,
                });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
        }
        let _ = child.wait();
        if let Some(receipt) = &self.reaped_receipt {
            fs::write(receipt, b"reaped-on-drop").ok();
        }
    }
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "collab-restore-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let state = root.join("state");
        let project = root.join("project");
        fs::create_dir(&state).unwrap();
        fs::create_dir(&project).unwrap();
        let cli = root.join("rex");
        fs::write(&cli, include_str!("fixtures/restore/rex.py")).unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
        let outer_reaped = root.join("outer-reaped");
        Self {
            root,
            state,
            project,
            create_lifetime: Mutex::new(None),
            cleanup_receipt: None,
            outer_reaped,
            preflight_pid: Mutex::new(None),
        }
    }

    fn map(&self) -> Value {
        serde_json::from_str(
            &include_str!("fixtures/restore/identity-map.json")
                .replace("@PROJECT@", self.project.to_str().unwrap()),
        )
        .unwrap()
    }

    fn run(
        &self,
        map: Value,
        mode: &str,
        worker: &str,
        app: &str,
        binding: &str,
        generation: &str,
        approve: bool,
    ) -> (i32, Value, Vec<Value>) {
        let map_path = self.state.join("identity-map.json");
        let original = serde_json::to_vec(&map).unwrap();
        fs::write(&map_path, &original).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_collab"));
        command
            .current_dir(&self.project)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("COLLAB_STATE_DIR", &self.state)
            .env("COLLAB_TEST_REX_CLI", self.root.join("rex"))
            .env("RESTORE_FIXTURE_MODE", mode)
            .env("RESTORE_FIXTURE_ROOT", &self.state)
            .args([
                "restore",
                "--worker",
                worker,
                "--app",
                app,
                "--binding-id",
                binding,
                "--generation",
                generation,
                "--session-label",
                "restore-fixture",
                "--operation-id",
                "fixture-operation",
            ]);
        if approve {
            command.arg("--approve");
        }
        let stdout_path = self.root.join("restore.stdout");
        let stderr_path = self.root.join("restore.stderr");
        command
            .stdout(fs::File::create(&stdout_path).unwrap())
            .stderr(fs::File::create(&stderr_path).unwrap())
            .process_group(0);
        let child = command.spawn().unwrap();
        let owned = OwnedChild::new(child, None);
        let output = owned
            .wait_with_output(
                Instant::now() + Duration::from_secs(60),
                &stdout_path,
                &stderr_path,
            )
            .unwrap();
        let result: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!("{error}: {}", String::from_utf8_lossy(&output.stderr));
        });
        if mode != "changed_binding" {
            assert_eq!(fs::read(map_path).unwrap(), original);
        }
        let receipts = fs::read_to_string(self.state.join("receipts.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect::<Vec<Value>>();
        eprintln!("{mode}: result={result} receipts={}", json!(receipts));
        (output.status.code().unwrap(), result, receipts)
    }

    fn has_carrier(&self) -> bool {
        self.state.join("active-session").exists()
    }

    fn spawn_interrupt_restore(&self, mode: &str) -> (OwnedChild, PathBuf, PathBuf) {
        self.spawn_interrupt_restore_with_signal_barrier(mode, None)
    }

    fn spawn_interrupt_restore_with_signal_barrier(
        &self,
        mode: &str,
        signal_barrier: Option<&std::path::Path>,
    ) -> (OwnedChild, PathBuf, PathBuf) {
        let map = self.map();
        let map_path = self.state.join("identity-map.json");
        fs::write(&map_path, serde_json::to_vec(&map).unwrap()).unwrap();
        let (lifecycle_reader, lifecycle_writer) = UnixStream::pair().unwrap();
        let lifecycle_fd = lifecycle_writer.as_raw_fd();
        let flags = unsafe { libc::fcntl(lifecycle_fd, libc::F_GETFD) };
        assert!(flags >= 0, "read fixture lifetime fd flags");
        assert_eq!(
            unsafe { libc::fcntl(lifecycle_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) },
            0,
            "make fixture lifetime fd inheritable"
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_collab"));
        command
            .current_dir(&self.project)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("COLLAB_STATE_DIR", &self.state)
            .env("COLLAB_TEST_REX_CLI", self.root.join("rex"))
            .env("RESTORE_FIXTURE_MODE", mode)
            .env("RESTORE_FIXTURE_ROOT", &self.state)
            .env("COLLAB_TEST_REX_LIFETIME_FD", lifecycle_fd.to_string());
        if let Some(signal_barrier) = signal_barrier {
            command.env(
                "COLLAB_TEST_RESTORE_SIGNAL_CONSUMER_BARRIER",
                signal_barrier,
            );
        }
        command.args([
            "restore",
            "--worker",
            WORKER,
            "--app",
            "appserver-cli",
            "--binding-id",
            BINDING,
            "--generation",
            "1",
            "--session-label",
            "restore-fixture",
            "--operation-id",
            "fixture-operation",
            "--approve",
        ]);
        let stdout_path = self.root.join("interrupt.stdout");
        let stderr_path = self.root.join("interrupt.stderr");
        command
            .stdout(fs::File::create(&stdout_path).unwrap())
            .stderr(fs::File::create(&stderr_path).unwrap())
            .process_group(0);
        let child = command.spawn().unwrap();
        let outer_pid = child.id();
        drop(lifecycle_writer);
        let pid_path = self.state.join("created-child-owned-pid");
        let eof_path = self.state.join("create-lifetime-eof");
        let withhold_ack = matches!(mode, "pre_marker_timeout" | "pre_marker_unwind");
        let close_ack = mode == "pre_marker_closed_ack";
        let invalid_ack = mode == "pre_marker_invalid_ack";
        let supervisor = std::thread::spawn({
            let pid_path = pid_path.clone();
            let eof_path = eof_path.clone();
            move || {
                let mut lifecycle_reader = lifecycle_reader;
                let mut pid_bytes = [0_u8; std::mem::size_of::<libc::pid_t>()];
                if std::io::Read::read_exact(&mut lifecycle_reader, &mut pid_bytes).is_err() {
                    let _ = fs::write(&eof_path, b"eof-before-announcement");
                    return;
                }
                let pid = libc::pid_t::from_ne_bytes(pid_bytes);
                let _ = fs::write(&pid_path, pid.to_string());
                let _ = fs::write(eof_path.with_extension("announced"), b"owned");
                if close_ack {
                    lifecycle_reader
                        .shutdown(std::net::Shutdown::Write)
                        .unwrap();
                } else if invalid_ack {
                    std::io::Write::write_all(&mut lifecycle_reader, b"x").unwrap();
                } else if !withhold_ack {
                    let _ = std::io::Write::write_all(&mut lifecycle_reader, b"a");
                }
                let mut sink = [0_u8; 64];
                loop {
                    match std::io::Read::read(&mut lifecycle_reader, &mut sink) {
                        Ok(0) => {
                            let _ = fs::write(&eof_path, b"eof");
                            break;
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
            }
        });
        *self.create_lifetime.lock().unwrap() = Some(CreateLifetime {
            supervisor: Some(supervisor),
            pid_path,
            eof_path,
            outer_pid,
            outer_reaped: self.outer_reaped.clone(),
        });
        (
            OwnedChild::new(child, Some(self.outer_reaped.clone())),
            stdout_path,
            stderr_path,
        )
    }

    fn interrupt_preflight_with_withheld_consumer(&self, mode: &str) -> (i32, Value, Vec<Value>) {
        let barrier = self.root.join("withheld-signal-consumer");
        let (mut child, stdout_path, stderr_path) =
            self.spawn_interrupt_restore_with_signal_barrier(mode, Some(&barrier));
        let ready = barrier.with_extension("ready");
        let deadline = Instant::now() + Duration::from_secs(15);
        while !ready.exists() && Instant::now() < deadline {
            assert!(
                child.try_wait().unwrap().is_none(),
                "restore exited before the signal consumer became ready"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(ready.exists(), "signal consumer readiness timed out");
        let in_flight = self.state.join("preflight-inflight");
        while !in_flight.exists() && Instant::now() < deadline {
            assert!(
                child.try_wait().unwrap().is_none(),
                "restore exited before the preflight read was in flight"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(in_flight.exists(), "preflight read was not in flight");
        let rex_pid = wait_for_pid(
            &self.state.join("preflight-child-pid"),
            Instant::now() + Duration::from_secs(5),
        )
        .expect("fixture records the exact preflight child PID");
        *self.preflight_pid.lock().unwrap() = Some(rex_pid);
        assert_eq!(
            unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) },
            0
        );
        assert_eq!(
            unsafe { libc::kill(rex_pid as libc::pid_t, libc::SIGINT) },
            0
        );
        assert!(
            !barrier.exists(),
            "the fixture must withhold the asynchronous flag publisher"
        );
        let output = child
            .wait_with_output(
                Instant::now() + Duration::from_secs(15),
                &stdout_path,
                &stderr_path,
            )
            .unwrap();
        let result: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
        let receipts = fs::read_to_string(self.state.join("receipts.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect::<Vec<Value>>();
        eprintln!("{mode}: result={result} receipts={}", json!(receipts));
        (output.status.code().unwrap(), result, receipts)
    }

    fn interrupt_during_create(&self, mode: &str, signal_group: bool) -> (i32, Value, Vec<Value>) {
        let (mut child, stdout_path, stderr_path) = self.spawn_interrupt_restore(mode);
        let preflight = mode == "cancel_preflight" || mode.starts_with("cancel_preflight_");
        let in_flight = self.state.join(if preflight {
            "preflight-inflight"
        } else {
            "create-inflight"
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if in_flight.exists() {
                break;
            }
            if let Some(status) = child.try_wait().unwrap() {
                let output = child
                    .wait_with_output(
                        Instant::now() + Duration::from_secs(5),
                        &stdout_path,
                        &stderr_path,
                    )
                    .unwrap();
                panic!(
                    "restore exited before the in-flight marker ({status}): {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            if Instant::now() >= deadline {
                let _ = signal_group;
                let output = child
                    .wait_with_output(
                        Instant::now() + Duration::from_secs(5),
                        &stdout_path,
                        &stderr_path,
                    )
                    .unwrap();
                panic!(
                    "fixture {mode} did not reach its in-flight marker: stdout={} stderr={}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) },
            0,
            "failed to interrupt restore"
        );
        if signal_group && preflight {
            let rex_pid_path = self.state.join("preflight-child-pid");
            let rex_pid = wait_for_pid(&rex_pid_path, Instant::now() + Duration::from_secs(5))
                .expect("fixture supervisor records exact preflight child PID");
            *self.preflight_pid.lock().unwrap() = Some(rex_pid);
            assert_eq!(
                unsafe { libc::kill(rex_pid as libc::pid_t, libc::SIGINT) },
                0
            );
        }
        if !preflight {
            fs::write(self.state.join("release-create"), b"signal-delivered")
                .expect("failed to release fixture create response");
        }
        let output = child
            .wait_with_output(
                Instant::now() + Duration::from_secs(15),
                &stdout_path,
                &stderr_path,
            )
            .unwrap();
        let result: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
        let receipts = fs::read_to_string(self.state.join("receipts.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect::<Vec<Value>>();
        eprintln!("{mode}: result={result} receipts={}", json!(receipts));
        (output.status.code().unwrap(), result, receipts)
    }

    fn reclaim_fixture_create_child(&self) -> bool {
        if let Some(pid) = self.preflight_pid.lock().unwrap().take() {
            if pid <= 1 {
                return false;
            }
            if process_is_alive(pid) {
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
                let deadline = Instant::now() + Duration::from_secs(5);
                while process_is_alive(pid) && deadline > Instant::now() {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            if process_is_alive(pid) {
                return false;
            }
        }
        let Some(mut lifetime) = self.create_lifetime.lock().unwrap().take() else {
            return true;
        };
        if !lifetime.outer_reaped.exists() || process_is_alive(lifetime.outer_pid) {
            eprintln!(
                "fixture cleanup unconfirmed: outer child {} reaped={} alive={}",
                lifetime.outer_pid,
                lifetime.outer_reaped.exists(),
                process_is_alive(lifetime.outer_pid)
            );
            return false;
        }
        let _ = fs::write(self.state.join("release-create"), b"fixture-unwind-cleanup");
        let announce_deadline = Instant::now() + Duration::from_secs(5);
        while !lifetime.pid_path.exists()
            && !lifetime.eof_path.exists()
            && Instant::now() < announce_deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid = wait_for_pid(&lifetime.pid_path, announce_deadline);
        if let Some(pid) = pid {
            if pid <= 1 {
                eprintln!("fixture cleanup unconfirmed: invalid announced child PID {pid}");
                return false;
            }
            if process_is_alive(pid) {
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
                let kill_deadline = Instant::now() + Duration::from_secs(5);
                while process_is_alive(pid) && Instant::now() < kill_deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            if process_is_alive(pid) {
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
                let kill_deadline = Instant::now() + Duration::from_secs(2);
                while process_is_alive(pid) && Instant::now() < kill_deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            if process_is_alive(pid) {
                eprintln!("fixture cleanup unconfirmed: announced child {pid} remains alive");
                return false;
            }
        }
        let eof_deadline = Instant::now() + Duration::from_secs(5);
        while !lifetime.eof_path.exists() && Instant::now() < eof_deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !lifetime.eof_path.exists() {
            eprintln!("fixture cleanup unconfirmed: create lifetime channel did not reach EOF");
            return false;
        }
        if let Some(supervisor) = lifetime.supervisor.take() {
            if supervisor.join().is_err() {
                return false;
            }
        }
        let eof = fs::read_to_string(&lifetime.eof_path).unwrap_or_default();
        let Some(pid) = pid else {
            if eof != "eof-before-announcement" {
                eprintln!("fixture cleanup unconfirmed: child announcement is missing");
                return false;
            }
            if let Some(receipt) = &self.cleanup_receipt {
                let _ = fs::write(receipt, format!("pid=none eof={eof} root_removed=true\n"));
            }
            return true;
        };
        if eof != "eof" {
            eprintln!("fixture cleanup unconfirmed: unexpected lifetime evidence {eof:?}");
            return false;
        }
        if let Some(receipt) = &self.cleanup_receipt {
            let _ = fs::write(receipt, format!("pid={pid} eof={eof} root_removed=true\n"));
        }
        eprintln!(
            "fixture cleanup confirmed: outer={} create={:?} lifetime={:?} removing {}",
            lifetime.outer_pid,
            pid,
            eof,
            self.root.display()
        );
        true
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.reclaim_fixture_create_child() {
            eprintln!(
                "fixture cleanup unconfirmed; preserving {}",
                self.root.display()
            );
            return;
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn process_is_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

fn wait_for_pid(path: &std::path::Path, deadline: Instant) -> Option<u32> {
    loop {
        if let Ok(text) = fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse::<u32>() {
                return Some(pid);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn process_belongs_to_fixture(pid: u32, root: &std::path::Path) -> bool {
    Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()
        .is_some_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains(root.to_string_lossy().as_ref())
        })
}

fn terminate_owned_fixture_child(pid: u32, _root: &std::path::Path) {
    if pid > 1 && process_is_alive(pid) {
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(5);
        while process_is_alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn methods(receipts: &[Value]) -> Vec<&str> {
    receipts
        .iter()
        .map(|call| call["method"].as_str().unwrap())
        .collect()
}

#[test]
fn restore_public_reads_authoritative_binding_and_verifies_new_carrier() {
    let f = Fixture::new();
    let (code, result, receipts) =
        f.run(f.map(), "ok", WORKER, "appserver-cli", BINDING, "1", true);
    assert_eq!(code, 0, "{result}");
    assert_eq!(result["ok"], true);
    let result = &result["result"];
    assert_eq!(result["restored_session_id"], SESSION);
    assert_eq!(result["restored_window_id"], WINDOW);
    assert_eq!(result["restored_block_id"], BLOCK);
    assert_eq!(result["plan"]["binding_id"], BINDING);
    assert_eq!(result["plan"]["binding_generation"], 1);
    assert_eq!(result["plan"]["operation_id"], "fixture-operation");
    assert_eq!(result["live_resume"], "unverified");
    assert_eq!(
        result["resume"],
        json!({"profile":"gcm","codex_session_id":"10000000-0000-0000-0000-000000000001",
            "target_block_id":BLOCK,"canonical_cwd":f.project}),
    );
    assert_eq!(result["create_receipt"], receipts[2]["response"]);
    let mut expected = result["plan"]["saved_layout"].clone();
    expected["session_id"] = json!(SESSION);
    expected["window_id"] = json!(WINDOW);
    expected["block_id"] = json!(BLOCK);
    expected["layer_id"] = json!("layer:30000000-0000-0000-0000-000000000004");
    expected["session_revision"] = json!(1);
    expected["layout"]["block_id"] = json!(BLOCK);
    assert_eq!(result["layout_verification"], expected);
    assert_eq!(
        methods(&receipts),
        [
            "server.status",
            "session.list",
            "session.create",
            "session.view",
            "server.status"
        ]
    );
    assert!(f.has_carrier());
}

#[test]
fn restore_public_rejects_bad_inputs_before_mutation() {
    for (mode, detail, worker, app, binding, generation) in [
        (
            "no_profile",
            "requires a typed Codex profile",
            WORKER,
            "appserver-cli",
            BINDING,
            "1",
        ),
        (
            "no_session",
            "requires an agent session",
            WORKER,
            "appserver-cli",
            BINDING,
            "1",
        ),
        (
            "no_layout",
            "RESTORE_LAYOUT_REQUIRED",
            WORKER,
            "appserver-cli",
            BINDING,
            "1",
        ),
        (
            "ambiguous",
            "target block exactly once",
            WORKER,
            "appserver-cli",
            BINDING,
            "1",
        ),
        (
            "split",
            "RESTORE_LAYOUT_INCOMPLETE_OR_UNSUPPORTED",
            WORKER,
            "appserver-cli",
            BINDING,
            "1",
        ),
        (
            "wrong_project",
            "RESTORE_BINDING_NOT_FOUND",
            WORKER,
            "appserver-cli",
            BINDING,
            "1",
        ),
        (
            "wrong_worker",
            "RESTORE_BINDING_NOT_FOUND",
            "other",
            "appserver-cli",
            BINDING,
            "1",
        ),
        (
            "wrong_app",
            "RESTORE_BINDING_NOT_FOUND",
            WORKER,
            "other",
            BINDING,
            "1",
        ),
        (
            "wrong_binding",
            "RESTORE_BINDING_MISMATCH",
            WORKER,
            "appserver-cli",
            "other",
            "1",
        ),
        (
            "stale",
            "RESTORE_BINDING_MISMATCH",
            WORKER,
            "appserver-cli",
            BINDING,
            "2",
        ),
        (
            "existing",
            "RESTORE_TARGET_ALREADY_EXISTS",
            WORKER,
            "appserver-cli",
            BINDING,
            "1",
        ),
        (
            "unapproved",
            "RESTORE_APPROVAL_REQUIRED",
            WORKER,
            "appserver-cli",
            BINDING,
            "1",
        ),
    ] {
        let f = Fixture::new();
        let mut map = f.map();
        if mode == "wrong_project" {
            let other = f.root.join("other-project");
            fs::create_dir(&other).unwrap();
            map = serde_json::from_str(
                &include_str!("fixtures/restore/identity-map.json")
                    .replace("@PROJECT@", other.to_str().unwrap()),
            )
            .unwrap();
        }
        let current = &mut map["slots"][0]["current"];
        match mode {
            "no_profile" => {
                current["host"].as_object_mut().unwrap().remove("profile");
            }
            "no_session" => {
                current["binding"]
                    .as_object_mut()
                    .unwrap()
                    .remove("session_id");
            }
            "no_layout" => {
                current.as_object_mut().unwrap().remove("rex_layout");
            }
            "ambiguous" | "split" => {
                let block = current["rex_layout"]["block_id"].clone();
                let other = if mode == "ambiguous" {
                    block.clone()
                } else {
                    json!("block:20000000-0000-0000-0000-000000000099")
                };
                current["rex_layout"]["layout"] = json!({
                    "rect": [0,0,1,1],
                    "split": {"split_id": "split:test", "direction":"horizontal","ratio":0.5,
                        "before":{"rect":[0,0,0.5,1],"block_id":block},
                        "after":{"rect":[0.5,0,0.5,1],"block_id":other}}
                });
            }
            _ => {}
        }
        let (code, result, receipts) = f.run(
            map,
            mode,
            worker,
            app,
            binding,
            generation,
            mode != "unapproved",
        );
        assert_eq!(code, 2, "{mode}: {result}");
        assert!(
            result["error"]["detail"].as_str().unwrap().contains(detail),
            "{mode}: {result}"
        );
        assert_eq!(result["ok"], false);
        assert!(!f.has_carrier());
        if mode == "existing" {
            assert_eq!(methods(&receipts), ["server.status", "session.list"]);
        } else {
            assert!(receipts.is_empty());
        }
    }
}

#[test]
fn restore_public_cleans_failed_carrier_and_never_emits_partial_success() {
    for mode in [
        "atomic_failure",
        "bad_receipt",
        "bad_view",
        "changed_binding",
    ] {
        let f = Fixture::new();
        let (code, result, receipts) =
            f.run(f.map(), mode, WORKER, "appserver-cli", BINDING, "1", true);
        assert_eq!(code, 2, "{result}");
        assert_eq!(result["ok"], false);
        assert!(result.get("result").is_none());
        assert!(!f.has_carrier(), "{mode}: {result}");
        if mode == "atomic_failure" {
            assert_eq!(
                methods(&receipts),
                ["server.status", "session.list", "session.create"]
            );
            assert_eq!(result["error"]["stage"], "session.create");
            assert_eq!(
                result["error"]["create_outcome"]["outcome"],
                "confirmed_rejected"
            );
            assert_eq!(
                result["error"]["create_outcome"]["native_code"],
                "bad_request"
            );
            assert_eq!(
                result["error"]["create_context"]["session_label"],
                "restore-fixture"
            );
            assert!(result["error"].get("cleanup").is_none());
            assert!(!f.has_carrier());
        } else {
            assert_eq!(methods(&receipts).last(), Some(&"kill"));
            assert_eq!(
                result["error"]["cleanup"],
                json!({"outcome":"closed","session_id": SESSION})
            );
            assert_eq!(result["error"]["create_receipt"], receipts[2]["response"]);
        }
    }
}

#[test]
fn restore_public_preserves_unknown_create_outcome_and_refuses_unsafe_retry() {
    let f = Fixture::new();
    let (code, result, receipts) = f.run(
        f.map(),
        "create_response_lost",
        WORKER,
        "appserver-cli",
        BINDING,
        "1",
        true,
    );
    assert_eq!(code, 2, "{result}");
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["stage"], "session.create");
    assert_eq!(result["error"]["create_outcome"]["outcome"], "unknown");
    assert_eq!(
        result["error"]["create_context"],
        json!({
            "server":"unix:///fixture/rex.sock",
            "session_label":"restore-fixture",
            "operation_id":"fixture-operation"
        })
    );
    assert!(result["error"].get("cleanup").is_none());
    assert!(result["error"].get("create_receipt").is_none());
    assert_eq!(
        methods(&receipts),
        ["server.status", "session.list", "session.create"]
    );
    assert!(
        f.has_carrier(),
        "fixture must retain the unconfirmed session"
    );
    assert!(result.get("result").is_none(), "{result}");
    assert!(
        !result.to_string().contains("restored_session_id"),
        "{result}"
    );
    assert!(!methods(&receipts).contains(&"kill"), "{receipts:?}");

    let (retry_code, retry, retry_receipts) = f.run(
        f.map(),
        "create_response_lost",
        WORKER,
        "appserver-cli",
        BINDING,
        "1",
        true,
    );
    assert_eq!(retry_code, 2, "{retry}");
    assert_eq!(retry["error"]["stage"], "session.list");
    assert!(
        retry["error"]["detail"]
            .as_str()
            .unwrap()
            .contains("RESTORE_TARGET_ALREADY_EXISTS"),
        "{retry}"
    );
    assert_eq!(
        methods(&retry_receipts)[3..],
        ["server.status", "session.list"]
    );
    assert!(!methods(&retry_receipts).contains(&"kill"));
    assert!(f.has_carrier(), "retry must not guess ownership or kill");
}

#[test]
fn restore_public_reports_cleanup_unconfirmed_after_post_create_verification_failure() {
    let f = Fixture::new();
    let (code, result, receipts) = f.run(
        f.map(),
        "bad_view_kill_failure",
        WORKER,
        "appserver-cli",
        BINDING,
        "1",
        true,
    );
    assert_eq!(code, 2, "{result}");
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["stage"], "session.view");
    let cleanup = &result["error"]["cleanup"];
    assert_eq!(cleanup["outcome"], "unconfirmed");
    assert_eq!(cleanup["session_id"], SESSION);
    assert!(
        cleanup["detail"].as_str().unwrap().contains("kill"),
        "{result}"
    );
    assert!(methods(&receipts).contains(&"kill"), "{receipts:?}");
    assert!(
        f.has_carrier(),
        "unconfirmed cleanup must retain the exact session"
    );
}

#[test]
fn restore_public_cancellation_is_typed_and_routes_through_exact_id_cleanup() {
    for (mode, expected_cleanup, expected_carrier) in [
        ("cancel_after_create", "closed", false),
        ("cancel_after_create_kill_failure", "unconfirmed", true),
    ] {
        let f = Fixture::new();
        let (code, result, receipts) = f.interrupt_during_create(mode, false);
        assert_eq!(code, 2, "{mode}: {result}");
        assert_eq!(result["ok"], false, "{mode}: {result}");
        assert!(result.get("result").is_none(), "{mode}: {result}");
        assert_eq!(result["error"]["stage"], "cancelled", "{mode}: {result}");
        assert_eq!(
            result["error"]["create_receipt"], receipts[2]["response"],
            "{mode}: {result}"
        );
        assert_eq!(
            result["error"]["cleanup"]["outcome"], expected_cleanup,
            "{mode}: {result}"
        );
        assert_eq!(
            result["error"]["cleanup"]["session_id"], SESSION,
            "{mode}: exact cleanup must use the returned session ID"
        );
        assert_eq!(
            methods(&receipts),
            [
                "server.status",
                "session.list",
                "session.create",
                "server.status",
                "kill"
            ],
            "{mode}: {result}"
        );
        assert_eq!(f.has_carrier(), expected_carrier, "{mode}: {result}");
    }
}

#[test]
fn restore_public_foreground_sigint_during_preflight_is_typed_without_mutation() {
    for (mode, completed_reads) in [
        ("cancel_preflight", vec![]),
        ("cancel_preflight_status", vec![]),
        ("cancel_preflight_list", vec!["server.status"]),
    ] {
        let f = Fixture::new();
        let before = serde_json::to_vec(&f.map()).unwrap();
        let (code, result, receipts) = f.interrupt_during_create(mode, true);
        assert_eq!(code, 2, "{mode}: {result}");
        assert_eq!(result["error"]["stage"], "cancelled", "{mode}: {result}");
        assert!(
            result.to_string().contains("RESTORE_CANCELLED"),
            "{mode}: {result}"
        );
        assert!(result.get("result").is_none(), "{mode}: {result}");
        assert_eq!(methods(&receipts), completed_reads, "{mode}: {receipts:?}");
        assert!(!f.has_carrier(), "{mode}");
        assert_eq!(fs::read(f.state.join("identity-map.json")).unwrap(), before);
    }
}

#[test]
fn restore_public_status_sigint_wins_when_async_consumer_is_withheld() {
    foreground_sigint_wins_when_async_consumer_is_withheld("cancel_preflight_status", vec![]);
}

#[test]
fn restore_public_list_sigint_wins_when_async_consumer_is_withheld() {
    foreground_sigint_wins_when_async_consumer_is_withheld(
        "cancel_preflight_list",
        vec!["server.status"],
    );
}

fn foreground_sigint_wins_when_async_consumer_is_withheld(mode: &str, completed_reads: Vec<&str>) {
    let f = Fixture::new();
    let before = serde_json::to_vec(&f.map()).unwrap();
    let (code, result, receipts) = f.interrupt_preflight_with_withheld_consumer(mode);
    assert_eq!(code, 2, "{mode}: {result}");
    assert_eq!(result["error"]["stage"], "cancelled", "{mode}: {result}");
    assert!(result.get("result").is_none(), "{mode}: {result}");
    assert!(result["error"]["cleanup"].is_null(), "{mode}: {result}");
    assert_eq!(methods(&receipts), completed_reads, "{mode}: {receipts:?}");
    assert!(
        !methods(&receipts).contains(&"session.create"),
        "{mode}: {receipts:?}"
    );
    assert!(
        !methods(&receipts).contains(&"kill"),
        "{mode}: {receipts:?}"
    );
    assert_eq!(fs::read(f.state.join("identity-map.json")).unwrap(), before);
    assert!(!f.has_carrier(), "{mode}");
}

#[test]
fn restore_public_preflight_native_errors_without_cancellation_are_preserved() {
    for (mode, stage, expected_calls) in [
        ("status_failure", "server.status", vec!["server.status"]),
        (
            "list_failure",
            "session.list",
            vec!["server.status", "session.list"],
        ),
    ] {
        let f = Fixture::new();
        let (code, result, receipts) =
            f.run(f.map(), mode, WORKER, "appserver-cli", BINDING, "1", true);
        assert_eq!(code, 2, "{mode}: {result}");
        assert_eq!(result["error"]["stage"], stage, "{mode}: {result}");
        assert!(
            !result.to_string().contains("RESTORE_CANCELLED"),
            "{mode}: {result}"
        );
        assert!(result.get("result").is_none(), "{mode}: {result}");
        assert_eq!(methods(&receipts), expected_calls, "{mode}: {receipts:?}");
        assert!(!f.has_carrier(), "{mode}");
    }
}

#[test]
fn restore_public_foreground_sigint_retains_create_receipt_for_exact_cleanup() {
    let f = Fixture::new();
    let (code, result, receipts) = f.interrupt_during_create("cancel_after_create", false);
    assert_eq!(code, 2, "{result}");
    assert_eq!(result["ok"], false, "{result}");
    assert!(result.get("result").is_none(), "{result}");
    assert_eq!(result["error"]["stage"], "cancelled", "{result}");
    assert_eq!(
        result["error"]["create_receipt"], receipts[2]["response"],
        "the isolated Rex child must return its create receipt: {result}"
    );
    assert_eq!(result["error"]["cleanup"]["outcome"], "closed", "{result}");
    assert_eq!(
        result["error"]["cleanup"]["session_id"], SESSION,
        "cleanup must use the exact returned session ID: {result}"
    );
    assert_eq!(
        methods(&receipts),
        [
            "server.status",
            "session.list",
            "session.create",
            "server.status",
            "kill"
        ],
        "{result}"
    );
    assert!(!f.has_carrier(), "{result}");
}

#[test]
fn restore_public_pending_cancel_preserves_unusable_create_reply_without_kill() {
    let f = Fixture::new();
    let (code, result, receipts) = f.interrupt_during_create("cancel_unusable_reply", false);
    assert_eq!(code, 2, "{result}");
    assert_eq!(result["error"]["stage"], "cancelled", "{result}");
    assert_eq!(
        result["error"]["create_outcome"]["outcome"], "unknown",
        "{result}"
    );
    assert_eq!(
        result["error"]["create_receipt"], receipts[2]["response"],
        "{result}"
    );
    assert_eq!(
        result["error"]["create_context"]["operation_id"], "fixture-operation",
        "{result}"
    );
    assert!(result["error"]["cleanup"].is_null(), "{result}");
    assert_eq!(
        methods(&receipts),
        ["server.status", "session.list", "session.create"]
    );
    assert!(f.has_carrier(), "unknown ownership must prevent kill");
    assert!(
        !result.to_string().contains("restored_session_id"),
        "{result}"
    );
}

#[test]
fn restore_public_fixture_acknowledgement_failures_exit_before_python_and_clean_up() {
    for (mode, errno) in [
        ("pre_marker_closed_ack", libc::EPIPE),
        ("pre_marker_invalid_ack", libc::EINVAL),
    ] {
        let mut fixture = Fixture::new();
        let root = fixture.root.clone();
        let cleanup_receipt = std::env::temp_dir().join(format!(
            "restore-ack-failure-cleanup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fixture.cleanup_receipt = Some(cleanup_receipt.clone());
        let (child, stdout, stderr) = fixture.spawn_interrupt_restore(mode);
        let outer_pid = child.id();
        let output = child
            .wait_with_output(Instant::now() + Duration::from_secs(15), &stdout, &stderr)
            .unwrap();
        let pid = wait_for_pid(
            &fixture.state.join("created-child-owned-pid"),
            Instant::now() + Duration::from_secs(5),
        )
        .expect("supervisor records the exact pre-exec child PID");
        let python_started = fixture.state.join("create-child-pid").exists();
        let carrier_created = fixture.has_carrier();
        let receipts = fs::read_to_string(fixture.state.join("receipts.jsonl")).unwrap();
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        drop(fixture);
        let cleanup = fs::read_to_string(&cleanup_receipt).unwrap();
        fs::remove_file(&cleanup_receipt).unwrap();
        assert!(!process_is_alive(outer_pid), "{mode}: outer child remains");
        assert!(!process_is_alive(pid), "{mode}: create child remains");
        assert!(!root.exists(), "{mode}: fixture root remains");
        assert!(cleanup.contains(&format!("pid={pid}")), "{cleanup}");
        assert!(cleanup.contains("eof=eof"), "{cleanup}");
        assert_eq!(output.status.code(), Some(2), "{mode}: {output:?}");
        assert_eq!(result["error"]["stage"], "session.create", "{result}");
        assert!(
            result["error"]["detail"]
                .as_str()
                .unwrap()
                .contains(&std::io::Error::from_raw_os_error(errno).to_string()),
            "{mode}: {result}"
        );
        assert!(result["error"]["cleanup"].is_null(), "{result}");
        assert!(
            !python_started,
            "{mode}: Python executed despite failed ack"
        );
        assert!(
            !carrier_created,
            "{mode}: carrier created despite failed ack"
        );
        let receipts: Vec<Value> = receipts
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(methods(&receipts), ["server.status", "session.list"]);
        eprintln!("{mode}: result={result} cleanup={cleanup} root_absent=true outer={outer_pid} child={pid} absent=true");
    }
}

#[test]
fn restore_public_fixture_timeout_reclaims_held_create_child_before_root_removal() {
    let fixture = Fixture::new();
    let root = fixture.root.clone();
    let (mut child, stdout, stderr) = fixture.spawn_interrupt_restore("held_cleanup");
    let deadline = Instant::now() + Duration::from_secs(15);
    let marker = fixture.state.join("create-inflight");
    while !marker.exists() && Instant::now() < deadline {
        assert!(child.try_wait().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(marker.exists(), "create did not reach its held marker");
    let pid = wait_for_pid(
        &fixture.state.join("create-child-pid"),
        Instant::now() + Duration::from_secs(5),
    )
    .expect("create child publishes its exact PID");
    let output = child
        .wait_with_output(Instant::now(), &stdout, &stderr)
        .unwrap();
    assert!(
        !output.status.success(),
        "timeout must stop the outer child"
    );
    drop(fixture);
    assert!(
        !process_is_alive(pid),
        "exact create child survived timeout cleanup"
    );
    assert!(!root.exists(), "root must be removed only after child exit");
}

#[test]
fn restore_public_fixture_timeout_reclaims_child_before_python_marker() {
    let mut fixture = Fixture::new();
    let root = fixture.root.clone();
    let cleanup_receipt = std::env::temp_dir().join(format!(
        "restore-pre-marker-timeout-cleanup-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fixture.cleanup_receipt = Some(cleanup_receipt.clone());
    let (mut child, stdout, stderr) = fixture.spawn_interrupt_restore("pre_marker_timeout");
    let owned_pid = fixture.state.join("created-child-owned-pid");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !owned_pid.exists() && Instant::now() < deadline {
        assert!(
            child.try_wait().unwrap().is_none(),
            "restore exited before the pre-exec child announced ownership"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid = wait_for_pid(&owned_pid, Instant::now() + Duration::from_secs(5))
        .expect("pre-exec supervisor records the exact child PID");
    assert!(
        !fixture.state.join("create-child-pid").exists(),
        "the Python PID marker must not be published before ack"
    );
    let _ = child
        .wait_with_output(Instant::now(), &stdout, &stderr)
        .unwrap();
    drop(fixture);
    let cleanup = fs::read_to_string(&cleanup_receipt)
        .expect("fixture publishes lifetime cleanup evidence before root removal");
    let _ = fs::remove_file(&cleanup_receipt);
    assert!(cleanup.contains(&format!("pid={pid}")), "{cleanup}");
    assert!(cleanup.contains("eof=eof"), "{cleanup}");
    let leaked = process_is_alive(pid);
    let root_removed = !root.exists();
    terminate_owned_fixture_child(pid, &root);
    assert!(
        !leaked,
        "fixture removed the root before the pre-Python child exited"
    );
    assert!(
        root_removed,
        "fixture root remained after confirmed child exit"
    );
}

#[test]
fn restore_public_fixture_unwind_reclaims_held_create_child_before_root_removal() {
    let receipt = std::env::temp_dir().join(format!(
        "restore-held-cleanup-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe({
        let receipt = receipt.clone();
        move || {
            let fixture = Fixture::new();
            let (mut child, _, _) = fixture.spawn_interrupt_restore("held_cleanup");
            let deadline = Instant::now() + Duration::from_secs(15);
            let pid_path = fixture.state.join("create-child-pid");
            while !pid_path.exists() && Instant::now() < deadline {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "restore exited before held marker"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            let pid = wait_for_pid(&pid_path, Instant::now() + Duration::from_secs(5))
                .expect("create child publishes its exact PID");
            fs::write(&receipt, format!("{} {pid}", fixture.root.display())).unwrap();
            panic!("exercise fixture unwind while create is held");
        }
    }));
    assert!(panic.is_err());
    let evidence = fs::read_to_string(&receipt).unwrap();
    let (root, pid) = evidence.rsplit_once(' ').unwrap();
    let pid = pid.parse::<u32>().unwrap();
    let leaked = process_is_alive(pid);
    if leaked && process_belongs_to_fixture(pid, std::path::Path::new(root)) {
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(5);
        while process_is_alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let _ = fs::remove_file(receipt);
    assert!(
        !leaked,
        "fixture left its exact create child alive after unwind"
    );
    assert!(
        !std::path::Path::new(root).exists(),
        "fixture root remained after confirmed child cleanup"
    );
}

#[test]
fn restore_public_fixture_unwind_reclaims_child_before_python_marker() {
    let receipt = std::env::temp_dir().join(format!(
        "restore-pre-marker-unwind-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe({
        let receipt = receipt.clone();
        let cleanup_receipt = receipt.with_extension("cleanup");
        move || {
            let mut fixture = Fixture::new();
            fixture.cleanup_receipt = Some(cleanup_receipt);
            let (mut child, _, _) = fixture.spawn_interrupt_restore("pre_marker_unwind");
            let owned_pid = fixture.state.join("created-child-owned-pid");
            let deadline = Instant::now() + Duration::from_secs(15);
            while !owned_pid.exists() && Instant::now() < deadline {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "restore exited before the pre-exec child announced ownership"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            let pid = wait_for_pid(&owned_pid, Instant::now() + Duration::from_secs(5))
                .expect("pre-exec supervisor records the exact child PID");
            assert!(
                !fixture.state.join("create-child-pid").exists(),
                "the Python PID marker must not be published before ack"
            );
            fs::write(&receipt, format!("{} {pid}", fixture.root.display())).unwrap();
            panic!("exercise fixture unwind before Python marker publication");
        }
    }));
    assert!(panic.is_err());
    let evidence = fs::read_to_string(&receipt).unwrap();
    let (root, pid) = evidence.rsplit_once(' ').unwrap();
    let root = std::path::PathBuf::from(root);
    let pid = pid.parse::<u32>().unwrap();
    let leaked = process_is_alive(pid);
    let cleanup_receipt = receipt.with_extension("cleanup");
    let cleanup = fs::read_to_string(&cleanup_receipt)
        .expect("fixture publishes lifetime cleanup evidence before root removal");
    let _ = fs::remove_file(&cleanup_receipt);
    let root_removed = !root.exists();
    terminate_owned_fixture_child(pid, &root);
    assert!(cleanup.contains(&format!("pid={pid}")), "{cleanup}");
    assert!(cleanup.contains("eof=eof"), "{cleanup}");
    let _ = fs::remove_file(receipt);
    assert!(
        !leaked,
        "fixture removed the root before the pre-Python child exited"
    );
    assert!(
        root_removed,
        "fixture root remained after confirmed child exit"
    );
}
