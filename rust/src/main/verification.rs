use super::*;

pub(super) fn produce_lifecycle_chain(root: &Path, module_id: &str, phase: &str, input_path: &str) {
    if !matches!(
        phase,
        "architecture" | "effectiveness" | "merge" | "promotion"
    ) {
        fail("PRODUCER_PHASE_INVALID");
    }
    let _producer_lock = producer_lock(root);
    match phase {
        "architecture" => lifecycle_chain_architecture(root, module_id, input_path),
        "effectiveness" => lifecycle_chain_effectiveness(root, module_id, input_path),
        "merge" => lifecycle_chain_merge(root, module_id, input_path),
        "promotion" => lifecycle_chain_promotion(root, module_id, input_path),
        _ => unreachable!(),
    }
}

pub(super) fn record_str<'a>(record: &'a Value, path: &str, name: &str) -> &'a str {
    record
        .pointer(path)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail(format!("INVALID_RECORD:{}:{}", name, path)))
}

pub(super) fn record_array<'a>(record: &'a Value, path: &str, name: &str) -> &'a Vec<Value> {
    record
        .pointer(path)
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| fail(format!("INVALID_RECORD:{}:{}", name, path)))
}

pub(super) fn assert_record_schema(
    evidence: &Value,
    review: &Value,
    promotion: &Value,
    allow_legacy_rehydrate_bindings: bool,
    validation: EvidenceValidationMode,
) {
    for (record, name, fields) in [
        (
            evidence,
            "evidence-record.json",
            &[
                "evidence_id",
                "issue_id",
                "experiment_id",
                "phase",
                "source_commit",
                "created_at",
                "expires_at",
                "scope_hash",
            ][..],
        ),
        (
            review,
            "review-record.json",
            &[
                "review_id",
                "issue_id",
                "promotion_id",
                "review_kind",
                "fix_candidate_id",
                "reviewed_commit",
                "reviewed_tree_hash",
                "reviewed_diff_hash",
                "reviewed_artifact_hash",
                "reviewed_scope_hash",
                "resource_map_hash",
                "function_map_hash",
                "mainline_call_map_hash",
                "verification_map_hash",
                "created_at",
            ][..],
        ),
        (
            promotion,
            "promotion-record.json",
            &[
                "promotion_id",
                "issue_id",
                "experiment_id",
                "module_id",
                "base_commit",
                "source_commit",
                "candidate_commit",
                "merged_commit",
                "new_active_version",
                "review_id",
                "worktree_record_id",
                "reproduction_record_id",
                "fix_candidate_id",
                "architecture_review_id",
                "effectiveness_record_id",
                "merge_record_id",
                "root_cause",
                "design_id",
                "change_reason_comment",
                "playground_cleanup_record_id",
                "created_at",
            ][..],
        ),
    ] {
        for field in fields {
            record_str(record, &format!("/{}", field), name);
        }
    }
    assert_evidence_record(evidence, "evidence-record.json", validation);
    for path in [
        "/reviewer/adapter",
        "/reviewer/identity",
        "/verdict",
        "/reviewed_commit",
        "/reviewed_artifact_hash",
        "/reviewed_scope_hash",
        "/created_at",
    ] {
        record_str(review, path, "review-record.json");
    }
    for path in [
        "/base_commit",
        "/new_active_version",
        "/review_id",
        "/change_set_id",
        "/compatibility_level",
        "/root_cause",
        "/design_id",
        "/change_reason_comment",
        "/playground_cleanup_record_id",
        "/created_at",
    ] {
        record_str(promotion, path, "promotion-record.json");
    }
    if !matches!(
        promotion.get("compatibility_level").and_then(Value::as_str),
        Some("compatible" | "migration_required" | "breaking")
    ) {
        fail("INVALID_PROMOTION_RECORD");
    }
    let reviewer = review
        .get("reviewer")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("INVALID_REVIEW_RECORD"));
    for key in ["adapter", "identity"] {
        if reviewer
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            fail("INVALID_REVIEW_RECORD");
        }
    }
    if review
        .get("evidence_ids")
        .and_then(Value::as_array)
        .map(|values| values.is_empty() || values.iter().any(|value| value.as_str().is_none()))
        .unwrap_or(true)
    {
        fail("INVALID_REVIEW_RECORD");
    }
    if !allow_legacy_rehydrate_bindings {
        assert_lifecycle_chain_review_identity(review);
    }
    if !promotion
        .get("previous_active_version")
        .map(|value| value.is_null() || value.as_str().is_some())
        .unwrap_or(false)
        || promotion
            .get("evidence_ids")
            .and_then(Value::as_array)
            .map(|values| values.is_empty() || values.iter().any(|value| value.as_str().is_none()))
            .unwrap_or(true)
        || promotion
            .get("artifact_hash")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        || promotion
            .get("scope_hash")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        || promotion
            .get("public_api_hash")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
    {
        fail("INVALID_PROMOTION_RECORD");
    }
    let gates = promotion
        .get("required_gate_results")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| fail("INVALID_PROMOTION_RECORD"));
    for gate in gates {
        for key in ["gate_id", "producer"] {
            if gate
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                fail("INVALID_PROMOTION_RECORD");
            }
        }
        if gate.get("result").and_then(Value::as_str).is_none() {
            fail("INVALID_PROMOTION_RECORD");
        }
    }
}

