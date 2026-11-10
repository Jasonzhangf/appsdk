use super::*;

const THREAD_C: &str = "01a0d641-13f1-7b31-8d84-530f0f2d3fdd";

fn setup() -> (AppFixture, String, String) {
    let mut fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    let second = fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "isolated dashboard fixture approval",
        ],
        THREAD_A,
    );
    (
        fixture,
        first["identity"]["worker_id"].as_str().unwrap().into(),
        second["identity"]["worker_id"].as_str().unwrap().into(),
    )
}

fn publish(fixture: &AppFixture, id: &str) -> Value {
    fixture.run_public(
        &[
            "board",
            "publish",
            id,
            "--title",
            "任务板接口",
            "--description",
            "公开协议测试",
            "--delivery-condition",
            "接口行为通过",
            "--test-condition",
            "cargo test --test appserver_two_tui_integration",
            "--priority",
            "p1",
            "--long-horizon-content",
            "Continue this task from its current board state and preserve its delivery and test conditions",
            "--long-horizon-every-ms",
            "3600000",
        ],
        THREAD_A,
    )
}

fn delivery_without_candidate_probe(mode: &str) {
    use std::os::unix::fs::PermissionsExt;

    struct ReleaseProbe(PathBuf);
    impl Drop for ReleaseProbe {
        fn drop(&mut self) {
            let _ = std::fs::write(&self.0, b"release");
        }
    }

    let fixture = AppFixture::new();
    let other = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    let project = fixture.project.canonicalize().unwrap();
    let git_output = Command::new("which").arg("git").output().unwrap();
    assert!(git_output.status.success());
    let real_git = String::from_utf8(git_output.stdout)
        .unwrap()
        .trim()
        .to_owned();
    for arguments in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "initial fixture",
        ],
    ] {
        assert!(Command::new(&real_git)
            .current_dir(&project)
            .args(arguments)
            .status()
            .unwrap()
            .success());
    }
    let worktree = fixture.root.join("delivery-worktree");
    assert!(Command::new(&real_git)
        .current_dir(&project)
        .args(["worktree", "add", "-q", "-b", "delivery-liveness"])
        .arg(&worktree)
        .arg("main")
        .status()
        .unwrap()
        .success());
    let worktree = worktree.canonicalize().unwrap();
    fixture.run_public(&["task", "register", "delivery-liveness"], THREAD_B);

    let entered = fixture.root.join("candidate-entered");
    let release = fixture.root.join("candidate-release");
    let wrapper = fixture.path_prepend.join("git");
    let script = format!(
        r#"#!/usr/bin/env python3
import os
from pathlib import Path
import subprocess
import sys
import time

release = Path({release})
if sys.argv[1:] == ["--hold-candidate-output"]:
    while not release.exists():
        time.sleep(0.01)
    sys.exit(0)
if os.getcwd() == {worktree} and sys.argv[1:] == ["rev-parse", "--verify", "HEAD^{{commit}}"]:
    Path({entered}).write_text({mode})
    if {mode} == "inherited-output":
        subprocess.Popen([sys.executable, __file__, "--hold-candidate-output"])
    else:
        while not release.exists():
            time.sleep(0.01)
os.execv({real_git}, ["git"] + sys.argv[1:])
"#,
        release = serde_json::to_string(&release).unwrap(),
        worktree = serde_json::to_string(&worktree).unwrap(),
        entered = serde_json::to_string(&entered).unwrap(),
        mode = serde_json::to_string(mode).unwrap(),
        real_git = serde_json::to_string(&real_git).unwrap(),
    );
    std::fs::write(&wrapper, script).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (completed_before_release, before_release, results) = std::thread::scope(|scope| {
        // Drop this before scoped threads are joined, even if an assertion fails.
        let _release = ReleaseProbe(release.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let delivered = tx.clone();
        let fixture_ref = &fixture;
        let delivery_worktree = worktree.clone();
        scope.spawn(move || {
            let deliver_arguments = [
                "task",
                "deliver",
                "delivery-liveness",
                "--worktree",
                delivery_worktree.to_str().unwrap(),
                "--evidence",
                "real delivery evidence",
            ];
            let output = fixture_ref.command_public(&deliver_arguments, THREAD_B);
            delivered
                .send(("delivery", json_from_output(output)))
                .unwrap();
        });
        let mut results = HashMap::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !entered.exists() && results.is_empty() && Instant::now() < deadline {
            if let Ok((name, output)) = rx.recv_timeout(Duration::from_millis(20)) {
                results.insert(name, output);
            }
        }
        for (name, arguments) in [("ping", vec!["status"]), ("context", vec!["context"])] {
            let tx = tx.clone();
            scope.spawn(move || {
                tx.send((
                    name,
                    json_from_output(fixture_ref.command_public(&arguments, THREAD_A)),
                ))
                .unwrap();
            });
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while results.len() < 3 && Instant::now() < deadline {
            if let Ok((name, output)) = rx.recv_timeout(Duration::from_millis(20)) {
                results.insert(name, output);
            }
        }
        let completed_before_release = results.len() == 3;
        let before_release = results.keys().copied().collect::<Vec<_>>();
        std::fs::write(&release, b"release").unwrap();
        while results.len() < 3 {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok((name, output)) => {
                    results.insert(name, output);
                }
                Err(_) => break,
            }
        }
        (completed_before_release, before_release, results)
    });
    assert!(
        completed_before_release,
        "{mode}: delivery-path requests did not complete before release; probe_entered={}, completed={before_release:?}, results={results:?}",
        entered.exists()
    );
    assert_eq!(results.len(), 3, "{results:?}");
    assert!(
        results["delivery"].is_object()
            && results["ping"].is_object()
            && results["context"].is_object(),
        "{results:?}"
    );
    assert!(
        !entered.exists(),
        "delivery still queried an automatic candidate"
    );
    let delivered = &results["delivery"];
    assert_eq!(delivered["delivery_kind"], "code");
    assert_eq!(delivered["status"], "delivered");
    assert_eq!(delivered["evidence"], "real delivery evidence");
    assert_eq!(delivered["delivery_complete"], false);
    let context = &results["context"];
    assert_eq!(
        context["identity"]["worker_id"],
        other["identity"]["worker_id"]
    );
    let status = fixture.run_public(&["task", "status", "delivery-liveness"], THREAD_B);
    assert_eq!(status["status"], "delivered");
}

fn json_from_output(output: std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("collab CLI emits JSON");
    value
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .cloned()
        .unwrap_or(value)
}

struct HeldGitIntegration {
    fixture: AppFixture,
    entered: PathBuf,
    hold: PathBuf,
    commit: String,
}

impl HeldGitIntegration {
    fn release_and_wait(&self) {
        let _ = std::fs::remove_file(&self.hold);
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.entered.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for HeldGitIntegration {
    fn drop(&mut self) {
        self.release_and_wait();
    }
}

fn prepare_held_git_integration() -> HeldGitIntegration {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "isolated integration liveness fixture",
        ],
        THREAD_A,
    );

    let project = fixture.project.canonicalize().unwrap();
    let git_output = Command::new("which").arg("git").output().unwrap();
    assert!(git_output.status.success());
    let real_git = String::from_utf8(git_output.stdout)
        .unwrap()
        .trim()
        .to_owned();
    for arguments in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "integration fixture",
        ],
    ] {
        assert!(Command::new(&real_git)
            .current_dir(&project)
            .args(arguments)
            .status()
            .unwrap()
            .success());
    }
    let commit_output = Command::new(&real_git)
        .current_dir(&project)
        .args(["rev-parse", "--verify", "refs/heads/main^{commit}"])
        .output()
        .unwrap();
    assert!(commit_output.status.success());
    let commit = String::from_utf8(commit_output.stdout)
        .unwrap()
        .trim()
        .to_owned();

    let worktree = fixture.project.join("integration-worktree");
    assert!(Command::new(&real_git)
        .current_dir(&project)
        .args(["worktree", "add", "-q", "-b", "integration-liveness"])
        .arg(&worktree)
        .arg("main")
        .status()
        .unwrap()
        .success());
    let worktree = worktree.canonicalize().unwrap();

    fixture.run_public(&["task", "register", "integration-liveness"], THREAD_B);
    let delivered = fixture.run_public(
        &[
            "task",
            "deliver",
            "integration-liveness",
            "--worktree",
            worktree.to_str().unwrap(),
            "--evidence",
            "delivered for integration liveness",
        ],
        THREAD_B,
    );
    assert_eq!(delivered["delivery_kind"], "code");
    let reviewed = fixture.run_public(
        &[
            "task",
            "review",
            "integration-liveness",
            "--accept",
            "--evidence",
            "integration liveness review accepted",
        ],
        THREAD_A,
    );
    assert_eq!(reviewed["merge_pending"], json!(true));

    let seam = fixture.project.join(".agent-collab");
    let hold = seam.join("held-integration-resolution");
    let entered = seam.join("held-integration-resolution-entered");
    std::fs::write(&hold, b"held").unwrap();

    HeldGitIntegration {
        fixture,
        entered,
        hold,
        commit,
    }
}

fn wait_for_path(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(path.exists(), "timed out waiting for {}", path.display());
}

fn parse_public_response(output: std::process::Output) -> (bool, Value) {
    let success = output.status.success();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let response = stderr
            .lines()
            .find_map(|line| line.strip_prefix("collab response: "))
            .unwrap_or_else(|| {
                panic!(
                    "collab CLI did not emit JSON; stdout={} stderr={stderr}",
                    String::from_utf8_lossy(&output.stdout),
                )
            });
        serde_json::from_str(response).unwrap_or_else(|error| {
            panic!("collab CLI error response is not JSON: {error}; response={response}")
        })
    });
    let value = value
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .cloned()
        .unwrap_or(value);
    (success, value)
}

fn seed_historical_layerfs_task(
    fixture: &mut AppFixture,
    id: &str,
    owner: &str,
    created_by: &str,
    worktree: &str,
    branch: &str,
) -> Value {
    fixture.update_current_state(|state| {
        state["tasks"][id] = json!({
            "id": id,
            "owner": owner,
            "created_by": created_by,
            "feature_id": Some(format!("{id}-feature")),
            "worktree_path": worktree,
            "branch": branch,
            "base_commit": "layerfs-base",
            "priority": "p1",
            "status": "working",
            "next_step": "continue the historical LayerFS task",
            "wait": null,
            "isolation_mode": "layerfs_workspace",
            "created_ms": 1,
            "updated_ms": 1
        });
    });
    fixture.current_state()["tasks"][id].clone()
}

