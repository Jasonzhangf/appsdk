include!("support/context_operation_fixture.rs");

fn fixture_lock() -> std::sync::MutexGuard<'static, ()> {
    TMUX_FIXTURE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn native_codex_fixture(root: &Path) -> PathBuf {
    let source = root.join("codex-fixture.c");
    let executable = root.join("codex");
    std::fs::write(
        &source,
        r#"#define _POSIX_C_SOURCE 200809L
#include <signal.h>
#include <unistd.h>

int main(int argc, char **argv) {
    sigset_t signals;
    int signal_number;

    if (argc != 2 || argv[1][0] == '\0') {
        return 64;
    }
    sigemptyset(&signals);
    sigaddset(&signals, SIGTERM);
    sigaddset(&signals, SIGINT);
    if (sigprocmask(SIG_BLOCK, &signals, NULL) != 0) {
        return 70;
    }
    for (;;) {
        if (sigwait(&signals, &signal_number) == 0) {
            return 0;
        }
    }
}
"#,
    )
    .expect("write public Codex process fixture");
    let output = Command::new("cc")
        .args(["-std=c11", "-O0", "-o"])
        .arg(&executable)
        .arg(&source)
        .output()
        .expect("compile public Codex process fixture");
    assert!(
        output.status.success(),
        "compile public Codex process fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    executable
}

fn quiet_judgment_fixture(label: &str) -> (Fixture, Pane) {
    let (mut fixture, master) = Fixture::isolated(label);
    let config_dir = fixture.root.join("home/.appsdk");
    std::fs::create_dir_all(&config_dir).expect("create fixture config directory");
    std::fs::write(
        config_dir.join("config.toml"),
        "[timers]\ntick_interval_ms=60000\n",
    )
    .expect("write fixture timer config");
    fixture.restart_daemon();
    (fixture, master)
}

fn appserver_command(
    fixture: &Fixture,
    host: &TestAppServer,
    session_id: &str,
    thread_id: &str,
    args: &[&str],
) -> std::process::Output {
    let endpoint = host
        .endpoint()
        .strip_prefix("unix://")
        .expect("fixture AppServer uses a unix endpoint")
        .to_owned();
    fixture
        .configured_command(args, None)
        .env("CODEX_APP_SERVER_SOCKET", endpoint)
        .env("CODEX_SESSION_ID", session_id)
        .env("CODEX_THREAD_ID", thread_id)
        .output()
        .expect("run public AppServer-bound CLI")
}

fn appserver_run_ok(
    fixture: &Fixture,
    host: &TestAppServer,
    session_id: &str,
    thread_id: &str,
    args: &[&str],
) -> Value {
    let output = appserver_command(fixture, host, session_id, thread_id, args);
    assert!(
        output.status.success(),
        "collab {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("AppServer-bound CLI emits JSON")
}

fn promote_master(fixture: &Fixture, pane: &Pane, approval: &str) -> Value {
    fixture.run_ok(&["master", "promote", "--approval", approval], Some(pane))
}

fn new_peer_pane(fixture: &Fixture, master: &Pane, session_name: &str, command: &str) -> Pane {
    let output = tmux(
        &fixture.tmux_socket,
        &[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            session_name,
            command,
        ],
    );
    assert!(
        output.status.success(),
        "create peer pane: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fixture.track_tmux_session(session_name);
    Pane {
        server_pid: master.server_pid,
        pane_id: String::from_utf8(output.stdout)
            .expect("peer pane id is UTF-8")
            .trim()
            .to_owned(),
        session_anchor: format!("session-{session_name}"),
        thread_anchor: format!("thread-{session_name}"),
    }
}

fn register_peer(fixture: &Fixture, label: &str, pane: &Pane) -> (Value, String) {
    let seeded = fixture.registered_context(label, pane);
    assert_eq!(seeded["result"]["outcome"], "completed", "{seeded}");
    let worker_id = seeded["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("registered peer has a worker id")
        .to_owned();
    (seeded, worker_id)
}

fn snapshot_capture(fixture: &Fixture, caller: &Pane, target_id: &str) -> Value {
    fixture.run_ok(
        &["worker", "snapshot", target_id, "--lines", "20"],
        Some(caller),
    )
}

fn judge(
    fixture: &Fixture,
    caller: &Pane,
    capture: &Value,
    presence: Option<&str>,
    activity: Option<&str>,
    basis: &str,
) -> Value {
    let capture = serde_json::to_string(capture).expect("serialize judgment capture");
    let mut args = vec!["worker", "judge", "--capture", capture.as_str()];
    if let Some(presence) = presence {
        args.extend(["--presence", presence]);
    }
    if let Some(activity) = activity {
        args.extend(["--activity", activity]);
    }
    args.extend(["--basis", basis]);
    fixture.run_ok(&args, Some(caller))
}

fn peer_status(fixture: &Fixture, caller: &Pane, target_id: &str) -> Value {
    let status = fixture.run_ok(&["worker", "status", target_id], Some(caller));
    status["workers"]
        .as_array()
        .and_then(|workers| workers.iter().find(|worker| worker["id"] == target_id))
        .cloned()
        .unwrap_or_else(|| panic!("worker status does not include {target_id}: {status}"))
}

#[test]
fn tmux_exact_exit_survives_daemon_process_restart() {
    let _fixture_lock = fixture_lock();
    let (mut fixture, caller) = Fixture::isolated("tmux-exact-restart");
    register_peer(&fixture, "tmux-exact-caller", &caller);
    let peer = new_peer_pane(&fixture, &caller, "tmux-exact-peer", "sleep 600");
    let (_, peer_id) = register_peer(&fixture, "tmux-exact-register", &peer);
    let codex = native_codex_fixture(&fixture.root);
    let pid_file = fixture.root.join("exact.pid");
    let script = format!(
        "{} 600 &\necho $! > {}\nwait\n",
        shell_quote(&codex.to_string_lossy()),
        shell_quote(&pid_file.to_string_lossy())
    );
    fixture.run_script_in_pane_on_socket(&fixture.tmux_socket, &peer, "exact-process", &script);
    assert!(wait_until(Duration::from_secs(15), || pid_file.exists()));
    assert!(
        wait_until(Duration::from_secs(20), || {
            peer_status(&fixture, &caller, &peer_id)["transport_view"]["agent_presence"] == "online"
        }),
        "current exact execution must first be publicly observed"
    );
    assert!(
        fixture.current_state()["tmux_executions"][&peer_id].is_object(),
        "online and execution reference must commit together"
    );
    let old_daemon = fixture.daemon_pid();
    fixture.stop_daemon();
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    assert!(wait_until(Duration::from_secs(10), || unsafe {
        libc::kill(pid, 0)
    } != 0));
    fixture.restart_daemon();
    assert_ne!(
        old_daemon,
        fixture.daemon_pid(),
        "restart must replace the OS daemon process"
    );
    let mut last = Value::Null;
    assert!(
        wait_until(Duration::from_secs(20), || {
            last = peer_status(&fixture, &caller, &peer_id);
            last["transport_view"]["agent_presence"] == "offline"
        }),
        "persisted exact execution must prove exit after restart: {last}"
    );
    assert_eq!(
        last["transport_view"]["presence_observation"]["reason"],
        "observed_codex_process_exited"
    );
}

#[test]
fn tmux_restart_preserves_suspended_execution_and_prefers_same_pane_new_execution() {
    let _fixture_lock = fixture_lock();
    let (mut fixture, caller) = Fixture::isolated("tmux-live-restart");
    register_peer(&fixture, "tmux-live-caller", &caller);
    let peer = new_peer_pane(&fixture, &caller, "tmux-live-peer", "sleep 600");
    let (_, peer_id) = register_peer(&fixture, "tmux-live-register", &peer);
    let codex = native_codex_fixture(&fixture.root);
    let pid_file = fixture.root.join("live.pid");
    let script = format!(
        "{} 600 &\necho $! > {}\nwait\n",
        shell_quote(&codex.to_string_lossy()),
        shell_quote(&pid_file.to_string_lossy())
    );
    fixture.run_script_in_pane_on_socket(&fixture.tmux_socket, &peer, "live-process", &script);
    assert!(wait_until(Duration::from_secs(15), || pid_file.exists()));
    assert!(wait_until(Duration::from_secs(20), || peer_status(
        &fixture, &caller, &peer_id
    )["transport_view"]["agent_presence"]
        == "online"));
    let old_reference = fixture.current_state()["tmux_executions"][&peer_id].clone();
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(old_reference["execution"]["pid"], pid);
    fixture.stop_daemon();
    assert_eq!(unsafe { libc::kill(pid, libc::SIGSTOP) }, 0);
    fixture.restart_daemon();
    assert!(wait_until(Duration::from_secs(20), || peer_status(
        &fixture, &caller, &peer_id
    )["transport_view"]["agent_presence"]
        == "online"));
    assert_eq!(
        fixture.current_state()["tmux_executions"][&peer_id],
        old_reference
    );
    assert!(fixture.current_state()["name_reassignments"]
        .as_object()
        .unwrap()
        .values()
        .all(|value| value.as_object().unwrap().is_empty()));
    fixture.stop_daemon();
    assert_eq!(unsafe { libc::kill(pid, libc::SIGCONT) }, 0);
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    assert!(wait_until(Duration::from_secs(10), || unsafe {
        libc::kill(pid, 0)
    } != 0));
    std::fs::remove_file(&pid_file).unwrap();
    fixture.run_script_in_pane_on_socket(&fixture.tmux_socket, &peer, "new-live-process", &script);
    assert!(wait_until(Duration::from_secs(15), || pid_file.exists()));
    let new_pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_ne!(pid, new_pid);
    fixture.restart_daemon();
    assert!(wait_until(Duration::from_secs(20), || peer_status(
        &fixture, &caller, &peer_id
    )["transport_view"]["agent_presence"]
        == "online"));
    assert_eq!(
        fixture.current_state()["tmux_executions"][&peer_id]["execution"]["pid"],
        new_pid
    );
    assert!(fixture.current_state()["name_reassignments"]
        .as_object()
        .unwrap()
        .values()
        .all(|value| value.as_object().unwrap().is_empty()));
    assert_eq!(unsafe { libc::kill(new_pid, libc::SIGTERM) }, 0);
    assert!(wait_until(Duration::from_secs(10), || unsafe {
        libc::kill(new_pid, 0)
    } != 0));
}

#[test]
fn master_can_classify_unknown_presence_offline_and_finish_status_judgment() {
    let _fixture_lock = fixture_lock();
    let (fixture, master) = quiet_judgment_fixture("worker-judgment-offline");
    register_peer(&fixture, "worker-judgment-offline-master", &master);
    promote_master(
        &fixture,
        &master,
        "isolated snapshot Offline classification",
    );
    let peer = new_peer_pane(
        &fixture,
        &master,
        "worker-judgment-offline-peer",
        "sleep 600",
    );
    let (_, peer_id) = register_peer(&fixture, "worker-judgment-offline-register", &peer);
    let captured = snapshot_capture(&fixture, &master, &peer_id);
    assert_eq!(captured["snapshot_collected"], true, "{captured}");
    assert_eq!(
        captured["judgment_capture"]["missing_fields"],
        json!(["presence", "activity"]),
        "{captured}"
    );
    let classified = judge(
        &fixture,
        &master,
        &captured["judgment_capture"],
        Some("offline"),
        None,
        "current snapshot shows no running agent session",
    );
    assert_eq!(classified["outcome"], "applied", "{classified}");
    assert_eq!(
        classified["applied_fields"],
        json!(["presence"]),
        "{classified}"
    );
    assert_eq!(
        classified["presence"]["presence"], "offline",
        "{classified}"
    );
    let satisfied = snapshot_capture(&fixture, &master, &peer_id);
    assert_eq!(satisfied["snapshot_collected"], false, "{satisfied}");
    assert_eq!(satisfied["judgment_status"], "satisfied", "{satisfied}");
    let status = peer_status(&fixture, &master, &peer_id);
    assert_eq!(status["presence"], "missing", "{status}");
    assert_eq!(
        status["transport_view"]["agent_presence"], "offline",
        "{status}"
    );
}

#[test]
fn master_judgment_applies_only_missing_fields_then_snapshot_stops() {
    let _fixture_lock = fixture_lock();
    let (fixture, master) = quiet_judgment_fixture("worker-judgment-apply");
    register_peer(&fixture, "worker-judgment-master", &master);
    promote_master(&fixture, &master, "isolated snapshot judgment acceptance");

    let peer = new_peer_pane(&fixture, &master, "worker-judgment-peer", "sleep 600");
    let (_, peer_id) = register_peer(&fixture, "worker-judgment-peer-register", &peer);

    let first = snapshot_capture(&fixture, &master, &peer_id);
    assert_eq!(first["snapshot_collected"], true, "{first}");
    assert_eq!(first["judgment_status"], "ready", "{first}");
    assert_eq!(first["session_probe"], "unsupported", "{first}");
    let first_capture = &first["judgment_capture"];
    assert_eq!(first_capture["worker_id"], peer_id, "{first}");
    assert_eq!(
        first_capture["missing_fields"],
        json!(["presence", "activity"]),
        "{first}"
    );

    let partial = judge(
        &fixture,
        &master,
        first_capture,
        Some("online"),
        Some("unknown"),
        "visible Codex prompt",
    );
    assert_eq!(partial["outcome"], "partial", "{partial}");
    assert_eq!(partial["applied_fields"], json!(["presence"]), "{partial}");
    assert_eq!(partial["presence"]["presence"], "online", "{partial}");
    assert!(
        partial.get("activity").is_none()
            || partial["activity"].is_null()
            || partial["activity"]["activity"] == "unknown",
        "{partial}"
    );

    let second = snapshot_capture(&fixture, &master, &peer_id);
    assert_eq!(second["snapshot_collected"], true, "{second}");
    assert_eq!(second["judgment_status"], "ready", "{second}");
    assert_eq!(
        second["judgment_capture"]["missing_fields"],
        json!(["activity"]),
        "{second}"
    );
    let second_capture = &second["judgment_capture"];

    let determined_presence = judge(
        &fixture,
        &master,
        second_capture,
        Some("offline"),
        None,
        "attempt to replace an already determined presence",
    );
    assert_eq!(
        determined_presence["outcome"], "rejected",
        "{determined_presence}"
    );
    assert_eq!(
        determined_presence["presence"]["presence"], "online",
        "{determined_presence}"
    );

    let applied = judge(
        &fixture,
        &master,
        second_capture,
        None,
        Some("busy"),
        "active Codex output",
    );
    assert_eq!(applied["outcome"], "applied", "{applied}");
    assert_eq!(applied["applied_fields"], json!(["activity"]), "{applied}");
    assert_eq!(applied["activity"]["activity"], "busy", "{applied}");
    assert!(applied["activity"]["idle_since_ms"].is_null(), "{applied}");

    let status = peer_status(&fixture, &master, &peer_id);
    assert_eq!(status["presence"], "present", "{status}");
    assert_eq!(status["agent_state"], "working", "{status}");
    assert_eq!(
        status["transport_view"]["activity_observation"]["activity"], "busy",
        "{status}"
    );

    let replay = judge(
        &fixture,
        &master,
        second_capture,
        None,
        Some("idle"),
        "late replay",
    );
    assert_eq!(replay["outcome"], "stale_facts", "{replay}");
    assert!(
        replay["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("native presence or activity changed"),
        "{replay}"
    );

    let satisfied = snapshot_capture(&fixture, &master, &peer_id);
    assert_eq!(satisfied["snapshot_collected"], false, "{satisfied}");
    assert_eq!(satisfied["judgment_status"], "satisfied", "{satisfied}");
    assert_eq!(
        satisfied["reason"], "current status facts are already sufficient",
        "{satisfied}"
    );
}

#[test]
fn judgment_requires_current_master_and_rejects_peer_and_former_master() {
    let _fixture_lock = fixture_lock();
    let (fixture, first_master) = quiet_judgment_fixture("worker-judgment-authority");
    register_peer(&fixture, "worker-judgment-first-master", &first_master);
    let successor = new_peer_pane(
        &fixture,
        &first_master,
        "worker-judgment-successor",
        "sleep 600",
    );
    register_peer(&fixture, "worker-judgment-successor-register", &successor);
    let target = new_peer_pane(
        &fixture,
        &first_master,
        "worker-judgment-target",
        "sleep 600",
    );
    let (_, target_id) = register_peer(&fixture, "worker-judgment-target-register", &target);

    let capture = snapshot_capture(&fixture, &first_master, &target_id);
    assert_eq!(capture["snapshot_collected"], true, "{capture}");
    let capture = &capture["judgment_capture"];

    let awaiting = judge(
        &fixture,
        &first_master,
        capture,
        Some("online"),
        None,
        "no current master",
    );
    assert_eq!(awaiting["outcome"], "awaiting_master", "{awaiting}");
    assert!(awaiting["applied_fields"].is_null(), "{awaiting}");

    promote_master(
        &fixture,
        &first_master,
        "isolated current master for judgment rejection",
    );
    let peer_rejected = judge(
        &fixture,
        &successor,
        capture,
        Some("online"),
        None,
        "ordinary peer attempt",
    );
    assert_eq!(peer_rejected["outcome"], "unauthorized", "{peer_rejected}");
    assert!(peer_rejected["applied_fields"].is_null(), "{peer_rejected}");

    promote_master(
        &fixture,
        &successor,
        "isolated successor master for old-master rejection",
    );
    let capture_json = serde_json::to_string(capture).expect("serialize judgment capture");
    let former_master_rejected = fixture.command(
        &[
            "worker",
            "judge",
            "--capture",
            capture_json.as_str(),
            "--presence",
            "online",
            "--basis",
            "former master late attempt",
        ],
        Some(&first_master),
    );
    assert!(
        !former_master_rejected.status.success(),
        "{former_master_rejected:?}"
    );
    let former_master_error = String::from_utf8_lossy(&former_master_rejected.stderr);
    assert!(
        former_master_error.contains("BOARD_IDENTITY_REQUIRED"),
        "{former_master_error}"
    );
    let state = fixture.current_state();
    let target_presence = state["agent_presence"].as_object().and_then(|projects| {
        projects
            .values()
            .filter_map(Value::as_object)
            .find_map(|workers| workers.get(&target_id))
    });
    assert!(
        target_presence.is_none(),
        "former master rejection must leave target presence unchanged: {state}"
    );
}

#[test]
fn judgment_rejects_capture_after_target_session_rebind() {
    let _fixture_lock = fixture_lock();
    let (fixture, master) = quiet_judgment_fixture("worker-judgment-stale-target");
    register_peer(&fixture, "worker-judgment-stale-master", &master);
    promote_master(
        &fixture,
        &master,
        "isolated stale-target judgment acceptance",
    );

    let target = new_peer_pane(&fixture, &master, "worker-judgment-stale-peer", "sleep 600");
    let (_, target_id) = register_peer(&fixture, "worker-judgment-stale-target-register", &target);
    let captured = snapshot_capture(&fixture, &master, &target_id);
    assert_eq!(captured["snapshot_collected"], true, "{captured}");
    let capture = &captured["judgment_capture"];

    let rebound = Pane {
        server_pid: target.server_pid,
        pane_id: target.pane_id.clone(),
        session_anchor: "worker-judgment-rebound-session".into(),
        thread_anchor: "worker-judgment-rebound-thread".into(),
    };
    register_peer(&fixture, "worker-judgment-stale-target-rebind", &rebound);

    let stale = judge(
        &fixture,
        &master,
        capture,
        Some("online"),
        None,
        "late judgment for the old target binding",
    );
    assert_eq!(stale["outcome"], "stale_target", "{stale}");
    assert!(stale["applied_fields"].is_null(), "{stale}");
}

#[test]
fn appserver_session_status_resolves_missing_fields_without_snapshot() {
    let _fixture_lock = fixture_lock();
    let fixture = Fixture::without_tmux("worker-session-status");
    let host = TestAppServer::start_with_effect(
        &fixture.root,
        "worker-session-status-session",
        "worker-session-status-thread",
        "ack_only",
    );
    let seeded = fixture.context_provide(
        "worker-session-status-seed",
        &json!({
            "session_id": "worker-session-status-session",
            "thread_id": "worker-session-status-thread",
            "endpoint": host.endpoint(),
            "namespace": "codex_tui"
        }),
        None,
    );
    assert_eq!(seeded["result"]["outcome"], "completed", "{seeded}");
    let target_id = seeded["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("registered AppServer peer has a worker id")
        .to_owned();

    let promoted = appserver_run_ok(
        &fixture,
        &host,
        "worker-session-status-session",
        "worker-session-status-thread",
        &[
            "master",
            "promote",
            "--approval",
            "isolated session-status capability acceptance",
        ],
    );
    assert_eq!(promoted["role_brief"]["role"], "master", "{promoted}");

    let first = appserver_run_ok(
        &fixture,
        &host,
        "worker-session-status-session",
        "worker-session-status-thread",
        &["worker", "snapshot", &target_id, "--lines", "20"],
    );
    assert_eq!(first["snapshot_collected"], false, "{first}");
    assert_eq!(first["session_probe"], "applied", "{first}");
    assert_eq!(first["judgment_status"], "satisfied", "{first}");
    assert_eq!(
        first["judgment_capture"]["missing_fields"],
        json!([]),
        "{first}"
    );
    assert_eq!(
        first["judgment_capture"]["presence"]["presence"], "online",
        "{first}"
    );
    assert_eq!(
        first["judgment_capture"]["activity"]["activity"], "idle",
        "{first}"
    );

    let second = appserver_run_ok(
        &fixture,
        &host,
        "worker-session-status-session",
        "worker-session-status-thread",
        &["worker", "snapshot", &target_id, "--lines", "20"],
    );
    assert_eq!(second["snapshot_collected"], false, "{second}");
    assert_eq!(second["session_probe"], "not_required", "{second}");
    assert_eq!(second["judgment_status"], "satisfied", "{second}");

    let status = appserver_run_ok(
        &fixture,
        &host,
        "worker-session-status-session",
        "worker-session-status-thread",
        &["worker", "status", &target_id],
    );
    assert_eq!(status["workers"][0]["presence"], "present", "{status}");
    assert_eq!(status["workers"][0]["agent_state"], "idle", "{status}");
}
