fn project_goal_request(
    root: &Path,
    operation: &str,
    base_goal_revision: Option<u64>,
    requirements_version: u64,
    goal_id: &str,
    status: &str,
) -> PathBuf {
    let request = serde_json::json!({
        "operation": operation,
        "project_id": serde_json::from_slice::<Value>(
            &fs::read(root.join(".appsdk/project.json")).unwrap()
        ).unwrap()["project_id"],
        "base_goal_revision": base_goal_revision,
        "requirements_version": requirements_version,
        "authorization": {
            "scope": {
                "allowed_paths": ["rust/src/main/**"],
                "forbidden_paths": [".appsdk/requirements.json"]
            },
            "role": "user",
            "source": "test-conversation/project-goal-apply",
            "original_text": "用户明确要求创建或替代项目目标，并保留既有项目数据。"
        },
        "goal": {
            "goal_id": goal_id,
            "raw_request": format!("完成 {goal_id}"),
            "understood_objective": format!("交付 {goal_id} 的真实目标"),
            "acceptance_criteria": ["公开入口可观察目标内容和当前需求绑定"],
            "non_goals": ["不修改需求账本"],
            "assumptions": [],
            "ambiguities": [],
            "questions": [],
            "scope": {
                "allowed_paths": ["rust/src/main/**"],
                "forbidden_paths": [".appsdk/requirements.json"]
            },
            "status": status,
            "confirmed_by": "fixture-user",
            "confirmed_at": "2026-11-18T00:00:00Z",
            "created_at": "2026-11-18T00:00:00Z"
        }
    });
    let input = root.join(format!("{goal_id}-{operation}.json"));
    fs::write(&input, serde_json::to_vec_pretty(&request).unwrap()).unwrap();
    input
}

fn run_project_goal_apply(
    root: &Path,
    input: &Path,
) -> std::process::Output {
    run(&[
        "goal",
        root.to_str().unwrap(),
        "apply",
        "--input",
        input.to_str().unwrap(),
        "--json",
    ])
}

