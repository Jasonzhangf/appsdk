use super::*;

pub(super) fn collab_live_closure_challenge(
    closure_id: &str,
    path: &str,
    source_commit: &str,
    artifact_hash: &str,
    environment_id: &str,
    endpoint_generation: u64,
) -> String {
    format!(
        "appsdk-collab-live:{closure_id}:{path}:{source_commit}:{artifact_hash}:{environment_id}:{endpoint_generation}"
    )
}

pub(super) fn collab_live_closure_route(
    root: &Path,
    expected_project_scope: &str,
    expected_app_scope: &str,
    expected_worker_id: &str,
    expected_binding_id: &str,
    expected_tmux_endpoint: &Value,
) -> Value {
    let endpoint = expected_tmux_endpoint;
    let socket_path = endpoint
        .get("socket_path")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("COLLAB_LIVE_CLOSURE_TMUX_ENDPOINT_INVALID"));
    let server_pid = endpoint
        .get("server_pid")
        .and_then(Value::as_u64)
        .filter(|pid| *pid > 0)
        .unwrap_or_else(|| fail("COLLAB_LIVE_CLOSURE_TMUX_ENDPOINT_INVALID"));
    let session_id = endpoint
        .get("tmux_session_id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("COLLAB_LIVE_CLOSURE_TMUX_ENDPOINT_INVALID"));
    let pane_id = endpoint
        .get("pane_id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("COLLAB_LIVE_CLOSURE_TMUX_ENDPOINT_INVALID"));
    let tmux = format!("{socket_path},{server_pid},0");
    let mut context_command = Command::new("collab");
    context_command
        .arg("context")
        .current_dir(root)
        .env("TMUX", &tmux)
        .env("TMUX_PANE", pane_id)
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID");
    if let Some(value) = endpoint.get("codex_session_id").and_then(Value::as_str) {
        context_command.env("CODEX_SESSION_ID", value);
    }
    if let Some(value) = endpoint.get("codex_thread_id").and_then(Value::as_str) {
        context_command.env("CODEX_THREAD_ID", value);
    }
    let context = run_goal_collab_command(context_command, GOAL_COLLAB_READ_TIMEOUT)
        .unwrap_or_else(|error| fail(error));
    if !context.status.success() {
        let detail = String::from_utf8_lossy(&context.stderr).trim().to_string();
        fail(if detail.is_empty() {
            "COLLAB_LIVE_CLOSURE_CONTEXT_FAILED".to_string()
        } else {
            detail
        });
    }
    let context: Value = serde_json::from_slice(&context.stdout)
        .unwrap_or_else(|_| fail("COLLAB_LIVE_CLOSURE_CONTEXT_INVALID"));
    if context.get("registered").and_then(Value::as_bool) != Some(true)
        || context.pointer("/liveness/live").and_then(Value::as_bool) != Some(true)
    {
        fail("COLLAB_LIVE_CLOSURE_DAEMON_NOT_LIVE");
    }
    if context.pointer("/project_root").and_then(Value::as_str) != Some(expected_project_scope) {
        fail("COLLAB_LIVE_CLOSURE_PROJECT_SCOPE_MISMATCH");
    }
    if context
        .pointer("/identity/worker_id")
        .and_then(Value::as_str)
        != Some(expected_worker_id)
        || context.pointer("/identity/transport/tmux_endpoint") != Some(expected_tmux_endpoint)
    {
        fail("COLLAB_LIVE_CLOSURE_IDENTITY_MISSING");
    }

    let mut route_command = Command::new("collab");
    route_command
        .args(["route", "resolve", "--tmux-session-id"])
        .arg(session_id)
        .arg("--pane-id")
        .arg(pane_id)
        .current_dir(root)
        .env("TMUX", &tmux)
        .env("TMUX_PANE", pane_id)
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID");
    if let Some(value) = endpoint.get("codex_session_id").and_then(Value::as_str) {
        route_command.env("CODEX_SESSION_ID", value);
    }
    if let Some(value) = endpoint.get("codex_thread_id").and_then(Value::as_str) {
        route_command.env("CODEX_THREAD_ID", value);
    }
    let route = run_goal_collab_command(route_command, GOAL_COLLAB_READ_TIMEOUT)
        .unwrap_or_else(|error| fail(error));
    if !route.status.success() {
        let detail = String::from_utf8_lossy(&route.stderr).trim().to_string();
        fail(if detail.is_empty() {
            "COLLAB_LIVE_CLOSURE_ROUTE_FAILED".to_string()
        } else {
            detail
        });
    }
    let route: Value = serde_json::from_slice(&route.stdout)
        .unwrap_or_else(|_| fail("COLLAB_LIVE_CLOSURE_ROUTE_INVALID"));
    if route
        .get("endpoint_generation")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        == 0
        || route.get("project_scope").and_then(Value::as_str) != Some(expected_project_scope)
        || route.get("canonical_root").and_then(Value::as_str) != Some(expected_project_scope)
        || route.get("app_scope_id").and_then(Value::as_str) != Some(expected_app_scope)
        || route.get("agent_id").and_then(Value::as_str) != Some(expected_worker_id)
        || route.get("binding_id").and_then(Value::as_str) != Some(expected_binding_id)
        || route.get("tmux_endpoint") != Some(expected_tmux_endpoint)
        || route.get("storage_root").and_then(Value::as_str).is_none()
    {
        fail("COLLAB_LIVE_CLOSURE_ROUTE_INVALID");
    }
    route
}

pub(super) fn collab_live_closure_message(
    root: &Path,
    message_id: &str,
    expected_sender: &str,
    expected_receiver: &str,
    expected_challenge: &str,
) -> Value {
    assert_identifier(message_id, "INVALID_COLLAB_MESSAGE_ID");
    let mut command = Command::new("collab");
    command.args(["msg", message_id]).current_dir(root);
    let message = run_goal_collab_command(command, GOAL_COLLAB_READ_TIMEOUT)
        .unwrap_or_else(|error| fail(error));
    if !message.status.success() {
        let detail = String::from_utf8_lossy(&message.stderr).trim().to_string();
        fail(if detail.is_empty() {
            "COLLAB_LIVE_CLOSURE_MESSAGE_FAILED".to_string()
        } else {
            detail
        });
    }
    let message: Value = serde_json::from_slice(&message.stdout)
        .unwrap_or_else(|_| fail("COLLAB_LIVE_CLOSURE_MESSAGE_INVALID"));
    if message.get("id").and_then(Value::as_str) != Some(message_id)
        || message.get("from").and_then(Value::as_str) != Some(expected_sender)
        || message.get("to").and_then(Value::as_str) != Some(expected_receiver)
        || message.get("state").and_then(Value::as_str) != Some("read")
        || message.get("subject").and_then(Value::as_str) != Some(expected_challenge)
    {
        fail("COLLAB_LIVE_CLOSURE_MESSAGE_NOT_CHALLENGE_BOUND");
    }
    message
}

pub(super) fn assert_collab_live_closure(
    root: &Path,
    module_id: &str,
    promotion: &Value,
    issue_id: &str,
    candidate_id: &str,
    artifact_hash: &str,
    scope_hash: &str,
    source_commit: &str,
) {
    let closure_id = producer_string(
        promotion,
        "/collab_live_closure_record_id",
        "COLLAB_LIVE_CLOSURE_RECORD_MISSING",
    );
    assert_identifier(&closure_id, "INVALID_COLLAB_LIVE_CLOSURE_ID");
    let closure = read_record(root, &format!("collab-live-closure-{closure_id}.json"));
    for path in [
        "/closure_id",
        "/issue_id",
        "/module_id",
        "/fix_candidate_id",
        "/artifact_hash",
        "/scope_hash",
        "/source_commit",
        "/environment_id",
        "/entrypoint",
        "/collab_identity/worker_id",
        "/collab_identity/binding_id",
        "/collab_identity/app_scope_id",
        "/collab_identity/project_scope_id",
        "/route_receipt/storage_root",
        "/route_receipt/resolved_at",
        "/created_at",
    ] {
        record_str(&closure, path, "collab-live-closure-record.json");
    }
    if record_str(&closure, "/closure_id", "collab-live-closure-record.json") != closure_id
        || record_str(&closure, "/issue_id", "collab-live-closure-record.json") != issue_id
        || record_str(&closure, "/module_id", "collab-live-closure-record.json") != module_id
        || record_str(
            &closure,
            "/fix_candidate_id",
            "collab-live-closure-record.json",
        ) != candidate_id
        || record_str(
            &closure,
            "/artifact_hash",
            "collab-live-closure-record.json",
        ) != artifact_hash
        || record_str(&closure, "/scope_hash", "collab-live-closure-record.json") != scope_hash
        || record_str(
            &closure,
            "/source_commit",
            "collab-live-closure-record.json",
        ) != source_commit
        || closure
            .pointer("/route_receipt/source")
            .and_then(Value::as_str)
            != Some("collab_cli")
        || closure
            .pointer("/route_receipt/daemon_live")
            .and_then(Value::as_bool)
            != Some(true)
    {
        fail("COLLAB_LIVE_CLOSURE_BINDING_MISMATCH");
    }
    let environment_id = record_str(
        &closure,
        "/environment_id",
        "collab-live-closure-record.json",
    );
    let entrypoint = record_str(&closure, "/entrypoint", "collab-live-closure-record.json");
    let project_scope = record_str(
        &closure,
        "/collab_identity/project_scope_id",
        "collab-live-closure-record.json",
    );
    let app_scope = record_str(
        &closure,
        "/collab_identity/app_scope_id",
        "collab-live-closure-record.json",
    );
    let worker_id = record_str(
        &closure,
        "/collab_identity/worker_id",
        "collab-live-closure-record.json",
    );
    let tmux_endpoint = closure
        .pointer("/route_receipt/tmux_endpoint")
        .filter(|value| value.is_object())
        .unwrap_or_else(|| {
            fail("INVALID_RECORD:collab-live-closure-record.json:/route_receipt/tmux_endpoint")
        });
    let binding_id = record_str(
        &closure,
        "/collab_identity/binding_id",
        "collab-live-closure-record.json",
    );
    let route = collab_live_closure_route(
        root,
        project_scope,
        app_scope,
        worker_id,
        binding_id,
        tmux_endpoint,
    );
    if route.get("endpoint_generation") != closure.pointer("/route_receipt/endpoint_generation")
        || route.get("project_scope") != closure.pointer("/route_receipt/route_scope/project_scope")
        || route.get("app_scope_id") != closure.pointer("/route_receipt/route_scope/app_scope_id")
        || route.get("storage_root") != closure.pointer("/route_receipt/storage_root")
    {
        fail("COLLAB_LIVE_CLOSURE_ROUTE_DRIFT");
    }
    let evidence_ids = closure
        .get("evidence_ids")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("COLLAB_LIVE_CLOSURE_MATRIX_MISSING"));
    let path_receipts = closure
        .get("path_receipts")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("COLLAB_LIVE_CLOSURE_MATRIX_MISSING"));
    if evidence_ids.len() != COLLAB_LIVE_CLOSURE_PATHS.len()
        || path_receipts.len() != COLLAB_LIVE_CLOSURE_PATHS.len()
    {
        fail("COLLAB_LIVE_CLOSURE_MATRIX_MISSING");
    }
    let mut seen_evidence_ids = std::collections::HashSet::new();
    let mut seen_message_ids = std::collections::HashSet::new();
    let endpoint_generation = route
        .get("endpoint_generation")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fail("COLLAB_LIVE_CLOSURE_ROUTE_INVALID"));
    for path in COLLAB_LIVE_CLOSURE_PATHS {
        let evidence_id = evidence_ids
            .get(path)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| fail(format!("COLLAB_LIVE_CLOSURE_PATH_MISSING:{}", path)));
        if !seen_evidence_ids.insert(evidence_id.to_string()) {
            fail(format!("COLLAB_LIVE_CLOSURE_EVIDENCE_REUSED:{}", path));
        }
        let receipt = path_receipts
            .get(path)
            .filter(|value| value.is_object())
            .unwrap_or_else(|| fail(format!("COLLAB_LIVE_CLOSURE_PATH_MISSING:{}", path)));
        let message_id = record_str(receipt, "/message_id", "collab-live-closure-record.json");
        let challenge = record_str(receipt, "/challenge", "collab-live-closure-record.json");
        let expected_challenge = collab_live_closure_challenge(
            &closure_id,
            path,
            source_commit,
            artifact_hash,
            environment_id,
            endpoint_generation,
        );
        if record_str(receipt, "/evidence_id", "collab-live-closure-record.json") != evidence_id
            || record_str(receipt, "/source_commit", "collab-live-closure-record.json")
                != source_commit
            || record_str(receipt, "/artifact_hash", "collab-live-closure-record.json")
                != artifact_hash
            || record_str(
                receipt,
                "/environment_id",
                "collab-live-closure-record.json",
            ) != environment_id
            || record_str(receipt, "/entrypoint", "collab-live-closure-record.json") != entrypoint
            || receipt
                .pointer("/endpoint_generation")
                .and_then(Value::as_u64)
                != Some(endpoint_generation)
            || challenge != expected_challenge
        {
            fail(format!("COLLAB_LIVE_CLOSURE_PATH_MISMATCH:{}", path));
        }
        if !seen_message_ids.insert(message_id.to_string()) {
            fail(format!("COLLAB_LIVE_CLOSURE_MESSAGE_REUSED:{}", path));
        }
        let expected_sender = if path.starts_with("daemon_to_") {
            "daemon"
        } else if path.starts_with("master_to_") {
            "master"
        } else if path.starts_with("peer_to_") {
            "peer"
        } else {
            "daemon"
        };
        let expected_receiver = if path.ends_with("_to_master") {
            "master"
        } else {
            "peer"
        };
        if record_str(receipt, "/sender", "collab-live-closure-record.json") != expected_sender
            || record_str(receipt, "/receiver", "collab-live-closure-record.json")
                != expected_receiver
        {
            fail(format!(
                "COLLAB_LIVE_CLOSURE_PATH_DIRECTION_MISMATCH:{}",
                path
            ));
        }
        collab_live_closure_message(
            root,
            message_id,
            expected_sender,
            expected_receiver,
            &expected_challenge,
        );
        let evidence = evidence_by_id(root, module_id, evidence_id);
        assert_evidence_record(
            &evidence,
            evidence_id,
            EvidenceValidationMode::Current(Utc::now()),
        );
        let expected_phase = if path == "restart_replay" {
            "deployment_restart"
        } else {
            "deployed_blackbox"
        };
        let expected_kind = if path == "restart_replay" {
            "restart"
        } else {
            "sample_replay"
        };
        if record_str(&evidence, "/issue_id", evidence_id) != issue_id
            || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
            || record_str(&evidence, "/scope_hash", evidence_id) != scope_hash
            || record_str(&evidence, "/source_commit", evidence_id) != source_commit
            || record_str(&evidence, "/artifact_hash", evidence_id) != artifact_hash
            || record_str(&evidence, "/environment_id", evidence_id) != environment_id
            || record_str(&evidence, "/entrypoint", evidence_id) != entrypoint
            || record_str(&evidence, "/result", evidence_id) != "pass"
            || record_str(&evidence, "/phase", evidence_id) != expected_phase
            || record_str(&evidence, "/kind", evidence_id) != expected_kind
        {
            fail(format!("COLLAB_LIVE_CLOSURE_EVIDENCE_MISMATCH:{}", path));
        }
    }
}