fn install_layerfs_stub(fixture: &AppFixture, calls: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let path = fixture.path_prepend.join("layerfs");
    std::fs::write(
        &path,
        format!(
            r#"#!/bin/sh
printf '%s\n' "layerfs $*" >> {calls}
printf '{{"code":"UnexpectedCall","message":"retired workspace entry must not run"}}\n'
"#,
            calls = serde_json::to_string(&calls.display().to_string()).unwrap(),
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
}

fn assert_no_retired_workspace_calls(calls: &std::path::Path) {
    let recorded = std::fs::read_to_string(calls).unwrap_or_default();
    assert!(
        recorded.trim().is_empty(),
        "the retired workspace entry must receive zero calls: {recorded}"
    );
}

fn run_held_git_integration(
    probe: &HeldGitIntegration,
    during: impl FnOnce(&AppFixture),
) -> std::process::Output {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let fixture = &probe.fixture;
        let commit = probe.commit.clone();
        scope.spawn(move || {
            let args = vec![
                "task".to_owned(),
                "integrated".to_owned(),
                "integration-liveness".to_owned(),
                "--commit".to_owned(),
                commit,
                "--evidence".to_owned(),
                "held integration proof".to_owned(),
            ];
            let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
            let output = fixture.command_public(&refs, THREAD_A);
            tx.send(output).unwrap();
        });
        wait_for_path(&probe.entered, Duration::from_secs(5));
        during(&probe.fixture);
        probe.release_and_wait();
        rx.recv_timeout(Duration::from_secs(10))
            .expect("held integration request must complete after release")
    })
}

#[test]
fn task_integration_held_git_cannot_freeze_context() {
    let probe = prepare_held_git_integration();
    let other = probe.fixture.run_public(&["context"], THREAD_B);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut results = HashMap::new();
    std::thread::scope(|scope| {
        let fixture = &probe.fixture;
        let commit = probe.commit.clone();
        let tx_integration = tx.clone();
        scope.spawn(move || {
            let args = vec![
                "task".to_owned(),
                "integrated".to_owned(),
                "integration-liveness".to_owned(),
                "--commit".to_owned(),
                commit,
                "--evidence".to_owned(),
                "held integration proof".to_owned(),
            ];
            let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
            let output = fixture.command_public(&refs, THREAD_A);
            tx_integration
                .send(("integration", json_from_output(output)))
                .unwrap();
        });
        wait_for_path(&probe.entered, Duration::from_secs(5));
        for (name, args) in [
            ("ping", vec!["status".to_owned()]),
            ("context", vec!["context".to_owned()]),
        ] {
            let fixture = &probe.fixture;
            let tx = tx.clone();
            scope.spawn(move || {
                let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
                let output = fixture.command_public(&refs, THREAD_B);
                tx.send((name, json_from_output(output))).unwrap();
            });
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while results.len() < 2 && Instant::now() < deadline {
            if let Ok((name, output)) = rx.recv_timeout(Duration::from_millis(20)) {
                results.insert(name, output);
            }
        }
        assert_eq!(results.len(), 2, "{results:?}");
        assert!(
            !results.contains_key("integration"),
            "integration request completed before the held Git proof was released"
        );
        let status = fixture.run_public(&["task", "status", "integration-liveness"], THREAD_B);
        assert_eq!(status["status"], "accepted");
        probe.release_and_wait();
        while results.len() < 3 {
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok((name, output)) => {
                    results.insert(name, output);
                }
                Err(_) => break,
            }
        }
    });
    assert_eq!(results.len(), 3, "{results:?}");
    assert_eq!(
        results["context"]["identity"]["worker_id"],
        other["identity"]["worker_id"]
    );
    assert_eq!(results["integration"]["status"], "merged");
    assert_eq!(results["integration"]["commit"], probe.commit);
    let status = probe
        .fixture
        .run_public(&["task", "status", "integration-liveness"], THREAD_B);
    assert_eq!(status["status"], "merged");
}

#[test]
fn task_integration_held_git_rechecks_current_master_authority() {
    let probe = prepare_held_git_integration();
    let output = run_held_git_integration(&probe, |fixture| {
        fixture.run_public(
            &[
                "master",
                "clear",
                "--approval",
                "old master releases during held integration proof",
            ],
            THREAD_A,
        );
        fixture.run_public(
            &[
                "master",
                "promote",
                "--approval",
                "new master takes over during held integration proof",
            ],
            THREAD_B,
        );
    });
    let (success, result) = parse_public_response(output);
    assert!(!success, "{result}");
    assert_eq!(
        result["error"],
        "task integrated requires task owner or live master authority"
    );
    let state = probe.fixture.current_state();
    assert_eq!(state["tasks"]["integration-liveness"]["status"], "accepted");
    assert!(
        state["pending_merges"]["integration-liveness"].is_object(),
        "the pending merge must survive the stale old-master request"
    );
}

#[test]
fn task_integration_held_git_rejects_changed_review_subject() {
    let probe = prepare_held_git_integration();
    let output = run_held_git_integration(&probe, |fixture| {
        fixture.run_public(
            &[
                "task",
                "update",
                "integration-liveness",
                "--status",
                "rework",
                "--next",
                "address review findings",
            ],
            THREAD_B,
        );
        for status in ["working", "verifying", "reviewed"] {
            fixture.run_public(
                &["task", "update", "integration-liveness", "--status", status],
                THREAD_B,
            );
        }
        let worktree = fixture.project.join("integration-worktree");
        fixture.run_public(
            &[
                "task",
                "deliver",
                "integration-liveness",
                "--worktree",
                worktree.to_str().unwrap(),
                "--evidence",
                "redelivered after held integration proof started",
            ],
            THREAD_B,
        );
        let reviewed = fixture.run_public(
            &[
                "task",
                "review",
                "integration-liveness",
                "--accept",
                "--evidence",
                "reaccepted changed review subject",
            ],
            THREAD_A,
        );
        assert_eq!(reviewed["merge_pending"], json!(true));
    });
    let (success, result) = parse_public_response(output);
    assert!(!success, "{result}");
    assert_eq!(result["error"], "TASK_INTEGRATION_SUBJECT_CHANGED");
    let state = probe.fixture.current_state();
    assert_eq!(state["tasks"]["integration-liveness"]["status"], "accepted");
    assert_eq!(
        state["task_lifecycle"]["integration-liveness"]["delivery_evidence"],
        "redelivered after held integration proof started"
    );
    assert_eq!(
        state["task_lifecycle"]["integration-liveness"]["review_evidence"],
        "reaccepted changed review subject"
    );
    assert!(
        state["pending_merges"]["integration-liveness"].is_object(),
        "the new review must leave its pending merge intact"
    );
}

#[test]
fn task_integration_held_git_ignores_unrelated_state_update() {
    let probe = prepare_held_git_integration();
    let output = run_held_git_integration(&probe, |fixture| {
        fixture.run_public(
            &["task", "register", "unrelated-integration-state"],
            THREAD_B,
        );
    });
    let (success, result) = parse_public_response(output);
    assert!(success, "{result}");
    assert_eq!(result["status"], "merged");
    let state = probe.fixture.current_state();
    assert_eq!(state["tasks"]["integration-liveness"]["status"], "merged");
    assert!(state["tasks"]["unrelated-integration-state"].is_object());
}

#[test]
fn task_delivery_candidate_delay_cannot_freeze_context() {
    delivery_without_candidate_probe("delayed-command");
}

#[test]
fn task_delivery_inherited_output_cannot_freeze_context() {
    delivery_without_candidate_probe("inherited-output");
}

#[test]
fn historical_workspace_status_returns_raw_data_without_external_calls() {
    let (mut fixture, master, peer) = setup();
    let calls = fixture.root.join("workspace-tool-calls.log");
    install_layerfs_stub(&fixture, &calls);
    let before = seed_historical_layerfs_task(
        &mut fixture,
        "historical-workspace",
        &peer,
        &master,
        "layerfs:demo/historical-workspace",
        "historical-branch",
    );

    let status = fixture.run_public(&["task", "status", "historical-workspace"], THREAD_B);
    assert_eq!(status["worktree"], "layerfs:demo/historical-workspace");
    assert_eq!(status["branch"], "historical-branch");
    assert_no_retired_workspace_calls(&calls);
    assert_eq!(
        fixture.current_state()["tasks"]["historical-workspace"],
        before,
        "status must not rewrite the original stored record"
    );
}

#[test]
fn historical_workspace_close_fails_without_external_calls_or_deleting_same_named_resources() {
    let (mut fixture, master, peer) = setup();
    let project = fixture.project.canonicalize().unwrap();
    for arguments in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "initial fixture",
        ],
    ] {
        assert!(Command::new("git")
            .current_dir(&project)
            .args(arguments)
            .status()
            .unwrap()
            .success());
    }
    assert!(Command::new("git")
        .current_dir(&project)
        .args(["branch", "retired-workspace"])
        .status()
        .unwrap()
        .success());
    let side_directory = fixture.project.join("playground/retired-workspace");
    std::fs::create_dir_all(&side_directory).unwrap();
    std::fs::write(side_directory.join("user-data.txt"), "preserve\n").unwrap();

    let calls = fixture.root.join("workspace-tool-calls.log");
    install_layerfs_stub(&fixture, &calls);
    let before = seed_historical_layerfs_task(
        &mut fixture,
        "historical-close",
        &peer,
        &master,
        "layerfs:demo/historical-close",
        "retired-workspace",
    );
    let project_scope = project.to_string_lossy().to_string();
    fixture.update_current_state(|state| {
        state["tasks"]["historical-close"]["status"] = json!("merged");
        state["task_lifecycle"]["historical-close"] = json!({
            "delivery_kind": "read_only",
            "delivery_evidence": "read-only delivery evidence",
            "delivered_ms": 1000,
            "review_evidence": "independent review pass",
            "reviewer": master,
            "reviewed_ms": 2000
        });
        state["cleanup_receipts"]["historical-close"] = json!({
            "id": "cleanup-historical-close",
            "task_id": "historical-close",
            "worktree_path": "layerfs:demo/historical-close",
            "branch": "retired-workspace",
            "verified_ms": 3000,
            "verification": "verified"
        });
        state["worktree_bindings"]["binding-historical-close"] = json!({
            "worktree_root": "layerfs:demo/historical-close",
            "owning_project_scope": project_scope,
            "task_id": "historical-close",
            "owner_agent_id": peer,
            "binding_id": "binding-historical-close",
            "base_commit": "layerfs-base",
            "isolation_mode": "layerfs_workspace"
        });
    });

    let close = fixture.command_public(&["task", "close", "historical-close"], THREAD_B);
    assert!(
        !close.status.success(),
        "an invalid workspace close must fail"
    );
    assert_no_retired_workspace_calls(&calls);
    let state = fixture.current_state();
    assert_eq!(state["tasks"]["historical-close"]["status"], "merged");
    assert_eq!(
        state["tasks"]["historical-close"]["worktree_path"],
        before["worktree_path"]
    );
    assert!(side_directory.join("user-data.txt").is_file());
    assert!(Command::new("git")
        .current_dir(&project)
        .args(["rev-parse", "--verify", "refs/heads/retired-workspace"])
        .status()
        .unwrap()
        .success());
}

