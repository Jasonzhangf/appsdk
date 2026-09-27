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
