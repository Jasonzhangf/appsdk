include!("support/context_operation_fixture.rs");

use std::fs;

fn lifecycle_identity(fixture: &Fixture, snapshot: &Value) -> Value {
    let id = snapshot["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap();
    serde_json::from_slice(
        &fs::read(
            fixture
                .host_state
                .join("identities")
                .join(id)
                .join("identity.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

fn a6_fixture(label: &str, effect: &str) -> (Fixture, TestAppServer, Value) {
    let (fixture, host, identity) = lifecycle_native_fixture_effect(label, effect);
    let output = fixture
        .configured_command(
            &[
                "master",
                "promote",
                "--approval",
                "isolated Create acceptance",
            ],
            None,
        )
        .env(
            "CODEX_APP_SERVER_SOCKET",
            host.endpoint().strip_prefix("unix://").unwrap(),
        )
        .env("CODEX_SESSION_ID", "lifecycle-session")
        .env("CODEX_THREAD_ID", "lifecycle-thread")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (fixture, host, identity)
}

fn a6_create(fixture: &Fixture, master: &Value, peer: &str, cwd: &Path, op: &str) -> Value {
    lifecycle_wire(
        fixture,
        master,
        json!({"op":"PeerLifecycle","request":{
            "action":"create","worker_id":master["worker_id"],"token":master["token"],
            "peer_id":peer,"cwd":cwd,"model":null,"operation_id":op,"query_capability":"a6-capability"
        }}),
    )
}

#[test]
fn peer_lifecycle_a6_success_ready_exact_registered_thread_and_replay() {
    let (fixture, host, master) = a6_fixture("a6-ready", "create_ready");
    let created = a6_create(&fixture, &master, "a6-ordinary", &fixture.root, "a6-create");
    assert_eq!(created["result"]["outcome"], "complete", "{created}");
    assert_eq!(created["result"]["create"]["thread_id"], "created-thread-1");
    assert_eq!(
        created["result"]["target"]["transport"]["thread_id"],
        "created-thread-1"
    );
    assert_eq!(
        created["result"]["target"]["transport"]["session_id"],
        "created-session"
    );
    assert_eq!(
        created["result"]["create"]["readiness"]["state"],
        "verified"
    );
    let readiness_challenge = &created["result"]["create"]["readiness"]["challenge"];
    assert_eq!(readiness_challenge["state"], "verified", "{created}");
    assert_eq!(
        readiness_challenge["turn_id"], "create-readiness-turn",
        "{created}"
    );
    assert!(
        readiness_challenge["marker_file"]
            .as_str()
            .unwrap()
            .starts_with(".collab-peer-lifecycle-challenge-"),
        "{created}"
    );
    assert_eq!(
        readiness_challenge["marker_sha256"].as_str().unwrap().len(),
        64,
        "{created}"
    );
    assert!(readiness_challenge["dispatched_ms"].is_i64(), "{created}");
    assert!(
        !created.to_string().contains("commandExecution"),
        "the supported producer exposes no commandExecution item: {created}"
    );
    let identity: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .host_state
                .join("identities/a6-ordinary/identity.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(identity["runtime"]["native_thread_id"], "created-thread-1");
    assert_eq!(
        identity["runtime"]["appserver_id"],
        master["runtime"]["appserver_id"]
    );
    assert_eq!(
        lifecycle_read(&fixture, &identity, None)["result"]["outcome"],
        "ok"
    );
    let replay = a6_create(&fixture, &master, "a6-ordinary", &fixture.root, "a6-create");
    assert_eq!(replay["result"], created["result"]);
    assert_eq!(
        lifecycle_query(&fixture, &master, "a6-create", "a6-capability")["result"],
        created["result"]
    );
    assert_eq!(host.start_request_count(), 1);
    assert_eq!(host.readiness_request_count(), 1);
    assert_eq!(
        a6_create(
            &fixture,
            &master,
            "different-peer",
            &fixture.root,
            "a6-create"
        )["result"]["outcome"],
        "refused"
    );
    assert_eq!(
        a6_create(
            &fixture,
            &master,
            "a6-ordinary",
            &fixture.root,
            "another-op"
        )["result"]["outcome"],
        "refused"
    );
    assert_eq!(host.start_request_count(), 1);
    let journal = fs::read_to_string(fixture.project_journal()).unwrap();
    let events: Vec<Value> = journal
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let records: Vec<&Value> = events
        .iter()
        .filter(|event| {
            event["ev"] == "PeerLifecycleOperationRecorded"
                && event["operation"]["operation_id"] == "a6-create"
        })
        .collect();
    assert_eq!(records[0]["operation"]["phase"], "intent_persisted");
    assert_eq!(records[1]["operation"]["phase"], "host_dispatch_claimed");
    assert!(
        !journal.contains("commandExecution"),
        "readiness proof must not depend on a fabricated commandExecution item"
    );
    let exact = events
        .iter()
        .position(|event| {
            event["ev"] == "PeerLifecycleOperationRecorded"
                && event["operation"]["create"]["thread_id"] == "created-thread-1"
        })
        .unwrap();
    let register = events
        .iter()
        .position(|event| event["ev"] == "Registered" && event["worker"]["id"] == "a6-ordinary")
        .unwrap();
    assert!(exact < register, "thread receipt precedes formal Register");
}

#[test]
fn peer_lifecycle_a6_concurrent_duplicate_consumes_one_send() {
    let (fixture, host, master) = a6_fixture("a6-concurrent", "create_ready");
    let responses = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            a6_create(
                &fixture,
                &master,
                "a6-concurrent-peer",
                &fixture.root,
                "a6-concurrent-op",
            )
        });
        let second = scope.spawn(|| {
            a6_create(
                &fixture,
                &master,
                "a6-concurrent-peer",
                &fixture.root,
                "a6-concurrent-op",
            )
        });
        (first.join().unwrap(), second.join().unwrap())
    });
    assert!(
        responses.0["result"]["outcome"] == "complete"
            || responses.1["result"]["outcome"] == "complete",
        "{responses:?}"
    );
    assert_eq!(host.start_request_count(), 1);
}

#[test]
fn peer_lifecycle_a6_response_loss_restart_reservation_and_no_resend() {
    let (mut fixture, host, master) = a6_fixture("a6-loss", "create_response_loss");
    let first = a6_create(
        &fixture,
        &master,
        "a6-lost-peer",
        &fixture.root,
        "a6-loss-op",
    );
    assert_eq!(first["result"]["outcome"], "unknown", "{first}");
    assert!(first["result"]["create"].get("thread_id").is_none());
    fixture.restart_daemon();
    assert_eq!(
        lifecycle_query(&fixture, &master, "a6-loss-op", "a6-capability")["result"],
        first["result"]
    );
    assert_eq!(
        a6_create(
            &fixture,
            &master,
            "a6-lost-peer",
            &fixture.root,
            "a6-loss-op"
        )["result"],
        first["result"]
    );
    assert_eq!(
        a6_create(
            &fixture,
            &master,
            "a6-lost-peer",
            &fixture.root,
            "a6-new-op"
        )["result"]["outcome"],
        "refused"
    );
    assert_eq!(host.start_request_count(), 1);
}

#[test]
fn peer_lifecycle_a6_claim_only_restart_is_unknown_and_never_resends() {
    let (mut fixture, host, master) = a6_fixture("a6-claim", "create_response_loss");
    a6_create(
        &fixture,
        &master,
        "a6-claimed-peer",
        &fixture.root,
        "a6-claim-op",
    );
    fixture.stop_daemon();
    // Crash boundary: retain journal through the real durable claim, before host outcome.
    let journal = fs::read_to_string(fixture.project_journal()).unwrap();
    let mut prefix = String::new();
    for line in journal.lines() {
        prefix.push_str(line);
        prefix.push('\n');
        let event: Value = serde_json::from_str(line).unwrap();
        if event["operation"]["operation_id"] == "a6-claim-op"
            && event["operation"]["phase"] == "host_dispatch_claimed"
        {
            break;
        }
    }
    fs::write(fixture.project_journal(), prefix).unwrap();
    fixture.restart_daemon();
    let query = lifecycle_query(&fixture, &master, "a6-claim-op", "a6-capability");
    assert_eq!(query["result"]["outcome"], "unknown", "{query}");
    assert_eq!(query["result"]["phase"], "host_dispatch_claimed");
    assert_eq!(
        a6_create(
            &fixture,
            &master,
            "a6-claimed-peer",
            &fixture.root,
            "a6-claim-op"
        )["result"],
        query["result"]
    );
    assert_eq!(host.start_request_count(), 1);
}

#[test]
fn peer_lifecycle_a6_known_thread_registration_failure_retains_partial_and_cleanup_truth() {
    let (mut fixture, host, master) = a6_fixture("a6-register-failure", "create_register_failure");
    let failed = a6_create(
        &fixture,
        &master,
        "a6-partial-peer",
        &fixture.root,
        "a6-partial-op",
    );
    assert_eq!(failed["result"]["outcome"], "partial", "{failed}");
    assert_eq!(failed["result"]["create"]["thread_id"], "created-thread-1");
    assert!(failed["result"]["create"]["stages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|stage| stage == "cleanup_not_attempted_thread_retained"));
    fixture.restart_daemon();
    assert_eq!(
        lifecycle_query(&fixture, &master, "a6-partial-op", "a6-capability")["result"],
        failed["result"]
    );
    assert_eq!(host.start_request_count(), 1);
}

#[test]
fn peer_lifecycle_a6_ack_and_idle_are_insufficient_for_readiness() {
    let (fixture, host, master) = a6_fixture("a6-ack", "ack_only");
    let created = a6_create(&fixture, &master, "a6-ack-peer", &fixture.root, "a6-ack-op");
    assert_eq!(created["result"]["outcome"], "partial", "{created}");
    assert_eq!(created["result"]["phase"], "readback_pending", "{created}");
    assert_eq!(created["result"]["create"]["thread_id"], "created-thread-1");
    assert_eq!(
        created["result"]["create"]["readiness"]["state"], "pending",
        "{created}"
    );
    assert_eq!(
        created["result"]["create"]["readiness"]["challenge"]["turn_id"], "create-readiness-turn",
        "{created}"
    );
    assert!(
        created["error"]
            .as_str()
            .unwrap_or_default()
            .contains("READINESS_PENDING"),
        "{created}"
    );
    assert_eq!(host.start_request_count(), 1);
    assert_eq!(host.readiness_request_count(), 1);
    assert_eq!(
        a6_create(&fixture, &master, "a6-ack-peer", &fixture.root, "a6-ack-op")["result"],
        created["result"]
    );
    assert_eq!(host.start_request_count(), 1);
    assert_eq!(host.readiness_request_count(), 1);
}

// A delayed readiness turn returns pending inside the bounded first response,
// then the same-operation readback finalizes it without repeating thread/start
// or the readiness effect.
#[test]
fn peer_lifecycle_a6_delayed_readback_finalizes_without_resend() {
    let (fixture, host, master) = a6_fixture("a6-delayed", "create_ready_delayed");
    let first = a6_create(
        &fixture,
        &master,
        "a6-delayed-peer",
        &fixture.root,
        "a6-delayed-op",
    );
    assert_eq!(first["result"]["outcome"], "partial", "{first}");
    assert_eq!(first["result"]["phase"], "readback_pending", "{first}");
    assert_eq!(
        first["result"]["create"]["readiness"]["state"], "pending",
        "{first}"
    );
    assert_eq!(host.start_request_count(), 1);
    assert_eq!(host.readiness_request_count(), 1);

    let finalized = a6_create(
        &fixture,
        &master,
        "a6-delayed-peer",
        &fixture.root,
        "a6-delayed-op",
    );
    assert_eq!(finalized["result"]["outcome"], "complete", "{finalized}");
    assert_eq!(finalized["result"]["phase"], "complete", "{finalized}");
    assert_eq!(
        finalized["result"]["create"]["readiness"]["state"], "verified",
        "{finalized}"
    );
    assert_eq!(
        finalized["result"]["create"]["readiness"]["challenge"]["turn_id"], "create-readiness-turn",
        "{finalized}"
    );
    assert_eq!(
        finalized["result"]["target"]["transport"]["thread_id"], "created-thread-1",
        "{finalized}"
    );
    assert_eq!(
        lifecycle_query(&fixture, &master, "a6-delayed-op", "a6-capability")["result"],
        finalized["result"]
    );
    assert_eq!(host.start_request_count(), 1);
    assert_eq!(host.readiness_request_count(), 1);
}

// Wrong thread, turn, or marker evidence never finalizes; the operation stays
// pending and never resends the readiness effect.
#[test]
fn peer_lifecycle_a6_wrong_challenge_correlation_cannot_finalize() {
    for effect in [
        "create_ready_wrong_marker",
        "create_ready_wrong_turn",
        "create_ready_wrong_thread",
    ] {
        let (fixture, host, master) = a6_fixture(&format!("a6-wrong-{effect}"), effect);
        let first = a6_create(
            &fixture,
            &master,
            "a6-wrong-peer",
            &fixture.root,
            "a6-wrong-op",
        );
        assert_eq!(first["result"]["outcome"], "partial", "{effect}: {first}");
        assert_eq!(
            first["result"]["phase"], "readback_pending",
            "{effect}: {first}"
        );
        assert_eq!(
            first["result"]["create"]["readiness"]["state"], "pending",
            "{effect}: {first}"
        );
        let replay = a6_create(
            &fixture,
            &master,
            "a6-wrong-peer",
            &fixture.root,
            "a6-wrong-op",
        );
        assert_eq!(replay["result"]["outcome"], "partial", "{effect}: {replay}");
        assert_ne!(
            replay["result"]["create"]["readiness"]["state"], "verified",
            "{effect}: {replay}"
        );
        assert_eq!(host.start_request_count(), 1, "{effect}");
        assert_eq!(host.readiness_request_count(), 1, "{effect}");
    }
}

// A changed binding generation between dispatch and readback cannot finalize
// the frozen operation.
#[test]
fn peer_lifecycle_a6_changed_generation_cannot_finalize() {
    let (mut fixture, host, master) = a6_fixture("a6-generation", "create_ready_delayed");
    let first = a6_create(
        &fixture,
        &master,
        "a6-generation-peer",
        &fixture.root,
        "a6-generation-op",
    );
    assert_eq!(first["result"]["outcome"], "partial", "{first}");
    let mut binding = read_lines(&fixture.project_journal())
        .into_iter()
        .rev()
        .find(|record| {
            record["ev"] == "GlobalRuntimeBound"
                && record["binding"]["agent_id"] == "a6-generation-peer"
        })
        .expect("the created peer's committed runtime binding");
    let generation = binding["binding"]["endpoint_generation"]
        .as_u64()
        .expect("binding generation is numeric");
    binding["binding"]["endpoint_generation"] = json!(generation + 1);
    lifecycle_replay_events(&mut fixture, &master, &[binding]);

    let replay = a6_create(
        &fixture,
        &master,
        "a6-generation-peer",
        &fixture.root,
        "a6-generation-op",
    );
    assert_eq!(replay["result"]["outcome"], "partial", "{replay}");
    assert_eq!(replay["result"]["phase"], "readback_pending", "{replay}");
    assert_ne!(
        replay["result"]["create"]["readiness"]["state"], "verified",
        "{replay}"
    );
    assert_eq!(host.start_request_count(), 1);
    assert_eq!(host.readiness_request_count(), 1);
}

#[test]
fn peer_lifecycle_a6_nonmaster_cross_main_and_app_have_no_host_effect() {
    let (fixture, host, identity) = lifecycle_native_fixture_effect("a6-nonmaster", "create_ready");
    assert_eq!(
        a6_create(
            &fixture,
            &identity,
            "a6-denied-peer",
            &fixture.root,
            "a6-denied"
        )["result"]["outcome"],
        "refused"
    );
    assert_eq!(host.start_request_count(), 0);
    let (fixture, host, master) = a6_fixture("a6-cross-main", "create_ready");
    git_init_main(&fixture.root);
    let other = lifecycle_sibling(&fixture.root, "other-main");
    fs::create_dir_all(&other).unwrap();
    git_init_main(&other);
    assert_eq!(
        a6_create(&fixture, &master, "a6-cross-peer", &other, "a6-cross-op")["result"]["outcome"],
        "refused"
    );
    let mut wrong_app = master.clone();
    wrong_app["runtime"]["appserver_id"] = json!("another-app");
    assert_eq!(
        a6_create(
            &fixture,
            &wrong_app,
            "a6-app-peer",
            &fixture.root,
            "a6-app-op"
        )["ok"],
        false
    );
    assert_eq!(host.start_request_count(), 0);
    fs::remove_dir_all(other).unwrap();
}

#[test]
fn peer_lifecycle_a6_separate_linked_worktree_same_registered_main_succeeds() {
    let (fixture, host, master) = a6_fixture("a6-linked", "create_ready");
    git_init_main(&fixture.root);
    let linked = lifecycle_sibling(&fixture.root, "execution-worktree");
    git_add_linked_worktree(&fixture.root, &linked, "a6-execution");
    let created = a6_create(&fixture, &master, "a6-linked-peer", &linked, "a6-linked-op");
    assert_eq!(created["result"]["outcome"], "complete", "{created}");
    assert_eq!(
        created["result"]["target"]["project_scope"],
        canonical_root(&fixture)
    );
    assert_eq!(
        created["result"]["create"]["cwd"],
        fs::canonicalize(&linked)
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        created["result"]["target"]["app_scope_id"],
        master["runtime"]["appserver_id"]
    );
    assert_eq!(host.start_request_count(), 1);
    git_run(
        &fixture.root,
        &["worktree", "remove", "--force", linked.to_str().unwrap()],
    );
}

fn lifecycle_wire(fixture: &Fixture, identity: &Value, request: Value) -> Value {
    let mut envelope = request;
    envelope["project_context"] = json!({
        "canonical_root": canonical_root(fixture), "project_scope": canonical_root(fixture),
        "app_scope_id": identity["runtime"]["appserver_id"], "runtime_context": identity["runtime"]
    });
    // The real daemon deserializes this public RequestEnvelope wire shape.
    let mut socket = UnixStream::connect(fixture.host_state.join("server.sock")).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    serde_json::to_writer(&mut socket, &envelope).unwrap();
    socket.write_all(b"\n").unwrap();
    let mut line = String::new();
    BufReader::new(socket).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn lifecycle_read(fixture: &Fixture, identity: &Value, target: Option<&str>) -> Value {
    lifecycle_wire(
        fixture,
        identity,
        json!({"op": "PeerLifecycle", "request": {
            "action": "read", "worker_id": identity["worker_id"], "token": identity["token"], "target_id": target
        }}),
    )
}

fn lifecycle_surfaces(fixture: &Fixture, identity: &Value) -> (Vec<u8>, Vec<u8>, Vec<u8>, Value) {
    let leases = lifecycle_wire(
        fixture,
        identity,
        json!({"op": "NotificationStatus", "worker_id": identity["worker_id"], "token": identity["token"]}),
    );
    (
        fs::read(fixture.host_journal()).unwrap_or_default(),
        fs::read(fixture.project_journal()).unwrap(),
        fs::read(fixture.host_routes()).unwrap(),
        leases,
    )
}

fn lifecycle_native_fixture(label: &str) -> (Fixture, TestAppServer, Value) {
    lifecycle_native_fixture_effect(label, "ack_only")
}

fn lifecycle_native_fixture_effect(label: &str, effect: &str) -> (Fixture, TestAppServer, Value) {
    let fixture = Fixture::without_tmux(label);
    let host = TestAppServer::start_with_effect(
        &fixture.root,
        "lifecycle-session",
        "lifecycle-thread",
        effect,
    );
    let initial = fixture.command(&["context", "--op", "lifecycle-seed"], None);
    assert_eq!(initial.status.code(), Some(2));
    let seed = fixture.context_provide(
        "lifecycle-seed",
        &json!({
            "session_id": "lifecycle-session", "thread_id": "lifecycle-thread",
            "endpoint": host.endpoint(), "namespace": "codex_tui"
        }),
        None,
    );
    assert_eq!(seed["result"]["outcome"], "completed", "{seed}");
    let identity = lifecycle_identity(&fixture, &seed);
    (fixture, host, identity)
}

fn lifecycle_native_pair_fixture(
    label: &str,
    child_effect: &str,
) -> (Fixture, TestAppServer, TestAppServer, Value, Value) {
    let fixture = Fixture::without_tmux(label);
    let master_host = TestAppServer::start_with_effect_at(
        &fixture.root,
        "lifecycle-master.sock",
        "lifecycle-master-session",
        "lifecycle-master-thread",
        "ack_only",
    );
    let master_seed = fixture.context_provide(
        "lifecycle-master-seed",
        &json!({
            "session_id": "lifecycle-master-session",
            "thread_id": "lifecycle-master-thread",
            "endpoint": master_host.endpoint(),
            "namespace": "codex_tui"
        }),
        None,
    );
    assert_eq!(
        master_seed["result"]["outcome"], "completed",
        "{master_seed}"
    );
    let master = lifecycle_identity(&fixture, &master_seed);
    let master_socket_path = master_host
        .endpoint()
        .strip_prefix("unix://")
        .expect("test AppServer endpoint uses unix://")
        .to_owned();
    let promoted_output = fixture
        .configured_command(
            &[
                "master",
                "promote",
                "--approval",
                "isolated lifecycle close fixture",
            ],
            None,
        )
        .env("CODEX_APP_SERVER_SOCKET", master_socket_path)
        .env("CODEX_SESSION_ID", "lifecycle-master-session")
        .env("CODEX_THREAD_ID", "lifecycle-master-thread")
        .output()
        .expect("run master promote with its live AppServer binding");
    assert!(
        promoted_output.status.success(),
        "master promote failed: stdout={} stderr={}",
        String::from_utf8_lossy(&promoted_output.stdout),
        String::from_utf8_lossy(&promoted_output.stderr)
    );
    let promoted: Value =
        serde_json::from_slice(&promoted_output.stdout).expect("master promote emits JSON");
    assert_eq!(promoted["role_brief"]["role"], "master", "{promoted}");
    assert_eq!(promoted["master"], master["worker_id"], "{promoted}");

    let child_host = TestAppServer::start_with_effect_at(
        &fixture.root,
        "lifecycle-child.sock",
        "lifecycle-child-session",
        "lifecycle-child-thread",
        child_effect,
    );
    let child_seed = fixture.context_provide(
        "lifecycle-child-seed",
        &json!({
            "session_id": "lifecycle-child-session",
            "thread_id": "lifecycle-child-thread",
            "endpoint": child_host.endpoint(),
            "namespace": "codex_tui"
        }),
        None,
    );
    assert_eq!(child_seed["result"]["outcome"], "completed", "{child_seed}");
    let child = lifecycle_identity(&fixture, &child_seed);
    (fixture, master_host, child_host, master, child)
}

#[test]
fn peer_lifecycle_c5_master_closes_exact_peer_and_preserves_master_route() {
    let (fixture, _master_host, child_host, master, child) =
        lifecycle_native_pair_fixture("peer-c5", "ack_only");
    let child_id = child["worker_id"].as_str().unwrap();
    let target = lifecycle_read(&fixture, &master, Some(child_id));
    assert_eq!(target["result"]["outcome"], "ok", "{target}");
    let target = target["result"]["target"].clone();

    let closed = lifecycle_close(
        &fixture,
        &master,
        "peer-c5-close",
        "peer-c5-capability",
        &target,
        "close isolated idle peer",
    );
    assert_eq!(closed["result"]["outcome"], "complete", "{closed}");
    assert_eq!(
        closed["result"]["close"]["runtime_archive"]["state"],
        "verified"
    );
    assert_eq!(
        closed["result"]["close"]["binding_retirement"]["state"],
        "verified"
    );
    assert_eq!(
        closed["result"]["close"]["route_retirement"]["state"],
        "verified"
    );
    assert_eq!(
        closed["result"]["close"]["subscription_retirement"]["state"],
        "verified"
    );
    assert_eq!(child_host.archive_request_count(), 1);
    assert_eq!(
        lifecycle_read(&fixture, &master, None)["result"]["outcome"],
        "ok"
    );
}

#[test]
fn peer_lifecycle_r1_authenticated_self_read_is_pure() {
    let (fixture, _host, identity) = lifecycle_native_fixture("peer-r1");
    let before = lifecycle_surfaces(&fixture, &identity);
    let read = lifecycle_read(&fixture, &identity, None);
    assert_eq!(read["result"]["outcome"], "ok", "{read}");
    assert_eq!(read["result"]["target"]["worker_id"], identity["worker_id"]);
    assert_eq!(
        read["result"]["target"]["binding_id"],
        identity["runtime"]["binding_id"]
    );
    assert_eq!(read["result"]["projection"]["presence"], "present");
    assert!(read["result"].get("operation_id").is_none());
    assert!(read["result"].get("phase").is_none());
    assert!(!read
        .to_string()
        .contains(identity["token"].as_str().unwrap()));
    let mut forged = identity.clone();
    forged["token"] = json!("wrong-token");
    let refused = lifecycle_read(&fixture, &forged, None);
    assert_eq!(refused["result"]["outcome"], "refused", "{refused}");
    assert_eq!(before, lifecycle_surfaces(&fixture, &identity));
    let invalid = lifecycle_wire(
        &fixture,
        &identity,
        json!({"op": "PeerLifecycle", "request": {
            "action": "read", "worker_id": identity["worker_id"], "token": identity["token"], "cwd": "/tmp"
        }}),
    );
    assert_eq!(invalid["ok"], false, "{invalid}");
}

#[test]
fn peer_lifecycle_r2_unknown_observation_is_pure() {
    let (fixture, host, identity) = lifecycle_native_fixture("peer-r2");
    drop(host);
    let before = lifecycle_surfaces(&fixture, &identity);
    let read = lifecycle_read(&fixture, &identity, None);
    assert_eq!(read["result"]["outcome"], "unknown", "{read}");
    assert_eq!(read["result"]["projection"]["presence"], "unknown");
    assert_eq!(before, lifecycle_surfaces(&fixture, &identity));
}

// Seed historical reducer events only while the isolated fixture is stopped.
// Requests and readbacks still cross the real daemon socket; no production
// hooks or alternate lifecycle implementation are involved.
fn lifecycle_replay_events(fixture: &mut Fixture, identity: &Value, events: &[Value]) {
    fixture.stop_daemon();
    let mut journal = fs::OpenOptions::new()
        .append(true)
        .open(fixture.project_journal())
        .unwrap();
    for event in events {
        serde_json::to_writer(&mut journal, event).unwrap();
        journal.write_all(b"\n").unwrap();
    }
    drop(journal);
    fixture.restart_daemon();
    let loaded = lifecycle_wire(fixture, identity, json!({"op": "Workers"}));
    assert_eq!(loaded["ok"], true, "{loaded}");
}

#[test]
fn peer_lifecycle_r3_legacy_close_never_proves_full_terminal() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("peer-r3");
    let seed = fixture.context(Some("peer-r3-parent"), Some(&pane));
    let parent = lifecycle_identity(&fixture, &seed);
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "isolated lifecycle fixture",
        ],
        Some(&pane),
    );
    let child_pane = second_pane(&fixture, "r3-child", pane.server_pid);
    let seed = fixture.context(Some("peer-r3-child"), Some(&child_pane));
    let child = lifecycle_identity(&fixture, &seed);
    let child_id = child["worker_id"].as_str().unwrap();
    lifecycle_replay_events(
        &mut fixture,
        &parent,
        &[json!({
            "ev": "WorkerClosed", "worker_id": child_id, "closed_by": parent["worker_id"], "reason": "legacy record-only fixture", "at_ms": 1
        })],
    );
    let before = lifecycle_surfaces(&fixture, &parent);
    let read = lifecycle_read(&fixture, &parent, Some(child_id));
    assert_eq!(read["result"]["outcome"], "unknown", "{read}");
    assert_eq!(
        read["result"]["close"]["close_outcome"],
        "closed_record_only"
    );
    assert_eq!(
        read["result"]["close"]["runtime_archive"]["state"],
        "unknown"
    );
    assert!(read["result"].get("target").is_none());
    assert_eq!(before, lifecycle_surfaces(&fixture, &parent));
    let fence = json!({"ev": "ResponsibilityFenceSet", "fence": {
        "operation_id": "legacy-exact-fence", "worker_id": child_id,
        "project_scope": canonical_root(&fixture), "app_scope": child["runtime"]["appserver_id"],
        "binding_id": child["runtime"]["binding_id"], "endpoint_generation": child["runtime"]["endpoint_generation"],
        "snapshot": {}, "state": "closing", "created_ms": 1
    }});
    lifecycle_replay_events(&mut fixture, &parent, &[fence]);
    let before = lifecycle_surfaces(&fixture, &parent);
    let read = lifecycle_read(&fixture, &parent, Some(child_id));
    assert_eq!(read["result"]["outcome"], "cleanup-open", "{read}");
    assert_eq!(before, lifecycle_surfaces(&fixture, &parent));
}

#[test]
fn peer_lifecycle_r4_managed_visibility_is_authorized_and_pure() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("peer-r4");
    let parent = lifecycle_identity(
        &fixture,
        &fixture.context(Some("peer-r4-parent"), Some(&pane)),
    );
    let child_pane = second_pane(&fixture, "r4-child", pane.server_pid);
    let child = lifecycle_identity(
        &fixture,
        &fixture.context(Some("peer-r4-child"), Some(&child_pane)),
    );
    let other_pane = second_pane(&fixture, "r4-other", pane.server_pid);
    let other = lifecycle_identity(
        &fixture,
        &fixture.context(Some("peer-r4-other"), Some(&other_pane)),
    );
    lifecycle_replay_events(
        &mut fixture,
        &parent,
        &[json!({"ev": "SubagentUpdated", "subagent": {
            "id": "managed-private", "parent": parent["worker_id"], "peer": child["worker_id"],
            "status": "ready", "thread_id": child["transport"]["thread_id"], "profile": null,
            "created_ms": 1, "ready_deadline_ms": 0, "last_message": "private-assignment", "error": null
        }})],
    );
    let before = lifecycle_surfaces(&fixture, &parent);
    let own = lifecycle_read(&fixture, &other, None);
    assert!(
        own["result"]["projection"].get("managed").is_none(),
        "{own}"
    );
    let refused = lifecycle_read(&fixture, &other, child["worker_id"].as_str());
    assert_eq!(refused["result"]["outcome"], "refused", "{refused}");
    assert!(refused["result"].get("projection").is_none());
    assert!(!refused.to_string().contains("managed-private"));
    let allowed = lifecycle_read(&fixture, &parent, child["worker_id"].as_str());
    assert_eq!(
        allowed["result"]["projection"]["managed"][0]["id"], "managed-private",
        "{allowed}"
    );
    assert!(!allowed.to_string().contains("private-assignment"));
    assert_eq!(before, lifecycle_surfaces(&fixture, &parent));
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "isolated current-master visibility fixture",
        ],
        Some(&other_pane),
    );
    let before = lifecycle_surfaces(&fixture, &other);
    let master_read = lifecycle_read(&fixture, &other, child["worker_id"].as_str());
    assert_eq!(
        master_read["result"]["projection"]["managed"][0]["id"], "managed-private",
        "{master_read}"
    );
    assert!(!master_read.to_string().contains("private-assignment"));
    assert_eq!(before, lifecycle_surfaces(&fixture, &other));
}

