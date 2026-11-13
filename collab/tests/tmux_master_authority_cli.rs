include!("support/tmux_cli_fixture.rs");

#[test]
fn context_session_reuse_and_minimal_master_approval() {
    let (mut fixture, first) = single_pane_fixture("identity-session-main");
    let daemon_pid = fixture.daemon.as_ref().unwrap().id();
    let initial = fixture.registered_context(&first, true);
    let worker = initial["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let split = tmux(
        &fixture.tmux_socket,
        &[
            "split-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            &first.pane_id,
            "sleep 600",
        ],
    );
    let pane_id = String::from_utf8(split.stdout).unwrap().trim().to_owned();
    let second = Pane {
        pane_id,
        server_pid: first.server_pid,
        session_anchor: first.session_anchor.clone(),
        thread_anchor: first.thread_anchor.clone(),
    };
    let moved = fixture.run_context(&["context", "--op", "identity-session-move"], Some(&second));
    assert_eq!(moved["identity"]["worker_id"], worker, "{moved}");
    assert_eq!(
        moved["binding"]["tmux_endpoint"]["pane_id"], second.pane_id,
        "{moved}"
    );
    let other_session = "identity-session-incumbent";
    let created = tmux(
        &fixture.tmux_socket,
        &[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            other_session,
            "sleep 600",
        ],
    );
    let stable = tmux(
        &fixture.tmux_socket,
        &[
            "display-message",
            "-p",
            "-t",
            other_session,
            "#{session_id}",
        ],
    );
    fixture
        .owned_sessions
        .push(String::from_utf8(stable.stdout).unwrap().trim().to_owned());
    let pane_id = String::from_utf8(created.stdout).unwrap().trim().to_owned();
    let incumbent = Pane {
        pane_id,
        server_pid: first.server_pid,
        session_anchor: "incumbent-native-session".into(),
        thread_anchor: "incumbent-native-thread".into(),
    };
    let registered = fixture.registered_context(&incumbent, true);
    let incumbent_worker = registered["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture.run_ok(
        &["master", "promote", "--approval", "user assigned incumbent"],
        Some(&incumbent),
    );
    let peer = fixture.run_context(&["context", "--op", "identity-peer-usable"], Some(&second));
    assert_eq!(peer["identity"]["worker_id"], worker, "{peer}");
    assert_eq!(peer["identity"]["role"], "worker", "{peer}");
    assert_eq!(peer["master"]["worker_id"], incumbent_worker, "{peer}");
    let peer_message = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &incumbent_worker,
            "--subject",
            "peer remains usable",
            "ordinary peer message",
        ],
        Some(&second),
    );
    let received = fixture.run_ok(&["recv", "--timeout", "0"], Some(&incumbent));
    assert!(
        received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == peer_message["msg_id"]),
        "{received}"
    );
    let reply = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &worker,
            "--subject",
            "reply",
            "peer recovery reply",
        ],
        Some(&incumbent),
    );
    let received = fixture.run_ok(&["recv", "--timeout", "0"], Some(&second));
    assert!(
        received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == reply["msg_id"]),
        "{received}"
    );
    let args = ["context", "--op", "identity-minimal-master", "--approve-grant", "{\"action\":\"restore_master\",\"authorization\":\"user approved current session master\",\"replace_confirmed\":true}"];
    let master = fixture.run_context(&args, Some(&second));
    assert_eq!(master["identity"]["role"], "master", "{master}");
    assert_eq!(
        fixture.run_ok(&["master", "status"], Some(&second))["master"]["worker_id"],
        worker
    );
    let replay = fixture.run_context(&args, Some(&second));
    assert_eq!(replay["master"]["worker_id"], worker, "{replay}");
    let current = fixture.run_context(&["context"], Some(&second));
    assert_eq!(current["identity"]["role"], "master", "{current}");
    assert!(!current["next_actions"]
        .to_string()
        .contains("--approve-grant"));
    let returned = fixture.run_context(&["context"], Some(&incumbent));
    assert_eq!(returned["identity"]["role"], "worker", "{returned}");
    assert_eq!(returned["master"]["worker_id"], worker, "{returned}");
    let returned_worker = returned["identity"]["worker_id"].as_str().unwrap();
    let sent = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            returned_worker,
            "--subject",
            "master",
            "master recovery message",
        ],
        Some(&second),
    );
    let received = fixture.run_ok(&["recv", "--timeout", "0"], Some(&incumbent));
    assert!(
        received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == sent["msg_id"]),
        "{received}"
    );
    let reply = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &worker,
            "--subject",
            "reply",
            "master recovery reply",
        ],
        Some(&incumbent),
    );
    let received = fixture.run_ok(&["recv", "--timeout", "0"], Some(&second));
    assert!(
        received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == reply["msg_id"]),
        "{received}"
    );
    assert_eq!(fixture.daemon.as_ref().unwrap().id(), daemon_pid);
    assert!(fixture
        .daemon
        .as_mut()
        .unwrap()
        .try_wait()
        .unwrap()
        .is_none());
}

