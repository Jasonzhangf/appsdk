use super::*;

pub(super) fn resolve_recorded_mainline_commit(root: &Path, mainline_ref: &str) -> String {
    let exact = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{}^{{commit}}", mainline_ref),
        ])
        .output()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if exact.status.success() {
        let commit = String::from_utf8_lossy(&exact.stdout).trim().to_string();
        if commit.is_empty() {
            fail("MAINLINE_REF_MISSING");
        }
        return commit;
    }

    let branch = mainline_ref
        .strip_prefix("refs/heads/")
        .unwrap_or(mainline_ref);
    let valid = Command::new("git")
        .args(["check-ref-format", "--branch", branch])
        .output()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if !valid.status.success() {
        fail("MAINLINE_REF_MISSING");
    }
    let refs = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["for-each-ref", "--format=%(refname)", "refs/remotes"])
        .output()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if !refs.status.success() {
        fail("VCS_ADAPTER_FAILED");
    }
    let suffix = format!("/{}", branch);
    let refs_text = String::from_utf8_lossy(&refs.stdout);
    let matches = refs_text
        .lines()
        .filter(|reference| reference.ends_with(&suffix))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [reference] => git_value(
            root,
            &["rev-parse", &format!("{}^{{commit}}", reference)],
            "MAINLINE_REF_MISSING",
        ),
        [] => fail("MAINLINE_REF_MISSING"),
        _ => fail("MAINLINE_REF_AMBIGUOUS"),
    }
}

pub(super) fn assert_single_merge_gate(root: &Path, module_id: &str) {
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let effectiveness_name = module_record_name("effectiveness-record", module_id);
    let merge_name = module_record_name("merge-record", module_id);
    let candidate = read_record(root, &candidate_name);
    let effectiveness = read_record(root, &effectiveness_name);
    let merge = read_record(root, &merge_name);
    let candidate_commit = record_str(&candidate, "/head_commit", &candidate_name);
    let candidate_tree = record_str(&candidate, "/tree_hash", &candidate_name);
    let merge_commit = record_str(&merge, "/merge_commit", &merge_name);
    if record_str(&merge, "/issue_id", &merge_name)
        != record_str(&candidate, "/issue_id", &candidate_name)
        || record_str(&merge, "/module_id", &merge_name) != module_id
        || record_str(&merge, "/fix_candidate_id", &merge_name)
            != record_str(&candidate, "/fix_candidate_id", &candidate_name)
        || record_str(&merge, "/effectiveness_id", &merge_name)
            != record_str(&effectiveness, "/effectiveness_id", &effectiveness_name)
        || record_str(&merge, "/candidate_commit", &merge_name) != candidate_commit
        || record_str(&merge, "/candidate_tree_hash", &merge_name) != candidate_tree
        || record_str(&merge, "/merged_tree_hash", &merge_name) != candidate_tree
        || merge.get("change_identity").and_then(Value::as_str) != Some("exact")
        || merge.get("result").and_then(Value::as_str) != Some("pass")
        || record_time(&effectiveness, &effectiveness_name) > record_time(&merge, &merge_name)
    {
        fail("MAINLINE_MERGE_RECORD_MISMATCH");
    }
    if git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", candidate_commit)],
        "FIX_CANDIDATE_COMMIT_MISSING",
    ) != candidate_tree
        || git_value(
            root,
            &["rev-parse", &format!("{}^{{tree}}", merge_commit)],
            "MAINLINE_MERGE_COMMIT_MISSING",
        ) != candidate_tree
    {
        fail("MAINLINE_MERGE_IDENTITY_MISMATCH");
    }
    let candidate_merged = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "merge-base",
            "--is-ancestor",
            candidate_commit,
            merge_commit,
        ])
        .status()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if !candidate_merged.success() {
        fail("FIX_CANDIDATE_NOT_MERGED");
    }
    let mainline_head =
        resolve_recorded_mainline_commit(root, record_str(&merge, "/mainline_ref", &merge_name));
    let merge_on_mainline = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["merge-base", "--is-ancestor", merge_commit, &mainline_head])
        .status()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if !merge_on_mainline.success() {
        fail("RECORDED_MERGE_NOT_ON_MAINLINE");
    }
}

