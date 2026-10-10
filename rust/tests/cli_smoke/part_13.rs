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
    let old_migration = root.join(".appsdk/migrations/0.1.5-to-0.1.6");
    if old_migration.exists() {
        fs::remove_dir_all(old_migration).unwrap();
    }
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

    // Keep both immutable archive layouts for the selector regression. The
    // versioned archive is authoritative when it exists; the compatibility
    // history archive remains a fallback for older publications.
    let version_archive = root.join("protected/history-versions/app-core/active-v1");
    fs::create_dir_all(version_archive.join("library")).unwrap();
    for name in [
        "freeze-artifact.json",
        "module-artifact.json",
        "module-contract.json",
        "source-snapshot.json",
    ] {
        fs::copy(
            root.join("protected/history/app-core").join(name),
            version_archive.join(name),
        )
        .unwrap();
    }
    for entry in module_artifact["artifacts"].as_array().unwrap() {
        let relative = entry["path"].as_str().unwrap();
        let source = root
            .join("protected/history/app-core/library")
            .join(relative);
        let target = version_archive.join("library").join(relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(source, target).unwrap();
    }

    // A present generated module projection is checked before any historical
    // fallback. Corruption must fail closed even though the protected archive
    // remains available.
    let generated_module_artifact = root.join("generated/modules/app-core/module.compiled.json");
    let generated_module_artifact_text = fs::read_to_string(&generated_module_artifact).unwrap();
    fs::write(&generated_module_artifact, "{}\n").unwrap();
    let tampered_generated = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!tampered_generated.status.success());
    assert!(String::from_utf8_lossy(&tampered_generated.stderr)
        .contains("MODULE_ARTIFACT_MISMATCH:app-core"));
    fs::write(&generated_module_artifact, generated_module_artifact_text).unwrap();

    // The generated fast path still has to prove the immutable publication
    // graph. A damaged protected archive cannot be hidden by a valid
    // generated projection.
    let archive_with_generated = version_archive.clone();
    let archive_with_generated_text =
        fs::read_to_string(archive_with_generated.join("module-artifact.json")).unwrap();
    fs::write(archive_with_generated.join("module-artifact.json"), "{}\n").unwrap();
    let tampered_archive_with_generated = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!tampered_archive_with_generated.status.success());
    assert!(
        String::from_utf8_lossy(&tampered_archive_with_generated.stderr)
            .contains("MODULE_ARTIFACT_HISTORY_HASH_MISMATCH:app-core")
    );
    fs::write(
        archive_with_generated.join("module-artifact.json"),
        archive_with_generated_text,
    )
    .unwrap();

    fs::remove_dir_all(root.join("generated")).unwrap();
    fs::remove_dir_all(root.join("active")).unwrap();

    // Review admission for an immutable module must resolve the published
    // artifact from protected history after the rebuildable checkout
    // projection has been removed. Historical evidence may also be expired
    // now; it remains admissible only through the historical graph path.
    let evidence_file = root.join(".appsdk/records/evidence-record-app-core.json");
    let mut expired_historical_evidence: Value =
        serde_json::from_str(&fs::read_to_string(&evidence_file).unwrap()).unwrap();
    expired_historical_evidence["expires_at"] = Value::String("2026-01-02T00:00:00Z".into());
    fs::write(
        &evidence_file,
        serde_json::to_string_pretty(&expired_historical_evidence).unwrap() + "\n",
    )
    .unwrap();
    let historical_admission = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(
        historical_admission.status.success(),
        "{}",
        String::from_utf8_lossy(&historical_admission.stderr)
    );
    assert!(
        String::from_utf8_lossy(&historical_admission.stdout).contains("\"mode\":\"historical\"")
    );

    // Historical freshness is evaluated as of publication, not as of the
    // current clock. An expiry after the freeze but before now is valid (the
    // successful admission above); an expiry before the publication terminal
    // timestamp is invalid and must not be resurrected by Historical mode.
    let mut expired_before_publication = expired_historical_evidence.clone();
    expired_before_publication["expires_at"] = Value::String("2026-01-01T00:03:01Z".into());
    fs::write(
        &evidence_file,
        serde_json::to_string_pretty(&expired_before_publication).unwrap() + "\n",
    )
    .unwrap();
    let historical_expired_before_publication = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!historical_expired_before_publication.status.success());
    assert!(
        String::from_utf8_lossy(&historical_expired_before_publication.stderr)
            .contains("EXPIRED_EVIDENCE_RECORD:evidence-record.json")
    );
    fs::write(
        &evidence_file,
        serde_json::to_string_pretty(&expired_historical_evidence).unwrap() + "\n",
    )
    .unwrap();

    // A valid versioned archive and a valid compatibility archive are the
    // same publication, not an ambiguity. Corrupting the lower-priority
    // compatibility copy must not shadow the versioned source.
    let current_archive = root.join("protected/history/app-core");
    let current_archive_text =
        fs::read_to_string(current_archive.join("module-artifact.json")).unwrap();
    fs::write(current_archive.join("module-artifact.json"), "{}\n").unwrap();
    let version_priority = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(
        version_priority.status.success(),
        "{}",
        String::from_utf8_lossy(&version_priority.stderr)
    );
    fs::write(
        current_archive.join("module-artifact.json"),
        current_archive_text,
    )
    .unwrap();

    // Once the versioned archive exists, its corruption is terminal even if
    // the compatibility archive is intact; there is no silent fallback.
    let version_archive_artifact = version_archive.join("module-artifact.json");
    let version_archive_text = fs::read_to_string(&version_archive_artifact).unwrap();
    fs::write(&version_archive_artifact, "{}\n").unwrap();
    let damaged_version = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!damaged_version.status.success());
    assert!(String::from_utf8_lossy(&damaged_version.stderr)
        .contains("MODULE_ARTIFACT_HISTORY_HASH_MISMATCH:app-core"));
    fs::write(&version_archive_artifact, version_archive_text).unwrap();

    // Removing the complete version archive re-enables the compatibility
    // fallback, which remains covered by the same entrypoint.
    fs::remove_dir_all(&version_archive).unwrap();
    let compatibility_fallback = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(
        compatibility_fallback.status.success(),
        "{}",
        String::from_utf8_lossy(&compatibility_fallback.stderr)
    );

    // A damaged or missing immutable archive must fail closed. The verifier
    // must not fall back to source or recreate a generated artifact during
    // review admission.
    let historical_archive = root.join("protected/history/app-core/module-artifact.json");
    let historical_archive_text = fs::read_to_string(&historical_archive).unwrap();
    fs::write(&historical_archive, "{}\n").unwrap();
    let tampered_history = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!tampered_history.status.success());
    assert!(String::from_utf8_lossy(&tampered_history.stderr)
        .contains("MODULE_ARTIFACT_HISTORY_HASH_MISMATCH:app-core"));
    fs::write(&historical_archive, historical_archive_text).unwrap();

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

    // A frozen publication remains recoverable after the delivery evidence
    // freshness window closes. Rehydration validates the historical graph and
    // immutable artifact, while current admission still requires fresh
    // evidence.
    let historical_evidence = root.join(".appsdk/records/evidence-record-app-core.json");
    let mut historical_evidence_value: Value =
        serde_json::from_str(&fs::read_to_string(&historical_evidence).unwrap()).unwrap();
    historical_evidence_value["expires_at"] = Value::String("2026-01-02T00:00:00Z".into());
    fs::write(
        &historical_evidence,
        serde_json::to_string_pretty(&historical_evidence_value).unwrap() + "\n",
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

    historical_evidence_value["expires_at"] = Value::String("2099-01-01T00:00:00Z".into());
    fs::write(
        &historical_evidence,
        serde_json::to_string_pretty(&historical_evidence_value).unwrap() + "\n",
    )
    .unwrap();
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