pub(super) fn lifecycle_chain_promotion_id(
    issue_id: &str,
    module_id: &str,
    candidate_id: &str,
) -> String {
    producer_stable_id(
        "promotion",
        &serde_json::json!({
            "issue_id": issue_id,
            "module_id": module_id,
            "fix_candidate_id": candidate_id
        }),
    )
}

pub(super) fn lifecycle_chain_architecture(root: &Path, module_id: &str, input_path: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    assert_declared_contracts(root, &project);
    assert_goal_confirmed(root);
    assert_lifecycle_producer_map_binding(root, &project, module_id, LifecycleProducer::Chain);
    let observation = lifecycle_chain_input(root, input_path, "architecture");
    let (worktree, _reproduction, candidate, validation) =
        lifecycle_chain_candidate(root, module_id);
    let issue_id = producer_string(&worktree, "/issue_id", "worktree-record.json");
    let candidate_id =
        producer_string(&candidate, "/fix_candidate_id", "fix-candidate-record.json");
    let candidate_commit = producer_string(&candidate, "/head_commit", "fix-candidate-record.json");
    let candidate_tree = producer_string(&candidate, "/tree_hash", "fix-candidate-record.json");
    let scope_hash = producer_string(&candidate, "/scope_hash", "fix-candidate-record.json");
    if git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", candidate_commit)],
        "FIX_CANDIDATE_COMMIT_MISSING",
    ) != candidate_tree
    {
        fail("FIX_CANDIDATE_TREE_MISMATCH");
    }
    assert_lifecycle_chain_candidate_at_head(
        root,
        &project,
        module_id,
        &candidate_commit,
        &candidate_tree,
    );
    let module = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    let artifact = read_module_artifact(root, &project, module_id);
    module_artifact_matches_project(module, &artifact);
    let evidence_ids = lifecycle_chain_required_array(
        &observation,
        "/evidence_ids",
        "ARCHITECTURE_REVIEW_EVIDENCE_MISSING",
    );
    let review_time = Utc::now();
    for id in &evidence_ids {
        let evidence = lifecycle_chain_validate_evidence(
            root,
            module_id,
            id,
            &issue_id,
            &scope_hash,
            &candidate_commit,
        );
        if record_time(&evidence, id) > review_time {
            fail("ARCHITECTURE_REVIEW_EVIDENCE_FUTURE");
        }
    }
    let reviewer = observation
        .get("reviewer")
        .filter(|value| {
            value
                .get("adapter")
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty())
                && value
                    .get("identity")
                    .and_then(Value::as_str)
                    .is_some_and(|v| !v.is_empty())
        })
        .cloned()
        .unwrap_or_else(|| fail("ARCHITECTURE_REVIEWER_MISSING"));
    let mut project_bindings = observation.get("project_bindings").map(|value| {
        if !value.is_object() {
            fail("ARCHITECTURE_REVIEW_PROJECT_BINDINGS_INVALID");
        }
        value.clone()
    });
    let verdict = producer_string(
        &observation,
        "/verdict",
        "ARCHITECTURE_REVIEW_VERDICT_MISSING",
    );
    if !matches!(
        verdict.as_str(),
        "pass" | "fail" | "unknown" | "new_version_required" | "manual_auth_required"
    ) {
        fail("ARCHITECTURE_REVIEW_VERDICT_INVALID");
    }
    if verdict == "pass" {
        assert_review_author_readiness(root, module_id);
        let binding = observation
            .get("requirements_review")
            .unwrap_or_else(|| fail("ARCHITECTURE_REQUIREMENTS_REVIEW_MISSING"));
        assert_review_requirements_binding(root, module_id, binding);
        project_bindings.get_or_insert_with(|| serde_json::json!({}))["requirements_review"] =
            binding.clone();
    }
    let promotion_id = lifecycle_chain_promotion_id(&issue_id, module_id, &candidate_id);
    let review_evidence_ids = serde_json::json!(evidence_ids);
    let review_id = producer_stable_id(
        "review",
        &lifecycle_chain_review_identity(
            &promotion_id,
            &candidate_id,
            &reviewer,
            &verdict,
            &review_evidence_ids,
            project_bindings.as_ref(),
        ),
    );
    let mut review = serde_json::json!({
        "review_id": review_id,
        "review_kind": "architecture",
        "issue_id": issue_id,
        "promotion_id": promotion_id,
        "fix_candidate_id": candidate_id,
        "pre_review_validation_id": producer_string(&validation, "/validation_id", "pre-review-validation-record.json"),
        "reviewer": reviewer,
        "verdict": verdict,
        "evidence_ids": evidence_ids,
        "reviewed_commit": candidate_commit,
        "reviewed_tree_hash": candidate_tree,
        "reviewed_diff_hash": producer_string(&candidate, "/diff_hash", "fix-candidate-record.json"),
        "reviewed_artifact_hash": producer_string(&artifact, "/artifact_hash", "module-artifact"),
        "reviewed_scope_hash": scope_hash,
        "resource_map_hash": file_sha256(&root.join(".appsdk/maps/resource-map.json"), "resource-map.json"),
        "function_map_hash": file_sha256(&root.join(".appsdk/maps/function-map.json"), "function-map.json"),
        "mainline_call_map_hash": file_sha256(&root.join(".appsdk/maps/mainline-call-map.json"), "mainline-call-map.json"),
        "verification_map_hash": file_sha256(&root.join(".appsdk/maps/verification-map.json"), "verification-map.json"),
        "created_at": review_time.to_rfc3339()
    });
    if let Some(bindings) = project_bindings {
        review["project_bindings"] = bindings;
    }
    for path in [
        "/review_id",
        "/promotion_id",
        "/fix_candidate_id",
        "/pre_review_validation_id",
        "/reviewed_commit",
        "/reviewed_tree_hash",
        "/reviewed_diff_hash",
        "/reviewed_artifact_hash",
        "/reviewed_scope_hash",
        "/resource_map_hash",
        "/function_map_hash",
        "/mainline_call_map_hash",
        "/verification_map_hash",
        "/created_at",
    ] {
        producer_string(&review, path, "ARCHITECTURE_REVIEW_RECORD_INVALID");
    }
    // A cached architecture PASS is valid only while the pre-review validation
    // it references still passes for the current candidate and artifact.
    assert_pre_review_validation_gate(root, module_id, &artifact);
    let review_name = module_record_name("review-record", module_id);
    if let Some(existing) = lifecycle_chain_read_record_if_present(root, module_id, "review-record")
    {
        assert_lifecycle_chain_review_identity(&existing);
        if lifecycle_chain_record_is_pass("review-record", &existing) {
            let existing_record_hash = sha256(&canonical(&existing));
            let existing_attempt_id =
                lifecycle_chain_attempt_identity(module_id, "review-record", &existing_record_hash);
            lifecycle_chain_validate_attempt_ledger(
                root,
                module_id,
                "review-record",
                &existing_attempt_id,
                &existing_record_hash,
                &existing,
            );
            if verdict != "pass" {
                fail("LIFECYCLE_CHAIN_PASS_IMMUTABLE");
            }
            if producer_without_fields(&existing, &["created_at"])
                == producer_without_fields(&review, &["created_at"])
            {
                lifecycle_chain_assert_reusable_record(
                    &existing,
                    &review,
                    &review_name,
                    "ARCHITECTURE_REVIEW_IDENTITY_MISMATCH",
                    true,
                );
                let existing_time = record_time(&existing, &review_name);
                for id in &evidence_ids {
                    let evidence = evidence_by_id(root, module_id, id);
                    if record_time(&evidence, id) > existing_time {
                        fail("ARCHITECTURE_REVIEW_EVIDENCE_DRIFT");
                    }
                }
                lifecycle_chain_output(&existing, true);
                return;
            }
        }
    }
    let reused = lifecycle_chain_write_record(root, module_id, "review-record", &review);
    lifecycle_chain_output(&review, reused);
}

