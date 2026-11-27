#![cfg(all(unix, debug_assertions, feature = "lifecycle-test-hooks"))]

use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
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
}

struct OwnedChild {
    child: Option<std::process::Child>,
    process_group: bool,
}

impl OwnedChild {
    fn new(child: std::process::Child, process_group: bool) -> Self {
        Self {
            child: Some(child),
            process_group,
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
        let pid = child.id();
        loop {
            if let Some(status) = child.try_wait()? {
                self.reap_group(pid, Instant::now() + Duration::from_secs(5))?;
                return Ok(Output {
                    status,
                    stdout: fs::read(stdout_path)?,
                    stderr: fs::read(stderr_path)?,
                });
            }
            if Instant::now() >= deadline {
                if self.process_group {
                    unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
                }
                let _ = child.kill();
                let status = child.wait()?;
                self.reap_group(pid, Instant::now() + Duration::from_secs(5))?;
                return Ok(Output {
                    status,
                    stdout: fs::read(stdout_path)?,
                    stderr: fs::read(stderr_path)?,
                });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn reap_group(&self, pid: u32, deadline: Instant) -> std::io::Result<()> {
        if !self.process_group {
            return Ok(());
        }
        let group = -(pid as libc::pid_t);
        if unsafe { libc::kill(group, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return Ok(());
        }
        unsafe { libc::kill(group, libc::SIGKILL) };
        loop {
            if unsafe { libc::kill(group, 0) } != 0
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "owned process group cleanup unconfirmed",
                ));
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
            if self.process_group {
                unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
            } else {
                let _ = child.kill();
            }
        }
        let pid = child.id();
        let _ = child.wait();
        let _ = self.reap_group(pid, Instant::now() + Duration::from_secs(5));
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
        Self {
            root,
            state,
            project,
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
        let owned = OwnedChild::new(child, true);
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
        let map = self.map();
        let map_path = self.state.join("identity-map.json");
        fs::write(&map_path, serde_json::to_vec(&map).unwrap()).unwrap();
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
        (OwnedChild::new(child, true), stdout_path, stderr_path)
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
                if signal_group {
                    unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
                }
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
        let signal_target = if signal_group {
            -(child.id() as libc::pid_t)
        } else {
            child.id() as libc::pid_t
        };
        let signal = unsafe { libc::kill(signal_target, libc::SIGINT) };
        assert_eq!(signal, 0, "failed to interrupt restore");
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
        let marker = self.state.join("create-child-pid");
        let Ok(text) = fs::read_to_string(marker) else {
            return true;
        };
        let Ok(pid) = text.trim().parse::<u32>() else {
            return false;
        };
        if pid <= 1 || !process_is_alive(pid) {
            return true;
        }
        let _ = fs::write(self.state.join("release-create"), b"fixture-unwind-cleanup");
        let deadline = Instant::now() + Duration::from_secs(5);
        while process_is_alive(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if process_is_alive(pid) && process_belongs_to_fixture(pid, &self.root) {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            let kill_deadline = Instant::now() + Duration::from_secs(2);
            while process_is_alive(pid) && Instant::now() < kill_deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        !process_is_alive(pid)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.reclaim_fixture_create_child() {
            eprintln!("fixture cleanup unconfirmed; preserving {}", self.root.display());
            return;
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn process_is_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
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
    let (code, result, receipts) = f.interrupt_during_create("cancel_after_create", true);
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
    let (code, result, receipts) = f.interrupt_during_create("cancel_unusable_reply", true);
    assert_eq!(code, 2, "{result}");
    assert_eq!(result["error"]["stage"], "cancelled", "{result}");
    assert_eq!(result["error"]["create_outcome"]["outcome"], "unknown", "{result}");
    assert_eq!(result["error"]["create_receipt"], receipts[2]["response"], "{result}");
    assert_eq!(result["error"]["create_context"]["operation_id"], "fixture-operation", "{result}");
    assert!(result["error"]["cleanup"].is_null(), "{result}");
    assert_eq!(methods(&receipts), ["server.status", "session.list", "session.create"]);
    assert!(f.has_carrier(), "unknown ownership must prevent kill");
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
    let pid: u32 = fs::read_to_string(fixture.state.join("create-child-pid"))
        .unwrap().trim().parse().unwrap();
    let output = child.wait_with_output(Instant::now(), &stdout, &stderr).unwrap();
    assert!(!output.status.success(), "timeout must stop the outer child");
    drop(fixture);
    assert!(!process_is_alive(pid), "exact create child survived timeout cleanup");
    assert!(!root.exists(), "root must be removed only after child exit");
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
                assert!(child.try_wait().unwrap().is_none(), "restore exited before held marker");
                std::thread::sleep(Duration::from_millis(20));
            }
            let pid = fs::read_to_string(&pid_path)
                .expect("create child publishes its exact PID")
                .trim()
                .parse::<u32>()
                .unwrap();
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
    assert!(!leaked, "fixture left its exact create child alive after unwind");
    assert!(!std::path::Path::new(root).exists(), "fixture root remained after confirmed child cleanup");
}
