struct RequirementLockFixture(PathBuf);

const REQUIREMENT_LOCK_PREVIOUS_SDK: &[u8] = include_bytes!(
    "../../../docs/evidence/user-requirement-truth-lock-20261007/session-lock/fixture-appsdk-0011.tar.gz"
);

impl RequirementLockFixture {
    fn new(name: &str) -> Self {
        let root = temp_root(name);
        let output = run(&["new", root.to_str().unwrap()]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Self(root)
    }

    fn request(&self, request_id: &str, id: &str, base: u64, operation: &str, text: &str) -> Value {
        let project: Value =
            serde_json::from_slice(&fs::read(self.0.join(".appsdk/project.json")).unwrap())
                .unwrap();
        let mut input = serde_json::json!({
            "project_id": project["project_id"], "request_id": request_id,
            "requirement_id": id, "base_version": base, "operation": operation,
            "authorization": {"role":"user", "source":format!("test-conversation/{request_id}"),
                "original_text":format!("用户明确{operation}指定需求{id}：{text}")}
        });
        if matches!(operation, "create" | "replace") {
            input["text"] = Value::String(text.into());
        }
        input
    }

    fn apply(&self, input: &Value) -> std::process::Output {
        let path = self.0.join("requirement-input.json");
        fs::write(&path, serde_json::to_vec_pretty(input).unwrap()).unwrap();
        run(&[
            "requirements",
            "apply",
            self.0.to_str().unwrap(),
            "--input",
            path.to_str().unwrap(),
        ])
    }

    fn read(&self, command: &str) -> Value {
        requirement_lock_json(run(&["requirements", command, self.0.to_str().unwrap()]))
    }

    fn goal(&self, version: u64) {
        let goal = serde_json::json!({"goal_id":"requirement-task", "raw_request":"实现并遵循项目需求", "understood_objective":"实现项目需求",
            "acceptance_criteria":["保留权威需求和历史"], "non_goals":[], "assumptions":[], "ambiguities":[], "questions":[],
            "status":"confirmed", "confirmed_by":"fixture-user", "confirmed_at":"2026-10-07T00:00:00Z", "created_at":"2026-10-07T00:00:00Z",
            "requirements_version":version});
        fs::write(
            self.0.join(".appsdk/goal.json"),
            serde_json::to_vec(&goal).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for RequirementLockFixture {
    fn drop(&mut self) {
        if self.0.join(".appsdk").exists() {
            let _ = fs::set_permissions(self.0.join(".appsdk"), fs::Permissions::from_mode(0o755));
        }
        let _ = fs::remove_dir_all(&self.0);
        let _ = fs::remove_dir_all(test_global_registry_root_for_project(&self.0));
        if let (Some(parent), Some(name)) =
            (self.0.parent(), self.0.file_name().and_then(|v| v.to_str()))
        {
            let _ = fs::remove_file(parent.join(format!(".appsdk-reset-transaction-{name}.lock")));
        }
    }
}

fn requirement_lock_json(output: std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn requirement_lock_original_text_versions_and_explicit_revoke() {
    let f = RequirementLockFixture::new("requirement-lock-history");
    assert_eq!(f.read("show")["status"], "not_established");
    let original = "用户原文：只能用户提出修改才能修改，否则永远作为真理存在\n保留所有历史。";
    let first = f.request("message-first", "authority", 0, "create", original);
    assert_eq!(requirement_lock_json(f.apply(&first))["version"], 1);
    assert_eq!(f.read("show")["requirements"][0]["text"], original);
    let other = f.request(
        "message-other",
        "quality",
        0,
        "create",
        "reviewer必须逐条核验权威需求",
    );
    requirement_lock_json(f.apply(&other));
    let change = f.request(
        "message-change",
        "authority",
        1,
        "replace",
        "用户在会话里提供授权就行了，不要过度校验",
    );
    assert_eq!(
        requirement_lock_json(f.apply(&change))["requirement_version"],
        2
    );
    let current = f.read("show");
    assert_eq!(current["version"], 3);
    assert_eq!(current["requirements"].as_array().unwrap().len(), 2);
    let history = f.read("history");
    assert_eq!(history["changes"][0]["request"]["text"], original);
    assert_eq!(
        history["changes"][2]["request"]["authorization"],
        change["authorization"]
    );
    let revoke = f.request("message-revoke", "authority", 2, "revoke", "明确撤销该条");
    requirement_lock_json(f.apply(&revoke));
    assert_eq!(f.read("history")["version"], 4);
    assert_eq!(f.read("verify")["status"], "locked");
    let restored = f.request(
        "message-restore",
        "authority",
        3,
        "replace",
        "用户明确恢复为新版本",
    );
    assert_eq!(
        requirement_lock_json(f.apply(&restored))["requirement_version"],
        4
    );
}

#[test]
fn requirement_lock_missing_authorization_scope_and_replay_preserve_history() {
    let f = RequirementLockFixture::new("requirement-lock-authorization");
    let first = f.request(
        "message-first",
        "authority",
        0,
        "create",
        "长期需求不可自行降低验收",
    );
    requirement_lock_json(f.apply(&first));
    let before = f.read("history");
    assert_eq!(requirement_lock_json(f.apply(&first))["reused"], true);
    let mut request = f.request("message-change", "authority", 1, "replace", "降低验收");
    request.as_object_mut().unwrap().remove("authorization");
    request["confirmed_by"] = Value::String("user".into());
    assert!(!f.apply(&request).status.success());
    let mut request = f.request("message-agent", "authority", 1, "replace", "agent自行撤销");
    request["authorization"]["role"] = Value::String("agent".into());
    assert!(!f.apply(&request).status.success());
    let mut request = f.request("message-missing", "authority", 1, "replace", "没有会话来源");
    request["authorization"]
        .as_object_mut()
        .unwrap()
        .remove("source");
    assert!(!f.apply(&request).status.success());
    let mut request = f.request("message-project", "authority", 1, "replace", "错项目");
    request["project_id"] = Value::String("other-project".into());
    assert!(!f.apply(&request).status.success());
    let request = f.request("message-stale", "authority", 0, "replace", "旧版本");
    assert!(!f.apply(&request).status.success());
    let request = f.request("message-absent", "missing", 1, "replace", "错条目");
    assert!(!f.apply(&request).status.success());
    let mut reused = first.clone();
    reused["text"] = Value::String("同请求偷偷改内容".into());
    let rejected = f.apply(&reused);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("REQUIREMENTS_REQUEST_CONFLICT"));
    assert_eq!(f.read("history"), before);
}

#[test]
fn requirement_lock_cancel_and_write_failure_do_not_commit() {
    let f = RequirementLockFixture::new("requirement-lock-cancel-write");
    let cancel = f.request("message-cancel", "authority", 0, "cancel", "取消提交");
    assert_eq!(requirement_lock_json(f.apply(&cancel))["cancelled"], true);
    assert_eq!(f.read("show")["status"], "not_established");
    let first = f.request("message-first", "authority", 0, "create", "原需求");
    requirement_lock_json(f.apply(&first));
    let before = f.read("history");
    let permissions = fs::metadata(f.0.join(".appsdk")).unwrap().permissions();
    fs::set_permissions(f.0.join(".appsdk"), fs::Permissions::from_mode(0o555)).unwrap();
    let change = f.request(
        "message-write",
        "authority",
        1,
        "replace",
        "明确变更但写入失败",
    );
    let failed = f.apply(&change);
    fs::set_permissions(f.0.join(".appsdk"), permissions).unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("REQUIREMENTS_WRITE_FAILED"));
    assert_eq!(f.read("history"), before);
    assert!(
        !fs::read_dir(f.0.join(".appsdk")).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("staging"))
    );
    assert_eq!(requirement_lock_json(f.apply(&change))["version"], 2);
    assert_eq!(requirement_lock_json(f.apply(&change))["reused"], true);
}

#[test]
fn requirement_lock_rejects_inconsistent_result_and_wrong_project_ledger() {
    let f = RequirementLockFixture::new("requirement-lock-consistency");
    requirement_lock_json(f.apply(&f.request(
        "conversation/message:1",
        "authority",
        0,
        "create",
        "用户原文",
    )));
    let ledger = f.0.join(".appsdk/requirements.json");
    let original = fs::read(&ledger).unwrap();
    let mut changed: Value = serde_json::from_slice(&original).unwrap();
    changed["changes"][0]["result"]["text"] = Value::String("没有用户指令的投影改写".into());
    fs::write(&ledger, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(!run(&["requirements", "show", f.0.to_str().unwrap()])
        .status
        .success());
    fs::write(&ledger, &original).unwrap();
    let project_path = f.0.join(".appsdk/project.json");
    let mut project: Value = serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();
    project["project_id"] = Value::String("another-project".into());
    fs::write(project_path, serde_json::to_vec(&project).unwrap()).unwrap();
    assert!(!run(&["requirements", "verify", f.0.to_str().unwrap()])
        .status
        .success());
}

#[test]
fn requirement_lock_read_error_is_not_an_absent_lock() {
    let f = RequirementLockFixture::new("requirement-lock-read-error");
    let appsdk = f.0.join(".appsdk");
    let saved = f.0.join("saved-appsdk");
    fs::rename(&appsdk, &saved).unwrap();
    fs::write(&appsdk, "not a directory").unwrap();
    let output = run(&["requirements", "show", f.0.to_str().unwrap()]);
    fs::remove_file(&appsdk).unwrap();
    fs::rename(&saved, &appsdk).unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("REQUIREMENTS_INVALID"));
}

#[test]
fn requirement_lock_previous_official_sdk_pin_upgrade_retains_history() {
    let root = temp_root("requirement-lock-upgrade-0011");
    fs::create_dir_all(&root).unwrap();
    let archive = root.join("previous-sdk.tar.gz");
    fs::write(&archive, REQUIREMENT_LOCK_PREVIOUS_SDK).unwrap();
    assert!(Command::new("tar")
        .args([
            "-xzf",
            archive.to_str().unwrap(),
            "-C",
            root.to_str().unwrap()
        ])
        .status()
        .unwrap()
        .success());
    fs::remove_file(archive).unwrap();
    let f = RequirementLockFixture(root);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(f.0.join(".appsdk/sdk.lock")).unwrap()).unwrap()
            ["version"],
        "0.1.0011"
    );
    requirement_lock_json(f.apply(&f.request(
        "upgrade/user-message:1",
        "authority",
        0,
        "create",
        "SDK升级必须保留用户需求",
    )));
    let ledger = f.0.join(".appsdk/requirements.json");
    let before = fs::read(&ledger).unwrap();
    let output = run(&[
        "pin-lock",
        f.0.to_str().unwrap(),
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(&ledger).unwrap(), before);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(f.0.join(".appsdk/sdk.lock")).unwrap()).unwrap()
            ["version"],
        "0.1.0013"
    );
    assert!(f
        .0
        .join(".appsdk/migrations/0.1.0011-to-0.1.0012/record.json")
        .is_file());
    assert_eq!(
        f.read("show")["requirements"][0]["text"],
        "SDK升级必须保留用户需求"
    );
}

