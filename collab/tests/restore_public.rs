#![cfg(all(unix, debug_assertions, feature = "lifecycle-test-hooks"))]

use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
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
        let output = command.output().unwrap();
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

    fn interrupt_during_create(&self, mode: &str, signal_group: bool) -> (i32, Value, Vec<Value>) {
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
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if signal_group {
            command.process_group(0);
        }
        let mut child = command.spawn().unwrap();
        let in_flight = self.state.join("create-inflight");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if in_flight.exists() {
                break;
            }
            if let Some(status) = child.try_wait().unwrap() {
                let output = child.wait_with_output().unwrap();
                panic!(
                    "restore exited before the in-flight marker ({status}): {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            if Instant::now() >= deadline {
                child.kill().ok();
                child.wait().ok();
                panic!("fixture create did not reach its in-flight marker");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let signal_target = if signal_group {
            format!("-{}", child.id())
        } else {
            child.id().to_string()
        };
        let signal = Command::new("kill")
            .args(["-INT", &signal_target])
            .status()
            .unwrap();
        assert!(signal.success(), "failed to interrupt restore");
        fs::write(self.state.join("release-create"), b"signal-delivered")
            .expect("failed to release fixture create response");
        let output = child.wait_with_output().unwrap();
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
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
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
