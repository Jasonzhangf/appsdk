use super::*;

pub(super) fn verify_review_admission(root: &Path, module_id: &str) {
    assert_project_root_safe(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    assert_goal_confirmed(root);
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
    let stage = record_str(module, "/stage", "module");
    if matches!(stage, "frozen" | "retired") {
        // Frozen and retired modules are immutable publications. Their
        // generated checkout projection may have been intentionally removed,
        // so review admission must resolve the artifact from the immutable
        // historical archive and validate the publication graph only.
        verify_internal(root, true, false, false, false);
        let artifact = read_historical_module_artifact(root, &project, module, module_id);
        if !matches!(
            artifact.get("stage").and_then(Value::as_str),
            Some("frozen" | "retired")
        ) {
            fail(format!(
                "HISTORICAL_MODULE_ARTIFACT_STAGE_MISMATCH:{}",
                module_id
            ));
        }
        assert_historical_frozen_record_graph(root, module_id, &artifact);
        println!(
            "{{\"ok\":true,\"gate\":\"review_admission\",\"module_id\":\"{}\",\"mode\":\"historical\"}}",
            module_id
        );
        return;
    }

    let artifact = assert_review_author_readiness(root, module_id);
    // At architecture_stable the full verifier owns this check. Before that
    // stage, admission must still reject a supplied stale PASS; context
    // assembly deliberately does not depend on that downstream record.
    if stage != "architecture_stable"
        && lifecycle_chain_read_record_if_present(root, module_id, "review-record")
            .is_some_and(|review| review["verdict"] == "pass")
    {
        assert_fix_architecture_gate(root, module_id, &artifact);
    }
    verify_internal(root, true, true, true, false);
    println!(
        "{{\"ok\":true,\"gate\":\"review_admission\",\"module_id\":\"{}\"}}",
        module_id
    );
}

pub(super) fn explain_review_admission_preflight(root: &Path, module_id: &str, module: &Value) {
    let records_root = root.join(".appsdk").join("records");
    let evidence_root = records_root.join("evidence").join(module_id);
    let mut required = vec![
        (
            "fix_candidate",
            module_record_name("fix-candidate-record", module_id),
            "project::lifecycle_adapter",
            "produce from the clean owner worktree and candidate commit",
        ),
        (
            "development_whitebox",
            "evidence/<module>/whitebox-1.json".to_string(),
            "project::whitebox_adapter",
            "run the declared development whitebox and persist its actual result",
        ),
    ];
    let deployment_operations = module_deployment_operations(module);
    if deployment_operations.contains(&"install") {
        required.push((
            "deployment_install",
            "evidence/<module>/install-1.json".to_string(),
            "project::deployment_adapter",
            "install the exact candidate artifact and persist the real receipt",
        ));
    }
    if deployment_operations.contains(&"restart") {
        required.push((
            "deployment_restart",
            "evidence/<module>/restart-1.json".to_string(),
            "project::deployment_adapter",
            "restart the exact installed artifact and persist the real receipt",
        ));
    }
    required.extend([
        (
            "deployed_blackbox",
            "evidence/<module>/blackbox-1.json".to_string(),
            "project::blackbox_adapter",
            "exercise the deployed public entrypoint and persist the actual result",
        ),
        (
            "pre_review_validation",
            module_record_name("pre-review-validation-record", module_id),
            "project::lifecycle_adapter",
            "bind the disjoint evidence IDs and causal timestamps after all gates pass",
        ),
    ]);
    let missing: Vec<Value> = required
        .iter()
        .filter(|(_, relative, _, _)| {
            let path = if relative.starts_with("evidence/") {
                evidence_root.join(relative.strip_prefix("evidence/<module>/").unwrap())
            } else {
                records_root.join(relative)
            };
            !path.is_file()
        })
        .map(|(kind, relative, producer, next)| {
            serde_json::json!({
                "kind": kind,
                "path": relative,
                "producer": producer,
                "next": next
            })
        })
        .collect();
    if missing.is_empty()
        || !missing.iter().any(|entry| {
            matches!(
                entry.get("kind").and_then(Value::as_str),
                Some("fix_candidate" | "pre_review_validation")
            )
        })
    {
        return;
    }
    let present: Vec<String> = required
        .iter()
        .filter_map(|(_, relative, _, _)| {
            let path = if relative.starts_with("evidence/") {
                evidence_root.join(relative.strip_prefix("evidence/<module>/").unwrap())
            } else {
                records_root.join(relative)
            };
            path.is_file().then(|| relative.clone())
        })
        .collect();
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "error": "REVIEW_ADMISSION_BLOCKED",
            "module_id": module_id,
            "admission": "blocked",
            "missing": missing,
            "present": present,
            "retry_allowed": false,
            "idempotent": true,
            "next": "enable or run the declared project adapters; let each adapter persist real evidence; rerun the same admission command",
            "forbidden": [
                "do not hand-create lifecycle records",
                "do not copy records from another project or version",
                "do not invent hashes, receipts, timestamps, or producer identities",
                "do not retry this command until the listed external state changes"
            ]
        }))
        .unwrap()
    );
    std::process::exit(1);
}

