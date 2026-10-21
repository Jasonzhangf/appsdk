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

// The background scheduler independently appends ledger-maintenance events on
// its own tick. Exclude only those known timer-owned records so the snapshot
// assertion still fails on any command, registration, or snapshot receipt.
fn journal_excluding_background_maintenance(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| {
            let record: Value = serde_json::from_str(line).expect("typed project event");
            let background_adjudication_acceptance = record["ev"] == "NotificationDeliveryAccepted"
                && record["message_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("name-adjudication-presence-"));
            !line.contains("\"GlobalRuntimeBindingLedgerClassified\"")
                && !line.contains("\"GlobalLedgerScanReceiptRecorded\"")
                && !background_adjudication_acceptance
        })
        .map(str::to_owned)
        .collect()
}

fn remove_host_binding_route(path: &std::path::Path, binding_id: &str) {
    let mut removed = 0;
    let lines = std::fs::read_to_string(path)
        .expect("read host journal")
        .lines()
        .filter(|line| {
            let value: Value = serde_json::from_str(line).expect("parse host journal record");
            let matches = value["ev"] == "GlobalCurrentThreadRouteSet"
                && value["binding"]["binding_id"] == binding_id;
            removed += usize::from(matches);
            !matches
        })
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    assert_eq!(
        removed, 1,
        "fixture must remove exactly one host publication for {binding_id}"
    );
    std::fs::write(path, lines).expect("write host journal without target route");
}

fn registered_context_at(
    fixture: &Fixture,
    project_root: &std::path::Path,
    label: &str,
    pane: &Pane,
) -> Value {
    let done = fixture.root.join(format!("{label}.participant-done"));
    let result_path = fixture.root.join(format!("{label}.participant-result"));
    let script = format!(
        "cd {}\n\"$COLLAB_BINARY\" context --op {} >{} 2>\"$PANE_STDERR\"\nprintf done >{}\nsleep 600\n",
        shell_quote(&project_root.to_string_lossy()),
        shell_quote(label),
        shell_quote(&result_path.to_string_lossy()),
        shell_quote(&done.to_string_lossy())
    );
    let consumer = fixture.run_in_pane(pane, label, &script);
    assert!(
        wait_until(Duration::from_secs(15), || done.exists()),
        "external project participant did not complete: {} {}",
        consumer.output(),
        std::fs::read_to_string(&result_path).unwrap_or_default()
    );
    serde_json::from_slice(&std::fs::read(result_path).unwrap()).unwrap()
}

fn command_at(
    fixture: &Fixture,
    project_root: &std::path::Path,
    args: &[&str],
    pane: &Pane,
) -> Output {
    fixture
        .configured_command(args, Some(pane))
        .current_dir(project_root)
        .output()
        .expect("run public collab CLI in external project")
}

