use super::*;

const THREAD_C: &str = "01a0d641-13f1-7b31-8d84-530f0f2d3fdd";

fn relationship_fixture() -> AppFixture {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"relationship consumer"}"#,
        ],
        THREAD_A,
    );
    fixture
}

fn notices(fixture: &AppFixture, thread: &str, subject: &str) -> Vec<Value> {
    let received = fixture.run_public(&["recv", "--timeout", "0"], thread);
    received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["subject"] == subject)
        .cloned()
        .collect()
}

#[test]
fn minimal_dispatch_prefers_unassigned_over_owned_or_invited_peer() {
    for owned in [false, true] {
        let fixture = relationship_fixture();
        let alice = fixture.run_public(&["context"], THREAD_B);
        let bob = fixture.run_public(&["context"], THREAD_C);
        assert_eq!(alice["work_roles"], json!([]));
        publish(&fixture, "first", &[]);
        if owned {
            fixture.run_public(
                &["board", "claim", "first", "--expected-revision", "1"],
                THREAD_B,
            );
        } else {
            fixture.run_public(
                &["board", "dispatch", "first", "--expected-revision", "1"],
                THREAD_A,
            );
        }
        fixture.run_public(&["context", "--work-roles", "designer"], THREAD_B);
        publish(&fixture, "preferred", &["--prefer-role", "designer"]);
        let preferred = fixture.run_public(
            &["board", "dispatch", "preferred", "--expected-revision", "1"],
            THREAD_A,
        );
        assert_eq!(
            preferred["task"]["invited_peer"],
            alice["identity"]["worker_id"]
        );
        publish(&fixture, "pinned", &["--executor", "Alice"]);
        let pinned = fixture.run_public(
            &["board", "dispatch", "pinned", "--expected-revision", "1"],
            THREAD_A,
        );
        assert_eq!(
            pinned["task"]["invited_peer"],
            alice["identity"]["worker_id"]
        );
        publish(&fixture, "second", &[]);
        let result = fixture.run_public(
            &["board", "dispatch", "second", "--expected-revision", "1"],
            THREAD_A,
        );
        assert_eq!(
            result["task"]["invited_peer"], bob["identity"]["worker_id"],
            "owned={owned}: {result}"
        );
    }
}

#[test]
fn minimal_registration_notice_is_once_and_role_changes_are_distinct() {
    let mut fixture = relationship_fixture();
    assert!(notices(&fixture, THREAD_A, "peer-registered").is_empty());
    fixture.run_public(&["context"], THREAD_B);
    let joined = notices(&fixture, THREAD_A, "peer-registered");
    assert_eq!(joined.len(), 1, "{joined:?}");
    fixture.run_public(&["context"], THREAD_B);
    assert!(notices(&fixture, THREAD_A, "peer-registered").is_empty());
    fixture.run_public(&["context", "--work-roles", "reviewer"], THREAD_C);
    let received = fixture.run_public(&["recv", "--timeout", "0"], THREAD_A);
    for subject in ["peer-registered", "work-roles-changed"] {
        assert_eq!(
            received["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["subject"] == subject)
                .count(),
            1,
            "{received}"
        );
    }
    fixture.run_public(&["context", "--work-roles", "reviewer"], THREAD_C);
    assert!(notices(&fixture, THREAD_A, "work-roles-changed").is_empty());
    fixture.stop_daemon();
    fixture.start_daemon();
    fixture.run_public(&["context"], THREAD_B);
    assert!(notices(&fixture, THREAD_A, "peer-registered").is_empty());
}

#[test]
fn minimal_role_change_notifies_counterpart_without_reassigning_task() {
    let fixture = relationship_fixture();
    fixture.run_public(&["context"], THREAD_B);
    notices(&fixture, THREAD_A, "peer-registered");
    publish(&fixture, "retained", &[]);
    fixture.run_public(
        &["board", "claim", "retained", "--expected-revision", "1"],
        THREAD_B,
    );
    let before = fixture.run_public(&["task", "status", "retained"], THREAD_B);
    fixture.run_public(&["context", "--work-roles", "designer"], THREAD_B);
    assert_eq!(notices(&fixture, THREAD_A, "work-roles-changed").len(), 1);
    fixture.run_public(&["context", "--work-roles", "designer"], THREAD_B);
    assert!(notices(&fixture, THREAD_A, "work-roles-changed").is_empty());
    fixture.run_public(
        &["context", "--for-name", "Alice", "--work-roles", "reviewer"],
        THREAD_A,
    );
    assert_eq!(notices(&fixture, THREAD_B, "work-roles-changed").len(), 1);
    let after = fixture.run_public(&["task", "status", "retained"], THREAD_B);
    assert_eq!(before["owner"], after["owner"]);
    assert_eq!(before["status"], after["status"]);
}

#[test]
fn minimal_registration_without_master_has_no_notice_or_later_backfill() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_master","authorization":"fixture establishes a master after peers registered"}"#], THREAD_A);
    assert!(notices(&fixture, THREAD_A, "peer-registered").is_empty());
    fixture.run_public(&["context"], THREAD_B);
    assert!(notices(&fixture, THREAD_A, "peer-registered").is_empty());
    fixture.run_public(&["context"], THREAD_C);
    assert_eq!(notices(&fixture, THREAD_A, "peer-registered").len(), 1);
    assert!(notices(&fixture, THREAD_B, "peer-registered").is_empty());
}

