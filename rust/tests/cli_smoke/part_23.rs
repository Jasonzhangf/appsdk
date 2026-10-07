#[test]
fn pin_lock_preserves_review_history_and_requires_current_requirements_ack() {
    let root = temp_root("review-context-upgrade-history");
    prepare_lifecycle_chain_fixture(&root);
    let review_path = root.join(".appsdk/records/review-record-app-core.json");
    let mut legacy = review_record(&root);
    legacy.as_object_mut().unwrap().remove("project_bindings");
    let evidence_ids: Vec<&str> = legacy["evidence_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    let legacy_id = stable_review_id(
        legacy["promotion_id"].as_str().unwrap(),
        legacy["fix_candidate_id"].as_str().unwrap(),
        &legacy["reviewer"],
        "pass",
        &evidence_ids,
    );
    legacy["review_id"] = Value::String(legacy_id.clone());
    fs::write(&review_path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
    let legacy_bytes = fs::read(&review_path).unwrap();

    // Simulate an older consumer with project-owned maps. Pin-lock must keep
    // its review history; upgrading the SDK cannot create a reviewer check.
    let project_path = root.join(".appsdk/project.json");
    let mut project: Value = serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();
    project["sdk"]["version"] = Value::String("0.1.0010".into());
    fs::write(&project_path, serde_json::to_vec_pretty(&project).unwrap()).unwrap();
    let pinned = run(&[
        "pin-lock",
        root.to_str().unwrap(),
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        pinned.status.success(),
        "pin-lock failed: {}",
        String::from_utf8_lossy(&pinned.stderr)
    );
    let lock: Value =
        serde_json::from_slice(&fs::read(root.join(".appsdk/sdk.lock")).unwrap()).unwrap();
    assert_eq!(lock["version"], "0.1.0011");
    assert_eq!(fs::read(&review_path).unwrap(), legacy_bytes);
    let migration_path = root.join(".appsdk/migrations/0.1.0010-to-0.1.0011/record.json");
    let migration_bytes = fs::read(&migration_path).unwrap();
    let migration: Value = serde_json::from_slice(&migration_bytes).unwrap();
    assert!(migration["legacy_reconciled_reviews"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["module_id"] == "app-core" && entry["review_id"] == legacy_id));

    let rejected = run(&[
        "verify",
        "--review-admission",
        root.to_str().unwrap(),
        "--module",
        "app-core",
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("ARCHITECTURE_REQUIREMENTS_REVIEW_MISSING"));
    let stale_context = review_context_output(&root, "app-core");
    set_goal_field(
        &root,
        "user_original_input",
        Value::String("post-upgrade changed requirement material".into()),
    );
    let stale_input = write_architecture_input(
        &root,
        Some(serde_json::json!({
            "context_id": stale_context["context_id"], "checked": true
        })),
        None,
    );
    let rejected = produce_architecture(&root, &stale_input);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("ARCHITECTURE_REQUIREMENTS_REVIEW_CONTEXT_MISMATCH"));
    assert_eq!(fs::read(&review_path).unwrap(), legacy_bytes);

    let fresh_context = review_context_output(&root, "app-core");
    let fresh_input = write_architecture_input(
        &root,
        Some(serde_json::json!({
            "context_id": fresh_context["context_id"], "checked": true
        })),
        None,
    );
    let produced = produce_architecture(&root, &fresh_input);
    assert!(
        produced.status.success(),
        "new review failed: {}",
        String::from_utf8_lossy(&produced.stderr)
    );
    let current = review_record(&root);
    assert_eq!(
        current["project_bindings"]["requirements_review"]["context_id"],
        fresh_context["context_id"]
    );
    assert_ne!(current["review_id"], legacy_id);
    let admitted = run(&[
        "verify",
        "--review-admission",
        root.to_str().unwrap(),
        "--module",
        "app-core",
    ]);
    assert!(
        admitted.status.success(),
        "new review admission failed: {}",
        String::from_utf8_lossy(&admitted.stderr)
    );
    assert_eq!(
        fs::read(&migration_path).unwrap(),
        migration_bytes,
        "migration history must not be rewritten by review"
    );
    fs::remove_dir_all(root).unwrap();
}
