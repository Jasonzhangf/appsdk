#[test]
fn init_fresh_preflights_all_original_paths_before_restoring_any_target() {
    let root = temp_root("init-fresh-preflight-all-originals");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    init_git(&root);
    let outside = temp_root("init-fresh-preflight-all-originals-outside");
    fs::create_dir_all(&outside).unwrap();
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
    fs::remove_dir_all(root.join(".appsdk-control")).unwrap();
    symlink(&outside, root.join(".appsdk-control")).unwrap();
    let mut targets = fresh_reset_marker_targets(&root, &["generated"]);
    targets[0]["original_exists"] = Value::Bool(true);
    fs::write(
        transaction.join("marker.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "transaction_id": "fresh-init-preflight-all-originals",
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

    let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("GOVERNANCE_RESET_RECOVERY_REQUIRED"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(!root.join(".appsdk").exists());
    assert!(transaction.join("quarantine/target-0").is_dir());
    assert!(fs::symlink_metadata(root.join(".appsdk-control"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(transaction).unwrap();
    fs::remove_dir_all(outside).unwrap();
    let _ = fs::remove_file(reset_transaction_lock_path(&root));
}

#[test]
fn init_fresh_cli_recovers_marker_created_from_relative_root_via_absolute_root() {
    let root = temp_root("init-fresh-cli-marker");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let outside = temp_root("init-fresh-cli-marker-outside");
    fs::create_dir_all(&outside).unwrap();
    fs::remove_dir_all(root.join(".appsdk-control")).unwrap();
    symlink(&outside, root.join(".appsdk-control")).unwrap();
    init_git(&root);

    let relative_failed = run_in(
        root.parent().unwrap(),
        &[
            "init",
            root.file_name().unwrap().to_str().unwrap(),
            "--fresh",
            "--discard-legacy",
        ],
    );
    assert!(!relative_failed.status.success());
    assert!(
        String::from_utf8_lossy(&relative_failed.stderr)
            .contains("GOVERNANCE_PATH_SYMLINK:.appsdk-control"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&relative_failed.stdout),
        String::from_utf8_lossy(&relative_failed.stderr)
    );
    let transaction = root.parent().unwrap().join(format!(
        ".appsdk-reset-transaction-{}",
        root.file_name().unwrap().to_string_lossy()
    ));
    let marker: Value =
        serde_json::from_str(&fs::read_to_string(transaction.join("marker.json")).unwrap())
            .unwrap();
    assert_eq!(
        marker["root"].as_str().unwrap(),
        root.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(marker["phase"], "preflight_failed");

    fs::remove_file(root.join(".appsdk-control")).unwrap();
    fs::create_dir(root.join(".appsdk-control")).unwrap();
    init_git(&root);
    let recovered = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!recovered.status.success());
    assert!(String::from_utf8_lossy(&recovered.stderr).contains("GOVERNANCE_RESET_RECOVERED_RETRY"));
    assert!(!transaction.exists());
    assert!(run(&["init", root_text, "--fresh", "--discard-legacy"])
        .status
        .success());
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn init_fresh_rejects_invalid_recovery_marker_before_deleting_active_or_business() {
    let root = temp_root("init-fresh-invalid-recovery-marker");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    fs::write(root.join("active/legacy.txt"), "keep-active\n").unwrap();
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
            "transaction_id": "fresh-init-invalid-marker",
            "root": root.to_string_lossy(),
            "phase": "prepared",
            "error": null,
            "created_dirs": [],
            "generated_roots": ["generated"],
            "targets": [{
                "relative": "active/legacy.txt",
                "kind": "file",
                "original_exists": true,
                "backup": "quarantine/target-0",
                "staged": null,
                "quarantined": false,
                "published": false
            }],
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();

    let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("GOVERNANCE_RESET_RECOVERY_REQUIRED")
    );
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("active/legacy.txt")).unwrap(),
        "keep-active\n"
    );
    assert!(transaction.exists());
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(transaction).unwrap();
    let _ = fs::remove_file(reset_transaction_lock_path(&root));
}

#[test]
fn init_fresh_does_not_trust_marker_generated_roots_for_recovery_deletion() {
    let root = temp_root("init-fresh-marker-generated-root");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    fs::create_dir_all(root.join("victim")).unwrap();
    fs::write(root.join("victim/keep.txt"), "must-remain\n").unwrap();
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
            "transaction_id": "fresh-init-marker-generated-root",
            "root": root.to_string_lossy(),
            "phase": "prepared",
            "error": null,
            "created_dirs": [],
            "generated_roots": ["victim"],
            "targets": [{
                "relative": "victim",
                "kind": "dir",
                "original_exists": true,
                "backup": "quarantine/target-0",
                "staged": null,
                "quarantined": false,
                "published": false
            }],
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();

    let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("GOVERNANCE_RESET_RECOVERY_REQUIRED")
    );
    assert_eq!(
        fs::read_to_string(root.join("victim/keep.txt")).unwrap(),
        "must-remain\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    assert!(transaction.exists());
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(transaction).unwrap();
    let _ = fs::remove_file(reset_transaction_lock_path(&root));
}

#[test]
fn init_fresh_rejects_dangling_transaction_symlink_before_recovery() {
    let root = temp_root("init-fresh-dangling-transaction");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    init_git(&root);
    let transaction = root.parent().unwrap().join(format!(
        ".appsdk-reset-transaction-{}",
        root.file_name().unwrap().to_string_lossy()
    ));
    symlink(transaction.with_extension("missing-target"), &transaction).unwrap();

    let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("GOVERNANCE_RESET_RECOVERY_REQUIRED")
    );
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    assert!(fs::symlink_metadata(&transaction)
        .unwrap()
        .file_type()
        .is_symlink());
    fs::remove_dir_all(&root).unwrap();
    fs::remove_file(transaction).unwrap();
    let _ = fs::remove_file(reset_transaction_lock_path(&root));
}

#[test]
fn init_fresh_returns_busy_while_same_root_transaction_lock_is_held() {
    let root = temp_root("init-fresh-lock-busy");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    init_git(&root);
    let transaction = root.parent().unwrap().join(format!(
        ".appsdk-reset-transaction-{}",
        root.file_name().unwrap().to_string_lossy()
    ));
    fs::create_dir_all(transaction.join("quarantine")).unwrap();
    let lock_path = reset_transaction_lock_path(&root);
    let lock = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    hold_advisory_lock(&lock);

    let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("GOVERNANCE_RESET_BUSY"));
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    assert!(transaction.exists());
    assert!(lock_path.is_file());
    drop(lock);
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(transaction).unwrap();
    let _ = fs::remove_file(lock_path);
}

#[test]
fn init_fresh_creates_missing_contract_parent_dirs_in_transaction_plan() {
    let root = temp_root("init-fresh-missing-contract-parents");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(root.join("business.txt"), "keep\n").unwrap();
    fs::remove_dir_all(root.join("contracts/records")).unwrap();
    fs::remove_dir_all(root.join("contracts/transitions")).unwrap();
    init_git(&root);
    assert!(!root.join("contracts/records").exists());
    assert!(!root.join("contracts/transitions").exists());

    let initialized = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    assert!(root
        .join("contracts/records/worktree-record.schema.json")
        .is_file());
    assert!(root
        .join("contracts/transitions/zone-transition-manifest.json")
        .is_file());
    assert_eq!(
        fs::read_to_string(root.join("business.txt")).unwrap(),
        "keep\n"
    );
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(&root).unwrap();
    let _ = fs::remove_file(reset_transaction_lock_path(&root));
}

#[test]
fn init_fresh_nested_generated_roots_use_single_quarantine_owner() {
    let root = temp_root("init-fresh-nested-generated");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project_path = root.join(".appsdk/project.json");
    let project = fs::read_to_string(&project_path).unwrap();
    fs::write(
        &project_path,
        project.replace(
            "\"generated_root\": \"generated/**\"",
            "\"generated_root\": \"generated/nested/**\"",
        ),
    )
    .unwrap();
    fs::create_dir_all(root.join("generated/old")).unwrap();
    fs::write(root.join("generated/old/artifact.bin"), "old\n").unwrap();
    fs::create_dir_all(root.join("generated/nested/old")).unwrap();
    fs::write(root.join("generated/nested/old/artifact.bin"), "old\n").unwrap();
    init_git(&root);

    let initialized = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    assert!(!root.join("generated/nested/old/artifact.bin").exists());
    let reset: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/reset-governance-record.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        reset["removed"],
        serde_json::json!([
            ".appsdk",
            ".appsdk-control",
            "generated",
            "generated/nested"
        ])
    );
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(&root).unwrap();
    let _ = fs::remove_file(reset_transaction_lock_path(&root));
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
    let lock_path = reset_transaction_lock_path(&root);

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
    assert!(!lock_path.exists());
    fs::remove_dir_all(root).unwrap();
    let _ = fs::remove_file(lock_path);
}

#[test]
fn init_fresh_refuses_dirty_worktree_without_mutating_governance() {
    let root = temp_root("init-fresh-dirty-worktree");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    let project_before = fs::read_to_string(root.join(".appsdk/project.json")).unwrap();
    let lock_path = reset_transaction_lock_path(&root);
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
    assert_reset_lock_released(&lock_path);
    fs::remove_dir_all(root).unwrap();
    let _ = fs::remove_file(lock_path);
}

#[test]
fn init_fresh_rejects_control_symlink_before_removing_legacy_state() {
    let root = temp_root("init-fresh-control-symlink");
    let outside = temp_root("init-fresh-control-symlink-target");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/records/reset-governance-record.json"),
        "legacy-reset\n",
    )
    .unwrap();
    let project_before = fs::read_to_string(root.join(".appsdk/project.json")).unwrap();
    let record_before =
        fs::read_to_string(root.join(".appsdk/records/reset-governance-record.json")).unwrap();
    let mut gitignore = fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
    gitignore.push_str("\n.appsdk-control\n");
    fs::write(root.join(".gitignore"), gitignore).unwrap();
    fs::remove_dir_all(root.join(".appsdk-control")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("sentinel"), "outside\n").unwrap();
    symlink(&outside, root.join(".appsdk-control")).unwrap();
    init_git(&root);

    let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("GOVERNANCE_PATH_SYMLINK:.appsdk-control"));
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/project.json")).unwrap(),
        project_before
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/records/reset-governance-record.json")).unwrap(),
        record_before
    );
    assert_eq!(
        fs::read_to_string(outside.join("sentinel")).unwrap(),
        "outside\n"
    );
    assert!(root.join(".appsdk-control").is_symlink());
    fs::remove_file(root.join(".appsdk-control")).unwrap();
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn init_fresh_rejects_non_directory_control_target_before_removing_legacy_state() {
    let root = temp_root("init-fresh-control-file");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/records/reset-governance-record.json"),
        "legacy-reset\n",
    )
    .unwrap();
    let project_before = fs::read_to_string(root.join(".appsdk/project.json")).unwrap();
    let record_before =
        fs::read_to_string(root.join(".appsdk/records/reset-governance-record.json")).unwrap();
    let mut gitignore = fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
    gitignore.push_str("\n.appsdk-control\n");
    fs::write(root.join(".gitignore"), gitignore).unwrap();
    fs::remove_dir_all(root.join(".appsdk-control")).unwrap();
    fs::write(root.join(".appsdk-control"), "legacy-control-file\n").unwrap();
    init_git(&root);

    let rejected = run(&["init", root_text, "--fresh", "--discard-legacy"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("GOVERNANCE_PATH_NOT_DIRECTORY:.appsdk-control"));
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/project.json")).unwrap(),
        project_before
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/records/reset-governance-record.json")).unwrap(),
        record_before
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk-control")).unwrap(),
        "legacy-control-file\n"
    );
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
        r#"{"sdk":"appsdk","version":"0.1.0009","digest":"sha256:replace-with-compiled-sdk-digest","compiler_digest":"sha256:replace-with-compiler-digest","bundle_digest":"sha256:replace-with-sdk-bundle-digest","bundle_manifest_digest":"sha256:replace-with-bundle-manifest-digest","contract_schema":1}
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
        serde_json::from_str::<Value>(include_str!("../../../contracts/sdk-bundle.manifest.json"))
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
fn reset_governance_refuses_git_and_collab_generated_roots_without_mutating_state() {
    for reserved in [".git", ".agent-collab"] {
        let root = temp_root(&format!("reset-reserved-generated-{reserved}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        let project_path = root.join(".appsdk/project.json");
        let project = fs::read_to_string(&project_path).unwrap();
        fs::write(
            &project_path,
            project.replace(
                "\"generated_root\": \"generated/**\"",
                &format!("\"generated_root\": \"{reserved}/**\""),
            ),
        )
        .unwrap();
        fs::write(root.join(".appsdk/legacy-record.json"), "legacy\n").unwrap();
        if reserved == ".agent-collab" {
            let mut gitignore = fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
            gitignore.push_str("\n.agent-collab/\n");
            fs::write(root.join(".gitignore"), gitignore).unwrap();
        }
        init_git(&root);
        let marker = root.join(reserved).join("keep");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, "must-remain\n").unwrap();
        let project_before = fs::read_to_string(&project_path).unwrap();

        let rejected = run(&["reset-governance", root_text, "--discard-legacy"]);
        assert!(!rejected.status.success());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("RESET_GENERATED_ROOT_CONFLICT"));
        assert_eq!(fs::read_to_string(&project_path).unwrap(), project_before);
        assert_eq!(
            fs::read_to_string(root.join(".appsdk/legacy-record.json")).unwrap(),
            "legacy\n"
        );
        assert_eq!(fs::read_to_string(&marker).unwrap(), "must-remain\n");
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn reset_governance_refuses_case_variant_reserved_roots_when_filesystem_merges_names() {
    for reserved in [".Git", ".Appsdk", ".AGENT-COLLAB", "Active", "Protected"] {
        let root = temp_root(&format!("reset-reserved-generated-case-{reserved}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        let project_path = root.join(".appsdk/project.json");
        let project = fs::read_to_string(&project_path).unwrap();
        fs::write(
            &project_path,
            project.replace(
                "\"generated_root\": \"generated/**\"",
                &format!("\"generated_root\": \"{reserved}/**\""),
            ),
        )
        .unwrap();
        fs::write(root.join(".appsdk/legacy-record.json"), "legacy\n").unwrap();
        fs::create_dir_all(root.join(".agent-collab")).unwrap();
        init_git(&root);

        let case_insensitive = filesystem_merges_case(&root);
        let project_before = fs::read_to_string(&project_path).unwrap();
        let legacy_before = fs::read_to_string(root.join(".appsdk/legacy-record.json")).unwrap();
        let rejected = run(&["reset-governance", root_text, "--discard-legacy"]);
        if case_insensitive {
            assert!(
                !rejected.status.success(),
                "stdout={} stderr={}",
                String::from_utf8_lossy(&rejected.stdout),
                String::from_utf8_lossy(&rejected.stderr)
            );
            assert!(
                String::from_utf8_lossy(&rejected.stderr).contains("RESET_GENERATED_ROOT_CONFLICT")
            );
            assert_eq!(fs::read_to_string(&project_path).unwrap(), project_before);
            assert_eq!(
                fs::read_to_string(root.join(".appsdk/legacy-record.json")).unwrap(),
                legacy_before
            );
            assert!(root.join(reserved).is_dir());
        } else {
            assert!(
                rejected.status.success(),
                "stdout={} stderr={}",
                String::from_utf8_lossy(&rejected.stdout),
                String::from_utf8_lossy(&rejected.stderr)
            );
            assert!(!root.join(reserved).exists());
            assert!(project_path.is_file());
        }
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn reset_governance_accepts_generated_subdirectory_roots() {
    let root = temp_root("reset-generated-subdirectory");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project_path = root.join(".appsdk/project.json");
    let project = fs::read_to_string(&project_path).unwrap();
    fs::write(
        &project_path,
        project.replace(
            "\"generated_root\": \"generated/**\"",
            "\"generated_root\": \"generated/modules/**\"",
        ),
    )
    .unwrap();
    fs::create_dir_all(root.join("generated/modules/app-core")).unwrap();
    fs::write(
        root.join("generated/modules/app-core/module.compiled.json"),
        "old\n",
    )
    .unwrap();
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
    assert!(!root.join("generated/modules").exists());
    assert_eq!(
        fs::read_to_string(root.join("protected/history/keep.txt")).unwrap(),
        "keep\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reset_governance_refuses_missing_invalid_or_unreadable_project_contract_without_mutating_state()
{
    for (name, expected_error) in [
        ("missing", "PROJECT_CONTRACT_MISSING"),
        ("malformed", "INVALID_PROJECT_CONTRACT"),
        (
            "type-error",
            "INVALID_GOVERNANCE_ROOT:/governance/generated_root",
        ),
        ("unreadable", "PROJECT_CONTRACT_MISSING"),
    ] {
        let root = temp_root(&format!("reset-invalid-project-{name}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        let project_path = root.join(".appsdk/project.json");
        let project = fs::read_to_string(&project_path).unwrap();
        match name {
            "missing" => fs::remove_file(&project_path).unwrap(),
            "malformed" => fs::write(&project_path, "{not-json}\n").unwrap(),
            "type-error" => fs::write(
                &project_path,
                project.replace(
                    "\"generated_root\": \"generated/**\"",
                    "\"generated_root\": 42",
                ),
            )
            .unwrap(),
            // `fs::read_to_string` on a directory fails even when tests run as
            // root, while chmod-based unreadability is not deterministic and
            // can dirty the test repo before the CLI performs its clean check.
            "unreadable" => {
                fs::remove_file(&project_path).unwrap();
                fs::create_dir_all(&project_path).unwrap();
            }
            _ => unreachable!(),
        }

        let receipt_path = root.join(".appsdk/records/evidence/reset-canary-receipt.json");
        fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
        fs::write(
            &receipt_path,
            r#"{"receipt_id":"reset-canary","status":"preserved"}
"#,
        )
        .unwrap();
        let marker_path = root.join(".appsdk/transactions/canary/marker.json");
        fs::create_dir_all(marker_path.parent().unwrap()).unwrap();
        fs::write(&marker_path, "marker-canary\n").unwrap();
        fs::create_dir_all(root.join("generated/old-artifact")).unwrap();
        fs::write(root.join("generated/old-artifact/artifact.bin"), "old\n").unwrap();
        init_git(&root);
        let project_before = if project_path.is_file() {
            Some(fs::read_to_string(&project_path).unwrap())
        } else {
            None
        };
        let receipt_before = fs::read_to_string(&receipt_path).unwrap();
        let marker_before = fs::read_to_string(&marker_path).unwrap();
        let rejected = run(&["reset-governance", root_text, "--discard-legacy"]);
        assert!(
            !rejected.status.success(),
            "case={name} stdout={} stderr={}",
            String::from_utf8_lossy(&rejected.stdout),
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains(expected_error),
            "case={name} stderr={}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        match (name, project_before) {
            ("missing", None) => assert!(!project_path.exists(), "case={name}"),
            ("unreadable", None) => assert!(project_path.is_dir(), "case={name}"),
            (_, Some(project_before)) => {
                assert_eq!(
                    fs::read_to_string(&project_path).unwrap(),
                    project_before,
                    "case={name}"
                );
            }
            (_, None) => unreachable!(),
        }
        assert_eq!(
            fs::read_to_string(&receipt_path).unwrap(),
            receipt_before,
            "case={name}"
        );
        assert_eq!(
            fs::read_to_string(&marker_path).unwrap(),
            marker_before,
            "case={name}"
        );
        assert!(
            root.join("generated/old-artifact/artifact.bin").is_file(),
            "case={name}"
        );
        assert!(
            !root
                .join(".appsdk/records/reset-governance-record.json")
                .exists(),
            "case={name}"
        );
        fs::remove_dir_all(root).unwrap();
    }
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
    let stdout = String::from_utf8_lossy(&first.stdout);
    let stderr = String::from_utf8_lossy(&first.stderr);
    let init_output = format!("{stdout}\n{stderr}");
    assert!(
        init_output.contains("COLLAB_INIT_")
            || init_output.contains("collab-channel")
            || init_output.contains("collab "),
        "init must report the server-owned Collab registration result: {init_output}"
    );
    if let Some(notice) = stdout.lines().find_map(|line| {
        line.strip_prefix("collab-channel ")
            .map(|body| serde_json::from_str::<serde_json::Value>(body).unwrap())
    }) {
        assert!(notice["transport_selected"].is_object());
        assert_eq!(notice["independent_work_allowed"], true);
    }
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
fn init_existing_collab_control_project_recovers_identity_without_preparation() {
    let root = temp_root("init-existing-collab-control");
    fs::create_dir_all(root.join(".agent-collab/server")).unwrap();

    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        format!(
            "#!/bin/sh\nif [ \"$1\" != \"init\" ]; then exit 64; fi\nprintf '%s\\n' '{{\"ok\":true,\"runtime\":{{\"runtimeId\":\"runtime-existing-collab-control\",\"appserverId\":\"appserver-cli\",\"namespace\":\"codex_tui\",\"endpoint\":\"unix:///tmp/codex.sock\",\"projectRoot\":\"{}\",\"capabilities\":[\"send_message_to_thread\"],\"processId\":4242}},\"transport_selected\":{{\"kind\":\"appserver\",\"endpoint\":\"unix:///tmp/codex.sock\",\"namespace\":\"codex_tui\",\"thread_id\":\"thread-existing-collab-control\",\"capabilities\":[\"send_message_to_thread\"],\"self_check\":\"test\"}}}}'\n",
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
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("collab-channel"), "{stdout}");
    assert!(stdout.contains("initialized collab identity"), "{stdout}");
    assert!(
        stdout.contains("thread-existing-collab-control"),
        "{stdout}"
    );
    assert!(!root.join(".appsdk/project.json").exists());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("PREPARATION_MISSING"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_accepts_registered_tmux_collab_channel_without_appserver_runtime_registration() {
    let root = temp_root("init-tmux-collab-channel");
    fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    let endpoint = serde_json::json!({
        "socket_path": "/tmp/collab-tmux-init.sock",
        "server_pid": 4242,
        "tmux_session_id": "$1",
        "pane_id": "%1",
        "pane_pid": 4343,
        "codex_session_id": "session-tmux-init",
        "codex_thread_id": "thread-tmux-init"
    });
    let response = serde_json::json!({
        "ok": true,
        "runtime": {
            "runtimeId": "runtime-tmux-init",
            "appserverId": "appserver-cli",
            "transport": "tmux",
            "tmuxEndpoint": endpoint,
            "projectRoot": root.canonicalize().unwrap(),
            "capabilities": ["send_message_to_pane", "probe_pane"],
            "processId": 4242
        },
        "transport_selected": {
            "kind": "tmux",
            "endpoint": "/tmp/collab-tmux-init.sock",
            "namespace": "$1",
            "session_id": "session-tmux-init",
            "thread_id": "thread-tmux-init",
            "tmux_endpoint": endpoint,
            "capabilities": ["send_message_to_pane", "probe_pane"],
            "self_check": "server verified registered tmux pane"
        }
    });
    fs::write(
        &fake_collab,
        format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", response),
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let registry_root = test_global_registry_root_for_project(&root);
    let output = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("APPSDK_HOME", &registry_root)
        .env("PATH", &fake_bin)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("collab-channel"), "{stdout}");
    assert!(stdout.contains("\"kind\":\"tmux\""), "{stdout}");
    assert!(stdout.contains("\"runtime_receipt\":null"), "{stdout}");
    assert!(
        !registry_root.join("runtimes.jsonl").exists(),
        "tmux pane identity must not be recorded as an AppServer runtime"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_sdk_source_workspace_uses_canonical_zone_transition_contract() {
    let root = temp_root("init-sdk-source-workspace");
    fs::create_dir_all(root.join("contracts/transitions")).unwrap();
    fs::write(
        root.join("contracts/transitions/zone-transition-manifest.json"),
        include_str!("../../../contracts/transitions/zone-transition-manifest.json"),
    )
    .unwrap();
    fs::write(
        root.join("contracts/transitions/zone-transition.manifest.json"),
        include_str!("../../../contracts/transitions/zone-transition.manifest.json"),
    )
    .unwrap();
    let root_text = root.to_str().unwrap();
    confirm_preparation(&root, ".", "project_refactor");

    let initialized = run(&["init", root_text]);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    let project: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/project.json")).unwrap())
            .unwrap();
    assert_eq!(
        project["governance"]["zone_transition_contract"],
        "contracts/transitions/zone-transition.manifest.json"
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
fn init_preserves_legacy_zone_transition_contract_alias() {
    let root = temp_root("init-legacy-zone-transition-alias");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["governance"]["zone_transition_contract"] =
        Value::String("contracts/transitions/zone-transition-manifest.json".into());
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();

    let initialized = run(&["init", root_text]);
    assert!(
        initialized.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&initialized.stdout),
        String::from_utf8_lossy(&initialized.stderr)
    );
    let after_init: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    assert_eq!(
        after_init["governance"]["zone_transition_contract"],
        "contracts/transitions/zone-transition-manifest.json"
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
        serde_json::from_str(include_str!("../../../contracts/maps/module-registry.json")).unwrap();
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
fn lifecycle_producers_accept_project_maps_without_sdk_producer_projection() {
    let root = temp_root("lifecycle-producer-custom-project-maps");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    for (map_name, key) in [
        ("resource-map.json", "resources"),
        ("function-map.json", "functions"),
        ("mainline-call-map.json", "edges"),
        ("verification-map.json", "gates"),
    ] {
        let map_path = root.join(".appsdk/maps").join(map_name);
        let mut map: Value = serde_json::from_str(&fs::read_to_string(&map_path).unwrap()).unwrap();
        map[key].as_array_mut().unwrap().retain(|entry| {
            let id = entry
                .get("resource_id")
                .or_else(|| entry.get("function_id"))
                .or_else(|| entry.get("chain_id"))
                .or_else(|| entry.get("gate_id"))
                .and_then(Value::as_str);
            !match (map_name, id) {
                ("resource-map.json", Some("lifecycle_record_producer_input"))
                | ("resource-map.json", Some("lifecycle_chain_producer_input"))
                | ("resource-map.json", Some("fix_worktree"))
                | ("resource-map.json", Some("fix_evidence_set"))
                | ("resource-map.json", Some("fix_reproduction"))
                | ("function-map.json", Some("lifecycle_record_producer"))
                | ("function-map.json", Some("lifecycle_chain_record_producer"))
                | ("mainline-call-map.json", Some("lifecycle-record-production-v1"))
                | ("mainline-call-map.json", Some("lifecycle-record-chain-production-v1"))
                | ("verification-map.json", Some("worktree_clean"))
                | ("verification-map.json", Some("baseline_reproduced"))
                | ("verification-map.json", Some("lifecycle_chain_record_producer")) => true,
                _ => false,
            }
        });
        fs::write(
            &map_path,
            serde_json::to_string_pretty(&map).unwrap() + "\n",
        )
        .unwrap();
    }

    let input = root.join("producer-input.json");
    fs::write(&input, "{}\n").unwrap();
    let records = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!records.status.success());
    assert!(String::from_utf8_lossy(&records.stderr).contains("GOAL_NOT_CONFIRMED:received"));
    assert!(!String::from_utf8_lossy(&records.stderr).contains("LIFECYCLE_PRODUCER_MAP_TAMPERED"));
    assert!(!root
        .join(".appsdk/records/worktree-record-app-core.json")
        .exists());

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
    let chain = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "architecture",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!chain.status.success());
    assert!(String::from_utf8_lossy(&chain.stderr).contains("PRODUCER_ARCHITECTURE_INPUT_MISSING"));
    assert!(!String::from_utf8_lossy(&chain.stderr).contains("LIFECYCLE_PRODUCER_MAP_TAMPERED"));
    fs::remove_dir_all(root).unwrap();
}
