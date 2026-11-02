include!("support/context_operation_fixture.rs");

#[test]
fn public_master_commands_discover_send_consume_and_reply_across_projects() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, source_pane) =
        Fixture::isolated_with_pane_command("master-cli-roundtrip", "sleep 600");
    let source_root = fixture.root.canonicalize().unwrap();
    let target_root = create_external_project(&fixture, "roundtrip-target");
    let target_pane = create_session(&fixture, "roundtrip-target");
    let source = register_context(&fixture, &source_root, &source_pane, "source", false);
    let target = register_context(&fixture, &target_root, &target_pane, "target", false);
    let (source_id, _) = identity(&source);
    let (target_id, _) = identity(&target);
    for (root, pane) in [(&source_root, &source_pane), (&target_root, &target_pane)] {
        run_ok_at(
            &fixture,
            root,
            pane,
            &[
                "master",
                "promote",
                "--approval",
                "user approved isolated roundtrip master",
            ],
            false,
        );
    }
    let status = run_ok_at(
        &fixture,
        &target_root,
        &source_pane,
        &["master", "status"],
        false,
    );
    assert_eq!(status["master"]["worker_id"], target_id, "{status}");
    let sent = run_ok_at(
        &fixture,
        &source_root,
        &source_pane,
        &[
            "master",
            "send",
            "--project",
            target_root.to_str().unwrap(),
            "--to",
            &target_id,
            "--subject",
            "action",
            "return the result",
        ],
        false,
    );
    assert_eq!(sent["durable"], true, "{sent}");
    let received = run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &["recv", "--timeout", "0"],
        false,
    );
    let request = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == sent["msg_id"])
        .expect("request consumed");
    assert_eq!(request["id"], sent["msg_id"], "{received}");
    assert_eq!(
        request["from"],
        format!("{source_id}@{}", source_root.display())
    );
    let reply = run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &[
            "master",
            "send",
            "--project",
            source_root.to_str().unwrap(),
            "--to",
            &source_id,
            "--subject",
            "result",
            "verified result",
        ],
        false,
    );
    let returned = run_ok_at(
        &fixture,
        &source_root,
        &source_pane,
        &["recv", "--timeout", "0"],
        false,
    );
    let result = returned["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == reply["msg_id"])
        .expect("reply consumed");
    assert_eq!(result["body"], "verified result");
    assert_eq!(
        result["from"],
        format!("{target_id}@{}", target_root.display())
    );
    run_ok_at(
        &fixture,
        &source_root,
        &source_pane,
        &[
            "master",
            "clear",
            "--approval",
            "user approved isolated source clear",
        ],
        false,
    );
    let rejected = command_at(
        &fixture,
        &source_root,
        &source_pane,
        &[
            "master",
            "send",
            "--project",
            target_root.to_str().unwrap(),
            "--to",
            &target_id,
            "--subject",
            "unauthorized",
            "must not enqueue",
        ],
        false,
    )
    .output()
    .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("CROSS_PROJECT_SEND_REJECTED"));
    let empty = run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &["recv", "--timeout", "0"],
        false,
    );
    assert!(
        empty["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message["subject"] != "unauthorized"),
        "rejected sender must not enqueue its message: {empty}"
    );
    drop(fixture);
    std::fs::remove_dir_all(target_root).unwrap();
}

fn parse_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("collab CLI emits JSON")
}

fn create_session(fixture: &Fixture, label: &str) -> Pane {
    let output = Command::new("tmux")
        .args([
            "-S",
            fixture.tmux_socket.to_str().unwrap(),
            "new-session",
            "-d",
            "-s",
            label,
            "sleep 600",
        ])
        .output()
        .expect("create owned tmux session");
    assert!(
        output.status.success(),
        "create owned tmux session failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fixture.track_tmux_session(label);
    let pane_id = String::from_utf8(
        Command::new("tmux")
            .args([
                "-S",
                fixture.tmux_socket.to_str().unwrap(),
                "display-message",
                "-p",
                "-t",
                label,
                "#{pane_id}",
            ])
            .output()
            .expect("read owned pane id")
            .stdout,
    )
    .expect("pane id is UTF-8")
    .trim()
    .to_owned();
    let server_pid = String::from_utf8(
        Command::new("tmux")
            .args([
                "-S",
                fixture.tmux_socket.to_str().unwrap(),
                "display-message",
                "-p",
                "#{pid}",
            ])
            .output()
            .expect("read owned tmux server pid")
            .stdout,
    )
    .expect("tmux server pid is UTF-8")
    .trim()
    .parse::<u32>()
    .expect("tmux server pid is numeric");
    let pane = Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: format!("session-{pane_id}"),
        thread_anchor: format!("thread-{pane_id}"),
    };
    fixture.start_pane_runner_on_socket(&fixture.tmux_socket, &pane.pane_id, "sleep 600");
    pane
}

