use super::*;

const THREAD_C: &str = "01a0d641-13f1-7b31-8d84-530f0f2d3fdd";
const THREAD_HANDOFF: &str = "01a0d641-13f1-7b31-8d84-530f0f2d3fde";

fn transport_refreshed(fixture: &AppFixture, session: &str, thread: &str) -> Value {
    public_snapshot_with_session(fixture, &["context"], session, thread)
}

fn public_snapshot_with_session(
    fixture: &AppFixture,
    args: &[&str],
    session: &str,
    thread: &str,
) -> Value {
    let output = fixture.command_public_with_session(args, session, thread);
    assert!(
        output.status.success(),
        "public command {:?} failed: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("public response is JSON");
    value
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .cloned()
        .unwrap_or(value)
}

fn raw_public_response(fixture: &AppFixture, request: &Value) -> Value {
    let mut stream = UnixStream::connect(fixture.host_state.join("server.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let line = serde_json::to_string(request).unwrap();
    std::io::Write::write_all(&mut stream, line.as_bytes()).unwrap();
    std::io::Write::write_all(&mut stream, b"\n").unwrap();
    let mut reader = std::io::BufReader::new(stream);
    let mut response = String::new();
    std::io::BufRead::read_line(&mut reader, &mut response).unwrap();
    serde_json::from_str(&response).unwrap()
}

fn active_slot(state: &Value, worker_id: &str) -> Value {
    state["slots"]
        .as_array()
        .unwrap_or_else(|| panic!("state has no binding slots: {state}"))
        .iter()
        .find(|slot| {
            slot["identity_status"] == "active"
                && slot["current"]["binding"]["agent_id"] == worker_id
        })
        .unwrap_or_else(|| panic!("worker has no active slot: {state}"))
        .clone()
}

fn identity_map(fixture: &AppFixture) -> Value {
    serde_json::from_slice(&std::fs::read(fixture.host_state.join("identity-map.json")).unwrap())
        .unwrap()
}

fn slot_generation(fixture: &AppFixture, worker_id: &str) -> u64 {
    let state = identity_map(fixture);
    active_slot(&state, worker_id)["generation"]
        .as_u64()
        .expect("active slot has a generation")
}

// Move a slot's generation forward without going through the bind/rebind effect
// path. This reproduces the state a pre-recovery daemon left behind: the
// identity generation advanced while an in-flight ticket phase stayed at the
// older generation with no pending control effect left to replay.
fn set_slot_generation(fixture: &AppFixture, worker_id: &str, generation: u64) {
    let mut map = identity_map(fixture);
    let slot = map["slots"]
        .as_array_mut()
        .expect("identity map slots")
        .iter_mut()
        .find(|slot| {
            slot["identity_status"] == "active"
                && slot["current"]["binding"]["agent_id"] == worker_id
        })
        .expect("worker must hold an active slot");
    let previous = slot["generation"].as_u64().expect("slot generation");
    assert!(
        generation > previous,
        "simulated drift must advance the generation: previous={previous} next={generation}"
    );
    slot["generation"] = json!(generation);
    std::fs::write(
        fixture.host_state.join("identity-map.json"),
        serde_json::to_vec(&map).expect("identity map JSON"),
    )
    .expect("persist simulated legacy generation drift");
}

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
        publish(
            &fixture,
            "first",
            &[
                "--long-horizon-content",
                "Complete the first task under its current board contract",
                "--long-horizon-every-ms",
                "3600000",
            ],
        );
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
        publish(
            &fixture,
            "preferred",
            &[
                "--prefer-role",
                "designer",
                "--long-horizon-content",
                "Complete the role-preferred task under its current board contract",
                "--long-horizon-every-ms",
                "3600000",
            ],
        );
        let preferred = fixture.run_public(
            &["board", "dispatch", "preferred", "--expected-revision", "1"],
            THREAD_A,
        );
        assert_eq!(
            preferred["task"]["invited_peer"],
            alice["identity"]["worker_id"]
        );
        // Keep the first task's ownership/invitation for selection while
        // releasing its finite goal slot before another explicit override.
        fixture.run_public(
            &[
                "goal",
                "reconcile",
                "--goal-id",
                "task:first",
                "--owner",
                "Alice",
                "--desired",
                "ended",
            ],
            THREAD_A,
        );
        publish(
            &fixture,
            "pinned",
            &[
                "--executor",
                "Alice",
                "--long-horizon-content",
                "Complete the pinned task under its current board contract",
                "--long-horizon-every-ms",
                "3600000",
            ],
        );
        let pinned = fixture.run_public(
            &["board", "dispatch", "pinned", "--expected-revision", "1"],
            THREAD_A,
        );
        assert_eq!(
            pinned["task"]["invited_peer"],
            alice["identity"]["worker_id"]
        );
        publish(
            &fixture,
            "second",
            &[
                "--long-horizon-content",
                "Complete the second task under its current board contract",
                "--long-horizon-every-ms",
                "3600000",
            ],
        );
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
    publish(
        &fixture,
        "role-preferred",
        &[
            "--prefer-role",
            "designer",
            "--long-horizon-content",
            "Complete the designer task under its current board contract",
            "--long-horizon-every-ms",
            "3600000",
        ],
    );
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
    publish(
        &fixture,
        "role-no-match",
        &[
            "--prefer-role",
            "translator",
            "--long-horizon-content",
            "Complete the translator task under its current board contract",
            "--long-horizon-every-ms",
            "3600000",
        ],
    );
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
    publish(
        &fixture,
        "role-missing",
        &[
            "--executor",
            "MissingDesigner",
            "--long-horizon-content",
            "Complete the explicitly pinned task under its current board contract",
            "--long-horizon-every-ms",
            "3600000",
        ],
    );
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
fn contract_change_reopens_admitted_execute_phase_for_redelivery() {
    let fixture = setup_roles();
    publish(
        &fixture,
        "contract-rework",
        &["--executor", "Alice", "--reviewer", "Bob"],
    );
    fixture.run_public(
        &[
            "board",
            "claim",
            "contract-rework",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            "contract-rework",
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
            "contract-rework",
            "--no-candidate",
            "--evidence",
            "first report",
            "--attempt",
            "1",
        ],
        THREAD_B,
    );
    let delivered = fixture.current_state();
    let phase = delivered["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["task_id"] == "contract-rework")
        .unwrap();
    assert_eq!(phase["status"], "completed");
    assert_eq!(phase["attempt_generation"], 1);

    let delivered_board = fixture.run_public(&["board", "show"], THREAD_B);
    let delivered_revision = task(&delivered_board, "contract-rework")["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    fixture.run_public(
        &[
            "board",
            "describe",
            "contract-rework",
            "--expected-revision",
            &delivered_revision,
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
    let changed = fixture.run_public(&["task", "status", "contract-rework"], THREAD_B);
    assert_eq!(changed["status"], "rework");
    assert_eq!(changed["review"]["valid"], false);

    let changed_board = fixture.run_public(&["board", "show"], THREAD_B);
    let changed_revision = task(&changed_board, "contract-rework")["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    fixture.run_public(
        &[
            "board",
            "start",
            "contract-rework",
            "--expected-revision",
            &changed_revision,
            "--attempt",
            "2",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "task",
            "deliver",
            "contract-rework",
            "--no-candidate",
            "--evidence",
            "corrected report",
            "--attempt",
            "2",
        ],
        THREAD_B,
    );
    let reviewed = fixture.run_public(
        &[
            "task",
            "review",
            "contract-rework",
            "--accept",
            "--evidence",
            "corrected report passed",
        ],
        THREAD_C,
    );
    assert_eq!(reviewed["status"], "accepted");
    assert_eq!(reviewed["disposition"], "no_merge_required");
    fixture.run_public(&["task", "close", "contract-rework"], THREAD_B);
    let history = fixture.run_public(&["task", "status", "contract-rework"], THREAD_B);
    assert_eq!(history["status"], "closed");
    assert_eq!(history["delivery_complete"], true);
}

#[test]
fn direct_rework_reopens_admitted_execute_phase_for_redelivery() {
    let fixture = setup_roles();
    publish(
        &fixture,
        "direct-rework",
        &["--executor", "Alice", "--reviewer", "Bob"],
    );
    fixture.run_public(
        &[
            "board",
            "claim",
            "direct-rework",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            "direct-rework",
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
            "direct-rework",
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
            "update",
            "direct-rework",
            "--status",
            "rework",
            "--next",
            "rework the first report",
        ],
        THREAD_B,
    );
    let changed = fixture.run_public(&["task", "status", "direct-rework"], THREAD_B);
    assert_eq!(changed["status"], "rework");

    let changed_board = fixture.run_public(&["board", "show"], THREAD_B);
    let changed_revision = task(&changed_board, "direct-rework")["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    fixture.run_public(
        &[
            "board",
            "start",
            "direct-rework",
            "--expected-revision",
            &changed_revision,
            "--attempt",
            "2",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "task",
            "deliver",
            "direct-rework",
            "--no-candidate",
            "--evidence",
            "corrected report",
            "--attempt",
            "2",
        ],
        THREAD_B,
    );
    let reviewed = fixture.run_public(
        &[
            "task",
            "review",
            "direct-rework",
            "--accept",
            "--evidence",
            "corrected report passed",
        ],
        THREAD_C,
    );
    assert_eq!(reviewed["status"], "accepted");
    assert_eq!(reviewed["disposition"], "no_merge_required");
    fixture.run_public(&["task", "close", "direct-rework"], THREAD_B);
    let history = fixture.run_public(&["task", "status", "direct-rework"], THREAD_B);
    assert_eq!(history["status"], "closed");
    assert_eq!(history["delivery_complete"], true);
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

#[test]
fn master_backfills_missing_execute_phase_and_restores_start_deliver() {
    let mut fixture = setup_roles();
    let published = publish(&fixture, "ticket-backfill", &["--executor", "Alice"]);
    let slot = published["ticket"]["phases"][0]["slot"]
        .as_str()
        .expect("admitted ticket has an execution slot")
        .to_owned();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let owner_worker = slot_worker_id(&board, &slot);
    let owner_thread = ticket_thread(&fixture, &owner_worker);

    let claimed = fixture.run_public(
        &[
            "board",
            "claim",
            "ticket-backfill",
            "--expected-revision",
            "1",
        ],
        owner_thread,
    );
    let owner = claimed["task"]["owner"]
        .as_str()
        .expect("claimed ticket has an owner")
        .to_owned();
    let original_revision = claimed["task"]["revision"]
        .as_u64()
        .expect("claimed ticket has a board revision");
    assert_eq!(claimed["task"]["owner_name"], slot);
    assert_eq!(claimed["task"]["status"], "working");

    fixture.update_current_state(|state| {
        state["ticket_phases"]
            .as_array_mut()
            .expect("phase records persist")
            .retain(|phase| phase["task_id"] != "ticket-backfill");
    });
    let after_loss = fixture.run_public(&["board", "show"], THREAD_A);
    assert_eq!(
        task(&after_loss, "ticket-backfill")["revision"],
        original_revision
    );
    let revision = original_revision.to_string();

    let red = rejected(
        &fixture,
        &[
            "board",
            "start",
            "ticket-backfill",
            "--expected-revision",
            &revision,
            "--attempt",
            "1",
        ],
        owner_thread,
    );
    assert!(red.contains("TICKET_PHASE_NOT_CLAIMED"), "{red}");
    eprintln!("backfill_red_start_error={red}");

    let non_master = rejected(
        &fixture,
        &[
            "board",
            "backfill-execute",
            "ticket-backfill",
            "--expected-revision",
            &revision,
        ],
        owner_thread,
    );
    assert!(non_master.contains("BOARD_MASTER_REQUIRED"), "{non_master}");

    let backfilled = fixture.run_public(
        &[
            "board",
            "backfill-execute",
            "ticket-backfill",
            "--expected-revision",
            &revision,
        ],
        THREAD_A,
    );
    assert_eq!(backfilled["task"]["owner"], owner);
    assert_eq!(backfilled["task"]["revision"], original_revision);
    let phases = backfilled["ticket"]["phases"]
        .as_array()
        .expect("backfill returns the ticket projection");
    assert_eq!(phases.len(), 1);
    assert_eq!(phases[0]["status"], "claimed");
    assert_eq!(phases[0]["slot"], slot);
    assert_eq!(
        phases[0]["binding_generation"],
        slot_generation(&fixture, &owner)
    );
    assert_eq!(phases[0]["attempt_generation"], 1);

    let duplicate = rejected(
        &fixture,
        &[
            "board",
            "backfill-execute",
            "ticket-backfill",
            "--expected-revision",
            &revision,
        ],
        THREAD_A,
    );
    assert!(
        duplicate.contains("TICKET_BACKFILL_PHASE_EXISTS"),
        "{duplicate}"
    );
    eprintln!("backfill_duplicate_error={duplicate}");

    let started = fixture.run_public(
        &[
            "board",
            "start",
            "ticket-backfill",
            "--expected-revision",
            &revision,
            "--attempt",
            "1",
        ],
        owner_thread,
    );
    assert_eq!(started["task"]["status"], "working");
    let delivered = fixture.run_public(
        &[
            "task",
            "deliver",
            "ticket-backfill",
            "--evidence",
            "backfill restores start and deliver",
            "--attempt",
            "1",
            "--no-candidate",
        ],
        owner_thread,
    );
    assert_eq!(delivered["status"], "delivered");
    let final_state = fixture.current_state();
    let phase = final_state["ticket_phases"]
        .as_array()
        .expect("phase records persist")
        .iter()
        .find(|phase| phase["task_id"] == "ticket-backfill")
        .expect("backfilled phase remains durable");
    assert_eq!(phase["status"], "completed");
    assert_eq!(phase["attempt_generation"], 1);
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
fn claimed_binding_handoff_reclaims_slot_after_transport_refresh() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "claimed binding handoff fixture",
        ],
        THREAD_A,
    );

    let published = publish(&fixture, "claimed-handoff", &[]);
    let slot = published["ticket"]["phases"][0]["slot"].as_str().unwrap();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let worker = slot_worker_id(&board, slot);
    let owner_thread = ticket_thread(&fixture, &worker);

    let before = fixture.run_public(&["context"], owner_thread);
    assert_eq!(before["identity"]["worker_id"], worker);
    assert_eq!(before["binding"]["native_thread_id"], owner_thread);

    let claimed = fixture.run_public(
        &[
            "board",
            "claim",
            "claimed-handoff",
            "--expected-revision",
            "1",
        ],
        owner_thread,
    );
    assert_eq!(claimed["task"]["status"], "working");
    assert_eq!(claimed["task"]["revision"], 2);

    let mut new_thread = ThreadState::new();
    new_thread.session_id = Some(owner_thread.to_owned());
    fixture
        .state
        .lock()
        .unwrap()
        .insert(THREAD_HANDOFF.into(), new_thread);

    let old_generation = slot_generation(&fixture, &worker);
    let refreshed = transport_refreshed(&fixture, owner_thread, THREAD_HANDOFF);
    let new_generation = slot_generation(&fixture, &worker);
    let revision = fixture.current_state()["board_details"]["claimed-handoff"]["revision"]
        .as_u64()
        .expect("board details revision")
        .to_string();

    assert_eq!(refreshed["identity"]["worker_id"], worker);
    assert_eq!(refreshed["binding"]["native_thread_id"], THREAD_HANDOFF);
    assert_eq!(refreshed["binding"]["session_id"], owner_thread);
    assert!(
        new_generation > old_generation,
        "transport refresh must advance binding generation: old={old_generation} new={new_generation}"
    );

    let old_start = fixture.command_public_with_session(
        &[
            "board",
            "start",
            "claimed-handoff",
            "--expected-revision",
            revision.as_str(),
            "--attempt",
            "1",
        ],
        owner_thread,
        THREAD_HANDOFF,
    );
    let old_start_error = format!(
        "{} {}",
        String::from_utf8_lossy(&old_start.stdout),
        String::from_utf8_lossy(&old_start.stderr)
    );
    assert!(
        !old_start.status.success() && old_start_error.contains("TICKET_STALE_BINDING"),
        "old attempt must be rejected as stale binding after refresh: {old_start_error}"
    );

    let output = fixture.command_public_with_session(
        &[
            "board",
            "claim",
            "claimed-handoff",
            "--expected-revision",
            revision.as_str(),
        ],
        owner_thread,
        THREAD_HANDOFF,
    );
    assert!(
        output.status.success(),
        "same-name ticket must be re-claimable after transport refresh; old start={old_start_error}; claim stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn running_binding_handoff_reclaims_slot_after_transport_refresh() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "running binding handoff fixture",
        ],
        THREAD_A,
    );

    let published = publish(&fixture, "running-handoff", &[]);
    let slot = published["ticket"]["phases"][0]["slot"].as_str().unwrap();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let worker = slot_worker_id(&board, slot);
    let owner_thread = ticket_thread(&fixture, &worker);

    let before = fixture.run_public(&["context"], owner_thread);
    assert_eq!(before["identity"]["worker_id"], worker);
    assert_eq!(before["binding"]["native_thread_id"], owner_thread);

    let claimed = fixture.run_public(
        &[
            "board",
            "claim",
            "running-handoff",
            "--expected-revision",
            "1",
        ],
        owner_thread,
    );
    assert_eq!(claimed["task"]["status"], "working");
    assert_eq!(claimed["task"]["revision"], 2);
    let started = fixture.run_public(
        &[
            "board",
            "start",
            "running-handoff",
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        owner_thread,
    );
    assert_eq!(started["task"]["status"], "working");

    let mut new_thread = ThreadState::new();
    new_thread.session_id = Some(owner_thread.to_owned());
    fixture
        .state
        .lock()
        .unwrap()
        .insert(THREAD_HANDOFF.into(), new_thread);

    let old_generation = slot_generation(&fixture, &worker);
    let refreshed = transport_refreshed(&fixture, owner_thread, THREAD_HANDOFF);
    let new_generation = slot_generation(&fixture, &worker);
    let revision = fixture.current_state()["board_details"]["running-handoff"]["revision"]
        .as_u64()
        .expect("board details revision")
        .to_string();

    assert_eq!(refreshed["identity"]["worker_id"], worker);
    assert_eq!(refreshed["binding"]["native_thread_id"], THREAD_HANDOFF);
    assert_eq!(refreshed["binding"]["session_id"], owner_thread);
    assert!(
        new_generation > old_generation,
        "transport refresh must advance binding generation: old={old_generation} new={new_generation}"
    );

    let before_rejection = fixture.current_state();
    let old_result = fixture.command_public_with_session(
        &[
            "task",
            "deliver",
            "running-handoff",
            "--evidence",
            "old result after refresh",
            "--attempt",
            "1",
            "--no-candidate",
        ],
        owner_thread,
        THREAD_HANDOFF,
    );
    let old_result_error = format!(
        "{} {}",
        String::from_utf8_lossy(&old_result.stdout),
        String::from_utf8_lossy(&old_result.stderr)
    );
    assert!(
        !old_result.status.success() && old_result_error.contains("TICKET_STALE_BINDING"),
        "old attempt must be rejected as stale binding after refresh: {old_result_error}"
    );
    let after_rejection = fixture.current_state();
    assert_eq!(
        after_rejection["tasks"]["running-handoff"], before_rejection["tasks"]["running-handoff"],
        "stale result rejection must not change the business task"
    );
    assert_eq!(
        after_rejection["board_details"]["running-handoff"],
        before_rejection["board_details"]["running-handoff"],
        "stale result rejection must not change board evidence"
    );
    assert_eq!(
        after_rejection["ticket_phases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|phase| phase["task_id"] == "running-handoff")
            .collect::<Vec<_>>(),
        before_rejection["ticket_phases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|phase| phase["task_id"] == "running-handoff")
            .collect::<Vec<_>>(),
        "stale result rejection must not change ticket phases"
    );
    assert!(
        after_rejection["task_lifecycle"]["running-handoff"].is_null(),
        "stale result rejection must not create lifecycle evidence: {after_rejection}"
    );

    let output = fixture.command_public_with_session(
        &[
            "board",
            "claim",
            "running-handoff",
            "--expected-revision",
            revision.as_str(),
        ],
        owner_thread,
        THREAD_HANDOFF,
    );
    assert!(
        output.status.success(),
        "same-name running ticket must be re-claimable after transport refresh; old result={old_result_error}; claim stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn pre_recovery_stale_running_attempt_reclaims_from_public_claim() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "pre-recovery stale running fixture",
        ],
        THREAD_A,
    );

    let published = publish(&fixture, "legacy-stale-running", &[]);
    let slot = published["ticket"]["phases"][0]["slot"].as_str().unwrap();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let worker = slot_worker_id(&board, slot);
    let owner_thread = ticket_thread(&fixture, &worker);

    fixture.run_public(
        &[
            "board",
            "claim",
            "legacy-stale-running",
            "--expected-revision",
            "1",
        ],
        owner_thread,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            "legacy-stale-running",
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        owner_thread,
    );

    let old_generation = slot_generation(&fixture, &worker);
    set_slot_generation(&fixture, &worker, old_generation + 1);

    let drifted = fixture.current_state();
    let phase = drifted["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["task_id"] == "legacy-stale-running")
        .unwrap();
    assert_eq!(phase["status"], "running");
    assert_eq!(phase["binding_generation"], old_generation);

    let stale_error = rejected(
        &fixture,
        &[
            "board",
            "start",
            "legacy-stale-running",
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        owner_thread,
    );
    assert!(
        stale_error.contains("TICKET_STALE_BINDING"),
        "{stale_error}"
    );

    let claimed = fixture.run_public(
        &[
            "board",
            "claim",
            "legacy-stale-running",
            "--expected-revision",
            "2",
        ],
        owner_thread,
    );
    assert_eq!(claimed["task"]["status"], "working");
    assert_eq!(claimed["task"]["revision"], 3);

    let recovered = fixture.current_state();
    let newest = recovered["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|phase| phase["task_id"] == "legacy-stale-running")
        .max_by_key(|phase| phase["batch"].as_u64().unwrap())
        .unwrap();
    assert_eq!(newest["status"], "claimed");
    assert_eq!(newest["binding_generation"], old_generation + 1);
    assert_eq!(newest["attempt_generation"], 2);

    let old_start = rejected(
        &fixture,
        &[
            "board",
            "start",
            "legacy-stale-running",
            "--expected-revision",
            "3",
            "--attempt",
            "1",
        ],
        owner_thread,
    );
    assert!(old_start.contains("TICKET_STALE_BINDING"), "{old_start}");

    let started = fixture.run_public(
        &[
            "board",
            "start",
            "legacy-stale-running",
            "--expected-revision",
            "3",
            "--attempt",
            "2",
        ],
        owner_thread,
    );
    assert_eq!(started["task"]["status"], "working");

    let delivered = fixture.run_public(
        &[
            "task",
            "deliver",
            "legacy-stale-running",
            "--evidence",
            "pre-recovery stale running recovery",
            "--attempt",
            "2",
            "--no-candidate",
        ],
        owner_thread,
    );
    assert_eq!(delivered["status"], "delivered");
}

#[test]
fn rework_stale_running_attempt_reclaims_from_public_claim() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "rework stale running fixture",
        ],
        THREAD_A,
    );

    let published = publish(&fixture, "rework-stale-running", &[]);
    let slot = published["ticket"]["phases"][0]["slot"].as_str().unwrap();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let worker = slot_worker_id(&board, slot);
    let owner_thread = ticket_thread(&fixture, &worker);

    fixture.run_public(
        &[
            "board",
            "claim",
            "rework-stale-running",
            "--expected-revision",
            "1",
        ],
        owner_thread,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            "rework-stale-running",
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        owner_thread,
    );
    fixture.run_public(
        &[
            "task",
            "deliver",
            "rework-stale-running",
            "--evidence",
            "first run",
            "--attempt",
            "1",
            "--no-candidate",
        ],
        owner_thread,
    );
    fixture.run_public(
        &[
            "task",
            "update",
            "rework-stale-running",
            "--status",
            "rework",
            "--next",
            "rework the first run",
        ],
        owner_thread,
    );
    let reworked = fixture.run_public(&["task", "status", "rework-stale-running"], owner_thread);
    assert_eq!(reworked["status"], "rework");

    // Rework reopens the execute phase as a fresh claimed attempt at the
    // generation current at rework time.
    let old_generation = slot_generation(&fixture, &worker);
    set_slot_generation(&fixture, &worker, old_generation + 1);

    let drifted = fixture.current_state();
    let phase = drifted["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|phase| phase["task_id"] == "rework-stale-running")
        .max_by_key(|phase| phase["batch"].as_u64().unwrap())
        .unwrap();
    assert_eq!(phase["status"], "claimed");
    assert_eq!(phase["binding_generation"], old_generation);
    assert_eq!(phase["attempt_generation"], 2);

    let revision = drifted["board_details"]["rework-stale-running"]["revision"]
        .as_u64()
        .expect("board details revision")
        .to_string();

    let stale_start = rejected(
        &fixture,
        &[
            "board",
            "start",
            "rework-stale-running",
            "--expected-revision",
            revision.as_str(),
            "--attempt",
            "2",
        ],
        owner_thread,
    );
    assert!(
        stale_start.contains("TICKET_STALE_BINDING"),
        "{stale_start}"
    );

    // Pre-fix this claim returns BOARD_TASK_NOT_PENDING because rework is not
    // in the working|blocked set accepted by ticket_stale_active_claim_phase.
    let claimed = fixture.run_public(
        &[
            "board",
            "claim",
            "rework-stale-running",
            "--expected-revision",
            revision.as_str(),
        ],
        owner_thread,
    );
    assert_eq!(claimed["task"]["status"], "working");

    let recovered = fixture.current_state();
    let newest = recovered["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|phase| phase["task_id"] == "rework-stale-running")
        .max_by_key(|phase| phase["batch"].as_u64().unwrap())
        .unwrap();
    assert_eq!(newest["status"], "claimed");
    assert_eq!(newest["binding_generation"], old_generation + 1);
    assert_eq!(newest["attempt_generation"], 3);

    let recovered_revision = recovered["board_details"]["rework-stale-running"]["revision"]
        .as_u64()
        .expect("recovered board details revision")
        .to_string();

    let old_deliver = rejected(
        &fixture,
        &[
            "task",
            "deliver",
            "rework-stale-running",
            "--evidence",
            "old generation result",
            "--attempt",
            "2",
            "--no-candidate",
        ],
        owner_thread,
    );
    assert!(
        old_deliver.contains("TICKET_STALE_BINDING"),
        "{old_deliver}"
    );

    let started = fixture.run_public(
        &[
            "board",
            "start",
            "rework-stale-running",
            "--expected-revision",
            recovered_revision.as_str(),
            "--attempt",
            "3",
        ],
        owner_thread,
    );
    assert_eq!(started["task"]["status"], "working");

    let delivered = fixture.run_public(
        &[
            "task",
            "deliver",
            "rework-stale-running",
            "--evidence",
            "rework stale running recovery",
            "--attempt",
            "3",
            "--no-candidate",
        ],
        owner_thread,
    );
    assert_eq!(delivered["status"], "delivered");
}

#[test]
fn pending_deliver_without_or_wrong_attempt_stays_rejected() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "pending stale gate fixture",
        ],
        THREAD_A,
    );

    let published = publish(&fixture, "pending-stale-gate", &[]);
    let slot = published["ticket"]["phases"][0]["slot"]
        .as_str()
        .unwrap()
        .to_owned();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let worker = slot_worker_id(&board, &slot);
    let owner_thread = ticket_thread(&fixture, &worker);

    let without_attempt = rejected(
        &fixture,
        &[
            "task",
            "deliver",
            "pending-stale-gate",
            "--evidence",
            "missing attempt",
            "--no-candidate",
        ],
        owner_thread,
    );
    assert!(
        without_attempt.contains("BOARD_ACCEPT_REQUIRED"),
        "{without_attempt}"
    );

    let wrong_attempt = rejected(
        &fixture,
        &[
            "task",
            "deliver",
            "pending-stale-gate",
            "--evidence",
            "wrong attempt",
            "--attempt",
            "9",
            "--no-candidate",
        ],
        owner_thread,
    );
    assert!(
        wrong_attempt.contains("BOARD_ACCEPT_REQUIRED")
            && !wrong_attempt.contains("TICKET_STALE_BINDING"),
        "{wrong_attempt}"
    );

    let after_controls = fixture.current_state();
    let phase = after_controls["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["task_id"] == "pending-stale-gate")
        .unwrap();
    assert_eq!(phase["status"], "queued");
}

#[test]
fn stale_deliver_bad_token_does_not_leak_ticket() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "bad token stale gate fixture",
        ],
        THREAD_A,
    );

    let published = publish(&fixture, "bad-token-stale", &[]);
    let slot = published["ticket"]["phases"][0]["slot"]
        .as_str()
        .unwrap()
        .to_owned();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let worker = slot_worker_id(&board, &slot);
    let owner_thread = ticket_thread(&fixture, &worker);
    fixture.run_public(
        &[
            "board",
            "claim",
            "bad-token-stale",
            "--expected-revision",
            "1",
        ],
        owner_thread,
    );
    fixture.run_public(
        &[
            "board",
            "start",
            "bad-token-stale",
            "--expected-revision",
            "2",
            "--attempt",
            "1",
        ],
        owner_thread,
    );

    let mut new_thread = ThreadState::new();
    new_thread.session_id = Some(owner_thread.to_owned());
    fixture
        .state
        .lock()
        .unwrap()
        .insert(THREAD_HANDOFF.into(), new_thread);
    let refreshed = transport_refreshed(&fixture, owner_thread, THREAD_HANDOFF);
    assert_eq!(refreshed["binding"]["native_thread_id"], THREAD_HANDOFF);

    let scope = std::fs::canonicalize(&fixture.project).unwrap();
    let scope_text = scope.to_string_lossy();
    let binding = &refreshed["binding"];
    let request = json!({
        "op": "TaskDeliver",
        "worker_id": worker,
        "token": "not-the-recorded-token",
        "task_id": "bad-token-stale",
        "evidence": "bad token probe",
        "attempt": 1,
        "worktree": null,
        "no_candidate": true,
        "project_context": {
            "app_scope_id": binding["app_scope_id"],
            "project_scope": scope_text,
            "canonical_root": scope_text,
            "runtime_context": {
                "agent_id": binding["agent_id"],
                "runtime_id": binding["runtime_id"],
                "appserver_id": binding["app_scope_id"],
                "binding_id": binding["binding_id"],
                "session_id": binding["session_id"],
                "native_thread_id": binding["native_thread_id"],
            }
        }
    });
    let response = raw_public_response(&fixture, &request);
    assert_eq!(response["ok"], json!(false), "{response}");
    let error = response["error"].as_str().unwrap();
    assert!(error.contains("token mismatch"), "{response}");
    assert!(
        !error.contains("TICKET_STALE_BINDING")
            && !error.contains("claimed_generation")
            && !error.contains("current_generation")
            && !error.contains("bad-token-stale"),
        "{response}"
    );
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

fn resource_output_json(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let json = serde_json::from_str::<Value>(&stdout).unwrap_or(Value::Null);
    json!({
        "exit_code": output.status.code(),
        "stdout": stdout,
        "stderr": stderr,
        "json": json,
    })
}

fn resource_context_snapshot(output: &Output) -> Value {
    let receipt = resource_output_json(output);
    let snapshot = &receipt["json"]["result"]["snapshot"];
    json!({
        "exit_code": receipt["exit_code"],
        "name": snapshot["name"],
        "worker_id": snapshot["identity"]["worker_id"],
        "session_id": snapshot["binding"]["session_id"],
        "transport": snapshot["identity"]["transport"],
    })
}

fn resource_slot_snapshot(fixture: &AppFixture, name: &str) -> Value {
    let map = identity_map(fixture);
    map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["identity_key"]["name"] == name)
        .map(|slot| {
            json!({
                "generation": slot["generation"],
                "identity_status": slot["identity_status"],
                "worker_id": slot["current"]["binding"]["agent_id"],
                "session_id": slot["current"]["binding"]["session_id"],
                "thread_id": slot["current"]["transport"]["thread_id"],
            })
        })
        .unwrap_or_else(|| panic!("missing name slot {name}: {map}"))
}

fn resource_task_phases(state: &Value, task_id: &str) -> Value {
    state["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|phase| phase["task_id"] == task_id)
        .cloned()
        .collect()
}

fn resource_task_snapshot(state: &Value, task_id: &str) -> Value {
    json!({
        "task": state["tasks"][task_id],
        "worktree_bindings": state["worktree_bindings"]
            .as_object()
            .map(|bindings| {
                bindings
                    .iter()
                    .filter(|(_, value)| value["task_id"] == task_id)
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<serde_json::Map<String, Value>>()
            })
            .unwrap_or_default(),
        "ticket_phases": resource_task_phases(state, task_id),
        "pending_merges": state["pending_merges"][task_id],
        "task_lifecycle": state["task_lifecycle"][task_id],
    })
}

fn resource_find_revision(value: &Value, task_id: &str) -> Option<u64> {
    match value {
        Value::Object(map) => {
            if map.get("id").and_then(Value::as_str) == Some(task_id) {
                if let Some(revision) = map.get("revision").and_then(Value::as_u64) {
                    return Some(revision);
                }
            }
            map.values()
                .find_map(|child| resource_find_revision(child, task_id))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| resource_find_revision(child, task_id)),
        _ => None,
    }
}

fn write_json_evidence_log(report: &Value, env_var: &str) {
    let Some(path) = std::env::var_os(env_var).map(PathBuf::from) else {
        return;
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, serde_json::to_vec_pretty(report).unwrap()).unwrap();
    eprintln!("{env_var}={}", path.display());
}

struct ResourceBoundScenario {
    worktree_text: String,
    bob_worker: String,
    new_thread: String,
    before_slot: Value,
    after_slot: Value,
    rebound_snapshot: Value,
    publish_receipt: Value,
    claim_receipt: Value,
    relocate_receipt: Value,
}

fn resource_prepare_bound_task(
    fixture: &mut AppFixture,
    task_id: &str,
    label: &str,
) -> ResourceBoundScenario {
    let alice = fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);
    let approval = format!("resource red {label}");
    fixture.run_public(
        &["master", "promote", "--approval", approval.as_str()],
        THREAD_A,
    );
    assert_eq!(alice["name"], "Alice");
    assert_eq!(bob["name"], "Bob");
    let bob_worker = bob["identity"]["worker_id"]
        .as_str()
        .expect("bob context has worker id")
        .to_owned();

    let base = worktree_fixture_base(fixture, "initial fixture");
    let worktree_text = add_git_worktree(fixture, &base, "resource-wt", "codex/resource");

    let title = format!("resource red {label}");
    let description = format!("resource red {label}");
    let delivery_condition = format!("{label} evidence");
    let test_condition = format!("{label} receipt");
    let published = fixture.command_public(
        &[
            "board",
            "publish",
            task_id,
            "--title",
            title.as_str(),
            "--description",
            description.as_str(),
            "--delivery-condition",
            delivery_condition.as_str(),
            "--test-condition",
            test_condition.as_str(),
            "--executor",
            "Bob",
        ],
        THREAD_A,
    );
    let claimed = fixture.command_public(
        &["board", "claim", task_id, "--expected-revision", "1"],
        THREAD_B,
    );
    let relocated = fixture.command_public(
        &[
            "task",
            "relocate",
            task_id,
            "--worktree",
            &worktree_text,
            "--branch",
            "codex/resource",
        ],
        THREAD_B,
    );
    assert!(
        published.status.success(),
        "publish must succeed: {}",
        resource_output_json(&published)
    );
    assert!(
        claimed.status.success(),
        "claim must succeed: {}",
        resource_output_json(&claimed)
    );
    assert!(
        relocated.status.success(),
        "relocate must succeed: {}",
        resource_output_json(&relocated)
    );
    let mut publish_receipt = resource_output_json(&published);
    publish_receipt["command"] = json!(format!(
        "collab board publish {task_id} --title {title} --executor Bob"
    ));
    let mut claim_receipt = resource_output_json(&claimed);
    claim_receipt["command"] = json!(format!(
        "collab board claim {task_id} --expected-revision 1"
    ));
    let mut relocate_receipt = resource_output_json(&relocated);
    relocate_receipt["command"] = json!(format!(
        "collab task relocate {task_id} --worktree {worktree_text} --branch codex/resource"
    ));

    let before_slot = resource_slot_snapshot(fixture, "Bob");
    let new_thread = format!("{}-resource-new-native-thread", THREAD_B);
    fixture
        .state
        .lock()
        .unwrap()
        .entry(new_thread.clone())
        .or_insert_with(ThreadState::new)
        .session_id = Some(THREAD_B.to_owned());
    let rebound = fixture.command_public_with_session(&["context"], THREAD_B, &new_thread);
    assert!(
        rebound.status.success(),
        "rebind context: {}",
        resource_output_json(&rebound)
    );
    let rebound_snapshot = resource_context_snapshot(&rebound);
    assert_eq!(
        rebound_snapshot["worker_id"],
        json!(bob_worker),
        "same-session thread refresh must preserve the logical worker"
    );
    assert_eq!(
        rebound_snapshot["session_id"],
        json!(THREAD_B),
        "same-session thread refresh must preserve the session id"
    );
    let after_slot = resource_slot_snapshot(fixture, "Bob");
    assert!(
        after_slot["generation"].as_u64().unwrap() > before_slot["generation"].as_u64().unwrap(),
        "same-session thread refresh must advance binding generation: before={before_slot} after={after_slot}"
    );

    ResourceBoundScenario {
        worktree_text,
        bob_worker,
        new_thread,
        before_slot,
        after_slot,
        rebound_snapshot,
        publish_receipt,
        claim_receipt,
        relocate_receipt,
    }
}

fn worktree_fixture_base(fixture: &mut AppFixture, commit_message: &str) -> PathBuf {
    let config_path = fixture.root.join("home/.appsdk/config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let base = fixture.root.join("wtbase");
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    std::fs::write(
        &config_path,
        format!(
            "[worktree]\nbase = {:?}\nlayout = \"{{project-key}}/{{task-slug}}\"\n",
            base.to_string_lossy()
        ),
    )
    .unwrap();
    fixture.restart_daemon();

    let project = fixture.project.canonicalize().unwrap();
    let git_init = Command::new("git")
        .current_dir(&project)
        .args(["init", "-q", "-b", "main"])
        .output()
        .expect("run git init");
    assert!(
        git_init.status.success(),
        "git init failed: stdout={} stderr={}",
        String::from_utf8_lossy(&git_init.stdout),
        String::from_utf8_lossy(&git_init.stderr)
    );
    let git_commit = Command::new("git")
        .current_dir(&project)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            commit_message,
        ])
        .output()
        .expect("run git commit");
    assert!(
        git_commit.status.success(),
        "git commit failed: stdout={} stderr={}",
        String::from_utf8_lossy(&git_commit.stdout),
        String::from_utf8_lossy(&git_commit.stderr)
    );
    base
}

fn add_git_worktree(fixture: &AppFixture, base: &Path, slug: &str, branch: &str) -> String {
    let project = fixture.project.canonicalize().unwrap();
    let worktree_path = base.join("project").join(slug);
    std::fs::create_dir_all(worktree_path.parent().unwrap()).unwrap();
    let git_worktree = Command::new("git")
        .current_dir(&project)
        .args(["worktree", "add", "-q", "-b", branch])
        .arg(&worktree_path)
        .arg("main")
        .output()
        .expect("run git worktree add");
    assert!(
        git_worktree.status.success(),
        "git worktree add failed: stdout={} stderr={}",
        String::from_utf8_lossy(&git_worktree.stdout),
        String::from_utf8_lossy(&git_worktree.stderr)
    );
    worktree_path
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[test]
fn resource_reclaim_after_rebind_keeps_worktree() {
    let mut fixture = AppFixture::new();
    let task_id = "resource-persistent-a-reclaim-task";
    let scenario = resource_prepare_bound_task(&mut fixture, task_id, "red-a");
    let state_after_rebind = fixture.current_state();
    let resources_after_rebind = resource_task_snapshot(&state_after_rebind, task_id);

    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let revision = resource_find_revision(&board, task_id)
        .unwrap_or_else(|| panic!("board view has no revision for {task_id}: {board}"));
    let reclaim = fixture.command_public_with_session(
        &[
            "board",
            "claim",
            task_id,
            "--expected-revision",
            &revision.to_string(),
        ],
        THREAD_B,
        &scenario.new_thread,
    );
    let mut reclaim_receipt = resource_output_json(&reclaim);
    reclaim_receipt["command"] = json!(format!(
        "collab board claim {task_id} --expected-revision {revision} (current carrier after same-session new-thread rebind)"
    ));
    let resources_after_claim = resource_task_snapshot(&fixture.current_state(), task_id);
    let report = json!({
        "label": "RED A",
        "task_id": task_id,
        "worktree_path": scenario.worktree_text,
        "bob_worker": scenario.bob_worker,
        "public_sequence": [scenario.publish_receipt, scenario.claim_receipt, scenario.relocate_receipt],
        "rebind": {
            "command": "collab context (same worker, same session, new native thread)",
            "snapshot": scenario.rebound_snapshot,
            "before_slot": scenario.before_slot,
            "after_slot": scenario.after_slot,
        },
        "resources_after_rebind": resources_after_rebind,
        "board_revision_for_reclaim": revision,
        "reclaim_command": reclaim_receipt["command"],
        "reclaim": reclaim_receipt,
        "reclaim_succeeded": reclaim.status.success(),
        "resources_after_reclaim": resources_after_claim,
    });
    write_json_evidence_log(&report, "RESOURCE_PERSISTENT_A_LOG");

    if !reclaim.status.success() {
        let text = format!(
            "{}{}",
            reclaim_receipt["stderr"].as_str().unwrap_or_default(),
            reclaim_receipt["stdout"].as_str().unwrap_or_default()
        );
        assert!(
            text.contains("BOARD_RESOURCE_OWNER_CONFLICT"),
            "reclaim failed without the resource gate: {reclaim_receipt}"
        );
    }
    assert!(
        reclaim.status.success(),
        "RED A target: claim after same-name rebind must succeed and keep the original worktree; actual: {}",
        reclaim_receipt
    );
}

#[test]
fn resource_other_task_cannot_relocate_to_kept_worktree() {
    let mut fixture = AppFixture::new();
    let first_id = "resource-persistent-b-reclaim-task";
    let second_id = "resource-persistent-b-second-task";
    let scenario = resource_prepare_bound_task(&mut fixture, first_id, "red-b");
    let first_after_rebind = resource_task_snapshot(&fixture.current_state(), first_id);
    assert_eq!(
        first_after_rebind["worktree_bindings"]
            .as_object()
            .map(|bindings| bindings.len())
            .unwrap_or(0),
        1,
        "original worktree binding must be retained by the original task"
    );

    let published = fixture.command_public(
        &[
            "board",
            "publish",
            second_id,
            "--title",
            "resource second",
            "--description",
            "resource second",
            "--delivery-condition",
            "second evidence",
            "--test-condition",
            "second test",
            "--executor",
            "Bob",
        ],
        THREAD_A,
    );
    let second_claim = fixture.command_public_with_session(
        &["board", "claim", second_id, "--expected-revision", "1"],
        THREAD_B,
        &scenario.new_thread,
    );
    assert!(
        published.status.success(),
        "second publish must succeed: {}",
        resource_output_json(&published)
    );
    assert!(
        second_claim.status.success(),
        "second claim must succeed to reach the relocate contract: {}",
        resource_output_json(&second_claim)
    );
    let second_relocate = fixture.command_public_with_session(
        &[
            "task",
            "relocate",
            second_id,
            "--worktree",
            &scenario.worktree_text,
            "--branch",
            "codex/resource",
        ],
        THREAD_B,
        &scenario.new_thread,
    );
    let mut relocate_receipt = resource_output_json(&second_relocate);
    relocate_receipt["command"] = json!(format!(
        "collab task relocate {second_id} --worktree {} --branch codex/resource",
        scenario.worktree_text
    ));
    let mut publish_receipt = resource_output_json(&published);
    publish_receipt["command"] = json!(format!("collab board publish {second_id} --executor Bob"));
    let mut claim_receipt = resource_output_json(&second_claim);
    claim_receipt["command"] = json!(format!(
        "collab board claim {second_id} --expected-revision 1"
    ));
    let state_after = fixture.current_state();
    let first_after = resource_task_snapshot(&state_after, first_id);
    let second_after = resource_task_snapshot(&state_after, second_id);
    let binding_count = state_after["worktree_bindings"]
        .as_object()
        .map(|bindings| {
            bindings
                .values()
                .filter(|binding| binding["worktree_root"] == scenario.worktree_text)
                .count()
        })
        .unwrap_or(0);
    let report = json!({
        "label": "RED B",
        "first_task_id": first_id,
        "second_task_id": second_id,
        "worktree_path": scenario.worktree_text,
        "bob_worker": scenario.bob_worker,
        "original_task_public_sequence": [scenario.publish_receipt, scenario.claim_receipt, scenario.relocate_receipt],
        "rebind": {
            "command": "collab context (same worker, same session, new native thread)",
            "snapshot": scenario.rebound_snapshot,
            "before_slot": scenario.before_slot,
            "after_slot": scenario.after_slot,
        },
        "first_after_rebind": first_after_rebind,
        "second_publish": publish_receipt,
        "second_claim": claim_receipt,
        "second_relocate": relocate_receipt,
        "second_relocate_succeeded": second_relocate.status.success(),
        "first_after_relocate_attempt": first_after,
        "second_after_relocate_attempt": second_after,
        "same_path_binding_count": binding_count,
    });
    write_json_evidence_log(&report, "RESOURCE_PERSISTENT_B_LOG");

    if !second_relocate.status.success() {
        let text = format!(
            "{}{}",
            relocate_receipt["stderr"].as_str().unwrap_or_default(),
            relocate_receipt["stdout"].as_str().unwrap_or_default()
        );
        assert!(
            text.contains("TASK_RESOURCE_CONFLICT")
                || text.contains("worktree is already declared by task")
                || text.contains("BOARD_RESOURCE_OWNER_CONFLICT"),
            "relocate was rejected without resource conflict: {relocate_receipt}"
        );
    }
    assert!(
        !second_relocate.status.success(),
        "RED B target: another task must not bind the kept worktree; relocate succeeded and added binding {}",
        relocate_receipt
    );
}

#[test]
fn claimed_executor_reassignment_lets_new_slot_start_and_deliver() {
    let fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);
    let carol = fixture.run_public(&["context"], THREAD_C);
    assert_eq!(alice["name"], "Alice");
    assert_eq!(bob["name"], "Bob");
    assert_eq!(carol["name"], "Carol");
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "executor reassignment fixture",
        ],
        THREAD_A,
    );
    let carol_worker = carol["identity"]["worker_id"]
        .as_str()
        .expect("Carol worker id")
        .to_owned();

    let id = "executor-reassignment";
    publish(
        &fixture,
        id,
        &[
            "--executor",
            "Bob",
            "--long-horizon-content",
            "Complete the reassigned task under its current board contract",
            "--long-horizon-every-ms",
            "3600000",
        ],
    );
    let claimed = fixture.run_public(
        &["board", "claim", id, "--expected-revision", "1"],
        THREAD_B,
    );
    assert_eq!(claimed["task"]["status"], "working");
    assert_eq!(claimed["task"]["revision"], 2);

    let before = fixture.current_state();
    let old_phase = before["ticket_phases"]
        .as_array()
        .expect("ticket phases persist")
        .iter()
        .find(|phase| {
            phase["task_id"] == id
                && phase["phase"] == "execute"
                && phase["slot_name"] == "Bob"
                && phase["status"] == "claimed"
        })
        .cloned()
        .expect("Bob must hold the original claimed execute phase");
    let old_attempt = old_phase["attempt_generation"]
        .as_u64()
        .expect("original attempt is visible");
    let bob_generation = slot_generation(
        &fixture,
        bob["identity"]["worker_id"]
            .as_str()
            .expect("Bob worker id"),
    );

    fixture.run_public(
        &[
            "board",
            "policy",
            "--scope",
            "task",
            "--id",
            id,
            "--executor",
            "Carol",
            "--expected-revision",
            "2",
        ],
        THREAD_A,
    );
    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            id,
            "--to",
            "Carol",
            "--expected-revision",
            "3",
        ],
        THREAD_A,
    );
    assert_eq!(invited["task"]["invited_peer"], carol_worker);
    let accepted = fixture.run_public(
        &[
            "board",
            "respond",
            id,
            "--accept",
            "--expected-revision",
            "4",
        ],
        THREAD_C,
    );
    assert_eq!(accepted["task"]["owner"], carol_worker);

    let duplicate_accept = fixture.command_public(
        &[
            "board",
            "respond",
            id,
            "--accept",
            "--expected-revision",
            "4",
        ],
        THREAD_C,
    );
    assert!(
        !duplicate_accept.status.success(),
        "the consumed invitation must not be accepted twice"
    );
    let phases_after_accept: Vec<Value> = fixture.current_state()["ticket_phases"]
        .as_array()
        .expect("ticket phases persist")
        .iter()
        .filter(|phase| phase["task_id"] == id && phase["phase"] == "execute")
        .cloned()
        .collect();
    let accepted_revision = accepted["task"]["revision"]
        .as_u64()
        .expect("accepted task revision")
        .to_string();
    let reinvited = fixture.run_public(
        &[
            "board",
            "invite",
            id,
            "--to",
            "Carol",
            "--expected-revision",
            &accepted_revision,
        ],
        THREAD_A,
    );
    let reinvited_revision = reinvited["task"]["revision"]
        .as_u64()
        .expect("reinvitation revision")
        .to_string();
    let reaccepted = fixture.run_public(
        &[
            "board",
            "respond",
            id,
            "--accept",
            "--expected-revision",
            &reinvited_revision,
        ],
        THREAD_C,
    );
    let phases_after_reaccept: Vec<Value> = fixture.current_state()["ticket_phases"]
        .as_array()
        .expect("ticket phases persist")
        .iter()
        .filter(|phase| phase["task_id"] == id && phase["phase"] == "execute")
        .cloned()
        .collect();
    assert_eq!(
        phases_after_reaccept, phases_after_accept,
        "same-slot reacceptance must keep the active attempt and create no duplicate batch"
    );

    let after_accept = fixture.current_state();
    let new_phase = after_accept["ticket_phases"]
        .as_array()
        .expect("ticket phases persist")
        .iter()
        .filter(|phase| {
            phase["task_id"] == id && phase["phase"] == "execute" && phase["status"] != "revoked"
        })
        .max_by_key(|phase| phase["batch"].as_u64().unwrap_or_default())
        .cloned()
        .expect("accept must leave a current execute phase");
    let new_attempt = new_phase["attempt_generation"]
        .as_u64()
        .expect("the current execute attempt is visible");
    let carol_generation = slot_generation(&fixture, &carol_worker);

    let revision = reaccepted["task"]["revision"]
        .as_u64()
        .expect("reaccepted task revision")
        .to_string();

    // This is the target RED point: Carol must start with the attempt exposed by
    // the current visible ticket projection, not a hard-coded old attempt.
    let started = fixture.run_public(
        &[
            "board",
            "start",
            id,
            "--expected-revision",
            &revision,
            "--attempt",
            &new_attempt.to_string(),
        ],
        THREAD_C,
    );
    assert_eq!(started["task"]["status"], "working");
    assert!(
        new_attempt > old_attempt,
        "the reassignment must allocate a fresh attempt: old={old_attempt} new={new_attempt}"
    );
    assert_eq!(new_phase["slot_name"], "Carol");
    assert_eq!(
        new_phase["binding_generation"],
        json!(carol_generation),
        "the new phase must use Carol's current binding generation"
    );
    let running = started["ticket"]["phases"]
        .as_array()
        .expect("start returns ticket phases")
        .iter()
        .find(|phase| phase["status"] == "running")
        .cloned()
        .expect("Carol's phase must be running");
    assert_eq!(running["slot"], "Carol");
    assert_eq!(running["attempt_generation"], json!(new_attempt));

    // The displaced executor loses both execution operations before Carol
    // delivers, so the rejection is not hidden by a completed task.
    let old_start = fixture.command_public(
        &[
            "board",
            "start",
            id,
            "--expected-revision",
            &revision,
            "--attempt",
            &old_attempt.to_string(),
        ],
        THREAD_B,
    );
    assert!(
        !old_start.status.success(),
        "Bob's old claim must not start after reassignment: {} {}",
        String::from_utf8_lossy(&old_start.stdout),
        String::from_utf8_lossy(&old_start.stderr)
    );
    let old_deliver = fixture.command_public(
        &[
            "task",
            "deliver",
            id,
            "--evidence",
            "old executor result",
            "--attempt",
            &old_attempt.to_string(),
            "--no-candidate",
        ],
        THREAD_B,
    );
    assert!(
        !old_deliver.status.success(),
        "Bob's old claim must not deliver after reassignment: {} {}",
        String::from_utf8_lossy(&old_deliver.stdout),
        String::from_utf8_lossy(&old_deliver.stderr)
    );

    let active_after_rejections = fixture.current_state()["ticket_phases"]
        .as_array()
        .expect("ticket phases persist")
        .iter()
        .filter(|phase| {
            phase["task_id"] == id
                && phase["phase"] == "execute"
                && matches!(phase["status"].as_str(), Some("claimed" | "running"))
        })
        .count();
    assert_eq!(
        active_after_rejections, 1,
        "only Carol's current attempt may be active"
    );

    let delivered = fixture.run_public(
        &[
            "task",
            "deliver",
            id,
            "--evidence",
            "Carol completed the reassigned ticket",
            "--attempt",
            &new_attempt.to_string(),
            "--no-candidate",
        ],
        THREAD_C,
    );
    assert_eq!(delivered["status"], "delivered");

    let completed_state = fixture.current_state();
    let revoked = completed_state["ticket_phases"]
        .as_array()
        .expect("ticket phases persist")
        .iter()
        .find(|phase| {
            phase["task_id"] == id
                && phase["phase"] == "execute"
                && phase["status"] == "revoked"
                && phase["slot_name"] == "Bob"
        })
        .cloned()
        .expect("Bob's original phase must remain as history");
    assert_eq!(revoked["attempt_generation"], json!(old_attempt));
    assert_eq!(revoked["binding_generation"], json!(bob_generation));
    assert!(revoked["result"].is_null());

    let revision = completed_state["board_details"][id]["revision"]
        .as_u64()
        .expect("completed board revision")
        .to_string();
    fixture.run_public(
        &[
            "board",
            "invite",
            id,
            "--to",
            "Carol",
            "--expected-revision",
            &revision,
        ],
        THREAD_A,
    );
    let completed_revision = fixture.current_state()["board_details"][id]["revision"]
        .as_u64()
        .expect("invitation revision")
        .to_string();
    fixture.run_public(
        &[
            "board",
            "respond",
            id,
            "--accept",
            "--expected-revision",
            &completed_revision,
        ],
        THREAD_C,
    );
    let repeated = fixture.command_public(
        &[
            "board",
            "start",
            id,
            "--expected-revision",
            &completed_revision,
            "--attempt",
            &new_attempt.to_string(),
        ],
        THREAD_C,
    );
    assert!(
        !repeated.status.success(),
        "a completed ticket must not be reopened by another invitation: {} {}",
        String::from_utf8_lossy(&repeated.stdout),
        String::from_utf8_lossy(&repeated.stderr)
    );
}

