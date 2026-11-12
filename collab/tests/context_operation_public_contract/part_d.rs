#[test]
fn identity_map_reset_intent_finishes_before_startup_restoration() {
    let (mut fixture, _host, identity) = lifecycle_native_fixture_effect("map-reset-cut", "ack_only");
    fixture.stop_daemon();
    let path = fixture.host_state.join("identity-map.json");
    let mut map: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let scope = identity["project_scope"].as_str().unwrap();
    map["pending_resets"] = json!([scope]);
    fs::write(&path, serde_json::to_vec(&map).unwrap()).unwrap();
    fixture.restart_after_external_stop();
    let restored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(restored["pending_resets"].as_array().is_none_or(|items| items.is_empty()), "accepted reset must finish before restoring identities");
    assert!(!restored["slots"].as_array().unwrap().iter().any(|slot| slot["identity_key"]["project_scope"] == scope));
}

#[test]
fn peer_lifecycle_c1_unfinished_task_refuses_close_without_archive() {
    let (fixture, _master_host, child_host, master, child) =
        lifecycle_native_pair_fixture("peer-c1", "ack_only");
    let worker_id = child["worker_id"].as_str().unwrap().to_owned();
    lifecycle_register_task(&fixture, &child, "peer-c1-task");
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

// Self-unregister retires identity while preserving its current task record.
#[test]
fn peer_lifecycle_c7_self_unregister_with_task_retires_only_its_binding() {
    let (fixture, master_host, master) = lifecycle_native_fixture("peer-c7-responsibility");
    let task = lifecycle_register_task(&fixture, &master, "peer-c7-self-task");
    let state_path = fixture.project_state();
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
    let state: Value = serde_json::from_slice(&fs::read(state_path).unwrap()).unwrap();
    assert_eq!(state["state"]["tasks"]["peer-c7-self-task"], task);
    let map: Value = serde_json::from_slice(&fs::read(fixture.host_state.join("identity-map.json")).unwrap()).unwrap();
    assert!(!map["slots"].as_array().unwrap().iter().any(|slot|
        slot["identity_status"] == "active" && slot["current"]["binding"]["agent_id"] == master["worker_id"]));
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
    let restore = |fixture: &Fixture, operation: &str, thread: &str, host: &TestAppServer| {
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
    let old = restore(&fixture, "desktop-old", "old-desktop-thread", &old_host);
    let saved_identity = lifecycle_identity(&fixture, &old);
    assert_eq!(old["result"]["outcome"], "completed", "{old}");
    let restored = restore(&fixture, "desktop-new", "new-desktop-thread", &new_host);
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
    assert!(after["binding"].get("endpoint_generation").is_none());
    let again = restore(&fixture, "desktop-repeat", "new-desktop-thread", &new_host);
    assert_eq!(again["result"]["snapshot"]["binding"], after["binding"]);
    let current = fixture.current_state();
    let routes = current["global"]["current_thread_routes"].as_array().unwrap();
    assert!(!routes.iter().any(|row| row[1]["native_thread_id"] == "old-desktop-thread"));
    assert!(routes.iter().any(|row| row[1]["native_thread_id"] == "new-desktop-thread"));

    // A changed Desktop thread without current bridge enrollment must fail
    // on that capability. History cannot publish the saved endpoint and must
    // not retire or replace identity, binding, route or subscription state.
    let missing_routes = current["global"]["current_thread_routes"].clone();
    let missing_workers = current["workers"].clone();
    let missing_map = fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let missing_facts =
        json!({"session_id":"stable-desktop-session","thread_id":"latest-desktop-thread"})
            .to_string();
    let output = fixture
        .configured_command(
            &[
                "context",
                "--op",
                "desktop-missing-endpoint",
                "--provide",
                &missing_facts,
            ],
            None,
        )
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("DESKTOP_BRIDGE_CAPABILITY_REQUIRED"), "{output:?}");
    assert!(output.stdout.is_empty(), "do not invent an AppServer recovery requirement");
    assert_eq!(
        lifecycle_identity(&fixture, &restored),
        current_identity,
        "a missing-facts call must leave the persisted identity untouched"
    );
    assert!(
        !fixture
            .host_state
            .join("register-reply-evidence.jsonl")
            .exists(),
        "ordinary restoration does not challenge"
    );
    let unchanged = restore(&fixture, "desktop-state-check", "new-desktop-thread", &new_host);
    assert_eq!(
        unchanged["result"]["snapshot"]["binding"],
        after["binding"],
        "{unchanged}"
    );
    assert_eq!(
        unchanged["result"]["snapshot"]["subscriptions"],
        after["subscriptions"],
        "{unchanged}"
    );
    let current = fixture.current_state();
    assert_eq!(current["global"]["current_thread_routes"], missing_routes);
    assert_eq!(current["workers"], missing_workers);
    assert_eq!(fs::read(fixture.host_state.join("identity-map.json")).unwrap(), missing_map);

    // Complete current facts on the moved thread rebind through the same
    // identity and endpoint, then repeat idempotently.
    let moved = restore(&fixture, "desktop-thread-full", "latest-desktop-thread", &new_host);
    assert_eq!(
        moved["result"]["snapshot"]["binding"]["native_thread_id"],
        "latest-desktop-thread",
        "{moved}"
    );
    assert_eq!(
        moved["result"]["snapshot"]["identity"]["transport"]["endpoint"],
        new_host.endpoint(),
        "{moved}"
    );
    assert!(moved["result"]["snapshot"]["binding"].get("endpoint_generation").is_none());
    assert!(
        moved["result"]["snapshot"]["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["event"] == "direct-message" && s["target"] == "latest-desktop-thread"),
        "{moved}"
    );
    let moved_identity = lifecycle_identity(&fixture, &moved);
    let moved_binding = moved["result"]["snapshot"]["binding"].clone();
    let moved_again = restore(
        &fixture,
        "desktop-thread-full-repeat",
        "latest-desktop-thread",
        &new_host,
    );
    assert_eq!(
        moved_again["result"]["snapshot"]["binding"],
        moved_binding,
        "{moved_again}"
    );
    let current = fixture.current_state();
    let routes = current["global"]["current_thread_routes"].as_array().unwrap();
    assert!(!routes.iter().any(|row| row[1]["native_thread_id"] == "new-desktop-thread"));
    assert!(routes.iter().any(|row| row[1]["native_thread_id"] == "latest-desktop-thread"));

    // Restart retains the current endpoint, binding and credentials.
    fixture.restart_daemon();
    let replay = restore(&fixture, "desktop-replay", "latest-desktop-thread", &new_host);
    assert_eq!(replay["result"]["snapshot"]["binding"], moved_binding);
    assert_eq!(lifecycle_identity(&fixture, &replay), moved_identity);
    assert!(
        !fixture
            .host_state
            .join("register-reply-evidence.jsonl")
            .exists(),
        "ordinary restoration does not challenge"
    );
    let before_query = lifecycle_surfaces(&fixture, &moved_identity);
    let provided = json!({"session_id":"stable-desktop-session", "thread_id":"latest-desktop-thread",
        "endpoint":new_host.endpoint(), "namespace":"codex_app"}).to_string();
    let query = fixture.configured_command(&["context", "--query", "--provide", &provided], None)
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop").output().unwrap();
    let queried = success_payload(&query);
    assert_eq!(queried["result"]["outcome"], "completed", "{queried}");
    assert_eq!(
        queried["result"]["snapshot"]["identity"]["worker_id"],
        moved_identity["worker_id"]
    );
    assert_eq!(
        queried["result"]["snapshot"]["binding"],
        moved_binding,
        "{queried}"
    );
    assert_eq!(
        lifecycle_surfaces(&fixture, &moved_identity),
        before_query,
        "a current read-only query must not mutate identity or business state"
    );
}

#[test]
fn public_context_rex_facts_skip_desktop_discovery() {
    let fixture = Fixture::without_tmux("rex-explicit-transport");
    let provide = json!({
        "session_id": "rex-agent-session",
        "rex": {
            "server": "missing-rex-server",
            "session_id": "session:missing-rex-session",
            "block_id": "block:missing-rex-block",
            "cwd": fixture.root.to_string_lossy(),
        },
    })
    .to_string();
    let output = fixture
        .configured_command(
            &[
                "context",
                "--op",
                "rex-explicit-transport",
                "--provide",
                &provide,
            ],
            None,
        )
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("DESKTOP_BRIDGE_CAPABILITY_REQUIRED"),
        "a Rex-only report must not go through Desktop discovery: {stderr}"
    );
    assert!(
        stderr.contains("REX_NOT_DELIVERED:") || stderr.contains("REX_UNKNOWN:"),
        "the CLI must preserve the Rex adapter failure: {stderr}"
    );
}

#[test]
fn public_context_partial_rex_environment_skips_desktop_bridge() {
    let fixture = Fixture::without_tmux("rex-partial-env");
    let output = fixture
        .configured_command(&["context", "--op", "rex-partial-env"], None)
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
        .env(
            "REX_SESSION",
            "session:01a1c47f-7e66-7df6-b484-e727bc9e1811",
        )
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("DESKTOP_BRIDGE_CAPABILITY_REQUIRED")
            && !stderr.contains("DESKTOP_BRIDGE_THREAD_REQUIRED"),
        "a partial Rex environment must not go through Desktop discovery: {stderr}"
    );
}