#[test]
fn requirement_lock_guidance_plan_consumes_current_goal_binding() {
    let f = RequirementLockFixture::new("requirement-lock-guidance");
    requirement_lock_json(f.apply(&f.request(
        "message-first",
        "authority",
        0,
        "create",
        "任务必须遵循当前需求",
    )));
    f.goal(0);
    assert!(run(&["guide", "compile", f.0.to_str().unwrap()])
        .status
        .success());
    init_git(&f.0);
    fs::write(f.0.join("plan.json"), serde_json::to_vec(&serde_json::json!({
        "schema_version":1,"mode":"develop","goal_id":"requirement-task","task_id":"requirement-task",
        "module_id":"app-core","objective":"遵循需求","scope_paths":["playground/experiments/**"],
        "steps":[{"step_id":"step-1","node_id":"requirements","action":"analyze","owner":"app-core","expected_evidence":["requirements"]}]
    })).unwrap()).unwrap();
    let args = [
        "guide",
        "plan",
        f.0.to_str().unwrap(),
        "--task",
        "requirement-task",
        "--input",
        "plan.json",
    ];
    let stale = run(&args);
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("REQUIREMENTS_VERSION_STALE"));
    f.goal(1);
    let current = run(&args);
    assert!(
        current.status.success(),
        "{}",
        String::from_utf8_lossy(&current.stderr)
    );
}

