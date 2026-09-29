#[test]
fn init_does_not_recover_broad_runtime_binding_rejections() {
    let root = temp_root("init-collab-runtime-binding-not-recoverable");
    fs::create_dir_all(&root).unwrap();
    confirm_preparation(&root, ".", "project_refactor");

    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    let probe = root.join("collab-probe.txt");
    fs::write(
        &fake_collab,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> "{}"
case "$*" in
  "init")
    printf '%s\n' 'RUNTIME_BINDING_REJECTED: candidate App Server thread is already bound to another worker' >&2
    exit 1
    ;;
  "worker recover")
    printf '%s\n' 'recover must not run for this error' >&2
    exit 64
    ;;
  *)
    printf '%s\n' "unexpected collab command: $*" >&2
    exit 64
    ;;
esac
"#,
            probe.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();
    init_git(&root);
    let path = format!("{}:{}", fake_bin.display(), env::var("PATH").unwrap());

    let output = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
        .env("PATH", &path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("COLLAB_INIT_FAILED"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("RUNTIME_BINDING_REJECTED"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let invocations = fs::read_to_string(&probe).unwrap();
    assert_eq!(invocations, "init\n");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_bounds_oversized_collab_timeout_override() {
    let root = temp_root("init-collab-oversized-timeout");
    fs::create_dir_all(&root).unwrap();
    confirm_preparation(&root, ".", "project_refactor");

    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{{\"ok\":true,\"runtime\":{{\"runtimeId\":\"runtime-oversized-timeout\",\"appserverId\":\"appserver-cli\",\"namespace\":\"codex_tui\",\"endpoint\":\"unix:///tmp/codex.sock\",\"projectRoot\":\"{}\",\"capabilities\":[\"session_status\",\"read_thread\",\"send_message_to_thread\",\"wait_reply\"],\"processId\":4242}},\"transport_selected\":{{\"kind\":\"appserver\",\"endpoint\":\"unix:///tmp/codex.sock\",\"namespace\":\"codex_tui\",\"thread_id\":\"thread-oversized-timeout\",\"capabilities\":[\"session_status\",\"read_thread\",\"send_message_to_thread\",\"wait_reply\"],\"self_check\":\"test\"}}}}'\n",
            root.canonicalize().unwrap().display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
        .env("PATH", &fake_bin)
        .env("APPSDK_COLLAB_INIT_TIMEOUT_MS", u64::MAX.to_string())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("collab-channel"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("COLLAB_INIT_TIMEOUT"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let registry = test_global_registry_root_for_project(&root).join("runtimes.jsonl");
    let runtime = fs::read_to_string(registry).unwrap();
    assert!(runtime.contains("runtime-oversized-timeout"), "{runtime}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_bounds_hanging_collab_bootstrap_without_faking_success() {
    let root = temp_root("init-collab-timeout");
    fs::create_dir_all(&root).unwrap();
    confirm_preparation(&root, ".", "project_refactor");

    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        "#!/bin/sh\nprintf 'started\\n' > \"$APPSDK_COLLAB_PROBE\"\nwhile :; do :; done\n",
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();
    let probe = root.join("collab-timeout-probe.txt");
    let started = Instant::now();
    let output = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
        .env("PATH", &fake_bin)
        .env("APPSDK_COLLAB_PROBE", &probe)
        .env("APPSDK_COLLAB_INIT_TIMEOUT_MS", "1000")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("COLLAB_INIT_TIMEOUT"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&probe).unwrap(), "started\n");
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("started"),
        "timed-out Collab output must not be reported as a successful bootstrap"
    );
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
fn subworker_entry_uses_the_compatibility_child_route_without_governance() {
    let root = temp_root("subworker-forward");
    fs::create_dir_all(&root).unwrap();
    let fake = root.join("collab");
    fs::write(&fake, "#!/bin/sh\nprintf '%s\\n' \"$@\"\nexit 23\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(binary())
        .args(["subworker", "status", "child"])
        .current_dir(&root)
        .env("PATH", &root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(23));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "subagent\nstatus\nchild\n"
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
    assert_eq!(intake_json["standard_template"]["version"], "0.1.0008");
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
fn repeated_init_refreshes_sdk_bundle_without_overwriting_project_truth() {
    let root = temp_root("init-refreshes-sdk-bundle");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project = root.join(".appsdk/project.json");
    let goal = root.join(".appsdk/goal.json");
    let map = root.join(".appsdk/maps/resource-map.json");
    let record = root.join(".appsdk/records/project-record.json");
    let active = root.join("active/lib/project-active.txt");
    let protected = root.join("protected/history/project-history.txt");
    fs::write(
        &project,
        fs::read_to_string(&project)
            .unwrap()
            .replace("change-me", "project-owned"),
    )
    .unwrap();
    let mut goal_value: Value = serde_json::from_slice(&fs::read(&goal).unwrap()).unwrap();
    goal_value["raw_request"] = Value::String("project-owned request".into());
    fs::write(&goal, serde_json::to_vec_pretty(&goal_value).unwrap()).unwrap();
    fs::write(
        &map,
        "{\"schema_version\":1,\"resources\":[{\"resource_id\":\"project-owned\"}]}\n",
    )
    .unwrap();
    fs::write(&record, "project record\n").unwrap();
    fs::write(&active, "active\n").unwrap();
    fs::write(&protected, "protected\n").unwrap();

    let project_before = fs::read(&project).unwrap();
    let goal_before = fs::read(&goal).unwrap();
    let map_before = fs::read(&map).unwrap();
    let record_before = fs::read(&record).unwrap();
    let active_before = fs::read(&active).unwrap();
    let protected_before = fs::read(&protected).unwrap();

    let stale_contract = root.join(".appsdk/contracts/project.schema.json");
    let stale_skill = root.join(".appsdk/skills/appsdk-project-governance/SKILL.md");
    fs::write(&stale_contract, "stale contract\n").unwrap();
    fs::write(&stale_skill, "stale skill\n").unwrap();

    let initialized = run(&["init", root_text]);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    assert_eq!(fs::read(&project).unwrap(), project_before);
    assert_eq!(fs::read(&goal).unwrap(), goal_before);
    assert_eq!(fs::read(&map).unwrap(), map_before);
    assert_eq!(fs::read(&record).unwrap(), record_before);
    assert_eq!(fs::read(&active).unwrap(), active_before);
    assert_eq!(fs::read(&protected).unwrap(), protected_before);
    assert_eq!(
        fs::read_to_string(stale_contract).unwrap(),
        include_str!("../../../contracts/project.schema.json")
    );
    assert_eq!(
        fs::read_to_string(stale_skill).unwrap(),
        include_str!("../../../skills/appsdk-project-governance/SKILL.md")
    );
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_init_preserves_historical_migration_bundle_witness() {
    let root = temp_root("init-preserves-migration-bundle-witness");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (original_record, previous_bundle_digest) = install_previous_bundle_migration_record(&root);

    // Model an SDK that already advanced once: the lock currently points at an
    // intermediate bundle and records the historical migration bundle as the
    // previous witness. A later init must keep that historical witness rather
    // than overwrite it with the immediate intermediate bundle.
    let intermediate_bundle_digest = format!("sha256:{}", "9".repeat(64));
    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["bundle_digest"] = Value::String(intermediate_bundle_digest.clone());
    lock["previous_bundle_digest"] = Value::String(previous_bundle_digest.clone());
    fs::write(&lock_path, serde_json::to_vec_pretty(&lock).unwrap()).unwrap();

    let initialized = run(&["init", root_text]);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );

    let lock: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/sdk.lock")).unwrap()).unwrap();
    let resources: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/sdk-resources.json")).unwrap())
            .unwrap();
    assert_eq!(lock["bundle_digest"], resources["bundle_digest"]);
    assert_eq!(lock["previous_bundle_digest"], previous_bundle_digest);
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json")).unwrap(),
        original_record
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
fn ordinary_verify_reports_stale_migration_without_blocking_development() {
    let root = temp_root("ordinary-verify-stale-migration");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let migration_root = root.join(".appsdk/migrations/0.1.5-to-0.1.6");
    fs::create_dir_all(&migration_root).unwrap();
    fs::write(migration_root.join("record.json"), "{\"legacy\":true}\n").unwrap();
    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
    lock["bundle_digest"] = Value::String(format!("sha256:{}", "a".repeat(64)));
    fs::write(&lock_path, serde_json::to_vec_pretty(&lock).unwrap()).unwrap();

    let ordinary = run(&["verify", root_text]);
    assert!(
        ordinary.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&ordinary.stdout),
        String::from_utf8_lossy(&ordinary.stderr)
    );
    assert!(String::from_utf8_lossy(&ordinary.stderr).contains("legacy SDK migration history"));

    let admission = run(&["verify", "--admission", root_text]);
    assert!(!admission.status.success());
    assert!(String::from_utf8_lossy(&admission.stderr).contains("INVALID_SDK_MIGRATION_RECORD"));
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
fn verify_distinguishes_assessed_delivery_from_unevaluated_development() {
    let root = temp_root("verify-delivery-assessment");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    // Ordinary verify is a development probe: it may succeed without
    // evaluating delivery, so it must not claim an ok/delivery result.
    let ordinary = run(&["verify", root_text]);
    assert!(
        ordinary.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&ordinary.stdout),
        String::from_utf8_lossy(&ordinary.stderr)
    );
    let ordinary_json: Value = serde_json::from_slice(&ordinary.stdout).unwrap();
    assert_eq!(ordinary_json["command_ok"], true);
    assert_eq!(ordinary_json["ok"], false);
    assert_eq!(ordinary_json["development_ready"], true);
    assert_eq!(ordinary_json["delivery_assessed"], false);
    assert_eq!(ordinary_json["delivery_verified"], false);
    assert_eq!(ordinary_json["reason"], "delivery_not_evaluated");

    // Admission actually runs the delivery checks. Reaching success must
    // report delivery as assessed and verified rather than unevaluated.
    let admission = run(&["verify", "--admission", root_text]);
    assert!(
        admission.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&admission.stdout),
        String::from_utf8_lossy(&admission.stderr)
    );
    let admission_json: Value = serde_json::from_slice(&admission.stdout).unwrap();
    assert_eq!(admission_json["command_ok"], true);
    assert_eq!(admission_json["delivery_assessed"], true);
    assert_eq!(admission_json["delivery_verified"], true);
    assert_eq!(admission_json["ok"], true);
    assert!(admission_json["reason"].is_null());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_allows_missing_known_sdk_resource_only_after_governance_reset() {
    let root = temp_root("sdk-resource-reset-missing");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    let reset = run(&["reset-governance", root_text, "--discard-legacy"]);
    assert!(
        reset.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&reset.stdout),
        String::from_utf8_lossy(&reset.stderr)
    );
    let resource = root.join(".appsdk/contracts/memory/memory-entry.schema.json");
    fs::remove_file(&resource).unwrap();
    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );
    assert!(String::from_utf8_lossy(&verified.stderr).contains("resource missing"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_still_rejects_missing_sdk_resource_without_reset() {
    let root = temp_root("sdk-resource-no-reset-missing");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let resource = root.join(".appsdk/contracts/memory/memory-entry.schema.json");
    fs::remove_file(&resource).unwrap();
    let rejected = run(&["verify", root_text]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("SDK_RESOURCE_MISMATCH:.appsdk/contracts/memory/memory-entry.schema.json"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_rejects_contracts_that_drop_canonical_semantics() {
    let root = temp_root("declared-contract-minimums");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let worktree_path = root.join("contracts/records/worktree-record.schema.json");
    let worktree_before = fs::read(&worktree_path).unwrap();
    let mut weakened_worktree: Value = serde_json::from_slice(&worktree_before).unwrap();
    weakened_worktree["required"]
        .as_array_mut()
        .unwrap()
        .retain(|value| value.as_str() != Some("module_id"));
    fs::write(
        &worktree_path,
        serde_json::to_string_pretty(&weakened_worktree).unwrap() + "\n",
    )
    .unwrap();
    let missing_required = run(&["verify", root_text]);
    assert!(!missing_required.status.success());
    assert!(String::from_utf8_lossy(&missing_required.stderr)
        .contains("DECLARED_RECORD_CONTRACT_MISMATCH"));
    fs::write(&worktree_path, worktree_before).unwrap();

    let zone_path = root.join("contracts/transitions/zone-transition.manifest.json");
    let zone_before = fs::read(&zone_path).unwrap();
    let mut weakened_zone: Value = serde_json::from_slice(&zone_before).unwrap();
    weakened_zone["transitions"].as_array_mut().unwrap()[0] = serde_json::json!({});
    fs::write(
        &zone_path,
        serde_json::to_string_pretty(&weakened_zone).unwrap() + "\n",
    )
    .unwrap();
    let empty_transition = run(&["verify", root_text]);
    assert!(!empty_transition.status.success());
    assert!(String::from_utf8_lossy(&empty_transition.stderr)
        .contains("INVALID_DECLARED_ZONE_CONTRACT"));
    fs::write(&zone_path, zone_before).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_rejects_nested_and_noncanonical_contract_weakening() {
    let root = temp_root("declared-contract-nested-and-zone-minimums");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let sdk_resources_path = root.join(".appsdk/sdk-resources.json");
    let sdk_resources_before = fs::read(&sdk_resources_path).unwrap();

    let worktree_path = root.join("contracts/records/worktree-record.schema.json");
    let worktree_projection_path =
        root.join(".appsdk/contracts/records/worktree-record.schema.json");
    let worktree_before = fs::read(&worktree_path).unwrap();
    let mut weakened_worktree: Value = serde_json::from_slice(&worktree_before).unwrap();
    weakened_worktree["allOf"][0]["then"]
        .as_object_mut()
        .unwrap()
        .remove("required");
    fs::write(
        &worktree_path,
        serde_json::to_string_pretty(&weakened_worktree).unwrap() + "\n",
    )
    .unwrap();
    fs::write(
        &worktree_projection_path,
        serde_json::to_string_pretty(&weakened_worktree).unwrap() + "\n",
    )
    .unwrap();
    set_sdk_resource_digest(
        &root,
        "contracts/records/worktree-record.schema.json",
        &worktree_projection_path,
    );
    let missing_allof = run(&["verify", "--admission", root_text]);
    assert!(
        !missing_allof.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&missing_allof.stdout),
        String::from_utf8_lossy(&missing_allof.stderr)
    );
    assert!(
        String::from_utf8_lossy(&missing_allof.stderr)
            .contains("DECLARED_RECORD_CONTRACT_MISMATCH"),
        "stderr={}",
        String::from_utf8_lossy(&missing_allof.stderr)
    );
    restore_sdk_contract(
        &worktree_path,
        &worktree_projection_path,
        &sdk_resources_path,
        &worktree_before,
        &sdk_resources_before,
    );

    let promotion_path = root.join("contracts/records/promotion-record.schema.json");
    let promotion_projection_path =
        root.join(".appsdk/contracts/records/promotion-record.schema.json");
    let promotion_before = fs::read(&promotion_path).unwrap();
    let mut legacy_promotion: Value = serde_json::from_slice(&promotion_before).unwrap();
    legacy_promotion["required"]
        .as_array_mut()
        .unwrap()
        .retain(|value| value.as_str() != Some("bug_closure_verified"));
    legacy_promotion["properties"]
        .as_object_mut()
        .unwrap()
        .remove("bug_closure_verified");
    fs::write(
        &promotion_path,
        serde_json::to_string_pretty(&legacy_promotion).unwrap() + "\n",
    )
    .unwrap();
    fs::write(
        &promotion_projection_path,
        serde_json::to_string_pretty(&legacy_promotion).unwrap() + "\n",
    )
    .unwrap();
    set_sdk_resource_digest(
        &root,
        "contracts/records/promotion-record.schema.json",
        &promotion_projection_path,
    );
    let accepted_legacy_promotion = run(&["verify", "--admission", root_text]);
    assert!(
        accepted_legacy_promotion.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&accepted_legacy_promotion.stdout),
        String::from_utf8_lossy(&accepted_legacy_promotion.stderr)
    );
    restore_sdk_contract(
        &promotion_path,
        &promotion_projection_path,
        &sdk_resources_path,
        &promotion_before,
        &sdk_resources_before,
    );

    let zone_path = root.join("contracts/transitions/zone-transition.manifest.json");
    let zone_projection_path =
        root.join(".appsdk/contracts/transitions/zone-transition.manifest.json");
    let zone_before = fs::read(&zone_path).unwrap();
    let mut conflicting_zone: Value = serde_json::from_slice(&zone_before).unwrap();
    conflicting_zone["transitions"]
        .as_array_mut()
        .unwrap()
        .retain(|transition| {
            transition["from"].as_str() != Some("playground")
                || transition["to"].as_str() != Some("playground")
        });
    let mut conflicting_transition = conflicting_zone["transitions"][0].clone();
    conflicting_transition["from"] = Value::String("playground".into());
    conflicting_transition["to"] = Value::String("playground".into());
    conflicting_transition["allowed"] = Value::Bool(false);
    conflicting_zone["transitions"]
        .as_array_mut()
        .unwrap()
        .push(conflicting_transition);
    fs::write(
        &zone_path,
        serde_json::to_string_pretty(&conflicting_zone).unwrap() + "\n",
    )
    .unwrap();
    fs::write(
        &zone_projection_path,
        serde_json::to_string_pretty(&conflicting_zone).unwrap() + "\n",
    )
    .unwrap();
    set_sdk_resource_digest(
        &root,
        "contracts/transitions/zone-transition.manifest.json",
        &zone_projection_path,
    );
    let conflicting = run(&["verify", "--admission", root_text]);
    assert!(
        !conflicting.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&conflicting.stdout),
        String::from_utf8_lossy(&conflicting.stderr)
    );
    assert!(
        String::from_utf8_lossy(&conflicting.stderr).contains("INVALID_DECLARED_ZONE_CONTRACT"),
        "stderr={}",
        String::from_utf8_lossy(&conflicting.stderr)
    );
    restore_sdk_contract(
        &zone_path,
        &zone_projection_path,
        &sdk_resources_path,
        &zone_before,
        &sdk_resources_before,
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_rejects_parallel_zone_contract_dropping_live_closure() {
    let root = temp_root("declared-zone-parallel-live-closure");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let project_before = fs::read(&project_path).unwrap();
    let mut project: Value = serde_json::from_slice(&project_before).unwrap();
    project["development_scenarios"]["enabled"] =
        Value::Array(vec![Value::String("multi_worker_collaboration".into())]);
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();

    let zone_path = root.join("contracts/transitions/zone-transition.manifest.json");
    let zone_projection_path =
        root.join(".appsdk/contracts/transitions/zone-transition.manifest.json");
    let zone_before = fs::read(&zone_path).unwrap();
    let mut v4_zone: Value = serde_json::from_slice(&zone_before).unwrap();
    for transition in v4_zone["transitions"].as_array_mut().unwrap() {
        if transition["from"].as_str() == Some("playground")
            && transition["to"].as_str() == Some("active")
        {
            transition["record_required"]
                .as_array_mut()
                .unwrap()
                .retain(|record| record.as_str() != Some("CollabLiveClosureRecordWhenParallel"));
        }
    }
    fs::write(
        &zone_path,
        serde_json::to_string_pretty(&v4_zone).unwrap() + "\n",
    )
    .unwrap();
    fs::write(
        &zone_projection_path,
        serde_json::to_string_pretty(&v4_zone).unwrap() + "\n",
    )
    .unwrap();
    set_sdk_resource_digest(
        &root,
        "contracts/transitions/zone-transition.manifest.json",
        &zone_projection_path,
    );

    let rejected = run(&["verify", "--admission", root_text]);
    assert!(
        !rejected.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("INVALID_DECLARED_ZONE_CONTRACT"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn install_bundle_resources_projects_declared_record_contract_sources() {
    let root = temp_root("project-record-contract-projection");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let source = root.join("contracts/records/worktree-record.schema.json");
    let installed = root.join(".appsdk/contracts/records/worktree-record.schema.json");
    let mut schema: Value = serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
    schema["properties"]["project_extension"] = serde_json::json!({"type": "string"});
    schema["required"]
        .as_array_mut()
        .unwrap()
        .push(Value::String("project_extension".into()));
    fs::write(
        &source,
        serde_json::to_string_pretty(&schema).unwrap() + "\n",
    )
    .unwrap();

    let refreshed = run(&["init", root_text]);
    assert!(
        refreshed.status.success(),
        "{}",
        String::from_utf8_lossy(&refreshed.stderr)
    );
    assert_eq!(fs::read(&source).unwrap(), fs::read(&installed).unwrap());

    let record: Value =
        serde_json::from_slice(&fs::read(root.join(".appsdk/sdk-resources.json")).unwrap())
            .unwrap();
    let entry = record["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["source"] == "contracts/records/worktree-record.schema.json")
        .unwrap();
    assert_eq!(
        entry["digest"],
        digest(&fs::read_to_string(&source).unwrap())
    );

    fs::write(&installed, "{\"drifted\":true}\n").unwrap();
    let rejected = run(&["verify", root_text]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("SDK_RESOURCE_MISMATCH:.appsdk/contracts/records/worktree-record.schema.json"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reset_receipt_validation_is_mode_aware_and_fail_closed() {
    let root = temp_root("reset-receipt-validation");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    assert!(Command::new("git")
        .args(["-C", root_text, "branch", "-M", "codex/reset-receipt-test"])
        .status()
        .unwrap()
        .success());
    assert!(run(&["reset-governance", root_text, "--discard-legacy"])
        .status
        .success());

    let receipt_path = root.join(".appsdk/records/reset-governance-record.json");
    let receipt_before = fs::read(&receipt_path).unwrap();
    assert!(run(&["verify", root_text]).status.success());

    let mut discard_without_transaction: Value = serde_json::from_slice(&receipt_before).unwrap();
    discard_without_transaction
        .as_object_mut()
        .unwrap()
        .remove("transaction_id");
    discard_without_transaction["reset_id"] = Value::String("reset-12345".into());
    fs::write(
        &receipt_path,
        serde_json::to_string_pretty(&discard_without_transaction).unwrap() + "\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args([
            "-C",
            root_text,
            "add",
            ".appsdk/records/reset-governance-record.json"
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "legacy reset receipt"])
        .status()
        .unwrap()
        .success());
    let missing_discard_transaction = run(&["verify", root_text]);
    assert!(
        missing_discard_transaction.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&missing_discard_transaction.stdout),
        String::from_utf8_lossy(&missing_discard_transaction.stderr)
    );
    assert!(String::from_utf8_lossy(&missing_discard_transaction.stderr)
        .contains("authorized legacy governance reset receipt has no transaction_id"));
    let missing_discard_result: Value =
        serde_json::from_slice(&missing_discard_transaction.stdout).unwrap();
    assert_eq!(missing_discard_result["baseline_status"], "required");
    assert_eq!(missing_discard_result["reason"], "baseline_required");

    let mut tampered_legacy_discard = discard_without_transaction;
    tampered_legacy_discard["branch"] = Value::String("codex/tampered".into());
    fs::write(
        &receipt_path,
        serde_json::to_string_pretty(&tampered_legacy_discard).unwrap() + "\n",
    )
    .unwrap();
    let tampered_legacy = run(&["verify", root_text]);
    assert!(!tampered_legacy.status.success());
    assert!(String::from_utf8_lossy(&tampered_legacy.stderr)
        .contains("INVALID_RESET_GOVERNANCE_RECORD"));

    fs::write(&receipt_path, &receipt_before).unwrap();
    assert!(Command::new("git")
        .args([
            "-C",
            root_text,
            "add",
            ".appsdk/records/reset-governance-record.json"
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            root_text,
            "commit",
            "-m",
            "restore transactional reset receipt"
        ])
        .status()
        .unwrap()
        .success());

    let mut malformed_legacy_discard: Value = serde_json::from_slice(&receipt_before).unwrap();
    malformed_legacy_discard
        .as_object_mut()
        .unwrap()
        .remove("transaction_id");
    malformed_legacy_discard["reset_id"] = Value::String("reset-not-a-pid".into());
    fs::write(
        &receipt_path,
        serde_json::to_string_pretty(&malformed_legacy_discard).unwrap() + "\n",
    )
    .unwrap();
    let malformed_legacy = run(&["verify", root_text]);
    assert!(!malformed_legacy.status.success());
    assert!(String::from_utf8_lossy(&malformed_legacy.stderr)
        .contains("INVALID_RESET_GOVERNANCE_RECORD"));
    fs::write(&receipt_path, &receipt_before).unwrap();

    let mut discard_with_empty_transaction: Value =
        serde_json::from_slice(&receipt_before).unwrap();
    discard_with_empty_transaction["transaction_id"] = Value::String(String::new());
    fs::write(
        &receipt_path,
        serde_json::to_string_pretty(&discard_with_empty_transaction).unwrap() + "\n",
    )
    .unwrap();
    let empty_discard_transaction = run(&["verify", root_text]);
    assert!(!empty_discard_transaction.status.success());
    assert!(String::from_utf8_lossy(&empty_discard_transaction.stderr)
        .contains("INVALID_RESET_GOVERNANCE_RECORD"));
    fs::write(&receipt_path, &receipt_before).unwrap();

    let mut mismatched_discard_transaction: Value =
        serde_json::from_slice(&receipt_before).unwrap();
    mismatched_discard_transaction["transaction_id"] = Value::String("transaction-1".into());
    mismatched_discard_transaction["reset_id"] = Value::String("reset-1".into());
    fs::write(
        &receipt_path,
        serde_json::to_string_pretty(&mismatched_discard_transaction).unwrap() + "\n",
    )
    .unwrap();
    let mismatched_discard = run(&["verify", root_text]);
    assert!(!mismatched_discard.status.success());
    assert!(String::from_utf8_lossy(&mismatched_discard.stderr)
        .contains("INVALID_RESET_GOVERNANCE_RECORD"));
    fs::write(&receipt_path, &receipt_before).unwrap();

    let mut fresh_without_transaction: Value = serde_json::from_slice(&receipt_before).unwrap();
    fresh_without_transaction["mode"] = Value::String("fresh_init".into());
    fresh_without_transaction
        .as_object_mut()
        .unwrap()
        .remove("transaction_id");
    fs::write(
        &receipt_path,
        serde_json::to_string_pretty(&fresh_without_transaction).unwrap() + "\n",
    )
    .unwrap();
    let missing_transaction = run(&["verify", root_text]);
    assert!(!missing_transaction.status.success());
    assert!(String::from_utf8_lossy(&missing_transaction.stderr)
        .contains("INVALID_RESET_GOVERNANCE_RECORD"));
    fs::write(&receipt_path, &receipt_before).unwrap();

    let mut mismatched_transaction: Value = serde_json::from_slice(&receipt_before).unwrap();
    mismatched_transaction["mode"] = Value::String("fresh_init".into());
    mismatched_transaction["transaction_id"] = Value::String("transaction-1".into());
    mismatched_transaction["reset_id"] = Value::String("reset-1".into());
    fs::write(
        &receipt_path,
        serde_json::to_string_pretty(&mismatched_transaction).unwrap() + "\n",
    )
    .unwrap();
    let mismatch = run(&["verify", root_text]);
    assert!(!mismatch.status.success());
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("INVALID_RESET_GOVERNANCE_RECORD"));
    fs::write(&receipt_path, &receipt_before).unwrap();

    let mut future_receipt: Value = serde_json::from_slice(&receipt_before).unwrap();
    future_receipt["created_at"] = Value::String("2999-01-01T00:00:00Z".into());
    fs::write(
        &receipt_path,
        serde_json::to_string_pretty(&future_receipt).unwrap() + "\n",
    )
    .unwrap();
    let future = run(&["verify", root_text]);
    assert!(!future.status.success());
    assert!(String::from_utf8_lossy(&future.stderr).contains("INVALID_RESET_GOVERNANCE_RECORD"));
    fs::write(&receipt_path, receipt_before).unwrap();
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
fn verify_requires_lock_version_to_match_project_version() {
    let root = temp_root("sdk-lock-project-version-pin");
    fs::create_dir_all(&root).unwrap();
    let root_text = root.to_str().unwrap();
    confirm_preparation(&root, ".", "new_project");
    assert!(run(&["init", root_text]).status.success());

    let project_file = root.join(".appsdk/project.json");
    let project_before = fs::read(&project_file).unwrap();
    let project: Value = serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    let lock_file = root.join(".appsdk/sdk.lock");
    let lock_before = fs::read(&lock_file).unwrap();
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_file).unwrap()).unwrap();
    lock["version"] = Value::String("0.1.5".into());
    assert_ne!(
        lock["version"], project["sdk"]["version"],
        "fixture must separate lock and project versions"
    );
    lock["contract_schema"] = project["schema_version"].clone();
    fs::write(
        &lock_file,
        serde_json::to_string_pretty(&lock).unwrap() + "\n",
    )
    .unwrap();

    let result = run(&["verify", root_text]);
    assert!(
        !result.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("INVALID_SDK_LOCK"),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );

    let mut project: Value = serde_json::from_slice(&project_before).unwrap();
    project["sdk"]["version"] = Value::String("0.1.5".into());
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    fs::write(&lock_file, &lock_before).unwrap();
    let project_mismatch = run(&["verify", root_text]);
    assert!(
        !project_mismatch.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&project_mismatch.stdout),
        String::from_utf8_lossy(&project_mismatch.stderr)
    );
    assert!(
        String::from_utf8_lossy(&project_mismatch.stderr)
            .contains("PROJECT_SDK_VERSION_PIN_MISMATCH:0.1.5:required_binary=appsdk-0.1.5"),
        "stderr={}",
        String::from_utf8_lossy(&project_mismatch.stderr)
    );
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