#[test]
fn peer_lifecycle_create_cli_exposes_frozen_arguments() {
    let create = worker_help("create");
    for argument in ["TARGET_ID", "--cwd", "--model", "--op"] {
        assert!(create.contains(argument), "{create}");
    }
    let read = worker_help("read");
    assert!(read.contains("TARGET_ID"), "{read}");

    let snapshot = worker_help("snapshot");
    assert!(snapshot.contains("TARGET_ID"), "{snapshot}");
    assert!(snapshot.contains("--lines"), "{snapshot}");
    assert!(snapshot.contains("1..=200"), "{snapshot}");

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
fn worker_snapshot_public_cli_returns_bounded_tmux_buffer_without_a_write() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, master) = Fixture::isolated("worker-snapshot-public");
    let seeded = fixture.registered_context("worker-snapshot-master", &master);
    assert_eq!(seeded["result"]["outcome"], "completed", "{seeded}");
    let promoted = fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "isolated worker snapshot acceptance",
        ],
        Some(&master),
    );
    assert_eq!(promoted["role_brief"]["role"], "master", "{promoted}");

    let split = tmux(
        &fixture.tmux_socket,
        &[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            "snapshot-peer",
            "sleep 600",
        ],
    );
    assert!(
        split.status.success(),
        "split fixture pane: {}",
        String::from_utf8_lossy(&split.stderr)
    );
    let peer_pane_id = String::from_utf8(split.stdout)
        .expect("peer pane id is utf8")
        .trim()
        .to_owned();
    fixture
        .owned_sessions
        .lock()
        .unwrap()
        .push("snapshot-peer".into());
    let peer = Pane {
        server_pid: master.server_pid,
        pane_id: peer_pane_id.clone(),
        session_anchor: format!("session-{peer_pane_id}"),
        thread_anchor: format!("thread-{peer_pane_id}"),
    };
    let peer_seeded = fixture.registered_context("worker-snapshot-peer", &peer);
    assert_eq!(
        peer_seeded["result"]["outcome"], "completed",
        "{peer_seeded}"
    );
    let peer_id = peer_seeded["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("snapshot peer has a worker id")
        .to_owned();
    settle_snapshot_background_notifications(
        &fixture,
        &master,
        seeded["result"]["snapshot"]["identity"]["worker_id"]
            .as_str()
            .expect("snapshot master has a worker id"),
        &peer_id,
    );

    let prepared = tmux(
        &fixture.tmux_socket,
        &[
            "respawn-pane",
            "-k",
            "-t",
            &peer.pane_id,
            "i=1; while [ $i -le 200 ]; do printf 'PUB-%03d\\n' \"$i\"; i=$((i+1)); done; sleep 600",
        ],
    );
    assert!(
        prepared.status.success(),
        "prepare peer buffer: {}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let capture = tmux(
            &fixture.tmux_socket,
            &["capture-pane", "-p", "-t", &peer.pane_id, "-S", "-4096"],
        );
        if String::from_utf8_lossy(&capture.stdout).contains("PUB-200") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "public snapshot fixture did not publish its terminal line"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let journal_before = journal_excluding_background_maintenance(&fixture.project_journal());

    let snapshot = fixture.run_ok(
        &["worker", "snapshot", &peer_id, "--lines", "40"],
        Some(&master),
    );
    assert_eq!(snapshot["transport"], "tmux", "{snapshot}");
    assert_eq!(snapshot["worker_id"], peer_id, "{snapshot}");
    assert_eq!(snapshot["pane_id"], peer.pane_id, "{snapshot}");
    assert_eq!(snapshot["lines"], 40, "{snapshot}");
    assert_eq!(snapshot["durable"], false, "{snapshot}");
    let lines = snapshot["output"]
        .as_str()
        .expect("snapshot output is text")
        .lines()
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 40);
    assert_eq!(lines.first().copied(), Some("PUB-161"));
    assert_eq!(lines.last().copied(), Some("PUB-200"));
    assert_eq!(
        journal_excluding_background_maintenance(&fixture.project_journal()),
        journal_before,
        "snapshot must not write a durable receipt or journal event"
    );
}

