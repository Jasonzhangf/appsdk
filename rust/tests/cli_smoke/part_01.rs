use serde_json::Value;
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_appsdk"))
}

fn memory_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_project-memory"))
}

fn test_global_registry_root_for_project(project: &Path) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    project.to_string_lossy().hash(&mut hasher);
    std::env::temp_dir().join(format!(
        "appsdk-rust-global-registry-tests-{}-{:016x}",
        std::process::id(),
        hasher.finish()
    ))
}

fn test_global_registry_root_for_args(args: &[&str]) -> PathBuf {
    args.iter()
        .skip(1)
        .map(Path::new)
        .find(|path| path.is_absolute())
        .map(test_global_registry_root_for_project)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "appsdk-rust-global-registry-tests-{}-default",
                std::process::id()
            ))
        })
}

#[cfg(unix)]
fn hold_advisory_lock(file: &fs::File) {
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    assert_eq!(unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) }, 0);
}

#[cfg(unix)]
fn assert_reset_lock_released(path: &Path) {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap_or_else(|error| panic!("lock path {}: {error}", path.display()));
    hold_advisory_lock(&file);
}

#[cfg(not(unix))]
fn assert_reset_lock_released(path: &Path) {
    assert!(path.is_file(), "lock path {} is missing", path.display());
}

fn reset_transaction_lock_path(root: &Path) -> PathBuf {
    let transaction_dir = root.parent().unwrap().join(format!(
        ".appsdk-reset-transaction-{}",
        root.file_name().unwrap().to_string_lossy()
    ));
    PathBuf::from(format!("{}.lock", transaction_dir.display()))
}

fn fresh_reset_marker_targets(root: &Path, generated_roots: &[&str]) -> Vec<Value> {
    let mut generated = Vec::new();
    for relative in generated_roots {
        if generated.iter().any(|existing: &&str| {
            *relative == *existing || relative.starts_with(&format!("{existing}/"))
        }) {
            continue;
        }
        generated.retain(|existing: &&str| !existing.starts_with(&format!("{relative}/")));
        generated.push(*relative);
    }

    let mut contract_relatives = Vec::new();
    for directory in ["contracts/records", "contracts/transitions"] {
        for entry in fs::read_dir(root.join(directory)).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                contract_relatives.push(
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
    ] {
        if !contract_relatives
            .iter()
            .any(|existing| existing == relative)
        {
            contract_relatives.push(relative.to_string());
        }
    }
    contract_relatives.sort();

    let mut relatives = vec![".appsdk".to_string(), ".appsdk-control".to_string()];
    relatives.extend(generated.iter().map(|relative| (*relative).to_string()));
    relatives.extend(contract_relatives);
    relatives.push(".gitignore".to_string());
    relatives
        .iter()
        .enumerate()
        .map(|(index, relative)| {
            let kind = if relative == ".appsdk"
                || relative == ".appsdk-control"
                || generated.iter().any(|root| *root == relative)
            {
                "dir"
            } else {
                "file"
            };
            let staged =
                if generated.iter().any(|root| *root == relative) && relative != "generated" {
                    Value::Null
                } else {
                    Value::String(format!("staging/{relative}"))
                };
            serde_json::json!({
                "relative": relative,
                "kind": kind,
                "original_exists": fs::symlink_metadata(root.join(relative)).is_ok(),
                "backup": format!("quarantine/target-{index}"),
                "staged": staged,
                "quarantined": false,
                "published": false
            })
        })
        .collect()
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
    // These repositories are deleted by the test. Detached maintenance can
    // recreate .git/objects after teardown has already removed it.
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "config",
            "maintenance.auto",
            "false",
        ])
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
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "branch", "-M", "codex/test"])
        .status()
        .unwrap()
        .success());
}

fn run(args: &[&str]) -> std::process::Output {
    let mut command = Command::new(binary());
    command
        .args(args)
        .env("APPSDK_HOME", test_global_registry_root_for_args(args))
        .env_remove("TMUX_PANE");
    if let Some(root) = args
        .iter()
        .skip(1)
        .map(Path::new)
        .find(|path| path.is_absolute())
    {
        apply_test_git_bug_fixture(&mut command, root);
    } else {
        command.env_remove("GIT_BUG_BIN");
    }
    command.output().unwrap()
}

fn run_in(root: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(binary());
    command
        .args(args)
        .current_dir(root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(root))
        .env_remove("TMUX_PANE");
    apply_test_git_bug_fixture(&mut command, root);
    command.output().unwrap()
}

fn run_bug_in(root: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(binary());
    command
        .args(args)
        .current_dir(root)
        .env("APPSDK_ROOT", root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(root))
        .env_remove("TMUX_PANE");
    apply_test_git_bug_fixture(&mut command, root);
    command.output().unwrap()
}