pub(super) fn assert_fix_merge_gate(root: &Path, module_id: &str) {
    let project = read_project(root);
    if assert_development_scenarios(root, &project).multi_worktree_merge_queue {
        assert_parallel_merge_gate(root, module_id);
    } else {
        assert_single_merge_gate(root, module_id);
    }
}

pub(super) fn assert_fix_lifecycle_graph(
    root: &Path,
    module_id: &str,
    review: &Value,
    promotion: &Value,
    artifact: &Value,
) {
    let project = read_project(root);
    let scenarios = assert_development_scenarios(root, &project);
    let parallel_development = scenarios.multi_worktree_merge_queue;
    let collaboration_development = scenarios.multi_worker_collaboration;
    assert_fix_architecture_gate(root, module_id, artifact);
    assert_fix_effectiveness_gate(root, module_id);
    assert_fix_merge_gate(root, module_id);
    let worktree_name = module_record_name("worktree-record", module_id);
    let reproduction_name = module_record_name("reproduction-record", module_id);
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let effectiveness_name = module_record_name("effectiveness-record", module_id);
    let merge_name = module_record_name("merge-record", module_id);
    let worktree = read_record(root, &worktree_name);
    let reproduction = read_record(root, &reproduction_name);
    let candidate = read_record(root, &candidate_name);
    let effectiveness = read_record(root, &effectiveness_name);
    let merge = read_record(root, &merge_name);

    let issue_id = record_str(&worktree, "/issue_id", &worktree_name);
    let scope_hash = record_str(&worktree, "/scope_hash", &worktree_name);
    let base_commit = record_str(&worktree, "/base_commit", &worktree_name);
    let candidate_commit = record_str(&candidate, "/head_commit", &candidate_name);
    let candidate_tree = record_str(&candidate, "/tree_hash", &candidate_name);
    let review_id = record_str(review, "/review_id", "review-record.json");
    assert_worktree_candidate_ancestry(
        root,
        record_str(&worktree, "/head_commit", &worktree_name),
        candidate_commit,
    );
    for (record, name) in [
        (&reproduction, reproduction_name.as_str()),
        (&candidate, candidate_name.as_str()),
        (&effectiveness, effectiveness_name.as_str()),
        (&merge, merge_name.as_str()),
        (review, "review-record.json"),
        (promotion, "promotion-record.json"),
    ] {
        if record_str(record, "/issue_id", name) != issue_id {
            fail("FIX_LIFECYCLE_ISSUE_MISMATCH");
        }
    }
    for (record, name) in [
        (&worktree, worktree_name.as_str()),
        (&reproduction, reproduction_name.as_str()),
        (&candidate, candidate_name.as_str()),
        (&effectiveness, effectiveness_name.as_str()),
        (&merge, merge_name.as_str()),
    ] {
        if record_str(record, "/module_id", name) != module_id {
            fail("FIX_LIFECYCLE_MODULE_MISMATCH");
        }
    }
    if worktree.get("initial_clean") != Some(&Value::Bool(true))
        || worktree.get("final_clean") != Some(&Value::Bool(true))
        || worktree.get("isolation_mode").and_then(Value::as_str) != Some("isolated_worktree")
    {
        fail("FIX_WORKTREE_NOT_CLEAN_ISOLATED");
    }
    if record_str(&reproduction, "/worktree_id", &reproduction_name)
        != record_str(&worktree, "/worktree_id", &worktree_name)
        || record_str(&candidate, "/worktree_id", &candidate_name)
            != record_str(&worktree, "/worktree_id", &worktree_name)
        || record_str(&reproduction, "/base_commit", &reproduction_name) != base_commit
        || record_str(&candidate, "/base_commit", &candidate_name) != base_commit
        || reproduction.get("result").and_then(Value::as_str) != Some("reproduced")
    {
        fail("FIX_REPRODUCTION_GRAPH_MISMATCH");
    }
    if record_str(&candidate, "/scope_hash", &candidate_name) != scope_hash
        || record_str(review, "/review_kind", "review-record.json") != "architecture"
        || record_str(review, "/fix_candidate_id", "review-record.json")
            != record_str(&candidate, "/fix_candidate_id", &candidate_name)
        || record_str(review, "/reviewed_commit", "review-record.json") != candidate_commit
        || record_str(review, "/reviewed_tree_hash", "review-record.json") != candidate_tree
        || record_str(review, "/reviewed_scope_hash", "review-record.json") != scope_hash
        || record_str(review, "/reviewed_diff_hash", "review-record.json")
            != record_str(&candidate, "/diff_hash", &candidate_name)
        || review.get("verdict").and_then(Value::as_str) != Some("pass")
    {
        fail("ARCHITECTURE_REVIEW_INPUT_MISMATCH");
    }
    assert_review_map_bindings(root, module_id, review, "review-record.json");
    if record_str(&effectiveness, "/fix_candidate_id", &effectiveness_name)
        != record_str(&candidate, "/fix_candidate_id", &candidate_name)
        || record_str(
            &effectiveness,
            "/architecture_review_id",
            &effectiveness_name,
        ) != review_id
        || record_str(&effectiveness, "/reviewed_commit", &effectiveness_name) != candidate_commit
        || record_str(&effectiveness, "/reviewed_tree_hash", &effectiveness_name) != candidate_tree
        || effectiveness.get("source_unchanged_since_review") != Some(&Value::Bool(true))
        || effectiveness.get("result").and_then(Value::as_str) != Some("pass")
        || effectiveness.get("reproduction_input_hashes") != reproduction.get("input_hashes")
    {
        fail("POST_ARCHITECTURE_EFFECTIVENESS_MISMATCH");
    }
    let baseline_id = record_str(&reproduction, "/baseline_evidence_id", &reproduction_name);
    if record_str(&effectiveness, "/baseline_evidence_id", &effectiveness_name) != baseline_id {
        fail("POST_ARCHITECTURE_BASELINE_MISMATCH");
    }
    let mut required_evidence = vec![baseline_id.to_string()];
    required_evidence.push(
        record_str(
            &effectiveness,
            "/fixed_replay_evidence_id",
            &effectiveness_name,
        )
        .to_string(),
    );
    for path in [
        "/positive_evidence_ids",
        "/negative_evidence_ids",
        "/blackbox_evidence_ids",
    ] {
        required_evidence.extend(
            record_array(&effectiveness, path, &effectiveness_name)
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| fail("INVALID_EFFECTIVENESS_EVIDENCE_ID"))
                        .to_string()
                }),
        );
    }
    required_evidence.extend(
        record_array(&candidate, "/verification_evidence_ids", &candidate_name)
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .unwrap_or_else(|| fail("INVALID_CANDIDATE_EVIDENCE_ID"))
                    .to_string()
            }),
    );
    required_evidence.sort();
    required_evidence.dedup();
    let mut phases = Vec::new();
    for id in &required_evidence {
        let evidence = evidence_by_id(root, module_id, id);
        assert_evidence_record(&evidence, id, EvidenceValidationMode::Current(Utc::now()));
        if record_str(&evidence, "/evidence_id", id) != id
            || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
            || record_str(&evidence, "/issue_id", id) != issue_id
            || record_str(&evidence, "/scope_hash", id) != scope_hash
            || evidence.get("result").and_then(Value::as_str) != Some("pass")
        {
            fail("FIX_EVIDENCE_SCOPE_MISMATCH");
        }
        phases.push(record_str(&evidence, "/phase", id).to_string());
    }
    for phase in [
        "baseline_reproduction",
        "fix_candidate",
        "positive_intervention",
        "negative_intervention",
        "post_architecture_effectiveness",
    ] {
        if !phases.iter().any(|value| value == phase) {
            fail(format!("MISSING_FIX_EVIDENCE_PHASE:{}", phase));
        }
    }
    for value in record_array(review, "/evidence_ids", "review-record.json") {
        let id = value
            .as_str()
            .unwrap_or_else(|| fail("INVALID_REVIEW_EVIDENCE_ID"));
        let evidence = evidence_by_id(root, module_id, id);
        assert_evidence_record(&evidence, id, EvidenceValidationMode::Current(Utc::now()));
        if record_str(&evidence, "/phase", id) == "post_architecture_effectiveness"
            || record_time(&evidence, id) > record_time(review, "review-record.json")
        {
            fail("ARCHITECTURE_REVIEW_USES_POST_REVIEW_EVIDENCE");
        }
    }
    let merge_commit = record_str(&merge, "/merge_commit", &merge_name);
    if record_str(promotion, "/worktree_record_id", "promotion-record.json")
        != record_str(&worktree, "/worktree_id", &worktree_name)
        || record_str(
            promotion,
            "/reproduction_record_id",
            "promotion-record.json",
        ) != record_str(&reproduction, "/reproduction_id", &reproduction_name)
        || record_str(promotion, "/fix_candidate_id", "promotion-record.json")
            != record_str(&candidate, "/fix_candidate_id", &candidate_name)
        || record_str(
            promotion,
            "/architecture_review_id",
            "promotion-record.json",
        ) != review_id
        || record_str(
            promotion,
            "/effectiveness_record_id",
            "promotion-record.json",
        ) != record_str(&effectiveness, "/effectiveness_id", &effectiveness_name)
        || record_str(promotion, "/merge_record_id", "promotion-record.json")
            != record_str(&merge, "/merge_id", &merge_name)
        || record_str(promotion, "/candidate_commit", "promotion-record.json") != candidate_commit
        || record_str(promotion, "/merged_commit", "promotion-record.json") != merge_commit
        || record_str(promotion, "/source_commit", "promotion-record.json") != merge_commit
    {
        fail("PROMOTION_FIX_LIFECYCLE_REFERENCE_MISMATCH");
    }
    if parallel_development {
        let queue_name = format!(
            "merge-queue-record-{}.json",
            record_str(promotion, "/merge_queue_record_id", "promotion-record.json")
        );
        let integration_name = format!(
            "integration-record-{}.json",
            record_str(promotion, "/integration_record_id", "promotion-record.json")
        );
        let receipt_name = format!(
            "mainline-receipt-record-{}.json",
            record_str(
                promotion,
                "/mainline_receipt_record_id",
                "promotion-record.json",
            )
        );
        let queue = read_record(root, &queue_name);
        let integration = read_record(root, &integration_name);
        let receipt = read_record(root, &receipt_name);
        if record_str(promotion, "/merge_queue_record_id", "promotion-record.json")
            != record_str(&queue, "/queue_entry_id", &queue_name)
            || record_str(promotion, "/integration_record_id", "promotion-record.json")
                != record_str(&integration, "/integration_id", &integration_name)
            || record_str(
                promotion,
                "/mainline_receipt_record_id",
                "promotion-record.json",
            ) != record_str(&receipt, "/receipt_id", &receipt_name)
        {
            fail("PROMOTION_PARALLEL_MERGE_REFERENCE_MISMATCH");
        }
    }
    if collaboration_development {
        assert_collab_live_closure(
            root,
            module_id,
            promotion,
            issue_id,
            record_str(&candidate, "/fix_candidate_id", &candidate_name),
            record_str(artifact, "/artifact_hash", "artifact"),
            scope_hash,
            merge_commit,
        );
    }
    if !(record_time(&worktree, &worktree_name) <= record_time(&reproduction, &reproduction_name)
        && record_time(&reproduction, &reproduction_name)
            <= record_time(&candidate, &candidate_name)
        && record_time(&candidate, &candidate_name) <= record_time(review, "review-record.json")
        && record_time(review, "review-record.json")
            <= record_time(&effectiveness, &effectiveness_name)
        && record_time(&effectiveness, &effectiveness_name) <= record_time(&merge, &merge_name)
        && record_time(&merge, &merge_name) <= record_time(promotion, "promotion-record.json"))
    {
        fail("FIX_LIFECYCLE_ORDER_INVALID");
    }
    assert_bug_tracker_solution_evidence(root, issue_id, promotion);
}