pub(super) fn assert_review_map_bindings(
    root: &Path,
    module_id: &str,
    review: &Value,
    review_name: &str,
) {
    let bindings = [
        ("resource-map.json", "/resource_map_hash"),
        ("function-map.json", "/function_map_hash"),
        ("mainline-call-map.json", "/mainline_call_map_hash"),
        ("verification-map.json", "/verification_map_hash"),
    ];
    if bindings.iter().all(|(map, path)| {
        record_str(review, path, review_name)
            == file_sha256(&root.join(".appsdk/maps").join(map), map)
    }) {
        return;
    }
    let project = read_project(root);
    let module = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    let stage = module
        .get("stage")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("INVALID_MODULE_CONTRACT"));
    let review_id = record_str(review, "/review_id", review_name);
    let migration = SDK_MAP_MIGRATION_STEPS
        .iter()
        .rev()
        .find_map(|step| {
            if !sdk_map_migration_root(root, step)
                .join("record.json")
                .is_file()
            {
                return None;
            }
            let migration =
                assert_sdk_migration_record(root, step, sdk_map_migration_checks_live_target(step))
                    .unwrap_or_else(|| fail("ARCHITECTURE_REVIEW_MAP_STALE"));
            let retained_review = |key: &str| {
                migration
                    .get(key)
                    .and_then(Value::as_array)
                    .is_some_and(|reviews| {
                        reviews.iter().any(|entry| {
                            entry.get("module_id").and_then(Value::as_str) == Some(module_id)
                                && entry.get("review_id").and_then(Value::as_str) == Some(review_id)
                        })
                    })
            };
            if !retained_review("frozen_reviews") && !retained_review("legacy_reconciled_reviews") {
                return None;
            }
            let map_bindings_match = bindings.iter().all(|(map, path)| {
                let expected = record_str(review, path, review_name);
                let migration_entry = migration
                    .get("maps")
                    .and_then(Value::as_array)
                    .and_then(|maps| {
                        maps.iter()
                            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(*map))
                    })
                    .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
                expected == record_str(migration_entry, "/source_digest", "sdk-map-migration")
            });
            map_bindings_match.then_some(migration)
        })
        .unwrap_or_else(|| fail("ARCHITECTURE_REVIEW_MAP_STALE"));
    let retained_review = |key: &str| {
        migration
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|reviews| {
                reviews.iter().any(|entry| {
                    entry.get("module_id").and_then(Value::as_str) == Some(module_id)
                        && entry.get("review_id").and_then(Value::as_str) == Some(review_id)
                })
            })
    };
    if record_time(review, review_name) > record_time(&migration, "sdk-migration-record")
        || (!retained_review("frozen_reviews") && !retained_review("legacy_reconciled_reviews"))
        || (!matches!(stage, "frozen" | "retired") && !retained_review("legacy_reconciled_reviews"))
    {
        fail("ARCHITECTURE_REVIEW_MAP_STALE");
    }
}