fn test_git_bug_fixture_for_root(root: &Path) -> Option<PathBuf> {
    let candidate = root.with_extension("fake-git-bug").join("git-bug");
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

fn apply_test_git_bug_fixture(command: &mut Command, root: &Path) {
    if let Some(path) = test_git_bug_fixture_for_root(root) {
        command.env("GIT_BUG_BIN", path);
    } else {
        command.env_remove("GIT_BUG_BIN");
    }
}

fn run_memory(root: &Path, args: &[&str], home: &Path) -> std::process::Output {
    Command::new(memory_binary())
        .args(args)
        .current_dir(root)
        .env("PROJECT_MEMORY_HOME", home)
        .env("COLLAB_STATE_DIR", home.join("collab"))
        .output()
        .unwrap()
}

fn filesystem_merges_case(root: &Path) -> bool {
    match (
        fs::canonicalize(root.join(".appsdk")),
        fs::canonicalize(root.join(".APPSDK")),
    ) {
        (Ok(actual), Ok(variant)) => actual == variant,
        _ => false,
    }
}

#[test]
fn project_commands_default_to_cwd_and_help_never_resolves_a_project() {
    let root = temp_root("cwd-default");
    fs::create_dir_all(&root).unwrap();

    for args in [
        &["--help"][..],
        &["verify", "--help"],
        &["compile", "--help"],
    ] {
        let help = run_in(&root, args);
        assert!(
            help.status.success(),
            "{}",
            String::from_utf8_lossy(&help.stderr)
        );
        assert!(String::from_utf8_lossy(&help.stdout).contains("Usage:"));
        assert!(!String::from_utf8_lossy(&help.stderr).contains("PROJECT_ROOT_MISSING"));
    }

    assert!(run(&["new", root.to_str().unwrap()]).status.success());
    assert!(run_in(&root, &["verify"]).status.success());
    assert!(run_in(&root, &["guide", "compile"]).status.success());
    assert!(run_in(&root, &["guide", "status"]).status.success());
    fs::remove_dir_all(root).unwrap();
}

fn sdk_source_registry_result(
    name: &str,
    modules: Value,
    source_paths: &[&str],
) -> std::process::Output {
    let root = temp_root(name);
    fs::create_dir_all(root.join("contracts/maps")).unwrap();
    fs::write(
        root.join("contracts/maps/module-registry.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "modules": modules,
        }))
        .unwrap(),
    )
    .unwrap();
    for source_path in source_paths {
        let path = root.join(source_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }
    init_git(&root);
    let output = run_in(&root, &["verify-sdk-source-registry", "."]);
    fs::remove_dir_all(root).unwrap();
    output
}

#[test]
fn sdk_source_registry_enforces_single_active_owner_for_long_horizon_sources() {
    let architecture_registry = serde_json::json!({
        "module_id": "architecture-registry",
        "status": "active",
        "owner": "appsdk::architecture",
        "owned_paths": ["contracts/maps/**"],
        "forbidden_paths": ["active/lib/**", "protected/**"]
    });

    let owned = sdk_source_registry_result(
        "sdk-source-registry-owned",
        serde_json::json!([
            architecture_registry.clone(),
            {
                "module_id": "runtime-core",
                "status": "active",
                "owner": "appsdk::runtime",
                "owned_paths": [
                    "rust/src/long_horizon_policy.rs",
                    "rust/src/long_horizon_role.rs"
                ],
                "forbidden_paths": ["active/lib/**", "protected/**"]
            }
        ]),
        &[
            "rust/src/long_horizon_policy.rs",
            "rust/src/long_horizon_role.rs",
        ],
    );
    assert!(
        owned.status.success(),
        "{}",
        String::from_utf8_lossy(&owned.stderr)
    );
    assert!(String::from_utf8_lossy(&owned.stdout).contains("\"gate\":\"sdk_source_registry\""));

    let unowned = sdk_source_registry_result(
        "sdk-source-registry-unowned",
        serde_json::json!([architecture_registry.clone()]),
        &["rust/src/long_horizon_policy.rs"],
    );
    assert!(!unowned.status.success());
    assert!(String::from_utf8_lossy(&unowned.stderr)
        .contains("SDK_SOURCE_OWNER_CARDINALITY:rust/src/long_horizon_policy.rs:"));

    let multiply_owned = sdk_source_registry_result(
        "sdk-source-registry-multiply-owned",
        serde_json::json!([
            architecture_registry,
            {
                "module_id": "runtime-core-a",
                "status": "active",
                "owner": "appsdk::runtime_a",
                "owned_paths": ["rust/src/long_horizon_role.rs"],
                "forbidden_paths": ["active/lib/**", "protected/**"]
            },
            {
                "module_id": "runtime-core-b",
                "status": "active",
                "owner": "appsdk::runtime_b",
                "owned_paths": ["rust/src/long_horizon_role.rs"],
                "forbidden_paths": ["active/lib/**", "protected/**"]
            }
        ]),
        &["rust/src/long_horizon_role.rs"],
    );
    assert!(!multiply_owned.status.success());
    assert!(String::from_utf8_lossy(&multiply_owned.stderr).contains(
        "SDK_SOURCE_OWNER_CARDINALITY:rust/src/long_horizon_role.rs:runtime-core-a,runtime-core-b"
    ));
}