/// B01: a tmux registration records a Codex session/thread anchor, but a tmux
/// pane address cannot answer whether the agent is resident. The recorded grant
/// holder must still be able to act, and every public projection must agree on
/// that holder while the transport observation stays independent.
#[test]
fn master_authority_tmux_unknown_holder_keeps_control_and_projection() {
    let (fixture, pane) = single_pane_fixture("collab-master-authority-b01");
    let registered = fixture.registered_context(&pane, true);
    let worker_id = registered["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names the registered worker")
        .to_owned();
    let project_scope = registered["binding"]["project_scope"]
        .as_str()
        .expect("context receipt names the project scope")
        .to_owned();
    let app_scope_id = registered["binding"]["app_scope_id"]
        .as_str()
        .expect("context receipt names the app scope")
        .to_owned();
    assert_eq!(registered["identity"]["role"], "worker", "{registered}");
    assert_eq!(
        registered["role_brief"]["derivation"]["kind"], "peer",
        "{registered}"
    );
    assert!(registered["master"].is_null(), "{registered}");
    let ordinary_authority = &registered["role_brief"]["authority"];
    assert_eq!(ordinary_authority["managed_subagent"], false);
    assert_eq!(ordinary_authority["must_obey_master"], false);
    assert_eq!(ordinary_authority["may_decline_master_invite"], true);
    let ordinary_role_task = registered["role_brief"]["role_task"]
        .as_str()
        .expect("ordinary peer role task");
    let ordinary_responsibilities = registered["role_brief"]["responsibilities"]
        .as_array()
        .expect("ordinary peer responsibilities")
        .iter()
        .filter_map(|line| line.as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    assert!(ordinary_role_task.contains("long-horizon"));
    assert!(ordinary_role_task.contains("orchestrator"));
    assert!(ordinary_responsibilities.contains("codex exec --profile gcm"));
    assert!(ordinary_responsibilities.contains("verify subworker evidence"));
    assert_eq!(registered["role_brief"]["goal_level"], "delegated_goal");
    assert_eq!(registered["identity"]["role"], "worker");
    assert!(!ordinary_responsibilities.contains("saturate every live present peer first"));

    let promoted = fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated tmux master",
        ],
        Some(&pane),
    );
    assert_eq!(promoted["master"], worker_id);
    assert_scope("promote receipt", &promoted, &project_scope, &app_scope_id);

    let status = fixture.run_ok(&["master", "status"], Some(&pane));
    assert_eq!(
        status["master"]["worker_id"], worker_id,
        "status must project the current grant holder: {status}"
    );
    assert_scope("assigned status", &status, &project_scope, &app_scope_id);
    assert!(
        status["recorded_unusable"].is_null(),
        "recorded_unusable must not carry a second authority model: {status}"
    );

    wait_for_agent_observations(&registered);
    let context = fixture.run_context(&["context"], Some(&pane));
    assert_eq!(
        context["master"]["worker_id"], worker_id,
        "context must project the current grant holder: {context}"
    );
    assert_eq!(context["identity"]["role"], "master");
    assert_scope("assigned context", &context, &project_scope, &app_scope_id);
    assert!(context["recorded_unusable"].is_null(), "{context}");
    let master_responsibilities = context["role_brief"]["responsibilities"]
        .as_array()
        .expect("master responsibilities")
        .iter()
        .filter_map(|line| line.as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    assert!(master_responsibilities.contains("long-horizon goals"));
    assert!(master_responsibilities.contains("no suitable authorized peer can take the work"));
    assert!(master_responsibilities.contains("bounded subagent tasks"));
    assert!(master_responsibilities.contains("no authorized next action remains"));
    assert!(
        master_responsibilities.contains("close unused owned patrol tasks and timed subscriptions")
    );
    assert!(master_responsibilities.contains("keep direct-message reception"));
    assert!(master_responsibilities.contains("never to itself"));
    assert!(master_responsibilities.contains("codex exec --profile gcm"));
    assert!(master_responsibilities.contains("no inherited master authority"));
    assert!(!master_responsibilities.contains("saturate every live present peer first"));
    let next_action = context["role_brief"]["next_action"]
        .as_str()
        .expect("master next action");
    assert_eq!(
        context["next_actions"],
        serde_json::json!([]),
        "idle master next_actions must be empty when no task next_step exists: {context}"
    );
    let next_action_lower = next_action.to_lowercase();
    assert!(
        next_action_lower.contains("invite suitable, available and authorized peers first"),
        "{next_action}"
    );
    assert!(
        next_action_lower.contains("no suitable authorized peer can take the work"),
        "{next_action}"
    );
    assert!(
        next_action_lower.contains(
            "retain the goal and route the remaining bounded work through the existing independent"
        ),
        "{next_action}"
    );
    assert!(
        next_action_lower.contains("codex exec --profile gcm"),
        "{next_action}"
    );
    assert!(
        next_action_lower.contains("project-canonical intake"),
        "{next_action}"
    );
    assert!(
        next_action_lower.contains("protected_holds"),
        "{next_action}"
    );
    assert!(
        next_action_lower.contains(
            "reusing a valid patrol batch and renewing it only per the applicable duties"
        ),
        "{next_action}"
    );
    assert!(
        next_action_lower.contains("do not re-register or reset it per round"),
        "{next_action}"
    );
    // The fixture runner provides a reachable pane, but no confirmed agent.
    assert_eq!(context["liveness"]["presence"], "unknown");
    assert_eq!(
        context["agent"]["presence_observation"]["reason"],
        "unsupported_foreground_process"
    );
    assert_eq!(context["liveness"]["live"], true);
    assert_eq!(context["master"]["endpoint_live"], true);
    assert_eq!(context["agent"]["thread_state"], "unknown");
    assert_eq!(context["agent"]["activity"], "unknown");
    assert!(context["agent"]["latest_turn_status"].is_null());
    assert!(context["agent"]["can_accept_direct_input"].is_null());

    let published = fixture.run_ok(
        &[
            "board",
            "publish",
            "tmux-master-task",
            "--title",
            "tmux master control",
            "--description",
            "control without a transport liveness probe",
            "--delivery-condition",
            "the board accepts the publish",
            "--test-condition",
            "this test",
            "--priority",
            "p2",
        ],
        Some(&pane),
    );
    assert_eq!(published["task"]["id"], "tmux-master-task");

    let board = fixture.run_ok(&["board", "show"], Some(&pane));
    assert_scope("assigned board", &board, &project_scope, &app_scope_id);
    let master_rows: Vec<&Value> = board["workers"]
        .as_array()
        .expect("board lists workers")
        .iter()
        .filter(|worker| worker["role"] == "master")
        .collect();
    assert_eq!(master_rows.len(), 1, "exactly one master role: {board}");
    assert_eq!(master_rows[0]["id"], worker_id);
    assert_ne!(
        master_rows[0]["status"], "online",
        "a pane address must not make the agent online: {board}"
    );
}

/// B03: an authenticated peer clears the current grant without becoming master
/// and without probing the incumbent. A second clear is idempotent, and no
/// task, peer, message or consumption state is deleted.
#[test]
fn master_authority_clear_removes_grant_and_preserves_business_state() {
    let (mut fixture, pane) = single_pane_fixture("collab-master-authority-b03");
    let registered = fixture.registered_context(&pane, true);
    let worker_id = registered["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names the registered worker")
        .to_owned();
    let project_scope = registered["binding"]["project_scope"]
        .as_str()
        .expect("context receipt names the project scope")
        .to_owned();
    let app_scope_id = registered["binding"]["app_scope_id"]
        .as_str()
        .expect("context receipt names the app scope")
        .to_owned();
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated tmux master",
        ],
        Some(&pane),
    );

    fixture.run_ok(
        &[
            "task",
            "register",
            "authority-clear-task",
            "--next",
            "survive clear",
        ],
        Some(&pane),
    );
    let sent = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &worker_id,
            "--subject",
            "authority clear",
            "preserve me",
        ],
        Some(&pane),
    );
    let message_id = sent["msg_id"]
        .as_str()
        .or_else(|| sent["message_id"].as_str())
        .expect("send names the durable message")
        .to_owned();
    let task_before = fixture.current_state_for(&fixture.root, &app_scope_id)["tasks"]
        ["authority-clear-task"]
        .clone();

    let cleared = fixture.run_ok(
        &[
            "master",
            "clear",
            "--approval",
            "user approved isolated clear",
        ],
        Some(&pane),
    );
    assert_eq!(cleared["was_empty"], false, "{cleared}");
    assert!(cleared["master"].is_null(), "{cleared}");
    assert_eq!(cleared["previous_worker_id"], worker_id);
    assert_scope("clear receipt", &cleared, &project_scope, &app_scope_id);

    let status = fixture.run_ok(&["master", "status"], Some(&pane));
    assert!(status["master"].is_null(), "{status}");
    assert_scope("empty status", &status, &project_scope, &app_scope_id);

    let context = fixture.run_context(&["context"], Some(&pane));
    assert!(context["master"].is_null(), "{context}");
    assert_scope("empty context", &context, &project_scope, &app_scope_id);
    assert_eq!(context["identity"]["role"], "worker");
    assert!(
        !context["tasks"]
            .as_array()
            .expect("context lists tasks")
            .iter()
            .any(|task| task["id"] == "authority-clear-task"),
        "the new ordinary name must not inherit Jenny's work: {context}"
    );
    let retained = fixture.run_ok(&["task", "status", "authority-clear-task"], Some(&pane));
    assert_eq!(retained["owner_name"], "Jenny", "{retained}");
    assert_eq!(retained["owner"], "", "{retained}");
    assert_eq!(
        fixture.current_state_for(&fixture.root, &app_scope_id)["tasks"]["authority-clear-task"],
        task_before
    );
    assert!(
        context["inbox"]["messages"]
            .as_array()
            .expect("context lists inbox messages")
            .iter()
            .any(|message| message["id"].as_str() == Some(message_id.as_str())),
        "clear must not delete messages: {context}"
    );

    let again = fixture.run_ok(
        &[
            "master",
            "clear",
            "--approval",
            "user approved isolated clear",
        ],
        Some(&pane),
    );
    assert_eq!(
        again["was_empty"], true,
        "second clear is idempotent: {again}"
    );
    assert!(again["master"].is_null(), "{again}");
    assert_scope(
        "second clear receipt",
        &again,
        &project_scope,
        &app_scope_id,
    );

    // A real isolated daemon restart must not resurrect any typed or legacy
    // holder, and the public task/message facts must still be present.
    fixture.restart_daemon();

    let restarted_status = fixture.run_ok(&["master", "status"], Some(&pane));
    assert!(
        restarted_status["master"].is_null(),
        "clear must not resurrect authority after restart: {restarted_status}"
    );
    assert_scope(
        "restarted empty status",
        &restarted_status,
        &project_scope,
        &app_scope_id,
    );
    let restarted_context = fixture.run_context(&["context"], Some(&pane));
    assert!(restarted_context["master"].is_null(), "{restarted_context}");
    assert!(
        !restarted_context["tasks"]
            .as_array()
            .expect("restarted context lists tasks")
            .iter()
            .any(|task| task["id"] == "authority-clear-task"),
        "restart must preserve name ownership: {restarted_context}"
    );
    let retained = fixture.run_ok(&["task", "status", "authority-clear-task"], Some(&pane));
    assert_eq!(retained["owner_name"], "Jenny", "{retained}");
    assert_eq!(retained["owner"], "", "{retained}");
    assert_eq!(
        fixture.current_state_for(&fixture.root, &app_scope_id)["tasks"]["authority-clear-task"],
        task_before
    );
    assert!(
        restarted_context["inbox"]["messages"]
            .as_array()
            .expect("restarted context lists inbox messages")
            .iter()
            .any(|message| message["id"].as_str() == Some(message_id.as_str())),
        "restart must preserve the mailbox: {restarted_context}"
    );

    let published = fixture.run_ok(
        &[
            "board",
            "publish",
            "after-clear",
            "--title",
            "t",
            "--description",
            "d",
            "--delivery-condition",
            "c",
            "--test-condition",
            "t",
            "--priority",
            "p2",
        ],
        Some(&pane),
    );
    assert_eq!(published["task"]["status"], "pending");
    let rejected = fixture.command(
        &[
            "board",
            "invite",
            "after-clear",
            "--to",
            "other-peer",
            "--expected-revision",
            "1",
        ],
        Some(&pane),
    );
    assert!(
        !rejected.status.success(),
        "a cleared holder must lose master-only invitation authority"
    );
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("BOARD_MASTER_REQUIRED"));
}

