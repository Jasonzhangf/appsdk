#[derive(Debug, PartialEq, Eq)]
enum OrdinaryInitTreeEntry {
    Directory,
    File(Vec<u8>),
    Symlink(PathBuf),
}

fn ordinary_init_tree_snapshot(root: &Path) -> Vec<(PathBuf, OrdinaryInitTreeEntry)> {
    fn visit(root: &Path, current: &Path, entries: &mut Vec<(PathBuf, OrdinaryInitTreeEntry)>) {
        let mut children = fs::read_dir(current)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let path = child.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.file_type().is_symlink() {
                entries.push((
                    relative,
                    OrdinaryInitTreeEntry::Symlink(fs::read_link(&path).unwrap()),
                ));
            } else if metadata.is_dir() {
                entries.push((relative, OrdinaryInitTreeEntry::Directory));
                visit(root, &path, entries);
            } else {
                entries.push((
                    relative,
                    OrdinaryInitTreeEntry::File(fs::read(&path).unwrap()),
                ));
            }
        }
    }

    let mut entries = Vec::new();
    visit(root, root, &mut entries);
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

const ORDINARY_INIT_PREVIOUS_SDK: &[u8] = include_bytes!(
    "../../../docs/evidence/user-requirement-truth-lock-20261007/session-lock/fixture-appsdk-0011.tar.gz"
);

fn extract_ordinary_init_previous_sdk(name: &str) -> PathBuf {
    let root = temp_root(name);
    fs::create_dir_all(&root).unwrap();
    let archive = root.with_extension("tar.gz");
    fs::write(&archive, ORDINARY_INIT_PREVIOUS_SDK).unwrap();
    let output = Command::new("tar")
        .args([
            "-xzf",
            archive.to_str().unwrap(),
            "-C",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_file(archive).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(root.join(".appsdk/project.json")).unwrap())
            .unwrap()["sdk"]["version"],
        "0.1.0011"
    );
    root
}

fn run_ordinary_init_without_collab(root: &Path) -> std::process::Output {
    Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .env("APPSDK_HOME", test_global_registry_root_for_project(root))
        .env("COLLAB_STATE_DIR", root.join(".test-collab-state"))
        .env("GIT_CEILING_DIRECTORIES", root)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap()
}

#[test]
fn ordinary_init_rejects_previous_sdk_pin_before_any_write() {
    let root = extract_ordinary_init_previous_sdk("ordinary-init-previous-pin");
    let before = ordinary_init_tree_snapshot(&root);

    let output = run_ordinary_init_without_collab(&root);
    assert!(
        !output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("SDK_VERSION_MIGRATION_REQUIRED:0.1.0011"),
        "stderr={stderr}"
    );
    assert!(stderr.contains("pin-lock"), "stderr={stderr}");
    assert_eq!(ordinary_init_tree_snapshot(&root), before);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ordinary_init_succeeds_after_official_migration_and_preserves_current_pin() {
    let root = extract_ordinary_init_previous_sdk("ordinary-init-migrated-pin");
    let pinned = run(&[
        "pin-lock",
        root.to_str().unwrap(),
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        pinned.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&pinned.stdout),
        String::from_utf8_lossy(&pinned.stderr)
    );
    let current_version = include_str!("../../../rust/release-version").trim();
    let lock: Value =
        serde_json::from_slice(&fs::read(root.join(".appsdk/sdk.lock")).unwrap()).unwrap();
    assert_eq!(lock["version"], current_version);

    let agents = root.join("AGENTS.md");
    let project_skill = root.join("skills/init-owned/SKILL.md");
    let record = root.join(".appsdk/records/init-owned.json");
    let active = root.join("active/lib/init-owned.txt");
    let protected = root.join("protected/history/init-owned.txt");
    fs::write(&agents, "# Project-owned rules\n").unwrap();
    fs::create_dir_all(project_skill.parent().unwrap()).unwrap();
    fs::write(&project_skill, "# Project-owned skill\n").unwrap();
    fs::write(&record, "project record\n").unwrap();
    fs::write(&active, "active\n").unwrap();
    fs::write(&protected, "protected\n").unwrap();
    let owned =
        [&agents, &project_skill, &record, &active, &protected].map(|path| fs::read(path).unwrap());

    let initialized = run_ordinary_init_without_collab(&root);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    for (path, expected) in [
        (&agents, &owned[0]),
        (&project_skill, &owned[1]),
        (&record, &owned[2]),
        (&active, &owned[3]),
        (&protected, &owned[4]),
    ] {
        assert_eq!(&fs::read(path).unwrap(), expected, "{}", path.display());
    }
    let verified = run(&["verify", root.to_str().unwrap()]);
    assert!(
        verified.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );

    let current = ordinary_init_tree_snapshot(&root);
    let repeated = run_ordinary_init_without_collab(&root);
    assert!(
        repeated.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&repeated.stdout),
        String::from_utf8_lossy(&repeated.stderr)
    );
    assert_eq!(ordinary_init_tree_snapshot(&root), current);

    fs::remove_dir_all(&root).unwrap();
    let _ = fs::remove_dir_all(test_global_registry_root_for_project(&root));
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
    assert_eq!(intake_json["readiness"], "needs_conditional_authorization");
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
        intake_json["proposal_schema"]["approval_required"],
        "uncovered_durable_changes_only"
    );
    assert_eq!(
        intake_json["proposal_schema"]["authorization_reuse"],
        "existing_session_authorization"
    );
    assert_eq!(
        intake_json["proposal_schema"]["recommended_change_required_fields"],
        serde_json::json!([
            "path",
            "owner",
            "action",
            "basis",
            "retained_safeguards",
            "affected_entrypoints"
        ])
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
    assert_eq!(
        intake_json["after_user_approval"]["compile_condition"],
        "only when Guidance is selected and its rule sources are authorized"
    );
    assert_eq!(
        intake_json["next"]["requires"],
        "authorization_check_for_uncovered_durable_changes"
    );
    assert!(intake_json["agent_instruction"]
        .as_str()
        .unwrap()
        .contains("CI/hook"));
    assert!(intake_json["questions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|question| {
            question["question_id"] == "approval_boundary"
                && question["prompt"]
                    .as_str()
                    .unwrap()
                    .contains("existing conversation authorization")
        }));
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