fn create_external_project(fixture: &Fixture, name: &str) -> PathBuf {
    let root = fixture
        .root
        .parent()
        .expect("fixture root has a parent")
        .join(format!(
            "{}-{name}",
            fixture
                .root
                .file_name()
                .expect("fixture root has a file name")
                .to_string_lossy()
        ));
    std::fs::create_dir_all(root.join(".agent-collab")).expect("create isolated project baseline");
    std::fs::canonicalize(root).expect("canonicalize isolated project")
}

fn command_at(
    fixture: &Fixture,
    project_root: &Path,
    pane: &Pane,
    args: &[&str],
    no_native_identity: bool,
) -> Command {
    let mut command = fixture.configured_command(args, Some(pane));
    command.current_dir(project_root);
    if no_native_identity {
        command
            .env("CODEX_SESSION_ID", &pane.session_anchor)
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CODEX_THREAD");
    }
    command
}

fn run_ok_at(
    fixture: &Fixture,
    project_root: &Path,
    pane: &Pane,
    args: &[&str],
    no_native_identity: bool,
) -> Value {
    let output = command_at(fixture, project_root, pane, args, no_native_identity)
        .output()
        .expect("run public collab CLI in isolated project");
    assert!(
        output.status.success(),
        "collab {args:?} failed in {}: stdout={} stderr={}",
        project_root.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_json(&output.stdout)
}

fn register_context(
    fixture: &Fixture,
    project_root: &Path,
    pane: &Pane,
    operation_id: &str,
    no_native_identity: bool,
) -> Value {
    let result = fixture
        .root
        .join(format!("{operation_id}.participant-result"));
    let error = fixture
        .root
        .join(format!("{operation_id}.participant-error"));
    let done = fixture
        .root
        .join(format!("{operation_id}.participant-done"));
    let unset = if no_native_identity {
        "unset CODEX_SESSION_ID CODEX_THREAD_ID CODEX_THREAD\n"
    } else {
        ""
    };
    let provide = if no_native_identity {
        format!(
            " --provide {}",
            shell_quote(&json!({"session_id": pane.session_anchor}).to_string())
        )
    } else {
        String::new()
    };
    let script = format!(
        "set -e\ncd {}\n{}\"$COLLAB_BINARY\" context --op {}{} >{} 2>{}\nprintf done >{}\nsleep 600\n",
        shell_quote(&project_root.to_string_lossy()),
        unset,
        shell_quote(operation_id),
        provide,
        shell_quote(&result.to_string_lossy()),
        shell_quote(&error.to_string_lossy()),
        shell_quote(&done.to_string_lossy())
    );
    let consumer = fixture.run_in_pane(pane, operation_id, &script);
    assert!(
        wait_until(Duration::from_secs(15), || done.exists()),
        "participant registration failed: {} {}",
        consumer.output(),
        std::fs::read_to_string(error).unwrap_or_default()
    );
    let context = parse_json(&std::fs::read(result).expect("participant context result"));
    assert_eq!(context["result"]["outcome"], "completed", "{context}");
    context
}

fn identity(context: &Value) -> (String, String) {
    let snapshot = &context["result"]["snapshot"];
    (
        snapshot["identity"]["worker_id"]
            .as_str()
            .expect("worker id")
            .to_owned(),
        snapshot["binding"]["app_scope_id"]
            .as_str()
            .expect("app scope id")
            .to_owned(),
    )
}

fn project_context(project_root: &Path, app_scope_id: &str) -> Value {
    let canonical_root =
        std::fs::canonicalize(project_root).expect("canonicalize project context root");
    json!({
        "app_scope_id": app_scope_id,
        "canonical_root": canonical_root.to_string_lossy(),
        "project_scope": canonical_root.to_string_lossy()
    })
}

fn wire_request(socket: &Path, request: &Value) -> Value {
    let mut stream =
        std::os::unix::net::UnixStream::connect(socket).expect("connect isolated daemon socket");
    stream
        .write_all(serde_json::to_string(request).unwrap().as_bytes())
        .expect("write daemon request");
    stream.write_all(b"\n").expect("terminate daemon request");
    stream.flush().expect("flush daemon request");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).expect("read daemon response");
    serde_json::from_str(&line).expect("daemon emits one JSON response")
}