pub(super) fn assert_regression_report(
    root: &Path,
    module_id: &str,
    module: &Value,
    promotion: &Value,
    artifact: &Value,
) -> (Value, String) {
    let name = module_record_name("regression-report", module_id);
    let report = read_record(root, &name);
    for path in [
        "/regression_report_id",
        "/module_id",
        "/source_commit",
        "/artifact_hash",
        "/public_api_hash",
        "/scope_hash",
        "/input_hash",
        "/suite_id",
        "/command/program",
        "/command/working_directory",
        "/producer/adapter",
        "/producer/identity",
        "/created_at",
    ] {
        record_str(&report, path, &name);
    }
    let tc = report
        .get("test_characteristics")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail(format!("INVALID_REGRESSION_REPORT:{}", name)));
    if tc.get("whitebox") != Some(&Value::Bool(true))
        || tc.get("blackbox") != Some(&Value::Bool(true))
    {
        fail(format!("INVALID_REGRESSION_REPORT:{}", name));
    }
    let policy = module
        .get("regression")
        .unwrap_or_else(|| fail(format!("REGRESSION_CONTRACT_REQUIRED:{}", module_id)));
    if record_str(&report, "/module_id", &name) != module_id
        || record_str(&report, "/source_commit", &name)
            != record_str(promotion, "/source_commit", "promotion-record.json")
        || record_str(&report, "/artifact_hash", &name)
            != record_str(artifact, "/artifact_hash", "artifact")
        || record_str(&report, "/public_api_hash", &name)
            != record_str(promotion, "/public_api_hash", "promotion-record.json")
        || record_str(&report, "/scope_hash", &name)
            != record_str(promotion, "/scope_hash", "promotion-record.json")
        || record_str(&report, "/input_hash", &name)
            != record_str(artifact, "/artifact_hash", "artifact")
        || record_str(&report, "/suite_id", &name)
            != record_str(policy, "/suite_id", "regression-policy")
        || report.get("command") != policy.get("command")
    {
        fail("REGRESSION_REPORT_INPUT_MISMATCH");
    }
    let test_count = report
        .get("test_count")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fail("INVALID_REGRESSION_REPORT"));
    let passed = report
        .get("passed")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fail("INVALID_REGRESSION_REPORT"));
    let failed = report
        .get("failed")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fail("INVALID_REGRESSION_REPORT"));
    let skipped = report
        .get("skipped")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fail("INVALID_REGRESSION_REPORT"));
    let minimum = policy
        .get("minimum_test_count")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fail("INVALID_REGRESSION_CONTRACT"));
    if report.get("result").and_then(Value::as_str) != Some("pass")
        || test_count < minimum
        || passed != test_count
        || failed != 0
        || (!policy
            .get("allow_skipped")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && skipped != 0)
    {
        fail("REGRESSION_REPORT_NOT_PASSED");
    }
    let report_hash = sha256(&canonical(&report));
    (report, report_hash)
}