#[test]
fn sdk_source_registry_requires_project_contract_before_ignoring_governance_files() {
    let result = sdk_source_registry_result(
        "sdk-source-registry-project-governance",
        serde_json::json!([
            {
                "module_id": "architecture-registry",
                "status": "active",
                "owner": "appsdk::architecture",
                "owned_paths": ["contracts/maps/**"],
                "forbidden_paths": ["active/lib/**", "protected/**"]
            },
            {
                "module_id": "runtime-core",
                "status": "active",
                "owner": "appsdk::runtime",
                "owned_paths": ["rust/src/main.rs"],
                "forbidden_paths": ["active/lib/**", "protected/**"]
            }
        ]),
        &[
            "AGENTS.md",
            ".appsdk-prepare.json",
            ".appsdk/records/legacy.json",
            "rust/src/main.rs",
        ],
    );
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("SDK_SOURCE_OWNER_CARDINALITY:"));
}

#[test]
fn sdk_source_registry_rejects_invalid_project_contract() {
    let result = sdk_source_registry_result(
        "sdk-source-registry-invalid-project-contract",
        serde_json::json!([{
            "module_id": "runtime-core",
            "status": "active",
            "owner": "appsdk::runtime",
            "owned_paths": ["rust/src/main.rs"],
            "forbidden_paths": []
        }]),
        &[".appsdk/project.json", "rust/src/main.rs"],
    );
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("INVALID_PROJECT_CONTRACT"));
}

