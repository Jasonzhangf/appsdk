use super::*;

pub(super) fn produce_lifecycle_records(root: &Path, module_id: &str, input_path: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    assert_declared_contracts(root, &project);
    assert_lifecycle_producer_map_binding(root, &project, module_id, LifecycleProducer::Records);
    assert_goal_confirmed(root);
    let goal = read_goal(root);
    let input_file = producer_input_path(root, input_path);
    let input: Value = serde_json::from_str(
        &fs::read_to_string(input_file).unwrap_or_else(|_| fail("PRODUCER_INPUT_READ_FAILED")),
    )
    .unwrap_or_else(|_| fail("INVALID_PRODUCER_INPUT"));
    if !input.is_object() {
        fail("INVALID_PRODUCER_INPUT");
    }
    // Validate again after taking the lock so a concurrent map edit cannot be
    // accepted between the read-only preflight and the record transaction.
    let _producer_lock = producer_lock(root);
    assert_declared_contracts(root, &project);
    assert_lifecycle_producer_map_binding(root, &project, module_id, LifecycleProducer::Records);
    if producer_string(&input, "/goal_id", "PRODUCER_GOAL_MISSING")
        != producer_string(&goal, "/goal_id", "INVALID_GOAL_CLARIFICATION_RECORD")
    {
        fail("PRODUCER_GOAL_MISMATCH");
    }
    let module = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    let stage = producer_string(module, "/stage", "INVALID_MODULE_CONTRACT");
    if stage == "frozen" || stage == "retired" {
        fail(format!("PRODUCER_MODULE_STAGE_FORBIDDEN:{}", stage));
    }
    let input_hash = sha256(&canonical(&input));
    // A durable marker is structurally checked during preflight so malformed
    // transactions fail with their own diagnostic; record recovery itself is
    // still delayed until all current-candidate gates have passed below.
    let transaction_replace_current =
        producer_record_transaction_validate_marker(root, module_id, &input_hash);
    let records_root = root.join(".appsdk").join("records");
    let worktree = input
        .get("worktree")
        .unwrap_or_else(|| fail("PRODUCER_WORKTREE_MISSING"));
    let reproduction = input
        .get("reproduction")
        .unwrap_or_else(|| fail("PRODUCER_REPRODUCTION_MISSING"));
    let baseline = input
        .get("baseline_evidence")
        .unwrap_or_else(|| fail("PRODUCER_BASELINE_EVIDENCE_MISSING"));
    // These paths are module-stable, so reject a repeated producer call before
    // any later clean-worktree gate can mask the idempotent result.
    let expected_scope_hash = producer_scope_hash(root, &project, module, module_id);
    for path in [
        "/worktree_id",
        "/module_id",
        "/base_ref",
        "/base_commit",
        "/branch",
        "/head_commit",
        "/scope_hash",
    ] {
        producer_string(worktree, path, "INVALID_WORKTREE_RECORD");
    }
    producer_bool(worktree, "/initial_clean", "WORKTREE_INITIAL_NOT_CLEAN");
    producer_bool(worktree, "/final_clean", "WORKTREE_FINAL_NOT_CLEAN");
    if producer_string(worktree, "/module_id", "INVALID_WORKTREE_RECORD") != module_id
        || producer_string(worktree, "/isolation_mode", "INVALID_WORKTREE_RECORD")
            != "isolated_worktree"
    {
        fail("PRODUCER_MODULE_MISMATCH");
    }
    if producer_string(worktree, "/scope_hash", "INVALID_WORKTREE_RECORD") != expected_scope_hash {
        fail(format!(
            "PRODUCER_SCOPE_MISMATCH:expected={}",
            expected_scope_hash
        ));
    }
    let worktree_issue = producer_issue(worktree, "/issue_id", "INVALID_WORKTREE_RECORD");
    let goal_issue_binding =
        producer_goal_issue_binding_for_input(&goal, worktree, &worktree_issue);
    if !worktree_issue.is_empty()
        && worktree_issue != "none"
        && !worktree_issue.starts_with("legacy-")
        && worktree.get("bug_triage").is_none()
    {
        fail("BUG_TRIAGE_MISSING");
    }
    assert_bug_tracker_triage_evidence(worktree, &worktree_issue, Some(root), true);
    if producer_string(worktree, "/base_commit", "INVALID_WORKTREE_RECORD")
        == producer_string(worktree, "/head_commit", "INVALID_WORKTREE_RECORD")
    {
        // A baseline-only candidate is valid for SDK smoke tests and for an
        // adapter that has not committed source changes yet; the later
        // FixCandidate gate still binds the actual candidate commit.
    }
    let current_branch = git_value(
        root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        "PRODUCER_BRANCH_UNAVAILABLE",
    );
    if matches!(current_branch.as_str(), "main" | "master" | "v4-cordis") {
        fail("PRODUCER_PROTECTED_BRANCH");
    }
    if current_branch != producer_string(worktree, "/branch", "INVALID_WORKTREE_RECORD") {
        fail("PRODUCER_BRANCH_MISMATCH");
    }
    let worktree_list = git_value(
        root,
        &["worktree", "list", "--porcelain"],
        "PRODUCER_WORKTREE_UNAVAILABLE",
    );
    let current_root = root
        .canonicalize()
        .unwrap_or_else(|_| fail("PRODUCER_WORKTREE_UNAVAILABLE"));
    let git_worktree_root = worktree_list
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .filter_map(|path| Path::new(path).canonicalize().ok())
        .filter(|path| current_root == *path || current_root.starts_with(path))
        .max_by_key(|path| path.components().count())
        .unwrap_or_else(|| fail("PRODUCER_WORKTREE_NOT_REGISTERED"));
    let project_relative_path = current_root
        .strip_prefix(&git_worktree_root)
        .unwrap_or_else(|_| fail("PRODUCER_WORKTREE_NOT_REGISTERED"))
        .to_path_buf();
    let base_commit = producer_string(worktree, "/base_commit", "INVALID_WORKTREE_RECORD");
    let head_commit = producer_string(worktree, "/head_commit", "INVALID_WORKTREE_RECORD");
    if git_value(
        root,
        &["rev-parse", &base_commit],
        "PRODUCER_BASE_COMMIT_INVALID",
    ) != base_commit
        || git_value(
            root,
            &["rev-parse", &head_commit],
            "PRODUCER_HEAD_COMMIT_INVALID",
        ) != head_commit
    {
        fail("PRODUCER_COMMIT_INVALID");
    }
    if git_value(root, &["rev-parse", "HEAD"], "PRODUCER_HEAD_COMMIT_INVALID") != head_commit {
        fail("PRODUCER_HEAD_COMMIT_MISMATCH");
    }
    let base_ref = producer_string(worktree, "/base_ref", "INVALID_WORKTREE_RECORD");
    if git_value(root, &["rev-parse", &base_ref], "PRODUCER_BASE_REF_INVALID") != base_commit {
        fail("PRODUCER_BASE_REF_MISMATCH");
    }
    let ancestry = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "merge-base",
            "--is-ancestor",
        ])
        .args([base_commit.as_str(), head_commit.as_str()])
        .status()
        .unwrap_or_else(|_| fail("PRODUCER_VCS_UNAVAILABLE"));
    if !ancestry.success() {
        fail("PRODUCER_BASE_NOT_ANCESTOR");
    }
    for path in [
        "/reproduction_id",
        "/issue_id",
        "/module_id",
        "/base_commit",
    ] {
        producer_string(reproduction, path, "INVALID_REPRODUCTION_RECORD");
    }
    if producer_string(reproduction, "/module_id", "INVALID_REPRODUCTION_RECORD") != module_id
        || producer_issue(reproduction, "/issue_id", "INVALID_REPRODUCTION_RECORD")
            != worktree_issue
        || producer_string(reproduction, "/base_commit", "INVALID_REPRODUCTION_RECORD")
            != base_commit
    {
        fail("PRODUCER_REPRODUCTION_MISMATCH");
    }
    for path in [
        "/issue_id",
        "/experiment_id",
        "/phase",
        "/kind",
        "/source_commit",
        "/scope_hash",
        "/scope/module_id",
        "/producer/adapter",
        "/producer/identity",
    ] {
        producer_string(baseline, path, "INVALID_BASELINE_EVIDENCE");
    }
    if producer_issue(baseline, "/issue_id", "INVALID_BASELINE_EVIDENCE") != worktree_issue
        || producer_string(baseline, "/scope/module_id", "INVALID_BASELINE_EVIDENCE") != module_id
        || producer_string(baseline, "/scope_hash", "INVALID_BASELINE_EVIDENCE")
            != producer_string(worktree, "/scope_hash", "INVALID_WORKTREE_RECORD")
        || producer_string(baseline, "/source_commit", "INVALID_BASELINE_EVIDENCE") != base_commit
        || producer_string(baseline, "/phase", "INVALID_BASELINE_EVIDENCE")
            != "baseline_reproduction"
        || !matches!(
            producer_string(baseline, "/kind", "INVALID_BASELINE_EVIDENCE").as_str(),
            "red_test" | "sample_replay" | "gate" | "runtime"
        )
    {
        fail("PRODUCER_BASELINE_MISMATCH");
    }
    let (program, args, working_directory, expected_status, expected_error_token) =
        producer_command(baseline);
    let command_declaration = serde_json::json!({
        "program": program,
        "args": args,
        "working_directory": working_directory,
        "expected_exit_status": expected_status,
        "expected_error_token": expected_error_token
    });
    let input_hashes = vec![sha256(&canonical(&command_declaration))];
    let current_root = root
        .canonicalize()
        .unwrap_or_else(|_| fail("PRODUCER_WORKTREE_UNAVAILABLE"));
    let worktree_id = producer_stable_id(
        "worktree",
        &serde_json::json!({
            "root": current_root,
            "module_id": module_id,
            "issue_id": worktree_issue,
            "base_commit": base_commit,
            "head_commit": head_commit,
            "branch": current_branch,
            "scope_hash": expected_scope_hash
        }),
    );
    let reproduction_id = producer_stable_id(
        "reproduction",
        &serde_json::json!({
            "worktree_id": worktree_id,
            "input_hashes": input_hashes,
            "error_token": expected_error_token
        }),
    );
    let baseline_id = producer_stable_id(
        "baseline",
        &serde_json::json!({
            "reproduction_id": reproduction_id,
            "source_commit": base_commit,
            "input_hashes": input_hashes,
            "command": command_declaration
        }),
    );
    let targets = producer_record_targets(root, module_id, &input, &baseline_id);
    let status = git_value(
        root,
        &["status", "--porcelain", "--untracked-files=all"],
        "PRODUCER_VCS_UNAVAILABLE",
    );
    if !status.is_empty() {
        let allowed_targets = targets
            .iter()
            .filter_map(|(target, _)| target.strip_prefix(root).ok())
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>();
        let transaction = producer_transaction_dir(root, module_id);
        let marker_path = transaction.join("marker.json");
        let transaction_targets = fs::read_to_string(marker_path)
            .ok()
            .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
            .and_then(|marker| marker.get("records").cloned())
            .and_then(|records| records.as_array().cloned())
            .map(|records| {
                records
                    .iter()
                    .filter_map(|entry| entry.get("target").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            });
        let status_only = status.lines().all(|line| {
            line.get(3..).map(str::trim).is_some_and(|path| {
                allowed_targets.iter().any(|target| target == path)
                    || path.starts_with(&format!(".appsdk/records/evidence/{}/", module_id))
                    || transaction_targets
                        .as_ref()
                        .is_some_and(|targets| targets.iter().any(|target| target == path))
                    || path.starts_with(&format!(".appsdk/transactions/producer-{}/", module_id))
                    || path.starts_with(&format!(".appsdk/records/attempts/{}/", module_id))
            })
        });
        if !status_only {
            fail("PRODUCER_WORKTREE_DIRTY");
        }
    }
    if let Some(reused) = producer_try_reuse_records(
        root,
        module_id,
        &input,
        &base_ref,
        &base_commit,
        &head_commit,
        &current_branch,
        &expected_scope_hash,
        &worktree_id,
        &reproduction_id,
        &baseline_id,
        goal_issue_binding
            .as_ref()
            .map(|(goal_issue_id, binding)| (goal_issue_id.as_str(), binding.as_str())),
        &input_hashes,
        &command_declaration,
        expected_status,
        &expected_error_token,
    ) {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": true,
                "module_id": module_id,
                "goal_id": input["goal_id"],
                "reused": true,
                "records": reused.iter().map(|(target, record)| serde_json::json!({
                    "path": target.strip_prefix(root).unwrap_or(target).display().to_string(),
                    "id": record.get("worktree_id").or_else(|| record.get("reproduction_id")).or_else(|| record.get("evidence_id"))
                })).collect::<Vec<_>>()
            }))
            .unwrap()
        );
        return;
    }
    let bug_triage = worktree.get("bug_triage");
    if producer_transaction_dir(root, module_id).exists() {
        let replace_current = transaction_replace_current.unwrap_or(false);
        if let Some(recovered) = producer_record_transaction_recover(
            root,
            module_id,
            &input_hash,
            replace_current,
            |recovered| {
                assert_recovered_record_bindings(
                    root,
                    recovered,
                    module_id,
                    &worktree_issue,
                    &base_ref,
                    &base_commit,
                    &head_commit,
                    &current_branch,
                    &expected_scope_hash,
                    &worktree_id,
                    &reproduction_id,
                    &baseline_id,
                    goal_issue_binding
                        .as_ref()
                        .map(|(goal_issue_id, binding)| (goal_issue_id.as_str(), binding.as_str())),
                    &input_hashes,
                    &command_declaration,
                    expected_status,
                    bug_triage,
                );
            },
        ) {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "ok": true,
                    "module_id": module_id,
                    "goal_id": input["goal_id"],
                    "recovered": true,
                    "records": recovered.iter().map(|(target, record)| serde_json::json!({
                        "path": target.strip_prefix(root).unwrap_or(target).display().to_string(),
                        "id": record.get("worktree_id").or_else(|| record.get("reproduction_id")).or_else(|| record.get("evidence_id"))
                    })).collect::<Vec<_>>()
                }))
                .unwrap()
            );
            return;
        }
    }
    // Validate the declaration before creating a temporary worktree. The
    // baseline checkout must be disposable even when its directory is
    // malformed or absent at the declared source commit.
    let _candidate_command_directory =
        producer_command_dir(root, &working_directory).unwrap_or_else(|error| fail(error));
    let baseline_started_at = Utc::now();
    let baseline_root = producer_baseline_worktree(root, &base_commit);
    match producer_baseline_git_value(&baseline_root, &["rev-parse", "HEAD"]) {
        Ok(head) if head == base_commit => {}
        Ok(_) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail("BASELINE_WORKTREE_COMMIT_MISMATCH");
        }
        Err(error) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail(error);
        }
    }
    let baseline_project_root = baseline_root.join(&project_relative_path);
    let command_directory = match producer_command_dir(&baseline_project_root, &working_directory) {
        Ok(path) => path,
        Err(error) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail(error);
        }
    };
    let output = match Command::new(&program)
        .args(&args)
        .current_dir(&command_directory)
        .output()
    {
        Ok(output) => output,
        Err(_) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail("BASELINE_COMMAND_FAILED");
        }
    };
    let actual_status = output.status.code().unwrap_or(-1);
    if actual_status != expected_status {
        remove_producer_baseline_worktree(root, &baseline_root);
        fail(format!(
            "BASELINE_COMMAND_STATUS_MISMATCH:expected={}:actual={}",
            expected_status, actual_status
        ));
    }
    match producer_baseline_git_value(&baseline_root, &["rev-parse", "HEAD"]) {
        Ok(head) if head == base_commit => {}
        Ok(_) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail("BASELINE_COMMAND_CHANGED_COMMIT");
        }
        Err(error) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail(error);
        }
    }
    let output_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output_text.contains(&expected_error_token) {
        remove_producer_baseline_worktree(root, &baseline_root);
        fail("BASELINE_ERROR_TOKEN_MISSING");
    }
    match producer_baseline_git_value(
        &baseline_root,
        &["status", "--porcelain", "--untracked-files=all"],
    ) {
        Ok(status) if status.is_empty() => {}
        Ok(_) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail("BASELINE_COMMAND_DIRTY_WORKTREE");
        }
        Err(error) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail(error);
        }
    }
    remove_producer_baseline_worktree(root, &baseline_root);
    let output_hash = sha256(&format!(
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ));
    let replace_current = producer_archive_current_set(root, module_id);
    for target in [
        records_root.join(module_record_name("worktree-record", module_id)),
        records_root.join(module_record_name("reproduction-record", module_id)),
    ] {
        assert_no_symlink_components(root, &target, "record_control");
        if target.exists() && !replace_current {
            fail(format!(
                "LIFECYCLE_RECORD_EXISTS:{}",
                target.strip_prefix(root).unwrap_or(&target).display()
            ));
        }
    }
    let mut observed_worktree = worktree.clone();
    observed_worktree["worktree_id"] = Value::String(worktree_id);
    observed_worktree["base_commit"] = Value::String(base_commit.clone());
    observed_worktree["head_commit"] = Value::String(head_commit.clone());
    observed_worktree["branch"] = Value::String(current_branch);
    observed_worktree["initial_clean"] = Value::Bool(true);
    observed_worktree["final_clean"] = Value::Bool(true);
    observed_worktree["created_at"] = Value::String(baseline_started_at.to_rfc3339());
    observed_worktree
        .as_object_mut()
        .unwrap()
        .remove("goal_issue_binding");
    if let Some((goal_issue_id, goal_issue_binding)) = &goal_issue_binding {
        observed_worktree["goal_issue_id"] = Value::String(goal_issue_id.clone());
        observed_worktree["goal_issue_binding"] = Value::String(goal_issue_binding.clone());
    }
    if let Some(observed_bug_triage) = worktree.get("bug_triage") {
        if !observed_bug_triage.is_object() {
            fail("INVALID_BUG_TRIAGE");
        }
        observed_worktree["bug_triage"] = observed_bug_triage.clone();
        observed_worktree["bug_triage_query_binding"] =
            Value::String(bug_triage_binding(&worktree_issue, observed_bug_triage));
    }
    let mut observed_reproduction = reproduction.clone();
    observed_reproduction["reproduction_id"] = Value::String(reproduction_id);
    observed_reproduction["worktree_id"] = observed_worktree["worktree_id"].clone();
    observed_reproduction["input_hashes"] = serde_json::json!(input_hashes);
    observed_reproduction["baseline_evidence_id"] = Value::String(baseline_id.clone());
    observed_reproduction["first_divergence"] =
        Value::String(format!("baseline_error_token:{}", expected_error_token));
    observed_reproduction["base_commit"] = Value::String(base_commit);
    observed_reproduction["result"] = Value::String("reproduced".into());
    observed_reproduction["created_at"] = Value::String(Utc::now().to_rfc3339());
    let mut observed_baseline = baseline.clone();
    observed_baseline["source_commit"] = Value::String(producer_string(
        &worktree,
        "/base_commit",
        "INVALID_WORKTREE_RECORD",
    ));
    observed_baseline["evidence_id"] = Value::String(baseline_id);
    observed_baseline["input_hashes"] = serde_json::json!(input_hashes);
    observed_baseline["producer"] = serde_json::json!({
        "adapter": "appsdk",
        "identity": "appsdk-lifecycle-record-producer"
    });
    observed_baseline["result"] = Value::String("pass".into());
    observed_baseline["command"] = command_declaration;
    observed_baseline["exit_status"] = Value::Number(actual_status.into());
    observed_baseline["output_hash"] = Value::String(output_hash);
    observed_baseline["created_at"] = Value::String(Utc::now().to_rfc3339());
    observed_baseline["expires_at"] =
        Value::String((Utc::now() + chrono::Duration::hours(24)).to_rfc3339());
    let targets = vec![
        (targets[0].0.clone(), observed_worktree),
        (targets[1].0.clone(), observed_reproduction),
        (targets[2].0.clone(), observed_baseline),
    ];
    assert_produced_record_shapes(root, &targets, module_id);
    producer_commit_records(root, module_id, &input_hash, replace_current, &targets);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "ok": true,
            "module_id": module_id,
            "goal_id": input["goal_id"],
            "records": targets.iter().map(|(target, record)| serde_json::json!({
                "path": target.strip_prefix(root).unwrap_or(target).display().to_string(),
                "id": record.get("worktree_id").or_else(|| record.get("reproduction_id")).or_else(|| record.get("evidence_id"))
            })).collect::<Vec<_>>()
        }))
        .unwrap()
    );
}

