include!("support/context_operation_fixture.rs");

fn workers_for_route(fixture: &Fixture, project_root: &Path, app_scope_id: &str) -> Value {
    let canonical_root = std::fs::canonicalize(project_root).expect("canonicalize project root");
    wire_request(
        &fixture.host_state.join("server.sock"),
        &json!({
            "op": "Workers",
            "project_context": {
                "app_scope_id": app_scope_id,
                "canonical_root": canonical_root.to_string_lossy(),
                "project_scope": canonical_root.to_string_lossy()
            }
        }),
    )
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
            .expect("read tmux server pid")
            .stdout,
    )
    .expect("tmux server pid is UTF-8")
    .trim()
    .parse::<u32>()
    .expect("tmux server pid is numeric");
    Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: format!("session-{pane_id}"),
        thread_anchor: format!("thread-{pane_id}"),
    }
}

fn register_context_in_project(
    fixture: &Fixture,
    project_root: &Path,
    pane: &Pane,
    operation_id: &str,
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
    let provide = json!({"session_id": pane.session_anchor}).to_string();
    let script = format!(
        "set -e\ncd {}\nunset CODEX_SESSION_ID CODEX_THREAD_ID CODEX_THREAD\n\"$COLLAB_BINARY\" context --op {} --provide {} >{} 2>{}\nprintf done >{}\nsleep 600\n",
        shell_quote(&project_root.to_string_lossy()),
        shell_quote(operation_id),
        shell_quote(&provide),
        shell_quote(&result.to_string_lossy()),
        shell_quote(&error.to_string_lossy()),
        shell_quote(&done.to_string_lossy())
    );
    fixture.run_script_in_pane_on_socket(&fixture.tmux_socket, pane, operation_id, &script);
    assert!(
        wait_until(Duration::from_secs(15), || done.exists()),
        "participant registration failed: {}",
        std::fs::read_to_string(error).unwrap_or_default()
    );
    let context = parse_json(&std::fs::read(result).expect("participant context result"));
    assert_eq!(context["result"]["outcome"], "completed", "{context}");
    context
}

fn identity(context: &Value) -> (String, String, String) {
    let snapshot = &context["result"]["snapshot"];
    (
        snapshot["identity"]["worker_id"]
            .as_str()
            .expect("worker id")
            .to_owned(),
        snapshot["binding"]["binding_id"]
            .as_str()
            .expect("binding id")
            .to_owned(),
        snapshot["binding"]["app_scope_id"]
            .as_str()
            .expect("app scope id")
            .to_owned(),
    )
}

fn approve_master(fixture: &Fixture, pane: &Pane) -> Value {
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "isolated stale identity maintenance black-box approval",
        ],
        Some(pane),
    )
}

