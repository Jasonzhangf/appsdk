#[test]
fn lifecycle_record_producer_binds_clean_worktree_and_baseline() {
    let root = temp_root("lifecycle-record-producer");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/goal.json"),
        r#"{"goal_id":"goal-1","issue_id":"goal-issue-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#,
    )
    .unwrap();
    init_git(&root);
    let map_path = root.join(".appsdk/maps/resource-map.json");
    let mut extended_map: Value =
        serde_json::from_str(&fs::read_to_string(&map_path).unwrap()).unwrap();
    extended_map["resources"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "resource_id":"producer-test-extension",
            "owner":"test::extension",
            "truth_store":"test",
            "allowed_operations":["read"]
        }));
    fs::write(
        &map_path,
        serde_json::to_string_pretty(&extended_map).unwrap() + "\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", ".appsdk/maps/resource-map.json"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "map extension"])
        .status()
        .unwrap()
        .success());
    assert!(run(&["promote", root_text, "--to", "source_implemented"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    assert!(run(&["compile-module", root_text, "--module", "app-core"])
        .status
        .success());
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
    assert!(Command::new("git")
        .args(["-C", root_text, "add", ".appsdk/project.json"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "candidate"])
        .status()
        .unwrap()
        .success());
    let commit = git_test_value(&root, &["rev-parse", "HEAD"]);
    let artifact: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json")).unwrap(),
    )
    .unwrap();
    let scope_hash = digest(&canonical(&serde_json::json!({
        "module_id":"app-core",
        "source_hash":artifact["source_hash"],
        "contract_hash":artifact["contract_hash"],
        "registry_binding":{"mode":"exact"}
    })));
    let input_path = root.with_extension("producer-input.json");
    let fake_bin = root.with_extension("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_git_bug = fake_bin.join("git-bug");
    fs::write(
        &fake_git_bug,
        r#"#!/bin/sh
case "$1 $2" in
  "bug show")
    printf '{"human_id":"%s","status":"open","title":"test issue"}\n' "$3"
    exit 0
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_git_bug, fs::Permissions::from_mode(0o755)).unwrap();
    let produce_with = |input: &Path, git_bug: &Path| {
        Command::new(binary())
            .args([
                "produce-lifecycle-records",
                root_text,
                "--module",
                "app-core",
                "--input",
                input.to_str().unwrap(),
            ])
            .env("GIT_BUG_BIN", git_bug)
            .env_remove("TMUX_PANE")
            .output()
            .unwrap()
    };
    let produce = |input: &Path| produce_with(input, &fake_git_bug);
    let producer_triage_binding = digest(&canonical(&serde_json::json!({
        "issue_id": "issue-producer-1",
        "query": "appsdk bug list -q issue-producer-1",
        "mode": "new_confirmed",
        "reopened_from_issue_id": null
    })));
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "goal_id":"goal-1",
            "worktree": {
                "worktree_id":"caller-worktree-id","issue_id":"issue-producer-1","goal_issue_id":"goal-issue-1","module_id":"app-core",
                "base_ref":"HEAD","base_commit":commit,"branch":"codex/test","head_commit":commit,
                "initial_clean":true,"final_clean":true,"isolation_mode":"isolated_worktree",
                "scope_hash":scope_hash,"created_at":"2026-01-01T00:00:00Z",
                "bug_triage":{"query_executed":true,"query":"appsdk bug list -q issue-producer-1","mode":"new_confirmed","reopened_from_issue_id":null},
                "bug_triage_query_binding":producer_triage_binding
            },
            "reproduction": {
                "reproduction_id":"caller-reproduction-id","issue_id":"issue-producer-1","module_id":"app-core",
                "worktree_id":"caller-worktree-id","base_commit":commit,"input_hashes":["caller-input"],
                "baseline_evidence_id":"caller-baseline-id","first_divergence":"caller text",
                "result":"reproduced","created_at":"2026-01-01T00:01:00Z"
            },
            "baseline_evidence": {
                "evidence_id":"caller-evidence-id","issue_id":"issue-producer-1","experiment_id":"experiment-producer-1",
                "phase":"baseline_reproduction","kind":"red_test","source_commit":commit,"scope":{"module_id":"app-core"},
                "producer":{"adapter":"test","identity":"lifecycle-producer-test"},"result":"pass",
                "command":{"program":"sh","args":["-c","printf baseline-error-token >&2; exit 1"],"working_directory":".","expected_exit_status":1,"expected_error_token":"baseline-error-token"},
                "created_at":"2026-01-01T00:00:30Z","expires_at":"2099-01-01T00:00:00Z",
                "input_hashes":["input-producer-1"],"scope_hash":scope_hash
            }
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let mut valid_input: Value =
        serde_json::from_str(&fs::read_to_string(&input_path).unwrap()).unwrap();

    let set_goal_issue = |issue_id: Option<Value>| -> String {
        let goal_path = root.join(".appsdk/goal.json");
        let mut goal: Value =
            serde_json::from_str(&fs::read_to_string(&goal_path).unwrap()).unwrap();
        match issue_id {
            Some(issue_id) => goal["issue_id"] = issue_id,
            None => {
                goal.as_object_mut().unwrap().remove("issue_id");
            }
        }
        fs::write(
            &goal_path,
            serde_json::to_string_pretty(&goal).unwrap() + "\n",
        )
        .unwrap();
        assert!(Command::new("git")
            .args(["-C", root_text, "add", ".appsdk/goal.json"])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["-C", root_text, "commit", "-m", "goal issue variant"])
            .status()
            .unwrap()
            .success());
        git_test_value(&root, &["rev-parse", "HEAD"])
    };
    let set_input_commit = |input: &mut Value, commit: &str| {
        input["worktree"]["base_commit"] = Value::String(commit.to_string());
        input["worktree"]["head_commit"] = Value::String(commit.to_string());
        input["reproduction"]["base_commit"] = Value::String(commit.to_string());
        input["baseline_evidence"]["source_commit"] = Value::String(commit.to_string());
    };

    for issue_id in [Some(Value::Null), None, Some(Value::from(7))] {
        let goal_commit = set_goal_issue(issue_id);
        let mut invalid_goal_input = valid_input.clone();
        invalid_goal_input["worktree"]
            .as_object_mut()
            .unwrap()
            .remove("goal_issue_id");
        set_input_commit(&mut invalid_goal_input, &goal_commit);
        fs::write(
            &input_path,
            serde_json::to_string_pretty(&invalid_goal_input).unwrap() + "\n",
        )
        .unwrap();
        let invalid_goal_result = produce(&input_path);
        assert!(!invalid_goal_result.status.success());
        assert!(String::from_utf8_lossy(&invalid_goal_result.stderr)
            .contains("PRODUCER_GOAL_ISSUE_MISMATCH"));
    }

    let same_issue_commit = set_goal_issue(Some(Value::String("issue-producer-1".into())));
    let mut same_issue_input = valid_input.clone();
    same_issue_input["worktree"]
        .as_object_mut()
        .unwrap()
        .remove("goal_issue_id");
    set_input_commit(&mut same_issue_input, &same_issue_commit);
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&same_issue_input).unwrap() + "\n",
    )
    .unwrap();
    let same_issue_result = produce(&input_path);
    assert!(
        same_issue_result.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&same_issue_result.stdout),
        String::from_utf8_lossy(&same_issue_result.stderr)
    );
    let same_issue_worktree: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/worktree-record-app-core.json")).unwrap(),
    )
    .unwrap();
    assert!(same_issue_worktree.get("goal_issue_id").is_none());
    assert!(same_issue_worktree.get("goal_issue_binding").is_none());
    fs::remove_file(root.join(".appsdk/records/worktree-record-app-core.json")).unwrap();
    fs::remove_file(root.join(".appsdk/records/reproduction-record-app-core.json")).unwrap();
    fs::remove_dir_all(root.join(".appsdk/records/evidence/app-core")).unwrap();

    let restored_goal_commit = set_goal_issue(Some(Value::String("goal-issue-1".into())));
    set_input_commit(&mut valid_input, &restored_goal_commit);
    let mut missing_goal_issue = valid_input.clone();
    missing_goal_issue["worktree"]
        .as_object_mut()
        .unwrap()
        .remove("goal_issue_id");
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&missing_goal_issue).unwrap() + "\n",
    )
    .unwrap();
    let missing_goal_issue_result = produce(&input_path);
    assert!(!missing_goal_issue_result.status.success());
    assert!(String::from_utf8_lossy(&missing_goal_issue_result.stderr)
        .contains("PRODUCER_GOAL_ISSUE_MISMATCH"));

    let mut forged_goal_issue = valid_input.clone();
    forged_goal_issue["worktree"]["goal_issue_id"] = Value::String("forged-goal-issue".into());
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&forged_goal_issue).unwrap() + "\n",
    )
    .unwrap();
    let forged_goal_issue_result = produce(&input_path);
    assert!(!forged_goal_issue_result.status.success());
    assert!(String::from_utf8_lossy(&forged_goal_issue_result.stderr)
        .contains("PRODUCER_GOAL_ISSUE_MISMATCH"));

    let mut invalid_triage = valid_input.clone();
    invalid_triage["worktree"]["bug_triage"]["mode"] = Value::String("bogus".into());
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&invalid_triage).unwrap() + "\n",
    )
    .unwrap();
    let invalid_triage_result = produce(&input_path);
    assert!(!invalid_triage_result.status.success());
    assert!(
        String::from_utf8_lossy(&invalid_triage_result.stderr).contains("BUG_TRIAGE_MODE_INVALID"),
        "{}",
        String::from_utf8_lossy(&invalid_triage_result.stderr)
    );
    assert!(!root
        .join(".appsdk/records/worktree-record-app-core.json")
        .exists());
    let mut non_boolean_query_flag = valid_input.clone();
    non_boolean_query_flag["worktree"]["bug_triage"]["query_executed"] =
        Value::String("true".into());
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&non_boolean_query_flag).unwrap() + "\n",
    )
    .unwrap();
    let non_boolean_query_result = produce(&input_path);
    assert!(!non_boolean_query_result.status.success());
    assert!(String::from_utf8_lossy(&non_boolean_query_result.stderr)
        .contains("BUG_TRIAGE_QUERY_MISSING"));

    let mut invalid_reopened_source_type = valid_input.clone();
    invalid_reopened_source_type["worktree"]["bug_triage"]["reopened_from_issue_id"] =
        Value::Bool(false);
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&invalid_reopened_source_type).unwrap() + "\n",
    )
    .unwrap();
    let invalid_reopened_source_type_result = produce(&input_path);
    assert!(!invalid_reopened_source_type_result.status.success());
    assert!(
        String::from_utf8_lossy(&invalid_reopened_source_type_result.stderr)
            .contains("BUG_TRIAGE_REOPENED_SOURCE_INVALID")
    );

    let mut forged_query = valid_input.clone();
    forged_query["worktree"]["bug_triage"]["query"] =
        Value::String("appsdk bug list -q prefixissue-producer-1suffix".into());
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&forged_query).unwrap() + "\n",
    )
    .unwrap();
    let forged_query_result = produce(&input_path);
    assert!(!forged_query_result.status.success());
    assert!(
        String::from_utf8_lossy(&forged_query_result.stderr).contains("BUG_TRIAGE_QUERY_UNBOUND")
    );

    let mismatched_bin = root.with_extension("mismatched-bin");
    fs::create_dir_all(&mismatched_bin).unwrap();
    let mismatched_git_bug = mismatched_bin.join("git-bug");
    fs::write(
        &mismatched_git_bug,
        r#"#!/bin/sh
case "$1 $2" in
  "bug show")
    printf '%s\n' '{"human_id":"different-issue","status":"open"}'
    exit 0
    ;;
  *)
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&mismatched_git_bug, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&valid_input).unwrap() + "\n",
    )
    .unwrap();
    let mismatched_result = produce_with(&input_path, &mismatched_git_bug);
    assert!(!mismatched_result.status.success());
    assert!(String::from_utf8_lossy(&mismatched_result.stderr)
        .contains("BUG_TRIAGE_QUERY_IDENTITY_MISMATCH"));

    fs::write(
        &input_path,
        serde_json::to_string_pretty(&valid_input).unwrap() + "\n",
    )
    .unwrap();
    let produced = produce(&input_path);
    assert!(
        produced.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&produced.stdout),
        String::from_utf8_lossy(&produced.stderr)
    );
    assert!(root
        .join(".appsdk/records/worktree-record-app-core.json")
        .is_file());
    assert!(root
        .join(".appsdk/records/reproduction-record-app-core.json")
        .is_file());
    let evidence_files = fs::read_dir(root.join(".appsdk/records/evidence/app-core"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(evidence_files.len(), 1);
    assert!(evidence_files[0].is_file());
    let produced_worktree: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/worktree-record-app-core.json")).unwrap(),
    )
    .unwrap();
    let produced_reproduction: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/reproduction-record-app-core.json"))
            .unwrap(),
    )
    .unwrap();
    let produced_evidence: Value =
        serde_json::from_str(&fs::read_to_string(&evidence_files[0]).unwrap()).unwrap();
    assert_ne!(produced_worktree["worktree_id"], "caller-worktree-id");
    assert_ne!(
        produced_reproduction["reproduction_id"],
        "caller-reproduction-id"
    );
    assert_eq!(
        produced_reproduction["input_hashes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        produced_evidence["producer"],
        serde_json::json!({"adapter":"appsdk","identity":"appsdk-lifecycle-record-producer"})
    );
    let expected_triage_binding = digest(&canonical(&serde_json::json!({
        "issue_id": "issue-producer-1",
        "query": "appsdk bug list -q issue-producer-1",
        "mode": "new_confirmed",
        "reopened_from_issue_id": null
    })));
    assert_eq!(
        produced_worktree["bug_triage_query_binding"],
        expected_triage_binding
    );
    assert_eq!(produced_worktree["goal_issue_id"], "goal-issue-1");
    assert_eq!(
        produced_worktree["goal_issue_binding"],
        digest(&canonical(&serde_json::json!({
            "goal_issue_id": "goal-issue-1",
            "issue_id": "issue-producer-1"
        })))
    );
    assert_eq!(produced_evidence["exit_status"], 1);
    let repeated = produce(&input_path);
    assert!(
        repeated.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&repeated.stdout),
        String::from_utf8_lossy(&repeated.stderr)
    );
    let repeated_json: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(repeated_json["reused"], true);

    // A cached producer result is valid only when all three records remain
    // present.  Removing any one member of the set must fail closed instead
    // of rerunning the baseline command or treating a partial set as a hit.
    let worktree_path = root.join(".appsdk/records/worktree-record-app-core.json");
    let reproduction_path = root.join(".appsdk/records/reproduction-record-app-core.json");
    let evidence_path = evidence_files[0].clone();
    for path in [&worktree_path, &reproduction_path, &evidence_path] {
        let saved = fs::read(path).unwrap();
        fs::remove_file(path).unwrap();
        let partial = produce(&input_path);
        assert!(!partial.status.success(), "missing {:?} must fail", path);
        assert!(
            String::from_utf8_lossy(&partial.stderr).contains("PRODUCER_RECORD_SET_INCOMPLETE"),
            "path={:?} stderr={}",
            path,
            String::from_utf8_lossy(&partial.stderr)
        );
        fs::write(path, saved).unwrap();
    }
    let restored = produce(&input_path);
    assert!(restored.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&restored.stdout).unwrap()["reused"],
        true
    );

    let worktree_bytes = fs::read(&worktree_path).unwrap();
    let mut forged_goal_binding: Value = serde_json::from_slice(&worktree_bytes).unwrap();
    forged_goal_binding["goal_issue_binding"] = Value::String("sha256:forged".into());
    fs::write(
        &worktree_path,
        serde_json::to_string_pretty(&forged_goal_binding).unwrap() + "\n",
    )
    .unwrap();
    let forged_goal_binding_result = produce(&input_path);
    assert!(!forged_goal_binding_result.status.success());
    assert!(String::from_utf8_lossy(&forged_goal_binding_result.stderr)
        .contains("PRODUCER_REUSE_WORKTREE_MISMATCH"));
    fs::write(&worktree_path, worktree_bytes).unwrap();

    // A new candidate identity must re-run the baseline, preserve the prior
    // three-record set in producer history, and replace only the current
    // projections. The next invocation of that same identity is a cache hit.
    fs::write(root.join("candidate-source.txt"), "candidate\n").unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "candidate-source.txt"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "candidate identity"])
        .status()
        .unwrap()
        .success());
    let next_commit = git_test_value(&root, &["rev-parse", "HEAD"]);
    let mut next_input = valid_input.clone();
    next_input["worktree"]["base_ref"] = Value::String("HEAD".into());
    next_input["worktree"]["base_commit"] = Value::String(next_commit.clone());
    next_input["worktree"]["head_commit"] = Value::String(next_commit.clone());
    next_input["reproduction"]["base_commit"] = Value::String(next_commit);
    next_input["baseline_evidence"]["source_commit"] =
        next_input["worktree"]["base_commit"].clone();
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&next_input).unwrap() + "\n",
    )
    .unwrap();
    let reentered = produce(&input_path);
    assert!(
        reentered.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&reentered.stdout),
        String::from_utf8_lossy(&reentered.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&reentered.stdout).unwrap()["reused"],
        Value::Null
    );
    let producer_attempts = root.join(".appsdk/records/attempts/app-core/producer-records.jsonl");
    let producer_attempt: Value =
        serde_json::from_str(fs::read_to_string(&producer_attempts).unwrap().trim()).unwrap();
    assert_eq!(producer_attempt["result"], "stale");
    assert_eq!(producer_attempt["records"].as_array().unwrap().len(), 3);
    for entry in producer_attempt["records"].as_array().unwrap() {
        let record_json = entry["record_json"].as_str().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(record_json).unwrap(),
            entry["record"]
        );
    }
    let reentered_again = produce(&input_path);
    assert!(reentered_again.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&reentered_again.stdout).unwrap()["reused"],
        true
    );
    assert_eq!(
        fs::read_to_string(&producer_attempts)
            .unwrap()
            .lines()
            .count(),
        1
    );
    let valid_attempts = fs::read_to_string(&producer_attempts).unwrap();
    let valid_attempt: Value = serde_json::from_str(valid_attempts.trim()).unwrap();
    for (field, replacement) in [
        (
            "record_hash",
            Value::String(format!("sha256:{}", "a".repeat(64))),
        ),
        (
            "archive_id",
            Value::String("producer-attempt-forged".into()),
        ),
    ] {
        let mut tampered = valid_attempt.clone();
        tampered[field] = replacement;
        fs::write(
            &producer_attempts,
            serde_json::to_string(&tampered).unwrap() + "\n",
        )
        .unwrap();
        let rejected = produce(&input_path);
        assert!(!rejected.status.success(), "tampered {field} must fail");
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains("PRODUCER_RECORD_ARCHIVE_INVALID"),
            "field={field} stderr={}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        fs::write(&producer_attempts, &valid_attempts).unwrap();
    }
    let ledger_recovered = produce(&input_path);
    assert!(ledger_recovered.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&ledger_recovered.stdout).unwrap()["reused"],
        true
    );
    fs::remove_file(input_path).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_record_producer_runs_nested_project_baseline_in_project_directory() {
    let git_root = temp_root("lifecycle-record-producer-nested-git-root");
    let project_root = git_root.join("packages/app");
    fs::create_dir_all(&project_root).unwrap();
    let project_text = project_root.to_str().unwrap();
    assert!(run(&["new", project_text]).status.success());
    fs::write(project_root.join("nested-baseline-marker"), "nested\n").unwrap();
    fs::write(
        project_root.join(".appsdk/goal.json"),
        r#"{"goal_id":"goal-1","issue_id":null,"raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#,
    )
    .unwrap();
    init_git(&git_root);
    assert!(
        run(&["promote", project_text, "--to", "source_implemented"])
            .status
            .success()
    );
    assert!(run(&["promote", project_text, "--to", "contract_bound"])
        .status
        .success());
    assert!(
        run(&["compile-module", project_text, "--module", "app-core"])
            .status
            .success()
    );
    assert!(run(&[
        "promote-module",
        project_text,
        "--module",
        "app-core",
        "--to",
        "contract_bound",
    ])
    .status
    .success());
    assert!(Command::new("git")
        .args(["-C", git_root.to_str().unwrap(), "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            git_root.to_str().unwrap(),
            "commit",
            "-m",
            "nested candidate"
        ])
        .status()
        .unwrap()
        .success());
    let commit = git_test_value(&project_root, &["rev-parse", "HEAD"]);
    let artifact: Value = serde_json::from_str(
        &fs::read_to_string(project_root.join("generated/modules/app-core/module.compiled.json"))
            .unwrap(),
    )
    .unwrap();
    let scope_hash = digest(&canonical(&serde_json::json!({
        "module_id":"app-core",
        "source_hash":artifact["source_hash"],
        "contract_hash":artifact["contract_hash"],
        "registry_binding":{"mode":"exact"}
    })));
    let command = serde_json::json!({
        "program":"sh",
        "args":["-c","test -f nested-baseline-marker && printf nested-baseline >&2; exit 1"],
        "working_directory":".",
        "expected_exit_status":1,
        "expected_error_token":"nested-baseline"
    });
    let input_hashes = vec![digest(&canonical(&command))];
    let current_root = project_root.canonicalize().unwrap();
    let worktree_id = format!(
        "worktree-{}",
        digest(&canonical(&serde_json::json!({
            "root": current_root,
            "module_id":"app-core",
            "issue_id":"none",
            "base_commit":commit,
            "head_commit":commit,
            "branch":"codex/test",
            "scope_hash":scope_hash
        })))
        .strip_prefix("sha256:")
        .unwrap()
    );
    let reproduction_id = format!(
        "reproduction-{}",
        digest(&canonical(&serde_json::json!({
            "worktree_id":worktree_id,
            "input_hashes":input_hashes,
            "error_token":"nested-baseline"
        })))
        .strip_prefix("sha256:")
        .unwrap()
    );
    let baseline_id = format!(
        "baseline-{}",
        digest(&canonical(&serde_json::json!({
            "reproduction_id":reproduction_id,
            "source_commit":commit,
            "input_hashes":input_hashes,
            "command":command
        })))
        .strip_prefix("sha256:")
        .unwrap()
    );
    let input = serde_json::json!({
        "goal_id":"goal-1",
        "worktree": {
            "worktree_id":worktree_id,"issue_id":"none","module_id":"app-core",
            "base_ref":"HEAD","base_commit":commit,"branch":"codex/test","head_commit":commit,
            "initial_clean":true,"final_clean":true,"isolation_mode":"isolated_worktree",
            "scope_hash":scope_hash,"created_at":"2026-01-01T00:00:00Z"
        },
        "reproduction": {
            "reproduction_id":reproduction_id,"issue_id":"none","module_id":"app-core",
            "worktree_id":worktree_id,"base_commit":commit,"input_hashes":input_hashes,
            "baseline_evidence_id":baseline_id,"first_divergence":"baseline","result":"reproduced",
            "created_at":"2026-01-01T00:00:00Z"
        },
        "baseline_evidence": {
            "evidence_id":baseline_id,"issue_id":"none","experiment_id":"experiment",
            "phase":"baseline_reproduction","kind":"red_test","source_commit":commit,
            "scope":{"module_id":"app-core"},"producer":{"adapter":"appsdk","identity":"appsdk-lifecycle-record-producer"},
            "result":"pass","created_at":"2026-01-01T00:00:00Z","expires_at":"2099-01-01T00:00:00Z",
            "input_hashes":input_hashes,"scope_hash":scope_hash,"command":command,"exit_status":1,
            "output_hash":digest("stdout=\nstderr=nested-baseline")
        }
    });
    let input_path = git_root.with_extension("nested-producer-input.json");
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&input).unwrap() + "\n",
    )
    .unwrap();
    let produced = run(&[
        "produce-lifecycle-records",
        project_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(
        produced.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&produced.stdout),
        String::from_utf8_lossy(&produced.stderr)
    );
    assert!(project_root
        .join(".appsdk/records/evidence/app-core")
        .is_dir());
    fs::remove_file(input_path).unwrap();
    fs::remove_dir_all(git_root).unwrap();
}

