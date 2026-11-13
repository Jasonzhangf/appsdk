include!("support/context_operation_fixture.rs");

// `worker close --self` retires the pane's control plane but must not leave a
// persisted identity whose old project scope still owns the same pane anchors.
// If it does, a later `collab context` in another project cwd fails closed
// with IDENTITY_RESTORE_CROSS_PROJECT before the pane can register there.

fn parse_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("collab CLI emits JSON")
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

/// Build a raw project context from a resolved root; enough for context-free
/// daemon reads such as board shows without re-registering the pane.
fn project_context_json(project_root: &Path, app_scope_id: &str) -> Value {
    json!({
        "app_scope_id": app_scope_id,
        "canonical_root": project_root.to_string_lossy(),
        "project_scope": project_root.to_string_lossy()
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

/// Open project board read: task records survive peer control-plane removal.
fn board_tasks(fixture: &Fixture, project_context: &Value) -> Vec<String> {
    let board = wire_request(
        &fixture.host_state.join("server.sock"),
        &json!({"op": "BoardShow", "project_context": project_context}),
    );
    assert_eq!(board["ok"], true, "{board}");
    board["tasks"]
        .as_array()
        .expect("board tasks array")
        .iter()
        .map(|task| task["id"].as_str().expect("task id").to_owned())
        .collect()
}

#[test]
fn self_close_clears_old_anchors_before_cross_project_context() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) =
        Fixture::isolated_with_pane_command("self-close-cross-project", "sleep 600");
    let source_root = fixture.root.canonicalize().unwrap();
    let target_root = create_external_project(&fixture, "next-project");

    let source = register_context(&fixture, &source_root, &pane, "self-close-source", false);
    let (source_id, source_app_scope) = identity(&source);
    let source_name = source["result"]["snapshot"]["name"]
        .as_str()
        .expect("original name")
        .to_owned();
    assert!(!source_name.is_empty(), "original name must be non-empty");
    let source_session = source["result"]["snapshot"]["binding"]["session_id"]
        .as_str()
        .expect("source binding session")
        .to_owned();
    assert!(
        !source_session.is_empty(),
        "source binding session must be non-empty"
    );
    assert_eq!(source_session, pane.session_anchor);

    let task = run_ok_at(
        &fixture,
        &source_root,
        &pane,
        &[
            "task",
            "register",
            "self-close-open-task",
            "--priority",
            "p1",
            "--next",
            "continue after control-plane removal",
        ],
        false,
    );
    assert_eq!(task["task"], "self-close-open-task", "{task}");

    let consumed = run_ok_at(
        &fixture,
        &source_root,
        &pane,
        &[
            "send",
            "--unknown-sender",
            "--to",
            &source_id,
            "--subject",
            "already consumed",
            "--content",
            "keep consumed history",
            "--delivery",
            "queued",
        ],
        false,
    );
    let consumed_id = consumed["msg_id"].as_str().unwrap();
    let receipt = run_ok_at(
        &fixture,
        &source_root,
        &pane,
        &[
            "recv",
            "--receive-id",
            "before-self-close",
            "--timeout",
            "0",
        ],
        false,
    );
    assert!(
        receipt["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == consumed_id),
        "{receipt}"
    );

    let sent = run_ok_at(
        &fixture,
        &source_root,
        &pane,
        &[
            "send",
            "--unknown-sender",
            "--to",
            &source_id,
            "--subject",
            "preserve me",
            "--content",
            "business payload must survive self close",
            "--delivery",
            "queued",
        ],
        false,
    );
    let msg_id = sent["msg_id"].as_str().expect("sent message id").to_owned();
    let explicit = run_ok_at(
        &fixture,
        &source_root,
        &pane,
        &[
            "send",
            "--unknown-sender",
            "--to",
            &source_id,
            "--target-session",
            &source_session,
            "--subject",
            "preserve exact session",
            "--content",
            "current-session payload must survive retirement",
            "--delivery",
            "queued",
        ],
        false,
    );
    let explicit_id = explicit["msg_id"]
        .as_str()
        .expect("explicit-session message id")
        .to_owned();
    let source_before = fixture.current_state_for(&source_root, &source_app_scope);
    let task_before = source_before["tasks"]["self-close-open-task"].clone();
    let message_before = source_before["msgs"][&msg_id].clone();
    let explicit_before = source_before["msgs"][&explicit_id].clone();
    assert!(!task_before.is_null());
    assert!(!message_before.is_null());
    assert!(!explicit_before.is_null());
    assert_eq!(
        message_before["body"],
        "business payload must survive self close"
    );
    assert_eq!(
        explicit_before["body"],
        "current-session payload must survive retirement"
    );
    assert!(
        source_before["mailbox_name_addresses"]
            .get(&msg_id)
            .is_none(),
        "unpinned raw-worker message starts without a name address: {source_before}"
    );
    assert!(
        source_before["mailbox_name_addresses"]
            .get(&explicit_id)
            .is_none(),
        "exact-session raw-worker message starts without a name address: {source_before}"
    );
    assert!(source_before["target_sessions"].get(&msg_id).is_none());
    assert_eq!(
        source_before["target_sessions"][&explicit_id], source_session,
        "the public send must keep the actual source session"
    );
    let map: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        map["slots"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|slot| slot["identity_status"] == "active"
                && slot["current"]["binding"]["agent_id"] == source_id)
            .count(),
        1
    );

    let pane_pid_before = tmux(
        &fixture.tmux_socket,
        &["display-message", "-p", "-t", &pane.pane_id, "#{pane_pid}"],
    )
    .stdout;

    let closed = run_ok_at(
        &fixture,
        &source_root,
        &pane,
        &["worker", "close", "--self"],
        false,
    );
    assert_eq!(closed["result"]["outcome"], "complete", "{closed}");
    let retired = fixture.current_state_for(&source_root, &source_app_scope);
    assert!(retired["workers"].get(&source_id).is_none());
    assert!(!retired["global"]["current_thread_routes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|route| route[1]["agent_id"] == source_id));

    let pane_pid_after = tmux(
        &fixture.tmux_socket,
        &["display-message", "-p", "-t", &pane.pane_id, "#{pane_pid}"],
    )
    .stdout;
    assert_eq!(
        pane_pid_after, pane_pid_before,
        "self close must keep the owning pane alive"
    );

    let tasks_after = board_tasks(
        &fixture,
        &project_context_json(&source_root, &source_app_scope),
    );
    assert!(
        tasks_after.iter().any(|id| id == "self-close-open-task"),
        "self close must preserve open business task: {tasks_after:?}"
    );
    let source_after = fixture.current_state_for(&source_root, &source_app_scope);
    assert_eq!(source_after["tasks"]["self-close-open-task"], task_before);
    assert_eq!(source_after["msgs"][&msg_id], message_before);
    assert_eq!(source_after["msgs"][&explicit_id], explicit_before);
    let old_default = format!("sub-default-direct-message-{source_id}");
    assert_eq!(
        source_after["notification_subscriptions"][&old_default]["status"], "cancelled",
        "retirement must stop the old default wake subscription"
    );
    assert_eq!(
        source_after["receive_receipts"],
        source_before["receive_receipts"]
    );

    // The public entrance that the fix must unblock: the same pane registers
    // in a second real project directory after a normal self close.
    let second = run_ok_at(
        &fixture,
        &target_root,
        &pane,
        &["context", "--op", "self-close-next"],
        false,
    );
    assert_eq!(second["result"]["outcome"], "completed", "{second}");
    assert_eq!(
        second["result"]["snapshot"]["binding"]["project_scope"].as_str(),
        target_root.to_str(),
        "{second}"
    );
    let target_id = second["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap();
    let target_app = second["result"]["snapshot"]["scope"]["app_scope_id"]
        .as_str()
        .unwrap();
    let map: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let active: Vec<_> = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|slot| slot["identity_status"] == "active")
        .collect();
    assert!(
        !active
            .iter()
            .any(|slot| slot["current"]["binding"]["agent_id"] == source_id),
        "source carrier remains retired"
    );
    assert_eq!(
        active
            .iter()
            .filter(|slot| slot["current"]["binding"]["agent_id"] == target_id
                && slot["identity_key"]["project_scope"] == target_root.to_str().unwrap()
                && slot["identity_key"]["app_scope_id"] == target_app)
            .count(),
        1
    );
    let source_final = fixture.current_state_for(&source_root, &source_app_scope);
    assert_eq!(source_final["tasks"]["self-close-open-task"], task_before);
    assert_eq!(source_final["msgs"][&msg_id], message_before);
    assert_eq!(source_final["msgs"][&explicit_id], explicit_before);
    assert_eq!(
        source_final["receive_receipts"],
        source_before["receive_receipts"]
    );

    // A separate current caller reacquires the now-unoccupied source name.
    // It consumes the original queued message, not a recreated copy or history.
    let session = "self-close-returned-holder";
    let created = tmux(
        &fixture.tmux_socket,
        &[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            session,
            "-c",
            source_root.to_str().unwrap(),
            "sleep 600",
        ],
    );
    fixture.track_tmux_session(session);
    let returned_pane = Pane {
        pane_id: String::from_utf8(created.stdout).unwrap().trim().to_owned(),
        server_pid: pane.server_pid,
        session_anchor: "self-close-new-agent-session".into(),
        thread_anchor: "self-close-new-agent-thread".into(),
    };
    let returned = register_context(
        &fixture,
        &source_root,
        &returned_pane,
        "self-close-reacquired-name",
        false,
    );
    let returned_name = returned["result"]["snapshot"]["name"]
        .as_str()
        .expect("returned name");
    assert!(!returned_name.is_empty(), "returned name must be non-empty");
    assert_eq!(returned_name, source_name);
    let returned_worker = returned["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("returned worker")
        .to_owned();
    let returned_session = returned["result"]["snapshot"]["binding"]["session_id"]
        .as_str()
        .expect("returned binding session")
        .to_owned();
    assert!(
        !returned_worker.is_empty(),
        "returned worker must be non-empty"
    );
    assert_ne!(returned_worker, source_id, "the successor is a new worker");
    assert_eq!(returned_session, returned_pane.session_anchor);
    assert_ne!(
        returned_session, source_session,
        "the successor is a new execution session"
    );
    let handoff = fixture.current_state_for(&source_root, &source_app_scope);
    let receive_args = ["recv", "--receive-id", "after-self-close", "--timeout", "0"];
    let received = run_ok_at(&fixture, &source_root, &returned_pane, &receive_args, false);
    let messages = received["messages"].as_array().unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message["id"] == msg_id && message["body"] == message_before["body"]),
        "{received}"
    );
    assert!(
        messages.iter().any(|message| {
            message["id"] == explicit_id && message["body"] == explicit_before["body"]
        }),
        "{received}"
    );
    assert!(
        !messages.iter().any(|message| message["id"] == consumed_id),
        "{received}"
    );
    for id in [&msg_id, &explicit_id] {
        assert_eq!(
            handoff["msgs"][id]["to"], returned_worker,
            "message {id} must transfer to the current name holder"
        );
        assert_eq!(
            handoff["wake_bindings"][id],
            format!("sub-default-direct-message-{returned_worker}"),
            "message {id} must wake the successor"
        );
    }
    assert_eq!(
        handoff["target_sessions"][&explicit_id], returned_session,
        "the exact-session message must follow the successor session"
    );
    assert_eq!(
        handoff["notification_subscriptions"]
            [&format!("sub-default-direct-message-{returned_worker}")]["worker_id"],
        returned_worker,
        "the successor default wake subscription must belong to the new holder"
    );
    assert_eq!(
        retired["mailbox_name_addresses"][&msg_id]["name"], source_name,
        "the unpinned message must retain the retired name relation"
    );
    assert_eq!(
        retired["mailbox_name_addresses"][&explicit_id]["name"], source_name,
        "the matching current-session message must retain the retired name relation"
    );
    assert!(retired["target_sessions"].get(&msg_id).is_none());
    assert_eq!(
        retired["target_sessions"][&explicit_id], source_session,
        "retirement must preserve the original target session before handoff"
    );
    let replay = run_ok_at(&fixture, &source_root, &returned_pane, &receive_args, false);
    assert_eq!(replay["messages"], received["messages"]);
    let after_receive = fixture.current_state_for(&source_root, &source_app_scope);
    assert_eq!(
        after_receive["msgs"][consumed_id],
        source_before["msgs"][consumed_id]
    );
    assert_eq!(
        after_receive["tasks"]["self-close-open-task"]["id"],
        task_before["id"]
    );
    assert_eq!(
        after_receive["tasks"]["self-close-open-task"]["status"],
        task_before["status"]
    );
    drop(fixture);
    std::fs::remove_dir_all(target_root).expect("remove isolated target project");
}