#[test]
fn task_register_rejects_scheme_and_malformed_git_paths_without_side_effects() {
    let (fixture, _, _) = setup();
    let calls = fixture.root.join("workspace-tool-calls.log");
    install_layerfs_stub(&fixture, &calls);
    let before = fixture.current_state();

    for (id, worktree) in [
        ("retired-register", "retired:demo/task-1"),
        ("relative-scheme-register", "demo:task-1"),
        ("parent-register", "../outside-task"),
    ] {
        let output = fixture.command_public(
            &[
                "task",
                "register",
                id,
                "--worktree",
                worktree,
                "--branch",
                "task-1",
                "--priority",
                "p1",
            ],
            THREAD_B,
        );
        assert!(
            !output.status.success(),
            "invalid workspace {worktree} must be rejected: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let status = fixture.command_public(&["task", "status", id], THREAD_B);
        assert!(
            !status.status.success(),
            "rejected task {id} must not exist"
        );
    }
    assert_no_retired_workspace_calls(&calls);
    let after = fixture.current_state();
    assert_eq!(after["tasks"], before["tasks"]);
    assert_eq!(after["worktree_bindings"], before["worktree_bindings"]);
}

#[test]
fn scheduler_dispatch_rejects_scheme_workspace_before_any_effect() {
    let (fixture, _, _) = setup();
    let calls = fixture.root.join("workspace-tool-calls.log");
    install_layerfs_stub(&fixture, &calls);
    let before = fixture.current_state();
    let output = fixture.command_public(
        &[
            "subagent",
            "dispatch",
            "--request-id",
            "retired-workspace-deny",
            "--subject",
            "workspace validation",
            "--worktree-path",
            "retired:demo/task-1",
            "--priority",
            "p1",
            "body",
        ],
        THREAD_A,
    );
    assert!(
        !output.status.success(),
        "scheme workspace must be rejected"
    );
    assert_no_retired_workspace_calls(&calls);
    let after = fixture.current_state();
    assert!(
        after["tasks"]
            .get("task-scheduler-retired-workspace-deny")
            .is_none(),
        "rejected dispatch must not create a task"
    );
    assert_eq!(after["subagents"], before["subagents"]);
    assert_eq!(after["msgs"], before["msgs"]);
}

#[test]
fn git_workspace_register_and_status_keep_working_without_the_retired_entry() {
    let (mut fixture, _, _) = setup();
    let calls = fixture.root.join("workspace-tool-calls.log");
    install_layerfs_stub(&fixture, &calls);
    let base = fixture.root.join("external-worktrees");
    let project_key = fixture
        .project
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_owned()
        .to_string();
    let worktree = base.join(project_key).join("git-task");
    std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    let config = fixture.root.join("home/.appsdk/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        format!(
            "[worktree]\nbase = {}\nlayout = \"{{project-key}}/{{task-slug}}\"\n",
            serde_json::to_string(&base.display().to_string()).unwrap()
        ),
    )
    .unwrap();
    fixture.restart_daemon();

    let registered = fixture.run_public(
        &[
            "task",
            "register",
            "git-task",
            "--worktree",
            worktree.to_str().unwrap(),
            "--branch",
            "codex/git-task",
            "--priority",
            "p1",
        ],
        THREAD_B,
    );
    assert_eq!(registered["cleanup"], "pending");
    let status = fixture.run_public(&["task", "status", "git-task"], THREAD_B);
    let stored_worktree = fixture.current_state()["tasks"]["git-task"]["worktree_path"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(status["worktree"], stored_worktree);
    assert_eq!(status["branch"], "codex/git-task");
    assert_eq!(
        fixture.current_state()["tasks"]["git-task"]["isolation_mode"],
        "isolated_worktree"
    );
    assert_no_retired_workspace_calls(&calls);
}

#[test]
fn layerfs_relocate_from_git_worktree_is_rejected_without_layerfs_calls() {
    use std::os::unix::fs::PermissionsExt;

    let mut fixture = AppFixture::new();
    let worktrees = fixture.root.join("worktrees");
    let project = fixture.project.canonicalize().unwrap();
    let worktree = worktrees.join("project/layerfs-relocate-task");
    std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    let config = fixture.root.join("home/.appsdk/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        format!(
            "[worktree]\nbase = {}\nlayout = \"{{project-key}}/{{task-slug}}\"\n",
            serde_json::to_string(&worktrees.display().to_string()).unwrap()
        ),
    )
    .unwrap();
    fixture.restart_daemon();

    for arguments in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "initial fixture",
        ],
    ] {
        assert!(Command::new("git")
            .current_dir(&project)
            .args(arguments)
            .status()
            .unwrap()
            .success());
    }
    assert!(Command::new("git")
        .current_dir(&project)
        .args(["worktree", "add", "-q", "-b", "codex/layerfs-relocate-task",])
        .arg(&worktree)
        .arg("main")
        .status()
        .unwrap()
        .success());
    let worktree = worktree.canonicalize().unwrap();

    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "isolated LayerFS relocate fixture",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "task",
            "register",
            "layerfs-relocate-task",
            "--worktree",
            worktree.to_str().unwrap(),
            "--branch",
            "codex/layerfs-relocate-task",
            "--priority",
            "p1",
        ],
        THREAD_B,
    );
    let before = fixture.current_state()["tasks"]["layerfs-relocate-task"].clone();

    let calls = fixture.root.join("layerfs-calls.log");
    let layerfs = fixture.path_prepend.join("layerfs");
    std::fs::write(
        &layerfs,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"layerfs $*\" >> '{}'\nprintf '{{\"items\":[{{\"name\":\"layerfs-task\"}}]}}\\n'\n",
            calls.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&layerfs).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&layerfs, permissions).unwrap();

    let output = fixture.command_public(
        &[
            "task",
            "relocate",
            "layerfs-relocate-task",
            "--worktree",
            "layerfs:demo/layerfs-task",
            "--branch",
            "layerfs-task",
        ],
        THREAD_B,
    );
    assert!(
        !output.status.success(),
        "moving a Git worktree task to LayerFS must fail closed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("worktree path must match the configured worktree base/layout"),
        "{stderr}"
    );
    assert_eq!(
        fixture.current_state()["tasks"]["layerfs-relocate-task"],
        before,
        "the rejected relocation must not change the worktree, owner or status"
    );
    assert!(
        !calls.exists() || std::fs::read_to_string(&calls).unwrap().trim().is_empty(),
        "the isolated LayerFS entry must receive zero calls"
    );
}

#[test]
fn unclosed_task_with_retired_owner_is_visible_without_an_assignee() {
    let (mut fixture, master, peer) = setup();
    publish(&fixture, "orphaned-open-task");
    let claimed = fixture.run_public(
        &[
            "board",
            "claim",
            "orphaned-open-task",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    assert_eq!(claimed["task"]["owner"], peer);
    fixture.stop_daemon();
    // Start the read-only consumer from a retained task whose owner has
    // already retired. Retirement admission itself is not under test here.
    let map_path = fixture.host_state.join("identity-map.json");
    let mut map: Value = serde_json::from_slice(&std::fs::read(&map_path).unwrap()).unwrap();
    let slot = map["slots"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|slot| slot["current"]["binding"]["agent_id"] == peer)
        .unwrap();
    slot["previous"] = slot["current"].take();
    slot["identity_status"] = json!("unregistered");
    slot["generation"] = json!(slot["generation"].as_u64().unwrap() + 1);
    std::fs::write(map_path, serde_json::to_vec(&map).unwrap()).unwrap();
    let path = fixture.project_state_for(&fixture.project, "appserver-cli");
    let mut document: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    document["state"]["workers"]
        .as_object_mut()
        .unwrap()
        .remove(&peer)
        .unwrap();
    document["state"]["keepalives"]
        .as_object_mut()
        .unwrap()
        .remove(&peer);
    std::fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
    let before = fixture.current_state();
    fixture.start_daemon();
    // The timer owns unavailable-name reflow. Let that current episode settle
    // before measuring whether the Board reader changes the assignment.
    let deadline = Instant::now() + Duration::from_secs(5);
    let assignment = loop {
        let assignment = fixture.current_state()["task_assignments"]["orphaned-open-task"].clone();
        if matches!(
            assignment["reflow"]["delivery"].as_str(),
            Some("delivered" | "failed")
        ) {
            break assignment;
        }
        assert!(
            Instant::now() < deadline,
            "retired-owner reflow did not settle: {assignment}"
        );
        thread::sleep(Duration::from_millis(20));
    };
    for field in ["task_id", "name", "session_id"] {
        assert_eq!(
            assignment[field],
            before["task_assignments"]["orphaned-open-task"][field]
        );
    }
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "orphaned-open-task")
        .unwrap();
    assert_eq!(task["owner"], "", "{task}");
    assert_eq!(task["owner_display_name"], "", "{task}");
    assert_eq!(task["publisher"], master);
    assert_eq!(task["status"], "working");
    assert_eq!(task["revision"], claimed["task"]["revision"]);
    let after = fixture.current_state();
    for field in ["tasks", "board_details"] {
        assert_eq!(
            after[field]["orphaned-open-task"], before[field]["orphaned-open-task"],
            "board must preserve the current {field} record"
        );
    }
    assert_eq!(
        after["task_assignments"]["orphaned-open-task"], assignment,
        "Board read must preserve the settled current assignment"
    );
}

fn role_patrol_text(context: &Value) -> String {
    let brief = &context["role_brief"];
    let responsibilities = brief["responsibilities"]
        .as_array()
        .expect("role_brief responsibilities are explicit")
        .iter()
        .filter_map(Value::as_str);
    let next_action = brief["next_action"]
        .as_str()
        .expect("role_brief next_action is explicit");
    responsibilities
        .chain(std::iter::once(next_action))
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_role_patrol_mentions(label: &str, context: &Value, role: &str, keywords: &[&str]) {
    assert_eq!(context["role_brief"]["role"], role, "{label}: {context}");
    let text = role_patrol_text(context);
    let haystack = text.to_ascii_lowercase();
    for keyword in keywords {
        assert!(
            haystack.contains(keyword.to_ascii_lowercase().as_str()),
            "{label}: role_brief responsibilities/next_action must mention {keyword:?}: {text}"
        );
    }
}

fn notification_subscription_count(fixture: &AppFixture, thread: &str, label: &str) -> usize {
    let status = fixture.run_public(&["notify", "status"], thread);
    let subscriptions = status["subscriptions"]
        .as_array()
        .unwrap_or_else(|| panic!("{label}: notify status lists subscriptions: {status}"));
    subscriptions.len()
}

fn assert_no_serialized_role_control(label: &str, value: &Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let normalized = key.to_ascii_lowercase();
                assert!(
                    !matches!(
                        normalized.as_str(),
                        "authority"
                            | "derivation"
                            | "token"
                            | "role_brief"
                            | "managed_subagent"
                            | "must_obey_master"
                            | "may_decline_master_invite"
                    ),
                    "{label}: serialized role control surface leaked through key {key:?}: {value}"
                );
                assert_no_serialized_role_control(label, child);
            }
        }
        Value::Array(items) => {
            for item in items {
                assert_no_serialized_role_control(label, item);
            }
        }
        _ => {}
    }
}