#[test]
fn worktree_schema_requires_triage_only_for_non_legacy_issue_ids() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../contracts/records/worktree-record.schema.json"
    ))
    .unwrap();
    let required = schema["required"].as_array().unwrap();
    let properties = schema["properties"].as_object().unwrap();
    let goal_issue_id = properties
        .get("goal_issue_id")
        .expect("missing goal_issue_id property");
    assert_eq!(goal_issue_id["type"], "string");
    let goal_issue_binding = properties
        .get("goal_issue_binding")
        .expect("missing goal_issue_binding property");
    assert_eq!(goal_issue_binding["type"], "string");
    assert_eq!(goal_issue_binding["pattern"], "^sha256:[a-f0-9]{64}$");
    assert!(!required.iter().any(|value| value == "goal_issue_id"));
    assert!(!required.iter().any(|value| value == "goal_issue_binding"));
    assert!(!required.iter().any(|value| value == "bug_triage"));
    assert!(!required
        .iter()
        .any(|value| value == "bug_triage_query_binding"));
    let conditional = schema["allOf"].as_array().unwrap().iter().find(|rule| {
        rule.get("if")
            .and_then(|condition| condition.get("properties"))
            .and_then(|properties| properties.get("issue_id"))
            .and_then(|issue_id| issue_id.get("not"))
            .and_then(|not| not.get("pattern"))
            .and_then(Value::as_str)
            .is_some_and(|pattern| pattern == "^(?:none$|legacy-)")
    });
    let conditional = conditional.expect("missing non-legacy issue conditional");
    let then_required = conditional["then"]["required"].as_array().unwrap();
    assert!(then_required.iter().any(|value| value == "bug_triage"));
    assert!(then_required
        .iter()
        .any(|value| value == "bug_triage_query_binding"));
    let triage = &properties["bug_triage"]["properties"];
    let modes = triage["mode"]["enum"].as_array().unwrap();
    for mode in ["new_confirmed", "reused", "reopened_same_record"] {
        assert!(
            modes.iter().any(|value| value == mode),
            "missing canonical triage mode: {mode}"
        );
    }
    assert_eq!(
        triage["matched_issue_id"]["type"],
        serde_json::json!(["string", "null"])
    );
    let triage_required = triage_required(&properties["bug_triage"]);
    assert!(triage_required.iter().any(|value| *value == "query_result"));
}