#[test]
fn minimal_registration_notice_survives_a_rejected_native_wake() {
    let fixture = relationship_fixture();
    fixture
        .state
        .lock()
        .unwrap()
        .get_mut(THREAD_A)
        .unwrap()
        .reject_input = true;
    let peer = fixture.run_public(&["context"], THREAD_B);
    let current = fixture.current_state();
    let messages: Vec<_> = current["msgs"]
        .as_object()
        .unwrap()
        .values()
        .filter(|message| message["subject"] == "peer-registered")
        .collect();
    assert_eq!(messages.len(), 1);
    let id = messages[0]["id"].as_str().unwrap();
    assert_eq!(messages[0]["state"], "pending");
    assert!(current["notification_delivery_accepted"][id].is_null());
    assert!(current["notification_delivery_failures"][id].is_object());
    fixture
        .state
        .lock()
        .unwrap()
        .get_mut(THREAD_A)
        .unwrap()
        .reject_input = false;
    assert_eq!(notices(&fixture, THREAD_A, "peer-registered")[0]["id"], id);
    let again = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(again["binding"], peer["binding"]);
    assert!(notices(&fixture, THREAD_A, "peer-registered").is_empty());
}

#[test]
fn minimal_registration_worker_and_notice_share_current_publication() {
    let fixture = relationship_fixture();
    let path = fixture.project_state_for(&fixture.project, "appserver-cli");
    let retained = fixture.root.join("retained-current-state.json");
    std::fs::rename(&path, &retained).unwrap();
    std::fs::create_dir(&path).unwrap();
    let refused = fixture.command_public(&["context"], THREAD_B);
    assert!(
        !refused.status.success(),
        "unpublished Register must remain a failure"
    );
    let text = String::from_utf8_lossy(&refused.stderr);
    assert!(text.contains("CURRENT_STATE_WRITE_FAILED"), "{text}");
    let map: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let attempted = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["current"]["binding"]["session_id"] == THREAD_B)
        .unwrap();
    let worker = &attempted["current"]["binding"]["agent_id"];
    let who = fixture.run_public(&["who"], THREAD_A);
    assert!(!who["workers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| &row["id"] == worker));
    let inbox = fixture.run_public(&["inbox"], THREAD_A);
    assert!(!inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["subject"] == "peer-registered"));
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(retained, path).unwrap();
}

#[test]
fn minimal_workflow_change_notifies_only_affected_peers_and_reset() {
    let fixture = relationship_fixture();
    fixture.run_public(&["context", "--work-roles", "designer"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    let spec = json!({"role_task":"Design accepted work", "responsibilities":["Keep accepted task obligations"], "next_action":"Read current task", "completion_action":"Return evidence"}).to_string();
    let args = [
        "context",
        "--workflow-target",
        "role:designer",
        "--workflow-template",
        &spec,
    ];
    fixture.run_public(&args, THREAD_A);
    assert_eq!(
        notices(&fixture, THREAD_B, "workflow-template-changed").len(),
        1
    );
    assert!(notices(&fixture, THREAD_C, "workflow-template-changed").is_empty());
    fixture.run_public(&args, THREAD_A);
    assert!(notices(&fixture, THREAD_B, "workflow-template-changed").is_empty());
    fixture.run_public(
        &[
            "context",
            "--workflow-target",
            "role:designer",
            "--reset-workflow-template",
        ],
        THREAD_A,
    );
    assert_eq!(
        notices(&fixture, THREAD_B, "workflow-template-changed").len(),
        1
    );
}

#[test]
fn minimal_role_notice_survives_rejected_wake_without_rolling_back_roles() {
    let fixture = relationship_fixture();
    fixture.run_public(&["context"], THREAD_B);
    notices(&fixture, THREAD_A, "peer-registered");
    fixture
        .state
        .lock()
        .unwrap()
        .get_mut(THREAD_A)
        .unwrap()
        .reject_input = true;
    let updated = fixture.run_public(&["context", "--work-roles", "designer"], THREAD_B);
    assert_eq!(updated["work_roles"], json!(["designer"]));
    let receipt = &updated["work_roles_update"]["notifications"][0];
    assert_eq!(receipt["durable"], true, "{updated}");
    assert_eq!(receipt["ok"], false, "{updated}");
    assert!(
        receipt["notification_error"].as_str().is_some(),
        "{updated}"
    );
    assert_eq!(notices(&fixture, THREAD_A, "work-roles-changed").len(), 1);
    assert!(notices(&fixture, THREAD_A, "work-roles-changed").is_empty());
}

fn setup_roles() -> AppFixture {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"work roles consumer"}"#,
        ],
        THREAD_A,
    );
    let designer = fixture.run_public(&["context", "--work-roles", "designer,reviewer"], THREAD_B);
    assert_eq!(designer["work_roles"], json!(["designer", "reviewer"]));
    fixture.run_public(&["context", "--work-roles", "reviewer"], THREAD_C);
    fixture
}

fn member<'a>(board: &'a Value, name: &str) -> &'a Value {
    board["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|peer| peer["display_name"] == name)
        .unwrap()
}

fn publish(fixture: &AppFixture, id: &str, flags: &[&str]) -> Value {
    let mut args = vec![
        "board",
        "publish",
        id,
        "--title",
        "前端页面",
        "--description",
        "角色分工公开消费者",
        "--delivery-condition",
        "审查通过",
        "--test-condition",
        "公开入口断言",
    ];
    args.extend_from_slice(flags);
    fixture.run_public(&args, THREAD_A)
}

fn task<'a>(board: &'a Value, id: &str) -> &'a Value {
    board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .unwrap()
}

fn rejected(fixture: &AppFixture, args: &[&str], thread: &str) -> String {
    let result = fixture.command_public(args, thread);
    assert!(
        !result.status.success(),
        "unexpected success: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    format!(
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    )
}

#[test]
fn named_reviewer_accepts_a_delivered_task_without_being_owner_or_master() {
    let mut fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"review consumer"}"#,
        ],
        THREAD_A,
    );
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    publish(&fixture, "named-review", &[]);
    fixture.run_public(
        &["board", "claim", "named-review", "--expected-revision", "1"],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            "named-review",
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "task",
            "deliver",
            "named-review",
            "--no-candidate",
            "--evidence",
            "report ready",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let mut details = task(&board, "named-review").clone();
    details["public_visibility"] = json!(true);
    details["work_policy"] = json!({"reviewer_name":"Bob"});
    details["invitation"] = Value::Null;
    // Existing saved task input isolates reviewer admission from the new
    // policy writer, so a failure identifies the original authorization gap.
    fixture.update_current_state(|state| state["board_details"]["named-review"] = details);
    let accepted = fixture.run_public(
        &[
            "task",
            "review",
            "named-review",
            "--accept",
            "--evidence",
            "Bob reviewed",
        ],
        THREAD_C,
    );
    assert_eq!(accepted["status"], "accepted");
}

