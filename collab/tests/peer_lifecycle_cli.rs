include!("support/context_operation_fixture.rs");

fn worker_help(action: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_collab"))
        .args(["worker", action, "--help"])
        .output()
        .expect("run candidate collab CLI help");
    assert!(
        output.status.success(),
        "worker {action} --help failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("CLI help is UTF-8")
}

#[test]
fn peer_lifecycle_create_cli_exposes_frozen_arguments() {
    let create = worker_help("create");
    for argument in ["TARGET_ID", "--cwd", "--model", "--op"] {
        assert!(create.contains(argument), "{create}");
    }
    let read = worker_help("read");
    assert!(read.contains("TARGET_ID"), "{read}");

    let update = worker_help("update");
    assert!(update.contains("--cwd"), "{update}");
    assert!(update.contains("--op"), "{update}");

    let close = worker_help("close");
    assert!(close.contains("--reason"), "{close}");
    assert!(close.contains("--op"), "{close}");

    let query = worker_help("query");
    assert!(query.contains("--op"), "{query}");

    let root_help = Command::new(env!("CARGO_BIN_EXE_collab"))
        .args(["worker", "--help"])
        .output()
        .expect("run worker help");
    assert!(root_help.status.success());
    let root_help = String::from_utf8(root_help.stdout).expect("worker help is UTF-8");
    assert!(root_help.contains("\n  create "), "{root_help}");
    for unsupported in ["\n  register ", "\n  force "] {
        assert!(
            !root_help.to_ascii_lowercase().contains(unsupported),
            "worker help unexpectedly exposes {unsupported}: {root_help}"
        );
    }
}

#[test]
fn peer_lifecycle_create_cli_and_mcp_round_trip_through_isolated_daemon() {
    let fixture = Fixture::without_tmux("peer-lifecycle-adapter");
    let appserver = TestAppServer::start_with_effect(
        &fixture.root,
        "lifecycle-session",
        "lifecycle-thread",
        "ready_and_update_ok",
    );
    let seeded = fixture.context_provide(
        "peer-lifecycle-seed",
        &serde_json::json!({
            "session_id": "lifecycle-session",
            "thread_id": "lifecycle-thread",
            "endpoint": appserver.endpoint(),
            "namespace": "codex_tui"
        }),
        None,
    );
    assert_eq!(seeded["result"]["outcome"], "completed", "{seeded}");

    let endpoint = appserver.endpoint();
    let endpoint_path = endpoint
        .strip_prefix("unix://")
        .expect("fixture AppServer uses a unix endpoint")
        .to_owned();
    let configure_runtime = |command: &mut Command| {
        command
            .env("CODEX_APP_SERVER_SOCKET", &endpoint_path)
            .env("CODEX_SESSION_ID", "lifecycle-session")
            .env("CODEX_THREAD_ID", "lifecycle-thread");
    };
    let mut promote = fixture.configured_command(
        &[
            "master",
            "promote",
            "--approval",
            "isolated CLI Create acceptance",
        ],
        None,
    );
    configure_runtime(&mut promote);
    let promoted = promote.output().unwrap();
    assert!(
        promoted.status.success(),
        "{}",
        String::from_utf8_lossy(&promoted.stderr)
    );
    let mut context = fixture.configured_command(&["context"], None);
    configure_runtime(&mut context);
    let context = context.output().expect("read master context");
    assert!(
        context.status.success(),
        "master context failed: stdout={} stderr={}",
        String::from_utf8_lossy(&context.stdout),
        String::from_utf8_lossy(&context.stderr)
    );
    let context: serde_json::Value = serde_json::from_slice(&context.stdout).unwrap();
    let peer_lifecycle = context["result"]["snapshot"]["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|operation| operation["kind"] == "peer_lifecycle")
        .expect("master context must expose peer lifecycle commands");
    assert_eq!(peer_lifecycle["operations"].as_array().unwrap().len(), 5);
    assert_eq!(
        peer_lifecycle["scope_rule"],
        "the actor, target, and requested cwd must resolve to the same registered canonical project main and app scope; cwd may be any existing directory or linked worktree in that project"
    );
    for action in ["create", "read", "update", "close", "query"] {
        assert!(
            peer_lifecycle["operations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["action"] == action
                    && entry["command"]
                        .as_str()
                        .is_some_and(|command| command.starts_with("collab worker "))
                    && entry["success"].is_string()
                    && entry["failure"].is_string()),
            "missing complete {action} operation card: {peer_lifecycle}"
        );
    }
    let cwd = fixture.root.to_str().unwrap();
    for _ in 0..2 {
        let mut command = fixture.configured_command(
            &[
                "worker",
                "create",
                "cli-created-peer",
                "--cwd",
                cwd,
                "--op",
                "cli-create-op",
            ],
            None,
        );
        configure_runtime(&mut command);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let created: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(created["result"]["outcome"], "complete", "{created}");
        assert_eq!(created["result"]["create"]["thread_id"], "created-thread-1");
        assert_eq!(
            created["result"]["create"]["readiness"]["state"], "verified",
            "{created}"
        );
        assert_eq!(
            created["result"]["create"]["readiness"]["challenge"]["turn_id"],
            "create-readiness-turn",
            "{created}"
        );
        assert!(
            !created.to_string().contains("commandExecution"),
            "{created}"
        );
    }
    assert_eq!(appserver.start_request_count(), 1);
    assert_eq!(appserver.readiness_request_count(), 1);

    let read = {
        let mut command = fixture.configured_command(&["worker", "read"], None);
        configure_runtime(&mut command);
        command.output().expect("run CLI lifecycle read")
    };
    assert!(
        read.status.success(),
        "CLI read failed: stdout={} stderr={}",
        String::from_utf8_lossy(&read.stdout),
        String::from_utf8_lossy(&read.stderr)
    );
    let read: serde_json::Value = serde_json::from_slice(&read.stdout).unwrap();
    assert_eq!(read["result"]["outcome"], "ok", "{read}");

    let mcp_call = |arguments: Value| {
        let mcp_input = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "collab_peer_lifecycle",
                "arguments": arguments
            }
        });
        let mut mcp = Command::new(collab_test_mcp_binary());
        mcp.current_dir(&fixture.root)
            .env("COLLAB_STATE_DIR", &fixture.host_state)
            .env("CODEX_HOME", fixture.root.join("home"))
            .env("COLLAB_BIN", &fixture.binary)
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI");
        configure_runtime(&mut mcp);
        let mut child = mcp
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn MCP adapter");
        use std::io::Write;
        serde_json::to_writer(child.stdin.as_mut().unwrap(), &mcp_input).unwrap();
        child.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
        drop(child.stdin.take());
        let output = child.wait_with_output().expect("read MCP adapter response");
        assert!(
            output.status.success(),
            "MCP call failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["id"], 1, "{response}");
        assert_eq!(response["result"]["isError"], false, "{response}");
        let content = response["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str::<serde_json::Value>(content).unwrap()
    };
    let created = mcp_call(serde_json::json!({
        "action": "create",
        "target_id": "mcp-created-peer",
        "cwd": cwd,
        "operation_id": "mcp-create-op"
    }));
    assert_eq!(created["result"]["outcome"], "complete", "{created}");
    assert_eq!(created["result"]["create"]["thread_id"], "created-thread-2");
    assert_eq!(
        created["result"]["create"]["readiness"]["state"], "verified",
        "{created}"
    );
    assert_eq!(
        created["result"]["create"]["readiness"]["challenge"]["turn_id"], "create-readiness-turn",
        "{created}"
    );
    assert_eq!(appserver.readiness_request_count(), 2);

    let mcp_read = mcp_call(serde_json::json!({
        "action": "read",
        "target_id": "mcp-created-peer"
    }));
    assert_eq!(mcp_read["result"]["outcome"], "ok", "{mcp_read}");

    let mut update = fixture.configured_command(
        &["worker", "update", "mcp-created-peer", "--cwd", cwd],
        None,
    );
    configure_runtime(&mut update);
    let output = update.output().expect("run CLI lifecycle update");
    assert!(
        output.status.success(),
        "CLI update failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let update: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(update["result"]["outcome"], "complete", "{update}");
    assert_eq!(
        update["result"]["update"]["effective_cwd"]["state"], "verified",
        "{update}"
    );
    assert_eq!(
        update["result"]["update"]["effective_cwd"]["turn_id"], "update-challenge-turn",
        "{update}"
    );
    assert_eq!(
        update["result"]["update"]["challenge"]["state"], "verified",
        "{update}"
    );

    let closed = mcp_call(serde_json::json!({
        "action": "close",
        "target_id": "mcp-created-peer",
        "reason": "verify lifecycle MCP close"
    }));
    assert_eq!(closed["result"]["outcome"], "complete", "{closed}");
    assert_eq!(
        closed["result"]["close"]["runtime_archive"]["state"],
        "verified"
    );
    assert_eq!(appserver.archive_request_count(), 1);
    assert_eq!(appserver.settings_request_count(), 1);
    assert_eq!(appserver.challenge_request_count(), 1);
    assert_eq!(appserver.start_request_count(), 2);
}

#[test]
fn verified_public_create_can_be_bound_and_read_back_as_managed() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let fixture = Fixture::without_tmux("subagent-bind-public");
    let appserver = TestAppServer::start_with_effect(
        &fixture.root,
        "subagent-bind-session",
        "subagent-bind-master-thread",
        "ready_and_update_ok",
    );
    let seeded = fixture.context_provide(
        "subagent-bind-seed",
        &json!({
            "session_id": "subagent-bind-session",
            "thread_id": "subagent-bind-master-thread",
            "endpoint": appserver.endpoint(),
            "namespace": "codex_tui"
        }),
        None,
    );
    assert_eq!(seeded["result"]["outcome"], "completed", "{seeded}");

    let endpoint_path = appserver
        .endpoint()
        .strip_prefix("unix://")
        .expect("fixture AppServer uses a unix endpoint")
        .to_owned();
    let configure_runtime = |command: &mut Command| {
        command
            .env("CODEX_APP_SERVER_SOCKET", &endpoint_path)
            .env("CODEX_SESSION_ID", "subagent-bind-session")
            .env("CODEX_THREAD_ID", "subagent-bind-master-thread");
    };
    let subagent_mcp_call = |arguments: Value| {
        let request = json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"tools/call",
            "params":{"name":"collab_subagent","arguments":arguments}
        });
        let mut command = Command::new(collab_test_mcp_binary());
        command
            .current_dir(&fixture.root)
            .env("COLLAB_STATE_DIR", &fixture.host_state)
            .env("CODEX_HOME", fixture.root.join("home"))
            .env("COLLAB_BIN", &fixture.binary)
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI");
        configure_runtime(&mut command);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn actual collab-mcp adapter");
        serde_json::to_writer(child.stdin.as_mut().unwrap(), &request).unwrap();
        child.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
        drop(child.stdin.take());
        let output = child
            .wait_with_output()
            .expect("read subagent MCP response");
        assert!(
            output.status.success(),
            "MCP stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(envelope["id"], 1, "{envelope}");
        envelope["result"].clone()
    };

    let mut promote = fixture.configured_command(
        &[
            "master",
            "promote",
            "--approval",
            "isolated managed Bind acceptance",
        ],
        None,
    );
    configure_runtime(&mut promote);
    let promoted = promote.output().expect("promote isolated master");
    assert!(
        promoted.status.success(),
        "{}",
        String::from_utf8_lossy(&promoted.stderr)
    );

    let cwd = fixture.root.to_str().expect("fixture path is UTF-8");
    let mut create = fixture.configured_command(
        &[
            "worker",
            "create",
            "bind-created-peer",
            "--cwd",
            cwd,
            "--op",
            "bind-create-op",
        ],
        None,
    );
    configure_runtime(&mut create);
    let created = create.output().expect("run public peer Create");
    assert!(
        created.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&created.stdout),
        String::from_utf8_lossy(&created.stderr)
    );
    let created: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(created["result"]["outcome"], "complete", "{created}");
    assert_eq!(
        created["result"]["create"]["readiness"]["state"],
        "verified"
    );

    let bind_args = [
        "subagent",
        "bind",
        "managed-created-peer",
        "--create-op",
        "bind-create-op",
    ];
    let bind_reply = subagent_mcp_call(json!({
        "action":"bind",
        "id":"managed-created-peer",
        "create_operation_id":"bind-create-op"
    }));
    assert_eq!(bind_reply["isError"], false, "{bind_reply}");
    let bound: Value = serde_json::from_str(
        bind_reply["content"][0]["text"]
            .as_str()
            .expect("Bind MCP result text"),
    )
    .expect("Bind MCP returns structured JSON");
    assert_eq!(bound["association_commit"], "committed", "{bound}");
    assert_eq!(bound["subagent"]["peer"], "bind-created-peer");
    assert_eq!(bound["subagent"]["create_operation_id"], "bind-create-op");
    assert!(bound["subagent"]["binding_id"].is_string());
    assert!(bound["subagent"]["endpoint_generation"].as_u64().is_some());

    let mut repeated_bind = fixture.configured_command(&bind_args, None);
    configure_runtime(&mut repeated_bind);
    let repeated = repeated_bind.output().expect("repeat exact Bind");
    assert!(
        repeated.status.success(),
        "{}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    let repeated: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(repeated["association_commit"], "reused", "{repeated}");

    let configure_child_runtime = |command: &mut Command| {
        command
            .env("CODEX_APP_SERVER_SOCKET", &endpoint_path)
            .env("CODEX_SESSION_ID", "created-session")
            .env("CODEX_THREAD_ID", "created-thread-1");
    };
    let mut child_context = fixture.configured_command(&["context"], None);
    configure_child_runtime(&mut child_context);
    let child_context = child_context
        .output()
        .expect("register created child runtime");
    assert!(
        child_context.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&child_context.stdout),
        String::from_utf8_lossy(&child_context.stderr)
    );
    let mut ready =
        fixture.configured_command(&["subagent", "ready", "managed-created-peer"], None);
    configure_child_runtime(&mut ready);
    let ready = ready
        .output()
        .expect("child reports ready through public CLI");
    assert!(
        ready.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&ready.stdout),
        String::from_utf8_lossy(&ready.stderr)
    );
    let ready: Value = serde_json::from_slice(&ready.stdout).unwrap();
    assert_eq!(ready["subagent_action"]["action"], "ready", "{ready}");
    assert_eq!(
        ready["subagent_action"]["state_commit"], "committed",
        "{ready}"
    );

    let send_reply = subagent_mcp_call(json!({
        "action":"send",
        "id":"managed-created-peer",
        "subject":"managed-child task",
        "body":"Complete the public managed Send path."
    }));
    assert_eq!(send_reply["isError"], false, "{send_reply}");
    let sent: Value = serde_json::from_str(
        send_reply["content"][0]["text"]
            .as_str()
            .expect("Send MCP result text"),
    )
    .expect("Send MCP returns structured JSON");
    assert_eq!(sent["subagent_action"]["action"], "send", "{sent}");
    assert_eq!(
        sent["subagent_action"]["state_commit"], "not-needed",
        "{sent}"
    );
    let success_msg_id = sent["message"]["msg_id"]
        .as_str()
        .expect("durable Send message id");
    let mut message_status = fixture.configured_command(&["msg", success_msg_id], None);
    configure_runtime(&mut message_status);
    let message_status = message_status
        .output()
        .expect("read durable message status");
    assert!(message_status.status.success());
    let message_status: Value = serde_json::from_slice(&message_status.stdout).unwrap();
    assert_eq!(
        message_status["to"], "bind-created-peer",
        "{message_status}"
    );
    assert_eq!(
        message_status["body"],
        "Complete the public managed Send path."
    );

    let mut child_recv = fixture.configured_command(&["recv", "--timeout", "1"], None);
    child_recv
        .env("CODEX_APP_SERVER_SOCKET", &endpoint_path)
        .env("CODEX_SESSION_ID", "created-session")
        .env("CODEX_THREAD_ID", "created-thread-1");
    let child_recv = child_recv
        .output()
        .expect("child consumes durable Send message");
    assert!(
        child_recv.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&child_recv.stdout),
        String::from_utf8_lossy(&child_recv.stderr)
    );
    let child_recv: Value = serde_json::from_slice(&child_recv.stdout).unwrap();
    assert_eq!(
        child_recv["messages"][0]["id"], success_msg_id,
        "{child_recv}"
    );
    let mut consumed_status = fixture.configured_command(&["msg", success_msg_id], None);
    configure_runtime(&mut consumed_status);
    let consumed_status = consumed_status
        .output()
        .expect("read consumed durable message");
    let consumed_status: Value = serde_json::from_slice(&consumed_status.stdout).unwrap();
    assert_eq!(
        consumed_status["consumed_by_recv"], true,
        "{consumed_status}"
    );

    // Exercise the public partial-commit path with a second managed child.
    // Its ready transition and durable message commit must remain visible when
    // the downstream AppServer notification fails.
    let mut create_failed_delivery = fixture.configured_command(
        &[
            "worker",
            "create",
            "bind-failure-peer",
            "--cwd",
            cwd,
            "--op",
            "bind-failure-create-op",
        ],
        None,
    );
    configure_runtime(&mut create_failed_delivery);
    let created_failed_delivery = create_failed_delivery
        .output()
        .expect("create second peer for partial notification outcome");
    assert!(
        created_failed_delivery.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&created_failed_delivery.stdout),
        String::from_utf8_lossy(&created_failed_delivery.stderr)
    );
    let mut bind_failed_delivery = fixture.configured_command(
        &[
            "subagent",
            "bind",
            "managed-failure-peer",
            "--create-op",
            "bind-failure-create-op",
        ],
        None,
    );
    configure_runtime(&mut bind_failed_delivery);
    let bound_failed_delivery = bind_failed_delivery
        .output()
        .expect("bind second verified Create result");
    assert!(bound_failed_delivery.status.success());

    let mut failed_child_context = fixture.configured_command(&["context"], None);
    failed_child_context
        .env("CODEX_APP_SERVER_SOCKET", &endpoint_path)
        .env("CODEX_SESSION_ID", "created-session-2")
        .env("CODEX_THREAD_ID", "created-thread-2");
    let failed_child_context = failed_child_context
        .output()
        .expect("register second created child runtime");
    assert!(
        failed_child_context.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&failed_child_context.stdout),
        String::from_utf8_lossy(&failed_child_context.stderr)
    );
    appserver.set_effect("reject_generic_turn_start");

    let mut failed_ready =
        fixture.configured_command(&["subagent", "ready", "managed-failure-peer"], None);
    failed_ready
        .env("CODEX_APP_SERVER_SOCKET", &endpoint_path)
        .env("CODEX_SESSION_ID", "created-session-2")
        .env("CODEX_THREAD_ID", "created-thread-2");
    let failed_ready = failed_ready
        .output()
        .expect("ready notification is rejected");
    assert!(!failed_ready.status.success());
    let failed_ready_error = String::from_utf8_lossy(&failed_ready.stderr);
    assert!(
        failed_ready_error.contains("APPSERVER_NOTIFICATION_REJECTED"),
        "{failed_ready_error}"
    );
    assert!(
        failed_ready_error.contains("\"state_commit\":\"committed\""),
        "{failed_ready_error}"
    );

    let failed_send_reply = subagent_mcp_call(json!({
        "action":"send",
        "id":"managed-failure-peer",
        "subject":"managed-child partial delivery",
        "body":"Preserve the durable message and repair signal."
    }));
    assert_eq!(failed_send_reply["isError"], true, "{failed_send_reply}");
    let failed_send_error = failed_send_reply["content"][0]["text"]
        .as_str()
        .expect("failed Send MCP error text");
    let failed_send_response_json = failed_send_error
        .lines()
        .find_map(|line| line.strip_prefix("collab response: "))
        .expect("failed Send MCP output preserves the daemon response");
    let failed_send_response: Value = serde_json::from_str(failed_send_response_json)
        .expect("failed Send daemon response is structured JSON");
    assert!(
        failed_send_response["error"]
            .as_str()
            .is_some_and(|error| {
                error.starts_with("APPSERVER_NOTIFICATION_REJECTED:")
                    && error.contains("fixture refused generic notification")
            }),
        "{failed_send_response}"
    );
    assert_eq!(failed_send_response["durable"], true, "{failed_send_response}");
    assert_eq!(
        failed_send_response["failure"],
        "notification_delivery_failed",
        "{failed_send_response}"
    );
    assert_eq!(
        failed_send_response["notification"],
        "subscribed-not-sent",
        "{failed_send_response}"
    );
    assert_eq!(
        failed_send_response["repair_required"],
        true,
        "{failed_send_response}"
    );
    assert_eq!(
        failed_send_response["escalation"],
        "the message is durable but this wake has an ambiguous submission outcome; do not retry this wake, have the recipient run collab recv, and send a new message if another wake is needed",
        "{failed_send_response}"
    );
    let failed_action = &failed_send_response["subagent_action"];
    assert_eq!(failed_action["subagent_id"], "managed-failure-peer");
    assert_eq!(failed_action["action"], "send");
    assert_eq!(failed_action["state_commit"], "not-needed");
    assert_eq!(failed_action["error_state_commit"], "not-attempted");
    assert_eq!(failed_action["status"], "assigned");
    let failed_send_msg_id = failed_action["durable_msg_id"]
        .as_str()
        .expect("failed Send retains its durable message ID")
        .to_owned();
    assert_eq!(failed_send_response["msg_id"], failed_send_msg_id);

    // The rejected wake is not retried. The child reads the durable message,
    // and public MsgStatus proves the same ID was consumed.
    let mut failed_child_recv = fixture.configured_command(&["recv", "--timeout", "1"], None);
    failed_child_recv
        .env("CODEX_APP_SERVER_SOCKET", &endpoint_path)
        .env("CODEX_SESSION_ID", "created-session-2")
        .env("CODEX_THREAD_ID", "created-thread-2");
    let failed_child_recv = failed_child_recv
        .output()
        .expect("child consumes the durable message after rejected wake");
    assert!(
        failed_child_recv.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&failed_child_recv.stdout),
        String::from_utf8_lossy(&failed_child_recv.stderr)
    );
    let failed_child_recv: Value = serde_json::from_slice(&failed_child_recv.stdout).unwrap();
    let failed_message = &failed_child_recv["messages"][0];
    assert_eq!(failed_message["id"], failed_send_msg_id);
    assert_eq!(failed_message["subject"], "managed-child partial delivery");
    assert_eq!(
        failed_message["body"],
        "Preserve the durable message and repair signal."
    );
    let mut failed_message_status = fixture
        .configured_command(&["msg", failed_send_msg_id.as_str()], None);
    configure_runtime(&mut failed_message_status);
    let failed_message_status = failed_message_status
        .output()
        .expect("read failed-send message status");
    let failed_message_status: Value =
        serde_json::from_slice(&failed_message_status.stdout).unwrap();
    assert_eq!(
        failed_message_status["consumed_by_recv"], true,
        "{failed_message_status}"
    );

    // A removed subscription is a distinct, non-error repair terminal: the
    // durable message remains available and can still be consumed.
    let mut create_no_subscription = fixture.configured_command(
        &[
            "worker",
            "create",
            "bind-no-subscription-peer",
            "--cwd",
            cwd,
            "--op",
            "bind-no-subscription-create-op",
        ],
        None,
    );
    configure_runtime(&mut create_no_subscription);
    let created_no_subscription = create_no_subscription
        .output()
        .expect("create peer for mailbox-only repair terminal");
    assert!(
        created_no_subscription.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&created_no_subscription.stdout),
        String::from_utf8_lossy(&created_no_subscription.stderr)
    );
    let mut bind_no_subscription = fixture.configured_command(
        &[
            "subagent",
            "bind",
            "managed-no-subscription-peer",
            "--create-op",
            "bind-no-subscription-create-op",
        ],
        None,
    );
    configure_runtime(&mut bind_no_subscription);
    assert!(bind_no_subscription
        .output()
        .expect("bind no-subscription peer")
        .status
        .success());
    let mut no_subscription_child_context = fixture.configured_command(&["context"], None);
    no_subscription_child_context
        .env("CODEX_APP_SERVER_SOCKET", &endpoint_path)
        .env("CODEX_SESSION_ID", "created-session-3")
        .env("CODEX_THREAD_ID", "created-thread-3");
    assert!(no_subscription_child_context
        .output()
        .expect("register no-subscription child runtime")
        .status
        .success());
    let mut no_subscription_ready =
        fixture.configured_command(&["subagent", "ready", "managed-no-subscription-peer"], None);
    no_subscription_ready
        .env("CODEX_APP_SERVER_SOCKET", &endpoint_path)
        .env("CODEX_SESSION_ID", "created-session-3")
        .env("CODEX_THREAD_ID", "created-thread-3");
    let no_subscription_ready = no_subscription_ready
        .output()
        .expect("ready transition commits despite rejected wake");
    assert!(!no_subscription_ready.status.success());
    assert!(String::from_utf8_lossy(&no_subscription_ready.stderr)
        .contains("\"state_commit\":\"committed\""));

    let mut child_notify_status = fixture.configured_command(&["notify", "status"], None);
    child_notify_status
        .env("CODEX_APP_SERVER_SOCKET", &endpoint_path)
        .env("CODEX_SESSION_ID", "created-session-3")
        .env("CODEX_THREAD_ID", "created-thread-3");
    let child_notify_status = child_notify_status
        .output()
        .expect("inspect exact child-owned notification subscription");
    let child_notify_status: Value = serde_json::from_slice(&child_notify_status.stdout).unwrap();
    let subscription_id = child_notify_status["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|subscription| subscription["event"] == "direct-message")
        .and_then(|subscription| subscription["id"].as_str())
        .expect("child direct-message subscription");
    let mut unsubscribe =
        fixture.configured_command(&["notify", "unsubscribe", subscription_id], None);
    unsubscribe
        .env("CODEX_APP_SERVER_SOCKET", &endpoint_path)
        .env("CODEX_SESSION_ID", "created-session-3")
        .env("CODEX_THREAD_ID", "created-thread-3");
    let unsubscribe = unsubscribe.output().expect("unsubscribe exact child route");
    assert!(unsubscribe.status.success());
    let no_subscription_reply = subagent_mcp_call(json!({
        "action":"send",
        "id":"managed-no-subscription-peer",
        "subject":"managed-child mailbox repair",
        "body":"Read this durable message without a wake subscription."
    }));
    assert_eq!(
        no_subscription_reply["isError"], false,
        "{no_subscription_reply}"
    );
    let no_subscription: Value = serde_json::from_str(
        no_subscription_reply["content"][0]["text"]
            .as_str()
            .expect("mailbox-only Send MCP result"),
    )
    .expect("mailbox-only Send result is JSON");
    assert_eq!(
        no_subscription["message"]["durable"], true,
        "{no_subscription}"
    );
    assert_eq!(
        no_subscription["message"]["repair_required"], true,
        "{no_subscription}"
    );
    assert_eq!(
        no_subscription["message"]["notification"], "mailbox-only-no-subscription",
        "{no_subscription}"
    );
    let no_subscription_msg_id = no_subscription["message"]["msg_id"]
        .as_str()
        .expect("mailbox-only durable message ID");
    let mut mailbox_only_recv = fixture.configured_command(&["recv", "--timeout", "1"], None);
    mailbox_only_recv
        .env("CODEX_APP_SERVER_SOCKET", &endpoint_path)
        .env("CODEX_SESSION_ID", "created-session-3")
        .env("CODEX_THREAD_ID", "created-thread-3");
    let mailbox_only_recv = mailbox_only_recv
        .output()
        .expect("consume mailbox-only durable managed message");
    assert!(mailbox_only_recv.status.success());
    let mailbox_only_recv: Value = serde_json::from_slice(&mailbox_only_recv.stdout).unwrap();
    assert_eq!(
        mailbox_only_recv["messages"][0]["id"], no_subscription_msg_id,
        "{mailbox_only_recv}"
    );

    let mut list = fixture.configured_command(&["subagent", "list"], None);
    configure_runtime(&mut list);
    let listed = list.output().expect("read managed child list");
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let listed: Value = serde_json::from_slice(&listed.stdout).unwrap();
    let record = listed["subagents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["id"] == "managed-created-peer")
        .expect("managed Bind is visible through public List");
    assert_eq!(record["peer"], "bind-created-peer");
    assert_eq!(record["create_operation_id"], "bind-create-op");
    let failed_record = listed["subagents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["id"] == "managed-failure-peer")
        .expect("failed managed Send state remains visible through public List");
    assert_eq!(failed_record["status"], "assigned", "{failed_record}");
}

#[cfg(feature = "context-cancel-test-hooks")]
#[test]
fn public_delayed_readback_commit_failure_keeps_marker_and_recovers() {
    let mut fixture = Fixture::without_tmux("public-readback-commit-failure");
    let appserver = TestAppServer::start_with_effect(
        &fixture.root,
        "readback-commit-session",
        "readback-commit-master-thread",
        "create_ready_delayed",
    );
    let seeded = fixture.context_provide(
        "readback-commit-seed",
        &json!({
            "session_id": "readback-commit-session",
            "thread_id": "readback-commit-master-thread",
            "endpoint": appserver.endpoint(),
            "namespace": "codex_tui"
        }),
        None,
    );
    assert_eq!(seeded["result"]["outcome"], "completed", "{seeded}");
    let endpoint_path = appserver
        .endpoint()
        .strip_prefix("unix://")
        .expect("fixture AppServer uses unix endpoint")
        .to_owned();
    let configure_master_runtime = |command: &mut Command| {
        command
            .env("CODEX_APP_SERVER_SOCKET", &endpoint_path)
            .env("CODEX_SESSION_ID", "readback-commit-session")
            .env("CODEX_THREAD_ID", "readback-commit-master-thread");
    };
    let mut promote = fixture.configured_command(
        &[
            "master",
            "promote",
            "--approval",
            "isolated readback commit proof",
        ],
        None,
    );
    configure_master_runtime(&mut promote);
    assert!(promote.output().unwrap().status.success());

    let cwd = fixture.root.to_string_lossy().into_owned();
    let create_args = [
        "worker",
        "create",
        "readback-commit-peer",
        "--cwd",
        cwd.as_str(),
        "--op",
        "public-readback-create",
    ];
    let mut create = fixture.configured_command(&create_args, None);
    configure_master_runtime(&mut create);
    let first_create = create.output().expect("create delayed public peer");
    let first_create: Value = serde_json::from_slice(&first_create.stdout).unwrap();
    assert_eq!(
        first_create["error"], "PEER_LIFECYCLE_READINESS_PENDING",
        "{first_create}"
    );
    assert_eq!(
        first_create["result"]["phase"], "readback_pending",
        "{first_create}"
    );
    let create_marker = first_create["result"]["create"]["readiness"]["challenge"]["marker_file"]
        .as_str()
        .unwrap();
    let create_marker_path = fixture.root.join(create_marker);
    assert!(create_marker_path.exists());

    fixture.restart_daemon_with_env(
        "COLLAB_TEST_FAIL_PEER_LIFECYCLE_COMPLETION_APPEND",
        "public-readback-create",
    );
    let mut fail_create_completion = fixture.configured_command(&create_args, None);
    configure_master_runtime(&mut fail_create_completion);
    let failed_create = fail_create_completion
        .output()
        .expect("completion append fails through public Create replay");
    assert!(!failed_create.status.success());
    let failed_create_text = format!(
        "{}{}",
        String::from_utf8_lossy(&failed_create.stdout),
        String::from_utf8_lossy(&failed_create.stderr)
    );
    assert!(
        failed_create_text.contains("PEER_LIFECYCLE_READBACK_COMMIT_FAILED"),
        "{failed_create_text}"
    );
    assert!(
        create_marker_path.exists(),
        "failed completion commit retains proof marker"
    );

    fixture.restart_daemon();
    let mut recover_create = fixture.configured_command(&create_args, None);
    configure_master_runtime(&mut recover_create);
    let recovered_create = recover_create.output().expect("retry same Create readback");
    assert!(
        recovered_create.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&recovered_create.stdout),
        String::from_utf8_lossy(&recovered_create.stderr)
    );
    let recovered_create: Value = serde_json::from_slice(&recovered_create.stdout).unwrap();
    assert_eq!(
        recovered_create["result"]["outcome"], "complete",
        "{recovered_create}"
    );
    assert!(!create_marker_path.exists());
    assert_eq!(appserver.start_request_count(), 1);
    assert_eq!(appserver.readiness_request_count(), 1);

    appserver.set_effect("update_challenge_delayed");
    let update_args = [
        "worker",
        "update",
        "readback-commit-peer",
        "--cwd",
        cwd.as_str(),
    ];
    let mut update = fixture.configured_command(&update_args, None);
    configure_master_runtime(&mut update);
    let first_update = update.output().expect("start delayed public Update");
    assert!(
        !first_update.stdout.is_empty(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first_update.stdout),
        String::from_utf8_lossy(&first_update.stderr)
    );
    let first_update: Value = serde_json::from_slice(&first_update.stdout).unwrap();
    assert_eq!(
        first_update["error"], "PEER_LIFECYCLE_EFFECTIVE_CWD_PENDING",
        "{first_update}"
    );
    assert_eq!(
        first_update["result"]["phase"], "readback_pending",
        "{first_update}"
    );
    let update_operation_id = first_update["result"]["operation_id"]
        .as_str()
        .expect("pending Update returns its operation ID")
        .to_owned();
    let update_retry_args = [
        "worker",
        "update",
        "readback-commit-peer",
        "--cwd",
        cwd.as_str(),
        "--op",
        update_operation_id.as_str(),
    ];
    let update_marker = first_update["result"]["update"]["challenge"]["marker_file"]
        .as_str()
        .unwrap();
    let update_marker_path = fixture.root.join(update_marker);
    assert!(update_marker_path.exists());

    fixture.restart_daemon_with_env(
        "COLLAB_TEST_FAIL_PEER_LIFECYCLE_COMPLETION_APPEND",
        update_operation_id.as_str(),
    );
    let mut fail_update_completion = fixture.configured_command(&update_retry_args, None);
    configure_master_runtime(&mut fail_update_completion);
    let failed_update = fail_update_completion
        .output()
        .expect("completion append fails through public Update replay");
    let failed_update_text = format!(
        "{}{}",
        String::from_utf8_lossy(&failed_update.stdout),
        String::from_utf8_lossy(&failed_update.stderr)
    );
    let failed_update_response: Value = serde_json::from_slice(&failed_update.stdout)
        .expect("failed Update readback returns structured error JSON");
    assert!(
        failed_update_text.contains("MIGRATION_ADMISSION_FROZEN"),
        "{failed_update_text}"
    );
    assert_eq!(
        failed_update_response["ok"], false,
        "{failed_update_response}"
    );
    assert!(
        failed_update_response["error"]
            .as_str()
            .is_some_and(|error| error.starts_with("MIGRATION_ADMISSION_FROZEN:")),
        "{failed_update_response}"
    );
    assert!(
        update_marker_path.exists(),
        "failed completion commit retains proof marker"
    );

    fixture.restart_daemon();
    let mut recover_update = fixture.configured_command(&update_retry_args, None);
    configure_master_runtime(&mut recover_update);
    let recovered_update = recover_update.output().expect("retry same Update readback");
    assert!(
        recovered_update.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&recovered_update.stdout),
        String::from_utf8_lossy(&recovered_update.stderr)
    );
    let recovered_update: Value = serde_json::from_slice(&recovered_update.stdout).unwrap();
    assert_eq!(
        recovered_update["result"]["outcome"], "complete",
        "{recovered_update}"
    );
    assert!(!update_marker_path.exists());
    assert_eq!(appserver.settings_request_count(), 1);
    assert_eq!(appserver.challenge_request_count(), 1);
}