pub(super) fn assert_fix_architecture_gate(root: &Path, module_id: &str, artifact: &Value) {
    let worktree_name = module_record_name("worktree-record", module_id);
    let reproduction_name = module_record_name("reproduction-record", module_id);
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let review_name = module_record_name("review-record", module_id);
    let worktree = read_record(root, &worktree_name);
    let reproduction = read_record(root, &reproduction_name);
    let candidate = read_record(root, &candidate_name);
    let review = read_record(root, &review_name);
    assert_pre_review_validation_gate(root, module_id, artifact);
    let validation_name = module_record_name("pre-review-validation-record", module_id);
    let validation = read_record(root, &validation_name);
    if record_str(&review, "/pre_review_validation_id", &review_name)
        != record_str(&validation, "/validation_id", &validation_name)
        || record_time(&validation, &validation_name) > record_time(&review, &review_name)
    {
        fail("PRE_REVIEW_VALIDATION_MISMATCH");
    }
    let issue_id = record_str(&worktree, "/issue_id", &worktree_name);
    let scope_hash = record_str(&worktree, "/scope_hash", &worktree_name);
    let candidate_commit = record_str(&candidate, "/head_commit", &candidate_name);
    let candidate_tree = record_str(&candidate, "/tree_hash", &candidate_name);
    assert_worktree_candidate_ancestry(
        root,
        record_str(&worktree, "/head_commit", &worktree_name),
        candidate_commit,
    );
    if record_str(&worktree, "/module_id", &worktree_name) != module_id
        || record_str(&reproduction, "/module_id", &reproduction_name) != module_id
        || record_str(&candidate, "/module_id", &candidate_name) != module_id
        || record_str(&reproduction, "/issue_id", &reproduction_name) != issue_id
        || record_str(&candidate, "/issue_id", &candidate_name) != issue_id
        || record_str(&review, "/issue_id", &review_name) != issue_id
    {
        fail("FIX_ARCHITECTURE_SCOPE_MISMATCH");
    }
    if worktree.get("initial_clean") != Some(&Value::Bool(true))
        || worktree.get("final_clean") != Some(&Value::Bool(true))
        || worktree.get("isolation_mode").and_then(Value::as_str) != Some("isolated_worktree")
    {
        fail("FIX_WORKTREE_NOT_CLEAN_ISOLATED");
    }
    assert_bug_tracker_triage_evidence(&worktree, issue_id, Some(root), true);
    if record_str(&reproduction, "/worktree_id", &reproduction_name)
        != record_str(&worktree, "/worktree_id", &worktree_name)
        || record_str(&candidate, "/worktree_id", &candidate_name)
            != record_str(&worktree, "/worktree_id", &worktree_name)
        || record_str(&reproduction, "/base_commit", &reproduction_name)
            != record_str(&worktree, "/base_commit", &worktree_name)
        || record_str(&candidate, "/base_commit", &candidate_name)
            != record_str(&worktree, "/base_commit", &worktree_name)
        || reproduction.get("result").and_then(Value::as_str) != Some("reproduced")
    {
        fail("FIX_REPRODUCTION_GRAPH_MISMATCH");
    }
    if record_str(&candidate, "/scope_hash", &candidate_name) != scope_hash
        || record_str(&review, "/review_kind", &review_name) != "architecture"
        || record_str(&review, "/fix_candidate_id", &review_name)
            != record_str(&candidate, "/fix_candidate_id", &candidate_name)
        || record_str(&review, "/reviewed_commit", &review_name) != candidate_commit
        || record_str(&review, "/reviewed_tree_hash", &review_name) != candidate_tree
        || record_str(&review, "/reviewed_diff_hash", &review_name)
            != record_str(&candidate, "/diff_hash", &candidate_name)
        || record_str(&review, "/reviewed_scope_hash", &review_name) != scope_hash
        || record_str(&review, "/reviewed_artifact_hash", &review_name)
            != record_str(artifact, "/artifact_hash", "artifact")
        || review.get("verdict").and_then(Value::as_str) != Some("pass")
    {
        fail("ARCHITECTURE_REVIEW_INPUT_MISMATCH");
    }
    assert_review_map_bindings(root, module_id, &review, &review_name);
    let baseline_id = record_str(&reproduction, "/baseline_evidence_id", &reproduction_name);
    let baseline = evidence_by_id(root, module_id, baseline_id);
    assert_evidence_record(
        &baseline,
        baseline_id,
        EvidenceValidationMode::Current(Utc::now()),
    );
    if record_str(&baseline, "/phase", baseline_id) != "baseline_reproduction"
        || baseline.get("result").and_then(Value::as_str) != Some("pass")
        || baseline.get("input_hashes") != reproduction.get("input_hashes")
    {
        fail("BASELINE_REPRODUCTION_EVIDENCE_MISMATCH");
    }
    let mut candidate_phases = Vec::new();
    for value in record_array(&candidate, "/verification_evidence_ids", &candidate_name) {
        let id = value
            .as_str()
            .unwrap_or_else(|| fail("INVALID_CANDIDATE_EVIDENCE_ID"));
        let evidence = evidence_by_id(root, module_id, id);
        assert_evidence_record(&evidence, id, EvidenceValidationMode::Current(Utc::now()));
        if record_str(&evidence, "/issue_id", id) != issue_id
            || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
            || record_str(&evidence, "/scope_hash", id) != scope_hash
            || record_str(&evidence, "/source_commit", id) != candidate_commit
            || evidence.get("result").and_then(Value::as_str) != Some("pass")
            || record_time(&evidence, id) > record_time(&review, &review_name)
        {
            fail("FIX_CANDIDATE_EVIDENCE_MISMATCH");
        }
        candidate_phases.push(record_str(&evidence, "/phase", id).to_string());
    }
    for phase in [
        "fix_candidate",
        "positive_intervention",
        "negative_intervention",
    ] {
        if !candidate_phases.iter().any(|value| value == phase) {
            fail(format!("MISSING_FIX_EVIDENCE_PHASE:{}", phase));
        }
    }
    for value in record_array(&review, "/evidence_ids", &review_name) {
        let id = value
            .as_str()
            .unwrap_or_else(|| fail("INVALID_REVIEW_EVIDENCE_ID"));
        let evidence = evidence_by_id(root, module_id, id);
        assert_evidence_record(&evidence, id, EvidenceValidationMode::Current(Utc::now()));
        if record_str(&evidence, "/issue_id", id) != issue_id
            || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
            || record_str(&evidence, "/scope_hash", id) != scope_hash
            || record_str(&evidence, "/source_commit", id) != candidate_commit
            || evidence.get("result").and_then(Value::as_str) != Some("pass")
            || record_str(&evidence, "/phase", id) == "post_architecture_effectiveness"
            || record_time(&evidence, id) > record_time(&review, &review_name)
        {
            fail("ARCHITECTURE_REVIEW_EVIDENCE_MISMATCH");
        }
    }
    assert_lifecycle_chain_review_identity_or_frozen_legacy(root, module_id, &review);
    let project = read_project(root);
    let historical = project["modules"].as_array().is_some_and(|modules| {
        modules.iter().any(|module| {
            module["module_id"].as_str() == Some(module_id)
                && matches!(module["stage"].as_str(), Some("frozen" | "retired"))
        })
    });
    if !historical {
        let binding = review
            .pointer("/project_bindings/requirements_review")
            .unwrap_or_else(|| fail("ARCHITECTURE_REQUIREMENTS_REVIEW_MISSING"));
        assert_review_requirements_binding(root, module_id, binding);
    }
    if git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", candidate_commit)],
        "FIX_CANDIDATE_COMMIT_MISSING",
    ) != candidate_tree
        || !(record_time(&worktree, &worktree_name)
            <= record_time(&reproduction, &reproduction_name)
            && record_time(&reproduction, &reproduction_name)
                <= record_time(&candidate, &candidate_name)
            && record_time(&candidate, &candidate_name) <= record_time(&review, &review_name))
    {
        fail("FIX_ARCHITECTURE_ORDER_OR_IDENTITY_INVALID");
    }
}

