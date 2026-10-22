#[test]
fn peer_lifecycle_c1_unfinished_task_refuses_close_without_archive() {
    let (mut fixture, _master_host, child_host, master, child) =
        lifecycle_native_pair_fixture("peer-c1", "ack_only");
    let worker_id = child["worker_id"].as_str().unwrap().to_owned();
    lifecycle_replay_events(
        &mut fixture,
        &child,
        &[json!({
            "ev": "TaskCreated",
            "task": {
                "id": "peer-c1-task",
                "owner": worker_id,
                "created_by": worker_id,
                "created_ms": 1,
                "updated_ms": 1
            }
        })],
    );
    let target = lifecycle_read(&fixture, &master, Some(&worker_id))["result"]["target"].clone();
    let before = lifecycle_surfaces(&fixture, &master);
    let operation_id = "peer-c1-close";
    let capability = "peer-c1-capability";

    let refused = lifecycle_close(
        &fixture,
        &master,
        operation_id,
        capability,
        &target,
        "close idle peer",
    );
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["result"]["outcome"], "refused", "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .expect("a refused close is a typed failure")
            .contains("RESPONSIBILITY_CONFLICT"),
        "{refused}"
    );
    assert_eq!(
        refused["result"]["requires"]["kind"], "peer_responsibility",
        "{refused}"
    );
    assert!(
        refused["result"]["requires"]["sources"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "peer-c1-task"),
        "{refused}"
    );
    assert_eq!(
        child_host.archive_request_count(),
        0,
        "an unfinished task must be refused before any archive effect"
    );
    let query = lifecycle_query(&fixture, &master, operation_id, capability);
    assert_eq!(query["result"]["outcome"], "unknown", "{query}");
    assert!(
        query["error"]
            .as_str()
            .expect("an absent operation query is a typed failure")
            .contains("OPERATION_UNKNOWN"),
        "{query}"
    );
    assert_eq!(before, lifecycle_surfaces(&fixture, &master));
}