#[test]
fn roles_registration_master_update_clear_and_current_state_restart_are_public() {
    let mut fixture = setup_roles();
    let before = fixture.run_public(&["board", "show"], THREAD_A);
    assert_eq!(
        member(&before, "Alice")["work_roles"],
        json!(["designer", "reviewer"])
    );
    let restored = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(restored["work_roles"], json!(["designer", "reviewer"]));
    rejected(
        &fixture,
        &["context", "--work-roles", "designer", "--for-name", "Bob"],
        THREAD_B,
    );
    fixture.run_public(
        &["context", "--work-roles", "designer", "--for-name", "Bob"],
        THREAD_A,
    );
    fixture.restart_daemon();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    assert_eq!(
        member(&board, "Alice")["work_roles"],
        json!(["designer", "reviewer"])
    );
    assert_eq!(member(&board, "Bob")["work_roles"], json!(["designer"]));
    fixture.run_public(&["context", "--work-roles", ""], THREAD_B);
    let cleared = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(cleared["work_roles"], json!([]));
    fixture.run_public(
        &[
            "board",
            "policy",
            "--scope",
            "project",
            "--work-kind",
            "frontend",
            "--executor",
            "Alice",
        ],
        THREAD_A,
    );
    let state_path = fixture.project_state_for(&fixture.project, "appserver-cli");
    fixture.run_public(&["reset", "--project"], THREAD_A);
    match std::fs::read(state_path) {
        Ok(bytes) => {
            let retained: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(retained["state"]["name_work_roles"]
                .as_object()
                .unwrap()
                .is_empty());
            assert!(retained["state"]["project_work_policy"]
                .as_object()
                .unwrap()
                .is_empty());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("read reset current state: {error}"),
    }
    assert!(
        fixture.host_state.join("server.sock").exists(),
        "project reset preserves the host daemon"
    );
}

fn joined_role_text(values: &[&str]) -> String {
    values.join(" ").to_lowercase()
}

// Read the three current canonical work-role coordination rules from the
// shipped asset so this consumer asserts delivered rule text instead of
// brittle prose fragments that drift whenever the canonical wording changes.
fn canonical_work_role_rules() -> Vec<String> {
    let asset: Value = serde_json::from_str(include_str!(
        "../../skills/collab/references/workflow-templates.json"
    ))
    .expect("shipped workflow-templates.json must parse");
    let rules = &asset["coordination_rules"];
    [
        "work_role_assignment_rule",
        "work_role_execution_rule",
        "work_role_next_action",
    ]
    .iter()
    .map(|key| {
        rules[key]
            .as_str()
            .unwrap_or_else(|| panic!("missing canonical coordination rule {key}"))
            .to_owned()
    })
    .collect()
}

fn static_operation_kinds(snapshot: &Value) -> Vec<String> {
    snapshot["operations"]
        .as_array()
        .expect("context operations")
        .iter()
        .filter_map(|operation| operation["kind"].as_str())
        .filter(|kind| {
            matches!(
                *kind,
                "responsibility" | "next_action" | "completion_action" | "notification_rule"
            )
        })
        .map(str::to_owned)
        .collect()
}

#[test]
fn context_role_guidance_reaches_master_and_peer_with_latest_dynamic_roles() {
    let fixture = setup_roles();
    let master = fixture.run_public(&["context"], THREAD_A);
    let peer = fixture.run_public(&["context"], THREAD_B);

    for (label, snapshot) in [("master", &master), ("peer", &peer)] {
        let brief = &snapshot["role_brief"];
        assert!(
            brief.get("work_roles").is_none() && brief.get("tasks").is_none(),
            "{label} role_brief must reuse top-level dynamic values instead of mirroring them: {brief}"
        );
        let responsibilities = joined_role_text(
            &brief["responsibilities"]
                .as_array()
                .expect("role responsibilities")
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>(),
        );
        for rule in canonical_work_role_rules() {
            let expected = rule.to_lowercase();
            assert!(
                responsibilities.contains(expected.as_str()),
                "{label} role_brief responsibilities miss canonical coordination rule: {responsibilities}"
            );
        }
        assert!(
            !brief["next_action"]
                .as_str()
                .expect("role next_action")
                .to_lowercase()
                .contains("task constraints:"),
            "{label} common task-constraint text must live once in responsibilities, not be mirrored into next_action: {brief}"
        );
        assert!(
            snapshot["role_brief"]["workflow_template"]["identity"]
                .get("template")
                .is_none(),
            "{label} workflow_template must reference the selected source and revision without repeating its text: {snapshot}"
        );
        let duplicated = static_operation_kinds(snapshot);
        assert!(
            duplicated.is_empty(),
            "{label} operations must contain only fact-triggered actions; static workflow text belongs once in role_brief: {duplicated:?}"
        );
    }

    publish(
        &fixture,
        "role-context",
        &["--executor", "Alice", "--reviewer", "Bob"],
    );
    let before = fixture.run_public(&["board", "show"], THREAD_A);
    let saved = task(&before, "role-context");
    assert_eq!(saved["executor_name"], "Alice");
    assert_eq!(saved["reviewer_name"], "Bob");
    assert_eq!(saved["delivery_condition"], "审查通过");
    assert_eq!(saved["test_condition"], "公开入口断言");

    let replaced = fixture.run_public(&["context", "--work-roles", "translator"], THREAD_B);
    assert_eq!(replaced["work_roles"], json!(["translator"]));
    let after_replace = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(after_replace["work_roles"], json!(["translator"]));
    assert_eq!(
        member(&fixture.run_public(&["board", "show"], THREAD_A), "Alice")["work_roles"],
        json!(["translator"])
    );
    assert!(after_replace["role_brief"].get("work_roles").is_none());

    let cleared = fixture.run_public(&["context", "--work-roles", ""], THREAD_B);
    assert_eq!(cleared["work_roles"], json!([]));
    let after_clear = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(after_clear["work_roles"], json!([]));
    assert_eq!(
        member(&fixture.run_public(&["board", "show"], THREAD_A), "Alice")["work_roles"],
        json!([])
    );
    assert!(after_clear["role_brief"].get("work_roles").is_none());

    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let unchanged = task(&board, "role-context");
    assert_eq!(unchanged["executor_name"], "Alice");
    assert_eq!(unchanged["reviewer_name"], "Bob");
    assert_eq!(unchanged["delivery_condition"], "审查通过");
    assert_eq!(unchanged["test_condition"], "公开入口断言");
}

#[test]
fn pinned_executor_and_reviewer_complete_a_read_only_task_through_public_cli() {
    let fixture = setup_roles();
    fixture.run_public(
        &[
            "board",
            "policy",
            "--scope",
            "project",
            "--work-kind",
            "frontend",
            "--executor",
            "Alice",
            "--reviewer",
            "Bob",
            "--prefer-role",
            "designer",
        ],
        THREAD_A,
    );
    publish(&fixture, "role-delivery", &["--work-kind", "frontend"]);
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let pending = task(&board, "role-delivery");
    assert_eq!(pending["executor_name"], "Alice");
    assert_eq!(pending["reviewer_name"], "Bob");
    rejected(
        &fixture,
        &[
            "board",
            "claim",
            "role-delivery",
            "--expected-revision",
            "1",
        ],
        THREAD_C,
    );
    rejected(
        &fixture,
        &[
            "board",
            "invite",
            "role-delivery",
            "--to",
            "Bob",
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "board",
            "claim",
            "role-delivery",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            "role-delivery",
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "task",
            "deliver",
            "role-delivery",
            "--evidence",
            "设计稿与公开验收通过",
            "--no-candidate",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    rejected(
        &fixture,
        &[
            "task",
            "review",
            "role-delivery",
            "--accept",
            "--evidence",
            "作者不能代审",
        ],
        THREAD_B,
    );
    rejected(
        &fixture,
        &[
            "task",
            "review",
            "role-delivery",
            "--accept",
            "--evidence",
            "master不能代审",
        ],
        THREAD_A,
    );
    rejected(&fixture, &["task", "close", "role-delivery"], THREAD_B);
    let reviewed = fixture.run_public(
        &[
            "task",
            "review",
            "role-delivery",
            "--accept",
            "--evidence",
            "Bob完成审查",
        ],
        THREAD_C,
    );
    assert_eq!(reviewed["status"], "accepted");
    assert_eq!(reviewed["disposition"], "no_merge_required");
    fixture.run_public(&["task", "close", "role-delivery"], THREAD_B);
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    assert!(!board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == "role-delivery"));
    let history = fixture.run_public(&["task", "status", "role-delivery"], THREAD_A);
    assert_eq!(history["status"], "closed");
    assert_eq!(history["delivery_complete"], true);
}

#[test]
fn role_preference_goal_override_and_missing_pinned_executor_stay_explicit() {
    let mut fixture = setup_roles();
    fixture.run_public(
        &[
            "board",
            "policy",
            "--scope",
            "project",
            "--work-kind",
            "frontend",
            "--prefer-role",
            "designer",
            "--reviewer",
            "Bob",
        ],
        THREAD_A,
    );
    publish(&fixture, "role-goal", &["--work-kind", "frontend"]);
    fixture.run_public(
        &[
            "board",
            "policy",
            "--scope",
            "goal",
            "--id",
            "role-goal",
            "--work-kind",
            "frontend",
            "--executor",
            "Bob",
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    publish(
        &fixture,
        "role-child",
        &[
            "--work-kind",
            "frontend",
            "--parent-task",
            "role-goal",
            "--executor",
            "Alice",
        ],
    );
    fixture.run_public(
        &[
            "board",
            "policy",
            "--scope",
            "project",
            "--work-kind",
            "frontend",
            "--reviewer",
            "Alice",
        ],
        THREAD_A,
    );
    fixture.restart_daemon();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    assert_eq!(task(&board, "role-child")["executor_name"], "Alice");
    assert_eq!(task(&board, "role-child")["reviewer_name"], "Bob");
    publish(&fixture, "role-preferred", &["--prefer-role", "designer"]);
    fixture.run_public(
        &[
            "board",
            "dispatch",
            "role-preferred",
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    assert_eq!(
        task(&board, "role-preferred")["dispatch_selection"],
        "Alice"
    );
    publish(&fixture, "role-no-match", &["--prefer-role", "translator"]);
    fixture.run_public(
        &[
            "board",
            "dispatch",
            "role-no-match",
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    assert!(!task(&board, "role-no-match")["dispatch_reason"]
        .as_str()
        .unwrap()
        .is_empty());
    publish(&fixture, "role-missing", &["--executor", "MissingDesigner"]);
    let response = fixture.command_public(
        &[
            "board",
            "dispatch",
            "role-missing",
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    let body = format!(
        "{} {}",
        String::from_utf8_lossy(&response.stdout),
        String::from_utf8_lossy(&response.stderr)
    );
    assert!(
        body.contains("waiting") || body.contains("unavailable"),
        "{body}"
    );
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    assert_eq!(task(&board, "role-missing")["status"], "pending");
}

#[test]
fn rework_contract_changes_and_manual_cleanup_do_not_reuse_review_acceptance() {
    let fixture = setup_roles();
    publish(
        &fixture,
        "review-changes",
        &["--executor", "Alice", "--reviewer", "Bob"],
    );
    fixture.run_public(
        &[
            "board",
            "claim",
            "review-changes",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            "review-changes",
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "task",
            "deliver",
            "review-changes",
            "--no-candidate",
            "--evidence",
            "first report",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "task",
            "review",
            "review-changes",
            "--rework",
            "--evidence",
            "fix the findings",
        ],
        THREAD_C,
    );
    rejected(
        &fixture,
        &["task", "update", "review-changes", "--status", "accepted"],
        THREAD_B,
    );
    let rework_board = fixture.run_public(&["board", "show"], THREAD_B);
    let rework_revision = task(&rework_board, "review-changes")["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    fixture.run_public(
        &[
            "board",
            "start",
            "review-changes",
            "--expected-revision",
            &rework_revision,
            "--attempt",
            "2",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "task",
            "deliver",
            "review-changes",
            "--no-candidate",
            "--evidence",
            "corrected report",
            "--attempt",
            "2",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "task",
            "review",
            "review-changes",
            "--accept",
            "--evidence",
            "corrected report passed",
        ],
        THREAD_C,
    );
    let accepted = fixture.run_public(&["task", "status", "review-changes"], THREAD_B);
    assert_eq!(accepted["review"]["valid"], true);
    fixture.run_public(
        &[
            "task",
            "update",
            "review-changes",
            "--next",
            "prepare final delivery",
        ],
        THREAD_B,
    );
    let unchanged = fixture.run_public(&["task", "status", "review-changes"], THREAD_B);
    assert_eq!(
        unchanged["review"]["subject_revision"],
        accepted["review"]["subject_revision"]
    );
    assert_eq!(unchanged["review"]["valid"], true);
    let board = fixture.run_public(&["board", "show"], THREAD_B);
    let revision = task(&board, "review-changes")["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    fixture.run_public(
        &[
            "board",
            "describe",
            "review-changes",
            "--expected-revision",
            &revision,
            "--title",
            "前端页面",
            "--description",
            "角色分工公开消费者",
            "--delivery-condition",
            "新验收条件",
            "--test-condition",
            "公开入口断言",
        ],
        THREAD_B,
    );
    let changed = fixture.run_public(&["task", "status", "review-changes"], THREAD_B);
    assert_eq!(changed["status"], "rework");
    assert_eq!(changed["review"]["valid"], false);
    rejected(&fixture, &["task", "close", "review-changes"], THREAD_B);
    let stopped = fixture.run_public(
        &[
            "task",
            "close",
            "review-changes",
            "--force",
            "--reason",
            "user cancelled this report",
        ],
        THREAD_A,
    );
    assert_eq!(stopped["delivery_complete"], false);
    let cleaned = fixture.run_public(&["task", "finalize-cleanup", "review-changes"], THREAD_A);
    assert_eq!(cleaned["delivery_complete"], false);
    let repeated = fixture.run_public(&["task", "finalize-cleanup", "review-changes"], THREAD_A);
    assert_eq!(repeated["delivery_complete"], false);
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    assert!(!board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == "review-changes"));
    let history = fixture.run_public(&["task", "status", "review-changes"], THREAD_A);
    assert_eq!(history["delivery_complete"], false);
}

#[test]
fn publish_admits_ticket_to_shortest_slot_queue_and_rotates_on_ties() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "ticket admission fixture",
        ],
        THREAD_A,
    );

    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let mut peers: Vec<String> = board["workers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|worker| worker["role"] == "peer" && worker["status"] != "offline")
        .map(|worker| worker["display_name"].as_str().unwrap().to_owned())
        .collect();
    peers.sort();
    let n = peers.len();
    assert!(
        n >= 2,
        "admission needs at least two eligible slots: {board}"
    );

    for (index, peer) in peers.iter().enumerate() {
        let id = format!("ticket-empty-{index}");
        let published = publish(&fixture, &id, &[]);
        assert_eq!(published["ticket"]["phases"][0]["phase"], "execute");
        assert_eq!(published["ticket"]["phases"][0]["status"], "queued");
        assert_eq!(
            published["ticket"]["phases"][0]["slot"], *peer,
            "each empty slot must be chosen before any one-item queue: {published}"
        );
    }

    let tied = publish(&fixture, "ticket-rotate", &[]);
    assert_eq!(
        tied["ticket"]["phases"][0]["slot"], peers[0],
        "equal queue lengths must rotate past the last selected slot: {tied}"
    );

    let persisted = fixture.current_state();
    let phases = persisted["ticket_phases"]
        .as_array()
        .expect("phase records persist");
    assert_eq!(phases.len(), n + 1);
    assert_eq!(
        persisted["ticket_rotation"]["execute:generic"], peers[0],
        "{persisted}"
    );
}

#[test]
fn board_claim_advances_the_admitted_execute_phase_once() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "ticket admission fixture",
        ],
        THREAD_A,
    );

    let published = publish(&fixture, "ticket-claim", &[]);
    let slot = published["ticket"]["phases"][0]["slot"]
        .as_str()
        .unwrap()
        .to_owned();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let worker_id = board["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["display_name"] == slot)
        .map(|worker| worker["id"].as_str().unwrap().to_owned())
        .expect("admitted slot must be one of the registered peers");
    let thread = [THREAD_B, THREAD_C]
        .iter()
        .find(|thread| {
            fixture.run_public(&["context"], thread)["identity"]["worker_id"] == worker_id
        })
        .copied()
        .expect("admitted slot must map to one of the registered peer threads");
    let claimed = fixture.run_public(
        &["board", "claim", "ticket-claim", "--expected-revision", "1"],
        thread,
    );
    assert_eq!(claimed["task"]["status"], "working");
    assert_eq!(claimed["task"]["revision"], 2);

    let persisted = fixture.current_state();
    let phase = persisted["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["task_id"] == "ticket-claim")
        .unwrap();
    assert_eq!(phase["phase"], "execute");
    assert_eq!(phase["status"], "claimed");
    assert_eq!(phase["slot_name"], slot.as_str());
    assert_eq!(phase["attempt_generation"], 1);
}

#[test]
fn ticket_start_and_result_require_the_current_claimed_attempt() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    fixture.run_public(
        &["master", "promote", "--approval", "ticket start fixture"],
        THREAD_A,
    );

    let published = publish(&fixture, "ticket-lifecycle", &[]);
    let slot = published["ticket"]["phases"][0]["slot"]
        .as_str()
        .unwrap()
        .to_owned();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let worker_id = board["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["display_name"] == slot)
        .map(|worker| worker["id"].as_str().unwrap().to_owned())
        .expect("admitted slot must be one of the registered peers");
    let thread = [THREAD_B, THREAD_C]
        .iter()
        .find(|thread| {
            fixture.run_public(&["context"], thread)["identity"]["worker_id"] == worker_id
        })
        .copied()
        .expect("admitted slot must map to one of the registered peer threads");
    let other_thread = [THREAD_B, THREAD_C]
        .into_iter()
        .find(|candidate| *candidate != thread)
        .expect("fixture must retain a non-owner peer");

    let claimed = fixture.run_public(
        &[
            "board",
            "claim",
            "ticket-lifecycle",
            "--expected-revision",
            "1",
        ],
        thread,
    );
    assert_eq!(claimed["task"]["status"], "working");
    assert_eq!(claimed["task"]["revision"], 2);

    // Only the claim owner may start the claimed ticket.
    let not_owner = rejected(
        &fixture,
        &[
            "board",
            "start",
            "ticket-lifecycle",
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        other_thread,
    );
    assert!(
        not_owner.contains("TICKET_CLAIM_OWNER_MISMATCH"),
        "{not_owner}"
    );

    // Result before Started is rejected and leaves the phase claimed.
    let premature = rejected(
        &fixture,
        &[
            "task",
            "deliver",
            "ticket-lifecycle",
            "--evidence",
            "premature evidence",
            "--attempt",
            "1",
            "--no-candidate",
        ],
        thread,
    );
    assert!(
        premature.contains("TICKET_PHASE_NOT_RUNNING"),
        "{premature}"
    );

    // Admitted ticket results must carry the claimed attempt.
    let without_attempt = rejected(
        &fixture,
        &[
            "task",
            "deliver",
            "ticket-lifecycle",
            "--evidence",
            "missing attempt",
            "--no-candidate",
        ],
        thread,
    );
    assert!(
        without_attempt.contains("TICKET_ATTEMPT_REQUIRED"),
        "{without_attempt}"
    );

    let after_rejections = fixture.current_state();
    let phase = after_rejections["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["task_id"] == "ticket-lifecycle")
        .unwrap();
    assert_eq!(phase["status"], "claimed");
    assert_eq!(phase["attempt_generation"], 1);

    let started = fixture.run_public(
        &[
            "board",
            "start",
            "ticket-lifecycle",
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        thread,
    );
    assert_eq!(started["task"]["status"], "working");
    let phase_after_start = fixture.current_state()["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["task_id"] == "ticket-lifecycle")
        .unwrap()
        .clone();
    assert_eq!(phase_after_start["status"], "running");

    // A stale attempt on a running phase is rejected and leaves it running.
    let stale = rejected(
        &fixture,
        &[
            "task",
            "deliver",
            "ticket-lifecycle",
            "--evidence",
            "stale attempt",
            "--attempt",
            "9",
            "--no-candidate",
        ],
        thread,
    );
    assert!(stale.contains("TICKET_STALE_ATTEMPT"), "{stale}");
    let phase_after_stale = fixture.current_state()["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["task_id"] == "ticket-lifecycle")
        .unwrap()
        .clone();
    assert_eq!(phase_after_stale["status"], "running");

    let delivered = fixture.run_public(
        &[
            "task",
            "deliver",
            "ticket-lifecycle",
            "--evidence",
            "real evidence",
            "--attempt",
            "1",
            "--no-candidate",
        ],
        thread,
    );
    assert_eq!(delivered["status"], "delivered");
    let phase_after_delivery = fixture.current_state()["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["task_id"] == "ticket-lifecycle")
        .unwrap()
        .clone();
    assert_eq!(phase_after_delivery["status"], "completed");

    // Repeated delivery must not complete the same phase twice.
    let repeat = rejected(
        &fixture,
        &[
            "task",
            "deliver",
            "ticket-lifecycle",
            "--evidence",
            "repeat evidence",
            "--attempt",
            "1",
            "--no-candidate",
        ],
        thread,
    );
    assert!(
        repeat.contains("TICKET_RESULT_ALREADY_COMMITTED")
            || repeat.contains("must be owned and working"),
        "{repeat}"
    );
    let phase_after_repeat = fixture.current_state()["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["task_id"] == "ticket-lifecycle")
        .unwrap()
        .clone();
    assert_eq!(phase_after_repeat["status"], "completed");
}

#[test]
fn legacy_task_register_delivery_still_succeeds_without_attempt() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["task", "register", "legacy-register"], THREAD_A);
    let delivered = fixture.run_public(
        &[
            "task",
            "deliver",
            "legacy-register",
            "--evidence",
            "legacy delivery evidence",
            "--no-candidate",
        ],
        THREAD_A,
    );
    assert_eq!(delivered["status"], "delivered");
    let persisted = fixture.current_state();
    assert!(!persisted["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .any(|phase| phase["task_id"] == "legacy-register"));
}

#[test]
fn admitted_ticket_missing_execute_phase_is_rejected_not_treated_as_legacy() {
    let mut fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "ticket missing phase fixture",
        ],
        THREAD_A,
    );

    let published = publish(&fixture, "ticket-missing-phase", &[]);
    let slot = published["ticket"]["phases"][0]["slot"]
        .as_str()
        .unwrap()
        .to_owned();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let worker_id = board["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["display_name"] == slot)
        .map(|worker| worker["id"].as_str().unwrap().to_owned())
        .expect("admitted slot must be one of the registered peers");
    let thread = [THREAD_B, THREAD_C]
        .iter()
        .find(|thread| {
            fixture.run_public(&["context"], thread)["identity"]["worker_id"] == worker_id
        })
        .copied()
        .expect("admitted slot must map to one of the registered peer threads");
    fixture.run_public(
        &[
            "board",
            "claim",
            "ticket-missing-phase",
            "--expected-revision",
            "1",
        ],
        thread,
    );
    let state = fixture.current_state();
    assert_eq!(
        state["board_details"]["ticket-missing-phase"]["admitted"],
        true
    );
    fixture.update_current_state(|state| {
        let phases = state["ticket_phases"]
            .as_array_mut()
            .expect("phase records persist");
        phases.retain(|phase| phase["task_id"] != "ticket-missing-phase");
    });

    let rejected = rejected(
        &fixture,
        &[
            "task",
            "deliver",
            "ticket-missing-phase",
            "--evidence",
            "missing phase",
            "--attempt",
            "1",
            "--no-candidate",
        ],
        thread,
    );
    assert!(rejected.contains("TICKET_PHASE_MISSING"), "{rejected}");
}

fn seed_idle(fixture: &mut AppFixture, worker_id: &str, thread: &str) {
    let project = std::fs::canonicalize(&fixture.project)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let worker = worker_id.to_owned();
    let session = thread.to_owned();
    let scope = project.clone();
    fixture.update_current_state(move |state| {
        state["agent_presence"][scope.clone()][worker.clone()] = json!({
            "project_scope": scope.clone(),
            "worker_id": worker.clone(),
            "session_id": session.clone(),
            "presence": "online",
            "reason": "fixture",
            "observed_ms": 1
        });
        state["agent_activity"][scope.clone()][worker.clone()] = json!({
            "project_scope": scope,
            "worker_id": worker,
            "session_id": session,
            "activity": "idle",
            "background": false,
            "reason": "fixture",
            "observed_ms": 1,
            "idle_since_ms": 1
        });
    });
}

fn slot_worker_id(board: &Value, slot: &str) -> String {
    board["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["display_name"] == slot)
        .and_then(|worker| worker["id"].as_str())
        .expect("slot must be a registered worker")
        .to_owned()
}

fn ticket_thread(fixture: &AppFixture, worker_id: &str) -> &'static str {
    [THREAD_B, THREAD_C]
        .into_iter()
        .find(|thread| {
            fixture.run_public(&["context"], thread)["identity"]["worker_id"] == worker_id
        })
        .expect("slot must map to a fixture thread")
}

#[test]
fn idle_continuation_skips_dependency_blocked_priority_and_releases_on_completion() {
    let mut fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "continuation dependency fixture",
        ],
        THREAD_A,
    );

    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let mut peers: Vec<String> = board["workers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|worker| worker["role"] == "peer" && worker["status"] != "offline")
        .map(|worker| worker["display_name"].as_str().unwrap().to_owned())
        .collect();
    peers.sort();
    let (slot, helper) = (peers[0].clone(), peers[1].clone());
    let slot_thread = ticket_thread(&fixture, &slot_worker_id(&board, &slot));
    let helper_thread = ticket_thread(&fixture, &slot_worker_id(&board, &helper));
    seed_idle(&mut fixture, &slot_worker_id(&board, &slot), slot_thread);

    let dep = publish(&fixture, "ticket-dep", &["--executor", &helper]);
    assert_eq!(dep["ticket"]["phases"][0]["status"], "queued");

    let high = publish(
        &fixture,
        "ticket-high",
        &[
            "--executor",
            &slot,
            "--priority",
            "p0",
            "--depends-on",
            "ticket-dep",
        ],
    );
    assert_eq!(high["ticket"]["phases"][0]["status"], "queued");
    assert_eq!(
        high["ticket"]["phases"][0]["depends_on"],
        json!(["ticket-dep"])
    );

    let low = publish(
        &fixture,
        "ticket-low",
        &["--executor", &slot, "--priority", "p2"],
    );
    assert_eq!(
        low["ticket"]["phases"][0]["status"], "claimed",
        "the ready p2 ticket must claim an idle slot before a blocked p0"
    );
    assert_eq!(low["task"]["status"], "working");

    let low_revision = low["task"]["revision"].as_u64().unwrap().to_string();
    fixture.run_public(
        &[
            "board",
            "start",
            "ticket-low",
            "--expected-revision",
            &low_revision,
            "--attempt",
            "1",
        ],
        slot_thread,
    );
    fixture.run_public(
        &[
            "task",
            "deliver",
            "ticket-low",
            "--evidence",
            "ready work",
            "--attempt",
            "1",
            "--no-candidate",
        ],
        slot_thread,
    );
    let state = fixture.current_state();
    let phase = |id: &str| {
        state["ticket_phases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|phase| phase["task_id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(phase("ticket-low")["status"], "completed");
    assert_eq!(
        phase("ticket-high")["status"],
        "queued",
        "the blocked p0 must keep waiting after the ready ticket completes"
    );

    let dep_claimed = fixture.run_public(
        &["board", "claim", "ticket-dep", "--expected-revision", "1"],
        helper_thread,
    );
    let dep_revision = dep_claimed["task"]["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    fixture.run_public(
        &[
            "board",
            "start",
            "ticket-dep",
            "--expected-revision",
            &dep_revision,
            "--attempt",
            "1",
        ],
        helper_thread,
    );
    fixture.run_public(
        &[
            "task",
            "deliver",
            "ticket-dep",
            "--evidence",
            "dependency complete",
            "--attempt",
            "1",
            "--no-candidate",
        ],
        helper_thread,
    );

    let state = fixture.current_state();
    let high = state["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["task_id"] == "ticket-high")
        .unwrap()
        .clone();
    assert_eq!(
        high["status"], "claimed",
        "a completed dependency must release another slot's idle queue: {state}"
    );
    assert_eq!(high["slot_name"], slot);
    assert_eq!(high["attempt_generation"], 1);
}

#[test]
fn idle_continuation_claims_fifo_within_same_priority_after_release() {
    let mut fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "continuation fifo fixture",
        ],
        THREAD_A,
    );

    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let mut peers: Vec<String> = board["workers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|worker| worker["role"] == "peer" && worker["status"] != "offline")
        .map(|worker| worker["display_name"].as_str().unwrap().to_owned())
        .collect();
    peers.sort();
    let slot = peers[0].clone();
    let slot_thread = ticket_thread(&fixture, &slot_worker_id(&board, &slot));
    seed_idle(&mut fixture, &slot_worker_id(&board, &slot), slot_thread);

    let first = publish(&fixture, "ticket-fifo-first", &["--executor", &slot]);
    assert_eq!(first["ticket"]["phases"][0]["status"], "claimed");
    let second = publish(&fixture, "ticket-fifo-second", &["--executor", &slot]);
    assert_eq!(second["ticket"]["phases"][0]["status"], "queued");

    let first_revision = first["task"]["revision"].as_u64().unwrap().to_string();
    fixture.run_public(
        &[
            "board",
            "start",
            "ticket-fifo-first",
            "--expected-revision",
            &first_revision,
            "--attempt",
            "1",
        ],
        slot_thread,
    );
    fixture.run_public(
        &[
            "task",
            "deliver",
            "ticket-fifo-first",
            "--evidence",
            "first",
            "--attempt",
            "1",
            "--no-candidate",
        ],
        slot_thread,
    );

    let state = fixture.current_state();
    let second_phase = state["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["task_id"] == "ticket-fifo-second")
        .unwrap()
        .clone();
    assert_eq!(
        second_phase["status"], "claimed",
        "same-priority FIFO must claim the earlier ticket on release"
    );
    assert_eq!(second_phase["attempt_generation"], 1);
}

#[test]
fn concurrent_claims_leave_one_effective_execute_claim() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "concurrent claim fixture",
        ],
        THREAD_A,
    );

    let published = publish(&fixture, "ticket-race", &[]);
    let slot = published["ticket"]["phases"][0]["slot"]
        .as_str()
        .unwrap()
        .to_owned();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let owner_thread = ticket_thread(&fixture, &slot_worker_id(&board, &slot));
    let other_thread = [THREAD_B, THREAD_C]
        .into_iter()
        .find(|thread| *thread != owner_thread)
        .expect("fixture must retain a second peer thread");

    let mut owner = fixture.spawn_public(
        &["board", "claim", "ticket-race", "--expected-revision", "1"],
        owner_thread,
    );
    let mut other = fixture.spawn_public(
        &["board", "claim", "ticket-race", "--expected-revision", "1"],
        other_thread,
    );
    let owner = owner.wait_with_output().unwrap();
    let other = other.wait_with_output().unwrap();
    assert!(
        owner.status.success(),
        "the queued slot must win its own claim: {}",
        String::from_utf8_lossy(&owner.stderr)
    );
    assert!(
        !other.status.success(),
        "a different slot must not claim another slot's queued ticket"
    );
    // Two slots race one pending ticket. The daemon serializes claims on the
    // state lock, so the loser is rejected either by the slot gate (it ran
    // first) or by the revision guard (the winner committed first). Both are
    // correct refusals; only the invariant below may be asserted.
    let other_error = format!(
        "{}{}",
        String::from_utf8_lossy(&other.stdout),
        String::from_utf8_lossy(&other.stderr)
    );
    assert!(
        other_error.contains("TICKET_SLOT_MISMATCH") || other_error.contains("BOARD_STALE_REVISION"),
        "the losing claim must be refused by the slot gate or the revision guard, got: {other_error}"
    );

    let state = fixture.current_state();
    let claimed: Vec<&Value> = state["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|phase| phase["task_id"] == "ticket-race" && phase["status"] == "claimed")
        .collect();
    assert_eq!(claimed.len(), 1, "only one execute phase may hold a claim");
    assert_eq!(claimed[0]["slot_name"], slot.as_str());
    assert_eq!(claimed[0]["attempt_generation"], 1);
}

#[test]
fn claim_from_a_non_queued_slot_is_rejected_by_the_slot_gate() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "non-queued slot claim fixture",
        ],
        THREAD_A,
    );

    let published = publish(&fixture, "ticket-slot-gate", &[]);
    let slot = published["ticket"]["phases"][0]["slot"]
        .as_str()
        .unwrap()
        .to_owned();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let owner_thread = ticket_thread(&fixture, &slot_worker_id(&board, &slot));
    let other_thread = [THREAD_B, THREAD_C]
        .into_iter()
        .find(|thread| *thread != owner_thread)
        .expect("fixture must retain a second peer thread");

    // A peer that does not hold the queued slot claims with the still-current
    // revision. There is no race here, so the slot gate must be the refusal.
    let rejected = fixture.command_public(
        &[
            "board",
            "claim",
            "ticket-slot-gate",
            "--expected-revision",
            "1",
        ],
        other_thread,
    );
    assert!(
        !rejected.status.success(),
        "a non-queued slot must not claim another slot's queued ticket"
    );
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("TICKET_SLOT_MISMATCH")
            || String::from_utf8_lossy(&rejected.stdout).contains("TICKET_SLOT_MISMATCH"),
        "the slot gate must refuse a non-queued slot: {}",
        String::from_utf8_lossy(&rejected.stderr)
    );

    let state = fixture.current_state();
    let phase = state["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["task_id"] == "ticket-slot-gate")
        .unwrap();
    assert_eq!(phase["status"], "queued");
    assert_eq!(phase["slot_name"], slot.as_str());
    assert_eq!(phase["attempt_generation"], 0);
    assert_eq!(state["tasks"]["ticket-slot-gate"]["status"], "pending");
}