#[test]
fn top_level_snapshot_reads_a_peer_buffer_for_a_non_master_caller() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, master) = Fixture::isolated("snapshot-top-level");
    let help = Command::new(env!("CARGO_BIN_EXE_collab"))
        .args(["snapshot", "--help"])
        .output()
        .expect("run top-level snapshot help");
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("snapshot help is UTF-8");
    assert!(help.contains("TARGET_ID"), "{help}");
    assert!(help.contains("--lines"), "{help}");

    let seeded = fixture.registered_context("snapshot-top-level-master", &master);
    assert_eq!(seeded["result"]["outcome"], "completed", "{seeded}");
    let promoted = fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "isolated top-level snapshot acceptance",
        ],
        Some(&master),
    );
    let master_id = promoted["role_brief"]["worker_id"]
        .as_str()
        .or_else(|| seeded["result"]["snapshot"]["identity"]["worker_id"].as_str())
        .expect("master has a worker id")
        .to_owned();

    let split = tmux(
        &fixture.tmux_socket,
        &[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            "snapshot-peer",
            "sleep 600",
        ],
    );
    assert!(
        split.status.success(),
        "split fixture pane: {}",
        String::from_utf8_lossy(&split.stderr)
    );
    let peer_pane_id = String::from_utf8(split.stdout)
        .expect("peer pane id is utf8")
        .trim()
        .to_owned();
    fixture
        .owned_sessions
        .lock()
        .unwrap()
        .push("snapshot-peer".into());
    let peer = Pane {
        server_pid: master.server_pid,
        pane_id: peer_pane_id.clone(),
        session_anchor: format!("session-{peer_pane_id}"),
        thread_anchor: format!("thread-{peer_pane_id}"),
    };
    let peer_seeded = fixture.registered_context("snapshot-top-level-peer", &peer);
    assert_eq!(
        peer_seeded["result"]["snapshot"]["identity"]["role"], "worker",
        "snapshot caller must remain an ordinary peer: {peer_seeded}"
    );
    settle_snapshot_background_notifications(
        &fixture,
        &master,
        &master_id,
        peer_seeded["result"]["snapshot"]["identity"]["worker_id"]
            .as_str()
            .expect("snapshot caller has a worker id"),
    );
    let prepared = tmux(
        &fixture.tmux_socket,
        &[
            "respawn-pane",
            "-k",
            "-t",
            &master.pane_id,
            "printf 'MASTER-BUFFER\\n'; sleep 600",
        ],
    );
    assert!(
        prepared.status.success(),
        "prepare master buffer: {}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let capture = tmux(
            &fixture.tmux_socket,
            &["capture-pane", "-p", "-t", &master.pane_id, "-S", "-4096"],
        );
        if String::from_utf8_lossy(&capture.stdout).contains("MASTER-BUFFER") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "master pane fixture did not publish its buffer"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let capture = || {
        String::from_utf8(
            tmux(
                &fixture.tmux_socket,
                &["capture-pane", "-p", "-t", &master.pane_id, "-S", "-4096"],
            )
            .stdout,
        )
        .expect("master capture is utf8")
    };
    let buffer_before = capture();
    let queued = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &master_id,
            "--subject",
            "snapshot must not consume mail",
            "--delivery",
            "queued",
            "durable body for the target",
        ],
        Some(&peer),
    );
    let message_id = queued["msg_id"]
        .as_str()
        .expect("queued message has an id")
        .to_owned();
    let pending = fixture.run_ok(&["msg", &message_id], Some(&peer));
    assert_eq!(pending["consumed_by_recv"], false, "{pending}");
    eprintln!("snapshot target message before read: {pending}");
    let journal_before = journal_excluding_background_maintenance(&fixture.project_journal());

    let snapshot = fixture.run_ok(&["snapshot", &master_id, "--lines", "20"], Some(&peer));
    assert_eq!(snapshot["transport"], "tmux", "{snapshot}");
    assert_eq!(snapshot["worker_id"], master_id, "{snapshot}");
    assert_eq!(snapshot["pane_id"], master.pane_id, "{snapshot}");
    assert_eq!(snapshot["durable"], false, "{snapshot}");
    assert!(
        snapshot["output"]
            .as_str()
            .unwrap_or_default()
            .contains("MASTER-BUFFER"),
        "{snapshot}"
    );
    assert_eq!(
        capture(),
        buffer_before,
        "snapshot must not submit input or change the target pane"
    );
    assert_eq!(
        journal_excluding_background_maintenance(&fixture.project_journal()),
        journal_before,
        "snapshot must not write a journal record or durable receipt"
    );
    let receipt = fixture.run_ok(&["msg", &message_id], Some(&master));
    assert_eq!(
        receipt["consumed_by_recv"], false,
        "snapshot must not consume the target mailbox: {receipt}"
    );
    eprintln!("snapshot target message after read: {receipt}");
}

