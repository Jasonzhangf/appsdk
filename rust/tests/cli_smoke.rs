use serde_json::Value;
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_appsdk"))
}

fn memory_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_project-memory"))
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
    Command::new(binary())
        .args(args)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap()
}

fn run_in(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .args(args)
        .current_dir(root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap()
}

fn run_bug_in(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .args(args)
        .current_dir(root)
        .env("APPSDK_ROOT", root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap()
}

fn run_memory(root: &Path, args: &[&str], home: &Path) -> std::process::Output {
    Command::new(memory_binary())
        .args(args)
        .current_dir(root)
        .env("PROJECT_MEMORY_HOME", home)
        .output()
        .unwrap()
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
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    fs::write(root.join(".appsdk/legacy-record.json"), "legacy\n").unwrap();
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
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    assert!(root
        .join(".appsdk/records/reset-governance-record.json")
        .exists());
    assert!(!root.join(".appsdk/legacy-record.json").exists());
    assert!(!root.join(".appsdk-control/runtime").exists());
    assert!(!root.join("generated/old-artifact").exists());
    assert!(run(&["reset-governance", root_text, "--discard-legacy"])
        .status
        .success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_starts_a_new_governance_epoch_without_legacy_witnesses() {
    let root = temp_root("init-fresh-governance-epoch");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
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
    let transition_contract = root.join("contracts/transitions/zone-transition.manifest.json");
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
    assert!(!root.join("generated/legacy-output").exists());
    assert!(!root
        .join(".appsdk/migrations/0.1.5-to-0.1.6/record.json")
        .exists());
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&worktree_contract).unwrap()).unwrap(),
        serde_json::from_str::<Value>(include_str!(
            "../../contracts/records/worktree-record.schema.json"
        ))
        .unwrap()
    );
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&transition_contract).unwrap()).unwrap(),
        serde_json::from_str::<Value>(include_str!(
            "../../contracts/transitions/zone-transition.manifest.json"
        ))
        .unwrap()
    );
    let reset: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/reset-governance-record.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(reset["mode"], "fresh_init");
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_requires_explicit_legacy_discard_and_preserves_state_on_rejection() {
    let root = temp_root("init-fresh-confirmation");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    let (_, _) = install_previous_bundle_migration_record(&root);
    init_git(&root);
    let migration_record =
        fs::read_to_string(root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json")).unwrap();

    let rejected = run(&["init", root_text, "--fresh"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("INIT_FRESH_REQUIRES_DISCARD_LEGACY_CONFIRMATION"));
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json")).unwrap(),
        migration_record
    );
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_refuses_main_worktree_without_mutating_governance() {
    let root = temp_root("init-fresh-main-worktree");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    assert!(Command::new("git")
        .args(["-C", root_text, "branch", "-M", "main"])
        .status()
        .unwrap()
        .success());
    let project_before = fs::read_to_string(root.join(".appsdk/project.json")).unwrap();

    let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("RESET_REQUIRES_NON_MAIN_WORKTREE"));
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/project.json")).unwrap(),
        project_before
    );
    assert!(!root
        .join(".appsdk/records/reset-governance-record.json")
        .exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_refuses_dirty_worktree_without_mutating_governance() {
    let root = temp_root("init-fresh-dirty-worktree");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    let project_before = fs::read_to_string(root.join(".appsdk/project.json")).unwrap();
    fs::write(root.join("uncommitted.txt"), "must remain\n").unwrap();

    let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("RESET_REQUIRES_CLEAN_WORKTREE"));
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/project.json")).unwrap(),
        project_before
    );
    assert_eq!(
        fs::read_to_string(root.join("uncommitted.txt")).unwrap(),
        "must remain\n"
    );
    assert!(!root
        .join(".appsdk/records/reset-governance-record.json")
        .exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_requires_an_existing_governance_project() {
    let root = temp_root("init-fresh-missing-project");
    let root_text = root.to_str().unwrap();

    let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("INIT_FRESH_REQUIRES_EXISTING_PROJECT")
    );
    assert!(!root.exists());
}

#[test]
fn reset_governance_init_and_compile_do_not_require_pin_lock() {
    let root = temp_root("reset-governance-unbound-lock");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    assert!(run(&["reset-governance", root_text, "--discard-legacy"])
        .status
        .success());
    assert!(run(&["init", root_text]).status.success());

    let goal_path = root.join(".appsdk/goal.json");
    let mut goal: Value = serde_json::from_str(&fs::read_to_string(&goal_path).unwrap()).unwrap();
    goal["status"] = Value::String("confirmed".into());
    goal["confirmed_by"] = Value::String("test".into());
    goal["confirmed_at"] = Value::String("2026-01-01T00:00:00Z".into());
    fs::write(
        &goal_path,
        serde_json::to_string_pretty(&goal).unwrap() + "\n",
    )
    .unwrap();
    for stage in ["source_implemented", "contract_bound"] {
        assert!(run(&["promote", root_text, "--to", stage]).status.success());
    }
    let compiled = run(&["compile", root_text]);
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    assert!(root.join("generated/project.compiled.json").is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_upgrades_legacy_placeholder_lock_without_pin_lock() {
    let root = temp_root("init-upgrades-placeholder-lock");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/sdk.lock"),
        r#"{"sdk":"appsdk","version":"0.1.6","digest":"sha256:replace-with-compiled-sdk-digest","compiler_digest":"sha256:replace-with-compiler-digest","bundle_digest":"sha256:replace-with-sdk-bundle-digest","bundle_manifest_digest":"sha256:replace-with-bundle-manifest-digest","contract_schema":1}
"#,
    )
    .unwrap();

    let initialized = run(&["init", root_text]);
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let lock: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/sdk.lock")).unwrap()).unwrap();
    assert!(lock.get("digest").is_none());
    assert!(lock.get("compiler_digest").is_none());
    assert!(lock.get("binary_ref").is_none());
    assert_eq!(
        lock["bundle_resources"],
        serde_json::from_str::<Value>(include_str!("../../contracts/sdk-bundle.manifest.json"))
            .unwrap()["resources"]
    );
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_rejects_malformed_or_wrong_identity_sdk_lock() {
    for (name, lock) in [
        ("malformed-sdk-lock", "{not-json}\n"),
        (
            "wrong-identity-sdk-lock",
            r#"{"sdk":"other","version":"0.1.6","contract_schema":1}
"#,
        ),
    ] {
        let root = temp_root(name);
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        fs::write(root.join(".appsdk/sdk.lock"), lock).unwrap();
        let initialized = run(&["init", root_text]);
        assert!(!initialized.status.success());
        assert!(
            String::from_utf8_lossy(&initialized.stderr).contains("INVALID_SDK_LOCK"),
            "stderr={}",
            String::from_utf8_lossy(&initialized.stderr)
        );
        assert_eq!(
            fs::read_to_string(root.join(".appsdk/sdk.lock")).unwrap(),
            lock
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn reset_governance_refuses_main_worktree() {
    let root = temp_root("reset-main-worktree");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    assert!(Command::new("git")
        .args(["-C", root_text, "branch", "-M", "main"])
        .status()
        .unwrap()
        .success());
    let reset = run(&["reset-governance", root_text, "--discard-legacy"]);
    assert!(!reset.status.success());
    assert!(String::from_utf8_lossy(&reset.stderr).contains("RESET_REQUIRES_NON_MAIN_WORKTREE"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reset_governance_removes_declared_generated_root_without_touching_protected() {
    let root = temp_root("reset-declared-generated-root");
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
    fs::create_dir_all(root.join("build-output/old-delivery")).unwrap();
    fs::write(root.join("build-output/old-delivery/artifact.bin"), "old\n").unwrap();
    fs::create_dir_all(root.join("protected/history")).unwrap();
    fs::write(root.join("protected/history/keep.txt"), "keep\n").unwrap();
    init_git(&root);
    let reset = run(&["reset-governance", root_text, "--discard-legacy"]);
    assert!(
        reset.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&reset.stdout),
        String::from_utf8_lossy(&reset.stderr)
    );
    assert!(!root.join("build-output").exists());
    assert!(root.join("generated").is_dir());
    assert!(fs::read_dir(root.join("generated"))
        .unwrap()
        .next()
        .is_none());
    assert_eq!(
        fs::read_to_string(root.join("protected/history/keep.txt")).unwrap(),
        "keep\n"
    );
    fs::remove_dir_all(root).unwrap();
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
    assert!(String::from_utf8_lossy(&first.stdout).contains("collab peer bootstrap pending"));
    let notice = String::from_utf8_lossy(&first.stdout)
        .lines()
        .find_map(|line| {
            line.strip_prefix("collab-channel ")
                .map(|body| serde_json::from_str::<serde_json::Value>(body).unwrap())
        })
        .unwrap();
    assert_eq!(notice["notification_channel"], "none");
    assert_eq!(notice["subscription_created"], false);
    assert_eq!(notice["independent_work_allowed"], true);
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(gitignore.starts_with("# project rules\nnode_modules/\n"));
    assert_eq!(gitignore.matches("# BEGIN APPSDK MANAGED").count(), 1);
    assert!(gitignore.contains(".appsdk-control/"));
    assert!(gitignore.contains(".appsdk/sdk.bin"));
    assert!(gitignore.contains("/active/lib/"));
    assert!(gitignore.contains("/generated/"));
    let project_agents = fs::read_to_string(root.join("AGENTS.md")).unwrap();
    for section in [
        "## Project Truth",
        "## Semantic Invariants",
        "## Ownership",
        "## Architecture Truth",
        "## Development Process Control",
        "## Git Protection",
        "## Task Routing",
        "## Evidence Boundary",
    ] {
        assert!(project_agents.contains(section), "missing {section}");
    }
    for project_specific in ["RouteCodex", "rccv3", "Provider", "/Users/", "/Volumes/"] {
        assert!(
            !project_agents.contains(project_specific),
            "template leaked project-specific content: {project_specific}"
        );
    }
    for path in [
        ".appsdk/project.json",
        ".appsdk/goal.json",
        ".appsdk/sdk.lock",
        ".appsdk/sdk-resources.json",
        ".appsdk/docs/design/appsdk-project-integration.md",
        ".appsdk/docs/design/fix-lifecycle-v2.md",
        ".appsdk/rules/appsdk-project-governance.md",
        ".appsdk/skills/appsdk-project-governance/SKILL.md",
        ".appsdk/templates/minimal/AGENTS.md",
        ".appsdk/maps/resource-map.json",
        ".appsdk/maps/module-registry.json",
        ".appsdk/contracts/records/worktree-record.schema.json",
        ".appsdk/contracts/records/effectiveness-record.schema.json",
        ".appsdk/contracts/records/merge-record.schema.json",
        ".appsdk/contracts/guidance/tour-review.schema.json",
        ".appsdk/contracts/memory/memory-entry.schema.json",
        "playground/experiments",
        "active/lib",
        "protected/source",
        "protected/contracts",
        "protected/history",
        "generated",
        ".appsdk-control",
        "memory/index.md",
    ] {
        assert!(root.join(path).exists(), "missing {}", path);
    }
    let memory_index = fs::read_to_string(root.join("memory/index.md")).unwrap();
    for entrance in ["[Plan]", "[Path]", "[Knowledge]", "[Lesson]"] {
        assert!(
            memory_index.contains(entrance),
            "missing memory entrance {entrance}"
        );
    }

    let second = run(&["init", root_text]);
    assert!(second.status.success());
    assert_eq!(
        fs::read_to_string(root.join("AGENTS.md")).unwrap(),
        project_agents
    );
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
fn lifecycle_record_producer_is_bound_in_canonical_and_embedded_maps() {
    let root = temp_root("lifecycle-producer-map-binding");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let function_map: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/maps/function-map.json")).unwrap(),
    )
    .unwrap();
    let function = function_map["functions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["function_id"] == "lifecycle_record_producer")
        .unwrap();
    assert!(function["entry_symbols"]
        .as_array()
        .unwrap()
        .iter()
        .any(|symbol| symbol == "produce_lifecycle_records"));

    let resource_map: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/maps/resource-map.json")).unwrap(),
    )
    .unwrap();
    assert!(resource_map["resources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["resource_id"] == "lifecycle_record_producer_input"));

    let mainline_map: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/maps/mainline-call-map.json")).unwrap(),
    )
    .unwrap();
    assert!(mainline_map["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |entry| entry["chain_id"] == "lifecycle-record-production-v1"
                && entry["output_resource_id"] == "fix_worktree"
        ));
    assert!(mainline_map["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |entry| entry["chain_id"] == "lifecycle-record-production-v1"
                && entry["output_resource_id"] == "fix_evidence_set"
        ));
    assert!(mainline_map["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |entry| entry["chain_id"] == "lifecycle-record-production-v1"
                && entry["output_resource_id"] == "fix_reproduction"
        ));

    let registry: Value =
        serde_json::from_str(include_str!("../../contracts/maps/module-registry.json")).unwrap();
    assert_eq!(
        registry["modules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["module_id"] == "runtime-core")
            .unwrap()["symbol_owners"]["produce_lifecycle_records"],
        "appsdk::fix_lifecycle"
    );
    assert!(function_map["functions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["function_id"] == "lifecycle_chain_record_producer"));
    assert!(resource_map["resources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["resource_id"] == "lifecycle_chain_producer_input"));
    assert!(mainline_map["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["chain_id"] == "lifecycle-record-chain-production-v1"));
    assert!(serde_json::from_str::<Value>(
        &fs::read_to_string(root.join(".appsdk/maps/verification-map.json")).unwrap()
    )
    .unwrap()["gates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["gate_id"] == "lifecycle_chain_record_producer"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_producer_rejects_unknown_phase_without_mutating_records() {
    let root = temp_root("lifecycle-chain-invalid-phase");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let input = root.join("chain-input.json");
    fs::write(&input, "{}\n").unwrap();
    let rejected = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "unknown",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("PRODUCER_PHASE_INVALID"));
    assert!(!root
        .join(".appsdk/records/review-record-app-core.json")
        .exists());
    assert!(!root
        .join(".appsdk/records/effectiveness-record-app-core.json")
        .exists());
    assert!(!root
        .join(".appsdk/records/merge-record-app-core.json")
        .exists());
    assert!(!root
        .join(".appsdk/records/promotion-record-app-core.json")
        .exists());
    fs::remove_file(input).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_accepts_committed_candidate_records() {
    let root = temp_root("lifecycle-chain-record-commit");
    let root_text = root.to_str().unwrap();
    let _artifact_hash = prepare_lifecycle_chain_fixture(&root);
    let records = root.join(".appsdk/records");
    fs::remove_file(records.join("review-record-app-core.json")).unwrap();
    fs::remove_file(records.join("effectiveness-record-app-core.json")).unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", ".appsdk/records"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "lifecycle records"])
        .status()
        .unwrap()
        .success());
    let input = root.join("architecture-input.json");
    fs::write(
        &input,
        serde_json::to_string_pretty(&serde_json::json!({
            "architecture": {
                "reviewer": {"adapter":"test","identity":"chain-reviewer"},
                "verdict": "pass",
                "evidence_ids": ["candidate-evidence-1","positive-1","negative-1"]
            }
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let architecture = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "architecture",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(
        architecture.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&architecture.stdout),
        String::from_utf8_lossy(&architecture.stderr)
    );
    assert!(records.join("review-record-app-core.json").is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_accepts_candidate_descendant_of_observed_worktree_head() {
    let root = temp_root("lifecycle-chain-descendant-candidate");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let records = root.join(".appsdk/records");
    let observed_head = git_test_value(&root, &["rev-parse", "HEAD"]);
    fs::write(
        root.join("candidate-source-change.txt"),
        "candidate source\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "candidate-source-change.txt"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "candidate source"])
        .status()
        .unwrap()
        .success());
    let candidate_commit = git_test_value(&root, &["rev-parse", "HEAD"]);
    let candidate_tree = git_test_value(&root, &["rev-parse", "HEAD^{tree}"]);
    assert_ne!(observed_head, candidate_commit);
    assert!(Command::new("git")
        .args([
            "-C",
            root_text,
            "merge-base",
            "--is-ancestor",
            &observed_head,
            &candidate_commit
        ])
        .status()
        .unwrap()
        .success());

    let candidate_file = records.join("fix-candidate-record-app-core.json");
    let mut candidate: Value =
        serde_json::from_str(&fs::read_to_string(&candidate_file).unwrap()).unwrap();
    candidate["head_commit"] = Value::String(candidate_commit.clone());
    candidate["tree_hash"] = Value::String(candidate_tree.clone());
    fs::write(
        &candidate_file,
        serde_json::to_string_pretty(&candidate).unwrap() + "\n",
    )
    .unwrap();

    let validation_file = records.join("pre-review-validation-record-app-core.json");
    let mut validation: Value =
        serde_json::from_str(&fs::read_to_string(&validation_file).unwrap()).unwrap();
    validation["candidate_commit"] = Value::String(candidate_commit.clone());
    validation["candidate_tree_hash"] = Value::String(candidate_tree.clone());
    fs::write(
        &validation_file,
        serde_json::to_string_pretty(&validation).unwrap() + "\n",
    )
    .unwrap();

    let evidence_dir = records.join("evidence/app-core");
    for id in [
        "candidate-evidence-1",
        "positive-1",
        "negative-1",
        "whitebox-1",
        "install-1",
        "restart-1",
        "blackbox-1",
    ] {
        let file = evidence_dir.join(format!("{id}.json"));
        let mut evidence: Value =
            serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        evidence["source_commit"] = Value::String(candidate_commit.clone());
        fs::write(
            &file,
            serde_json::to_string_pretty(&evidence).unwrap() + "\n",
        )
        .unwrap();
    }
    let evidence_record_file = records.join("evidence-record-app-core.json");
    let mut evidence_record: Value =
        serde_json::from_str(&fs::read_to_string(&evidence_record_file).unwrap()).unwrap();
    evidence_record["source_commit"] = Value::String(candidate_commit.clone());
    fs::write(
        &evidence_record_file,
        serde_json::to_string_pretty(&evidence_record).unwrap() + "\n",
    )
    .unwrap();

    fs::remove_file(records.join("review-record-app-core.json")).unwrap();
    fs::remove_file(records.join("effectiveness-record-app-core.json")).unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", ".appsdk/records"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "candidate records"])
        .status()
        .unwrap()
        .success());

    let input = root.join("architecture-input.json");
    fs::write(
        &input,
        serde_json::to_string_pretty(&serde_json::json!({
            "architecture": {
                "reviewer": {"adapter":"test","identity":"chain-reviewer"},
                "verdict": "pass",
                "evidence_ids": ["candidate-evidence-1","positive-1","negative-1"]
            }
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let architecture = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "architecture",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(
        architecture.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&architecture.stdout),
        String::from_utf8_lossy(&architecture.stderr)
    );
    let worktree: Value = serde_json::from_str(
        &fs::read_to_string(records.join("worktree-record-app-core.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(worktree["head_commit"], observed_head);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_merge_rejects_effectiveness_mismatch_before_writing_record() {
    let root = temp_root("lifecycle-chain-merge-gate");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    fs::remove_file(root.join(".appsdk/records/merge-record-app-core.json")).unwrap();
    let effectiveness_file = root.join(".appsdk/records/effectiveness-record-app-core.json");
    let mut effectiveness: Value =
        serde_json::from_str(&fs::read_to_string(&effectiveness_file).unwrap()).unwrap();
    effectiveness["fix_candidate_id"] = Value::String("forged-candidate".into());
    fs::write(
        &effectiveness_file,
        serde_json::to_string_pretty(&effectiveness).unwrap() + "\n",
    )
    .unwrap();
    let input = root.join("merge-input.json");
    fs::write(&input, r#"{"merge":{"mainline_ref":"HEAD"}}"#).unwrap();
    let rejected = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "merge",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("POST_ARCHITECTURE_EFFECTIVENESS_MISMATCH"));
    assert!(!root
        .join(".appsdk/records/merge-record-app-core.json")
        .exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_promotion_rejects_merge_graph_mismatch_before_writing_record() {
    let root = temp_root("lifecycle-chain-promotion-gate");
    let root_text = root.to_str().unwrap();
    let artifact_hash = prepare_lifecycle_chain_fixture(&root);
    fs::remove_file(root.join(".appsdk/records/promotion-record-app-core.json")).unwrap();
    let merge_file = root.join(".appsdk/records/merge-record-app-core.json");
    let mut merge: Value = serde_json::from_str(&fs::read_to_string(&merge_file).unwrap()).unwrap();
    merge["effectiveness_id"] = Value::String("forged-effectiveness".into());
    fs::write(
        &merge_file,
        serde_json::to_string_pretty(&merge).unwrap() + "\n",
    )
    .unwrap();
    let input = root.join("promotion-input.json");
    let input_value = serde_json::json!({
        "promotion": {
            "experiment_id": "experiment-1",
            "new_active_version": "active-v2",
            "previous_active_version": null,
            "compatibility_level": "compatible",
            "evidence_ids": ["candidate-evidence-1"],
            "required_gate_results": [
                {"gate_id":"contract_valid","result":"pass","producer":"test"},
                {"gate_id":"sdk_lock_integrity","result":"pass","producer":"test"},
                {"gate_id":"remote_main_receipt","result":"pass","producer":"test"},
                {"gate_id":"lifecycle_chain_record_producer","result":"pass","producer":"test"},
                {"gate_id":"fix_lifecycle_graph","result":"pass","producer":"forged"},
                {"gate_id":"mainline_merge_identity","result":"pass","producer":"forged"}
            ],
            "change_set_id": "change-2",
            "root_cause": "root cause",
            "design_id": "design-1",
            "change_reason_comment": "reason",
            "playground_cleanup_record_id": "cleanup-1",
            "artifact_hash": artifact_hash
        }
    });
    fs::write(
        &input,
        serde_json::to_string_pretty(&input_value).unwrap() + "\n",
    )
    .unwrap();
    let rejected = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "promotion",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("MAINLINE_MERGE_RECORD_MISMATCH"));
    assert!(!root
        .join(".appsdk/records/promotion-record-app-core.json")
        .exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_promotion_writes_bound_record_for_project_module() {
    let root = temp_root("lifecycle-chain-promotion-success");
    let root_text = root.to_str().unwrap();
    let artifact_hash = prepare_lifecycle_chain_fixture(&root);
    fs::remove_file(root.join(".appsdk/records/promotion-record-app-core.json")).unwrap();
    let input = root.join("promotion-input.json");
    let input_value = serde_json::json!({
        "promotion": {
            "experiment_id": "experiment-1",
            "new_active_version": "active-v2",
            "previous_active_version": null,
            "compatibility_level": "compatible",
            "evidence_ids": ["candidate-evidence-1"],
            "required_gate_results": [
                {"gate_id":"contract_valid","result":"pass","producer":"test"},
                {"gate_id":"sdk_lock_integrity","result":"pass","producer":"test"},
                {"gate_id":"remote_main_receipt","result":"pass","producer":"test"},
                {"gate_id":"mainline_merge_identity","result":"pass","producer":"test"},
                {"gate_id":"fix_lifecycle_graph","result":"pass","producer":"test"},
                {"gate_id":"lifecycle_chain_record_producer","result":"pass","producer":"test"}
            ],
            "change_set_id": "change-2",
            "root_cause": "root cause",
            "design_id": "design-1",
            "change_reason_comment": "reason",
            "playground_cleanup_record_id": "cleanup-1",
            "artifact_hash": artifact_hash
        }
    });
    fs::write(
        &input,
        serde_json::to_string_pretty(&input_value).unwrap() + "\n",
    )
    .unwrap();
    let produced = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "promotion",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(
        produced.status.success(),
        "{}",
        String::from_utf8_lossy(&produced.stderr)
    );
    let promotion: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/promotion-record-app-core.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(promotion["module_id"], "app-core");
    assert_eq!(promotion["issue_id"], "issue-1");
    assert_eq!(promotion["fix_candidate_id"], "candidate-1");
    assert_eq!(promotion["merge_record_id"], "merge-1");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_accepts_committed_records_after_candidate() {
    let root = temp_root("lifecycle-chain-record-commit");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let records = root.join(".appsdk/records");
    fs::remove_file(records.join("review-record-app-core.json")).unwrap();
    fs::remove_file(records.join("effectiveness-record-app-core.json")).unwrap();
    let candidate_commit = git_test_value(&root, &["rev-parse", "HEAD"]);
    assert!(Command::new("git")
        .args(["-C", root_text, "add", ".appsdk/records"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "lifecycle records"])
        .status()
        .unwrap()
        .success());
    let architecture_input = root.join("architecture-input.json");
    fs::write(
        &architecture_input,
        serde_json::to_string_pretty(&serde_json::json!({
            "architecture": {
                "reviewer": {"adapter":"test","identity":"chain-reviewer"},
                "verdict": "pass",
                "evidence_ids": ["candidate-evidence-1","positive-1","negative-1"]
            }
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let architecture = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "architecture",
        "--input",
        architecture_input.to_str().unwrap(),
    ]);
    assert!(
        architecture.status.success(),
        "candidate={candidate_commit} stdout={} stderr={}",
        String::from_utf8_lossy(&architecture.stdout),
        String::from_utf8_lossy(&architecture.stderr)
    );
    assert!(Command::new("git")
        .args([
            "-C",
            root_text,
            "add",
            ".appsdk/records/review-record-app-core.json"
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "architecture record"])
        .status()
        .unwrap()
        .success());
    let effectiveness_input = root.join("effectiveness-input.json");
    fs::write(
        &effectiveness_input,
        serde_json::to_string_pretty(&serde_json::json!({
            "effectiveness": {
                "fixed_replay_evidence_id": "effective-1",
                "positive_evidence_ids": ["post-positive-1"],
                "negative_evidence_ids": ["post-negative-1"],
                "blackbox_evidence_ids": ["effective-1"]
            }
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let effectiveness = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "effectiveness",
        "--input",
        effectiveness_input.to_str().unwrap(),
    ]);
    assert!(
        effectiveness.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&effectiveness.stdout),
        String::from_utf8_lossy(&effectiveness.stderr)
    );
    assert!(records.join("effectiveness-record-app-core.json").is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_rejects_controlled_source_after_candidate() {
    let root = temp_root("lifecycle-chain-controlled-drift");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let records = root.join(".appsdk/records");
    fs::remove_file(records.join("review-record-app-core.json")).unwrap();
    fs::remove_file(records.join("effectiveness-record-app-core.json")).unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", ".appsdk/records"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "lifecycle records"])
        .status()
        .unwrap()
        .success());
    let drift = root.join("playground/experiments/committed-candidate-drift.txt");
    fs::write(&drift, "controlled drift\n").unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", drift.to_str().unwrap()])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "controlled source drift"])
        .status()
        .unwrap()
        .success());
    let input = root.join("architecture-input.json");
    fs::write(
        &input,
        serde_json::to_string_pretty(&serde_json::json!({
            "architecture": {
                "reviewer": {"adapter":"test","identity":"chain-reviewer"},
                "verdict": "pass",
                "evidence_ids": ["candidate-evidence-1","positive-1","negative-1"]
            }
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let rejected = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "architecture",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("CANDIDATE_CONTROLLED_SOURCE_DRIFT"));
    assert!(!records.join("review-record-app-core.json").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_rejects_candidate_tree_mismatch() {
    let root = temp_root("lifecycle-chain-tree-drift");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let records = root.join(".appsdk/records");
    fs::remove_file(records.join("review-record-app-core.json")).unwrap();
    let candidate_file = records.join("fix-candidate-record-app-core.json");
    let mut candidate: Value =
        serde_json::from_str(&fs::read_to_string(&candidate_file).unwrap()).unwrap();
    candidate["tree_hash"] = Value::String("sha256:forged-candidate-tree".into());
    fs::write(
        &candidate_file,
        serde_json::to_string_pretty(&candidate).unwrap() + "\n",
    )
    .unwrap();
    let input = root.join("architecture-input.json");
    fs::write(
        &input,
        serde_json::to_string_pretty(&serde_json::json!({
            "architecture": {
                "reviewer": {"adapter":"test","identity":"chain-reviewer"},
                "verdict": "pass",
                "evidence_ids": ["candidate-evidence-1","positive-1","negative-1"]
            }
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let rejected = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "architecture",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("FIX_CANDIDATE_TREE_MISMATCH"));
    assert!(!records.join("review-record-app-core.json").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_rejects_non_ancestor_candidate() {
    let root = temp_root("lifecycle-chain-non-ancestor");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let records = root.join(".appsdk/records");
    fs::remove_file(records.join("review-record-app-core.json")).unwrap();
    fs::remove_file(records.join("effectiveness-record-app-core.json")).unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", ".appsdk/records"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "lifecycle records"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "checkout", "-b", "candidate-side"])
        .status()
        .unwrap()
        .success());
    fs::write(records.join("candidate-side-record.json"), "{}\n").unwrap();
    assert!(Command::new("git")
        .args([
            "-C",
            root_text,
            "add",
            ".appsdk/records/candidate-side-record.json"
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "candidate side"])
        .status()
        .unwrap()
        .success());
    let side_commit = git_test_value(&root, &["rev-parse", "HEAD"]);
    let side_tree = git_test_value(&root, &["rev-parse", "HEAD^{tree}"]);
    assert!(Command::new("git")
        .args(["-C", root_text, "checkout", "codex/test"])
        .status()
        .unwrap()
        .success());
    let candidate_file = records.join("fix-candidate-record-app-core.json");
    let mut candidate: Value =
        serde_json::from_str(&fs::read_to_string(&candidate_file).unwrap()).unwrap();
    candidate["head_commit"] = Value::String(side_commit.clone());
    candidate["tree_hash"] = Value::String(side_tree);
    fs::write(
        &candidate_file,
        serde_json::to_string_pretty(&candidate).unwrap() + "\n",
    )
    .unwrap();
    let validation_file = records.join("pre-review-validation-record-app-core.json");
    let mut validation: Value =
        serde_json::from_str(&fs::read_to_string(&validation_file).unwrap()).unwrap();
    validation["candidate_commit"] = Value::String(side_commit.clone());
    validation["candidate_tree_hash"] = candidate["tree_hash"].clone();
    fs::write(
        &validation_file,
        serde_json::to_string_pretty(&validation).unwrap() + "\n",
    )
    .unwrap();
    let evidence_dir = records.join("evidence/app-core");
    for entry in fs::read_dir(&evidence_dir).unwrap() {
        let path = entry.unwrap().path();
        let mut evidence: Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        evidence["source_commit"] = Value::String(side_commit.clone());
        fs::write(
            &path,
            serde_json::to_string_pretty(&evidence).unwrap() + "\n",
        )
        .unwrap();
    }
    let input = root.join("architecture-input.json");
    fs::write(
        &input,
        serde_json::to_string_pretty(&serde_json::json!({
            "architecture": {
                "reviewer": {"adapter":"test","identity":"chain-reviewer"},
                "verdict": "pass",
                "evidence_ids": ["candidate-evidence-1","positive-1","negative-1"]
            }
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let rejected = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "architecture",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("LIFECYCLE_CHAIN_CANDIDATE_DRIFT"));
    assert!(!records.join("review-record-app-core.json").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_architecture_binds_project_bindings_to_review() {
    let root = temp_root("lifecycle-chain-project-bindings");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let records = root.join(".appsdk/records");
    let review_file = records.join("review-record-app-core.json");
    fs::remove_file(&review_file).unwrap();
    let input = root.join("architecture-input.json");
    let bindings = serde_json::json!({
        "v4_product_map_root": "docs/architecture/maps",
        "v4_product_map_hashes": {
            "resource_map_hash": "sha256:resource",
            "function_map_hash": "sha256:function",
            "mainline_call_map_hash": "sha256:mainline",
            "verification_map_hash": "sha256:verification"
        }
    });
    let write_input = |bindings: Option<Value>| {
        let mut architecture = serde_json::json!({
            "reviewer": {"adapter": "test", "identity": "chain-reviewer"},
            "verdict": "pass",
            "evidence_ids": ["candidate-evidence-1", "positive-1", "negative-1"]
        });
        if let Some(bindings) = bindings {
            architecture["project_bindings"] = bindings;
        }
        fs::write(
            &input,
            serde_json::to_string_pretty(&serde_json::json!({"architecture": architecture}))
                .unwrap()
                + "\n",
        )
        .unwrap();
    };
    let produce = || {
        run(&[
            "produce-lifecycle-chain",
            root_text,
            "--module",
            "app-core",
            "--phase",
            "architecture",
            "--input",
            input.to_str().unwrap(),
        ])
    };

    write_input(Some(bindings.clone()));
    let first = produce();
    assert!(
        first.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first_review: Value =
        serde_json::from_str(&fs::read_to_string(&review_file).unwrap()).unwrap();
    assert_eq!(first_review["project_bindings"], bindings);
    let first_review_id = first_review["review_id"].as_str().unwrap().to_string();

    fs::remove_file(&review_file).unwrap();
    write_input(Some(bindings));
    let second = produce();
    assert!(second.status.success());
    let second_review: Value =
        serde_json::from_str(&fs::read_to_string(&review_file).unwrap()).unwrap();
    assert_eq!(
        second_review["review_id"].as_str(),
        Some(first_review_id.as_str())
    );

    fs::remove_file(&review_file).unwrap();
    write_input(Some(serde_json::json!({
        "v4_product_map_root": "docs/architecture/maps",
        "v4_product_map_hashes": {
            "resource_map_hash": "sha256:resource",
            "function_map_hash": "sha256:function",
            "mainline_call_map_hash": "sha256:mainline",
            "verification_map_hash": "sha256:verification"
        },
        "client_provider_entrypoint": "stream"
    })));
    let changed = produce();
    assert!(changed.status.success());
    let changed_review: Value =
        serde_json::from_str(&fs::read_to_string(&review_file).unwrap()).unwrap();
    assert_ne!(changed_review["review_id"], first_review["review_id"]);
    assert_eq!(
        changed_review["project_bindings"]["client_provider_entrypoint"],
        "stream"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_architecture_rejects_non_object_project_bindings() {
    for (label, bindings) in [
        ("null", Value::Null),
        ("array", serde_json::json!(["routecodex", "provider"])),
        ("string", Value::String("routecodex".into())),
        ("boolean", Value::Bool(true)),
    ] {
        let root = temp_root(&format!("lifecycle-chain-project-bindings-{label}"));
        let root_text = root.to_str().unwrap();
        prepare_lifecycle_chain_fixture(&root);
        let records = root.join(".appsdk/records");
        let review_file = records.join("review-record-app-core.json");
        fs::remove_file(&review_file).unwrap();
        let input = root.join("architecture-input.json");
        fs::write(
            &input,
            serde_json::to_string_pretty(&serde_json::json!({
                "architecture": {
                    "reviewer": {"adapter": "test", "identity": "chain-reviewer"},
                    "verdict": "pass",
                    "evidence_ids": ["candidate-evidence-1", "positive-1", "negative-1"],
                    "project_bindings": bindings
                }
            }))
            .unwrap()
                + "\n",
        )
        .unwrap();
        let rejected = run(&[
            "produce-lifecycle-chain",
            root_text,
            "--module",
            "app-core",
            "--phase",
            "architecture",
            "--input",
            input.to_str().unwrap(),
        ]);
        assert!(!rejected.status.success(), "unexpected success for {label}");
        assert!(String::from_utf8_lossy(&rejected.stderr)
            .contains("ARCHITECTURE_REVIEW_PROJECT_BINDINGS_INVALID"));
        assert!(!review_file.exists());
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn lifecycle_chain_rejects_tampered_persisted_review_identity() {
    for (label, tamper, command, expected_error) in [
        (
            "project-bindings",
            "project_bindings",
            "effectiveness",
            "ARCHITECTURE_REVIEW_IDENTITY_MISMATCH",
        ),
        (
            "review-id",
            "review_id",
            "architecture_stable",
            "ARCHITECTURE_REVIEW_IDENTITY_MISMATCH",
        ),
    ] {
        let root = temp_root(&format!("lifecycle-chain-review-identity-{label}"));
        let root_text = root.to_str().unwrap();
        prepare_lifecycle_chain_fixture(&root);
        let records = root.join(".appsdk/records");
        let review_file = records.join("review-record-app-core.json");
        fs::remove_file(&review_file).unwrap();
        let input = root.join("architecture-input.json");
        fs::write(
            &input,
            serde_json::to_string_pretty(&serde_json::json!({
                "architecture": {
                    "reviewer": {"adapter": "test", "identity": "chain-reviewer"},
                    "verdict": "pass",
                    "evidence_ids": ["candidate-evidence-1", "positive-1", "negative-1"],
                    "project_bindings": {
                        "v4_product_map_root": "docs/architecture/maps",
                        "v4_product_map_hashes": {
                            "resource_map_hash": "sha256:resource",
                            "function_map_hash": "sha256:function",
                            "mainline_call_map_hash": "sha256:mainline",
                            "verification_map_hash": "sha256:verification"
                        }
                    }
                }
            }))
            .unwrap()
                + "\n",
        )
        .unwrap();
        let produced = run(&[
            "produce-lifecycle-chain",
            root_text,
            "--module",
            "app-core",
            "--phase",
            "architecture",
            "--input",
            input.to_str().unwrap(),
        ]);
        assert!(
            produced.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&produced.stdout),
            String::from_utf8_lossy(&produced.stderr)
        );
        let mut review: Value =
            serde_json::from_str(&fs::read_to_string(&review_file).unwrap()).unwrap();
        if tamper == "project_bindings" {
            review["project_bindings"]["v4_product_map_root"] =
                Value::String("docs/architecture/forged".into());
        } else {
            review["review_id"] = Value::String("review-forged".into());
        }
        fs::write(
            &review_file,
            serde_json::to_string_pretty(&review).unwrap() + "\n",
        )
        .unwrap();
        let rejected = if command == "effectiveness" {
            let effectiveness_input = root.join("effectiveness-input.json");
            fs::write(&effectiveness_input, r#"{"effectiveness":{}}"#).unwrap();
            run(&[
                "produce-lifecycle-chain",
                root_text,
                "--module",
                "app-core",
                "--phase",
                "effectiveness",
                "--input",
                effectiveness_input.to_str().unwrap(),
            ])
        } else {
            run(&[
                "promote-module",
                root_text,
                "--module",
                "app-core",
                "--to",
                command,
            ])
        };
        assert!(!rejected.status.success(), "unexpected success for {label}");
        assert!(String::from_utf8_lossy(&rejected.stderr).contains(expected_error));
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn lifecycle_chain_promotion_rejects_tampered_project_promotion_gate_map() {
    let root = temp_root("lifecycle-chain-promotion-map-tamper");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    fs::remove_file(root.join(".appsdk/records/promotion-record-app-core.json")).unwrap();
    let map_file = root.join(".appsdk/maps/verification-map.json");
    let mut map: Value = serde_json::from_str(&fs::read_to_string(&map_file).unwrap()).unwrap();
    let gate = map["gates"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|gate| gate["gate_id"] == "contract_valid")
        .unwrap();
    gate["required_for"] = serde_json::json!(["compile"]);
    fs::write(
        &map_file,
        serde_json::to_string_pretty(&map).unwrap() + "\n",
    )
    .unwrap();
    let input = root.join("promotion-input.json");
    fs::write(&input, "{}\n").unwrap();
    let rejected = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "promotion",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("LIFECYCLE_PRODUCER_MAP_TAMPERED:verification-map.json"));
    assert!(!root
        .join(".appsdk/records/promotion-record-app-core.json")
        .exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_record_producer_rejects_drifted_project_map_before_records() {
    let root = temp_root("lifecycle-record-producer-map-drift");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let map = root.join(".appsdk/maps/function-map.json");
    fs::write(&map, r#"{"schema_version":1,"functions":[]}"#).unwrap();
    let input = root.join("producer-input.json");
    fs::write(&input, "{}\n").unwrap();

    let rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("INVALID_GOVERNANCE_MAP:function-map.json"));
    assert!(!root
        .join(".appsdk/records/worktree-record-app-core.json")
        .exists());
    assert!(!root
        .join(".appsdk/records/reproduction-record-app-core.json")
        .exists());
    assert!(!root.join(".appsdk/records/evidence/app-core").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_record_producer_rejects_missing_invalid_and_shadowed_canonical_maps() {
    let cases = [
        ("missing", "function-map.json"),
        ("empty", "resource-map.json"),
        ("invalid", "verification-map.json"),
        ("shadowed", "mainline-call-map.json"),
    ];
    for (kind, map_name) in cases {
        let root = temp_root(&format!("lifecycle-record-producer-map-{kind}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        let map_path = root.join(".appsdk/maps").join(map_name);
        let expected_error = match kind {
            "missing" => {
                fs::remove_file(&map_path).unwrap();
                format!("MISSING_GOVERNANCE_MAP:{map_name}")
            }
            "empty" => {
                fs::write(&map_path, r#"{"schema_version":1,"resources":[]}"#).unwrap();
                format!("INVALID_GOVERNANCE_MAP:{map_name}")
            }
            "invalid" => {
                fs::write(&map_path, "{\n").unwrap();
                format!("INVALID_GOVERNANCE_MAP:{map_name}")
            }
            "shadowed" => {
                let mut map: Value =
                    serde_json::from_str(&fs::read_to_string(&map_path).unwrap()).unwrap();
                let entries = map["edges"].as_array_mut().unwrap();
                let canonical = entries
                    .iter()
                    .find(|entry| entry["chain_id"] == "lifecycle-record-production-v1")
                    .cloned()
                    .unwrap();
                let mut shadow = canonical;
                shadow["caller"] = Value::String("shadowed_producer".into());
                entries.push(shadow);
                fs::write(
                    &map_path,
                    serde_json::to_string_pretty(&map).unwrap() + "\n",
                )
                .unwrap();
                format!("LIFECYCLE_PRODUCER_MAP_TAMPERED:{map_name}")
            }
            _ => unreachable!(),
        };
        let input = root.join("producer-input.json");
        fs::write(&input, "{}\n").unwrap();
        let rejected = run(&[
            "produce-lifecycle-records",
            root_text,
            "--module",
            "app-core",
            "--input",
            input.to_str().unwrap(),
        ]);
        assert!(
            !rejected.status.success(),
            "map case {kind} unexpectedly passed"
        );
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains(&expected_error),
            "map case {kind}: expected {expected_error}, stderr={}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(!root
            .join(".appsdk/records/worktree-record-app-core.json")
            .exists());
        assert!(!root
            .join(".appsdk/records/reproduction-record-app-core.json")
            .exists());
        assert!(!root.join(".appsdk/records/evidence/app-core").exists());
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn lifecycle_record_producer_rejects_tampered_canonical_entry_in_each_map() {
    let cases = [
        (
            "resource-map.json",
            "resources",
            "resource_id",
            "lifecycle_record_producer_input",
            "owner",
        ),
        (
            "function-map.json",
            "functions",
            "function_id",
            "lifecycle_record_producer",
            "owner",
        ),
        (
            "mainline-call-map.json",
            "edges",
            "chain_id",
            "lifecycle-record-production-v1",
            "caller",
        ),
        (
            "verification-map.json",
            "gates",
            "gate_id",
            "worktree_clean",
            "command",
        ),
        (
            "function-map.json",
            "functions",
            "function_id",
            "lifecycle_chain_record_producer",
            "owner",
        ),
        (
            "mainline-call-map.json",
            "edges",
            "chain_id",
            "lifecycle-record-chain-production-v1",
            "caller",
        ),
        (
            "verification-map.json",
            "gates",
            "gate_id",
            "lifecycle_chain_record_producer",
            "command",
        ),
        (
            "resource-map.json",
            "resources",
            "resource_id",
            "lifecycle_chain_producer_input",
            "owner",
        ),
    ];
    for (map_name, key, id_key, id, field) in cases {
        let root = temp_root(&format!("lifecycle-record-producer-map-tampered-{id_key}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        let map_path = root.join(".appsdk/maps").join(map_name);
        let mut map: Value = serde_json::from_str(&fs::read_to_string(&map_path).unwrap()).unwrap();
        let entry = map[key]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|entry| entry[id_key] == id)
            .unwrap();
        entry[field] = Value::String("tampered_producer_contract".into());
        fs::write(
            &map_path,
            serde_json::to_string_pretty(&map).unwrap() + "\n",
        )
        .unwrap();
        let input = root.join("producer-input.json");
        fs::write(&input, "{}\n").unwrap();
        let rejected = run(&[
            "produce-lifecycle-records",
            root_text,
            "--module",
            "app-core",
            "--input",
            input.to_str().unwrap(),
        ]);
        assert!(
            !rejected.status.success(),
            "map {map_name} unexpectedly passed"
        );
        assert!(
            String::from_utf8_lossy(&rejected.stderr)
                .contains(&format!("LIFECYCLE_PRODUCER_MAP_TAMPERED:{map_name}")),
            "map {map_name}: stderr={}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(!root
            .join(".appsdk/records/worktree-record-app-core.json")
            .exists());
        assert!(!root
            .join(".appsdk/records/reproduction-record-app-core.json")
            .exists());
        assert!(!root.join(".appsdk/records/evidence/app-core").exists());
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn lifecycle_record_producer_recovers_partial_group_commit() {
    let root = temp_root("lifecycle-record-producer-recovery");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/goal.json"),
        r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#,
    )
    .unwrap();
    init_git(&root);
    assert!(run(&["promote", root_text, "--to", "source_implemented"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    assert!(run(&["compile-module", root_text, "--module", "app-core"])
        .status
        .success());
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
    assert!(Command::new("git")
        .args(["-C", root_text, "add", ".appsdk/project.json"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "candidate"])
        .status()
        .unwrap()
        .success());
    let commit = git_test_value(&root, &["rev-parse", "HEAD"]);
    let artifact: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json")).unwrap(),
    )
    .unwrap();
    let scope_hash = digest(&canonical(&serde_json::json!({
        "module_id":"app-core",
        "source_hash":artifact["source_hash"],
        "contract_hash":artifact["contract_hash"]
    })));
    let command = serde_json::json!({
        "program":"sh",
        "args":["-c","printf baseline-error-token >&2; exit 1"],
        "working_directory":".",
        "expected_exit_status":1,
        "expected_error_token":"baseline-error-token"
    });
    let input_hashes = vec![digest(&canonical(&command))];
    let current_root = root.canonicalize().unwrap();
    let worktree_id = format!(
        "worktree-{}",
        digest(&canonical(&serde_json::json!({
            "root": current_root,
            "module_id":"app-core",
            "issue_id":"none",
            "base_commit":commit,
            "head_commit":commit,
            "branch":"codex/test",
            "scope_hash":scope_hash
        })))
        .strip_prefix("sha256:")
        .unwrap()
    );
    let reproduction_id = format!(
        "reproduction-{}",
        digest(&canonical(&serde_json::json!({
            "worktree_id":worktree_id,
            "input_hashes":input_hashes,
            "error_token":"baseline-error-token"
        })))
        .strip_prefix("sha256:")
        .unwrap()
    );
    let baseline_id = format!(
        "baseline-{}",
        digest(&canonical(&serde_json::json!({
            "reproduction_id":reproduction_id,
            "source_commit":commit,
            "input_hashes":input_hashes,
            "command":command
        })))
        .strip_prefix("sha256:")
        .unwrap()
    );
    let input = serde_json::json!({
        "goal_id":"goal-1",
        "worktree": {
            "worktree_id":worktree_id,"issue_id":"none","module_id":"app-core",
            "base_ref":"HEAD","base_commit":commit,"branch":"codex/test","head_commit":commit,
            "initial_clean":true,"final_clean":true,"isolation_mode":"isolated_worktree",
            "scope_hash":scope_hash,"created_at":"2026-01-01T00:00:00Z"
        },
        "reproduction": {
            "reproduction_id":reproduction_id,"issue_id":"none","module_id":"app-core",
            "worktree_id":worktree_id,"base_commit":commit,"input_hashes":input_hashes,
            "baseline_evidence_id":baseline_id,"first_divergence":"baseline","result":"reproduced",
            "created_at":"2026-01-01T00:00:00Z"
        },
        "baseline_evidence": {
            "evidence_id":baseline_id,"issue_id":"none","experiment_id":"experiment",
            "phase":"baseline_reproduction","kind":"red_test","source_commit":commit,
            "scope":{"module_id":"app-core"},"producer":{"adapter":"appsdk","identity":"appsdk-lifecycle-record-producer"},
            "result":"pass","created_at":"2026-01-01T00:00:00Z","expires_at":"2099-01-01T00:00:00Z",
            "input_hashes":input_hashes,"scope_hash":scope_hash,"command":command,"exit_status":1,
            "output_hash":digest("stdout=\nstderr=baseline-error-token")
        }
    });
    let input_path = root.with_extension("producer-input.json");
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&input).unwrap() + "\n",
    )
    .unwrap();
    let input_hash = digest(&canonical(&input));
    let records = [
        (
            ".appsdk/records/worktree-record-app-core.json".to_string(),
            input["worktree"].clone(),
        ),
        (
            ".appsdk/records/reproduction-record-app-core.json".to_string(),
            input["reproduction"].clone(),
        ),
        (
            format!(".appsdk/records/evidence/app-core/{}.json", baseline_id),
            input["baseline_evidence"].clone(),
        ),
    ];
    let transaction = root.join(".appsdk/transactions/producer-app-core");
    fs::create_dir_all(&transaction).unwrap();
    let mut marker_records = Vec::new();
    for (index, (target, record)) in records.iter().enumerate() {
        let staged = transaction.join(format!("record-{}.json", index));
        let bytes = serde_json::to_string_pretty(record).unwrap() + "\n";
        fs::write(&staged, &bytes).unwrap();
        marker_records.push(serde_json::json!({
            "target": target,
            "staging": format!("record-{}.json", index),
            "digest": digest(&bytes)
        }));
        if index == 0 {
            fs::create_dir_all(root.join(".appsdk/records")).unwrap();
            fs::hard_link(&staged, root.join(target)).unwrap();
        }
    }
    let marker = serde_json::json!({
        "schema_version":1,"module_id":"app-core","input_hash":input_hash,"phase":"commit","records":marker_records
    });
    let marker_path = transaction.join("marker.json");
    fs::write(
        &marker_path,
        serde_json::to_string_pretty(&marker).unwrap() + "\n",
    )
    .unwrap();
    let mut tampered_marker = marker.clone();
    tampered_marker["records"][0]["target"] =
        Value::String(".appsdk/records/reproduction-record-app-core.json".into());
    fs::write(
        &marker_path,
        serde_json::to_string_pretty(&tampered_marker).unwrap() + "\n",
    )
    .unwrap();
    let rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("PRODUCER_TRANSACTION_TARGET_INVALID")
    );
    fs::write(
        &marker_path,
        serde_json::to_string_pretty(&marker).unwrap() + "\n",
    )
    .unwrap();
    let rewrite_staged = |index: usize, record: &Value| {
        let bytes = serde_json::to_string_pretty(record).unwrap() + "\n";
        fs::write(transaction.join(format!("record-{}.json", index)), &bytes).unwrap();
        let mut current_marker: Value =
            serde_json::from_str(&fs::read_to_string(&marker_path).unwrap()).unwrap();
        current_marker["records"][index]["digest"] = Value::String(digest(&bytes));
        fs::write(
            &marker_path,
            serde_json::to_string_pretty(&current_marker).unwrap() + "\n",
        )
        .unwrap();
    };
    let mut wrong_triage = records[0].1.clone();
    wrong_triage["bug_triage"] = serde_json::json!({
        "query_executed":true,
        "query":"forged",
        "mode":"new_confirmed",
        "reopened_from_issue_id":null
    });
    rewrite_staged(0, &wrong_triage);
    let triage_rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(!triage_rejected.status.success());
    assert!(String::from_utf8_lossy(&triage_rejected.stderr)
        .contains("PRODUCER_RECOVERY_WORKTREE_BINDING_MISMATCH"));
    assert!(transaction.is_dir());
    assert!(!root
        .join(".appsdk/records/reproduction-record-app-core.json")
        .exists());
    rewrite_staged(0, &records[0].1);

    let mut wrong_reproduction = records[1].1.clone();
    wrong_reproduction["issue_id"] = Value::String("forged-issue".into());
    rewrite_staged(1, &wrong_reproduction);
    let record_rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(!record_rejected.status.success());
    assert!(
        String::from_utf8_lossy(&record_rejected.stderr).contains("PRODUCER_RECORD_SCHEMA_INVALID")
    );
    assert!(transaction.is_dir());
    rewrite_staged(1, &records[1].1);

    let mut wrong_baseline = records[2].1.clone();
    wrong_baseline["scope_hash"] = Value::String("forged-scope".into());
    rewrite_staged(2, &wrong_baseline);
    let baseline_rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(!baseline_rejected.status.success());
    assert!(String::from_utf8_lossy(&baseline_rejected.stderr)
        .contains("PRODUCER_RECOVERY_BASELINE_BINDING_MISMATCH"));
    assert!(transaction.is_dir());
    rewrite_staged(2, &records[2].1);

    assert!(Command::new("git")
        .args(["-C", root_text, "branch", "-m", "codex/drifted"])
        .status()
        .unwrap()
        .success());
    let identity_rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(!identity_rejected.status.success());
    assert!(String::from_utf8_lossy(&identity_rejected.stderr).contains("PRODUCER_BRANCH_MISMATCH"));
    assert!(transaction.is_dir());
    assert!(!root
        .join(".appsdk/records/reproduction-record-app-core.json")
        .exists());
    assert!(!root.join(".appsdk/records/evidence/app-core").exists());
    assert!(Command::new("git")
        .args(["-C", root_text, "branch", "-m", "codex/test"])
        .status()
        .unwrap()
        .success());
    let recovered = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(
        recovered.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&recovered.stdout),
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(!transaction.exists());
    for (target, _) in records {
        assert!(root.join(&target).is_file(), "missing {target}");
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bundled_goal_clarification_contract_supports_verify_and_admission() {
    let root = temp_root("bundled-goal-clarification-contract");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let contract = "contracts/records/goal-clarification-record.schema.json";
    let project: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/project.json")).unwrap())
            .unwrap();
    assert!(project["governance"]["record_contracts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value.as_str() == Some(contract)));
    assert!(root.join(contract).is_file());
    assert!(root.join(".appsdk").join(contract).is_file());

    for args in [
        &["verify", root_text][..],
        &["verify", "--admission", root_text][..],
    ] {
        let verified = run(args);
        assert!(
            verified.status.success(),
            "args={args:?} stderr={}",
            String::from_utf8_lossy(&verified.stderr)
        );
    }

    fs::remove_file(root.join(contract)).unwrap();
    for args in [
        &["verify", root_text][..],
        &["verify", "--admission", root_text][..],
    ] {
        let rejected = run(args);
        assert!(!rejected.status.success(), "args={args:?}");
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains("DECLARED_RECORD_CONTRACT_MISSING"),
            "args={args:?} stderr={}",
            String::from_utf8_lossy(&rejected.stderr)
        );
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_automatically_attempts_collab_without_blocking_independent_work() {
    let root = temp_root("init-collab-peer");
    fs::create_dir_all(&root).unwrap();
    confirm_preparation(&root, ".", "project_refactor");

    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
if [ "$1" != "init" ]; then
  exit 64
fi
{
  printf 'cwd=%s\n' "$PWD"
  printf 'pane=%s\n' "$TMUX_PANE"
  printf 'probe=%s\n' "$APPSDK_COLLAB_ENV_PROBE"
  printf 'args=%s\n' "$*"
} >> "$APPSDK_COLLAB_PROBE"
exit 73
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let search_path = std::env::join_paths(
        std::iter::once(fake_bin.clone()).chain(std::env::split_paths(&inherited_path)),
    )
    .unwrap();
    let probe = root.join("collab-init-probe.txt");
    let output = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("PATH", search_path)
        .env("TMUX_PANE", "%42")
        .env("APPSDK_COLLAB_ENV_PROBE", "same-environment")
        .env("APPSDK_COLLAB_PROBE", &probe)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        probe.exists(),
        "AppSDK should automatically initialize Collab"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("COLLAB_INIT_FAILED"));

    let repeated = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env(
            "PATH",
            std::env::join_paths(
                std::iter::once(fake_bin.clone()).chain(std::env::split_paths(&inherited_path)),
            )
            .unwrap(),
        )
        .env("TMUX_PANE", "%42")
        .env("APPSDK_COLLAB_ENV_PROBE", "same-environment")
        .env("APPSDK_COLLAB_PROBE", &probe)
        .output()
        .unwrap();
    assert!(
        repeated.status.success(),
        "{}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    let invocation = fs::read_to_string(&probe).unwrap();
    assert_eq!(invocation.matches("args=init\n").count(), 2);
    assert!(invocation.contains("pane=%42\n"));
    assert!(invocation.contains("probe=same-environment\n"));
    let child_cwd = invocation
        .lines()
        .find_map(|line| line.strip_prefix("cwd="))
        .unwrap();
    assert_eq!(
        fs::canonicalize(child_cwd).unwrap(),
        fs::canonicalize(&root).unwrap()
    );
    fs::write(
        &fake_collab,
        "#!/bin/sh\nprintf '{\"ok\":true,\"worker_id\":\"test-peer\"}\\n'\n",
    )
    .unwrap();
    let ready = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("TMUX_PANE", "%42")
        .output()
        .unwrap();
    assert!(ready.status.success());
    assert!(String::from_utf8_lossy(&ready.stdout).contains("test-peer"));
    fs::remove_file(&fake_collab).unwrap();
    let unavailable = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("TMUX_PANE", "%42")
        .output()
        .unwrap();
    assert!(unavailable.status.success());
    assert!(String::from_utf8_lossy(&unavailable.stderr).contains("COLLAB_INIT_UNAVAILABLE"));
    assert!(run(&["verify", root.to_str().unwrap()]).status.success());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn subagent_entry_forwards_without_governance_or_second_registry() {
    let root = temp_root("subagent-forward");
    fs::create_dir_all(&root).unwrap();
    let fake = root.join("collab");
    fs::write(&fake, "#!/bin/sh\nprintf '%s\\n' \"$@\"\nexit 23\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(binary())
        .args([
            "subagent",
            "send",
            "child",
            "--subject",
            "topic",
            "original body with spaces",
        ])
        .current_dir(&root)
        .env("PATH", &root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(23));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "subagent\nsend\nchild\n--subject\ntopic\noriginal body with spaces\n"
    );
    assert!(!root.join(".appsdk").exists());
    assert!(!root.join(".agent-collab").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn collab_entry_forwards_without_governance_or_second_registry() {
    let root = temp_root("collab-forward");
    fs::create_dir_all(&root).unwrap();
    let fake = root.join("collab");
    fs::write(&fake, "#!/bin/sh\nprintf '%s\\n' \"$@\"\nexit 42\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(binary())
        .args(["collab", "status", "--all"])
        .current_dir(&root)
        .env("PATH", &root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(42));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "status\n--all\n");
    assert!(!root.join(".appsdk").exists());
    assert!(!root.join(".agent-collab").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_agent_contract_is_created_for_new_projects_but_never_overwrites_project_rules() {
    let created = temp_root("project-agent-contract-new");
    let created_text = created.to_str().unwrap();
    assert!(run(&["new", created_text]).status.success());
    let template = fs::read_to_string(created.join("AGENTS.md")).unwrap();
    assert!(template.contains("## Development Process Control"));

    fs::write(created.join("AGENTS.md"), "# Existing project rules\n").unwrap();
    assert!(run(&["init", created_text]).status.success());
    assert_eq!(
        fs::read_to_string(created.join("AGENTS.md")).unwrap(),
        "# Existing project rules\n"
    );

    fs::remove_file(created.join("AGENTS.md")).unwrap();
    assert!(run(&["init", created_text]).status.success());
    assert!(!created.join("AGENTS.md").exists());
    fs::remove_dir_all(created).unwrap();
}

#[test]
fn repeated_init_projects_standard_template_and_bootstrap_upgrade_proposal() {
    let root = temp_root("guidance-template-upgrade");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());

    let project_file = root.join(".appsdk/project.json");
    let project_before = fs::read(&project_file).unwrap();
    fs::write(root.join("AGENTS.md"), "# Project-owned rules\n").unwrap();
    let agents_before = fs::read(root.join("AGENTS.md")).unwrap();
    fs::create_dir_all(root.join("skills/project-flow")).unwrap();
    fs::write(
        root.join("skills/project-flow/SKILL.md"),
        "---\nname: project-flow\ndescription: Project-owned flow.\n---\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".appsdk/guidance")).unwrap();
    fs::write(
        root.join(".appsdk/guidance/project-guidance.json"),
        "{\"schema_version\":1}\n",
    )
    .unwrap();
    fs::write(root.join(".appsdk/records/project-record.json"), "{}\n").unwrap();
    fs::write(root.join("active/lib/project-active.txt"), "active\n").unwrap();
    fs::write(
        root.join("protected/history/project-history.txt"),
        "history\n",
    )
    .unwrap();
    let project_skill_before = fs::read(root.join("skills/project-flow/SKILL.md")).unwrap();
    let machine_guidance_before =
        fs::read(root.join(".appsdk/guidance/project-guidance.json")).unwrap();
    let lifecycle_record_before =
        fs::read(root.join(".appsdk/records/project-record.json")).unwrap();
    let active_before = fs::read(root.join("active/lib/project-active.txt")).unwrap();
    let protected_before = fs::read(root.join("protected/history/project-history.txt")).unwrap();
    let reference = root.join(".appsdk/templates/minimal/AGENTS.md");
    fs::write(&reference, "stale template reference\n").unwrap();
    let governance_map = root.join(".appsdk/maps/function-map.json");
    let governance_map_before = fs::read(&governance_map).unwrap();
    let sdk_resources = root.join(".appsdk/sdk-resources.json");
    let sdk_resources_before = fs::read(&sdk_resources).unwrap();

    let initialized = run(&["init", root_text]);
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let initialized_stdout = String::from_utf8_lossy(&initialized.stdout);
    assert!(initialized_stdout.contains("--task guidance-upgrade"));
    assert!(initialized_stdout.contains("--mode bootstrap"));
    assert_eq!(fs::read(&project_file).unwrap(), project_before);
    assert_eq!(fs::read(&governance_map).unwrap(), governance_map_before);
    assert_eq!(fs::read(&sdk_resources).unwrap(), sdk_resources_before);
    assert_eq!(fs::read(root.join("AGENTS.md")).unwrap(), agents_before);
    assert_eq!(
        fs::read(root.join("skills/project-flow/SKILL.md")).unwrap(),
        project_skill_before
    );
    assert_eq!(
        fs::read(root.join(".appsdk/guidance/project-guidance.json")).unwrap(),
        machine_guidance_before
    );
    assert_eq!(
        fs::read(root.join(".appsdk/records/project-record.json")).unwrap(),
        lifecycle_record_before
    );
    assert_eq!(
        fs::read(root.join("active/lib/project-active.txt")).unwrap(),
        active_before
    );
    assert_eq!(
        fs::read(root.join("protected/history/project-history.txt")).unwrap(),
        protected_before
    );
    assert!(fs::read_to_string(&reference)
        .unwrap()
        .contains("## Development Process Control"));

    let intake = run(&[
        "guide",
        "init",
        root_text,
        "--task",
        "guidance-upgrade",
        "--mode",
        "bootstrap",
        "--module",
        "app-core",
    ]);
    assert!(
        intake.status.success(),
        "{}",
        String::from_utf8_lossy(&intake.stderr)
    );
    let intake_json: Value = serde_json::from_slice(&intake.stdout).unwrap();
    assert_eq!(intake_json["setup_kind"], "template_upgrade_review");
    assert_eq!(
        intake_json["reason_code"],
        "GUIDANCE_TEMPLATE_UPGRADE_PROPOSAL_REQUIRED"
    );
    assert_eq!(intake_json["readiness"], "needs_user_approval");
    assert_eq!(intake_json["writes_state"], false);
    assert_eq!(
        intake_json["standard_template"]["path"],
        ".appsdk/templates/minimal/AGENTS.md"
    );
    assert_eq!(intake_json["standard_template"]["version"], "0.1.6");
    assert_eq!(
        intake_json["standard_template"]["digest"],
        file_digest(&reference)
    );
    let reference_source = intake_json["read_first"]
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["source_id"] == "appsdk-standard-project-agent-template")
        .unwrap();
    assert_eq!(reference_source["kind"], "template");
    assert_eq!(reference_source["disposition"], "standard_reference");
    assert_eq!(reference_source["required"], false);
    assert_eq!(reference_source["enforcement"], "advisory");
    assert_eq!(
        intake_json["read_first"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["source_id"],
        "appsdk-standard-project-agent-template"
    );
    assert_eq!(
        intake_json["proposal_schema"]["setup_kind"],
        "template_upgrade_review"
    );
    assert_eq!(
        intake_json["proposal_schema"]["standard_template"]["digest"],
        file_digest(&reference)
    );
    assert_eq!(
        intake_json["proposal_schema"]["recommended_changes"],
        serde_json::json!([])
    );
    assert_eq!(
        intake_json["proposal_schema"]["retained_project_rules"],
        serde_json::json!([])
    );
    assert_eq!(
        intake_json["proposal_schema"]["declined_template_items"],
        serde_json::json!([])
    );
    assert_eq!(intake_json["proposal_schema"]["approval_required"], true);
    assert!(!intake_json["read_first"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| {
            source["source_id"] == "appsdk-standard-project-agent-template"
                && source["disposition"] == "declared"
        }));
    let project_after: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    assert!(!project_after["guidance"]["rule_sources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source["source_id"] == "appsdk-standard-project-agent-template"));
    assert!(!root
        .join(".appsdk-control/guidance/guidance-upgrade")
        .exists());
    assert_eq!(fs::read(&project_file).unwrap(), project_before);
    assert_eq!(fs::read(root.join("AGENTS.md")).unwrap(), agents_before);
    assert_eq!(
        fs::read(root.join("skills/project-flow/SKILL.md")).unwrap(),
        project_skill_before
    );
    assert_eq!(
        fs::read(root.join(".appsdk/guidance/project-guidance.json")).unwrap(),
        machine_guidance_before
    );
    assert_eq!(
        fs::read(root.join(".appsdk/records/project-record.json")).unwrap(),
        lifecycle_record_before
    );
    assert_eq!(
        fs::read(root.join("active/lib/project-active.txt")).unwrap(),
        active_before
    );
    assert_eq!(
        fs::read(root.join("protected/history/project-history.txt")).unwrap(),
        protected_before
    );

    fs::remove_file(&reference).unwrap();
    let verified_without_reference = run(&["verify", root_text]);
    assert!(
        verified_without_reference.status.success(),
        "{}",
        String::from_utf8_lossy(&verified_without_reference.stderr)
    );

    let outside = temp_root("guidance-template-upgrade-outside");
    fs::create_dir_all(&outside).unwrap();
    let outside_file = outside.join("AGENTS.md");
    fs::write(&outside_file, "outside\n").unwrap();
    symlink(&outside_file, &reference).unwrap();
    let symlinked_init = run(&["init", root_text]);
    assert!(!symlinked_init.status.success());
    assert!(String::from_utf8_lossy(&symlinked_init.stderr)
        .contains("GOVERNANCE_PATH_SYMLINK:guidance_standard_template"));
    assert_eq!(fs::read_to_string(&outside_file).unwrap(), "outside\n");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(outside).unwrap();
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
fn collaboration_is_optional_and_does_not_require_a_merge_queue() {
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
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    project["development_scenarios"]["enabled"] = serde_json::json!(["multi_worktree_merge_queue"]);
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap(),
    )
    .unwrap();
    let missing_ownership = run(&["verify", root_text]);
    assert!(!missing_ownership.status.success());
    assert!(String::from_utf8_lossy(&missing_ownership.stderr)
        .contains("MERGE_QUEUE_COLLABORATION_REQUIRED"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_accepts_current_zone_transition_contract_path() {
    let root = temp_root("current-zone-transition-contract");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["governance"]["zone_transition_contract"] =
        Value::String("contracts/transitions/zone-transition.manifest.json".into());
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();

    let result = run(&["verify", root_text]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
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
fn verify_accepts_legacy_lock_bundle_resources() {
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
    assert!(
        result.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
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
    let result = run(&["pin-lock", root, "--binary", sdk_binary.to_str().unwrap()]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn prepare_lifecycle_chain_fixture(root: &PathBuf) -> String {
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(root);
    let goal_file = root.join(".appsdk/goal.json");
    let mut goal: Value = serde_json::from_str(&fs::read_to_string(&goal_file).unwrap()).unwrap();
    goal["status"] = Value::String("confirmed".into());
    goal["confirmed_by"] = Value::String("test".into());
    goal["confirmed_at"] = Value::String("2026-01-01T00:00:00Z".into());
    fs::write(
        &goal_file,
        serde_json::to_string_pretty(&goal).unwrap() + "\n",
    )
    .unwrap();
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
    let artifact: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json")).unwrap(),
    )
    .unwrap();
    let artifact_hash = artifact["artifact_hash"].as_str().unwrap().to_string();
    write_records(root, "app-core", &artifact_hash, false, "issue-1");
    artifact_hash
}

fn install_legacy_governance_maps(root: &Path) {
    for (name, content) in [
        (
            "resource-map.json",
            include_str!("../../contracts/migrations/0.1.5/governance-maps/resource-map.json"),
        ),
        (
            "function-map.json",
            include_str!("../../contracts/migrations/0.1.5/governance-maps/function-map.json"),
        ),
        (
            "mainline-call-map.json",
            include_str!("../../contracts/migrations/0.1.5/governance-maps/mainline-call-map.json"),
        ),
        (
            "verification-map.json",
            include_str!("../../contracts/migrations/0.1.5/governance-maps/verification-map.json"),
        ),
    ] {
        fs::write(root.join(".appsdk/maps").join(name), content).unwrap();
    }
}

fn install_current_governance_maps(root: &Path) {
    for (name, content) in [
        (
            "resource-map.json",
            include_str!("../../contracts/maps/resource-map.json"),
        ),
        (
            "function-map.json",
            include_str!("../../contracts/maps/function-map.json"),
        ),
        (
            "mainline-call-map.json",
            include_str!("../../contracts/maps/mainline-call-map.json"),
        ),
        (
            "verification-map.json",
            include_str!("../../contracts/maps/verification-map.json"),
        ),
    ] {
        fs::write(root.join(".appsdk/maps").join(name), content).unwrap();
    }
}

fn install_previous_bundle_migration_record(root: &Path) -> (String, String) {
    let migration_root = root.join(".appsdk/migrations/0.1.5-to-0.1.6");
    fs::create_dir_all(migration_root.join("maps")).unwrap();
    let previous_bundle_digest = format!("sha256:{}", "1".repeat(64));
    let previous_manifest_digest = format!("sha256:{}", "2".repeat(64));
    let mut maps = Vec::new();
    for (name, content) in [
        (
            "resource-map.json",
            include_str!("../../contracts/migrations/0.1.5/governance-maps/resource-map.json"),
        ),
        (
            "function-map.json",
            include_str!("../../contracts/migrations/0.1.5/governance-maps/function-map.json"),
        ),
        (
            "mainline-call-map.json",
            include_str!("../../contracts/migrations/0.1.5/governance-maps/mainline-call-map.json"),
        ),
        (
            "verification-map.json",
            include_str!("../../contracts/migrations/0.1.5/governance-maps/verification-map.json"),
        ),
    ] {
        fs::write(migration_root.join("maps").join(name), content).unwrap();
        maps.push(serde_json::json!({
            "name": name,
            "source_digest": digest(content),
            "target_digest": format!("sha256:{}", "3".repeat(64)),
            "snapshot_path": format!(".appsdk/migrations/0.1.5-to-0.1.6/maps/{}", name)
        }));
    }
    let record = serde_json::json!({
        "schema_version": 1,
        "migration_id": "appsdk-0.1.5-to-0.1.6",
        "source_version": "0.1.5",
        "target_version": "0.1.6",
        "bundle_digest": previous_bundle_digest,
        "maps": maps,
        "frozen_reviews": [],
        "legacy_reconciled_reviews": [],
        "created_at": "2026-01-01T00:00:00Z"
    });
    fs::write(
        migration_root.join("record.json"),
        serde_json::to_string_pretty(&record).unwrap() + "\n",
    )
    .unwrap();

    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["version"] = Value::String("0.1.6".into());
    lock["bundle_digest"] = Value::String(previous_bundle_digest.clone());
    lock["bundle_manifest_digest"] = Value::String(previous_manifest_digest);
    fs::write(
        lock_path,
        serde_json::to_string_pretty(&lock).unwrap() + "\n",
    )
    .unwrap();
    (
        serde_json::to_string_pretty(&record).unwrap() + "\n",
        previous_bundle_digest,
    )
}

#[test]
fn pin_lock_reconciles_previous_bundle_target_without_rewriting_migration_record() {
    let root = temp_root("pin-lock-guidance-bundle-refresh");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (original_record, previous_bundle_digest) = install_previous_bundle_migration_record(&root);

    let migrated = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        migrated.status.success(),
        "{}",
        String::from_utf8_lossy(&migrated.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json")).unwrap(),
        original_record
    );
    let lock: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/sdk.lock")).unwrap()).unwrap();
    assert_eq!(lock["previous_bundle_digest"], previous_bundle_digest);
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_accepts_chained_previous_bundle_witness_without_rewriting_migration_record() {
    let root = temp_root("pin-lock-chained-bundle-witness");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (original_record, previous_bundle_digest) = install_previous_bundle_migration_record(&root);
    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    let intermediate_bundle_digest = format!("sha256:{}", "b".repeat(64));
    lock["bundle_digest"] = Value::String(intermediate_bundle_digest.clone());
    lock["previous_bundle_digest"] = Value::String(previous_bundle_digest.clone());
    fs::write(
        &lock_path,
        serde_json::to_string_pretty(&lock).unwrap() + "\n",
    )
    .unwrap();

    let stale = run(&["verify", root_text]);
    assert!(
        stale.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&stale.stderr)
    );

    let migrated = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        migrated.status.success(),
        "{}",
        String::from_utf8_lossy(&migrated.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json")).unwrap(),
        original_record
    );
    let lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    let resources: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/sdk-resources.json")).unwrap())
            .unwrap();
    assert_eq!(lock["bundle_digest"], resources["bundle_digest"]);
    assert_ne!(lock["bundle_digest"], intermediate_bundle_digest);
    assert_eq!(lock["previous_bundle_digest"], previous_bundle_digest);
    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_invalid_chained_bundle_witness_without_overwrite() {
    let root = temp_root("pin-lock-invalid-chained-witness");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (original_record, previous_bundle_digest) = install_previous_bundle_migration_record(&root);
    let record_path = root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json");
    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["bundle_digest"] = Value::String("sha256:invalid".into());
    lock["previous_bundle_digest"] = Value::String(previous_bundle_digest.clone());
    let malformed_lock = serde_json::to_string_pretty(&lock).unwrap() + "\n";
    fs::write(&lock_path, &malformed_lock).unwrap();

    let malformed = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!malformed.status.success());
    assert!(String::from_utf8_lossy(&malformed.stderr)
        .contains("SDK_MIGRATION_BUNDLE_WITNESS_REQUIRED"));
    assert_eq!(fs::read_to_string(&lock_path).unwrap(), malformed_lock);
    assert_eq!(fs::read_to_string(&record_path).unwrap(), original_record);

    lock["bundle_digest"] = Value::String(format!("sha256:{}", "b".repeat(64)));
    lock["previous_bundle_digest"] = Value::String(format!("sha256:{}", "c".repeat(64)));
    let unrelated_lock = serde_json::to_string_pretty(&lock).unwrap() + "\n";
    fs::write(&lock_path, &unrelated_lock).unwrap();
    let unrelated = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!unrelated.status.success());
    assert!(String::from_utf8_lossy(&unrelated.stderr)
        .contains("SDK_MIGRATION_BUNDLE_WITNESS_REQUIRED"));
    assert_eq!(fs::read_to_string(&lock_path).unwrap(), unrelated_lock);
    assert_eq!(fs::read_to_string(&record_path).unwrap(), original_record);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_unreconciled_live_map_without_overwrite() {
    let root = temp_root("pin-lock-unreconciled-live-map");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (original_record, _) = install_previous_bundle_migration_record(&root);
    let tampered_map = "{\"tampered\":true}\n";
    let map_path = root.join(".appsdk/maps/resource-map.json");
    fs::write(&map_path, tampered_map).unwrap();

    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("SDK_MIGRATION_LIVE_MAP_UNRECONCILED:resource-map.json"));
    assert_eq!(fs::read_to_string(map_path).unwrap(), tampered_map);
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json")).unwrap(),
        original_record
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_current_maps_without_previous_bundle_witness() {
    let root = temp_root("pin-lock-current-map-without-witness");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (original_record, _) = install_previous_bundle_migration_record(&root);
    install_current_governance_maps(&root);

    let lock_path = root.join(".appsdk/sdk.lock");
    let resources: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/sdk-resources.json")).unwrap())
            .unwrap();
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["bundle_digest"] = resources["bundle_digest"].clone();
    lock.as_object_mut()
        .unwrap()
        .remove("previous_bundle_digest");
    fs::write(
        &lock_path,
        serde_json::to_string_pretty(&lock).unwrap() + "\n",
    )
    .unwrap();

    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("SDK_MIGRATION_BUNDLE_WITNESS_REQUIRED")
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json")).unwrap(),
        original_record
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_preserves_historical_custom_maps_with_bundle_witness() {
    let root = temp_root("pin-lock-historical-custom-maps");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (original_record, _) = install_previous_bundle_migration_record(&root);
    let record_path = root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json");
    let mut record: Value = serde_json::from_str(&original_record).unwrap();
    let mut preserved = Vec::new();
    for entry in record["maps"].as_array_mut().unwrap() {
        let name = entry["name"].as_str().unwrap().to_string();
        let live = root.join(".appsdk/maps").join(&name);
        let snapshot = root.join(entry["snapshot_path"].as_str().unwrap());
        let content = fs::read_to_string(&live).unwrap() + "\n";
        entry["canonical_source_digest"] = entry["source_digest"].clone();
        entry["canonical_target_digest"] = entry["target_digest"].clone();
        entry["source_digest"] = Value::String(digest(&content));
        entry["target_digest"] = Value::String(digest(&content));
        fs::write(&live, &content).unwrap();
        fs::write(&snapshot, &content).unwrap();
        preserved.push((live, snapshot, content));
    }
    let historical_record = serde_json::to_string_pretty(&record).unwrap() + "\n";
    fs::write(&record_path, &historical_record).unwrap();
    for _ in 0..2 {
        let result = run(&[
            "pin-lock",
            root_text,
            "--binary",
            binary().to_str().unwrap(),
        ]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(fs::read_to_string(&record_path).unwrap(), historical_record);
        for (live, snapshot, content) in &preserved {
            assert_eq!(&fs::read_to_string(live).unwrap(), content);
            assert_eq!(&fs::read_to_string(snapshot).unwrap(), content);
        }
        let verified = run(&["verify", root_text]);
        assert!(
            verified.status.success(),
            "{}",
            String::from_utf8_lossy(&verified.stderr)
        );
    }
    let lock_path = root.join(".appsdk/sdk.lock");
    let valid_lock = fs::read_to_string(&lock_path).unwrap();
    let mut lock: Value = serde_json::from_str(&valid_lock).unwrap();
    lock.as_object_mut()
        .unwrap()
        .remove("previous_bundle_digest");
    fs::write(&lock_path, serde_json::to_string_pretty(&lock).unwrap()).unwrap();
    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("SDK_MIGRATION_BUNDLE_WITNESS_REQUIRED")
    );
    fs::write(&lock_path, valid_lock).unwrap();
    fs::write(&preserved[0].1, "tampered snapshot").unwrap();
    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("SDK_MIGRATION_SNAPSHOT_MISMATCH:resource-map.json"));
    fs::write(&preserved[0].1, &preserved[0].2).unwrap();
    record["maps"][0]["canonical_target_digest"] = Value::String("sha256:invalid".into());
    fs::write(&record_path, serde_json::to_string_pretty(&record).unwrap()).unwrap();
    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("INVALID_SDK_MIGRATION_RECORD"));
    fs::write(&record_path, &historical_record).unwrap();
    fs::write(&preserved[0].0, "{\"tampered\":true}\n").unwrap();
    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("SDK_MIGRATION_TARGET_MAP_MISMATCH:resource-map.json"));
    assert_eq!(fs::read_to_string(&record_path).unwrap(), historical_record);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_custom_map_record_from_bundle_reconciliation() {
    let root = temp_root("pin-lock-custom-map-record");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (original_record, _) = install_previous_bundle_migration_record(&root);
    let record_path = root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json");
    let mut record: Value = serde_json::from_str(&original_record).unwrap();
    record["maps"][0]["canonical_source_digest"] = Value::String(digest(include_str!(
        "../../contracts/migrations/0.1.5/governance-maps/resource-map.json"
    )));
    record["maps"][0]["canonical_target_digest"] = Value::String(digest(include_str!(
        "../../contracts/maps/resource-map.json"
    )));
    fs::write(
        &record_path,
        serde_json::to_string_pretty(&record).unwrap() + "\n",
    )
    .unwrap();

    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("SDK_MIGRATION_TARGET_MAP_MISMATCH:resource-map.json"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_reconciliation_is_idempotent_and_snapshot_remains_immutable() {
    let root = temp_root("pin-lock-reconciliation-idempotent");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (original_record, _) = install_previous_bundle_migration_record(&root);

    let first = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let lock_path = root.join(".appsdk/sdk.lock");
    let first_lock = fs::read_to_string(&lock_path).unwrap();

    let second = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json")).unwrap(),
        original_record
    );
    assert_eq!(fs::read_to_string(&lock_path).unwrap(), first_lock);

    let snapshot_path = root.join(".appsdk/migrations/0.1.5-to-0.1.6/maps/resource-map.json");
    let snapshot = fs::read_to_string(&snapshot_path).unwrap();
    fs::write(&snapshot_path, "{}\n").unwrap();
    let rejected = run(&["verify", root_text]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("SDK_MIGRATION_SNAPSHOT_MISMATCH:resource-map.json"));
    fs::write(&snapshot_path, snapshot).unwrap();
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_rejects_malformed_previous_bundle_digest() {
    let root = temp_root("invalid-previous-bundle-digest");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    pin_test_lock(root_text);
    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["previous_bundle_digest"] = Value::String("sha256:not-a-digest".into());
    fs::write(
        &lock_path,
        serde_json::to_string_pretty(&lock).unwrap() + "\n",
    )
    .unwrap();

    let rejected = run(&["verify", root_text]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("INVALID_SDK_BUNDLE_DIGEST"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_migrates_only_supported_sdk_and_matching_bundle_binary() {
    let root = temp_root("pin-lock-version-migration");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project_file = root.join(".appsdk/project.json");
    let lock_file = root.join(".appsdk/sdk.lock");
    let original_lock = fs::read_to_string(&lock_file).unwrap();
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();

    project["sdk"]["version"] = Value::String("0.1.2".into());
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let unsupported = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!unsupported.status.success());
    assert!(String::from_utf8_lossy(&unsupported.stderr)
        .contains("UNSUPPORTED_SDK_MIGRATION:0.1.2:0.1.6"));
    assert_eq!(fs::read_to_string(&lock_file).unwrap(), original_lock);

    project["sdk"]["version"] = Value::String("0.1.5".into());
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let wrong_binary = root.join("wrong-appsdk");
    fs::write(&wrong_binary, "not the running AppSDK Bundle\n").unwrap();
    let mismatched = run(&[
        "pin-lock",
        root_text,
        "--binary",
        wrong_binary.to_str().unwrap(),
    ]);
    assert!(!mismatched.status.success());
    assert!(String::from_utf8_lossy(&mismatched.stderr).contains("SDK_PIN_BINARY_BUNDLE_MISMATCH"));
    assert_eq!(fs::read_to_string(&lock_file).unwrap(), original_lock);
    fs::remove_file(wrong_binary).unwrap();

    install_legacy_governance_maps(&root);

    let migrated = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        migrated.status.success(),
        "{}",
        String::from_utf8_lossy(&migrated.stderr)
    );
    let migrated_project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    let migrated_lock: Value =
        serde_json::from_str(&fs::read_to_string(&lock_file).unwrap()).unwrap();
    assert_eq!(migrated_project["sdk"]["version"], "0.1.6");
    assert_eq!(migrated_lock["version"], "0.1.6");
    assert!(run(&["verify", root_text]).status.success());

    let migration_root = root.join(".appsdk/migrations/0.1.5-to-0.1.6");
    let migration_record = migration_root.join("record.json");
    assert!(migration_record.is_file());
    for name in [
        "resource-map.json",
        "function-map.json",
        "mainline-call-map.json",
        "verification-map.json",
    ] {
        assert_eq!(
            fs::read_to_string(migration_root.join("maps").join(name)).unwrap(),
            fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../contracts/migrations/0.1.5/governance-maps")
                    .join(name)
            )
            .unwrap()
        );
        assert_eq!(
            fs::read_to_string(root.join(".appsdk/maps").join(name)).unwrap(),
            fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../contracts/maps")
                    .join(name)
            )
            .unwrap()
        );
    }
    let snapshot = migration_root.join("maps/resource-map.json");
    let source_map = fs::read_to_string(&snapshot).unwrap();
    fs::write(&snapshot, "{}\n").unwrap();
    let snapshot_rejected = run(&["verify", root_text]);
    assert!(!snapshot_rejected.status.success());
    assert!(String::from_utf8_lossy(&snapshot_rejected.stderr)
        .contains("SDK_MIGRATION_SNAPSHOT_MISMATCH:resource-map.json"));
    fs::write(&snapshot, source_map).unwrap();
    assert!(run(&["verify", root_text]).status.success());

    let resumed = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(resumed.status.success());
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();

    let partial = temp_root("pin-lock-partial-0.1.6-map-migration");
    let partial_text = partial.to_str().unwrap();
    assert!(run(&["new", partial_text]).status.success());
    install_legacy_governance_maps(&partial);
    let repaired = run(&[
        "pin-lock",
        partial_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        repaired.status.success(),
        "{}",
        String::from_utf8_lossy(&repaired.stderr)
    );
    assert!(partial
        .join(".appsdk/migrations/0.1.5-to-0.1.6/record.json")
        .is_file());
    assert!(run(&["verify", partial_text]).status.success());
    fs::remove_dir_all(partial).unwrap();
}

#[test]
fn pin_lock_migrates_stale_project_record_contracts() {
    let root = temp_root("record-contract-migration");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let worktree = root.join("contracts/records/worktree-record.schema.json");
    let promotion = root.join("contracts/records/promotion-record.schema.json");
    let mut current_worktree: Value =
        serde_json::from_str(&fs::read_to_string(&worktree).unwrap()).unwrap();
    current_worktree["properties"]
        .as_object_mut()
        .unwrap()
        .remove("bug_triage");
    fs::write(
        &worktree,
        serde_json::to_vec_pretty(&current_worktree).unwrap(),
    )
    .unwrap();
    let mut current_promotion: Value =
        serde_json::from_str(&fs::read_to_string(&promotion).unwrap()).unwrap();
    current_promotion["properties"]
        .as_object_mut()
        .unwrap()
        .remove("bug_closure_verified");
    fs::write(
        &promotion,
        serde_json::to_vec_pretty(&current_promotion).unwrap(),
    )
    .unwrap();
    assert!(!run(&["verify", root_text]).status.success());
    assert!(run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap()
    ])
    .status
    .success());
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&worktree).unwrap()).unwrap(),
        serde_json::from_str::<Value>(include_str!(
            "../../contracts/records/worktree-record.schema.json"
        ))
        .unwrap()
    );
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&promotion).unwrap()).unwrap(),
        serde_json::from_str::<Value>(include_str!(
            "../../contracts/records/promotion-record.schema.json"
        ))
        .unwrap()
    );
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_reconciles_matching_authoring_bundle_mirror() {
    let root = temp_root("pin-lock-authoring-bundle-mirror");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["sdk"]["version"] = Value::String("0.1.5".into());
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    install_legacy_governance_maps(&root);

    let previous_manifest = serde_json::json!({
        "schema_version": 1,
        "sdk": "appsdk",
        "version": "0.1.5",
        "runtime_entrypoint": "rust-binary"
    });
    fs::create_dir_all(root.join("contracts")).unwrap();
    let previous_bytes = serde_json::to_vec_pretty(&previous_manifest).unwrap();
    fs::write(
        root.join("contracts/sdk-bundle.manifest.json"),
        &previous_bytes,
    )
    .unwrap();
    fs::write(
        root.join(".appsdk/contracts/sdk-bundle.manifest.json"),
        &previous_bytes,
    )
    .unwrap();

    let migrated = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        migrated.status.success(),
        "{}",
        String::from_utf8_lossy(&migrated.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("contracts/sdk-bundle.manifest.json")).unwrap(),
        include_str!("../../contracts/sdk-bundle.manifest.json")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_drifted_authoring_bundle_mirror_before_migration() {
    let root = temp_root("pin-lock-authoring-bundle-drift");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["sdk"]["version"] = Value::String("0.1.5".into());
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    install_legacy_governance_maps(&root);

    fs::create_dir_all(root.join("contracts")).unwrap();
    fs::write(
        root.join("contracts/sdk-bundle.manifest.json"),
        br#"{"schema_version":1,"sdk":"appsdk","version":"0.1.5","runtime_entrypoint":"rust-binary"}
"#,
    )
    .unwrap();
    fs::write(
        root.join(".appsdk/contracts/sdk-bundle.manifest.json"),
        br#"{"schema_version":1,"sdk":"appsdk","version":"0.1.5","runtime_entrypoint":"other"}
"#,
    )
    .unwrap();
    let original_project = fs::read_to_string(&project_file).unwrap();
    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("SDK_AUTHORING_BUNDLE_MIRROR_DRIFT"));
    assert_eq!(fs::read_to_string(&project_file).unwrap(), original_project);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn migrated_project_verifies_without_local_sdk_witness_or_binary_digest_match() {
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
    let source_promote = run_in(&root, &["promote", "--to", "source_implemented"]);
    assert!(
        source_promote.status.success(),
        "{}",
        String::from_utf8_lossy(&source_promote.stderr)
    );
    assert!(run_in(&root, &["promote", "--to", "contract_bound"])
        .status
        .success());
    let compile = run_in(&root, &["compile"]);
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
    let verified_after_digest_change = run(&["verify", root_text]);
    assert!(
        verified_after_digest_change.status.success(),
        "{}",
        String::from_utf8_lossy(&verified_after_digest_change.stderr)
    );
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

fn stable_review_id(
    promotion_id: &str,
    fix_candidate_id: &str,
    reviewer: &Value,
    verdict: &str,
    evidence_ids: &[&str],
) -> String {
    let identity = serde_json::json!({
        "promotion_id": promotion_id,
        "fix_candidate_id": fix_candidate_id,
        "reviewer": reviewer,
        "verdict": verdict,
        "evidence_ids": evidence_ids
    });
    format!(
        "review-{}",
        digest(&canonical(&identity))
            .strip_prefix("sha256:")
            .unwrap()
    )
}

fn install_authoritative_bug_fixture(root: &Path, issue_id: &str) -> PathBuf {
    let fake_bin = root.with_extension("fake-git-bug");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_git_bug = fake_bin.join("git-bug");
    fs::write(
        &fake_git_bug,
        format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n  \"bug show\")\n    printf '%s\\n' '{{\"id\":\"bug-object\",\"human_id\":\"{}\",\"status\":\"closed\",\"comments\":[{{\"id\":\"comment-1\",\"message\":\"### Solution / Resolution\\nfixed\"}}]}}'\n    exit 0\n    ;;\n  *) exit 64 ;;\nesac\n",
            issue_id
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_git_bug, fs::Permissions::from_mode(0o755)).unwrap();
    let ops = root.join("bug-ops.json");
    fs::write(
        &ops,
        r#"{"ops":[{"type":4,"status":2,"timestamp":"2026-01-01T00:00:00Z"}]}"#,
    )
    .unwrap();
    let blob = git_test_value(root, &["hash-object", "-w", ops.to_str().unwrap()]);
    let tree_input = format!("100644 blob {}\tops\n", blob);
    let mut mktree = Command::new("git")
        .args(["-C", root.to_str().unwrap(), "mktree"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    mktree
        .stdin
        .as_mut()
        .unwrap()
        .write_all(tree_input.as_bytes())
        .unwrap();
    let tree = mktree.wait_with_output().unwrap();
    assert!(tree.status.success());
    let tree = String::from_utf8(tree.stdout).unwrap().trim().to_string();
    let commit = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "commit-tree",
            &tree,
            "-m",
            "close bug",
        ])
        .output()
        .unwrap();
    assert!(commit.status.success());
    let commit = String::from_utf8(commit.stdout).unwrap().trim().to_string();
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "update-ref",
            "refs/bugs/bug-object",
            &commit
        ])
        .status()
        .unwrap()
        .success());
    fs::remove_file(ops).unwrap();
    fake_git_bug
}

fn write_records(
    root: &PathBuf,
    module_id: &str,
    artifact_hash: &str,
    include_freeze: bool,
    issue_id: &str,
) {
    let fake_git_bug = install_authoritative_bug_fixture(root, issue_id);
    unsafe {
        env::set_var("GIT_BUG_BIN", &fake_git_bug);
    }
    let records = root.join(".appsdk/records");
    fs::create_dir_all(&records).unwrap();
    let evidence_dir = records.join("evidence").join(module_id);
    fs::create_dir_all(&evidence_dir).unwrap();
    let commit = git_test_value(root, &["rev-parse", "HEAD"]);
    let tree = git_test_value(root, &["rev-parse", "HEAD^{tree}"]);
    let review_id = stable_review_id(
        "promotion-1",
        "candidate-1",
        &serde_json::json!({"adapter":"test","identity":"test"}),
        "pass",
        &["candidate-evidence-1", "positive-1", "negative-1"],
    );
    let map_root = root.join(".appsdk/maps");
    let evidence = |id: &str, phase: &str, kind: &str, created_at: &str| {
        serde_json::json!({
            "evidence_id": id,
            "issue_id": issue_id,
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
        evidence_dir.join("whitebox-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "evidence_id":"whitebox-1","issue_id":issue_id,"experiment_id":"experiment-1",
            "phase":"development_whitebox","kind":"gate","source_commit":commit,
            "artifact_hash":artifact_hash,"execution_surface":"development_whitebox",
            "scope":{"module_id":module_id},"producer":{"adapter":"test","identity":"whitebox-runner"},
            "result":"pass","created_at":"2026-01-01T00:03:10Z","expires_at":"2099-01-01T00:00:00Z",
            "input_hashes":["input-1"],"scope_hash":"scope-1"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        evidence_dir.join("install-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "evidence_id":"install-1","issue_id":issue_id,"experiment_id":"experiment-1",
            "phase":"deployment_install","kind":"install","source_commit":commit,
            "artifact_hash":artifact_hash,"execution_surface":"deployed_blackbox",
            "environment_id":"test-deployment","entrypoint":"test://installed-app",
            "scope":{"module_id":module_id,"entrypoint":"test://installed-app"},
            "producer":{"adapter":"test","identity":"test-deployment-adapter"},"result":"pass",
            "created_at":"2026-01-01T00:03:15Z","expires_at":"2099-01-01T00:00:00Z",
            "input_hashes":["input-1"],"scope_hash":"scope-1"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        evidence_dir.join("restart-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "evidence_id":"restart-1","issue_id":issue_id,"experiment_id":"experiment-1",
            "phase":"deployment_restart","kind":"restart","source_commit":commit,
            "artifact_hash":artifact_hash,"execution_surface":"deployed_blackbox",
            "environment_id":"test-deployment","entrypoint":"test://installed-app",
            "scope":{"module_id":module_id,"entrypoint":"test://installed-app"},
            "producer":{"adapter":"test","identity":"test-deployment-adapter"},"result":"pass",
            "created_at":"2026-01-01T00:03:20Z","expires_at":"2099-01-01T00:00:00Z",
            "input_hashes":["input-1"],"scope_hash":"scope-1"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        evidence_dir.join("blackbox-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "evidence_id":"blackbox-1","issue_id":issue_id,"experiment_id":"experiment-1",
            "phase":"deployed_blackbox","kind":"runtime","source_commit":commit,
            "artifact_hash":artifact_hash,"execution_surface":"deployed_blackbox",
            "environment_id":"test-deployment","entrypoint":"test://installed-app",
            "scope":{"module_id":module_id,"entrypoint":"test://installed-app"},
            "producer":{"adapter":"test","identity":"test-deployment-adapter"},"result":"pass",
            "created_at":"2026-01-01T00:03:30Z","expires_at":"2099-01-01T00:00:00Z",
            "input_hashes":["input-1"],"scope_hash":"scope-1"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
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
    let mut worktree_record = serde_json::json!({
        "worktree_id":"worktree-1","issue_id":issue_id,"module_id":module_id,
        "base_ref":"HEAD","base_commit":commit,"branch":"test-fix","head_commit":commit,
        "initial_clean":true,"final_clean":true,"isolation_mode":"isolated_worktree",
        "scope_hash":"scope-1","created_at":"2026-01-01T00:00:00Z",
        "bug_triage":{"query_executed":true,"query":format!("appsdk bug list -q {}", issue_id),"mode":"new_confirmed","reopened_from_issue_id":null}
    });
    worktree_record["bug_triage_query_binding"] =
        Value::String(digest(&canonical(&serde_json::json!({
            "issue_id": issue_id,
            "query": format!("appsdk bug list -q {}", issue_id),
            "mode": "new_confirmed",
            "reopened_from_issue_id": null
        }))));
    fs::write(
        records.join(format!("worktree-record-{module_id}.json")),
        serde_json::to_string_pretty(&worktree_record).unwrap() + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("reproduction-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "reproduction_id":"reproduction-1","issue_id":issue_id,"module_id":module_id,
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
            "fix_candidate_id":"candidate-1","issue_id":issue_id,"module_id":module_id,
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
        records.join(format!("pre-review-validation-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "validation_id":"pre-review-validation-1","issue_id":issue_id,"module_id":module_id,
            "fix_candidate_id":"candidate-1","candidate_commit":commit,"candidate_tree_hash":tree,
            "artifact_hash":artifact_hash,"whitebox_producer":{"adapter":"test","identity":"whitebox-runner"},
            "whitebox_evidence_ids":["whitebox-1"],
            "blackbox_evidence_ids":["blackbox-1"],"deployment":{"environment_id":"test-deployment",
            "install_receipt_id":"install-1","restart_receipt_id":"restart-1",
            "entrypoint":"test://installed-app","producer":{"adapter":"test","identity":"test-deployment-adapter"},
            "observed_at":"2026-01-01T00:03:30Z"},"source_unchanged":true,
            "result":"pass","created_at":"2026-01-01T00:03:45Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("review-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "review_id":review_id,"issue_id":issue_id,"promotion_id":"promotion-1",
            "review_kind":"architecture","fix_candidate_id":"candidate-1",
            "pre_review_validation_id":"pre-review-validation-1",
            "reviewer":{"adapter":"test","identity":"test"},"verdict":"pass",
            "evidence_ids":["candidate-evidence-1","positive-1","negative-1"],"reviewed_commit":commit,
            "reviewed_tree_hash":tree,"reviewed_diff_hash":"sha256:test-diff",
            "reviewed_artifact_hash":artifact_hash,"reviewed_scope_hash":"scope-1",
            "resource_map_hash":file_digest(&map_root.join("resource-map.json")),
            "function_map_hash":file_digest(&map_root.join("function-map.json")),
            "mainline_call_map_hash":file_digest(&map_root.join("mainline-call-map.json")),
            "verification_map_hash":file_digest(&map_root.join("verification-map.json")),
            "created_at":"2026-01-01T00:04:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("effectiveness-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "effectiveness_id":"effectiveness-1","issue_id":issue_id,"module_id":module_id,
            "fix_candidate_id":"candidate-1","architecture_review_id":review_id,
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
            "merge_id":"merge-1","issue_id":issue_id,"module_id":module_id,
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
        "promotion_id":"promotion-1","issue_id":issue_id,"experiment_id":"experiment-1",
        "module_id":module_id,"base_commit":commit,"source_commit":commit,
        "candidate_commit":commit,"merged_commit":commit,
        "worktree_record_id":"worktree-1","reproduction_record_id":"reproduction-1",
        "fix_candidate_id":"candidate-1","architecture_review_id":review_id,
        "effectiveness_record_id":"effectiveness-1","merge_record_id":"merge-1",
        "previous_active_version":null,"new_active_version":"active-v1",
        "artifact_hash":artifact_hash,"scope_hash":"scope-1","public_api_hash":"api-1",
        "review_id":review_id,"evidence_ids":["candidate-evidence-1","effective-1"],
        "required_gate_results":[{"gate_id":"fix_lifecycle_graph","result":"pass","producer":"test"}],
        "change_set_id":"change-1","compatibility_level":"compatible","root_cause":"test root cause",
        "design_id":"design-1","change_reason_comment":"test reason",
        "playground_cleanup_record_id":"cleanup-1","bug_closure_verified":true,"created_at":"2026-01-01T00:07:00Z"
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
                r#"{{"freeze_id":"freeze-1","issue_id":"{}","module_id":"{}","promotion_id":"promotion-1","promotion_record_hash":"{}","artifact_record_id":"candidate-evidence-1","source_commit_or_tag":"{}","active_version":"active-v1","previous_active_version":null,"library_hash":"{}","public_api_hash":"api-1","review_id":"{}","previous_active_immutable":false,"git_clean":true,"clean_scope":{{"base_commit":"{}","changed_paths":[],"ignored_paths":[],"generated_policy":"tracked_hash"}},"owners":{{"vcs":"test","compiler":"test","api_extractor":"test","review":"test","artifact_registry":"test"}},"created_at":"2026-01-01T00:08:00Z"}}"#,
                issue_id, module_id, promotion_hash, commit, artifact_hash, review_id, commit
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
    write_records(root, module_id, artifact_hash, include_freeze, "issue-1");
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
    write_records(&root, module_id, artifact_hash, true, "issue-1");
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
        "review_id": stable_review_id(
            "promotion-1",
            "candidate-1",
            &serde_json::json!({"adapter":"test","identity":"test"}),
            "pass",
            &["candidate-evidence-1", "positive-1", "negative-1"],
        ),
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
fn lifecycle_mutation_rejects_main_branch() {
    let root = temp_root("main-mutation");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    assert!(Command::new("git")
        .args(["-C", root_text, "branch", "-M", "main"])
        .status()
        .unwrap()
        .success());

    let compile = run(&["compile", root_text]);
    assert!(!compile.status.success());
    assert!(String::from_utf8_lossy(&compile.stderr).contains("MAIN_WORKTREE_MUTATION_FORBIDDEN"));

    let verify = run(&["verify", root_text]);
    assert!(verify.status.success(), "verify should remain read-only");
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
fn development_dependencies_require_current_artifacts_and_freeze_order() {
    let root = temp_root("development-dependencies");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let goal_path = root.join(".appsdk/goal.json");
    let mut goal: Value = serde_json::from_slice(&fs::read(&goal_path).unwrap()).unwrap();
    goal["status"] = Value::from("confirmed");
    goal["confirmed_by"] = Value::from("test");
    goal["confirmed_at"] = Value::from("2026-01-01T00:00:00Z");
    fs::write(&goal_path, serde_json::to_vec_pretty(&goal).unwrap()).unwrap();
    let project_path = root.join(".appsdk/project.json");
    let mut project: Value = serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();
    let mut edge = project["modules"][0].clone();
    edge["module_id"] = Value::from("app-edge");
    edge["source_owner"] = Value::from("app-edge");
    edge["owned_paths"] = serde_json::json!(["playground/edge/**"]);
    edge["active_artifact"] = Value::from("active/lib/app-edge/**");
    edge["dependency_modules"] = serde_json::json!(["app-core"]);
    edge["build"]["args"] = serde_json::json!(["-c", "mkdir -p generated/modules/app-edge/lib && printf edge > generated/modules/app-edge/lib/edge.txt"]);
    edge["artifact_paths"] = serde_json::json!(["edge.txt"]);
    project["modules"].as_array_mut().unwrap().push(edge);
    fs::create_dir_all(root.join("playground/edge")).unwrap();
    fs::write(&project_path, serde_json::to_vec_pretty(&project).unwrap()).unwrap();
    pin_test_lock(root_text);
    assert!(run(&["promote", root_text, "--to", "source_implemented"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    project = serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();
    let compiled = run(&["compile", root_text]);
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let core_path = root.join("generated/modules/app-core/module.compiled.json");
    let edge_path = root.join("generated/modules/app-edge/module.compiled.json");
    let core: Value = serde_json::from_slice(&fs::read(&core_path).unwrap()).unwrap();
    let edge: Value = serde_json::from_slice(&fs::read(&edge_path).unwrap()).unwrap();
    assert_eq!(
        edge["dependency_hashes"][0]["artifact_hash"],
        core["artifact_hash"]
    );
    fs::write(root.join("playground/experiments/changed.txt"), "changed").unwrap();
    let stale = run(&["compile-module", root_text, "--module", "app-edge"]);
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("MODULE_DEPENDENCY_ARTIFACT_STALE"));
    assert!(run(&["compile", root_text]).status.success());
    let current: Value = serde_json::from_slice(&fs::read(&core_path).unwrap()).unwrap();
    let library = root
        .join("generated/modules/app-core/lib")
        .join(current["artifacts"][0]["path"].as_str().unwrap());
    fs::write(library, "tampered dependency bytes").unwrap();
    let tampered = run(&["compile-module", root_text, "--module", "app-edge"]);
    assert!(!tampered.status.success());
    assert!(String::from_utf8_lossy(&tampered.stderr).contains("MODULE_DEPENDENCY_ARTIFACT_STALE"));
    assert!(run(&["compile", root_text]).status.success());
    project["modules"][0]["dependency_modules"] = serde_json::json!(["app-core"]);
    fs::write(&project_path, serde_json::to_vec_pretty(&project).unwrap()).unwrap();
    let cycle = run(&["compile-module", root_text, "--module", "app-core"]);
    assert!(!cycle.status.success());
    assert!(String::from_utf8_lossy(&cycle.stderr).contains("MODULE_DEPENDENCY_ORDER"));
    project["modules"][0]["dependency_modules"] = serde_json::json!([]);
    // Publication must not turn a development dependency into an immutable one.
    project["modules"][1]["stage"] = Value::from("architecture_stable");
    fs::write(&project_path, serde_json::to_vec_pretty(&project).unwrap()).unwrap();
    init_git(&root);
    let freeze = run(&["freeze", root_text, "--module", "app-edge"]);
    assert!(!freeze.status.success());
    assert!(
        String::from_utf8_lossy(&freeze.stderr).contains("MODULE_DEPENDENCY_NOT_FROZEN"),
        "{}",
        String::from_utf8_lossy(&freeze.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn compile_resolves_project_artifact_paths_and_normal_node_modules_links() {
    let root = temp_root("project-artifact-path-and-node-modules");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let goal_path = root.join(".appsdk/goal.json");
    let mut goal: Value = serde_json::from_slice(&fs::read(&goal_path).unwrap()).unwrap();
    goal["status"] = Value::String("confirmed".into());
    goal["confirmed_by"] = Value::String("test".into());
    goal["confirmed_at"] = Value::String("2026-01-01T00:00:00Z".into());
    fs::write(&goal_path, serde_json::to_vec_pretty(&goal).unwrap()).unwrap();

    let project_path = root.join(".appsdk/project.json");
    let mut project: Value = serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();
    let module = &mut project["modules"][0];
    module["module_id"] = Value::String("relay-service".into());
    module["source_owner"] = Value::String("relay-service".into());
    module["owned_paths"] = serde_json::json!(["services/relay/**", "protocol/relay/**"]);
    module["active_artifact"] = Value::String("active/lib/relay-service".into());
    module["generated_outputs"] = serde_json::json!([
        "services/relay/dist/**",
        "generated/modules/relay-service/**"
    ]);
    module["contract_paths"] = serde_json::json!(["docs/relay-service.md"]);
    module["build"] = serde_json::json!({
        "program": "sh",
        "args": [
            "-c",
            "mkdir -p generated/modules/relay-service/lib && printf relay > generated/modules/relay-service/lib/relay.tar"
        ],
        "working_directory": "."
    });
    module["artifact_paths"] = serde_json::json!(["generated/modules/relay-service/lib/relay.tar"]);
    module["regression"]["input_paths"] = serde_json::json!(["services/relay/**"]);
    fs::create_dir_all(root.join("services/relay/src")).unwrap();
    fs::create_dir_all(root.join("services/relay/node_modules/typescript/bin")).unwrap();
    fs::create_dir_all(root.join("services/relay/node_modules/.bin")).unwrap();
    fs::write(root.join("services/relay/src/index.ts"), "export {}\n").unwrap();
    fs::write(
        root.join("services/relay/node_modules/typescript/bin/tsc"),
        "#!/bin/sh\n",
    )
    .unwrap();
    symlink(
        "../typescript/bin/tsc",
        root.join("services/relay/node_modules/.bin/tsc"),
    )
    .unwrap();
    fs::create_dir_all(root.join("protocol/relay")).unwrap();
    fs::write(root.join("protocol/relay/protocol.ts"), "export {}\n").unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/relay-service.md"), "relay contract\n").unwrap();
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();

    pin_test_lock(root_text);
    assert!(run(&["promote", root_text, "--to", "source_implemented"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    project = serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();

    let compiled = run(&["compile", root_text]);
    assert!(
        compiled.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );
    let artifact_path = root.join("generated/modules/relay-service/module.compiled.json");
    let artifact: Value = serde_json::from_slice(&fs::read(&artifact_path).unwrap()).unwrap();
    assert_eq!(
        artifact["artifacts"][0]["path"],
        "generated/modules/relay-service/lib/relay.tar"
    );
    assert!(root
        .join("generated/modules/relay-service/lib/relay.tar")
        .is_file());

    project["modules"][0]["artifact_paths"] =
        serde_json::json!(["generated/modules/relay-service/lib/missing.tar"]);
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let missing = run(&["compile", root_text]);
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("ARTIFACT_PATH_MISSING"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&missing.stdout),
        String::from_utf8_lossy(&missing.stderr)
    );

    project["modules"][0]["artifact_paths"] = serde_json::json!(["../relay.tar"]);
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let wrong = run(&["compile", root_text]);
    assert!(!wrong.status.success());
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("INVALID_MODULE_ARTIFACT_PATH"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn confirmed_goal_and_initialized_lock_allow_compile_and_adjacent_promote() {
    let root = temp_root("positive");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let goal_file = root.join(".appsdk/goal.json");
    fs::write(&goal_file, r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#).unwrap();
    fs::write(root.join(".appsdk/project.json"), r#"{
  "schema_version": 1,
  "project_id": "change-me",
  "sdk": {"name": "appsdk", "version": "0.1.6"},
  "lifecycle": {"stage": "draft"},
  "development_scenarios": {"manifest": ".appsdk/contracts/development-scenarios.manifest.json", "enabled": []},
  "access": {"protected_paths":[".appsdk/**"]},
  "governance": {"playground_root":"playground/**","active_root":"active/**","protected_root":"protected/**","generated_root":"generated/**","active_kind":"immutable_consumable_library","protected_kinds":["source"],"generated_kinds":["compiler_output"],"freeze_requirements":["git_clean"],"promotion_requires":["evidence"],"runtime_forbidden_roots":["playground/**"],"record_contracts":["contracts/records/worktree-record.schema.json","contracts/records/reproduction-record.schema.json","contracts/records/evidence-record.schema.json","contracts/records/fix-candidate-record.schema.json","contracts/records/goal-clarification-record.schema.json","contracts/records/review-record.schema.json","contracts/records/effectiveness-record.schema.json","contracts/records/pre-review-validation-record.schema.json","contracts/records/collaboration-record.schema.json","contracts/records/collaboration-index.schema.json","contracts/records/merge-queue-record.schema.json","contracts/records/merge-queue-state.schema.json","contracts/records/integration-record.schema.json","contracts/records/mainline-receipt-record.schema.json","contracts/records/merge-record.schema.json","contracts/records/promotion-record.schema.json","contracts/records/regression-report.schema.json","contracts/records/freeze-record.schema.json","contracts/records/record-graph.contract.json"],"zone_transition_contract":"contracts/transitions/zone-transition-manifest.json","playground_retention":"archive_then_remove","debug_merge_comment_required":true},
  "lifecycles": {"issue":"open","library":"draft","source_snapshot":"mutable","artifact":"generated"},
    "modules": [{"module_id":"app-core","stage":"source_implemented","owned_paths":["playground/experiments/**"],"source_owner":"app-core","active_artifact":"active/lib/app-core/**","generated_outputs":["generated/**"],"contract_paths":["contracts/records/**","contracts/transitions/**"],"dependency_modules":[],"build":{"program":"sh","args":["-c","mkdir -p generated/modules/app-core/lib && printf 'app-core placeholder\\n' > generated/modules/app-core/lib/app-core.placeholder"],"working_directory":"."},"artifact_paths":["app-core.placeholder"],"regression":{"required_before_freeze":true,"suite_id":"app-core-regression","command":{"program":"cargo","args":["test"],"working_directory":"."},"input_paths":["playground/experiments/**"],"minimum_test_count":1,"allow_skipped":false,"ordinary_mode_after_freeze":"disabled","reenable_on":["source_change","contract_change","public_api_change","artifact_change","dependency_change"]}}]
}
"#).unwrap();
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
fn lifecycle_record_producer_binds_clean_worktree_and_baseline() {
    let root = temp_root("lifecycle-record-producer");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/goal.json"),
        r#"{"goal_id":"goal-1","issue_id":null,"raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#,
    )
    .unwrap();
    init_git(&root);
    let map_path = root.join(".appsdk/maps/resource-map.json");
    let mut extended_map: Value =
        serde_json::from_str(&fs::read_to_string(&map_path).unwrap()).unwrap();
    extended_map["resources"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "resource_id":"producer-test-extension",
            "owner":"test::extension",
            "truth_store":"test",
            "allowed_operations":["read"]
        }));
    fs::write(
        &map_path,
        serde_json::to_string_pretty(&extended_map).unwrap() + "\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", ".appsdk/maps/resource-map.json"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "map extension"])
        .status()
        .unwrap()
        .success());
    assert!(run(&["promote", root_text, "--to", "source_implemented"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    assert!(run(&["compile-module", root_text, "--module", "app-core"])
        .status
        .success());
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
    assert!(Command::new("git")
        .args(["-C", root_text, "add", ".appsdk/project.json"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "candidate"])
        .status()
        .unwrap()
        .success());
    let commit = git_test_value(&root, &["rev-parse", "HEAD"]);
    let artifact: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json")).unwrap(),
    )
    .unwrap();
    let scope_hash = digest(&canonical(&serde_json::json!({
        "module_id":"app-core",
        "source_hash":artifact["source_hash"],
        "contract_hash":artifact["contract_hash"]
    })));
    let input_path = root.with_extension("producer-input.json");
    let fake_bin = root.with_extension("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_git_bug = fake_bin.join("git-bug");
    fs::write(
        &fake_git_bug,
        r#"#!/bin/sh
case "$1 $2" in
  "bug show")
    printf '{"human_id":"%s","status":"open","title":"test issue"}\n' "$3"
    exit 0
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_git_bug, fs::Permissions::from_mode(0o755)).unwrap();
    let produce_with = |input: &Path, git_bug: &Path| {
        Command::new(binary())
            .args([
                "produce-lifecycle-records",
                root_text,
                "--module",
                "app-core",
                "--input",
                input.to_str().unwrap(),
            ])
            .env("GIT_BUG_BIN", git_bug)
            .env_remove("TMUX_PANE")
            .output()
            .unwrap()
    };
    let produce = |input: &Path| produce_with(input, &fake_git_bug);
    let producer_triage_binding = digest(&canonical(&serde_json::json!({
        "issue_id": "issue-producer-1",
        "query": "appsdk bug list -q issue-producer-1",
        "mode": "new_confirmed",
        "reopened_from_issue_id": null
    })));
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "goal_id":"goal-1",
            "worktree": {
                "worktree_id":"caller-worktree-id","issue_id":"issue-producer-1","module_id":"app-core",
                "base_ref":"HEAD","base_commit":commit,"branch":"codex/test","head_commit":commit,
                "initial_clean":true,"final_clean":true,"isolation_mode":"isolated_worktree",
                "scope_hash":scope_hash,"created_at":"2026-01-01T00:00:00Z",
                "bug_triage":{"query_executed":true,"query":"appsdk bug list -q issue-producer-1","mode":"new_confirmed","reopened_from_issue_id":null},
                "bug_triage_query_binding":producer_triage_binding
            },
            "reproduction": {
                "reproduction_id":"caller-reproduction-id","issue_id":"issue-producer-1","module_id":"app-core",
                "worktree_id":"caller-worktree-id","base_commit":commit,"input_hashes":["caller-input"],
                "baseline_evidence_id":"caller-baseline-id","first_divergence":"caller text",
                "result":"reproduced","created_at":"2026-01-01T00:01:00Z"
            },
            "baseline_evidence": {
                "evidence_id":"caller-evidence-id","issue_id":"issue-producer-1","experiment_id":"experiment-producer-1",
                "phase":"baseline_reproduction","kind":"red_test","source_commit":commit,"scope":{"module_id":"app-core"},
                "producer":{"adapter":"test","identity":"lifecycle-producer-test"},"result":"pass",
                "command":{"program":"sh","args":["-c","printf baseline-error-token >&2; exit 1"],"working_directory":".","expected_exit_status":1,"expected_error_token":"baseline-error-token"},
                "created_at":"2026-01-01T00:00:30Z","expires_at":"2099-01-01T00:00:00Z",
                "input_hashes":["input-producer-1"],"scope_hash":scope_hash
            }
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let valid_input: Value =
        serde_json::from_str(&fs::read_to_string(&input_path).unwrap()).unwrap();
    let mut invalid_triage = valid_input.clone();
    invalid_triage["worktree"]["bug_triage"]["mode"] = Value::String("bogus".into());
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&invalid_triage).unwrap() + "\n",
    )
    .unwrap();
    let invalid_triage_result = produce(&input_path);
    assert!(!invalid_triage_result.status.success());
    assert!(
        String::from_utf8_lossy(&invalid_triage_result.stderr).contains("BUG_TRIAGE_MODE_INVALID"),
        "{}",
        String::from_utf8_lossy(&invalid_triage_result.stderr)
    );
    assert!(!root
        .join(".appsdk/records/worktree-record-app-core.json")
        .exists());
    let mut non_boolean_query_flag = valid_input.clone();
    non_boolean_query_flag["worktree"]["bug_triage"]["query_executed"] =
        Value::String("true".into());
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&non_boolean_query_flag).unwrap() + "\n",
    )
    .unwrap();
    let non_boolean_query_result = produce(&input_path);
    assert!(!non_boolean_query_result.status.success());
    assert!(String::from_utf8_lossy(&non_boolean_query_result.stderr)
        .contains("BUG_TRIAGE_QUERY_MISSING"));

    let mut invalid_reopened_source_type = valid_input.clone();
    invalid_reopened_source_type["worktree"]["bug_triage"]["reopened_from_issue_id"] =
        Value::Bool(false);
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&invalid_reopened_source_type).unwrap() + "\n",
    )
    .unwrap();
    let invalid_reopened_source_type_result = produce(&input_path);
    assert!(!invalid_reopened_source_type_result.status.success());
    assert!(
        String::from_utf8_lossy(&invalid_reopened_source_type_result.stderr)
            .contains("BUG_TRIAGE_REOPENED_SOURCE_INVALID")
    );

    let mut forged_query = valid_input.clone();
    forged_query["worktree"]["bug_triage"]["query"] =
        Value::String("appsdk bug list -q prefixissue-producer-1suffix".into());
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&forged_query).unwrap() + "\n",
    )
    .unwrap();
    let forged_query_result = produce(&input_path);
    assert!(!forged_query_result.status.success());
    assert!(
        String::from_utf8_lossy(&forged_query_result.stderr).contains("BUG_TRIAGE_QUERY_UNBOUND")
    );

    let mismatched_bin = root.with_extension("mismatched-bin");
    fs::create_dir_all(&mismatched_bin).unwrap();
    let mismatched_git_bug = mismatched_bin.join("git-bug");
    fs::write(
        &mismatched_git_bug,
        r#"#!/bin/sh
case "$1 $2" in
  "bug show")
    printf '%s\n' '{"human_id":"different-issue","status":"open"}'
    exit 0
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&mismatched_git_bug, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&valid_input).unwrap() + "\n",
    )
    .unwrap();
    let mismatched_result = produce_with(&input_path, &mismatched_git_bug);
    assert!(!mismatched_result.status.success());
    assert!(String::from_utf8_lossy(&mismatched_result.stderr)
        .contains("BUG_TRIAGE_QUERY_IDENTITY_MISMATCH"));

    fs::write(
        &input_path,
        serde_json::to_string_pretty(&valid_input).unwrap() + "\n",
    )
    .unwrap();
    let produced = produce(&input_path);
    assert!(
        produced.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&produced.stdout),
        String::from_utf8_lossy(&produced.stderr)
    );
    assert!(root
        .join(".appsdk/records/worktree-record-app-core.json")
        .is_file());
    assert!(root
        .join(".appsdk/records/reproduction-record-app-core.json")
        .is_file());
    let evidence_files = fs::read_dir(root.join(".appsdk/records/evidence/app-core"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(evidence_files.len(), 1);
    assert!(evidence_files[0].is_file());
    let produced_worktree: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/worktree-record-app-core.json")).unwrap(),
    )
    .unwrap();
    let produced_reproduction: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/reproduction-record-app-core.json"))
            .unwrap(),
    )
    .unwrap();
    let produced_evidence: Value =
        serde_json::from_str(&fs::read_to_string(&evidence_files[0]).unwrap()).unwrap();
    assert_ne!(produced_worktree["worktree_id"], "caller-worktree-id");
    assert_ne!(
        produced_reproduction["reproduction_id"],
        "caller-reproduction-id"
    );
    assert_eq!(
        produced_reproduction["input_hashes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        produced_evidence["producer"],
        serde_json::json!({"adapter":"appsdk","identity":"appsdk-lifecycle-record-producer"})
    );
    let expected_triage_binding = digest(&canonical(&serde_json::json!({
        "issue_id": "issue-producer-1",
        "query": "appsdk bug list -q issue-producer-1",
        "mode": "new_confirmed",
        "reopened_from_issue_id": null
    })));
    assert_eq!(
        produced_worktree["bug_triage_query_binding"],
        expected_triage_binding
    );
    assert_eq!(produced_evidence["exit_status"], 1);
    let repeated = produce(&input_path);
    assert!(!repeated.status.success());
    assert!(
        String::from_utf8_lossy(&repeated.stderr).contains("LIFECYCLE_RECORD_EXISTS"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&repeated.stdout),
        String::from_utf8_lossy(&repeated.stderr)
    );
    fs::remove_file(input_path).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn worktree_schema_requires_triage_only_for_non_legacy_issue_ids() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../contracts/records/worktree-record.schema.json"
    ))
    .unwrap();
    let required = schema["required"].as_array().unwrap();
    assert!(!required.iter().any(|value| value == "bug_triage"));
    assert!(!required
        .iter()
        .any(|value| value == "bug_triage_query_binding"));
    let conditional = schema["allOf"].as_array().unwrap().iter().find(|rule| {
        rule.get("if")
            .and_then(|condition| condition.get("properties"))
            .and_then(|properties| properties.get("issue_id"))
            .and_then(|issue_id| issue_id.get("not"))
            .and_then(|not| not.get("pattern"))
            .and_then(Value::as_str)
            .is_some_and(|pattern| pattern == "^(?:none$|legacy-)")
    });
    let conditional = conditional.expect("missing non-legacy issue conditional");
    let then_required = conditional["then"]["required"].as_array().unwrap();
    assert!(then_required.iter().any(|value| value == "bug_triage"));
    assert!(then_required
        .iter()
        .any(|value| value == "bug_triage_query_binding"));
}

#[test]
fn promotion_schema_requires_authoritative_bug_closure() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../contracts/records/promotion-record.schema.json"
    ))
    .unwrap();
    assert!(schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "bug_closure_verified"));
    assert_eq!(
        schema["properties"]["bug_closure_verified"]["type"],
        "boolean"
    );
}

#[test]
fn lifecycle_record_producer_rejects_dirty_or_mismatched_input_without_records() {
    let root = temp_root("lifecycle-record-producer-reject");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/goal.json"),
        r#"{"goal_id":"goal-1","issue_id":null,"raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#,
    )
    .unwrap();
    init_git(&root);
    let commit = git_test_value(&root, &["rev-parse", "HEAD"]);
    fs::write(root.join("dirty.txt"), "dirty\n").unwrap();
    let input_path = root.with_extension("producer-input.json");
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "goal_id":"wrong-goal",
            "worktree": {"worktree_id":"worktree-1","issue_id":"issue-1","module_id":"app-core","base_ref":"HEAD","base_commit":commit,"branch":"codex/test","head_commit":commit,"initial_clean":true,"final_clean":true,"isolation_mode":"isolated_worktree","scope_hash":"scope-1","created_at":"2026-01-01T00:00:00Z"},
            "reproduction": {"reproduction_id":"reproduction-1","issue_id":"issue-1","module_id":"app-core","worktree_id":"worktree-1","base_commit":commit,"input_hashes":["input-1"],"baseline_evidence_id":"baseline-1","first_divergence":"test","result":"reproduced","created_at":"2026-01-01T00:01:00Z"},
            "baseline_evidence": {"evidence_id":"baseline-1","issue_id":"issue-1","experiment_id":"experiment-1","phase":"baseline_reproduction","kind":"red_test","source_commit":commit,"scope":{"module_id":"app-core"},"producer":{"adapter":"test","identity":"test"},"command":{"program":"sh","args":["-c","exit 1"],"working_directory":".","expected_exit_status":1},"input_hashes":["input-1"],"scope_hash":"scope-1"}
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("PRODUCER_GOAL_MISMATCH"));
    assert!(!root
        .join(".appsdk/records/worktree-record-app-core.json")
        .exists());
    fs::remove_file(input_path).unwrap();
    fs::remove_file(root.join("dirty.txt")).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn initialized_lock_is_not_bound_to_the_running_binary() {
    let root = temp_root("unbound-sdk-lock");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/goal.json"),
        r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#,
    )
    .unwrap();
    for stage in ["source_implemented", "contract_bound"] {
        assert!(run(&["promote", root_text, "--to", stage]).status.success());
    }

    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["digest"] = Value::String(format!("sha256:{}", "a".repeat(64)));
    lock["compiler_digest"] = Value::String(format!("sha256:{}", "b".repeat(64)));
    lock["binary_ref"] = Value::String("historical-binary-witness".into());
    fs::write(
        &lock_path,
        serde_json::to_string_pretty(&lock).unwrap() + "\n",
    )
    .unwrap();

    let compile = run(&["compile", root_text]);
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn initialized_lock_rejects_wrong_version_schema_and_malformed_bundle_digest() {
    for (name, mutate, expected) in [
        ("wrong-version", "version", "INVALID_SDK_LOCK"),
        ("wrong-schema", "contract_schema", "INVALID_SDK_LOCK"),
        (
            "malformed-bundle",
            "bundle_digest",
            "INVALID_SDK_BUNDLE_DIGEST",
        ),
    ] {
        let root = temp_root(name);
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        let lock_path = root.join(".appsdk/sdk.lock");
        let mut lock: Value =
            serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
        match mutate {
            "version" => lock["version"] = Value::String("0.1.5".into()),
            "contract_schema" => lock["contract_schema"] = Value::from(2),
            "bundle_digest" => lock["bundle_digest"] = Value::String("sha256:not-a-digest".into()),
            _ => unreachable!(),
        }
        fs::write(
            &lock_path,
            serde_json::to_string_pretty(&lock).unwrap() + "\n",
        )
        .unwrap();
        let verified = run(&["verify", root_text]);
        assert!(!verified.status.success());
        assert!(
            String::from_utf8_lossy(&verified.stderr).contains(expected),
            "stderr={}",
            String::from_utf8_lossy(&verified.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }
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
fn compile_module_option_does_not_build_unrelated_modules() {
    let root = temp_root("module-scoped-compile");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let goal_file = root.join(".appsdk/goal.json");
    let mut goal: Value = serde_json::from_str(&fs::read_to_string(&goal_file).unwrap()).unwrap();
    goal["status"] = serde_json::json!("confirmed");
    goal["confirmed_by"] = serde_json::json!("test");
    goal["confirmed_at"] = serde_json::json!("2026-01-01T00:00:00Z");
    fs::write(&goal_file, serde_json::to_string_pretty(&goal).unwrap()).unwrap();

    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    let mut android_probe = project["modules"][0].clone();
    android_probe["module_id"] = Value::from("android-probe");
    android_probe["source_owner"] = Value::from("android-probe");
    android_probe["owned_paths"] = serde_json::json!(["playground/android-probe/**"]);
    android_probe["active_artifact"] = Value::from("active/lib/android-probe/**");
    android_probe["build"]["args"] =
        serde_json::json!(["-c", "touch android-probe-was-built && exit 42"]);
    android_probe["artifact_paths"] = serde_json::json!(["android-probe.placeholder"]);

    project["modules"][0]["module_id"] = Value::from("client-connection");
    project["modules"][0]["source_owner"] = Value::from("client-connection");
    project["modules"][0]["active_artifact"] = Value::from("active/lib/client-connection/**");
    project["modules"][0]["build"]["args"] = serde_json::json!([
        "-c",
        "mkdir -p generated/modules/client-connection/lib && printf client > generated/modules/client-connection/lib/client.placeholder"
    ]);
    project["modules"][0]["artifact_paths"] = serde_json::json!(["client.placeholder"]);
    project["modules"]
        .as_array_mut()
        .unwrap()
        .push(android_probe);
    fs::create_dir_all(root.join("playground/android-probe")).unwrap();
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap(),
    )
    .unwrap();

    let compiled = run(&["compile", root_text, "--module", "client-connection"]);
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    assert!(root
        .join("generated/modules/client-connection/module.compiled.json")
        .is_file());
    let review = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "client-connection",
    ]);
    assert!(!review.status.success());
    assert!(
        String::from_utf8_lossy(&review.stderr).contains("REVIEW_ADMISSION_BLOCKED"),
        "{}",
        String::from_utf8_lossy(&review.stderr)
    );
    assert!(!root.join("android-probe-was-built").exists());
    assert!(!root
        .join("generated/modules/android-probe/module.compiled.json")
        .exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn review_preflight_honors_empty_deployment_operations() {
    for (name, operations, requires_deployment) in [
        ("explicit-empty", Some(serde_json::json!([])), false),
        ("legacy-omitted", None, true),
    ] {
        let root = temp_root(&format!("review-preflight-{name}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());

        let project_file = root.join(".appsdk/project.json");
        let mut project: Value =
            serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
        if let Some(operations) = operations {
            project["modules"][0]["deployment_operations"] = operations;
        }
        fs::write(
            &project_file,
            serde_json::to_string_pretty(&project).unwrap(),
        )
        .unwrap();

        let goal_file = root.join(".appsdk/goal.json");
        let mut goal: Value =
            serde_json::from_str(&fs::read_to_string(&goal_file).unwrap()).unwrap();
        goal["status"] = serde_json::json!("confirmed");
        goal["confirmed_by"] = serde_json::json!("test");
        goal["confirmed_at"] = serde_json::json!("2026-01-01T00:00:00Z");
        fs::write(&goal_file, serde_json::to_string_pretty(&goal).unwrap()).unwrap();

        assert!(run(&["compile", root_text, "--module", "app-core"])
            .status
            .success());
        let admission = run(&[
            "verify",
            "--review-admission",
            root_text,
            "--module",
            "app-core",
        ]);
        assert!(!admission.status.success());
        let stderr = String::from_utf8_lossy(&admission.stderr);
        assert!(stderr.contains("REVIEW_ADMISSION_BLOCKED"), "{stderr}");
        assert_eq!(
            stderr.contains("\"kind\": \"deployment_install\""),
            requires_deployment,
            "{stderr}"
        );
        assert_eq!(
            stderr.contains("\"kind\": \"deployment_restart\""),
            requires_deployment,
            "{stderr}"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn review_requires_only_declared_deployment_operations_and_binds_the_contract() {
    for operations in [serde_json::json!([]), serde_json::json!(["install"])] {
        let root = temp_root("deployment-applicability");
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        let project_file = root.join(".appsdk/project.json");
        let mut project: Value =
            serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
        project["modules"][0]["deployment_operations"] = operations.clone();
        fs::write(
            &project_file,
            serde_json::to_string_pretty(&project).unwrap(),
        )
        .unwrap();
        init_git(&root);
        let goal_file = root.join(".appsdk/goal.json");
        let mut goal: Value =
            serde_json::from_str(&fs::read_to_string(&goal_file).unwrap()).unwrap();
        goal["status"] = serde_json::json!("confirmed");
        goal["confirmed_by"] = serde_json::json!("test");
        goal["confirmed_at"] = serde_json::json!("2026-01-01T00:00:00Z");
        fs::write(&goal_file, serde_json::to_string_pretty(&goal).unwrap()).unwrap();
        pin_test_lock(root_text);
        for stage in ["source_implemented", "contract_bound"] {
            assert!(run(&["promote", root_text, "--to", stage]).status.success());
        }
        assert!(run(&["compile", root_text]).status.success());
        let artifact: Value = serde_json::from_str(
            &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json"))
                .unwrap(),
        )
        .unwrap();
        write_records(
            &root,
            "app-core",
            artifact["artifact_hash"].as_str().unwrap(),
            false,
            "issue-1",
        );
        let validation_file =
            root.join(".appsdk/records/pre-review-validation-record-app-core.json");
        let mut validation: Value =
            serde_json::from_str(&fs::read_to_string(&validation_file).unwrap()).unwrap();
        validation["deployment"]
            .as_object_mut()
            .unwrap()
            .remove("restart_receipt_id");
        if operations.as_array().unwrap().is_empty() {
            validation["deployment"]
                .as_object_mut()
                .unwrap()
                .remove("install_receipt_id");
        }
        fs::write(
            &validation_file,
            serde_json::to_string_pretty(&validation).unwrap(),
        )
        .unwrap();
        let admission = run(&[
            "verify",
            "--review-admission",
            root_text,
            "--module",
            "app-core",
        ]);
        assert!(
            admission.status.success(),
            "{}",
            String::from_utf8_lossy(&admission.stderr)
        );
        // A missing required blackbox must still fail even with no service receipts.
        fs::remove_file(root.join(".appsdk/records/evidence/app-core/blackbox-1.json")).unwrap();
        assert!(!run(&[
            "verify",
            "--review-admission",
            root_text,
            "--module",
            "app-core"
        ])
        .status
        .success());
        write_records(
            &root,
            "app-core",
            artifact["artifact_hash"].as_str().unwrap(),
            false,
            "issue-1",
        );
        let mut drifted: Value =
            serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
        drifted["modules"][0]["deployment_operations"] = serde_json::json!(["install", "restart"]);
        fs::write(
            &project_file,
            serde_json::to_string_pretty(&drifted).unwrap(),
        )
        .unwrap();
        assert!(
            !run(&[
                "verify",
                "--review-admission",
                root_text,
                "--module",
                "app-core"
            ])
            .status
            .success(),
            "changing verification applicability invalidates the candidate"
        );
        fs::remove_dir_all(root).unwrap();
    }
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
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
    let review_file = root.join(".appsdk/records/review-record-app-core.json");
    fs::remove_file(&review_file).unwrap();
    let review_admission = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(
        review_admission.status.success(),
        "{}",
        String::from_utf8_lossy(&review_admission.stderr)
    );
    let source_drift_file = root.join("playground/experiments/review-admission-drift.txt");
    fs::write(&source_drift_file, "drift\n").unwrap();
    let source_drift = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!source_drift.status.success());
    assert!(
        String::from_utf8_lossy(&source_drift.stderr).contains("CANDIDATE_CONTROLLED_SOURCE_DRIFT")
    );
    fs::remove_file(&source_drift_file).unwrap();
    let artifact_file = root.join("generated/modules/app-core/lib/app-core.placeholder");
    let artifact_content = fs::read(&artifact_file).unwrap();
    fs::write(&artifact_file, "stale artifact\n").unwrap();
    let stale_artifact = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!stale_artifact.status.success());
    assert!(String::from_utf8_lossy(&stale_artifact.stderr)
        .contains("REVIEW_ADMISSION_ARTIFACT_SOURCE_DRIFT"));
    fs::write(&artifact_file, artifact_content).unwrap();
    let candidate_file = root.join(".appsdk/records/fix-candidate-record-app-core.json");
    let mut wrong_candidate_tree: Value =
        serde_json::from_str(&fs::read_to_string(&candidate_file).unwrap()).unwrap();
    wrong_candidate_tree["tree_hash"] =
        Value::String("0000000000000000000000000000000000000000".into());
    fs::write(
        &candidate_file,
        serde_json::to_string_pretty(&wrong_candidate_tree).unwrap() + "\n",
    )
    .unwrap();
    let wrong_tree = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!wrong_tree.status.success());
    assert!(String::from_utf8_lossy(&wrong_tree.stderr).contains("FIX_CANDIDATE_TREE_MISMATCH"));
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
    let restart_receipt_file = root.join(".appsdk/records/evidence/app-core/restart-1.json");
    let mut wrong_restart_producer: Value =
        serde_json::from_str(&fs::read_to_string(&restart_receipt_file).unwrap()).unwrap();
    wrong_restart_producer["producer"]["adapter"] = Value::String("forged-adapter".into());
    fs::write(
        &restart_receipt_file,
        serde_json::to_string_pretty(&wrong_restart_producer).unwrap() + "\n",
    )
    .unwrap();
    let forged_restart_receipt = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!forged_restart_receipt.status.success());
    assert!(String::from_utf8_lossy(&forged_restart_receipt.stderr)
        .contains("DEPLOYMENT_RECEIPT_EVIDENCE_MISMATCH"));
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
    let whitebox_file = root.join(".appsdk/records/evidence/app-core/whitebox-1.json");
    let mut forged_whitebox_producer: Value =
        serde_json::from_str(&fs::read_to_string(&whitebox_file).unwrap()).unwrap();
    forged_whitebox_producer["producer"]["adapter"] = Value::String("forged-adapter".into());
    fs::write(
        &whitebox_file,
        serde_json::to_string_pretty(&forged_whitebox_producer).unwrap() + "\n",
    )
    .unwrap();
    let forged_whitebox = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!forged_whitebox.status.success());
    assert!(String::from_utf8_lossy(&forged_whitebox.stderr)
        .contains("DEVELOPMENT_WHITEBOX_EVIDENCE_MISMATCH"));
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
    fs::remove_file(&restart_receipt_file).unwrap();
    let missing_restart_receipt = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!missing_restart_receipt.status.success());
    assert!(String::from_utf8_lossy(&missing_restart_receipt.stderr)
        .contains("MISSING_EVIDENCE_RECORD:restart-1"));
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
    let mut late_restart_receipt: Value =
        serde_json::from_str(&fs::read_to_string(&restart_receipt_file).unwrap()).unwrap();
    late_restart_receipt["created_at"] = Value::String("2026-01-01T00:03:35Z".into());
    fs::write(
        &restart_receipt_file,
        serde_json::to_string_pretty(&late_restart_receipt).unwrap() + "\n",
    )
    .unwrap();
    let invalid_causal_order = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!invalid_causal_order.status.success());
    assert!(String::from_utf8_lossy(&invalid_causal_order.stderr)
        .contains("PRE_REVIEW_CAUSAL_ORDER_MISMATCH"));
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
    let blackbox_file = root.join(".appsdk/records/evidence/app-core/blackbox-1.json");
    let mut expired_blackbox: Value =
        serde_json::from_str(&fs::read_to_string(&blackbox_file).unwrap()).unwrap();
    expired_blackbox["expires_at"] = Value::String("2026-01-02T00:00:00Z".into());
    fs::write(
        &blackbox_file,
        serde_json::to_string_pretty(&expired_blackbox).unwrap() + "\n",
    )
    .unwrap();
    let expired_evidence = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!expired_evidence.status.success());
    assert!(String::from_utf8_lossy(&expired_evidence.stderr)
        .contains("EXPIRED_EVIDENCE_RECORD:blackbox-1"));
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
    fs::remove_file(&blackbox_file).unwrap();
    let missing_deployed_blackbox = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!missing_deployed_blackbox.status.success());
    assert!(!String::from_utf8_lossy(&missing_deployed_blackbox.stdout).contains("\"ok\":true"));
    assert!(String::from_utf8_lossy(&missing_deployed_blackbox.stderr)
        .contains("MISSING_EVIDENCE_RECORD:blackbox-1"));
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
    let validation_file = root.join(".appsdk/records/pre-review-validation-record-app-core.json");
    fs::remove_file(&validation_file).unwrap();
    let missing_blackbox_gate = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ]);
    assert!(!missing_blackbox_gate.status.success());
    assert!(String::from_utf8_lossy(&missing_blackbox_gate.stderr)
        .contains("MISSING_RECORD:pre-review-validation-record-app-core.json"));
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
    let mut relabeled_blackbox: Value =
        serde_json::from_str(&fs::read_to_string(&blackbox_file).unwrap()).unwrap();
    relabeled_blackbox["execution_surface"] = Value::String("development_whitebox".into());
    fs::write(
        &blackbox_file,
        serde_json::to_string_pretty(&relabeled_blackbox).unwrap() + "\n",
    )
    .unwrap();
    let relabeled_blackbox_gate = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ]);
    assert!(!relabeled_blackbox_gate.status.success());
    assert!(String::from_utf8_lossy(&relabeled_blackbox_gate.stderr)
        .contains("PRE_REVIEW_EVIDENCE_MISMATCH"));
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
    let mut wrong_artifact_blackbox: Value =
        serde_json::from_str(&fs::read_to_string(&blackbox_file).unwrap()).unwrap();
    wrong_artifact_blackbox["artifact_hash"] = Value::String("sha256:wrong-artifact".into());
    fs::write(
        &blackbox_file,
        serde_json::to_string_pretty(&wrong_artifact_blackbox).unwrap() + "\n",
    )
    .unwrap();
    let wrong_artifact_gate = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ]);
    assert!(!wrong_artifact_gate.status.success());
    assert!(String::from_utf8_lossy(&wrong_artifact_gate.stderr)
        .contains("DEPLOYED_BLACKBOX_EVIDENCE_MISMATCH"));
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
    fs::write(&source_drift_file, "drift after admission\n").unwrap();
    let promotion_source_drift = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ]);
    assert!(!promotion_source_drift.status.success());
    assert!(String::from_utf8_lossy(&promotion_source_drift.stderr)
        .contains("CANDIDATE_CONTROLLED_SOURCE_DRIFT"));
    fs::remove_file(&source_drift_file).unwrap();
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
    let worktree_file = root.join(".appsdk/records/worktree-record-app-core.json");
    let mut forged_binding: Value =
        serde_json::from_str(&fs::read_to_string(&worktree_file).unwrap()).unwrap();
    forged_binding["bug_triage_query_binding"] = Value::String("sha256:forged".into());
    fs::write(
        &worktree_file,
        serde_json::to_string_pretty(&forged_binding).unwrap() + "\n",
    )
    .unwrap();
    let forged_binding_result = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ]);
    assert!(!forged_binding_result.status.success());
    assert!(String::from_utf8_lossy(&forged_binding_result.stderr)
        .contains("BUG_TRIAGE_QUERY_BINDING_MISMATCH"));
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
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
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
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
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
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
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
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
    let reuse_file = root.join(".appsdk/records/effectiveness-record-app-core.json");
    let mut reused: Value =
        serde_json::from_str(&fs::read_to_string(&reuse_file).unwrap()).unwrap();
    reused["positive_evidence_ids"] = serde_json::json!(["positive-1"]);
    reused["negative_evidence_ids"] = serde_json::json!(["negative-1"]);
    reused["fixed_replay_evidence_id"] = serde_json::json!("blackbox-1");
    reused["blackbox_evidence_ids"] = serde_json::json!(["blackbox-1"]);
    fs::write(&reuse_file, serde_json::to_string_pretty(&reused).unwrap()).unwrap();
    let reuse_result = run(&["verify", root_text]);
    assert!(
        reuse_result.status.success(),
        "{}",
        String::from_utf8_lossy(&reuse_result.stderr)
    );
    let positive_file = root.join(".appsdk/records/evidence/app-core/positive-1.json");
    let mut stale_positive: Value =
        serde_json::from_str(&fs::read_to_string(&positive_file).unwrap()).unwrap();
    stale_positive["input_hashes"] = serde_json::json!(["unrelated-input"]);
    fs::write(
        &positive_file,
        serde_json::to_string_pretty(&stale_positive).unwrap(),
    )
    .unwrap();
    assert!(
        !run(&["verify", root_text]).status.success(),
        "reuse must preserve reproduction input identity"
    );
    write_records(&root, "app-core", &architecture_hash, true, "issue-1");
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
    write_records(&root, "app-core", &architecture_hash, true, "issue-1");
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
    write_records(&root, "app-core", &architecture_hash, true, "issue-1");
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
    write_records(&root, "app-core", &architecture_hash, false, "issue-1");
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
    write_records(&root, "app-core", &architecture_hash, true, "issue-1");
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
    write_records(&root, "app-edge", &edge_hash, false, "issue-1");
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
fn rehydrate_frozen_rebuilds_fresh_checkout_projections() {
    let root = temp_root("begin-version");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    enable_regression_contract(&root);
    init_git(&root);
    fs::write(root.join(".appsdk/goal.json"), r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
    "#).unwrap();
    pin_test_lock(root_text);
    fs::remove_dir_all(root.join(".appsdk/migrations/0.1.5-to-0.1.6")).unwrap();
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
    write_records(&root, "app-core", &v1_hash, false, "issue-1");
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
    write_records(&root, "app-core", &v1_hash, true, "issue-1");
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

    install_legacy_governance_maps(&root);
    let review_file = root.join(".appsdk/records/review-record-app-core.json");
    let mut review: Value =
        serde_json::from_str(&fs::read_to_string(&review_file).unwrap()).unwrap();
    review["resource_map_hash"] = Value::String(format!("sha256:{}", "0".repeat(64)));
    review["function_map_hash"] = Value::String(
        "sha256:69f16dfe5d056634f6164cd325dbdbdf134588890b69f101c0d9045e6a01d776".into(),
    );
    review["mainline_call_map_hash"] = Value::String(
        "sha256:c36e4f9d5cff527d7f98b339772a53c87f420db58bb2215b6b68210e01124673".into(),
    );
    review["verification_map_hash"] = Value::String(
        "sha256:8dcc1e9444f62f7e407fa1e37a6c8d004de55587950f8e600dc5f3c711981230".into(),
    );
    fs::write(
        &review_file,
        serde_json::to_string_pretty(&review).unwrap() + "\n",
    )
    .unwrap();
    let project_file = root.join(".appsdk/project.json");
    let mut legacy_project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    legacy_project["sdk"]["version"] = Value::String("0.1.5".into());
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&legacy_project).unwrap() + "\n",
    )
    .unwrap();
    let lock_file = root.join(".appsdk/sdk.lock");
    let mut legacy_lock: Value =
        serde_json::from_str(&fs::read_to_string(&lock_file).unwrap()).unwrap();
    legacy_lock["version"] = Value::String("0.1.5".into());
    fs::write(
        &lock_file,
        serde_json::to_string_pretty(&legacy_lock).unwrap() + "\n",
    )
    .unwrap();
    let review_mismatch = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!review_mismatch.status.success());
    assert!(String::from_utf8_lossy(&review_mismatch.stderr)
        .contains("SDK_MIGRATION_FROZEN_REVIEW_MAP_MISMATCH:app-core:resource-map.json"));
    assert!(!root.join(".appsdk/migrations/0.1.5-to-0.1.6").exists());
    review["resource_map_hash"] = Value::String(
        "sha256:67f189bf15330e542bc82349b78a1d7e29ec050b112ecffa165173b078d9204e".into(),
    );
    fs::write(
        &review_file,
        serde_json::to_string_pretty(&review).unwrap() + "\n",
    )
    .unwrap();
    let migrated = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        migrated.status.success(),
        "{}",
        String::from_utf8_lossy(&migrated.stderr)
    );
    let historical_review_verified = run(&["verify", root_text]);
    assert!(
        historical_review_verified.status.success(),
        "{}",
        String::from_utf8_lossy(&historical_review_verified.stderr)
    );
    let frozen_commit = String::from_utf8_lossy(
        &Command::new("git")
            .args(["-C", root_text, "rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();
    let merge_file = root.join(".appsdk/records/merge-record-app-core.json");
    let mut merge: Value = serde_json::from_str(&fs::read_to_string(&merge_file).unwrap()).unwrap();
    merge["mainline_ref"] = Value::String("release/frozen-v1".into());
    fs::write(
        &merge_file,
        serde_json::to_string_pretty(&merge).unwrap() + "\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args([
            "-C",
            root_text,
            "update-ref",
            "refs/remotes/origin/release/frozen-v1",
            &frozen_commit,
        ])
        .status()
        .unwrap()
        .success());

    fs::remove_dir_all(root.join("generated")).unwrap();
    fs::remove_dir_all(root.join("active")).unwrap();
    fs::remove_dir_all(root.join("protected/history")).unwrap();
    let blocked = run(&[
        "begin-version",
        root_text,
        "--module",
        "app-core",
        "--from",
        "active-v1",
        "--to",
        "active-v2",
    ]);
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("ACTIVE_INDEX_MISSING"));

    let gitignore = root.join(".gitignore");
    let original_gitignore = fs::read_to_string(&gitignore).unwrap();
    fs::write(&gitignore, format!("{}protected/\n", original_gitignore)).unwrap();
    let ignored = run(&["rehydrate-frozen", root_text, "--module", "app-core"]);
    assert!(!ignored.status.success());
    assert!(String::from_utf8_lossy(&ignored.stderr).contains("PROTECTED_ARCHIVE_IGNORED"));
    assert!(!root.join("generated/modules/app-core").exists());
    fs::write(&gitignore, original_gitignore).unwrap();

    let drift = root.join("playground/experiments/rehydrate-drift.txt");
    fs::write(&drift, "committed source drift\n").unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "source drift"])
        .status()
        .unwrap()
        .success());
    let rejected = run(&["rehydrate-frozen", root_text, "--module", "app-core"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("FROZEN_REHYDRATE_ARTIFACT_HASH_MISMATCH"));
    assert!(!root.join("protected/history/app-core").exists());
    assert!(!root.join("active/lib/app-core").exists());

    fs::remove_file(drift).unwrap();
    fs::remove_dir_all(root.join("generated")).unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "restore frozen source"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            root_text,
            "update-ref",
            "refs/remotes/backup/release/frozen-v1",
            &frozen_commit,
        ])
        .status()
        .unwrap()
        .success());
    let ambiguous = run(&["rehydrate-frozen", root_text, "--module", "app-core"]);
    assert!(!ambiguous.status.success());
    assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("MAINLINE_REF_AMBIGUOUS"));
    assert!(!root.join("protected/history/app-core").exists());
    assert!(!root.join("active/lib/app-core").exists());
    assert!(Command::new("git")
        .args([
            "-C",
            root_text,
            "update-ref",
            "-d",
            "refs/remotes/backup/release/frozen-v1",
        ])
        .status()
        .unwrap()
        .success());

    let unowned_active = root.join("active/lib/app-core/active-v1");
    fs::create_dir_all(&unowned_active).unwrap();
    fs::write(unowned_active.join("artifact.json"), "{}\n").unwrap();
    let unowned = run(&["rehydrate-frozen", root_text, "--module", "app-core"]);
    assert!(!unowned.status.success());
    assert!(String::from_utf8_lossy(&unowned.stderr)
        .contains("FROZEN_REHYDRATE_UNOWNED_PARTIAL_PROJECTION"));
    fs::remove_dir_all(root.join("active")).unwrap();

    // Historical frozen records predate the current delivery triage contract.
    // Rehydration must validate their immutable publication graph without
    // requiring a newly invented delivery record field.
    let worktree_record = root.join(".appsdk/records/worktree-record-app-core.json");
    let mut historical_worktree: Value =
        serde_json::from_str(&fs::read_to_string(&worktree_record).unwrap()).unwrap();
    historical_worktree
        .as_object_mut()
        .unwrap()
        .remove("bug_triage");
    historical_worktree
        .as_object_mut()
        .unwrap()
        .remove("bug_triage_query_binding");
    fs::write(
        &worktree_record,
        serde_json::to_string_pretty(&historical_worktree).unwrap() + "\n",
    )
    .unwrap();

    let restored = run(&["rehydrate-frozen", root_text, "--module", "app-core"]);
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    assert_eq!(fs::read_to_string(&active_v1).unwrap(), active_v1_text);
    assert!(root
        .join("protected/history/app-core/module-artifact.json")
        .is_file());
    assert!(root
        .join("generated/modules/app-core/module.compiled.json")
        .is_file());
    let rehydrated_verify = run(&["verify", root_text]);
    assert!(
        rehydrated_verify.status.success(),
        "{}",
        String::from_utf8_lossy(&rehydrated_verify.stderr)
    );
    let transaction = root.join(".appsdk/transactions/rehydrate-app-core");
    assert!(!transaction.exists());
    fs::create_dir_all(&transaction).unwrap();
    fs::write(
        transaction.join("marker.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "module_id": "app-core",
            "version": "active-v1",
            "artifact_hash": v1_hash,
            "phase": "active_published",
            "created_at": "2026-01-02T00:00:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let resumed = run(&["rehydrate-frozen", root_text, "--module", "app-core"]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert!(!transaction.exists());

    fs::create_dir_all(&transaction).unwrap();
    fs::write(
        transaction.join("marker.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "module_id": "app-core",
            "version": "active-v1",
            "artifact_hash": format!("sha256:{}", "0".repeat(64)),
            "phase": "active_published",
            "created_at": "2026-01-02T00:00:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let mismatched_transaction = run(&["rehydrate-frozen", root_text, "--module", "app-core"]);
    assert!(!mismatched_transaction.status.success());
    assert!(String::from_utf8_lossy(&mismatched_transaction.stderr)
        .contains("FROZEN_REHYDRATE_TRANSACTION_MISMATCH"));
    fs::remove_dir_all(&transaction).unwrap();

    let duplicate = run(&["rehydrate-frozen", root_text, "--module", "app-core"]);
    assert!(
        duplicate.status.success(),
        "{}",
        String::from_utf8_lossy(&duplicate.stderr)
    );

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
    let active_v2 = root.join("active/lib/app-core/active-v2/artifact.json");
    let active_v2_text = fs::read_to_string(&active_v2).unwrap();
    fs::remove_dir_all(root.join("generated")).unwrap();
    fs::remove_dir_all(root.join("active")).unwrap();
    fs::remove_dir_all(root.join("protected/history")).unwrap();
    let restored_v2 = run(&["rehydrate-frozen", root_text, "--module", "app-core"]);
    assert!(
        restored_v2.status.success(),
        "{}",
        String::from_utf8_lossy(&restored_v2.stderr)
    );
    assert_eq!(fs::read_to_string(&active_v1).unwrap(), active_v1_text);
    assert_eq!(fs::read_to_string(&active_v2).unwrap(), active_v2_text);
    let restored_v2_verify = run(&["verify", root_text]);
    assert!(
        restored_v2_verify.status.success(),
        "{}",
        String::from_utf8_lossy(&restored_v2_verify.stderr)
    );
    let previous_archive_artifact =
        root.join("protected/history-versions/app-core/active-v1/module-artifact.json");
    let mut previous_archive: Value =
        serde_json::from_str(&fs::read_to_string(&previous_archive_artifact).unwrap()).unwrap();
    previous_archive["artifact_paths"] = serde_json::json!(["legacy/app-core.placeholder"]);
    fs::write(
        &previous_archive_artifact,
        serde_json::to_string_pretty(&previous_archive).unwrap() + "\n",
    )
    .unwrap();
    let idempotent_v2 = run(&["rehydrate-frozen", root_text, "--module", "app-core"]);
    assert!(
        idempotent_v2.status.success(),
        "{}",
        String::from_utf8_lossy(&idempotent_v2.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn existing_governance_without_guide_gets_read_only_setup_proposal() {
    let root = temp_root("guidance-existing-bootstrap");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    fs::write(
        root.join("AGENTS.md"),
        "# Project rules\n\nUse the project build and deployed smoke commands.\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("skills/local-development")).unwrap();
    fs::write(
        root.join("skills/local-development/SKILL.md"),
        "---\nname: local-development\ndescription: Project-local development procedure.\n---\n",
    )
    .unwrap();
    fs::write(
        root.join("protected/history/legacy.txt"),
        "legacy protected truth\n",
    )
    .unwrap();
    let legacy_protected = fs::read(root.join("protected/history/legacy.txt")).unwrap();
    fs::create_dir_all(root.join("skills/nested/child")).unwrap();
    fs::write(
        root.join("skills/nested/child/SKILL.md"),
        "---\nname: nested\ndescription: Must not be discovered recursively.\n---\n",
    )
    .unwrap();

    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project.as_object_mut().unwrap().remove("guidance");
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let legacy_project = fs::read(&project_file).unwrap();
    let initialized = run(&["init", root_text]);
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let initialized_stdout = String::from_utf8_lossy(&initialized.stdout);
    assert!(initialized_stdout.contains("appsdk guide init"));
    assert!(initialized_stdout.contains("--mode bootstrap"));
    assert!(!initialized_stdout.contains("next appsdk guide compile"));
    assert_eq!(fs::read(&project_file).unwrap(), legacy_project);
    assert_eq!(
        fs::read(root.join("protected/history/legacy.txt")).unwrap(),
        legacy_protected
    );

    let status = run(&["guide", "status", root_text]);
    assert!(status.status.success());
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["reason_code"], "GUIDANCE_SETUP_REQUIRED");
    assert!(status_json["next"]["command"]
        .as_str()
        .unwrap()
        .contains("--mode bootstrap"));

    let intake = run(&[
        "guide",
        "init",
        root_text,
        "--task",
        "guidance-setup",
        "--mode",
        "bootstrap",
        "--module",
        "app-core",
    ]);
    assert!(
        intake.status.success(),
        "{}",
        String::from_utf8_lossy(&intake.stderr)
    );
    let intake_json: Value = serde_json::from_slice(&intake.stdout).unwrap();
    assert_eq!(
        intake_json["reason_code"],
        "GUIDANCE_SETUP_PROPOSAL_REQUIRED"
    );
    assert_eq!(intake_json["readiness"], "needs_user_approval");
    assert_eq!(intake_json["writes_state"], false);
    assert_eq!(intake_json["existing_governance"]["preserved"], true);
    assert!(intake_json["read_first"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source["path"] == "AGENTS.md"));
    assert!(intake_json["read_first"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source["path"] == "skills/local-development/SKILL.md"));
    assert!(intake_json["read_first"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| { source["path"] == ".appsdk/skills/appsdk-project-governance/SKILL.md" }));
    assert!(!intake_json["read_first"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source["path"] == "skills/nested/child/SKILL.md"));
    assert_eq!(
        intake_json["proposal_schema"]["proposal_type"],
        "GuidanceSetupProposal"
    );
    assert_eq!(
        intake_json["skill_commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|skill| skill["command"] == "$appsdk-project-governance")
            .count(),
        1
    );
    assert!(intake_json["questions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|question| question["question_id"] == "project_commands"));
    assert!(intake_json["after_user_approval"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|command| command == "appsdk guide compile"));
    assert!(!root
        .join(".appsdk-control/guidance/guidance-setup")
        .exists());
    assert_eq!(fs::read(&project_file).unwrap(), legacy_project);
    assert_eq!(
        fs::read(root.join("protected/history/legacy.txt")).unwrap(),
        legacy_protected
    );

    let mut approved_project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    approved_project["guidance"] = serde_json::json!({
        "enforcement": "advisory",
        "compiled_manifest": ".appsdk/guidance/compiled.json",
        "rule_sources": [
            {"source_id":"project-agents","kind":"agents","path":"AGENTS.md","required":false,"precedence":100},
            {"source_id":"appsdk-governance-skill","kind":"skill","path":".appsdk/skills/appsdk-project-governance/SKILL.md","contract_path":".appsdk/skills/appsdk-project-governance/appsdk-guidance.json","required":true,"precedence":200},
            {"source_id":"local-development","kind":"skill","path":"skills/local-development/SKILL.md","required":false,"precedence":300}
        ]
    });
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&approved_project).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&["guide", "compile", root_text]).status.success());
    assert!(run(&["verify", root_text]).status.success());
    let task_intake = run(&[
        "guide",
        "init",
        root_text,
        "--task",
        "feature-1",
        "--mode",
        "develop",
        "--module",
        "app-core",
    ]);
    assert!(task_intake.status.success());
    let task_intake_json: Value = serde_json::from_slice(&task_intake.stdout).unwrap();
    assert_eq!(task_intake_json["reason_code"], "GUIDANCE_INTAKE_REQUIRED");
    assert!(task_intake_json["skill_commands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|skill| skill["command"] == "$local-development"));
    assert_eq!(
        fs::read(root.join("protected/history/legacy.txt")).unwrap(),
        legacy_protected
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_compile_is_deterministic_and_optional_for_existing_commands() {
    let left = temp_root("guidance-compile-left");
    let right = temp_root("guidance-compile-right");
    let left_text = left.to_str().unwrap();
    let right_text = right.to_str().unwrap();
    assert!(run(&["new", left_text]).status.success());
    assert!(run(&["new", right_text]).status.success());

    assert!(run(&["verify", left_text]).status.success());
    let before = run(&["guide", "status", left_text]);
    assert!(before.status.success());
    let before_json: Value = serde_json::from_slice(&before.stdout).unwrap();
    assert_eq!(before_json["reason_code"], "GUIDANCE_NOT_COMPILED");
    assert_eq!(before_json["next"]["command"], "appsdk guide compile");
    assert_eq!(before_json["guide_flow_required"], false);
    assert!(before_json["next"]["then"]
        .as_str()
        .unwrap()
        .contains("appsdk guide init"));

    assert!(run(&["guide", "compile", left_text]).status.success());
    assert!(run(&["guide", "compile", right_text]).status.success());
    assert_eq!(
        fs::read(left.join(".appsdk/guidance/compiled.json")).unwrap(),
        fs::read(right.join(".appsdk/guidance/compiled.json")).unwrap()
    );

    let status = run(&["guide", "develop", left_text, "--module", "app-core"]);
    assert!(status.status.success());
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["domain"], "develop");
    assert_eq!(
        status_json["lifecycle"]["module_stage"],
        "source_implemented"
    );
    assert_eq!(status_json["next"]["node_id"], "requirements");
    assert_eq!(status_json["enforcement"], "advisory");
    assert_eq!(status_json["guide_flow_required"], false);
    let init = run(&[
        "guide", "init", left_text, "--task", "optional", "--mode", "develop", "--module",
        "app-core",
    ]);
    assert!(init.status.success());
    let init_json: Value = serde_json::from_slice(&init.stdout).unwrap();
    assert_eq!(init_json["guide_flow_required"], false);
    let close = run(&["guide", "close", left_text, "--task", "optional"]);
    assert!(close.status.success());
    let closed: Value = serde_json::from_slice(&close.stdout).unwrap();
    assert_eq!(closed["cleanup_required"], false);
    assert_eq!(closed["remaining_gaps"], serde_json::json!([]));

    fs::remove_dir_all(left).unwrap();
    fs::remove_dir_all(right).unwrap();
}

#[test]
fn bundled_guidance_uses_project_neutral_feature_and_debug_context() {
    let root = temp_root("guidance-project-neutral-context");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let guidance: Value = serde_json::from_str(
        &fs::read_to_string(
            root.join(".appsdk/skills/appsdk-project-governance/appsdk-guidance.json"),
        )
        .unwrap(),
    )
    .unwrap();

    for (rule_id, severity) in [
        ("changed-scope-control-truth", "forbidden"),
        ("changed-scope-single-owner", "forbidden"),
        ("changed-scope-configured-orchestration", "forbidden"),
        ("changed-scope-ablation-and-sharing", "advisory"),
        ("historical-architecture-debt", "advisory"),
        ("project-context-binding", "warning"),
        ("debug-notes-required", "warning"),
        ("map-gate-update", "advisory"),
    ] {
        assert!(guidance["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| { rule["rule_id"] == rule_id && rule["severity"] == severity }));
    }
    assert!(guidance["rules"].as_array().unwrap().iter().any(|rule| {
        rule["rule_id"] == "adjacent-transition" && rule["severity"] == "forbidden"
    }));

    let workflows = guidance["workflows"].as_array().unwrap();
    let develop = workflows
        .iter()
        .find(|workflow| workflow["domain"] == "develop")
        .unwrap();
    let develop_nodes = develop["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["node_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    for node in [
        "requirements",
        "map_check",
        "architecture",
        "detailed_design",
        "implementation",
        "map_update",
    ] {
        assert!(develop_nodes.contains(&node), "missing develop node {node}");
    }
    for (from, to) in [
        ("map_check", "architecture"),
        ("map_check", "implementation"),
        ("architecture", "detailed_design"),
        ("architecture", "implementation"),
        ("validation", "map_update"),
        ("validation", "review"),
    ] {
        assert!(
            develop["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|edge| { edge["from"] == from && edge["to"] == to }),
            "missing develop edge {from}->{to}"
        );
    }

    let debug = workflows
        .iter()
        .find(|workflow| workflow["domain"] == "debug")
        .unwrap();
    let debug_nodes = debug["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["node_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        debug_nodes,
        [
            "orient",
            "explore",
            "resolve",
            "candidate",
            "validation",
            "review",
            "integration",
            "cleanup"
        ]
    );

    for domain in ["develop", "debug"] {
        let workflow = workflows
            .iter()
            .find(|workflow| workflow["domain"] == domain)
            .unwrap();
        let review = workflow["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["node_id"] == "review")
            .unwrap();
        assert!(review["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|evidence| evidence == "architecture-conformance"));
    }

    let rendered = serde_json::to_string(&guidance).unwrap();
    for project_specific in [
        "RouteCodex",
        "rccv3",
        "v3-function-map",
        "v3-verification-map",
    ] {
        assert!(!rendered.contains(project_specific));
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_init_projects_declared_context_and_commands() {
    let root = temp_root("guidance-init");
    let root_text = root.to_str().unwrap();
    let created = run(&["new", root_text]);
    assert!(created.status.success());
    let created_stdout = String::from_utf8_lossy(&created.stdout);
    assert!(created_stdout.contains("appsdk guide compile"));
    assert!(created_stdout.contains("appsdk guide init"));

    let help = run(&["guide", "--help"]);
    assert!(help.status.success());
    let help_json: Value = serde_json::from_slice(&help.stdout).unwrap();
    assert_eq!(help_json["commands"][1]["command"], "init");

    fs::write(
        root.join("AGENTS.md"),
        "# Project rules\n\nRead the project-owned Skills before planning.\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("skills/local-debug")).unwrap();
    fs::write(
        root.join("skills/local-debug/SKILL.md"),
        "---\nname: local-debug\ndescription: Project-local debug procedure.\n---\n",
    )
    .unwrap();
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["guidance"]["rule_sources"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "source_id": "local-debug",
            "kind": "skill",
            "path": "skills/local-debug/SKILL.md",
            "required": false,
            "precedence": 300
        }));
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let before_compile = run(&[
        "guide",
        "init",
        root_text,
        "--task",
        "task-intake",
        "--mode",
        "develop",
        "--module",
        "app-core",
    ]);
    assert!(before_compile.status.success());
    let before_json: Value = serde_json::from_slice(&before_compile.stdout).unwrap();
    assert_eq!(before_json["reason_code"], "GUIDANCE_NOT_COMPILED");
    assert_eq!(before_json["missing_commands"][0], "appsdk guide compile");
    assert!(before_json["missing_commands"][1]
        .as_str()
        .unwrap()
        .contains("appsdk guide init"));
    assert!(!root.join(".appsdk-control/guidance/task-intake").exists());

    assert!(run(&["guide", "compile", root_text]).status.success());
    let developed = run(&[
        "guide",
        "init",
        root_text,
        "--task",
        "task-intake",
        "--mode",
        "develop",
        "--module",
        "app-core",
    ]);
    assert!(
        developed.status.success(),
        "{}",
        String::from_utf8_lossy(&developed.stderr)
    );
    let developed_json: Value = serde_json::from_slice(&developed.stdout).unwrap();
    assert_eq!(developed_json["reason_code"], "GUIDANCE_INTAKE_REQUIRED");
    assert_eq!(developed_json["task_id"], "task-intake");
    assert_eq!(developed_json["mode"], "develop");
    assert_eq!(developed_json["module"]["module_id"], "app-core");
    assert!(developed_json["read_first"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source["path"] == "AGENTS.md"));
    assert!(developed_json["skill_commands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|skill| skill["command"] == "$appsdk-project-governance"));
    assert!(developed_json["skill_commands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|skill| skill["command"] == "$local-debug"));
    assert!(developed_json["questions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|question| question["question_id"] == "architecture_confirmation"));
    assert!(developed_json["next"]["command"]
        .as_str()
        .unwrap()
        .contains("appsdk guide develop"));
    assert!(!root.join(".appsdk-control/guidance/task-intake").exists());

    let debugged = run(&[
        "guide",
        "init",
        root_text,
        "--task",
        "task-debug",
        "--mode",
        "debug",
        "--module",
        "app-core",
    ]);
    assert!(debugged.status.success());
    let debugged_json: Value = serde_json::from_slice(&debugged.stdout).unwrap();
    assert!(debugged_json["questions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|question| question["question_id"] == "failure_sample"));
    assert!(debugged_json["questions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|question| question["question_id"] == "causal_evidence"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_projects_missing_module_path_and_recovers_after_rebind() {
    let root = temp_root("guidance-module-path");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    fs::write(root.join(".appsdk/goal.json"), r#"{"goal_id":"goal-change-me","raw_request":"bind module","understood_objective":"bind module","acceptance_criteria":["compile"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#).unwrap();

    fs::remove_dir_all(root.join("playground/experiments")).unwrap();
    let compile = run(&["compile-module", root_text, "--module", "app-core"]);
    assert!(!compile.status.success());
    assert!(
        String::from_utf8_lossy(&compile.stderr)
            .contains("MODULE_PATH_MISSING:app-core:playground/experiments/**"),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let status = run(&[
        "guide",
        "governance-preflight",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(status.status.success());
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(
        status_json["reason_code"],
        "MODULE_PATH_MISSING:app-core:playground/experiments/**"
    );
    assert_eq!(status_json["first_failing_gate"], "module_binding");
    assert_eq!(status_json["next"]["owner"], "app-core");
    assert!(status_json["next"]["action"]
        .as_str()
        .unwrap()
        .contains(".appsdk/project.json"));

    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["modules"][0]["owned_paths"] =
        serde_json::json!(["playground/**", "protected/source/**", "tests/core/**"]);
    project["modules"][0]["regression"]["input_paths"] =
        serde_json::json!(["playground/**", "tests/core/**"]);
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();

    let recovered_compile = run(&["compile-module", root_text, "--module", "app-core"]);
    assert!(
        recovered_compile.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered_compile.stderr)
    );
    assert!(run(&["guide", "compile", root_text]).status.success());
    let recovered = run(&[
        "guide",
        "governance-preflight",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(recovered.status.success());
    let recovered_json: Value = serde_json::from_slice(&recovered.stdout).unwrap();
    assert_eq!(recovered_json["readiness"], "ready");
    assert_eq!(recovered_json["reason_code"], "PLAN_REQUIRED");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_rejects_undeclared_paths_and_non_adjacent_agent_plans() {
    let root = temp_root("guidance-plan-validation");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["guidance"]["rule_sources"][1]["contract_path"] =
        Value::String("../outside.json".into());
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let escaped = run(&["guide", "compile", root_text]);
    assert!(!escaped.status.success());
    assert!(String::from_utf8_lossy(&escaped.stderr).contains("GUIDANCE_RULE_SOURCE_PATH_ESCAPE"));

    project["guidance"]["rule_sources"][1]["contract_path"] =
        Value::String(".appsdk/skills/appsdk-project-governance/appsdk-guidance.json".into());
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let proposal_file = root.join("plan.json");
    fs::write(
        &proposal_file,
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "mode": "develop",
            "goal_id": "goal-change-me",
            "task_id": "task-1",
            "module_id": "app-core",
            "objective": "test plan validation",
            "scope_paths": ["playground/experiments/**"],
            "current_node": "requirements",
            "steps": [
                {"step_id":"step-1","node_id":"requirements","action":"analyze","owner":"app-core","expected_evidence":["requirements"]},
                {"step_id":"step-2","node_id":"detailed_design","action":"design","owner":"app-core","expected_evidence":["detailed-design"]}
            ]
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let supplied_state = run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-1",
        "--input",
        "plan.json",
    ]);
    assert!(!supplied_state.status.success());
    assert!(String::from_utf8_lossy(&supplied_state.stderr)
        .contains("GUIDANCE_DERIVED_FIELD_FORBIDDEN:current_node"));

    let mut proposal: Value =
        serde_json::from_str(&fs::read_to_string(&proposal_file).unwrap()).unwrap();
    proposal.as_object_mut().unwrap().remove("current_node");
    fs::write(
        &proposal_file,
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    let skipped = run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-1",
        "--input",
        "plan.json",
    ]);
    assert!(!skipped.status.success());
    assert!(String::from_utf8_lossy(&skipped.stderr)
        .contains("GUIDANCE_NON_ADJACENT_TRANSITION:requirements:detailed_design"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_plan_update_is_evidence_bound_idempotent_and_drift_safe() {
    let root = temp_root("guidance-update");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "mode": "develop",
            "goal_id": "goal-change-me",
            "task_id": "task-2",
            "module_id": "app-core",
            "objective": "test plan updates",
            "scope_paths": ["playground/experiments/**"],
            "steps": [
                {"step_id":"step-1","node_id":"requirements","action":"analyze","owner":"app-core","expected_evidence":["requirements"]},
                {"step_id":"step-2","node_id":"map_check","action":"bind maps","owner":"app-core","expected_evidence":["function-map-binding","verification-map-binding"]}
            ]
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let planned = run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-2",
        "--input",
        "plan.json",
    ]);
    assert!(
        planned.status.success(),
        "{}",
        String::from_utf8_lossy(&planned.stderr)
    );
    assert!(root
        .join(".appsdk-control/guidance/task-2/plan.json")
        .is_file());

    fs::write(
        root.join("result.json"),
        r#"{"schema_version":1,"event_id":"event-1","step_id":"step-1","result":"pass","observations":["closed"],"evidence":[]}
"#,
    )
    .unwrap();
    let missing = run(&[
        "guide",
        "update",
        root_text,
        "--task",
        "task-2",
        "--input",
        "result.json",
    ]);
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("GUIDANCE_PASS_REQUIRES_EVIDENCE:step-1")
    );

    fs::write(
        root.join("result.json"),
        r#"{"schema_version":1,"event_id":"event-1","step_id":"step-1","result":"pass","observations":["closed"],"evidence":["requirements-1"]}
"#,
    )
    .unwrap();
    let first = run(&[
        "guide",
        "update",
        root_text,
        "--task",
        "task-2",
        "--input",
        "result.json",
    ]);
    assert!(first.status.success());
    let duplicate = run(&[
        "guide",
        "update",
        root_text,
        "--task",
        "task-2",
        "--input",
        "result.json",
    ]);
    assert!(duplicate.status.success());
    let duplicate_json: Value = serde_json::from_slice(&duplicate.stdout).unwrap();
    assert_eq!(duplicate_json["idempotent"], true);

    fs::write(
        root.join("result.json"),
        r#"{"schema_version":1,"event_id":"event-1","step_id":"step-1","result":"pass","observations":["changed"],"evidence":["requirements-1"]}
"#,
    )
    .unwrap();
    let conflict = run(&[
        "guide",
        "update",
        root_text,
        "--task",
        "task-2",
        "--input",
        "result.json",
    ]);
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("GUIDANCE_EVENT_CONFLICT:event-1"));

    let next = run(&["guide", "next", root_text, "--task", "task-2"]);
    assert!(next.status.success());
    let next_json: Value = serde_json::from_slice(&next.stdout).unwrap();
    assert_eq!(next_json["next"]["node_id"], "map_check");

    let goal_file = root.join(".appsdk/goal.json");
    let mut goal: Value = serde_json::from_str(&fs::read_to_string(&goal_file).unwrap()).unwrap();
    goal["understood_objective"] = Value::String("drifted objective".into());
    fs::write(
        &goal_file,
        serde_json::to_string_pretty(&goal).unwrap() + "\n",
    )
    .unwrap();
    fs::write(
        root.join("result.json"),
        r#"{"schema_version":1,"event_id":"event-1","step_id":"step-1","result":"pass","observations":["closed"],"evidence":["requirements-1"]}
"#,
    )
    .unwrap();
    let replay_after_drift = run(&[
        "guide",
        "update",
        root_text,
        "--task",
        "task-2",
        "--input",
        "result.json",
    ]);
    assert!(replay_after_drift.status.success());
    let replay_json: Value = serde_json::from_slice(&replay_after_drift.stdout).unwrap();
    assert_eq!(replay_json["idempotent"], true);

    fs::write(
        root.join("result.json"),
        r#"{"schema_version":1,"event_id":"event-2","step_id":"step-2","result":"pass","observations":[],"evidence":["architecture-1"]}
"#,
    )
    .unwrap();
    let drift = run(&[
        "guide",
        "update",
        root_text,
        "--task",
        "task-2",
        "--input",
        "result.json",
    ]);
    assert!(!drift.status.success());
    assert!(String::from_utf8_lossy(&drift.stderr).contains("GUIDANCE_CONTEXT_DRIFT:goal"));

    let status = run(&["guide", "next", root_text, "--task", "task-2"]);
    assert!(status.status.success());
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(
        status_json["next"]["revision_reason"],
        "rule_context_changed"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_tour_review_requires_node_content_before_flow_update() {
    let root = temp_root("guidance-tour-review");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);
    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "mode": "develop",
            "goal_id": "goal-change-me",
            "task_id": "tour-task",
            "module_id": "app-core",
            "objective": "tour review ordering",
            "scope_paths": ["playground/experiments/**"],
            "steps": [
                {"step_id":"step-1","node_id":"requirements","action":"inspect","owner":"app-core","expected_evidence":["requirements"]},
                {"step_id":"step-2","node_id":"map_check","action":"inspect","owner":"app-core","expected_evidence":["function-map-binding","verification-map-binding"]}
            ]
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "plan.json"
    ])
    .status
    .success());

    fs::write(
        root.join("tour.json"),
        r#"{"schema_version":1,"tour_id":"tour-1","selected_path":["requirements","map_check"]}
"#,
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "tour",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "tour.json"
    ])
    .status
    .success());

    fs::write(
        root.join("flow-review.json"),
        r#"{"schema_version":1,"review_id":"flow-before-nodes","stage":"flow_review","flow_update":{"order":["requirements","map_check"],"edges":[{"from":"requirements","to":"map_check"}],"rules":["keep-adjacent"]}}
"#,
    )
    .unwrap();
    let blocked = run(&[
        "guide",
        "review",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "flow-review.json",
    ]);
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr)
        .contains("GUIDANCE_FLOW_REVIEW_REQUIRES_NODE_APPROVAL"));

    fs::write(
        root.join("node-one.json"),
        r#"{"schema_version":1,"review_id":"node-1","stage":"node_review","node_updates":[{"node_id":"requirements","verdict":"accept","content":"confirmed requirements"}]}
"#,
    )
    .unwrap();
    let first = run(&[
        "guide",
        "review",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "node-one.json",
    ]);
    assert!(first.status.success());
    let first_json: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first_json["node_review_complete"], false);

    fs::write(
        root.join("node-two.json"),
        r#"{"schema_version":1,"review_id":"node-2","stage":"node_review","node_updates":[{"node_id":"map_check","verdict":"approved","content":"confirmed maps"}]}
"#,
    )
    .unwrap();
    let second = run(&[
        "guide",
        "review",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "node-two.json",
    ]);
    assert!(second.status.success());
    let second_json: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(second_json["node_review_complete"], true);
    assert_eq!(second_json["flow_review_allowed"], true);

    fs::write(
        root.join("empty-flow.json"),
        r#"{"schema_version":1,"review_id":"empty-flow","stage":"flow_review"}
"#,
    )
    .unwrap();
    let empty_flow = run(&[
        "guide",
        "review",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "empty-flow.json",
    ]);
    assert!(!empty_flow.status.success());
    assert!(String::from_utf8_lossy(&empty_flow.stderr).contains("GUIDANCE_FLOW_UPDATE_REQUIRED"));

    fs::write(
        root.join("rejected-flow.json"),
        r#"{"schema_version":1,"review_id":"flow-rejected","stage":"flow_review","verdict":"reject","flow_update":{"order":["requirements","map_check"],"edges":[{"from":"requirements","to":"map_check"}],"rules":["keep-adjacent"]}}
"#,
    )
    .unwrap();
    let rejected_flow = run(&[
        "guide",
        "review",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "rejected-flow.json",
    ]);
    assert!(rejected_flow.status.success());
    let rejected_json: Value = serde_json::from_slice(&rejected_flow.stdout).unwrap();
    assert_eq!(rejected_json["verdict"], "rejected");
    assert_eq!(rejected_json["flow_revision"], Value::Null);
    assert!(rejected_json["next"]
        .as_str()
        .unwrap()
        .contains("submit flow_review again"));

    let flow = run(&[
        "guide",
        "review",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "flow-review.json",
    ]);
    assert!(
        flow.status.success(),
        "{}",
        String::from_utf8_lossy(&flow.stderr)
    );
    let flow_json: Value = serde_json::from_slice(&flow.stdout).unwrap();
    assert_eq!(flow_json["stage"], "flow_review");
    assert_eq!(flow_json["active_revision_changed"], false);
    let events =
        fs::read_to_string(root.join(".appsdk-control/guidance/tour-task/events.jsonl")).unwrap();
    let flow_line = events
        .lines()
        .find(|line| line.contains("\"stage\":\"flow_review\""))
        .unwrap();
    assert!(flow_line.contains("node-requirements-"));
    assert!(flow_line.contains("node-map_check-"));

    fs::write(
        root.join("node-revision.json"),
        r#"{"schema_version":1,"review_id":"node-3","stage":"node_review","node_updates":[{"node_id":"requirements","verdict":"accept","content":"reconfirmed requirements"}]}
"#,
    )
    .unwrap();
    let revised_node = run(&[
        "guide",
        "review",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "node-revision.json",
    ]);
    assert!(revised_node.status.success());
    let status = run(&["guide", "status", root_text, "--task", "tour-task"]);
    assert!(status.status.success());
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["tour_review"]["stage"], "flow_review");
    assert_eq!(status_json["tour_review"]["flow_review_complete"], false);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_memory_index_query_review_and_compact_are_layered() {
    let root = temp_root("project-memory");
    fs::create_dir_all(&root).unwrap();
    let memory_home = temp_root("project-memory-home");
    fs::create_dir_all(&memory_home).unwrap();
    let root_text = root.to_str().unwrap();
    let home_text = memory_home.to_str().unwrap();
    let anchor = run_memory(
        &root,
        &[
            "entry",
            "--id",
            "plan-root",
            "--category",
            "plan",
            "--text",
            "stable project anchor",
            "--importance",
            "95",
            "--tag",
            "owner",
        ],
        &memory_home,
    );
    assert!(
        anchor.status.success(),
        "{}",
        String::from_utf8_lossy(&anchor.stderr)
    );
    let fact = run_memory(
        &root,
        &[
            "entry",
            "--id",
            "fact-one",
            "--category",
            "knowledge",
            "--text",
            "SQLite stores the rebuildable index",
            "--tag",
            "storage",
        ],
        &memory_home,
    );
    assert!(
        fact.status.success(),
        "{}",
        String::from_utf8_lossy(&fact.stderr)
    );
    let queried = run_memory(&root, &["query", "SQLite"], &memory_home);
    assert!(queried.status.success());
    let queried_json: Value = serde_json::from_slice(&queried.stdout).unwrap();
    assert_eq!(queried_json["semantic_backend"]["status"], "candidate-only");
    assert!(queried_json["keyword_matches"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["id"] == "fact-one"));
    let inspected = run_memory(&root, &["verify"], &memory_home);
    assert!(inspected.status.success());
    let inspected_json: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(inspected_json["ok"], true);

    fs::create_dir_all(root.join(".agent-collab/runs/run-memory")).unwrap();
    fs::write(
        root.join(".agent-collab/runs/run-memory/notes.jsonl"),
        r#"{"record_type":"lesson","memory":{"id":"lesson-one","category":"lesson","content":"verified review ordering","tags":["review"]}}
"#,
    )
    .unwrap();
    let reviewed = run_memory(&root, &["review", "--run", "run-memory"], &memory_home);
    assert!(
        reviewed.status.success(),
        "{}",
        String::from_utf8_lossy(&reviewed.stderr)
    );
    let reviewed_json: Value = serde_json::from_slice(&reviewed.stdout).unwrap();
    assert_eq!(reviewed_json["checked"], true);
    let duplicate = run_memory(
        &root,
        &[
            "entry",
            "--id",
            "lesson-one",
            "--category",
            "lesson",
            "--text",
            "verified review ordering",
            "--tag",
            "ordering",
        ],
        &memory_home,
    );
    assert!(duplicate.status.success());
    let compacted = run_memory(&root, &["compact"], &memory_home);
    assert!(
        compacted.status.success(),
        "{}",
        String::from_utf8_lossy(&compacted.stderr)
    );
    let lesson = run_memory(&root, &["get", "lesson-one"], &memory_home);
    let lesson_json: Value = serde_json::from_slice(&lesson.stdout).unwrap();
    let tags = lesson_json["matches"][0]["tags"].as_array().unwrap();
    assert!(tags.iter().any(|tag| tag == "review"));
    assert!(tags.iter().any(|tag| tag == "ordering"));
    assert!(!home_text.is_empty() && !root_text.is_empty());
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
}

#[test]
fn project_memory_edges_and_latest_compaction_are_explicit() {
    let root = temp_root("project-memory-edges");
    let caller = temp_root("project-memory-edge-caller");
    let memory_home = temp_root("project-memory-edge-home");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&caller).unwrap();
    fs::create_dir_all(&memory_home).unwrap();
    fs::create_dir_all(root.join("memory")).unwrap();
    fs::write(
        root.join("memory/knowledge.jsonl"),
        concat!(
            r#"{"id":"anchor-edge","category":"knowledge","title":"Anchor","content":"stable node anchor","tags":["stable"],"source_refs":["guide/node"],"related_ids":["lesson-edge"],"semantic_relations":[{"to_id":"lesson-edge","type":"similar_lesson","score":0.91,"model_revision":"wemm-test-v1"}],"importance":95,"memory_level":1,"review_status":"reviewed","review_evidence":["review/anchor-edge"]}"#,
            "\n",
            r#"{"id":"latest-entry","category":"knowledge","content":"old content","tags":["old"],"source_refs":["source/old"],"updated_at":"2026-01-01T00:00:00Z"}"#,
            "\n",
            r#"{"id":"latest-entry","category":"knowledge","content":"new content","tags":["new"],"source_refs":["source/new"],"updated_at":"2026-01-02T00:00:00Z"}"#,
            "\n",
            r#"{"id":"lesson-edge","category":"lesson","content":"verified historical lesson","tags":["lesson"],"source_refs":["review/1"]}"#,
            "\n"
        ),
    )
    .unwrap();

    let indexed = run_memory(&root, &["index"], &memory_home);
    assert!(
        indexed.status.success(),
        "{}",
        String::from_utf8_lossy(&indexed.stderr)
    );

    let root_text = root.to_str().unwrap();
    let queried = run_memory(&caller, &["query", "stable", root_text], &memory_home);
    assert!(
        queried.status.success(),
        "{}",
        String::from_utf8_lossy(&queried.stderr)
    );
    let queried_json: Value = serde_json::from_slice(&queried.stdout).unwrap();
    assert!(queried_json["anchors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["id"] == "anchor-edge"));
    assert!(queried_json["declared_related"]
        .as_array()
        .unwrap()
        .iter()
        .any(|edge| edge["from_id"] == "anchor-edge" && edge["to_id"] == "lesson-edge"));
    assert!(queried_json["semantic_related"]
        .as_array()
        .unwrap()
        .iter()
        .any(|edge| edge["type"] == "similar_lesson" && edge["model_revision"] == "wemm-test-v1"));

    let inspected = run_memory(&caller, &["get", "anchor-edge", root_text], &memory_home);
    assert!(inspected.status.success());
    let inspected_json: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(inspected_json["matches"][0]["scope"], "project");

    let source_before = fs::read_to_string(root.join("memory/knowledge.jsonl")).unwrap();
    let compacted = run_memory(&root, &["compact"], &memory_home);
    assert!(
        compacted.status.success(),
        "{}",
        String::from_utf8_lossy(&compacted.stderr)
    );
    let source_after = fs::read_to_string(root.join("memory/knowledge.jsonl")).unwrap();
    assert_eq!(
        source_after, source_before,
        "compact must retain raw event history"
    );
    let latest = run_memory(&root, &["get", "latest-entry"], &memory_home);
    assert!(latest.status.success());
    let latest: Value = serde_json::from_slice(&latest.stdout).unwrap();
    let latest = &latest["matches"][0];
    assert_eq!(latest["content"], "new content");
    assert!(latest["tags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tag| tag == "old"));
    assert!(latest["tags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tag| tag == "new"));
    assert!(latest["source_refs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source == "source/old"));
    assert!(latest["source_refs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source == "source/new"));

    let empty = run_memory(&root, &["query", "   "], &memory_home);
    assert!(!empty.status.success());
    assert!(String::from_utf8_lossy(&empty.stderr).contains("MEMORY_QUERY_EMPTY"));
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(caller).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
}

#[test]
fn project_memory_updates_follow_current_version_and_verify_source_drift() {
    let root = temp_root("project-memory-versioning");
    let memory_home = temp_root("project-memory-versioning-home");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&memory_home).unwrap();

    for (content, tag) in [("A", "a"), ("B", "b"), ("A", "c")] {
        let result = run_memory(
            &root,
            &[
                "entry",
                "--id",
                "cycle",
                "--category",
                "knowledge",
                "--text",
                content,
                "--tag",
                tag,
            ],
            &memory_home,
        );
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let source = fs::read_to_string(root.join("memory/knowledge.jsonl")).unwrap();
    assert_eq!(
        source.lines().count(),
        3,
        "A -> B -> A must retain three events"
    );
    let current = run_memory(&root, &["get", "cycle"], &memory_home);
    assert!(current.status.success());
    let current: Value = serde_json::from_slice(&current.stdout).unwrap();
    assert_eq!(current["matches"][0]["content"], "A");
    for tag in ["a", "b", "c"] {
        assert!(current["matches"][0]["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == tag));
    }
    let wrong_category = run_memory(
        &root,
        &[
            "entry",
            "--id",
            "cycle",
            "--category",
            "lesson",
            "--text",
            "cross category",
        ],
        &memory_home,
    );
    assert!(!wrong_category.status.success());
    assert!(String::from_utf8_lossy(&wrong_category.stderr).contains("MEMORY_CATEGORY_CHANGE"));

    let source_path = root.join("memory/knowledge.jsonl");
    let mut changed = fs::read_to_string(&source_path).unwrap();
    changed.push_str(r#"{"id":"drifted","category":"knowledge","content":"added outside index","tags":[],"source_refs":[]}"#);
    changed.push('\n');
    fs::write(&source_path, &changed).unwrap();
    let stale = run_memory(&root, &["verify"], &memory_home);
    assert!(stale.status.success());
    let stale: Value = serde_json::from_slice(&stale.stdout).unwrap();
    assert_eq!(stale["ok"], false);
    assert!(stale["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|scope| { scope["scope"] == "project" && scope["source_consistent"] == false }));
    let refreshed = run_memory(&root, &["query", "drifted"], &memory_home);
    assert!(refreshed.status.success());
    let refreshed: Value = serde_json::from_slice(&refreshed.stdout).unwrap();
    assert!(refreshed["keyword_matches"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["id"] == "drifted"));
    let healthy = run_memory(&root, &["verify"], &memory_home);
    let healthy: Value = serde_json::from_slice(&healthy.stdout).unwrap();
    assert_eq!(healthy["ok"], true);

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
}

#[test]
fn project_memory_migration_and_reentry_are_resumable_and_source_preserving() {
    let root = temp_root("project-memory-migration");
    let memory_home = temp_root("project-memory-migration-home");
    let root_text = root.to_str().unwrap();
    fs::create_dir_all(root.join("memory")).unwrap();
    fs::create_dir_all(&memory_home).unwrap();
    let legacy = concat!(
        r#"{"id":"legacy-path","category":"path","content":"legacy node","tags":["node"],"source_refs":["old/path"]}"#,
        "\n",
        r#"{"id":"legacy-lesson","category":"lesson","text":"legacy lesson","tags":["old"]}"#,
        "\n"
    );
    fs::write(
        root.join("memory/path.jsonl"),
        r#"{"id":"legacy-path","category":"path","content":"legacy node","tags":["existing"]}
"#,
    )
    .unwrap();
    fs::write(root.join("memory/entries.jsonl"), legacy).unwrap();

    let migrated = run_memory(&root, &["migrate"], &memory_home);
    assert!(
        migrated.status.success(),
        "{}",
        String::from_utf8_lossy(&migrated.stderr)
    );
    let migrated: Value = serde_json::from_slice(&migrated.stdout).unwrap();
    assert_eq!(migrated["status"], "complete");
    assert_eq!(migrated["migrated_entries"], 2);
    assert_eq!(migrated["raw_sources_retained"], true);
    assert_eq!(
        fs::read_to_string(root.join("memory/entries.jsonl")).unwrap(),
        legacy
    );
    assert!(root.join("memory/path.jsonl").is_file());
    assert!(root.join("memory/lesson.jsonl").is_file());
    let migrated_path = run_memory(&root, &["get", "legacy-path"], &memory_home);
    let migrated_path: Value = serde_json::from_slice(&migrated_path.stdout).unwrap();
    let migrated_tags = migrated_path["matches"][0]["tags"].as_array().unwrap();
    assert!(migrated_tags.iter().any(|tag| tag == "existing"));
    assert!(migrated_tags.iter().any(|tag| tag == "node"));
    assert!(migrated_path["matches"][0]["source_refs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source == "old/path"));
    let migrated_detail = root.join(migrated_path["matches"][0]["detail_path"].as_str().unwrap());
    assert!(migrated_detail.is_file());
    assert!(fs::read_to_string(root.join("memory/index.md"))
        .unwrap()
        .contains("### legacy-path"));
    let exported = run_memory(&root, &["export"], &memory_home);
    assert!(exported.status.success());
    let exported: Value = serde_json::from_slice(&exported.stdout).unwrap();
    assert_eq!(exported["project"]["entries"], 2);

    fs::create_dir_all(root.join(".agent-collab/runs/reentry-run")).unwrap();
    fs::write(
        root.join(".agent-collab/runs/reentry-run/notes.jsonl"),
        r#"{"event_id":"e1","node_id":"path-node","step_id":"step-2","status":"working"}
"#,
    )
    .unwrap();
    fs::remove_dir_all(memory_home.join("projects")).unwrap();
    let reentered = run_memory(&root, &["reentry", "--run", "reentry-run"], &memory_home);
    assert!(
        reentered.status.success(),
        "{}",
        String::from_utf8_lossy(&reentered.stderr)
    );
    let reentered: Value = serde_json::from_slice(&reentered.stdout).unwrap();
    assert_eq!(reentered["status"], "ready");
    assert_eq!(reentered["run_id"], "reentry-run");
    assert_eq!(reentered["preserves_run_id"], true);
    assert_eq!(reentered["index"]["rebuilt"], true);
    assert_eq!(reentered["resume_from"]["node_id"], "path-node");
    assert!(reentered["next_queries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|query| query == "path-node"));
    let explicit_project = run_memory(
        &memory_home,
        &["reentry", "--run", "reentry-run", root_text],
        &memory_home,
    );
    assert!(
        explicit_project.status.success(),
        "{}",
        String::from_utf8_lossy(&explicit_project.stderr)
    );
    let explicit_project: Value = serde_json::from_slice(&explicit_project.stdout).unwrap();
    assert_eq!(explicit_project["run_id"], "reentry-run");
    assert_eq!(explicit_project["status"], "ready");

    let marker_path = root.join("memory/migration.json");
    let mut marker: Value =
        serde_json::from_str(&fs::read_to_string(&marker_path).unwrap()).unwrap();
    marker["status"] = Value::String("in_progress".into());
    fs::write(
        &marker_path,
        serde_json::to_string_pretty(&marker).unwrap() + "\n",
    )
    .unwrap();
    let resumed = run_memory(&root, &["migrate"], &memory_home);
    assert!(resumed.status.success());
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["status"], "complete");
    assert_eq!(resumed["resumed"], true);
    let already = run_memory(&root, &["migrate"], &memory_home);
    assert!(already.status.success());
    let already: Value = serde_json::from_slice(&already.stdout).unwrap();
    assert_eq!(already["status"], "already_complete");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
}

#[test]
fn project_memory_uses_review_levels_markdown_details_and_tag_search() {
    let root = temp_root("project-memory-review-levels");
    let memory_home = temp_root("project-memory-review-levels-home");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&memory_home).unwrap();

    let created = run_memory(
        &root,
        &[
            "entry",
            "--title",
            "Canonical memory title",
            "--text",
            "The full detail stays outside the short index",
            "--tag",
            "architecture",
            "--tag",
            "review",
        ],
        &memory_home,
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let created: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(created["memory_level"], 3);
    assert_eq!(created["review_status"], "unreviewed");
    let id = created["id"].as_str().unwrap();
    assert_eq!(created["detail_path"], format!("memory/L3/{id}.md"));
    let detail = root.join(created["detail_path"].as_str().unwrap());
    assert!(detail.is_file());
    assert!(fs::read_to_string(&detail)
        .unwrap()
        .contains("The full detail stays outside the short index"));
    let index = fs::read_to_string(root.join("memory/index.md")).unwrap();
    assert!(index.contains("### Canonical memory title"));
    assert!(index.contains("- tags: `architecture`, `review`"));
    assert!(index.contains(&format!("details: [L3/{id}.md](L3/{id}.md)")));
    assert!(!index.contains("The full detail stays outside the short index"));
    assert!(index.contains("## Level 3"));
    assert!(index.contains(&format!(
        "- L3: Canonical memory title (knowledge) [architecture,review] -> L3/{id}.md"
    )));

    let tagged = run_memory(&root, &["query", "--tag", "architecture"], &memory_home);
    assert!(tagged.status.success());
    let tagged: Value = serde_json::from_slice(&tagged.stdout).unwrap();
    assert!(tagged["keyword_matches"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["id"] == id && entry["tags"].as_array().unwrap().len() == 2));

    let promoted = run_memory(
        &root,
        &[
            "promote",
            "--id",
            id,
            "--level",
            "2",
            "--evidence",
            "review/run-1#memory-1",
        ],
        &memory_home,
    );
    assert!(
        promoted.status.success(),
        "{}",
        String::from_utf8_lossy(&promoted.stderr)
    );
    let promoted: Value = serde_json::from_slice(&promoted.stdout).unwrap();
    assert_eq!(promoted["level"], 2);
    let current = run_memory(&root, &["get", id], &memory_home);
    let current: Value = serde_json::from_slice(&current.stdout).unwrap();
    assert_eq!(current["matches"][0]["memory_level"], 2);
    assert_eq!(current["matches"][0]["review_status"], "reviewed");
    assert_eq!(
        current["matches"][0]["detail_path"],
        format!("memory/L2/{id}.md")
    );
    assert!(root.join(format!("memory/L2/{id}.md")).is_file());
    assert!(fs::read_to_string(root.join("memory/index.md"))
        .unwrap()
        .contains("## Level 2"));
    assert!(fs::read_to_string(root.join("memory/index.md"))
        .unwrap()
        .contains(&format!(
            "- L2: Canonical memory title (knowledge) [architecture,review] -> L2/{id}.md"
        )));

    fs::remove_dir_all(memory_home.join("projects")).unwrap();
    let rebuilt = run_memory(&root, &["query", "--tag", "review"], &memory_home);
    assert!(rebuilt.status.success());
    let rebuilt: Value = serde_json::from_slice(&rebuilt.stdout).unwrap();
    assert!(rebuilt["keyword_matches"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["id"] == id && entry["memory_level"] == 2));

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
}

#[test]
fn project_memory_handwritten_l3_is_automatically_indexed_once() {
    for trigger in ["get", "index", "verify"] {
        let root = temp_root(&format!("memory-handwritten-{trigger}"));
        let memory_home = temp_root(&format!("memory-handwritten-home-{trigger}"));
        fs::create_dir_all(root.join("memory/L3")).unwrap();
        fs::create_dir_all(&memory_home).unwrap();
        // Exercise both an existing SQLite projection and a missing one.
        if trigger == "get" {
            assert!(run_memory(&root, &["index"], &memory_home).status.success());
        }
        for id in ["manual-a", "manual-b"] {
            fs::write(root.join(format!("memory/L3/{id}.md")), format!(
                "<!-- project-memory:v1 {{\"id\":\"{id}\",\"category\":\"knowledge\",\"tags\":[\"handwritten\"],\"memory_level\":1,\"review_status\":\"reviewed\",\"review_evidence\":[\"forged\"]}} -->\n\n# Manual title\n\nHandwritten fact\n<!-- project-memory:end -->\n"
            )).unwrap();
        }
        let args = if trigger == "get" {
            vec!["get", "manual-a"]
        } else {
            vec![trigger]
        };
        let result = run_memory(&root, &args, &memory_home);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        for id in ["manual-a", "manual-b"] {
            let result = run_memory(&root, &["get", id], &memory_home);
            let value: Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(value["matches"][0]["content"], "Handwritten fact");
            assert_eq!(value["matches"][0]["memory_level"], 3);
            assert_eq!(value["matches"][0]["review_status"], "unreviewed");
            assert!(value["matches"][0]["review_evidence"]
                .as_array()
                .is_none_or(|v| v.is_empty()));
        }
        assert!(run_memory(&root, &["index"], &memory_home).status.success());
        assert_eq!(
            fs::read_to_string(root.join("memory/knowledge.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        let query = run_memory(&root, &["query", "--tag", "handwritten"], &memory_home);
        assert!(query.status.success());
        assert!(String::from_utf8_lossy(&query.stdout).contains("manual-b"));
    }
}

#[test]
fn project_memory_handwritten_l3_validates_batch_before_writes() {
    let root = temp_root("memory-handwritten-invalid");
    let memory_home = temp_root("memory-handwritten-invalid-home");
    fs::create_dir_all(root.join("memory/L3")).unwrap();
    fs::create_dir_all(&memory_home).unwrap();
    let valid = "<!-- project-memory:v1 {\"id\":\"a-valid\"} -->\n\n# Valid\n\nFact\n<!-- project-memory:end -->\n";
    fs::write(root.join("memory/L3/a-valid.md"), valid).unwrap();
    let invalid = "# Not a marked memory\n\nNo ID\n";
    fs::write(root.join("memory/L3/z-invalid.md"), invalid).unwrap();
    let result = run_memory(&root, &["index"], &memory_home);
    assert!(!result.status.success());
    assert!(!root.join("memory/knowledge.jsonl").exists());
    assert_eq!(
        fs::read_to_string(root.join("memory/L3/a-valid.md")).unwrap(),
        valid
    );
    assert_eq!(
        fs::read_to_string(root.join("memory/L3/z-invalid.md")).unwrap(),
        invalid
    );
    fs::remove_file(root.join("memory/L3/z-invalid.md")).unwrap();
    fs::rename(
        root.join("memory/L3/a-valid.md"),
        root.join("memory/L3/wrong-name.md"),
    )
    .unwrap();
    let result = run_memory(&root, &["index"], &memory_home);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("MEMORY_DETAIL_FILENAME_MISMATCH"));
    assert!(!root.join("memory/knowledge.jsonl").exists());
}

#[test]
fn project_memory_markdown_round_trip_import_is_idempotent() {
    let root = temp_root("project-memory-markdown-round-trip");
    let memory_home = temp_root("project-memory-markdown-round-trip-home");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&memory_home).unwrap();

    let created = run_memory(
        &root,
        &[
            "entry",
            "--id",
            "round-trip",
            "--category",
            "lesson",
            "--title",
            "Original title",
            "--text",
            "Original detail",
            "--tag",
            "compatibility",
        ],
        &memory_home,
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let created: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(created["write_mode"], "one_shot");
    let detail = root.join(created["detail_path"].as_str().unwrap());
    let mut markdown = fs::read_to_string(&detail).unwrap();
    assert!(markdown.starts_with("<!-- project-memory:v1 "));
    markdown = markdown
        .replace("# Original title", "# Edited title")
        .replace("Original detail", "Edited detail");
    fs::write(&detail, markdown).unwrap();

    let imported = run_memory(&root, &["import"], &memory_home);
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let imported: Value = serde_json::from_slice(&imported.stdout).unwrap();
    assert_eq!(imported["status"], "complete");
    assert_eq!(imported["imported_entries"], 1);

    let current = run_memory(&root, &["get", "round-trip"], &memory_home);
    let current: Value = serde_json::from_slice(&current.stdout).unwrap();
    assert_eq!(current["matches"][0]["title"], "Edited title");
    assert_eq!(current["matches"][0]["content"], "Edited detail");
    assert_eq!(
        fs::read_to_string(root.join("memory/lesson.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2,
        "import appends an event and preserves the original event"
    );

    let repeated = run_memory(&root, &["import"], &memory_home);
    assert!(repeated.status.success());
    let repeated: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(repeated["status"], "already_current");
    assert_eq!(repeated["skipped_existing"], 1);
    assert_eq!(
        fs::read_to_string(root.join("memory/lesson.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2,
        "repeating import must not append another event"
    );

    fs::create_dir_all(root.join("memory/details")).unwrap();
    let legacy_detail = root.join("memory/details/legacy-markdown.md");
    fs::write(
        &legacy_detail,
        "# Legacy Markdown\n\nLegacy detail\n\n---\n\n- id: `legacy-markdown`\n- tags: `legacy` `compatible`\n- source_refs: `old/export`\n",
    )
    .unwrap();
    let legacy_import = run_memory(&root, &["import"], &memory_home);
    assert!(legacy_import.status.success());
    let legacy_import: Value = serde_json::from_slice(&legacy_import.stdout).unwrap();
    assert_eq!(legacy_import["imported_entries"], 1);
    let legacy = run_memory(&root, &["get", "legacy-markdown"], &memory_home);
    let legacy: Value = serde_json::from_slice(&legacy.stdout).unwrap();
    assert_eq!(legacy["matches"][0]["content"], "Legacy detail");
    assert_eq!(legacy["matches"][0]["category"], "knowledge");
    assert!(legacy["matches"][0]["tags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tag| tag == "compatible"));

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
}

#[test]
fn optional_memory_does_not_block_governance_initialization() {
    let root = temp_root("optional-memory-init");
    assert!(run(&["new", root.to_str().unwrap()]).status.success());
    fs::remove_dir_all(root.join("memory")).unwrap();
    fs::write(root.join("memory"), "business file").unwrap();
    let result = run(&["init", root.to_str().unwrap()]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(root.join(".appsdk/project.json").is_file());
    assert!(String::from_utf8_lossy(&result.stderr).contains("MEMORY_DIR_INVALID"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_detects_declared_rule_source_drift_and_symlink() {
    let root = temp_root("guidance-rule-source-drift");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let agents_target = root.join("project-agents.md");
    fs::write(&agents_target, "# Project rules\n").unwrap();
    fs::remove_file(root.join("AGENTS.md")).unwrap();
    symlink(&agents_target, root.join("AGENTS.md")).unwrap();
    let linked = run(&["guide", "compile", root_text]);
    assert!(!linked.status.success());
    assert!(String::from_utf8_lossy(&linked.stderr).contains("GUIDANCE_RULE_SOURCE_SYMLINK"));
    fs::remove_file(root.join("AGENTS.md")).unwrap();
    fs::write(root.join("AGENTS.md"), "# Project rules\n").unwrap();
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let outside = temp_root("guidance-control-symlink-target");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(root.join(".appsdk-control/guidance")).unwrap();
    symlink(
        &outside,
        root.join(".appsdk-control/guidance/task-control-symlink"),
    )
    .unwrap();
    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "mode": "debug",
            "goal_id": "goal-change-me",
            "task_id": "task-control-symlink",
            "module_id": "app-core",
            "objective": "reject redirected control state",
            "scope_paths": ["playground/experiments/**"],
            "steps": [{"step_id":"debug-1","node_id":"orient","action":"bind project context","owner":"app-core","expected_evidence":["orientation-record","function-map-binding","verification-map-binding"]}]
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let redirected = run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-control-symlink",
        "--input",
        "plan.json",
    ]);
    assert!(!redirected.status.success());
    assert!(String::from_utf8_lossy(&redirected.stderr).contains("GUIDANCE_TASK_CONTROL_SYMLINK"));
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    fs::remove_file(root.join(".appsdk-control/guidance/task-control-symlink")).unwrap();
    fs::remove_dir_all(outside).unwrap();

    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "mode": "debug",
            "goal_id": "goal-change-me",
            "task_id": "task-rule-drift",
            "module_id": "app-core",
            "objective": "detect declared rule drift",
            "scope_paths": ["playground/experiments/**"],
            "steps": [{"step_id":"debug-1","node_id":"orient","action":"bind project context","owner":"app-core","expected_evidence":["orientation-record","function-map-binding","verification-map-binding"]}]
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-rule-drift",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    fs::write(root.join("AGENTS.md"), "# Project rules\n\nChanged.\n").unwrap();
    let status = run(&["guide", "next", root_text, "--task", "task-rule-drift"]);
    assert!(status.status.success());
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(
        status_json["reason_code"],
        "GUIDANCE_COMPILED_CONTEXT_DRIFT:rule_sources"
    );
    assert_eq!(status_json["next"]["command"], "appsdk guide compile");

    assert!(run(&["guide", "compile", root_text]).status.success());
    let revised_status = run(&["guide", "next", root_text, "--task", "task-rule-drift"]);
    assert!(revised_status.status.success());
    let revised_json: Value = serde_json::from_slice(&revised_status.stdout).unwrap();
    assert_eq!(
        revised_json["reason_code"],
        "GUIDANCE_CONTEXT_DRIFT:guidance_manifest"
    );
    assert_eq!(
        revised_json["next"]["revision_reason"],
        "guidance_manifest_changed"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_plan_rejects_concurrent_writer_lock() {
    let root = temp_root("guidance-task-lock");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);
    let control_dir = root.join(".appsdk-control/guidance/task-locked");
    fs::create_dir_all(&control_dir).unwrap();
    fs::write(
        control_dir.join("write.lock"),
        format!("pid={} op=plan created=0\n", std::process::id()),
    )
    .unwrap();

    fs::write(root.join("plan.json"), "{}").unwrap();
    let res = run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-locked",
        "--input",
        "plan.json",
    ]);
    assert!(!res.status.success());
    assert!(String::from_utf8_lossy(&res.stderr).contains("GUIDANCE_TASK_LOCKED"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_event_ledger_reports_bad_line_number() {
    let root = temp_root("guidance-events-line");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let plan_file = root.join("plan.json");
    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-events",
        "module_id": "app-core",
        "objective": "line number for corrupted ledger",
        "scope_paths": ["playground/experiments/input.txt"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::create_dir_all(root.join("playground/experiments")).unwrap();
    fs::write(root.join("playground/experiments/input.txt"), "v1\n").unwrap();
    fs::write(
        &plan_file,
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-events",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    let events = root.join(".appsdk-control/guidance/task-events/events.jsonl");
    let mut content = fs::read_to_string(&events).unwrap();
    content.push_str("not-json\n");
    fs::write(&events, content).unwrap();

    let status = run(&["guide", "next", root_text, "--task", "task-events"]);
    assert!(!status.status.success());
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(
        stderr.contains("GUIDANCE_EVENTS_INVALID:line=2"),
        "{}",
        stderr
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_event_ledger_quarantines_partial_tail_and_continues() {
    let root = temp_root("guidance-events-tail-recovery");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let plan_file = root.join("plan.json");
    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-tail",
        "module_id": "app-core",
        "objective": "quarantine partial event tail",
        "scope_paths": ["playground/experiments/input.txt"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::create_dir_all(root.join("playground/experiments")).unwrap();
    fs::write(root.join("playground/experiments/input.txt"), "v1\n").unwrap();
    fs::write(
        &plan_file,
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-tail",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    let events = root.join(".appsdk-control/guidance/task-tail/events.jsonl");
    let content = fs::read_to_string(&events).unwrap();
    fs::write(&events, format!("{}not-json", content)).unwrap();

    let status = run(&["guide", "next", root_text, "--task", "task-tail"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let payload: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["reason_code"], "NEXT_STEP_READY");
    assert!(fs::read_to_string(&events).unwrap().ends_with('\n'));
    assert!(fs::read_dir(events.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with("events.corrupt-tail.")));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_plan_rebuilds_from_journal_after_plan_cache_loss() {
    let root = temp_root("guidance-plan-cache-loss");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-cache-loss",
        "module_id": "app-core",
        "objective": "rebuild plan from journal after crash before plan cache",
        "scope_paths": ["playground/experiments/input.txt"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::create_dir_all(root.join("playground/experiments")).unwrap();
    fs::write(root.join("playground/experiments/input.txt"), "v1\n").unwrap();
    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-cache-loss",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    let plan_cache = root.join(".appsdk-control/guidance/task-cache-loss/plan.json");
    assert!(plan_cache.is_file());
    fs::remove_file(&plan_cache).unwrap();

    let status = run(&["guide", "next", root_text, "--task", "task-cache-loss"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let payload: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["reason_code"], "NEXT_STEP_READY");
    assert!(plan_cache.is_file());
    let rebuilt: Value = serde_json::from_str(&fs::read_to_string(&plan_cache).unwrap()).unwrap();
    assert_eq!(rebuilt["task_id"], "task-cache-loss");
    assert!(!rebuilt["plan_hash"].as_str().unwrap().is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_plan_quarantines_invalid_cache_and_rebuilds_from_journal() {
    let root = temp_root("guidance-plan-cache-invalid");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-cache-invalid",
        "module_id": "app-core",
        "objective": "quarantine invalid plan cache and rebuild from journal",
        "scope_paths": ["playground/experiments/input.txt"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::create_dir_all(root.join("playground/experiments")).unwrap();
    fs::write(root.join("playground/experiments/input.txt"), "v1\n").unwrap();
    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-cache-invalid",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    let control_dir = root.join(".appsdk-control/guidance/task-cache-invalid");
    fs::write(control_dir.join("plan.json"), "not-json\n").unwrap();

    let status = run(&["guide", "next", root_text, "--task", "task-cache-invalid"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let payload: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["reason_code"], "NEXT_STEP_READY");
    let plan_cache = control_dir.join("plan.json");
    assert!(plan_cache.is_file());
    let rebuilt: Value = serde_json::from_str(&fs::read_to_string(&plan_cache).unwrap()).unwrap();
    assert_eq!(rebuilt["task_id"], "task-cache-invalid");
    assert!(fs::read_dir(&control_dir)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with("plan.invalid.")));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_plan_rebuilds_when_cache_is_stale() {
    let root = temp_root("guidance-plan-cache-stale");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-cache-stale",
        "module_id": "app-core",
        "objective": "journal objective",
        "scope_paths": ["playground/experiments/input.txt"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::create_dir_all(root.join("playground/experiments")).unwrap();
    fs::write(root.join("playground/experiments/input.txt"), "v1\n").unwrap();
    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-cache-stale",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    let control_dir = root.join(".appsdk-control/guidance/task-cache-stale");
    let plan_cache = control_dir.join("plan.json");
    let mut stale =
        serde_json::from_str::<Value>(&fs::read_to_string(&plan_cache).unwrap()).unwrap();
    stale["objective"] = Value::String("stale objective".into());
    stale["plan_hash"] = Value::String("sha256:stale".into());
    fs::write(
        &plan_cache,
        serde_json::to_string_pretty(&stale).unwrap() + "\n",
    )
    .unwrap();

    let status = run(&["guide", "next", root_text, "--task", "task-cache-stale"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let payload: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["reason_code"], "NEXT_STEP_READY");
    let rebuilt: Value = serde_json::from_str(&fs::read_to_string(&plan_cache).unwrap()).unwrap();
    assert_eq!(rebuilt["objective"], "journal objective");
    assert!(fs::read_dir(&control_dir)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with("plan.stale.")));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_scope_state_detects_content_change_without_git_status_shape_change() {
    let root = temp_root("guidance-scope-content-drift");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    let scope_dir = root.join("playground/experiments");
    fs::create_dir_all(&scope_dir).unwrap();
    fs::write(scope_dir.join("input.txt"), "v1\n").unwrap();
    init_git(&root);

    let plan_file = root.join("plan.json");
    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-content-drift",
        "module_id": "app-core",
        "objective": "detect same-path file content changes",
        "scope_paths": ["playground/experiments/input.txt"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::write(
        &plan_file,
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-content-drift",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    fs::write(scope_dir.join("input.txt"), "v2\n").unwrap();
    let status = run(&["guide", "next", root_text, "--task", "task-content-drift"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let payload: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["reason_code"], "GUIDANCE_CONTEXT_DRIFT:source");
    assert_eq!(payload["next"]["revision_reason"], "source_drift");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_plan_revision_requires_reason_and_preserves_history() {
    let root = temp_root("guidance-plan-revision");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let plan_file = root.join("plan.json");
    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-revision",
        "module_id": "app-core",
        "objective": "initial objective",
        "scope_paths": ["playground/experiments/**"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::write(
        &plan_file,
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-revision",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    let mut revised = proposal.clone();
    revised["objective"] = Value::String("revised objective".into());
    fs::write(
        &plan_file,
        serde_json::to_string_pretty(&revised).unwrap() + "\n",
    )
    .unwrap();
    let missing_reason = run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-revision",
        "--input",
        "plan.json",
    ]);
    assert!(!missing_reason.status.success());
    assert!(String::from_utf8_lossy(&missing_reason.stderr)
        .contains("GUIDANCE_PLAN_REVISION_REASON_REQUIRED"));

    revised["revision_reason"] = Value::String("new_evidence".into());
    fs::write(
        &plan_file,
        serde_json::to_string_pretty(&revised).unwrap() + "\n",
    )
    .unwrap();
    let accepted = run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-revision",
        "--input",
        "plan.json",
    ]);
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );

    let events =
        fs::read_to_string(root.join(".appsdk-control/guidance/task-revision/events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["record_type"], "PlanRecord");
    assert_eq!(events[1]["record_type"], "PlanRevisionRecord");
    assert_eq!(events[1]["reason"], "new_evidence");
    assert_eq!(events[2]["record_type"], "PlanRecord");
    assert_ne!(events[0]["plan_hash"], events[2]["plan_hash"]);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bug_command_lifecycle() {
    let setup_check = run(&["setup-deps", "--check"]);
    assert!(
        setup_check.status.success(),
        "setup-deps failed: {}",
        String::from_utf8_lossy(&setup_check.stderr)
    );
    assert!(String::from_utf8_lossy(&setup_check.stdout).contains("git_bug"));

    let root = temp_root("bug-lifecycle");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("README.md"), "# Bug Test\n").unwrap();
    init_git(&root);

    // Initial bug list should be empty
    let initial_list = run_bug_in(&root, &["bug", "list", "--json"]);
    assert!(
        initial_list.status.success(),
        "{}",
        String::from_utf8_lossy(&initial_list.stderr)
    );
    let list_json: Value = serde_json::from_slice(&initial_list.stdout).unwrap();
    assert_eq!(list_json.as_array().unwrap().len(), 0);

    // Create a new bug
    let created = run_bug_in(
        &root,
        &[
            "bug",
            "new",
            "-t",
            "Test Bug Lifecycle",
            "-m",
            "Testing bug lifecycle tracking",
            "-l",
            "test,p0",
        ],
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let create_json: Value = serde_json::from_slice(&created.stdout).unwrap();
    let bug_id = create_json["id"].as_str().unwrap();
    assert!(!bug_id.is_empty());

    // Filter by label
    let filtered_list = run_bug_in(&root, &["bug", "list", "--json", "-l", "p0"]);
    assert!(filtered_list.status.success());
    let filtered_json: Value = serde_json::from_slice(&filtered_list.stdout).unwrap();
    assert_eq!(filtered_json.as_array().unwrap().len(), 1);
    assert_eq!(filtered_json[0]["title"], "Test Bug Lifecycle");

    // Show bug details
    let show = run_bug_in(&root, &["bug", "show", bug_id, "--json"]);
    assert!(show.status.success());
    let show_json: Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(show_json["title"], "Test Bug Lifecycle");

    // Comment on bug
    let comment = run_bug_in(
        &root,
        &[
            "bug",
            "comment",
            bug_id,
            "-m",
            "Solution verified and implemented",
        ],
    );
    assert!(
        comment.status.success(),
        "{}",
        String::from_utf8_lossy(&comment.stderr)
    );

    // Close bug
    let close = run_bug_in(
        &root,
        &[
            "bug",
            "close",
            bug_id,
            "-m",
            "Solution verified and implemented",
        ],
    );
    assert!(
        close.status.success(),
        "{}",
        String::from_utf8_lossy(&close.stderr)
    );

    // Verify closed status in list
    let closed_list = run_bug_in(&root, &["bug", "list", "--status", "closed", "--json"]);
    assert!(closed_list.status.success());
    let closed_json: Value = serde_json::from_slice(&closed_list.stdout).unwrap();
    assert_eq!(closed_json.as_array().unwrap().len(), 1);
    assert_eq!(closed_json[0]["status"], "closed");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bug_command_upstream_fallback() {
    let setup_check = run(&["setup-deps", "--check"]);
    assert!(setup_check.status.success());

    let upstream_root = temp_root("bug-upstream");
    fs::create_dir_all(&upstream_root).unwrap();
    fs::write(upstream_root.join("README.md"), "# Upstream SDK Repo\n").unwrap();
    init_git(&upstream_root);

    // Create a bug in upstream repo
    let created = Command::new(binary())
        .args(&[
            "bug",
            "new",
            "-t",
            "Upstream Daemon Issue",
            "-m",
            "Daemon lock issue in upstream",
            "-l",
            "upstream,p1",
        ])
        .current_dir(&upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let create_json: Value = serde_json::from_slice(&created.stdout).unwrap();
    let bug_id = create_json["id"].as_str().unwrap();

    let client_root = temp_root("bug-client");
    fs::create_dir_all(&client_root).unwrap();
    fs::write(client_root.join("README.md"), "# Client Project\n").unwrap();
    init_git(&client_root);

    // 1. In client root, upstream reads require an explicit store selector.
    let show = Command::new(binary())
        .args(&["bug", "show", bug_id, "--json", "--upstream"])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let show_json: Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(show_json["title"], "Upstream Daemon Issue");

    // A missing local issue must not be read from the configured upstream
    // store without an explicit selector.
    let implicit_show = Command::new(binary())
        .args(&["bug", "show", bug_id, "--json"])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!implicit_show.status.success());
    assert!(String::from_utf8_lossy(&implicit_show.stderr).contains("GIT_BUG_SHOW_FAILED"));

    // 2. The explicit selector also applies to filtered list reads.
    let list_q = Command::new(binary())
        .args(&["bug", "list", "-q", "Daemon", "--json", "--upstream"])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        list_q.status.success(),
        "{}",
        String::from_utf8_lossy(&list_q.stderr)
    );
    let list_json: Value = serde_json::from_slice(&list_q.stdout).unwrap();
    assert_eq!(list_json.as_array().unwrap().len(), 1);
    assert_eq!(list_json[0]["title"], "Upstream Daemon Issue");

    let implicit_list = Command::new(binary())
        .args(&["bug", "list", "-q", "Daemon", "--json"])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(implicit_list.status.success());
    let implicit_list_json: Value = serde_json::from_slice(&implicit_list.stdout).unwrap();
    assert!(implicit_list_json.as_array().unwrap().is_empty());

    // 3. A comment without --upstream remains local and must not target upstream.
    let local_comment = Command::new(binary())
        .args(&[
            "bug",
            "comment",
            bug_id,
            "-m",
            "Verified in client environment",
        ])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!local_comment.status.success());
    assert!(String::from_utf8_lossy(&local_comment.stderr).contains("GIT_BUG_COMMENT_FAILED"));

    let comment = Command::new(binary())
        .args(&[
            "bug",
            "comment",
            bug_id,
            "-m",
            "Verified in client environment",
            "--upstream",
        ])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        comment.status.success(),
        "{}",
        String::from_utf8_lossy(&comment.stderr)
    );
    let upstream_after_comment = Command::new(binary())
        .args(&["bug", "show", bug_id, "--json", "--upstream"])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    let upstream_json: Value = serde_json::from_slice(&upstream_after_comment.stdout).unwrap();
    assert!(upstream_json["comments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|comment| { comment["message"] == "Verified in client environment" }));

    // 4. Close requires a solution and keeps explicit repository targeting.
    let close = Command::new(binary())
        .args(&[
            "bug",
            "close",
            bug_id,
            "-m",
            "Fixed and verified",
            "--upstream",
        ])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        close.status.success(),
        "{}",
        String::from_utf8_lossy(&close.stderr)
    );

    let _ = fs::remove_dir_all(upstream_root);
    let _ = fs::remove_dir_all(client_root);
}

#[test]
fn bug_reads_external_repo_requires_explicit_upstream_under_local_lock() {
    let home = temp_root("bug-resolution-home");
    let upstream_root = home.join("Documents/github/appsdk");
    fs::create_dir_all(&upstream_root).unwrap();
    fs::write(upstream_root.join("README.md"), "# Upstream SDK Repo\n").unwrap();
    init_git(&upstream_root);

    let create_upstream = Command::new(binary())
        .args([
            "bug",
            "new",
            "-t",
            "Resolved Upstream Issue",
            "-m",
            "Upstream issue selected without APPSDK_ROOT",
        ])
        .current_dir(&upstream_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        create_upstream.status.success(),
        "{}",
        String::from_utf8_lossy(&create_upstream.stderr)
    );
    let upstream_bug: Value = serde_json::from_slice(&create_upstream.stdout).unwrap();
    let bug_id = upstream_bug["id"].as_str().unwrap().to_string();

    let client_root = temp_root("bug-resolution-client");
    fs::create_dir_all(&client_root).unwrap();
    fs::write(client_root.join("README.md"), "# Client Repo\n").unwrap();
    init_git(&client_root);
    let create_local = Command::new(binary())
        .args(["bug", "new", "-t", "Local Issue", "-m", "must not be read"])
        .current_dir(&client_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(create_local.status.success());

    // The default writer and a successful local read share the client store,
    // even when an upstream store is discoverable through HOME.
    let local_list = Command::new(binary())
        .args(["bug", "list", "--json"])
        .current_dir(&client_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        local_list.status.success(),
        "{}",
        String::from_utf8_lossy(&local_list.stderr)
    );
    let local_list_json: Value = serde_json::from_slice(&local_list.stdout).unwrap();
    assert!(local_list_json
        .as_array()
        .unwrap()
        .iter()
        .any(|bug| bug["title"] == "Local Issue"));

    let lock_path = client_root.join(".git/git-bug/lock");
    let held = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .unwrap();
    hold_advisory_lock(&held);

    let list_child = Command::new(binary())
        .args(["bug", "list", "--json"])
        .current_dir(&client_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let show_child = Command::new(binary())
        .args(["bug", "show", &bug_id, "--json"])
        .current_dir(&client_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let list = list_child.wait_with_output().unwrap();
    assert!(!list.status.success());
    assert!(String::from_utf8_lossy(&list.stderr).contains("GIT_BUG_LIST_FAILED"));

    let show = show_child.wait_with_output().unwrap();
    assert!(!show.status.success());
    assert!(String::from_utf8_lossy(&show.stderr).contains("GIT_BUG_SHOW_FAILED"));

    // A locked local store must not turn a default comment into an upstream
    // mutation; upstream writes remain explicit.
    let local_comment = Command::new(binary())
        .args(["bug", "comment", &bug_id, "-m", "external client comment"])
        .current_dir(&client_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!local_comment.status.success());
    assert!(String::from_utf8_lossy(&local_comment.stderr).contains("GIT_BUG_COMMENT_FAILED"));

    let comment = Command::new(binary())
        .args([
            "bug",
            "comment",
            &bug_id,
            "-m",
            "external client comment",
            "--upstream",
        ])
        .current_dir(&client_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        comment.status.success(),
        "{}",
        String::from_utf8_lossy(&comment.stderr)
    );
    drop(held);

    let upstream_after_comment = Command::new(binary())
        .args(["bug", "show", &bug_id, "--json", "--upstream"])
        .current_dir(&client_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    let upstream_json: Value = serde_json::from_slice(&upstream_after_comment.stdout).unwrap();
    assert!(upstream_json["comments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|comment| { comment["message"] == "external client comment" }));

    let missing_home = temp_root("bug-resolution-missing-home");
    let missing_client = temp_root("bug-resolution-missing-client");
    fs::create_dir_all(&missing_client).unwrap();
    fs::write(
        missing_client.join("README.md"),
        "# Missing Upstream Client\n",
    )
    .unwrap();
    init_git(&missing_client);
    let missing = Command::new(binary())
        .args(["bug", "list", "--json", "--upstream"])
        .current_dir(&missing_client)
        .env("HOME", &missing_home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("APPSDK_UPSTREAM_REPO_NOT_FOUND"));

    fs::remove_dir_all(home).unwrap();
    fs::remove_dir_all(client_root).unwrap();
    let _ = fs::remove_dir_all(missing_home);
    fs::remove_dir_all(missing_client).unwrap();
}

#[test]
fn bug_close_comment_failure_is_explicit() {
    let root = temp_root("bug-close-comment-failure");
    fs::create_dir_all(&root).unwrap();
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_git_bug = fake_bin.join("git-bug");
    fs::write(
        &fake_git_bug,
        r#"#!/bin/sh
case "$1 $2 $3" in
  "user -f json")
    printf '%s\n' '[{"id":"user-1"}]'
    exit 0
    ;;
  "bug comment new")
    echo "solution comment write failed" >&2
    exit 2
    ;;
  "bug status close")
    echo "should not close"
    exit 0
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_git_bug, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", fake_bin.display());

    let res = Command::new(binary())
        .args(["bug", "close", "abc123", "-m", "Solution verified"])
        .current_dir(&root)
        .env("PATH", &path)
        .env("HOME", &home)
        .env_remove("GIT_BUG_BIN")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!res.status.success());
    let stderr = String::from_utf8_lossy(&res.stderr);
    assert!(stderr.contains("GIT_BUG_COMMENT_FAILED"), "{}", stderr);
    assert!(!stderr.contains("should not close"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_show_briefs_role_fleet_and_notification_rules() {
    let root = temp_root("longhorizon-show");
    fs::create_dir_all(root.join(".appsdk-control")).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    printf '%s\n' '{"workers":[{"id":"master-peer","active_task":null,"endpoint_live":true,"identity_valid":true,"suspected_offline":false,"agent_state":"waiting"}],"tasks":[],"subagents":[]}'
    ;;
  "master status")
    printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true,"pane":"%42"}}'
    ;;
  "context ")
    printf '%s\n' '{"identity":{"worker_id":"master-peer","pane":"%42"},"tasks":[],"inbox":{"unread":0}}'
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    // Without a registered goal the briefing still teaches the contract and
    // tells the master how to register one.
    let bare = Command::new(binary())
        .args(["longhorizon", "show"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        bare.status.success(),
        "{}",
        String::from_utf8_lossy(&bare.stderr)
    );
    let bare_text = String::from_utf8_lossy(&bare.stdout);
    assert!(bare_text.contains("你的主要任务不是写代码"));
    assert!(bare_text.contains("goal subscribe"));

    // A typo must not be silently swallowed as a project path.
    let typo = run_in(&root, &["longhorizon", "bogus"]);
    assert!(!typo.status.success());
    assert!(String::from_utf8_lossy(&typo.stderr).contains("UNKNOWN_LONGHORIZON_SUBCOMMAND"));

    let goal_file = root.join("plan.md");
    fs::write(
        &goal_file,
        "---\ntitle: t\n---\n\n# Ship It\n\n推动目标完成。\n",
    )
    .unwrap();
    fs::write(
        root.join(".appsdk-control/long-task-goal.json"),
        format!(
            r#"{{"schema_version":1,"goal_path":"{}","interval":"10m","active":true,"registered_at":"2026-09-07T00:00:00Z"}}"#,
            goal_file.display()
        ),
    )
    .unwrap();

    let res = Command::new(binary())
        .args(["longhorizon", "show"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        res.status.success(),
        "{}",
        String::from_utf8_lossy(&res.stderr)
    );
    let text = String::from_utf8_lossy(&res.stdout);

    // Charter, fleet rules and notification rules all travel with the wake.
    assert!(text.contains("不要空转，不要假装完成"));
    assert!(text.contains("最多 5 个"));
    assert!(text.contains("绝不能以 ACK、已读或一段总结结束一轮"));
    assert!(text.contains("collab worker close"));
    assert!(text.contains("collab subagent snapshot"));

    // Goal objective is read out of the markdown, past the frontmatter.
    assert!(text.contains("# Ship It"));
    assert!(text.contains("推动目标完成。"));
    assert!(!text.contains("title: t"));

    let json_res = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(json_res.status.success());
    let payload: Value = serde_json::from_slice(&json_res.stdout).unwrap();
    assert_eq!(payload["role"], "master");
    assert_eq!(payload["goal"]["registered"], true);
    assert_eq!(payload["goal"]["interval"], "10m");
    assert!(payload["charter"].as_str().unwrap().contains("调度"));
    assert!(payload["fleet_rules"].as_str().unwrap().contains("5 个"));
    assert!(payload["notification_rules"]
        .as_str()
        .unwrap()
        .contains("P0"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_show_never_upgrades_worker_or_unknown_to_master() {
    let root = temp_root("longhorizon-role-projection");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    printf '%s\n' '{"workers":[{"id":"worker-peer","role":"peer","active_task":"task-1","endpoint_live":true,"identity_valid":true,"suspected_offline":false,"agent_state":"working"}],"tasks":[{"id":"task-1","status":"working","owner":"worker-peer","next_step":"run tests"}],"subagents":[]}'
    ;;
  "master status")
    printf '%s\n' '{"master":{"worker_id":"other-peer","endpoint_live":true,"pane":"%42"}}'
    ;;
  "context ")
    printf '%s\n' '{"identity":{"worker_id":"worker-peer"},"tasks":[{"id":"task-1"}],"inbox":{"unread":0}}'
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let worker = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        worker.status.success(),
        "{}",
        String::from_utf8_lossy(&worker.stderr)
    );
    let payload: Value = serde_json::from_slice(&worker.stdout).unwrap();
    assert_eq!(payload["role"], "worker");
    assert!(!payload["charter"]
        .as_str()
        .unwrap()
        .contains("你是本项目的 master"));
    assert!(payload["charter"].as_str().unwrap().contains("独立 worker"));

    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    printf '%s\n' '{"workers":[],"tasks":[],"subagents":[]}'
    ;;
  "context ")
    exit 1
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    let unknown = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        unknown.status.success(),
        "{}",
        String::from_utf8_lossy(&unknown.stderr)
    );
    let payload: Value = serde_json::from_slice(&unknown.stdout).unwrap();
    assert_eq!(payload["role"], "unknown");
    assert!(payload["charter"].as_str().unwrap().contains("身份未验证"));
    assert!(!payload["charter"]
        .as_str()
        .unwrap()
        .contains("你是本项目的 master"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_show_requires_authoritative_master_identity() {
    let root = temp_root("longhorizon-master-authority");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    case "${ROLE_CASE:-master}" in
      worker) printf '%s\n' '{"workers":[{"id":"current-peer","role":"peer","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
      subagent) printf '%s\n' '{"workers":[],"tasks":[],"subagents":[{"peer":"current-peer","status":"working"}]}' ;;
      *) printf '%s\n' '{"workers":[{"id":"current-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
    esac
    ;;
  "master status")
    case "${ROLE_CASE:-master}" in
      master) printf '%s\n' '{"master":{"worker_id":"current-peer","endpoint_live":true,"pane":"%42"}}' ;;
      mismatch) printf '%s\n' '{"master":{"worker_id":"other-peer","endpoint_live":true,"pane":"%42"}}' ;;
      pane-mismatch) printf '%s\n' '{"master":{"worker_id":"current-peer","endpoint_live":true,"pane":"%43"}}' ;;
      *) exit 44 ;;
    esac
    ;;
  "context ") printf '%s\n' '{"identity":{"worker_id":"current-peer","pane":"%42"}}' ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let run = |case_name: &str| {
        Command::new(binary())
            .args(["longhorizon", "show", "--json"])
            .current_dir(&root)
            .env("PATH", &fake_bin)
            .env("ROLE_CASE", case_name)
            .env_remove("TMUX_PANE")
            .output()
            .unwrap()
    };
    for (case_name, expected_role) in [
        ("master", "master"),
        ("worker", "unknown"),
        ("subagent", "managed-subagent"),
        ("missing", "unknown"),
        ("mismatch", "worker"),
        ("pane-mismatch", "unknown"),
    ] {
        let result = run(case_name);
        assert!(
            result.status.success(),
            "case={case_name} stderr={}",
            String::from_utf8_lossy(&result.stderr)
        );
        let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(payload["role"], expected_role, "case={case_name}");
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_show_rejects_invalid_worker_before_master_match() {
    let root = temp_root("longhorizon-invalid-master-worker");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    case "${STATUS_VARIANT:-endpoint}" in
      endpoint) printf '%s\n' '{"workers":[{"id":"current-peer","endpoint_live":false,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
      identity) printf '%s\n' '{"workers":[{"id":"current-peer","endpoint_live":true,"identity_valid":false,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
      offline) printf '%s\n' '{"workers":[{"id":"current-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":true}],"tasks":[],"subagents":[]}' ;;
    esac
    ;;
  "master status") printf '%s\n' '{"master":{"worker_id":"current-peer","endpoint_live":true,"pane":"%42"}}' ;;
  "context ") printf '%s\n' '{"identity":{"worker_id":"current-peer","pane":"%42"}}' ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    for variant in ["endpoint", "identity", "offline"] {
        let result = Command::new(binary())
            .args(["longhorizon", "show", "--json"])
            .current_dir(&root)
            .env("PATH", &fake_bin)
            .env("STATUS_VARIANT", variant)
            .env_remove("TMUX_PANE")
            .output()
            .unwrap();
        assert!(result.status.success(), "variant={variant}");
        let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(payload["role"], "unknown", "variant={variant}");
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_show_never_masks_bug_read_failures() {
    let root = temp_root("longhorizon-bug-read");
    fs::create_dir_all(&root).unwrap();
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    printf '%s\n' '{"workers":[],"tasks":[],"subagents":[]}'
    ;;
  "context ")
    exit 1
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", fake_bin.display());

    let missing = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &path)
        .env("HOME", &home)
        .env_remove("GIT_BUG_BIN")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        missing.status.success(),
        "{}",
        String::from_utf8_lossy(&missing.stderr)
    );
    let payload: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(payload["open_bugs"].as_array().unwrap().len(), 0);
    assert!(
        payload["open_bugs_error"]
            .as_str()
            .unwrap()
            .contains("GIT_BUG_NOT_FOUND"),
        "{}",
        payload["open_bugs_error"]
    );

    let text = Command::new(binary())
        .args(["longhorizon", "show"])
        .current_dir(&root)
        .env("PATH", &path)
        .env("HOME", &home)
        .env_remove("GIT_BUG_BIN")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(text.status.success());
    assert!(String::from_utf8_lossy(&text.stdout).contains("开放缺陷读取失败"));

    let fake_git_bug = fake_bin.join("git-bug");
    fs::write(
        &fake_git_bug,
        r#"#!/bin/sh
case "$1 $2" in
  "bug --status")
    printf '%s\n' 'not-json'
    exit 0
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_git_bug, fs::Permissions::from_mode(0o755)).unwrap();

    let invalid = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &path)
        .env("HOME", &home)
        .env_remove("GIT_BUG_BIN")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        invalid.status.success(),
        "{}",
        String::from_utf8_lossy(&invalid.stderr)
    );
    let payload: Value = serde_json::from_slice(&invalid.stdout).unwrap();
    assert_eq!(payload["open_bugs"].as_array().unwrap().len(), 0);
    assert!(
        payload["open_bugs_error"]
            .as_str()
            .unwrap()
            .contains("GIT_BUG_OPEN_JSON_INVALID"),
        "{}",
        payload["open_bugs_error"]
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_subscription_and_master_prompt_lifecycle() {
    let root = temp_root("goal-sub");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "notify subscribe")
    printf '%s\n' "$*" > notify-args
    case "$*" in
      *"--at-ms "*) ;;
      *) printf '%s\n' 'goal deadline requires --at-ms' >&2; exit 42 ;;
    esac
    case "$*" in
      *"--every-ms"*|*"--repeat-count"*) printf '%s\n' 'periodic goal deadline is invalid' >&2; exit 43 ;;
    esac
    while [ "$#" -gt 0 ]; do
      if [ "$1" = "--subject" ]; then printf '%s' "$2" > goal-subject; fi
      shift
    done
    if [ "${UNARMED_SUBSCRIBE:-}" = "1" ]; then
      printf '%s\n' '{"subscription_id":"goal-sub-unarmed","status":"pending"}'
    else
      printf '%s\n' '{"subscription_id":"goal-sub-1"}'
    fi
    ;;
  "notify status")
    subject=$(/bin/cat goal-subject 2>/dev/null || printf '%s' 'missing-subject')
    if [ "${RECONCILE_CANCEL:-}" = "1" ]; then
      printf '%s\n' "{\"subscriptions\":[{\"id\":\"goal-sub-2\",\"status\":\"armed\",\"event\":\"deadline\",\"subject\":\"$subject\"}]}"
    elif [ "${DUPLICATE_SUBJECT:-}" = "1" ]; then
      printf '%s\n' "{\"subscriptions\":[{\"id\":\"goal-sub-1\",\"status\":\"armed\",\"event\":\"deadline\",\"subject\":\"$subject\"},{\"id\":\"goal-sub-2\",\"status\":\"armed\",\"event\":\"deadline\",\"subject\":\"$subject\"}]}"
    else
      printf '%s\n' "{\"subscriptions\":[{\"id\":\"goal-sub-1\",\"status\":\"armed\",\"event\":\"deadline\",\"subject\":\"$subject\"}]}"
    fi
    ;;
  "status --all")
    printf '%s\n' '{"workers":[{"id":"master-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}'
    ;;
  "master status")
    printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true,"pane":"%42"}}'
    ;;
  "context ")
    printf '%s\n' '{"identity":{"worker_id":"master-peer","pane":"%42"}}'
    ;;
  "notify unsubscribe")
    if [ "${RECONCILE_CANCEL:-}" = "1" ]; then
      if [ "$3" = "goal-sub-1" ]; then exit 44; fi
      printf '%s\n' '{"subscription_id":"goal-sub-2","status":"cancelled"}'
      exit 0
    fi
    printf '%s\n' '{"subscription_id":"goal-sub-1","status":"cancelled"}'
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    // 1. Non-md file should fail
    let non_md = root.join("goal.txt");
    fs::write(&non_md, "some goal").unwrap();
    let res_non_md = run_in(&root, &["goal", "subscribe", "--goal", "goal.txt"]);
    assert!(!res_non_md.status.success());
    assert!(String::from_utf8_lossy(&res_non_md.stderr).contains("GOAL_PATH_MUST_BE_MD_FILE"));

    // 2. Non-existent md file should fail
    let res_not_found = run_in(&root, &["goal", "subscribe", "--goal", "missing-goal.md"]);
    assert!(!res_not_found.status.success());
    assert!(String::from_utf8_lossy(&res_not_found.stderr).contains("GOAL_FILE_NOT_FOUND"));

    // 3. Valid md goal registration with interval
    let valid_goal = root.join("long-task.md");
    fs::write(
        &valid_goal,
        "# Sample Long-Horizon Goal\nDeliver feature X.\n",
    )
    .unwrap();

    let sub_res = Command::new(binary())
        .args([
            "goal",
            "subscribe",
            "--goal",
            "long-task.md",
            "--interval",
            "5m",
            "--json",
        ])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        sub_res.status.success(),
        "{}",
        String::from_utf8_lossy(&sub_res.stderr)
    );
    let sub_json: Value = serde_json::from_slice(&sub_res.stdout).unwrap();
    assert_eq!(sub_json["active"], true);
    assert_eq!(sub_json["interval"], "5m");
    assert_eq!(sub_json["every_ms"], 300000);
    assert_eq!(sub_json["desired"], "subscribed");
    assert_eq!(sub_json["observed"], "subscribed");
    assert_eq!(sub_json["goal_id"], sub_json["goal_id"]);
    assert!(sub_json["goal_id"].as_str().unwrap().starts_with("sha256:"));
    let prompt = sub_json["master_prompt"].as_str().unwrap();
    assert!(prompt.contains("Master 专属"));
    assert!(prompt.contains("饱和"));
    assert!(prompt.contains("appsdk bug"));
    assert_eq!(sub_json["schedule"], "one-shot");
    assert_eq!(sub_json["local_schedule"], "periodic-rearm-intent");
    assert_eq!(sub_json["repeat_count"], 1);
    assert_eq!(sub_json["requested_repeat_count"], 100);
    assert!(sub_json["subject"]
        .as_str()
        .unwrap()
        .starts_with("goal:sha256:"));
    assert!(prompt.contains("每次 Collab deadline 都是单次触发"));
    let notify_args = fs::read_to_string(root.join("notify-args")).unwrap();
    assert!(notify_args.contains("--at-ms "));
    assert!(!notify_args.contains("--every-ms"));
    assert!(!notify_args.contains("--repeat-count"));

    let duplicate = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("DUPLICATE_SUBJECT", "1")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(duplicate.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("GOAL_RECONCILE_SUBJECT_AMBIGUOUS"));

    let rearm = Command::new(binary())
        .args([
            "goal",
            "subscribe",
            "--goal",
            "long-task.md",
            "--interval",
            "5m",
            "--json",
        ])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        rearm.status.success(),
        "{}",
        String::from_utf8_lossy(&rearm.stderr)
    );
    let rearm_json: Value = serde_json::from_slice(&rearm.stdout).unwrap();
    assert_eq!(rearm_json["desired"], "subscribed");

    // 4. Check goal status
    let status_res = Command::new(binary())
        .args(["goal", "status", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(status_res.status.success());
    let status_json: Value = serde_json::from_slice(&status_res.stdout).unwrap();
    assert_eq!(status_json["active"], true);
    assert_eq!(status_json["interval"], "5m");
    assert_eq!(status_json["desired"], "subscribed");
    assert_eq!(status_json["observed"], "subscribed");

    let record_path = root.join(".appsdk-control/long-task-goal.json");
    let mut record: Value =
        serde_json::from_str(&fs::read_to_string(&record_path).unwrap()).unwrap();
    record["subscription_id"] = Value::Null;
    record["collab_subscription"] = Value::Null;
    fs::write(&record_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    let status_reconciled = Command::new(binary())
        .args(["goal", "status", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(status_reconciled.status.success());
    let status_reconciled_json: Value = serde_json::from_slice(&status_reconciled.stdout).unwrap();
    assert_eq!(status_reconciled_json["active"], true);
    assert_eq!(status_reconciled_json["subscription_id"], "goal-sub-1");

    // 5. Check standalone prompt command
    let prompt_res = Command::new(binary())
        .args([
            "goal",
            "prompt",
            "--goal",
            "long-task.md",
            "--interval",
            "10m",
        ])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(prompt_res.status.success());
    let prompt_text = String::from_utf8_lossy(&prompt_res.stdout);
    assert!(prompt_text.contains("长程任务目标文档"));
    assert!(prompt_text.contains("10m"));
    assert!(prompt_text.contains("本地重唤醒意图"));

    // 6. Cancel goal
    let cancel_res = Command::new(binary())
        .args(["goal", "cancel"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("RECONCILE_CANCEL", "1")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(cancel_res.status.success());
    assert!(String::from_utf8_lossy(&cancel_res.stdout).contains("goal-sub-2"));

    let cancel_again = Command::new(binary())
        .args(["goal", "cancel", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(cancel_again.status.success());
    let cancel_again_json: Value = serde_json::from_slice(&cancel_again.stdout).unwrap();
    assert_eq!(cancel_again_json["idempotent"], true);
    assert_eq!(cancel_again_json["status"], "cancelled");
    assert!(cancel_again_json["cancel_receipt"].is_object());
    assert!(cancel_again_json["revision"].as_u64().unwrap() >= 3);

    let post_cancel_status = run_in(&root, &["goal", "status", "--json"]);
    assert!(post_cancel_status.status.success());
    let post_cancel_json: Value = serde_json::from_slice(&post_cancel_status.stdout).unwrap();
    assert_eq!(post_cancel_json["active"], false);
    assert_eq!(post_cancel_json["desired"], "unsubscribed");
    assert_eq!(post_cancel_json["observed"], "cancelled");
    assert_eq!(post_cancel_json["subscription_id"], "goal-sub-2");
    assert!(post_cancel_json["record"]["cancel_receipt"].is_object());

    let unarmed = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("UNARMED_SUBSCRIBE", "1")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(unarmed.status.code(), Some(1));
    let unarmed_json: Value = serde_json::from_slice(&unarmed.stdout).unwrap();
    assert_eq!(unarmed_json["active"], false);
    assert_eq!(unarmed_json["desired"], "recovery_required");
    assert_eq!(unarmed_json["observed"], "unknown");
    assert_eq!(unarmed_json["subscription_id"], "goal-sub-unarmed");
    assert!(unarmed_json["error"]
        .as_str()
        .unwrap()
        .contains("GOAL_ONE_SHOT_SUBSCRIPTION_NOT_ARMED:pending"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_owner_gate_rejects_authoritative_master_mismatch() {
    let root = temp_root("goal-owner-gate-mismatch");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    case "${STATUS_VARIANT:-live}" in
      stale) printf '%s\n' '{"workers":[{"id":"master-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":true}],"tasks":[],"subagents":[]}' ;;
      *) printf '%s\n' '{"workers":[{"id":"master-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
    esac
    ;;
  "master status")
    printf '%s\n' "{\"master\":{\"worker_id\":\"${MASTER_WORKER:-master-peer}\",\"endpoint_live\":${MASTER_LIVE:-true},\"pane\":\"${MASTER_PANE:-%42}\"}}"
    ;;
  "context ")
    printf '%s\n' "{\"identity\":{\"worker_id\":\"master-peer\",\"pane\":\"${CONTEXT_PANE:-%42}\"}}"
    ;;
  "notify status") printf '%s\n' '{"subscriptions":[]}' ;;
  "notify subscribe") printf '%s\n' '{"subscription_id":"owner-gate-sub"}' ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let run = |extra_env: &[(&str, &str)]| {
        let mut command = Command::new(binary());
        command
            .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
            .current_dir(&root)
            .env("PATH", &fake_bin)
            .env_remove("TMUX_PANE");
        for (key, value) in extra_env {
            command.env(key, value);
        }
        command.output().unwrap()
    };

    let identity_mismatch = run(&[("MASTER_WORKER", "other-peer")]);
    assert_eq!(identity_mismatch.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&identity_mismatch.stderr).contains("GOAL_OWNER_IDENTITY_MISMATCH")
    );

    let pane_mismatch = run(&[("MASTER_PANE", "%43")]);
    assert_eq!(pane_mismatch.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&pane_mismatch.stderr).contains("GOAL_OWNER_PANE_MISMATCH"));

    let offline_master = run(&[("MASTER_LIVE", "false")]);
    assert_eq!(offline_master.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&offline_master.stderr).contains("GOAL_OWNER_MASTER_NOT_LIVE"));

    let stale_master_projection = run(&[("STATUS_VARIANT", "stale")]);
    assert!(
        stale_master_projection.status.success(),
        "{}",
        String::from_utf8_lossy(&stale_master_projection.stderr)
    );
    let stale_payload: Value = serde_json::from_slice(&stale_master_projection.stdout).unwrap();
    assert_eq!(stale_payload["active"], true);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_owner_gate_rejects_missing_worker_liveness_fields() {
    let root = temp_root("goal-owner-gate-missing-fields");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    case "${STATUS_VARIANT:-complete}" in
      endpoint) printf '%s\n' '{"workers":[{"id":"master-peer","identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
      identity) printf '%s\n' '{"workers":[{"id":"master-peer","endpoint_live":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
      offline) printf '%s\n' '{"workers":[{"id":"master-peer","endpoint_live":true,"identity_valid":true}],"tasks":[],"subagents":[]}' ;;
      *) printf '%s\n' '{"workers":[{"id":"master-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
    esac
    ;;
  "master status") printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true,"pane":"%42"}}' ;;
  "context ") printf '%s\n' '{"identity":{"worker_id":"master-peer","pane":"%42"}}' ;;
  "notify subscribe") printf '%s\n' '{"subscription_id":"missing-fields-sub"}' ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let run = |variant: &str| {
        Command::new(binary())
            .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
            .current_dir(&root)
            .env("PATH", &fake_bin)
            .env("STATUS_VARIANT", variant)
            .env_remove("TMUX_PANE")
            .output()
            .unwrap()
    };

    for (variant, expected) in [
        ("endpoint", "GOAL_OWNER_NOT_LIVE"),
        ("identity", "GOAL_OWNER_IDENTITY_INVALID"),
        ("offline", "GOAL_OWNER_SUSPECTED_OFFLINE"),
    ] {
        let result = run(variant);
        assert_eq!(result.status.code(), Some(1), "variant={variant}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(expected),
            "variant={variant} stderr={}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_subscribe_drains_large_collab_output_without_timeout() {
    let root = temp_root("goal-large-collab-output");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all")
    (
      printf '%s' '{"workers":[{"id":"master-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[],"padding":"'
      /bin/dd if=/dev/zero bs=4194304 count=1 2>/dev/null | /usr/bin/tr '\0' 'x'
      printf '%s\n' '"}'
    ) &
    (
      /bin/dd if=/dev/zero bs=4194304 count=1 2>/dev/null | /usr/bin/tr '\0' 'e' >&2
    ) &
    wait
    ;;
  "master status") printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true,"pane":"%42"}}' ;;
  "context ") printf '%s\n' '{"identity":{"worker_id":"master-peer","pane":"%42"}}' ;;
  "notify subscribe") printf '%s\n' '{"subscription_id":"large-output-sub"}' ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let started = Instant::now();
    let result = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();

    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "large Collab output took {:?}: {}",
        started.elapsed(),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(payload["active"], true);
    assert_eq!(payload["subscription_id"], "large-output-sub");
    assert_eq!(payload["observed"], "subscribed");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_subscribe_allows_slow_collab_write_within_write_budget() {
    let root = temp_root("goal-slow-subscribe-write");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all") printf '%s\n' '{"workers":[{"id":"master-peer","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
  "master status") printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true,"pane":"%42"}}' ;;
  "context ") printf '%s\n' '{"identity":{"worker_id":"master-peer","pane":"%42"}}' ;;
  "notify subscribe")
    /bin/sleep 20
    printf '%s\n' '{"subscription_id":"slow-subscribe-write"}'
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let started = Instant::now();
    let result = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();

    assert!(
        started.elapsed() >= std::time::Duration::from_secs(19),
        "slow Collab write returned too early: {:?}",
        started.elapsed()
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "slow Collab write exceeded write budget: {:?}",
        started.elapsed()
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(payload["active"], true);
    assert_eq!(payload["observed"], "subscribed");
    assert_eq!(payload["subscription_id"], "slow-subscribe-write");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_subscribe_failure_does_not_report_active() {
    let root = temp_root("goal-sub-fail");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::write(
        fake_bin.join("collab"),
        "#!/bin/sh\ncase \"$1 $2\" in\n  \"status --all\") printf '%s\\n' '{\"workers\":[{\"id\":\"master-peer\",\"role\":\"master\",\"endpoint_live\":true,\"identity_valid\":true,\"suspected_offline\":false}],\"tasks\":[],\"subagents\":[]}' ;;\n  \"master status\") printf '%s\\n' '{\"master\":{\"worker_id\":\"master-peer\",\"endpoint_live\":true,\"pane\":\"%42\"}}' ;;\n  \"context \") printf '%s\\n' '{\"identity\":{\"worker_id\":\"master-peer\",\"pane\":\"%42\"}}' ;;\n  *) printf '%s\\n' 'daemon stopped' >&2; exit 44 ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&fake_bin.join("collab"), fs::Permissions::from_mode(0o755)).unwrap();

    let sub = Command::new(binary())
        .args([
            "goal",
            "subscribe",
            "--goal",
            "long-task.md",
            "--interval",
            "5m",
            "--json",
        ])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(sub.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&sub.stderr).contains("COLLAB_SUBSCRIBE_FAILED"));
    let sub_json: Value = serde_json::from_slice(&sub.stdout).unwrap();
    assert_eq!(sub_json["active"], false);
    assert_eq!(sub_json["desired"], "subscribed");
    assert_eq!(sub_json["observed"], "unknown");

    let status = Command::new(binary())
        .args(["goal", "status", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(status.status.success());
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["active"], false);
    assert_eq!(status_json["observed"], "unknown");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_lifecycle_rejects_invalid_subscription_and_preserves_pending_cancel() {
    let root = temp_root("goal-invalid-response");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "notify subscribe")
    printf '%s\n' 'not-json'
    ;;
  "notify status")
    printf '%s\n' 'also-not-json'
    ;;
  "status --all")
    printf '%s\n' '{"workers":[{"id":"master-peer","role":"master","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}'
    ;;
  "master status")
    printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true,"pane":"%42"}}'
    ;;
  "context ")
    printf '%s\n' '{"identity":{"worker_id":"master-peer","pane":"%42"}}'
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let subscribed = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(subscribed.status.code(), Some(1));
    let subscribe_json: Value = serde_json::from_slice(&subscribed.stdout).unwrap();
    assert_eq!(subscribe_json["active"], false);
    assert_eq!(subscribe_json["desired"], "subscribed");
    assert_eq!(subscribe_json["observed"], "unknown");
    assert!(subscribe_json["error"]
        .as_str()
        .unwrap()
        .contains("COLLAB_SUBSCRIBE_RESPONSE_INVALID"));

    let status = Command::new(binary())
        .args(["goal", "status", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(status.status.success());
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["active"], false);
    assert_eq!(status_json["observed"], "unknown");

    let cancel = Command::new(binary())
        .args(["goal", "cancel", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(cancel.status.code(), Some(1));
    let cancel_json: Value = serde_json::from_slice(&cancel.stdout).unwrap();
    assert_eq!(cancel_json["desired"], "cancel_pending");
    assert_eq!(cancel_json["observed"], "unknown");
    assert!(root.join(".appsdk-control/long-task-goal.json").is_file());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_cancel_failure_keeps_exact_subscription_record() {
    let root = temp_root("goal-cancel-failure");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "notify subscribe")
    printf '%s\n' '{"subscription":{"id":"sub-exact","status":"armed"}}'
    ;;
  "notify status")
    printf '%s\n' '{"subscriptions":[{"id":"sub-exact","status":"armed","event":"deadline","subject":"goal:long-task.md"}]}'
    ;;
  "status --all")
    printf '%s\n' '{"workers":[{"id":"master-peer","role":"master","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}'
    ;;
  "master status")
    printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true,"pane":"%42"}}'
    ;;
  "context ")
    printf '%s\n' '{"identity":{"worker_id":"master-peer","pane":"%42"}}'
    ;;
  "notify unsubscribe")
    printf '%s\n' 'unsubscribe failed' >&2
    exit 44
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let subscribed = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(subscribed.status.success());
    let cancel = Command::new(binary())
        .args(["goal", "cancel", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(cancel.status.code(), Some(1));
    let cancel_json: Value = serde_json::from_slice(&cancel.stdout).unwrap();
    assert_eq!(cancel_json["desired"], "cancel_pending");
    assert_eq!(cancel_json["observed"], "unknown");
    assert_eq!(cancel_json["subscription_id"], "sub-exact");
    assert!(root.join(".appsdk-control/long-task-goal.json").is_file());

    let resubscribe = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(resubscribe.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&resubscribe.stderr)
        .contains("GOAL_CANCEL_PENDING_RECONCILIATION_REQUIRED"));
    let resubscribe_json: Value = serde_json::from_slice(&resubscribe.stdout).unwrap();
    assert_eq!(resubscribe_json["desired"], "cancel_pending");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_lifecycle_reconciles_legacy_subject_and_exposes_one_shot_recovery() {
    let root = temp_root("goal-legacy-periodic-recovery");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "status --all") printf '%s\n' '{"workers":[{"id":"master-peer","role":"master","endpoint_live":true,"identity_valid":true,"suspected_offline":false}],"tasks":[],"subagents":[]}' ;;
  "master status") printf '%s\n' '{"master":{"worker_id":"master-peer","endpoint_live":true,"pane":"%42"}}' ;;
  "context ") printf '%s\n' '{"identity":{"worker_id":"master-peer","pane":"%42"}}' ;;
  "notify subscribe") printf '%s\n' '{"subscription_id":"sub-periodic"}' ;;
  "notify status")
    if [ "${STATUS_EXPIRED:-}" = "1" ]; then
      printf '%s\n' '{"subscriptions":[{"id":"sub-periodic","status":"expired","event":"deadline","subject":"goal:sha256:legacy"}]}'
    elif [ "${DEDUPE_SUBJECT:-}" = "1" ]; then
      count=$((`/bin/cat status-count 2>/dev/null || printf '0'` + 1))
      printf '%s' "$count" > status-count
      if [ "$count" = "1" ]; then
        printf '%s\n' '{"subscriptions":[{"id":"sub-periodic","status":"armed","event":"deadline","subject":"goal:legacy-retained"}]}'
      elif [ "$count" = "2" ]; then
        printf '%s\n' '{"subscriptions":[{"id":"sub-periodic","status":"armed","event":"deadline","subject":"goal:long-task.md"}]}'
      else
        printf '%s\n' '{"subscriptions":[{"id":"sub-periodic","status":"armed","event":"deadline","subject":"goal:sha256:current"}]}'
      fi
    elif [ "${LEGACY_SUBJECT:-}" = "1" ]; then
      printf '%s\n' '{"subscriptions":[{"id":"sub-periodic","status":"armed","event":"deadline","subject":"goal:long-task.md"}]}'
    else
      printf '%s\n' '{"subscriptions":[{"id":"sub-periodic","status":"armed","event":"deadline","subject":"goal:sha256:current"}]}'
    fi
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let initial = Command::new(binary())
        .args([
            "goal",
            "subscribe",
            "--goal",
            "long-task.md",
            "--interval",
            "5m",
        ])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let initial_text = String::from_utf8_lossy(&initial.stdout);
    assert!(initial_text.contains(
        "One-shot deadline: first trigger after 5m (local rearm intent: every 5m, up to 100 deliveries; TTL 604800 seconds)"
    ));
    assert!(initial_text.contains("one-shot armed; rearm is explicit and verifiable"));

    let record_path = root.join(".appsdk-control/long-task-goal.json");
    let mut legacy: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    legacy["subject"] = Value::String("goal:long-task.md".into());
    legacy["subscription_id"] = Value::Null;
    legacy["collab_subscription"] = Value::Null;
    fs::write(&record_path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
    let reconciled = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("LEGACY_SUBJECT", "1")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        reconciled.status.success(),
        "{}",
        String::from_utf8_lossy(&reconciled.stderr)
    );
    let reconciled: Value = serde_json::from_slice(&reconciled.stdout).unwrap();
    assert_eq!(reconciled["subject"], "goal:long-task.md");
    assert_eq!(reconciled["subscription_id"], "sub-periodic");

    let mut dedupe_record: Value =
        serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    dedupe_record["subject"] = Value::String("goal:legacy-retained".into());
    dedupe_record["subscription_id"] = Value::Null;
    dedupe_record["collab_subscription"] = Value::Null;
    fs::write(
        &record_path,
        serde_json::to_vec_pretty(&dedupe_record).unwrap(),
    )
    .unwrap();
    let dedupe = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("DEDUPE_SUBJECT", "1")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        dedupe.status.success(),
        "{}",
        String::from_utf8_lossy(&dedupe.stderr)
    );
    let dedupe: Value = serde_json::from_slice(&dedupe.stdout).unwrap();
    assert_eq!(dedupe["subscription_id"], "sub-periodic");
    assert_eq!(dedupe["subject"], "goal:legacy-retained");
    let status_calls: u32 = fs::read_to_string(root.join("status-count"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(status_calls >= 3, "subject candidates were not all queried");

    let expired = Command::new(binary())
        .args(["goal", "status", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env("STATUS_EXPIRED", "1")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        expired.status.success(),
        "{}",
        String::from_utf8_lossy(&expired.stderr)
    );
    let expired: Value = serde_json::from_slice(&expired.stdout).unwrap();
    assert_eq!(expired["desired"], "recovery_required");
    assert_eq!(expired["observed"], "expired");
    assert!(expired["error"]
        .as_str()
        .unwrap()
        .contains("GOAL_ONE_SHOT_SUBSCRIPTION_NOT_ARMED:expired"));
    assert!(
        expired["record"]["recovery"]
            .as_str()
            .unwrap()
            .contains("appsdk goal subscribe"),
        "recovery={:?}",
        expired["record"]["recovery"]
    );

    let rearmed = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        rearmed.status.success(),
        "{}",
        String::from_utf8_lossy(&rearmed.stderr)
    );
    let rearmed: Value = serde_json::from_slice(&rearmed.stdout).unwrap();
    assert_eq!(rearmed["desired"], "subscribed");
    assert!(rearmed["recovery_history"].is_array());

    for (flag, expected) in [
        ("--repeat", "GOAL_REPEAT_COUNT_INVALID: 'oops'"),
        ("--ttl-seconds", "GOAL_TTL_INVALID: 'oops'"),
        ("--ttl-seconds", "GOAL_TTL_INVALID: '0'"),
    ] {
        let value = if expected.contains("'0'") {
            "0"
        } else {
            "oops"
        };
        let invalid = Command::new(binary())
            .args(["goal", "subscribe", "--goal", "long-task.md", flag, value])
            .current_dir(&root)
            .env("PATH", &fake_bin)
            .env_remove("TMUX_PANE")
            .output()
            .unwrap();
        assert_eq!(invalid.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&invalid.stderr).contains(expected));
    }

    let overflow = Command::new(binary())
        .args([
            "goal",
            "subscribe",
            "--goal",
            "long-task.md",
            "--interval",
            "18446744073709551615s",
        ])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(overflow.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&overflow.stderr)
        .contains("GOAL_DURATION_OVERFLOW:18446744073709551615s"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_status_reconciles_armed_subscription_after_recovery_required() {
    let root = temp_root("goal-status-recovery-reconciliation");
    fs::create_dir_all(root.join(".appsdk-control")).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
case "$1 $2" in
  "notify status")
    printf '%s\n' '{"subscriptions":[{"id":"sub-recovered","worker_id":"master-peer","event":"deadline","subject":"goal:retained-subject","status":"armed"}]}'
    ;;
  *) exit 64 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        root.join(".appsdk-control/long-task-goal.json"),
        serde_json::json!({
            "schema_version": 1,
            "goal_id": "sha256:current-goal",
            "goal_path": "long-task.md",
            "subject": "goal:retained-subject",
            "desired": "recovery_required",
            "observed": "unknown",
            "active": false,
            "collab_subscribed": true,
            "collab_subscription": null,
            "subscription_id": null,
            "remote_state": "unknown",
            "error": "GOAL_STATUS_SUBSCRIPTION_ID_MISSING",
            "revision": 3
        })
        .to_string(),
    )
    .unwrap();

    let status = Command::new(binary())
        .args(["goal", "status", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["active"], true);
    assert_eq!(status_json["desired"], "subscribed");
    assert_eq!(status_json["observed"], "subscribed");
    assert_eq!(status_json["subscription_id"], "sub-recovered");
    assert_eq!(status_json["record"]["subject"], "goal:retained-subject");
    assert!(status_json["record"]["error"].is_null());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_subscribe_persistence_failure_retains_reconciliation_state() {
    let root = temp_root("goal-persistence-failure");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Goal\n").unwrap();
    let control_dir = root.join(".appsdk-control");
    fs::create_dir_all(&control_dir).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let marker = root.join("remote-called");
    let fake_touch = fake_bin.join("touch");
    fs::write(
        &fake_touch,
        "#!/bin/sh\n/bin/chmod a-w .appsdk-control\n/usr/bin/touch \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&fake_touch, fs::Permissions::from_mode(0o755)).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n  \"status --all\") printf '%s\\n' '{{\"workers\":[{{\"id\":\"master-peer\",\"role\":\"master\",\"endpoint_live\":true,\"identity_valid\":true,\"suspected_offline\":false}}],\"tasks\":[],\"subagents\":[]}}' ;;\n  \"master status\") printf '%s\\n' '{{\"master\":{{\"worker_id\":\"master-peer\",\"endpoint_live\":true,\"pane\":\"%42\"}}}}' ;;\n  \"context \") printf '%s\\n' '{{\"identity\":{{\"worker_id\":\"master-peer\",\"pane\":\"%42\"}}}}' ;;\n  *) touch '{}' ; printf '%s\\n' '{{\"subscription_id\":\"orphan\"}}' ;;\nesac\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let result = Command::new(binary())
        .args(["goal", "subscribe", "--goal", "long-task.md", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(payload["active"], false);
    assert_eq!(payload["desired"], "subscribed");
    assert_eq!(payload["observed"], "unknown");
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("GOAL_RECORD_WRITE_FAILED"));
    assert_eq!(payload["subscription_id"], "orphan");
    assert!(marker.exists());

    fs::set_permissions(&control_dir, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_stale_lock_is_recovered_after_owner_exit() {
    let root = temp_root("goal-stale-lock");
    let control_dir = root.join(".appsdk-control");
    fs::create_dir_all(&control_dir).unwrap();
    let mut exited = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let stale_pid = exited.id();
    assert!(exited.wait().unwrap().success());
    let lock_path = control_dir.join("long-task-goal.lock");
    fs::write(
        &lock_path,
        format!("pid={} owner=crashed-goal-worker\n", stale_pid),
    )
    .unwrap();

    let status = run_in(&root, &["goal", "status", "--json"]);
    assert!(status.status.success());
    let payload: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["error"], "GOAL_RECORD_NOT_FOUND");
    assert!(!lock_path.exists());

    fs::write(&lock_path, "").unwrap();
    let empty_lock = run_in(&root, &["goal", "status", "--json"]);
    assert!(empty_lock.status.success());
    let empty_payload: Value = serde_json::from_slice(&empty_lock.stdout).unwrap();
    assert_eq!(empty_payload["error"], "GOAL_RECORD_NOT_FOUND");
    assert!(!lock_path.exists());
    let recovery_receipt = fs::read_dir(&control_dir)
        .unwrap()
        .flatten()
        .find_map(|entry| {
            if !entry
                .file_name()
                .to_string_lossy()
                .starts_with("long-task-goal.lock.recovery.")
            {
                return None;
            }
            let receipt: Value = serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()?;
            (receipt["reason"] == "empty metadata").then_some(entry)
        })
        .expect("empty lock recovery receipt");
    let recovery_receipt: Value =
        serde_json::from_slice(&fs::read(recovery_receipt.path()).unwrap()).unwrap();
    assert_eq!(recovery_receipt["status"], "recovered");
    assert_eq!(recovery_receipt["original_metadata"], "");
    assert_eq!(recovery_receipt["reason"], "empty metadata");

    fs::write(&lock_path, "pid=").unwrap();
    let truncated_lock = run_in(&root, &["goal", "status", "--json"]);
    assert!(truncated_lock.status.success());
    let truncated_payload: Value = serde_json::from_slice(&truncated_lock.stdout).unwrap();
    assert_eq!(truncated_payload["error"], "GOAL_RECORD_NOT_FOUND");
    assert!(!lock_path.exists());

    let mut live_owner = Command::new("/bin/sh")
        .args(["-c", "sleep 2"])
        .spawn()
        .unwrap();
    fs::write(
        &lock_path,
        format!("pid={} owner=live-owner\n", live_owner.id()),
    )
    .unwrap();
    let reused_pid = run_in(&root, &["goal", "status", "--json"]);
    assert!(reused_pid.status.success());
    let reused_payload: Value = serde_json::from_slice(&reused_pid.stdout).unwrap();
    assert_eq!(reused_payload["error"], "GOAL_RECORD_NOT_FOUND");

    let held = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .unwrap();
    fs::write(
        &lock_path,
        format!("pid={} owner=live-owner\n", live_owner.id()),
    )
    .unwrap();
    hold_advisory_lock(&held);
    let busy = run_in(&root, &["goal", "status", "--json"]);
    assert_eq!(busy.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&busy.stderr).contains("GOAL_LOCK_BUSY"));
    drop(held);
    live_owner.kill().unwrap();
    live_owner.wait().unwrap();
    let recovered = run_in(&root, &["goal", "status", "--json"]);
    assert!(recovered.status.success());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_subscribe_timeout_failure_remains_explicit() {
    let root = temp_root("goal-sub-timeout");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("long-task.md"), "# Sample Long-Horizon Goal\n").unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::write(
        fake_bin.join("collab"),
        "#!/bin/sh\ncase \"$1 $2\" in\n  \"status --all\") printf '%s\\n' '{\"workers\":[{\"id\":\"master-peer\",\"role\":\"master\",\"endpoint_live\":true,\"identity_valid\":true,\"suspected_offline\":false}],\"tasks\":[],\"subagents\":[]}' ;;\n  \"master status\") printf '%s\\n' '{\"master\":{\"worker_id\":\"master-peer\",\"endpoint_live\":true,\"pane\":\"%42\"}}' ;;\n  \"context \") printf '%s\\n' '{\"identity\":{\"worker_id\":\"master-peer\",\"pane\":\"%42\"}}' ;;\n  \"notify subscribe\") printf '%s\\n' 'collab request timed out' >&2; exit 124 ;;\n  *) printf '%s\\n' 'unexpected collab command' >&2; exit 64 ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&fake_bin.join("collab"), fs::Permissions::from_mode(0o755)).unwrap();

    let result = Command::new(binary())
        .args([
            "goal",
            "subscribe",
            "--goal",
            "long-task.md",
            "--interval",
            "5m",
            "--json",
        ])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("COLLAB_SUBSCRIBE_FAILED"), "{stderr}");
    assert!(stderr.contains("exit=124"), "{stderr}");
    let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(payload["active"], false);
    assert_eq!(payload["desired"], "subscribed");
    assert_eq!(payload["observed"], "unknown");
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("COLLAB_SUBSCRIBE_FAILED:exit=124"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_bug_permission_failure_remains_explicit() {
    let root = temp_root("longhorizon-bug-permission");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        "#!/bin/sh\nprintf '%s\\n' '{\"workers\":[],\"tasks\":[],\"subagents\":[]}'\n",
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();
    let fake_git_bug = fake_bin.join("git-bug");
    fs::write(
        &fake_git_bug,
        "#!/bin/sh\nprintf '%s\\n' 'permission denied' >&2\nexit 126\n",
    )
    .unwrap();
    fs::set_permissions(&fake_git_bug, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", fake_bin.display());

    let result = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &path)
        .env_remove("GIT_BUG_BIN")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();

    assert!(result.status.success());
    let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(payload["open_bugs"].as_array().unwrap().len(), 0);
    assert!(payload["open_bugs_error"]
        .as_str()
        .unwrap()
        .contains("GIT_BUG_OPEN_READ_FAILED"));
    assert!(payload["open_bugs_error"]
        .as_str()
        .unwrap()
        .contains("permission denied"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn longhorizon_bug_read_does_not_fall_back_to_upstream_after_local_failure() {
    let root = temp_root("longhorizon-bug-upstream-fallback");
    fs::create_dir_all(&root).unwrap();
    let home = root.join("home");
    let upstream = home.join("Documents/github/appsdk");
    fs::create_dir_all(&upstream).unwrap();
    fs::write(upstream.join("README.md"), "# Upstream SDK Repo\n").unwrap();
    init_git(&upstream);

    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        "#!/bin/sh\nprintf '%s\\n' '{\"workers\":[],\"tasks\":[],\"subagents\":[]}'\n",
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let fake_git_bug = fake_bin.join("git-bug");
    let fake_git_bug_script = "#!/bin/sh\nprintf '%s\\n' 'permission denied' >&2\nexit 126\n";
    fs::write(&fake_git_bug, fake_git_bug_script).unwrap();
    fs::set_permissions(&fake_git_bug, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:/usr/bin:/bin", fake_bin.display());

    let result = Command::new(binary())
        .args(["longhorizon", "show", "--json"])
        .current_dir(&root)
        .env("PATH", &path)
        .env("HOME", &home)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(result.status.success());
    let payload: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(payload["open_bugs"].as_array().unwrap().is_empty());
    assert!(payload["open_bugs_error"]
        .as_str()
        .unwrap()
        .contains("GIT_BUG_OPEN_READ_FAILED"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn task_block_governance_reminder_lifecycle() {
    let root = temp_root("task-block");
    fs::create_dir_all(&root).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
if [ "$1" = "task" ] && [ "$2" = "block" ] && [ "$3" = "task-404" ]; then
  printf '%s\n' '{"ok":true,"task_id":"task-404","status":"blocked"}'
  exit 0
fi
if [ "$1" = "task" ] && [ "$2" = "block" ] && [ "$3" = "task-405" ]; then
  printf '%s\n' '{"ok":true,"task_id":"task-405","status":"blocked"}'
  exit 0
fi
printf '%s\n' 'task not found'
exit 44
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let block_res = Command::new(binary())
        .args([
            "task",
            "block",
            "task-404",
            "--reason",
            "Waiting on upstream SDK bug fix",
            "--json",
        ])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(block_res.status.success());
    let block_json: Value = serde_json::from_slice(&block_res.stdout).unwrap();
    assert_eq!(block_json["status"], "blocked");
    assert_eq!(block_json["reminders_stopped"], false);
    assert_eq!(block_json["task_id"], "task-404");
    let rule = block_json["rule"].as_str().unwrap();
    assert!(rule.contains("cause, owner, unblock condition"));

    // Also check human text output contains the prominent reminder
    let human_res = Command::new(binary())
        .args(["task", "block", "task-405"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(human_res.status.success());
    let human_text = String::from_utf8_lossy(&human_res.stdout);
    assert!(human_text.contains("通知策略以 Collab 响应为准"));
    assert!(human_text.contains("合法等待必须写清原因"));

    fs::write(
        &fake_collab,
        "#!/bin/sh\nprintf '%s\\n' 'task not found' >&2\nexit 44\n",
    )
    .unwrap();
    let failed = Command::new(binary())
        .args(["task", "block", "task-404", "--json"])
        .current_dir(&root)
        .env("PATH", &fake_bin)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(44));
    assert!(String::from_utf8_lossy(&failed.stderr).contains("COLLAB_TASK_BLOCK_FAILED"));
    assert!(!String::from_utf8_lossy(&failed.stdout).contains("\"ok\":true"));

    fs::remove_dir_all(root).unwrap();
}