#[test]
fn project_goal_apply_preserves_existing_long_horizon_subscription_state() {
    let root = temp_root("project-goal-apply-subscription");
    assert!(run(&["new", root.to_str().unwrap()]).status.success());
    let goal_path = root.join(".appsdk/goal.json");
    let initial_goal = fs::read(&goal_path).unwrap();
    let initial: Value = serde_json::from_slice(&initial_goal).unwrap();
    let initial_revision = initial["goal_revision"].as_u64().unwrap_or(0);

    let goal_id = "goal-with-existing-subscription";
    let encoded_owner: String = "master-peer"
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let encoded_goal: String = goal_id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let subscription_path = root
        .join(".appsdk-control/goals")
        .join(encoded_owner)
        .join("records")
        .join(format!("{encoded_goal}.json"));
    fs::create_dir_all(subscription_path.parent().unwrap()).unwrap();
    let subscription = serde_json::json!({
        "goal_id": goal_id,
        "owner": "master-peer",
        "goal_path": "long-task.md",
        "desired": "subscribed",
        "active": true,
        "interval": "20m",
        "subscription_id": "sub-existing",
        "record_source": "owner-scoped",
        "project_scope": root,
        "session_id": "test-session",
        "collab_subscription": {
            "id": "sub-existing",
            "status": "armed",
            "subject": "goal:task:existing",
            "fired_count": 2
        }
    });
    let subscription_bytes = serde_json::to_vec_pretty(&subscription).unwrap();
    fs::write(&subscription_path, &subscription_bytes).unwrap();

    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let collab_calls = root.join("collab-calls");
    let fake_collab = fake_bin.join("collab");
    fs::write(
        &fake_collab,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 91\n",
            collab_calls.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_collab, fs::Permissions::from_mode(0o755)).unwrap();
    let apply = |input: &Path| {
        let mut command = Command::new(binary());
        command
            .args([
                "goal",
                root.to_str().unwrap(),
                "apply",
                "--input",
                input.to_str().unwrap(),
                "--json",
            ])
            .current_dir(&root)
            .env("APPSDK_HOME", test_global_registry_root_for_project(&root))
            .env("PATH", &fake_bin)
            .env_remove("TMUX_PANE");
        apply_test_git_bug_fixture(&mut command, &root);
        command.output().unwrap()
    };

    let replace = project_goal_request(
        &root,
        "replace",
        Some(initial_revision),
        0,
        "goal-with-subscription",
        "confirmed",
    );
    let replaced = apply(&replace);
    assert!(
        replaced.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&replaced.stdout),
        String::from_utf8_lossy(&replaced.stderr)
    );
    let receipt: Value = serde_json::from_slice(&replaced.stdout).unwrap();
    assert_eq!(receipt["subscription_changed"], false);
    let goal_bytes = fs::read(&goal_path).unwrap();
    assert_ne!(goal_bytes, initial_goal, "replace must change .appsdk/goal.json");
    assert_eq!(
        fs::read(&subscription_path).unwrap(),
        subscription_bytes,
        "goal apply must preserve the existing owner-scoped subscription projection"
    );
    assert!(
        !collab_calls.exists(),
        "goal apply must not call Collab or mutate its canonical subscription state"
    );

    let mut rejected_request: Value =
        serde_json::from_slice(&fs::read(&replace).unwrap()).unwrap();
    rejected_request["base_goal_revision"] = serde_json::json!(initial_revision);
    rejected_request["authorization"]
        .as_object_mut()
        .unwrap()
        .remove("source");
    let rejected_path = root.join("rejected-apply.json");
    fs::write(
        &rejected_path,
        serde_json::to_vec_pretty(&rejected_request).unwrap(),
    )
    .unwrap();
    let rejected = apply(&rejected_path);
    assert!(!rejected.status.success());
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(diagnostics.contains("GOAL_APPLY_AUTHORIZATION_INVALID"), "{diagnostics}");
    assert_eq!(fs::read(&subscription_path).unwrap(), subscription_bytes);
    assert_eq!(fs::read(&goal_path).unwrap(), goal_bytes);
    assert!(!collab_calls.exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_goal_apply_creates_and_replaces_with_history() {
    let root = temp_root("project-goal-apply");
    assert!(run(&["new", root.to_str().unwrap()]).status.success());
    fs::remove_file(root.join(".appsdk/goal.json")).unwrap();

    let create = project_goal_request(&root, "create", None, 0, "goal-first", "confirmed");
    let created = run_project_goal_apply(&root, &create);
    assert!(
        created.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&created.stdout),
        String::from_utf8_lossy(&created.stderr)
    );
    let current: Value =
        serde_json::from_slice(&fs::read(root.join(".appsdk/goal.json")).unwrap()).unwrap();
    assert_eq!(current["goal_id"], "goal-first");
    assert_eq!(current["requirements_version"], 0);
    assert_eq!(current["goal_revision"], 1);

    let replace = project_goal_request(&root, "replace", Some(1), 0, "goal-second", "confirmed");
    let replaced = run_project_goal_apply(&root, &replace);
    assert!(
        replaced.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&replaced.stdout),
        String::from_utf8_lossy(&replaced.stderr)
    );
    let current: Value =
        serde_json::from_slice(&fs::read(root.join(".appsdk/goal.json")).unwrap()).unwrap();
    assert_eq!(current["goal_id"], "goal-second");
    assert_eq!(current["goal_revision"], 2);
    assert_eq!(current["replacement_history"][0]["goal"]["goal_id"], "goal-first");
    assert_eq!(current["replacement_history"][0]["superseded_by_revision"], 2);
    assert!(!root.join(".appsdk-control/long-task-goal.json").exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_goal_apply_rejects_invalid_input_without_mutation() {
    let root = temp_root("project-goal-apply-reject");
    assert!(run(&["new", root.to_str().unwrap()]).status.success());
    let goal_path = root.join(".appsdk/goal.json");
    let before = fs::read(&goal_path).unwrap();

    let invalid = project_goal_request(&root, "replace", Some(0), 0, "goal-invalid", "admitted");
    let rejected = run_project_goal_apply(&root, &invalid);
    assert!(!rejected.status.success());
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(diagnostics.contains("GOAL_APPLY"), "{diagnostics}");
    assert_eq!(fs::read(&goal_path).unwrap(), before);

    let scope = project_goal_request(&root, "replace", Some(0), 0, "goal-scope", "confirmed");
    let mut scope_request: Value =
        serde_json::from_slice(&fs::read(&scope).unwrap()).unwrap();
    scope_request["goal"]["scope"]["allowed_paths"] = serde_json::json!(["outside/**"]);
    fs::write(&scope, serde_json::to_vec_pretty(&scope_request).unwrap()).unwrap();
    let scope_output = run_project_goal_apply(&root, &scope);
    assert!(!scope_output.status.success());
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&scope_output.stdout),
        String::from_utf8_lossy(&scope_output.stderr)
    );
    assert!(
        diagnostics.contains("GOAL_APPLY_SCOPE_OUT_OF_AUTHORIZATION"),
        "{diagnostics}"
    );
    assert_eq!(fs::read(&goal_path).unwrap(), before);

    let authorization = project_goal_request(
        &root,
        "replace",
        Some(0),
        0,
        "goal-authorization",
        "confirmed",
    );
    let mut authorization_request: Value =
        serde_json::from_slice(&fs::read(&authorization).unwrap()).unwrap();
    authorization_request["authorization"]
        .as_object_mut()
        .unwrap()
        .remove("source");
    fs::write(
        &authorization,
        serde_json::to_vec_pretty(&authorization_request).unwrap(),
    )
    .unwrap();
    let authorization_output = run_project_goal_apply(&root, &authorization);
    assert!(!authorization_output.status.success());
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&authorization_output.stdout),
        String::from_utf8_lossy(&authorization_output.stderr)
    );
    assert!(diagnostics.contains("GOAL_APPLY_AUTHORIZATION_INVALID"), "{diagnostics}");
    assert_eq!(fs::read(&goal_path).unwrap(), before);

    let malformed = root.join("goal-malformed.json");
    fs::write(&malformed, b"{malformed").unwrap();
    let malformed_output = run_project_goal_apply(&root, &malformed);
    assert!(!malformed_output.status.success());
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&malformed_output.stdout),
        String::from_utf8_lossy(&malformed_output.stderr)
    );
    assert!(diagnostics.contains("GOAL_APPLY_INPUT_INVALID"), "{diagnostics}");
    assert_eq!(fs::read(&goal_path).unwrap(), before);

    let stale = project_goal_request(&root, "replace", Some(0), 1, "goal-stale", "confirmed");
    let stale_output = run_project_goal_apply(&root, &stale);
    assert!(!stale_output.status.success());
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&stale_output.stdout),
        String::from_utf8_lossy(&stale_output.stderr)
    );
    assert!(diagnostics.contains("REQUIREMENTS_MISSING"), "{diagnostics}");
    assert_eq!(fs::read(&goal_path).unwrap(), before);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_goal_apply_recovers_stale_goal_for_current_consumer() {
    let fixture = RequirementLockFixture::new("project-goal-apply-stale-consumer");
    requirement_lock_json(fixture.apply(&fixture.request(
        "requirement-first",
        "authority",
        0,
        "create",
        "项目需求必须保持当前绑定",
    )));
    fs::remove_file(fixture.0.join(".appsdk/goal.json")).unwrap();
    let create = project_goal_request(
        &fixture.0,
        "create",
        None,
        1,
        "goal-current",
        "confirmed",
    );
    let created = run_project_goal_apply(&fixture.0, &create);
    assert!(
        created.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&created.stdout),
        String::from_utf8_lossy(&created.stderr)
    );
    assert!(run(&["verify", "--admission", fixture.0.to_str().unwrap()]).status.success());

    requirement_lock_json(fixture.apply(&fixture.request(
        "requirement-change",
        "authority",
        1,
        "replace",
        "用户明确更新当前需求",
    )));
    let stale = run(&["verify", "--admission", fixture.0.to_str().unwrap()]);
    assert!(!stale.status.success());
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&stale.stdout),
        String::from_utf8_lossy(&stale.stderr)
    );
    assert!(diagnostics.contains("REQUIREMENTS_VERSION_STALE"), "{diagnostics}");

    let replace = project_goal_request(
        &fixture.0,
        "replace",
        Some(1),
        2,
        "goal-recovered",
        "confirmed",
    );
    assert!(run_project_goal_apply(&fixture.0, &replace).status.success());
    let current: Value = serde_json::from_slice(
        &fs::read(fixture.0.join(".appsdk/goal.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(current["requirements_version"], 2);
    assert_eq!(current["replacement_history"][0]["goal"]["goal_id"], "goal-current");
    assert!(run(&["verify", "--admission", fixture.0.to_str().unwrap()]).status.success());
}
