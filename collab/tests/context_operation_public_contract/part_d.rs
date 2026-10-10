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

// C7: the current master cannot close itself; the refusal leaves no operation
// record and neither the master route nor its child changes state.
#[test]
fn peer_lifecycle_c7_current_master_cannot_close_itself() {
    let (fixture, master_host, child_host, master, _child) =
        lifecycle_native_pair_fixture("peer-c7", "ack_only");
    let own = lifecycle_read(&fixture, &master, None);
    assert_eq!(own["result"]["outcome"], "ok", "{own}");
    let target = own["result"]["target"].clone();

    let refused = lifecycle_close(
        &fixture,
        &master,
        "peer-c7-close",
        "peer-c7-capability",
        &target,
        "close self",
    );
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["result"]["outcome"], "refused", "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .expect("a self-close is a typed refusal")
            .contains("CLOSE_UNAUTHORIZED"),
        "{refused}"
    );
    let query = lifecycle_query(&fixture, &master, "peer-c7-close", "peer-c7-capability");
    assert_eq!(query["result"]["outcome"], "unknown", "{query}");
    assert!(
        query["error"]
            .as_str()
            .expect("an absent operation query is a typed failure")
            .contains("OPERATION_UNKNOWN"),
        "{query}"
    );
    assert_eq!(
        lifecycle_read(&fixture, &master, None)["result"]["outcome"],
        "ok",
        "the master route must survive its own refused close"
    );
    assert_eq!(
        master_host.archive_request_count(),
        0,
        "a self-close refusal must not reach the master host"
    );
    assert_eq!(
        child_host.archive_request_count(),
        0,
        "a self-close refusal must not reach the child host"
    );
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
    let master_seed = fixture.context(Some("peer-c8-master"), Some(&master_pane));
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
    let child_seed = fixture.context(Some("peer-c8-child"), Some(&child_pane));
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
