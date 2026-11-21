include!("support/context_operation_fixture.rs");

fn mcp_tool_text(result: &Value) -> &str {
    result["content"]
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item["text"].as_str())
        .unwrap_or_else(|| panic!("MCP result has no text payload: {result}"))
}

fn registered_native_identity(fixture: &Fixture, host: &TestAppServer) -> (String, String, String) {
    let seed = fixture.context_provide(
        "mcp-caller-facts-seed",
        &json!({
            "session_id": "mcp-caller-session",
            "thread_id": "mcp-caller-thread",
            "endpoint": host.endpoint(),
            "namespace": "codex_tui"
        }),
        None,
    );
    assert_eq!(seed["result"]["outcome"], "completed", "{seed}");
    let identity = seed["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("registered identity has a worker id")
        .to_owned();
    let session = seed["result"]["snapshot"]["binding"]["session_id"]
        .as_str()
        .expect("registered identity has a session id")
        .to_owned();
    let thread = seed["result"]["snapshot"]["identity"]["transport"]["thread_id"]
        .as_str()
        .expect("registered identity has a native thread id")
        .to_owned();
    (identity, session, thread)
}

#[test]
fn mcp_context_hydrates_executor_facts_and_uses_enrolled_identity() {
    let fixture = Fixture::without_tmux("mcp-caller-hydrated");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    let (expected_worker, expected_session, _) = registered_native_identity(&fixture, &host);
    let map_before = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let state_before = fixture.current_state();

    let mut mcp =
        Mcp::start_with_executor_facts(&fixture, "mcp-caller-session", "mcp-caller-thread");
    let result = mcp.call_context(
        2,
        json!({"project_scope": fixture.root, "app_scope_id": "appserver-cli"}),
    );
    assert_eq!(result["isError"], false, "{result}");
    let payload: Value = serde_json::from_str(mcp_tool_text(&result))
        .expect("MCP context returns the public CLI JSON envelope");
    assert_eq!(
        payload["result"]["snapshot"]["identity"]["worker_id"], expected_worker,
        "{payload}"
    );
    assert_eq!(
        payload["result"]["snapshot"]["binding"]["session_id"], expected_session,
        "{payload}"
    );
    assert_eq!(
        payload["result"]["snapshot"]["identity"]["transport"]["kind"], "appserver",
        "{payload}"
    );
    assert_eq!(
        std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        map_before,
        "query must not mutate identity-map"
    );
    assert_eq!(
        fixture.current_state(),
        state_before,
        "query must not mutate task state"
    );
    mcp.finish();
    drop(fixture);
}

#[test]
fn mcp_callers_share_identity_context_board_and_task_views() {
    let fixture = Fixture::without_tmux("mcp-caller-shared");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    let (expected_worker, _, _) = registered_native_identity(&fixture, &host);
    let task_id = "mcp-caller-shared-task";
    let map_before = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let state_before = fixture.current_state();

    let mut mcp =
        Mcp::start_with_executor_facts(&fixture, "mcp-caller-session", "mcp-caller-thread");
    let context = mcp.call_context(
        3,
        json!({"project_scope": fixture.root, "app_scope_id": "appserver-cli"}),
    );
    let context_payload: Value = serde_json::from_str(mcp_tool_text(&context)).unwrap();
    assert_eq!(
        context_payload["result"]["snapshot"]["identity"]["worker_id"], expected_worker,
        "{context_payload}"
    );

    let board = mcp.call_tool(4, "collab_board_show", json!({}));
    assert_eq!(board["isError"], false, "{board}");
    let board_payload: Value = serde_json::from_str(mcp_tool_text(&board)).unwrap();
    assert!(
        board_payload.get("tasks").is_some() || board_payload.get("error").is_none(),
        "{board_payload}"
    );

    let task = mcp.call_tool(5, "collab_task_status", json!({"id":task_id}));
    assert_eq!(task["isError"], true, "{task}");
    let task_payload_text = mcp_tool_text(&task);
    assert!(
        !task_payload_text.contains("BOARD_IDENTITY_REQUIRED"),
        "{task_payload_text}"
    );
    assert!(
        task_payload_text.contains("task not found"),
        "caller authentication must precede task lookup: {task_payload_text}"
    );
    assert_eq!(
        std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        map_before
    );
    assert_eq!(fixture.current_state(), state_before);
    mcp.finish();
    drop(fixture);
}

#[test]
fn mcp_context_fails_closed_without_caller_facts() {
    let fixture = Fixture::without_tmux("mcp-caller-facts");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    let (expected_worker, expected_session, expected_thread) =
        registered_native_identity(&fixture, &host);
    assert_eq!(expected_thread, "mcp-caller-thread");
    let map_before = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let state_before = fixture.current_state();

    let mut mcp = Mcp::start(&fixture, None);
    let result = mcp.call_context(
        1,
        json!({
            "project_scope": fixture.root,
            "app_scope_id": "appserver-cli"
        }),
    );
    assert_eq!(result["isError"], false, "{result}");
    let payload: Value = serde_json::from_str(mcp_tool_text(&result))
        .expect("MCP context returns the public CLI JSON envelope");
    assert_eq!(payload["ok"], false, "{payload}");
    assert_eq!(payload["result"]["outcome"], "missing_facts", "{payload}");
    assert_eq!(
        payload["result"]["requires"]["fields"],
        json!(["session_id", "thread_id", "endpoint"]),
        "{payload}"
    );
    assert!(payload["result"]["snapshot"].is_null(), "{payload}");
    assert_eq!(
        std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        map_before,
        "a denied MCP query must not mutate the identity map"
    );
    assert_eq!(
        fixture.current_state(),
        state_before,
        "a denied MCP query must not mutate task state"
    );
    assert!(!expected_worker.is_empty() && !expected_session.is_empty());
    mcp.finish();

    drop(fixture);
}
