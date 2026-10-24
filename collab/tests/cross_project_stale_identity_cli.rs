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
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("project master"));
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

struct SourceGrant {
    assigned_by: String,
    approval: Option<String>,
    assigned_ms: i64,
}

fn source_grant(
    fixture: &Fixture,
    source_root: &Path,
    source_pane: &Pane,
    source_id: &str,
) -> SourceGrant {
    let status = run_ok_at(
        fixture,
        source_root,
        source_pane,
        &["master", "status"],
        false,
    );
    assert_eq!(status["master"]["worker_id"], source_id, "{status}");
    SourceGrant {
        assigned_by: status["master"]["assigned_by"]
            .as_str()
            .expect("source assignment actor")
            .to_owned(),
        approval: status["master"]["approval"].as_str().map(str::to_owned),
        assigned_ms: status["master"]["assigned_ms"]
            .as_i64()
            .expect("source assignment time"),
    }
}

fn cross_project_request(
    source_id: &str,
    source_root: &Path,
    grant: &SourceGrant,
    target_id: &str,
    target_context: &Value,
    subject: &str,
    body: &str,
    assigned_ms: i64,
) -> Value {
    json!({
        "op": "CrossProjectSend",
        "from": source_id,
        "from_project": source_root.to_string_lossy(),
        "source_master_assigned_by": grant.assigned_by,
        "source_master_approval": grant.approval,
        "source_master_assigned_ms": assigned_ms,
        "to": target_id,
        "subject": subject,
        "body": body,
        "in_reply_to": null,
        "project_context": target_context
    })
}

fn message_status(fixture: &Fixture, target_context: &Value, msg_id: &str) -> Value {
    wire_request(
        &fixture.host_state.join("server.sock"),
        &json!({
            "op": "MsgStatus",
            "msg_id": msg_id,
            "project_context": target_context
        }),
    )
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
    let grant = source_grant(&fixture, &source_root, &source_pane, &source_id);

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
            &grant,
            &target_id,
            &target_context,
            "Reject target peer",
            "target has not been promoted",
            grant.assigned_ms,
        ),
    );
    assert_rejected_without_message(
        &fixture,
        &target_context,
        &target_id,
        &peer_rejected,
        "requires the target to be the project master",
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
    let source_rejected = wire_request(
        &fixture.host_state.join("server.sock"),
        &cross_project_request(
            &source_id,
            &source_root,
            &grant,
            &target_id,
            &target_context,
            "Reject source approval",
            "source assignment timestamp is forged",
            grant.assigned_ms + 1,
        ),
    );
    assert_rejected_without_message(
        &fixture,
        &target_context,
        &target_id,
        &source_rejected,
        "CROSS_PROJECT_SOURCE_REJECTED",
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
            &grant,
            &target_id,
            &target_context,
            "Cross-project stale identity",
            "durable message through a live tmux endpoint",
            grant.assigned_ms,
        ),
    );
    assert_eq!(delivered["ok"], true, "{delivered}");
    assert_eq!(delivered["durable"], true, "{delivered}");
    assert_eq!(delivered["cross_project"], true, "{delivered}");
    assert_eq!(delivered["source_master"], source_id, "{delivered}");
    assert_eq!(delivered["target_master"], target_id, "{delivered}");
    assert_eq!(delivered["consumed"], false, "{delivered}");
    let msg_id = delivered["msg_id"]
        .as_str()
        .expect("durable message id")
        .to_owned();

    let pending = message_status(&fixture, &target_context, &msg_id);
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
    let replayed = message_status(&fixture, &target_context, &msg_id);
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
            &grant,
            &dead_id,
            &dead_context,
            "Reject dead endpoint",
            "this message must not commit",
            grant.assigned_ms,
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