#[test]
fn public_context_invalid_rex_environment_fails_without_desktop_fallback() {
    let fixture = Fixture::without_tmux("rex-invalid-env");
    let output = fixture
        .configured_command(&["context", "--op", "rex-invalid-env"], None)
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
        .env("REX_SESSION", "session:not-a-canonical-id")
        .env("REX_BLOCK", "block:not-a-canonical-id")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("REX_NOT_DELIVERED:") || stderr.contains("REX_UNKNOWN:"),
        "invalid Rex facts must return a typed Rex discovery error: {stderr}"
    );
    assert!(
        !stderr.contains("DESKTOP_BRIDGE_CAPABILITY_REQUIRED")
            && !stderr.contains("DESKTOP_BRIDGE_THREAD_REQUIRED"),
        "an invalid Rex pair must not fall through to Desktop discovery: {stderr}"
    );
    assert!(
        !fixture.host_state.join("identity-map.json").exists(),
        "invalid Rex facts must not publish an identity"
    );
}

fn register_appserver_binding(
    fixture: &Fixture,
    host: &TestAppServer,
    session_id: &str,
    thread_id: &str,
    operation: &str,
) -> Value {
    let provide = json!({
        "session_id": session_id,
        "thread_id": thread_id,
        "endpoint": host.endpoint(),
        "namespace": "codex_app"
    })
    .to_string();
    let output = fixture
        .configured_command(&["context", "--op", operation, "--provide", &provide], None)
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
        .output()
        .unwrap();
    assert!(output.status.success(), "AppServer register failed: {output:?}");
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["outcome"], "completed", "{response}");
    response
}