pub(super) fn lifecycle_chain_effectiveness(root: &Path, module_id: &str, input_path: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    assert_declared_contracts(root, &project);
    assert_goal_confirmed(root);
    let observation = lifecycle_chain_input(root, input_path, "effectiveness");
    let (worktree, reproduction, candidate, _validation) =
        lifecycle_chain_candidate(root, module_id);
    let review_name = module_record_name("review-record", module_id);
    let review = read_record(root, &review_name);
    assert_lifecycle_chain_review_identity(&review);
    if review.get("verdict").and_then(Value::as_str) != Some("pass") {
        fail("ARCHITECTURE_REVIEW_NOT_PASS");
    }
    let artifact = read_module_artifact(root, &project, module_id);
    let module = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    module_artifact_matches_project(module, &artifact);
    let issue_id = producer_string(&worktree, "/issue_id", "worktree-record.json");
    let candidate_id =
        producer_string(&candidate, "/fix_candidate_id", "fix-candidate-record.json");
    let candidate_commit = producer_string(&candidate, "/head_commit", "fix-candidate-record.json");
    let candidate_tree = producer_string(&candidate, "/tree_hash", "fix-candidate-record.json");
    let scope_hash = producer_string(&candidate, "/scope_hash", "fix-candidate-record.json");
    assert_lifecycle_chain_candidate_at_head(
        root,
        &project,
        module_id,
        &candidate_commit,
        &candidate_tree,
    );
    let fixed_id = producer_string(
        &observation,
        "/fixed_replay_evidence_id",
        "EFFECTIVENESS_FIXED_REPLAY_MISSING",
    );
    let positive_ids = lifecycle_chain_required_array(
        &observation,
        "/positive_evidence_ids",
        "EFFECTIVENESS_POSITIVE_EVIDENCE_MISSING",
    );
    let negative_ids = lifecycle_chain_required_array(
        &observation,
        "/negative_evidence_ids",
        "EFFECTIVENESS_NEGATIVE_EVIDENCE_MISSING",
    );
    let blackbox_ids = lifecycle_chain_required_array(
        &observation,
        "/blackbox_evidence_ids",
        "EFFECTIVENESS_BLACKBOX_EVIDENCE_MISSING",
    );
    let mut all_ids = vec![fixed_id.clone()];
    all_ids.extend(positive_ids.iter().cloned());
    all_ids.extend(negative_ids.iter().cloned());
    all_ids.extend(blackbox_ids.iter().cloned());
    let effectiveness_time = Utc::now();
    let mut phases = std::collections::HashSet::new();
    for id in &all_ids {
        let evidence = lifecycle_chain_validate_evidence(
            root,
            module_id,
            id,
            &issue_id,
            &scope_hash,
            &candidate_commit,
        );
        if evidence.get("input_hashes") != reproduction.get("input_hashes") {
            fail("EFFECTIVENESS_INPUT_MISMATCH");
        }
        if record_time(&evidence, id) > effectiveness_time {
            fail("EFFECTIVENESS_EVIDENCE_FUTURE");
        }
        phases.insert(producer_string(&evidence, "/phase", id));
    }
    if !phases.contains("positive_intervention")
        || !phases.contains("negative_intervention")
        || (!phases.contains("post_architecture_effectiveness")
            && !phases.contains("deployed_blackbox"))
    {
        fail("EFFECTIVENESS_REQUIRED_PHASE_MISSING");
    }
    let review_id = producer_string(&review, "/review_id", &review_name);
    let effectiveness_id = producer_stable_id(
        "effectiveness",
        &serde_json::json!({"fix_candidate_id": candidate_id, "architecture_review_id": review_id, "evidence_ids": all_ids}),
    );
    let effectiveness = serde_json::json!({
        "effectiveness_id": effectiveness_id,
        "issue_id": issue_id,
        "module_id": module_id,
        "fix_candidate_id": candidate_id,
        "architecture_review_id": review_id,
        "reviewed_commit": candidate_commit,
        "reviewed_tree_hash": candidate_tree,
        "reproduction_input_hashes": reproduction["input_hashes"].clone(),
        "baseline_evidence_id": producer_string(&reproduction, "/baseline_evidence_id", "reproduction-record.json"),
        "fixed_replay_evidence_id": fixed_id,
        "positive_evidence_ids": positive_ids,
        "negative_evidence_ids": negative_ids,
        "blackbox_evidence_ids": blackbox_ids,
        "source_unchanged_since_review": true,
        "result": "pass",
        "created_at": effectiveness_time.to_rfc3339()
    });
    for path in [
        "/effectiveness_id",
        "/issue_id",
        "/module_id",
        "/fix_candidate_id",
        "/architecture_review_id",
        "/reviewed_commit",
        "/reviewed_tree_hash",
        "/baseline_evidence_id",
        "/fixed_replay_evidence_id",
        "/created_at",
    ] {
        producer_string(&effectiveness, path, "EFFECTIVENESS_RECORD_INVALID");
    }
    // Reuse still validates the complete upstream architecture gate.  This is
    // read-only and avoids rerunning any external review/effectiveness action.
    assert_fix_architecture_gate(root, module_id, &artifact);
    if let Some(existing) =
        lifecycle_chain_read_record_if_present(root, module_id, "effectiveness-record")
    {
        if lifecycle_chain_record_is_pass("effectiveness-record", &existing) {
            let existing_record_hash = sha256(&canonical(&existing));
            let existing_attempt_id = lifecycle_chain_attempt_identity(
                module_id,
                "effectiveness-record",
                &existing_record_hash,
            );
            lifecycle_chain_validate_attempt_ledger(
                root,
                module_id,
                "effectiveness-record",
                &existing_attempt_id,
                &existing_record_hash,
                &existing,
            );
            if producer_without_fields(&existing, &["created_at"])
                == producer_without_fields(&effectiveness, &["created_at"])
            {
                // A cache hit must satisfy the same complete effectiveness
                // gate as a downstream consumer. This catches evidence
                // timestamp or reference drift before returning reused=true.
                assert_fix_effectiveness_gate(root, module_id);
                lifecycle_chain_assert_reusable_record(
                    &existing,
                    &effectiveness,
                    &module_record_name("effectiveness-record", module_id),
                    "EFFECTIVENESS_RECORD_IDENTITY_MISMATCH",
                    true,
                );
                lifecycle_chain_output(&existing, true);
                return;
            }
        }
    }
    let reused =
        lifecycle_chain_write_record(root, module_id, "effectiveness-record", &effectiveness);
    lifecycle_chain_output(&effectiveness, reused);
}