fn workers_at(fixture: &Fixture, context: &Value) -> Value {
    wire_request(
        &fixture.host_state.join("server.sock"),
        &json!({"op": "Workers", "project_context": context}),
    )
}

fn worker_row(workers: &Value, worker_id: &str) -> Value {
    workers["workers"]
        .as_array()
        .expect("workers array")
        .iter()
        .find(|worker| worker["id"] == worker_id)
        .cloned()
        .unwrap_or_else(|| panic!("worker {worker_id} missing from {workers}"))
}

fn message_count(fixture: &Fixture, context: &Value, worker_id: &str) -> u64 {
    let row = worker_row(&workers_at(fixture, context), worker_id);
    row["pending_notifications"].as_u64().unwrap_or_default()
        + row["unacked_notifications"].as_u64().unwrap_or_default()
}

struct SourceCredentials {
    token: String,
    command: Value,
}

fn source_credentials(
    fixture: &Fixture,
    source_root: &Path,
    source_pane: &Pane,
    source_id: &str,
    source_context: &Value,
) -> SourceCredentials {
    let status = run_ok_at(
        fixture,
        source_root,
        source_pane,
        &["master", "status"],
        false,
    );
    assert_eq!(status["master"]["worker_id"], source_id, "{status}");
    let identity_path = fixture
        .host_state
        .join("identities")
        .join(source_id)
        .join("identity.json");
    let persisted: Value = serde_json::from_slice(&std::fs::read(identity_path).unwrap()).unwrap();
    let binding = &source_context["result"]["snapshot"]["binding"];
    SourceCredentials {
        token: persisted["token"].as_str().unwrap().to_owned(),
        command: json!({"command_id":"cross-project-test-command", "operation_id":"cross-project-test-operation",
            "actor_binding_id":binding["binding_id"], "endpoint_generation":binding["endpoint_generation"],
            "scope":{"app_scope_id":binding["app_scope_id"], "project_scope_id":binding["project_scope"]}}),
    }
}

fn cross_project_request(
    source_id: &str,
    source_root: &Path,
    credentials: &SourceCredentials,
    target_id: &str,
    target_context: &Value,
    subject: &str,
    body: &str,
    forge_token: bool,
) -> Value {
    json!({
        "op": "CrossProjectSend",
        "from": source_id,
        "from_project": source_root.to_string_lossy(),
        "worker_id": source_id,
        "token": if forge_token { "forged-token" } else { &credentials.token },
        "command": credentials.command,
        "to": target_id,
        "subject": subject,
        "body": body,
        "in_reply_to": null,
        "project_context": target_context
    })
}

fn message_status(
    fixture: &Fixture,
    target_root: &Path,
    target_pane: &Pane,
    msg_id: &str,
) -> Value {
    // `collab msg` is the public read owner; it attaches the caller runtime
    // identity the daemon now requires before the target lookup.
    run_ok_at(fixture, target_root, target_pane, &["msg", msg_id], false)
}

fn assert_rejected_without_message(
    fixture: &Fixture,
    target_context: &Value,
    target_id: &str,
    response: &Value,
    expected_error: &str,
    before: u64,
) {
    assert_eq!(response["ok"], false, "{response}");
    assert!(
        response["error"]
            .as_str()
            .unwrap_or_default()
            .contains(expected_error),
        "{response}"
    );
    assert!(response.get("msg_id").is_none(), "{response}");
    assert_eq!(
        message_count(fixture, target_context, target_id),
        before,
        "rejected cross-project send committed a message: {response}"
    );
}

