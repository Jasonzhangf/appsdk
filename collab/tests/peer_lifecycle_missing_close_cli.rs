include!("support/context_operation_fixture.rs");

struct OwnedTmuxSession {
    socket: PathBuf,
    session: String,
}

impl OwnedTmuxSession {
    fn kill(&mut self) {
        if self.socket.exists() {
            let output = Command::new("tmux")
                .args([
                    "-S",
                    self.socket.to_str().unwrap(),
                    "kill-session",
                    "-t",
                    &self.session,
                ])
                .output()
                .expect("close owned tmux session");
            assert!(
                output.status.success(),
                "close owned tmux session failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    fn wait_for_server_exit(&self) {
        assert!(
            wait_until(Duration::from_secs(5), || {
                !Command::new("tmux")
                    .args(["-S", self.socket.to_str().unwrap(), "list-sessions"])
                    .output()
                    .is_ok_and(|output| output.status.success())
            }),
            "owned tmux server did not exit after its final session closed"
        );
    }
}

impl Drop for OwnedTmuxSession {
    fn drop(&mut self) {
        if self.socket.exists() {
            let _ = Command::new("tmux")
                .args([
                    "-S",
                    self.socket.to_str().unwrap(),
                    "kill-session",
                    "-t",
                    &self.session,
                ])
                .output();
        }
    }
}

fn parse_json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("collab CLI emits JSON")
}

fn create_session_on_socket(fixture: &Fixture, label: &str, socket: &Path) -> Pane {
    let output = Command::new("tmux")
        .args([
            "-S",
            socket.to_str().unwrap(),
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
    fixture.declare_tmux_endpoint(socket);
    let pane_id = String::from_utf8(
        Command::new("tmux")
            .args([
                "-S",
                socket.to_str().unwrap(),
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
                socket.to_str().unwrap(),
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
    fixture.start_pane_runner_on_socket(socket, &pane.pane_id, "sleep 600");
    pane
}

fn run_script_in_pane_on_socket(
    fixture: &Fixture,
    socket: &Path,
    pane: &Pane,
    label: &str,
    script: &str,
) {
    fixture.run_script_in_pane_on_socket(socket, pane, label, script);
}

fn register_context_in_project(
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

fn register_context_on_socket(
    fixture: &Fixture,
    socket: &Path,
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
    run_script_in_pane_on_socket(fixture, socket, pane, operation_id, &script);
    assert!(
        wait_until(Duration::from_secs(15), || done.exists()),
        "participant registration failed: {}",
        std::fs::read_to_string(error).unwrap_or_default()
    );
    let context = parse_json(&std::fs::read(result).expect("participant context result"));
    assert_eq!(context["result"]["outcome"], "completed", "{context}");
    context
}

fn promote_master(fixture: &Fixture, project_root: &Path, pane: &Pane) -> Value {
    let output = fixture
        .configured_command(
            &[
                "master",
                "promote",
                "--approval",
                "isolated route-less peer close acceptance",
            ],
            Some(pane),
        )
        .current_dir(project_root)
        .output()
        .expect("promote fixture master");
    assert!(
        output.status.success(),
        "master promote failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_json(&output.stdout)
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

fn central_runtime_journal(fixture: &Fixture, project_root: &Path) -> PathBuf {
    assert_eq!(
        std::fs::canonicalize(project_root).expect("canonicalize project root"),
        fixture.root.canonicalize().expect("canonical fixture root")
    );
    fixture.project_journal()
}

fn retire_binding_route(fixture: &mut Fixture, project_root: &Path, binding_id: &str) {
    let journal_path = central_runtime_journal(fixture, project_root);
    let binding = std::fs::read_to_string(&journal_path)
        .expect("read project journal")
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| {
            record["ev"] == "GlobalCurrentThreadRouteSet"
                && record["binding"]["binding_id"] == binding_id
        })
        .last()
        .and_then(|record| record.get("binding").cloned())
        .expect("target route publication");
    fixture.stop_daemon();
    let mut journal = std::fs::OpenOptions::new()
        .append(true)
        .open(&journal_path)
        .expect("open project journal for typed replay");
    serde_json::to_writer(
        &mut journal,
        &json!({"ev": "GlobalCurrentThreadRouteRetired", "binding": binding}),
    )
    .expect("serialize route retirement");
    journal.write_all(b"\n").expect("append route retirement");
    drop(journal);
    fixture.restart_daemon();
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

fn worker_row(workers: &Value, worker_id: &str) -> Value {
    workers["workers"]
        .as_array()
        .expect("workers array")
        .iter()
        .find(|worker| worker["id"] == worker_id)
        .cloned()
        .unwrap_or_else(|| panic!("worker {worker_id} missing from {workers}"))
}

fn read_exact(fixture: &Fixture, master: &Pane, target_id: &str) -> Value {
    fixture.run_ok(&["worker", "read", target_id], Some(master))
}

fn assert_not_retired(
    fixture: &Fixture,
    master: &Pane,
    project_root: &Path,
    app_scope_id: &str,
    target_id: &str,
    binding_id: &str,
    close: &Value,
    expected_runtime_archive: &str,
) {
    let result = &close["result"];
    assert_eq!(
        result["close"]["runtime_archive"]["state"], expected_runtime_archive,
        "{close}"
    );
    for stage in [
        "worker_retirement",
        "binding_retirement",
        "route_retirement",
        "subscription_retirement",
    ] {
        assert_eq!(
            result["close"][stage]["state"], "not-attempted",
            "{stage} must not run after a non-terminal host probe: {close}"
        );
    }
    let workers = workers_for_route(fixture, project_root, app_scope_id);
    let row = worker_row(&workers, target_id);
    assert_eq!(row["transport"]["kind"], "tmux", "{row}");
    let read = read_exact(fixture, master, target_id);
    assert_eq!(read["result"]["target"]["binding_id"], binding_id, "{read}");
}

fn close_target(fixture: &Fixture, master: &Pane, target_id: &str, reason: &str) -> Value {
    let output = fixture
        .configured_command(
            &["worker", "close", target_id, "--reason", reason],
            Some(master),
        )
        .output()
        .expect("run public worker close");
    assert!(
        !output.stdout.is_empty(),
        "worker close returned no typed receipt: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    parse_json(&output.stdout)
}

fn query_operation(fixture: &Fixture, master: &Pane, operation_id: &str) -> Value {
    let output = fixture
        .configured_command(&["worker", "query", "--op", operation_id], Some(master))
        .output()
        .expect("run public worker query");
    assert!(
        !output.stdout.is_empty(),
        "worker query returned no typed receipt: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    parse_json(&output.stdout)
}

fn setup_master_and_route_less_target(label: &str) -> (Fixture, Pane, Pane, Value) {
    let (mut fixture, master) = Fixture::isolated_with_pane_command(label, "sleep 600");
    fixture.run_ok(&["context"], Some(&master));
    promote_master(&fixture, &fixture.root, &master);

    let target = create_session_on_socket(&fixture, "route-less-target", &fixture.tmux_socket);
    fixture.track_tmux_session("route-less-target");
    let target_context = register_context_in_project(
        &fixture,
        &fixture.root,
        &target,
        &format!("{label}-target"),
        true,
    );
    let (_, binding_id, _) = identity(&target_context);
    let project_root = fixture.root.clone();
    retire_binding_route(&mut fixture, &project_root, &binding_id);
    (fixture, master, target, target_context)
}

#[test]
fn close_retires_a_route_less_tmux_peer_after_its_exact_pane_is_missing() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, master, target, target_context) =
        setup_master_and_route_less_target("stale-peer-close");
    let (target_id, binding_id, app_scope_id) = identity(&target_context);

    let workers = workers_for_route(&fixture, &fixture.root, &app_scope_id);
    let row = worker_row(&workers, &target_id);
    // This fixture proves a durable identity and live endpoint. Its runner
    // does not prove agent presence; the close must still probe the endpoint.
    assert_eq!(row["identity_valid"], true, "{row}");
    assert_eq!(row["endpoint_live"], true, "{row}");

    let route = fixture
        .command_on_socket(
            &fixture.tmux_socket,
            &["route", "resolve", "--pane-id", &target.pane_id],
            &target,
        )
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_THREAD")
        .output()
        .expect("run route resolve");
    assert!(
        !route.status.success()
            && String::from_utf8_lossy(&route.stderr).contains("ROUTE_RESOLVE_NOT_FOUND"),
        "route must be absent before the missing-pane close: stdout={} stderr={}",
        String::from_utf8_lossy(&route.stdout),
        String::from_utf8_lossy(&route.stderr)
    );

    let killed = Command::new("tmux")
        .args([
            "-S",
            fixture.tmux_socket.to_str().unwrap(),
            "kill-session",
            "-t",
            "route-less-target",
        ])
        .output()
        .expect("close owned route-less target session");
    assert!(
        killed.status.success(),
        "close owned route-less target session failed: {}",
        String::from_utf8_lossy(&killed.stderr)
    );

    let close = close_target(
        &fixture,
        &master,
        &target_id,
        "remove route-less no-session binding causing tombstone poison",
    );
    assert_eq!(close["ok"], true, "{close}");
    assert_eq!(close["result"]["outcome"], "complete", "{close}");
    assert_eq!(close["result"]["phase"], "complete", "{close}");
    assert_eq!(
        close["result"]["target"]["binding_id"], binding_id,
        "{close}"
    );
    assert_eq!(
        close["result"]["close"]["runtime_archive"]["state"], "missing",
        "{close}"
    );
    assert_eq!(
        close["result"]["close"]["worker_retirement"]["state"], "verified",
        "{close}"
    );
    assert_eq!(
        close["result"]["close"]["binding_retirement"]["state"], "verified",
        "{close}"
    );
    assert_eq!(
        close["result"]["close"]["route_retirement"]["state"], "not-applicable",
        "{close}"
    );
    assert_eq!(
        close["result"]["close"]["subscription_retirement"]["state"], "verified",
        "{close}"
    );

    let operation_id = close["result"]["operation_id"]
        .as_str()
        .expect("close operation id")
        .to_owned();
    let queried = query_operation(&fixture, &master, &operation_id);
    assert_eq!(queried["result"], close["result"], "{queried}");

    fixture.stop_daemon();
    fixture.restart_daemon();
    let replayed = query_operation(&fixture, &master, &operation_id);
    assert_eq!(replayed["result"], close["result"], "{replayed}");

    let workers = workers_for_route(&fixture, &fixture.root, &app_scope_id);
    assert!(
        workers["workers"]
            .as_array()
            .expect("workers array")
            .iter()
            .all(|worker| worker["id"] != target_id),
        "retired worker still listed after replay: {workers}"
    );
}

#[test]
fn close_route_less_present_target_stays_cleanup_open_without_retirement() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, master, _target, target_context) =
        setup_master_and_route_less_target("route-less-present-close");
    let (target_id, binding_id, app_scope_id) = identity(&target_context);

    let workers = workers_for_route(&fixture, &fixture.root, &app_scope_id);
    let row = worker_row(&workers, &target_id);
    // A live pane is sufficient for the refusal under test, independent of
    // whether the background agent probe has classified the runner yet.
    assert_eq!(row["identity_valid"], true, "{row}");
    assert_eq!(row["endpoint_live"], true, "{row}");

    let close = close_target(
        &fixture,
        &master,
        &target_id,
        "must not archive a live route-less pane",
    );
    assert_eq!(close["result"]["outcome"], "cleanup-open", "{close}");
    assert_eq!(close["result"]["phase"], "cleanup-open", "{close}");
    assert!(
        close["error"]
            .as_str()
            .unwrap_or_default()
            .contains("TRANSPORT_UNSUPPORTED"),
        "{close}"
    );
    assert_not_retired(
        &fixture,
        &master,
        &fixture.root,
        &app_scope_id,
        &target_id,
        &binding_id,
        &close,
        "refused",
    );
    let operation_id = close["result"]["operation_id"]
        .as_str()
        .expect("close operation id");
    let queried = query_operation(&fixture, &master, operation_id);
    assert_eq!(queried["result"], close["result"], "{queried}");
}

#[test]
fn close_route_less_unknown_target_stays_unknown_without_retirement() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, master) =
        Fixture::isolated_with_pane_command("route-less-unknown-close", "sleep 600");
    fixture.run_ok(&["context"], Some(&master));
    promote_master(&fixture, &fixture.root, &master);

    let unknown_socket = fixture.root.join("unknown.sock");
    create_session_on_socket(&fixture, "route-less-unknown-keeper", &unknown_socket);
    let mut keeper_session = OwnedTmuxSession {
        socket: unknown_socket.clone(),
        session: "route-less-unknown-keeper".to_owned(),
    };
    let target = create_session_on_socket(&fixture, "route-less-unknown-target", &unknown_socket);
    let mut target_session = OwnedTmuxSession {
        socket: unknown_socket.clone(),
        session: "route-less-unknown-target".to_owned(),
    };
    let target_context = register_context_on_socket(
        &fixture,
        &unknown_socket,
        &fixture.root,
        &target,
        "route-less-unknown-context",
    );
    let (target_id, binding_id, app_scope_id) = identity(&target_context);
    let project_root = fixture.root.clone();
    retire_binding_route(&mut fixture, &project_root, &binding_id);

    target_session.kill();
    keeper_session.kill();
    target_session.wait_for_server_exit();
    let workers = workers_for_route(&fixture, &fixture.root, &app_scope_id);
    let row = worker_row(&workers, &target_id);
    assert_eq!(row["identity_valid"], true, "{row}");
    assert_eq!(row["endpoint_live"], Value::Null, "{row}");

    let close = close_target(
        &fixture,
        &master,
        &target_id,
        "must not retire a target with an unknown tmux terminal",
    );
    assert_eq!(close["result"]["outcome"], "unknown", "{close}");
    assert_eq!(close["result"]["phase"], "unknown", "{close}");
    assert!(
        close["error"]
            .as_str()
            .unwrap_or_default()
            .contains("PEER_LIFECYCLE_TMUX_PROBE_UNKNOWN"),
        "{close}"
    );
    assert_not_retired(
        &fixture,
        &master,
        &fixture.root,
        &app_scope_id,
        &target_id,
        &binding_id,
        &close,
        "unknown",
    );
    let operation_id = close["result"]["operation_id"]
        .as_str()
        .expect("close operation id");
    let queried = query_operation(&fixture, &master, operation_id);
    assert_eq!(queried["result"], close["result"], "{queried}");
}