fn kill_owned_session(fixture: &Fixture, session: &str) {
    let output = Command::new("tmux")
        .args([
            "-S",
            fixture.tmux_socket.to_str().unwrap(),
            "kill-session",
            "-t",
            session,
        ])
        .output()
        .expect("kill owned tmux session");
    assert!(
        output.status.success(),
        "kill owned tmux session {session}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn isolated_daemon_maintenance_retires_missing_peer_and_retains_live_peer() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, master) =
        Fixture::isolated_with_pane_command("stale-maint-master", "sleep 600");
    fixture.run_ok(&["context"], Some(&master));
    approve_master(&fixture, &master);
    let project_root = fixture.root.clone();

    let live_pane = create_session(&fixture, "stale-maint-live");
    let dead_pane = create_session(&fixture, "stale-maint-dead");
    let task_pane = create_session(&fixture, "stale-maint-task");
    let live_context =
        register_context_in_project(&fixture, &project_root, &live_pane, "live-peer");
    let dead_context =
        register_context_in_project(&fixture, &project_root, &dead_pane, "dead-peer");
    let task_context =
        register_context_in_project(&fixture, &project_root, &task_pane, "task-peer");
    let (live_id, _, app_scope_id) = identity(&live_context);
    let (dead_id, dead_binding_id, _) = identity(&dead_context);
    let (task_id, _, _) = identity(&task_context);

    assert_ne!(live_id, dead_id);
    assert_ne!(task_id, live_id);

    // Seed an unread durable message to the task peer. Its pane is killed after
    // the message is queued, so the retained side can afterwards read it through
    // the public interface without consuming it.
    let sent = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &task_id,
            "--subject",
            "stale-maint-retained-unread",
            "--delivery",
            "queued",
            "retain this unread mailbox",
        ],
        Some(&master),
    );
    let retained_msg_id = sent["msg_id"]
        .as_str()
        .or_else(|| sent["message_id"].as_str())
        .expect("retained message id")
        .to_owned();

    // Seed real business responsibility through the public CLI: an open task
    // and an unread durable message. The target must be retained when its
    // endpoint disappears.
    fixture.run_ok(
        &[
            "task",
            "register",
            "stale-maint-retained-task",
            "--owner",
            &task_id,
        ],
        Some(&task_pane),
    );
    // Only close the exact test-owned target sessions. The daemon, live peer
    // session, and tmux server remain intact.
    kill_owned_session(&fixture, "stale-maint-dead");
    kill_owned_session(&fixture, "stale-maint-task");
    fixture.restart_daemon();

    let retired = wait_until(Duration::from_secs(60), || {
        let workers = workers_for_route(&fixture, &project_root, &app_scope_id);
        workers["workers"]
            .as_array()
            .expect("workers array")
            .iter()
            .all(|worker| worker["id"] != dead_id)
    });
    assert!(retired, "dead peer must be retired by daemon maintenance");

    let live = workers_for_route(&fixture, &project_root, &app_scope_id);
    assert!(
        live["workers"]
            .as_array()
            .expect("workers array")
            .iter()
            .any(|row| row["id"] == live_id && row["endpoint_live"] == Value::Bool(true)),
        "live peer must stay live after maintenance: {live}"
    );

    assert!(
        live["workers"]
            .as_array()
            .expect("workers array")
            .iter()
            .any(|row| row["id"] == task_id),
        "responsibility-bearing dead peer must be retained: {live}"
    );
    let task = fixture.run_ok(
        &["task", "status", "stale-maint-retained-task"],
        Some(&master),
    );
    assert_eq!(task["id"], "stale-maint-retained-task", "{task}");

    let retained_msg = fixture.run_ok(&["msg", &retained_msg_id], Some(&master));
    assert_eq!(retained_msg["to"], task_id, "{retained_msg}");
    assert_eq!(retained_msg["state"], "pending", "{retained_msg}");
    assert_eq!(
        retained_msg["consumed_by_recv"], false,
        "readable status must not consume the retained mailbox: {retained_msg}"
    );

    let retired_read = fixture.run_ok(&["worker", "read", &dead_id], Some(&master));
    assert_eq!(
        retired_read["result"]["outcome"], "closed",
        "public lifecycle read must report the closed target: {retired_read}"
    );
    assert_eq!(
        retired_read["result"]["close"]["worker_retirement"]["state"], "verified",
        "{retired_read}"
    );
    assert_eq!(
        retired_read["result"]["close"]["binding_retirement"]["state"], "verified",
        "{retired_read}"
    );
    assert_eq!(
        retired_read["result"]["close"]["close_outcome"], "complete",
        "{retired_read}"
    );
    let route_retirement = retired_read["result"]["close"]["route_retirement"]["state"]
        .as_str()
        .unwrap_or_default();
    assert!(
        matches!(route_retirement, "verified" | "not-applicable"),
        "route must be retired or explicitly absent: {retired_read}"
    );

    // Route/binding retirement is observable through the public journal.
    let journal = std::fs::read_to_string(fixture.project_journal()).expect("read journal");
    assert!(
        journal.contains(&format!("\"binding_id\":\"{dead_binding_id}\"")),
        "retired binding remains in durable journal"
    );

    kill_owned_session(&fixture, "stale-maint-live");
    std::fs::remove_dir_all(&fixture.root).ok();
}