#[test]
fn cross_project_route_projection_loss_preserves_binding_and_durable_enqueue() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, source_pane) =
        Fixture::isolated_with_pane_command("cross-project-stale-identity", "sleep 600");
    let source_root = fixture.root.clone();
    let target_root = create_external_project(&fixture, "target");
    let dead_root = create_external_project(&fixture, "dead");

    let source_context = register_context(
        &fixture,
        &source_root,
        &source_pane,
        "source-context",
        false,
    );
    let (source_id, _) = identity(&source_context);
    let promoted = run_ok_at(
        &fixture,
        &source_root,
        &source_pane,
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated source master",
        ],
        false,
    );
    assert_eq!(promoted["master"], source_id, "{promoted}");
    let credentials = source_credentials(
        &fixture,
        &source_root,
        &source_pane,
        &source_id,
        &source_context,
    );

    let target_pane = create_session(&fixture, "target-pane");
    let target_context =
        register_context(&fixture, &target_root, &target_pane, "target-context", true);
    let (target_id, target_app_scope) = identity(&target_context);
    let target_context = project_context(&target_root, &target_app_scope);

    let before = message_count(&fixture, &target_context, &target_id);
    let peer_rejected = wire_request(
        &fixture.host_state.join("server.sock"),
        &cross_project_request(
            &source_id,
            &source_root,
            &credentials,
            &target_id,
            &target_context,
            "Reject target peer",
            "target has not been promoted",
            false,
        ),
    );
    assert_rejected_without_message(
        &fixture,
        &target_context,
        &target_id,
        &peer_rejected,
        "CROSS_PROJECT_SEND_REJECTED",
        before,
    );

    let promoted = run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated stale-identity target master",
        ],
        true,
    );
    assert_eq!(promoted["master"], target_id, "{promoted}");

    let before = message_count(&fixture, &target_context, &target_id);
    for (field, value) in [
        ("reply_policy", json!("required")),
        ("target_session", json!("other-session")),
    ] {
        let mut request = cross_project_request(
            &source_id,
            &source_root,
            &credentials,
            &target_id,
            &target_context,
            "Unsupported option",
            "must not enqueue",
            false,
        );
        request[field] = value;
        let rejected = wire_request(&fixture.host_state.join("server.sock"), &request);
        assert_rejected_without_message(
            &fixture,
            &target_context,
            &target_id,
            &rejected,
            "CROSS_PROJECT_OPTION_UNSUPPORTED",
            before,
        );
    }
    let source_rejected = wire_request(
        &fixture.host_state.join("server.sock"),
        &cross_project_request(
            &source_id,
            &source_root,
            &credentials,
            &target_id,
            &target_context,
            "Reject forged credential",
            "source token is forged",
            true,
        ),
    );
    assert_rejected_without_message(
        &fixture,
        &target_context,
        &target_id,
        &source_rejected,
        "token mismatch",
        before,
    );

    let stale_workers = workers_at(&fixture, &target_context);
    let stale_target = worker_row(&stale_workers, &target_id);
    assert_eq!(stale_target["presence"], "present", "{stale_target}");
    assert_eq!(stale_target["identity_valid"], true, "{stale_target}");
    assert_eq!(stale_target["endpoint_live"], true, "{stale_target}");
    assert_eq!(stale_target["role"], "master", "{stale_target}");

    let delivered = wire_request(
        &fixture.host_state.join("server.sock"),
        &cross_project_request(
            &source_id,
            &source_root,
            &credentials,
            &target_id,
            &target_context,
            "Cross-project stale identity",
            "durable message through a live tmux endpoint",
            false,
        ),
    );
    assert_eq!(delivered["ok"], true, "{delivered}");
    assert_eq!(delivered["durable"], true, "{delivered}");
    assert_eq!(delivered["cross_project"], true, "{delivered}");
    assert_eq!(delivered["sender"], source_id, "{delivered}");
    assert_eq!(delivered["recipient"], target_id, "{delivered}");
    assert_eq!(delivered["consumed"], false, "{delivered}");
    let msg_id = delivered["msg_id"]
        .as_str()
        .expect("durable message id")
        .to_owned();

    let pending = message_status(&fixture, &target_root, &target_pane, &msg_id);
    assert_eq!(pending["state"], "pending", "{pending}");
    assert_eq!(pending["consumed_by_recv"], false, "{pending}");
    assert_eq!(pending["to"], target_id, "{pending}");
    assert_eq!(
        pending["subject"], "Cross-project stale identity",
        "{pending}"
    );
    assert_eq!(
        pending["body"], "durable message through a live tmux endpoint",
        "{pending}"
    );

    fixture.stop_daemon();
    fixture.restart_daemon();
    let replayed = message_status(&fixture, &target_root, &target_pane, &msg_id);
    assert_eq!(replayed["id"], msg_id, "{replayed}");
    assert_eq!(replayed["state"], "pending", "{replayed}");
    assert_eq!(replayed["consumed_by_recv"], false, "{replayed}");
    assert_eq!(replayed["to"], target_id, "{replayed}");
    assert_eq!(replayed["subject"], pending["subject"], "{replayed}");
    assert_eq!(replayed["body"], pending["body"], "{replayed}");

    let dead_pane = create_session(&fixture, "dead-pane");
    let dead_context = register_context(&fixture, &dead_root, &dead_pane, "dead-context", true);
    let (dead_id, dead_app_scope) = identity(&dead_context);
    let dead_context = project_context(&dead_root, &dead_app_scope);
    let promoted = run_ok_at(
        &fixture,
        &dead_root,
        &dead_pane,
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated dead-endpoint target master",
        ],
        true,
    );
    assert_eq!(promoted["master"], dead_id, "{promoted}");

    let before = message_count(&fixture, &dead_context, &dead_id);
    let killed = Command::new("tmux")
        .args([
            "-S",
            fixture.tmux_socket.to_str().unwrap(),
            "kill-session",
            "-t",
            "dead-pane",
        ])
        .output()
        .expect("close owned dead target session");
    assert!(
        killed.status.success(),
        "close owned dead target session failed: {}",
        String::from_utf8_lossy(&killed.stderr)
    );
    let dead_row = worker_row(&workers_at(&fixture, &dead_context), &dead_id);
    assert_eq!(dead_row["endpoint_live"], false, "{dead_row}");

    let dead_rejected = wire_request(
        &fixture.host_state.join("server.sock"),
        &cross_project_request(
            &source_id,
            &source_root,
            &credentials,
            &dead_id,
            &dead_context,
            "Reject dead endpoint",
            "this message must not commit",
            false,
        ),
    );
    assert_rejected_without_message(
        &fixture,
        &dead_context,
        &dead_id,
        &dead_rejected,
        "requires a reachable target binding",
        before,
    );

    fixture.stop_daemon();
    std::fs::remove_dir_all(&target_root).expect("remove isolated target project");
    std::fs::remove_dir_all(&dead_root).expect("remove isolated dead project");
}