pub(super) fn git_value(root: &Path, args: &[&str], error: &str) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap_or_else(|_| fail(error));
    if !output.status.success() {
        fail(error);
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

pub(super) fn is_linked_git_worktree(root: &Path) -> bool {
    let output = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ])
        .output();
    let Ok(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from);
    let Some(git_dir) = lines.next() else {
        return false;
    };
    let Some(common_dir) = lines.next() else {
        return false;
    };
    if lines.next().is_some() {
        return false;
    }
    match (fs::canonicalize(git_dir), fs::canonicalize(common_dir)) {
        (Ok(git_dir), Ok(common_dir)) => git_dir != common_dir,
        _ => false,
    }
}

pub(super) fn assert_ordinary_init_canonical_project_main_tree(root: &Path, fresh: bool) {
    if fresh {
        return;
    }
    let mut ancestor = Some(root);
    while let Some(candidate) = ancestor {
        if candidate.exists() {
            if is_linked_git_worktree(candidate) {
                fail(format!(
                    "INIT_REQUIRES_CANONICAL_PROJECT_MAIN_TREE:{}",
                    candidate.display()
                ));
            }
        }
        ancestor = candidate.parent();
    }
}

pub(super) fn assert_mutation_worktree(root: &Path) {
    let output = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "symbolic-ref",
            "--quiet",
            "--short",
            "HEAD",
        ])
        .output()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "main" {
        fail("MAIN_WORKTREE_MUTATION_FORBIDDEN");
    }
}

pub(super) fn assert_candidate_source_identity(root: &Path, module: &Value, candidate_commit: &str) {
    let mut controlled_paths = Vec::new();
    for key in ["owned_paths", "contract_paths"] {
        for value in record_array(module, &format!("/{}", key), "module") {
            controlled_paths.push(
                value
                    .as_str()
                    .unwrap_or_else(|| fail("INVALID_MODULE_CONTROLLED_PATH")),
            );
        }
    }
    let diff_status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--quiet", candidate_commit, "--"])
        .args(&controlled_paths)
        .status()
        .unwrap_or_else(|_| fail("CANDIDATE_SOURCE_GIT_UNAVAILABLE"));
    if !diff_status.success() {
        fail("CANDIDATE_CONTROLLED_SOURCE_DRIFT");
    }
    let untracked = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--others", "--exclude-standard", "--"])
        .args(&controlled_paths)
        .output()
        .unwrap_or_else(|_| fail("CANDIDATE_SOURCE_GIT_UNAVAILABLE"));
    if !untracked.status.success() || !untracked.stdout.is_empty() {
        fail("CANDIDATE_CONTROLLED_SOURCE_DRIFT");
    }
}

pub(super) fn assert_worktree_candidate_ancestry(root: &Path, worktree_head: &str, candidate_commit: &str) {
    if !Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "merge-base",
            "--is-ancestor",
            worktree_head,
            candidate_commit,
        ])
        .status()
        .is_ok_and(|status| status.success())
    {
        fail("FIX_REPRODUCTION_GRAPH_MISMATCH");
    }
}

