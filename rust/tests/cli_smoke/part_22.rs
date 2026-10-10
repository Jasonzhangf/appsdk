fn review_context_output(root: &Path, module_id: &str) -> Value {
    let output = run(&[
        "review-context",
        root.to_str().unwrap(),
        "--module",
        module_id,
    ]);
    assert!(
        output.status.success(),
        "review-context failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "review-context stdout is not exactly one JSON value: {error}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("review-context output is not an object: {value}"));
    assert_eq!(
        object.len(),
        3,
        "review-context must return exactly context_id, context, prompt: {value}"
    );
    assert!(value["context_id"]
        .as_str()
        .is_some_and(|context_id| !context_id.is_empty()));
    assert!(value["context"].is_object());
    assert!(value["prompt"]
        .as_str()
        .is_some_and(|prompt| !prompt.is_empty()));
    value
}

fn real_requirements_review(root: &Path) -> Value {
    let context = review_context_output(root, "app-core");
    serde_json::json!({"context_id": context["context_id"], "checked": true})
}

fn stable_review_id(
    promotion_id: &str,
    fix_candidate_id: &str,
    reviewer: &Value,
    verdict: &str,
    evidence_ids: &[&str],
) -> String {
    stable_review_id_with_bindings(
        promotion_id,
        fix_candidate_id,
        reviewer,
        verdict,
        evidence_ids,
        None,
    )
}

fn stable_review_id_with_bindings(
    promotion_id: &str,
    fix_candidate_id: &str,
    reviewer: &Value,
    verdict: &str,
    evidence_ids: &[&str],
    project_bindings: Option<&Value>,
) -> String {
    let mut identity = serde_json::json!({"promotion_id":promotion_id,"fix_candidate_id":fix_candidate_id,"reviewer":reviewer,"verdict":verdict,"evidence_ids":evidence_ids});
    if let Some(bindings) = project_bindings {
        identity["project_bindings"] = bindings.clone();
    }
    format!(
        "review-{}",
        digest(&canonical(&identity))
            .strip_prefix("sha256:")
            .unwrap()
    )
}

// Synthetic adapter fixtures must bind the inputs they actually supply. This
// helper renews fixture references after a deliberate author-input change;
// production recovery is exercised through public producers in the test below.
fn renew_fixture_requirements_review(root: &Path, module_id: &str) {
    let context = review_context_output(root, module_id);
    let records = root.join(".appsdk/records");
    let review_path = records.join(format!("review-record-{module_id}.json"));
    let mut review: Value = serde_json::from_slice(&fs::read(&review_path).unwrap()).unwrap();
    review["project_bindings"]["requirements_review"] =
        serde_json::json!({"context_id":context["context_id"],"checked":true});
    let ids: Vec<&str> = review["evidence_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    let id = stable_review_id_with_bindings(
        review["promotion_id"].as_str().unwrap(),
        review["fix_candidate_id"].as_str().unwrap(),
        &review["reviewer"],
        review["verdict"].as_str().unwrap(),
        &ids,
        Some(&review["project_bindings"]),
    );
    review["review_id"] = Value::String(id.clone());
    fs::write(&review_path, serde_json::to_vec(&review).unwrap()).unwrap();
    for kind in ["effectiveness-record", "promotion-record", "freeze-record"] {
        let path = records.join(format!("{kind}-{module_id}.json"));
        if !path.exists() {
            continue;
        }
        let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for field in ["architecture_review_id", "review_id"] {
            if record.get(field).is_some() {
                record[field] = Value::String(id.clone());
            }
        }
        if kind == "freeze-record" {
            let promotion: Value = serde_json::from_slice(
                &fs::read(records.join(format!("promotion-record-{module_id}.json"))).unwrap(),
            )
            .unwrap();
            record["promotion_record_hash"] = Value::String(digest(&canonical(&promotion)));
        }
        fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
    }
}