pub(super) fn assert_lifecycle_chain_promotion_gates(gates: &[Value]) {
    let verification_map: Value =
        serde_json::from_str(canonical_governance_map("verification-map.json"))
            .unwrap_or_else(|_| fail("PROMOTION_VERIFICATION_MAP_INVALID"));
    let expected = record_array(&verification_map, "/gates", "verification-map.json")
        .iter()
        .filter(|gate| {
            gate.get("required_for")
                .and_then(Value::as_array)
                .is_some_and(|uses| {
                    uses.iter()
                        .any(|use_case| use_case.as_str() == Some("promotion"))
                })
        })
        .map(|gate| record_str(gate, "/gate_id", "verification-map.json"))
        .collect::<Vec<_>>();
    if expected.is_empty() {
        fail("PROMOTION_GATES_UNDECLARED");
    }
    let mut actual = std::collections::HashSet::new();
    for gate in gates {
        let gate_id = record_str(gate, "/gate_id", "PROMOTION_GATE_INVALID");
        if !actual.insert(gate_id) || gate.get("result").and_then(Value::as_str) != Some("pass") {
            fail("PROMOTION_GATE_INVALID");
        }
        if gate
            .get("producer")
            .and_then(Value::as_str)
            .filter(|producer| !producer.is_empty())
            .is_none()
        {
            fail("PROMOTION_GATE_INVALID");
        }
    }
    if actual.len() != expected.len() || expected.iter().any(|gate_id| !actual.contains(gate_id)) {
        fail("PROMOTION_GATE_SET_MISMATCH");
    }
}

