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
            include_str!("../../../contracts/migrations/0.1.5/governance-maps/resource-map.json"),
        ),
        (
            "function-map.json",
            include_str!("../../../contracts/migrations/0.1.5/governance-maps/function-map.json"),
        ),
        (
            "mainline-call-map.json",
            include_str!("../../../contracts/migrations/0.1.5/governance-maps/mainline-call-map.json"),
        ),
        (
            "verification-map.json",
            include_str!("../../../contracts/migrations/0.1.5/governance-maps/verification-map.json"),
        ),
    ] {
        fs::write(root.join(".appsdk/maps").join(name), content).unwrap();
    }
}

fn install_current_governance_maps(root: &Path) {
    for (name, content) in [
        (
            "resource-map.json",
            include_str!("../../../contracts/maps/resource-map.json"),
        ),
        (
            "function-map.json",
            include_str!("../../../contracts/maps/function-map.json"),
        ),
        (
            "mainline-call-map.json",
            include_str!("../../../contracts/maps/mainline-call-map.json"),
        ),
        (
            "verification-map.json",
            include_str!("../../../contracts/maps/verification-map.json"),
        ),
    ] {
        fs::write(root.join(".appsdk/maps").join(name), content).unwrap();
    }
}

#[test]
fn current_resource_map_owns_the_current_bundle_and_generic_migration_paths() {
    let map: Value =
        serde_json::from_str(include_str!("../../../contracts/maps/resource-map.json")).unwrap();
    let resources = map["resources"].as_array().unwrap();
    let truth_store = |resource_id: &str| {
        resources
            .iter()
            .find(|resource| resource["resource_id"] == resource_id)
            .and_then(|resource| resource["truth_store"].as_str())
            .unwrap()
    };

    assert_eq!(
        truth_store("sdk_bundle"),
        "AppSDK 0.1.0010 embedded Bundle manifest/resources"
    );
    assert_eq!(
        truth_store("historical_governance_maps"),
        ".appsdk/migrations/<source>-to-<target>/maps/** when materialized by pin-lock; absent after fresh reset"
    );
    assert_eq!(
        truth_store("sdk_migration_record"),
        ".appsdk/migrations/<source>-to-<target>/record.json when materialized by pin-lock; absent after fresh reset"
    );
    for text in [
        include_str!("../../../contracts/migrations/sdk-0.1.5-to-0.1.6.json"),
        include_str!("../../../contracts/migrations/sdk-0.1.6-to-0.1.0007.json"),
    ] {
        let descriptor: Value = serde_json::from_str(text).unwrap();
        assert_eq!(descriptor["materialization"], "pin_lock_when_migrating");
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
            include_str!("../../../contracts/migrations/0.1.5/governance-maps/resource-map.json"),
        ),
        (
            "function-map.json",
            include_str!("../../../contracts/migrations/0.1.5/governance-maps/function-map.json"),
        ),
        (
            "mainline-call-map.json",
            include_str!("../../../contracts/migrations/0.1.5/governance-maps/mainline-call-map.json"),
        ),
        (
            "verification-map.json",
            include_str!("../../../contracts/migrations/0.1.5/governance-maps/verification-map.json"),
        ),
    ] {
        fs::write(migration_root.join("maps").join(name), content).unwrap();
        let target_digest = digest(match name {
            "resource-map.json" => {
                include_str!("../../../contracts/migrations/0.1.6/governance-maps/resource-map.json")
            }
            "function-map.json" => {
                include_str!("../../../contracts/migrations/0.1.6/governance-maps/function-map.json")
            }
            "mainline-call-map.json" => include_str!(
                "../../../contracts/migrations/0.1.6/governance-maps/mainline-call-map.json"
            ),
            "verification-map.json" => include_str!(
                "../../../contracts/migrations/0.1.6/governance-maps/verification-map.json"
            ),
            _ => unreachable!(),
        });
        maps.push(serde_json::json!({
            "name": name,
            "source_digest": digest(content),
            "target_digest": target_digest,
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
    lock["version"] = Value::String("0.1.0010".into());
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
fn pin_lock_accepts_all_materialized_migration_bundle_witnesses() {
    let root = temp_root("pin-lock-all-materialized-bundle-witnesses");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (first_record, first_bundle_digest) = install_previous_bundle_migration_record(&root);
    let second_root = root.join(".appsdk/migrations/0.1.6-to-0.1.0007");
    fs::create_dir_all(second_root.join("maps")).unwrap();
    let second_bundle_digest = format!("sha256:{}", "3".repeat(64));
    let mut maps = Vec::new();
    for (name, source, target) in [
        (
            "resource-map.json",
            include_str!("../../../contracts/migrations/0.1.6/governance-maps/resource-map.json"),
            include_str!("../../../contracts/maps/resource-map.json"),
        ),
        (
            "function-map.json",
            include_str!("../../../contracts/migrations/0.1.6/governance-maps/function-map.json"),
            include_str!("../../../contracts/maps/function-map.json"),
        ),
        (
            "mainline-call-map.json",
            include_str!("../../../contracts/migrations/0.1.6/governance-maps/mainline-call-map.json"),
            include_str!("../../../contracts/maps/mainline-call-map.json"),
        ),
        (
            "verification-map.json",
            include_str!("../../../contracts/migrations/0.1.6/governance-maps/verification-map.json"),
            include_str!("../../../contracts/maps/verification-map.json"),
        ),
    ] {
        fs::write(second_root.join("maps").join(name), source).unwrap();
        maps.push(serde_json::json!({
            "name": name,
            "source_digest": digest(source),
            "target_digest": digest(target),
            "snapshot_path": format!(".appsdk/migrations/0.1.6-to-0.1.0007/maps/{}", name)
        }));
    }
    let second_record = serde_json::json!({
        "schema_version": 1,
        "migration_id": "appsdk-0.1.6-to-0.1.0007",
        "source_version": "0.1.6",
        "target_version": "0.1.0007",
        "bundle_digest": second_bundle_digest,
        "maps": maps,
        "frozen_reviews": [],
        "legacy_reconciled_reviews": [],
        "created_at": "2026-01-02T00:00:00Z"
    });
    let second_record = serde_json::to_string_pretty(&second_record).unwrap() + "\n";
    fs::write(second_root.join("record.json"), &second_record).unwrap();

    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["bundle_digest"] = Value::String(second_bundle_digest.clone());
    lock["previous_bundle_digest"] = Value::String(first_bundle_digest.clone());
    fs::write(
        &lock_path,
        serde_json::to_string_pretty(&lock).unwrap() + "\n",
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
    let lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    let witnesses = lock["previous_bundle_digests"].as_array().unwrap();
    assert!(witnesses
        .iter()
        .any(|digest| digest.as_str() == Some(first_bundle_digest.as_str())));
    assert!(witnesses
        .iter()
        .any(|digest| digest.as_str() == Some(second_bundle_digest.as_str())));
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json")).unwrap(),
        first_record
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/migrations/0.1.6-to-0.1.0007/record.json")).unwrap(),
        second_record
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_requires_lock_anchor_for_materialized_migration_bundle_witnesses() {
    let root = temp_root("pin-lock-requires-lock-anchor");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (original_record, _) = install_previous_bundle_migration_record(&root);
    let second_root = root.join(".appsdk/migrations/0.1.6-to-0.1.0007");
    fs::create_dir_all(second_root.join("maps")).unwrap();
    let second_bundle_digest = format!("sha256:{}", "3".repeat(64));
    let mut maps = Vec::new();
    for (name, source, target) in [
        (
            "resource-map.json",
            include_str!("../../../contracts/migrations/0.1.6/governance-maps/resource-map.json"),
            include_str!("../../../contracts/maps/resource-map.json"),
        ),
        (
            "function-map.json",
            include_str!("../../../contracts/migrations/0.1.6/governance-maps/function-map.json"),
            include_str!("../../../contracts/maps/function-map.json"),
        ),
        (
            "mainline-call-map.json",
            include_str!("../../../contracts/migrations/0.1.6/governance-maps/mainline-call-map.json"),
            include_str!("../../../contracts/maps/mainline-call-map.json"),
        ),
        (
            "verification-map.json",
            include_str!("../../../contracts/migrations/0.1.6/governance-maps/verification-map.json"),
            include_str!("../../../contracts/maps/verification-map.json"),
        ),
    ] {
        fs::write(second_root.join("maps").join(name), source).unwrap();
        maps.push(serde_json::json!({
            "name": name,
            "source_digest": digest(source),
            "target_digest": digest(target),
            "snapshot_path": format!(".appsdk/migrations/0.1.6-to-0.1.0007/maps/{}", name)
        }));
    }
    let second_record = serde_json::json!({
        "schema_version": 1,
        "migration_id": "appsdk-0.1.6-to-0.1.0007",
        "source_version": "0.1.6",
        "target_version": "0.1.0007",
        "bundle_digest": second_bundle_digest,
        "maps": maps,
        "frozen_reviews": [],
        "legacy_reconciled_reviews": [],
        "created_at": "2026-01-02T00:00:00Z"
    });
    let second_record = serde_json::to_string_pretty(&second_record).unwrap() + "\n";
    fs::write(second_root.join("record.json"), &second_record).unwrap();

    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["bundle_digest"] = Value::String(format!("sha256:{}", "4".repeat(64)));
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
    assert_eq!(
        fs::read_to_string(second_root.join("record.json")).unwrap(),
        second_record
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_malformed_plural_bundle_witnesses_without_overwrite() {
    let root = temp_root("pin-lock-malformed-plural-bundle-witnesses");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (original_record, previous_bundle_digest) = install_previous_bundle_migration_record(&root);
    let record_path = root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json");
    let lock_path = root.join(".appsdk/sdk.lock");
    let original_lock = fs::read_to_string(&lock_path).unwrap();
    let mut lock: Value = serde_json::from_str(&original_lock).unwrap();
    lock["previous_bundle_digest"] = Value::String(previous_bundle_digest);

    for malformed in [
        serde_json::json!("not-an-array"),
        serde_json::json!([7]),
        serde_json::json!(["sha256:not-a-digest"]),
    ] {
        lock["previous_bundle_digests"] = malformed;
        let malformed_lock = serde_json::to_string_pretty(&lock).unwrap() + "\n";
        fs::write(&lock_path, &malformed_lock).unwrap();
        let rejected = run(&[
            "pin-lock",
            root_text,
            "--binary",
            binary().to_str().unwrap(),
        ]);
        assert!(!rejected.status.success());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("INVALID_SDK_BUNDLE_DIGEST"));
        assert_eq!(fs::read_to_string(&lock_path).unwrap(), malformed_lock);
        assert_eq!(fs::read_to_string(&record_path).unwrap(), original_record);
    }
    fs::write(&lock_path, original_lock).unwrap();
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
fn pin_lock_rejects_equal_malformed_bundle_witness_without_overwrite() {
    let root = temp_root("pin-lock-equal-malformed-witness");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (original_record, previous_bundle_digest) = install_previous_bundle_migration_record(&root);
    let record_path = root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json");
    let lock_path = root.join(".appsdk/sdk.lock");
    let mut record: Value = serde_json::from_str(&original_record).unwrap();
    record["bundle_digest"] = Value::String("sha256:invalid".into());
    let malformed_record = serde_json::to_string_pretty(&record).unwrap() + "\n";
    fs::write(&record_path, &malformed_record).unwrap();
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["bundle_digest"] = Value::String("sha256:invalid".into());
    lock["previous_bundle_digest"] = Value::String(previous_bundle_digest);
    let malformed_lock = serde_json::to_string_pretty(&lock).unwrap() + "\n";
    fs::write(&lock_path, &malformed_lock).unwrap();

    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("INVALID_SDK_MIGRATION_RECORD"));
    assert_eq!(fs::read_to_string(&lock_path).unwrap(), malformed_lock);
    assert_eq!(fs::read_to_string(&record_path).unwrap(), malformed_record);
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
        entry["canonical_source_digest"] = entry["source_digest"].clone();
        entry["canonical_target_digest"] = entry["target_digest"].clone();
        let historical = fs::read_to_string(&snapshot).unwrap();
        preserved.push((live, snapshot, historical));
    }
    install_current_governance_maps(&root);
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
        for (_live, snapshot, content) in &preserved {
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
        .remove("previous_bundle_digests");
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
    record["maps"][0]["canonical_target_digest"] =
        Value::String(format!("sha256:{}", "f".repeat(64)));
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
        .contains("SDK_MIGRATION_LIVE_MAP_UNRECONCILED:resource-map.json"));
    assert_eq!(fs::read_to_string(&record_path).unwrap(), historical_record);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_accepts_historical_custom_target_different_from_canonical_target() {
    let root = temp_root("pin-lock-historical-custom-target");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (_, previous_bundle_digest) = install_previous_bundle_migration_record(&root);
    let migration_root = root.join(".appsdk/migrations/0.1.0007-to-0.1.0008");
    fs::create_dir_all(migration_root.join("maps")).unwrap();
    let manifest: Value = serde_json::from_str(include_str!(
        "../../../contracts/migrations/sdk-0.1.0007-to-0.1.0008.json"
    ))
    .unwrap();
    let mut maps = Vec::new();
    let mut preserved = Vec::new();
    for (name, source) in [
        (
            "resource-map.json",
            include_str!("../../../contracts/migrations/0.1.0007/governance-maps/resource-map.json"),
        ),
        (
            "function-map.json",
            include_str!("../../../contracts/migrations/0.1.0007/governance-maps/function-map.json"),
        ),
        (
            "mainline-call-map.json",
            include_str!(
                "../../../contracts/migrations/0.1.0007/governance-maps/mainline-call-map.json"
            ),
        ),
        (
            "verification-map.json",
            include_str!(
                "../../../contracts/migrations/0.1.0007/governance-maps/verification-map.json"
            ),
        ),
    ] {
        let declared = manifest["maps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == name)
            .unwrap();
        let live = root.join(".appsdk/maps").join(name);
        let snapshot = migration_root.join("maps").join(name);
        let custom = fs::read_to_string(&live).unwrap() + "\n";
        fs::write(&live, &custom).unwrap();
        fs::write(&snapshot, source).unwrap();
        preserved.push((live, snapshot, custom.clone()));
        maps.push(serde_json::json!({
            "name": name,
            "source_digest": digest(source),
            "target_digest": digest(&custom),
            "canonical_source_digest": declared["source_digest"].clone(),
            "canonical_target_digest": declared["target_digest"].clone(),
            "snapshot_path": format!(".appsdk/migrations/0.1.0007-to-0.1.0008/maps/{}", name)
        }));
    }
    let record = serde_json::json!({
        "schema_version": 1,
        "migration_id": "appsdk-0.1.0007-to-0.1.0008",
        "source_version": "0.1.0007",
        "target_version": "0.1.0008",
        "bundle_digest": previous_bundle_digest,
        "maps": maps,
        "frozen_reviews": [],
        "legacy_reconciled_reviews": [],
        "created_at": "2026-01-02T00:00:00Z"
    });
    let historical_record = serde_json::to_string_pretty(&record).unwrap() + "\n";
    let record_path = migration_root.join("record.json");
    fs::write(&record_path, &historical_record).unwrap();

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
    for (live, snapshot, custom) in &preserved {
        assert_eq!(&fs::read_to_string(live).unwrap(), custom);
        assert!(snapshot.is_file());
    }
    assert!(run(&["verify", root_text]).status.success());
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
    record["maps"][3]["canonical_source_digest"] = Value::String(digest(include_str!(
        "../../../contracts/migrations/0.1.5/governance-maps/verification-map.json"
    )));
    record["maps"][3]["canonical_target_digest"] = Value::String(digest(include_str!(
        "../../../contracts/maps/verification-map.json"
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
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("INVALID_SDK_MIGRATION_RECORD"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_malformed_migration_record_bundle_digest() {
    let root = temp_root("invalid-migration-record-bundle-digest");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let (original_record, _) = install_previous_bundle_migration_record(&root);
    let record_path = root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json");
    let mut record: Value = serde_json::from_str(&original_record).unwrap();
    record["bundle_digest"] = Value::String("sha256:invalid".into());
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
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("INVALID_SDK_MIGRATION_RECORD"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_migration_record_missing_required_map_digest() {
    for field in ["source_digest", "target_digest"] {
        let root = temp_root(&format!("invalid-migration-record-missing-{field}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        let (original_record, _) = install_previous_bundle_migration_record(&root);
        let record_path = root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json");
        let mut record: Value = serde_json::from_str(&original_record).unwrap();
        record["maps"][0].as_object_mut().unwrap().remove(field);
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
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("INVALID_SDK_MIGRATION_RECORD"));
        fs::remove_dir_all(root).unwrap();
    }
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
        .contains("UNSUPPORTED_SDK_MIGRATION:0.1.2:0.1.0010"));
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
    assert_eq!(migrated_project["sdk"]["version"], "0.1.0010");
    assert_eq!(migrated_lock["version"], "0.1.0010");
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
fn init_preserves_project_record_contracts_without_migrating_declaration() {
    let root = temp_root("init-record-contract-nondestructive");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let live_closure = root.join("contracts/records/collab-live-closure-record.schema.json");
    let project_owned_record = root.join("contracts/records/worktree-record.schema.json");
    let mut project_owned_record_value: Value =
        serde_json::from_str(&fs::read_to_string(&project_owned_record).unwrap()).unwrap();
    project_owned_record_value["$comment"] = Value::String("project-owned extension".into());
    fs::write(
        &project_owned_record,
        serde_json::to_vec_pretty(&project_owned_record_value).unwrap(),
    )
    .unwrap();
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    let mut legacy = serde_json::Value::Array(Vec::new());
    for entry in project["governance"]["record_contracts"]
        .as_array()
        .unwrap()
        .iter()
    {
        let entry_str = entry.as_str().unwrap();
        if entry_str != "contracts/records/collab-live-closure-record.schema.json" {
            legacy.as_array_mut().unwrap().push(entry.clone());
        }
    }
    project["governance"]["record_contracts"] = legacy;
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let project_before = fs::read(&project_path).unwrap();
    let record_before = fs::read(&project_owned_record).unwrap();

    let init = run(&["init", root_text]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    assert_eq!(fs::read(&project_path).unwrap(), project_before);
    assert_eq!(fs::read(&project_owned_record).unwrap(), record_before);
    assert!(live_closure.is_file());
    let rejected = run(&["verify", root_text]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("NON_CANONICAL_RECORD_CONTRACT_SET"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}