pub(super) fn assert_lifecycle_chain_candidate_at_head(
    root: &Path,
    project: &Value,
    module_id: &str,
    candidate_commit: &str,
    candidate_tree: &str,
) {
    let module = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or_else(|| fail(format!("UNKNOWN_MODULE:{}", module_id)));
    if git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", candidate_commit)],
        "PRODUCER_CANDIDATE_TREE_UNAVAILABLE",
    ) != candidate_tree
    {
        fail("LIFECYCLE_CHAIN_CANDIDATE_DRIFT");
    }
    let head_commit = git_value(
        root,
        &["rev-parse", "HEAD"],
        "PRODUCER_HEAD_COMMIT_UNAVAILABLE",
    );
    if !Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "merge-base",
            "--is-ancestor",
            candidate_commit,
            &head_commit,
        ])
        .status()
        .is_ok_and(|status| status.success())
    {
        fail("LIFECYCLE_CHAIN_CANDIDATE_DRIFT");
    }
    assert_candidate_source_identity(root, module, candidate_commit);
    let git_root = PathBuf::from(git_value(
        root,
        &["rev-parse", "--show-toplevel"],
        "CANDIDATE_SOURCE_GIT_UNAVAILABLE",
    ));
    let project_root = root
        .canonicalize()
        .unwrap_or_else(|_| fail("CANDIDATE_SOURCE_GIT_UNAVAILABLE"));
    let git_root = git_root
        .canonicalize()
        .unwrap_or_else(|_| fail("CANDIDATE_SOURCE_GIT_UNAVAILABLE"));
    let project_relative = project_root
        .strip_prefix(&git_root)
        .unwrap_or_else(|_| fail("CANDIDATE_SOURCE_GIT_UNAVAILABLE"));
    let records_prefix = if project_relative.as_os_str().is_empty() {
        ".appsdk/records/".to_string()
    } else {
        format!("{}/.appsdk/records/", project_relative.to_string_lossy())
    };
    let changed = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--name-only", candidate_commit, &head_commit])
        .output()
        .unwrap_or_else(|_| fail("CANDIDATE_SOURCE_GIT_UNAVAILABLE"));
    if !changed.status.success()
        || String::from_utf8_lossy(&changed.stdout)
            .lines()
            .any(|path| !path.starts_with(&records_prefix))
    {
        fail("LIFECYCLE_CHAIN_CANDIDATE_DRIFT");
    }
}

pub(super) fn git_ls_remote(root: &Path, remote: &str, remote_ref: &str) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-remote", remote, remote_ref])
        .output()
        .unwrap_or_else(|_| fail("REMOTE_ADAPTER_UNAVAILABLE"));
    if !output.status.success() {
        fail("REMOTE_MAIN_QUERY_FAILED");
    }
    String::from_utf8(output.stdout)
        .ok()
        .and_then(|value| value.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| fail("REMOTE_MAIN_REF_MISSING"))
}

pub(super) fn record_time(record: &Value, name: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(record_str(record, "/created_at", name))
        .unwrap_or_else(|_| fail(format!("INVALID_RECORD_TIME:{}", name)))
        .with_timezone(&Utc)
}

pub(super) fn record_datetime(record: &Value, path: &str, name: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(record_str(record, path, name))
        .unwrap_or_else(|_| fail(format!("INVALID_RECORD_TIME:{}:{}", name, path)))
        .with_timezone(&Utc)
}