pub(super) fn lifecycle_chain_input(root: &Path, input_path: &str, phase: &str) -> Value {
    let input_file = producer_input_path(root, input_path);
    let input: Value = serde_json::from_str(
        &fs::read_to_string(input_file).unwrap_or_else(|_| fail("PRODUCER_INPUT_READ_FAILED")),
    )
    .unwrap_or_else(|_| fail("INVALID_PRODUCER_INPUT"));
    if !input.is_object() {
        fail("INVALID_PRODUCER_INPUT");
    }
    input
        .get(phase)
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| {
            fail(format!(
                "PRODUCER_{}_INPUT_MISSING",
                phase.to_ascii_uppercase()
            ))
        })
}

pub(super) fn lifecycle_chain_required_array(
    value: &Value,
    path: &str,
    error: &str,
) -> Vec<String> {
    let values = value
        .pointer(path)
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| fail(error));
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let id = value
            .as_str()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| fail(error));
        result.push(id.to_string());
    }
    result
}

pub(super) fn lifecycle_chain_candidate(
    root: &Path,
    module_id: &str,
) -> (Value, Value, Value, Value) {
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let candidate = read_record(root, &candidate_name);
    let validation = read_record(
        root,
        &module_record_name("pre-review-validation-record", module_id),
    );
    let reproduction = read_record(root, &module_record_name("reproduction-record", module_id));
    let worktree = read_record(root, &module_record_name("worktree-record", module_id));
    if producer_string(&candidate, "/module_id", &candidate_name) != module_id
        || producer_string(
            &validation,
            "/module_id",
            "pre-review-validation-record.json",
        ) != module_id
        || producer_string(&reproduction, "/module_id", "reproduction-record.json") != module_id
        || producer_string(&worktree, "/module_id", "worktree-record.json") != module_id
    {
        fail("LIFECYCLE_CHAIN_MODULE_MISMATCH");
    }
    (worktree, reproduction, candidate, validation)
}