fn triage_required(triage: &Value) -> Vec<&str> {
    let required = triage["required"]
        .as_array()
        .expect("bug_triage required must be an array");
    required
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("bug_triage requirement must be a string")
        })
        .collect()
}

#[test]
fn promotion_schema_requires_authoritative_bug_closure() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../contracts/records/promotion-record.schema.json"
    ))
    .unwrap();
    assert!(schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "bug_closure_verified"));
    assert_eq!(
        schema["properties"]["bug_closure_verified"]["type"],
        "boolean"
    );
    let collaboration_requires_closure = schema["allOf"].as_array().unwrap().iter().any(|rule| {
        rule.pointer("/if/required/0").and_then(Value::as_str) == Some("collaboration_record_id")
            && rule.pointer("/then/required/0").and_then(Value::as_str)
                == Some("collab_live_closure_record_id")
    });
    assert!(
        collaboration_requires_closure,
        "collaboration promotion must require collab_live_closure_record_id"
    );
}

#[test]
fn lifecycle_record_producer_rejects_dirty_or_mismatched_input_without_records() {
    let root = temp_root("lifecycle-record-producer-reject");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/goal.json"),
        r#"{"goal_id":"goal-1","issue_id":null,"raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#,
    )
    .unwrap();
    init_git(&root);
    let commit = git_test_value(&root, &["rev-parse", "HEAD"]);
    fs::write(root.join("dirty.txt"), "dirty\n").unwrap();
    let input_path = root.with_extension("producer-input.json");
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&serde_json::json!({
            "goal_id":"wrong-goal",
            "worktree": {"worktree_id":"worktree-1","issue_id":"issue-1","module_id":"app-core","base_ref":"HEAD","base_commit":commit,"branch":"codex/test","head_commit":commit,"initial_clean":true,"final_clean":true,"isolation_mode":"isolated_worktree","scope_hash":"scope-1","created_at":"2026-01-01T00:00:00Z"},
            "reproduction": {"reproduction_id":"reproduction-1","issue_id":"issue-1","module_id":"app-core","worktree_id":"worktree-1","base_commit":commit,"input_hashes":["input-1"],"baseline_evidence_id":"baseline-1","first_divergence":"test","result":"reproduced","created_at":"2026-01-01T00:01:00Z"},
            "baseline_evidence": {"evidence_id":"baseline-1","issue_id":"issue-1","experiment_id":"experiment-1","phase":"baseline_reproduction","kind":"red_test","source_commit":commit,"scope":{"module_id":"app-core"},"producer":{"adapter":"test","identity":"test"},"command":{"program":"sh","args":["-c","exit 1"],"working_directory":".","expected_exit_status":1},"input_hashes":["input-1"],"scope_hash":"scope-1"}
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("PRODUCER_GOAL_MISMATCH"));
    assert!(!root
        .join(".appsdk/records/worktree-record-app-core.json")
        .exists());
    fs::remove_file(input_path).unwrap();
    fs::remove_file(root.join("dirty.txt")).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn initialized_lock_is_not_bound_to_the_running_binary() {
    let root = temp_root("unbound-sdk-lock");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/goal.json"),
        r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#,
    )
    .unwrap();
    for stage in ["source_implemented", "contract_bound"] {
        assert!(run(&["promote", root_text, "--to", stage]).status.success());
    }

    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["digest"] = Value::String(format!("sha256:{}", "a".repeat(64)));
    lock["compiler_digest"] = Value::String(format!("sha256:{}", "b".repeat(64)));
    lock["binary_ref"] = Value::String("historical-binary-witness".into());
    fs::write(
        &lock_path,
        serde_json::to_string_pretty(&lock).unwrap() + "\n",
    )
    .unwrap();

    let compile = run(&["compile", root_text]);
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn initialized_lock_rejects_wrong_version_schema_and_malformed_bundle_digest() {
    for (name, mutate, expected) in [
        ("wrong-version", "version", "INVALID_SDK_LOCK"),
        ("wrong-schema", "contract_schema", "INVALID_SDK_LOCK"),
        (
            "malformed-bundle",
            "bundle_digest",
            "INVALID_SDK_BUNDLE_DIGEST",
        ),
    ] {
        let root = temp_root(name);
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        let lock_path = root.join(".appsdk/sdk.lock");
        let mut lock: Value =
            serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
        match mutate {
            "version" => lock["version"] = Value::String("0.1.5".into()),
            "contract_schema" => lock["contract_schema"] = Value::from(2),
            "bundle_digest" => lock["bundle_digest"] = Value::String("sha256:not-a-digest".into()),
            _ => unreachable!(),
        }
        fs::write(
            &lock_path,
            serde_json::to_string_pretty(&lock).unwrap() + "\n",
        )
        .unwrap();
        let verified = run(&["verify", root_text]);
        assert!(!verified.status.success());
        assert!(
            String::from_utf8_lossy(&verified.stderr).contains(expected),
            "stderr={}",
            String::from_utf8_lossy(&verified.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }
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
fn compile_module_option_does_not_build_unrelated_modules() {
    let root = temp_root("module-scoped-compile");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let goal_file = root.join(".appsdk/goal.json");
    let mut goal: Value = serde_json::from_str(&fs::read_to_string(&goal_file).unwrap()).unwrap();
    goal["status"] = serde_json::json!("confirmed");
    goal["confirmed_by"] = serde_json::json!("test");
    goal["confirmed_at"] = serde_json::json!("2026-01-01T00:00:00Z");
    fs::write(&goal_file, serde_json::to_string_pretty(&goal).unwrap()).unwrap();

    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    let mut android_probe = project["modules"][0].clone();
    android_probe["module_id"] = Value::from("android-probe");
    android_probe["source_owner"] = Value::from("android-probe");
    android_probe["owned_paths"] = serde_json::json!(["playground/android-probe/**"]);
    android_probe["active_artifact"] = Value::from("active/lib/android-probe/**");
    android_probe["build"]["args"] =
        serde_json::json!(["-c", "touch android-probe-was-built && exit 42"]);
    android_probe["artifact_paths"] = serde_json::json!(["android-probe.placeholder"]);

    project["modules"][0]["module_id"] = Value::from("client-connection");
    project["modules"][0]["source_owner"] = Value::from("client-connection");
    project["modules"][0]["active_artifact"] = Value::from("active/lib/client-connection/**");
    project["modules"][0]["build"]["args"] = serde_json::json!([
        "-c",
        "mkdir -p generated/modules/client-connection/lib && printf client > generated/modules/client-connection/lib/client.placeholder"
    ]);
    project["modules"][0]["artifact_paths"] = serde_json::json!(["client.placeholder"]);
    project["modules"]
        .as_array_mut()
        .unwrap()
        .push(android_probe);
    fs::create_dir_all(root.join("playground/android-probe")).unwrap();
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap(),
    )
    .unwrap();

    let compiled = run(&["compile", root_text, "--module", "client-connection"]);
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    assert!(root
        .join("generated/modules/client-connection/module.compiled.json")
        .is_file());
    let review = run(&[
        "verify",
        "--review-admission",
        root_text,
        "--module",
        "client-connection",
    ]);
    assert!(!review.status.success());
    assert!(
        String::from_utf8_lossy(&review.stderr).contains("REVIEW_ADMISSION_BLOCKED"),
        "{}",
        String::from_utf8_lossy(&review.stderr)
    );
    assert!(!root.join("android-probe-was-built").exists());
    assert!(!root
        .join("generated/modules/android-probe/module.compiled.json")
        .exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn review_preflight_honors_empty_deployment_operations() {
    for (name, operations, requires_deployment) in [
        ("explicit-empty", Some(serde_json::json!([])), false),
        ("legacy-omitted", None, true),
    ] {
        let root = temp_root(&format!("review-preflight-{name}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());

        let project_file = root.join(".appsdk/project.json");
        let mut project: Value =
            serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
        if let Some(operations) = operations {
            project["modules"][0]["deployment_operations"] = operations;
        }
        fs::write(
            &project_file,
            serde_json::to_string_pretty(&project).unwrap(),
        )
        .unwrap();

        let goal_file = root.join(".appsdk/goal.json");
        let mut goal: Value =
            serde_json::from_str(&fs::read_to_string(&goal_file).unwrap()).unwrap();
        goal["status"] = serde_json::json!("confirmed");
        goal["confirmed_by"] = serde_json::json!("test");
        goal["confirmed_at"] = serde_json::json!("2026-01-01T00:00:00Z");
        fs::write(&goal_file, serde_json::to_string_pretty(&goal).unwrap()).unwrap();

        assert!(run(&["compile", root_text, "--module", "app-core"])
            .status
            .success());
        let admission = run(&[
            "verify",
            "--review-admission",
            root_text,
            "--module",
            "app-core",
        ]);
        assert!(!admission.status.success());
        let stderr = String::from_utf8_lossy(&admission.stderr);
        assert!(stderr.contains("REVIEW_ADMISSION_BLOCKED"), "{stderr}");
        assert_eq!(
            stderr.contains("\"kind\": \"deployment_install\""),
            requires_deployment,
            "{stderr}"
        );
        assert_eq!(
            stderr.contains("\"kind\": \"deployment_restart\""),
            requires_deployment,
            "{stderr}"
        );
        fs::remove_dir_all(root).unwrap();
    }
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
            "issue-1",
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
            "issue-1",
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