// Explicit Rex facts must reach the Rex adapter even when the agent session
// already has a mapped AppServer binding.
#[test]
fn public_context_rex_facts_reach_rex_adapter_after_mapped_appserver_binding() {
    let fixture = Fixture::without_tmux("rex-facts-after-appserver");
    let host = TestAppServer::start_with_effect_at(
        &fixture.root,
        "old-desktop.sock",
        "rebind-agent-session",
        "old-desktop-thread",
        "completed_turn",
    );
    let _ = register_appserver_binding(
        &fixture,
        &host,
        "rebind-agent-session",
        "old-desktop-thread",
        "rex-rebind-register-appserver",
    );
    drop(host);
    let saved_binding = fs::read(fixture.host_state.join("identity-map.json"))
        .expect("read the published Desktop binding before Rex rebind");
    let provide = json!({
        "session_id": "rebind-agent-session",
        "rex": {
            "server": "missing-rex-server",
            "session_id": "session:11111111-1111-4111-8111-111111111111",
            "block_id": "block:22222222-2222-4222-8222-222222222222",
            "cwd": fixture.root.to_string_lossy()
        }
    })
    .to_string();
    let output = fixture
        .configured_command(
            &["context", "--op", "rex-rebind-explicit", "--provide", &provide],
            None,
        )
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("DESKTOP_BRIDGE_THREAD_REQUIRED"),
        "a saved Desktop binding must not demand its old native thread when Rex facts are supplied: {stderr}"
    );
    assert!(
        stderr.contains("REX_NOT_DELIVERED:") || stderr.contains("REX_UNKNOWN:"),
        "the Rex adapter failure must be preserved: {stderr}"
    );
    assert_eq!(
        fs::read(fixture.host_state.join("identity-map.json"))
            .expect("read the identity map after Rex adapter failure"),
        saved_binding,
        "a failed Rex rebind must not partially replace the published Desktop binding"
    );
}