/// B04: a blank approval is not authorization. Both promote and clear must
/// refuse it and leave the recorded authority unchanged.
#[test]
fn master_authority_rejects_blank_approval_without_state_change() {
    let (fixture, pane) = single_pane_fixture("collab-master-authority-b04");
    fixture.registered_context(&pane, true);

    let promote = fixture.command(&["master", "promote", "--approval", "   "], Some(&pane));
    assert!(
        !promote.status.success(),
        "blank promote approval must be rejected: stdout={} stderr={}",
        String::from_utf8_lossy(&promote.stdout),
        String::from_utf8_lossy(&promote.stderr)
    );
    let clear = fixture.command(&["master", "clear", "--approval", "  "], Some(&pane));
    assert!(
        !clear.status.success(),
        "blank clear approval must be rejected: stdout={} stderr={}",
        String::from_utf8_lossy(&clear.stdout),
        String::from_utf8_lossy(&clear.stderr)
    );

    let status = fixture.run_ok(&["master", "status"], Some(&pane));
    assert!(status["master"].is_null(), "{status}");
}

/// B10: the same stable authority state must be projected identically by
/// `master status`, `context`, `board show`, and the real dashboard HTTP GET.
/// The dashboard is a read-only loopback consumer: it must serve the current
/// grant holder while keeping the per-worker agent observation separate, and it
/// must not expose any authority write route.
#[test]
fn master_authority_dashboard_get_agrees_with_status_context_and_board() {
    let (fixture, pane) = single_pane_fixture("collab-master-authority-b10");
    let registered = fixture.registered_context(&pane, true);
    let worker_id = registered["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names the registered worker")
        .to_owned();
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated dashboard master",
        ],
        Some(&pane),
    );

    for id in ["dashboard-open", "dashboard-terminal"] {
        let published = fixture.run_ok(
            &[
                "board",
                "publish",
                id,
                "--title",
                id,
                "--description",
                "observer projection",
                "--delivery-condition",
                "public view reflects closure",
                "--test-condition",
                "HTTP and CLI agree",
            ],
            Some(&pane),
        );
        if id == "dashboard-terminal" {
            let revision = published["task"]["revision"].as_u64().unwrap().to_string();
            fixture.run_ok(
                &["board", "claim", id, "--expected-revision", &revision],
                Some(&pane),
            );
        }
    }

    let status = fixture.run_ok(&["master", "status"], Some(&pane));
    let context = fixture.run_context(&["context"], Some(&pane));
    let board = fixture.run_ok(&["board", "show"], Some(&pane));
    assert_eq!(status["master"]["worker_id"], worker_id, "{status}");
    assert_eq!(context["master"]["worker_id"], worker_id, "{context}");
    assert_eq!(board["master"]["worker_id"], worker_id, "{board}");
    // The board's master role is the authority projection, while the member
    // status stays a transport observation: a tmux pane is not an online agent.
    let master_rows: Vec<&Value> = board["workers"]
        .as_array()
        .expect("board lists workers")
        .iter()
        .filter(|worker| worker["role"] == "master")
        .collect();
    assert_eq!(master_rows.len(), 1, "exactly one master role: {board}");
    assert_eq!(master_rows[0]["id"], worker_id);
    assert_ne!(
        master_rows[0]["status"], "online",
        "a pane address must not make the agent online: {board}"
    );

    // The dashboard binds an OS-selected loopback port and prints its URL plus
    // the capability fragment. Parse that line, then drive the real HTTP GET
    // with the exact bearer capability the page would use.
    let mut child = fixture
        .configured_command(&["dashboard", "--port", "0"], None, "Codex TUI")
        .env("CODEX_SESSION_ID", &pane.session_anchor)
        .env("CODEX_THREAD_ID", &pane.thread_anchor)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn isolated dashboard");
    let stdout = child.stdout.take().expect("dashboard stdout is piped");
    let mut reader = BufReader::new(stdout);
    let mut announcement = String::new();
    reader
        .read_line(&mut announcement)
        .expect("dashboard announces its URL before serving");
    let announcement: Value =
        serde_json::from_str(announcement.trim()).expect("dashboard prints one JSON announcement");
    assert_eq!(announcement["read_only"], true, "{announcement}");
    let url = announcement["url"]
        .as_str()
        .expect("dashboard announces a URL")
        .to_owned();
    let (authority, capability) = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split_once("/#"))
        .expect("dashboard URL carries the loopback authority and capability fragment");
    let capability = capability.to_owned();
    let authority = authority.to_owned();

    let snapshot = dashboard_get(&authority, &capability, None);
    assert!(
        snapshot.starts_with("HTTP/1.1 200"),
        "authorized dashboard GET must succeed: {snapshot}"
    );
    let body = dashboard_body(&snapshot);
    let served: Value = serde_json::from_str(&body).expect("dashboard serves the board snapshot");
    assert_eq!(
        served["master"]["worker_id"], worker_id,
        "dashboard must project the same current grant holder: {served}"
    );
    let panel_master: Vec<&Value> = served["workers"]
        .as_array()
        .expect("dashboard lists workers")
        .iter()
        .filter(|worker| worker["role"] == "master")
        .collect();
    assert_eq!(panel_master.len(), 1, "{served}");
    assert_eq!(panel_master[0]["id"], worker_id);
    assert_ne!(
        panel_master[0]["status"], "online",
        "the panel must keep the agent observation separate from authority: {served}"
    );

    assert_eq!(served["tasks"].as_array().unwrap().len(), 2);
    let revision = served["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "dashboard-terminal")
        .unwrap()["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    let cancelled = fixture.run_ok(
        &[
            "board",
            "update",
            "dashboard-terminal",
            "--expected-revision",
            &revision,
            "--status",
            "cancelled",
        ],
        Some(&pane),
    );
    assert_eq!(cancelled["task"]["status"], "cancelled");
    let refreshed = dashboard_get(&authority, &capability, None);
    assert!(refreshed.starts_with("HTTP/1.1 200"));
    let refreshed: Value = serde_json::from_str(&dashboard_body(&refreshed)).unwrap();
    let cli = fixture.run_ok(&["board", "show"], Some(&pane));
    for view in [&refreshed, &cli] {
        let tasks = view["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1, "{view}");
        assert_eq!(tasks[0]["id"], "dashboard-open");
    }
    let history = fixture.run_ok(&["task", "status", "dashboard-terminal"], Some(&pane));
    assert_eq!(history["status"], "cancelled");

    // No dashboard route writes authority: the observation is GET-only and a
    // write method on the same path is refused rather than applied.
    let refused = dashboard_get(&authority, &capability, Some("POST"));
    assert!(
        !refused.starts_with("HTTP/1.1 200"),
        "the dashboard must not accept an authority write: {refused}"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = reader;
}

/// One dashboard request. `method` defaults to `GET`; the dashboard rejects any
/// method that is not registered.
fn dashboard_get(authority: &str, capability: &str, method: Option<&str>) -> String {
    let mut stream = TcpStream::connect(authority).expect("connect isolated dashboard");
    let method = method.unwrap_or("GET");
    let request = format!(
        "{method} /api/board HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {capability}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn dashboard_body(response: &str) -> String {
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default()
}

/// The accepted projection contract keeps `{project_scope, app_scope_id}` at
/// the top level of every authority-bearing public response, including Empty.
fn assert_scope(label: &str, value: &Value, project_scope: &str, app_scope_id: &str) {
    assert_eq!(
        value["scope"]["project_scope"], project_scope,
        "{label} must project the exact project scope: {value}"
    );
    assert_eq!(
        value["scope"]["app_scope_id"], app_scope_id,
        "{label} must project the exact app scope: {value}"
    );
}

/// Send one raw public request envelope to the isolated daemon socket and
/// return its parsed response. This is the same wire the CLI uses; the test
/// only bypasses the CLI so it can forge a token, scope, or binding tuple.
fn wire_request(socket: &Path, request: &Value) -> Value {
    let mut stream =
        std::os::unix::net::UnixStream::connect(socket).expect("connect isolated daemon socket");
    stream
        .write_all(serde_json::to_string(request).unwrap().as_bytes())
        .unwrap();
    stream.write_all(b"\n").unwrap();
    stream.flush().unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).expect("daemon emits one JSON response line")
}

/// Build the typed runtime context the CLI attaches, from the caller's own
/// context receipt, so forged variants differ only in the field under test.
fn runtime_context_from_receipt(receipt: &Value) -> Value {
    let binding = &receipt["binding"];
    json!({
        "agent_id": binding["agent_id"],
        "runtime_id": binding["runtime_id"],
        "appserver_id": binding["app_scope_id"],
        "binding_id": binding["binding_id"],
        "session_id": binding["session_id"],
        "native_thread_id": binding["native_thread_id"],
    })
}

/// Build a registration request for the public `Register` operation. The
/// fixture supplies its own token and a distinct tmux pane; the daemon owns the
/// current binding id.
fn tmux_register_request(
    worker_id: &str,
    token: &str,
    root: &Path,
    pane: &Pane,
    tmux_socket: &Path,
) -> Value {
    let observed = tmux(
        tmux_socket,
        &[
            "display-message",
            "-p",
            "-t",
            &pane.pane_id,
            "#{pid}\t#{session_id}\t#{pane_pid}",
        ],
    );
    let observed = String::from_utf8(observed.stdout).expect("tmux facts are UTF-8");
    let fields: Vec<_> = observed.trim().split('\t').collect();
    let server_pid: u32 = fields[0].parse().expect("observed server PID");
    let session_id = fields[1];
    let pane_pid: u32 = fields[2].parse().expect("observed pane PID");
    let canonical_root = std::fs::canonicalize(root)
        .expect("canonicalize fixture root")
        .to_string_lossy()
        .into_owned();
    json!({
        "op": "Register",
        "worker_id": worker_id,
        "token": token,
        "cwd": canonical_root,
        "candidates": {
            "tmux": {
                "endpoint": {
                    "socket_path": tmux_socket,
                    "server_pid": server_pid,
                    "tmux_session_id": session_id,
                    "pane_id": pane.pane_id,
                    "pane_pid": pane_pid,
                    "codex_session_id": pane.session_anchor,
                    "codex_thread_id": pane.thread_anchor,
                },
                "cwd": canonical_root,
            },
        },
        "project_context": {
            "app_scope_id": "appserver-cli",
            "canonical_root": canonical_root,
            "project_scope": canonical_root,
        },
    })
}

/// B06/B07: same-principal continuation keeps its current grant. A distinct
/// principal inherits nothing, and an old binding tuple cannot mutate it.
#[test]
fn master_authority_same_principal_recovery_and_new_principal_do_not_inherit() {
    let (mut fixture, first_pane) = single_pane_fixture("collab-master-authority-b06");
    let registered = fixture.registered_context(&first_pane, true);
    let worker_id = registered["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names the registered worker")
        .to_owned();
    let old_binding = registered["binding"].clone();
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated recovery master",
        ],
        Some(&first_pane),
    );

    // A second authenticated principal on its own pane is an independent peer.
    let created = tmux(
        &fixture.tmux_socket,
        &[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            "collab-master-authority-b06-second",
            "sleep 600",
        ],
    );
    let second_pane_id = String::from_utf8(created.stdout).unwrap().trim().to_owned();
    let stable = tmux(
        &fixture.tmux_socket,
        &[
            "display-message",
            "-p",
            "-t",
            &second_pane_id,
            "#{session_id}",
        ],
    );
    fixture
        .owned_sessions
        .push(String::from_utf8(stable.stdout).unwrap().trim().to_owned());
    let second_pane = Pane {
        pane_id: second_pane_id.clone(),
        session_anchor: format!("session-{second_pane_id}"),
        thread_anchor: format!("thread-{second_pane_id}"),
        server_pid: first_pane.server_pid,
    };
    let second_registered = fixture.registered_context(&second_pane, true);
    let socket = fixture.host_state.join("server.sock");
    assert_ne!(second_registered["identity"]["worker_id"], worker_id);
    assert_eq!(
        second_registered["identity"]["role"], "worker",
        "{second_registered}"
    );
    let second_status = fixture.run_ok(&["master", "status"], Some(&second_pane));
    assert_eq!(
        second_status["master"]["worker_id"], worker_id,
        "a new principal must not inherit authority by registering a pane: {second_status}"
    );

    // Same-principal recovery moves to a new pane. The grant follows only the
    // exact current principal and binding.
    let created = tmux(
        &fixture.tmux_socket,
        &[
            "split-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            &first_pane.pane_id,
            "sleep 600",
        ],
    );
    let recovery_pane_id = String::from_utf8(created.stdout).unwrap().trim().to_owned();
    assert!(!recovery_pane_id.is_empty(), "{recovery_pane_id}");
    let recovery_pane = Pane {
        pane_id: recovery_pane_id.clone(),
        session_anchor: format!("session-{recovery_pane_id}"),
        thread_anchor: format!("thread-{recovery_pane_id}"),
        server_pid: first_pane.server_pid,
    };
    let identity_file = fixture
        .host_state
        .join("identities")
        .join(&worker_id)
        .join("identity.json");
    let identity: Value =
        serde_json::from_slice(&std::fs::read(&identity_file).expect("read own identity"))
            .expect("identity is JSON");
    let token = identity["token"]
        .as_str()
        .expect("identity carries the credential")
        .to_owned();
    let mut recovery_request = tmux_register_request(
        &worker_id,
        &token,
        &fixture.root,
        &recovery_pane,
        &fixture.tmux_socket,
    );
    recovery_request["project_context"]["runtime_context"] =
        runtime_context_from_receipt(&registered);
    let recovered = wire_request(&socket, &recovery_request);
    assert_eq!(
        recovered["ok"], true,
        "same-principal recovery must succeed: {recovered}"
    );
    let mapped = fixture.current_mapping_for(&fixture.root, "appserver-cli", &worker_id);
    let new_binding = mapped["binding"].clone();
    let credential: Value =
        serde_json::from_slice(&std::fs::read(&identity_file).unwrap()).unwrap();
    for field in [
        "agent_id",
        "runtime_id",
        "binding_id",
        "session_id",
        "native_thread_id",
    ] {
        assert_eq!(credential["runtime"][field], new_binding[field], "{field}");
    }
    assert_eq!(new_binding["session_id"], recovery_pane.session_anchor);
    assert_eq!(new_binding["native_thread_id"], recovery_pane.thread_anchor);
    assert_eq!(
        mapped["transport"]["tmux_endpoint"]["pane_id"],
        recovery_pane.pane_id
    );
    let new_runtime = runtime_context_from_receipt(&json!({"binding": new_binding}));
    let new_route = json!({
        "app_scope_id": new_binding["app_scope_id"],
        "canonical_root": new_binding["project_scope"],
        "project_scope": new_binding["project_scope"],
        "runtime_context": new_runtime,
    });
    let recovered_context = wire_request(
        &socket,
        &json!({
            "op": "Context", "worker_id": worker_id, "token": token,
            "project_context": new_route,
        }),
    );
    assert_eq!(recovered_context["ok"], true, "{recovered_context}");
    assert_eq!(
        recovered_context["identity"]["worker_id"], worker_id,
        "recovery must keep the principal: {recovered_context}"
    );
    for field in [
        "agent_id",
        "runtime_id",
        "binding_id",
        "session_id",
        "native_thread_id",
    ] {
        assert_eq!(
            recovered_context["binding"][field], new_binding[field],
            "{field}"
        );
    }
    let recovered_status = wire_request(
        &socket,
        &json!({
            "op": "MasterStatus", "project_context": new_route,
        }),
    );
    assert_eq!(
        recovered_status["master"]["worker_id"], worker_id,
        "same-principal recovery must reissue the grant: {recovered_status}"
    );

    let current_state = fixture.current_state_for(&fixture.root, "appserver-cli");
    // The previous session/thread tuple remains stale with a valid credential.
    let stale = wire_request(
        &socket,
        &json!({
            "op": "MasterClear",
            "worker_id": worker_id,
            "token": token,
            "approval": "stale binding clear",
            "project_context": {
                "app_scope_id": old_binding["app_scope_id"],
                "canonical_root": old_binding["project_scope"],
                "project_scope": old_binding["project_scope"],
                "runtime_context": {
                    "agent_id": old_binding["agent_id"],
                    "runtime_id": old_binding["runtime_id"],
                    "appserver_id": old_binding["app_scope_id"],
                    "binding_id": old_binding["binding_id"],
                    "session_id": old_binding["session_id"],
                    "native_thread_id": old_binding["native_thread_id"],
                },
            },
        }),
    );
    assert_eq!(
        stale["ok"], false,
        "old binding tuple must be rejected: {stale}"
    );
    let unchanged = wire_request(
        &socket,
        &json!({
            "op": "MasterStatus", "project_context": new_route,
        }),
    );
    assert_eq!(
        unchanged["master"]["worker_id"], worker_id,
        "a stale binding refusal must not change authority: {unchanged}"
    );
    for field in [
        "worker_id",
        "grant_id",
        "grant_generation",
        "binding_id",
        "assigned_by",
        "assigned_ms",
        "approval",
        "boundary",
        "scope",
    ] {
        assert_eq!(
            unchanged["master"][field], recovered_status["master"][field],
            "{field}"
        );
    }
    assert_eq!(
        fixture.current_state_for(&fixture.root, "appserver-cli")["tasks"],
        current_state["tasks"]
    );
    assert_eq!(
        fixture.current_mapping_for(&fixture.root, "appserver-cli", &worker_id),
        mapped
    );
}

