use serde_json::Value;
use std::fs;
use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_appsdk"))
}

fn memory_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_project-memory"))
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
    let result = run(&["pin-lock", root, "--binary", sdk_binary.to_str().unwrap()]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
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
        evidence_dir.join("whitebox-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "evidence_id":"whitebox-1","issue_id":"issue-1","experiment_id":"experiment-1",
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
            "evidence_id":"install-1","issue_id":"issue-1","experiment_id":"experiment-1",
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
            "evidence_id":"restart-1","issue_id":"issue-1","experiment_id":"experiment-1",
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
            "evidence_id":"blackbox-1","issue_id":"issue-1","experiment_id":"experiment-1",
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
        records.join(format!("pre-review-validation-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "validation_id":"pre-review-validation-1","issue_id":"issue-1","module_id":module_id,
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
            "review_id":"review-1","issue_id":"issue-1","promotion_id":"promotion-1",
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
fn confirmed_goal_and_pinned_lock_allow_compile_and_adjacent_promote() {
    let root = temp_root("positive");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let goal_file = root.join(".appsdk/goal.json");
    fs::write(&goal_file, r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#).unwrap();
    fs::write(root.join(".appsdk/sdk.lock"), format!(r#"{{"sdk":"appsdk","version":"0.1.6","digest":"sha256:{}","compiler_digest":"sha256:{}","contract_schema":1}}
"#, "a".repeat(64), "b".repeat(64))).unwrap();
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
    write_records(&root, "app-core", &architecture_hash, false);
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
    write_records(&root, "app-core", &architecture_hash, false);
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
    write_records(&root, "app-core", &architecture_hash, false);
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
    write_records(&root, "app-core", &architecture_hash, false);
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
    write_records(&root, "app-core", &architecture_hash, false);
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
    write_records(&root, "app-core", &architecture_hash, false);
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
    write_records(&root, "app-core", &architecture_hash, false);
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
    write_records(&root, "app-core", &architecture_hash, false);
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
    write_records(&root, "app-core", &architecture_hash, false);
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
    write_records(&root, "app-core", &architecture_hash, false);
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
    write_records(&root, "app-core", &architecture_hash, false);
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
    write_records(&root, "app-core", &architecture_hash, false);
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
            r#"{"id":"anchor-edge","category":"knowledge","title":"Anchor","content":"stable node anchor","tags":["stable"],"source_refs":["guide/node"],"related_ids":["lesson-edge"],"semantic_relations":[{"to_id":"lesson-edge","type":"similar_lesson","score":0.91,"model_revision":"wemm-test-v1"}],"importance":95,"layer":1}"#,
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
