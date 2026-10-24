include!("support/context_operation_fixture.rs");

fn parse_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("collab CLI emits JSON")
}

fn ceo_wire_request(fixture: &Fixture, request: Value) -> Value {
    let mut stream = UnixStream::connect(fixture.host_state.join("server.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .write_all(serde_json::to_string(&request).unwrap().as_bytes())
        .unwrap();
    stream.write_all(b"\n").unwrap();
    stream.flush().unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn durable_surfaces(fixture: &Fixture) -> Vec<(String, Vec<u8>)> {
    let mut surfaces = Vec::new();
    for (label, path) in [
        ("host_journal", fixture.host_journal()),
        ("host_routes", fixture.host_routes()),
        ("project_journal", fixture.project_journal()),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            surfaces.push((label.to_owned(), bytes));
        }
    }
    surfaces
}

fn lock_tmux() -> std::sync::MutexGuard<'static, ()> {
    TMUX_FIXTURE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn tmux_windows(fixture: &Fixture, session: &str) -> Vec<String> {
    let output = tmux(
        &fixture.tmux_socket,
        &["list-windows", "-t", session, "-F", "#{window_id}"],
    );
    String::from_utf8(output.stdout)
        .expect("window ids are UTF-8")
        .lines()
        .map(str::to_owned)
        .collect()
}

fn set_tmux_environment(fixture: &Fixture, session: &str, key: &str, value: &str) {
    tmux(
        &fixture.tmux_socket,
        &["set-environment", "-t", session, key, value],
    );
}

fn install_fake_codex(fixture: &Fixture, log: &Path) -> PathBuf {
    let bin = fixture.root.join("fake-codex-bin");
    std::fs::create_dir_all(&bin).expect("create fake codex bin");
    let script = bin.join("codex");
    let body = format!(
        "#!/bin/sh\n{{\n\
         printf 'arg=%s\\n' \"$@\"\n\
         printf 'ENV_CODEX_SESSION_ID=%s\\n' \"${{CODEX_SESSION_ID+present}}\"\n\
         printf 'ENV_CODEX_THREAD_ID=%s\\n' \"${{CODEX_THREAD_ID+present}}\"\n\
         printf 'ENV_CODEX_INTERNAL_ORIGINATOR_OVERRIDE=%s\\n' \"${{CODEX_INTERNAL_ORIGINATOR_OVERRIDE+present}}\"\n\
         printf 'ENV_CODEX_APP_TOOLS_PIPE_PATH=%s\\n' \"${{CODEX_APP_TOOLS_PIPE_PATH+present}}\"\n\
         }} > {}\n\
         sleep 300\n",
        shell_quote(log.to_str().expect("fake log path is UTF-8"))
    );
    std::fs::write(&script, body).expect("write fake codex");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("make fake codex executable");
    bin
}

fn with_fake_codex_path(bin: &Path) -> String {
    let existing = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_owned());
    format!("{}:{existing}", bin.display())
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

fn run_at(
    fixture: &Fixture,
    project_root: &Path,
    pane: &Pane,
    args: &[&str],
    no_native_identity: bool,
) -> std::process::Output {
    command_at(fixture, project_root, pane, args, no_native_identity)
        .output()
        .expect("run public collab CLI in isolated project")
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

fn identity(context: &Value) -> String {
    context["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("registered identity worker id")
        .to_owned()
}

fn received_message<'a>(received: &'a Value, msg_id: &str) -> &'a Value {
    received["messages"]
        .as_array()
        .expect("recv messages array")
        .iter()
        .find(|message| message["id"] == msg_id)
        .unwrap_or_else(|| panic!("message {msg_id} missing from {received}"))
}

#[test]
fn ceo_status_reads_host_without_registration_and_reports_default_workspace() {
    let fixture = Fixture::without_tmux("ceo-status-unregistered");
    let before = durable_surfaces(&fixture);
    let output = fixture.command(&["ceo", "status"], None);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let status = parse_json(&output.stdout);
    assert!(status["ceo"].is_null(), "{status}");
    let default_workspace = status["default_workspace"]
        .as_str()
        .expect("CEO status reports the default workspace");
    assert!(
        default_workspace.ends_with("/workspace"),
        "unexpected default workspace: {default_workspace}"
    );
    assert_eq!(status["is_current"], false, "{status}");
    let unrelated = fixture.root.join("unregistered-directory");
    std::fs::create_dir(&unrelated).unwrap();
    let outside = fixture
        .configured_command(&["ceo", "status"], None)
        .current_dir(&unrelated)
        .output()
        .unwrap();
    assert!(
        outside.status.success(),
        "global status required a registered cwd: {outside:?}"
    );
    assert_eq!(parse_json(&outside.stdout)["is_current"], false);
    assert_eq!(
        durable_surfaces(&fixture),
        before,
        "status registered or mutated a project"
    );
}

#[test]
fn ceo_approval_and_reply_flags_fail_before_identity_or_state_writes() {
    let fixture = Fixture::without_tmux("ceo-args");
    let before = durable_surfaces(&fixture);

    let empty_approval = fixture.command(&["ceo", "promote", "--approval", ""], None);
    assert!(!empty_approval.status.success(), "{empty_approval:?}");
    let stderr = String::from_utf8_lossy(&empty_approval.stderr).to_ascii_lowercase();
    assert!(
        stderr.contains("approval"),
        "empty approval must name the approval requirement: {stderr}"
    );

    let conflict = fixture.command(
        &[
            "ceo",
            "send",
            "--project",
            fixture.root.to_str().unwrap(),
            "--subject",
            "x",
            "--reply-required",
            "--reply-on-completion",
            "body",
        ],
        None,
    );
    assert!(!conflict.status.success());
    let conflict_stderr = String::from_utf8_lossy(&conflict.stderr);
    assert!(
        conflict_stderr.contains("cannot be used with"),
        "reply flags must be mutually exclusive before identity lookup: {conflict_stderr}"
    );

    assert_eq!(
        durable_surfaces(&fixture),
        before,
        "argument validation changed durable host state"
    );
}

#[test]
fn ceo_mutations_without_registered_identity_fail_without_side_effects() {
    let fixture = Fixture::without_tmux("ceo-no-identity");
    let before = durable_surfaces(&fixture);
    for args in [
        vec![
            "ceo",
            "send",
            "--project",
            fixture.root.to_str().unwrap(),
            "--subject",
            "x",
            "body",
        ],
        vec![
            "ceo",
            "append-task",
            "--project",
            fixture.root.to_str().unwrap(),
            "--id",
            "task-x",
            "--title",
            "title",
            "--description",
            "description",
            "--delivery-condition",
            "deliver",
            "--test-condition",
            "test",
        ],
        vec!["ceo", "reply", "--subject", "x", "body"],
        vec!["ceo", "progress"],
        vec![
            "ceo",
            "bootstrap-ready",
            "--op",
            "op-x",
            "--subject",
            "ready",
            "body",
        ],
    ] {
        let output = fixture.command(&args, None);
        assert!(!output.status.success(), "unexpected success for {args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        assert!(
            stderr.contains("identity") || stderr.contains("session"),
            "expected an explicit identity failure for {args:?}: {stderr}"
        );
    }
    assert_eq!(
        durable_surfaces(&fixture),
        before,
        "unregistered CEO mutations changed durable host state"
    );
}

#[test]
fn ceo_cross_project_task_message_reply_progress_and_clear_roundtrip() {
    let _guard = lock_tmux();
    let (mut fixture, ceo_pane) = Fixture::isolated_with_pane_command("ceo-roundtrip", "sleep 600");
    let ceo_root = fixture.root.canonicalize().unwrap();
    let target_root = create_external_project(&fixture, "target");
    let target_pane = create_session(&fixture, "ceo-roundtrip-target");

    let ceo_context = register_context(&fixture, &ceo_root, &ceo_pane, "ceo-register", false);
    let ceo_id = identity(&ceo_context);
    let target_context = register_context(
        &fixture,
        &target_root,
        &target_pane,
        "target-register",
        false,
    );
    let target_id = identity(&target_context);

    let promoted = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "promote",
            "--approval",
            "user explicitly approved the isolated CEO",
        ],
        false,
    );
    assert_eq!(promoted["is_current"], true, "{promoted}");
    let status = run_ok_at(&fixture, &ceo_root, &ceo_pane, &["ceo", "status"], false);
    assert_eq!(status["is_current"], true, "{status}");
    assert_eq!(status["worker_id"], ceo_id, "{status}");
    let current = run_ok_at(&fixture, &ceo_root, &ceo_pane, &["context"], false);
    let current = &current["result"]["snapshot"];
    assert_eq!(current["ceo"]["is_current"], true, "{current}");
    assert_eq!(
        current["identity"]["role"], "worker",
        "global CEO must not acquire project master authority"
    );
    assert!(
        current["ceo_brief"]["responsibilities"].is_array(),
        "{current}"
    );
    let forged = ceo_wire_request(
        &fixture,
        json!({
            "op":"Ceo","worker_id":ceo_id,"token":"forged-token",
            "command":{"action":"send","project":target_root,"subject":"forged","body":"must not enqueue"},
            "project_context":{"app_scope_id":"appserver-cli","canonical_root":ceo_root,"project_scope":ceo_root}
        }),
    );
    assert_eq!(
        forged["ok"], false,
        "a supplied CEO id without its credential was trusted: {forged}"
    );
    run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &["context", "--work-roles", "ceo"],
        false,
    );
    let specialty = run_at(
        &fixture,
        &target_root,
        &target_pane,
        &["ceo", "progress"],
        false,
    );
    assert!(
        !specialty.status.success(),
        "a ceo work role granted global authority: {specialty:?}"
    );

    run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &[
            "master",
            "promote",
            "--approval",
            "user explicitly approved the isolated target master",
        ],
        false,
    );

    let sent = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "send",
            "--project",
            target_root.to_str().unwrap(),
            "--subject",
            "action",
            "--reply-required",
            "please return an exact result",
        ],
        false,
    );
    let msg_id = sent["msg_id"].as_str().expect("CEO send message id");
    let received = run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &["recv", "--timeout", "0"],
        false,
    );
    let request = received_message(&received, msg_id);
    assert_eq!(
        request["body"], "please return an exact result",
        "{request}"
    );

    let reply = run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &[
            "ceo",
            "reply",
            "--subject",
            "result",
            "--in-reply-to",
            msg_id,
            "verified result",
        ],
        false,
    );
    let reply_id = reply["msg_id"].as_str().expect("CEO reply message id");
    let repeated_reply = run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &[
            "ceo",
            "reply",
            "--subject",
            "result",
            "--in-reply-to",
            msg_id,
            "verified result",
        ],
        false,
    );
    assert_eq!(
        repeated_reply["msg_id"], reply_id,
        "identical reply created a second message: {repeated_reply}"
    );
    let after_reply = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "progress",
            "--project",
            target_root.to_str().unwrap(),
        ],
        false,
    );
    assert!(
        !after_reply["projects"][0]["pending_replies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["message_id"] == msg_id),
        "actual reply left the original marker: {after_reply}"
    );
    let ceo_received = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &["recv", "--timeout", "0"],
        false,
    );
    assert_eq!(
        received_message(&ceo_received, reply_id)["body"],
        "verified result",
        "{ceo_received}"
    );

    let appended = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "append-task",
            "--project",
            target_root.to_str().unwrap(),
            "--id",
            "ceo-task-1",
            "--title",
            "CEO assigned task",
            "--description",
            "Implement the requested behavior.",
            "--delivery-condition",
            "Public behavior is verified.",
            "--test-condition",
            "The targeted public test passes.",
            "--priority",
            "p1",
            "--reply-on-completion",
        ],
        false,
    );
    assert_eq!(appended["task_id"], "ceo-task-1", "{appended}");
    let target_received = run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &["recv", "--timeout", "0"],
        false,
    );
    let task_message_id = appended["msg_id"].as_str().expect("append-task message id");
    received_message(&target_received, task_message_id);
    let task = run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &["task", "status", "ceo-task-1"],
        false,
    );
    assert_eq!(task["owner"], target_id, "{task}");
    assert_eq!(task["priority"], "p1", "{task}");
    let board = run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &["board", "show"],
        false,
    );
    let board_task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "ceo-task-1")
        .expect("appended task is on the original project board");
    assert_eq!(board_task["title"], "CEO assigned task", "{board}");

    let progress = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "progress",
            "--project",
            target_root.to_str().unwrap(),
        ],
        false,
    );
    let progress_text = progress.to_string();
    assert!(progress_text.contains(&target_id), "{progress}");
    assert!(progress_text.contains("ceo-task-1"), "{progress}");
    run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &[
            "ceo",
            "reply",
            "--subject",
            "still working",
            "ordinary progress does not satisfy completion",
        ],
        false,
    );
    let waiting = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "progress",
            "--project",
            target_root.to_str().unwrap(),
        ],
        false,
    );
    assert!(
        waiting["projects"][0]["pending_replies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["message_id"] == task_message_id),
        "ordinary progress cleared completion marker: {waiting}"
    );
    fixture.restart_daemon();
    let replayed = run_ok_at(&fixture, &ceo_root, &ceo_pane, &["ceo", "status"], false);
    assert_eq!(
        replayed["is_current"], true,
        "global grant did not survive daemon replay: {replayed}"
    );
    let resumed = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "progress",
            "--project",
            target_root.to_str().unwrap(),
        ],
        false,
    );
    assert!(
        resumed["projects"][0]["pending_replies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["message_id"] == task_message_id),
        "replay lost completion marker: {resumed}"
    );

    run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "clear",
            "--approval",
            "user explicitly approved clearing the isolated CEO",
        ],
        false,
    );
    let cleared = run_ok_at(&fixture, &ceo_root, &ceo_pane, &["ceo", "status"], false);
    assert_eq!(cleared["is_current"], false, "{cleared}");
    let rejected = run_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "send",
            "--project",
            target_root.to_str().unwrap(),
            "--subject",
            "unauthorized",
            "must not enqueue",
        ],
        false,
    );
    assert!(!rejected.status.success(), "{rejected:?}");
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
        "{empty}"
    );

    drop(fixture);
    std::fs::remove_dir_all(target_root).unwrap();
}