/// B09/B15: real routed mutations preserve other project/app scopes, and
/// concurrent clear/promote observations are complete committed states.
fn scope_registration_participant(
    fixture: &Fixture,
    pane: &Pane,
    root: &Path,
    app: &str,
    worker: &str,
    token: &str,
    socket: &Path,
) -> Value {
    let mut register = tmux_register_request(worker, token, root, pane, socket);
    register["project_context"]["app_scope_id"] = json!(app);
    let registered = wire_request(&fixture.host_state.join("server.sock"), &register);
    assert_eq!(registered["ok"], true, "{registered}");
    registered
}

struct ScopeSession {
    socket: PathBuf,
    stable_id: String,
}

impl Drop for ScopeSession {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .arg("-S")
            .arg(&self.socket)
            .args(["kill-session", "-t", &self.stable_id])
            .output();
    }
}

#[test]
fn master_authority_scope_isolation_and_concurrent_mutations() {
    let (fixture, first_pane) = single_pane_fixture("collab-master-scopes");
    let initial = fixture.registered_context(&first_pane, true);
    let socket = fixture.host_state.join("server.sock");
    let other_root = fixture.root.join("other-project");
    seed_baseline(&other_root);
    let mut routes = Vec::new();
    let mut owned_scope_sessions = Vec::new();
    for (index, (root, app)) in [
        (&fixture.root, "authority-app-a"),
        (&fixture.root, "authority-app-b"),
        (&other_root, "authority-app-a"),
    ]
    .into_iter()
    .enumerate()
    {
        let label = format!("authority-scope-{index}");
        // App scopes share the same project prefix and fixed display slots.
        // Their independent endpoints need separate owned tmux sockets.
        let scope_socket = fixture.root.join(format!("scope-{index}.sock"));
        let created = tmux(
            &scope_socket,
            &[
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-s",
                &label,
                "-c",
                root.to_str().unwrap(),
                "sleep 600",
            ],
        );
        let pane_id = String::from_utf8(created.stdout).unwrap().trim().to_owned();
        let facts = tmux(
            &scope_socket,
            &[
                "display-message",
                "-p",
                "-t",
                &pane_id,
                "#{pid}\t#{session_id}",
            ],
        );
        let facts = String::from_utf8(facts.stdout).unwrap();
        let (server_pid, stable_id) = facts.trim().split_once('\t').unwrap();
        owned_scope_sessions.push(ScopeSession {
            socket: scope_socket.clone(),
            stable_id: stable_id.into(),
        });
        let pane = Pane {
            server_pid: server_pid.parse().unwrap(),
            session_anchor: format!("session-{pane_id}"),
            thread_anchor: format!("thread-{pane_id}"),
            pane_id,
        };
        let worker = format!("scope-worker-{index}");
        let token = format!("scope-token-{index}");
        let registered = scope_registration_participant(
            &fixture,
            &pane,
            root,
            app,
            &worker,
            &token,
            &scope_socket,
        );
        assert_eq!(registered["ok"], true, "scope registration: {registered}");
        let binding = fixture.current_mapping_for(root, app, &worker)["binding"].clone();
        let runtime = runtime_context_from_receipt(&json!({"binding": binding}));
        let route = json!({
            "app_scope_id": binding["app_scope_id"],
            "canonical_root": binding["project_scope"],
            "project_scope": binding["project_scope"], "runtime_context": runtime,
        });
        let promote = json!({"op":"MasterPromote", "worker_id":worker,
            "token":token, "approval":"approved scope fixture", "project_context":route});
        if index == 1 {
            // Fail only source State publication after the cross-app map
            // commit. The public retry must finish the remaining effect.
            let source_path = fixture.project_state_for(&fixture.root, "authority-app-a");
            let held_path = fixture.root.join("source-grant-state-held.json");
            std::fs::rename(&source_path, &held_path).unwrap();
            std::fs::create_dir(&source_path).unwrap();
            let failed = wire_request(&socket, &promote);
            let pending_map: Value = serde_json::from_slice(
                &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
            )
            .unwrap();
            // Restore before assertions so even an unexpected receipt keeps
            // fixture-owned storage available for normal shutdown.
            std::fs::remove_dir(&source_path).unwrap();
            std::fs::rename(&held_path, &source_path).unwrap();
            assert_eq!(failed["ok"], false, "{failed}");
            assert!(
                failed["error"]
                    .as_str()
                    .unwrap()
                    .contains("PENDING_EFFECT_CROSS_SCOPE_SOURCE_FAILED"),
                "{failed}"
            );
            assert!(
                pending_map["effects"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|effect| {
                        effect["app_scope_id"] == "authority-app-b"
                            && effect["target_worker_id"] == worker
                    }),
                "source failure must retain the bounded target intent"
            );
            let source_state = fixture.current_state_for(&fixture.root, "authority-app-a");
            let project = route["project_scope"].as_str().unwrap();
            assert!(
                source_state["global"]["projects"][project]["master_grants"]
                    .as_object()
                    .unwrap()
                    .values()
                    .any(|grant| grant["agent_id"] == "scope-worker-0"),
                "failed source commit must keep the old grant until retry"
            );
        }
        let result = wire_request(&socket, &promote);
        assert_eq!(result["ok"], true, "scope promote: {result}");
        if index == 1 {
            let map: Value = serde_json::from_slice(
                &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
            )
            .unwrap();
            assert!(
                map["effects"].as_array().unwrap().iter().all(|effect| {
                    effect["app_scope_id"] != "authority-app-b"
                        || effect["target_worker_id"] != worker
                }),
                "successful retry must complete its intent"
            );
        }
        routes.push((worker, token, route, promote));
    }
    // Jenny is unique across app scopes in a project. The second promotion
    // displaced the first scope's holder; a different project stays isolated.
    let (displaced_worker, displaced_token, displaced_route, _) = &routes[0];
    let source_state = fixture.current_state_for(&fixture.root, "authority-app-a");
    let grants = &source_state["global"]["projects"]
        [displaced_route["project_scope"].as_str().unwrap()]["master_grants"];
    assert!(grants.is_object(), "typed grants must be present");
    assert!(
        grants
            .as_object()
            .unwrap()
            .values()
            .all(|grant| { grant["agent_id"] != *displaced_worker }),
        "displaced holder still has a typed grant"
    );
    let old_status = wire_request(
        &socket,
        &json!({"op":"MasterStatus", "project_context":displaced_route}),
    );
    assert!(old_status["master"].is_null(), "{old_status}");
    let denied = wire_request(
        &socket,
        &json!({"op":"MasterWakeHold",
        "worker_id":displaced_worker, "token":displaced_token,
        "reason":"displaced holder cannot hold", "ttl_seconds":60,
        "project_context":displaced_route}),
    );
    assert_eq!(denied["ok"], false, "{denied}");
    let source_after = fixture.current_state_for(&fixture.root, "authority-app-a");
    assert!(source_state["master_wake"].is_object());
    assert_eq!(
        source_state["master_wake"]["wake_hold"],
        source_after["master_wake"]["wake_hold"]
    );
    for (worker, token, route, _) in &routes[1..] {
        let held = wire_request(
            &socket,
            &json!({"op":"MasterWakeHold",
            "worker_id":worker, "token":token, "reason":"current master allowed",
            "ttl_seconds":60, "project_context":route}),
        );
        assert_eq!(held["ok"], true, "{held}");
        let root = Path::new(route["canonical_root"].as_str().unwrap());
        let held_state = fixture.current_state_for(root, route["app_scope_id"].as_str().unwrap());
        assert_eq!(
            held_state["master_wake"]["wake_hold"]["reason"],
            "current master allowed"
        );
        let released = wire_request(
            &socket,
            &json!({"op":"MasterWakeRelease",
            "worker_id":worker, "token":token, "project_context":route}),
        );
        assert_eq!(released["ok"], true, "{released}");
    }
    let displaced = wire_request(
        &socket,
        &json!({"op":"Context", "worker_id":displaced_worker,
            "token":displaced_token,"project_context":displaced_route}),
    );
    assert_eq!(displaced["ok"], false, "{displaced}");
    assert!(displaced["error"]
        .as_str()
        .unwrap()
        .contains("IDENTITY_MAP_NOT_FOUND"));
    for (worker, token, route, _) in &routes[1..] {
        for request in [
            json!({"op":"MasterStatus", "project_context":route}),
            json!({"op":"Context", "worker_id":worker,"token":token,"project_context":route}),
            json!({"op":"BoardShow", "project_context":route}),
        ] {
            let result = wire_request(&socket, &request);
            assert_eq!(result["ok"], true, "{result}");
            assert_eq!(result["scope"]["project_scope"], route["project_scope"]);
            assert_eq!(result["scope"]["app_scope_id"], route["app_scope_id"]);
            assert_eq!(result["master"]["worker_id"], *worker, "{result}");
        }
    }
    let (worker, token, route, promote) = &routes[1];
    let clear = json!({"op":"MasterClear", "worker_id":worker, "token":token,
        "approval":"approved concurrent fixture", "project_context":route});
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let children: Vec<_> = [clear.clone(), promote.clone()]
        .into_iter()
        .map(|request| {
            let socket = socket.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                wire_request(&socket, &request)
            })
        })
        .collect();
    barrier.wait();
    for _ in 0..12 {
        let observed = wire_request(
            &socket,
            &json!({"op":"MasterStatus", "project_context":route}),
        );
        assert_eq!(observed["ok"], true, "{observed}");
        assert_eq!(observed["scope"]["project_scope"], route["project_scope"]);
        assert_eq!(observed["scope"]["app_scope_id"], route["app_scope_id"]);
        assert!(
            observed["master"].is_null() || observed["master"]["worker_id"] == *worker,
            "partial or foreign authority: {observed}"
        );
    }
    for child in children {
        let result = child.join().unwrap();
        assert_eq!(result["ok"], true, "{result}");
    }
    assert_eq!(wire_request(&socket, &clear)["ok"], true);
    for (index, (worker, _, route, _)) in routes.iter().enumerate() {
        let status = wire_request(
            &socket,
            &json!({"op":"MasterStatus", "project_context":route}),
        );
        assert_eq!(status["ok"], true, "{status}");
        if index <= 1 {
            assert!(status["master"].is_null(), "{status}");
        } else {
            assert_eq!(status["master"]["worker_id"], *worker, "{status}");
        }
    }
    let original = wire_request(
        &socket,
        &json!({"op":"MasterStatus",
        "project_context": {
            "app_scope_id": initial["scope"]["app_scope_id"],
            "canonical_root": initial["scope"]["project_scope"],
            "project_scope": initial["scope"]["project_scope"],
        }}),
    );
    assert_eq!(original["ok"], true, "{original}");
    assert!(original["master"].is_null());
    assert_eq!(original["scope"], initial["scope"]);
}