pub(super) fn lifecycle_chain_merge(root: &Path, module_id: &str, input_path: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    assert_declared_contracts(root, &project);
    assert_goal_confirmed(root);
    let observation = lifecycle_chain_input(root, input_path, "merge");
    let (worktree, _reproduction, candidate, _validation) =
        lifecycle_chain_candidate(root, module_id);
    let effectiveness = read_record(root, &module_record_name("effectiveness-record", module_id));
    let issue_id = producer_string(&worktree, "/issue_id", "worktree-record.json");
    let candidate_id =
        producer_string(&candidate, "/fix_candidate_id", "fix-candidate-record.json");
    let candidate_commit = producer_string(&candidate, "/head_commit", "fix-candidate-record.json");
    let candidate_tree = producer_string(&candidate, "/tree_hash", "fix-candidate-record.json");
    let mainline_ref = producer_string(&observation, "/mainline_ref", "MERGE_MAINLINE_REF_MISSING");
    let merge_commit = git_value(root, &["rev-parse", "HEAD"], "MERGE_COMMIT_UNAVAILABLE");
    let merged_tree = git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", merge_commit)],
        "MERGE_TREE_UNAVAILABLE",
    );
    if git_value(
        root,
        &["rev-parse", &format!("{}^{{commit}}", mainline_ref)],
        "MERGE_MAINLINE_REF_MISSING",
    ) != merge_commit
    {
        fail("MERGE_MAINLINE_HEAD_MISMATCH");
    }
    if git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", candidate_commit)],
        "FIX_CANDIDATE_COMMIT_MISSING",
    ) != candidate_tree
        || !Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "merge-base",
                "--is-ancestor",
                candidate_commit.as_str(),
                merge_commit.as_str(),
            ])
            .status()
            .is_ok_and(|status| status.success())
    {
        fail("MERGE_CANDIDATE_IDENTITY_MISMATCH");
    }
    let change_identity = if candidate_commit == merge_commit {
        "exact"
    } else {
        let requested = producer_string(
            &observation,
            "/change_identity",
            "MERGE_CHANGE_IDENTITY_MISSING",
        );
        if requested != "tested_integration_exact" {
            fail("MERGE_CHANGE_IDENTITY_REQUIRED");
        }
        "tested_integration_exact"
    };
    assert_lifecycle_chain_candidate_at_head(
        root,
        &project,
        module_id,
        &candidate_commit,
        &candidate_tree,
    );
    if effectiveness.get("result").and_then(Value::as_str) != Some("pass")
        || effectiveness.get("module_id").and_then(Value::as_str) != Some(module_id)
        || effectiveness.get("issue_id").and_then(Value::as_str) != Some(&issue_id)
        || effectiveness
            .get("fix_candidate_id")
            .and_then(Value::as_str)
            != Some(&candidate_id)
        || effectiveness.get("reviewed_commit").and_then(Value::as_str) != Some(&candidate_commit)
        || effectiveness
            .get("reviewed_tree_hash")
            .and_then(Value::as_str)
            != Some(&candidate_tree)
    {
        fail("POST_ARCHITECTURE_EFFECTIVENESS_MISMATCH");
    }
    let merge_time = Utc::now();
    let merge_id = producer_stable_id(
        "merge",
        &serde_json::json!({"fix_candidate_id": candidate_id, "effectiveness_id": effectiveness["effectiveness_id"], "merge_commit": merge_commit, "mainline_ref": mainline_ref}),
    );
    let merge = serde_json::json!({
        "merge_id": merge_id,
        "issue_id": issue_id,
        "module_id": module_id,
        "fix_candidate_id": candidate_id,
        "effectiveness_id": effectiveness["effectiveness_id"].clone(),
        "mainline_ref": mainline_ref,
        "candidate_commit": candidate_commit,
        "merge_commit": merge_commit,
        "candidate_tree_hash": candidate_tree,
        "merged_tree_hash": merged_tree,
        "change_identity": change_identity,
        "result": "pass",
        "created_at": merge_time.to_rfc3339()
    });
    let merge_name = module_record_name("merge-record", module_id);
    // A merge cache hit must continue to prove the effectiveness graph; the
    // check only reads existing records and Git identity.
    assert_fix_effectiveness_gate(root, module_id);
    if let Some(existing) = lifecycle_chain_read_record_if_present(root, module_id, "merge-record")
    {
        if lifecycle_chain_record_is_pass("merge-record", &existing) {
            let existing_record_hash = sha256(&canonical(&existing));
            let existing_attempt_id =
                lifecycle_chain_attempt_identity(module_id, "merge-record", &existing_record_hash);
            lifecycle_chain_validate_attempt_ledger(
                root,
                module_id,
                "merge-record",
                &existing_attempt_id,
                &existing_record_hash,
                &existing,
            );
            if producer_without_fields(&existing, &["created_at"])
                == producer_without_fields(&merge, &["created_at"])
            {
                lifecycle_chain_assert_reusable_record(
                    &existing,
                    &merge,
                    &merge_name,
                    "MERGE_RECORD_IDENTITY_MISMATCH",
                    true,
                );
                lifecycle_chain_output(&existing, true);
                return;
            }
        }
    }
    let reused = lifecycle_chain_write_record(root, module_id, "merge-record", &merge);
    lifecycle_chain_output(&merge, reused);
}

