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

/// Seed the trusted launcher-owned enrollment store for a managed Desktop
/// scope. The filename is opaque to the reader, so only content matters.
fn seed_desktop_enrollment(
    fixture: &Fixture,
    pipe: &Path,
    label: &str,
    session: &str,
    thread: &str,
) {
    use std::os::unix::fs::PermissionsExt;
    let directory = fixture
        .host_state
        .join("runtime")
        .join("desktop")
        .join("enrollments");
    std::fs::create_dir_all(&directory).expect("create enrollment store");
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .expect("enrollment store is private");
    let cwd = fixture
        .root
        .canonicalize()
        .expect("canonical project root for enrollment");
    let record = json!({
        "schema": "collab-desktop-enrollment/v2",
        "host_pipe": pipe.to_string_lossy(),
        "cwd": cwd.to_string_lossy(),
        "binding": {"session": session, "thread": thread}
    });
    let path = directory.join(format!("{label}.json"));
    std::fs::write(
        &path,
        serde_json::to_vec(&record).expect("serialize enrollment record"),
    )
    .expect("write enrollment record");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .expect("enrollment record is private");
}

#[test]
fn mcp_context_hydrates_executor_facts_and_uses_enrolled_identity() {
    let fixture = Fixture::without_tmux("mcp-caller-hydrated");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    let (expected_worker, expected_session, _) = registered_native_identity(&fixture, &host);
    let map_before = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let state_before = fixture.current_state();

    let pipe = fixture.root.join("fixture-host.pipe");
    seed_desktop_enrollment(
        &fixture,
        &pipe,
        "seeded",
        "mcp-caller-session",
        "mcp-caller-thread",
    );
    let mut mcp = Mcp::start_managed_desktop(&fixture, &pipe, "mcp-caller-thread");
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

    let pipe = fixture.root.join("host.pipe");
    seed_desktop_enrollment(
        &fixture,
        &pipe,
        "seeded",
        "mcp-caller-session",
        "mcp-caller-thread",
    );
    let mut mcp = Mcp::start_managed_desktop(&fixture, &pipe, "mcp-caller-thread");
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
fn mcp_rejects_supplied_desktop_caller_facts_without_mutation() {
    let fixture = Fixture::without_tmux("mcp-caller-provided");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    registered_native_identity(&fixture, &host);
    let resources_before = fixture.current_resources();

    let mut mcp =
        Mcp::start_with_executor_facts(&fixture, "mcp-caller-session", "mcp-caller-thread");
    let result = mcp.call_context(
        6,
        json!({
            "project_scope": fixture.root,
            "app_scope_id": "appserver-cli",
            "provide": {
                "session_id": "foreign-session",
                "thread_id": "mcp-caller-thread"
            }
        }),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        mcp_tool_text(&result).contains("DESKTOP_CALLER_FACTS_REJECTED"),
        "{result}"
    );
    assert_eq!(fixture.current_resources(), resources_before);
    mcp.finish();
    drop(fixture);
}

#[test]
fn mcp_rejects_ambiguous_desktop_caller_facts_without_mutation() {
    let fixture = Fixture::without_tmux("mcp-caller-ambiguous");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    registered_native_identity(&fixture, &host);
    let resources_before = fixture.current_resources();

    let mut mcp =
        Mcp::start_with_executor_facts(&fixture, "mcp-caller-session", "mcp-caller-thread");
    let result = mcp.call_tool_with_meta(
        8,
        "collab_board_show",
        json!({}),
        json!({
            "openai/threadId": "mcp-caller-thread",
            "thread": {"id": "foreign-thread"}
        }),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        mcp_tool_text(&result).contains("DESKTOP_CALLER_FACTS_AMBIGUOUS"),
        "{result}"
    );
    assert_eq!(fixture.current_resources(), resources_before);
    mcp.finish();
    drop(fixture);
}

#[test]
fn mcp_board_denies_missing_caller_even_when_master_exists() {
    let fixture = Fixture::without_tmux("mcp-caller-master-only");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    let (master, _, _) = registered_native_identity(&fixture, &host);
    let socket = host
        .endpoint()
        .strip_prefix("unix://")
        .expect("test AppServer endpoint uses unix://")
        .to_owned();
    let promoted = fixture
        .configured_command(
            &[
                "master",
                "promote",
                "--approval",
                "isolated master-status-only fixture",
            ],
            None,
        )
        .env("CODEX_APP_SERVER_SOCKET", socket)
        .env("CODEX_SESSION_ID", "mcp-caller-session")
        .env("CODEX_THREAD_ID", "mcp-caller-thread")
        .output()
        .expect("run isolated master promotion");
    assert!(
        promoted.status.success(),
        "master promote failed: stdout={} stderr={}",
        String::from_utf8_lossy(&promoted.stdout),
        String::from_utf8_lossy(&promoted.stderr)
    );
    let promoted: Value = serde_json::from_slice(&promoted.stdout).unwrap();
    assert_eq!(promoted["master"], master, "{promoted}");
    let resources_before = fixture.current_resources();

    let mut mcp = Mcp::start(&fixture, None);
    let board = mcp.call_tool(7, "collab_board_show", json!({}));
    assert_eq!(board["isError"], true, "{board}");
    assert!(
        mcp_tool_text(&board).contains("BOARD_IDENTITY_REQUIRED"),
        "master status alone must not authorize board access: {board}"
    );
    assert_eq!(fixture.current_resources(), resources_before);
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

#[test]
fn managed_desktop_context_resolves_session_from_enrollment() {
    let fixture = Fixture::without_tmux("mcp-managed-enrolled");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    let (expected_worker, expected_session, expected_thread) =
        registered_native_identity(&fixture, &host);
    assert_eq!(expected_session, "mcp-caller-session");
    assert_eq!(expected_thread, "mcp-caller-thread");
    let pipe = fixture.root.join("host.pipe");
    seed_desktop_enrollment(
        &fixture,
        &pipe,
        "seeded",
        &expected_session,
        &expected_thread,
    );
    let map_before = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let state_before = fixture.current_state();

    let mut mcp = Mcp::start_managed_desktop(&fixture, &pipe, &expected_thread);
    let result = mcp.call_context(
        21,
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
        "managed query must not mutate identity-map"
    );
    assert_eq!(
        fixture.current_state(),
        state_before,
        "managed query must not mutate task state"
    );
    mcp.finish();
    drop(fixture);
}

#[test]
fn managed_desktop_context_fails_closed_without_enrollment() {
    let fixture = Fixture::without_tmux("mcp-managed-unenrolled");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    registered_native_identity(&fixture, &host);
    let pipe = fixture.root.join("host.pipe");
    let resources_before = fixture.current_resources();

    let mut mcp = Mcp::start_managed_desktop(&fixture, &pipe, "mcp-caller-thread");
    let result = mcp.call_context(
        22,
        json!({"project_scope": fixture.root, "app_scope_id": "appserver-cli"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        mcp_tool_text(&result).contains("DESKTOP_BRIDGE_SESSION_REQUIRED"),
        "{result}"
    );
    assert_eq!(fixture.current_resources(), resources_before);
    mcp.finish();
    drop(fixture);
}

#[test]
fn managed_desktop_context_fails_closed_on_ambiguous_enrollment() {
    let fixture = Fixture::without_tmux("mcp-managed-ambiguous");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    registered_native_identity(&fixture, &host);
    let pipe = fixture.root.join("host.pipe");
    seed_desktop_enrollment(
        &fixture,
        &pipe,
        "first",
        "mcp-caller-session",
        "mcp-caller-thread",
    );
    seed_desktop_enrollment(
        &fixture,
        &pipe,
        "second",
        "managed-rival-session",
        "mcp-caller-thread",
    );
    let resources_before = fixture.current_resources();

    let mut mcp = Mcp::start_managed_desktop(&fixture, &pipe, "mcp-caller-thread");
    let result = mcp.call_context(
        23,
        json!({"project_scope": fixture.root, "app_scope_id": "appserver-cli"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        mcp_tool_text(&result).contains("DESKTOP_BRIDGE_SESSION_AMBIGUOUS"),
        "{result}"
    );
    assert_eq!(fixture.current_resources(), resources_before);
    mcp.finish();
    drop(fixture);
}

#[test]
fn managed_desktop_rejects_conflicting_executor_thread_aliases() {
    let fixture = Fixture::without_tmux("mcp-managed-conflict");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    registered_native_identity(&fixture, &host);
    let pipe = fixture.root.join("host.pipe");
    seed_desktop_enrollment(
        &fixture,
        &pipe,
        "seeded",
        "mcp-caller-session",
        "mcp-caller-thread",
    );
    let resources_before = fixture.current_resources();

    let mut mcp = Mcp::start_managed_desktop(&fixture, &pipe, "mcp-caller-thread");
    let result = mcp.call_tool_with_meta(
        24,
        "collab_board_show",
        json!({}),
        json!({
            "openai/threadId": "mcp-caller-thread",
            "thread": {"id": "foreign-thread"}
        }),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        mcp_tool_text(&result).contains("DESKTOP_CALLER_FACTS_AMBIGUOUS"),
        "{result}"
    );
    assert_eq!(fixture.current_resources(), resources_before);
    mcp.finish();
    drop(fixture);
}

#[test]
fn managed_desktop_rejects_supplied_provide_facts() {
    let fixture = Fixture::without_tmux("mcp-managed-provide");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    registered_native_identity(&fixture, &host);
    let pipe = fixture.root.join("host.pipe");
    seed_desktop_enrollment(
        &fixture,
        &pipe,
        "seeded",
        "mcp-caller-session",
        "mcp-caller-thread",
    );
    let resources_before = fixture.current_resources();

    let mut mcp = Mcp::start_managed_desktop(&fixture, &pipe, "mcp-caller-thread");
    let result = mcp.call_context(
        25,
        json!({
            "project_scope": fixture.root,
            "app_scope_id": "appserver-cli",
            "provide": {
                "session_id": "foreign-session",
                "thread_id": "mcp-caller-thread"
            }
        }),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        mcp_tool_text(&result).contains("DESKTOP_CALLER_FACTS_REJECTED"),
        "{result}"
    );
    assert_eq!(fixture.current_resources(), resources_before);
    mcp.finish();
    drop(fixture);
}

#[test]
fn managed_desktop_never_honors_client_supplied_caller_facts() {
    let fixture = Fixture::without_tmux("mcp-managed-injected-facts");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    let (expected_worker, expected_session, expected_thread) =
        registered_native_identity(&fixture, &host);
    let pipe = fixture.root.join("host.pipe");
    seed_desktop_enrollment(
        &fixture,
        &pipe,
        "seeded",
        &expected_session,
        &expected_thread,
    );
    let map_before = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let state_before = fixture.current_state();

    let mut mcp = Mcp::start_managed_desktop(&fixture, &pipe, &expected_thread);
    let result = mcp.call_tool_raw_params(
        26,
        json!({
            "name": "collab_context",
            "arguments": {"project_scope": fixture.root, "app_scope_id": "appserver-cli"},
            "_meta": {
                "openai/threadId": expected_thread,
                "x-codex-turn-metadata": {"thread_id": expected_thread}
            },
            "_collab_caller_facts": {
                "session_id": "foreign-session",
                "thread_id": "foreign-thread"
            }
        }),
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
    assert!(
        !serde_json::to_string(&payload)
            .expect("serialize context payload")
            .contains("foreign-session"),
        "{payload}"
    );
    assert_eq!(
        std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        map_before,
        "a rejected client fact must not mutate identity-map"
    );
    assert_eq!(
        fixture.current_state(),
        state_before,
        "a rejected client fact must not mutate task state"
    );
    mcp.finish();
    drop(fixture);
}
#[test]
fn managed_desktop_environment_session_authorizes_only_through_bridge() {
    let fixture = Fixture::without_tmux("mcp-managed-env-authority");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    let (expected_worker, expected_session, expected_thread) =
        registered_native_identity(&fixture, &host);
    let pipe = fixture.root.join("host.pipe");
    seed_desktop_enrollment(
        &fixture,
        &pipe,
        "seeded",
        &expected_session,
        &expected_thread,
    );
    let map_before = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let state_before = fixture.current_state();

    let mut mcp = Mcp::start_managed_desktop_with_environment_facts(
        &fixture,
        &pipe,
        Some("foreign-session"),
        &expected_thread,
    );
    let context = mcp.call_context(
        25,
        json!({"project_scope": fixture.root, "app_scope_id": "appserver-cli"}),
    );
    assert_eq!(context["isError"], false, "{context}");
    let context_payload: Value = serde_json::from_str(mcp_tool_text(&context)).unwrap();
    assert_eq!(
        context_payload["result"]["snapshot"]["identity"]["worker_id"], expected_worker,
        "{context_payload}"
    );
    assert_eq!(
        context_payload["result"]["snapshot"]["binding"]["session_id"], expected_session,
        "{context_payload}"
    );
    assert!(
        !serde_json::to_string(&context_payload)
            .unwrap()
            .contains("foreign-session"),
        "{context_payload}"
    );

    let board = mcp.call_tool(26, "collab_board_show", json!({}));
    assert_eq!(board["isError"], false, "{board}");
    assert!(
        !mcp_tool_text(&board).contains("BOARD_IDENTITY_REQUIRED"),
        "{}",
        mcp_tool_text(&board)
    );
    let task = mcp.call_tool(27, "collab_task_status", json!({"id":"missing-task"}));
    assert!(
        mcp_tool_text(&task).contains("task not found"),
        "{}",
        mcp_tool_text(&task)
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
fn managed_desktop_reads_fail_closed_without_bridge_authority() {
    let fixture = Fixture::without_tmux("mcp-managed-no-bridge");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    registered_native_identity(&fixture, &host);
    let pipe = fixture.root.join("host.pipe");
    seed_desktop_enrollment(
        &fixture,
        &pipe,
        "seeded",
        "mcp-caller-session",
        "mcp-caller-thread",
    );
    let map_before = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let state_before = fixture.current_state();

    let mut mcp = Mcp::start_managed_desktop_without_bridge(
        &fixture,
        &pipe,
        Some("foreign-session"),
        "mcp-caller-thread",
    );
    let calls = [
        (
            "collab_context",
            json!({
                "project_scope": fixture.root,
                "app_scope_id": "appserver-cli",
                "query": true
            }),
        ),
        ("collab_board_show", json!({})),
        ("collab_task_status", json!({"id":"missing-task"})),
    ];
    for (offset, (name, args)) in calls.into_iter().enumerate() {
        let response = mcp.call_tool(28 + offset as u64, name, args);
        assert_eq!(response["isError"], true, "{name}: {response}");
        assert!(
            mcp_tool_text(&response).contains("DESKTOP_BRIDGE_"),
            "{name}: {}",
            mcp_tool_text(&response)
        );
        assert!(
            !mcp_tool_text(&response).contains("foreign-session"),
            "{name}: {}",
            mcp_tool_text(&response)
        );
    }
    assert_eq!(
        std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        map_before,
        "missing bridge proof must not mutate identity-map"
    );
    assert_eq!(
        fixture.current_state(),
        state_before,
        "missing bridge proof must not mutate task state"
    );
    mcp.finish();
    drop(fixture);
}

#[test]
fn managed_desktop_rejects_a_thread_the_native_host_does_not_know() {
    let fixture = Fixture::without_tmux("mcp-managed-unknown-thread");
    let host = TestAppServer::start(&fixture.root, "mcp-caller-session", "mcp-caller-thread");
    registered_native_identity(&fixture, &host);
    let map_before = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let state_before = fixture.current_state();

    // The host pipe answers, but only for the thread it really knows. The
    // request names a stranger, so the native read_thread rejects it and no
    // enrollment may be adopted on its behalf.
    let pipe = fixture.root.join("host.pipe");
    seed_desktop_enrollment(
        &fixture,
        &pipe,
        "seeded",
        "mcp-caller-session",
        "mcp-caller-thread",
    );
    let mut mcp = Mcp::start_managed_desktop_with_unknown_thread(
        &fixture,
        &pipe,
        Some("foreign-session"),
        "stranger-thread",
        "mcp-caller-thread",
    );
    let calls = [
        (
            "collab_context",
            json!({
                "project_scope": fixture.root,
                "app_scope_id": "appserver-cli",
                "query": true
            }),
        ),
        ("collab_board_show", json!({})),
        ("collab_board_describe", json!({})),
        ("collab_task_status", json!({"id":"missing-task"})),
    ];
    for (offset, (name, args)) in calls.into_iter().enumerate() {
        let response = mcp.call_tool(40 + offset as u64, name, args);
        assert_eq!(response["isError"], true, "{name}: {response}");
        let text = mcp_tool_text(&response);
        assert!(
            text.contains("DESKTOP_BRIDGE_NATIVE_REJECTED"),
            "{name} must be rejected by the native host: {text}"
        );
        assert!(
            !text.contains("mcp-caller-session") && !text.contains("foreign-session"),
            "{name} must not leak an enrollment session: {text}"
        );
    }
    assert_eq!(
        std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        map_before,
        "an unknown thread must not mutate identity-map"
    );
    assert_eq!(
        fixture.current_state(),
        state_before,
        "an unknown thread must not mutate task state"
    );
    mcp.finish();
    drop(fixture);
}
