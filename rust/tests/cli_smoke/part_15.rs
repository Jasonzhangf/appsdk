#[test]
fn guidance_event_ledger_quarantines_partial_tail_and_continues() {
    let root = temp_root("guidance-events-tail-recovery");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let plan_file = root.join("plan.json");
    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-tail",
        "module_id": "app-core",
        "objective": "quarantine partial event tail",
        "scope_paths": ["playground/experiments/input.txt"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::create_dir_all(root.join("playground/experiments")).unwrap();
    fs::write(root.join("playground/experiments/input.txt"), "v1\n").unwrap();
    fs::write(
        &plan_file,
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-tail",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    let events = root.join(".appsdk-control/guidance/task-tail/events.jsonl");
    let content = fs::read_to_string(&events).unwrap();
    fs::write(&events, format!("{}not-json", content)).unwrap();

    let status = run(&["guide", "next", root_text, "--task", "task-tail"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let payload: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["reason_code"], "NEXT_STEP_READY");
    assert!(fs::read_to_string(&events).unwrap().ends_with('\n'));
    assert!(fs::read_dir(events.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with("events.corrupt-tail.")));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_plan_rebuilds_from_journal_after_plan_cache_loss() {
    let root = temp_root("guidance-plan-cache-loss");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-cache-loss",
        "module_id": "app-core",
        "objective": "rebuild plan from journal after crash before plan cache",
        "scope_paths": ["playground/experiments/input.txt"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::create_dir_all(root.join("playground/experiments")).unwrap();
    fs::write(root.join("playground/experiments/input.txt"), "v1\n").unwrap();
    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-cache-loss",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    let plan_cache = root.join(".appsdk-control/guidance/task-cache-loss/plan.json");
    assert!(plan_cache.is_file());
    fs::remove_file(&plan_cache).unwrap();

    let status = run(&["guide", "next", root_text, "--task", "task-cache-loss"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let payload: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["reason_code"], "NEXT_STEP_READY");
    assert!(plan_cache.is_file());
    let rebuilt: Value = serde_json::from_str(&fs::read_to_string(&plan_cache).unwrap()).unwrap();
    assert_eq!(rebuilt["task_id"], "task-cache-loss");
    assert!(!rebuilt["plan_hash"].as_str().unwrap().is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_plan_quarantines_invalid_cache_and_rebuilds_from_journal() {
    let root = temp_root("guidance-plan-cache-invalid");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-cache-invalid",
        "module_id": "app-core",
        "objective": "quarantine invalid plan cache and rebuild from journal",
        "scope_paths": ["playground/experiments/input.txt"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::create_dir_all(root.join("playground/experiments")).unwrap();
    fs::write(root.join("playground/experiments/input.txt"), "v1\n").unwrap();
    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-cache-invalid",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    let control_dir = root.join(".appsdk-control/guidance/task-cache-invalid");
    fs::write(control_dir.join("plan.json"), "not-json\n").unwrap();

    let status = run(&["guide", "next", root_text, "--task", "task-cache-invalid"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let payload: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["reason_code"], "NEXT_STEP_READY");
    let plan_cache = control_dir.join("plan.json");
    assert!(plan_cache.is_file());
    let rebuilt: Value = serde_json::from_str(&fs::read_to_string(&plan_cache).unwrap()).unwrap();
    assert_eq!(rebuilt["task_id"], "task-cache-invalid");
    assert!(fs::read_dir(&control_dir)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with("plan.invalid.")));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_plan_rebuilds_when_cache_is_stale() {
    let root = temp_root("guidance-plan-cache-stale");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-cache-stale",
        "module_id": "app-core",
        "objective": "journal objective",
        "scope_paths": ["playground/experiments/input.txt"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::create_dir_all(root.join("playground/experiments")).unwrap();
    fs::write(root.join("playground/experiments/input.txt"), "v1\n").unwrap();
    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-cache-stale",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    let control_dir = root.join(".appsdk-control/guidance/task-cache-stale");
    let plan_cache = control_dir.join("plan.json");
    let mut stale =
        serde_json::from_str::<Value>(&fs::read_to_string(&plan_cache).unwrap()).unwrap();
    stale["objective"] = Value::String("stale objective".into());
    stale["plan_hash"] = Value::String("sha256:stale".into());
    fs::write(
        &plan_cache,
        serde_json::to_string_pretty(&stale).unwrap() + "\n",
    )
    .unwrap();

    let status = run(&["guide", "next", root_text, "--task", "task-cache-stale"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let payload: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["reason_code"], "NEXT_STEP_READY");
    let rebuilt: Value = serde_json::from_str(&fs::read_to_string(&plan_cache).unwrap()).unwrap();
    assert_eq!(rebuilt["objective"], "journal objective");
    assert!(fs::read_dir(&control_dir)
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with("plan.stale.")));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_scope_state_detects_content_change_without_git_status_shape_change() {
    let root = temp_root("guidance-scope-content-drift");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    let scope_dir = root.join("playground/experiments");
    fs::create_dir_all(&scope_dir).unwrap();
    fs::write(scope_dir.join("input.txt"), "v1\n").unwrap();
    init_git(&root);

    let plan_file = root.join("plan.json");
    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-content-drift",
        "module_id": "app-core",
        "objective": "detect same-path file content changes",
        "scope_paths": ["playground/experiments/input.txt"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::write(
        &plan_file,
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-content-drift",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    fs::write(scope_dir.join("input.txt"), "v2\n").unwrap();
    let status = run(&["guide", "next", root_text, "--task", "task-content-drift"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let payload: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["reason_code"], "GUIDANCE_CONTEXT_DRIFT:source");
    assert_eq!(payload["next"]["revision_reason"], "source_drift");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_plan_revision_requires_reason_and_preserves_history() {
    let root = temp_root("guidance-plan-revision");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let plan_file = root.join("plan.json");
    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-revision",
        "module_id": "app-core",
        "objective": "initial objective",
        "scope_paths": ["playground/experiments/**"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::write(
        &plan_file,
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-revision",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    let mut revised = proposal.clone();
    revised["objective"] = Value::String("revised objective".into());
    fs::write(
        &plan_file,
        serde_json::to_string_pretty(&revised).unwrap() + "\n",
    )
    .unwrap();
    let missing_reason = run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-revision",
        "--input",
        "plan.json",
    ]);
    assert!(!missing_reason.status.success());
    assert!(String::from_utf8_lossy(&missing_reason.stderr)
        .contains("GUIDANCE_PLAN_REVISION_REASON_REQUIRED"));

    revised["revision_reason"] = Value::String("new_evidence".into());
    fs::write(
        &plan_file,
        serde_json::to_string_pretty(&revised).unwrap() + "\n",
    )
    .unwrap();
    let accepted = run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-revision",
        "--input",
        "plan.json",
    ]);
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );

    let events =
        fs::read_to_string(root.join(".appsdk-control/guidance/task-revision/events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["record_type"], "PlanRecord");
    assert_eq!(events[1]["record_type"], "PlanRevisionRecord");
    assert_eq!(events[1]["reason"], "new_evidence");
    assert_eq!(events[2]["record_type"], "PlanRecord");
    assert_ne!(events[0]["plan_hash"], events[2]["plan_hash"]);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bug_command_lifecycle() {
    let setup_check = run(&["setup-deps", "--check"]);
    assert!(
        setup_check.status.success(),
        "setup-deps failed: {}",
        String::from_utf8_lossy(&setup_check.stderr)
    );
    assert!(String::from_utf8_lossy(&setup_check.stdout).contains("git_bug"));

    let root = temp_root("bug-lifecycle");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("README.md"), "# Bug Test\n").unwrap();
    init_git(&root);

    // Initial bug list should be empty
    let initial_list = run_bug_in(&root, &["bug", "list", "--json"]);
    assert!(
        initial_list.status.success(),
        "{}",
        String::from_utf8_lossy(&initial_list.stderr)
    );
    let list_json: Value = serde_json::from_slice(&initial_list.stdout).unwrap();
    assert_eq!(list_json.as_array().unwrap().len(), 0);

    // Create a new bug
    let created = run_bug_in(
        &root,
        &[
            "bug",
            "new",
            "-t",
            "Test Bug Lifecycle",
            "-m",
            "Testing bug lifecycle tracking",
            "-l",
            "test,p0",
        ],
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let create_json: Value = serde_json::from_slice(&created.stdout).unwrap();
    let bug_id = create_json["id"].as_str().unwrap();
    assert!(!bug_id.is_empty());

    // Filter by label
    let filtered_list = run_bug_in(&root, &["bug", "list", "--json", "-l", "p0"]);
    assert!(filtered_list.status.success());
    let filtered_json: Value = serde_json::from_slice(&filtered_list.stdout).unwrap();
    assert_eq!(filtered_json.as_array().unwrap().len(), 1);
    assert_eq!(filtered_json[0]["title"], "Test Bug Lifecycle");

    // Show bug details
    let show = run_bug_in(&root, &["bug", "show", bug_id, "--json"]);
    assert!(show.status.success());
    let show_json: Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(show_json["title"], "Test Bug Lifecycle");

    // Comment on bug
    let comment = run_bug_in(
        &root,
        &[
            "bug",
            "comment",
            bug_id,
            "-m",
            "Solution verified and implemented",
        ],
    );
    assert!(
        comment.status.success(),
        "{}",
        String::from_utf8_lossy(&comment.stderr)
    );

    // Close bug
    let close = run_bug_in(
        &root,
        &[
            "bug",
            "close",
            bug_id,
            "-m",
            "Solution verified and implemented",
        ],
    );
    assert!(
        close.status.success(),
        "{}",
        String::from_utf8_lossy(&close.stderr)
    );

    // Verify closed status in list
    let closed_list = run_bug_in(&root, &["bug", "list", "--status", "closed", "--json"]);
    assert!(closed_list.status.success());
    let closed_json: Value = serde_json::from_slice(&closed_list.stdout).unwrap();
    assert_eq!(closed_json.as_array().unwrap().len(), 1);
    assert_eq!(closed_json[0]["status"], "closed");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bug_intake_accepts_inline_json_input() {
    let root = temp_root("bug-intake-inline");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("README.md"), "# Inline Intake Test\n").unwrap();
    init_git(&root);

    let input = serde_json::json!({
        "execution_bound": true,
        "classification": "bug",
        "title": "Fix inline intake input",
        "original_input": "appsdk bug intake --input should accept inline JSON.",
        "scope": ["rust/src/main.rs", "rust/tests/cli_smoke.rs"],
        "owner": "appsdk::development_intake",
        "parent_id": "bug-inline-intake",
        "acceptance": ["inline JSON creates or deduplicates the governed bug"],
        "status": "received",
        "evidence_links": ["collab://m1790005209517-9"],
        "dedup_query": "Fix inline intake input"
    });
    let inline = serde_json::to_string(&input).unwrap();

    let created = run_bug_in(&root, &["bug", "intake", "--input", &inline]);
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let created_json: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(created_json["classification"], "bug");
    assert_eq!(created_json["created"], true);
    assert_eq!(created_json["deduplicated"], false);
    assert_eq!(
        created_json["bug_triage"]["query"],
        "git-bug bug \"Fix inline intake input\" -f json"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bug_intake_deduplicates_execution_work_and_rejects_read_only_conversation() {
    let root = temp_root("bug-intake");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("README.md"), "# Intake Test\n").unwrap();
    init_git(&root);

    let input = root.join("intake.json");
    fs::write(
        &input,
        serde_json::to_string_pretty(&serde_json::json!({
            "execution_bound": true,
            "classification": "feature",
            "title": "Add governed development intake",
            "original_input": "Implement governed intake before execution.",
            "scope": ["rust/src/main.rs", "rust/tests/cli_smoke.rs"],
            "owner": "appsdk::intake",
            "parent_id": "feature-3655c02",
            "acceptance": ["deduplicate before create", "bind lifecycle evidence"],
            "status": "received",
            "evidence_links": ["feature://3655c02"],
            "dedup_query": "governed development intake"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();

    let created = run_bug_in(
        &root,
        &["bug", "intake", "--input", input.to_str().unwrap()],
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let created_json: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(created_json["classification"], "feature");
    assert_eq!(created_json["deduplicated"], false);
    assert_eq!(created_json["created"], true);
    assert_eq!(created_json["governed_completion_requires_issue_id"], true);
    let issue_id = created_json["issue_id"].as_str().unwrap();
    assert!(!issue_id.is_empty());
    assert_eq!(created_json["bug_triage"]["query_executed"], true);
    assert_eq!(created_json["bug_triage"]["mode"], "new_confirmed");
    assert_eq!(
        created_json["bug_triage"]["query"],
        "git-bug bug \"governed development intake\" -f json"
    );
    let created_query_result = &created_json["bug_triage"]["query_result"];
    assert_eq!(
        created_query_result["query_result_hash"],
        digest(&canonical(&created_query_result["records"]))
    );
    assert_eq!(created_query_result["records"], serde_json::json!([]));
    assert_eq!(created_json["bug_triage"]["matched_issue_id"], Value::Null);
    assert_eq!(
        created_json["bug_triage"]["reopened_from_issue_id"],
        Value::Null
    );
    assert_eq!(
        created_json["bug_triage_query_binding"],
        digest(&canonical(&serde_json::json!({
            "issue_id": issue_id,
            "query": "git-bug bug \"governed development intake\" -f json",
            "mode": "new_confirmed",
            "reopened_from_issue_id": null,
            "matched_issue_id": null,
            "matched_title": null,
            "matched_classification": null,
            "query_result": {
                "records": [],
                "query_result_hash": digest(&canonical(&serde_json::json!([])))
            }
        })))
    );

    let shown = run_bug_in(&root, &["bug", "show", issue_id, "--json"]);
    assert!(shown.status.success());
    let shown_json: Value = serde_json::from_slice(&shown.stdout).unwrap();
    let body = shown_json["comments"][0]["message"].as_str().unwrap();
    assert!(body.contains("\"parent_id\": \"feature-3655c02\""));
    assert!(body.contains("\"evidence_links\": ["));
    assert!(body.contains("### Original Input\nImplement governed intake before execution."));
    assert!(body.contains("Classification: feature"));
    assert!(body.contains("Scope:\n- rust/src/main.rs\n- rust/tests/cli_smoke.rs"));
    assert!(body.contains("Owner: appsdk::intake"));
    assert!(body.contains("Parent: feature-3655c02"));
    assert!(body.contains("Acceptance:\n- deduplicate before create\n- bind lifecycle evidence"));
    assert!(body.contains("Status: received"));
    assert!(body.contains("Evidence links:\n- feature://3655c02"));

    let fake_bin = root.with_extension("readback-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_git_bug = fake_bin.join("git-bug");
    fs::write(
        &fake_git_bug,
        r#"#!/bin/sh
case "$1 $2" in
  "user -f")
    printf '%s\n' '[{"id":"test-user"}]'
    exit 0
    ;;
  "bug new")
    printf '%s\n' 'deadbeefdeadbeef'
    exit 0
    ;;
  "bug label")
    exit 0
    ;;
  "bug show")
    printf '%s\n' '{"human_id":"deadbeefdeadbeef","status":"open","title":"different title","labels":["classification:feature"]}'
    exit 0
    ;;
  *)
    if [ "$1" = "bug" ]; then
      printf '%s\n' '[]'
      exit 0
    fi
    exit 64
    ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_git_bug, fs::Permissions::from_mode(0o755)).unwrap();
    let forged_readback = Command::new(binary())
        .args(["bug", "intake", "--input", input.to_str().unwrap()])
        .current_dir(&root)
        .env("APPSDK_ROOT", &root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
        .env("GIT_BUG_BIN", &fake_git_bug)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!forged_readback.status.success());
    assert!(forged_readback.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&forged_readback.stderr)
            .contains("BUG_INTAKE_CREATE_TITLE_MISMATCH"),
        "{}",
        String::from_utf8_lossy(&forged_readback.stderr)
    );

    let reused = run_bug_in(
        &root,
        &["bug", "intake", "--input", input.to_str().unwrap()],
    );
    assert!(
        reused.status.success(),
        "{}",
        String::from_utf8_lossy(&reused.stderr)
    );
    let reused_json: Value = serde_json::from_slice(&reused.stdout).unwrap();
    assert_eq!(reused_json["issue_id"], issue_id);
    assert_eq!(reused_json["deduplicated"], true);
    assert_eq!(reused_json["created"], false);
    assert_eq!(reused_json["appended"], false);
    assert_eq!(reused_json["bug_triage"]["mode"], "reused");
    assert_eq!(reused_json["bug_triage"]["matched_issue_id"], issue_id);
    assert_eq!(
        reused_json["bug_triage"]["matched_title"],
        "Add governed development intake"
    );
    assert_eq!(
        reused_json["bug_triage"]["matched_classification"],
        "feature"
    );
    let reused_query_result = &reused_json["bug_triage"]["query_result"];
    assert_eq!(
        reused_query_result["query_result_hash"],
        digest(&canonical(&reused_query_result["records"]))
    );
    assert_eq!(
        reused_query_result["records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|record| record["human_id"] == issue_id)
            .count(),
        1
    );
    assert_eq!(
        reused_json["bug_triage"]["reopened_from_issue_id"],
        Value::Null
    );
    assert_eq!(
        reused_json["bug_triage_query_binding"],
        digest(&canonical(&serde_json::json!({
            "issue_id": issue_id,
            "query": "git-bug bug \"governed development intake\" -f json",
            "mode": "reused",
            "reopened_from_issue_id": null,
            "matched_issue_id": issue_id,
            "matched_title": "Add governed development intake",
            "matched_classification": "feature",
            "query_result": reused_query_result
        })))
    );

    let shown_after_reuse = run_bug_in(&root, &["bug", "show", issue_id, "--json"]);
    assert!(shown_after_reuse.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&shown_after_reuse.stdout).unwrap()["comments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let list = run_bug_in(&root, &["bug", "list", "--json"]);
    assert!(list.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&list.stdout)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let closed = run_bug_in(
        &root,
        &[
            "bug",
            "close",
            issue_id,
            "-m",
            "Temporary closure for intake reopen coverage",
        ],
    );
    assert!(closed.status.success());
    let reopened = run_bug_in(
        &root,
        &["bug", "intake", "--input", input.to_str().unwrap()],
    );
    assert!(
        reopened.status.success(),
        "{}",
        String::from_utf8_lossy(&reopened.stderr)
    );
    let reopened_json: Value = serde_json::from_slice(&reopened.stdout).unwrap();
    assert_eq!(reopened_json["issue_id"], issue_id);
    assert_eq!(reopened_json["deduplicated"], true);
    assert_eq!(reopened_json["reopened"], true);
    assert_eq!(reopened_json["appended"], false);
    assert_eq!(reopened_json["bug_triage"]["mode"], "reopened_same_record");
    assert_eq!(reopened_json["bug_triage"]["matched_issue_id"], issue_id);
    assert_eq!(
        reopened_json["bug_triage"]["matched_title"],
        "Add governed development intake"
    );
    assert_eq!(
        reopened_json["bug_triage"]["matched_classification"],
        "feature"
    );
    assert_eq!(
        reopened_json["bug_triage"]["reopened_from_issue_id"],
        issue_id
    );
    assert_eq!(
        reopened_json["bug_triage_query_binding"],
        digest(&canonical(&serde_json::json!({
            "issue_id": issue_id,
            "query": "git-bug bug \"governed development intake\" -f json",
            "mode": "reopened_same_record",
            "reopened_from_issue_id": issue_id,
            "matched_issue_id": issue_id,
            "matched_title": "Add governed development intake",
            "matched_classification": "feature",
            "query_result": reopened_json["bug_triage"]["query_result"]
        })))
    );
    let reopened_record = run_bug_in(&root, &["bug", "show", issue_id, "--json"]);
    assert!(reopened_record.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&reopened_record.stdout).unwrap()["status"],
        "open"
    );

    let invalid_classification = root.join("invalid-classification.json");
    let mut invalid: Value = serde_json::from_slice(&fs::read(&input).unwrap()).unwrap();
    invalid["classification"] = Value::String("question".into());
    fs::write(
        &invalid_classification,
        serde_json::to_string_pretty(&invalid).unwrap() + "\n",
    )
    .unwrap();
    let invalid_result = run_bug_in(
        &root,
        &[
            "bug",
            "intake",
            "--input",
            invalid_classification.to_str().unwrap(),
        ],
    );
    assert!(!invalid_result.status.success());
    assert!(String::from_utf8_lossy(&invalid_result.stderr)
        .contains("BUG_INTAKE_CLASSIFICATION_INVALID"));

    let read_only = root.join("read-only.json");
    fs::write(
        &read_only,
        serde_json::to_string_pretty(&serde_json::json!({
            "execution_bound": false,
            "classification": "feature",
            "title": "Explain current behavior",
            "original_input": "How does intake work?",
            "scope": [],
            "owner": "appsdk::intake",
            "parent_id": null,
            "acceptance": [],
            "status": "received",
            "evidence_links": [],
            "dedup_query": "explain current behavior"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let rejected = run_bug_in(
        &root,
        &["bug", "intake", "--input", read_only.to_str().unwrap()],
    );
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("DEVELOPMENT_INTAKE_READ_ONLY_CONVERSATION"));

    let after = run_bug_in(&root, &["bug", "list", "--json"]);
    assert!(after.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&after.stdout)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let legacy_title = "Legacy unlabeled intake issue";
    let legacy_created = run_bug_in(
        &root,
        &[
            "bug",
            "new",
            "-t",
            legacy_title,
            "-m",
            "Created before classification labels.",
        ],
    );
    assert!(legacy_created.status.success());
    let legacy_id = serde_json::from_slice::<Value>(&legacy_created.stdout).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let legacy_input = root.join("legacy-intake.json");
    let mut legacy: Value = serde_json::from_slice(&fs::read(&input).unwrap()).unwrap();
    legacy["classification"] = Value::String("bug".into());
    legacy["title"] = Value::String(legacy_title.into());
    legacy["original_input"] = Value::String("Reuse the existing legacy issue.".into());
    legacy["dedup_query"] = Value::String("Legacy unlabeled intake issue".into());
    fs::write(
        &legacy_input,
        serde_json::to_string_pretty(&legacy).unwrap() + "\n",
    )
    .unwrap();
    let migrated = run_bug_in(
        &root,
        &["bug", "intake", "--input", legacy_input.to_str().unwrap()],
    );
    assert!(
        migrated.status.success(),
        "{}",
        String::from_utf8_lossy(&migrated.stderr)
    );
    let migrated_json: Value = serde_json::from_slice(&migrated.stdout).unwrap();
    assert_eq!(migrated_json["issue_id"], legacy_id);
    assert_eq!(migrated_json["deduplicated"], true);
    assert_eq!(migrated_json["created"], false);
    assert_eq!(migrated_json["bug_triage"]["mode"], "reused");
    assert_eq!(migrated_json["bug_triage"]["matched_issue_id"], legacy_id);
    assert_eq!(migrated_json["bug_triage"]["matched_title"], legacy_title);
    assert_eq!(migrated_json["bug_triage"]["matched_classification"], "bug");
    let migrated_record = run_bug_in(&root, &["bug", "show", legacy_id.as_str(), "--json"]);
    assert!(migrated_record.status.success());
    let migrated_record_json: Value = serde_json::from_slice(&migrated_record.stdout).unwrap();
    assert!(migrated_record_json["labels"]
        .as_array()
        .unwrap()
        .iter()
        .any(|label| label == "classification:bug"));
    assert_eq!(
        migrated_record_json["comments"].as_array().unwrap().len(),
        2
    );
    let migrated_again = run_bug_in(
        &root,
        &["bug", "intake", "--input", legacy_input.to_str().unwrap()],
    );
    assert!(migrated_again.status.success());
    let migrated_record_again = run_bug_in(&root, &["bug", "show", legacy_id.as_str(), "--json"]);
    assert!(migrated_record_again.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&migrated_record_again.stdout).unwrap()["comments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let conflict_title = "Classification conflict remains isolated";
    let conflicting = run_bug_in(
        &root,
        &[
            "bug",
            "new",
            "-t",
            conflict_title,
            "-m",
            "Existing bug classification.",
            "-l",
            "classification:bug",
        ],
    );
    assert!(conflicting.status.success());
    let conflicting_id = serde_json::from_slice::<Value>(&conflicting.stdout).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let conflict_input = root.join("classification-conflict.json");
    let mut conflict: Value = serde_json::from_slice(&fs::read(&input).unwrap()).unwrap();
    conflict["title"] = Value::String(conflict_title.into());
    conflict["dedup_query"] = Value::String("Classification conflict remains isolated".into());
    fs::write(
        &conflict_input,
        serde_json::to_string_pretty(&conflict).unwrap() + "\n",
    )
    .unwrap();
    let isolated = run_bug_in(
        &root,
        &["bug", "intake", "--input", conflict_input.to_str().unwrap()],
    );
    assert!(isolated.status.success());
    let isolated_json: Value = serde_json::from_slice(&isolated.stdout).unwrap();
    assert_eq!(isolated_json["created"], true);
    assert_ne!(isolated_json["issue_id"], conflicting_id);

    let function_map: Value =
        serde_json::from_str(include_str!("../../../contracts/maps/function-map.json")).unwrap();
    let mainline_map: Value =
        serde_json::from_str(include_str!("../../../contracts/maps/mainline-call-map.json")).unwrap();
    let symbols = function_map["functions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|function| function["function_id"] == "development_intake")
        .unwrap()["entry_symbols"]
        .as_array()
        .unwrap();
    let chain = mainline_map["chains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|chain| chain["chain_id"] == "development-intake-v1")
        .unwrap();
    assert_eq!(chain["entry_symbol"], "bug_intake");
    assert_eq!(chain["terminal_symbol"], "bug_intake_triage");
    for edge in mainline_map["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|edge| edge["chain_id"] == "development-intake-v1")
    {
        for field in ["caller", "callee"] {
            let symbol = edge[field].as_str().unwrap();
            assert!(
                symbols.iter().any(|candidate| candidate == symbol),
                "unresolved development-intake symbol: {symbol}"
            );
            let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
            let mut sources = String::new();
            let mut stack = vec![src_root.clone()];
            while let Some(dir) = stack.pop() {
                for entry in std::fs::read_dir(&dir).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                        sources.push_str(&std::fs::read_to_string(&path).unwrap());
                    }
                }
            }
            assert!(
                sources.contains(&format!("fn {symbol}(")),
                "development-intake symbol missing from rust/src: {symbol}"
            );
        }
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bug_command_upstream_fallback() {
    let setup_check = run(&["setup-deps", "--check"]);
    assert!(setup_check.status.success());

    let upstream_root = temp_root("bug-upstream");
    fs::create_dir_all(&upstream_root).unwrap();
    fs::write(upstream_root.join("README.md"), "# Upstream SDK Repo\n").unwrap();
    init_git(&upstream_root);

    // Create a bug in upstream repo
    let created = Command::new(binary())
        .args(&[
            "bug",
            "new",
            "-t",
            "Upstream Daemon Issue",
            "-m",
            "Daemon lock issue in upstream",
            "-l",
            "upstream,p1",
        ])
        .current_dir(&upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let create_json: Value = serde_json::from_slice(&created.stdout).unwrap();
    let bug_id = create_json["id"].as_str().unwrap();

    let client_root = temp_root("bug-client");
    fs::create_dir_all(&client_root).unwrap();
    fs::write(client_root.join("README.md"), "# Client Project\n").unwrap();
    init_git(&client_root);

    // 1. In client root, upstream reads require an explicit store selector.
    let show = Command::new(binary())
        .args(&["bug", "show", bug_id, "--json", "--upstream"])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let show_json: Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(show_json["title"], "Upstream Daemon Issue");

    // A missing local issue must not be read from the configured upstream
    // store without an explicit selector.
    let implicit_show = Command::new(binary())
        .args(&["bug", "show", bug_id, "--json"])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!implicit_show.status.success());
    assert!(String::from_utf8_lossy(&implicit_show.stderr).contains("GIT_BUG_SHOW_FAILED"));

    // 2. The explicit selector also applies to filtered list reads.
    let list_q = Command::new(binary())
        .args(&["bug", "list", "-q", "Daemon", "--json", "--upstream"])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        list_q.status.success(),
        "{}",
        String::from_utf8_lossy(&list_q.stderr)
    );
    let list_json: Value = serde_json::from_slice(&list_q.stdout).unwrap();
    assert_eq!(list_json.as_array().unwrap().len(), 1);
    assert_eq!(list_json[0]["title"], "Upstream Daemon Issue");

    let implicit_list = Command::new(binary())
        .args(&["bug", "list", "-q", "Daemon", "--json"])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(implicit_list.status.success());
    let implicit_list_json: Value = serde_json::from_slice(&implicit_list.stdout).unwrap();
    assert!(implicit_list_json.as_array().unwrap().is_empty());

    // 3. A comment without --upstream remains local and must not target upstream.
    let local_comment = Command::new(binary())
        .args(&[
            "bug",
            "comment",
            bug_id,
            "-m",
            "Verified in client environment",
        ])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!local_comment.status.success());
    assert!(String::from_utf8_lossy(&local_comment.stderr).contains("GIT_BUG_COMMENT_FAILED"));

    let comment = Command::new(binary())
        .args(&[
            "bug",
            "comment",
            bug_id,
            "-m",
            "Verified in client environment",
            "--upstream",
        ])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        comment.status.success(),
        "{}",
        String::from_utf8_lossy(&comment.stderr)
    );
    let upstream_after_comment = Command::new(binary())
        .args(&["bug", "show", bug_id, "--json", "--upstream"])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    let upstream_json: Value = serde_json::from_slice(&upstream_after_comment.stdout).unwrap();
    assert!(upstream_json["comments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|comment| { comment["message"] == "Verified in client environment" }));

    // 4. Close requires a solution and keeps explicit repository targeting.
    let close = Command::new(binary())
        .args(&[
            "bug",
            "close",
            bug_id,
            "-m",
            "Fixed and verified",
            "--upstream",
        ])
        .current_dir(&client_root)
        .env("APPSDK_ROOT", &upstream_root)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        close.status.success(),
        "{}",
        String::from_utf8_lossy(&close.stderr)
    );

    let _ = fs::remove_dir_all(upstream_root);
    let _ = fs::remove_dir_all(client_root);
}

#[test]
fn bug_reads_external_repo_requires_explicit_upstream_under_local_lock() {
    let home = temp_root("bug-resolution-home");
    let upstream_root = home.join("Documents/github/appsdk");
    fs::create_dir_all(&upstream_root).unwrap();
    fs::write(upstream_root.join("README.md"), "# Upstream SDK Repo\n").unwrap();
    init_git(&upstream_root);

    let create_upstream = Command::new(binary())
        .args([
            "bug",
            "new",
            "-t",
            "Resolved Upstream Issue",
            "-m",
            "Upstream issue selected without APPSDK_ROOT",
        ])
        .current_dir(&upstream_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        create_upstream.status.success(),
        "{}",
        String::from_utf8_lossy(&create_upstream.stderr)
    );
    let upstream_bug: Value = serde_json::from_slice(&create_upstream.stdout).unwrap();
    let bug_id = upstream_bug["id"].as_str().unwrap().to_string();

    let client_root = temp_root("bug-resolution-client");
    fs::create_dir_all(&client_root).unwrap();
    fs::write(client_root.join("README.md"), "# Client Repo\n").unwrap();
    init_git(&client_root);
    let create_local = Command::new(binary())
        .args(["bug", "new", "-t", "Local Issue", "-m", "must not be read"])
        .current_dir(&client_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(create_local.status.success());

    // The default writer and a successful local read share the client store,
    // even when an upstream store is discoverable through HOME.
    let local_list = Command::new(binary())
        .args(["bug", "list", "--json"])
        .current_dir(&client_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        local_list.status.success(),
        "{}",
        String::from_utf8_lossy(&local_list.stderr)
    );
    let local_list_json: Value = serde_json::from_slice(&local_list.stdout).unwrap();
    assert!(local_list_json
        .as_array()
        .unwrap()
        .iter()
        .any(|bug| bug["title"] == "Local Issue"));

    let lock_path = client_root.join(".git/git-bug/lock");
    let held = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .unwrap();
    hold_advisory_lock(&held);

    let list_child = Command::new(binary())
        .args(["bug", "list", "--json"])
        .current_dir(&client_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let show_child = Command::new(binary())
        .args(["bug", "show", &bug_id, "--json"])
        .current_dir(&client_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let list = list_child.wait_with_output().unwrap();
    assert!(!list.status.success());
    assert!(String::from_utf8_lossy(&list.stderr).contains("GIT_BUG_LIST_FAILED"));

    let show = show_child.wait_with_output().unwrap();
    assert!(!show.status.success());
    assert!(String::from_utf8_lossy(&show.stderr).contains("GIT_BUG_SHOW_FAILED"));

    // A locked local store must not turn a default comment into an upstream
    // mutation; upstream writes remain explicit.
    let local_comment = Command::new(binary())
        .args(["bug", "comment", &bug_id, "-m", "external client comment"])
        .current_dir(&client_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!local_comment.status.success());
    assert!(String::from_utf8_lossy(&local_comment.stderr).contains("GIT_BUG_COMMENT_FAILED"));

    let comment = Command::new(binary())
        .args([
            "bug",
            "comment",
            &bug_id,
            "-m",
            "external client comment",
            "--upstream",
        ])
        .current_dir(&client_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        comment.status.success(),
        "{}",
        String::from_utf8_lossy(&comment.stderr)
    );
    drop(held);

    let upstream_after_comment = Command::new(binary())
        .args(["bug", "show", &bug_id, "--json", "--upstream"])
        .current_dir(&client_root)
        .env("HOME", &home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    let upstream_json: Value = serde_json::from_slice(&upstream_after_comment.stdout).unwrap();
    assert!(upstream_json["comments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|comment| { comment["message"] == "external client comment" }));

    let missing_home = temp_root("bug-resolution-missing-home");
    let missing_client = temp_root("bug-resolution-missing-client");
    fs::create_dir_all(&missing_client).unwrap();
    fs::write(
        missing_client.join("README.md"),
        "# Missing Upstream Client\n",
    )
    .unwrap();
    init_git(&missing_client);
    let missing = Command::new(binary())
        .args(["bug", "list", "--json", "--upstream"])
        .current_dir(&missing_client)
        .env("HOME", &missing_home)
        .env_remove("APPSDK_ROOT")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("APPSDK_UPSTREAM_REPO_NOT_FOUND"));

    fs::remove_dir_all(home).unwrap();
    fs::remove_dir_all(client_root).unwrap();
    let _ = fs::remove_dir_all(missing_home);
    fs::remove_dir_all(missing_client).unwrap();
}

#[test]
fn bug_close_comment_failure_is_explicit() {
    let root = temp_root("bug-close-comment-failure");
    fs::create_dir_all(&root).unwrap();
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_git_bug = fake_bin.join("git-bug");
    fs::write(
        &fake_git_bug,
        r#"#!/bin/sh
case "$1 $2 $3" in
  "user -f json")
    printf '%s\n' '[{"id":"user-1"}]'
    exit 0
    ;;
  "bug comment new")
    echo "solution comment write failed" >&2
    exit 2
    ;;
  "bug status close")
    echo "should not close"
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
    let path = format!("{}:/usr/bin:/bin", fake_bin.display());

    let res = Command::new(binary())
        .args(["bug", "close", "abc123", "-m", "Solution verified"])
        .current_dir(&root)
        .env("PATH", &path)
        .env("HOME", &home)
        .env_remove("GIT_BUG_BIN")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!res.status.success());
    let stderr = String::from_utf8_lossy(&res.stderr);
    assert!(stderr.contains("GIT_BUG_COMMENT_FAILED"), "{}", stderr);
    assert!(!stderr.contains("should not close"));

    fs::remove_dir_all(root).unwrap();
}
