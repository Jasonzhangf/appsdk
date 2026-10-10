use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "appsdk-reset-platform-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn appsdk(root: &Path, args: &[&str]) -> Output {
    let home = appsdk_home(root);
    Command::new(env!("CARGO_BIN_EXE_appsdk"))
        .args(args)
        .env("APPSDK_HOME", home)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap()
}

fn appsdk_home(root: &Path) -> PathBuf {
    let root_name = root.file_name().unwrap().to_string_lossy();
    root.parent()
        .unwrap_or(root)
        .join(format!("appsdk-test-home-{root_name}"))
}

fn cleanup_test_paths(root: &Path) {
    let home = appsdk_home(root);
    if root.exists() {
        fs::remove_dir_all(root).unwrap();
    }
    if home.exists() {
        fs::remove_dir_all(home).unwrap();
    }
}

fn new_project(root: &Path) {
    let output = appsdk(root, &["new", root.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_clean_non_main_worktree(root: &Path) {
    for args in [
        ["init", "--initial-branch=codex/reset-test"].as_slice(),
        ["config", "user.email", "test@appsdk.local"].as_slice(),
        ["config", "user.name", "AppSDK reset test"].as_slice(),
        ["add", "--all"].as_slice(),
        ["commit", "-m", "reset fixture"].as_slice(),
    ] {
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
}

fn interrupted_reset_targets(root: &Path) -> Vec<Value> {
    let mut contract_targets = Vec::new();
    for directory in ["contracts/records", "contracts/transitions"] {
        for entry in fs::read_dir(root.join(directory)).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                contract_targets.push(
                    entry
                        .path()
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    for relative in [
        "contracts/records/plan-record.schema.json",
        "contracts/records/plan-revision-record.schema.json",
        "contracts/records/step-execution-record.schema.json",
        "contracts/transitions/zone-transition-manifest.json",
    ] {
        if !contract_targets.iter().any(|existing| existing == relative) {
            contract_targets.push(relative.to_string());
        }
    }
    contract_targets.sort();

    let mut relatives = vec![
        ".appsdk".to_string(),
        ".appsdk-control".to_string(),
        "generated".to_string(),
    ];
    relatives.extend(contract_targets);
    relatives.push(".gitignore".into());

    relatives
        .into_iter()
        .enumerate()
        .map(|(index, relative)| {
            let kind = if [".appsdk", ".appsdk-control", "generated"].contains(&relative.as_str()) {
                "dir"
            } else {
                "file"
            };
            let staged = format!("staging/{relative}");
            let staged_path = root
                .parent()
                .unwrap()
                .join(format!(".appsdk-reset-transaction-{}", root.file_name().unwrap().to_string_lossy()))
                .join(&staged);
            if kind == "dir" {
                fs::create_dir_all(&staged_path).unwrap();
            } else {
                fs::create_dir_all(staged_path.parent().unwrap()).unwrap();
                let source = root.join(&relative);
                if source.is_file() {
                    fs::copy(source, &staged_path).unwrap();
                } else {
                    fs::write(&staged_path, b"staged\n").unwrap();
                }
            }
            json!({
                "relative": relative,
                "kind": kind,
                "original_exists": if index == 0 { true } else { fs::symlink_metadata(root.join(&relative)).is_ok() },
                "backup": format!("quarantine/target-{index}"),
                "staged": staged,
                "quarantined": false,
                "published": false
            })
        })
        .collect()
}

#[test]
fn reset_governance_preserves_business_data_and_removes_generated_state() {
    let root = temp_root("success");
    new_project(&root);
    fs::create_dir_all(root.join("active")).unwrap();
    fs::create_dir_all(root.join("protected/history")).unwrap();
    fs::create_dir_all(root.join("generated/old-artifact")).unwrap();
    fs::write(root.join("business.txt"), "business survives\n").unwrap();
    fs::write(root.join("active/user-data.txt"), "active survives\n").unwrap();
    fs::write(
        root.join("protected/history/user-data.txt"),
        "protected survives\n",
    )
    .unwrap();
    fs::write(
        root.join("generated/old-artifact/output.bin"),
        "remove generated data\n",
    )
    .unwrap();
    init_clean_non_main_worktree(&root);

    let output = appsdk(
        &root,
        &[
            "reset-governance",
            root.to_str().unwrap(),
            "--discard-legacy",
        ],
    );
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| serde_json::from_str(line).ok())
        .expect("reset emits a JSON result");
    assert_eq!(result["operation"], "governance.reset");
    assert_eq!(result["status"], "completed");
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "business survives\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("active/user-data.txt")).unwrap(),
        "active survives\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("protected/history/user-data.txt")).unwrap(),
        "protected survives\n"
    );
    assert!(root
        .join(".appsdk/records/reset-governance-record.json")
        .is_file());
    assert!(!root.join("generated/old-artifact").exists());
    cleanup_test_paths(&root);
}

#[test]
fn reset_governance_recovers_interrupted_quarantine_and_preserves_business_data() {
    let root = temp_root("interrupted-recovery");
    new_project(&root);
    fs::write(root.join("business.txt"), "business survives\n").unwrap();
    init_clean_non_main_worktree(&root);

    let project_name = root.file_name().unwrap().to_string_lossy();
    let transaction = root
        .parent()
        .unwrap()
        .join(format!(".appsdk-reset-transaction-{project_name}"));
    fs::create_dir_all(transaction.join("quarantine")).unwrap();
    let targets = interrupted_reset_targets(&root);
    fs::rename(
        root.join(".appsdk"),
        transaction.join("quarantine/target-0"),
    )
    .unwrap();
    let marker = json!({
        "schema_version": 1,
        "transaction_id": "reset-quarantine-marker-lag",
        "root": root.to_string_lossy(),
        "mode": "discard_legacy_control_plane",
        "phase": "prepared",
        "error": null,
        "created_dirs": [],
        "targets": targets,
        "updated_at": "2026-10-10T00:00:00Z"
    });
    fs::write(
        transaction.join("marker.json"),
        serde_json::to_vec_pretty(&marker).unwrap(),
    )
    .unwrap();

    let output = appsdk(
        &root,
        &[
            "reset-governance",
            root.to_str().unwrap(),
            "--discard-legacy",
        ],
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("GOVERNANCE_RESET_RECOVERED_RETRY"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!transaction.exists());
    assert!(root.join(".appsdk/project.json").is_file());
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "business survives\n"
    );
    cleanup_test_paths(&root);
}
