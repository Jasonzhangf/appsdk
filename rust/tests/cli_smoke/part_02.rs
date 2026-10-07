#[test]
fn init_fresh_completes_when_global_registry_is_unavailable() {
    let root = temp_root("init-fresh-registry-pending");
    let registry_parent = temp_root("init-fresh-registry-pending-home");
    let registry = registry_parent.join("linked");
    fs::create_dir_all(&registry_parent).unwrap();
    symlink(&registry_parent, &registry).unwrap();
    let root_text = root.to_str().unwrap();

    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "preserve\n").unwrap();
    init_git(&root);

    let output = Command::new(binary())
        .args(["init", root_text, "--fresh", "--discard-legacy"])
        .env("APPSDK_HOME", registry.to_str().unwrap())
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("GLOBAL_PROJECT_REGISTRATION_PENDING"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root
        .join(".appsdk/records/reset-governance-record.json")
        .is_file());
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "preserve\n"
    );
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(registry_parent).unwrap();
}

#[test]
fn initialization_failure_does_not_persist_global_registration() {
    let root = temp_root("global-registration-order");
    let registry = temp_root("global-registration-order-home");
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

    // Remove the successful registration so the following failed init can
    // prove that no new event is appended before local initialization ends.
    fs::remove_dir_all(&registry).unwrap();
    let gitignore_target = temp_root("global-registration-order-gitignore");
    fs::write(&gitignore_target, "managed elsewhere\n").unwrap();
    fs::remove_file(root.join(".gitignore")).unwrap();
    symlink(&gitignore_target, root.join(".gitignore")).unwrap();

    let initialized = Command::new(binary())
        .args(["init", root_text])
        .env("APPSDK_HOME", registry_text)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!initialized.status.success());
    assert!(
        String::from_utf8_lossy(&initialized.stderr).contains("GOVERNANCE_PATH_SYMLINK:gitignore")
    );
    assert!(!registry.join("projects.jsonl").exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(registry).unwrap_or(());
    fs::remove_file(gitignore_target).unwrap();
}

#[test]
fn registration_failure_is_preflighted_before_new_workspace_scaffold() {
    let root = temp_root("global-registration-preflight");
    let registry_parent = temp_root("global-registration-preflight-home");
    let registry_target = registry_parent.join("real");
    let registry = registry_parent.join("linked");
    fs::create_dir_all(&registry_target).unwrap();
    symlink(&registry_target, &registry).unwrap();

    let created = Command::new(binary())
        .args(["new", root.to_str().unwrap()])
        .env("APPSDK_HOME", registry.to_str().unwrap())
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!created.status.success());
    assert!(
        String::from_utf8_lossy(&created.stderr).contains("GLOBAL_REGISTRY_SYMLINK:registry_root")
    );
    assert!(root.is_dir());
    assert!(!root.join(".appsdk").exists());
    assert!(!root.join(".gitignore").exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(registry_parent).unwrap();
}

