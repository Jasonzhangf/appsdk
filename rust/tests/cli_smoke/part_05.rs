#[test]
fn lifecycle_chain_reenters_non_pass_downstream_stages_and_preserves_attempt_history() {
    let root = temp_root("lifecycle-chain-downstream-reentry");
    let root_text = root.to_str().unwrap();
    let artifact_hash = prepare_lifecycle_chain_fixture(&root);
    let records = root.join(".appsdk/records");

    let mark_effectiveness_non_pass = || {
        let path = records.join("effectiveness-record-app-core.json");
        let mut record: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        record["result"] = Value::String("fail".into());
        fs::write(&path, serde_json::to_string_pretty(&record).unwrap() + "\n").unwrap();
    };
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
    mark_effectiveness_non_pass();
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
    assert_eq!(
        serde_json::from_slice::<Value>(&effectiveness.stdout).unwrap()["reused"],
        false
    );
    let effectiveness_attempts = records.join("attempts/app-core/effectiveness-record.jsonl");
    let effectiveness_attempt = serde_json::from_str::<Value>(
        fs::read_to_string(&effectiveness_attempts)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(effectiveness_attempt["record"]["result"], "fail");

    let merge_path = records.join("merge-record-app-core.json");
    let mut merge_record: Value =
        serde_json::from_str(&fs::read_to_string(&merge_path).unwrap()).unwrap();
    merge_record["result"] = Value::String("fail".into());
    fs::write(
        &merge_path,
        serde_json::to_string_pretty(&merge_record).unwrap() + "\n",
    )
    .unwrap();
    let merge_input = root.join("merge-input.json");
    fs::write(&merge_input, r#"{"merge":{"mainline_ref":"HEAD"}}"#).unwrap();
    let merge = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "merge",
        "--input",
        merge_input.to_str().unwrap(),
    ]);
    assert!(
        merge.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&merge.stdout),
        String::from_utf8_lossy(&merge.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&merge.stdout).unwrap()["reused"],
        false
    );
    let merge_attempts = records.join("attempts/app-core/merge-record.jsonl");
    let merge_attempt = serde_json::from_str::<Value>(
        fs::read_to_string(&merge_attempts)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(merge_attempt["record"]["result"], "fail");

    let promotion_path = records.join("promotion-record-app-core.json");
    let mut promotion_record: Value =
        serde_json::from_str(&fs::read_to_string(&promotion_path).unwrap()).unwrap();
    promotion_record["required_gate_results"][0]["result"] = Value::String("fail".into());
    fs::write(
        &promotion_path,
        serde_json::to_string_pretty(&promotion_record).unwrap() + "\n",
    )
    .unwrap();
    let promotion_input = root.join("promotion-input.json");
    fs::write(
        &promotion_input,
        serde_json::to_string_pretty(&serde_json::json!({
            "promotion": {
                "experiment_id": "experiment-1",
                "new_active_version": "active-v2",
                "previous_active_version": null,
                "compatibility_level": "compatible",
                "evidence_ids": ["candidate-evidence-1"],
                "required_gate_results": [
                    {"gate_id":"goal_confirmed","result":"pass","producer":"test"},
                    {"gate_id":"contract_valid","result":"pass","producer":"test"},
                    {"gate_id":"sdk_lock_integrity","result":"pass","producer":"test"},
                    {"gate_id":"remote_main_receipt","result":"pass","producer":"test"},
                    {"gate_id":"mainline_merge_identity","result":"pass","producer":"test"},
                    {"gate_id":"fix_lifecycle_graph","result":"pass","producer":"test"},
                    {"gate_id":"artifact_hash","result":"pass","producer":"test"},
                    {"gate_id":"lifecycle_chain_record_producer","result":"pass","producer":"test"}
                ],
                "change_set_id": "change-2",
                "root_cause": "root cause",
                "design_id": "design-1",
                "change_reason_comment": "reason",
                "playground_cleanup_record_id": "cleanup-1",
                "artifact_hash": artifact_hash
            }
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let promotion = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "promotion",
        "--input",
        promotion_input.to_str().unwrap(),
    ]);
    assert!(
        promotion.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&promotion.stdout),
        String::from_utf8_lossy(&promotion.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&promotion.stdout).unwrap()["reused"],
        false
    );
    let promotion_attempts = records.join("attempts/app-core/promotion-record.jsonl");
    let promotion_attempt = serde_json::from_str::<Value>(
        fs::read_to_string(&promotion_attempts)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        promotion_attempt["record"]["required_gate_results"][0]["result"],
        "fail"
    );

    for (phase, input) in [
        ("effectiveness", &effectiveness_input),
        ("merge", &merge_input),
        ("promotion", &promotion_input),
    ] {
        let reused = run(&[
            "produce-lifecycle-chain",
            root_text,
            "--module",
            "app-core",
            "--phase",
            phase,
            "--input",
            input.to_str().unwrap(),
        ]);
        assert!(
            reused.status.success(),
            "phase={phase} stdout={} stderr={}",
            String::from_utf8_lossy(&reused.stdout),
            String::from_utf8_lossy(&reused.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&reused.stdout).unwrap()["reused"],
            true
        );
    }
    assert_eq!(
        fs::read_to_string(&effectiveness_attempts)
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_eq!(
        fs::read_to_string(&merge_attempts).unwrap().lines().count(),
        1
    );
    assert_eq!(
        fs::read_to_string(&promotion_attempts)
            .unwrap()
            .lines()
            .count(),
        1
    );
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
                {"gate_id":"goal_confirmed","result":"pass","producer":"test"},
                {"gate_id":"contract_valid","result":"pass","producer":"test"},
                {"gate_id":"sdk_lock_integrity","result":"pass","producer":"test"},
                {"gate_id":"remote_main_receipt","result":"pass","producer":"test"},
                {"gate_id":"lifecycle_chain_record_producer","result":"pass","producer":"test"},
                {"gate_id":"fix_lifecycle_graph","result":"pass","producer":"forged"},
                {"gate_id":"mainline_merge_identity","result":"pass","producer":"forged"},
                {"gate_id":"artifact_hash","result":"pass","producer":"test"}
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
                {"gate_id":"goal_confirmed","result":"pass","producer":"test"},
                {"gate_id":"contract_valid","result":"pass","producer":"test"},
                {"gate_id":"sdk_lock_integrity","result":"pass","producer":"test"},
                {"gate_id":"remote_main_receipt","result":"pass","producer":"test"},
                {"gate_id":"mainline_merge_identity","result":"pass","producer":"test"},
                {"gate_id":"fix_lifecycle_graph","result":"pass","producer":"test"},
                {"gate_id":"artifact_hash","result":"pass","producer":"test"},
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
    assert_eq!(promotion["bug_closure_verified"], true);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_promotion_supports_parallel_first_create_and_reuse() {
    let root = temp_root("lifecycle-chain-promotion-parallel-first-create");
    let root_text = root.to_str().unwrap();
    let artifact_hash = prepare_lifecycle_chain_fixture(&root);
    enable_parallel_development(&root);
    write_parallel_records(&root, "app-core", &artifact_hash, false);
    let records = root.join(".appsdk/records");
    fs::remove_file(records.join("promotion-record-app-core.json")).unwrap();
    let integration_commit = git_test_value(&root, &["rev-parse", "refs/heads/test-mainline"]);
    install_fixture_collab_cli(&root, &artifact_hash, &integration_commit);
    write_collab_live_closure_fixture(&root, "app-core", &artifact_hash, &integration_commit);
    let candidate_evidence = records.join("evidence/app-core/candidate-evidence-1.json");
    let mut candidate_evidence_value: Value =
        serde_json::from_str(&fs::read_to_string(&candidate_evidence).unwrap()).unwrap();
    candidate_evidence_value["source_commit"] = Value::String(integration_commit);
    fs::write(
        &candidate_evidence,
        serde_json::to_string_pretty(&candidate_evidence_value).unwrap() + "\n",
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
                {"gate_id":"goal_confirmed","result":"pass","producer":"test"},
                {"gate_id":"contract_valid","result":"pass","producer":"test"},
                {"gate_id":"sdk_lock_integrity","result":"pass","producer":"test"},
                {"gate_id":"remote_main_receipt","result":"pass","producer":"test"},
                {"gate_id":"mainline_merge_identity","result":"pass","producer":"test"},
                {"gate_id":"fix_lifecycle_graph","result":"pass","producer":"test"},
                {"gate_id":"artifact_hash","result":"pass","producer":"test"},
                {"gate_id":"lifecycle_chain_record_producer","result":"pass","producer":"test"}
            ],
            "collaboration_record_id": "collaboration-1",
            "merge_queue_record_id": "queue-1",
            "integration_record_id": "integration-1",
            "mainline_receipt_record_id": "receipt-1",
            "collab_live_closure_record_id": "fixture-not-live",
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

    let mut command = Command::new(binary());
    command
        .args([
            "produce-lifecycle-chain",
            root_text,
            "--module",
            "app-core",
            "--phase",
            "promotion",
            "--input",
            input.to_str().unwrap(),
        ])
        .env(
            "APPSDK_HOME",
            test_global_registry_root_for_args(&[root_text]),
        )
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("fixture-bin").display(),
                env::var("PATH").unwrap()
            ),
        )
        .env_remove("TMUX_PANE");
    apply_test_git_bug_fixture(&mut command, &root);
    let first = command.output().unwrap();
    assert!(
        first.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first_json: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first_json["reused"], false);
    let persisted: Value = serde_json::from_str(
        &fs::read_to_string(records.join("promotion-record-app-core.json")).unwrap(),
    )
    .unwrap();
    for field in [
        "collaboration_record_id",
        "merge_queue_record_id",
        "integration_record_id",
        "mainline_receipt_record_id",
        "collab_live_closure_record_id",
    ] {
        assert_eq!(persisted[field], input_value["promotion"][field]);
    }
    assert_eq!(persisted["bug_closure_verified"], true);

    let mut command = Command::new(binary());
    command
        .args([
            "produce-lifecycle-chain",
            root_text,
            "--module",
            "app-core",
            "--phase",
            "promotion",
            "--input",
            input.to_str().unwrap(),
        ])
        .env(
            "APPSDK_HOME",
            test_global_registry_root_for_args(&[root_text]),
        )
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("fixture-bin").display(),
                env::var("PATH").unwrap()
            ),
        )
        .env_remove("TMUX_PANE");
    apply_test_git_bug_fixture(&mut command, &root);
    let second = command.output().unwrap();
    assert!(
        second.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&second.stdout).unwrap()["reused"],
        true
    );
    fs::remove_dir_all(root).unwrap();
}

