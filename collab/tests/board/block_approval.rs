use super::*;
use std::io::Write;

// A distinct third thread so the approver identity is unambiguous.
const THREAD_D: &str = "01a0d643-1d55-7c4d-9c53-3f4b8f610001";

fn mcp_block_call(fixture: &AppFixture, thread: &str, tool: &str, arguments: Value) -> Value {
    let request = json!({
        "jsonrpc":"2.0",
        "id":1,
        "method":"tools/call",
        "params":{"name":tool,"arguments":arguments}
    });
    let mcp = binary()
        .parent()
        .expect("collab binary has a target directory")
        .join("collab-mcp");
    let mut command = Command::new(mcp);
    command
        .current_dir(&fixture.project)
        .env("COLLAB_STATE_DIR", &fixture.host_state)
        .env("HOME", fixture.root.join("home"))
        .env("COLLAB_APPSERVER_SOCKET", &fixture.app_socket)
        .env_remove("CODEX_APP_SERVER_SOCKET")
        .env_remove("COLLAB_APPSERVER_NAMESPACE")
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
        .env("CODEX_SESSION_ID", thread)
        .env("CODEX_THREAD_ID", thread)
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("COLLAB_WORKER")
        .env("CODEX_HOME", fixture.root.join("home"))
        .env("COLLAB_BIN", binary())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().expect("spawn actual collab-mcp");
    serde_json::to_writer(child.stdin.as_mut().unwrap(), &request).unwrap();
    child.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("read collab-mcp response");
    assert!(
        output.status.success(),
        "MCP stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["id"], 1, "{response}");
    assert_eq!(response["result"]["isError"], false, "{response}");
    serde_json::from_str(
        response["result"]["content"][0]["text"]
            .as_str()
            .expect("MCP result text"),
    )
    .expect("MCP result is structured JSON")
}

fn setup_block() -> (AppFixture, String, String, String) {
    let mut fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    let second = fixture.run_public(&["context"], THREAD_B);
    let third = fixture.run_public(&["context"], THREAD_D);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "isolated block approval fixture approval",
        ],
        THREAD_A,
    );
    (
        fixture,
        first["identity"]["worker_id"].as_str().unwrap().into(),
        second["identity"]["worker_id"].as_str().unwrap().into(),
        third["identity"]["worker_id"].as_str().unwrap().into(),
    )
}

fn publish_claim(fixture: &AppFixture, id: &str) -> Value {
    publish_claim_as(fixture, id, THREAD_B)
}

fn publish_claim_as(fixture: &AppFixture, id: &str, thread: &str) -> Value {
    fixture.run_public(
        &[
            "board",
            "publish",
            id,
            "--title",
            "阻塞审批用例",
            "--description",
            "公开阻塞审批协议",
            "--delivery-condition",
            "阻塞申请与决定被正确区分",
            "--test-condition",
            "cargo test --test appserver_two_tui_integration",
            "--priority",
            "p1",
        ],
        thread,
    );
    fixture.run_public(&["board", "claim", id, "--expected-revision", "1"], thread)
}