/// B05: an authenticated mutation over the public socket must reject a wrong
/// token, a mismatched project route, a mismatched app scope, and a wrong
/// binding id. Every rejection leaves the recorded authority and the
/// business state exactly as they were, and the raw token is never echoed.
#[test]
fn master_authority_public_wire_rejects_bad_token_scope_and_binding_without_state_change() {
    let (fixture, pane) = single_pane_fixture("collab-master-authority-b05");
    let receipt = fixture.registered_context(&pane, true);
    let worker_id = receipt["identity"]["worker_id"]
        .as_str()
        .expect("context receipt names the registered worker")
        .to_owned();
    // The public display never carries the credential, so the fixture reads its
    // own persisted identity to build the real (and forged) wire requests.
    let identity_file = fixture
        .host_state
        .join("identities")
        .join(&worker_id)
        .join("identity.json");
    let identity: Value =
        serde_json::from_slice(&std::fs::read(&identity_file).unwrap_or_else(|error| {
            panic!("read own identity {}: {error}", identity_file.display())
        }))
        .expect("persisted identity is JSON");
    let token = identity["token"]
        .as_str()
        .expect("persisted identity carries the credential")
        .to_owned();
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated wire master",
        ],
        Some(&pane),
    );
    fixture.run_ok(
        &[
            "task",
            "register",
            "wire-preserved-task",
            "--next",
            "stay put",
        ],
        Some(&pane),
    );

    let socket = fixture.host_state.join("server.sock");
    let runtime_context = runtime_context_from_receipt(&receipt);
    let context_for = |canonical_root: &str, app_scope: &str, runtime: Value| {
        json!({
            "app_scope_id": app_scope,
            "canonical_root": canonical_root,
            "project_scope": canonical_root,
            "runtime_context": runtime,
        })
    };
    let root = std::fs::canonicalize(&fixture.root)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let app_scope = receipt["binding"]["app_scope_id"]
        .as_str()
        .expect("receipt names the app scope")
        .to_owned();
    // The opaque token is the one credential the fixture must forge; every
    // other field is the caller's real context.
    let wrong_token_runtime = runtime_context.clone();
    let cases: Vec<(&str, &[&str], Value)> = vec![
        (
            "wrong token",
            &["token mismatch"],
            json!({
                "op": "MasterClear",
                "worker_id": worker_id,
                "token": "not-the-real-token",
                "approval": "forged clear",
                "project_context": context_for(&root, &app_scope, wrong_token_runtime),
            }),
        ),
        (
            "mismatched project route",
            &["RUNTIME_BINDING_REJECTED", "PROJECT_SCOPE_UNKNOWN"],
            json!({
                "op": "MasterClear",
                "worker_id": worker_id,
                "token": token,
                "approval": "forged clear",
                "project_context": context_for(
                    &format!("{root}/other-project"),
                    &app_scope,
                    runtime_context.clone(),
                ),
            }),
        ),
        (
            "mismatched app scope",
            &[
                "RUNTIME_BINDING_REJECTED",
                "PROJECT_SCOPE_UNKNOWN",
                "PROJECT_CONTEXT_INVALID",
            ],
            json!({
                "op": "MasterClear",
                "worker_id": worker_id,
                "token": token,
                "approval": "forged clear",
                "project_context": context_for(
                    &root,
                    "appserver-other-scope",
                    runtime_context.clone(),
                ),
            }),
        ),
        (
            "wrong current binding",
            &["SESSION_THREAD_BINDING_MISMATCH"],
            json!({
                "op": "MasterClear",
                "worker_id": worker_id,
                "token": token,
                "approval": "forged clear",
                "project_context": {
                    "app_scope_id": app_scope,
                    "canonical_root": root,
                    "project_scope": root,
                    "runtime_context": {
                        "agent_id": runtime_context["agent_id"],
                        "runtime_id": runtime_context["runtime_id"],
                        "appserver_id": runtime_context["appserver_id"],
                        "binding_id": "binding-not-the-current-owner",
                        "session_id": runtime_context["session_id"],
                        "native_thread_id": runtime_context["native_thread_id"],
                    },
                },
            }),
        ),
    ];

    for (label, expected_errors, request) in cases {
        let response = wire_request(&socket, &request);
        assert_eq!(
            response["ok"], false,
            "{label} must be rejected: {response}"
        );
        assert!(
            response["error"]
                .as_str()
                .is_some_and(|error| expected_errors.iter().any(|e| error.contains(e))),
            "{label} must fail at one of {expected_errors:?}: {response}"
        );
        let rendered = response.to_string();
        assert!(
            !rendered.contains("not-the-real-token"),
            "{label} must not echo the credential: {response}"
        );
    }

    // The holder is unchanged and the business state is intact.
    let status = fixture.run_ok(&["master", "status"], Some(&pane));
    assert_eq!(
        status["master"]["worker_id"], worker_id,
        "a rejected mutation must not change the holder: {status}"
    );
    let task = fixture.run_ok(&["task", "status", "wire-preserved-task"], Some(&pane));
    assert_eq!(task["owner"], worker_id, "{task}");
    assert_eq!(task["next_step"], "stay put", "{task}");
}