pub(super) fn lifecycle_chain_promotion(root: &Path, module_id: &str, input_path: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    assert_declared_contracts(root, &project);
    assert_goal_confirmed(root);
    assert_lifecycle_producer_map_binding(root, &project, module_id, LifecycleProducer::Chain);
    let observation = lifecycle_chain_input(root, input_path, "promotion");
    let (worktree, reproduction, candidate, _validation) =
        lifecycle_chain_candidate(root, module_id);
    let review = read_record(root, &module_record_name("review-record", module_id));
    let effectiveness = read_record(root, &module_record_name("effectiveness-record", module_id));
    let merge = read_record(root, &module_record_name("merge-record", module_id));
    let artifact = read_module_artifact(root, &project, module_id);
    let module = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    let scenarios = assert_development_scenarios(root, &project);
    let parallel_development = scenarios.multi_worktree_merge_queue;
    let collaboration_development = scenarios.multi_worker_collaboration;
    module_artifact_matches_project(module, &artifact);
    let issue_id = producer_string(&worktree, "/issue_id", "worktree-record.json");
    let candidate_id =
        producer_string(&candidate, "/fix_candidate_id", "fix-candidate-record.json");
    let merge_commit = producer_string(&merge, "/merge_commit", "merge-record.json");
    let artifact_hash = producer_string(&artifact, "/artifact_hash", "module-artifact");
    let scope_hash = producer_string(&candidate, "/scope_hash", "fix-candidate-record.json");
    let public_api_hash = producer_string(&artifact, "/public_api_hash", "module-artifact");
    if git_value(root, &["rev-parse", "HEAD"], "PROMOTION_HEAD_UNAVAILABLE") != merge_commit {
        fail("PROMOTION_MERGE_HEAD_MISMATCH");
    }
    if merge.get("result").and_then(Value::as_str) != Some("pass")
        || merge.get("issue_id").and_then(Value::as_str) != Some(&issue_id)
        || merge.get("module_id").and_then(Value::as_str) != Some(module_id)
        || merge.get("fix_candidate_id").and_then(Value::as_str) != Some(&candidate_id)
        || merge.get("effectiveness_id") != effectiveness.get("effectiveness_id")
        || git_value(
            root,
            &["rev-parse", &format!("{}^{{tree}}", merge_commit)],
            "PROMOTION_MERGE_TREE_UNAVAILABLE",
        ) != producer_string(&merge, "/merged_tree_hash", "merge-record.json")
    {
        fail("MAINLINE_MERGE_RECORD_MISMATCH");
    }
    let experiment_id = producer_string(
        &observation,
        "/experiment_id",
        "PROMOTION_EXPERIMENT_MISSING",
    );
    let new_version = producer_string(
        &observation,
        "/new_active_version",
        "PROMOTION_NEW_VERSION_MISSING",
    );
    assert_version(&new_version, "INVALID_ACTIVE_VERSION");
    let previous = if let Some(base) = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|m| m.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .and_then(|m| m.get("version_base"))
        .filter(|value| !value.is_null())
    {
        Value::String(
            record_str(base, "/previous_active_version", "module-version-base").to_string(),
        )
    } else {
        observation
            .get("previous_active_version")
            .filter(|value| value.is_null() || value.as_str().is_some())
            .cloned()
            .unwrap_or_else(|| fail("PROMOTION_PREVIOUS_VERSION_MISSING"))
    };
    let compatibility = producer_string(
        &observation,
        "/compatibility_level",
        "PROMOTION_COMPATIBILITY_MISSING",
    );
    if !matches!(
        compatibility.as_str(),
        "compatible" | "migration_required" | "breaking"
    ) {
        fail("PROMOTION_COMPATIBILITY_INVALID");
    }
    let evidence_ids =
        lifecycle_chain_required_array(&observation, "/evidence_ids", "PROMOTION_EVIDENCE_MISSING");
    for id in &evidence_ids {
        lifecycle_chain_validate_evidence(
            root,
            module_id,
            id,
            &issue_id,
            &scope_hash,
            &merge_commit,
        );
    }
    let gates = observation
        .get("required_gate_results")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .cloned()
        .unwrap_or_else(|| fail("PROMOTION_GATES_MISSING"));
    assert_lifecycle_chain_promotion_gates(&gates);
    for gate in &gates {
        if producer_string(gate, "/gate_id", "PROMOTION_GATE_INVALID") == ""
            || producer_string(gate, "/producer", "PROMOTION_GATE_INVALID") == ""
            || producer_string(gate, "/result", "PROMOTION_GATE_INVALID") != "pass"
        {
            fail("PROMOTION_GATE_INVALID");
        }
    }
    let cleanup_id = producer_string(
        &observation,
        "/playground_cleanup_record_id",
        "PROMOTION_CLEANUP_MISSING",
    );
    let cleanup = read_record(root, &format!("playground-cleanup-{}.json", cleanup_id));
    if producer_string(&cleanup, "/cleanup_id", "playground-cleanup-record") != cleanup_id {
        fail("PROMOTION_CLEANUP_MISMATCH");
    }
    let promotion_id = lifecycle_chain_promotion_id(&issue_id, module_id, &candidate_id);
    let promotion_time = Utc::now();
    let mut promotion = serde_json::json!({
        "promotion_id": promotion_id,
        "issue_id": issue_id,
        "experiment_id": experiment_id,
        "module_id": module_id,
        "worktree_record_id": worktree["worktree_id"].clone(),
        "reproduction_record_id": reproduction["reproduction_id"].clone(),
        "fix_candidate_id": candidate_id,
        "architecture_review_id": review["review_id"].clone(),
        "effectiveness_record_id": effectiveness["effectiveness_id"].clone(),
        "merge_record_id": merge["merge_id"].clone(),
        "base_commit": worktree["base_commit"].clone(),
        "candidate_commit": candidate["head_commit"].clone(),
        "merged_commit": merge_commit.clone(),
        "source_commit": merge_commit,
        "previous_active_version": previous,
        "new_active_version": new_version,
        "artifact_hash": artifact_hash,
        "scope_hash": scope_hash,
        "public_api_hash": public_api_hash,
        "review_id": review["review_id"].clone(),
        "evidence_ids": evidence_ids,
        "required_gate_results": gates,
        "change_set_id": producer_string(&observation, "/change_set_id", "PROMOTION_CHANGE_SET_MISSING"),
        "compatibility_level": compatibility,
        "root_cause": producer_string(&observation, "/root_cause", "PROMOTION_ROOT_CAUSE_MISSING"),
        "design_id": producer_string(&observation, "/design_id", "PROMOTION_DESIGN_MISSING"),
        "change_reason_comment": producer_string(&observation, "/change_reason_comment", "PROMOTION_REASON_MISSING"),
        "playground_cleanup_record_id": cleanup_id,
        // The producer owns the closure claim.  It is written only after the
        // authoritative git-bug closure check below succeeds; callers cannot
        // turn this into a cache hit by supplying a boolean in the input.
        "bug_closure_verified": true,
        "created_at": promotion_time.to_rfc3339()
    });
    if collaboration_development {
        for (field, error) in [
            (
                "collaboration_record_id",
                "PROMOTION_COLLABORATION_RECORD_MISSING",
            ),
            (
                "merge_queue_record_id",
                "PROMOTION_MERGE_QUEUE_RECORD_MISSING",
            ),
            (
                "integration_record_id",
                "PROMOTION_INTEGRATION_RECORD_MISSING",
            ),
            (
                "mainline_receipt_record_id",
                "PROMOTION_MAINLINE_RECEIPT_RECORD_MISSING",
            ),
            (
                "collab_live_closure_record_id",
                "PROMOTION_COLLAB_LIVE_CLOSURE_RECORD_MISSING",
            ),
        ] {
            promotion[field] =
                Value::String(producer_string(&observation, &format!("/{}", field), error));
        }
    }
    // Validate the merge graph before looking for a cached promotion.  In
    // parallel mode the first promotion has no persisted promotion record yet,
    // so validate against the candidate projection just constructed.
    if parallel_development {
        assert_parallel_merge_gate_for_promotion(root, module_id, &promotion);
    } else {
        assert_single_merge_gate(root, module_id);
    }
    if collaboration_development {
        assert_collab_live_closure(
            root,
            module_id,
            &promotion,
            &issue_id,
            &candidate_id,
            &artifact_hash,
            &scope_hash,
            &merge_commit,
        );
    }
    // A promotion is the closure boundary for a tracked defect.  Validate the
    // current bug state on every invocation, including cache probes, so a
    // reopened or otherwise unverifiable issue can never reuse an old PASS.
    assert_bug_tracker_solution_evidence(root, &issue_id, &promotion);
    if let Some(existing) =
        lifecycle_chain_read_record_if_present(root, module_id, "promotion-record")
    {
        if lifecycle_chain_record_is_pass("promotion-record", &existing) {
            let existing_record_hash = sha256(&canonical(&existing));
            let existing_attempt_id = lifecycle_chain_attempt_identity(
                module_id,
                "promotion-record",
                &existing_record_hash,
            );
            lifecycle_chain_validate_attempt_ledger(
                root,
                module_id,
                "promotion-record",
                &existing_attempt_id,
                &existing_record_hash,
                &existing,
            );
            if producer_without_fields(&existing, &["created_at"])
                == producer_without_fields(&promotion, &["created_at"])
            {
                lifecycle_chain_assert_reusable_record(
                    &existing,
                    &promotion,
                    &module_record_name("promotion-record", module_id),
                    "PROMOTION_RECORD_IDENTITY_MISMATCH",
                    true,
                );
                lifecycle_chain_output(&existing, true);
                return;
            }
        }
    }
    let reused = lifecycle_chain_write_record(root, module_id, "promotion-record", &promotion);
    lifecycle_chain_output(&promotion, reused);
}