// C7: the current master can unregister itself without archiving its live
// thread. Control-plane retirement is the whole close effect.
#[test]
fn peer_lifecycle_c7_current_master_can_unregister_itself() {
    let (fixture, master_host, child_host, master, _child) =
        lifecycle_native_pair_fixture("peer-c7", "ack_only");
    let own = lifecycle_read(&fixture, &master, None);
    assert_eq!(own["result"]["outcome"], "ok", "{own}");
    let target = own["result"]["target"].clone();

    let closed = lifecycle_close(
        &fixture,
        &master,
        "peer-c7-close",
        "peer-c7-capability",
        &target,
        "unregister self without archiving the live thread",
    );
    assert_eq!(closed["result"]["outcome"], "complete", "{closed}");
    assert_eq!(
        closed["result"]["close"]["runtime_archive"]["state"], "not-applicable",
        "{closed}"
    );
    assert_eq!(
        closed["result"]["close"]["worker_retirement"]["state"], "verified",
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
    assert_eq!(
        closed["result"]["close"]["subscription_retirement"]["state"], "verified",
        "{closed}"
    );
    let query = lifecycle_query(&fixture, &master, "peer-c7-close", "peer-c7-capability");
    assert_eq!(
        query["result"], closed["result"],
        "the retained self-unregister receipt must be queryable"
    );
    assert_eq!(
        master_host.archive_request_count(),
        0,
        "self-unregister must not archive the live master thread"
    );
    assert_eq!(
        child_host.archive_request_count(),
        0,
        "self-unregister must not touch an unrelated child host"
    );
}

// C7 responsibility counterexample: self-unregister uses the same durable
// responsibility snapshot as host close. An owned task refuses the operation
// before archive or control-plane retirement.
#[test]
fn peer_lifecycle_c7_self_unregister_with_task_retires_only_its_binding() {
    let (mut fixture, master_host, master) = lifecycle_native_fixture("peer-c7-responsibility");
    let worker_id = master["worker_id"].as_str().unwrap().to_owned();
    lifecycle_replay_events(
        &mut fixture,
        &master,
        &[json!({
            "ev": "TaskCreated",
            "task": {
                "id": "peer-c7-self-task",
                "owner": worker_id,
                "created_by": worker_id,
                "created_ms": 1,
                "updated_ms": 1
            }
        })],
    );
    let target = lifecycle_target(&fixture, &master);
    let completed = lifecycle_close(
        &fixture,
        &master,
        "peer-c7-responsibility-close",
        "peer-c7-responsibility-capability",
        &target,
        "attempt self-unregister with an owned task",
    );
    assert_eq!(completed["ok"], true, "{completed}");
    assert_eq!(completed["result"]["outcome"], "complete", "{completed}");
    assert_eq!(master_host.archive_request_count(), 0);
    assert!(std::fs::read_to_string(fixture.project_journal()).unwrap().contains("peer-c7-self-task"));
}

// C3: an unavailable archive result stays unknown and an identical operation
// retry returns the retained result without issuing another archive.
#[test]
fn peer_lifecycle_c3_unknown_archive_is_not_replayed() {
    let (fixture, _master_host, child_host, master, child) =
        lifecycle_native_pair_fixture("peer-c3", "archive_unknown");
    let target =
        lifecycle_read(&fixture, &master, child["worker_id"].as_str())["result"]["target"].clone();

    let first = lifecycle_close(
        &fixture,
        &master,
        "peer-c3-close",
        "peer-c3-capability",
        &target,
        "close peer after lost archive response",
    );
    assert_eq!(first["result"]["outcome"], "unknown", "{first}");
    assert_eq!(
        first["result"]["close"]["runtime_archive"]["state"],
        "unknown"
    );
    assert_eq!(child_host.archive_request_count(), 1);

    let retry = lifecycle_close(
        &fixture,
        &master,
        "peer-c3-close",
        "peer-c3-capability",
        &target,
        "close peer after lost archive response",
    );
    assert_eq!(
        retry["result"], first["result"],
        "same operation returns its retained result"
    );
    assert_eq!(
        child_host.archive_request_count(),
        1,
        "unknown host effects are not replayed"
    );
}

// C6: only the selected host's exact missing-thread error proves terminal
// absence; the control owners still retire the exact registered binding.
#[test]
fn peer_lifecycle_c6_exact_missing_thread_is_classified_and_retired() {
    let (fixture, _master_host, child_host, master, child) =
        lifecycle_native_pair_fixture("peer-c6", "archive_missing");
    let target =
        lifecycle_read(&fixture, &master, child["worker_id"].as_str())["result"]["target"].clone();

    let closed = lifecycle_close(
        &fixture,
        &master,
        "peer-c6-close",
        "peer-c6-capability",
        &target,
        "close exact missing peer",
    );
    assert_eq!(
        closed["result"]["close"]["runtime_archive"]["state"], "missing",
        "{closed}"
    );
    assert_eq!(closed["result"]["outcome"], "complete", "{closed}");
    assert_eq!(
        closed["result"]["close"]["binding_retirement"]["state"], "verified",
        "{closed}"
    );
    assert_eq!(
        closed["result"]["close"]["route_retirement"]["state"], "verified",
        "{closed}"
    );
    assert_eq!(child_host.archive_request_count(), 1);
    assert_eq!(
        lifecycle_read(
            &fixture,
            &master,
            Some(child["worker_id"].as_str().unwrap())
        )["result"]["outcome"],
        "closed",
        "the exact target remains absent after its control-plane retirement"
    );
}

// C8: a registered tmux target has no exact native archive operation. Close
// reports the unmet terminal obligation instead of falling back or succeeding.
#[test]
fn peer_lifecycle_c8_unsupported_transport_stays_cleanup_open() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, master_pane) = Fixture::isolated("peer-c8");
    let master_seed = fixture.registered_context("peer-c8-master", &master_pane);
    let master = lifecycle_identity(&fixture, &master_seed);
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "isolated unsupported transport fixture",
        ],
        Some(&master_pane),
    );
    let child_pane = second_pane(&fixture, "peer-c8-child", master_pane.server_pid);
    let child_seed = fixture.registered_context("peer-c8-child", &child_pane);
    let child = lifecycle_identity(&fixture, &child_seed);
    let target = lifecycle_read(
        &fixture,
        &master,
        Some(child["worker_id"].as_str().unwrap()),
    )["result"]["target"]
        .clone();

    let closed = lifecycle_close(
        &fixture,
        &master,
        "peer-c8-close",
        "peer-c8-capability",
        &target,
        "close unsupported tmux peer",
    );
    assert_eq!(closed["result"]["outcome"], "cleanup-open", "{closed}");
    assert_eq!(
        closed["result"]["close"]["runtime_archive"]["state"], "refused",
        "{closed}"
    );
    assert!(
        closed["error"]
            .as_str()
            .unwrap_or_default()
            .contains("TRANSPORT_UNSUPPORTED"),
        "{closed}"
    );
    let query = lifecycle_query(&fixture, &master, "peer-c8-close", "peer-c8-capability");
    assert_eq!(query["result"]["outcome"], "cleanup-open", "{query}");
}

