use super::*;

const THREAD_C: &str = "01a0d641-13f1-7b31-8d84-530f0f2d3fdd";

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
            "task",
            "deliver",
            "named-review",
            "--no-candidate",
            "--evidence",
            "report ready",
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
    fixture.append_name_events(&[
        json!({"ev":"BoardDetailsChanged","task_id":"named-review","details":details}),
    ]);
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
fn roles_registration_master_update_clear_and_replay_are_public() {
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
    let journal = fixture.name_journal();
    fixture.run_public(&["reset", "--project"], THREAD_A);
    let retained = std::fs::read_to_string(journal).unwrap_or_default();
    assert!(!retained.contains("NameWorkRolesUpdated"));
    assert!(!retained.contains("ProjectWorkPolicyUpdated"));
    assert!(
        fixture.host_state.join("server.sock").exists(),
        "project reset preserves the host daemon"
    );
}

fn joined_role_text(values: &[&str]) -> String {
    values.join(" ").to_lowercase()
}

fn responsibility_text(snapshot: &Value) -> String {
    joined_role_text(
        &snapshot["operations"]
            .as_array()
            .expect("context operations")
            .iter()
            .filter(|operation| operation["kind"] == "responsibility")
            .filter_map(|operation| operation["action"].as_str())
            .collect::<Vec<_>>(),
    )
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
        for expected in [
            "analyze the requirement before assigning work",
            "reuse items the user already specified or confirmed",
            "missing or ambiguous scope",
            "role requirements",
            "collab context --work-roles",
            "collab board policy",
            "accept or execute a concrete task",
            "identity recovery, work-role settings, discussion and status questions do not require creating a board task",
            "latest `collab context` snapshot",
            "current board task",
            "role preference is a selection hint, not execution authorization",
            "existing board claim or invite acceptance flow",
            "specified executor is a hard constraint",
            "the author and master cannot substitute",
            "dynamic role change does not rewrite existing task",
            "state the difference and obtain explicit user confirmation",
            "continue under the confirmed requirements without asking again",
            "read the board only when a specific task or detail is missing",
        ] {
            assert!(
                responsibilities.contains(expected),
                "{label} responsibilities miss `{expected}`: {responsibilities}"
            );
        }
        let next_action = brief["next_action"]
            .as_str()
            .expect("role next_action")
            .to_lowercase();
        for expected in [
            "role label is not authorization",
            "existing claim or invite acceptance flow",
            "require the specified reviewer",
            "dynamic role change does not rewrite existing tasks",
            "explicit confirmation",
        ] {
            assert!(
                next_action.contains(expected),
                "{label} next_action misses `{expected}`: {next_action}"
            );
        }
        let operations = responsibility_text(snapshot);
        assert!(
            operations.contains("collab context --work-roles")
                && operations.contains("current board task"),
            "{label} operations must derive the shared work-role rules: {operations}"
        );
        assert!(
            snapshot["operations"]
                .as_array()
                .expect("context operations")
                .iter()
                .any(|operation| operation["kind"] == "next_action"
                    && operation["action"]
                        .as_str()
                        .is_some_and(|action| action.contains("role label is not authorization"))),
            "{label} operations must derive the work-role next action: {snapshot}"
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
            "dispatch",
            "role-delivery",
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    let board = fixture.run_public(&["board", "show"], THREAD_B);
    let revision = task(&board, "role-delivery")["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    fixture.run_public(
        &[
            "board",
            "respond",
            "role-delivery",
            "--accept",
            "--expected-revision",
            &revision,
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
    assert_eq!(task(&board, "role-delivery")["status"], "closed");
    assert_eq!(
        task(&board, "role-delivery")["completion_delivery_complete"],
        true
    );
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
            "task",
            "deliver",
            "review-changes",
            "--no-candidate",
            "--evidence",
            "first report",
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
    fixture.run_public(
        &[
            "task",
            "deliver",
            "review-changes",
            "--no-candidate",
            "--evidence",
            "corrected report",
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
    assert_eq!(
        task(&board, "review-changes")["completion_delivery_complete"],
        false
    );
}