fn run_pane_without_thread(fixture: &Fixture, args: &[&str], pane: &Pane) -> Value {
    let mut command = fixture.configured_command(args, Some(pane), "Codex TUI");
    command.env_remove("CODEX_THREAD_ID");
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "collab {:?} failed: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("collab CLI emits JSON")
}

fn registered_pane_master_without_thread(fixture: &Fixture, pane: &Pane) -> Value {
    let label = format!(
        "pane-patrol-master-{}-{}",
        pane.server_pid,
        pane.pane_id.trim_start_matches('%')
    );
    let mut command =
        fixture.configured_command(&["context", "--op", &label], Some(pane), "Codex TUI");
    command.env_remove("CODEX_THREAD_ID");
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "pane-only participant setup failed: {} {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(value["result"]["outcome"], "completed", "{value}");
    value["result"]["snapshot"].clone()
}

fn context_pane_without_thread(fixture: &Fixture, pane: &Pane) -> Value {
    let mut command = fixture.configured_command(&["context"], Some(pane), "Codex TUI");
    command.env_remove("CODEX_THREAD_ID");
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "pane-only context failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    value
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .cloned()
        .unwrap_or(value)
}

#[test]
fn tmux_attached_master_rearms_cancelled_project_patrol_on_project_work() {
    let (fixture, pane) = single_pane_fixture("patrol-cancel-rearm");
    let registered = registered_pane_master_without_thread(&fixture, &pane);
    let worker = registered["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        registered["identity"]["runtime"]["native_thread_id"].is_null(),
        "the fixture must register a tmux carrier without a native thread: {registered}"
    );

    run_pane_without_thread(
        &fixture,
        &[
            "master",
            "promote",
            "--approval",
            "fixture user assigns the tmux session project master",
        ],
        &pane,
    );
    let armed = run_pane_without_thread(
        &fixture,
        &[
            "notify",
            "subscribe",
            "--event",
            "master-idle",
            "--subject",
            "project-patrol",
            "--every-ms",
            "900000",
            "--repeat-count",
            "100",
            "--ttl-seconds",
            "604800",
        ],
        &pane,
    );
    let patrol_id = armed["subscription"]["id"].as_str().unwrap().to_owned();
    assert_eq!(armed["subscription"]["worker_id"], worker, "{armed}");
    assert_eq!(armed["subscription"]["method"], "tmux", "{armed}");
    assert_eq!(
        armed["subscription"]["target"], pane.pane_id,
        "a tmux carrier without a native thread keeps the pane id as its patrol target: {armed}"
    );

    run_pane_without_thread(
        &fixture,
        &[
            "ceo",
            "promote",
            "--approval",
            "fixture user promotes the master to record project progress",
        ],
        &pane,
    );
    let progress = run_pane_without_thread(
        &fixture,
        &[
            "ceo",
            "reply",
            "--subject",
            "project-patrol-progress",
            "--urgent",
            "no actionable project work remains",
        ],
        &pane,
    );
    assert_eq!(progress["recorded"], true, "{progress}");

    let cancelled = run_pane_without_thread(
        &fixture,
        &[
            "notify",
            "cancel",
            "--event",
            "master-idle",
            "--reason",
            "fixture no actionable project work",
            "--resume-condition",
            "new project work or a durable wait release",
        ],
        &pane,
    );
    assert_eq!(
        cancelled["subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "{cancelled}"
    );
    assert_eq!(cancelled["status"], "cancelled", "{cancelled}");

    run_pane_without_thread(
        &fixture,
        &[
            "task",
            "register",
            "TMUX-PATROL-WORK",
            "--feature",
            "tmux-patrol-resource",
        ],
        &pane,
    );
    let rearmed = run_pane_without_thread(&fixture, &["notify", "status"], &pane);
    assert_eq!(
        rearmed["project_patrol"]["status"], "armed",
        "the tmux-attached master must rearm on new project work: {rearmed}"
    );
    assert_eq!(
        rearmed["project_patrol"]["subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "{rearmed}"
    );
}

#[test]
fn cancelled_project_patrol_follows_endpoint_move_after_context_recovery_and_rearms() {
    let (fixture, first) = single_pane_fixture("patrol-cancel-endpoint-move");
    let registered = registered_pane_master_without_thread(&fixture, &first);
    let worker = registered["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        registered["identity"]["runtime"]["native_thread_id"].is_null(),
        "the fixture must register a tmux carrier without a native thread: {registered}"
    );

    run_pane_without_thread(
        &fixture,
        &[
            "master",
            "promote",
            "--approval",
            "fixture user assigns the tmux session project master",
        ],
        &first,
    );
    let armed = run_pane_without_thread(
        &fixture,
        &[
            "notify",
            "subscribe",
            "--event",
            "master-idle",
            "--subject",
            "project-patrol",
            "--every-ms",
            "900000",
            "--repeat-count",
            "100",
            "--ttl-seconds",
            "604800",
        ],
        &first,
    );
    let patrol_id = armed["subscription"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        armed["subscription"]["target"], first.pane_id,
        "initial tmux patrol follows pane A: {armed}"
    );

    run_pane_without_thread(
        &fixture,
        &[
            "ceo",
            "promote",
            "--approval",
            "fixture user promotes the master to record project progress",
        ],
        &first,
    );
    let progress = run_pane_without_thread(
        &fixture,
        &[
            "ceo",
            "reply",
            "--subject",
            "project-patrol-endpoint-progress",
            "--urgent",
            "no actionable project work remains",
        ],
        &first,
    );
    assert_eq!(progress["recorded"], true, "{progress}");

    let cancelled = run_pane_without_thread(
        &fixture,
        &[
            "notify",
            "cancel",
            "--event",
            "master-idle",
            "--reason",
            "fixture no actionable project work before moving panes",
            "--resume-condition",
            "new project work or a durable wait release",
        ],
        &first,
    );
    assert_eq!(
        cancelled["subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "{cancelled}"
    );
    assert_eq!(cancelled["status"], "cancelled", "{cancelled}");

    let created = tmux(
        &fixture.tmux_socket,
        &[
            "split-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            &first.pane_id,
            "sleep 600",
        ],
    );
    let pane_id = String::from_utf8(created.stdout).unwrap().trim().to_owned();
    let second = Pane {
        pane_id,
        server_pid: first.server_pid,
        session_anchor: first.session_anchor.clone(),
        thread_anchor: first.thread_anchor.clone(),
    };

    let restored = context_pane_without_thread(&fixture, &second);
    assert_eq!(restored["identity"]["worker_id"], worker, "{restored}");
    assert_eq!(
        restored["binding"]["tmux_endpoint"]["pane_id"], second.pane_id,
        "context recovery moves the binding to pane B: {restored}"
    );
    let after_restore = run_pane_without_thread(&fixture, &["notify", "status"], &second);
    assert_eq!(
        after_restore["project_patrol"]["status"], "cancelled",
        "ordinary context recovery must not rearm the cancelled patrol: {after_restore}"
    );
    assert_eq!(
        after_restore["project_patrol"]["target"], second.pane_id,
        "the cancelled patrol must follow the current pane after context recovery: {after_restore}"
    );

    run_pane_without_thread(
        &fixture,
        &[
            "task",
            "register",
            "TMUX-PATROL-MOVED-WORK",
            "--feature",
            "tmux-patrol-endpoint-move",
        ],
        &second,
    );
    let rearmed = run_pane_without_thread(&fixture, &["notify", "status"], &second);
    assert_eq!(
        rearmed["project_patrol"]["status"], "armed",
        "the moved master must rearm on new project work: {rearmed}"
    );
    assert_eq!(
        rearmed["project_patrol"]["subscription_id"].as_str(),
        Some(patrol_id.as_str()),
        "{rearmed}"
    );
    assert_eq!(
        rearmed["project_patrol"]["target"], second.pane_id,
        "rearmed patrol stays at the current pane B: {rearmed}"
    );
}