fn address_for(listed: &Value, worker_id: &str) -> String {
    listed["targets"]
        .as_array()
        .expect("directory targets")
        .iter()
        .find(|target| target["worker_id"] == worker_id && target.get("address").is_some())
        .and_then(|target| target["address"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("address for {worker_id} missing from {listed}"))
}

#[test]
fn public_cross_project_peers_send_and_cross_level_requests_have_no_effect() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, source_master) = Fixture::isolated_with_pane_command("scope-matrix", "sleep 600");
    let source_root = fixture.root.canonicalize().unwrap();
    let target_root = create_external_project(&fixture, "scope-target");
    let source_peer = create_session(&fixture, "scope-source-peer");
    let target_master = create_session(&fixture, "scope-target-master");
    let target_peer = create_session(&fixture, "scope-target-peer");
    let unregistered = create_session(&fixture, "scope-unregistered");
    register_context(
        &fixture,
        &source_root,
        &source_master,
        "scope-source-master",
        false,
    );
    register_context(
        &fixture,
        &source_root,
        &source_peer,
        "scope-source-peer",
        false,
    );
    let target = register_context(
        &fixture,
        &target_root,
        &target_master,
        "scope-target-master",
        false,
    );
    let peer = register_context(
        &fixture,
        &target_root,
        &target_peer,
        "scope-target-peer",
        false,
    );
    for (root, pane) in [
        (&source_root, &source_master),
        (&target_root, &target_master),
    ] {
        run_ok_at(
            &fixture,
            root,
            pane,
            &[
                "master",
                "promote",
                "--approval",
                "user approved isolated master",
            ],
            false,
        );
    }
    let listed = run_ok_at(&fixture, &source_root, &source_master, &["list"], false);
    let master_address = address_for(&listed, &identity(&target).0);
    let peer_address = address_for(&listed, &identity(&peer).0);
    let sent = run_ok_at(
        &fixture,
        &source_root,
        &source_peer,
        &[
            "send",
            "--to",
            &peer_address,
            "--subject",
            "peer-message",
            "--content",
            "peer body",
        ],
        false,
    );
    assert_eq!(sent["durable"], true, "{sent}");
    let received = run_ok_at(
        &fixture,
        &target_root,
        &target_peer,
        &["recv", "--timeout", "0"],
        false,
    );
    assert!(
        received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == sent["msg_id"] && m["body"] == "peer body"),
        "{received}"
    );
    for (pane, address) in [
        (&source_peer, &master_address),
        (&source_master, &peer_address),
    ] {
        let denied = command_at(
            &fixture,
            &source_root,
            pane,
            &[
                "send",
                "--to",
                address,
                "--subject",
                "denied-cross-level",
                "--content",
                "must not arrive",
            ],
            false,
        )
        .output()
        .unwrap();
        assert!(
            !denied.status.success(),
            "cross-level request succeeded: {}",
            String::from_utf8_lossy(&denied.stdout)
        );
    }
    let denied = command_at(
        &fixture,
        &source_root,
        &unregistered,
        &[
            "send",
            "--to",
            &peer_address,
            "--subject",
            "denied-cross-level",
            "--content",
            "must not arrive",
        ],
        false,
    )
    .output()
    .unwrap();
    assert!(
        !denied.status.success(),
        "unregistered request succeeded: {}",
        String::from_utf8_lossy(&denied.stdout)
    );
    for pane in [&target_master, &target_peer] {
        let inbox = run_ok_at(
            &fixture,
            &target_root,
            pane,
            &["recv", "--timeout", "0"],
            false,
        );
        assert!(
            inbox["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|m| m["subject"] != "denied-cross-level"),
            "{inbox}"
        );
    }
    drop(fixture);
    std::fs::remove_dir_all(target_root).unwrap();
}