fn board_status(fixture: &AppFixture, id: &str) -> String {
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .map(|task| task["status"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| panic!("task {id} not on the board"))
}

#[test]
fn member_block_is_pending_until_master_approves() {
    let (fixture, _master, peer, _third) = setup_block();
    publish_claim(&fixture, "approval-happy");
    let applied = fixture.run_public(
        &[
            "task",
            "block",
            "approval-happy",
            "--next",
            "blocked on an external dependency",
        ],
        THREAD_B,
    );
    assert_eq!(applied["block_request"]["state"], "pending", "{applied}");
    assert_eq!(applied["block_request"]["requester"], peer, "{applied}");
    assert_eq!(
        applied["status"], "working",
        "task must not become blocked yet: {applied}"
    );
    assert_eq!(
        board_status(&fixture, "approval-happy"),
        "working",
        "a pending application never flips the board status"
    );
    let decided = fixture.run_public(
        &[
            "task",
            "update",
            "approval-happy",
            "--blocker-action",
            "approve",
            "--decision-reason",
            "confirmed external dependency",
        ],
        THREAD_A,
    );
    assert_eq!(decided["block_request"]["state"], "approved", "{decided}");
    assert_eq!(decided["status"], "blocked", "{decided}");
    assert_eq!(board_status(&fixture, "approval-happy"), "blocked");
}

#[test]
fn self_and_unauthorized_decisions_are_rejected_without_state_change() {
    let (fixture, _master, _peer, third) = setup_block();
    publish_claim(&fixture, "approval-auth");
    fixture.run_public(&["task", "block", "approval-auth"], THREAD_B);

    let self_decision = fixture.command_public(
        &[
            "task",
            "update",
            "approval-auth",
            "--blocker-action",
            "approve",
        ],
        THREAD_B,
    );
    assert!(!self_decision.status.success(), "self approval must fail");
    assert!(
        String::from_utf8_lossy(&self_decision.stderr).contains("TASK_BLOCK_SELF_DECISION_DENIED"),
        "{}",
        String::from_utf8_lossy(&self_decision.stderr)
    );

    let foreign = fixture.command_public(
        &[
            "task",
            "update",
            "approval-auth",
            "--blocker-action",
            "approve",
        ],
        THREAD_D,
    );
    assert!(
        !foreign.status.success(),
        "an unrelated peer must not decide"
    );
    assert!(
        String::from_utf8_lossy(&foreign.stderr).contains("TASK_BLOCK_DECISION_UNAUTHORIZED"),
        "{}",
        String::from_utf8_lossy(&foreign.stderr)
    );
    let _ = third;
    assert_eq!(board_status(&fixture, "approval-auth"), "working");

    let context = fixture.run_public(&["context"], THREAD_B);
    let requests = context["management"]["block_requests"].as_array().unwrap();
    assert_eq!(requests.len(), 1, "{context}");
    assert_eq!(requests[0]["task_id"], "approval-auth");
    assert_eq!(
        requests[0]["state"], "pending",
        "rejected decisions must not decide: {context}"
    );
}

#[test]
fn master_reject_keeps_task_running_and_records_rejection() {
    let (fixture, _master, _peer, _third) = setup_block();
    publish_claim(&fixture, "approval-reject");
    fixture.run_public(&["task", "block", "approval-reject"], THREAD_B);
    let decided = fixture.run_public(
        &[
            "task",
            "update",
            "approval-reject",
            "--blocker-action",
            "reject",
            "--decision-reason",
            "the work can proceed",
        ],
        THREAD_A,
    );
    assert_eq!(decided["block_request"]["state"], "rejected", "{decided}");
    assert_eq!(decided["status"], "working", "{decided}");
    assert_eq!(board_status(&fixture, "approval-reject"), "working");
}

#[test]
fn stale_decision_is_rejected_without_partial_state() {
    let (fixture, _master, _peer, _third) = setup_block();
    publish_claim(&fixture, "approval-stale");
    fixture.run_public(&["task", "block", "approval-stale"], THREAD_B);
    // Advance the task revision after the request was filed.
    fixture.run_public(
        &[
            "task",
            "update",
            "approval-stale",
            "--next",
            "plan changed after the application",
        ],
        THREAD_B,
    );
    let decided = fixture.command_public(
        &[
            "task",
            "update",
            "approval-stale",
            "--blocker-action",
            "approve",
        ],
        THREAD_A,
    );
    assert!(!decided.status.success(), "a stale decision must fail");
    assert!(
        String::from_utf8_lossy(&decided.stderr).contains("BOARD_STALE_REVISION"),
        "{}",
        String::from_utf8_lossy(&decided.stderr)
    );
    assert_eq!(board_status(&fixture, "approval-stale"), "working");
    let context = fixture.run_public(&["context"], THREAD_B);
    let requests = context["management"]["block_requests"].as_array().unwrap();
    assert_eq!(
        requests[0]["state"], "pending",
        "no partial commit: {context}"
    );
}

#[test]
fn repeated_decision_is_idempotent_and_conflicting_decision_is_rejected() {
    let (fixture, _master, _peer, _third) = setup_block();
    publish_claim(&fixture, "approval-repeat");
    fixture.run_public(&["task", "block", "approval-repeat"], THREAD_B);
    let first = fixture.run_public(
        &[
            "task",
            "update",
            "approval-repeat",
            "--blocker-action",
            "approve",
        ],
        THREAD_A,
    );
    assert_eq!(first["block_request"]["state"], "approved", "{first}");
    let replay = fixture.run_public(
        &[
            "task",
            "update",
            "approval-repeat",
            "--blocker-action",
            "approve",
        ],
        THREAD_A,
    );
    assert_eq!(replay["idempotent"], true, "{replay}");
    let conflicting = fixture.command_public(
        &[
            "task",
            "update",
            "approval-repeat",
            "--blocker-action",
            "reject",
        ],
        THREAD_A,
    );
    assert!(
        !conflicting.status.success(),
        "a conflicting decision must fail"
    );
    assert!(
        String::from_utf8_lossy(&conflicting.stderr).contains("TASK_BLOCK_ALREADY_DECIDED"),
        "{}",
        String::from_utf8_lossy(&conflicting.stderr)
    );
    assert_eq!(board_status(&fixture, "approval-repeat"), "blocked");
}

#[test]
fn mcp_block_and_decision_use_the_same_pending_approval_gate() {
    let (fixture, _master, _peer, _third) = setup_block();
    publish_claim(&fixture, "approval-mcp");

    let applied = mcp_block_call(
        &fixture,
        THREAD_B,
        "collab_task_block",
        json!({"id":"approval-mcp","next":"external dependency"}),
    );
    assert_eq!(applied["block_request"]["state"], "pending", "{applied}");
    assert_eq!(applied["status"], "working", "{applied}");

    let decided = mcp_block_call(
        &fixture,
        THREAD_A,
        "collab_task_update",
        json!({
            "id":"approval-mcp",
            "blocker_action":"approve",
            "decision_reason":"confirmed through the actual MCP adapter"
        }),
    );
    assert_eq!(decided["block_request"]["state"], "approved", "{decided}");
    assert_eq!(decided["status"], "blocked", "{decided}");
    assert_eq!(board_status(&fixture, "approval-mcp"), "blocked");
}

#[test]
fn pending_block_request_cannot_be_erased_by_delivery() {
    let (fixture, _master, _peer, _third) = setup_block();
    publish_claim(&fixture, "approval-delivery");
    fixture.run_public(&["task", "block", "approval-delivery"], THREAD_B);

    let deliver = fixture.command_public(
        &[
            "task",
            "deliver",
            "approval-delivery",
            "--evidence",
            "attempted while the block request is pending",
            "--no-candidate",
        ],
        THREAD_B,
    );
    assert!(
        !deliver.status.success(),
        "delivery must not erase a pending block application"
    );
    assert!(
        String::from_utf8_lossy(&deliver.stderr).contains("TASK_BLOCK_PENDING_APPROVAL"),
        "{}",
        String::from_utf8_lossy(&deliver.stderr)
    );
    assert_eq!(
        board_status(&fixture, "approval-delivery"),
        "working",
        "the pending application and task state stay intact"
    );
}

#[test]
fn context_scope_isolation_and_manager_brief_are_public() {
    let (fixture, master, peer, ceo) = setup_block();
    publish_claim(&fixture, "scope-task");
    fixture.run_public(&["task", "block", "scope-task"], THREAD_B);
    publish_claim_as(&fixture, "ceo-scope-task", THREAD_A);
    fixture.run_public(&["task", "block", "ceo-scope-task"], THREAD_A);
    let promoted = fixture.run_public(
        &[
            "ceo",
            "promote",
            "--approval",
            "isolated public context fixture approval",
        ],
        THREAD_D,
    );
    assert_eq!(promoted["is_current"], true, "{promoted}");
    assert_eq!(promoted["worker_id"], ceo, "{promoted}");
    let before = fixture.run_public(&["board", "show"], THREAD_A);
    let revision = before["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "scope-task")
        .unwrap()["revision"]
        .clone();

    let master_context = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(
        master_context["management"]["scope"], "project",
        "{master_context}"
    );
    let master_tasks = master_context["management"]["tasks"].as_array().unwrap();
    assert!(
        master_tasks
            .iter()
            .any(|task| task["id"] == "scope-task" && task["revision"].is_u64()),
        "the master sees authorized board task state with revisions: {master_context}"
    );
    let responsibilities = master_context["role_brief"]["responsibilities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        responsibilities
            .contains("Managers do not debug, reproduce, investigate root cause, or implement"),
        "master brief must forbid manager-level debug/implementation: {responsibilities}"
    );
    assert!(
        responsibilities.contains("prioritize P0/P1 by urgency and dependency"),
        "master brief must be board-first and priority oriented: {responsibilities}"
    );
    let peer_row = master_context["peers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["worker_id"] == peer)
        .expect("master context includes the peer activity row");
    assert_eq!(
        peer_row["activity"], "unknown",
        "an absent activity observation must stay unknown, not become idle: {peer_row}"
    );
    assert_eq!(
        peer_row["availability"], "unknown",
        "unknown activity must stay unknown rather than becoming idle: {peer_row}"
    );
    let after = fixture.run_public(&["board", "show"], THREAD_A);
    let projected_revision = after["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "scope-task")
        .unwrap()["revision"]
        .clone();
    assert_eq!(
        projected_revision, revision,
        "context projection must not mutate the board revision"
    );

    let ceo_context = fixture.run_public(&["context"], THREAD_D);
    assert_eq!(ceo_context["identity"]["role"], "worker", "{ceo_context}");
    assert_eq!(
        ceo_context["role_brief"]["role"], "worker",
        "the CEO grant must not rewrite the project role: {ceo_context}"
    );
    assert_eq!(ceo_context["ceo"]["is_current"], true, "{ceo_context}");
    assert_eq!(
        ceo_context["management"]["scope"], "project",
        "the current CEO must receive the project management scope: {ceo_context}"
    );
    let ceo_tasks = ceo_context["management"]["tasks"].as_array().unwrap();
    for task_id in ["scope-task", "ceo-scope-task"] {
        assert!(
            ceo_tasks.iter().any(|task| task["id"] == task_id),
            "the current CEO must see project task {task_id}: {ceo_context}"
        );
    }
    let ceo_requests = ceo_context["management"]["block_requests"]
        .as_array()
        .unwrap();
    assert_eq!(ceo_requests.len(), 2, "{ceo_context}");
    assert!(
        ceo_requests
            .iter()
            .any(|request| request["task_id"] == "ceo-scope-task"
                && request["requester"] == master),
        "the current CEO must see the master-owned block request: {ceo_context}"
    );
    let after_ceo = fixture.run_public(&["board", "show"], THREAD_A);
    assert_eq!(
        after_ceo["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == "scope-task")
            .unwrap()["revision"],
        revision,
        "the CEO context projection must not mutate the board revision"
    );

    let peer_context = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(
        peer_context["management"]["scope"], "self",
        "{peer_context}"
    );
    assert!(
        peer_context["management"]["tasks"]
            .as_array()
            .unwrap()
            .is_empty(),
        "an ordinary peer must not read the management scope: {peer_context}"
    );
    let peer_requests = peer_context["management"]["block_requests"]
        .as_array()
        .unwrap();
    assert_eq!(
        peer_requests.len(),
        1,
        "an ordinary peer sees only its own block request: {peer_context}"
    );
    assert_eq!(
        peer_requests[0]["task_id"], "scope-task",
        "a non-CEO peer must not see the master-owned block request: {peer_context}"
    );
}