pub(super) fn assert_record_graph(
    root: &Path,
    module_id: Option<&str>,
    artifact: &Value,
    require_freeze: bool,
) {
    assert_record_graph_mode(
        root,
        module_id,
        artifact,
        require_freeze,
        true,
        false,
        EvidenceValidationMode::Current(Utc::now()),
    );
}

// Frozen rehydration republishes an already accepted historical artifact. It
// must validate the immutable record graph and freeze bindings, but it must
// not re-run delivery-only gates (including current bug-triage evidence) that
// were introduced after the historical producer ran.
pub(super) fn assert_historical_frozen_record_graph(root: &Path, module_id: &str, artifact: &Value) {
    // Frozen modules are immutable historical publications. Their legacy
    // predecessor binding is not the current development/promotion contract;
    // validate the publication graph without requiring that old Active
    // projection to be present or byte-identical to a later record.
    // The merge/mainline binding remains authoritative and must still be
    // resolved before a historical publication is rehydrated.
    assert_fix_merge_gate(root, module_id);
    assert_record_graph_mode(
        root,
        Some(module_id),
        artifact,
        true,
        false,
        true,
        EvidenceValidationMode::Historical,
    );
}

pub(super) fn assert_record_graph_mode(
    root: &Path,
    module_id: Option<&str>,
    artifact: &Value,
    require_freeze: bool,
    enforce_current_lifecycle: bool,
    allow_legacy_rehydrate_bindings: bool,
    validation: EvidenceValidationMode,
) {
    if let Some(module_id) = module_id {
        let _ = read_record(root, &module_record_name("worktree-record", module_id));
    }
    let evidence_name = module_id
        .map(|id| module_record_name("evidence-record", id))
        .unwrap_or_else(|| "evidence-record.json".into());
    let review_name = module_id
        .map(|id| module_record_name("review-record", id))
        .unwrap_or_else(|| "review-record.json".into());
    let promotion_name = module_id
        .map(|id| module_record_name("promotion-record", id))
        .unwrap_or_else(|| "promotion-record.json".into());
    let evidence = read_record(root, &evidence_name);
    let review = read_record(root, &review_name);
    let promotion = read_record(root, &promotion_name);
    let historical_freeze = if matches!(&validation, EvidenceValidationMode::Historical) {
        let module_id = module_id.unwrap_or_else(|| fail("HISTORICAL_MODULE_REQUIRED"));
        let freeze_name = freeze_record_name(module_id);
        Some((freeze_name.clone(), read_record(root, &freeze_name)))
    } else {
        None
    };
    let validation = match validation {
        EvidenceValidationMode::Current(admission_time) => {
            EvidenceValidationMode::Current(admission_time)
        }
        EvidenceValidationMode::Historical => {
            let (freeze_name, freeze) = historical_freeze
                .as_ref()
                .unwrap_or_else(|| fail("HISTORICAL_FREEZE_REQUIRED"));
            let as_of = [
                record_time(&review, &review_name),
                record_time(&promotion, &promotion_name),
                record_time(freeze, freeze_name),
            ]
            .into_iter()
            .max()
            .unwrap_or_else(|| fail("HISTORICAL_PUBLICATION_TIME_MISSING"));
            EvidenceValidationMode::HistoricalAt(as_of)
        }
        EvidenceValidationMode::HistoricalAt(as_of) => EvidenceValidationMode::HistoricalAt(as_of),
    };
    assert_record_schema(
        &evidence,
        &review,
        &promotion,
        allow_legacy_rehydrate_bindings,
        validation,
    );
    if enforce_current_lifecycle {
        if let Some(module_id) = module_id {
            assert_fix_lifecycle_graph(root, module_id, &review, &promotion, artifact);
        }
    }
    let cleanup_id = record_str(
        &promotion,
        "/playground_cleanup_record_id",
        "promotion-record.json",
    );
    let cleanup = read_record(root, &format!("playground-cleanup-{}.json", cleanup_id));
    if record_str(&cleanup, "/cleanup_id", "playground-cleanup-record") != cleanup_id
        || !matches!(
            cleanup.get("disposition").and_then(Value::as_str),
            Some("archive_then_remove" | "remove" | "retain_open")
        )
    {
        fail("INVALID_PLAYGROUND_CLEANUP_RECORD");
    }
    let evidence_id = record_str(&evidence, "/evidence_id", "evidence-record.json");
    let issue_id = record_str(&evidence, "/issue_id", "evidence-record.json");
    let experiment_id = record_str(&evidence, "/experiment_id", "evidence-record.json");
    if record_str(&evidence, "/result", "evidence-record.json") != "pass"
        || record_str(&review, "/verdict", "review-record.json") != "pass"
    {
        fail("PROMOTION_EVIDENCE_NOT_PASSED");
    }
    if record_str(&review, "/issue_id", "review-record.json") != issue_id
        || record_str(&promotion, "/issue_id", "promotion-record.json") != issue_id
        || record_str(&promotion, "/experiment_id", "promotion-record.json") != experiment_id
    {
        fail("RECORD_GRAPH_SCOPE_MISMATCH");
    }
    if record_str(&review, "/promotion_id", "review-record.json")
        != record_str(&promotion, "/promotion_id", "promotion-record.json")
        || record_str(&review, "/review_id", "review-record.json")
            != record_str(&promotion, "/review_id", "promotion-record.json")
    {
        fail("RECORD_GRAPH_REFERENCE_MISMATCH");
    }
    let review_evidence_ids = record_array(&review, "/evidence_ids", "review-record.json");
    let promotion_evidence_ids = record_array(&promotion, "/evidence_ids", "promotion-record.json");
    if !review_evidence_ids
        .iter()
        .any(|id| id.as_str() == Some(evidence_id))
        || !promotion_evidence_ids
            .iter()
            .any(|id| id.as_str() == Some(evidence_id))
    {
        fail("RECORD_GRAPH_EVIDENCE_REFERENCE_MISMATCH");
    }
    if let Some(module_id) = module_id {
        if record_str(&promotion, "/module_id", "promotion-record.json") != module_id
            || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
        {
            fail("RECORD_GRAPH_MODULE_MISMATCH");
        }
    }
    let artifact_hash = record_str(artifact, "/artifact_hash", "artifact");
    if record_str(&review, "/reviewed_artifact_hash", "review-record.json") != artifact_hash
        || record_str(&promotion, "/artifact_hash", "promotion-record.json") != artifact_hash
    {
        fail("RECORD_GRAPH_ARTIFACT_MISMATCH");
    }
    if record_str(&review, "/reviewed_commit", "review-record.json")
        != record_str(&promotion, "/source_commit", "promotion-record.json")
        || record_str(&evidence, "/source_commit", "evidence-record.json")
            != record_str(&promotion, "/source_commit", "promotion-record.json")
        || record_str(&review, "/reviewed_scope_hash", "review-record.json")
            != record_str(&promotion, "/scope_hash", "promotion-record.json")
        || record_str(&evidence, "/scope_hash", "evidence-record.json")
            != record_str(&promotion, "/scope_hash", "promotion-record.json")
    {
        fail("RECORD_GRAPH_INPUT_MISMATCH");
    }
    let gates = record_array(
        &promotion,
        "/required_gate_results",
        "promotion-record.json",
    );
    if gates
        .iter()
        .any(|gate| gate.get("result").and_then(Value::as_str) != Some("pass"))
    {
        fail("PROMOTION_GATE_NOT_PASSED");
    }
    if require_freeze {
        let module_id = module_id.unwrap_or_else(|| fail("FREEZE_RECORD_MODULE_REQUIRED"));
        let project = read_project(root);
        let module = project
            .get("modules")
            .and_then(Value::as_array)
            .and_then(|modules| {
                modules.iter().find(|module| {
                    module.get("module_id").and_then(Value::as_str) == Some(module_id)
                })
            })
            .unwrap_or_else(|| fail("MODULE_NOT_FOUND"));
        let (regression, regression_hash) =
            assert_regression_report(root, module_id, module, &promotion, artifact);
        let freeze_name = freeze_record_name(module_id);
        let freeze = read_record(root, &freeze_name);
        let active_root = contract_root(root, &project, "/governance/active_root");
        let active_version = record_str(&freeze, "/active_version", &freeze_name);
        let active_artifact = active_root
            .join(module_id)
            .join(active_version)
            .join("artifact.json");
        if active_artifact.is_file()
            && !fs::symlink_metadata(&active_artifact)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
        {
            let active_value: Value = serde_json::from_str(
                &fs::read_to_string(&active_artifact)
                    .unwrap_or_else(|_| fail("ACTIVE_ARTIFACT_MISSING")),
            )
            .unwrap_or_else(|_| fail("INVALID_ACTIVE_ARTIFACT"));
            if record_str(&active_value, "/artifact_hash", "active_artifact")
                != record_str(&freeze, "/library_hash", &freeze_name)
            {
                fail("FREEZE_ACTIVE_HASH_MISMATCH");
            }
        }
        for path in [
            "/freeze_id",
            "/issue_id",
            "/module_id",
            "/promotion_id",
            "/promotion_record_hash",
            "/artifact_record_id",
            "/regression_report_id",
            "/regression_report_hash",
            "/source_commit_or_tag",
            "/active_version",
            "/library_hash",
            "/public_api_hash",
            "/review_id",
            "/created_at",
            "/clean_scope/base_commit",
            "/clean_scope/generated_policy",
        ] {
            record_str(&freeze, path, &freeze_name);
        }
        for path in ["/previous_active_immutable", "/git_clean"] {
            if freeze
                .get(path.trim_start_matches('/'))
                .and_then(Value::as_bool)
                .is_none()
            {
                fail(format!("INVALID_RECORD:{}:{}", freeze_name, path));
            }
        }
        let clean_scope = freeze
            .get("clean_scope")
            .and_then(Value::as_object)
            .unwrap_or_else(|| fail(format!("INVALID_RECORD:{}:/clean_scope", freeze_name)));
        for key in ["changed_paths", "ignored_paths"] {
            if clean_scope.get(key).and_then(Value::as_array).is_none() {
                fail(format!(
                    "INVALID_RECORD:{}/clean_scope/{}",
                    freeze_name, key
                ));
            }
        }
        let owners = freeze
            .get("owners")
            .and_then(Value::as_object)
            .unwrap_or_else(|| fail(format!("INVALID_RECORD:{}:/owners", freeze_name)));
        for key in [
            "vcs",
            "compiler",
            "api_extractor",
            "review",
            "artifact_registry",
        ] {
            if owners
                .get(key)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .is_none()
            {
                fail(format!("INVALID_RECORD:{}:/owners/{}", freeze_name, key));
            }
        }
        if record_str(&freeze, "/module_id", &freeze_name) != module_id {
            fail("FREEZE_RECORD_MODULE_MISMATCH");
        }
        if record_str(&freeze, "/promotion_id", &freeze_name)
            != record_str(&promotion, "/promotion_id", "promotion-record.json")
        {
            fail("FREEZE_RECORD_PROMOTION_MISMATCH");
        }
        if record_str(&freeze, "/review_id", &freeze_name)
            != record_str(&review, "/review_id", "review-record.json")
        {
            fail("FREEZE_RECORD_REVIEW_MISMATCH");
        }
        if record_str(&freeze, "/library_hash", &freeze_name) != artifact_hash {
            fail("FREEZE_RECORD_LIBRARY_HASH_MISMATCH");
        }
        if record_str(&freeze, "/active_version", &freeze_name)
            != record_str(&promotion, "/new_active_version", "promotion-record.json")
        {
            fail("FREEZE_RECORD_VERSION_MISMATCH");
        }
        if record_str(&freeze, "/artifact_record_id", &freeze_name) != evidence_id {
            fail("FREEZE_RECORD_ARTIFACT_RECORD_MISMATCH");
        }
        if record_str(&freeze, "/regression_report_id", &freeze_name)
            != record_str(
                &regression,
                "/regression_report_id",
                "regression-report.json",
            )
            || record_str(&freeze, "/regression_report_hash", &freeze_name) != regression_hash
        {
            fail("FREEZE_RECORD_REGRESSION_REPORT_MISMATCH");
        }
        if record_str(&freeze, "/promotion_record_hash", &freeze_name)
            != sha256(&canonical(&promotion))
        {
            fail("FREEZE_RECORD_PROMOTION_HASH_MISMATCH");
        }
        if record_str(&freeze, "/public_api_hash", &freeze_name)
            != record_str(&promotion, "/public_api_hash", "promotion-record.json")
        {
            fail("FREEZE_RECORD_PUBLIC_API_HASH_MISMATCH");
        }
        if record_str(&freeze, "/source_commit_or_tag", &freeze_name).is_empty()
            || record_str(&freeze, "/public_api_hash", &freeze_name).is_empty()
        {
            fail("FREEZE_RECORD_REQUIRED_FIELD_MISMATCH");
        }
        if record_str(&freeze, "/source_commit_or_tag", &freeze_name)
            != record_str(&promotion, "/source_commit", "promotion-record.json")
        {
            fail("FREEZE_RECORD_SOURCE_COMMIT_MISMATCH");
        }
        if freeze.get("git_clean") != Some(&Value::Bool(true)) {
            fail("FREEZE_REQUIREMENTS_NOT_MET");
        }
        let previous_version = freeze
            .get("previous_active_version")
            .and_then(Value::as_str);
        let previous_immutable = freeze
            .get("previous_active_immutable")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| fail("FREEZE_REQUIREMENTS_NOT_MET"));
        if previous_immutable != previous_version.is_some() {
            fail("FREEZE_PREVIOUS_ACTIVE_CLAIM_MISMATCH");
        }
        if let Some(version_base) = module.get("version_base").filter(|value| !value.is_null()) {
            if freeze
                .get("previous_active_version")
                .and_then(Value::as_str)
                != version_base
                    .get("previous_active_version")
                    .and_then(Value::as_str)
                || promotion
                    .get("previous_active_version")
                    .and_then(Value::as_str)
                    != version_base
                        .get("previous_active_version")
                        .and_then(Value::as_str)
                || promotion.get("new_active_version").and_then(Value::as_str)
                    != version_base
                        .get("new_active_version")
                        .and_then(Value::as_str)
                || promotion.get("base_artifact_hash").and_then(Value::as_str)
                    != version_base
                        .get("base_artifact_hash")
                        .and_then(Value::as_str)
                || promotion.get("base_commit").and_then(Value::as_str)
                    != version_base
                        .get("base_source_commit")
                        .and_then(Value::as_str)
            {
                fail("MODULE_VERSION_RECORD_MISMATCH");
            }
        }
        if let Some(previous) = freeze
            .get("previous_active_version")
            .and_then(Value::as_str)
        {
            let project = read_project(root);
            let active_root = contract_root(root, &project, "/governance/active_root");
            let previous_path = active_root.join(module_id).join(previous);
            assert_no_symlink_components(root, &previous_path, "previous_active");
            if !previous_path.is_dir() {
                if !allow_legacy_rehydrate_bindings {
                    fail("PREVIOUS_ACTIVE_MISSING");
                }
                // A legacy frozen checkout may retain the immutable target
                // archive without publishing its predecessor Active
                // projection. Rehydrate validates the target archive and
                // does not invent the missing predecessor.
            }
            if previous_path.is_dir() {
                let artifact = previous_path.join("artifact.json");
                if fs::symlink_metadata(&artifact)
                    .map(|metadata| metadata.file_type().is_symlink())
                    .unwrap_or(false)
                {
                    fail("PREVIOUS_ACTIVE_SYMLINK");
                }
                if !artifact.is_file() {
                    fail("PREVIOUS_ACTIVE_ARTIFACT_MISSING");
                }
                let previous_value: Value = serde_json::from_str(
                    &fs::read_to_string(&artifact)
                        .unwrap_or_else(|_| fail("PREVIOUS_ACTIVE_ARTIFACT_MISSING")),
                )
                .unwrap_or_else(|_| fail("INVALID_PREVIOUS_ACTIVE_ARTIFACT"));
                if previous_value
                    .get("module_id")
                    .and_then(Value::as_str)
                    .is_some()
                {
                    let module = project
                        .get("modules")
                        .and_then(Value::as_array)
                        .and_then(|modules| {
                            modules.iter().find(|module| {
                                module.get("module_id").and_then(Value::as_str) == Some(module_id)
                            })
                        })
                        .unwrap_or_else(|| fail("MODULE_NOT_FOUND"));
                    previous_active_matches_module(module, &previous_value);
                } else {
                    assert_artifact_matches(&project, &previous_value);
                }
                let previous_hash = record_str(
                    &previous_value,
                    "/artifact_hash",
                    "previous_active_artifact",
                );
                let promotion =
                    read_record(root, &module_record_name("promotion-record", module_id));
                if promotion
                    .pointer("/base_artifact_hash")
                    .and_then(Value::as_str)
                    != Some(previous_hash)
                    && !allow_legacy_rehydrate_bindings
                {
                    fail("PREVIOUS_ACTIVE_HASH_MISMATCH");
                }
                if record_str(&previous_value, "/module_id", "previous_active_artifact")
                    != module_id
                {
                    fail("PREVIOUS_ACTIVE_MODULE_MISSING");
                }
            }
        }
    }
}