pub(super) fn assert_fix_effectiveness_gate(root: &Path, module_id: &str) {
    let reproduction_name = module_record_name("reproduction-record", module_id);
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let review_name = module_record_name("review-record", module_id);
    let effectiveness_name = module_record_name("effectiveness-record", module_id);
    let reproduction = read_record(root, &reproduction_name);
    let candidate = read_record(root, &candidate_name);
    let review = read_record(root, &review_name);
    let effectiveness = read_record(root, &effectiveness_name);
    let validation_name = module_record_name("pre-review-validation-record", module_id);
    let validation = read_record(root, &validation_name);
    let issue_id = record_str(&candidate, "/issue_id", &candidate_name);
    let scope_hash = record_str(&candidate, "/scope_hash", &candidate_name);
    let candidate_commit = record_str(&candidate, "/head_commit", &candidate_name);
    let candidate_tree = record_str(&candidate, "/tree_hash", &candidate_name);
    if record_str(&effectiveness, "/issue_id", &effectiveness_name) != issue_id
        || record_str(&effectiveness, "/module_id", &effectiveness_name) != module_id
        || record_str(&effectiveness, "/fix_candidate_id", &effectiveness_name)
            != record_str(&candidate, "/fix_candidate_id", &candidate_name)
        || record_str(
            &effectiveness,
            "/architecture_review_id",
            &effectiveness_name,
        ) != record_str(&review, "/review_id", &review_name)
        || record_str(&effectiveness, "/reviewed_commit", &effectiveness_name) != candidate_commit
        || record_str(&effectiveness, "/reviewed_tree_hash", &effectiveness_name) != candidate_tree
        || effectiveness.get("reproduction_input_hashes") != reproduction.get("input_hashes")
        || effectiveness.get("source_unchanged_since_review") != Some(&Value::Bool(true))
        || effectiveness.get("result").and_then(Value::as_str) != Some("pass")
        || record_time(&review, &review_name) > record_time(&effectiveness, &effectiveness_name)
    {
        fail("POST_ARCHITECTURE_EFFECTIVENESS_MISMATCH");
    }
    let baseline_id = record_str(&reproduction, "/baseline_evidence_id", &reproduction_name);
    if record_str(&effectiveness, "/baseline_evidence_id", &effectiveness_name) != baseline_id {
        fail("POST_ARCHITECTURE_BASELINE_MISMATCH");
    }
    let mut ids = vec![record_str(
        &effectiveness,
        "/fixed_replay_evidence_id",
        &effectiveness_name,
    )
    .to_string()];
    for path in [
        "/positive_evidence_ids",
        "/negative_evidence_ids",
        "/blackbox_evidence_ids",
    ] {
        ids.extend(
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
    ids.sort();
    ids.dedup();
    let mut phases = Vec::new();
    for id in ids {
        let evidence = evidence_by_id(root, module_id, &id);
        assert_evidence_record(&evidence, &id, EvidenceValidationMode::Current(Utc::now()));
        if record_str(&evidence, "/issue_id", &id) != issue_id
            || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
            || record_str(&evidence, "/scope_hash", &id) != scope_hash
            || record_str(&evidence, "/source_commit", &id) != candidate_commit
            || evidence.get("result").and_then(Value::as_str) != Some("pass")
            || record_time(&evidence, &id) < record_time(&candidate, &candidate_name)
            || record_time(&evidence, &id) > record_time(&effectiveness, &effectiveness_name)
        {
            fail("POST_ARCHITECTURE_EFFECTIVENESS_EVIDENCE_MISMATCH");
        }
        if record_time(&evidence, &id) < record_time(&review, &review_name) {
            let bound_before_review =
                record_array(&candidate, "/verification_evidence_ids", &candidate_name)
                    .iter()
                    .chain(
                        record_array(&validation, "/blackbox_evidence_ids", &validation_name)
                            .iter(),
                    )
                    .any(|value| value.as_str() == Some(id.as_str()));
            if !bound_before_review
                || evidence.get("input_hashes") != reproduction.get("input_hashes")
                || evidence.get("artifact_hash") != validation.get("artifact_hash")
            {
                fail("EFFECTIVENESS_REUSED_EVIDENCE_MISMATCH");
            }
        }
        phases.push(record_str(&evidence, "/phase", &id).to_string());
    }
    for phase in ["positive_intervention", "negative_intervention"] {
        if !phases.iter().any(|value| value == phase) {
            fail(format!("MISSING_EFFECTIVENESS_EVIDENCE_PHASE:{}", phase));
        }
    }
    if !phases.iter().any(|phase| {
        matches!(
            phase.as_str(),
            "post_architecture_effectiveness" | "deployed_blackbox"
        )
    }) {
        fail("MISSING_EFFECTIVENESS_EVIDENCE_PHASE:public_entrypoint");
    }
}

pub(super) fn assert_parallel_merge_gate(root: &Path, module_id: &str) {
    let promotion_name = module_record_name("promotion-record", module_id);
    let promotion = read_record(root, &promotion_name);
    assert_parallel_merge_gate_for_promotion(root, module_id, &promotion);
}

pub(super) fn assert_parallel_merge_gate_for_promotion(
    root: &Path,
    module_id: &str,
    promotion: &Value,
) {
    let worktree_name = module_record_name("worktree-record", module_id);
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let effectiveness_name = module_record_name("effectiveness-record", module_id);
    let promotion_name = module_record_name("promotion-record", module_id);
    let merge_name = module_record_name("merge-record", module_id);
    let worktree = read_record(root, &worktree_name);
    let candidate = read_record(root, &candidate_name);
    let effectiveness = read_record(root, &effectiveness_name);
    let collaboration_name = format!(
        "collaboration-record-{}.json",
        record_str(&promotion, "/collaboration_record_id", &promotion_name)
    );
    let queue_name = format!(
        "merge-queue-record-{}.json",
        record_str(&promotion, "/merge_queue_record_id", &promotion_name)
    );
    let integration_name = format!(
        "integration-record-{}.json",
        record_str(&promotion, "/integration_record_id", &promotion_name)
    );
    let receipt_name = format!(
        "mainline-receipt-record-{}.json",
        record_str(&promotion, "/mainline_receipt_record_id", &promotion_name)
    );
    let collaboration = read_record(root, &collaboration_name);
    let queue = read_record(root, &queue_name);
    let integration = read_record(root, &integration_name);
    let receipt = read_record(root, &receipt_name);
    let collaboration_index = read_record(root, "collaboration-index.json");
    let queue_state = read_record(root, "merge-queue-state.json");
    let merge = read_record(root, &merge_name);
    let issue_id = record_str(&candidate, "/issue_id", &candidate_name);
    let candidate_id = record_str(&candidate, "/fix_candidate_id", &candidate_name);
    let candidate_commit = record_str(&candidate, "/head_commit", &candidate_name);
    let candidate_tree = record_str(&candidate, "/tree_hash", &candidate_name);
    let effectiveness_id = record_str(&effectiveness, "/effectiveness_id", &effectiveness_name);
    let collaboration_id = record_str(&collaboration, "/collaboration_id", &collaboration_name);
    let queue_id = record_str(&queue, "/queue_entry_id", &queue_name);
    let integration_id = record_str(&integration, "/integration_id", &integration_name);
    let receipt_id = record_str(&receipt, "/receipt_id", &receipt_name);
    let milestone_id = record_str(&collaboration, "/milestone_id", &collaboration_name);
    let parent_task_id = record_str(&collaboration, "/parent_task_id", &collaboration_name);
    let milestone_sequence = collaboration
        .get("milestone_sequence")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let integration_commit = record_str(&integration, "/integration_commit", &integration_name);
    let integration_tree = record_str(&integration, "/integration_tree_hash", &integration_name);
    let main_base_commit = record_str(&queue, "/main_base_commit", &queue_name);

    if milestone_id.is_empty()
        || parent_task_id.is_empty()
        || milestone_sequence == 0
        || record_str(&collaboration, "/milestone_scope", &collaboration_name).is_empty()
        || collaboration.get("independently_verifiable") != Some(&Value::Bool(true))
        || collaboration.get("one_milestone_per_worktree") != Some(&Value::Bool(true))
        || record_str(&worktree, "/milestone_id", &worktree_name) != milestone_id
    {
        fail("INCREMENTAL_MILESTONE_CONTRACT_REQUIRED");
    }
    let predecessor_collaboration_id = record_str(
        &collaboration,
        "/predecessor_collaboration_id",
        &collaboration_name,
    );
    let predecessor_receipt_id = record_str(
        &collaboration,
        "/predecessor_receipt_id",
        &collaboration_name,
    );
    if milestone_sequence == 1 {
        if predecessor_collaboration_id != "none" || predecessor_receipt_id != "none" {
            fail("FIRST_MILESTONE_PREDECESSOR_INVALID");
        }
    } else {
        if predecessor_collaboration_id == "none" || predecessor_receipt_id == "none" {
            fail("MILESTONE_PREDECESSOR_RECEIPT_REQUIRED");
        }
        let predecessor_collaboration_name =
            format!("collaboration-record-{}.json", predecessor_collaboration_id);
        let predecessor_receipt_name =
            format!("mainline-receipt-record-{}.json", predecessor_receipt_id);
        let predecessor_collaboration = read_record(root, &predecessor_collaboration_name);
        let predecessor_receipt = read_record(root, &predecessor_receipt_name);
        if record_str(
            &predecessor_collaboration,
            "/parent_task_id",
            &predecessor_collaboration_name,
        ) != parent_task_id
            || predecessor_collaboration
                .get("milestone_sequence")
                .and_then(Value::as_u64)
                != Some(milestone_sequence - 1)
            || record_str(
                &predecessor_collaboration,
                "/worktree_id",
                &predecessor_collaboration_name,
            ) == record_str(&collaboration, "/worktree_id", &collaboration_name)
            || predecessor_receipt.get("remote_verified") != Some(&Value::Bool(true))
            || predecessor_receipt.get("result").and_then(Value::as_str) != Some("pass")
        {
            fail("MILESTONE_PREDECESSOR_MISMATCH");
        }
        let predecessor_remote_commit = record_str(
            &predecessor_receipt,
            "/remote_main_commit",
            &predecessor_receipt_name,
        );
        let current_base = record_str(&worktree, "/base_commit", &worktree_name);
        let inherited = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "merge-base",
                "--is-ancestor",
                predecessor_remote_commit,
                current_base,
            ])
            .status()
            .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
        if !inherited.success() {
            fail("NEXT_MILESTONE_BASE_PRECEDES_REMOTE_RECEIPT");
        }
    }

    for (record, name) in [
        (&collaboration, collaboration_name.as_str()),
        (&queue, queue_name.as_str()),
        (&integration, integration_name.as_str()),
        (&receipt, receipt_name.as_str()),
        (&merge, merge_name.as_str()),
    ] {
        if record_str(record, "/issue_id", name) != issue_id
            || record_str(record, "/module_id", name) != module_id
        {
            fail("PARALLEL_DEVELOPMENT_SCOPE_MISMATCH");
        }
    }
    if collaboration.get("scenario_ids")
        != Some(&serde_json::json!([
            "multi_worker_collaboration",
            "multi_worktree_merge_queue"
        ]))
        || record_str(&collaboration, "/worktree_id", &collaboration_name)
            != record_str(&worktree, "/worktree_id", &worktree_name)
        || collaboration.get("exclusive_worktree") != Some(&Value::Bool(true))
        || collaboration.get("exclusive_claim") != Some(&Value::Bool(true))
        || collaboration.get("status").and_then(Value::as_str) != Some("handoff_ready")
    {
        fail("MULTI_WORKER_EXCLUSIVE_WORKTREE_REQUIRED");
    }
    for path in ["/run_id", "/semantic_claim_id", "/worker_id"] {
        if record_str(&collaboration, path, &collaboration_name).is_empty() {
            fail("INVALID_COLLABORATION_IDENTITY");
        }
    }
    let active_claims = record_array(
        &collaboration_index,
        "/active_claims",
        "collaboration-index.json",
    );
    let mut claim_ids = std::collections::HashSet::new();
    let mut worker_ids = std::collections::HashSet::new();
    let mut worktree_ids = std::collections::HashSet::new();
    let mut milestone_ids = std::collections::HashSet::new();
    let mut current_claim_found = false;
    for claim in active_claims {
        let semantic_id = record_str(claim, "/semantic_claim_id", "collaboration-index.json");
        let worker_id = record_str(claim, "/worker_id", "collaboration-index.json");
        let worktree_id = record_str(claim, "/worktree_id", "collaboration-index.json");
        let indexed_milestone_id = record_str(claim, "/milestone_id", "collaboration-index.json");
        if !claim_ids.insert(semantic_id)
            || !worker_ids.insert(worker_id)
            || !worktree_ids.insert(worktree_id)
            || !milestone_ids.insert(indexed_milestone_id)
        {
            fail("COLLABORATION_INDEX_NOT_EXCLUSIVE");
        }
        if record_str(claim, "/collaboration_id", "collaboration-index.json") == collaboration_id
            && indexed_milestone_id == milestone_id
        {
            current_claim_found = true;
        }
    }
    if !current_claim_found {
        fail("COLLABORATION_NOT_ACTIVE");
    }
    if record_str(&queue, "/collaboration_id", &queue_name) != collaboration_id
        || record_str(&queue, "/milestone_id", &queue_name) != milestone_id
        || queue.get("delivery_mode").and_then(Value::as_str) != Some("commit_merge_each_milestone")
        || record_str(&queue, "/fix_candidate_id", &queue_name) != candidate_id
        || record_str(&queue, "/effectiveness_id", &queue_name) != effectiveness_id
        || record_str(&queue, "/candidate_commit", &queue_name) != candidate_commit
        || queue
            .get("queue_position")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            == 0
        || record_str(&queue, "/merge_owner", &queue_name).is_empty()
        || queue.get("strategy").and_then(Value::as_str)
            != Some("integration_merge_then_fast_forward")
        || queue.get("status").and_then(Value::as_str) != Some("admitted")
    {
        fail("MERGE_QUEUE_ADMISSION_MISMATCH");
    }
    let ordered_entries =
        record_array(&queue_state, "/ordered_entry_ids", "merge-queue-state.json");
    let mut unique_entries = std::collections::HashSet::new();
    if record_str(&queue_state, "/merge_owner", "merge-queue-state.json")
        != record_str(&queue, "/merge_owner", &queue_name)
        || record_str(&queue_state, "/active_entry_id", "merge-queue-state.json") != queue_id
        || ordered_entries
            .iter()
            .any(|entry| !unique_entries.insert(entry.as_str().unwrap_or("")))
        || ordered_entries
            .get(
                queue
                    .get("queue_position")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize
                    - 1,
            )
            .and_then(Value::as_str)
            != Some(queue_id)
    {
        fail("GLOBAL_MERGE_QUEUE_STATE_MISMATCH");
    }
    let gate_results = integration
        .get("required_gate_results")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INTEGRATION_GATES_MISSING"));
    let verification_map: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/maps/verification-map.json"))
            .unwrap_or_else(|_| fail("VERIFICATION_MAP_MISSING")),
    )
    .unwrap_or_else(|_| fail("INVALID_VERIFICATION_MAP"));
    let expected_gates = record_array(&verification_map, "/gates", "verification-map.json")
        .iter()
        .filter(|gate| {
            gate.get("required_for")
                .and_then(Value::as_array)
                .is_some_and(|uses| {
                    uses.iter()
                        .any(|value| value.as_str() == Some("integration_verification"))
                })
        })
        .map(|gate| {
            (
                record_str(gate, "/gate_id", "verification-map.json"),
                record_str(gate, "/producer", "verification-map.json"),
            )
        })
        .collect::<Vec<_>>();
    let actual_gates = gate_results
        .iter()
        .map(|gate| {
            if gate.get("result").and_then(Value::as_str) != Some("pass")
                || record_str(gate, "/source_commit", &integration_name) != integration_commit
                || record_str(gate, "/tree_hash", &integration_name) != integration_tree
            {
                fail("INTEGRATION_GATE_BINDING_MISMATCH");
            }
            (
                record_str(gate, "/gate_id", &integration_name),
                record_str(gate, "/producer", &integration_name),
            )
        })
        .collect::<Vec<_>>();
    if expected_gates.is_empty()
        || actual_gates.len() != expected_gates.len()
        || actual_gates
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != actual_gates.len()
        || actual_gates
            .iter()
            .any(|gate| !expected_gates.contains(gate))
        || record_str(&integration, "/queue_entry_id", &integration_name) != queue_id
        || record_str(&integration, "/milestone_id", &integration_name) != milestone_id
        || record_str(&integration, "/candidate_commit", &integration_name) != candidate_commit
        || record_str(&integration, "/main_base_commit", &integration_name) != main_base_commit
        || integration.get("conflict_status").and_then(Value::as_str) != Some("clean")
        || integration.get("resolution_mode").and_then(Value::as_str) != Some("none")
        || !matches!(
            integration.get("impact_status").and_then(Value::as_str),
            Some("unchanged" | "revalidated")
        )
        || integration.get("result").and_then(Value::as_str) != Some("pass")
    {
        fail("INTEGRATION_RECORD_MISMATCH");
    }
    if git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", candidate_commit)],
        "FIX_CANDIDATE_COMMIT_MISSING",
    ) != candidate_tree
        || git_value(
            root,
            &["rev-parse", &format!("{}^{{tree}}", integration_commit)],
            "INTEGRATION_COMMIT_MISSING",
        ) != integration_tree
    {
        fail("TESTED_INTEGRATION_TREE_MISMATCH");
    }
    for ancestor in [candidate_commit, main_base_commit] {
        let reachable = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["merge-base", "--is-ancestor", ancestor, integration_commit])
            .status()
            .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
        if !reachable.success() {
            fail("INTEGRATION_ANCESTRY_MISMATCH");
        }
    }
    let local_main_ref = record_str(&receipt, "/local_main_ref", &receipt_name);
    let remote_name = record_str(&receipt, "/remote_name", &receipt_name);
    let remote_ref = record_str(&receipt, "/remote_ref", &receipt_name);
    let local_main_commit = git_value(
        root,
        &["rev-parse", local_main_ref],
        "LOCAL_MAIN_REF_MISSING",
    );
    let remote_main_commit = git_ls_remote(root, remote_name, remote_ref);
    if record_str(&receipt, "/integration_id", &receipt_name) != integration_id
        || record_str(&receipt, "/queue_entry_id", &receipt_name) != queue_id
        || record_str(&receipt, "/milestone_id", &receipt_name) != milestone_id
        || record_str(&receipt, "/integration_commit", &receipt_name) != integration_commit
        || record_str(&receipt, "/local_main_commit", &receipt_name) != local_main_commit
        || record_str(&receipt, "/remote_main_commit", &receipt_name) != remote_main_commit
        || record_str(&receipt, "/integration_tree_hash", &receipt_name) != integration_tree
        || receipt.get("candidate_reachable") != Some(&Value::Bool(true))
        || receipt.get("integration_local_reachable") != Some(&Value::Bool(true))
        || receipt.get("integration_remote_reachable") != Some(&Value::Bool(true))
        || receipt.get("remote_verified") != Some(&Value::Bool(true))
        || receipt.get("result").and_then(Value::as_str) != Some("pass")
    {
        fail("MAINLINE_RECEIPT_MISMATCH");
    }
    for main_commit in [&local_main_commit, &remote_main_commit] {
        let reachable = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "merge-base",
                "--is-ancestor",
                integration_commit,
                main_commit,
            ])
            .status()
            .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
        if !reachable.success() {
            fail("INTEGRATION_NOT_REACHABLE_FROM_MAIN");
        }
    }
    if record_str(&merge, "/queue_entry_id", &merge_name) != queue_id
        || record_str(&merge, "/integration_id", &merge_name) != integration_id
        || record_str(&merge, "/mainline_receipt_id", &merge_name) != receipt_id
        || record_str(&merge, "/milestone_id", &merge_name) != milestone_id
        || record_str(&merge, "/fix_candidate_id", &merge_name) != candidate_id
        || record_str(&merge, "/effectiveness_id", &merge_name) != effectiveness_id
        || record_str(&merge, "/mainline_ref", &merge_name) != local_main_ref
        || record_str(&merge, "/candidate_commit", &merge_name) != candidate_commit
        || record_str(&merge, "/integration_commit", &merge_name) != integration_commit
        || record_str(&merge, "/merge_commit", &merge_name) != integration_commit
        || record_str(&merge, "/candidate_tree_hash", &merge_name) != candidate_tree
        || record_str(&merge, "/integration_tree_hash", &merge_name) != integration_tree
        || record_str(&merge, "/merged_tree_hash", &merge_name) != integration_tree
        || merge.get("change_identity").and_then(Value::as_str) != Some("tested_integration_exact")
        || merge.get("result").and_then(Value::as_str) != Some("pass")
    {
        fail("PARALLEL_MAINLINE_MERGE_MISMATCH");
    }
    if !(record_time(&collaboration, &collaboration_name) <= record_time(&queue, &queue_name)
        && record_time(&effectiveness, &effectiveness_name) <= record_time(&queue, &queue_name)
        && record_time(&queue, &queue_name) <= record_time(&integration, &integration_name)
        && record_time(&integration, &integration_name) <= record_time(&receipt, &receipt_name)
        && record_time(&receipt, &receipt_name) <= record_time(&merge, &merge_name))
    {
        fail("PARALLEL_MERGE_ORDER_INVALID");
    }
}