#[test]
fn peer_lifecycle_closes_a_tmux_peer_only_after_its_pane_is_proven_missing() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, master_pane) = Fixture::isolated("peer-tmux-missing-close");
    let master_seed = fixture.registered_context("peer-tmux-missing-master", &master_pane);
    let master = lifecycle_identity(&fixture, &master_seed);
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "isolated missing tmux retirement fixture",
        ],
        Some(&master_pane),
    );
    let child_pane = second_pane(
        &fixture,
        "peer-tmux-missing-child",
        master_pane.server_pid,
    );
    let child_seed = fixture.registered_context("peer-tmux-missing-child", &child_pane);
    let child = lifecycle_identity(&fixture, &child_seed);
    let target = lifecycle_read(
        &fixture,
        &master,
        Some(child["worker_id"].as_str().unwrap()),
    )["result"]["target"]
        .clone();

    let killed = tmux(
        &fixture.tmux_socket,
        &["kill-pane", "-t", &child_pane.pane_id],
    );
    assert!(
        killed.status.success(),
        "kill lifecycle target pane: {}",
        String::from_utf8_lossy(&killed.stderr)
    );

    let closed = lifecycle_close(
        &fixture,
        &master,
        "peer-tmux-missing-close",
        "peer-tmux-missing-capability",
        &target,
        "retire a tmux peer whose exact pane is gone",
    );
    assert_eq!(closed["result"]["outcome"], "complete", "{closed}");
    assert_eq!(
        closed["result"]["close"]["runtime_archive"]["state"], "missing",
        "{closed}"
    );
    assert_eq!(
        closed["result"]["close"]["worker_retirement"]["state"], "verified",
        "{closed}"
    );
    let read = lifecycle_read(
        &fixture,
        &master,
        Some(child["worker_id"].as_str().unwrap()),
    );
    assert_eq!(read["result"]["outcome"], "closed", "{read}");
}

fn tmux_probe_wrapper(label: &str, script: &str) -> (std::path::PathBuf, std::ffi::OsString) {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!(
        "collab-lifecycle-tmux-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let wrapper = root.join("tmux");
    std::fs::write(&wrapper, script).unwrap();
    let mut permissions = std::fs::metadata(&wrapper).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&wrapper, permissions).unwrap();
    let original = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![root.clone()];
    paths.extend(std::env::split_paths(&original));
    (root, std::env::join_paths(paths).unwrap())
}