// A session-only restore of a mapped AppServer session must not fabricate a
// Desktop-thread requirement when the old native thread is unavailable.
#[test]
fn public_context_mapped_session_restore_skips_missing_old_native_thread() {
    let fixture = Fixture::without_tmux("mapped-restore-session-only");
    let host = TestAppServer::start_with_effect_at(
        &fixture.root,
        "old-desktop.sock",
        "rebind-agent-session",
        "old-desktop-thread",
        "completed_turn",
    );
    let _ = register_appserver_binding(
        &fixture,
        &host,
        "rebind-agent-session",
        "old-desktop-thread",
        "mapped-restore-register",
    );
    drop(host);
    let provide = json!({"session_id": "rebind-agent-session"}).to_string();
    let output = fixture
        .configured_command(
            &["context", "--op", "mapped-restore-session", "--provide", &provide],
            None,
        )
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex Desktop")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("DESKTOP_BRIDGE_THREAD_REQUIRED"),
        "a mapped session must not require the gone Desktop native thread: {stderr}"
    );
    assert!(
        output.status.success(),
        "a mapped session restore should reach the daemon: stdout={} stderr={stderr}",
        String::from_utf8_lossy(&output.stdout)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["outcome"], "completed", "{response}");
    assert_eq!(
        response["result"]["snapshot"]["identity"]["transport"]["kind"],
        "appserver",
        "session-only restore must retain the saved transport, not claim a Rex rebind"
    );
}

// A master's public `collab context` snapshot must expose, in one read, the
// durable subordinate managers of its scope: activity, open/blocked tasks and
// whether each subordinate keeps a patrol armed. The child itself has no
// durable subordinates, so its own snapshot digest stays empty.
#[test]
fn public_context_scope_digest_lists_durable_subordinate_manager() {
    let (fixture, master_host, child_host, _master, child) =
        lifecycle_native_pair_fixture("scope-digest", "ack_only");
    let child_id = child["worker_id"]
        .as_str()
        .expect("child identity")
        .to_owned();
    let master_snapshot = fixture.context_provide(
        "scope-digest-master-read",
        &json!({
            "session_id": "lifecycle-master-session",
            "thread_id": "lifecycle-master-thread",
            "endpoint": master_host.endpoint(),
            "namespace": "codex_tui"
        }),
        None,
    );
    assert_eq!(
        master_snapshot["result"]["outcome"], "completed",
        "{master_snapshot}"
    );
    let digest = master_snapshot["result"]["snapshot"]["scope_digest"]
        .as_array()
        .expect("master snapshot carries a scope digest");
    let row = digest
        .iter()
        .find(|row| row["worker_id"].as_str() == Some(child_id.as_str()))
        .expect("the durable subordinate appears in the master digest");
    assert_eq!(row["role"], "worker", "{row}");
    assert!(
        row["activity"].is_string(),
        "the digest reports subordinate activity: {row}"
    );
    assert_eq!(
        row["patrol"]["armed"], false,
        "an unarmed subordinate patrol reads as not armed: {row}"
    );
    assert!(
        row["open_tasks"].is_array() && row["blocked"].is_array(),
        "the digest reports subordinate work and blocking waits: {row}"
    );

    let child_snapshot = fixture.context_provide(
        "scope-digest-child-read",
        &json!({
            "session_id": "lifecycle-child-session",
            "thread_id": "lifecycle-child-thread",
            "endpoint": child_host.endpoint(),
            "namespace": "codex_tui"
        }),
        None,
    );
    assert_eq!(
        child_snapshot["result"]["outcome"], "completed",
        "{child_snapshot}"
    );
    assert_eq!(
        child_snapshot["result"]["snapshot"]["scope_digest"],
        json!([]),
        "a peer with no durable subordinates has an empty digest: {child_snapshot}"
    );
}