#[test]
fn current_resource_map_owns_the_current_bundle_and_generic_migration_paths() {
    let map: Value =
        serde_json::from_str(include_str!("../../../contracts/maps/resource-map.json")).unwrap();
    let resources = map["resources"].as_array().unwrap();
    let truth_store = |resource_id: &str| {
        resources
            .iter()
            .find(|resource| resource["resource_id"] == resource_id)
            .and_then(|resource| resource["truth_store"].as_str())
            .unwrap()
    };
    assert_eq!(
        truth_store("sdk_bundle"),
        "AppSDK 0.1.0014 embedded Bundle manifest/resources"
    );
    assert_eq!(truth_store("historical_governance_maps"), ".appsdk/migrations/<source>-to-<target>/maps/** when materialized by pin-lock; absent after fresh reset");
    assert_eq!(truth_store("sdk_migration_record"), ".appsdk/migrations/<source>-to-<target>/record.json when materialized by pin-lock; absent after fresh reset");
    for text in [
        include_str!("../../../contracts/migrations/sdk-0.1.5-to-0.1.6.json"),
        include_str!("../../../contracts/migrations/sdk-0.1.6-to-0.1.0007.json"),
    ] {
        let descriptor: Value = serde_json::from_str(text).unwrap();
        assert_eq!(descriptor["materialization"], "pin_lock_when_migrating");
    }
}

fn installed_review_template(root: &Path) -> PathBuf {
    root.join(
        ".appsdk/skills/appsdk-project-governance/references/authoritative-review-template.md",
    )
}

fn architecture_input(
    requirements_review: Option<Value>,
    project_bindings: Option<Value>,
) -> Value {
    let mut architecture = serde_json::json!({
        "reviewer": {"adapter": "test", "identity": "chain-reviewer"},
        "verdict": "pass",
        "evidence_ids": ["candidate-evidence-1", "positive-1", "negative-1"]
    });
    if let Some(requirements_review) = requirements_review {
        architecture["requirements_review"] = requirements_review;
    }
    if let Some(project_bindings) = project_bindings {
        architecture["project_bindings"] = project_bindings;
    }
    serde_json::json!({"architecture": architecture})
}

fn write_architecture_input(
    root: &Path,
    requirements_review: Option<Value>,
    project_bindings: Option<Value>,
) -> PathBuf {
    let path = root.join("architecture-input.json");
    fs::write(
        &path,
        serde_json::to_string_pretty(&architecture_input(requirements_review, project_bindings))
            .unwrap()
            + "\n",
    )
    .unwrap();
    path
}

fn produce_architecture(root: &Path, input: &Path) -> std::process::Output {
    run(&[
        "produce-lifecycle-chain",
        root.to_str().unwrap(),
        "--module",
        "app-core",
        "--phase",
        "architecture",
        "--input",
        input.to_str().unwrap(),
    ])
}

fn produce_architecture_with_real_context(root: &Path) -> std::process::Output {
    let context = review_context_output(root, "app-core");
    let input = write_architecture_input(
        root,
        Some(serde_json::json!({
            "context_id": context["context_id"],
            "checked": true
        })),
        None,
    );
    produce_architecture(root, &input)
}

fn review_record(root: &Path) -> Value {
    serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/review-record-app-core.json")).unwrap(),
    )
    .unwrap()
}

fn set_goal_field(root: &Path, field: &str, value: Value) {
    let path = root.join(".appsdk/goal.json");
    let mut goal: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    goal[field] = value;
    fs::write(&path, serde_json::to_string_pretty(&goal).unwrap() + "\n").unwrap();
}