#[test]
fn display_name_defaults_follow_current_master_without_renaming_identity() {
    let (mut fixture, master, peer) = setup();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    assert_eq!(board["master"]["display_name"], "Jenny");
    let members = board["workers"].as_array().unwrap();
    assert_eq!(
        members.iter().find(|w| w["id"] == master).unwrap()["display_name"],
        "Jenny"
    );
    assert_eq!(
        members.iter().find(|w| w["id"] == peer).unwrap()["display_name"],
        "Bob"
    );
    restore_online_master(&mut fixture, THREAD_B);
    // Handoff does not operate the former executor. Its own later context
    // call obtains an ordinary pool name without taking Jenny back.
    let resumed_peer = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(resumed_peer["identity"]["worker_id"], master);
    assert_eq!(resumed_peer["identity"]["display_name"], "Alice");
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    assert_eq!(board["master"]["worker_id"], peer);
    assert_eq!(board["master"]["display_name"], "Jenny");
    assert_eq!(
        board["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == master)
            .unwrap()["display_name"],
        "Alice"
    );
}

fn restore_online_master(fixture: &mut AppFixture, thread: &str) -> Value {
    // Presence is the precondition under test. A responsive Fake AppServer
    // alone is not a committed name-presence observation.
    let map: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let holder = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["identity_key"]["name"] == "Jenny" && slot["identity_status"] == "active")
        .unwrap();
    let project = holder["identity_key"]["project_scope"].as_str().unwrap();
    let binding = &holder["current"]["binding"];
    let worker = binding["agent_id"].as_str().unwrap();
    fixture.update_current_state(|state| {
        state["agent_presence"][project][worker] = json!({
            "project_scope":project, "worker_id":worker, "session_id":binding["session_id"],
            "presence":"online", "reason":"controlled current name-holder presence", "observed_ms":1
        })
    });
    fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_master","authorization":"isolated fixture confirms online replacement","replace_confirmed":true}"#], thread)
}