fn busy_receipt(name: &str, output: &Output) -> Value {
    let mut receipt = resource_output_json(output);
    receipt["name"] = json!(name);
    receipt
}

fn busy_active_carol_phases(state: &Value) -> Vec<Value> {
    state["ticket_phases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|phase| {
            phase["slot_name"] == "Carol"
                && matches!(phase["status"].as_str(), Some("claimed" | "running"))
        })
        .cloned()
        .collect()
}

/// A-013 busy-slot observation: Bob's claimed ticket is reassigned to Carol
/// while Carol already owns a Running execute attempt on a different ticket.
/// This records, rather than invents, what the current accept gate does with a
/// second active attempt on the target slot.
#[test]
fn reassignment_accept_while_executor_slot_already_running_leaves_two_active_execute_attempts() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);
    let carol = fixture.run_public(&["context"], THREAD_C);
    assert_eq!(alice["name"], "Alice");
    assert_eq!(bob["name"], "Bob");
    assert_eq!(carol["name"], "Carol");
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "busy-slot reassignment observation",
        ],
        THREAD_A,
    );
    let carol_worker = carol["identity"]["worker_id"]
        .as_str()
        .expect("Carol worker id")
        .to_owned();
    let bob_worker = bob["identity"]["worker_id"]
        .as_str()
        .expect("Bob worker id")
        .to_owned();

    // Two real worktrees let the observation read the resource-owner side
    // effects of reassigning a bound, claimed ticket to a busy slot.
    let base = worktree_fixture_base(&mut fixture, "busy observation fixture");
    let carol_worktree = add_git_worktree(&fixture, &base, "busy-carol", "codex/busy-carol");
    let bob_worktree = add_git_worktree(&fixture, &base, "busy-bob", "codex/busy-bob");

    let first_id = "busy-carol-current";
    let second_id = "busy-bob-reassigned";
    let mut sequence = Vec::new();

    let first_publish = fixture.command_public(
        &[
            "board",
            "publish",
            first_id,
            "--title",
            "carol current ticket",
            "--description",
            "carol current ticket",
            "--delivery-condition",
            "carol current evidence",
            "--test-condition",
            "carol current receipt",
            "--executor",
            "Carol",
        ],
        THREAD_A,
    );
    assert!(
        first_publish.status.success(),
        "first publish must succeed: {}",
        busy_receipt("publish-first", &first_publish)
    );
    sequence.push(busy_receipt("publish-first", &first_publish));

    let first_claim = fixture.command_public(
        &["board", "claim", first_id, "--expected-revision", "1"],
        THREAD_C,
    );
    assert!(
        first_claim.status.success(),
        "first claim must succeed: {}",
        busy_receipt("claim-first", &first_claim)
    );
    sequence.push(busy_receipt("claim-first", &first_claim));

    let first_relocate = fixture.command_public(
        &[
            "task",
            "relocate",
            first_id,
            "--worktree",
            &carol_worktree,
            "--branch",
            "codex/busy-carol",
        ],
        THREAD_C,
    );
    assert!(
        first_relocate.status.success(),
        "first relocate must succeed: {}",
        busy_receipt("relocate-first", &first_relocate)
    );
    sequence.push(busy_receipt("relocate-first", &first_relocate));

    let first_revision = fixture.current_state()["board_details"][first_id]["revision"]
        .as_u64()
        .unwrap()
        .to_string();

    let first_start = fixture.command_public(
        &[
            "board",
            "start",
            first_id,
            "--expected-revision",
            &first_revision,
            "--attempt",
            "1",
        ],
        THREAD_C,
    );
    assert!(
        first_start.status.success(),
        "first start must succeed: {}",
        busy_receipt("start-first", &first_start)
    );
    sequence.push(busy_receipt("start-first", &first_start));

    let second_publish = fixture.command_public(
        &[
            "board",
            "publish",
            second_id,
            "--title",
            "bob reassigned ticket",
            "--description",
            "bob reassigned ticket",
            "--delivery-condition",
            "bob reassigned evidence",
            "--test-condition",
            "bob reassigned receipt",
            "--executor",
            "Bob",
            "--long-horizon-content",
            "Complete the reassigned task after the current execute slot is released",
            "--long-horizon-every-ms",
            "3600000",
        ],
        THREAD_A,
    );
    assert!(
        second_publish.status.success(),
        "second publish must succeed: {}",
        busy_receipt("publish-second", &second_publish)
    );
    sequence.push(busy_receipt("publish-second", &second_publish));

    let second_claim = fixture.command_public(
        &["board", "claim", second_id, "--expected-revision", "1"],
        THREAD_B,
    );
    assert!(
        second_claim.status.success(),
        "second claim must succeed: {}",
        busy_receipt("claim-second", &second_claim)
    );
    sequence.push(busy_receipt("claim-second", &second_claim));

    let second_relocate = fixture.command_public(
        &[
            "task",
            "relocate",
            second_id,
            "--worktree",
            &bob_worktree,
            "--branch",
            "codex/busy-bob",
        ],
        THREAD_B,
    );
    assert!(
        second_relocate.status.success(),
        "second relocate must succeed: {}",
        busy_receipt("relocate-second", &second_relocate)
    );
    sequence.push(busy_receipt("relocate-second", &second_relocate));

    let before_accept = fixture.current_state();
    let bob_old_phase = before_accept["ticket_phases"]
        .as_array()
        .expect("ticket phases persist")
        .iter()
        .find(|phase| {
            phase["task_id"] == second_id
                && phase["phase"] == "execute"
                && phase["status"] == "claimed"
                && phase["slot_name"] == "Bob"
        })
        .cloned()
        .expect("Bob must hold the original claimed execute phase");
    let bob_old_attempt = bob_old_phase["attempt_generation"]
        .as_u64()
        .expect("original attempt is visible");
    let bob_generation = slot_generation(&fixture, &bob_worker);
    let carol_generation = slot_generation(&fixture, &carol_worker);
    let policy_revision = before_accept["board_details"][second_id]["revision"]
        .as_u64()
        .unwrap()
        .to_string();

    let policy = fixture.command_public(
        &[
            "board",
            "policy",
            "--scope",
            "task",
            "--id",
            second_id,
            "--executor",
            "Carol",
            "--expected-revision",
            &policy_revision,
        ],
        THREAD_A,
    );
    assert!(
        policy.status.success(),
        "policy must succeed: {}",
        busy_receipt("policy-executor-carol", &policy)
    );
    sequence.push(busy_receipt("policy-executor-carol", &policy));

    let invite_revision = fixture.current_state()["board_details"][second_id]["revision"]
        .as_u64()
        .unwrap()
        .to_string();

    let invited = fixture.command_public(
        &[
            "board",
            "invite",
            second_id,
            "--to",
            "Carol",
            "--expected-revision",
            &invite_revision,
        ],
        THREAD_A,
    );
    assert!(
        invited.status.success(),
        "invite must succeed: {}",
        busy_receipt("invite-carol", &invited)
    );
    sequence.push(busy_receipt("invite-carol", &invited));

    let accept_revision = fixture.current_state()["board_details"][second_id]["revision"]
        .as_u64()
        .unwrap()
        .to_string();

    let accepted = fixture.command_public(
        &[
            "board",
            "respond",
            second_id,
            "--accept",
            "--expected-revision",
            &accept_revision,
        ],
        THREAD_C,
    );
    let accepted_receipt = busy_receipt("accept-second", &accepted);
    sequence.push(accepted_receipt.clone());
    assert!(
        accepted.status.success(),
        "reassignment accept succeeded with the second active attempt: {accepted_receipt}"
    );

    let after_accept = fixture.current_state();
    let second_current_phase = after_accept["ticket_phases"]
        .as_array()
        .expect("ticket phases persist")
        .iter()
        .filter(|phase| {
            phase["task_id"] == second_id
                && phase["phase"] == "execute"
                && phase["status"] != "revoked"
        })
        .max_by_key(|phase| phase["batch"].as_u64().unwrap_or_default())
        .cloned()
        .expect("accept must leave a current execute phase on the reassigned ticket");
    let new_attempt = second_current_phase["attempt_generation"]
        .as_u64()
        .expect("the reassignment must expose the new attempt");
    let active_after_accept = busy_active_carol_phases(&after_accept);

    let second_revision = after_accept["board_details"][second_id]["revision"]
        .as_u64()
        .expect("reassigned board revision")
        .to_string();
    let second_start = fixture.command_public(
        &[
            "board",
            "start",
            second_id,
            "--expected-revision",
            &second_revision,
            "--attempt",
            &new_attempt.to_string(),
        ],
        THREAD_C,
    );
    let second_start_receipt = busy_receipt("start-second", &second_start);
    sequence.push(second_start_receipt.clone());
    assert!(
        second_start.status.success(),
        "Carol must be able to start the reassigned ticket while the first stays running: {second_start_receipt}"
    );

    let after_start = fixture.current_state();
    let active_after_start = busy_active_carol_phases(&after_start);

    let report = json!({
        "label": "A-013 busy slot observation",
        "first_task_id": first_id,
        "second_task_id": second_id,
        "carol_worker": carol_worker,
        "bob_worker": bob_worker,
        "carol_generation": carol_generation,
        "bob_generation": bob_generation,
        "bob_old_phase": bob_old_phase,
        "bob_old_attempt": bob_old_attempt,
        "new_attempt": new_attempt,
        "public_sequence": sequence,
        "first_resources_before_accept": resource_task_snapshot(&before_accept, first_id),
        "second_resources_before_accept": resource_task_snapshot(&before_accept, second_id),
        "second_current_phase_after_accept": second_current_phase,
        "active_carol_execute_attempts_after_accept": active_after_accept.len(),
        "first_resources_after_accept": resource_task_snapshot(&after_accept, first_id),
        "second_resources_after_accept": resource_task_snapshot(&after_accept, second_id),
        "active_carol_execute_attempts_after_start": active_after_start.len(),
        "first_resources_after_start": resource_task_snapshot(&after_start, first_id),
        "second_resources_after_start": resource_task_snapshot(&after_start, second_id),
    });
    write_json_evidence_log(&report, "BUSY_ACCEPT_OBSERVATION_LOG");

    // The current candidate admits two valid execute attempts on Carol's slot:
    // one Running phase for the first ticket and one freshly Claimed phase for
    // the reassigned ticket. Starting the reassigned ticket while the first
    // stays running confirms the accept gate does not reject or wait.
    assert_eq!(
        active_after_accept.len(),
        2,
        "Carol must hold two active execute attempts after accept: {active_after_accept:#?}"
    );
    assert_eq!(
        second_current_phase["slot_name"], "Carol",
        "the reassigned phase must belong to Carol: {second_current_phase:#?}"
    );
    assert_eq!(
        second_current_phase["status"], "claimed",
        "the reassigned phase must be claimed: {second_current_phase:#?}"
    );
    assert!(
        new_attempt > bob_old_attempt,
        "the reassignment must allocate a fresh attempt: old={bob_old_attempt} new={new_attempt}"
    );
    assert_eq!(
        second_current_phase["binding_generation"],
        json!(carol_generation),
        "the reassigned phase must use Carol's current binding generation"
    );
    assert_eq!(
        active_after_start.len(),
        2,
        "Carol must hold two running/claimed execute attempts after start: {active_after_start:#?}"
    );
    let first_after_start = after_start["ticket_phases"]
        .as_array()
        .expect("ticket phases persist")
        .iter()
        .find(|phase| {
            phase["task_id"] == first_id
                && phase["phase"] == "execute"
                && phase["status"] != "revoked"
        })
        .cloned()
        .expect("the first ticket keeps its execute phase");
    assert_eq!(
        first_after_start["status"], "running",
        "the first ticket stays running while the reassigned ticket starts: {first_after_start:#?}"
    );
}