fn settle_snapshot_background_notifications(
    fixture: &Fixture,
    master: &Pane,
    master_id: &str,
    peer_id: &str,
) {
    let records = || {
        std::fs::read_to_string(fixture.project_journal())
            .expect("read snapshot fixture journal")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("typed fixture event"))
            .collect::<Vec<_>>()
    };
    let initial = records();
    let occupancies = [master_id, peer_id].map(|worker_id| {
        initial
            .iter()
            .rev()
            .find(|event| {
                event["ev"] == "NameResolutionCommitted"
                    && event["occupancy"]["worker_id"] == worker_id
            })
            .expect("registered worker has a current typed occupancy")
            .clone()
    });
    let mut deliveries = Vec::new();
    assert!(
        wait_until(Duration::from_secs(15), || {
            let current = records();
            deliveries = occupancies
                .iter()
                .filter_map(|event| {
                    let occupancy = &event["occupancy"];
                    let reference = json!({
                        "project_scope": event["project_scope"],
                        "name": occupancy["name"],
                        "worker_id": occupancy["worker_id"],
                        "session_id": occupancy["session_id"],
                        "occupancy_revision": occupancy["occupancy_revision"]
                    });
                    let recovery = current.iter().rev().find(|record| {
                        record["ev"] == "NameRecoveryUpdated"
                            && record["recovery"]["occupancy_ref"] == reference
                            && record["recovery"]["request"]["invocation"]
                                == "presence_adjudication"
                    })?;
                    let op = recovery["recovery"]["operation_id"].as_str().unwrap();
                    let id = format!("name-adjudication-{op}");
                    let terminal = current.iter().rev().find(|record| {
                        record["message_id"] == id
                            && (record["ev"] == "NotificationDeliveryAccepted"
                                || record["ev"] == "NotificationDeliveryFailed")
                    })?;
                    Some((reference, op.to_owned(), id, terminal.clone()))
                })
                .collect();
            deliveries.len() == occupancies.len()
        }),
        "both current occupancies need exact adjudication delivery terminals: {occupancies:?}; {deliveries:?}"
    );
    // The Python runners are controlled simulated agents. Their inconclusive
    // probes legitimately request adjudication. Finish those independent wakes
    // before stamping the pure-read baseline; do not disable the monitor.
    for delivery in &deliveries {
        assert_eq!(
            delivery.3["ev"], "NotificationDeliveryAccepted",
            "{delivery:?}"
        );
        let receipt = fixture.run_ok(&["msg", &delivery.2], Some(master));
        assert_eq!(receipt["consumed_by_recv"], false, "{receipt}");
        eprintln!("snapshot background delivered: {delivery:?}; receipt={receipt}");
    }
    let received = fixture.run_ok(&["recv", "--timeout", "1"], Some(master));
    for (_, _, id, _) in deliveries {
        assert!(
            received["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|msg| msg["id"] == id),
            "background message {id} must actually be received: {received}"
        );
        let receipt = fixture.run_ok(&["msg", &id], Some(master));
        assert_eq!(receipt["consumed_by_recv"], true, "{receipt}");
        eprintln!("snapshot background consumed: {receipt}");
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
fn peer_lifecycle_self_unregister_public_cli_rebuilds_and_keeps_pane() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("peer-self-unregister");
    let seeded = fixture.registered_context("peer-self-unregister-seed", &pane);
    assert_eq!(seeded["result"]["outcome"], "completed", "{seeded}");
    let worker_id = seeded["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("registered self has a worker id")
        .to_owned();
    let binding_before = seeded["result"]["snapshot"]["binding"].clone();

    let promoted = fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "isolated self-unregister acceptance",
        ],
        Some(&pane),
    );
    assert_eq!(promoted["role_brief"]["role"], "master", "{promoted}");

    let pane_pid_before = tmux(
        &fixture.tmux_socket,
        &["display-message", "-p", "-t", &pane.pane_id, "#{pane_pid}"],
    )
    .stdout;
    let closed = fixture.command(
        &[
            "worker",
            "close",
            &worker_id,
            "--reason",
            "self-unregister while keeping this pane alive",
        ],
        Some(&pane),
    );
    assert!(
        closed.status.success(),
        "self-unregister failed: stdout={} stderr={}",
        String::from_utf8_lossy(&closed.stdout),
        String::from_utf8_lossy(&closed.stderr)
    );
    let closed: Value = serde_json::from_slice(&closed.stdout).unwrap();
    assert_eq!(closed["result"]["outcome"], "complete", "{closed}");
    assert_eq!(
        closed["result"]["close"]["runtime_archive"]["state"], "not-applicable",
        "{closed}"
    );
    assert_eq!(
        closed["result"]["close"]["binding_retirement"]["state"], "verified",
        "{closed}"
    );
    assert_eq!(
        closed["result"]["close"]["route_retirement"]["state"], "verified",
        "{closed}"
    );

    let pane_pid_after = tmux(
        &fixture.tmux_socket,
        &["display-message", "-p", "-t", &pane.pane_id, "#{pane_pid}"],
    )
    .stdout;
    assert_eq!(
        pane_pid_after, pane_pid_before,
        "self-unregister must not archive or replace the real pane"
    );

    let rebuilt = fixture.context(Some("peer-self-unregister-rebuild"), Some(&pane));
    assert_eq!(rebuilt["result"]["outcome"], "completed", "{rebuilt}");
    assert_eq!(
        rebuilt["result"]["snapshot"]["identity"]["worker_id"], worker_id,
        "{rebuilt}"
    );
    assert_eq!(
        rebuilt["result"]["snapshot"]["identity"]["role"], "worker",
        "a rebuilt self-unregistered identity must not inherit master authority: {rebuilt}"
    );
    let binding_after = rebuilt["result"]["snapshot"]["binding"].clone();
    assert_ne!(
        binding_after["endpoint_generation"], binding_before["endpoint_generation"],
        "rebuild must publish a new runtime generation: {rebuilt}"
    );
    assert_ne!(
        rebuilt["result"]["snapshot"]["master"]["worker_id"], worker_id,
        "the retired master grant must not survive rebuild: {rebuilt}"
    );

    let sent = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &worker_id,
            "--subject",
            "self-unregister rebuild receipt",
            "--delivery",
            "queued",
            "communication after rebuild",
        ],
        Some(&pane),
    );
    let msg_id = sent["msg_id"]
        .as_str()
        .expect("self-unregister rebuild send has a message id");
    let received = fixture.run_ok(&["recv", "--timeout", "1"], Some(&pane));
    assert!(
        received["messages"].as_array().unwrap().iter()
            .any(|message| message["id"] == msg_id),
        "the exact rebuild message must be consumed alongside any existing patrol notices: {received}"
    );
    let receipt = fixture.run_ok(&["msg", msg_id], Some(&pane));
    assert_eq!(receipt["consumed_by_recv"], true, "{receipt}");
}

