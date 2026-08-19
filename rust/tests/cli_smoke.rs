use serde_json::Value;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_appsdk"))
}

fn temp_root(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("appsdk-rust-{name}-{}-{nonce}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    root
}

fn init_git(root: &PathBuf) {
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "init"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "config",
            "user.email",
            "test@appsdk.local"
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "config",
            "user.name",
            "AppSDK Test"
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "commit", "-m", "baseline"])
        .status()
        .unwrap()
        .success());
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(binary()).args(args).output().unwrap()
}

fn confirm_preparation(root: &PathBuf, project_root: &str, change_kind: &str) {
    fs::write(
        root.join(".appsdk-prepare.json"),
        format!(
            r#"{{"schema_version":1,"preparation_id":"prepare-test","status":"confirmed","objective":"test objective","change_kind":"{}","project_root":"{}","legacy_roots":["legacy"],"new_roots":["{}"],"protected_roots":["{}/protected"],"runtime_forbidden_roots":["{}/generated"],"boundary":{{"allowed_paths":["{}"],"forbidden_paths":["v3/**"],"payload_control_separation":"confirmed"}},"acceptance_criteria":["pass"],"non_goals":["v3"],"assumptions":[],"questions":[],"confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}}"#,
            change_kind, project_root, project_root, project_root, project_root, project_root
        ),
    )
    .unwrap();
}

