#[test]
fn lifecycle_producers_accept_v4_project_map_projection_shape() {
    let root = temp_root("lifecycle-producer-v4-project-maps");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let resource_map_path = root.join(".appsdk/maps/resource-map.json");
    let mut resource_map: Value =
        serde_json::from_str(&fs::read_to_string(&resource_map_path).unwrap()).unwrap();
    resource_map["resources"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| {
            !matches!(
                entry.get("resource_id").and_then(Value::as_str),
                Some("lifecycle_record_producer_input")
                    | Some("lifecycle_chain_producer_input")
                    | Some("fix_reproduction")
            )
        });
    for id in ["fix_worktree", "fix_evidence_set"] {
        resource_map["resources"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|entry| entry["resource_id"] == id)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("relations");
    }
    fs::write(
        &resource_map_path,
        serde_json::to_string_pretty(&resource_map).unwrap() + "\n",
    )
    .unwrap();

    for (map_name, key, ids) in [
        (
            "function-map.json",
            "functions",
            vec![
                "lifecycle_record_producer",
                "lifecycle_chain_record_producer",
            ],
        ),
        (
            "mainline-call-map.json",
            "edges",
            vec![
                "lifecycle-record-production-v1",
                "lifecycle-record-chain-production-v1",
            ],
        ),
        (
            "verification-map.json",
            "gates",
            vec!["lifecycle_chain_record_producer"],
        ),
    ] {
        let map_path = root.join(".appsdk/maps").join(map_name);
        let mut map: Value = serde_json::from_str(&fs::read_to_string(&map_path).unwrap()).unwrap();
        map[key].as_array_mut().unwrap().retain(|entry| {
            !ids.iter().any(|id| {
                entry
                    .get("function_id")
                    .or_else(|| entry.get("chain_id"))
                    .or_else(|| entry.get("gate_id"))
                    .and_then(Value::as_str)
                    == Some(id)
            })
        });
        fs::write(
            &map_path,
            serde_json::to_string_pretty(&map).unwrap() + "\n",
        )
        .unwrap();
    }

    let input = root.join("producer-input.json");
    fs::write(&input, "{}\n").unwrap();
    let records = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!records.status.success());
    let records_stderr = String::from_utf8_lossy(&records.stderr);
    assert!(records_stderr.contains("GOAL_NOT_CONFIRMED:received"));
    assert!(!records_stderr.contains("LIFECYCLE_PRODUCER_MAP_TAMPERED"));

    let goal_path = root.join(".appsdk/goal.json");
    let mut goal: Value = serde_json::from_str(&fs::read_to_string(&goal_path).unwrap()).unwrap();
    goal["status"] = Value::String("confirmed".into());
    goal["confirmed_by"] = Value::String("test".into());
    goal["confirmed_at"] = Value::String("2026-01-01T00:00:00Z".into());
    fs::write(
        &goal_path,
        serde_json::to_string_pretty(&goal).unwrap() + "\n",
    )
    .unwrap();
    let chain = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "architecture",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!chain.status.success());
    let chain_stderr = String::from_utf8_lossy(&chain.stderr);
    assert!(chain_stderr.contains("PRODUCER_ARCHITECTURE_INPUT_MISSING"));
    assert!(!chain_stderr.contains("LIFECYCLE_PRODUCER_MAP_TAMPERED"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_producer_rejects_tampered_relations_projection() {
    let root = temp_root("lifecycle-producer-map-relations-tampered");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let map_path = root.join(".appsdk/maps/resource-map.json");
    let mut map: Value = serde_json::from_str(&fs::read_to_string(&map_path).unwrap()).unwrap();
    let entry = map["resources"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["resource_id"] == "fix_worktree")
        .unwrap();
    entry["relations"] = serde_json::json!({"produced_by":["tampered_producer"]});
    fs::write(
        &map_path,
        serde_json::to_string_pretty(&map).unwrap() + "\n",
    )
    .unwrap();
    let input = root.join("producer-input.json");
    fs::write(&input, "{}\n").unwrap();
    let rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("LIFECYCLE_PRODUCER_MAP_TAMPERED:resource-map.json"));
    assert!(!root
        .join(".appsdk/records/worktree-record-app-core.json")
        .exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_producer_rejects_duplicate_compatible_projection() {
    let root = temp_root("lifecycle-producer-map-duplicate-compatible");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let map_path = root.join(".appsdk/maps/resource-map.json");
    let mut map: Value = serde_json::from_str(&fs::read_to_string(&map_path).unwrap()).unwrap();
    let entry = map["resources"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["resource_id"] == "fix_worktree")
        .unwrap();
    entry.as_object_mut().unwrap().remove("relations");
    let duplicate = entry.clone();
    map["resources"].as_array_mut().unwrap().push(duplicate);
    fs::write(
        &map_path,
        serde_json::to_string_pretty(&map).unwrap() + "\n",
    )
    .unwrap();
    let input = root.join("producer-input.json");
    fs::write(&input, "{}\n").unwrap();
    let rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("LIFECYCLE_PRODUCER_MAP_TAMPERED:resource-map.json"));
    assert!(!root
        .join(".appsdk/records/worktree-record-app-core.json")
        .exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_producer_accepts_project_module_aggregating_registry_modules() {
    let root = temp_root("lifecycle-producer-aggregated-registry-modules");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["modules"][0]["module_id"] = Value::String("aggregate-core".into());
    project["modules"][0]["source_owner"] = Value::String("aggregate-core".into());
    project["modules"][0]["owned_paths"] = serde_json::json!([
        "playground/experiments/**",
        "protected/source/**",
        "tests/core/**"
    ]);
    project["modules"][0]["registry_binding"] = serde_json::json!({
        "mode": "aggregate",
        "modules": ["app-core", "source-contracts"]
    });
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();

    let registry_path = root.join(".appsdk/maps/module-registry.json");
    let mut registry: Value =
        serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
    let app_core = registry["modules"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|module| module["module_id"] == "app-core")
        .unwrap();
    app_core["owned_paths"] = serde_json::json!(["playground/experiments/**"]);
    registry["modules"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "module_id": "source-contracts",
            "status": "active",
            "owner": "source-contracts",
            "owned_paths": ["protected/source/**", "tests/core/**"],
            "forbidden_paths": ["active/lib/**", "generated/**"]
        }));
    fs::write(
        &registry_path,
        serde_json::to_string_pretty(&registry).unwrap() + "\n",
    )
    .unwrap();

    let input = root.join("producer-input.json");
    fs::write(&input, "{}\n").unwrap();
    let rejected_for_missing_goal = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "aggregate-core",
        "--input",
        input.to_str().unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&rejected_for_missing_goal.stderr);
    assert!(!rejected_for_missing_goal.status.success());
    assert!(stderr.contains("GOAL_NOT_CONFIRMED:received"), "{stderr}");
    assert!(
        !stderr.contains("LIFECYCLE_PRODUCER_MODULE_BINDING"),
        "{stderr}"
    );

    project["modules"][0]["registry_binding"]["modules"] =
        serde_json::json!(["app-core", "missing-module"]);
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let missing_registry_module = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "aggregate-core",
        "--input",
        input.to_str().unwrap(),
    ]);
    let missing_registry_stderr = String::from_utf8_lossy(&missing_registry_module.stderr);
    assert!(!missing_registry_module.status.success());
    assert!(
        missing_registry_stderr
            .contains("LIFECYCLE_PRODUCER_MODULE_REGISTRY_INVALID:missing-module"),
        "{missing_registry_stderr}"
    );

    project["modules"][0]["registry_binding"]["modules"] =
        serde_json::json!(["app-core", "source-contracts"]);
    registry["modules"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|module| module["module_id"] == "source-contracts")
        .unwrap()["status"] = Value::String("inactive".into());
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    fs::write(
        &registry_path,
        serde_json::to_string_pretty(&registry).unwrap() + "\n",
    )
    .unwrap();
    let inactive_registry_module = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "aggregate-core",
        "--input",
        input.to_str().unwrap(),
    ]);
    let inactive_registry_stderr = String::from_utf8_lossy(&inactive_registry_module.stderr);
    assert!(!inactive_registry_module.status.success());
    assert!(
        inactive_registry_stderr
            .contains("LIFECYCLE_PRODUCER_MODULE_BINDING_MISMATCH:source-contracts"),
        "{inactive_registry_stderr}"
    );

    registry["modules"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|module| module["module_id"] == "source-contracts")
        .unwrap()["status"] = Value::String("active".into());
    fs::write(
        &registry_path,
        serde_json::to_string_pretty(&registry).unwrap() + "\n",
    )
    .unwrap();

    let uncovered = root.join(".appsdk/project.json");
    project["modules"][0]["owned_paths"] =
        serde_json::json!(["playground/experiments/**", "unregistered/**"]);
    fs::write(
        &uncovered,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let rejected_for_uncovered_path = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "aggregate-core",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!rejected_for_uncovered_path.status.success());
    assert!(String::from_utf8_lossy(&rejected_for_uncovered_path.stderr)
        .contains("LIFECYCLE_PRODUCER_MODULE_BINDING_MISMATCH"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_producer_rejects_same_id_project_module_borrowing_other_registry_paths() {
    let root = temp_root("lifecycle-producer-same-id-path-borrow");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["modules"][0]["owned_paths"] =
        serde_json::json!(["playground/experiments/**", "tests/core/**"]);
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();

    let registry_path = root.join(".appsdk/maps/module-registry.json");
    let mut registry: Value =
        serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
    let app_core = registry["modules"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|module| module["module_id"] == "app-core")
        .unwrap();
    app_core["owned_paths"] = serde_json::json!(["playground/experiments/**"]);
    registry["modules"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "module_id": "other-owner",
            "status": "active",
            "owner": "other-owner",
            "owned_paths": ["tests/core/**"],
            "forbidden_paths": ["active/lib/**", "generated/**"]
        }));
    fs::write(
        &registry_path,
        serde_json::to_string_pretty(&registry).unwrap() + "\n",
    )
    .unwrap();

    let input = root.join("producer-input.json");
    fs::write(&input, "{}\n").unwrap();
    let rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input.to_str().unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(!rejected.status.success());
    assert!(
        stderr.contains("LIFECYCLE_PRODUCER_MODULE_BINDING_MISMATCH"),
        "{stderr}"
    );
    assert!(!root
        .join(".appsdk/records/worktree-record-app-core.json")
        .exists());

    project["modules"][0]["owned_paths"] = serde_json::json!(["playground/experiments/**"]);
    project["modules"][0]["registry_binding"] = serde_json::json!({
        "mode": "aggregate",
        "modules": ["other-owner"]
    });
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    let omitted_same_id = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input.to_str().unwrap(),
    ]);
    let omitted_same_id_stderr = String::from_utf8_lossy(&omitted_same_id.stderr);
    assert!(!omitted_same_id.status.success());
    assert!(
        omitted_same_id_stderr.contains("LIFECYCLE_PRODUCER_MODULE_BINDING_MISSING"),
        "{omitted_same_id_stderr}"
    );
    assert!(!root
        .join(".appsdk/records/worktree-record-app-core.json")
        .exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_contract_rejects_invalid_registry_binding_shapes() {
    let cases = [
        (
            "missing-aggregate-modules",
            serde_json::json!({"mode": "aggregate"}),
        ),
        (
            "empty-aggregate-modules",
            serde_json::json!({"mode": "aggregate", "modules": []}),
        ),
        (
            "duplicate-aggregate-modules",
            serde_json::json!({"mode": "aggregate", "modules": ["app-core", "app-core"]}),
        ),
        (
            "non-string-aggregate-module",
            serde_json::json!({"mode": "aggregate", "modules": [1]}),
        ),
        (
            "empty-aggregate-module",
            serde_json::json!({"mode": "aggregate", "modules": [""]}),
        ),
        (
            "exact-modules-forbidden",
            serde_json::json!({"mode": "exact", "modules": ["app-core"]}),
        ),
        ("unknown-mode", serde_json::json!({"mode": "implicit"})),
        (
            "unknown-property",
            serde_json::json!({"mode": "exact", "unexpected": true}),
        ),
    ];
    for (name, binding) in cases {
        let root = temp_root(&format!("registry-binding-contract-{name}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        let project_path = root.join(".appsdk/project.json");
        let mut project: Value =
            serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
        project["modules"][0]["registry_binding"] = binding;
        fs::write(
            &project_path,
            serde_json::to_string_pretty(&project).unwrap() + "\n",
        )
        .unwrap();
        let rejected = run(&["verify", root_text]);
        let stderr = String::from_utf8_lossy(&rejected.stderr);
        assert!(
            !rejected.status.success(),
            "case {name} unexpectedly passed"
        );
        assert!(
            stderr.contains("INVALID_REGISTRY_BINDING:app-core"),
            "case {name}: stderr={stderr}"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn embedded_project_schema_declares_registry_binding_contract() {
    let root = temp_root("registry-binding-schema");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let schema: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/contracts/project.schema.json")).unwrap(),
    )
    .unwrap();
    let binding = schema
        .pointer("/properties/modules/items/properties/registry_binding")
        .unwrap();
    assert_eq!(binding["type"], "object");
    assert_eq!(binding["additionalProperties"], false);
    assert_eq!(binding["required"], serde_json::json!(["mode"]));
    assert_eq!(
        binding["properties"]["mode"]["enum"],
        serde_json::json!(["exact", "aggregate"])
    );
    assert_eq!(binding["properties"]["modules"]["uniqueItems"], true);
    assert_eq!(binding["properties"]["modules"]["minItems"], 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_producer_rejects_unknown_phase_without_mutating_records() {
    let root = temp_root("lifecycle-chain-invalid-phase");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let input = root.join("chain-input.json");
    fs::write(&input, "{}\n").unwrap();
    let rejected = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "unknown",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("PRODUCER_PHASE_INVALID"));
    assert!(!root
        .join(".appsdk/records/review-record-app-core.json")
        .exists());
    assert!(!root
        .join(".appsdk/records/effectiveness-record-app-core.json")
        .exists());
    assert!(!root
        .join(".appsdk/records/merge-record-app-core.json")
        .exists());
    assert!(!root
        .join(".appsdk/records/promotion-record-app-core.json")
        .exists());
    fs::remove_file(input).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_reenters_non_pass_review_and_preserves_attempt_history() {
    for initial_verdict in ["fail", "unknown"] {
        let root = temp_root(&format!("lifecycle-chain-reentry-{initial_verdict}"));
        let root_text = root.to_str().unwrap();
        prepare_lifecycle_chain_fixture(&root);
        let requirements_review = real_requirements_review(&root);
        let review = root.join(".appsdk/records/review-record-app-core.json");
        fs::remove_file(&review).unwrap();
        let input = root.join("architecture-input.json");
        let write_input = |verdict: &str| {
            let mut architecture = serde_json::json!({
                "reviewer": {"adapter":"test","identity":"test"},
                "verdict": verdict,
                "evidence_ids": ["candidate-evidence-1","positive-1","negative-1"]
            });
            if verdict == "pass" {
                architecture["requirements_review"] = requirements_review.clone();
            }
            fs::write(
                &input,
                serde_json::to_string_pretty(&serde_json::json!({"architecture": architecture}))
                    .unwrap()
                    + "\n",
            )
            .unwrap();
        };

        write_input(initial_verdict);
        let first = run(&[
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
            first.status.success(),
            "verdict={initial_verdict} stdout={} stderr={}",
            String::from_utf8_lossy(&first.stdout),
            String::from_utf8_lossy(&first.stderr)
        );
        let repeated = run(&[
            "produce-lifecycle-chain",
            root_text,
            "--module",
            "app-core",
            "--phase",
            "architecture",
            "--input",
            input.to_str().unwrap(),
        ]);
        assert!(!repeated.status.success());
        assert!(
            String::from_utf8_lossy(&repeated.stderr).contains("LIFECYCLE_CHAIN_STAGE_NOT_PASS")
        );

        write_input("pass");
        let reentered = run(&[
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
            reentered.status.success(),
            "verdict={initial_verdict} stdout={} stderr={}",
            String::from_utf8_lossy(&reentered.stdout),
            String::from_utf8_lossy(&reentered.stderr)
        );
        let reentered_json: Value = serde_json::from_slice(&reentered.stdout).unwrap();
        assert_eq!(reentered_json["verdict"], "pass");
        assert_eq!(reentered_json["reused"], false);
        let attempts = root.join(".appsdk/records/attempts/app-core/review-record.jsonl");
        let attempts_text = fs::read_to_string(&attempts).unwrap();
        assert_eq!(attempts_text.lines().count(), 1);
        let attempt: Value = serde_json::from_str(attempts_text.lines().next().unwrap()).unwrap();
        assert_eq!(attempt["phase"], "review-record");
        assert_eq!(attempt["record"]["verdict"], initial_verdict);

        let reused = run(&[
            "produce-lifecycle-chain",
            root_text,
            "--module",
            "app-core",
            "--phase",
            "architecture",
            "--input",
            input.to_str().unwrap(),
        ]);
        assert!(reused.status.success());
        let reused_json: Value = serde_json::from_slice(&reused.stdout).unwrap();
        assert_eq!(reused_json["reused"], true);
        assert_eq!(fs::read_to_string(&attempts).unwrap().lines().count(), 1);

        // The append-only ledger is part of the evidence chain.  A forged
        // envelope must fail closed before a new projection can replace the
        // current non-pass record.
        fs::remove_file(&review).unwrap();
        write_input("fail");
        let non_pass = run(&[
            "produce-lifecycle-chain",
            root_text,
            "--module",
            "app-core",
            "--phase",
            "architecture",
            "--input",
            input.to_str().unwrap(),
        ]);
        assert!(non_pass.status.success());
        write_input("pass");
        let review_before_tamper = fs::read(&review).unwrap();
        let ledger_before_tamper = fs::read(&attempts).unwrap();
        let mut non_pass_review: Value = serde_json::from_slice(&review_before_tamper).unwrap();
        non_pass_review["verdict"] = Value::String("fail".into());
        fs::write(
            &review,
            serde_json::to_string_pretty(&non_pass_review).unwrap() + "\n",
        )
        .unwrap();
        let mut forged_attempt: Value = serde_json::from_str(
            String::from_utf8_lossy(&ledger_before_tamper)
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        forged_attempt["result"] = Value::String("pass".into());
        fs::write(
            &attempts,
            serde_json::to_string(&forged_attempt).unwrap() + "\n",
        )
        .unwrap();
        let forged = run(&[
            "produce-lifecycle-chain",
            root_text,
            "--module",
            "app-core",
            "--phase",
            "architecture",
            "--input",
            input.to_str().unwrap(),
        ]);
        assert!(!forged.status.success());
        assert!(
            String::from_utf8_lossy(&forged.stderr)
                .contains("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID"),
            "forged stderr={} stdout={}",
            String::from_utf8_lossy(&forged.stderr),
            String::from_utf8_lossy(&forged.stdout)
        );
        fs::write(&review, review_before_tamper).unwrap();
        fs::write(&attempts, ledger_before_tamper).unwrap();
        let recovered = run(&[
            "produce-lifecycle-chain",
            root_text,
            "--module",
            "app-core",
            "--phase",
            "architecture",
            "--input",
            input.to_str().unwrap(),
        ]);
        assert!(recovered.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&recovered.stdout).unwrap()["reused"],
            false
        );
        let recovered_again = run(&[
            "produce-lifecycle-chain",
            root_text,
            "--module",
            "app-core",
            "--phase",
            "architecture",
            "--input",
            input.to_str().unwrap(),
        ]);
        assert!(recovered_again.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&recovered_again.stdout).unwrap()["reused"],
            true
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn lifecycle_chain_new_candidate_preserves_pass_bytes_and_reuses_current() {
    let root = temp_root("lifecycle-chain-new-candidate");
    let root_text = root.to_str().unwrap();
    let artifact_hash = prepare_lifecycle_chain_fixture(&root);
    let records = root.join(".appsdk/records");
    let kinds = [
        "review-record",
        "effectiveness-record",
        "merge-record",
        "promotion-record",
    ];
    let previous_commit = git_test_value(&root, &["rev-parse", "HEAD"]);
    // Whitespace is intentional: preserving the parsed value alone loses the witness bytes.
    let previous: Vec<_> = kinds
        .iter()
        .map(|kind| {
            let file = records.join(format!("{kind}-app-core.json"));
            let bytes = format!(" \n{}\n", fs::read_to_string(&file).unwrap());
            fs::write(&file, &bytes).unwrap();
            bytes
        })
        .collect();
    fs::write(root.join("candidate-source-change.txt"), "new candidate\n").unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "candidate-source-change.txt"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "new source candidate"])
        .status()
        .unwrap()
        .success());
    let current_commit = git_test_value(&root, &["rev-parse", "HEAD"]);
    assert_ne!(previous_commit, current_commit);
    // Upstream evidence is a fixture for the new source. All four old PASS files remain
    // present while the real CLI performs each downstream transition.
    write_records(&root, "app-core", &artifact_hash, false, "issue-1");
    for (kind, bytes) in kinds.iter().zip(&previous) {
        fs::write(records.join(format!("{kind}-app-core.json")), bytes).unwrap();
    }
    for (kind, id_field, id) in [
        ("fix-candidate-record", "fix_candidate_id", "candidate-2"),
        (
            "pre-review-validation-record",
            "validation_id",
            "validation-2",
        ),
    ] {
        let file = records.join(format!("{kind}-app-core.json"));
        let mut value: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        value[id_field] = serde_json::json!(id);
        value["fix_candidate_id"] = serde_json::json!("candidate-2");
        fs::write(file, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    }
    let requirements_review = real_requirements_review(&root);
    let observations = [
        (
            "architecture",
            serde_json::json!({"reviewer":{"adapter":"test","identity":"test"},"verdict":"pass","evidence_ids":["candidate-evidence-1","positive-1","negative-1"],"requirements_review":requirements_review}),
        ),
        (
            "effectiveness",
            serde_json::json!({"fixed_replay_evidence_id":"blackbox-1","positive_evidence_ids":["positive-1"],"negative_evidence_ids":["negative-1"],"blackbox_evidence_ids":["blackbox-1"]}),
        ),
        ("merge", serde_json::json!({"mainline_ref":"HEAD"})),
        (
            "promotion",
            serde_json::json!({
                "experiment_id":"experiment-2","new_active_version":"active-v2","previous_active_version":null,
                "compatibility_level":"compatible","evidence_ids":["candidate-evidence-1"],
                "required_gate_results":[
                    {"gate_id":"goal_confirmed","result":"pass","producer":"test"},
                    {"gate_id":"contract_valid","result":"pass","producer":"test"},
                    {"gate_id":"sdk_lock_integrity","result":"pass","producer":"test"},
                    {"gate_id":"remote_main_receipt","result":"pass","producer":"test"},
                    {"gate_id":"mainline_merge_identity","result":"pass","producer":"test"},
                    {"gate_id":"fix_lifecycle_graph","result":"pass","producer":"test"},
                    {"gate_id":"artifact_hash","result":"pass","producer":"test"},
                    {"gate_id":"lifecycle_chain_record_producer","result":"pass","producer":"test"}
                ],"change_set_id":"change-2","root_cause":"root cause","design_id":"design-1",
                "change_reason_comment":"reason","playground_cleanup_record_id":"cleanup-1","artifact_hash":artifact_hash
            }),
        ),
    ];
    for ((phase, observation), (kind, old_bytes)) in
        observations.iter().zip(kinds.iter().zip(&previous))
    {
        let input = root.join(format!("{phase}-input.json"));
        fs::write(
            &input,
            serde_json::to_vec(&serde_json::json!({*phase:observation})).unwrap(),
        )
        .unwrap();
        let produce = || {
            run(&[
                "produce-lifecycle-chain",
                root_text,
                "--module",
                "app-core",
                "--phase",
                phase,
                "--input",
                input.to_str().unwrap(),
            ])
        };
        let output = produce();
        assert!(
            output.status.success(),
            "{phase}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            value["reused"], false,
            "{phase} must not reuse the previous candidate"
        );
        assert_eq!(value["fix_candidate_id"], "candidate-2");
        let file = records.join(format!("{kind}-app-core.json"));
        let current = fs::read(&file).unwrap();
        let ledger = records.join(format!("attempts/app-core/{kind}.jsonl"));
        let history = fs::read(&ledger).unwrap();
        let entries: Vec<Value> = String::from_utf8(history.clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["result"], "stale");
        assert_eq!(
            entries[0]["record"],
            serde_json::from_str::<Value>(old_bytes).unwrap()
        );
        assert_eq!(
            entries[0]["record_json"].as_str(),
            Some(old_bytes.as_str()),
            "{phase}: original JSON bytes must survive"
        );
        let reused = produce();
        assert!(
            reused.status.success(),
            "{phase}: {}",
            String::from_utf8_lossy(&reused.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&reused.stdout).unwrap()["reused"],
            true
        );
        assert_eq!(fs::read(file).unwrap(), current);
        assert_eq!(fs::read(ledger).unwrap(), history);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_replaces_stale_pass_and_preserves_stale_attempt() {
    let root = temp_root("lifecycle-chain-stale-pass");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let requirements_review = real_requirements_review(&root);
    let review = root.join(".appsdk/records/review-record-app-core.json");
    fs::remove_file(&review).unwrap();
    let input = root.join("architecture-input.json");
    let write_input = |identity: &str| {
        fs::write(
            &input,
            serde_json::to_string_pretty(&serde_json::json!({
                "architecture": {
                    "reviewer": {"adapter":"test","identity":identity},
                    "verdict": "pass",
                    "evidence_ids": ["candidate-evidence-1","positive-1","negative-1"],
                    "requirements_review": requirements_review
                }
            }))
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

    write_input("reviewer-a");
    assert!(produce().status.success());
    let first = fs::read(&review).unwrap();
    write_input("reviewer-b");
    let reentered = produce();
    assert!(
        reentered.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&reentered.stdout),
        String::from_utf8_lossy(&reentered.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&reentered.stdout).unwrap()["reused"],
        false
    );
    let second = fs::read(&review).unwrap();
    assert_ne!(first, second);
    let attempts = root.join(".appsdk/records/attempts/app-core/review-record.jsonl");
    let stale: Value = serde_json::from_str(fs::read_to_string(&attempts).unwrap().trim()).unwrap();
    assert_eq!(stale["result"], "stale");
    assert_eq!(
        stale["record"],
        serde_json::from_slice::<Value>(&first).unwrap()
    );

    let reused = produce();
    assert!(reused.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&reused.stdout).unwrap()["reused"],
        true
    );
    assert_eq!(fs::read_to_string(&attempts).unwrap().lines().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_replaces_stale_effectiveness_pass() {
    let root = temp_root("lifecycle-chain-stale-effectiveness");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let input = root.join("effectiveness-input.json");
    let write_input = || {
        fs::write(
            &input,
            serde_json::to_string_pretty(&serde_json::json!({
                "effectiveness": {
                    "fixed_replay_evidence_id": "effective-1",
                    "positive_evidence_ids": ["positive-1"],
                    "negative_evidence_ids": ["post-negative-1"],
                    "blackbox_evidence_ids": ["effective-1"]
                }
            }))
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
            "effectiveness",
            "--input",
            input.to_str().unwrap(),
        ])
    };
    write_input();
    let reentered = produce();
    assert!(
        reentered.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&reentered.stdout),
        String::from_utf8_lossy(&reentered.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&reentered.stdout).unwrap()["reused"],
        false
    );
    let attempts = root.join(".appsdk/records/attempts/app-core/effectiveness-record.jsonl");
    let stale: Value = serde_json::from_str(fs::read_to_string(&attempts).unwrap().trim()).unwrap();
    assert_eq!(stale["result"], "stale");
    let reused = produce();
    assert!(reused.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&reused.stdout).unwrap()["reused"],
        true
    );
    assert_eq!(fs::read_to_string(&attempts).unwrap().lines().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_replaces_stale_merge_pass() {
    let root = temp_root("lifecycle-chain-stale-merge");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let merge_record = root.join(".appsdk/records/merge-record-app-core.json");
    let mut stale: Value =
        serde_json::from_str(&fs::read_to_string(&merge_record).unwrap()).unwrap();
    stale["change_identity"] = Value::String("tested_integration_exact".into());
    fs::write(
        &merge_record,
        serde_json::to_string_pretty(&stale).unwrap() + "\n",
    )
    .unwrap();
    let input = root.join("merge-input.json");
    fs::write(&input, r#"{"merge":{"mainline_ref":"HEAD"}}"#).unwrap();

    let produced = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "merge",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(
        produced.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&produced.stdout),
        String::from_utf8_lossy(&produced.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&produced.stdout).unwrap()["reused"],
        false
    );
    let current: Value = serde_json::from_str(&fs::read_to_string(&merge_record).unwrap()).unwrap();
    assert_eq!(current["change_identity"], "exact");
    let attempts = root.join(".appsdk/records/attempts/app-core/merge-record.jsonl");
    let archived: Value =
        serde_json::from_str(fs::read_to_string(&attempts).unwrap().trim()).unwrap();
    assert_eq!(archived["result"], "stale");
    assert_eq!(
        archived["record"]["change_identity"],
        "tested_integration_exact"
    );

    let reused = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "merge",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(reused.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&reused.stdout).unwrap()["reused"],
        true
    );
    assert_eq!(fs::read_to_string(&attempts).unwrap().lines().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_replaces_stale_promotion_pass() {
    let root = temp_root("lifecycle-chain-stale-promotion");
    let root_text = root.to_str().unwrap();
    let artifact_hash = prepare_lifecycle_chain_fixture(&root);
    let promotion_record = root.join(".appsdk/records/promotion-record-app-core.json");
    let mut stale: Value =
        serde_json::from_str(&fs::read_to_string(&promotion_record).unwrap()).unwrap();
    stale["change_set_id"] = Value::String("change-1".into());
    fs::write(
        &promotion_record,
        serde_json::to_string_pretty(&stale).unwrap() + "\n",
    )
    .unwrap();
    let input = root.join("promotion-input.json");
    fs::write(
        &input,
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
        "stdout={} stderr={}",
        String::from_utf8_lossy(&produced.stdout),
        String::from_utf8_lossy(&produced.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&produced.stdout).unwrap()["reused"],
        false
    );
    let current: Value =
        serde_json::from_str(&fs::read_to_string(&promotion_record).unwrap()).unwrap();
    assert_eq!(current["change_set_id"], "change-2");
    let attempts = root.join(".appsdk/records/attempts/app-core/promotion-record.jsonl");
    let archived: Value =
        serde_json::from_str(fs::read_to_string(&attempts).unwrap().trim()).unwrap();
    assert_eq!(archived["result"], "stale");
    assert_eq!(archived["record"]["change_set_id"], "change-1");

    let reused = run(&[
        "produce-lifecycle-chain",
        root_text,
        "--module",
        "app-core",
        "--phase",
        "promotion",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(reused.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&reused.stdout).unwrap()["reused"],
        true
    );
    assert_eq!(fs::read_to_string(&attempts).unwrap().lines().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_rejects_invalid_attempt_history_before_reusing_canonical_record() {
    let root = temp_root("lifecycle-chain-attempt-integrity");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let requirements_review = real_requirements_review(&root);
    let review = root.join(".appsdk/records/review-record-app-core.json");
    fs::remove_file(&review).unwrap();
    let input = root.join("architecture-input.json");
    let write_input = |verdict: &str| {
        let mut architecture = serde_json::json!({
            "reviewer": {"adapter":"test","identity":"test"},
            "verdict": verdict,
            "evidence_ids": ["candidate-evidence-1","positive-1","negative-1"]
        });
        if verdict == "pass" {
            architecture["requirements_review"] = requirements_review.clone();
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

    write_input("fail");
    assert!(produce().status.success());
    write_input("pass");
    assert!(produce().status.success());

    let attempts = root.join(".appsdk/records/attempts/app-core/review-record.jsonl");
    let canonical_before = fs::read(&review).unwrap();
    let valid_ledger = fs::read_to_string(&attempts).unwrap();
    let valid_attempt: Value = serde_json::from_str(valid_ledger.lines().next().unwrap()).unwrap();
    for (label, invalid_ledger) in [
        ("wrong-result", {
            let mut attempt = valid_attempt.clone();
            attempt["result"] = Value::String("pass".into());
            serde_json::to_string(&attempt).unwrap() + "\n"
        }),
        ("missing-archived-at", {
            let mut attempt = valid_attempt.clone();
            attempt.as_object_mut().unwrap().remove("archived_at");
            serde_json::to_string(&attempt).unwrap() + "\n"
        }),
        ("invalid-archived-at", {
            let mut attempt = valid_attempt.clone();
            attempt["archived_at"] = Value::String("not-a-timestamp".into());
            serde_json::to_string(&attempt).unwrap() + "\n"
        }),
        (
            "duplicate-attempt-id",
            format!("{valid_ledger}{valid_ledger}"),
        ),
    ] {
        fs::write(&attempts, &invalid_ledger).unwrap();
        let rejected = produce();
        assert!(
            !rejected.status.success(),
            "case={label} stdout={} stderr={}",
            String::from_utf8_lossy(&rejected.stdout),
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(
            String::from_utf8_lossy(&rejected.stderr)
                .contains("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID"),
            "case={label} stderr={}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert_eq!(fs::read(&review).unwrap(), canonical_before, "case={label}");
        assert_eq!(
            fs::read_to_string(&attempts).unwrap(),
            invalid_ledger,
            "case={label}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_chain_rejects_invalid_attempt_history_before_first_canonical_write() {
    let root = temp_root("lifecycle-chain-attempt-first-write");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    let review = root.join(".appsdk/records/review-record-app-core.json");
    fs::remove_file(&review).unwrap();
    let attempts = root.join(".appsdk/records/attempts/app-core/review-record.jsonl");
    fs::create_dir_all(attempts.parent().unwrap()).unwrap();
    let invalid_ledger = "{\"schema_version\":1,\"result\":\"non_pass\"}\n";
    fs::write(&attempts, invalid_ledger).unwrap();
    let input = root.join("architecture-input.json");
    let requirements_review = real_requirements_review(&root);
    fs::write(
        &input,
        serde_json::to_string_pretty(&serde_json::json!({
            "architecture": {
                "reviewer": {"adapter":"test","identity":"test"},
                "verdict": "pass",
                "requirements_review": requirements_review,
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
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("LIFECYCLE_CHAIN_ATTEMPT_LEDGER_INVALID"),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&rejected.stderr),
        String::from_utf8_lossy(&rejected.stdout)
    );
    assert!(!review.exists());
    assert_eq!(fs::read_to_string(&attempts).unwrap(), invalid_ledger);
    fs::remove_dir_all(root).unwrap();
}