fn prepare_parallel_promotion_fixture(name: &str) -> (PathBuf, PathBuf, Value) {
    let root = temp_root(name);
    let artifact_hash = prepare_lifecycle_chain_fixture(&root);
    enable_parallel_development(&root);
    write_parallel_records(&root, "app-core", &artifact_hash, false);
    let records = root.join(".appsdk/records");
    fs::remove_file(records.join("promotion-record-app-core.json")).unwrap();
    let integration_commit = git_test_value(&root, &["rev-parse", "refs/heads/test-mainline"]);
    install_fixture_collab_cli(&root, &artifact_hash, &integration_commit);
    write_collab_live_closure_fixture(&root, "app-core", &artifact_hash, &integration_commit);
    let candidate_evidence = records.join("evidence/app-core/candidate-evidence-1.json");
    let mut candidate_evidence_value: Value =
        serde_json::from_str(&fs::read_to_string(&candidate_evidence).unwrap()).unwrap();
    candidate_evidence_value["source_commit"] = Value::String(integration_commit);
    fs::write(
        &candidate_evidence,
        serde_json::to_string_pretty(&candidate_evidence_value).unwrap() + "\n",
    )
    .unwrap();
    let input = serde_json::json!({
        "promotion": {
            "experiment_id": "experiment-1",
            "new_active_version": "active-v2",
            "previous_active_version": null,
            "compatibility_level": "compatible",
            "evidence_ids": ["candidate-evidence-1"],
            "required_gate_results": [
                {"gate_id":"goal_confirmed","result":"pass","producer":"test"},
                {"gate_id":"contract_valid","result":"pass","producer":"test"},
                {"gate_id":"sdk_lock_integrity","result":"pass","producer":"test"},
                {"gate_id":"remote_main_receipt","result":"pass","producer":"test"},
                {"gate_id":"mainline_merge_identity","result":"pass","producer":"test"},
                {"gate_id":"fix_lifecycle_graph","result":"pass","producer":"test"},
                {"gate_id":"artifact_hash","result":"pass","producer":"test"},
                {"gate_id":"lifecycle_chain_record_producer","result":"pass","producer":"test"}
            ],
            "collaboration_record_id": "collaboration-1",
            "merge_queue_record_id": "queue-1",
            "integration_record_id": "integration-1",
            "mainline_receipt_record_id": "receipt-1",
            "collab_live_closure_record_id": "fixture-not-live",
            "change_set_id": "change-2",
            "root_cause": "root cause",
            "design_id": "design-1",
            "change_reason_comment": "reason",
            "playground_cleanup_record_id": "cleanup-1",
            "artifact_hash": artifact_hash
        }
    });
    let input_path = root.join("promotion-input.json");
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&input).unwrap() + "\n",
    )
    .unwrap();
    (root, input_path, input)
}

