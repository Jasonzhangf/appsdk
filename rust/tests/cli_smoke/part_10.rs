#[test]
fn retire_lifecycle_records_honors_producer_lock() {
    let root = prepare_retire_fixture("retire-producer-lock", "stale-issue", "stale-issue");
    let control_dir = root.join(".appsdk-control");
    fs::create_dir_all(&control_dir).unwrap();
    let lock_path = control_dir.join("lifecycle-record-producer.lock");
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .unwrap();
    hold_advisory_lock(&lock_file);

    let result = run(&[
        "retire-lifecycle-records",
        root.to_str().unwrap(),
        "--module",
        "app-core",
        "--issue",
        "current-issue",
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("PRODUCER_BUSY"));
    assert!(root
        .join(".appsdk/records/fix-candidate-record-app-core.json")
        .is_file());
    assert!(root
        .join(".appsdk/records/pre-review-validation-record-app-core.json")
        .is_file());

    drop(lock_file);
    fs::remove_dir_all(root).unwrap();
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
    renew_fixture_requirements_review(&root, module_id);
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
fn ordinary_verify_tolerates_absent_goal_but_mutation_requires_it() {
    let root = temp_root("verify-without-goal");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let goal_file = root.join(".appsdk/goal.json");
    let original_goal = fs::read_to_string(&goal_file).unwrap();
    fs::remove_file(&goal_file).unwrap();

    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );

    let admission = run(&["verify", "--admission", root_text]);
    assert!(
        admission.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&admission.stdout),
        String::from_utf8_lossy(&admission.stderr)
    );

    let review_admission = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!review_admission.status.success());
    assert!(String::from_utf8_lossy(&review_admission.stderr)
        .contains("MISSING_GOAL_CLARIFICATION_RECORD"));

    let compile = run(&["compile", root_text]);
    assert!(!compile.status.success());
    assert!(String::from_utf8_lossy(&compile.stderr).contains("MISSING_GOAL_CLARIFICATION_RECORD"));

    fs::write(&goal_file, original_goal).unwrap();
    let unconfirmed = run(&["verify", "--admission", root_text]);
    assert!(
        unconfirmed.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&unconfirmed.stdout),
        String::from_utf8_lossy(&unconfirmed.stderr)
    );
    let review_unconfirmed = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "app-core",
    ]);
    assert!(!review_unconfirmed.status.success());
    assert!(
        String::from_utf8_lossy(&review_unconfirmed.stderr).contains("GOAL_NOT_CONFIRMED:received")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn development_stages_allow_omitted_regression_until_freeze() {
    let root = temp_root("development-without-regression");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let mut project: Value = serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();
    project["modules"][0]
        .as_object_mut()
        .unwrap()
        .remove("regression");
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let goal_path = root.join(".appsdk/goal.json");
    let mut goal: Value = serde_json::from_slice(&fs::read(&goal_path).unwrap()).unwrap();
    goal["status"] = Value::String("confirmed".into());
    goal["confirmed_by"] = Value::String("test".into());
    goal["confirmed_at"] = Value::String("2026-01-01T00:00:00Z".into());
    fs::write(
        &goal_path,
        serde_json::to_string_pretty(&goal).unwrap() + "\n",
    )
    .unwrap();
    init_git(&root);

    assert!(run(&["verify", root_text]).status.success());
    assert!(run(&["promote", root_text, "--to", "source_implemented"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    let compiled = run(&["compile", root_text]);
    assert!(
        compiled.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );
    let artifact: Value =
        serde_json::from_slice(&fs::read(root.join("generated/project.compiled.json")).unwrap())
            .unwrap();
    assert!(artifact["modules"][0].get("regression").is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_rejects_frozen_module_without_regression_contract() {
    let root = temp_root("frozen-without-regression");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let mut project: Value = serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();
    project["modules"][0]["stage"] = Value::String("frozen".into());
    project["modules"][0]
        .as_object_mut()
        .unwrap()
        .remove("regression");
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();

    let rejected = run(&["verify", "--admission", root_text]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("REGRESSION_CONTRACT_REQUIRED:app-core")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn architecture_stable_rejects_disabled_regression_before_freeze() {
    let root = temp_root("stable-disabled-regression");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let mut project: Value = serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();
    project["lifecycle"]["stage"] = Value::String("architecture_stable".into());
    project["modules"][0]["stage"] = Value::String("architecture_stable".into());
    project["modules"][0]["regression"]["required_before_freeze"] = Value::Bool(false);
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();

    let rejected = run(&["verify", "--admission", root_text]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("INVALID_REGRESSION_CONTRACT:app-core")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn freeze_rejects_missing_regression_before_persisting_frozen_state() {
    let root = temp_root("freeze-without-regression");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let mut project: Value = serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();
    project["lifecycle"]["stage"] = Value::String("architecture_stable".into());
    project["modules"][0]["stage"] = Value::String("architecture_stable".into());
    project["modules"][0]
        .as_object_mut()
        .unwrap()
        .remove("regression");
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let goal_file = root.join(".appsdk/goal.json");
    let mut goal: Value = serde_json::from_slice(&fs::read(&goal_file).unwrap()).unwrap();
    goal["status"] = Value::String("confirmed".into());
    goal["confirmed_by"] = Value::String("test".into());
    goal["confirmed_at"] = Value::String("2026-01-01T00:00:00Z".into());
    fs::write(
        &goal_file,
        serde_json::to_string_pretty(&goal).unwrap() + "\n",
    )
    .unwrap();
    init_git(&root);
    let original_project = fs::read_to_string(&project_path).unwrap();

    let rejected = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "frozen",
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("REGRESSION_CONTRACT_REQUIRED:app-core")
    );
    assert_eq!(fs::read_to_string(&project_path).unwrap(), original_project);
    assert!(!root
        .join(".appsdk/transactions")
        .join("freeze-app-core")
        .exists());
    assert!(!root.join("protected/history/app-core").exists());
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
fn compile_rejects_control_drift_between_module_builds() {
    let root = temp_root("compile-control-drift");
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
    let mut edge = project["modules"][0].clone();
    edge["module_id"] = Value::String("app-edge".into());
    edge["source_owner"] = Value::String("app-edge".into());
    edge["owned_paths"] = serde_json::json!(["playground/edge/**"]);
    edge["active_artifact"] = Value::String("active/lib/app-edge/**".into());
    edge["generated_outputs"] = serde_json::json!(["generated/modules/app-edge/**"]);
    edge["build"]["args"] = serde_json::json!([
        "-c",
        "mkdir -p generated/modules/app-edge/lib && printf edge > generated/modules/app-edge/lib/edge.txt"
    ]);
    edge["artifact_paths"] = serde_json::json!(["edge.txt"]);
    project["modules"].as_array_mut().unwrap().push(edge);
    fs::create_dir_all(root.join("playground/edge")).unwrap();

    project["modules"][0]["build"]["args"] = serde_json::json!([
        "-c",
        "mkdir -p generated/modules/app-core/lib && printf core > generated/modules/app-core/lib/app-core.placeholder && printf drift > .appsdk/goal.json"
    ]);
    fs::write(&project_path, serde_json::to_vec_pretty(&project).unwrap()).unwrap();
    pin_test_lock(root_text);
    assert!(run(&["promote", root_text, "--to", "source_implemented"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());

    let compile = run(&["compile", root_text]);
    assert!(!compile.status.success());
    assert!(String::from_utf8_lossy(&compile.stderr).contains("COMPILE_CONTROL_INPUT_DRIFT"));
    assert!(!root
        .join("generated/modules/app-edge/module.compiled.json")
        .exists());

    fs::write(
        &goal_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "goal_id": "goal-1",
            "raw_request": "compile",
            "understood_objective": "compile modules",
            "acceptance_criteria": ["build"],
            "non_goals": [],
            "assumptions": [],
            "ambiguities": [],
            "questions": [],
            "status": "confirmed",
            "confirmed_by": "test",
            "confirmed_at": "2026-01-01T00:00:00Z",
            "created_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();
    let outside = root
        .parent()
        .unwrap()
        .join("appsdk-compile-control-drift-outside");
    let symlink_command = format!(
        "mkdir -p generated/modules/app-core/lib && printf core > generated/modules/app-core/lib/app-core.placeholder && rm -rf generated/modules/app-edge && ln -s '{}' generated/modules/app-edge",
        outside.display()
    );
    project = serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();
    project["modules"][0]["build"]["args"] = serde_json::json!(["-c", symlink_command]);
    fs::write(&project_path, serde_json::to_vec_pretty(&project).unwrap()).unwrap();

    let symlink_compile = run(&["compile", root_text]);
    assert!(!symlink_compile.status.success());
    assert!(
        String::from_utf8_lossy(&symlink_compile.stderr)
            .contains("GOVERNANCE_PATH_SYMLINK:module_generated_output"),
        "{}",
        String::from_utf8_lossy(&symlink_compile.stderr)
    );
    assert!(!outside.join("lib/edge.txt").exists());
    fs::remove_dir_all(root).unwrap();
    let _ = fs::remove_dir_all(outside);
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
  "sdk": {"name": "appsdk", "version": "0.1.0014", "bundle_manifest": ".appsdk/contracts/sdk-bundle.manifest.json", "resource_record": ".appsdk/sdk-resources.json"},
  "lifecycle": {"stage": "draft"},
  "development_scenarios": {"manifest": ".appsdk/contracts/development-scenarios.manifest.json", "enabled": []},
  "access": {"protected_paths":[".appsdk/**"]},
  "governance": {"playground_root":"playground/**","active_root":"active/**","protected_root":"protected/**","generated_root":"generated/**","active_kind":"immutable_consumable_library","protected_kinds":["source"],"generated_kinds":["compiler_output"],"freeze_requirements":["git_clean"],"promotion_requires":["evidence"],"runtime_forbidden_roots":["playground/**"],"record_contracts":["contracts/records/worktree-record.schema.json","contracts/records/reproduction-record.schema.json","contracts/records/evidence-record.schema.json","contracts/records/fix-candidate-record.schema.json","contracts/records/goal-clarification-record.schema.json","contracts/records/review-record.schema.json","contracts/records/effectiveness-record.schema.json","contracts/records/pre-review-validation-record.schema.json","contracts/records/collaboration-record.schema.json","contracts/records/collaboration-index.schema.json","contracts/records/merge-queue-record.schema.json","contracts/records/merge-queue-state.schema.json","contracts/records/integration-record.schema.json","contracts/records/mainline-receipt-record.schema.json","contracts/records/collab-live-closure-record.schema.json","contracts/records/merge-record.schema.json","contracts/records/promotion-record.schema.json","contracts/records/regression-report.schema.json","contracts/records/freeze-record.schema.json","contracts/records/record-graph.contract.json"],"zone_transition_contract":"contracts/transitions/zone-transition-manifest.json","playground_retention":"archive_then_remove","debug_merge_comment_required":true},
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