#[test]
fn review_context_installs_template_and_preserves_goal_bytes() {
    let root = temp_root("review-context-template");
    prepare_lifecycle_chain_fixture(&root);

    let template_path = installed_review_template(&root);
    let template = fs::read_to_string(&template_path).unwrap();
    assert!(template.contains("# Authoritative Requirement Review Template"));
    assert!(template.contains("## Fixed SDK obligations"));
    assert!(template.contains("The reviewer must independently read the declared sources"));

    let skill =
        fs::read_to_string(root.join(".appsdk/skills/appsdk-project-governance/SKILL.md")).unwrap();
    assert!(skill.contains("authoritative-review-template.md"));
    let review_delivery = fs::read_to_string(
        root.join(".appsdk/skills/appsdk-project-governance/references/review-delivery.md"),
    )
    .unwrap();
    assert!(review_delivery.contains("authoritative-review-template.md"));

    let resources: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/sdk-resources.json")).unwrap())
            .unwrap();
    let template_resource = resources["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|resource| {
            resource["source"]
                == "skills/appsdk-project-governance/references/authoritative-review-template.md"
        })
        .expect("installed SDK resources must contain the authoritative review template");
    assert_eq!(
        template_resource["path"],
        ".appsdk/skills/appsdk-project-governance/references/authoritative-review-template.md"
    );
    assert!(template_resource["digest"].as_str().is_some());

    let goal_path = root.join(".appsdk/goal.json");
    let goal_before = fs::read(&goal_path).unwrap();
    let goal: Value = serde_json::from_slice(&goal_before).unwrap();
    let output = review_context_output(&root, "app-core");
    let prompt = output["prompt"].as_str().unwrap();
    assert!(prompt.contains(goal["raw_request"].as_str().unwrap()));
    assert!(prompt.contains(goal["understood_objective"].as_str().unwrap()));
    for acceptance in goal["acceptance_criteria"].as_array().unwrap() {
        assert!(prompt.contains(acceptance.as_str().unwrap()));
    }
    for non_goal in goal["non_goals"].as_array().unwrap() {
        assert!(prompt.contains(non_goal.as_str().unwrap()));
    }
    let candidate: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/fix-candidate-record-app-core.json"))
            .unwrap(),
    )
    .unwrap();
    assert!(prompt.contains(candidate["head_commit"].as_str().unwrap()));
    assert!(prompt.contains("candidate-evidence-1"));
    assert!(prompt.contains("Authoritative Requirement Review Packet"));
    let lower = prompt.to_ascii_lowercase();
    assert!(lower.contains("authentication"));
    assert!(lower.contains("authorization"));
    assert!(
        lower.contains("not available")
            || lower.contains("unverified")
            || lower.contains("missing")
    );
    assert_eq!(fs::read(&goal_path).unwrap(), goal_before);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn review_context_rejects_invalid_authority_and_missing_author_evidence() {
    let missing_goal = temp_root("review-context-missing-goal");
    prepare_lifecycle_chain_fixture(&missing_goal);
    fs::remove_file(missing_goal.join(".appsdk/goal.json")).unwrap();
    let missing_goal_output = run(&[
        "review-context",
        missing_goal.to_str().unwrap(),
        "--module",
        "app-core",
    ]);
    assert!(!missing_goal_output.status.success());
    assert!(String::from_utf8_lossy(&missing_goal_output.stderr)
        .contains("MISSING_GOAL_CLARIFICATION_RECORD"));
    fs::remove_dir_all(missing_goal).unwrap();

    let invalid_goal = temp_root("review-context-invalid-goal");
    prepare_lifecycle_chain_fixture(&invalid_goal);
    fs::write(invalid_goal.join(".appsdk/goal.json"), "{}\n").unwrap();
    let invalid_goal_output = run(&[
        "review-context",
        invalid_goal.to_str().unwrap(),
        "--module",
        "app-core",
    ]);
    assert!(!invalid_goal_output.status.success());
    assert!(String::from_utf8_lossy(&invalid_goal_output.stderr)
        .contains("INVALID_GOAL_CLARIFICATION_RECORD"));
    fs::remove_dir_all(invalid_goal).unwrap();

    let unconfirmed_goal = temp_root("review-context-unconfirmed-goal");
    prepare_lifecycle_chain_fixture(&unconfirmed_goal);
    set_goal_field(
        &unconfirmed_goal,
        "status",
        Value::String("received".into()),
    );
    let unconfirmed_output = run(&[
        "review-context",
        unconfirmed_goal.to_str().unwrap(),
        "--module",
        "app-core",
    ]);
    assert!(!unconfirmed_output.status.success());
    assert!(
        String::from_utf8_lossy(&unconfirmed_output.stderr).contains("GOAL_NOT_CONFIRMED:received")
    );
    fs::remove_dir_all(unconfirmed_goal).unwrap();

    let missing_evidence = temp_root("review-context-missing-author-evidence");
    prepare_lifecycle_chain_fixture(&missing_evidence);
    fs::remove_file(
        missing_evidence.join(".appsdk/records/pre-review-validation-record-app-core.json"),
    )
    .unwrap();
    let missing_evidence_output = run(&[
        "review-context",
        missing_evidence.to_str().unwrap(),
        "--module",
        "app-core",
    ]);
    assert!(!missing_evidence_output.status.success());
    let missing_evidence_stderr = String::from_utf8_lossy(&missing_evidence_output.stderr);
    let blocked: Value = serde_json::from_str(&missing_evidence_stderr).unwrap();
    assert_eq!(blocked["error"], "REVIEW_ADMISSION_BLOCKED");
    assert!(blocked["missing"].as_array().unwrap().iter().any(|item| {
        item["kind"] == "pre_review_validation"
            && item["path"] == "pre-review-validation-record-app-core.json"
    }));
    fs::remove_dir_all(missing_evidence).unwrap();

    let valid = temp_root("review-context-valid-author-evidence");
    prepare_lifecycle_chain_fixture(&valid);
    review_context_output(&valid, "app-core");
    fs::remove_dir_all(valid).unwrap();
}

