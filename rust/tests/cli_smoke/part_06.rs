#[test]
fn lifecycle_chain_rejects_tampered_persisted_review_identity() {
    for (label, tamper, command, expected_error) in [
        (
            "project-bindings",
            "project_bindings",
            "effectiveness",
            "ARCHITECTURE_REVIEW_IDENTITY_MISMATCH",
        ),
        (
            "review-id",
            "review_id",
            "architecture_stable",
            "ARCHITECTURE_REVIEW_IDENTITY_MISMATCH",
        ),
    ] {
        let root = temp_root(&format!("lifecycle-chain-review-identity-{label}"));
        let root_text = root.to_str().unwrap();
        prepare_lifecycle_chain_fixture(&root);
        let records = root.join(".appsdk/records");
        let requirements_review = real_requirements_review(&root);
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
                    "requirements_review": requirements_review,
                    "project_bindings": {
                        "v4_product_map_root": "docs/architecture/maps",
                        "v4_product_map_hashes": {
                            "resource_map_hash": "sha256:resource",
                            "function_map_hash": "sha256:function",
                            "mainline_call_map_hash": "sha256:mainline",
                            "verification_map_hash": "sha256:verification"
                        }
                    }
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
            "architecture",
            "--input",
            input.to_str().unwrap(),
        ]);
        assert!(
            produced.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&produced.stdout),
            String::from_utf8_lossy(&produced.stderr)
        );
        let mut review: Value =
            serde_json::from_str(&fs::read_to_string(&review_file).unwrap()).unwrap();
        if tamper == "project_bindings" {
            review["project_bindings"]["v4_product_map_root"] =
                Value::String("docs/architecture/forged".into());
        } else {
            review["review_id"] = Value::String("review-forged".into());
        }
        fs::write(
            &review_file,
            serde_json::to_string_pretty(&review).unwrap() + "\n",
        )
        .unwrap();
        let rejected = if command == "effectiveness" {
            let effectiveness_input = root.join("effectiveness-input.json");
            fs::write(&effectiveness_input, r#"{"effectiveness":{}}"#).unwrap();
            run(&[
                "produce-lifecycle-chain",
                root_text,
                "--module",
                "app-core",
                "--phase",
                "effectiveness",
                "--input",
                effectiveness_input.to_str().unwrap(),
            ])
        } else {
            run(&[
                "promote-module",
                root_text,
                "--module",
                "app-core",
                "--to",
                command,
            ])
        };
        assert!(!rejected.status.success(), "unexpected success for {label}");
        assert!(String::from_utf8_lossy(&rejected.stderr).contains(expected_error));
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn lifecycle_chain_promotion_rejects_tampered_project_promotion_gate_map() {
    let root = temp_root("lifecycle-chain-promotion-map-tamper");
    let root_text = root.to_str().unwrap();
    prepare_lifecycle_chain_fixture(&root);
    fs::remove_file(root.join(".appsdk/records/promotion-record-app-core.json")).unwrap();
    let map_file = root.join(".appsdk/maps/verification-map.json");
    let mut map: Value = serde_json::from_str(&fs::read_to_string(&map_file).unwrap()).unwrap();
    let gate = map["gates"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|gate| gate["gate_id"] == "contract_valid")
        .unwrap();
    gate["required_for"] = serde_json::json!(["compile"]);
    fs::write(
        &map_file,
        serde_json::to_string_pretty(&map).unwrap() + "\n",
    )
    .unwrap();
    let input = root.join("promotion-input.json");
    fs::write(&input, "{}\n").unwrap();
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
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("LIFECYCLE_PRODUCER_MAP_TAMPERED:verification-map.json"));
    assert!(!root
        .join(".appsdk/records/promotion-record-app-core.json")
        .exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_record_producer_rejects_drifted_project_map_before_records() {
    let root = temp_root("lifecycle-record-producer-map-drift");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let map = root.join(".appsdk/maps/function-map.json");
    fs::write(&map, r#"{"schema_version":1,"functions":[]}"#).unwrap();
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
        .contains("INVALID_GOVERNANCE_MAP:function-map.json"));
    assert!(!root
        .join(".appsdk/records/worktree-record-app-core.json")
        .exists());
    assert!(!root
        .join(".appsdk/records/reproduction-record-app-core.json")
        .exists());
    assert!(!root.join(".appsdk/records/evidence/app-core").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_record_producer_rejects_missing_invalid_and_shadowed_canonical_maps() {
    let cases = [
        ("missing", "function-map.json"),
        ("empty", "resource-map.json"),
        ("invalid", "verification-map.json"),
        ("shadowed", "mainline-call-map.json"),
    ];
    for (kind, map_name) in cases {
        let root = temp_root(&format!("lifecycle-record-producer-map-{kind}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        let map_path = root.join(".appsdk/maps").join(map_name);
        let expected_error = match kind {
            "missing" => {
                fs::remove_file(&map_path).unwrap();
                format!("MISSING_GOVERNANCE_MAP:{map_name}")
            }
            "empty" => {
                fs::write(&map_path, r#"{"schema_version":1,"resources":[]}"#).unwrap();
                format!("INVALID_GOVERNANCE_MAP:{map_name}")
            }
            "invalid" => {
                fs::write(&map_path, "{\n").unwrap();
                format!("INVALID_GOVERNANCE_MAP:{map_name}")
            }
            "shadowed" => {
                let mut map: Value =
                    serde_json::from_str(&fs::read_to_string(&map_path).unwrap()).unwrap();
                let entries = map["edges"].as_array_mut().unwrap();
                let canonical = entries
                    .iter()
                    .find(|entry| entry["chain_id"] == "lifecycle-record-production-v1")
                    .cloned()
                    .unwrap();
                let mut shadow = canonical;
                shadow["caller"] = Value::String("shadowed_producer".into());
                entries.push(shadow);
                fs::write(
                    &map_path,
                    serde_json::to_string_pretty(&map).unwrap() + "\n",
                )
                .unwrap();
                format!("LIFECYCLE_PRODUCER_MAP_TAMPERED:{map_name}")
            }
            _ => unreachable!(),
        };
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
        assert!(
            !rejected.status.success(),
            "map case {kind} unexpectedly passed"
        );
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains(&expected_error),
            "map case {kind}: expected {expected_error}, stderr={}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(!root
            .join(".appsdk/records/worktree-record-app-core.json")
            .exists());
        assert!(!root
            .join(".appsdk/records/reproduction-record-app-core.json")
            .exists());
        assert!(!root.join(".appsdk/records/evidence/app-core").exists());
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn lifecycle_record_producer_rejects_tampered_canonical_entry_in_each_map() {
    let cases = [
        (
            "resource-map.json",
            "resources",
            "resource_id",
            "lifecycle_record_producer_input",
            "owner",
        ),
        (
            "function-map.json",
            "functions",
            "function_id",
            "lifecycle_record_producer",
            "owner",
        ),
        (
            "mainline-call-map.json",
            "edges",
            "chain_id",
            "lifecycle-record-production-v1",
            "caller",
        ),
        (
            "verification-map.json",
            "gates",
            "gate_id",
            "worktree_clean",
            "command",
        ),
    ];
    for (map_name, key, id_key, id, field) in cases {
        let root = temp_root(&format!("lifecycle-record-producer-map-tampered-{id_key}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        let map_path = root.join(".appsdk/maps").join(map_name);
        let mut map: Value = serde_json::from_str(&fs::read_to_string(&map_path).unwrap()).unwrap();
        let entry = map[key]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|entry| entry[id_key] == id)
            .unwrap();
        entry[field] = Value::String("tampered_producer_contract".into());
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
        assert!(
            !rejected.status.success(),
            "map {map_name} unexpectedly passed"
        );
        assert!(
            String::from_utf8_lossy(&rejected.stderr)
                .contains(&format!("LIFECYCLE_PRODUCER_MAP_TAMPERED:{map_name}")),
            "map {map_name}: stderr={}",
            String::from_utf8_lossy(&rejected.stderr)
        );
        assert!(!root
            .join(".appsdk/records/worktree-record-app-core.json")
            .exists());
        assert!(!root
            .join(".appsdk/records/reproduction-record-app-core.json")
            .exists());
        assert!(!root.join(".appsdk/records/evidence/app-core").exists());
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn lifecycle_chain_producer_rejects_tampered_chain_contract_entries() {
    let cases = [
        (
            "resource-map.json",
            "resources",
            "resource_id",
            "lifecycle_chain_producer_input",
            "owner",
        ),
        (
            "function-map.json",
            "functions",
            "function_id",
            "lifecycle_chain_record_producer",
            "owner",
        ),
        (
            "mainline-call-map.json",
            "edges",
            "chain_id",
            "lifecycle-record-chain-production-v1",
            "caller",
        ),
        (
            "verification-map.json",
            "gates",
            "gate_id",
            "lifecycle_chain_record_producer",
            "command",
        ),
    ];
    for (map_name, key, id_key, id, field) in cases {
        let root = temp_root(&format!("lifecycle-chain-map-tampered-{id_key}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        let goal_path = root.join(".appsdk/goal.json");
        let mut goal: Value =
            serde_json::from_str(&fs::read_to_string(&goal_path).unwrap()).unwrap();
        goal["status"] = Value::String("confirmed".into());
        goal["confirmed_by"] = Value::String("test".into());
        goal["confirmed_at"] = Value::String("2026-01-01T00:00:00Z".into());
        fs::write(
            &goal_path,
            serde_json::to_string_pretty(&goal).unwrap() + "\n",
        )
        .unwrap();
        let map_path = root.join(".appsdk/maps").join(map_name);
        let mut map: Value = serde_json::from_str(&fs::read_to_string(&map_path).unwrap()).unwrap();
        let entry = map[key]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|entry| entry[id_key] == id)
            .unwrap();
        entry[field] = Value::String("tampered_chain_contract".into());
        fs::write(
            &map_path,
            serde_json::to_string_pretty(&map).unwrap() + "\n",
        )
        .unwrap();
        let input = root.join("chain-input.json");
        fs::write(&input, "{}\n").unwrap();
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
            String::from_utf8_lossy(&rejected.stderr).contains("LIFECYCLE_PRODUCER_MAP_TAMPERED:")
        );
        assert!(!root
            .join(".appsdk/records/review-record-app-core.json")
            .exists());
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn lifecycle_record_producer_recovers_partial_group_commit() {
    let root = temp_root("lifecycle-record-producer-recovery");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/goal.json"),
        r#"{"goal_id":"goal-1","issue_id":"none","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#,
    )
    .unwrap();
    init_git(&root);
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
    let command = serde_json::json!({
        "program":"sh",
        "args":["-c","printf baseline-error-token >&2; exit 1"],
        "working_directory":".",
        "expected_exit_status":1,
        "expected_error_token":"baseline-error-token"
    });
    let input_hashes = vec![digest(&canonical(&command))];
    let current_root = root.canonicalize().unwrap();
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
            "error_token":"baseline-error-token"
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
            "output_hash":digest("stdout=\nstderr=baseline-error-token")
        }
    });
    let input_path = root.with_extension("producer-input.json");
    fs::write(
        &input_path,
        serde_json::to_string_pretty(&input).unwrap() + "\n",
    )
    .unwrap();
    let input_hash = digest(&canonical(&input));
    let records = [
        (
            ".appsdk/records/worktree-record-app-core.json".to_string(),
            input["worktree"].clone(),
        ),
        (
            ".appsdk/records/reproduction-record-app-core.json".to_string(),
            input["reproduction"].clone(),
        ),
        (
            format!(".appsdk/records/evidence/app-core/{}.json", baseline_id),
            input["baseline_evidence"].clone(),
        ),
    ];
    let transaction = root.join(".appsdk/transactions/producer-app-core");
    fs::create_dir_all(&transaction).unwrap();
    let mut marker_records = Vec::new();
    for (index, (target, record)) in records.iter().enumerate() {
        let staged = transaction.join(format!("record-{}.json", index));
        let bytes = serde_json::to_string_pretty(record).unwrap() + "\n";
        fs::write(&staged, &bytes).unwrap();
        marker_records.push(serde_json::json!({
            "target": target,
            "staging": format!("record-{}.json", index),
            "digest": digest(&bytes)
        }));
        if index == 0 {
            fs::create_dir_all(root.join(".appsdk/records")).unwrap();
            fs::hard_link(&staged, root.join(target)).unwrap();
        }
    }
    let marker = serde_json::json!({
        "schema_version":1,"module_id":"app-core","input_hash":input_hash,"phase":"commit","records":marker_records
    });
    let marker_path = transaction.join("marker.json");
    fs::write(
        &marker_path,
        serde_json::to_string_pretty(&marker).unwrap() + "\n",
    )
    .unwrap();
    let mut tampered_marker = marker.clone();
    tampered_marker["records"][0]["target"] =
        Value::String(".appsdk/records/reproduction-record-app-core.json".into());
    fs::write(
        &marker_path,
        serde_json::to_string_pretty(&tampered_marker).unwrap() + "\n",
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
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("PRODUCER_TRANSACTION_TARGET_INVALID")
    );
    fs::write(
        &marker_path,
        serde_json::to_string_pretty(&marker).unwrap() + "\n",
    )
    .unwrap();
    let rewrite_staged = |index: usize, record: &Value| {
        let bytes = serde_json::to_string_pretty(record).unwrap() + "\n";
        fs::write(transaction.join(format!("record-{}.json", index)), &bytes).unwrap();
        let mut current_marker: Value =
            serde_json::from_str(&fs::read_to_string(&marker_path).unwrap()).unwrap();
        current_marker["records"][index]["digest"] = Value::String(digest(&bytes));
        fs::write(
            &marker_path,
            serde_json::to_string_pretty(&current_marker).unwrap() + "\n",
        )
        .unwrap();
    };
    let mut wrong_triage = records[0].1.clone();
    wrong_triage["bug_triage"] = serde_json::json!({
        "query_executed":true,
        "query":"forged",
        "mode":"new_confirmed",
        "reopened_from_issue_id":null
    });
    rewrite_staged(0, &wrong_triage);
    let triage_rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(!triage_rejected.status.success());
    assert!(String::from_utf8_lossy(&triage_rejected.stderr)
        .contains("PRODUCER_RECOVERY_WORKTREE_BINDING_MISMATCH"));
    assert!(transaction.is_dir());
    assert!(!root
        .join(".appsdk/records/reproduction-record-app-core.json")
        .exists());
    rewrite_staged(0, &records[0].1);

    let mut wrong_reproduction = records[1].1.clone();
    wrong_reproduction["issue_id"] = Value::String("forged-issue".into());
    rewrite_staged(1, &wrong_reproduction);
    let record_rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(!record_rejected.status.success());
    assert!(
        String::from_utf8_lossy(&record_rejected.stderr).contains("PRODUCER_RECORD_SCHEMA_INVALID")
    );
    assert!(transaction.is_dir());
    rewrite_staged(1, &records[1].1);

    let mut wrong_baseline = records[2].1.clone();
    wrong_baseline["scope_hash"] = Value::String("forged-scope".into());
    rewrite_staged(2, &wrong_baseline);
    let baseline_rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(!baseline_rejected.status.success());
    assert!(String::from_utf8_lossy(&baseline_rejected.stderr)
        .contains("PRODUCER_RECOVERY_BASELINE_BINDING_MISMATCH"));
    assert!(transaction.is_dir());
    rewrite_staged(2, &records[2].1);

    assert!(Command::new("git")
        .args(["-C", root_text, "branch", "-m", "codex/drifted"])
        .status()
        .unwrap()
        .success());
    let identity_rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(!identity_rejected.status.success());
    assert!(String::from_utf8_lossy(&identity_rejected.stderr).contains("PRODUCER_BRANCH_MISMATCH"));
    assert!(transaction.is_dir());
    assert!(!root
        .join(".appsdk/records/reproduction-record-app-core.json")
        .exists());
    assert!(!root.join(".appsdk/records/evidence/app-core").exists());
    assert!(Command::new("git")
        .args(["-C", root_text, "branch", "-m", "codex/test"])
        .status()
        .unwrap()
        .success());
    let recovered = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(
        recovered.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&recovered.stdout),
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(!transaction.exists());
    // A replacement transaction can be interrupted after the fixed
    // worktree/reproduction projections are visible but before the new
    // baseline is linked. Recovery must use the durable marker first; trying
    // to archive the mixed current projection would report a false partial
    // set and leave the transaction stuck.
    let replacement_records = records
        .iter()
        .map(|(target, _)| {
            (
                target.clone(),
                serde_json::from_str::<Value>(&fs::read_to_string(root.join(target)).unwrap())
                    .unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let replacement_baseline = root.join(&replacement_records[2].0);
    fs::remove_file(&replacement_baseline).unwrap();
    let replacement_transaction = root.join(".appsdk/transactions/producer-app-core");
    fs::create_dir_all(&replacement_transaction).unwrap();
    let mut staged_replacement_records = replacement_records.clone();
    staged_replacement_records[2].1["exit_status"] = Value::Number(2.into());
    let mut replacement_entries = Vec::new();
    for (index, (target, record)) in staged_replacement_records.iter().enumerate() {
        let staging = replacement_transaction.join(format!("record-{}.json", index));
        let bytes = serde_json::to_string_pretty(record).unwrap() + "\n";
        fs::write(&staging, &bytes).unwrap();
        replacement_entries.push(serde_json::json!({
            "target": target,
            "staging": format!("record-{}.json", index),
            "digest": digest(&bytes)
        }));
    }
    let mut replacement_marker = serde_json::json!({
        "schema_version":1,
        "module_id":"app-core",
        "input_hash":input_hash,
        "phase":"commit",
        "replace_current":true,
        "records":replacement_entries
    });
    fs::write(
        replacement_transaction.join("marker.json"),
        serde_json::to_string_pretty(&replacement_marker).unwrap() + "\n",
    )
    .unwrap();
    let status_rejected = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(!status_rejected.status.success());
    assert!(String::from_utf8_lossy(&status_rejected.stderr)
        .contains("PRODUCER_RECOVERY_BASELINE_STATUS_MISMATCH:expected=1:actual=2"));
    assert!(replacement_transaction.is_dir());
    assert!(!replacement_baseline.exists());
    let correct_baseline_bytes =
        serde_json::to_string_pretty(&replacement_records[2].1).unwrap() + "\n";
    fs::write(
        replacement_transaction.join("record-2.json"),
        &correct_baseline_bytes,
    )
    .unwrap();
    replacement_marker["records"][2]["digest"] = Value::String(digest(&correct_baseline_bytes));
    fs::write(
        replacement_transaction.join("marker.json"),
        serde_json::to_string_pretty(&replacement_marker).unwrap() + "\n",
    )
    .unwrap();
    let replacement_recovered = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(
        replacement_recovered.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&replacement_recovered.stdout),
        String::from_utf8_lossy(&replacement_recovered.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&replacement_recovered.stdout).unwrap()["recovered"],
        true
    );
    assert!(!replacement_transaction.exists());
    assert!(replacement_baseline.is_file());
    for (target, _) in &records {
        assert!(root.join(target).is_file(), "missing {target}");
    }

    let worktree_record = root.join(".appsdk/records/worktree-record-app-core.json");
    let original_worktree_record = fs::read_to_string(&worktree_record).unwrap();
    let project_file = root.join(".appsdk/project.json");
    let mut drifted_project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    drifted_project["modules"][0]["registry_binding"] = serde_json::json!({
        "mode": "aggregate",
        "modules": ["app-core"]
    });
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&drifted_project).unwrap() + "\n",
    )
    .unwrap();
    let binding_drift = run(&[
        "produce-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--input",
        input_path.to_str().unwrap(),
    ]);
    assert!(!binding_drift.status.success());
    assert!(
        String::from_utf8_lossy(&binding_drift.stderr).contains("PRODUCER_SCOPE_MISMATCH"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&binding_drift.stdout),
        String::from_utf8_lossy(&binding_drift.stderr)
    );
    assert_eq!(
        fs::read_to_string(&worktree_record).unwrap(),
        original_worktree_record
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bundled_goal_clarification_contract_supports_verify_and_admission() {
    let root = temp_root("bundled-goal-clarification-contract");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let contract = "contracts/records/goal-clarification-record.schema.json";
    let project: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/project.json")).unwrap())
            .unwrap();
    assert!(project["governance"]["record_contracts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value.as_str() == Some(contract)));
    assert!(root.join(contract).is_file());
    assert!(root.join(".appsdk").join(contract).is_file());

    for args in [
        &["verify", root_text][..],
        &["verify", "--admission", root_text][..],
    ] {
        let verified = run(args);
        assert!(
            verified.status.success(),
            "args={args:?} stderr={}",
            String::from_utf8_lossy(&verified.stderr)
        );
    }

    fs::remove_file(root.join(contract)).unwrap();
    for args in [
        &["verify", root_text][..],
        &["verify", "--admission", root_text][..],
    ] {
        let rejected = run(args);
        assert!(!rejected.status.success(), "args={args:?}");
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains("DECLARED_RECORD_CONTRACT_MISSING"),
            "args={args:?} stderr={}",
            String::from_utf8_lossy(&rejected.stderr)
        );
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_automatically_attempts_collab_without_blocking_independent_work() {
    let root = temp_root("init-collab-peer");
    fs::create_dir_all(&root).unwrap();
    confirm_preparation(&root, ".", "project_refactor");

    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        r#"#!/bin/sh
if [ "$1" != "init" ]; then
  exit 64
fi
{
  printf 'cwd=%s\n' "$PWD"
  printf 'probe=%s\n' "$APPSDK_COLLAB_ENV_PROBE"
  printf 'args=%s\n' "$*"
} >> "$APPSDK_COLLAB_PROBE"
exit 73
"#,
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let search_path = std::env::join_paths(
        std::iter::once(fake_bin.clone()).chain(std::env::split_paths(&inherited_path)),
    )
    .unwrap();
    let probe = root.join("collab-init-probe.txt");
    let output = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
        .env("PATH", search_path)
        .env("APPSDK_COLLAB_ENV_PROBE", "same-environment")
        .env("APPSDK_COLLAB_PROBE", &probe)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        probe.exists(),
        "AppSDK should automatically initialize Collab"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("COLLAB_INIT_FAILED"));

    let repeated = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
        .env(
            "PATH",
            std::env::join_paths(
                std::iter::once(fake_bin.clone()).chain(std::env::split_paths(&inherited_path)),
            )
            .unwrap(),
        )
        .env("APPSDK_COLLAB_ENV_PROBE", "same-environment")
        .env("APPSDK_COLLAB_PROBE", &probe)
        .output()
        .unwrap();
    assert!(
        repeated.status.success(),
        "{}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    let invocation = fs::read_to_string(&probe).unwrap();
    assert_eq!(invocation.matches("args=init\n").count(), 2);
    assert!(invocation.contains("probe=same-environment\n"));
    let child_cwd = invocation
        .lines()
        .find_map(|line| line.strip_prefix("cwd="))
        .unwrap();
    assert_eq!(
        fs::canonicalize(child_cwd).unwrap(),
        fs::canonicalize(&root).unwrap()
    );
    fs::write(
        &fake_collab,
        "#!/bin/sh\nprintf '%s\\n' '{\"ok\":true,\"runtime\":{\"runtimeId\":\"runtime-appserver-thread-1\",\"appserverId\":\"appserver-cli\",\"namespace\":\"codex_tui\",\"endpoint\":\"unix:///tmp/codex.sock\",\"projectRoot\":\"PLACEHOLDER_ROOT\",\"capabilities\":[\"session_status\",\"read_thread\",\"send_message_to_thread\",\"wait_reply\"],\"processId\":4242},\"transport_selected\":{\"kind\":\"appserver\",\"endpoint\":\"unix:///tmp/codex.sock\",\"namespace\":\"codex_tui\",\"thread_id\":\"thread-1\",\"capabilities\":[\"session_status\",\"read_thread\",\"send_message_to_thread\",\"wait_reply\"],\"self_check\":\"test\"}}'\n",
    )
    .unwrap();
    let fake_collab_text = fs::read_to_string(&fake_collab).unwrap().replace(
        "PLACEHOLDER_ROOT",
        root.canonicalize().unwrap().to_str().unwrap(),
    );
    fs::write(&fake_collab, fake_collab_text).unwrap();
    let ready = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
        .env("PATH", &fake_bin)
        .output()
        .unwrap();
    assert!(ready.status.success());
    assert!(
        String::from_utf8_lossy(&ready.stdout).contains("\"transport_selected\""),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&ready.stdout),
        String::from_utf8_lossy(&ready.stderr)
    );
    assert!(String::from_utf8_lossy(&ready.stdout).contains("\"kind\":\"appserver\""));
    let registry = test_global_registry_root_for_project(&root).join("runtimes.jsonl");
    let runtime = fs::read_to_string(registry).unwrap();
    assert!(runtime.contains("runtime.registered"), "{runtime}");
    assert!(runtime.contains("runtime-appserver-thread-1"), "{runtime}");
    assert!(String::from_utf8_lossy(&ready.stdout).contains("\"thread_id\":\"thread-1\""));
    assert!(!runtime.contains("tmuxSession"), "{runtime}");
    assert!(!runtime.contains("pane"), "{runtime}");
    assert!(!runtime.contains("TMax"), "{runtime}");

    let mismatched_root = temp_root("init-collab-root-mismatch");
    fs::create_dir_all(&mismatched_root).unwrap();
    confirm_preparation(&mismatched_root, ".", "project_refactor");
    let mismatched_collab = mismatched_root.join("fake-bin");
    fs::create_dir_all(&mismatched_collab).unwrap();
    let mismatched_binary = mismatched_collab.join("collab");
    fs::write(
        &mismatched_binary,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{{\"ok\":true,\"runtime\":{{\"runtimeId\":\"runtime-wrong-root\",\"appserverId\":\"appserver-cli\",\"namespace\":\"codex_tui\",\"endpoint\":\"unix:///tmp/codex.sock\",\"projectRoot\":\"{}\",\"capabilities\":[\"send_message_to_thread\"],\"processId\":4242}},\"transport_selected\":{{\"kind\":\"appserver\",\"endpoint\":\"unix:///tmp/codex.sock\",\"namespace\":\"codex_tui\",\"thread_id\":\"thread-wrong-root\",\"capabilities\":[\"send_message_to_thread\"],\"self_check\":\"test\"}}}}'\n",
            root.canonicalize().unwrap().display()
        ),
    )
    .unwrap();
    fs::set_permissions(&mismatched_binary, fs::Permissions::from_mode(0o755)).unwrap();
    let mismatched = Command::new(binary())
        .args(["init", mismatched_root.to_str().unwrap()])
        .current_dir(&mismatched_root)
        .env(
            "APPSDK_HOME",
            test_global_registry_root_for_project(&mismatched_root),
        )
        .env("PATH", &mismatched_collab)
        .output()
        .unwrap();
    assert!(mismatched.status.success());
    assert!(
        String::from_utf8_lossy(&mismatched.stderr).contains("COLLAB_INIT_RUNTIME_ROOT_MISMATCH"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&mismatched.stdout),
        String::from_utf8_lossy(&mismatched.stderr)
    );
    assert!(!test_global_registry_root_for_project(&mismatched_root)
        .join("runtimes.jsonl")
        .exists());

    fs::write(
        &mismatched_binary,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{{\"ok\":true,\"runtime\":{{\"runtimeId\":\"runtime-transport-mismatch\",\"appserverId\":\"appserver-cli\",\"namespace\":\"codex_tui\",\"endpoint\":\"unix:///tmp/runtime.sock\",\"projectRoot\":\"{}\",\"capabilities\":[\"send_message_to_thread\"],\"processId\":4242}},\"transport_selected\":{{\"kind\":\"appserver\",\"endpoint\":\"unix:///tmp/transport.sock\",\"namespace\":\"codex_tui\",\"thread_id\":\"thread-transport-mismatch\",\"capabilities\":[\"send_message_to_thread\"],\"self_check\":\"test\"}}}}'\n",
            mismatched_root.canonicalize().unwrap().display()
        ),
    )
    .unwrap();
    let mismatched_transport = Command::new(binary())
        .args(["init", mismatched_root.to_str().unwrap()])
        .current_dir(&mismatched_root)
        .env(
            "APPSDK_HOME",
            test_global_registry_root_for_project(&mismatched_root),
        )
        .env("PATH", &mismatched_collab)
        .output()
        .unwrap();
    assert!(mismatched_transport.status.success());
    assert!(
        String::from_utf8_lossy(&mismatched_transport.stderr)
            .contains("COLLAB_INIT_TRANSPORT_RUNTIME_MISMATCH"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&mismatched_transport.stdout),
        String::from_utf8_lossy(&mismatched_transport.stderr)
    );
    assert!(!test_global_registry_root_for_project(&mismatched_root)
        .join("runtimes.jsonl")
        .exists());

    fs::write(
        &mismatched_binary,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{{\"ok\":true,\"runtime\":{{\"runtimeId\":\"runtime-capability-missing\",\"appserverId\":\"appserver-cli\",\"namespace\":\"codex_tui\",\"endpoint\":\"unix:///tmp/codex.sock\",\"projectRoot\":\"{}\",\"capabilities\":[\"read_thread\"],\"processId\":4242}},\"transport_selected\":{{\"kind\":\"appserver\",\"endpoint\":\"unix:///tmp/codex.sock\",\"namespace\":\"codex_tui\",\"thread_id\":\"thread-capability-missing\",\"capabilities\":[\"read_thread\"],\"self_check\":\"test\"}}}}'\n",
            mismatched_root.canonicalize().unwrap().display()
        ),
    )
    .unwrap();
    let missing_capability = Command::new(binary())
        .args(["init", mismatched_root.to_str().unwrap()])
        .current_dir(&mismatched_root)
        .env(
            "APPSDK_HOME",
            test_global_registry_root_for_project(&mismatched_root),
        )
        .env("PATH", &mismatched_collab)
        .output()
        .unwrap();
    assert!(missing_capability.status.success());
    assert!(
        String::from_utf8_lossy(&missing_capability.stderr)
            .contains("COLLAB_INIT_APPSERVER_CAPABILITY_MISSING"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&missing_capability.stdout),
        String::from_utf8_lossy(&missing_capability.stderr)
    );
    assert!(!test_global_registry_root_for_project(&mismatched_root)
        .join("runtimes.jsonl")
        .exists());

    fs::write(
        &mismatched_binary,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{{\"ok\":true,\"runtime\":{{\"runtimeId\":\"runtime-thread-missing\",\"appserverId\":\"appserver-cli\",\"namespace\":\"codex_tui\",\"endpoint\":\"unix:///tmp/codex.sock\",\"projectRoot\":\"{}\",\"capabilities\":[\"send_message_to_thread\"],\"processId\":4242}},\"transport_selected\":{{\"kind\":\"appserver\",\"endpoint\":\"unix:///tmp/codex.sock\",\"namespace\":\"codex_tui\",\"capabilities\":[\"send_message_to_thread\"],\"self_check\":\"test\"}}}}'\n",
            mismatched_root.canonicalize().unwrap().display()
        ),
    )
    .unwrap();
    let missing_thread = Command::new(binary())
        .args(["init", mismatched_root.to_str().unwrap()])
        .current_dir(&mismatched_root)
        .env(
            "APPSDK_HOME",
            test_global_registry_root_for_project(&mismatched_root),
        )
        .env("PATH", &mismatched_collab)
        .output()
        .unwrap();
    assert!(missing_thread.status.success());
    assert!(
        String::from_utf8_lossy(&missing_thread.stderr)
            .contains("COLLAB_INIT_APPSERVER_THREAD_MISSING"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&missing_thread.stdout),
        String::from_utf8_lossy(&missing_thread.stderr)
    );
    assert!(!test_global_registry_root_for_project(&mismatched_root)
        .join("runtimes.jsonl")
        .exists());

    fs::write(
        &mismatched_binary,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{{\"ok\":true,\"runtime\":{{\"runtimeId\":\"runtime-thread-empty\",\"appserverId\":\"appserver-cli\",\"namespace\":\"codex_tui\",\"endpoint\":\"unix:///tmp/codex.sock\",\"projectRoot\":\"{}\",\"capabilities\":[\"send_message_to_thread\"],\"processId\":4242}},\"transport_selected\":{{\"kind\":\"appserver\",\"endpoint\":\"unix:///tmp/codex.sock\",\"namespace\":\"codex_tui\",\"thread_id\":\"   \",\"capabilities\":[\"send_message_to_thread\"],\"self_check\":\"test\"}}}}'\n",
            mismatched_root.canonicalize().unwrap().display()
        ),
    )
    .unwrap();
    let empty_thread = Command::new(binary())
        .args(["init", mismatched_root.to_str().unwrap()])
        .current_dir(&mismatched_root)
        .env(
            "APPSDK_HOME",
            test_global_registry_root_for_project(&mismatched_root),
        )
        .env("PATH", &mismatched_collab)
        .output()
        .unwrap();
    assert!(empty_thread.status.success());
    assert!(
        String::from_utf8_lossy(&empty_thread.stderr)
            .contains("COLLAB_INIT_APPSERVER_THREAD_MISSING"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&empty_thread.stdout),
        String::from_utf8_lossy(&empty_thread.stderr)
    );
    assert!(!test_global_registry_root_for_project(&mismatched_root)
        .join("runtimes.jsonl")
        .exists());
    fs::remove_dir_all(mismatched_root).unwrap();

    fs::write(
        &fake_collab,
        "#!/bin/sh\nprintf '%s\\n' '{\"ok\":true,\"worker_id\":\"test-peer\"}'\n",
    )
    .unwrap();
    let invalid = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
        .env("PATH", &fake_bin)
        .output()
        .unwrap();
    assert!(invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("COLLAB_INIT_INVALID_RESPONSE"));

    fs::write(&fake_collab, "#!/bin/sh\nexit 0\n").unwrap();
    let empty = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
        .env("PATH", &fake_bin)
        .output()
        .unwrap();
    assert!(empty.status.success());
    assert!(String::from_utf8_lossy(&empty.stderr)
        .contains("COLLAB_INIT_INVALID_RESPONSE:empty stdout"));

    fs::remove_file(&fake_collab).unwrap();
    let unavailable = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
        .env("PATH", &fake_bin)
        .output()
        .unwrap();
    assert!(unavailable.status.success());
    assert!(String::from_utf8_lossy(&unavailable.stderr).contains("COLLAB_INIT_UNAVAILABLE"));
    assert!(run(&["verify", root.to_str().unwrap()]).status.success());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_waits_for_slow_collab_route_recovery_before_timeout() {
    let root = temp_root("init-collab-slow-route-recovery");
    fs::create_dir_all(&root).unwrap();
    confirm_preparation(&root, ".", "project_refactor");

    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        format!(
            "#!/bin/sh\n/bin/sleep 6\nprintf '%s\\n' '{{\"ok\":true,\"runtime\":{{\"runtimeId\":\"runtime-slow-route-recovery\",\"appserverId\":\"appserver-cli\",\"namespace\":\"codex_tui\",\"endpoint\":\"unix:///tmp/codex.sock\",\"projectRoot\":\"{}\",\"capabilities\":[\"session_status\",\"read_thread\",\"send_message_to_thread\",\"wait_reply\"],\"processId\":4242}},\"transport_selected\":{{\"kind\":\"appserver\",\"endpoint\":\"unix:///tmp/codex.sock\",\"namespace\":\"codex_tui\",\"thread_id\":\"thread-slow-route-recovery\",\"capabilities\":[\"session_status\",\"read_thread\",\"send_message_to_thread\",\"wait_reply\"],\"self_check\":\"test\"}}}}'\n",
            root.canonicalize().unwrap().display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
        .env("PATH", &fake_bin)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("COLLAB_INIT_TIMEOUT"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("collab-channel"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("thread-slow-route-recovery"));
    let registry = test_global_registry_root_for_project(&root).join("runtimes.jsonl");
    let runtime = fs::read_to_string(registry).unwrap();
    assert!(runtime.contains("runtime-slow-route-recovery"), "{runtime}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_reports_token_mismatch_once_without_repair() {
    assert_init_reports_identity_failure_once(
        "init-collab-token-mismatch",
        "collab: token mismatch: identity does not own this worker_id",
    );
}

fn assert_init_reports_identity_failure_once(fixture_name: &str, init_error: &str) {
    let root = temp_root(fixture_name);
    fs::create_dir_all(&root).unwrap();
    confirm_preparation(&root, ".", "project_refactor");

    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    let probe = root.join("collab-probe.txt");
    fs::write(
        &fake_collab,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> "{}"
case "$*" in
  "init")
    printf '%s\n' '{}' >&2
    exit 1
    ;;
  *)
    printf '%s\n' "unexpected collab command: $*" >&2
    exit 64
    ;;
esac
"#,
            probe.display(),
            init_error
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();
    init_git(&root);
    let path = format!("{}:{}", fake_bin.display(), env::var("PATH").unwrap());

    let output = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
        .env("PATH", &path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("COLLAB_INIT_FAILED"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(init_error),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("collab-channel"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let invocations = fs::read_to_string(&probe).unwrap();
    assert_eq!(invocations, "init\n");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_reports_pending_identity_facts_without_repair() {
    let root = temp_root("init-collab-pending-identity-facts");
    fs::create_dir_all(&root).unwrap();
    confirm_preparation(&root, ".", "project_refactor");

    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_collab = fake_bin.join("collab");
    let probe = root.join("collab-probe.txt");
    fs::write(
        &fake_collab,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> "{}"
case "$*" in
  "init")
    printf '%s\n' '{{"ok":true,"snapshot":{{"registered":false,"requires_identity_update":{{"required":true,"reason":"IDENTITY_INFORMATION_REQUIRED","required_fields":["session_id","thread_id"]}}}}}}'
    ;;
  *)
    printf '%s\n' "unexpected collab command: $*" >&2
    exit 64
    ;;
esac
"#,
            probe.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();
    init_git(&root);
    let path = format!("{}:{}", fake_bin.display(), env::var("PATH").unwrap());

    let output = Command::new(binary())
        .args(["init", root.to_str().unwrap()])
        .current_dir(&root)
        .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
        .env("PATH", &path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("COLLAB_INIT_PENDING"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("\"required_fields\":[\"session_id\",\"thread_id\"]"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("collab-channel"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let invocations = fs::read_to_string(&probe).unwrap();
    assert_eq!(invocations, "init\n");
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("worker recover"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(root).unwrap();
}