#[test]
fn prepare_creates_template_and_init_rejects_unconfirmed_record() {
    let root = temp_root("prepare-gate");
    fs::create_dir_all(&root).unwrap();
    let root_text = root.to_str().unwrap();
    assert!(run(&["prepare", root_text]).status.success());
    let template = fs::read_to_string(root.join(".appsdk-prepare.json")).unwrap();
    assert!(template.contains("\"status\": \"draft\""));
    let init = run(&["init", root_text]);
    assert!(!init.status.success());
    assert!(String::from_utf8_lossy(&init.stderr).contains("PREPARATION_NOT_CONFIRMED"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_existing_project_creates_layout_and_manages_gitignore_idempotently() {
    let root = temp_root("init-existing");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(".gitignore"), "# project rules\nnode_modules/\n").unwrap();
    let root_text = root.to_str().unwrap();
    confirm_preparation(&root, ".", "project_refactor");

    let first = run(&["init", root_text]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(gitignore.starts_with("# project rules\nnode_modules/\n"));
    assert_eq!(gitignore.matches("# BEGIN APPSDK MANAGED").count(), 1);
    assert!(gitignore.contains(".appsdk-control/"));
    assert!(gitignore.contains(".appsdk/sdk.bin"));
    assert!(gitignore.contains("/active/lib/"));
    assert!(gitignore.contains("/generated/"));
    for path in [
        ".appsdk/project.json",
        ".appsdk/goal.json",
        ".appsdk/sdk.lock",
        ".appsdk/sdk-resources.json",
        ".appsdk/docs/design/appsdk-project-integration.md",
        ".appsdk/docs/design/fix-lifecycle-v2.md",
        ".appsdk/rules/appsdk-project-governance.md",
        ".appsdk/skills/appsdk-project-governance/SKILL.md",
        ".appsdk/maps/resource-map.json",
        ".appsdk/maps/module-registry.json",
        ".appsdk/contracts/records/worktree-record.schema.json",
        ".appsdk/contracts/records/effectiveness-record.schema.json",
        ".appsdk/contracts/records/merge-record.schema.json",
        "playground/experiments",
        "active/lib",
        "protected/source",
        "protected/contracts",
        "protected/history",
        "generated",
        ".appsdk-control",
    ] {
        assert!(root.join(path).exists(), "missing {}", path);
    }

    let second = run(&["init", root_text]);
    assert!(second.status.success());
    let gitignore_after = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert_eq!(gitignore_after.matches("# BEGIN APPSDK MANAGED").count(), 1);
    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_rejects_tampered_installed_sdk_resource() {
    let root = temp_root("sdk-resource-integrity");
    fs::create_dir_all(&root).unwrap();
    let root_text = root.to_str().unwrap();
    confirm_preparation(&root, ".", "new_project");
    assert!(run(&["init", root_text]).status.success());
    let resource = root.join(".appsdk/skills/appsdk-project-governance/SKILL.md");
    fs::write(&resource, "tampered\n").unwrap();
    let result = run(&["verify", root_text]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("SDK_RESOURCE_MISMATCH"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_requires_the_project_pinned_sdk_binary_version() {
    let root = temp_root("sdk-version-pin");
    fs::create_dir_all(&root).unwrap();
    let root_text = root.to_str().unwrap();
    confirm_preparation(&root, ".", "new_project");
    assert!(run(&["init", root_text]).status.success());
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["sdk"]["version"] = Value::String("0.1.2".into());
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let lock_file = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_file).unwrap()).unwrap();
    lock["version"] = Value::String("0.1.2".into());
    fs::write(
        &lock_file,
        serde_json::to_string_pretty(&lock).unwrap() + "\n",
    )
    .unwrap();
    let result = run(&["verify", root_text]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr)
        .contains("PROJECT_SDK_VERSION_PIN_MISMATCH:0.1.2:required_binary=appsdk-0.1.2"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parallel_development_scenarios_require_atomic_activation() {
    let root = temp_root("scenario-pair");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["development_scenarios"] = serde_json::json!({
        "manifest": ".appsdk/contracts/development-scenarios.manifest.json",
        "enabled": ["multi_worker_collaboration"]
    });
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let result = run(&["verify", root_text]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("DEVELOPMENT_SCENARIO_PAIR_REQUIRED"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parallel_development_requires_tested_integration_and_remote_main_receipt() {
    let root = temp_root("parallel-main-receipt");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    enable_parallel_development(&root);
    init_git(&root);
    fs::write(root.join(".appsdk/goal.json"), r#"{"goal_id":"goal-1","raw_request":"parallel change","understood_objective":"parallel change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#).unwrap();
    pin_test_lock(root_text);
    for stage in ["source_implemented", "contract_bound"] {
        assert!(run(&["promote", root_text, "--to", stage]).status.success());
    }
    assert!(run(&["compile", root_text]).status.success());
    for stage in ["compiled", "controlled_verified"] {
        assert!(run(&["promote", root_text, "--to", stage]).status.success());
    }
    for stage in ["contract_bound", "compiled", "controlled_verified"] {
        assert!(run(&[
            "promote-module",
            root_text,
            "--module",
            "app-core",
            "--to",
            stage,
        ])
        .status
        .success());
    }
    let module_artifact: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json")).unwrap(),
    )
    .unwrap();
    let architecture_hash = module_artifact["artifact_hash"].as_str().unwrap();
    write_parallel_records(&root, "app-core", architecture_hash, false);
    let promoted = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ]);
    assert!(
        promoted.status.success(),
        "{}",
        String::from_utf8_lossy(&promoted.stderr)
    );
    for (file, pointer, invalid, expected) in [
        (
            "collaboration-record-collaboration-1.json",
            "/exclusive_worktree",
            Value::Bool(false),
            "MULTI_WORKER_EXCLUSIVE_WORKTREE_REQUIRED",
        ),
        (
            "collaboration-record-collaboration-1.json",
            "/independently_verifiable",
            Value::Bool(false),
            "INCREMENTAL_MILESTONE_CONTRACT_REQUIRED",
        ),
        (
            "collaboration-record-collaboration-1.json",
            "/milestone_sequence",
            Value::Number(2.into()),
            "MILESTONE_PREDECESSOR_RECEIPT_REQUIRED",
        ),
        (
            "collaboration-index.json",
            "/active_claims",
            serde_json::json!([
                {"collaboration_id":"collaboration-1","semantic_claim_id":"claim-1","worker_id":"worker-1","worktree_id":"worktree-1","milestone_id":"milestone-1"},
                {"collaboration_id":"collaboration-2","semantic_claim_id":"claim-2","worker_id":"worker-2","worktree_id":"worktree-1","milestone_id":"milestone-2"}
            ]),
            "COLLABORATION_INDEX_NOT_EXCLUSIVE",
        ),
        (
            "merge-queue-record-queue-1.json",
            "/milestone_id",
            Value::String("wrong-milestone".into()),
            "MERGE_QUEUE_ADMISSION_MISMATCH",
        ),
        (
            "merge-queue-record-queue-1.json",
            "/candidate_commit",
            Value::String("wrong-candidate".into()),
            "MERGE_QUEUE_ADMISSION_MISMATCH",
        ),
        (
            "integration-record-integration-1.json",
            "/conflict_status",
            Value::String("conflict".into()),
            "INTEGRATION_RECORD_MISMATCH",
        ),
        (
            "integration-record-integration-1.json",
            "/required_gate_results/0/gate_id",
            Value::String("unknown-gate".into()),
            "INTEGRATION_RECORD_MISMATCH",
        ),
        (
            "integration-record-integration-1.json",
            "/integration_tree_hash",
            Value::String("wrong-tree".into()),
            "INTEGRATION_GATE_BINDING_MISMATCH",
        ),
        (
            "merge-record-app-core.json",
            "/fix_candidate_id",
            Value::String("wrong-candidate".into()),
            "PARALLEL_MAINLINE_MERGE_MISMATCH",
        ),
        (
            "merge-record-app-core.json",
            "/mainline_ref",
            Value::String("refs/heads/wrong".into()),
            "PARALLEL_MAINLINE_MERGE_MISMATCH",
        ),
        (
            "mainline-receipt-record-receipt-1.json",
            "/remote_verified",
            Value::Bool(false),
            "MAINLINE_RECEIPT_MISMATCH",
        ),
    ] {
        let path = root.join(".appsdk/records").join(file);
        let original = fs::read_to_string(&path).unwrap();
        let mut record: Value = serde_json::from_str(&original).unwrap();
        *record.pointer_mut(pointer).unwrap() = invalid;
        fs::write(&path, serde_json::to_string_pretty(&record).unwrap() + "\n").unwrap();
        let rejected = run(&["verify", root_text]);
        assert!(
            !rejected.status.success(),
            "accepted invalid {file}:{pointer}"
        );
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains(expected),
            "{file}:{pointer}: {}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        fs::write(&path, original).unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_rejects_symlinked_sdk_resources_record() {
    let root = temp_root("sdk-resources-symlink");
    fs::create_dir_all(&root).unwrap();
    let root_text = root.to_str().unwrap();
    confirm_preparation(&root, ".", "new_project");
    assert!(run(&["init", root_text]).status.success());
    let record_path = root.join(".appsdk/sdk-resources.json");
    let original = root.join(".appsdk/sdk-resources.original.json");
    fs::rename(&record_path, &original).unwrap();
    symlink(&original, &record_path).unwrap();
    let result = run(&["verify", root_text]);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("GOVERNANCE_PATH_SYMLINK:sdk_resources")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_rejects_escaping_sdk_resource_record_path() {
    let root = temp_root("sdk-resources-escape");
    fs::create_dir_all(&root).unwrap();
    let root_text = root.to_str().unwrap();
    confirm_preparation(&root, ".", "new_project");
    assert!(run(&["init", root_text]).status.success());
    let record_path = root.join(".appsdk/sdk-resources.json");
    let mut record: Value =
        serde_json::from_str(&fs::read_to_string(&record_path).unwrap()).unwrap();
    record["resources"][0]["path"] = Value::String("../escape".into());
    fs::write(
        &record_path,
        serde_json::to_string_pretty(&record).unwrap() + "\n",
    )
    .unwrap();
    let result = run(&["verify", root_text]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("SDK_RESOURCE_PATH_ESCAPE"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_rejects_tampered_lock_bundle_resources() {
    let root = temp_root("lock-bundle-resources");
    fs::create_dir_all(&root).unwrap();
    let root_text = root.to_str().unwrap();
    confirm_preparation(&root, ".", "new_project");
    assert!(run(&["init", root_text]).status.success());
    pin_test_lock(root_text);
    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["bundle_resources"]["contracts"][0] = Value::String("contracts/tampered.json".into());
    fs::write(
        &lock_path,
        serde_json::to_string_pretty(&lock).unwrap() + "\n",
    )
    .unwrap();
    let result = run(&["verify", root_text]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("SDK_LOCK_BUNDLE_RESOURCES_MISMATCH"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_rejects_symlinked_control_parent() {
    let root = temp_root("init-symlink-parent");
    fs::create_dir_all(&root).unwrap();
    let outside = root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(root.join(".appsdk")).unwrap();
    symlink(&outside, root.join(".appsdk/docs")).unwrap();
    let root_text = root.to_str().unwrap();
    confirm_preparation(&root, ".", "new_project");
    let result = run(&["init", root_text]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("GOVERNANCE_PATH_SYMLINK"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_can_place_new_project_in_configured_subdirectory() {
    let workspace = temp_root("init-subdirectory");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("legacy.rs"), "legacy source\n").unwrap();
    let workspace_text = workspace.to_str().unwrap();
    confirm_preparation(&workspace, "next-code", "project_refactor");

    let result = run(&["init", workspace_text, "--project-root", "next-code"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let project = workspace.join("next-code");
    assert!(workspace.join("legacy.rs").exists());
    assert!(project.join(".appsdk/project.json").exists());
    assert!(project.join("playground/experiments").exists());
    assert!(project.join("active/lib").exists());
    assert!(project.join("protected/source").exists());
    assert!(project.join("generated").exists());
    assert!(fs::read_to_string(project.join(".gitignore"))
        .unwrap()
        .contains("/generated/"));
    assert!(run(&["verify", project.to_str().unwrap()]).status.success());

    let invalid = run(&["init", workspace_text, "--project-root", "../escape"]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("INVALID_PROJECT_ROOT"));
    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn init_target_accepts_matching_parent_preparation() {
    let workspace = temp_root("init-target-parent-preparation");
    fs::create_dir_all(&workspace).unwrap();
    confirm_preparation(&workspace, "v4", "project_refactor");
    let target = workspace.join("v4");

    let result = run(&["init", target.to_str().unwrap()]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(target.join(".appsdk/project.json").exists());
    assert!(run(&["verify", target.to_str().unwrap()]).status.success());

    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn init_target_accepts_matching_nested_parent_preparation() {
    let workspace = temp_root("init-target-nested-parent-preparation");
    fs::create_dir_all(&workspace).unwrap();
    confirm_preparation(&workspace, "services/v4", "project_refactor");
    let target = workspace.join("services/v4");

    let result = run(&["init", target.to_str().unwrap()]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(target.join(".appsdk/project.json").exists());
    assert!(run(&["verify", target.to_str().unwrap()]).status.success());

    fs::remove_dir_all(workspace).unwrap();
}

fn pin_test_lock(root: &str) {
    let sdk_binary = binary();
    assert!(
        run(&["pin-lock", root, "--binary", sdk_binary.to_str().unwrap()])
            .status
            .success()
    );
}

#[test]
fn pinned_global_binary_verifies_without_local_sdk_witness() {
    let root = temp_root("global-sdk-no-local-witness");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/goal.json"),
        r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#,
    )
    .unwrap();
    pin_test_lock(root_text);
    let source_promote = run(&["promote", root_text, "--to", "source_implemented"]);
    assert!(
        source_promote.status.success(),
        "{}",
        String::from_utf8_lossy(&source_promote.stderr)
    );
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    let compile = run(&["compile", root_text]);
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    assert!(run(&["promote", root_text, "--to", "compiled"])
        .status
        .success());
    fs::remove_file(root.join(".appsdk/sdk.bin")).unwrap();

    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );

    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["digest"] = Value::String(format!("sha256:{}", "0".repeat(64)));
    lock["compiler_digest"] = lock["digest"].clone();
    fs::write(
        &lock_path,
        serde_json::to_string_pretty(&lock).unwrap() + "\n",
    )
    .unwrap();
    let rejected = run(&["verify", root_text]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("SDK_BINARY_DIGEST_MISMATCH"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_admission_skips_generated_artifact_requirement() {
    let root = temp_root("verify-admission");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/goal.json"),
        r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#,
    )
    .unwrap();
    pin_test_lock(root_text);
    let source_promote = run(&["promote", root_text, "--to", "source_implemented"]);
    assert!(
        source_promote.status.success(),
        "{}",
        String::from_utf8_lossy(&source_promote.stderr)
    );
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    let compile = run(&["compile", root_text]);
    assert!(compile.status.success());
    assert!(run(&["promote", root_text, "--to", "compiled"])
        .status
        .success());
    fs::remove_file(root.join("generated/project.compiled.json")).unwrap();

    let full = run(&["verify", root_text]);
    assert!(!full.status.success());
    assert!(String::from_utf8_lossy(&full.stderr).contains("COMPILED_STAGE_REQUIRES_ARTIFACT"));

    let admission = run(&["verify", "--admission", root_text]);
    assert!(
        admission.status.success(),
        "{}",
        String::from_utf8_lossy(&admission.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

fn canonical(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap(),
        Value::Array(values) => format!(
            "[{}]",
            values.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|key| format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap(),
                        canonical(&values[*key])
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn git_test_value(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn file_digest(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(fs::read(path).unwrap());
    format!("sha256:{:x}", hasher.finalize())
}

fn write_records(root: &PathBuf, module_id: &str, artifact_hash: &str, include_freeze: bool) {
    let records = root.join(".appsdk/records");
    fs::create_dir_all(&records).unwrap();
    let evidence_dir = records.join("evidence").join(module_id);
    fs::create_dir_all(&evidence_dir).unwrap();
    let commit = git_test_value(root, &["rev-parse", "HEAD"]);
    let tree = git_test_value(root, &["rev-parse", "HEAD^{tree}"]);
    let map_root = root.join(".appsdk/maps");
    let evidence = |id: &str, phase: &str, kind: &str, created_at: &str| {
        serde_json::json!({
            "evidence_id": id,
            "issue_id": "issue-1",
            "experiment_id": "experiment-1",
            "phase": phase,
            "kind": kind,
            "source_commit": commit,
            "artifact_hash": artifact_hash,
            "scope": {"module_id": module_id},
            "producer": {"adapter":"test","identity":"test"},
            "result":"pass",
            "created_at": created_at,
            "expires_at":"2099-01-01T00:00:00Z",
            "input_hashes":["input-1"],
            "scope_hash":"scope-1"
        })
    };
    for (id, phase, kind, created_at) in [
        (
            "baseline-1",
            "baseline_reproduction",
            "sample_replay",
            "2026-01-01T00:01:00Z",
        ),
        (
            "candidate-evidence-1",
            "fix_candidate",
            "build",
            "2026-01-01T00:03:00Z",
        ),
        (
            "positive-1",
            "positive_intervention",
            "positive_test",
            "2026-01-01T00:03:00Z",
        ),
        (
            "negative-1",
            "negative_intervention",
            "negative_test",
            "2026-01-01T00:03:00Z",
        ),
        (
            "effective-1",
            "post_architecture_effectiveness",
            "sample_replay",
            "2026-01-01T00:05:00Z",
        ),
        (
            "post-positive-1",
            "positive_intervention",
            "positive_test",
            "2026-01-01T00:05:00Z",
        ),
        (
            "post-negative-1",
            "negative_intervention",
            "negative_test",
            "2026-01-01T00:05:00Z",
        ),
    ] {
        fs::write(
            evidence_dir.join(format!("{id}.json")),
            serde_json::to_string_pretty(&evidence(id, phase, kind, created_at)).unwrap() + "\n",
        )
        .unwrap();
    }
    fs::write(
        records.join(format!("evidence-record-{module_id}.json")),
        serde_json::to_string_pretty(&evidence(
            "candidate-evidence-1",
            "fix_candidate",
            "build",
            "2026-01-01T00:03:00Z",
        ))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("worktree-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "worktree_id":"worktree-1","issue_id":"issue-1","module_id":module_id,
            "base_ref":"HEAD","base_commit":commit,"branch":"test-fix","head_commit":commit,
            "initial_clean":true,"final_clean":true,"isolation_mode":"isolated_worktree",
            "scope_hash":"scope-1","created_at":"2026-01-01T00:00:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("reproduction-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "reproduction_id":"reproduction-1","issue_id":"issue-1","module_id":module_id,
            "worktree_id":"worktree-1","base_commit":commit,"input_hashes":["input-1"],
            "baseline_evidence_id":"baseline-1","first_divergence":"test-owner",
            "result":"reproduced","created_at":"2026-01-01T00:01:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("fix-candidate-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "fix_candidate_id":"candidate-1","issue_id":"issue-1","module_id":module_id,
            "worktree_id":"worktree-1","base_commit":commit,"head_commit":commit,
            "tree_hash":tree,"diff_hash":"sha256:test-diff","design_id":"design-1",
            "owner":"app-core","scope_hash":"scope-1","changed_paths":[],
            "verification_evidence_ids":["candidate-evidence-1","positive-1","negative-1"],
            "created_at":"2026-01-01T00:03:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("review-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "review_id":"review-1","issue_id":"issue-1","promotion_id":"promotion-1",
            "review_kind":"architecture","fix_candidate_id":"candidate-1",
            "reviewer":{"adapter":"test","identity":"test"},"verdict":"pass",
            "evidence_ids":["candidate-evidence-1","positive-1","negative-1"],"reviewed_commit":commit,
            "reviewed_tree_hash":tree,"reviewed_diff_hash":"sha256:test-diff",
            "reviewed_artifact_hash":artifact_hash,"reviewed_scope_hash":"scope-1",
            "resource_map_hash":file_digest(&map_root.join("resource-map.json")),
            "function_map_hash":file_digest(&map_root.join("function-map.json")),
            "mainline_call_map_hash":file_digest(&map_root.join("mainline-call-map.json")),
            "verification_map_hash":file_digest(&map_root.join("verification-map.json")),
            "ai_confidence":1.0,"confidence_rationale":"architecture and boundary evidence",
            "created_at":"2026-01-01T00:04:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("effectiveness-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "effectiveness_id":"effectiveness-1","issue_id":"issue-1","module_id":module_id,
            "fix_candidate_id":"candidate-1","architecture_review_id":"review-1",
            "reviewed_commit":commit,"reviewed_tree_hash":tree,
            "reproduction_input_hashes":["input-1"],"baseline_evidence_id":"baseline-1",
            "fixed_replay_evidence_id":"effective-1","positive_evidence_ids":["post-positive-1"],
            "negative_evidence_ids":["post-negative-1"],"blackbox_evidence_ids":["effective-1"],
            "source_unchanged_since_review":true,"result":"pass","created_at":"2026-01-01T00:05:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("merge-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "merge_id":"merge-1","issue_id":"issue-1","module_id":module_id,
            "fix_candidate_id":"candidate-1","effectiveness_id":"effectiveness-1",
            "mainline_ref":"HEAD","candidate_commit":commit,"merge_commit":commit,
            "candidate_tree_hash":tree,"merged_tree_hash":tree,"change_identity":"exact",
            "result":"pass","created_at":"2026-01-01T00:06:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let promotion_value = serde_json::json!({
        "promotion_id":"promotion-1","issue_id":"issue-1","experiment_id":"experiment-1",
        "module_id":module_id,"base_commit":commit,"source_commit":commit,
        "candidate_commit":commit,"merged_commit":commit,
        "worktree_record_id":"worktree-1","reproduction_record_id":"reproduction-1",
        "fix_candidate_id":"candidate-1","architecture_review_id":"review-1",
        "effectiveness_record_id":"effectiveness-1","merge_record_id":"merge-1",
        "previous_active_version":null,"new_active_version":"active-v1",
        "artifact_hash":artifact_hash,"scope_hash":"scope-1","public_api_hash":"api-1",
        "review_id":"review-1","evidence_ids":["candidate-evidence-1","effective-1"],
        "required_gate_results":[{"gate_id":"fix_lifecycle_graph","result":"pass","producer":"test"}],
        "change_set_id":"change-1","compatibility_level":"compatible","root_cause":"test root cause",
        "design_id":"design-1","change_reason_comment":"test reason",
        "playground_cleanup_record_id":"cleanup-1","created_at":"2026-01-01T00:07:00Z"
    });
    let promotion = serde_json::to_string_pretty(&promotion_value).unwrap() + "\n";
    fs::write(
        records.join(format!("promotion-record-{module_id}.json")),
        &promotion,
    )
    .unwrap();
    fs::write(
        records.join(format!("playground-cleanup-cleanup-1.json")),
        r#"{"cleanup_id":"cleanup-1","disposition":"archive_then_remove","removed_paths":["playground/experiments/app-core"],"created_at":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    if include_freeze {
        let promotion_hash = digest(&canonical(&promotion_value));
        fs::write(
            records.join(format!("freeze-record-{module_id}.json")),
            format!(
                r#"{{"freeze_id":"freeze-1","issue_id":"issue-1","module_id":"{}","promotion_id":"promotion-1","promotion_record_hash":"{}","artifact_record_id":"candidate-evidence-1","source_commit_or_tag":"{}","active_version":"active-v1","previous_active_version":null,"library_hash":"{}","public_api_hash":"api-1","review_id":"review-1","previous_active_immutable":false,"git_clean":true,"clean_scope":{{"base_commit":"{}","changed_paths":[],"ignored_paths":[],"generated_policy":"tracked_hash"}},"owners":{{"vcs":"test","compiler":"test","api_extractor":"test","review":"test","artifact_registry":"test"}},"created_at":"2026-01-01T00:08:00Z"}}"#,
                module_id, promotion_hash, commit, artifact_hash, commit
            ),
        )
        .unwrap();
    }
}

fn enable_parallel_development(root: &Path) {
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["development_scenarios"] = serde_json::json!({
        "manifest": ".appsdk/contracts/development-scenarios.manifest.json",
        "enabled": ["multi_worker_collaboration", "multi_worktree_merge_queue"]
    });
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
}

fn write_parallel_records(
    root: &PathBuf,
    module_id: &str,
    artifact_hash: &str,
    include_freeze: bool,
) {
    write_records(root, module_id, artifact_hash, include_freeze);
    let records = root.join(".appsdk/records");
    let worktree_file = records.join(format!("worktree-record-{module_id}.json"));
    let mut worktree: Value =
        serde_json::from_str(&fs::read_to_string(&worktree_file).unwrap()).unwrap();
    worktree["milestone_id"] = Value::String("milestone-1".into());
    fs::write(
        &worktree_file,
        serde_json::to_string_pretty(&worktree).unwrap() + "\n",
    )
    .unwrap();
    let candidate_commit = git_test_value(root, &["rev-parse", "HEAD"]);
    let candidate_tree = git_test_value(root, &["rev-parse", "HEAD^{tree}"]);
    let marker = root.join(".appsdk/integration-marker");
    fs::write(&marker, "tested integration\n").unwrap();
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "add",
            ".appsdk/integration-marker"
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "commit",
            "-m",
            "tested integration",
        ])
        .status()
        .unwrap()
        .success());
    let integration_commit = git_test_value(root, &["rev-parse", "HEAD"]);
    let integration_tree = git_test_value(root, &["rev-parse", "HEAD^{tree}"]);
    assert_ne!(candidate_tree, integration_tree);
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "update-ref",
            "refs/heads/test-mainline",
            &integration_commit,
        ])
        .status()
        .unwrap()
        .success());
    let remote = root.join(".appsdk-control/test-remote.git");
    assert!(Command::new("git")
        .args(["init", "--bare", remote.to_str().unwrap()])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "push",
            remote.to_str().unwrap(),
            "HEAD:refs/heads/main",
        ])
        .status()
        .unwrap()
        .success());
    fs::write(
        records.join("collaboration-record-collaboration-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "collaboration_id":"collaboration-1","issue_id":"issue-1","module_id":module_id,
            "scenario_ids":["multi_worker_collaboration","multi_worktree_merge_queue"],
            "run_id":"run-1","semantic_claim_id":"claim-1","worker_id":"worker-1",
            "worktree_id":"worktree-1","exclusive_worktree":true,"exclusive_claim":true,
            "milestone_id":"milestone-1","parent_task_id":"parent-task-1","milestone_sequence":1,
            "predecessor_collaboration_id":"none","predecessor_receipt_id":"none",
            "milestone_scope":"one independently verifiable change","independently_verifiable":true,
            "one_milestone_per_worktree":true,
            "status":"handoff_ready","created_at":"2026-01-01T00:05:30Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join("merge-queue-record-queue-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "queue_entry_id":"queue-1","issue_id":"issue-1","module_id":module_id,
            "collaboration_id":"collaboration-1","fix_candidate_id":"candidate-1",
            "milestone_id":"milestone-1","delivery_mode":"commit_merge_each_milestone",
            "effectiveness_id":"effectiveness-1","candidate_commit":candidate_commit,
            "main_base_commit":candidate_commit,"queue_position":1,"merge_owner":"merge-owner-1",
            "strategy":"integration_merge_then_fast_forward","status":"admitted",
            "created_at":"2026-01-01T00:06:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join("integration-record-integration-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "integration_id":"integration-1","queue_entry_id":"queue-1","issue_id":"issue-1",
            "milestone_id":"milestone-1","module_id":module_id,"candidate_commit":candidate_commit,
            "main_base_commit":candidate_commit,"integration_commit":integration_commit,
            "integration_tree_hash":integration_tree,"conflict_status":"clean",
            "resolution_mode":"none","impact_status":"revalidated",
            "required_gate_results":[{"gate_id":"integration_affected_verification","result":"pass","producer":"appsdk::verifier","source_commit":integration_commit,"tree_hash":integration_tree}],
            "result":"pass","created_at":"2026-01-01T00:06:10Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join("mainline-receipt-record-receipt-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "receipt_id":"receipt-1","integration_id":"integration-1","queue_entry_id":"queue-1",
            "milestone_id":"milestone-1","issue_id":"issue-1","module_id":module_id,"local_main_ref":"refs/heads/test-mainline",
            "remote_name":remote.to_str().unwrap(),"remote_ref":"refs/heads/main","integration_commit":integration_commit,
            "local_main_commit":integration_commit,"remote_main_commit":integration_commit,
            "integration_tree_hash":integration_tree,"candidate_reachable":true,
            "integration_local_reachable":true,"integration_remote_reachable":true,
            "remote_verified":true,"producer":"test-host-vcs-adapter","observed_at":"2026-01-01T00:06:20Z",
            "result":"pass","created_at":"2026-01-01T00:06:20Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join("collaboration-index.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "active_claims":[{"collaboration_id":"collaboration-1","semantic_claim_id":"claim-1",
            "worker_id":"worker-1","worktree_id":"worktree-1","milestone_id":"milestone-1"}]
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join("merge-queue-state.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "merge_owner":"merge-owner-1","ordered_entry_ids":["queue-1"],"active_entry_id":"queue-1"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let merge_file = records.join(format!("merge-record-{module_id}.json"));
    let mut merge: Value = serde_json::from_str(&fs::read_to_string(&merge_file).unwrap()).unwrap();
    merge["queue_entry_id"] = Value::String("queue-1".into());
    merge["integration_id"] = Value::String("integration-1".into());
    merge["mainline_receipt_id"] = Value::String("receipt-1".into());
    merge["milestone_id"] = Value::String("milestone-1".into());
    merge["mainline_ref"] = Value::String("refs/heads/test-mainline".into());
    merge["integration_commit"] = Value::String(integration_commit.clone());
    merge["merge_commit"] = Value::String(integration_commit.clone());
    merge["integration_tree_hash"] = Value::String(integration_tree.clone());
    merge["merged_tree_hash"] = Value::String(integration_tree);
    merge["change_identity"] = Value::String("tested_integration_exact".into());
    merge["created_at"] = Value::String("2026-01-01T00:06:30Z".into());
    fs::write(
        &merge_file,
        serde_json::to_string_pretty(&merge).unwrap() + "\n",
    )
    .unwrap();
    let promotion_file = records.join(format!("promotion-record-{module_id}.json"));
    let mut promotion: Value =
        serde_json::from_str(&fs::read_to_string(&promotion_file).unwrap()).unwrap();
    promotion["merge_queue_record_id"] = Value::String("queue-1".into());
    promotion["collaboration_record_id"] = Value::String("collaboration-1".into());
    promotion["integration_record_id"] = Value::String("integration-1".into());
    promotion["mainline_receipt_record_id"] = Value::String("receipt-1".into());
    promotion["merged_commit"] = Value::String(integration_commit.clone());
    promotion["source_commit"] = Value::String(integration_commit);
    fs::write(
        &promotion_file,
        serde_json::to_string_pretty(&promotion).unwrap() + "\n",
    )
    .unwrap();
}

fn write_regression_report(root: &PathBuf, module_id: &str, artifact_hash: &str) -> String {
    let promotion: Value = serde_json::from_str(
        &fs::read_to_string(
            root.join(format!(".appsdk/records/promotion-record-{module_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let commit = promotion["source_commit"].as_str().unwrap();
    let report = serde_json::json!({
        "regression_report_id": "regression-app-core-v1",
        "module_id": module_id,
        "source_commit": commit,
        "artifact_hash": artifact_hash,
        "public_api_hash": "api-1",
        "scope_hash": "scope-1",
        "input_hash": artifact_hash,
        "suite_id": "app-core-regression",
        "command": {
            "program": "cargo",
            "args": ["test", "--test", "app-core"],
            "working_directory": "."
        },
        "test_count": 1,
        "passed": 1,
        "failed": 0,
        "skipped": 0,
        "result": "pass",
        "producer": {
            "adapter": "cargo",
            "identity": "appsdk-regression-gate"
        },
        "created_at": "2026-01-01T00:00:00Z",
        "test_characteristics": {
            "whitebox": true,
            "blackbox": true
        }
    });
    let hash = digest(&canonical(&report));
    fs::write(
        root.join(format!(
            ".appsdk/records/regression-report-{module_id}.json"
        )),
        serde_json::to_string_pretty(&report).unwrap() + "\n",
    )
    .unwrap();
    hash
}

fn write_v2_records(root: &Path, module_id: &str, base_hash: &str, artifact_hash: &str) {
    let root = root.to_path_buf();
    write_records(&root, module_id, artifact_hash, true);
    let records = root.join(".appsdk/records");
    let project: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/project.json")).unwrap())
            .unwrap();
    let base_commit = project["modules"][0]["version_base"]["base_source_commit"]
        .as_str()
        .unwrap();
    for kind in [
        "worktree-record",
        "reproduction-record",
        "fix-candidate-record",
    ] {
        let file = records.join(format!("{kind}-{module_id}.json"));
        let mut record: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        record["base_commit"] = Value::String(base_commit.into());
        if kind == "worktree-record" {
            record["base_ref"] = Value::String(base_commit.into());
        }
        fs::write(&file, serde_json::to_string_pretty(&record).unwrap() + "\n").unwrap();
    }
    let promotion_file = records.join(format!("promotion-record-{module_id}.json"));
    let mut promotion: Value =
        serde_json::from_str(&fs::read_to_string(&promotion_file).unwrap()).unwrap();
    promotion["previous_active_version"] = Value::String("active-v1".into());
    promotion["new_active_version"] = Value::String("active-v2".into());
    promotion["base_commit"] = Value::String(base_commit.into());
    promotion["base_artifact_hash"] = Value::String(base_hash.into());
    promotion["public_api_hash"] = Value::String("api-2".into());
    fs::write(
        &promotion_file,
        serde_json::to_string_pretty(&promotion).unwrap() + "\n",
    )
    .unwrap();
    let commit = git_test_value(&root, &["rev-parse", "HEAD"]);
    let regression = serde_json::json!({
        "regression_report_id": "regression-app-core-v2",
        "module_id": module_id,
        "source_commit": commit,
        "artifact_hash": artifact_hash,
        "public_api_hash": "api-2",
        "scope_hash": "scope-1",
        "input_hash": artifact_hash,
        "suite_id": "app-core-regression",
        "command": {"program":"cargo","args":["test","--test","app-core"],"working_directory":"."},
        "test_count": 1,
        "passed": 1,
        "failed": 0,
        "skipped": 0,
        "result": "pass",
        "producer": {"adapter":"cargo","identity":"appsdk-regression-gate"},
        "created_at": "2026-01-01T00:00:00Z",
        "test_characteristics": {"whitebox":true,"blackbox":true}
    });
    fs::write(
        records.join(format!("regression-report-{module_id}.json")),
        serde_json::to_string_pretty(&regression).unwrap() + "\n",
    )
    .unwrap();
    let freeze = serde_json::json!({
        "freeze_id": "freeze-2",
        "issue_id": "issue-1",
        "module_id": module_id,
        "promotion_id": "promotion-1",
        "promotion_record_hash": digest(&canonical(&promotion)),
        "artifact_record_id": "candidate-evidence-1",
        "regression_report_id": "regression-app-core-v2",
        "regression_report_hash": digest(&canonical(&regression)),
        "source_commit_or_tag": commit,
        "active_version": "active-v2",
        "previous_active_version": "active-v1",
        "library_hash": artifact_hash,
        "public_api_hash": "api-2",
        "review_id": "review-1",
        "previous_active_immutable": true,
        "git_clean": true,
        "clean_scope": {"base_commit":commit,"changed_paths":[],"ignored_paths":[],"generated_policy":"tracked_hash"},
        "owners": {"vcs":"test","compiler":"test","api_extractor":"test","review":"test","artifact_registry":"test"},
        "created_at": "2026-01-01T00:00:00Z"
    });
    fs::write(
        records.join(format!("freeze-record-{module_id}.json")),
        serde_json::to_string_pretty(&freeze).unwrap() + "\n",
    )
    .unwrap();
}

fn enable_regression_contract(root: &PathBuf) {
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["modules"][0]["regression"] = serde_json::json!({
        "required_before_freeze": true,
        "suite_id": "app-core-regression",
        "command": {
            "program": "cargo",
            "args": ["test", "--test", "app-core"],
            "working_directory": "."
        },
        "input_paths": ["playground/experiments/**"],
        "minimum_test_count": 1,
        "allow_skipped": false,
        "ordinary_mode_after_freeze": "disabled",
        "reenable_on": [
            "source_change",
            "contract_change",
            "public_api_change",
            "artifact_change",
            "dependency_change"
        ]
    });
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
}

#[test]
fn new_project_rejects_unconfirmed_compile_and_promote() {
    let root = temp_root("negative");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    let compile = run(&["compile", root_text]);
    assert!(!compile.status.success());
    assert!(String::from_utf8_lossy(&compile.stderr).contains("GOAL_NOT_CONFIRMED:received"));
    let promote = run(&["promote", root_text, "--to", "source_implemented"]);
    assert!(!promote.status.success());
    assert!(String::from_utf8_lossy(&promote.stderr).contains("GOAL_NOT_CONFIRMED:received"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_allows_pending_clarification_but_compile_rejects_it() {
    let root = temp_root("clarification-pending");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let goal_file = root.join(".appsdk/goal.json");
    fs::write(&goal_file, r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"clarify","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":["scope"],"questions":[{"question_id":"q-1","question":"Which module?","status":"open"}],"status":"clarification_pending","confirmed_by":null,"confirmed_at":null,"created_at":"2026-01-01T00:00:00Z"}
"#).unwrap();
    let verified = run(&["verify", root_text]);
    assert!(verified.status.success());
    let compile = run(&["compile", root_text]);
    assert!(!compile.status.success());
    assert!(!String::from_utf8_lossy(&compile.stderr).is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn confirmed_goal_and_pinned_lock_allow_compile_and_adjacent_promote() {
    let root = temp_root("positive");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let goal_file = root.join(".appsdk/goal.json");
    fs::write(&goal_file, r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#).unwrap();
    fs::write(root.join(".appsdk/sdk.lock"), format!(r#"{{"sdk":"appsdk","version":"0.1.4","digest":"sha256:{}","compiler_digest":"sha256:{}","contract_schema":1}}
"#, "a".repeat(64), "b".repeat(64))).unwrap();
    fs::write(root.join(".appsdk/project.json"), r#"{
  "schema_version": 1,
  "project_id": "change-me",
  "sdk": {"name": "appsdk", "version": "0.1.4"},
  "lifecycle": {"stage": "draft"},
  "development_scenarios": {"manifest": ".appsdk/contracts/development-scenarios.manifest.json", "enabled": []},
  "access": {"protected_paths":[".appsdk/**"]},
  "governance": {"playground_root":"playground/**","active_root":"active/**","protected_root":"protected/**","generated_root":"generated/**","active_kind":"immutable_consumable_library","protected_kinds":["source"],"generated_kinds":["compiler_output"],"freeze_requirements":["git_clean"],"promotion_requires":["evidence"],"runtime_forbidden_roots":["playground/**"],"record_contracts":["contracts/records/worktree-record.schema.json","contracts/records/reproduction-record.schema.json","contracts/records/evidence-record.schema.json","contracts/records/fix-candidate-record.schema.json","contracts/records/goal-clarification-record.schema.json","contracts/records/review-record.schema.json","contracts/records/effectiveness-record.schema.json","contracts/records/collaboration-record.schema.json","contracts/records/collaboration-index.schema.json","contracts/records/merge-queue-record.schema.json","contracts/records/merge-queue-state.schema.json","contracts/records/integration-record.schema.json","contracts/records/mainline-receipt-record.schema.json","contracts/records/merge-record.schema.json","contracts/records/promotion-record.schema.json","contracts/records/regression-report.schema.json","contracts/records/freeze-record.schema.json","contracts/records/record-graph.contract.json"],"zone_transition_contract":"contracts/transitions/zone-transition-manifest.json","playground_retention":"archive_then_remove","debug_merge_comment_required":true},
  "lifecycles": {"issue":"open","library":"draft","source_snapshot":"mutable","artifact":"generated"},
    "modules": [{"module_id":"app-core","stage":"source_implemented","owned_paths":["playground/experiments/**"],"source_owner":"app-core","active_artifact":"active/lib/app-core/**","generated_outputs":["generated/**"],"contract_paths":["contracts/records/**","contracts/transitions/**"],"dependency_modules":[],"build":{"program":"sh","args":["-c","mkdir -p generated/modules/app-core/lib && printf 'app-core placeholder\\n' > generated/modules/app-core/lib/app-core.placeholder"],"working_directory":"."},"artifact_paths":["app-core.placeholder"],"regression":{"required_before_freeze":true,"suite_id":"app-core-regression","command":{"program":"cargo","args":["test"],"working_directory":"."},"input_paths":["playground/experiments/**"],"minimum_test_count":1,"allow_skipped":false,"ordinary_mode_after_freeze":"disabled","reenable_on":["source_change","contract_change","public_api_change","artifact_change","dependency_change"]}}]
}
"#).unwrap();
    pin_test_lock(root_text);
    let source_promote = run(&["promote", root_text, "--to", "source_implemented"]);
    assert!(
        source_promote.status.success(),
        "{}",
        String::from_utf8_lossy(&source_promote.stderr)
    );
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    let compile = run(&["compile", root_text]);
    assert!(compile.status.success());
    assert!(root.join("generated/project.compiled.json").exists());
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "contract_bound",
    ])
    .status
    .success());
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "compiled",
    ])
    .status
    .success());
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "controlled_verified",
    ])
    .status
    .success());
    let module = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ]);
    assert!(!module.status.success());
    assert!(
        String::from_utf8_lossy(&module.stderr)
            .contains("MISSING_RECORD:worktree-record-app-core.json"),
        "{}",
        String::from_utf8_lossy(&module.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn freeze_rejects_without_architecture_stage_and_records() {
    let root = temp_root("freeze");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let result = run(&["freeze", root_text, "--module", "app-core"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("GOAL_NOT_CONFIRMED:received"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn active_publish_rejects_unfrozen_module() {
    let root = temp_root("active");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let result = run(&[
        "publish-active",
        root_text,
        "--module",
        "app-core",
        "--version",
        "v1",
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr)
        .contains("ACTIVE_PUBLISH_REQUIRES_FROZEN_MODULE:app-core"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn full_module_freeze_and_active_publish_require_record_graph() {
    let root = temp_root("full-lifecycle");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    enable_regression_contract(&root);
    init_git(&root);
    fs::write(root.join(".appsdk/goal.json"), r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#).unwrap();
    fs::write(root.join(".appsdk/sdk.lock"), format!(r#"{{"sdk":"appsdk","version":"0.1.0","digest":"sha256:{}","compiler_digest":"sha256:{}","contract_schema":1}}
"#, "a".repeat(64), "b".repeat(64))).unwrap();
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["lifecycle"]["stage"] = Value::String("draft".into());
    project["modules"][0]["stage"] = Value::String("source_implemented".into());
    project["modules"][0]["owned_paths"] = serde_json::json!(["playground/experiments/**"]);
    project["modules"][0]["generated_outputs"] = serde_json::json!(["generated/**"]);
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    pin_test_lock(root_text);
    let source_promote = run(&["promote", root_text, "--to", "source_implemented"]);
    assert!(source_promote.status.success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    let edge_compile = run(&["compile", root_text]);
    assert!(
        edge_compile.status.success(),
        "{}",
        String::from_utf8_lossy(&edge_compile.stderr)
    );
    assert!(run(&["promote", root_text, "--to", "compiled"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "controlled_verified"])
        .status
        .success());
    let edge_compile = run(&["compile", root_text]);
    assert!(
        edge_compile.status.success(),
        "{}",
        String::from_utf8_lossy(&edge_compile.stderr)
    );
    for stage in ["contract_bound", "compiled", "controlled_verified"] {
        assert!(run(&[
            "promote-module",
            root_text,
            "--module",
            "app-core",
            "--to",
            stage
        ])
        .status
        .success());
    }
    assert!(!run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable"
    ])
    .status
    .success());
    let module_artifact: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json")).unwrap(),
    )
    .unwrap();
    let architecture_hash = module_artifact
        .get("artifact_hash")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    write_records(&root, "app-core", &architecture_hash, false);
    let review_file = root.join(".appsdk/records/review-record-app-core.json");
    let mut stale_review: Value =
        serde_json::from_str(&fs::read_to_string(&review_file).unwrap()).unwrap();
    stale_review["resource_map_hash"] = Value::String("sha256:stale".into());
    fs::write(
        &review_file,
        serde_json::to_string_pretty(&stale_review).unwrap() + "\n",
    )
    .unwrap();
    let stale_architecture = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ]);
    assert!(!stale_architecture.status.success());
    assert!(String::from_utf8_lossy(&stale_architecture.stderr)
        .contains("ARCHITECTURE_REVIEW_MAP_STALE"));
    write_records(&root, "app-core", &architecture_hash, false);
    let mut missing_review_evidence: Value =
        serde_json::from_str(&fs::read_to_string(&review_file).unwrap()).unwrap();
    missing_review_evidence["evidence_ids"]
        .as_array_mut()
        .unwrap()
        .push(Value::String("missing-review-evidence".into()));
    fs::write(
        &review_file,
        serde_json::to_string_pretty(&missing_review_evidence).unwrap() + "\n",
    )
    .unwrap();
    let missing_review_result = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ]);
    assert!(!missing_review_result.status.success());
    assert!(String::from_utf8_lossy(&missing_review_result.stderr)
        .contains("MISSING_EVIDENCE_RECORD:missing-review-evidence"));
    write_records(&root, "app-core", &architecture_hash, false);
    for relative in [
        ".appsdk/records/effectiveness-record-app-core.json",
        ".appsdk/records/merge-record-app-core.json",
        ".appsdk/records/promotion-record-app-core.json",
        ".appsdk/records/playground-cleanup-cleanup-1.json",
        ".appsdk/records/evidence/app-core/effective-1.json",
    ] {
        fs::remove_file(root.join(relative)).unwrap();
    }
    let architecture = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ]);
    assert!(
        architecture.status.success(),
        "{}",
        String::from_utf8_lossy(&architecture.stderr)
    );
    assert!(!run(&["freeze", root_text, "--module", "app-core"])
        .status
        .success());
    write_records(&root, "app-core", &architecture_hash, false);
    for relative in [
        ".appsdk/records/merge-record-app-core.json",
        ".appsdk/records/promotion-record-app-core.json",
        ".appsdk/records/playground-cleanup-cleanup-1.json",
    ] {
        fs::remove_file(root.join(relative)).unwrap();
    }
    let effectiveness_only = run(&["verify", root_text]);
    assert!(
        effectiveness_only.status.success(),
        "{}",
        String::from_utf8_lossy(&effectiveness_only.stderr)
    );
    write_records(&root, "app-core", &architecture_hash, true);
    let effectiveness_file = root.join(".appsdk/records/effectiveness-record-app-core.json");
    let mut stale_effectiveness: Value =
        serde_json::from_str(&fs::read_to_string(&effectiveness_file).unwrap()).unwrap();
    stale_effectiveness["source_unchanged_since_review"] = Value::Bool(false);
    fs::write(
        &effectiveness_file,
        serde_json::to_string_pretty(&stale_effectiveness).unwrap() + "\n",
    )
    .unwrap();
    let stale_replay = run(&["verify", root_text]);
    assert!(!stale_replay.status.success());
    assert!(String::from_utf8_lossy(&stale_replay.stderr)
        .contains("POST_ARCHITECTURE_EFFECTIVENESS_MISMATCH"));
    write_records(&root, "app-core", &architecture_hash, true);
    let merge_file = root.join(".appsdk/records/merge-record-app-core.json");
    let promotion_file = root.join(".appsdk/records/promotion-record-app-core.json");
    let mut invalid_merge: Value =
        serde_json::from_str(&fs::read_to_string(&merge_file).unwrap()).unwrap();
    invalid_merge["merge_commit"] = Value::String("missing-merge-commit".into());
    fs::write(
        &merge_file,
        serde_json::to_string_pretty(&invalid_merge).unwrap() + "\n",
    )
    .unwrap();
    let mut invalid_promotion: Value =
        serde_json::from_str(&fs::read_to_string(&promotion_file).unwrap()).unwrap();
    invalid_promotion["merged_commit"] = Value::String("missing-merge-commit".into());
    invalid_promotion["source_commit"] = Value::String("missing-merge-commit".into());
    fs::write(
        &promotion_file,
        serde_json::to_string_pretty(&invalid_promotion).unwrap() + "\n",
    )
    .unwrap();
    let invalid_merge_result = run(&["verify", root_text]);
    assert!(!invalid_merge_result.status.success());
    assert!(String::from_utf8_lossy(&invalid_merge_result.stderr)
        .contains("MAINLINE_MERGE_COMMIT_MISSING"));
    write_records(&root, "app-core", &architecture_hash, true);
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "promotion-records"])
        .status()
        .unwrap()
        .success());
    let missing_regression = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "frozen",
    ]);
    assert!(!missing_regression.status.success());
    assert!(String::from_utf8_lossy(&missing_regression.stderr)
        .contains("MISSING_RECORD:regression-report-app-core.json"));
    let regression_report_hash = write_regression_report(&root, "app-core", &architecture_hash);
    let freeze_record = root.join(".appsdk/records/freeze-record-app-core.json");
    let mut freeze_record_value: Value =
        serde_json::from_str(&fs::read_to_string(&freeze_record).unwrap()).unwrap();
    freeze_record_value["regression_report_id"] = Value::String("regression-app-core-v1".into());
    freeze_record_value["regression_report_hash"] = Value::String(regression_report_hash);
    fs::write(
        &freeze_record,
        serde_json::to_string_pretty(&freeze_record_value).unwrap() + "\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "regression-report"])
        .status()
        .unwrap()
        .success());
    let freeze = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "frozen",
    ]);
    assert!(
        freeze.status.success(),
        "{}",
        String::from_utf8_lossy(&freeze.stderr)
    );
    assert!(root
        .join("protected/history/app-core/freeze-artifact.json")
        .exists());
    assert!(root
        .join("protected/history/app-core/module-contract.json")
        .exists());
    assert!(root
        .join("protected/history/app-core/source-snapshot.json")
        .exists());
    let pub_result = run(&[
        "publish-active",
        root_text,
        "--module",
        "app-core",
        "--version",
        "active-v1",
    ]);
    assert!(pub_result.status.success());
    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    assert!(root
        .join("active/lib/app-core/active-v1/artifact.json")
        .exists());
    let duplicate = run(&[
        "publish-active",
        root_text,
        "--module",
        "app-core",
        "--version",
        "active-v1",
    ]);
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("ACTIVE_VERSION_EXISTS"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn frozen_module_keeps_other_modules_mutable() {
    let root = temp_root("module-independence");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    enable_regression_contract(&root);
    init_git(&root);
    fs::write(root.join(".appsdk/goal.json"), r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#).unwrap();
    fs::write(root.join(".appsdk/sdk.lock"), format!(r#"{{"sdk":"appsdk","version":"0.1.0","digest":"sha256:{}","compiler_digest":"sha256:{}","contract_schema":1}}
"#, "a".repeat(64), "b".repeat(64))).unwrap();
    pin_test_lock(root_text);

    assert!(run(&["promote", root_text, "--to", "source_implemented"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    assert!(run(&["compile", root_text]).status.success());
    assert!(run(&["promote", root_text, "--to", "compiled"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "controlled_verified"])
        .status
        .success());
    for stage in ["contract_bound", "compiled", "controlled_verified"] {
        assert!(run(&[
            "promote-module",
            root_text,
            "--module",
            "app-core",
            "--to",
            stage
        ])
        .status
        .success());
    }
    let module_artifact: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json")).unwrap(),
    )
    .unwrap();
    let architecture_hash = module_artifact
        .get("artifact_hash")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    write_records(&root, "app-core", &architecture_hash, false);
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable"
    ])
    .status
    .success());
    write_records(&root, "app-core", &architecture_hash, true);
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "promotion-records"])
        .status()
        .unwrap()
        .success());
    let regression_report_hash = write_regression_report(&root, "app-core", &architecture_hash);
    let freeze_record = root.join(".appsdk/records/freeze-record-app-core.json");
    let mut freeze_record_value: Value =
        serde_json::from_str(&fs::read_to_string(&freeze_record).unwrap()).unwrap();
    freeze_record_value["regression_report_id"] = Value::String("regression-app-core-v1".into());
    freeze_record_value["regression_report_hash"] = Value::String(regression_report_hash);
    fs::write(
        &freeze_record,
        serde_json::to_string_pretty(&freeze_record_value).unwrap() + "\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "regression-report"])
        .status()
        .unwrap()
        .success());
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "frozen"
    ])
    .status
    .success());
    assert!(run(&[
        "publish-active",
        root_text,
        "--module",
        "app-core",
        "--version",
        "active-v1"
    ])
    .status
    .success());
    // Protected archive must contain the frozen module's source, library,
    // contract files, module contract, and hashes, so a frozen module is a
    // self-contained audit unit.
    for path in [
        "protected/history/app-core/source/playground/experiments",
        "protected/history/app-core/library/app-core.placeholder",
        "protected/history/app-core/contracts/records/evidence-record.schema.json",
        "protected/history/app-core/contracts/transitions/zone-transition-manifest.json",
        "protected/history/app-core/module-contract.json",
        "protected/history/app-core/freeze-artifact.json",
    ] {
        assert!(
            root.join(path).exists(),
            "protected archive is incomplete: {path}"
        );
    }
    let frozen_artifact_hash = {
        let value: Value = serde_json::from_str(
            &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json"))
                .unwrap(),
        )
        .unwrap();
        value
            .get("artifact_hash")
            .and_then(Value::as_str)
            .unwrap()
            .to_string()
    };

    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    let app_core_regression = project["modules"][0]["regression"].clone();
    project["modules"].as_array_mut().unwrap().push(serde_json::json!({
        "module_id": "app-edge",
        "stage": "source_implemented",
        "owned_paths": ["playground/experiments-edge/**"],
        "source_owner": "app-edge",
        "active_artifact": "active/lib/app-edge/**",
        "generated_outputs": ["generated/**"],
        "contract_paths": ["contracts/records/**", "contracts/transitions/**"],
        "dependency_modules": [],
        "build": {
            "program": "sh",
            "args": ["-c", "mkdir -p generated/modules/app-edge/lib && printf 'app-edge placeholder\\n' > generated/modules/app-edge/lib/app-edge.placeholder"],
            "working_directory": "."
        },
        "artifact_paths": ["app-edge.placeholder"],
        "regression": app_core_regression
    }));
    fs::create_dir_all(root.join("playground/experiments-edge")).unwrap();
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "add app-edge module"])
        .status()
        .unwrap()
        .success());

    assert!(run(&["compile", root_text]).status.success());
    let frozen_after: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        frozen_after
            .get("artifact_hash")
            .and_then(Value::as_str)
            .unwrap(),
        frozen_artifact_hash
    );
    let edge_artifact: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-edge/module.compiled.json")).unwrap(),
    )
    .unwrap();
    assert_ne!(
        edge_artifact
            .get("artifact_hash")
            .and_then(Value::as_str)
            .unwrap(),
        frozen_artifact_hash
    );
    for stage in ["contract_bound", "compiled", "controlled_verified"] {
        let promoted = run(&[
            "promote-module",
            root_text,
            "--module",
            "app-edge",
            "--to",
            stage,
        ]);
        assert!(
            promoted.status.success(),
            "promote app-edge {} failed: {}",
            stage,
            String::from_utf8_lossy(&promoted.stderr)
        );
    }
    let edge_hash = edge_artifact
        .get("artifact_hash")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    write_records(&root, "app-edge", &edge_hash, false);
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "app-edge records"])
        .status()
        .unwrap()
        .success());
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-edge",
        "--to",
        "architecture_stable"
    ])
    .status
    .success());
    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn begin_version_preserves_v1_and_opens_a_version_bound_source_stage() {
    let root = temp_root("begin-version");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    enable_regression_contract(&root);
    init_git(&root);
    fs::write(root.join(".appsdk/goal.json"), r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#).unwrap();
    pin_test_lock(root_text);
    assert!(run(&["promote", root_text, "--to", "source_implemented"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    assert!(run(&["compile", root_text]).status.success());
    assert!(run(&["promote", root_text, "--to", "compiled"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "controlled_verified"])
        .status
        .success());
    for stage in ["contract_bound", "compiled", "controlled_verified"] {
        assert!(run(&[
            "promote-module",
            root_text,
            "--module",
            "app-core",
            "--to",
            stage
        ])
        .status
        .success());
    }
    let module_artifact: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json")).unwrap(),
    )
    .unwrap();
    let v1_hash = module_artifact["artifact_hash"]
        .as_str()
        .unwrap()
        .to_string();
    write_records(&root, "app-core", &v1_hash, false);
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable"
    ])
    .status
    .success());
    write_records(&root, "app-core", &v1_hash, true);
    let regression_hash = write_regression_report(&root, "app-core", &v1_hash);
    let freeze_file = root.join(".appsdk/records/freeze-record-app-core.json");
    let mut freeze: Value =
        serde_json::from_str(&fs::read_to_string(&freeze_file).unwrap()).unwrap();
    freeze["regression_report_id"] = Value::String("regression-app-core-v1".into());
    freeze["regression_report_hash"] = Value::String(regression_hash);
    fs::write(
        &freeze_file,
        serde_json::to_string_pretty(&freeze).unwrap() + "\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "freeze-v1"])
        .status()
        .unwrap()
        .success());
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "frozen"
    ])
    .status
    .success());
    assert!(run(&[
        "publish-active",
        root_text,
        "--module",
        "app-core",
        "--version",
        "active-v1"
    ])
    .status
    .success());
    let active_v1 = root.join("active/lib/app-core/active-v1/artifact.json");
    let active_v1_text = fs::read_to_string(&active_v1).unwrap();

    let wrong_from = run(&[
        "begin-version",
        root_text,
        "--module",
        "app-core",
        "--from",
        "active-v0",
        "--to",
        "active-v2",
    ]);
    assert!(!wrong_from.status.success());
    assert!(String::from_utf8_lossy(&wrong_from.stderr).contains("MODULE_VERSION_FROM_NOT_CURRENT"));

    let opened = run(&[
        "begin-version",
        root_text,
        "--module",
        "app-core",
        "--from",
        "active-v1",
        "--to",
        "active-v2",
    ]);
    assert!(
        opened.status.success(),
        "{}",
        String::from_utf8_lossy(&opened.stderr)
    );
    let project: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/project.json")).unwrap())
            .unwrap();
    let v1_promotion: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/promotion-record-app-core.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(project["modules"][0]["stage"], "source_implemented");
    assert_eq!(
        project["modules"][0]["version_base"],
        serde_json::json!({
            "previous_active_version": "active-v1",
            "new_active_version": "active-v2",
            "base_artifact_hash": v1_hash,
            "base_source_commit": v1_promotion["source_commit"]
        })
    );
    assert_eq!(fs::read_to_string(&active_v1).unwrap(), active_v1_text);
    assert!(root
        .join("protected/history-versions/app-core/active-v1/freeze-artifact.json")
        .is_file());
    assert!(!root.join("protected/history/app-core").exists());
    assert!(root
        .join(".appsdk/records/history/app-core/active-v1/freeze-record-app-core.json")
        .is_file());
    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    // Version transitions may legitimately change the module build command
    // (for example migrating a frozen consumer to a resolver-managed link
    // surface). The previous Active artifact stays immutable; only the current
    // module contract advances, and the v2 regression still gates freeze.
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["modules"][0]["build"]["args"] = serde_json::json!([
        "-c",
        "mkdir -p generated/modules/app-core/lib && printf 'app-core placeholder v2\\n' > generated/modules/app-core/lib/app-core.placeholder"
    ]);
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "v2 build command change"])
        .status()
        .unwrap()
        .success());
    assert!(run(&["compile-module", root_text, "--module", "app-core"])
        .status
        .success());
    for stage in ["contract_bound", "compiled", "controlled_verified"] {
        assert!(run(&[
            "promote-module",
            root_text,
            "--module",
            "app-core",
            "--to",
            stage,
        ])
        .status
        .success());
    }
    let v2_artifact: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json")).unwrap(),
    )
    .unwrap();
    let v2_hash = v2_artifact["artifact_hash"].as_str().unwrap();
    write_v2_records(&root, "app-core", &v1_hash, v2_hash);
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ])
    .status
    .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "freeze-v2"])
        .status()
        .unwrap()
        .success());
    let frozen_v2 = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "frozen",
    ]);
    assert!(
        frozen_v2.status.success(),
        "{}",
        String::from_utf8_lossy(&frozen_v2.stderr)
    );
    let published = run(&[
        "publish-active",
        root_text,
        "--module",
        "app-core",
        "--version",
        "active-v2",
    ]);
    assert!(
        published.status.success(),
        "{}",
        String::from_utf8_lossy(&published.stderr)
    );
    assert_eq!(fs::read_to_string(&active_v1).unwrap(), active_v1_text);
    assert!(root
        .join("active/lib/app-core/active-v2/artifact.json")
        .is_file());
    let project: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/project.json")).unwrap())
            .unwrap();
    assert!(project["modules"][0].get("version_base").is_none());
    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}