#[test]
fn requirement_lock_parallel_old_version_has_one_commit() {
    let f = RequirementLockFixture::new("requirement-lock-parallel");
    requirement_lock_json(f.apply(&f.request("message-first", "authority", 0, "create", "原需求")));
    let a = f.0.join("a.json");
    let b = f.0.join("b.json");
    fs::write(
        &a,
        serde_json::to_vec(&f.request("message-a", "authority", 1, "replace", "用户变更A"))
            .unwrap(),
    )
    .unwrap();
    fs::write(
        &b,
        serde_json::to_vec(&f.request("message-b", "authority", 1, "replace", "用户变更B"))
            .unwrap(),
    )
    .unwrap();
    let ca = Command::new(binary())
        .args([
            "requirements",
            "apply",
            f.0.to_str().unwrap(),
            "--input",
            a.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let cb = Command::new(binary())
        .args([
            "requirements",
            "apply",
            f.0.to_str().unwrap(),
            "--input",
            b.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let oa = ca.wait_with_output().unwrap();
    let ob = cb.wait_with_output().unwrap();
    assert_ne!(oa.status.success(), ob.status.success());
    assert_eq!(f.read("history")["version"], 2);
}

#[test]
fn requirement_lock_goal_consumers_reject_stale_versions_and_corrupt_ledger() {
    let f = RequirementLockFixture::new("requirement-lock-consumers");
    requirement_lock_json(f.apply(&f.request("message-first", "authority", 0, "create", "原需求")));
    f.goal(1);
    assert!(run(&["verify", f.0.to_str().unwrap()]).status.success());
    requirement_lock_json(f.apply(&f.request(
        "message-change",
        "authority",
        1,
        "replace",
        "用户合法改版",
    )));
    for args in [
        vec!["verify", f.0.to_str().unwrap()],
        vec!["compile", f.0.to_str().unwrap()],
        vec![
            "review-context",
            f.0.to_str().unwrap(),
            "--module",
            "app-core",
        ],
    ] {
        let output = run(&args);
        assert!(
            !output.status.success(),
            "accepted stale requirement binding: {args:?}"
        );
        let diagnostics = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            diagnostics.contains("REQUIREMENTS_VERSION_STALE"),
            "{args:?}: {diagnostics}"
        );
    }
    f.goal(2);
    assert!(run(&["verify", f.0.to_str().unwrap()]).status.success());
    let ledger = f.0.join(".appsdk/requirements.json");
    fs::write(&ledger, b"{malformed").unwrap();
    assert!(!run(&["requirements", "verify", f.0.to_str().unwrap()])
        .status
        .success());
    assert!(!run(&["verify", f.0.to_str().unwrap()]).status.success());
}

#[test]
fn requirement_lock_sdk_refresh_and_reset_preserve_authorized_history() {
    let f = RequirementLockFixture::new("requirement-lock-reset");
    requirement_lock_json(f.apply(&f.request(
        "message-first",
        "authority",
        0,
        "create",
        "长期原文",
    )));
    let before = f.read("history");
    let refreshed = run(&["init", f.0.to_str().unwrap()]);
    assert!(
        refreshed.status.success(),
        "{}",
        String::from_utf8_lossy(&refreshed.stderr)
    );
    assert_eq!(f.read("history"), before);
    init_git(&f.0);
    requirement_lock_commit(&f.0, "fixture requirement baseline");
    let reset = run(&[
        "reset-governance",
        f.0.to_str().unwrap(),
        "--discard-legacy",
    ]);
    assert!(
        reset.status.success(),
        "{}",
        String::from_utf8_lossy(&reset.stderr)
    );
    assert_eq!(f.read("history"), before);
    requirement_lock_commit(&f.0, "fixture reset baseline");
    let fresh = run(&["init", f.0.to_str().unwrap(), "--fresh", "--discard-legacy"]);
    assert!(
        fresh.status.success(),
        "{}",
        String::from_utf8_lossy(&fresh.stderr)
    );
    assert_eq!(f.read("history"), before);
}

fn requirement_lock_commit(root: &Path, message: &str) {
    let status = Command::new("git")
        .args(["-C", root.to_str().unwrap(), "status", "--porcelain"])
        .output()
        .unwrap();
    assert!(status.status.success());
    if status.stdout.is_empty() {
        return;
    }
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "add", "."])
        .status()
        .unwrap()
        .success());
    let commit = Command::new("git")
        .args(["-C", root.to_str().unwrap(), "commit", "-m", message])
        .output()
        .unwrap();
    assert!(
        commit.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&commit.stdout),
        String::from_utf8_lossy(&commit.stderr)
    );
}