pub(super) fn lifecycle_chain_record_path(root: &Path, module_id: &str, kind: &str) -> PathBuf {
    root.join(".appsdk")
        .join("records")
        .join(module_record_name(kind, module_id))
}

pub(super) fn lifecycle_chain_read_record_if_present(
    root: &Path,
    module_id: &str,
    kind: &str,
) -> Option<Value> {
    let target = lifecycle_chain_record_path(root, module_id, kind);
    assert_no_symlink_components(root, &target, "lifecycle_chain_record");
    producer_read_record_if_present(&target, "LIFECYCLE_CHAIN_RECORD_INVALID")
}

pub(super) fn lifecycle_chain_record_is_pass(kind: &str, record: &Value) -> bool {
    let result = record.get("result").and_then(Value::as_str);
    let verdict = record.get("verdict").and_then(Value::as_str);
    // A contradictory status must never become a cache hit.  Promotion records
    // historically derive PASS from their gate set, while newer projections may
    // also carry an explicit result; every present status must agree.
    if record.get("result").is_some_and(|value| !value.is_string())
        || record
            .get("verdict")
            .is_some_and(|value| !value.is_string())
    {
        return false;
    }
    if result.is_some_and(|value| value != "pass") || verdict.is_some_and(|value| value != "pass") {
        return false;
    }
    match kind {
        "review-record" => verdict == Some("pass"),
        "promotion-record" => {
            let gates_pass = record
                .get("required_gate_results")
                .and_then(Value::as_array)
                .is_some_and(|gates| {
                    !gates.is_empty()
                        && gates
                            .iter()
                            .all(|gate| gate.get("result").and_then(Value::as_str) == Some("pass"))
                });
            result == Some("pass") || verdict == Some("pass") || gates_pass
        }
        _ => result == Some("pass"),
    }
}