fn close_with_tmux_probe(label: &str, script: &str, expected_error: &str) -> Value {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, master_pane) = Fixture::isolated(label);
    let master_seed = fixture.registered_context(&format!("{label}-master"), &master_pane);
    let master = lifecycle_identity(&fixture, &master_seed);
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "isolated uncertain tmux retirement fixture",
        ],
        Some(&master_pane),
    );
    let child_pane = second_pane(&fixture, &format!("{label}-child"), master_pane.server_pid);
    let child_seed = fixture.registered_context(&format!("{label}-child"), &child_pane);
    let child = lifecycle_identity(&fixture, &child_seed);
    let target = lifecycle_read(
        &fixture,
        &master,
        Some(child["worker_id"].as_str().unwrap()),
    )["result"]["target"]
        .clone();
    let endpoint = &target["transport"]["tmux_endpoint"];
    let script = script
        .replace("{server_pid}", &endpoint["server_pid"].to_string())
        .replace("{session}", endpoint["tmux_session_id"].as_str().unwrap())
        .replace("{pane}", endpoint["pane_id"].as_str().unwrap())
        .replace("{pane_pid}", &endpoint["pane_pid"].to_string());
    let (wrapper_root, path) = tmux_probe_wrapper(label, &script);
    fixture.restart_daemon_with_env("PATH", path.to_str().unwrap());

    let closed = lifecycle_close(
        &fixture,
        &master,
        &format!("{label}-close"),
        &format!("{label}-capability"),
        &target,
        "close a tmux peer whose exact pane state is uncertain",
    );
    assert_eq!(closed["result"]["outcome"], "unknown", "{closed}");
    assert_eq!(
        closed["result"]["close"]["runtime_archive"]["state"], "unknown",
        "{closed}"
    );
    assert!(
        closed["error"].as_str().unwrap_or_default().contains(expected_error),
        "{closed}"
    );
    let read = lifecycle_read(
        &fixture,
        &master,
        Some(child["worker_id"].as_str().unwrap()),
    );
    assert_ne!(read["result"]["outcome"], "closed", "{read}");
    assert_eq!(
        read["result"]["target"]["worker_id"],
        child["worker_id"],
        "an uncertain probe must keep the exact worker record: {read}"
    );
    assert_eq!(
        read["result"]["close"]["runtime_archive"]["state"], "unknown",
        "{read}"
    );
    let _ = std::fs::remove_dir_all(wrapper_root);
    fixture.restart_daemon();
    closed
}

// A probe that cannot observe the exact pane stays unknown. It must not be
// treated as a missing terminal, and no control-plane record may retire.
#[test]
fn peer_lifecycle_tmux_probe_error_stays_unknown_and_does_not_retire() {
    close_with_tmux_probe(
        "peer-tmux-probe-error",
        "#!/bin/sh\necho 'forced tmux probe failure' >&2\nexit 1\n",
        "PEER_LIFECYCLE_TMUX_PROBE_UNKNOWN",
    );
}

// A reachable tmux server that reports an unparseable pane-dead state is
// uncertain, not absent. The close must keep the target and its control plane.
#[test]
fn peer_lifecycle_tmux_unknown_presence_does_not_retire() {
    close_with_tmux_probe(
        "peer-tmux-unknown",
        "#!/bin/sh\nprintf '%s\\n' '{server_pid}\t{session}\t{pane}\t{pane_pid}\t?'\n",
        "TMUX_PANE_UNKNOWN",
    );
}

// C9: restart restores both the unknown Close receipt and its active fence.
// Query is read-only and a new Update cannot get past that retained fence.
#[test]
fn peer_lifecycle_c9_restart_preserves_unknown_close_fence_and_query() {
    let (mut fixture, _master_host, child_host, master, child) =
        lifecycle_native_pair_fixture("peer-c9", "archive_unknown");
    let child_id = child["worker_id"].as_str().unwrap();
    let target = lifecycle_read(&fixture, &master, Some(child_id))["result"]["target"].clone();
    let closed = lifecycle_close(
        &fixture,
        &master,
        "peer-c9-close",
        "peer-c9-capability",
        &target,
        "close peer with unknown host terminal",
    );
    assert_eq!(closed["result"]["outcome"], "unknown", "{closed}");
    assert_eq!(child_host.archive_request_count(), 1);

    fixture.stop_daemon();
    fixture.restart_daemon();
    let before_query = lifecycle_surfaces(&fixture, &master);
    let query = lifecycle_query(&fixture, &master, "peer-c9-close", "peer-c9-capability");
    assert_eq!(query["result"]["outcome"], "unknown", "{query}");
    assert_eq!(query["result"]["phase"], "unknown", "{query}");
    assert_eq!(
        before_query,
        lifecycle_surfaces(&fixture, &master),
        "query does not release or rewrite the fence"
    );
    let read = lifecycle_read(&fixture, &master, Some(child_id));
    assert_eq!(read["result"]["outcome"], "unknown", "{read}");
    assert_eq!(
        read["result"]["close"]["runtime_archive"]["state"], "unknown",
        "{read}"
    );

    let cwd = canonical_root(&fixture);
    let blocked = lifecycle_update(
        &fixture,
        &master,
        "peer-c9-update",
        "peer-c9-update-capability",
        &target,
        &cwd,
    );
    assert_eq!(blocked["ok"], false, "{blocked}");
    assert_eq!(
        child_host.settings_request_count(),
        0,
        "the restarted daemon retains the close fence"
    );
    assert_eq!(
        child_host.archive_request_count(),
        1,
        "neither query nor a fenced update replays Close"
    );
}