#[test]
fn requirement_lock_review_context_contains_all_items_history_and_invalidates_old_review() {
    let root = temp_root("requirement-lock-review");
    prepare_lifecycle_chain_fixture(&root);
    let f = RequirementLockFixture(root);
    requirement_lock_json(f.apply(&f.request(
        "message-first",
        "authority",
        0,
        "create",
        "只能用户明确修改需求",
    )));
    requirement_lock_json(f.apply(&f.request(
        "message-other",
        "quality",
        0,
        "create",
        "reviewer必须逐条核验",
    )));
    set_goal_field(&f.0, "requirements_version", serde_json::json!(2));
    fs::remove_file(f.0.join(".appsdk/records/review-record-app-core.json")).unwrap();
    let old = review_context_output(&f.0, "app-core");
    assert_eq!(
        old["context"]["long_term_requirements"]["requirements"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        old["context"]["long_term_requirements"]["history"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(old["prompt"]
        .as_str()
        .unwrap()
        .contains("test-conversation/message-first"));
    requirement_lock_json(f.apply(&f.request(
        "message-change",
        "authority",
        1,
        "replace",
        "用户在会话提供授权即可",
    )));
    let stale = run(&[
        "review-context",
        f.0.to_str().unwrap(),
        "--module",
        "app-core",
    ]);
    assert!(!stale.status.success());
    set_goal_field(&f.0, "requirements_version", serde_json::json!(3));
    let fresh = review_context_output(&f.0, "app-core");
    assert_ne!(fresh["context_id"], old["context_id"]);
    let stale_input = write_architecture_input(
        &f.0,
        Some(serde_json::json!({"context_id":old["context_id"],"checked":true})),
        None,
    );
    assert!(!produce_architecture(&f.0, &stale_input).status.success());
    let fresh_input = write_architecture_input(
        &f.0,
        Some(serde_json::json!({"context_id":fresh["context_id"],"checked":true})),
        None,
    );
    assert!(produce_architecture(&f.0, &fresh_input).status.success());
    assert!(f
        .0
        .join(".appsdk/contracts/records/user-requirement-request.schema.json")
        .is_file());
}