#[test]
fn ceo_append_task_without_target_master_is_rejected_without_task_or_message() {
    let _guard = lock_tmux();
    let (fixture, ceo_pane) = Fixture::isolated_with_pane_command("ceo-no-master", "sleep 600");
    let ceo_root = fixture.root.canonicalize().unwrap();
    let target_root = create_external_project(&fixture, "no-master-target");
    let target_pane = create_session(&fixture, "ceo-no-master-target");
    register_context(
        &fixture,
        &ceo_root,
        &ceo_pane,
        "ceo-no-master-register",
        false,
    );
    register_context(
        &fixture,
        &target_root,
        &target_pane,
        "target-no-master-register",
        false,
    );
    run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "promote",
            "--approval",
            "user explicitly approved the isolated CEO",
        ],
        false,
    );
    let rejected = run_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "append-task",
            "--project",
            target_root.to_str().unwrap(),
            "--id",
            "must-not-exist",
            "--title",
            "no master",
            "--description",
            "This must be rejected.",
            "--delivery-condition",
            "never",
            "--test-condition",
            "never",
        ],
        false,
    );
    assert!(!rejected.status.success(), "{rejected:?}");
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        stderr.to_ascii_lowercase().contains("master"),
        "no-master rejection must name the missing master: {stderr}"
    );
    let inbox = run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &["recv", "--timeout", "0"],
        false,
    );
    assert_eq!(
        inbox["messages"].as_array().map(Vec::len),
        Some(0),
        "{inbox}"
    );
    let task = run_at(
        &fixture,
        &target_root,
        &target_pane,
        &["task", "status", "must-not-exist"],
        false,
    );
    assert!(
        !task.status.success(),
        "rejected append-task created {task:?}"
    );

    drop(fixture);
    std::fs::remove_dir_all(target_root).unwrap();
}