#[test]
fn sdk_source_registry_ignores_project_governance_files_with_valid_contract() {
    let root = temp_root("sdk-source-registry-initialized-workspace");
    let root_text = root.to_str().unwrap();
    let created = run(&["new", root_text]);
    assert!(
        created.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&created.stdout),
        String::from_utf8_lossy(&created.stderr)
    );
    let prepared = run(&["prepare", root_text]);
    assert!(
        prepared.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&prepared.stdout),
        String::from_utf8_lossy(&prepared.stderr)
    );
    fs::create_dir_all(root.join("contracts/maps")).unwrap();
    fs::write(
        root.join("contracts/maps/module-registry.json"),
        include_str!("../../../contracts/maps/module-registry.json"),
    )
    .unwrap();
    fs::create_dir_all(root.join("rust/src")).unwrap();
    fs::write(root.join("rust/src/main.rs"), "fn main() {}\n").unwrap();
    let initialized = run(&["init", root_text]);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    init_git(&root);
    let result = run_in(&root, &["verify-sdk-source-registry", "."]);
    assert!(
        result.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("\"gate\":\"sdk_source_registry\""));
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
fn reset_governance_discards_only_control_plane_and_is_idempotent() {
    let root = temp_root("reset-governance");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["project_id"] = Value::String("preserved-reset-project".into());
    project["modules"][0]["module_note"] = Value::String("preserve-module-field".into());
    project["modules"][0]["build"]["args"][1] =
        Value::String("mkdir -p generated/modules/app-core/lib && printf 'preserved\\n' > generated/modules/app-core/lib/app-core.placeholder".into());
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let project_before = fs::read(&project_path).unwrap();
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    fs::write(root.join("active/legacy.txt"), "retain-active\n").unwrap();
    fs::write(
        root.join("protected/history/legacy.txt"),
        "retain-protected\n",
    )
    .unwrap();
    fs::write(root.join(".appsdk/legacy-record.json"), "legacy\n").unwrap();
    fs::write(
        root.join(".appsdk/records/reset-governance-record.json"),
        "{\"schema_version\":1,\"reset_id\":\"legacy-reset\",\"mode\":\"discard_legacy_control_plane\",\"preserved\":[\"business_source\"],\"removed\":[\".appsdk\"],\"branch\":\"codex/legacy\",\"created_at\":\"2026-01-01T00:00:00Z\"}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("generated/old-artifact")).unwrap();
    fs::write(root.join("generated/old-artifact/artifact.bin"), "old\n").unwrap();
    fs::create_dir_all(root.join(".appsdk-control/runtime")).unwrap();
    init_git(&root);
    assert!(Command::new("git")
        .args(["-C", root_text, "branch", "-M", "codex/reset-test"])
        .status()
        .unwrap()
        .success());
    let reset = run(&["reset-governance", root_text, "--discard-legacy"]);
    assert!(
        reset.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&reset.stdout),
        String::from_utf8_lossy(&reset.stderr)
    );
    let reset_result: Value = String::from_utf8_lossy(&reset.stdout)
        .lines()
        .find_map(|line| serde_json::from_str::<Value>(line).ok())
        .expect("reset must emit a machine-readable result");
    assert_eq!(reset_result["operation"], "governance.reset");
    assert_eq!(reset_result["status"], "completed");
    assert_eq!(reset_result["development_ready"], true);
    assert_eq!(reset_result["delivery_verified"], false);
    assert_eq!(reset_result["baseline_status"], "required");
    assert_eq!(reset_result["registration_status"], "pending");
    assert_eq!(reset_result["next_action"], "run_applicable_validation");
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("active/legacy.txt")).unwrap(),
        "retain-active\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("protected/history/legacy.txt")).unwrap(),
        "retain-protected\n"
    );
    assert_eq!(fs::read(&project_path).unwrap(), project_before);
    assert!(root
        .join(".appsdk/records/reset-governance-record.json")
        .exists());
    assert!(!root.join(".appsdk/legacy-record.json").exists());
    assert!(!root.join(".appsdk-control/runtime").exists());
    assert!(!root.join("generated/old-artifact").exists());
    let reset_path = root.join(".appsdk/records/reset-governance-record.json");
    let first: Value = serde_json::from_str(&fs::read_to_string(&reset_path).unwrap()).unwrap();
    assert_eq!(first["mode"], "discard_legacy_control_plane");
    let first_transaction_id = first["transaction_id"].as_str().unwrap().to_string();
    assert_eq!(first["reset_id"], first["transaction_id"]);
    assert!(first_transaction_id.starts_with("reset-governance-"));
    assert_ne!(first_transaction_id, "legacy-reset");
    assert_reset_lock_released(&reset_transaction_lock_path(&root));

    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "commit first reset"])
        .status()
        .unwrap()
        .success());
    fs::write(root.join(".appsdk/after-first.txt"), "must be removed\n").unwrap();
    fs::write(
        root.join(".appsdk/records/old-review-record.json"),
        "{\"old\":true}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".appsdk-control/runtime-two")).unwrap();
    fs::create_dir_all(root.join("generated/old-artifact-two")).unwrap();
    fs::write(
        root.join("generated/old-artifact-two/artifact.bin"),
        "old-two\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            root_text,
            "commit",
            "-m",
            "commit second reset inputs",
        ])
        .status()
        .unwrap()
        .success());

    assert!(run(&["reset-governance", root_text, "--discard-legacy"])
        .status
        .success());
    assert!(!root.join(".appsdk/after-first.txt").exists());
    assert!(!root.join(".appsdk/records/old-review-record.json").exists());
    assert!(!root.join(".appsdk-control/runtime-two").exists());
    assert!(!root.join("generated/old-artifact-two").exists());
    assert_eq!(fs::read(&project_path).unwrap(), project_before);
    let second: Value = serde_json::from_str(&fs::read_to_string(&reset_path).unwrap()).unwrap();
    assert_eq!(second["mode"], "discard_legacy_control_plane");
    let second_transaction_id = second["transaction_id"].as_str().unwrap().to_string();
    assert_eq!(second["reset_id"], second["transaction_id"]);
    assert_ne!(second_transaction_id, first_transaction_id);
    assert_reset_lock_released(&reset_transaction_lock_path(&root));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reset_governance_recovers_committed_discard_transaction_marker() {
    let root = temp_root("reset-governance-committed-discard-marker");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project_path = root.join(".appsdk/project.json");
    let project = fs::read_to_string(&project_path).unwrap();
    fs::write(
        &project_path,
        project.replace(
            "\"generated_root\": \"generated/**\"",
            "\"generated_root\": \"build-output/**\"",
        ),
    )
    .unwrap();
    fs::create_dir_all(root.join("build-output")).unwrap();
    fs::write(root.join("build-output/legacy.bin"), "legacy\n").unwrap();
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    init_git(&root);

    let reset = run(&["reset-governance", root_text, "--discard-legacy"]);
    assert!(
        reset.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&reset.stdout),
        String::from_utf8_lossy(&reset.stderr)
    );
    let reset_path = root.join(".appsdk/records/reset-governance-record.json");
    let record: Value = serde_json::from_str(&fs::read_to_string(&reset_path).unwrap()).unwrap();
    let transaction_id = record["transaction_id"].as_str().unwrap().to_string();
    assert_eq!(record["mode"], "discard_legacy_control_plane");
    assert!(!root.join("build-output").exists());

    let transaction = root.parent().unwrap().join(format!(
        ".appsdk-reset-transaction-{}",
        root.file_name().unwrap().to_string_lossy()
    ));
    fs::create_dir_all(transaction.join("quarantine")).unwrap();
    let mut targets = fresh_reset_marker_targets(&root, &["generated", "build-output"]);
    for target in &mut targets {
        target["original_exists"] = Value::Bool(true);
        target["quarantined"] = Value::Bool(true);
        target["published"] = Value::Bool(target["staged"].is_string());
    }
    fs::write(
        transaction.join("marker.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "transaction_id": transaction_id,
            "root": root.to_string_lossy(),
            "mode": "discard_legacy_control_plane",
            "phase": "committed",
            "error": null,
            "created_dirs": [],
            "generated_roots": ["generated", "build-output"],
            "targets": targets,
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(transaction.join("marker.staging.123.456"), "partial\n").unwrap();

    let resumed = run(&["reset-governance", root_text, "--discard-legacy"]);
    assert!(
        resumed.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&resumed.stdout),
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert!(!transaction.exists());
    assert!(!root.join("build-output").exists());
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reset_governance_nested_project_preserves_parent_dirty_gate() {
    let workspace = temp_root("reset-governance-nested-project-dirty-parent");
    fs::create_dir_all(&workspace).unwrap();
    let root = workspace.join("v4");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    init_git(&workspace);
    let project_before = fs::read_to_string(root.join(".appsdk/project.json")).unwrap();
    fs::write(workspace.join("unrelated.txt"), "outside project\n").unwrap();

    let rejected = run(&["reset-governance", root_text, "--discard-legacy"]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("RESET_REQUIRES_CLEAN_WORKTREE"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/project.json")).unwrap(),
        project_before
    );
    assert_eq!(
        fs::read_to_string(workspace.join("unrelated.txt")).unwrap(),
        "outside project\n"
    );

    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn reset_governance_nested_project_does_not_self_dirty_clean_worktree() {
    let workspace = temp_root("reset-governance-nested-project-clean-worktree");
    fs::create_dir_all(&workspace).unwrap();
    let root = workspace.join("v4");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    init_git(&workspace);
    let project_before = fs::read_to_string(root.join(".appsdk/project.json")).unwrap();
    let lock_path = reset_transaction_lock_path(&root);
    fs::write(&lock_path, "").unwrap();
    let status = Command::new("git")
        .args(["-C", root_text, "status", "--porcelain=v1", "-z"])
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_eq!(status.stdout, b"?? .appsdk-reset-transaction-v4.lock\0");
    let scoped_status = Command::new("git")
        .args([
            "-C",
            root_text,
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            lock_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(scoped_status.status.success());
    assert_eq!(scoped_status.stdout, status.stdout);

    let reset = run(&["reset-governance", root_text, "--discard-legacy"]);
    assert!(
        reset.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&reset.stdout),
        String::from_utf8_lossy(&reset.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/project.json")).unwrap(),
        project_before
    );

    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn reset_governance_nested_project_rejects_untracked_symlink_to_lock() {
    let workspace = temp_root("reset-governance-nested-project-lock-symlink");
    fs::create_dir_all(&workspace).unwrap();
    let root = workspace.join("v4");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    init_git(&workspace);
    let lock_path = workspace.join(".appsdk-reset-transaction-v4.lock");
    fs::write(&lock_path, "").unwrap();
    std::os::unix::fs::symlink(&lock_path, root.join("user-link")).unwrap();
    let project_before = fs::read_to_string(root.join(".appsdk/project.json")).unwrap();

    let reset = run(&["reset-governance", root_text, "--discard-legacy"]);
    assert!(!reset.status.success());
    assert!(
        String::from_utf8_lossy(&reset.stderr).contains("RESET_REQUIRES_CLEAN_WORKTREE"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&reset.stdout),
        String::from_utf8_lossy(&reset.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/project.json")).unwrap(),
        project_before
    );
    assert!(root.join("user-link").is_symlink());

    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn init_fresh_starts_a_new_governance_epoch_without_legacy_witnesses() {
    let root = temp_root("init-fresh-governance-epoch");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["project_id"] = Value::String("preserved-project".into());
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let project_before = fs::read(&project_path).unwrap();
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    fs::write(root.join("protected/history/legacy.txt"), "retain\n").unwrap();
    fs::write(root.join("active/legacy.txt"), "retain-active\n").unwrap();
    fs::create_dir_all(root.join("generated/legacy-output")).unwrap();
    fs::write(root.join("generated/legacy-output/result"), "rebuild\n").unwrap();
    let (_, _) = install_previous_bundle_migration_record(&root);

    let worktree_contract = root.join("contracts/records/worktree-record.schema.json");
    let mut stale_contract: Value =
        serde_json::from_str(&fs::read_to_string(&worktree_contract).unwrap()).unwrap();
    stale_contract["properties"]
        .as_object_mut()
        .unwrap()
        .remove("bug_triage");
    fs::write(
        &worktree_contract,
        serde_json::to_string_pretty(&stale_contract).unwrap() + "\n",
    )
    .unwrap();
    let transition_contract = root.join("contracts/transitions/zone-transition-manifest.json");
    fs::write(&transition_contract, "{\"stale\":true}\n").unwrap();
    fs::write(
        root.join(".appsdk/records/reset-governance-record.json"),
        "{\"schema_version\":1,\"mode\":\"discard_legacy_control_plane\"}\n",
    )
    .unwrap();
    init_git(&root);

    let initialized = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    assert!(String::from_utf8_lossy(&initialized.stdout).contains("fresh"));
    assert!(String::from_utf8_lossy(&initialized.stdout).contains("next appsdk guide compile"));
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("protected/history/legacy.txt")).unwrap(),
        "retain\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("active/legacy.txt")).unwrap(),
        "retain-active\n"
    );
    assert_eq!(fs::read(&project_path).unwrap(), project_before);
    assert!(!root.join("generated/legacy-output").exists());
    assert!(!root
        .join(".appsdk/migrations/0.1.5-to-0.1.6/record.json")
        .exists());
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&worktree_contract).unwrap()).unwrap(),
        serde_json::from_str::<Value>(include_str!(
            "../../../contracts/records/worktree-record.schema.json"
        ))
        .unwrap()
    );
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&transition_contract).unwrap()).unwrap(),
        serde_json::from_str::<Value>(include_str!(
            "../../../contracts/transitions/zone-transition.manifest.json"
        ))
        .unwrap()
    );
    let reset: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/reset-governance-record.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(reset["mode"], "fresh_init");
    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );
    let result: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(result["command_ok"], true);
    assert_eq!(
        result["ok"], false,
        "ordinary verify must not claim delivery verification"
    );
    assert_eq!(result["delivery_assessed"], false);
    assert_eq!(result["development_ready"], true);
    assert_eq!(result["delivery_verified"], false);
    assert_eq!(result["delivery_assessed"], false);
    assert_eq!(result["baseline_status"], "required");
    assert_eq!(result["reason"], "baseline_required");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_does_not_initialize_collab_peer() {
    let root = temp_root("init-fresh-collab-side-effect");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);

    let fake_bin = temp_root("init-fresh-collab-fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        "#!/bin/sh\nprintf 'invoked\\n' >> \"$APPSDK_COLLAB_PROBE\"\nexit 73\n",
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();
    let probe = fake_bin.join("collab-init-probe.txt");
    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let search_path = std::env::join_paths(
        std::iter::once(fake_bin.clone()).chain(std::env::split_paths(&inherited_path)),
    )
    .unwrap();

    let initialized = Command::new(binary())
        .args(["init", root_text, "--fresh", "--discard-legacy"])
        .current_dir(&root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
        .env("PATH", search_path)
        .env("APPSDK_COLLAB_PROBE", &probe)
        .output()
        .unwrap();
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    assert!(
        !probe.exists(),
        "fresh governance reset must not invoke Collab initialization"
    );
    assert!(
        !root.join(".agent-collab").exists(),
        "fresh governance reset must not create project-local Collab state"
    );
    fs::remove_dir_all(fake_bin).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_resets_frozen_lifecycle_evidence_while_preserving_contract() {
    let root = temp_root("init-fresh-frozen-lifecycle-evidence");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["lifecycle"]["stage"] = Value::String("source_implemented".into());
    project["modules"][0]["stage"] = Value::String("frozen".into());
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
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let project_before = fs::read(&project_path).unwrap();
    init_git(&root);

    let admission_before = run(&["verify", "--admission", root_text]);
    assert!(
        !admission_before.status.success(),
        "review admission must not accept a frozen module without its record"
    );
    let verify_before = run(&["verify", root_text]);
    assert!(!verify_before.status.success());
    assert!(
        String::from_utf8_lossy(&verify_before.stderr)
            .contains("MISSING_RECORD:freeze-record-app-core.json"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&verify_before.stdout),
        String::from_utf8_lossy(&verify_before.stderr)
    );

    let old_record = root.join(".appsdk/records/review-record-app-core.json");
    fs::create_dir_all(old_record.parent().unwrap()).unwrap();
    fs::write(&old_record, "{\"old\":true}\n").unwrap();
    fs::create_dir_all(root.join("generated/modules/app-core")).unwrap();
    fs::write(
        root.join("generated/modules/app-core/module.compiled.json"),
        "old\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "legacy lifecycle witness"])
        .status()
        .unwrap()
        .success());

    let initialized = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    assert_eq!(fs::read(&project_path).unwrap(), project_before);
    assert!(!old_record.exists());
    assert!(!root
        .join(".appsdk/records/freeze-record-app-core.json")
        .exists());
    assert!(!root
        .join("generated/modules/app-core/module.compiled.json")
        .exists());

    let after: Value = serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    assert_eq!(
        after,
        serde_json::from_slice::<Value>(&project_before).unwrap(),
        "fresh init must preserve the existing project contract"
    );
    assert_eq!(after["modules"][0]["module_id"], "app-core");
    assert_eq!(after["modules"][0]["source_owner"], "app-core");
    assert_eq!(after["modules"][0]["stage"], "frozen");

    let admission_after = run(&["verify", "--admission", root_text]);
    assert!(
        !admission_after.status.success(),
        "reset must not turn missing frozen evidence into an admission pass"
    );
    assert!(
        String::from_utf8_lossy(&admission_after.stderr)
            .contains("MISSING_RECORD:freeze-record-app-core.json"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&admission_after.stdout),
        String::from_utf8_lossy(&admission_after.stderr)
    );
    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_migrates_supported_legacy_sdk_pin_and_preserves_project_contract() {
    let root = temp_root("init-fresh-legacy-sdk-pin");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["project_id"] = Value::String("legacy-sdk-project".into());
    project["sdk"]["version"] = Value::String("0.1.5".into());
    project["modules"][0]["module_id"] = Value::String("legacy-module".into());
    project["modules"][0]["source_owner"] = Value::String("legacy-module".into());
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    fs::write(root.join("business.txt"), "preserve\n").unwrap();
    fs::write(root.join("protected/history/legacy.txt"), "preserve\n").unwrap();
    fs::write(root.join("active/legacy.txt"), "preserve\n").unwrap();
    fs::create_dir_all(root.join("generated/legacy-output")).unwrap();
    fs::write(root.join("generated/legacy-output/result"), "remove\n").unwrap();
    init_git(&root);

    let initialized = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );

    let after: Value = serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    let mut expected = project;
    expected["sdk"]["version"] = Value::String("0.1.0009".into());
    assert_eq!(after, expected);
    let lock: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/sdk.lock")).unwrap()).unwrap();
    assert_eq!(lock["version"], "0.1.0009");
    let reset: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/reset-governance-record.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(reset["mode"], "fresh_init");
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "preserve\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("protected/history/legacy.txt")).unwrap(),
        "preserve\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("active/legacy.txt")).unwrap(),
        "preserve\n"
    );
    assert!(!root.join("generated/legacy-output").exists());
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_rebuilds_any_legacy_sdk_pin_without_legacy_migration_records() {
    for version in ["0.1.2", "0.1.3", "0.1.4", "0.1.5", "9.9.9"] {
        let root = temp_root(&format!("init-fresh-legacy-{}", version.replace('.', "-")));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());

        let project_path = root.join(".appsdk/project.json");
        let mut project: Value =
            serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
        let project_id = format!("legacy-{}-project", version.replace('.', "-"));
        project["project_id"] = Value::String(project_id.clone());
        project["sdk"]["version"] = Value::String(version.into());
        project["modules"][0]["module_id"] = Value::String("legacy-module".into());
        project["modules"][0]["source_owner"] = Value::String("legacy-module".into());
        fs::write(
            &project_path,
            serde_json::to_string_pretty(&project).unwrap() + "\n",
        )
        .unwrap();
        fs::write(root.join("business.txt"), "preserve\n").unwrap();
        fs::write(root.join("active/legacy.txt"), "preserve\n").unwrap();
        fs::write(root.join("protected/history/legacy.txt"), "preserve\n").unwrap();
        init_git(&root);

        let initialized = run(&["init", root_text, "--fresh", "--discard-legacy"]);
        assert!(
            initialized.status.success(),
            "version={version} stdout={} stderr={}",
            String::from_utf8_lossy(&initialized.stdout),
            String::from_utf8_lossy(&initialized.stderr)
        );
        let after: Value =
            serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
        assert_eq!(after["project_id"], project_id);
        assert_eq!(after["sdk"]["version"], "0.1.0009");
        assert_eq!(after["modules"][0]["module_id"], "legacy-module");
        assert_eq!(after["modules"][0]["source_owner"], "legacy-module");
        assert!(!root.join(".appsdk/migrations/0.1.5-to-0.1.6").exists());
        assert_eq!(
            fs::read_to_string(root.join("business.txt")).unwrap(),
            "preserve\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("protected/history/legacy.txt")).unwrap(),
            "preserve\n"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn project_creation_and_initialization_persist_host_registration() {
    let root = temp_root("global-registration-cli");
    let registry = temp_root("global-registration-home");
    let root_text = root.to_str().unwrap();
    let registry_text = registry.to_str().unwrap();

    let created = Command::new(binary())
        .args(["new", root_text])
        .env("APPSDK_HOME", registry_text)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&created.stdout),
        String::from_utf8_lossy(&created.stderr)
    );
    let first_receipt = String::from_utf8_lossy(&created.stdout)
        .lines()
        .find_map(|line| {
            line.strip_prefix("appsdk-registration ")
                .map(|value| serde_json::from_str::<Value>(value).unwrap())
        })
        .expect("new must emit a registration receipt");
    assert_eq!(
        first_receipt["registry_root"],
        registry.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(
        first_receipt["project_root"],
        root.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(first_receipt["sdk_version"], "0.1.0009");
    assert_eq!(first_receipt["idempotent"], false);

    let initialized = Command::new(binary())
        .args(["init", root_text])
        .env("APPSDK_HOME", registry_text)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    let second_receipt = String::from_utf8_lossy(&initialized.stdout)
        .lines()
        .find_map(|line| {
            line.strip_prefix("appsdk-registration ")
                .map(|value| serde_json::from_str::<Value>(value).unwrap())
        })
        .expect("init must emit a registration receipt");
    assert_eq!(second_receipt["idempotent"], true);

    init_git(&root);
    let fresh = Command::new(binary())
        .args(["init", root_text, "--fresh", "--discard-legacy"])
        .env("APPSDK_HOME", registry_text)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        fresh.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&fresh.stdout),
        String::from_utf8_lossy(&fresh.stderr)
    );
    let fresh_receipt = String::from_utf8_lossy(&fresh.stdout)
        .lines()
        .find_map(|line| {
            line.strip_prefix("appsdk-registration ")
                .map(|value| serde_json::from_str::<Value>(value).unwrap())
        })
        .expect("fresh init must emit a registration receipt");
    assert_eq!(fresh_receipt["idempotent"], true);

    let lines = fs::read_to_string(registry.join("projects.jsonl")).unwrap();
    assert_eq!(lines.lines().count(), 1);
    let event: Value = serde_json::from_str(lines.lines().next().unwrap()).unwrap();
    assert_eq!(event["event"], "project.registered");
    assert_eq!(event["project_root"], first_receipt["project_root"]);
    assert_eq!(event["project_id"], first_receipt["project_id"]);

    let unauthorized_root = temp_root("global-registration-internal-bypass");
    let unauthorized = Command::new(binary())
        .args([
            "new",
            unauthorized_root.to_str().unwrap(),
            "--internal-reset-staging",
        ])
        .env("APPSDK_HOME", registry_text)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!unauthorized.status.success());
    assert!(String::from_utf8_lossy(&unauthorized.stderr).contains("USAGE: appsdk new"));
    assert!(!unauthorized_root.exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(registry).unwrap();
}

#[test]
fn ordinary_init_rejects_linked_worktree_before_registration_or_mutation() {
    let main = temp_root("init-worktree-main");
    let worktree = temp_root("init-worktree-linked");
    let registry = temp_root("init-worktree-registry");
    let main_text = main.to_str().unwrap();
    let worktree_text = worktree.to_str().unwrap();
    let registry_text = registry.to_str().unwrap();

    assert!(run(&["new", main_text]).status.success());
    init_git(&main);
    let added = Command::new("git")
        .args([
            "-C",
            main_text,
            "worktree",
            "add",
            "--detach",
            worktree_text,
            "HEAD",
        ])
        .status()
        .unwrap();
    assert!(added.success());

    let project_before = fs::read(worktree.join(".appsdk/project.json")).unwrap();
    let lock_before = fs::read(worktree.join(".appsdk/sdk.lock")).unwrap();
    let rejected = Command::new(binary())
        .args(["init", worktree_text])
        .env("APPSDK_HOME", registry_text)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("INIT_REQUIRES_CANONICAL_PROJECT_MAIN_TREE"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(
        fs::read(worktree.join(".appsdk/project.json")).unwrap(),
        project_before
    );
    assert_eq!(
        fs::read(worktree.join(".appsdk/sdk.lock")).unwrap(),
        lock_before
    );
    assert!(!registry.join("projects.jsonl").exists());
    assert!(!worktree.join(".appsdk-control").exists());

    let _ = Command::new("git")
        .args([
            "-C",
            main_text,
            "worktree",
            "remove",
            "--force",
            worktree_text,
        ])
        .status();
    fs::remove_dir_all(main).unwrap();
    let _ = fs::remove_dir_all(registry);
}

#[test]
fn ordinary_init_rejects_relocated_target_inside_linked_worktree() {
    let main = temp_root("init-worktree-relocated-main");
    let worktree = temp_root("init-worktree-relocated-linked");
    let registry = temp_root("init-worktree-relocated-registry");
    let main_text = main.to_str().unwrap();
    let worktree_text = worktree.to_str().unwrap();
    let registry_text = registry.to_str().unwrap();

    assert!(run(&["new", main_text]).status.success());
    init_git(&main);
    let added = Command::new("git")
        .args([
            "-C",
            main_text,
            "worktree",
            "add",
            "--detach",
            worktree_text,
            "HEAD",
        ])
        .status()
        .unwrap();
    assert!(added.success());
    fs::remove_dir_all(worktree.join(".appsdk")).unwrap();
    fs::create_dir_all(worktree.join("relocated")).unwrap();
    confirm_preparation(&worktree, "relocated", "new_project");

    let rejected = Command::new(binary())
        .args(["init", worktree_text])
        .env("APPSDK_HOME", registry_text)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("INIT_REQUIRES_CANONICAL_PROJECT_MAIN_TREE"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(!worktree.join("relocated/.appsdk").exists());
    assert!(!registry.join("projects.jsonl").exists());

    let _ = Command::new("git")
        .args([
            "-C",
            main_text,
            "worktree",
            "remove",
            "--force",
            worktree_text,
        ])
        .status();
    fs::remove_dir_all(main).unwrap();
    let _ = fs::remove_dir_all(registry);
}