#[test]
fn stale_peer_recovers_missing_host_route_and_self_unregisters() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("peer-stale-route-self-close");
    let seeded = fixture.registered_context("peer-stale-route-host-seed", &pane);
    assert_eq!(seeded["result"]["outcome"], "completed", "{seeded}");

    let external_project = fixture.root.join("external-project");
    std::fs::create_dir_all(external_project.join(".agent-collab")).unwrap();
    let split = tmux(
        &fixture.tmux_socket,
        &[
            "split-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            &pane.pane_id,
            "sleep 600",
        ],
    );
    assert!(
        split.status.success(),
        "split external project pane: {}",
        String::from_utf8_lossy(&split.stderr)
    );
    let external_pane = Pane {
        server_pid: pane.server_pid,
        pane_id: String::from_utf8(split.stdout)
            .expect("external pane id is utf8")
            .trim()
            .to_owned(),
        session_anchor: "external-session".into(),
        thread_anchor: "external-thread".into(),
    };
    let external = registered_context_at(
        &fixture,
        &external_project,
        "peer-stale-route-external-seed",
        &external_pane,
    );
    assert_eq!(external["result"]["outcome"], "completed", "{external}");
    let worker_id = external["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("registered peer has a worker id")
        .to_owned();
    let binding_id = external["result"]["snapshot"]["binding"]["binding_id"]
        .as_str()
        .expect("registered peer has a binding id")
        .to_owned();
    let pane_pid_before = tmux(
        &fixture.tmux_socket,
        &[
            "display-message",
            "-p",
            "-t",
            &external_pane.pane_id,
            "#{pane_pid}",
        ],
    )
    .stdout;

    fixture.stop_daemon();
    remove_host_binding_route(&fixture.project_journal(), &binding_id);
    fixture.restart_after_external_stop();

    let recovered = command_at(
        &fixture,
        &external_project,
        &["context", "--op", "peer-stale-route-external-recover"],
        &external_pane,
    );
    assert!(
        recovered.status.success(),
        "stale peer context recovery failed: stdout={} stderr={}",
        String::from_utf8_lossy(&recovered.stdout),
        String::from_utf8_lossy(&recovered.stderr)
    );
    let recovered: Value = serde_json::from_slice(&recovered.stdout).unwrap();
    assert_eq!(recovered["result"]["outcome"], "completed", "{recovered}");
    assert_eq!(
        recovered["result"]["snapshot"]["identity"]["worker_id"], worker_id,
        "{recovered}"
    );

    let closed = command_at(
        &fixture,
        &external_project,
        &[
            "worker",
            "close",
            &worker_id,
            "--reason",
            "self-unregister after missing host route recovery",
        ],
        &external_pane,
    );
    assert!(
        closed.status.success(),
        "self-unregister failed: stdout={} stderr={}",
        String::from_utf8_lossy(&closed.stdout),
        String::from_utf8_lossy(&closed.stderr)
    );
    let closed: Value = serde_json::from_slice(&closed.stdout).unwrap();
    assert_eq!(closed["result"]["outcome"], "complete", "{closed}");
    assert_eq!(
        closed["result"]["close"]["binding_retirement"]["state"], "verified",
        "{closed}"
    );
    assert_eq!(
        closed["result"]["close"]["route_retirement"]["state"], "verified",
        "{closed}"
    );
    let pane_pid_after = tmux(
        &fixture.tmux_socket,
        &[
            "display-message",
            "-p",
            "-t",
            &external_pane.pane_id,
            "#{pane_pid}",
        ],
    )
    .stdout;
    assert_eq!(
        pane_pid_after, pane_pid_before,
        "self-unregister must not replace the peer pane"
    );
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

    // Exercise the board through the actual CLI after public Create + Bind.
    let board_call = |args: &[&str], child: bool| -> Value {
        let mut command = fixture.configured_command(args, None);
        if child {
            configure_child_runtime(&mut command);
        } else {
            configure_runtime(&mut command);
        }
        let output = command.output().expect("managed board command");
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let publish = |id: &str, child: bool| {
        board_call(
            &[
                "board",
                "publish",
                id,
                "--title",
                "Board work",
                "--description",
                "Own scoped work",
                "--delivery-condition",
                "Verified result",
                "--test-condition",
                "Public CLI assertions",
            ],
            child,
        )
    };
    publish("managed-board-public", false);
    let claimed = board_call(
        &[
            "board",
            "claim",
            "managed-board-public",
            "--expected-revision",
            "1",
        ],
        true,
    );
    assert_eq!(claimed["task"]["status"], "working");
    let actual = board_call(&["task", "status", "managed-board-public"], true);
    assert_eq!(actual["owner"], "bind-created-peer");
    publish("managed-board-private", true);
    let private_board = board_call(&["board", "show"], true);
    assert!(private_board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|task| task["id"] == "managed-board-private"));

    let revision = claimed["task"]["revision"].as_u64().unwrap().to_string();
    let described = board_call(
        &[
            "board",
            "describe",
            "managed-board-public",
            "--expected-revision",
            &revision,
            "--title",
            "Updated public work",
            "--description",
            "Public work stays public",
            "--delivery-condition",
            "Verified result",
            "--test-condition",
            "Public CLI assertions",
        ],
        true,
    );
    let parent_board = board_call(&["board", "show"], false);
    let public = parent_board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "managed-board-public")
        .unwrap();
    assert_ne!(
        public["owner"], "bind-created-peer",
        "public responsibility is the parent"
    );
    // A failed claim uses the same public projection instead of exposing child identity.
    let public_revision = described["task"]["revision"].as_u64().unwrap().to_string();
    let mut competing = fixture.configured_command(
        &[
            "board",
            "claim",
            "managed-board-public",
            "--expected-revision",
            &public_revision,
        ],
        None,
    );
    configure_runtime(&mut competing);
    let rejected = competing.output().unwrap();
    assert!(!rejected.status.success());
    assert!(!String::from_utf8_lossy(&rejected.stderr).contains("bind-created-peer"));

    board_call(
        &[
            "board",
            "update",
            "managed-board-public",
            "--expected-revision",
            &public_revision,
            "--status",
            "blocked",
            "--next",
            "Await authorized input; continue private queue",
        ],
        true,
    );
    let private_claim = board_call(
        &[
            "board",
            "claim",
            "managed-board-private",
            "--expected-revision",
            "1",
        ],
        true,
    );
    assert_eq!(private_claim["task"]["status"], "working");
    let private_revision = private_claim["task"]["revision"]
        .as_u64()
        .unwrap()
        .to_string();
    board_call(
        &[
            "board",
            "update",
            "managed-board-private",
            "--expected-revision",
            &private_revision,
            "--status",
            "cancelled",
        ],
        true,
    );
    board_call(
        &[
            "task",
            "update",
            "managed-board-public",
            "--status",
            "cancelled",
        ],
        true,
    );

    // A parent remains authorized to dispatch after returning to ordinary peer.
    let mut clear = fixture.configured_command(
        &[
            "master",
            "clear",
            "--approval",
            "isolated parent dispatch acceptance",
        ],
        None,
    );
    configure_runtime(&mut clear);
    assert!(clear.output().unwrap().status.success());
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

    board_call(&["subagent", "working", "managed-created-peer"], true);
    let assigned_task = format!("task-{success_msg_id}");
    board_call(
        &["task", "update", &assigned_task, "--status", "blocked"],
        true,
    );
    let next_reply = subagent_mcp_call(json!({
        "action":"send", "id":"managed-created-peer",
        "subject":"next task after blocker", "body":"Continue independent work while the first task stays blocked."
    }));
    assert_eq!(next_reply["isError"], false, "{next_reply}");
    let mut promote_again = fixture.configured_command(
        &[
            "master",
            "promote",
            "--approval",
            "isolated remaining Create acceptance",
        ],
        None,
    );
    configure_runtime(&mut promote_again);
    assert!(promote_again.output().unwrap().status.success());

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
        failed_send_response["error"].as_str().is_some_and(|error| {
            error.starts_with("APPSERVER_NOTIFICATION_REJECTED:")
                && error.contains("fixture refused generic notification")
        }),
        "{failed_send_response}"
    );
    assert_eq!(
        failed_send_response["durable"], true,
        "{failed_send_response}"
    );
    assert_eq!(
        failed_send_response["failure"], "notification_delivery_failed",
        "{failed_send_response}"
    );
    assert_eq!(
        failed_send_response["notification"], "subscribed-not-sent",
        "{failed_send_response}"
    );
    assert_eq!(
        failed_send_response["repair_required"], true,
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
    let mut failed_message_status =
        fixture.configured_command(&["msg", failed_send_msg_id.as_str()], None);
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