#[test]
fn ceo_bootstrap_launches_fake_consumer_then_requires_real_master_handshake() {
    let _guard = lock_tmux();
    let session = "ceo-bootstrap-session";
    let (fixture, ceo_pane) = Fixture::isolated_with_pane_command(session, "sleep 600");
    let ceo_root = fixture.root.canonicalize().unwrap();
    register_context(
        &fixture,
        &ceo_root,
        &ceo_pane,
        "bootstrap-ceo-register",
        false,
    );
    run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "promote",
            "--approval",
            "user explicitly approved the isolated CEO",
        ],
        false,
    );

    let target_root = create_external_project(&fixture, "bootstrap-target");
    let target_pane = create_session(&fixture, "bootstrap-target-pane");
    let fake_log = fixture.root.join("fake-codex.log");
    let bin = install_fake_codex(&fixture, &fake_log);
    set_tmux_environment(&fixture, session, "PATH", &with_fake_codex_path(&bin));
    set_tmux_environment(&fixture, session, "CODEX_SESSION_ID", "parent-session");
    set_tmux_environment(&fixture, session, "CODEX_THREAD_ID", "parent-thread");
    set_tmux_environment(
        &fixture,
        session,
        "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
        "parent-originator",
    );
    set_tmux_environment(
        &fixture,
        session,
        "CODEX_APP_TOOLS_PIPE_PATH",
        "/tmp/parent-pipe",
    );
    let windows_before = tmux_windows(&fixture, session);
    let approval = "user explicitly approved the isolated bootstrap master";
    let operation_id = "bootstrap-op-1";
    let launched = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "bootstrap-master",
            "--project",
            target_root.to_str().unwrap(),
            "--approval",
            approval,
            "--tmux-socket",
            fixture.tmux_socket.to_str().unwrap(),
            "--tmux-session",
            session,
            "--op",
            operation_id,
        ],
        false,
    );
    assert_eq!(launched["phase"], "launched", "{launched}");
    assert!(
        wait_until(Duration::from_secs(15), || fake_log.exists()),
        "fake codex did not receive the launched command: {launched}"
    );
    let windows_after = tmux_windows(&fixture, session);
    assert_eq!(
        windows_after.len(),
        windows_before.len() + 1,
        "bootstrap must create exactly one window"
    );
    let argv = std::fs::read_to_string(&fake_log).expect("read fake codex log");
    assert!(
        argv.contains("arg=--dangerously-bypass-approvals-and-sandbox"),
        "{argv}"
    );
    assert!(argv.contains("arg=--profile\narg=gcm"), "{argv}");
    assert!(argv.contains("arg=--cd"), "{argv}");
    assert!(argv.contains(target_root.to_str().unwrap()), "{argv}");
    assert!(argv.contains("context"), "{argv}");
    assert!(argv.contains("master promote"), "{argv}");
    assert!(argv.contains("ceo bootstrap-ready"), "{argv}");
    assert!(argv.contains(operation_id), "{argv}");
    assert!(argv.contains(approval), "{argv}");
    for marker in [
        "ENV_CODEX_SESSION_ID=present",
        "ENV_CODEX_THREAD_ID=present",
        "ENV_CODEX_INTERNAL_ORIGINATOR_OVERRIDE=present",
        "ENV_CODEX_APP_TOOLS_PIPE_PATH=present",
    ] {
        assert!(
            !argv.contains(marker),
            "parent environment leaked: {marker}\n{argv}"
        );
    }

    let replay = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "bootstrap-master",
            "--project",
            target_root.to_str().unwrap(),
            "--approval",
            approval,
            "--tmux-socket",
            fixture.tmux_socket.to_str().unwrap(),
            "--tmux-session",
            session,
            "--op",
            operation_id,
        ],
        false,
    );
    assert_eq!(replay["should_launch"], false, "{replay}");
    assert_eq!(
        tmux_windows(&fixture, session).len(),
        windows_after.len(),
        "same operation must not create another window"
    );
    assert_eq!(
        std::fs::read_to_string(&fake_log).unwrap(),
        argv,
        "same operation must not launch fake codex again"
    );

    let target_context = register_context(
        &fixture,
        &target_root,
        &target_pane,
        "bootstrap-target-register",
        false,
    );
    let target_id = identity(&target_context);
    run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &[
            "master",
            "promote",
            "--approval",
            "user explicitly approved the isolated target master",
        ],
        false,
    );
    let wrong_pane_ready = run_at(
        &fixture,
        &target_root,
        &target_pane,
        &[
            "ceo",
            "bootstrap-ready",
            "--op",
            operation_id,
            "--subject",
            "wrong pane",
            "another pane is not the bootstrapped Codex",
        ],
        false,
    );
    assert!(
        !wrong_pane_ready.status.success(),
        "an unrelated master pane completed bootstrap: {wrong_pane_ready:?}"
    );
    run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &[
            "master",
            "clear",
            "--approval",
            "user approves replacing the isolated wrong-pane master",
        ],
        false,
    );
    let actual_pane_id = launched["operation"]["pane_id"]
        .as_str()
        .expect("actual launched pane")
        .to_owned();
    let target_pane = Pane {
        server_pid: ceo_pane.server_pid,
        session_anchor: format!("isolated-session-{actual_pane_id}"),
        thread_anchor: format!("isolated-thread-{actual_pane_id}"),
        pane_id: actual_pane_id,
    };
    let target_context = register_context(
        &fixture,
        &target_root,
        &target_pane,
        "bootstrap-actual-master-register",
        false,
    );
    let target_id = identity(&target_context);
    run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &[
            "master",
            "promote",
            "--approval",
            "user explicitly approved the launched isolated target master",
        ],
        false,
    );
    let ready = run_ok_at(
        &fixture,
        &target_root,
        &target_pane,
        &[
            "ceo",
            "bootstrap-ready",
            "--op",
            operation_id,
            "--subject",
            "bootstrap ready",
            "registered, promoted and ready for work",
        ],
        false,
    );
    assert_eq!(ready["phase"], "ready", "{ready}");
    assert!(ready.to_string().contains("ready"), "{ready}");
    let handshake = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &["recv", "--timeout", "0"],
        false,
    );
    let message = handshake["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["subject"] == "bootstrap ready")
        .unwrap_or_else(|| panic!("bootstrap handshake missing: {handshake}"));
    assert_eq!(message["from"], format!("Jenny@{}", target_root.display()));
    assert_eq!(
        ready["operation"]["project"],
        target_root.to_string_lossy().as_ref()
    );
    let progress = run_ok_at(&fixture, &ceo_root, &ceo_pane, &["ceo", "progress"], false);
    assert!(progress.to_string().contains(operation_id), "{progress}");
    assert!(progress.to_string().contains("ready"), "{progress}");

    let other_root = create_external_project(&fixture, "bootstrap-other");
    let other_pane = create_session(&fixture, "bootstrap-other-pane");
    register_context(
        &fixture,
        &other_root,
        &other_pane,
        "bootstrap-other-register",
        false,
    );
    run_ok_at(
        &fixture,
        &other_root,
        &other_pane,
        &[
            "master",
            "promote",
            "--approval",
            "user explicitly approved the unrelated master",
        ],
        false,
    );
    let rejected = run_at(
        &fixture,
        &other_root,
        &other_pane,
        &[
            "ceo",
            "bootstrap-ready",
            "--op",
            operation_id,
            "--subject",
            "invalid ready",
            "this project does not own the operation",
        ],
        false,
    );
    assert!(
        !rejected.status.success(),
        "unrelated master accepted ready: {rejected:?}"
    );
    let rejection = format!(
        "{}{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    )
    .to_ascii_lowercase();
    assert!(
        rejection.contains("master")
            || rejection.contains("project")
            || rejection.contains("operation"),
        "ready rejection must identify the ownership mismatch: {rejection}"
    );

    drop(fixture);
    std::fs::remove_dir_all(target_root).unwrap();
    std::fs::remove_dir_all(other_root).unwrap();
}

#[test]
fn ceo_bootstrap_quotes_approval_profile_and_project_without_execution() {
    let _guard = lock_tmux();
    let session = "ceo-bootstrap-quote-session";
    let (fixture, ceo_pane) = Fixture::isolated_with_pane_command(session, "sleep 600");
    let ceo_root = fixture.root.canonicalize().unwrap();
    register_context(&fixture, &ceo_root, &ceo_pane, "quote-ceo-register", false);
    run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "promote",
            "--approval",
            "user explicitly approved the isolated CEO",
        ],
        false,
    );
    let target_root = create_external_project(&fixture, "space target 'quoted");
    let marker = fixture.root.join("ceo-injection-marker");
    let fake_log = fixture.root.join("fake-codex-quote.log");
    let bin = install_fake_codex(&fixture, &fake_log);
    set_tmux_environment(&fixture, session, "PATH", &with_fake_codex_path(&bin));
    let approval = format!(
        "user approved; printf pwn; touch {}",
        marker.to_str().unwrap()
    );
    let profile = format!("custom-profile; touch {}", marker.to_str().unwrap());
    let launched = run_ok_at(
        &fixture,
        &ceo_root,
        &ceo_pane,
        &[
            "ceo",
            "bootstrap-master",
            "--project",
            target_root.to_str().unwrap(),
            "--approval",
            &approval,
            "--profile",
            &profile,
            "--tmux-socket",
            fixture.tmux_socket.to_str().unwrap(),
            "--tmux-session",
            session,
            "--op",
            "bootstrap-quote-op",
        ],
        false,
    );
    assert_eq!(launched["phase"], "launched", "{launched}");
    assert!(
        wait_until(Duration::from_secs(15), || fake_log.exists()),
        "fake codex did not receive the quoted command"
    );
    assert!(
        !marker.exists(),
        "approval/profile/path interpolation executed a command"
    );
    let argv = std::fs::read_to_string(&fake_log).unwrap();
    assert!(argv.contains(&format!("arg={profile}")), "{argv}");
    assert!(argv.contains(&approval), "{argv}");
    assert!(argv.contains(target_root.to_str().unwrap()), "{argv}");

    drop(fixture);
    std::fs::remove_dir_all(target_root).unwrap();
}