pub(super) fn assert_evidence_record(evidence: &Value, name: &str, validation: EvidenceValidationMode) {
    for path in [
        "/evidence_id",
        "/issue_id",
        "/experiment_id",
        "/phase",
        "/kind",
        "/source_commit",
        "/result",
        "/created_at",
        "/expires_at",
        "/scope_hash",
        "/scope/module_id",
        "/producer/adapter",
        "/producer/identity",
    ] {
        record_str(evidence, path, name);
    }
    if !matches!(
        evidence.get("kind").and_then(Value::as_str),
        Some(
            "red_test"
                | "positive_test"
                | "negative_test"
                | "sample_replay"
                | "build"
                | "install"
                | "restart"
                | "artifact"
                | "runtime"
                | "gate"
        )
    ) || evidence.get("result").and_then(Value::as_str) != Some("pass")
        || evidence
            .get("input_hashes")
            .and_then(Value::as_array)
            .map(|values| values.iter().any(|value| value.as_str().is_none()))
            .unwrap_or(true)
    {
        fail(format!("INVALID_EVIDENCE_RECORD:{}", name));
    }
    let created_at = record_time(evidence, name);
    let expires_at = record_datetime(evidence, "/expires_at", name);
    let expired = match validation {
        EvidenceValidationMode::Current(admission_time) => admission_time > expires_at,
        EvidenceValidationMode::HistoricalAt(as_of) => as_of > expires_at,
        EvidenceValidationMode::Historical => fail("HISTORICAL_VALIDATION_UNBOUND"),
    };
    if created_at > expires_at || expired {
        fail(format!("EXPIRED_EVIDENCE_RECORD:{}", name));
    }
}

pub(super) fn evidence_by_id(root: &Path, module_id: &str, evidence_id: &str) -> Value {
    assert_identifier(evidence_id, "INVALID_EVIDENCE_ID");
    let relative = format!(
        ".appsdk/records/evidence/{}/{}.json",
        module_id, evidence_id
    );
    let file = safe_owned_path(root, &relative, "evidence_record");
    serde_json::from_str(
        &fs::read_to_string(&file)
            .unwrap_or_else(|_| fail(format!("MISSING_EVIDENCE_RECORD:{}", evidence_id))),
    )
    .unwrap_or_else(|_| fail(format!("INVALID_EVIDENCE_RECORD:{}", evidence_id)))
}

pub(super) fn deployment_receipt_time(
    root: &Path,
    module_id: &str,
    evidence_id: &str,
    expected_phase: &str,
    expected_kind: &str,
    issue_id: &str,
    scope_hash: &str,
    candidate_commit: &str,
    artifact_hash: &str,
    environment_id: &str,
    entrypoint: &str,
    producer: &Value,
) -> DateTime<Utc> {
    let evidence = evidence_by_id(root, module_id, evidence_id);
    assert_evidence_record(
        &evidence,
        evidence_id,
        EvidenceValidationMode::Current(Utc::now()),
    );
    if record_str(&evidence, "/issue_id", evidence_id) != issue_id
        || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
        || record_str(&evidence, "/scope_hash", evidence_id) != scope_hash
        || record_str(&evidence, "/source_commit", evidence_id) != candidate_commit
        || record_str(&evidence, "/artifact_hash", evidence_id) != artifact_hash
        || record_str(&evidence, "/phase", evidence_id) != expected_phase
        || record_str(&evidence, "/kind", evidence_id) != expected_kind
        || record_str(&evidence, "/execution_surface", evidence_id) != "deployed_blackbox"
        || record_str(&evidence, "/environment_id", evidence_id) != environment_id
        || record_str(&evidence, "/entrypoint", evidence_id) != entrypoint
        || evidence.get("producer") != Some(producer)
    {
        fail("DEPLOYMENT_RECEIPT_EVIDENCE_MISMATCH");
    }
    record_time(&evidence, evidence_id)
}