#[test]
fn project_registration_waits_for_busy_host_lock() {
    let workspace = temp_root("global-registration-busy-waits");
    fs::create_dir_all(&workspace).unwrap();
    let root = workspace.join("project");
    let registry = workspace.join("registry");
    fs::create_dir_all(&registry).unwrap();
    let lock_path = registry.join("projects.jsonl.lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    hold_advisory_lock(&lock);

    let root_text = root.to_str().unwrap();
    let mut child = Command::new(binary())
        .args(["new", root_text])
        .env("APPSDK_HOME", registry.to_str().unwrap())
        .env_remove("TMUX_PANE")
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("registration exited while host lock was held: {status}");
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    drop(lock);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join(".appsdk/project.json").is_file());
    assert_eq!(
        fs::read_to_string(registry.join("projects.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn registration_failure_degrades_before_existing_workspace_refresh() {
    let root = temp_root("global-registration-existing-preflight");
    let registry_parent = temp_root("global-registration-existing-preflight-home");
    let registry = registry_parent.join("linked");
    let root_text = root.to_str().unwrap();
    let registry_text = registry.to_str().unwrap();

    let created = Command::new(binary())
        .args(["new", root_text])
        .env("APPSDK_HOME", registry_text)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(created.status.success());
    fs::remove_dir_all(&registry).unwrap();

    let real_registry = registry_parent.join("real");
    fs::create_dir_all(&real_registry).unwrap();
    symlink(&real_registry, &registry).unwrap();
    let initialized = Command::new(binary())
        .args(["init", root_text])
        .env("APPSDK_HOME", registry.to_str().unwrap())
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    assert!(String::from_utf8_lossy(&initialized.stderr)
        .contains("GLOBAL_PROJECT_REGISTRATION_PENDING"));
    assert!(root.join(".appsdk/project.json").is_file());
    assert!(!real_registry.join("projects.jsonl").exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(registry_parent).unwrap();
}

#[test]
fn init_fresh_nested_project_ignores_its_transaction_lock_when_checking_clean_worktree() {
    let workspace = temp_root("init-fresh-nested-project-lock");
    fs::create_dir_all(&workspace).unwrap();
    let root = workspace.join("v4");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    init_git(&workspace);
    fs::write(workspace.join("unrelated.txt"), "outside project\n").unwrap();

    let initialized = run(&[
        "init",
        workspace.to_str().unwrap(),
        "--project-root",
        "v4",
        "--fresh",
        "--discard-legacy",
    ]);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    let reset: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/reset-governance-record.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(reset["mode"], "fresh_init");
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    assert_reset_lock_released(&reset_transaction_lock_path(&root));

    fs::remove_dir_all(&workspace).unwrap();
    let _ = fs::remove_file(reset_transaction_lock_path(&root));
}

#[test]
fn init_fresh_nested_project_rejects_project_dirty_state_without_mutation() {
    let workspace = temp_root("init-fresh-nested-project-dirty");
    fs::create_dir_all(&workspace).unwrap();
    let root = workspace.join("v4");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&workspace);
    let project_before = fs::read_to_string(root.join(".appsdk/project.json")).unwrap();
    let lock_path = reset_transaction_lock_path(&root);
    fs::write(root.join("uncommitted.txt"), "must remain\n").unwrap();
    fs::write(workspace.join("unrelated.txt"), "outside project\n").unwrap();

    let rejected = run(&[
        "init",
        workspace.to_str().unwrap(),
        "--project-root",
        "v4",
        "--fresh",
        "--discard-legacy",
    ]);
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
        fs::read_to_string(root.join("uncommitted.txt")).unwrap(),
        "must remain\n"
    );
    assert!(!root
        .join(".appsdk/records/reset-governance-record.json")
        .exists());
    assert_reset_lock_released(&lock_path);

    fs::remove_dir_all(&workspace).unwrap();
    let _ = fs::remove_file(lock_path);
}

#[test]
fn init_fresh_rebuilds_malformed_project_contracts_from_canonical_content() {
    let root = temp_root("init-fresh-malformed-project-contracts");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join("contracts/records/worktree-record.schema.json"),
        "{\n",
    )
    .unwrap();
    fs::write(
        root.join("contracts/transitions/zone-transition-manifest.json"),
        "{\n",
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
    for relative in [
        "contracts/records/worktree-record.schema.json",
        "contracts/transitions/zone-transition-manifest.json",
        "contracts/transitions/zone-transition.manifest.json",
    ] {
        assert_eq!(
            serde_json::from_str::<Value>(&fs::read_to_string(root.join(relative)).unwrap())
                .unwrap(),
            serde_json::from_str::<Value>(match relative {
                "contracts/records/worktree-record.schema.json" => {
                    include_str!("../../../contracts/records/worktree-record.schema.json")
                }
                _ => include_str!("../../../contracts/transitions/zone-transition.manifest.json"),
            })
            .unwrap()
        );
    }
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_rejects_malformed_project_contract_before_resetting_state() {
    let root = temp_root("init-fresh-invalid-project-malformed");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    fs::write(root.join("generated/legacy-output"), "retain\n").unwrap();
    let project_path = root.join(".appsdk/project.json");
    fs::write(&project_path, "{not-json}\n").unwrap();
    init_git(&root);
    let project_before = fs::read_to_string(&project_path).unwrap();
    let reset_path = root.join(".appsdk/records/reset-governance-record.json");
    let reset_before = fs::read_to_string(&reset_path).ok();

    let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(
        !rejected.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("INVALID_PROJECT_CONTRACT"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(fs::read_to_string(&project_path).unwrap(), project_before);
    assert_eq!(fs::read_to_string(&reset_path).ok(), reset_before);
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("generated/legacy-output")).unwrap(),
        "retain\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_refills_missing_generated_root_from_current_baseline() {
    let root = temp_root("init-fresh-missing-generated-root");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    fs::create_dir_all(root.join("generated/legacy-output")).unwrap();
    fs::write(root.join("generated/legacy-output/result"), "remove\n").unwrap();
    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["governance"]
        .as_object_mut()
        .unwrap()
        .remove("generated_root");
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
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
    let after: Value = serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    assert_eq!(after["governance"]["generated_root"], "generated/**");
    assert!(!root.join("generated/legacy-output").exists());
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_rejects_semantically_invalid_project_contract_before_resetting_state() {
    let root = temp_root("init-fresh-invalid-project-semantic");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    fs::create_dir_all(root.join("generated/legacy-output")).unwrap();
    fs::write(root.join("generated/legacy-output/result"), "retain\n").unwrap();
    fs::write(
        root.join(".appsdk/legacy-record.json"),
        "{\"legacy\":true}\n",
    )
    .unwrap();
    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["project_id"] = Value::String("invalid_project".into());
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    init_git(&root);

    let project_before = fs::read(&project_path).unwrap();
    let legacy_before = fs::read_to_string(root.join(".appsdk/legacy-record.json")).unwrap();
    let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("INVALID_PROJECT_ID"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(fs::read(&project_path).unwrap(), project_before);
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/legacy-record.json")).unwrap(),
        legacy_before
    );
    assert_eq!(
        fs::read_to_string(root.join("generated/legacy-output/result")).unwrap(),
        "retain\n"
    );
    assert!(!root
        .join(".appsdk/records/reset-governance-record.json")
        .exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_normalizes_sdk_owned_contract_fields_without_losing_project_fields() {
    for (name, mutate) in [
        (
            "missing-bundle-manifest",
            (|project: &mut Value| {
                project["sdk"]
                    .as_object_mut()
                    .unwrap()
                    .remove("bundle_manifest");
            }) as fn(&mut Value),
        ),
        (
            "wrong-resource-record",
            (|project: &mut Value| {
                project["sdk"]["resource_record"] =
                    Value::String(".appsdk/tampered-sdk-resources.json".into());
            }) as fn(&mut Value),
        ),
        (
            "missing-sdk-owned-contract-projections",
            (|project: &mut Value| {
                let object = project.as_object_mut().unwrap();
                object.remove("schema_version");
                object.remove("guidance");
                object.remove("lifecycles");
                project["lifecycle"]
                    .as_object_mut()
                    .unwrap()
                    .remove("stage");
                project["development_scenarios"]
                    .as_object_mut()
                    .unwrap()
                    .remove("manifest");
                for key in [
                    "active_kind",
                    "protected_kinds",
                    "generated_kinds",
                    "freeze_requirements",
                    "promotion_requires",
                    "runtime_forbidden_roots",
                    "record_contracts",
                    "zone_transition_contract",
                    "playground_retention",
                    "debug_merge_comment_required",
                ] {
                    project["governance"].as_object_mut().unwrap().remove(key);
                }
            }) as fn(&mut Value),
        ),
        (
            "stale-sdk-owned-contract-projections",
            (|project: &mut Value| {
                project["guidance"]["enforcement"] = Value::String("mandatory".into());
                project["lifecycles"]["issue"] = Value::String("closed".into());
                project["governance"]["active_kind"] = Value::String("mutable_source".into());
                project["governance"]["record_contracts"] = Value::Array(Vec::new());
                project["governance"]["zone_transition_contract"] =
                    Value::String("stale/zone.json".into());
                project["governance"]["playground_retention"] = Value::String("delete_now".into());
            }) as fn(&mut Value),
        ),
    ] {
        let root = temp_root(&format!("init-fresh-normalize-sdk-{name}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());

        let project_path = root.join(".appsdk/project.json");
        let mut project: Value =
            serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
        project["project_id"] = Value::String("sdk-owned-normalization".into());
        project["modules"][0]["module_note"] = Value::String("preserve-project-field".into());
        mutate(&mut project);
        fs::write(
            &project_path,
            serde_json::to_string_pretty(&project).unwrap() + "\n",
        )
        .unwrap();
        fs::write(root.join("business.txt"), "preserve\n").unwrap();
        init_git(&root);

        let initialized = run(&["init", root_text, "--fresh", "--discard-legacy"]);
        assert!(
            initialized.status.success(),
            "case={name} stdout={} stderr={}",
            String::from_utf8_lossy(&initialized.stdout),
            String::from_utf8_lossy(&initialized.stderr)
        );
        let after: Value =
            serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
        assert_eq!(after["sdk"]["name"], "appsdk");
        assert_eq!(after["sdk"]["version"], "0.1.0011");
        assert_eq!(
            after["sdk"]["bundle_manifest"],
            ".appsdk/contracts/sdk-bundle.manifest.json"
        );
        assert_eq!(
            after["sdk"]["resource_record"],
            ".appsdk/sdk-resources.json"
        );
        assert_eq!(after["schema_version"], 1);
        assert_eq!(after["lifecycle"]["stage"], "draft");
        assert_eq!(after["guidance"]["enforcement"], "advisory");
        assert_eq!(after["lifecycles"]["issue"], "open");
        assert_eq!(
            after["development_scenarios"]["manifest"],
            ".appsdk/contracts/development-scenarios.manifest.json"
        );
        assert_eq!(
            after["governance"]["active_kind"],
            "immutable_consumable_library"
        );
        assert_eq!(
            after["governance"]["zone_transition_contract"],
            "contracts/transitions/zone-transition.manifest.json"
        );
        assert_eq!(
            after["governance"]["playground_retention"],
            "archive_then_remove"
        );
        assert!(!after["governance"]["record_contracts"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(after["project_id"], "sdk-owned-normalization");
        assert_eq!(after["modules"][0]["module_note"], "preserve-project-field");
        assert_eq!(
            fs::read_to_string(root.join("business.txt")).unwrap(),
            "preserve\n"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn init_fresh_preserves_project_owned_governance_constraints() {
    let root = temp_root("init-fresh-preserve-project-governance");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["project_id"] = Value::String("preserve-project-governance".into());
    project["governance"]["freeze_requirements"] = serde_json::json!([
        "git_clean",
        "source_commit_or_tag",
        "library_hash",
        "public_api_hash",
        "review_pass",
        "previous_active_immutable",
        "project_owner_signoff"
    ]);
    project["governance"]["promotion_requires"] = serde_json::json!([
        "experiment_evidence",
        "architecture_review_pass",
        "unique_owner",
        "required_gates",
        "project_integration_review"
    ]);
    project["governance"]["runtime_forbidden_roots"] =
        serde_json::json!(["playground/**", "generated/**", "legacy-private/**"]);
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
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
    let after: Value = serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    assert_eq!(
        after["governance"]["runtime_forbidden_roots"],
        serde_json::json!(["playground/**", "generated/**", "legacy-private/**"])
    );
    assert_eq!(
        after["governance"]["freeze_requirements"],
        project["governance"]["freeze_requirements"]
    );
    assert_eq!(
        after["governance"]["promotion_requires"],
        project["governance"]["promotion_requires"]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_preserves_project_module_registry_ownership() {
    let root = temp_root("init-fresh-preserve-module-registry");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["project_id"] = Value::String("preserve-module-registry".into());
    project["modules"][0]["module_id"] = Value::String("relay-service".into());
    project["modules"][0]["source_owner"] = Value::String("relay-service".into());
    project["modules"][0]["owned_paths"] =
        serde_json::json!(["services/relay/src/**", "protected/source/**"]);
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();

    let registry_path = root.join(".appsdk/maps/module-registry.json");
    fs::write(
        &registry_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "modules": [{
                "module_id": "relay-service",
                "status": "contract_bound",
                "owner": "relay-service",
                "owned_paths": ["services/relay/src/**", "protocol/relay/**"],
                "forbidden_paths": ["active/lib/**", "protected/**", "generated/**"],
                "verification_gates": ["relay-tls-wss"],
                "entry_symbols": ["relay::serve"],
                "symbol_owners": {"serve": "relay::serve"}
            }]
        }))
        .unwrap()
            + "\n",
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

    let registry: Value =
        serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
    let modules = registry["modules"].as_array().unwrap();
    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0]["module_id"], "relay-service");
    assert_eq!(modules[0]["owner"], "relay-service");
    assert_eq!(
        modules[0]["owned_paths"],
        serde_json::json!(["services/relay/src/**", "protected/source/**"])
    );
    let owned = modules[0]["owned_paths"].as_array().unwrap();
    let forbidden = modules[0]["forbidden_paths"].as_array().unwrap();
    assert!(
        owned.iter().all(|path| !forbidden.contains(path)),
        "owned paths must not be forbidden: {registry}"
    );
    assert!(
        !forbidden.iter().any(|path| path == "protected/**"),
        "project-owned protected path must not remain forbidden: {registry}"
    );
    assert!(
        forbidden.iter().any(|path| path == "generated/**"),
        "unrelated default forbidden path must remain: {registry}"
    );
    assert_eq!(
        modules[0]["verification_gates"],
        serde_json::json!(["relay-tls-wss"])
    );
    assert_eq!(
        modules[0]["entry_symbols"],
        serde_json::json!(["relay::serve"])
    );
    assert_eq!(
        modules[0]["symbol_owners"],
        serde_json::json!({"serve": "relay::serve"})
    );
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_derives_missing_registry_modules_from_project_contract() {
    let root = temp_root("init-fresh-derive-module-registry");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["modules"][0]["module_id"] = Value::String("relay-service".into());
    project["modules"][0]["source_owner"] = Value::String("relay-service".into());
    project["modules"][0]["owned_paths"] =
        serde_json::json!(["services/relay/src/**", "protected/source/**"]);
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
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

    let registry: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/maps/module-registry.json")).unwrap(),
    )
    .unwrap();
    let modules = registry["modules"].as_array().unwrap();
    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0]["module_id"], "relay-service");
    assert_eq!(modules[0]["owner"], "relay-service");
    assert_eq!(modules[0]["status"], "active");
    assert_eq!(
        modules[0]["owned_paths"],
        serde_json::json!(["services/relay/src/**", "protected/source/**"])
    );
    let forbidden = modules[0]["forbidden_paths"].as_array().unwrap();
    assert!(!forbidden.iter().any(|path| path == "protected/**"));
    assert!(
        !modules
            .iter()
            .any(|module| module["module_id"] == "app-core"),
        "stale registry module must not survive fresh init: {registry}"
    );
    assert!(run(&["verify", root_text]).status.success());
    let producer_input = root.join("producer-input.json");
    fs::write(&producer_input, "{}\n").unwrap();
    let producer = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "relay-service",
        "--input",
        producer_input.to_str().unwrap(),
    ]);
    let producer_stderr = String::from_utf8_lossy(&producer.stderr);
    assert!(!producer.status.success());
    assert!(
        producer_stderr.contains("GOAL_NOT_CONFIRMED:received"),
        "{producer_stderr}"
    );
    assert!(
        !producer_stderr.contains("LIFECYCLE_PRODUCER_MODULE_BINDING"),
        "{producer_stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_rejects_symlinked_module_registry_without_publishing_metadata() {
    let root = temp_root("init-fresh-symlinked-module-registry");
    let outside = temp_root("init-fresh-symlinked-module-registry-target");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let registry_path = root.join(".appsdk/maps/module-registry.json");
    let outside_registry = outside.join("module-registry.json");
    let external_registry = serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": 1,
        "modules": [{
            "module_id": "external-owner",
            "status": "active",
            "owner": "external-owner",
            "owned_paths": ["external/**"],
            "forbidden_paths": []
        }]
    }))
    .unwrap()
        + "\n";
    fs::create_dir_all(&outside).unwrap();
    fs::write(&outside_registry, &external_registry).unwrap();
    fs::remove_file(&registry_path).unwrap();
    symlink(&outside_registry, &registry_path).unwrap();

    let project_path = root.join(".appsdk/project.json");
    let project_before = fs::read_to_string(&project_path).unwrap();
    init_git(&root);

    let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("GOVERNANCE_PATH_SYMLINK:reset_module_registry"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(fs::read_to_string(&project_path).unwrap(), project_before);
    assert!(registry_path.is_symlink());
    assert_eq!(
        fs::read_to_string(&outside_registry).unwrap(),
        external_registry
    );
    assert!(!root
        .join(".appsdk/records/reset-governance-record.json")
        .exists());
    fs::remove_file(&registry_path).unwrap();
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn init_fresh_requires_project_security_boundary_before_resetting_unknown_contract() {
    let root = temp_root("init-fresh-minimal-contract-required");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["access"]
        .as_object_mut()
        .unwrap()
        .remove("protected_paths");
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    fs::write(root.join("business.txt"), "preserve\n").unwrap();
    fs::create_dir_all(root.join("generated/legacy-output")).unwrap();
    fs::write(root.join("generated/legacy-output/result"), "retain\n").unwrap();
    fs::write(root.join("protected/history/legacy.txt"), "retain\n").unwrap();
    init_git(&root);

    let project_before = fs::read(&project_path).unwrap();
    let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("GOVERNANCE_RESET_CONTRACT_REQUIRED:access.protected_paths"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(fs::read(&project_path).unwrap(), project_before);
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "preserve\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("generated/legacy-output/result")).unwrap(),
        "retain\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("protected/history/legacy.txt")).unwrap(),
        "retain\n"
    );
    assert!(!root
        .join(".appsdk/records/reset-governance-record.json")
        .exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_reset_epoch_skips_compiled_artifact_requirement_but_preserves_contract() {
    let root = temp_root("init-fresh-reset-epoch-artifact");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["project_id"] = Value::String("reset-epoch-artifact-project".into());
    project["lifecycle"]["stage"] = Value::String("controlled_verified".into());
    project["modules"][0]["stage"] = Value::String("frozen".into());
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let project_before = fs::read(&project_path).unwrap();

    let old_record = root.join(".appsdk/records/review-record-app-core.json");
    fs::create_dir_all(old_record.parent().unwrap()).unwrap();
    fs::write(&old_record, "{\"old\":true}\n").unwrap();
    fs::create_dir_all(root.join("generated/modules/app-core")).unwrap();
    fs::write(
        root.join("generated/modules/app-core/module.compiled.json"),
        "old\n",
    )
    .unwrap();
    fs::write(
        root.join("generated/project.compiled.json"),
        "old-project\n",
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
    assert!(root
        .join(".appsdk/records/reset-governance-record.json")
        .is_file());
    assert_eq!(
        fs::read(&project_path).unwrap(),
        project_before,
        "fresh init must preserve the existing project contract"
    );
    assert!(
        !root
            .join(".appsdk/records/review-record-app-core.json")
            .exists(),
        "fresh init must not copy legacy lifecycle records"
    );
    assert!(
        !root
            .join("generated/modules/app-core/module.compiled.json")
            .exists(),
        "fresh init must not copy legacy generated module artifacts"
    );
    assert!(
        !root.join("generated/project.compiled.json").exists(),
        "fresh init must not copy legacy project artifacts"
    );

    let current_project = fs::read(&project_path).unwrap();
    let after: Value = serde_json::from_slice(&current_project).unwrap();
    let before: Value = serde_json::from_slice(&project_before).unwrap();
    assert_eq!(after, before);
    assert_eq!(after["project_id"], "reset-epoch-artifact-project");
    assert_eq!(after["modules"][0]["module_id"], "app-core");
    assert_eq!(after["modules"][0]["source_owner"], "app-core");
    for key in ["owned_paths", "build", "contract_paths"] {
        assert_eq!(
            after["modules"][0][key], before["modules"][0][key],
            "fresh init must preserve module {key}"
        );
    }

    let admission = run(&["verify", "--admission", root_text]);
    assert!(
        !admission.status.success(),
        "reset must not make a missing compiled artifact admission-ready"
    );
    assert!(
        String::from_utf8_lossy(&admission.stderr).contains("COMPILED_STAGE_REQUIRES_ARTIFACT"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&admission.stdout),
        String::from_utf8_lossy(&admission.stderr)
    );
    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );
    let result: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(
        result["ok"], false,
        "unevaluated delivery must not be reported as ok"
    );
    assert_eq!(result["command_ok"], true);
    assert_eq!(result["development_ready"], true);
    assert_eq!(result["delivery_verified"], false);
    assert_eq!(result["delivery_assessed"], false);
    assert_eq!(result["baseline_status"], "required");
    assert_eq!(result["reason"], "baseline_required");

    let admission = run(&["verify", "--admission", root_text]);
    assert!(
        !admission.status.success(),
        "admission must not inherit skipped delivery evidence from reset"
    );
    assert!(
        String::from_utf8_lossy(&admission.stderr).contains("COMPILED_STAGE_REQUIRES_ARTIFACT"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&admission.stdout),
        String::from_utf8_lossy(&admission.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_recovers_prepared_transaction_before_retrying() {
    let root = temp_root("init-fresh-prepared-transaction");
    let root_text = root.to_str().unwrap();
    let lock_path = reset_transaction_lock_path(&root);
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    init_git(&root);
    let transaction = root.parent().unwrap().join(format!(
        ".appsdk-reset-transaction-{}",
        root.file_name().unwrap().to_string_lossy()
    ));
    fs::create_dir_all(transaction.join("quarantine")).unwrap();
    fs::write(
        transaction.join("marker.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "transaction_id": "fresh-init-test",
            "root": root.to_string_lossy(),
            "phase": "building",
            "error": null,
            "targets": [],
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let project_before = fs::read_to_string(root.join(".appsdk/project.json")).unwrap();

    let recovered = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!recovered.status.success());
    assert!(
        String::from_utf8_lossy(&recovered.stderr).contains("GOVERNANCE_RESET_RECOVERED_RETRY"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&recovered.stdout),
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(!transaction.exists());
    assert_reset_lock_released(&lock_path);
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/project.json")).unwrap(),
        project_before
    );
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );

    let initialized = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    assert!(run(&["verify", root_text]).status.success());
    assert_reset_lock_released(&lock_path);
    fs::remove_dir_all(root).unwrap();
    let _ = fs::remove_file(lock_path);
}

#[test]
fn init_fresh_recovers_quarantined_transaction_across_relative_and_absolute_roots() {
    let root = temp_root("init-fresh-quarantined-transaction");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    init_git(&root);
    let canonical_root = root.canonicalize().unwrap();
    let transaction = root.parent().unwrap().join(format!(
        ".appsdk-reset-transaction-{}",
        root.file_name().unwrap().to_string_lossy()
    ));
    fs::create_dir_all(transaction.join("quarantine")).unwrap();
    fs::rename(
        root.join(".appsdk"),
        transaction.join("quarantine/target-0"),
    )
    .unwrap();
    let mut targets = fresh_reset_marker_targets(&root, &["generated"]);
    targets[0]["original_exists"] = Value::Bool(true);
    fs::write(
        transaction.join("marker.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "transaction_id": "fresh-init-quarantined-test",
            "root": canonical_root.to_string_lossy(),
            "phase": "prepared",
            "error": null,
            "created_dirs": [],
            "generated_roots": ["generated"],
            "targets": targets,
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();

    let recovered = run_in(
        root.parent().unwrap(),
        &[
            "init",
            root.file_name().unwrap().to_str().unwrap(),
            "--fresh",
            "--discard-legacy",
        ],
    );
    assert!(!recovered.status.success());
    assert!(
        String::from_utf8_lossy(&recovered.stderr).contains("GOVERNANCE_RESET_RECOVERED_RETRY"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&recovered.stdout),
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(!transaction.exists());
    assert!(root.join(".appsdk/project.json").is_file());
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );

    let initialized = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_recovers_after_quarantine_rename_before_marker_update() {
    let root = temp_root("init-fresh-quarantine-marker-lag");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    init_git(&root);
    let transaction = root.parent().unwrap().join(format!(
        ".appsdk-reset-transaction-{}",
        root.file_name().unwrap().to_string_lossy()
    ));
    fs::create_dir_all(transaction.join("quarantine")).unwrap();
    fs::rename(
        root.join(".appsdk"),
        transaction.join("quarantine/target-0"),
    )
    .unwrap();
    let mut targets = fresh_reset_marker_targets(&root, &["generated"]);
    targets[0]["original_exists"] = Value::Bool(true);
    fs::write(
        transaction.join("marker.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "transaction_id": "fresh-init-quarantine-marker-lag",
            "root": root.to_string_lossy(),
            "phase": "prepared",
            "error": null,
            "created_dirs": [],
            "generated_roots": ["generated"],
            "targets": targets,
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(transaction.join("marker.staging.123.456"), "partial\n").unwrap();

    let recovered = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!recovered.status.success());
    assert!(
        String::from_utf8_lossy(&recovered.stderr).contains("GOVERNANCE_RESET_RECOVERED_RETRY"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&recovered.stdout),
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(!transaction.exists());
    assert!(root.join(".appsdk/project.json").is_file());
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_recovers_after_rollback_before_marker_update() {
    let root = temp_root("init-fresh-rollback-marker-lag");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    init_git(&root);
    let transaction = root.parent().unwrap().join(format!(
        ".appsdk-reset-transaction-{}",
        root.file_name().unwrap().to_string_lossy()
    ));
    fs::create_dir_all(transaction.join("quarantine")).unwrap();
    let mut targets = fresh_reset_marker_targets(&root, &["generated"]);
    targets[0]["quarantined"] = Value::Bool(true);
    fs::write(
        transaction.join("marker.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "transaction_id": "fresh-init-rollback-marker-lag",
            "root": root.to_string_lossy(),
            "phase": "quarantining",
            "error": null,
            "created_dirs": [],
            "generated_roots": ["generated"],
            "targets": targets,
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(transaction.join("marker.staging.123.456"), "partial\n").unwrap();

    let recovered = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!recovered.status.success());
    assert!(
        String::from_utf8_lossy(&recovered.stderr).contains("GOVERNANCE_RESET_RECOVERED_RETRY"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&recovered.stdout),
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(!transaction.exists());
    assert!(root.join(".appsdk/project.json").is_file());
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_recovers_markerless_empty_transaction() {
    let root = temp_root("init-fresh-markerless-empty-transaction");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    init_git(&root);
    let transaction = root.parent().unwrap().join(format!(
        ".appsdk-reset-transaction-{}",
        root.file_name().unwrap().to_string_lossy()
    ));
    fs::create_dir_all(transaction.join("quarantine")).unwrap();
    fs::create_dir_all(transaction.join("staging")).unwrap();
    fs::write(transaction.join("marker.staging.123.456"), "partial\n").unwrap();

    let recovered = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!recovered.status.success());
    assert!(
        String::from_utf8_lossy(&recovered.stderr).contains("GOVERNANCE_RESET_RECOVERED_RETRY"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&recovered.stdout),
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(!transaction.exists());
    assert!(root.join(".appsdk/project.json").is_file());
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_fresh_committed_cleanup_uses_marker_roots_after_legacy_contract_is_gone() {
    let root = temp_root("init-fresh-committed-custom-root");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["governance"]["generated_root"] = Value::String("build-output/**".into());
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("build-output")).unwrap();
    fs::write(root.join("build-output/legacy.bin"), "legacy\n").unwrap();
    init_git(&root);

    let initialized = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    let reset_path = root.join(".appsdk/records/reset-governance-record.json");
    let reset: Value = serde_json::from_str(&fs::read_to_string(&reset_path).unwrap()).unwrap();
    let transaction_id = reset["transaction_id"].as_str().unwrap();
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

    let resumed = run(&["init", root_text, "--fresh", "--discard-legacy"]);
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
fn init_fresh_rejects_orphan_quarantine_when_marker_plan_is_empty() {
    for phase in [
        "building",
        "build_failed",
        "preflight_failed",
        "prepared",
        "committed",
    ] {
        let root = temp_root(&format!("init-fresh-orphan-quarantine-{phase}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        fs::write(root.join("business.txt"), "keep\n").unwrap();
        init_git(&root);
        let transaction = root.parent().unwrap().join(format!(
            ".appsdk-reset-transaction-{}",
            root.file_name().unwrap().to_string_lossy()
        ));
        fs::create_dir_all(transaction.join("quarantine")).unwrap();
        fs::write(transaction.join("quarantine/target-0"), "orphan\n").unwrap();
        fs::write(
            transaction.join("marker.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "transaction_id": format!("fresh-init-orphan-{phase}"),
                "root": root.to_string_lossy(),
                "phase": phase,
                "error": null,
                "created_dirs": [],
                "generated_roots": ["generated"],
                "targets": [],
                "updated_at": "2026-01-01T00:00:00Z"
            }))
            .unwrap()
                + "\n",
        )
        .unwrap();

        let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
        assert!(!rejected.status.success(), "phase={phase}");
        assert!(
            String::from_utf8_lossy(&rejected.stderr)
                .contains("GOVERNANCE_RESET_RECOVERY_REQUIRED"),
            "phase={phase} stdout={} stderr={}",
            String::from_utf8_lossy(&rejected.stdout),
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(transaction.exists());
        assert!(transaction.join("quarantine/target-0").is_file());
        assert_eq!(
            fs::read_to_string(root.join("business.txt")).unwrap(),
            "keep\n"
        );
        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(transaction).unwrap();
        let _ = fs::remove_file(reset_transaction_lock_path(&root));
    }
}