#[test]
fn context_name_restore_persists_and_preserves_task_and_message_identity() {
    let (mut fixture, master, peer) = setup();
    let named = fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Carol","authorization":"isolated fixture changes its free work name"}"#], THREAD_B);
    assert_eq!(named["identity"]["worker_id"], peer);
    assert_eq!(named["identity"]["display_name"], "Carol");
    let who = fixture.run_public(&["who"], THREAD_B);
    assert_eq!(
        who["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == peer)
            .unwrap()["display_name"],
        "Carol"
    );
    let status = fixture.run_public(&["worker", "status", &peer], THREAD_B);
    assert_eq!(status["workers"][0]["display_name"], "Carol");
    assert_eq!(
        fixture.run_public(&["board", "show"], THREAD_A)["master"]["display_name"],
        "Jenny"
    );
    publish(&fixture, "named-work");
    let claimed = fixture.run_public(
        &["board", "claim", "named-work", "--expected-revision", "1"],
        THREAD_B,
    );
    assert_eq!(claimed["task"]["owner"], peer);
    assert_eq!(claimed["task"]["owner_display_name"], "Carol");
    assert_eq!(claimed["task"]["publisher"], master);
    assert_eq!(claimed["task"]["publisher_display_name"], "Jenny");
    fixture.run_public(
        &[
            "sendmessage",
            "--to",
            &master,
            "--subject",
            "named-peer",
            "stable route",
        ],
        THREAD_B,
    );
    let received = fixture.run_public(&["recv", "--timeout", "1"], THREAD_A);
    let message = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["subject"] == "named-peer")
        .expect("the sent peer message must be consumed independently of daemon notices");
    assert_eq!(message["from"], peer);
    assert_eq!(message["body"], "stable route");
    for name in ["  ", "Alice\nInjected"] {
        assert!(!fixture
            .command_public(
                &[
                    "context",
                    "--approve-grant",
                    &json!({
                        "action":"restore_name", "requested_name":name,
                        "authorization":"isolated invalid-name counterexample"
                    })
                    .to_string()
                ],
                THREAD_B
            )
            .status
            .success());
    }
    fixture.restart_daemon();
    let restored = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(restored["identity"]["display_name"], "Carol");
    assert_eq!(restored["identity"]["worker_id"], peer);
    assert_eq!(restored["master"]["display_name"], "Jenny");
    let task = fixture.run_public(&["board", "show"], THREAD_B);
    let task = task["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "named-work")
        .unwrap();
    assert_eq!(task["owner"], peer);
    assert_eq!(task["owner_display_name"], "Carol");
}

#[test]
fn board_publish_is_shared_but_does_not_start_execution() {
    let (fixture, master, _) = setup();
    let published = publish(&fixture, "board-one");
    assert_eq!(published["task"]["id"], "board-one");
    assert_eq!(published["task"]["status"], "pending");
    assert_eq!(published["task"]["owner"], master);
    assert_eq!(published["task"]["title"], "任务板接口");
    assert_eq!(published["task"]["revision"], 1);
    let board = fixture.run_public(&["board", "show"], THREAD_B);
    let tasks = board["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], "board-one");
    assert!(!serde_json::to_string(&board).unwrap().contains("\"token\""));
    let status = fixture.run_public(&["task", "status", "board-one"], THREAD_B);
    assert_eq!(status["status"], "pending");
    assert_eq!(status["owner"], master);
    assert!(!status["completion_missing"].is_null());
    for absent in [
        "delivery",
        "review",
        "integration",
        "merge",
        "cleanup",
        "keepalive",
    ] {
        assert!(
            status.get(absent).is_none(),
            "empty lifecycle or unrelated history {absent}: {status}"
        );
    }
    let bypass = fixture.command_public(
        &["task", "update", "board-one", "--status", "working"],
        THREAD_A,
    );
    assert!(!bypass.status.success(), "pending task must require accept");
    let peer_publish = fixture.command_public(
        &[
            "board",
            "publish",
            "peer-published",
            "--title",
            "x",
            "--description",
            "x",
            "--delivery-condition",
            "x",
            "--test-condition",
            "x",
        ],
        THREAD_B,
    );
    assert!(
        peer_publish.status.success(),
        "registered peer may publish pending work: {peer_publish:?}"
    );
}

#[test]
fn peer_publication_can_be_claimed_by_self_or_master() {
    let (fixture, master, peer) = setup();
    let peer_publish = fixture.run_public(
        &[
            "board",
            "publish",
            "peer-owned-queue",
            "--title",
            "peer 发布",
            "--description",
            "peer 队列",
            "--delivery-condition",
            "peer 交付",
            "--test-condition",
            "CLI 黑盒通过",
        ],
        THREAD_B,
    );
    assert_eq!(peer_publish["task"]["owner"], peer);
    assert_eq!(peer_publish["task"]["status"], "pending");
    let master_claim = fixture.run_public(
        &[
            "board",
            "claim",
            "peer-owned-queue",
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    assert_eq!(master_claim["task"]["owner"], master);
    assert_eq!(master_claim["task"]["status"], "working");
    assert_eq!(master_claim["task"]["revision"], 2);

    let own_publish = fixture.run_public(
        &[
            "board",
            "publish",
            "peer-self-claim",
            "--title",
            "后续工作",
            "--description",
            "排队登记",
            "--delivery-condition",
            "完成下一工作",
            "--test-condition",
            "CLI 黑盒通过",
        ],
        THREAD_B,
    );
    assert_eq!(own_publish["task"]["owner"], peer);
    let own_claim = fixture.run_public(
        &[
            "board",
            "claim",
            "peer-self-claim",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    assert_eq!(own_claim["task"]["owner"], peer);
    assert_eq!(own_claim["task"]["publisher"], peer);
    assert_eq!(own_claim["task"]["status"], "working");

    // The publisher now owns the task as working work, so a non-publisher
    // cannot claim the same task at the observed revision.
    let non_publisher_reclaim = fixture.command_public(
        &[
            "board",
            "claim",
            "peer-self-claim",
            "--expected-revision",
            "2",
        ],
        THREAD_A,
    );
    assert!(
        !non_publisher_reclaim.status.success(),
        "a non-publisher cannot claim work the publisher already owns: {non_publisher_reclaim:?}"
    );
}

#[test]
fn admitted_ticket_reviewer_change_keeps_delivered_then_rework_reopens_execute_phase() {
    let (fixture, _, peer) = setup();
    let id = "reviewer-change-rework";
    publish(&fixture, id);
    let claimed = fixture.run_public(
        &["board", "claim", id, "--expected-revision", "1"],
        THREAD_B,
    );
    assert_eq!(claimed["task"]["owner"], peer);
    fixture.run_public(
        &[
            "board",
            "start",
            id,
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    let delivered = fixture.run_public(
        &[
            "task",
            "deliver",
            id,
            "--no-candidate",
            "--evidence",
            "initial delivery",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    assert_eq!(delivered["status"], "delivered");
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let revision = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .unwrap()["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let policy = fixture.run_public(
        &[
            "board",
            "policy",
            "--scope",
            "task",
            "--id",
            id,
            "--reviewer",
            "ReviewerX",
            "--expected-revision",
            &revision,
        ],
        THREAD_A,
    );
    // A reviewer rebind is a routing change, not a typed rework decision. The
    // delivered task stays delivered and points at the new reviewer.
    assert_eq!(policy["task"]["status"], "delivered");
    assert_eq!(
        policy["task"]["next_step"],
        "await review by ReviewerX; use collab task review --accept or --rework"
    );
    // A typed rework decision, not the reviewer rebind, is what reopens the
    // execute phase so the owner can redo the work.
    let rework = fixture.run_public(&["task", "update", id, "--status", "rework"], THREAD_B);
    assert_eq!(rework["status"], "rework");
    let rework_board = fixture.run_public(&["board", "show"], THREAD_B);
    let rework_revision = rework_board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .unwrap()["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let started = fixture.run_public(
        &[
            "board",
            "start",
            id,
            "--expected-revision",
            &rework_revision,
            "--attempt",
            "2",
        ],
        THREAD_B,
    );
    assert_eq!(started["task"]["status"], "rework");
    let redelivered = fixture.run_public(
        &[
            "task",
            "deliver",
            id,
            "--no-candidate",
            "--evidence",
            "redelivery after reviewer change",
            "--attempt",
            "2",
        ],
        THREAD_B,
    );
    assert_eq!(redelivered["status"], "delivered");
}

#[test]
fn reviewer_rebind_keeps_delivered_task_in_delivered_with_updated_next_step() {
    let (fixture, _, peer) = setup();
    let id = "reviewer-rebind-delivered";
    publish(&fixture, id);
    fixture.run_public(
        &["board", "claim", id, "--expected-revision", "1"],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            id,
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "task",
            "deliver",
            id,
            "--no-candidate",
            "--evidence",
            "first delivery",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let revision = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .unwrap()["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let rebind = fixture.run_public(
        &[
            "board",
            "policy",
            "--scope",
            "task",
            "--id",
            id,
            "--reviewer",
            "ReviewerX",
            "--expected-revision",
            &revision,
        ],
        THREAD_A,
    );
    assert_eq!(rebind["task"]["status"], "delivered");
    assert_eq!(
        rebind["task"]["next_step"],
        "await review by ReviewerX; use collab task review --accept or --rework"
    );
    assert_eq!(rebind["task"]["reviewer_name"], "ReviewerX");
    assert_eq!(
        peer,
        fixture.run_public(&["board", "show"], THREAD_B)["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == id)
            .unwrap()["owner"]
    );
}

#[test]
fn master_can_set_ownerless_task_into_typed_wait_and_non_master_cannot() {
    let (mut fixture, _master, _peer) = setup();
    // A live third worker owns the blocking resource, so the wait has a real
    // responsible actor.
    fixture.run_public(&["context"], THREAD_C);
    let blocker = "blocking-resource";
    publish(&fixture, blocker);
    fixture.run_public(
        &["board", "claim", blocker, "--expected-revision", "1"],
        THREAD_C,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            blocker,
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        THREAD_C,
    );

    let id = "ownerless-wait";
    publish(&fixture, id);
    fixture.run_public(
        &["board", "claim", id, "--expected-revision", "1"],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            id,
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "task",
            "deliver",
            id,
            "--no-candidate",
            "--evidence",
            "ownerless evidence",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );

    // Drop the current binding and point the persisted owner at a name that has
    // no holder. The board must then project owner empty and needs_reassignment
    // while the delivery evidence stays in task_lifecycle. The two tasks share
    // a feature so the wait has a real matching resource.
    fixture.update_current_state(|state| {
        let task = state["tasks"][id].as_object_mut().unwrap();
        task.insert("owner".into(), json!("ghost"));
        task.insert("status".into(), json!("rework"));
        task.insert("feature_id".into(), json!("shared"));
        state["task_assignments"]
            .as_object_mut()
            .unwrap()
            .remove(id);
        state["tasks"][blocker]["feature_id"] = json!("shared");
    });

    let view = fixture.run_public(&["board", "show"], THREAD_A);
    let row = view["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .unwrap()
        .clone();
    assert_eq!(row["owner"], "");
    assert_eq!(row["needs_reassignment"], true);

    // A non-master peer cannot reach the repair path.
    let refused = fixture.command_public(&["task", "wait", id, "--for", blocker], THREAD_B);
    assert!(
        !refused.status.success(),
        "non-master must not repair an ownerless task: {}",
        String::from_utf8_lossy(&refused.stdout)
    );

    // The current master records the durable typed wait without losing the
    // owner row, the next-step contract, or the delivery evidence.
    fixture.run_public(&["task", "wait", id, "--for", blocker], THREAD_A);
    let after = fixture.current_state();
    assert_eq!(after["tasks"][id]["status"], "waiting");
    assert_eq!(
        after["tasks"][id]["next_step"],
        format!("WAITING_FOR={blocker}")
    );
    assert_eq!(after["tasks"][id]["owner"], "ghost");
    assert_eq!(
        after["task_lifecycle"][id]["delivery_evidence"],
        "ownerless evidence"
    );
}

/// Public CLI black box for c947cbe/10a0d54 release routing: a master records a
/// durable wait for a task whose persisted owner is unavailable, then releases
/// the blocking resource. The release notification must target the durable
/// waiter recorded in WaitSpec, not the unavailable persisted owner, while the
/// owner row and delivery evidence stay unchanged.
#[test]
fn ownerless_wait_release_notifies_the_durable_waiter_after_blocker_cleanup() {
    let (mut fixture, master, _peer) = setup();
    // A live third worker owns the blocking resource so the wait has a real
    // responsible actor.
    fixture.run_public(&["context"], THREAD_C);
    let blocker = "ownerless-release-blocker";
    publish(&fixture, blocker);
    fixture.run_public(
        &["board", "claim", blocker, "--expected-revision", "1"],
        THREAD_C,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            blocker,
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        THREAD_C,
    );

    let id = "ownerless-release-waiter";
    publish(&fixture, id);
    fixture.run_public(
        &["board", "claim", id, "--expected-revision", "1"],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            id,
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "task",
            "deliver",
            id,
            "--no-candidate",
            "--evidence",
            "ownerless evidence",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );

    // Point the persisted owner at a name with no holder while the delivery
    // evidence stays in task_lifecycle. Both tasks share a feature so the
    // blocking task holds a matching active resource.
    fixture.update_current_state(|state| {
        let task = state["tasks"][id].as_object_mut().unwrap();
        task.insert("owner".into(), json!("ghost"));
        task.insert("status".into(), json!("rework"));
        task.insert("feature_id".into(), json!("shared-release"));
        state["task_assignments"]
            .as_object_mut()
            .unwrap()
            .remove(id);
        state["tasks"][blocker]["feature_id"] = json!("shared-release");
    });

    // The current master records the durable typed wait and subscribes for the
    // release event the wait response points to.
    fixture.run_public(&["task", "wait", id, "--for", blocker], THREAD_A);
    let waiting = fixture.current_state();
    assert_eq!(waiting["tasks"][id]["status"], "waiting");
    assert_eq!(waiting["tasks"][id]["owner"], "ghost");
    assert_eq!(waiting["tasks"][id]["wait"]["waiter"], master);
    let wait_revision = fixture.run_public(&["board", "show"], THREAD_A)["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .unwrap()["revision"]
        .clone();
    let subscribed = fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "resource-released",
            "--subject",
            blocker,
        ],
        THREAD_A,
    );
    assert_eq!(subscribed["subscription"]["event"], "resource-released");

    // Force close the blocker, then finalize its cleanup. Finalization is the
    // legal completion path that releases dependents without changing the
    // waiter's owner or evidence.
    fixture.run_public(
        &[
            "task",
            "close",
            blocker,
            "--force",
            "--reason",
            "isolated fixture blocker released",
        ],
        THREAD_A,
    );
    fixture.run_public(&["task", "finalize-cleanup", blocker], THREAD_A);

    let after = fixture.current_state();
    assert_eq!(after["tasks"][id]["status"], "blocked");
    assert_eq!(after["tasks"][id]["owner"], "ghost");
    assert_eq!(
        after["task_lifecycle"][id]["delivery_evidence"],
        "ownerless evidence"
    );
    assert_eq!(
        after["tasks"][id]["next_step"],
        format!(
            "RESOURCE_RELEASED={blocker} recheck conflicts, then resume only after Server confirms free"
        )
    );
    let releases: Vec<&Value> = after["msgs"]
        .as_object()
        .unwrap()
        .values()
        .filter(|message| message["subject"] == format!("released:{blocker}"))
        .collect();
    assert_eq!(
        releases.len(),
        1,
        "exactly one release notification: {releases:?}"
    );
    assert_eq!(
        releases[0]["to"], master,
        "release notification must target the durable waiter, not the unavailable owner: {releases:?}"
    );
    assert_ne!(releases[0]["to"], "ghost");
    let release_revision = fixture.run_public(&["board", "show"], THREAD_A)["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .unwrap()["revision"]
        .clone();
    println!(
        "PROBE ownerless-release: revision_after_wait={wait_revision} \
         revision_after_release={release_revision} waiter={master} \
         notification_to={} owner_after=ghost evidence_preserved=true",
        releases[0]["to"]
    );
}

#[test]
fn admitted_ticket_owner_rework_status_reopens_execute_phase() {
    let (fixture, _, peer) = setup();
    let id = "owner-rework-cycle";
    publish(&fixture, id);
    fixture.run_public(
        &["board", "claim", id, "--expected-revision", "1"],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            id,
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    let delivered = fixture.run_public(
        &[
            "task",
            "deliver",
            id,
            "--no-candidate",
            "--evidence",
            "first delivery",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    assert_eq!(delivered["status"], "delivered");
    let reworked = fixture.run_public(&["task", "update", id, "--status", "rework"], THREAD_B);
    assert_eq!(reworked["status"], "rework");
    let board = fixture.run_public(&["board", "show"], THREAD_B);
    let rework_revision = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .unwrap()["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    fixture.run_public(
        &[
            "board",
            "start",
            id,
            "--expected-revision",
            &rework_revision,
            "--attempt",
            "2",
        ],
        THREAD_B,
    );
    let redelivered = fixture.run_public(
        &[
            "task",
            "deliver",
            id,
            "--no-candidate",
            "--evidence",
            "redelivery after owner rework",
            "--attempt",
            "2",
        ],
        THREAD_B,
    );
    assert_eq!(redelivered["status"], "delivered");
    assert_eq!(redelivered["evidence"], "redelivery after owner rework");
    let status = fixture.run_public(&["task", "status", id], THREAD_B);
    assert_eq!(status["status"], "delivered");
}

#[test]
fn concurrent_claims_admit_exactly_one_and_stale_claim_preserves_state() {
    let (fixture, master, peer) = setup();
    publish(&fixture, "claim-race");
    let fixture = std::sync::Arc::new(fixture);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let first_barrier = std::sync::Arc::clone(&barrier);
    let second_barrier = barrier.clone();
    let first_fixture = std::sync::Arc::clone(&fixture);
    let second_fixture = first_fixture.clone();
    let first = std::thread::spawn(move || {
        first_barrier.wait();
        first_fixture.command_public(
            &["board", "claim", "claim-race", "--expected-revision", "1"],
            THREAD_A,
        )
    });
    let second = std::thread::spawn(move || {
        second_barrier.wait();
        second_fixture.command_public(
            &["board", "claim", "claim-race", "--expected-revision", "1"],
            THREAD_B,
        )
    });
    let first = first.join().unwrap();
    let second = second.join().unwrap();
    let successes = [&first, &second]
        .iter()
        .filter(|output| output.status.success())
        .count();
    assert_eq!(
        successes, 1,
        "exactly one concurrent claim may succeed: {first:?} {second:?}"
    );
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "claim-race")
        .unwrap();
    assert_eq!(task["status"], "working");
    let actual_owner = task["owner"].as_str().unwrap();
    assert!(actual_owner == master || actual_owner == peer);
    assert_eq!(task["revision"], 2);
    let stale = fixture.command_public(
        &["board", "claim", "claim-race", "--expected-revision", "1"],
        THREAD_A,
    );
    assert!(!stale.status.success(), "stale claim must fail explicitly");
    let after = fixture.run_public(&["task", "status", "claim-race"], THREAD_A);
    assert_eq!(after["owner"], actual_owner);
    assert_eq!(after["status"], "working");
}

#[test]
fn blocked_task_keeps_responsibility_but_does_not_block_next_work_or_resume() {
    let (fixture, _, peer) = setup();
    publish(&fixture, "blocked-first");
    fixture.run_public(
        &[
            "board",
            "claim",
            "blocked-first",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "board",
            "update",
            "blocked-first",
            "--expected-revision",
            "2",
            "--status",
            "blocked",
            "--next",
            "等待外部输入",
        ],
        THREAD_B,
    );
    // A member block is a pending application. The master confirms the
    // external gate in the same request; approval commits the blocked state
    // and advances the board revision exactly as the direct transition used to.
    fixture.run_public(
        &[
            "task",
            "update",
            "blocked-first",
            "--blocker-action",
            "approve",
            "--decision-reason",
            "confirmed external input gate",
        ],
        THREAD_A,
    );
    let queued = fixture.run_public(
        &[
            "board",
            "publish",
            "blocked-next",
            "--title",
            "阻塞后续",
            "--description",
            "继续无冲突工作",
            "--delivery-condition",
            "第二项完成",
            "--test-condition",
            "CLI 黑盒通过",
        ],
        THREAD_B,
    );
    assert_eq!(queued["task"]["status"], "pending");
    fixture.run_public(
        &["board", "claim", "blocked-next", "--expected-revision", "1"],
        THREAD_B,
    );
    let premature_resume = fixture.command_public(
        &[
            "board",
            "update",
            "blocked-first",
            "--expected-revision",
            "3",
            "--status",
            "working",
        ],
        THREAD_B,
    );
    assert!(
        premature_resume.status.success(),
        "a peer can orchestrate multiple independent goals: {premature_resume:?}"
    );
    fixture.run_public(
        &[
            "board",
            "update",
            "blocked-next",
            "--expected-revision",
            "2",
            "--status",
            "cancelled",
        ],
        THREAD_B,
    );
    let resumed = fixture.run_public(
        &[
            "board",
            "update",
            "blocked-first",
            "--expected-revision",
            "4",
            "--status",
            "working",
            "--next",
            "外部输入已恢复",
        ],
        THREAD_B,
    );
    assert_eq!(resumed["task"]["owner"], peer);
    assert_eq!(resumed["task"]["status"], "working");
}

#[test]
fn claimed_and_queued_tasks_survive_daemon_replay() {
    let (mut fixture, _, peer) = setup();
    publish(&fixture, "replay-active");
    fixture.run_public(
        &[
            "board",
            "claim",
            "replay-active",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    let queued = fixture.run_public(
        &[
            "board",
            "publish",
            "replay-queued",
            "--title",
            "排队任务",
            "--description",
            "daemon 重启后仍待领取",
            "--delivery-condition",
            "恢复后领取",
            "--test-condition",
            "CLI 黑盒通过",
        ],
        THREAD_B,
    );
    assert_eq!(queued["task"]["revision"], 1);

    fixture.restart_daemon();

    let active = fixture.run_public(&["task", "status", "replay-active"], THREAD_A);
    assert_eq!(active["owner"], peer);
    assert_eq!(active["status"], "working");
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let tasks = board["tasks"].as_array().unwrap();
    let active = tasks
        .iter()
        .find(|task| task["id"] == "replay-active")
        .unwrap();
    let queued = tasks
        .iter()
        .find(|task| task["id"] == "replay-queued")
        .unwrap();
    assert_eq!(active["status"], "working");
    assert_eq!(active["revision"], 2);
    assert_eq!(queued["status"], "pending");
    assert_eq!(queued["owner"], peer);
    assert_eq!(queued["publisher"], peer);
    assert_eq!(queued["revision"], 1);
}

#[test]
fn board_owner_update_is_versioned_and_peer_tasks_remain_independent() {
    let (fixture, _, _) = setup();
    fixture.run_public(
        &["task", "register", "peer-own", "--next", "实现接口"],
        THREAD_B,
    );
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "peer-own")
        .unwrap();
    let revision = task["revision"].as_u64().unwrap().to_string();
    let updated = fixture.run_public(
        &[
            "board",
            "update",
            "peer-own",
            "--expected-revision",
            &revision,
            "--status",
            "verifying",
            "--next",
            "执行黑盒回归",
        ],
        THREAD_B,
    );
    assert_eq!(updated["task"]["status"], "verifying");
    assert!(updated["task"]["revision"].as_u64().unwrap() > revision.parse::<u64>().unwrap());
    let stale = fixture.command_public(
        &[
            "board",
            "update",
            "peer-own",
            "--expected-revision",
            &revision,
            "--next",
            "旧进度",
        ],
        THREAD_B,
    );
    assert!(!stale.status.success());
    let master_write = fixture.command_public(
        &[
            "board",
            "update",
            "peer-own",
            "--expected-revision",
            &updated["task"]["revision"].as_u64().unwrap().to_string(),
            "--next",
            "代写进度",
        ],
        THREAD_A,
    );
    assert!(!master_write.status.success());
    let task_after = fixture.run_public(&["task", "status", "peer-own"], THREAD_A);
    assert_eq!(task_after["next_step"], "执行黑盒回归");
}

#[test]
fn board_invitation_transfers_owner_only_after_peer_acceptance() {
    let (fixture, master, peer) = setup();
    publish(&fixture, "invitation");
    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            "invitation",
            "--to",
            &peer,
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    assert_eq!(invited["task"]["owner"], master);
    assert_eq!(invited["task"]["status"], "invited");
    assert_eq!(invited["task"]["revision"], 2);
    assert_eq!(invited["consumed"], false);
    let outsider = fixture.command_public(
        &[
            "board",
            "respond",
            "invitation",
            "--accept",
            "--expected-revision",
            "2",
        ],
        THREAD_A,
    );
    assert!(!outsider.status.success());
    let declined = fixture.run_public(
        &[
            "board",
            "respond",
            "invitation",
            "--decline",
            "--expected-revision",
            "2",
            "--reason",
            "保留独立工作",
        ],
        THREAD_B,
    );
    assert_eq!(declined["task"]["owner"], master);
    assert_eq!(declined["task"]["status"], "pending");
    assert_eq!(declined["task"]["revision"], 3);
    fixture.run_public(
        &[
            "board",
            "invite",
            "invitation",
            "--to",
            &peer,
            "--expected-revision",
            "3",
        ],
        THREAD_A,
    );
    let accepted = fixture.run_public(
        &[
            "board",
            "respond",
            "invitation",
            "--accept",
            "--expected-revision",
            "4",
        ],
        THREAD_B,
    );
    assert_eq!(accepted["task"]["owner"], peer);
    assert_eq!(accepted["task"]["status"], "working");
    assert_eq!(accepted["task"]["revision"], 5);
}

#[test]
fn working_task_can_be_reassigned_and_decline_or_withdraw_preserves_owner_status() {
    let (fixture, master, peer) = setup();
    publish(&fixture, "working-reassign");
    let claimed = fixture.run_public(
        &[
            "board",
            "claim",
            "working-reassign",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    assert_eq!(claimed["task"]["owner"], peer);
    assert_eq!(claimed["task"]["status"], "working");

    let carol = fixture.run_public(&["context"], THREAD_C)["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(carol, master);

    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            "working-reassign",
            "--to",
            &carol,
            "--expected-revision",
            "2",
        ],
        THREAD_A,
    );
    assert_eq!(invited["task"]["owner"], peer);
    assert_eq!(invited["task"]["status"], "working");
    assert_eq!(invited["task"]["invited_peer"], carol);
    let duplicate = fixture.command_public(
        &[
            "board",
            "invite",
            "working-reassign",
            "--to",
            &carol,
            "--expected-revision",
            "3",
        ],
        THREAD_A,
    );
    assert!(
        !duplicate.status.success(),
        "one task keeps one pending invitation"
    );
    let stale = fixture.command_public(
        &[
            "board",
            "respond",
            "working-reassign",
            "--accept",
            "--expected-revision",
            "2",
        ],
        THREAD_C,
    );
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("BOARD_STALE_REVISION"));

    let declined = fixture.run_public(
        &[
            "board",
            "respond",
            "working-reassign",
            "--decline",
            "--expected-revision",
            "3",
            "--reason",
            "not this round",
        ],
        THREAD_C,
    );
    assert_eq!(declined["task"]["owner"], peer);
    assert_eq!(declined["task"]["status"], "working");
    assert_eq!(declined["task"]["revision"], 4);

    let reinvited = fixture.run_public(
        &[
            "board",
            "invite",
            "working-reassign",
            "--to",
            &carol,
            "--expected-revision",
            "4",
        ],
        THREAD_A,
    );
    assert_eq!(reinvited["task"]["status"], "working");
    let withdrawn = fixture.run_public(
        &[
            "board",
            "withdraw",
            "working-reassign",
            "--expected-revision",
            "5",
            "--reason",
            "keep current owner",
        ],
        THREAD_A,
    );
    assert_eq!(withdrawn["task"]["owner"], peer);
    assert_eq!(withdrawn["task"]["status"], "working");
    assert_eq!(withdrawn["task"]["revision"], 6);

    let durable = fixture.run_public(&["task", "status", "working-reassign"], THREAD_A);
    assert_eq!(durable["owner"], peer);
    assert_eq!(durable["status"], "working");
}

#[test]
fn offline_name_holder_projects_empty_owner_then_name_reassignment_preserves_work() {
    resource_reassignment_consumer("accepted");
}

#[test]
fn working_resource_reassignment_allows_new_owner_to_resume() {
    resource_reassignment_consumer("working");
}

fn resource_reassignment_consumer(status: &str) {
    let (mut fixture, master, peer) = setup();
    publish(&fixture, "reassign-work");
    fixture.run_public(
        &[
            "board",
            "claim",
            "reassign-work",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    let carol = fixture.run_public(&["context"], THREAD_C)["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let project_scope = std::fs::canonicalize(&fixture.project)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let named = fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Carol","authorization":"fixture names the real reassignment recipient"}"#], THREAD_C);
    assert_eq!(named["name"], "Carol");
    assert_eq!(named["identity"]["worker_id"], carol);
    let worktree = "/tmp/collab-t5-reassign-worktree";
    fixture.update_current_state(|state| {
        state["tasks"]["reassign-work"] = json!({
                "id": "reassign-work",
                "owner": peer,
                "created_by": master,
                "feature_id": "reassign-feature",
                "worktree_path": worktree,
                "branch": "codex/reassign-work",
                "base_commit": "base-reassign",
                "priority": "p1",
                "status": status,
                "next_step": "integrate accepted candidate",
                "wait": null,
                "created_ms": 1,
                "updated_ms": 1000
        });
        state["task_lifecycle"]["reassign-work"] = json!({
                "delivery_evidence": "cargo test --test board green",
                "delivered_ms": 900,
                "delivery_commit": "candidate-reassign",
                "review_evidence": "independent review pass",
                "reviewer": master,
                "reviewed_ms": 950
        });
        state["worktree_bindings"]["binding-reassign-work"] = json!({
                "worktree_root": worktree,
                "owning_project_scope": project_scope,
                "task_id": "reassign-work",
                "owner_agent_id": peer,
                "binding_id": "binding-reassign-work",
                "base_commit": "base-reassign"
        });
        state["pending_merges"]["reassign-work"] = json!({
                "task_id": "reassign-work",
                "owner": peer,
                "requested_by": master,
                "requested_ms": 950,
                "candidate_commit": "candidate-reassign"
        });
        state["task_assignments"]["reassign-work"] = json!({
                "task_id": "reassign-work",
                "name": "Bob",
                "session_id": THREAD_B,
                "assignment_revision": 1,
                "updated_ms": 1000
        });
        state["agent_presence"][&project_scope][&peer] = json!({
                "project_scope": project_scope,
                "worker_id": peer,
                "session_id": THREAD_B,
                "presence": "offline",
                "reason": "fixture: agent exited",
                "observed_ms": 1001
        });
        state["agent_presence"][&project_scope][&carol] = json!({
                "project_scope": project_scope,
                "worker_id": carol,
                "session_id": THREAD_C,
                "presence": "online",
                "reason": "fixture: current session observed",
                "observed_ms": 1001
        });
    });

    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "reassign-work")
        .unwrap();
    assert_eq!(task["owner"], "", "{task}");
    assert_eq!(task["owner_display_name"], "", "{task}");
    assert_eq!(task["status"], status);
    assert_eq!(task["owner_name"], "Bob");
    assert!(task["owner_session_id"].is_null());
    assert_eq!(task["needs_reassignment"], true);
    assert_eq!(task["delivery_evidence"], "cargo test --test board green");
    assert_eq!(task["review_evidence"], "independent review pass");
    assert_eq!(task["cleanup_status"], "pending");
    let revision = task["revision"].as_u64().unwrap();

    let durable = fixture.run_public(&["task", "status", "reassign-work"], THREAD_A);
    assert_eq!(durable["owner"], peer);
    assert_eq!(durable["status"], status);
    assert_eq!(durable["worktree"], worktree);
    assert_eq!(durable["branch"], "codex/reassign-work");
    assert_eq!(
        durable["delivery"]["evidence"],
        "cargo test --test board green"
    );
    assert_eq!(durable["review"]["evidence"], "independent review pass");
    assert_eq!(durable["merge"]["owner"], peer);
    assert_eq!(durable["merge"]["pending"], true);

    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            "reassign-work",
            "--to",
            "Carol",
            "--expected-revision",
            &revision.to_string(),
        ],
        THREAD_A,
    );
    assert_eq!(invited["task"]["status"], status);
    assert_eq!(invited["task"]["invited_peer"], carol);
    let declined = fixture.run_public(
        &[
            "board",
            "respond",
            "reassign-work",
            "--decline",
            "--reason",
            "later",
            "--expected-revision",
            &invited["task"]["revision"].to_string(),
        ],
        THREAD_C,
    );
    assert_eq!(declined["task"]["status"], status);
    let retained = fixture.run_public(&["task", "status", "reassign-work"], THREAD_A);
    assert_eq!(retained["owner"], peer);
    assert_eq!(retained["merge"]["owner"], peer);
    assert_eq!(retained["delivery"], durable["delivery"]);
    assert_eq!(retained["review"], durable["review"]);
    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            "reassign-work",
            "--to",
            "Carol",
            "--expected-revision",
            &declined["task"]["revision"].to_string(),
        ],
        THREAD_A,
    );
    let withdrawn = fixture.run_public(
        &[
            "board",
            "withdraw",
            "reassign-work",
            "--reason",
            "later",
            "--expected-revision",
            &invited["task"]["revision"].to_string(),
        ],
        THREAD_A,
    );
    assert_eq!(withdrawn["task"]["status"], status);
    let retained = fixture.run_public(&["task", "status", "reassign-work"], THREAD_A);
    assert_eq!(retained["owner"], peer);
    assert_eq!(retained["merge"], durable["merge"]);
    assert_eq!(retained["worktree"], durable["worktree"]);
    assert_eq!(retained["delivery"], durable["delivery"]);
    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            "reassign-work",
            "--to",
            "Carol",
            "--expected-revision",
            &withdrawn["task"]["revision"].to_string(),
        ],
        THREAD_A,
    );
    let invitation_revision = invited["task"]["revision"].as_u64().unwrap();

    let accepted = fixture.run_public(
        &[
            "board",
            "respond",
            "reassign-work",
            "--accept",
            "--expected-revision",
            &invitation_revision.to_string(),
        ],
        THREAD_C,
    );
    assert_eq!(accepted["task"]["owner"], carol);
    assert_eq!(accepted["task"]["status"], status);
    assert_eq!(accepted["task"]["owner_name"], "Carol");
    assert_eq!(accepted["task"]["owner_session_id"], THREAD_C);
    assert_eq!(accepted["task"]["needs_reassignment"], false);
    assert_eq!(
        accepted["task"]["delivery_evidence"],
        "cargo test --test board green"
    );
    assert_eq!(
        accepted["task"]["review_evidence"],
        "independent review pass"
    );

    let reassigned = fixture.run_public(&["task", "status", "reassign-work"], THREAD_A);
    assert_eq!(reassigned["owner"], carol);
    assert_eq!(reassigned["status"], status);
    assert_eq!(reassigned["worktree"], worktree);
    assert_eq!(reassigned["branch"], "codex/reassign-work");
    assert_eq!(reassigned["feature_id"], "reassign-feature");
    assert_eq!(
        reassigned["delivery"]["evidence"],
        "cargo test --test board green"
    );
    assert_eq!(reassigned["review"]["evidence"], "independent review pass");
    assert_eq!(reassigned["merge"]["owner"], carol);
    assert_eq!(reassigned["merge"]["pending"], true);

    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "reassign-work")
        .unwrap();
    assert_eq!(task["owner"], carol);
    assert_eq!(task["owner_name"], "Carol");
    assert_eq!(task["needs_reassignment"], false);
    if status == "working" {
        let revision = task["revision"].as_u64().unwrap();
        fixture.run_public(
            &[
                "board",
                "update",
                "reassign-work",
                "--expected-revision",
                &revision.to_string(),
                "--status",
                "blocked",
            ],
            THREAD_C,
        );
        let blocked = fixture.run_public(&["board", "show"], THREAD_C);
        let revision = blocked["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == "reassign-work")
            .unwrap()["revision"]
            .as_u64()
            .unwrap();
        let resumed = fixture.run_public(
            &[
                "board",
                "update",
                "reassign-work",
                "--expected-revision",
                &revision.to_string(),
                "--status",
                "working",
            ],
            THREAD_C,
        );
        assert_eq!(resumed["task"]["owner"], carol);
        assert_eq!(resumed["task"]["status"], "working");
    }
}

#[test]
fn closed_invitation_cannot_transfer_and_master_withdraw_only_clears_metadata() {
    for status in ["closed", "cancelled"] {
        let (mut fixture, master, peer) = setup();
        publish(&fixture, "closed-offer");
        let forbidden = fixture.command_public(
            &[
                "board",
                "invite",
                "closed-offer",
                "--to",
                &master,
                "--expected-revision",
                "1",
            ],
            THREAD_B,
        );
        assert!(!forbidden.status.success());
        assert!(String::from_utf8_lossy(&forbidden.stderr).contains("BOARD_MASTER_REQUIRED"));
        let invited = fixture.run_public(
            &[
                "board",
                "invite",
                "closed-offer",
                "--to",
                &peer,
                "--expected-revision",
                "1",
            ],
            THREAD_A,
        );
        fixture.update_current_state(|state| {
            state["tasks"]["closed-offer"]["status"] = json!(status);
            state["tasks"]["closed-offer"]["next_step"] = json!("historical");
            state["tasks"]["closed-offer"]["updated_ms"] = json!(2);
            state["board_details"]["closed-offer"]["revision"] =
                json!(invited["task"]["revision"].as_u64().unwrap() + 1);
        });
        let board = fixture.run_public(&["board", "show"], THREAD_A);
        assert!(
            !board["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|task| task["id"] == "closed-offer"),
            "closed work must leave the board: {board}"
        );
        let historical = fixture.run_public(&["task", "status", "closed-offer"], THREAD_A);
        assert_eq!(historical["status"], status);
        assert_eq!(historical["owner"], master);
        // The stopped current-state fixture advances the invitation revision once.
        let revision = (invited["task"]["revision"].as_u64().unwrap() + 1).to_string();
        let response = fixture.command_public(
            &[
                "board",
                "respond",
                "closed-offer",
                "--accept",
                "--expected-revision",
                &revision,
            ],
            THREAD_B,
        );
        assert!(!response.status.success());
        assert!(String::from_utf8_lossy(&response.stderr).contains("BOARD_TASK_CLOSED"));
        let response = fixture.command_public(
            &[
                "board",
                "invite",
                "closed-offer",
                "--to",
                &peer,
                "--expected-revision",
                &revision,
            ],
            THREAD_A,
        );
        assert!(!response.status.success());
        let withdrawn = fixture.run_public(
            &[
                "board",
                "withdraw",
                "closed-offer",
                "--expected-revision",
                &revision,
                "--reason",
                "expire invitation",
            ],
            THREAD_A,
        );
        assert_eq!(withdrawn["task"]["status"], status);
        assert_eq!(withdrawn["task"]["owner"], master);
        assert!(withdrawn["task"]["invited_peer"].is_null());
        assert_eq!(withdrawn["task"]["next_step"], "historical");
    }
}

#[test]
fn peer_may_start_independent_work_while_an_invitation_is_pending() {
    let (fixture, master, peer) = setup();
    publish(&fixture, "offer-one");
    publish(&fixture, "offer-two");
    fixture.run_public(
        &[
            "board",
            "invite",
            "offer-one",
            "--to",
            &peer,
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    let second = fixture.command_public(
        &[
            "board",
            "invite",
            "offer-two",
            "--to",
            &peer,
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    assert!(
        second.status.success(),
        "a peer may receive multiple goal invitations: {second:?}"
    );
    fixture.run_public(
        &[
            "task",
            "register",
            "independent",
            "--next",
            "用户自己的任务",
        ],
        THREAD_B,
    );
    let accept = fixture.command_public(
        &[
            "board",
            "respond",
            "offer-one",
            "--accept",
            "--expected-revision",
            "2",
        ],
        THREAD_B,
    );
    assert!(
        accept.status.success(),
        "a peer may accept another goal: {accept:?}"
    );
    let declined = fixture.run_public(
        &[
            "board",
            "respond",
            "offer-two",
            "--decline",
            "--expected-revision",
            "2",
            "--reason",
            "正在自己的任务上工作",
        ],
        THREAD_B,
    );
    assert_eq!(declined["task"]["owner"], master);
    let independent = fixture.run_public(&["task", "status", "independent"], THREAD_B);
    assert_eq!(independent["owner"], peer);
    assert_eq!(independent["next_step"], "用户自己的任务");
}

#[test]
fn role_patrol_context_reminders_follow_master_and_peer_roles_across_restoration() {
    let mut fixture = AppFixture::new();
    let first_peer = fixture.run_public(&["context"], THREAD_B);
    let peer_keywords = [
        "patrol",
        "subscription",
        "scope",
        "master",
        "block",
        "non-overlapping bounded scopes",
        "close-before-open/sese",
        "canonical intake and deduplication",
        "owning project contract",
        "re-arm",
        "report channel",
        "stop conditions",
        "resumes and drives its own authorized work",
    ];
    assert_role_patrol_mentions("first peer context", &first_peer, "worker", &peer_keywords);

    fixture.run_public(&["context"], THREAD_A);
    let promoted = fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "isolated role patrol fixture promotion",
        ],
        THREAD_A,
    );
    let master_keywords = [
        "buglist",
        "p0",
        "p1",
        "peer",
        "progress",
        "worktree",
        "runtime",
        "cleanup",
        "registr",
        "promot",
        "restor",
        "inspect",
        "monitor",
        "event",
        "subscription",
        "only its own project",
        "canonical issue intake",
        "currently supported monitored events",
        "non-overlapping",
        "close-before-open/sese",
        "canonical intake and deduplication",
        "owning project contract",
        "goal",
        "deadline",
        "re-arm",
        "dispatch receipt",
        "report channel",
        "stop conditions",
    ];
    assert_role_patrol_mentions("master promotion", &promoted, "master", &master_keywords);
    let repeated_master = fixture.run_public(&["context"], THREAD_A);
    assert_role_patrol_mentions(
        "repeated master context",
        &repeated_master,
        "master",
        &master_keywords,
    );

    restore_online_master(&mut fixture, THREAD_B);
    let demoted_peer = fixture.run_public(&["context"], THREAD_A);
    assert_role_patrol_mentions(
        "demoted peer context",
        &demoted_peer,
        "worker",
        &peer_keywords,
    );

    let tasks_before = fixture.run_public(&["board", "show"], THREAD_A)["tasks"]
        .as_array()
        .expect("board show lists tasks")
        .len();
    assert_eq!(
        tasks_before, 0,
        "role context must not fabricate inspection tasks"
    );
    let master_subscriptions_before =
        notification_subscription_count(&fixture, THREAD_B, "master before restoration");
    let peer_subscriptions_before =
        notification_subscription_count(&fixture, THREAD_A, "peer before restoration");

    fixture.restart_daemon();

    let restored_master = fixture.run_public(&["context"], THREAD_B);
    assert_role_patrol_mentions(
        "restored master context",
        &restored_master,
        "master",
        &master_keywords,
    );
    let restored_peer = fixture.run_public(&["context"], THREAD_A);
    assert_role_patrol_mentions(
        "restored peer context",
        &restored_peer,
        "worker",
        &peer_keywords,
    );
    assert_eq!(
        notification_subscription_count(&fixture, THREAD_B, "master after restoration"),
        master_subscriptions_before,
        "context restoration must not create master patrol subscriptions"
    );
    assert_eq!(
        notification_subscription_count(&fixture, THREAD_A, "peer after restoration"),
        peer_subscriptions_before,
        "context restoration must not create peer patrol subscriptions"
    );
    let tasks_after = fixture.run_public(&["board", "show"], THREAD_A)["tasks"]
        .as_array()
        .expect("board show lists tasks")
        .len();
    assert_eq!(
        tasks_after, tasks_before,
        "context restoration must not fabricate board tasks"
    );
}

#[test]
fn role_patrol_board_invitation_is_durable_without_serialized_control_state() {
    let (fixture, master, peer) = setup();
    publish(&fixture, "role-patrol-invitation");
    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            "role-patrol-invitation",
            "--to",
            &peer,
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    assert_eq!(invited["task"]["owner"], master);
    assert_eq!(invited["task"]["status"], "invited");

    let received = fixture.run_public(&["recv", "--timeout", "1"], THREAD_B);
    let invitation = received["messages"]
        .as_array()
        .expect("recv lists durable messages")
        .iter()
        .find(|message| message["subject"] == "task-invitation:role-patrol-invitation")
        .expect("recipient receives the board invitation");
    assert_eq!(invitation["id"], invited["msg_id"]);
    assert_no_serialized_role_control("board invitation", invitation);

    let body = invitation["body"]
        .as_str()
        .expect("board invitation carries a text body");
    let body_lower = body.to_ascii_lowercase();
    assert!(
        !body.contains("Role task:") && !body.contains("Responsibilities:"),
        "invitation must carry its task contract without replaying the registered role brief: {body}"
    );
    for required in [
        "任务板接口",
        "公开协议测试",
        "接口行为通过",
        "cargo test --test appserver_two_tui_integration",
    ] {
        assert!(
            body.contains(required),
            "missing task contract {required}: {body}"
        );
    }
    assert!(
        !body_lower.contains("collab recv"),
        "the invitation body is already the recv payload; it must not require another read: {body}"
    );
    assert!(
        body.contains("collab board respond")
            && body.contains("--accept")
            && body.contains("--decline")
            && body.contains("--expected-revision"),
        "accepting or declining still carries the typed revision check: {body}"
    );

    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let task = board["tasks"]
        .as_array()
        .expect("board show lists tasks")
        .iter()
        .find(|task| task["id"] == "role-patrol-invitation")
        .expect("invited task remains visible");
    assert_eq!(task["owner"], master);
    assert_eq!(task["status"], "invited");

    let accepted = fixture.run_public(
        &[
            "board",
            "respond",
            "role-patrol-invitation",
            "--accept",
            "--expected-revision",
            "2",
        ],
        THREAD_B,
    );
    assert_eq!(accepted["task"]["owner"], peer);
    assert_eq!(accepted["task"]["status"], "working");
}

#[test]
fn recv_public_replay_keeps_payload_and_reply_without_delivery_diagnostics() {
    let (fixture, _, peer) = setup();
    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            &peer,
            "--subject",
            "receipt",
            "--reply-required",
            "body with records and query_result",
        ],
        THREAD_A,
    );
    let id = sent["msg_id"].as_str().unwrap();
    let first = fixture.run_public(
        &["recv", "--receive-id", "minimal-receipt", "--timeout", "0"],
        THREAD_B,
    );
    let message = first["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == id)
        .expect("received original message");
    assert_eq!(message["body"], "body with records and query_result");
    assert!(!message["reply_obligation"].is_null(), "{message}");
    for field in [
        "wake_attempt_count",
        "last_wake_attempt_ms",
        "retry_attempted",
    ] {
        assert!(
            message.get(field).is_none(),
            "delivery diagnostic {field}: {message}"
        );
    }
    let next = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            &peer,
            "--subject",
            "later",
            "leave this unread",
        ],
        THREAD_A,
    );
    let replay = fixture.run_public(
        &["recv", "--receive-id", "minimal-receipt", "--timeout", "0"],
        THREAD_B,
    );
    assert_eq!(replay["replayed"], true);
    assert_eq!(first["messages"], replay["messages"]);
    let later = fixture.run_public(&["msg", next["msg_id"].as_str().unwrap()], THREAD_B);
    assert_ne!(
        later["state"], "read",
        "replay consumed another batch: {later}"
    );
    let second = fixture.run_public(
        &["recv", "--receive-id", "minimal-next", "--timeout", "0"],
        THREAD_B,
    );
    assert!(second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["id"] == next["msg_id"]));
}

/// Public CLI black box for the accepted pending-close lifecycle: a live master
/// force-closes a pending task, a non-force close stays gated, and the pending
/// publisher cancels unbound work through the board update entry.
#[test]
fn pending_publisher_close_lifecycle_public_cli() {
    let (fixture, master, peer) = setup();

    publish(&fixture, "pending-close-force");
    let forced = fixture.run_public(
        &[
            "task",
            "close",
            "pending-close-force",
            "--force",
            "--reason",
            "legacy pending task force closed by live master",
        ],
        THREAD_A,
    );
    assert_eq!(forced["status"], "closed");
    assert_eq!(forced["owner"], master);
    assert_eq!(
        fixture.run_public(&["task", "status", "pending-close-force"], THREAD_A)["status"],
        "closed"
    );

    publish(&fixture, "pending-close-gated");
    let gated = fixture.command_public(&["task", "close", "pending-close-gated"], THREAD_A);
    assert!(
        !gated.status.success(),
        "non-force close must stay gated: {gated:?}"
    );
    let stderr = String::from_utf8_lossy(&gated.stderr);
    assert!(
        stderr.contains("BOARD_ACCEPT_REQUIRED"),
        "non-force close must return BOARD_ACCEPT_REQUIRED: {stderr}"
    );
    assert_eq!(
        fixture.run_public(&["task", "status", "pending-close-gated"], THREAD_A)["status"],
        "pending"
    );

    let published = fixture.run_public(
        &[
            "board",
            "publish",
            "pending-cancel-board",
            "--title",
            "pending cancel",
            "--description",
            "unbound pending work",
            "--delivery-condition",
            "cancelled",
            "--test-condition",
            "CLI 黑盒通过",
        ],
        THREAD_B,
    );
    assert_eq!(published["task"]["status"], "pending");
    assert_eq!(published["task"]["owner"], peer);
    let cancelled = fixture.run_public(
        &[
            "board",
            "update",
            "pending-cancel-board",
            "--expected-revision",
            "1",
            "--status",
            "cancelled",
        ],
        THREAD_B,
    );
    assert_eq!(cancelled["task"]["status"], "cancelled");
    assert_eq!(
        fixture.run_public(&["task", "status", "pending-cancel-board"], THREAD_B)["status"],
        "cancelled"
    );
}