#[test]
fn public_ceo_without_source_master_grant_sends_to_foreign_master_and_peer() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, source_master) = Fixture::isolated_with_pane_command("scope-ceo", "sleep 600");
    let source_root = fixture.root.canonicalize().unwrap();
    let target_root = create_external_project(&fixture, "scope-ceo-target");
    let ceo = create_session(&fixture, "scope-ceo-peer");
    let target_master = create_session(&fixture, "scope-ceo-target-master");
    let target_peer = create_session(&fixture, "scope-ceo-target-peer");
    register_context(
        &fixture,
        &source_root,
        &source_master,
        "ceo-source-master",
        false,
    );
    register_context(&fixture, &source_root, &ceo, "ceo-source-holder", false);
    let master = register_context(
        &fixture,
        &target_root,
        &target_master,
        "ceo-foreign-master",
        false,
    );
    let peer = register_context(
        &fixture,
        &target_root,
        &target_peer,
        "ceo-foreign-peer",
        false,
    );
    for (root, pane) in [
        (&source_root, &source_master),
        (&target_root, &target_master),
    ] {
        run_ok_at(
            &fixture,
            root,
            pane,
            &[
                "master",
                "promote",
                "--approval",
                "user approved isolated master",
            ],
            false,
        );
    }
    run_ok_at(
        &fixture,
        &source_root,
        &ceo,
        &["ceo", "promote", "--approval", "user approved isolated CEO"],
        false,
    );
    let listed = run_ok_at(&fixture, &source_root, &ceo, &["list"], false);
    let master_address = address_for(&listed, &identity(&master).0);
    let peer_address = address_for(&listed, &identity(&peer).0);
    for (pane, address) in [
        (&target_master, &master_address),
        (&target_peer, &peer_address),
    ] {
        let sent = run_ok_at(
            &fixture,
            &source_root,
            &ceo,
            &[
                "send",
                "--to",
                address,
                "--subject",
                "ceo-scope",
                "--content",
                "CEO body",
            ],
            false,
        );
        assert_eq!(sent["durable"], true, "{sent}");
        let inbox = run_ok_at(
            &fixture,
            &target_root,
            pane,
            &["recv", "--timeout", "0"],
            false,
        );
        assert!(
            inbox["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["id"] == sent["msg_id"] && m["body"] == "CEO body"),
            "{inbox}"
        );
    }
    drop(fixture);
    std::fs::remove_dir_all(target_root).unwrap();
}