fn lifecycle_target(fixture: &Fixture, identity: &Value) -> Value {
    let read = lifecycle_read(fixture, identity, None);
    assert_eq!(read["result"]["outcome"], "ok", "{read}");
    read["result"]["target"].clone()
}

fn lifecycle_update(
    fixture: &Fixture,
    identity: &Value,
    operation_id: &str,
    capability: &str,
    target: &Value,
    cwd: &str,
) -> Value {
    lifecycle_wire(
        fixture,
        identity,
        json!({"op": "PeerLifecycle", "request": {
            "action": "update",
            "worker_id": identity["worker_id"],
            "token": identity["token"],
            "operation_id": operation_id,
            "query_capability": capability,
            "target": target,
            "cwd": cwd
        }}),
    )
}

fn lifecycle_close(
    fixture: &Fixture,
    identity: &Value,
    operation_id: &str,
    capability: &str,
    target: &Value,
    reason: &str,
) -> Value {
    lifecycle_wire(
        fixture,
        identity,
        json!({"op": "PeerLifecycle", "request": {
            "action": "close",
            "worker_id": identity["worker_id"],
            "token": identity["token"],
            "operation_id": operation_id,
            "query_capability": capability,
            "target": target,
            "reason": reason
        }}),
    )
}

fn lifecycle_query(
    fixture: &Fixture,
    identity: &Value,
    operation_id: &str,
    capability: &str,
) -> Value {
    lifecycle_wire(
        fixture,
        identity,
        json!({"op": "PeerLifecycle", "request": {
            "action": "query",
            "operation_id": operation_id,
            "query_capability": capability
        }}),
    )
}

fn lifecycle_sibling(root: &Path, suffix: &str) -> PathBuf {
    let name = root
        .file_name()
        .expect("fixture root has a file name")
        .to_string_lossy();
    root.with_file_name(format!("{name}-{suffix}"))
}

fn git_run(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git inside the isolated fixture");
    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_init_main(root: &Path) {
    git_run(root, &["init", "-q", "-b", "main"]);
    git_run(
        root,
        &[
            "-c",
            "user.name=Collab Test",
            "-c",
            "user.email=collab-test@example.invalid",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "initial",
        ],
    );
}

fn git_add_linked_worktree(main: &Path, worktree: &Path, branch: &str) {
    git_run(
        main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            branch,
            worktree.to_str().expect("worktree path is UTF-8"),
            "main",
        ],
    );
}

// U1: the cwd intent is durable before any host effect and the operation query
// returns the same projection after a daemon restart.

include!("context_operation_public_contract/part_a.rs");
include!("context_operation_public_contract/part_b.rs");
include!("context_operation_public_contract/part_c.rs");
include!("context_operation_public_contract/part_d.rs");