pub(super) fn lifecycle_chain_attempt_identity(
    module_id: &str,
    kind: &str,
    record_hash: &str,
) -> String {
    producer_stable_id(
        "lifecycle-attempt",
        &serde_json::json!({
            "module_id": module_id,
            "phase": kind,
            "record_hash": record_hash
        }),
    )
}

pub(super) fn lifecycle_chain_attempt_path(root: &Path, module_id: &str, kind: &str) -> PathBuf {
    root.join(".appsdk")
        .join("records")
        .join("attempts")
        .join(module_id)
        .join(format!("{}.jsonl", kind))
}

pub(super) fn lifecycle_chain_append_attempt_with_result(
    root: &Path,
    module_id: &str,
    kind: &str,
    record: &Value,
    record_json: &str,
    result: &str,
) {
    if !matches!(result, "non_pass" | "stale") {
        fail("LIFECYCLE_CHAIN_ATTEMPT_RESULT_INVALID");
    }
    let target = lifecycle_chain_attempt_path(root, module_id, kind);
    let record_hash = sha256(&canonical(record));
    let attempt_id = lifecycle_chain_attempt_identity(module_id, kind, &record_hash);
    if lifecycle_chain_validate_attempt_ledger(
        root,
        module_id,
        kind,
        &attempt_id,
        &record_hash,
        record,
    ) {
        return;
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|_| fail("LIFECYCLE_CHAIN_ATTEMPT_WRITE_FAILED"));
    }
    let envelope = serde_json::json!({
        "schema_version": 1,
        "attempt_id": attempt_id,
        "module_id": module_id,
        "phase": kind,
        "result": result,
        "record_hash": record_hash,
        "record_json": record_json,
        "record": record,
        "archived_at": Utc::now().to_rfc3339()
    });
    let mut line = serde_json::to_vec(&envelope)
        .unwrap_or_else(|_| fail("LIFECYCLE_CHAIN_ATTEMPT_WRITE_FAILED"));
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&target)
        .unwrap_or_else(|_| fail("LIFECYCLE_CHAIN_ATTEMPT_WRITE_FAILED"));
    file.write_all(&line)
        .and_then(|_| file.sync_all())
        .unwrap_or_else(|_| fail("LIFECYCLE_CHAIN_ATTEMPT_WRITE_FAILED"));
    if let Some(parent) = target.parent() {
        if let Ok(file) = OpenOptions::new().read(true).open(parent) {
            let _ = file.sync_all();
        }
    }
}