#[test]
fn qqbot_ceo_address_works_in_external_ceo_workspace() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, host_pane) =
        Fixture::isolated_with_pane_command("qqbot-external-ceo", "sleep 600");
    let host_root = fixture.root.canonicalize().unwrap();
    let ceo_root = create_external_project(&fixture, "ceo-workspace");
    let ceo_pane = create_session(&fixture, "qqbot-external-holder");
    register_context(&fixture, &host_root, &host_pane, "qqbot-host", false);
    let context = register_context(&fixture, &ceo_root, &ceo_pane, "qqbot-ceo", false);
    let (ceo_id, _) = identity(&context);
    let promoted = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &["ceo", "promote", "--approval", "user approved isolated CEO"],
        false,
    );
    assert_eq!(promoted["is_current"], true, "{promoted}");
    let envelope = json!({"source_channel":"qqbot", "channel_instance":"qqbot",
        "external_user":"fixture-user", "external_conversation":"fixture-conversation",
        "platform_message_id":"external-ceo-event", "reply_route":"qqbot://c2c/fixture-conversation",
        "recipient_address":"ceo", "attachments":[]}).to_string();
    let sent = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "sendmessage",
            "--unknown-sender",
            "--to",
            "ceo",
            "--subject",
            "QQBot",
            "--content",
            "external CEO message",
            "--channel-envelope",
            &envelope,
        ],
        false,
    );
    let duplicate = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "sendmessage",
            "--unknown-sender",
            "--to",
            "ceo",
            "--subject",
            "QQBot",
            "--content",
            "external CEO message",
            "--channel-envelope",
            &envelope,
        ],
        false,
    );
    assert_eq!(duplicate["msg_id"], sent["msg_id"], "{duplicate}");
    assert_eq!(duplicate["deduplicated"], true, "{duplicate}");
    let inbox = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &["recv", "--timeout", "0"],
        false,
    );
    let message = inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == sent["msg_id"])
        .expect("external CEO must consume the message");
    assert_eq!(message["to"], ceo_id, "{message}");
    run_ok_at(
        &fixture,
        &host_root,
        &host_pane,
        &["ceo", "promote", "--approval", "user approved CEO transfer"],
        false,
    );
    let mut stale_envelope: Value = serde_json::from_str(&envelope).unwrap();
    stale_envelope["platform_message_id"] = json!("external-ceo-after-transfer");
    let stale_envelope = stale_envelope.to_string();
    let stale = command_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "sendmessage",
            "--unknown-sender",
            "--to",
            "ceo",
            "--subject",
            "QQBot",
            "--content",
            "must not reach former CEO",
            "--channel-envelope",
            &stale_envelope,
        ],
        false,
    )
    .output()
    .unwrap();
    assert!(
        !stale.status.success(),
        "old workspace must not use a stale CEO grant"
    );
    let inbox = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &["recv", "--timeout", "0"],
        false,
    );
    assert!(
        inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| !m["body"]
                .as_str()
                .unwrap_or("")
                .contains("must not reach former CEO")),
        "{inbox}"
    );
    drop(fixture);
    std::fs::remove_dir_all(ceo_root).unwrap();
}

/// One public `send` command and one host-wide `list` source cover local and
/// cross-project delivery. Authorization stays inside the daemon and the
/// failure names the current master so a caller knows who may send.
#[test]
fn unified_send_and_global_list_share_one_address_source() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, source_pane) = Fixture::isolated_with_pane_command("unified-send", "sleep 600");
    let source_root = fixture.root.canonicalize().unwrap();
    let target_root = create_external_project(&fixture, "unified-target");
    let target_pane = create_session(&fixture, "unified-target");
    let peer_pane = create_session(&fixture, "unified-peer");

    let source = register_context(&fixture, &source_root, &source_pane, "source", false);
    let peer = register_context(&fixture, &source_root, &peer_pane, "source-peer", false);
    let target = register_context(&fixture, &target_root, &target_pane, "target", false);
    let (source_id, _) = identity(&source);
    let (peer_id, _) = identity(&peer);
    let (target_id, _) = identity(&target);
    for (root, pane) in [(&source_root, &source_pane), (&target_root, &target_pane)] {
        run_ok_at(
            &fixture,
            root,
            pane,
            &[
                "master",
                "promote",
                "--approval",
                "user approved isolated unified master",
            ],
            false,
        );
    }

    // `list` is host-wide and must work outside any initialized project.
    let neutral = std::env::temp_dir();
    let listed = {
        let output = command_at(&fixture, &neutral, &source_pane, &["list"], false)
            .output()
            .expect("run collab list outside a project");
        assert!(
            output.status.success(),
            "collab list from {} failed: stdout={} stderr={}",
            neutral.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        parse_json(&output.stdout)
    };
    let target_address = address_for(&listed, &target_id);
    let peer_address = address_for(&listed, &peer_id);
    assert!(target_address.contains('.'), "{listed}");

    // One send command reaches the current master of another project.
    let sent = run_ok_at(
        &fixture,
        &source_root,
        &source_pane,
        &[
            "send",
            "--to",
            &target_address,
            "--subject",
            "action",
            "--content",
            "return the result",
        ],
        false,
    );
    assert_eq!(sent["durable"], true, "{sent}");
    let received = run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &["recv", "--timeout", "0"],
        false,
    );
    let request = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == sent["msg_id"])
        .expect("cross-project request consumed");
    assert_eq!(request["body"], "return the result");

    // The same command reaches a local peer; the subject is derived from the
    // content so a caller only needs an address and a body.
    let local = run_ok_at(
        &fixture,
        &source_root,
        &source_pane,
        &[
            "send",
            "--to",
            &peer_address,
            "--content",
            "local handoff body",
        ],
        false,
    );
    assert_eq!(local["durable"], true, "{local}");
    let peer_inbox = run_ok_at(
        &fixture,
        &source_root,
        &peer_pane,
        &["recv", "--timeout", "0"],
        false,
    );
    assert!(
        peer_inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == local["msg_id"]),
        "{peer_inbox}"
    );

    // A non-master peer gets an explicit permission error that names the
    // current master, and nothing is persisted.
    let denied = command_at(
        &fixture,
        &source_root,
        &peer_pane,
        &[
            "send",
            "--to",
            &target_address,
            "--subject",
            "denied",
            "--content",
            "must not enqueue",
        ],
        false,
    )
    .output()
    .expect("run unauthorized cross-project send");
    assert!(
        !denied.status.success(),
        "unauthorized send succeeded: {}",
        String::from_utf8_lossy(&denied.stdout)
    );
    let stderr = String::from_utf8_lossy(&denied.stderr);
    assert!(stderr.contains("CROSS_PROJECT_SEND_REJECTED"), "{stderr}");
    assert!(
        stderr.contains(&target_id),
        "permission error must identify the current target master: {stderr}"
    );
    let after = run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &["recv", "--timeout", "0"],
        false,
    );
    assert!(
        !after["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["subject"] == "denied"),
        "{after}"
    );
    drop(fixture);
    std::fs::remove_dir_all(target_root).expect("remove isolated target project");
}

