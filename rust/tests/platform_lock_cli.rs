//! Public CLI coverage that the shared platform lock actually guards the
//! communication mailbox and the lifecycle record producer.
//!
//! These tests drive the real `appsdk` binary and the real OS lock; they do not
//! depend on the Unix-only `cli_smoke` fixtures, so the target can run on the
//! native Windows gate as well.

use serde_json::json;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_appsdk"))
}

fn temp_root(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "appsdk-platform-lock-{name}-{}-{nonce}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    root
}

fn run(root: &Path, args: &[&str]) -> Output {
    // The global registry home lives next to (never inside) the project so the
    // producer worktree-cleanliness check is not disturbed by registry writes.
    Command::new(binary())
        .args(args)
        .env("APPSDK_HOME", root.with_extension("appsdk-home"))
        .env_remove("GIT_BUG_BIN")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap()
}

fn hold_lock(path: &Path) -> File {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.try_lock().unwrap();
    file
}

fn cleanup(root: &Path) {
    fs::remove_dir_all(root).ok();
    fs::remove_dir_all(root.with_extension("appsdk-home")).ok();
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_value(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn init_git(root: &Path) {
    git(root, &["init"]);
    git(root, &["config", "maintenance.auto", "false"]);
    git(root, &["config", "user.email", "test@appsdk.local"]);
    git(root, &["config", "user.name", "AppSDK Test"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "baseline"]);
    git(root, &["branch", "-M", "codex/test"]);
}

#[test]
fn communication_cli_lock_guards_mailbox() {
    let root = temp_root("communication");
    let communication_dir = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication_dir).unwrap();
    let lock_path = communication_dir.join("mailbox.jsonl.lock");
    let mailbox_path = communication_dir.join("mailbox.jsonl");
    let held = hold_lock(&lock_path);

    let request = json!({
        "op": "register_adapter",
        "adapter": {"adapterId": "lock-probe", "kind": "mailbox"}
    })
    .to_string();
    let args = [
        "communication",
        root.to_str().unwrap(),
        "--json",
        request.as_str(),
    ];

    let blocked = run(&root, &args);
    assert!(!blocked.status.success());
    assert!(
        String::from_utf8_lossy(&blocked.stderr).contains("communication_busy"),
        "stderr={}",
        String::from_utf8_lossy(&blocked.stderr)
    );
    assert!(
        !mailbox_path.exists(),
        "mailbox must not be written while the lock is held"
    );

    drop(held);
    let accepted = run(&root, &args);
    assert!(
        accepted.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&accepted.stdout),
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert!(
        mailbox_path.is_file(),
        "mailbox must be written after the lock is released"
    );

    cleanup(&root);
}

fn prepare_retire_fixture(name: &str) -> PathBuf {
    let root = temp_root(name);
    let root_text = root.to_str().unwrap();
    let new = run(&root, &["new", root_text]);
    assert!(
        new.status.success(),
        "appsdk new failed: {}",
        String::from_utf8_lossy(&new.stderr)
    );
    init_git(&root);
    let candidate_commit = git_value(&root, &["rev-parse", "HEAD"]);
    let candidate_tree = git_value(&root, &["rev-parse", "HEAD^{tree}"]);
    let records = root.join(".appsdk/records");
    fs::write(
        records.join("fix-candidate-record-app-core.json"),
        serde_json::to_string_pretty(&json!({
            "fix_candidate_id":"candidate-1",
            "issue_id":"stale-issue",
            "module_id":"app-core",
            "worktree_id":"worktree-1",
            "base_commit":candidate_commit.clone(),
            "head_commit":candidate_commit.clone(),
            "tree_hash":candidate_tree.clone(),
            "diff_hash":"sha256:diff",
            "design_id":"design-1",
            "owner":"test",
            "scope_hash":"sha256:scope",
            "changed_paths":[],
            "verification_evidence_ids":["evidence-1","evidence-2","evidence-3"],
            "created_at":"2026-01-01T00:00:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join("pre-review-validation-record-app-core.json"),
        serde_json::to_string_pretty(&json!({
            "validation_id":"validation-1",
            "issue_id":"stale-issue",
            "module_id":"app-core",
            "fix_candidate_id":"candidate-1",
            "candidate_commit":candidate_commit,
            "candidate_tree_hash":candidate_tree,
            "artifact_hash":"sha256:artifact",
            "whitebox_producer":{"adapter":"test","identity":"whitebox"},
            "whitebox_evidence_ids":["whitebox-1"],
            "blackbox_evidence_ids":["blackbox-1"],
            "deployment":{},
            "source_unchanged":true,
            "result":"pass",
            "created_at":"2026-01-01T00:01:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    git(&root, &["add", ".appsdk/records"]);
    git(&root, &["commit", "-m", "candidate records"]);
    root
}

#[test]
fn producer_cli_lock_guards_records() {
    let root = prepare_retire_fixture("producer");
    let root_text = root.to_str().unwrap();
    let records = root.join(".appsdk/records");
    let candidate_path = records.join("fix-candidate-record-app-core.json");
    let validation_path = records.join("pre-review-validation-record-app-core.json");
    let candidate_bytes = fs::read(&candidate_path).unwrap();
    let validation_bytes = fs::read(&validation_path).unwrap();

    let control_dir = root.join(".appsdk-control");
    fs::create_dir_all(&control_dir).unwrap();
    let held = hold_lock(&control_dir.join("lifecycle-record-producer.lock"));

    let args = [
        "retire-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--issue",
        "current-issue",
    ];
    let blocked = run(&root, &args);
    assert!(!blocked.status.success());
    assert!(
        String::from_utf8_lossy(&blocked.stderr).contains("PRODUCER_BUSY"),
        "stderr={}",
        String::from_utf8_lossy(&blocked.stderr)
    );
    assert_eq!(fs::read(&candidate_path).unwrap(), candidate_bytes);
    assert_eq!(fs::read(&validation_path).unwrap(), validation_bytes);

    drop(held);
    let retired = run(&root, &args);
    assert!(
        retired.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&retired.stdout),
        String::from_utf8_lossy(&retired.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&retired.stdout).unwrap();
    assert_eq!(receipt["retired"], true);
    assert!(!candidate_path.exists());
    assert!(!validation_path.exists());

    cleanup(&root);
}

#[test]
fn producer_cli_reports_lock_open_failure() {
    let root = temp_root("producer-open-failure");
    let root_text = root.to_str().unwrap();
    let control_dir = root.join(".appsdk-control");
    fs::create_dir_all(&control_dir).unwrap();
    // A directory occupying the lock path makes the producer lock open fail
    // before any record work starts.
    fs::create_dir_all(control_dir.join("lifecycle-record-producer.lock")).unwrap();

    let result = run(
        &root,
        &[
            "retire-lifecycle-records",
            root_text,
            "--module",
            "app-core",
            "--issue",
            "current-issue",
        ],
    );
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("PRODUCER_LOCK_FAILED"),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );

    cleanup(&root);
}