// C2: a verified host terminal does not close the operation when the exact
// control binding cannot advance its generation; the unresolved fence remains.
#[test]
fn peer_lifecycle_c2_host_terminal_with_control_retirement_failure_stays_open() {
    let (mut fixture, _master_host, child_host, master, child) =
        lifecycle_native_pair_fixture("peer-c2", "ack_only");
    let child_id = child["worker_id"].as_str().unwrap();
    let mut binding = read_lines(&fixture.project_journal())
        .into_iter()
        .rev()
        .find(|record| {
            record["ev"] == "GlobalRuntimeBound" && record["binding"]["agent_id"] == child_id
        })
        .expect("the exact child's committed runtime binding");
    let mut route = read_lines(&fixture.project_journal())
        .into_iter()
        .rev()
        .find(|record| {
            record["ev"] == "GlobalCurrentThreadRouteSet"
                && record["binding"]["agent_id"] == child_id
        })
        .expect("the exact child's committed current route");
    binding["binding"]["endpoint_generation"] = json!(u64::MAX);
    route["binding"]["endpoint_generation"] = json!(u64::MAX);
    lifecycle_replay_events(&mut fixture, &master, &[binding, route]);

    let target = lifecycle_read(&fixture, &master, Some(child_id))["result"]["target"].clone();
    assert_eq!(target["endpoint_generation"], json!(u64::MAX));
    let closed = lifecycle_close(
        &fixture,
        &master,
        "peer-c2-close",
        "peer-c2-capability",
        &target,
        "close peer with unadvanceable control generation",
    );
    assert_eq!(
        closed["result"]["close"]["runtime_archive"]["state"], "verified",
        "{closed}"
    );
    assert_eq!(closed["result"]["outcome"], "cleanup-open", "{closed}");
    assert_eq!(
        closed["result"]["close"]["binding_retirement"]["state"], "failed",
        "{closed}"
    );
    assert_eq!(
        closed["result"]["close"]["route_retirement"]["state"], "failed",
        "{closed}"
    );
    assert_eq!(
        child_host.archive_request_count(),
        1,
        "host terminal was proven exactly once"
    );
    assert_eq!(
        lifecycle_read(&fixture, &master, Some(child_id))["result"]["outcome"],
        "cleanup-open",
        "the control-plane fence remains visible after retirement fails"
    );
}
#[test]
fn desktop_context_same_session_refreshes_native_thread_and_subscription() {
    let mut fixture = Fixture::without_tmux("desktop-thread-refresh");
    let old_host = TestAppServer::start_with_effect_at(
        &fixture.root,
        "old.sock",
        "stable-desktop-session",
        "old-desktop-thread",
        "completed_turn",
    );
    let new_host = TestAppServer::start_with_effect_at(
        &fixture.root,
        "new.sock",
        "stable-desktop-session",
        "new-desktop-thread",
        "completed_turn",
    );
    let restore = |operation: &str, thread: &str, host: &TestAppServer| {
        let provide = json!({"session_id":"stable-desktop-session", "thread_id":thread,
            "endpoint":host.endpoint(), "namespace":"codex_app"})
        .to_string();
        let output = fixture
            .configured_command(&["context", "--op", operation, "--provide", &provide], None)
            .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
            .output()
            .unwrap();
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(output.status.success(), "{response}");
        response
    };
    let old = restore("desktop-old", "old-desktop-thread", &old_host);
    let saved_identity = lifecycle_identity(&fixture, &old);
    assert_eq!(old["result"]["outcome"], "completed", "{old}");
    let restored = restore("desktop-new", "new-desktop-thread", &new_host);
    assert_eq!(restored["result"]["outcome"], "completed", "{restored}");
    let before = &old["result"]["snapshot"];
    let after = &restored["result"]["snapshot"];
    assert_eq!(
        after["identity"]["worker_id"],
        before["identity"]["worker_id"]
    );
    assert_eq!(after["binding"]["session_id"], "stable-desktop-session");
    assert_eq!(
        after["binding"]["native_thread_id"], "new-desktop-thread",
        "{restored}"
    );
    assert_eq!(
        after["identity"]["transport"]["endpoint"],
        new_host.endpoint()
    );
    assert_eq!(
        after["identity"]["transport"]["thread_id"],
        "new-desktop-thread"
    );
    let subscriptions = after["subscriptions"].as_array().unwrap();
    assert!(
        subscriptions
            .iter()
            .any(|s| s["event"] == "direct-message" && s["target"] == "new-desktop-thread"),
        "{restored}"
    );
    let current_identity = lifecycle_identity(&fixture, &restored);
    assert_eq!(current_identity["token"], saved_identity["token"]);
    assert_eq!(
        current_identity["runtime"]["native_thread_id"],
        "new-desktop-thread"
    );
    assert!(
        after["binding"]["endpoint_generation"].as_u64().unwrap()
            > before["binding"]["endpoint_generation"].as_u64().unwrap()
    );
    let again = restore("desktop-repeat", "new-desktop-thread", &new_host);
    assert_eq!(again["result"]["snapshot"]["binding"], after["binding"]);
    let host_events: Vec<Value> = fs::read_to_string(fixture.project_journal())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(host_events
        .iter()
        .any(|e| e["ev"] == "GlobalCurrentThreadRouteRetired"
            && e["binding"]["native_thread_id"] == "old-desktop-thread"));
    assert!(host_events
        .iter()
        .any(|e| e["ev"] == "GlobalCurrentThreadRouteSet"
            && e["binding"]["native_thread_id"] == "new-desktop-thread"));
    let before_replay = fs::read_to_string(fixture.project_journal()).unwrap();
    fixture.restart_daemon();
    let partial =
        json!({"session_id":"stable-desktop-session","thread_id":"new-desktop-thread"}).to_string();
    let output = fixture
        .configured_command(
            &["context", "--op", "desktop-replay", "--provide", &partial],
            None,
        )
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
        .output()
        .unwrap();
    let replay: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{replay}");
    assert_eq!(replay["result"]["snapshot"]["binding"], after["binding"]);
    assert_eq!(lifecycle_identity(&fixture, &replay), current_identity);
    let replay_journal = fs::read_to_string(fixture.project_journal()).unwrap();
    assert_eq!(
        replay_journal.matches("CommandStarted").count(),
        before_replay.matches("CommandStarted").count()
    );
    assert!(
        !fixture
            .host_state
            .join("register-reply-evidence.jsonl")
            .exists(),
        "ordinary restoration does not challenge"
    );
    let moved_thread =
        json!({"session_id":"stable-desktop-session", "thread_id":"latest-desktop-thread"})
            .to_string();
    let output = fixture
        .configured_command(
            &[
                "context",
                "--op",
                "desktop-thread-only",
                "--provide",
                &moved_thread,
            ],
            None,
        )
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
        .output()
        .unwrap();
    let moved: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(output.status.success(), "{moved}");
    assert_eq!(
        moved["result"]["snapshot"]["binding"]["native_thread_id"],
        "latest-desktop-thread"
    );
    assert_eq!(
        moved["result"]["snapshot"]["identity"]["transport"]["endpoint"],
        new_host.endpoint()
    );
}