/// `ceo` is a host-wide address from a non-CEO project. An uncorrelated send
/// must land in the CEO mailbox as a message and a non-master peer must see the
/// current source master's identity in the denial.
#[test]
fn send_to_ceo_delivers_directly_and_names_the_source_master_when_denied() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, source_pane) = Fixture::isolated_with_pane_command("ceo-direct", "sleep 600");
    let source_root = fixture.root.canonicalize().unwrap();
    let ceo_root = create_external_project(&fixture, "ceo-workspace");
    let ceo_pane = create_session(&fixture, "ceo-workspace");
    let peer_pane = create_session(&fixture, "ceo-peer");

    let source = register_context(&fixture, &source_root, &source_pane, "ceo-source", false);
    register_context(&fixture, &source_root, &peer_pane, "ceo-source-peer", false);
    register_context(&fixture, &ceo_root, &ceo_pane, "ceo-target", false);
    let (source_id, _) = identity(&source);

    run_ok_at(
        &fixture,
        &source_root,
        &source_pane,
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated source master",
        ],
        false,
    );
    run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated ceo master",
        ],
        false,
    );
    let promoted = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &["ceo", "promote", "--approval", "user approved isolated ceo"],
        false,
    );
    assert_eq!(promoted["is_current"], true, "{promoted}");

    let sent = run_ok_at(
        &fixture,
        &source_root,
        &source_pane,
        &[
            "send",
            "--to",
            "ceo",
            "--subject",
            "direct",
            "--content",
            "direct ceo body",
        ],
        false,
    );
    assert_eq!(sent["durable"], true, "{sent}");
    let received = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &["recv", "--timeout", "0"],
        false,
    );
    let message = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == sent["msg_id"])
        .expect("CEO must consume the direct message");
    assert_eq!(message["body"], "direct ceo body", "{message}");
    assert_eq!(message["subject"], "direct", "{message}");

    let denied = command_at(
        &fixture,
        &source_root,
        &peer_pane,
        &[
            "send",
            "--to",
            "ceo",
            "--subject",
            "denied",
            "--content",
            "must not enqueue",
        ],
        false,
    )
    .output()
    .expect("run unauthorized send to ceo");
    assert!(
        !denied.status.success(),
        "unauthorized ceo send succeeded: {}",
        String::from_utf8_lossy(&denied.stdout)
    );
    let stderr = String::from_utf8_lossy(&denied.stderr);
    assert!(stderr.contains("PERMISSION_DENIED"), "{stderr}");
    assert!(
        stderr.contains(&source_id),
        "permission error must name the current source master: {stderr}"
    );

    drop(fixture);
    std::fs::remove_dir_all(ceo_root).expect("remove isolated ceo project");
}