fn run_parallel_promotion(
    root: &Path,
    input: &Path,
    path_prefix: Option<&Path>,
) -> std::process::Output {
    let mut command = Command::new(binary());
    command.args([
        "produce-lifecycle-chain",
        root.to_str().unwrap(),
        "--module",
        "app-core",
        "--phase",
        "promotion",
        "--input",
        input.to_str().unwrap(),
    ]);
    command
        .env("APPSDK_HOME", test_global_registry_root_for_project(root))
        .env_remove("TMUX_PANE");
    apply_test_git_bug_fixture(&mut command, root);
    if let Some(prefix) = path_prefix {
        command.env(
            "PATH",
            format!("{}:{}", prefix.display(), env::var("PATH").unwrap()),
        );
    }
    command.output().unwrap()
}

#[test]
fn lifecycle_chain_parallel_promotion_rejects_missing_collab_live_closure_path() {
    let (root, input, _) = prepare_parallel_promotion_fixture("collab-live-closure-missing-path");
    let closure_path = root.join(".appsdk/records/collab-live-closure-fixture-not-live.json");
    let mut closure: Value =
        serde_json::from_str(&fs::read_to_string(&closure_path).unwrap()).unwrap();
    closure["path_receipts"]
        .as_object_mut()
        .unwrap()
        .remove("restart_replay");
    fs::write(
        &closure_path,
        serde_json::to_string_pretty(&closure).unwrap() + "\n",
    )
    .unwrap();
    let result = run_parallel_promotion(&root, &input, Some(&root.join("fixture-bin")));
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("COLLAB_LIVE_CLOSURE_MATRIX_MISSING"),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_parallel_promotion_rejects_reused_collab_live_evidence() {
    let (root, input, _) = prepare_parallel_promotion_fixture("collab-live-closure-reused");
    let closure_path = root.join(".appsdk/records/collab-live-closure-fixture-not-live.json");
    let mut closure: Value =
        serde_json::from_str(&fs::read_to_string(&closure_path).unwrap()).unwrap();
    let peer_to_peer = closure["evidence_ids"]["peer_to_peer"].clone();
    closure["evidence_ids"]["restart_replay"] = peer_to_peer.clone();
    closure["path_receipts"]["restart_replay"]["evidence_id"] = peer_to_peer;
    fs::write(
        &closure_path,
        serde_json::to_string_pretty(&closure).unwrap() + "\n",
    )
    .unwrap();
    let result = run_parallel_promotion(&root, &input, Some(&root.join("fixture-bin")));
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("COLLAB_LIVE_CLOSURE_EVIDENCE_REUSED:restart_replay"),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_parallel_promotion_rejects_reused_collab_live_message() {
    let (root, input, _) = prepare_parallel_promotion_fixture("collab-live-closure-message-reused");
    let closure_path = root.join(".appsdk/records/collab-live-closure-fixture-not-live.json");
    let mut closure: Value =
        serde_json::from_str(&fs::read_to_string(&closure_path).unwrap()).unwrap();
    let peer_to_peer = closure["path_receipts"]["peer_to_peer"]["message_id"].clone();
    closure["path_receipts"]["restart_replay"]["message_id"] = peer_to_peer;
    fs::write(
        &closure_path,
        serde_json::to_string_pretty(&closure).unwrap() + "\n",
    )
    .unwrap();
    let result = run_parallel_promotion(&root, &input, Some(&root.join("fixture-bin")));
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("COLLAB_LIVE_CLOSURE_MESSAGE_REUSED:restart_replay"),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_parallel_promotion_rejects_collab_live_identity_drift() {
    let (root, input, _) = prepare_parallel_promotion_fixture("collab-live-closure-identity-drift");
    let closure_path = root.join(".appsdk/records/collab-live-closure-fixture-not-live.json");
    let mut closure: Value =
        serde_json::from_str(&fs::read_to_string(&closure_path).unwrap()).unwrap();
    closure["path_receipts"]["daemon_to_master"]["artifact_hash"] =
        Value::String("artifact-2".into());
    fs::write(
        &closure_path,
        serde_json::to_string_pretty(&closure).unwrap() + "\n",
    )
    .unwrap();
    let result = run_parallel_promotion(&root, &input, Some(&root.join("fixture-bin")));
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("COLLAB_LIVE_CLOSURE_PATH_MISMATCH:daemon_to_master"),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_parallel_promotion_rejects_wrong_collab_live_direction() {
    let (root, input, _) =
        prepare_parallel_promotion_fixture("collab-live-closure-wrong-direction");
    let closure_path = root.join(".appsdk/records/collab-live-closure-fixture-not-live.json");
    let mut closure: Value =
        serde_json::from_str(&fs::read_to_string(&closure_path).unwrap()).unwrap();
    closure["path_receipts"]["master_to_peer"]["sender"] = Value::String("peer".into());
    fs::write(
        &closure_path,
        serde_json::to_string_pretty(&closure).unwrap() + "\n",
    )
    .unwrap();
    let result = run_parallel_promotion(&root, &input, Some(&root.join("fixture-bin")));
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("COLLAB_LIVE_CLOSURE_PATH_DIRECTION_MISMATCH:master_to_peer"),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_parallel_promotion_rejects_source_only_collab_evidence() {
    let (root, input, _) = prepare_parallel_promotion_fixture("collab-live-closure-source-only");
    let evidence_path = root.join(".appsdk/records/evidence/app-core/collab-peer-to-peer.json");
    let mut evidence: Value =
        serde_json::from_str(&fs::read_to_string(&evidence_path).unwrap()).unwrap();
    evidence["phase"] = Value::String("fix_candidate".into());
    evidence["kind"] = Value::String("build".into());
    fs::write(
        &evidence_path,
        serde_json::to_string_pretty(&evidence).unwrap() + "\n",
    )
    .unwrap();
    let result = run_parallel_promotion(&root, &input, Some(&root.join("fixture-bin")));
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("COLLAB_LIVE_CLOSURE_EVIDENCE_MISMATCH:peer_to_peer"),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_parallel_promotion_preserves_route_unavailable_error() {
    let (root, input, _) =
        prepare_parallel_promotion_fixture("collab-live-closure-route-unavailable");
    let unavailable_bin = root.join("unavailable-bin");
    fs::create_dir_all(&unavailable_bin).unwrap();
    let collab = unavailable_bin.join("collab");
    fs::write(
        &collab,
        "#!/bin/sh\nprintf '%s\\n' 'DAEMON_UNKNOWN: cannot reach collab daemon' >&2\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&collab, fs::Permissions::from_mode(0o755)).unwrap();
    let result = run_parallel_promotion(&root, &input, Some(&unavailable_bin));
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("DAEMON_UNKNOWN"), "stderr={}", stderr);
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