pub(super) fn assert_pre_review_validation_gate(root: &Path, module_id: &str, artifact: &Value) {
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let validation_name = module_record_name("pre-review-validation-record", module_id);
    let candidate = read_record(root, &candidate_name);
    let validation = read_record(root, &validation_name);
    let issue_id = record_str(&candidate, "/issue_id", &candidate_name);
    let scope_hash = record_str(&candidate, "/scope_hash", &candidate_name);
    let candidate_commit = record_str(&candidate, "/head_commit", &candidate_name);
    let candidate_tree = record_str(&candidate, "/tree_hash", &candidate_name);
    let artifact_hash = record_str(artifact, "/artifact_hash", "artifact");
    let project = read_project(root);
    let module = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or_else(|| fail(format!("UNKNOWN_MODULE:{}", module_id)));
    if git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", candidate_commit)],
        "FIX_CANDIDATE_COMMIT_MISSING",
    ) != candidate_tree
    {
        fail("FIX_CANDIDATE_TREE_MISMATCH");
    }
    assert_candidate_source_identity(root, module, candidate_commit);
    let rebuilt_artifact = build_module_artifact(root, &project, module, module_id);
    if record_str(&rebuilt_artifact, "/artifact_hash", "rebuilt-artifact") != artifact_hash {
        fail("REVIEW_ADMISSION_ARTIFACT_SOURCE_DRIFT");
    }
    if record_str(&validation, "/issue_id", &validation_name) != issue_id
        || record_str(&validation, "/module_id", &validation_name) != module_id
        || record_str(&validation, "/fix_candidate_id", &validation_name)
            != record_str(&candidate, "/fix_candidate_id", &candidate_name)
        || record_str(&validation, "/candidate_commit", &validation_name) != candidate_commit
        || record_str(&validation, "/candidate_tree_hash", &validation_name) != candidate_tree
        || record_str(&validation, "/artifact_hash", &validation_name) != artifact_hash
        || validation.get("source_unchanged") != Some(&Value::Bool(true))
        || validation.get("result").and_then(Value::as_str) != Some("pass")
    {
        fail("PRE_REVIEW_VALIDATION_MISMATCH");
    }
    let environment_id = record_str(&validation, "/deployment/environment_id", &validation_name);
    let entrypoint = record_str(&validation, "/deployment/entrypoint", &validation_name);
    let producer = validation
        .pointer("/deployment/producer")
        .and_then(Value::as_object)
        .filter(|value| {
            ["adapter", "identity"].iter().all(|key| {
                value
                    .get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(|entry| !entry.is_empty())
            })
        })
        .map(|_| validation.pointer("/deployment/producer").unwrap())
        .unwrap_or_else(|| fail("DEPLOYMENT_BLACKBOX_RECEIPT_MISSING"));
    let whitebox_producer = validation
        .get("whitebox_producer")
        .and_then(Value::as_object)
        .filter(|value| {
            ["adapter", "identity"].iter().all(|key| {
                value
                    .get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(|entry| !entry.is_empty())
            })
        })
        .map(|_| validation.get("whitebox_producer").unwrap())
        .unwrap_or_else(|| fail("DEVELOPMENT_WHITEBOX_PRODUCER_MISSING"));
    if environment_id.is_empty() || entrypoint.is_empty() {
        fail("DEPLOYMENT_BLACKBOX_RECEIPT_MISSING");
    }
    let mut all_ids = std::collections::HashSet::new();
    let required_operations = module_deployment_operations(module);
    let mut receipt_times = Vec::new();
    for (operation, phase, path) in [
        (
            "install",
            "deployment_install",
            "/deployment/install_receipt_id",
        ),
        (
            "restart",
            "deployment_restart",
            "/deployment/restart_receipt_id",
        ),
    ] {
        // Validate supplied receipts too; optional does not mean silently ignored.
        if required_operations.contains(&operation) || validation.pointer(path).is_some() {
            let id = record_str(&validation, path, &validation_name);
            if !all_ids.insert(id) {
                fail("PRE_REVIEW_EVIDENCE_NOT_DISJOINT");
            }
            receipt_times.push(deployment_receipt_time(
                root,
                module_id,
                id,
                phase,
                operation,
                issue_id,
                scope_hash,
                candidate_commit,
                artifact_hash,
                environment_id,
                entrypoint,
                producer,
            ));
        }
    }
    let mut latest_whitebox = None;
    let mut earliest_whitebox = None;
    let mut earliest_blackbox = None;
    let mut latest_blackbox = None;
    for (path, phase, surface) in [
        (
            "/whitebox_evidence_ids",
            "development_whitebox",
            "development_whitebox",
        ),
        (
            "/blackbox_evidence_ids",
            "deployed_blackbox",
            "deployed_blackbox",
        ),
    ] {
        for value in record_array(&validation, path, &validation_name) {
            let id = value
                .as_str()
                .unwrap_or_else(|| fail("INVALID_PRE_REVIEW_EVIDENCE_ID"));
            if !all_ids.insert(id) {
                fail("PRE_REVIEW_EVIDENCE_NOT_DISJOINT");
            }
            let evidence = evidence_by_id(root, module_id, id);
            assert_evidence_record(&evidence, id, EvidenceValidationMode::Current(Utc::now()));
            if record_str(&evidence, "/issue_id", id) != issue_id
                || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
                || record_str(&evidence, "/scope_hash", id) != scope_hash
                || record_str(&evidence, "/source_commit", id) != candidate_commit
                || record_str(&evidence, "/phase", id) != phase
                || record_str(&evidence, "/execution_surface", id) != surface
                || evidence.get("result").and_then(Value::as_str) != Some("pass")
                || record_time(&evidence, id) > record_time(&validation, &validation_name)
            {
                fail("PRE_REVIEW_EVIDENCE_MISMATCH");
            }
            if surface == "deployed_blackbox"
                && (record_str(&evidence, "/artifact_hash", id) != artifact_hash
                    || record_str(&evidence, "/environment_id", id) != environment_id
                    || record_str(&evidence, "/entrypoint", id) != entrypoint
                    || evidence.get("producer") != Some(producer)
                    || !matches!(
                        evidence.get("kind").and_then(Value::as_str),
                        Some("runtime" | "sample_replay")
                    ))
            {
                fail("DEPLOYED_BLACKBOX_EVIDENCE_MISMATCH");
            }
            if surface == "development_whitebox"
                && (record_str(&evidence, "/artifact_hash", id) != artifact_hash
                    || evidence.get("producer") != Some(whitebox_producer))
            {
                fail("DEVELOPMENT_WHITEBOX_EVIDENCE_MISMATCH");
            }
            let evidence_time = record_time(&evidence, id);
            if surface == "development_whitebox" {
                earliest_whitebox = Some(match earliest_whitebox {
                    Some(current) if current < evidence_time => current,
                    _ => evidence_time,
                });
                latest_whitebox = Some(match latest_whitebox {
                    Some(current) if current > evidence_time => current,
                    _ => evidence_time,
                });
            } else {
                earliest_blackbox = Some(match earliest_blackbox {
                    Some(current) if current < evidence_time => current,
                    _ => evidence_time,
                });
                latest_blackbox = Some(match latest_blackbox {
                    Some(current) if current > evidence_time => current,
                    _ => evidence_time,
                });
            }
        }
    }
    let observed_at = record_datetime(&validation, "/deployment/observed_at", &validation_name);
    let mut previous_time =
        latest_whitebox.unwrap_or_else(|| fail("MISSING_DEVELOPMENT_WHITEBOX_EVIDENCE"));
    for time in receipt_times {
        if previous_time > time {
            fail("PRE_REVIEW_CAUSAL_ORDER_MISMATCH");
        }
        previous_time = time;
    }
    if record_time(&candidate, &candidate_name)
        > earliest_whitebox.unwrap_or_else(|| fail("MISSING_DEVELOPMENT_WHITEBOX_EVIDENCE"))
        || previous_time
            > earliest_blackbox.unwrap_or_else(|| fail("MISSING_DEPLOYED_BLACKBOX_EVIDENCE"))
        || latest_blackbox.unwrap_or_else(|| fail("MISSING_DEPLOYED_BLACKBOX_EVIDENCE"))
            > observed_at
        || observed_at > record_time(&validation, &validation_name)
    {
        fail("PRE_REVIEW_CAUSAL_ORDER_MISMATCH");
    }
}