pub(super) fn lifecycle_chain_append_attempt(
    root: &Path,
    module_id: &str,
    kind: &str,
    record: &Value,
    record_json: &str,
) {
    lifecycle_chain_append_attempt_with_result(
        root,
        module_id,
        kind,
        record,
        record_json,
        "non_pass",
    );
}

pub(super) fn lifecycle_chain_validate_attempt_ledger(
    root: &Path,
    module_id: &str,
    kind: &str,
    attempt_id: &str,
    record_hash: &str,
    record: &Value,
) -> bool {
    let target = lifecycle_chain_attempt_path(root, module_id, kind);
    assert_no_symlink_components(root, &target, "lifecycle_chain_attempt");
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID")
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return false,
        Err(_) => fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID"),
    }
    let contents = fs::read_to_string(&target)
        .unwrap_or_else(|_| fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID"));
    let mut matching_attempt = false;
    let mut seen_attempt_ids = BTreeSet::new();
    for line in contents.lines() {
        if line.trim().is_empty() {
            fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID");
        }
        let existing: Value = serde_json::from_str(line)
            .unwrap_or_else(|_| fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID"));
        if existing.get("schema_version").and_then(Value::as_u64) != Some(1)
            || existing.get("module_id").and_then(Value::as_str) != Some(module_id)
            || existing.get("phase").and_then(Value::as_str) != Some(kind)
            || !matches!(
                existing.get("result").and_then(Value::as_str),
                Some("non_pass" | "stale")
            )
            || existing.get("attempt_id").and_then(Value::as_str).is_none()
            || existing.get("record").is_none()
            || existing
                .get("record_hash")
                .and_then(Value::as_str)
                .is_none()
            || existing
                .get("archived_at")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
        {
            fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID");
        }
        let archived_at = DateTime::parse_from_rfc3339(existing["archived_at"].as_str().unwrap())
            .unwrap_or_else(|_| fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID"))
            .with_timezone(&Utc);
        if archived_at > Utc::now() {
            fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID");
        }
        let existing_attempt_id = existing["attempt_id"].as_str().unwrap();
        if !seen_attempt_ids.insert(existing_attempt_id.to_string()) {
            fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID");
        }
        let existing_record = &existing["record"];
        let existing_record_hash = sha256(&canonical(existing_record));
        if existing["record_hash"].as_str().unwrap() != existing_record_hash {
            fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID");
        }
        let expected_existing_attempt_id =
            lifecycle_chain_attempt_identity(module_id, kind, &existing_record_hash);
        if existing_attempt_id != expected_existing_attempt_id {
            fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID");
        }
        if let Some(record_json) = existing.get("record_json") {
            let record_json = record_json
                .as_str()
                .unwrap_or_else(|| fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID"));
            let parsed = serde_json::from_str::<Value>(record_json)
                .unwrap_or_else(|_| fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID"));
            if parsed != *existing_record {
                fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_CONFLICT");
            }
        } else if !producer_legacy_record_is_unambiguous(existing_record) {
            fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID");
        }
        if expected_existing_attempt_id == attempt_id {
            if existing_record_hash != record_hash || existing_record != record {
                fail("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_CONFLICT");
            }
            matching_attempt = true;
        }
    }
    matching_attempt
}

pub(super) fn lifecycle_chain_assert_reusable_record(
    existing: &Value,
    expected: &Value,
    name: &str,
    mismatch_error: &str,
    pass: bool,
) {
    if !pass {
        fail("LIFECYCLE_CHAIN_STAGE_NOT_PASS");
    }
    producer_assert_reuse_match(existing, expected, &["created_at"], mismatch_error);
    let created_at = record_time(existing, name);
    if created_at > Utc::now() {
        fail("LIFECYCLE_CHAIN_RECORD_FUTURE");
    }
}

pub(super) fn lifecycle_chain_output(record: &Value, reused: bool) {
    let mut output = record.clone();
    output["reused"] = Value::Bool(reused);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}

pub(super) fn lifecycle_chain_write_record(
    root: &Path,
    module_id: &str,
    kind: &str,
    record: &Value,
) -> bool {
    let target = lifecycle_chain_record_path(root, module_id, kind);
    assert_no_symlink_components(root, &target, "lifecycle_chain_record");
    if !target.exists() {
        let record_hash = sha256(&canonical(record));
        let attempt_id = lifecycle_chain_attempt_identity(module_id, kind, &record_hash);
        lifecycle_chain_validate_attempt_ledger(
            root,
            module_id,
            kind,
            &attempt_id,
            &record_hash,
            record,
        );
    }
    if target.exists() {
        let (existing, existing_json) =
            producer_read_record_bytes_if_present(&target, "LIFECYCLE_CHAIN_RECORD_INVALID")
                .unwrap_or_else(|| fail("LIFECYCLE_CHAIN_RECORD_INVALID"));
        record_time(&existing, &target.display().to_string());
        let existing_record_hash = sha256(&canonical(&existing));
        let existing_attempt_id =
            lifecycle_chain_attempt_identity(module_id, kind, &existing_record_hash);
        lifecycle_chain_validate_attempt_ledger(
            root,
            module_id,
            kind,
            &existing_attempt_id,
            &existing_record_hash,
            &existing,
        );
        let existing_pass = lifecycle_chain_record_is_pass(kind, &existing);
        let expected_pass = lifecycle_chain_record_is_pass(kind, record);
        if existing_pass {
            if !expected_pass {
                fail("LIFECYCLE_CHAIN_PASS_IMMUTABLE");
            }
            if producer_without_fields(&existing, &["created_at"])
                == producer_without_fields(record, &["created_at"])
            {
                lifecycle_chain_assert_reusable_record(
                    &existing,
                    record,
                    &target.display().to_string(),
                    "LIFECYCLE_CHAIN_RECORD_IDENTITY_MISMATCH",
                    true,
                );
                return true;
            }
            // A PASS projection is immutable as an audit witness, but the
            // current projection may advance to a new candidate identity. Keep
            // the complete old bytes in the append-only ledger before replacing
            // the current file.
            lifecycle_chain_append_attempt_with_result(
                root,
                module_id,
                kind,
                &existing,
                &existing_json,
                "stale",
            );
        }
        if !existing_pass {
            if producer_without_fields(&existing, &["created_at"])
                == producer_without_fields(record, &["created_at"])
            {
                fail("LIFECYCLE_CHAIN_STAGE_NOT_PASS");
            }
            lifecycle_chain_append_attempt(root, module_id, kind, &existing, &existing_json);
        }
        producer_durable_json(
            target.as_path(),
            record,
            "LIFECYCLE_CHAIN_RECORD_WRITE_FAILED",
        );
        return false;
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|_| fail("LIFECYCLE_CHAIN_RECORD_WRITE_FAILED"));
    }
    producer_durable_json(&target, record, "LIFECYCLE_CHAIN_RECORD_WRITE_FAILED");
    false
}

pub(super) fn lifecycle_chain_validate_evidence(
    root: &Path,
    module_id: &str,
    evidence_id: &str,
    issue_id: &str,
    scope_hash: &str,
    source_commit: &str,
) -> Value {
    let evidence = evidence_by_id(root, module_id, evidence_id);
    assert_evidence_record(
        &evidence,
        evidence_id,
        EvidenceValidationMode::Current(Utc::now()),
    );
    if producer_string(&evidence, "/evidence_id", evidence_id) != evidence_id
        || producer_string(&evidence, "/issue_id", evidence_id) != issue_id
        || producer_string(&evidence, "/scope/module_id", evidence_id) != module_id
        || producer_string(&evidence, "/scope_hash", evidence_id) != scope_hash
        || producer_string(&evidence, "/source_commit", evidence_id) != source_commit
        || producer_string(&evidence, "/result", evidence_id) != "pass"
    {
        fail("LIFECYCLE_CHAIN_EVIDENCE_MISMATCH");
    }
    evidence
}