#[test]
fn architecture_pass_requires_exact_checked_review_context() {
    let root = temp_root("review-context-pass-binding");
    prepare_lifecycle_chain_fixture(&root);
    let review_path = root.join(".appsdk/records/review-record-app-core.json");
    fs::remove_file(&review_path).unwrap();
    let context = review_context_output(&root, "app-core");
    let context_id = context["context_id"].as_str().unwrap().to_string();
    let input = write_architecture_input(
        &root,
        Some(serde_json::json!({"context_id": context_id, "checked": true})),
        None,
    );
    let produced = produce_architecture(&root, &input);
    assert!(
        produced.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&produced.stdout),
        String::from_utf8_lossy(&produced.stderr)
    );
    let review = review_record(&root);
    assert_eq!(
        review["project_bindings"]["requirements_review"],
        serde_json::json!({"context_id": context_id, "checked": true})
    );
    let admission = run(&[
        "verify",
        "--review-admission",
        root.to_str().unwrap(),
        "--module",
        "app-core",
    ]);
    assert!(
        admission.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&admission.stdout),
        String::from_utf8_lossy(&admission.stderr)
    );
    fs::remove_dir_all(root).unwrap();

    for (label, requirements_review) in [
        ("missing", None),
        (
            "fake",
            Some(serde_json::json!({
                "context_id": "review-context-fake",
                "checked": true
            })),
        ),
        (
            "unchecked",
            Some(serde_json::json!({
                "context_id": "review-context-unchecked",
                "checked": false
            })),
        ),
    ] {
        let root = temp_root(&format!("review-context-pass-binding-{label}"));
        prepare_lifecycle_chain_fixture(&root);
        let review_path = root.join(".appsdk/records/review-record-app-core.json");
        fs::remove_file(&review_path).unwrap();
        let input = write_architecture_input(&root, requirements_review, None);
        let rejected = produce_architecture(&root, &input);
        assert!(
            !rejected.status.success(),
            "architecture PASS accepted {label} requirements_review"
        );
        assert!(
            !review_path.exists(),
            "architecture PASS wrote a review record for {label} requirements_review"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn architecture_pass_rejects_stale_context_after_goal_material_change() {
    let root = temp_root("review-context-stale-ack");
    prepare_lifecycle_chain_fixture(&root);
    let review_path = root.join(".appsdk/records/review-record-app-core.json");
    fs::remove_file(&review_path).unwrap();
    let old_context = review_context_output(&root, "app-core");
    let old_context_id = old_context["context_id"].as_str().unwrap().to_string();

    set_goal_field(
        &root,
        "raw_request",
        Value::String("changed requirement after assembly".into()),
    );
    let stale_input = write_architecture_input(
        &root,
        Some(serde_json::json!({"context_id": old_context_id, "checked": true})),
        None,
    );
    let stale = produce_architecture(&root, &stale_input);
    assert!(!stale.status.success());
    assert!(
        !review_path.exists(),
        "stale review-context acknowledgement produced a review record"
    );

    let fresh_context = review_context_output(&root, "app-core");
    let fresh_context_id = fresh_context["context_id"].as_str().unwrap();
    assert_ne!(fresh_context_id, old_context_id);
    let fresh_input = write_architecture_input(
        &root,
        Some(serde_json::json!({
            "context_id": fresh_context_id,
            "checked": true
        })),
        None,
    );
    let fresh = produce_architecture(&root, &fresh_input);
    assert!(
        fresh.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&fresh.stdout),
        String::from_utf8_lossy(&fresh.stderr)
    );
    let review = review_record(&root);
    assert_eq!(
        review["project_bindings"]["requirements_review"]["context_id"],
        fresh_context_id
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn review_gate_rejects_tampered_goal_and_requirements_binding() {
    let goal_tamper = temp_root("review-context-goal-tamper");
    prepare_lifecycle_chain_fixture(&goal_tamper);
    let review_path = goal_tamper.join(".appsdk/records/review-record-app-core.json");
    fs::remove_file(&review_path).unwrap();
    let produced = produce_architecture_with_real_context(&goal_tamper);
    assert!(
        produced.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&produced.stdout),
        String::from_utf8_lossy(&produced.stderr)
    );
    set_goal_field(
        &goal_tamper,
        "acceptance_criteria",
        serde_json::json!(["changed acceptance after review"]),
    );
    let rejected = run(&[
        "verify",
        "--review-admission",
        goal_tamper.to_str().unwrap(),
        "--module",
        "app-core",
    ]);
    assert!(!rejected.status.success());
    fs::remove_dir_all(goal_tamper).unwrap();

    let binding_tamper = temp_root("review-context-binding-tamper");
    prepare_lifecycle_chain_fixture(&binding_tamper);
    let review_path = binding_tamper.join(".appsdk/records/review-record-app-core.json");
    fs::remove_file(&review_path).unwrap();
    assert!(produce_architecture_with_real_context(&binding_tamper)
        .status
        .success());
    let mut review = review_record(&binding_tamper);
    review["project_bindings"]["requirements_review"]["context_id"] =
        Value::String("review-context-forged".into());
    fs::write(
        &review_path,
        serde_json::to_string_pretty(&review).unwrap() + "\n",
    )
    .unwrap();
    let rejected = run(&[
        "promote-module",
        binding_tamper.to_str().unwrap(),
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("ARCHITECTURE_REVIEW_IDENTITY_MISMATCH")
    );
    fs::remove_dir_all(binding_tamper).unwrap();
}

#[test]
fn review_context_recovers_after_stale_architecture_pass() {
    let root = temp_root("review-context-stale-pass-recovery");
    prepare_lifecycle_chain_fixture(&root);
    let review_path = root.join(".appsdk/records/review-record-app-core.json");
    fs::remove_file(&review_path).unwrap();
    let old_context = review_context_output(&root, "app-core");
    let old_input = write_architecture_input(
        &root,
        Some(serde_json::json!({
            "context_id": old_context["context_id"],
            "checked": true
        })),
        None,
    );
    assert!(produce_architecture(&root, &old_input).status.success());
    let promoted = run(&[
        "promote-module",
        root.to_str().unwrap(),
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ]);
    assert!(
        promoted.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&promoted.stdout),
        String::from_utf8_lossy(&promoted.stderr)
    );
    let old_review_id = review_record(&root)["review_id"]
        .as_str()
        .unwrap()
        .to_string();

    set_goal_field(
        &root,
        "raw_request",
        Value::String("requirement changed after architecture PASS".into()),
    );
    let stale_gate = run(&[
        "verify",
        "--review-admission",
        root.to_str().unwrap(),
        "--module",
        "app-core",
    ]);
    assert!(
        !stale_gate.status.success(),
        "stale architecture PASS was accepted after requirement change"
    );

    let fresh_context = review_context_output(&root, "app-core");
    assert_ne!(
        fresh_context["context_id"].as_str().unwrap(),
        old_context["context_id"].as_str().unwrap()
    );
    let fresh_input = write_architecture_input(
        &root,
        Some(serde_json::json!({
            "context_id": fresh_context["context_id"],
            "checked": true
        })),
        None,
    );
    let fresh = produce_architecture(&root, &fresh_input);
    assert!(
        fresh.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&fresh.stdout),
        String::from_utf8_lossy(&fresh.stderr)
    );
    let fresh_review = review_record(&root);
    assert_ne!(fresh_review["review_id"].as_str().unwrap(), old_review_id);
    assert_eq!(
        fresh_review["project_bindings"]["requirements_review"]["context_id"],
        fresh_context["context_id"]
    );
    // Rebind downstream effectiveness through its public producer. The
    // candidate/validation already bind these unchanged fixture observations;
    // the old effectiveness record remains in the attempt history.
    let effectiveness_input = root.join("recovered-effectiveness-input.json");
    let replay_path = root.join(".appsdk/records/evidence/app-core/effective-1.json");
    let mut replay: Value = serde_json::from_slice(&fs::read(&replay_path).unwrap()).unwrap();
    replay["created_at"] = Value::String(chrono::Utc::now().to_rfc3339());
    fs::write(&replay_path, serde_json::to_vec(&replay).unwrap()).unwrap();
    fs::write(
        &effectiveness_input,
        serde_json::to_vec(&serde_json::json!({
            "effectiveness": {
                "fixed_replay_evidence_id": "effective-1",
                "positive_evidence_ids": ["positive-1"],
                "negative_evidence_ids": ["negative-1"],
                "blackbox_evidence_ids": ["effective-1"]
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let effectiveness = run(&[
        "produce-lifecycle-chain",
        root.to_str().unwrap(),
        "--module",
        "app-core",
        "--phase",
        "effectiveness",
        "--input",
        effectiveness_input.to_str().unwrap(),
    ]);
    assert!(
        effectiveness.status.success(),
        "effectiveness recovery: {}",
        String::from_utf8_lossy(&effectiveness.stderr)
    );
    let merge_input = root.join("recovered-merge-input.json");
    fs::write(&merge_input, r#"{"merge":{"mainline_ref":"HEAD"}}"#).unwrap();
    let merge = run(&[
        "produce-lifecycle-chain",
        root.to_str().unwrap(),
        "--module",
        "app-core",
        "--phase",
        "merge",
        "--input",
        merge_input.to_str().unwrap(),
    ]);
    assert!(
        merge.status.success(),
        "merge recovery: {}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let promotion_path = root.join(".appsdk/records/promotion-record-app-core.json");
    let mut promotion: Value = serde_json::from_slice(&fs::read(&promotion_path).unwrap()).unwrap();
    let verification_map: Value = serde_json::from_str(include_str!(
        "../../../contracts/maps/verification-map.json"
    ))
    .unwrap();
    promotion["required_gate_results"] = Value::Array(verification_map["gates"].as_array().unwrap().iter()
        .filter(|gate| gate["required_for"].as_array().unwrap().iter().any(|usage| usage == "promotion"))
        .map(|gate| serde_json::json!({"gate_id":gate["gate_id"], "result":"pass", "producer":"test-fixture"})).collect());
    promotion["evidence_ids"] = serde_json::json!(["candidate-evidence-1", "effective-1"]);
    let promotion_input = root.join("recovered-promotion-input.json");
    fs::write(
        &promotion_input,
        serde_json::to_vec(&serde_json::json!({"promotion":promotion})).unwrap(),
    )
    .unwrap();
    let bug_fixture = install_authoritative_bug_fixture(&root, "issue-1");
    let promoted = run_bug_in(
        &root,
        &[
            "produce-lifecycle-chain",
            root.to_str().unwrap(),
            "--module",
            "app-core",
            "--phase",
            "promotion",
            "--input",
            promotion_input.to_str().unwrap(),
        ],
    );
    assert!(
        promoted.status.success(),
        "promotion recovery: {}",
        String::from_utf8_lossy(&promoted.stderr)
    );
    let accepted = run(&[
        "verify",
        "--review-admission",
        root.to_str().unwrap(),
        "--module",
        "app-core",
    ]);
    assert!(
        accepted.status.success(),
        "fresh PASS rejected: {}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(bug_fixture.parent().unwrap()).unwrap();
}

#[test]
fn review_context_rejects_missing_or_modified_installed_template() {
    let missing = temp_root("review-context-template-missing");
    prepare_lifecycle_chain_fixture(&missing);
    fs::remove_file(installed_review_template(&missing)).unwrap();
    let missing_output = run(&[
        "review-context",
        missing.to_str().unwrap(),
        "--module",
        "app-core",
    ]);
    assert!(!missing_output.status.success());
    fs::remove_dir_all(missing).unwrap();

    let modified = temp_root("review-context-template-modified");
    prepare_lifecycle_chain_fixture(&modified);
    let template_path = installed_review_template(&modified);
    fs::write(
        &template_path,
        "modified SDK review template without the fixed duties\n",
    )
    .unwrap();
    let modified_output = run(&[
        "review-context",
        modified.to_str().unwrap(),
        "--module",
        "app-core",
    ]);
    assert!(!modified_output.status.success());
    fs::remove_dir_all(modified).unwrap();
}
