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
#[test]
fn peer_lifecycle_u1_update_persists_durable_intent_and_query_survives_restart() {
    let (mut fixture, host, identity) = lifecycle_native_fixture("peer-u1");
    let target = lifecycle_target(&fixture, &identity);
    let cwd = canonical_root(&fixture);
    let operation_id = "peer-u1-update";
    let capability = "peer-u1-capability";

    let update = lifecycle_update(&fixture, &identity, operation_id, capability, &target, &cwd);
    assert_eq!(update["ok"], false, "{update}");
    assert_eq!(update["result"]["action"], "update", "{update}");
    assert_eq!(update["result"]["outcome"], "partial", "{update}");
    assert_eq!(update["result"]["operation_id"], operation_id, "{update}");
    assert_eq!(update["result"]["phase"], "readback_pending", "{update}");
    assert_eq!(
        update["result"]["source"], "lifecycle_operation",
        "{update}"
    );
    let readback = &update["result"]["update"];
    assert_eq!(readback["intended_cwd"], cwd, "{update}");
    assert_eq!(readback["settings"]["state"], "acknowledged", "{update}");
    assert_eq!(readback["effective_cwd"]["state"], "unproven", "{update}");
    assert!(readback["effective_cwd"].get("cwd").is_none(), "{update}");
    assert_eq!(
        readback["challenge"]["turn_id"], "update-challenge-turn",
        "{update}"
    );
    assert_eq!(readback["challenge"]["state"], "pending", "{update}");
    assert!(
        update["error"]
            .as_str()
            .unwrap_or_default()
            .contains("EFFECTIVE_CWD_PENDING"),
        "{update}"
    );
    assert_eq!(host.settings_request_count(), 1);
    assert_eq!(host.challenge_request_count(), 1);
    assert!(!update.to_string().contains(capability), "{update}");

    let query = lifecycle_query(&fixture, &identity, operation_id, capability);
    assert_eq!(query["result"]["action"], "update", "{query}");
    assert_eq!(query["result"]["outcome"], "partial", "{query}");
    assert_eq!(query["result"]["phase"], "readback_pending", "{query}");
    assert_eq!(query["result"]["operation_id"], operation_id, "{query}");
    assert_eq!(query["result"]["update"]["intended_cwd"], cwd, "{query}");
    assert!(!query.to_string().contains(capability), "{query}");

    let denied = lifecycle_query(
        &fixture,
        &identity,
        operation_id,
        "peer-u1-wrong-capability",
    );
    assert_eq!(denied["ok"], false, "{denied}");
    assert_eq!(denied["result"]["outcome"], "refused", "{denied}");
    assert!(
        denied["error"]
            .as_str()
            .unwrap_or_default()
            .contains("QUERY_DENIED"),
        "{denied}"
    );

    fixture.restart_daemon();
    let after = lifecycle_query(&fixture, &identity, operation_id, capability);
    assert_eq!(after["result"]["outcome"], "partial", "{after}");
    assert_eq!(after["result"]["phase"], "readback_pending", "{after}");
    assert_eq!(after["result"]["operation_id"], operation_id, "{after}");
    assert_eq!(after["result"]["update"]["intended_cwd"], cwd, "{after}");
    assert_eq!(
        after["result"]["update"]["effective_cwd"]["state"], "unproven",
        "{after}"
    );
}

// U2: only a later real completed turn proves the effective cwd; a settings ACK
// alone never becomes verified.
#[test]
fn peer_lifecycle_u2_later_completed_turn_proves_effective_cwd() {
    let (fixture, host, identity) = lifecycle_native_fixture_effect("peer-u2", "completed_turn");
    let target = lifecycle_target(&fixture, &identity);
    let cwd = canonical_root(&fixture);

    let update = lifecycle_update(
        &fixture,
        &identity,
        "peer-u2-update",
        "peer-u2-capability",
        &target,
        &cwd,
    );
    assert_eq!(update["result"]["outcome"], "complete", "{update}");
    assert_eq!(update["result"]["phase"], "complete", "{update}");
    let readback = &update["result"]["update"];
    assert_eq!(readback["intended_cwd"], cwd, "{update}");
    assert_eq!(readback["settings"]["state"], "acknowledged", "{update}");
    assert_eq!(readback["effective_cwd"]["state"], "verified", "{update}");
    assert_eq!(readback["effective_cwd"]["cwd"], cwd, "{update}");
    assert_eq!(
        readback["effective_cwd"]["thread_id"], "lifecycle-thread",
        "{update}"
    );
    assert_eq!(
        readback["effective_cwd"]["turn_id"], "update-challenge-turn",
        "{update}"
    );
    assert_eq!(readback["challenge"]["state"], "verified", "{update}");
    assert_eq!(
        readback["challenge"]["turn_id"], "update-challenge-turn",
        "{update}"
    );
    assert_eq!(host.settings_request_count(), 1);
    assert_eq!(host.challenge_request_count(), 1);
}

// U2-negative: quoted, failed, prefix, stale same-second, and wrong-thread
// evidence never proves the effective cwd and never repeats the effects.
#[test]
fn peer_lifecycle_u2_negative_execution_evidence_never_finalizes() {
    for effect in [
        "update_challenge_quoted",
        "update_challenge_failed",
        "update_challenge_prefix",
        "update_challenge_stale",
        "update_challenge_wrong_thread",
    ] {
        let (fixture, host, identity) =
            lifecycle_native_fixture_effect(&format!("peer-u2-neg-{effect}"), effect);
        let target = lifecycle_target(&fixture, &identity);
        let cwd = canonical_root(&fixture);
        let operation_id = format!("peer-u2-neg-{effect}-update");
        let capability = format!("peer-u2-neg-{effect}-capability");
        let update = lifecycle_update(
            &fixture,
            &identity,
            &operation_id,
            &capability,
            &target,
            &cwd,
        );
        assert_eq!(update["result"]["outcome"], "partial", "{effect}: {update}");
        assert_eq!(
            update["result"]["phase"], "readback_pending",
            "{effect}: {update}"
        );
        assert_eq!(
            update["result"]["update"]["effective_cwd"]["state"], "unproven",
            "{effect}: {update}"
        );
        assert_eq!(host.settings_request_count(), 1, "{effect}");
        assert_eq!(host.challenge_request_count(), 1, "{effect}");
        let replay = lifecycle_update(
            &fixture,
            &identity,
            &operation_id,
            &capability,
            &target,
            &cwd,
        );
        assert_eq!(replay["result"]["outcome"], "partial", "{effect}: {replay}");
        assert_ne!(
            replay["result"]["update"]["effective_cwd"]["state"], "verified",
            "{effect}: {replay}"
        );
        assert_eq!(host.settings_request_count(), 1, "{effect}");
        assert_eq!(host.challenge_request_count(), 1, "{effect}");
    }
}

// U2-delayed: the initial response is pending; a later same-operation readback
// finalizes the already-dispatched challenge without repeating settings or the
// challenge turn.
#[test]
fn peer_lifecycle_u2_delayed_readback_finalizes_without_resend() {
    let (fixture, host, identity) =
        lifecycle_native_fixture_effect("peer-u2-delayed", "update_challenge_delayed");
    let target = lifecycle_target(&fixture, &identity);
    let cwd = canonical_root(&fixture);
    let operation_id = "peer-u2-delayed-update";
    let capability = "peer-u2-delayed-capability";

    let first = lifecycle_update(&fixture, &identity, operation_id, capability, &target, &cwd);
    assert_eq!(first["result"]["outcome"], "partial", "{first}");
    assert_eq!(first["result"]["phase"], "readback_pending", "{first}");
    assert_eq!(
        first["result"]["update"]["effective_cwd"]["state"], "unproven",
        "{first}"
    );
    assert_eq!(
        first["result"]["update"]["challenge"]["state"], "pending",
        "{first}"
    );
    assert_eq!(host.settings_request_count(), 1);
    assert_eq!(host.challenge_request_count(), 1);

    let finalized = lifecycle_update(&fixture, &identity, operation_id, capability, &target, &cwd);
    assert_eq!(finalized["result"]["outcome"], "complete", "{finalized}");
    assert_eq!(finalized["result"]["phase"], "complete", "{finalized}");
    assert_eq!(
        finalized["result"]["update"]["effective_cwd"]["state"], "verified",
        "{finalized}"
    );
    assert_eq!(
        finalized["result"]["update"]["effective_cwd"]["turn_id"], "update-challenge-turn",
        "{finalized}"
    );
    assert_eq!(
        finalized["result"]["update"]["challenge"]["state"], "verified",
        "{finalized}"
    );
    assert_eq!(host.settings_request_count(), 1);
    assert_eq!(host.challenge_request_count(), 1);
    assert_eq!(
        lifecycle_query(&fixture, &identity, operation_id, capability)["result"],
        finalized["result"]
    );
}

// U3: replaying the same key returns the stored projection and never repeats the
// host settings effect.
#[test]
fn peer_lifecycle_u3_same_key_replay_does_not_repeat_host_effect() {
    let (fixture, host, identity) = lifecycle_native_fixture("peer-u3");
    let target = lifecycle_target(&fixture, &identity);
    let cwd = canonical_root(&fixture);
    let operation_id = "peer-u3-update";
    let capability = "peer-u3-capability";

    let first = lifecycle_update(&fixture, &identity, operation_id, capability, &target, &cwd);
    assert_eq!(first["result"]["outcome"], "partial", "{first}");
    assert_eq!(first["result"]["phase"], "readback_pending", "{first}");
    assert_eq!(
        host.settings_request_count(),
        1,
        "the first update performs one settings effect"
    );
    assert_eq!(host.challenge_request_count(), 1);

    let second = lifecycle_update(&fixture, &identity, operation_id, capability, &target, &cwd);
    assert_eq!(second["result"]["outcome"], "partial", "{second}");
    assert_eq!(second["result"]["operation_id"], operation_id, "{second}");
    assert_eq!(second["result"]["update"]["intended_cwd"], cwd, "{second}");
    assert_eq!(
        host.settings_request_count(),
        1,
        "a same-key replay must not repeat the settings effect"
    );
    assert_eq!(
        host.challenge_request_count(),
        1,
        "a same-key replay must not repeat the challenge effect"
    );
}

// U4: a same-key request with a changed intent is refused and leaves the stored
// operation untouched.
#[test]
fn peer_lifecycle_u4_changed_intent_is_refused() {
    let (fixture, host, identity) = lifecycle_native_fixture("peer-u4");
    let target = lifecycle_target(&fixture, &identity);
    let cwd = canonical_root(&fixture);
    let operation_id = "peer-u4-update";
    let capability = "peer-u4-capability";

    let first = lifecycle_update(&fixture, &identity, operation_id, capability, &target, &cwd);
    assert_eq!(first["result"]["outcome"], "partial", "{first}");
    assert_eq!(host.settings_request_count(), 1);

    let alternative = fixture.root.join("peer-u4-alt");
    fs::create_dir_all(&alternative).unwrap();
    let alternative = fs::canonicalize(&alternative)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let changed = lifecycle_update(
        &fixture,
        &identity,
        operation_id,
        capability,
        &target,
        &alternative,
    );
    assert_eq!(changed["ok"], false, "{changed}");
    assert_eq!(changed["result"]["outcome"], "refused", "{changed}");
    assert!(
        changed["error"]
            .as_str()
            .unwrap_or_default()
            .contains("INTENT_CONFLICT"),
        "{changed}"
    );
    assert_eq!(
        host.settings_request_count(),
        1,
        "a changed intent must not cause a second host effect"
    );
    let query = lifecycle_query(&fixture, &identity, operation_id, capability);
    assert_eq!(query["result"]["update"]["intended_cwd"], cwd, "{query}");
}

// U5: immutable target identity fields cannot be replaced; a forged generation
// is refused before any host effect and creates no operation.
#[test]
fn peer_lifecycle_u5_immutable_target_fields_refuse_without_effect() {
    let (fixture, host, identity) = lifecycle_native_fixture("peer-u5");
    let target = lifecycle_target(&fixture, &identity);
    let cwd = canonical_root(&fixture);
    let mut tampered = target.clone();
    let generation = tampered["endpoint_generation"].as_u64().unwrap();
    tampered["endpoint_generation"] = json!(generation + 1);

    let refused = lifecycle_update(
        &fixture,
        &identity,
        "peer-u5-update",
        "peer-u5-capability",
        &tampered,
        &cwd,
    );
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["result"]["outcome"], "refused", "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap_or_default()
            .contains("TARGET_MISMATCH"),
        "{refused}"
    );
    assert_eq!(
        host.settings_request_count(),
        0,
        "a forged immutable field must not reach the host"
    );
    let query = lifecycle_query(&fixture, &identity, "peer-u5-update", "peer-u5-capability");
    assert_eq!(query["result"]["outcome"], "unknown", "{query}");
    assert!(
        query["error"]
            .as_str()
            .unwrap_or_default()
            .contains("OPERATION_UNKNOWN"),
        "{query}"
    );
}

// U6: an unknown host outcome stays unknown/truthful and is queryable.
#[test]
fn peer_lifecycle_u6_unknown_host_effect_is_truthful() {
    let (fixture, _host, identity) = lifecycle_native_fixture_effect("peer-u6", "unknown");
    let target = lifecycle_target(&fixture, &identity);
    let cwd = canonical_root(&fixture);
    let operation_id = "peer-u6-update";
    let capability = "peer-u6-capability";

    let update = lifecycle_update(&fixture, &identity, operation_id, capability, &target, &cwd);
    assert_eq!(update["ok"], false, "{update}");
    assert_eq!(update["result"]["outcome"], "unknown", "{update}");
    assert_eq!(update["result"]["phase"], "unknown", "{update}");
    assert_eq!(
        update["result"]["update"]["settings"]["state"], "unknown",
        "{update}"
    );
    assert_eq!(
        update["result"]["update"]["effective_cwd"]["state"], "unproven",
        "{update}"
    );
    let query = lifecycle_query(&fixture, &identity, operation_id, capability);
    assert_eq!(query["result"]["outcome"], "unknown", "{query}");
    assert_eq!(query["result"]["phase"], "unknown", "{query}");
    assert_eq!(
        query["result"]["update"]["settings"]["state"], "unknown",
        "{query}"
    );
}

// U7: a peer executing from an external linked worktree that resolves to the
// same registered main/app route may update its cwd with real execution proof.
#[test]
fn peer_lifecycle_u7_linked_worktree_resolves_to_same_main_and_updates() {
    let fixture = Fixture::without_tmux("peer-u7");
    let host = TestAppServer::start_with_effect(
        &fixture.root,
        "lifecycle-session",
        "lifecycle-thread",
        "completed_turn",
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
    let target = lifecycle_target(&fixture, &identity);

    let worktree = lifecycle_sibling(&fixture.root, "linked");
    git_init_main(&fixture.root);
    git_add_linked_worktree(&fixture.root, &worktree, "peer-u7-linked");
    let worktree_cwd = fs::canonicalize(&worktree)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_ne!(worktree_cwd, canonical_root(&fixture));

    let update = lifecycle_update(
        &fixture,
        &identity,
        "peer-u7-update",
        "peer-u7-capability",
        &target,
        &worktree_cwd,
    );
    assert_eq!(update["result"]["outcome"], "complete", "{update}");
    assert_eq!(
        update["result"]["update"]["intended_cwd"], worktree_cwd,
        "{update}"
    );
    assert_eq!(
        update["result"]["update"]["effective_cwd"]["state"], "verified",
        "{update}"
    );
    assert_eq!(
        update["result"]["update"]["effective_cwd"]["cwd"], worktree_cwd,
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
    assert_eq!(host.settings_request_count(), 1);
    assert_eq!(host.challenge_request_count(), 1);
}

// U8a: a cwd that resolves to a different Git main cannot be tied to the bound
// peer's project/app scope, so the update is refused with no host effect.
#[test]
fn peer_lifecycle_u8_cross_main_update_is_refused_without_effect() {
    let (fixture, host, identity) = lifecycle_native_fixture("peer-u8-scope");
    let target = lifecycle_target(&fixture, &identity);

    let other = lifecycle_sibling(&fixture.root, "other");
    fs::create_dir_all(&other).unwrap();
    git_init_main(&other);
    let other_cwd = fs::canonicalize(&other)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_ne!(other_cwd, canonical_root(&fixture));

    let refused = lifecycle_update(
        &fixture,
        &identity,
        "peer-u8-scope-update",
        "peer-u8-capability",
        &target,
        &other_cwd,
    );
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["result"]["outcome"], "refused", "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap_or_default()
            .contains("OUT_OF_SCOPE"),
        "{refused}"
    );
    assert_eq!(
        host.settings_request_count(),
        0,
        "a cross-main update must not reach the host"
    );
    let query = lifecycle_query(
        &fixture,
        &identity,
        "peer-u8-scope-update",
        "peer-u8-capability",
    );
    assert_eq!(query["result"]["outcome"], "unknown", "{query}");
}

// U8b: an unfinished task owned by the peer blocks the update; the typed result
// names the conflicting responsibility and no host effect occurs.
#[test]
fn peer_lifecycle_u8_responsibility_conflict_is_refused_without_effect() {
    let (mut fixture, host, identity) = lifecycle_native_fixture("peer-u8-resp");
    let worker_id = identity["worker_id"].as_str().unwrap().to_owned();
    lifecycle_replay_events(
        &mut fixture,
        &identity,
        &[json!({
            "ev": "TaskCreated",
            "task": {
                "id": "peer-u8-task",
                "owner": worker_id,
                "created_by": worker_id,
                "created_ms": 1,
                "updated_ms": 1
            }
        })],
    );
    let target = lifecycle_target(&fixture, &identity);
    let cwd = canonical_root(&fixture);
    let refused = lifecycle_update(
        &fixture,
        &identity,
        "peer-u8-resp-update",
        "peer-u8-capability",
        &target,
        &cwd,
    );
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(refused["result"]["outcome"], "refused", "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap_or_default()
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
            .any(|id| id == "peer-u8-task"),
        "{refused}"
    );
    assert_eq!(
        host.settings_request_count(),
        0,
        "a responsibility conflict must not reach the host"
    );
}

fn text(output: &std::process::Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json_stdout(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "expected typed JSON stdout (status {:?}): {error}; output={}",
            output.status.code(),
            text(output)
        )
    })
}

fn stable_surfaces(
    fixture: &Fixture,
    pane: &Pane,
    operation_id: &str,
) -> (Vec<Value>, Vec<u8>, [usize; 6], Value) {
    let notification_state = fixture.run_ok(&["notify", "status"], Some(pane));
    (
        operation_records(&fixture.host_journal(), operation_id),
        fs::read(fixture.host_routes()).unwrap_or_default(),
        project_register_effect_counts(&fixture.project_journal()),
        notification_state,
    )
}

fn second_pane(fixture: &Fixture, label: &str, server_pid: u32) -> Pane {
    let output = tmux(
        &fixture.tmux_socket,
        &["new-session", "-d", "-P", "-F", "#{pane_id}", "sleep 600"],
    );
    let pane_id = String::from_utf8(output.stdout)
        .expect("second pane id is UTF-8")
        .trim()
        .to_owned();
    Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: format!("session-{label}-{pane_id}"),
        thread_anchor: format!("thread-{label}-{pane_id}"),
    }
}

fn operation_records(path: &Path, operation_id: &str) -> Vec<Value> {
    journal_operations(path, operation_id)
}

fn project_register_effect_counts(path: &Path) -> [usize; 6] {
    let events = read_lines(path);
    [
        "CommandStarted",
        "CommandCompleted",
        "GlobalProjectRegistered",
        "GlobalRuntimeBound",
        "Registered",
        "GlobalCurrentThreadRouteSet",
    ]
    .map(|event| events.iter().filter(|record| record["ev"] == event).count())
}

fn operation_line_lengths(path: &Path, operation_id: &str) -> Vec<u64> {
    let bytes = fs::read(path).expect("read host operation journal");
    bytes
        .split_inclusive(|byte| *byte == b'\n')
        .filter_map(|line| {
            let record: Value = serde_json::from_slice(line).ok()?;
            (record["operation_id"] == operation_id).then_some(line.len() as u64)
        })
        .collect()
}

fn context_args(
    operation_id: Option<&str>,
    project: Option<&Path>,
    app_scope: Option<&str>,
    approve_identity: Option<&Value>,
    approve_grant: Option<&Value>,
    provide: Option<&Value>,
    query: bool,
) -> Vec<String> {
    let mut args = vec!["context".to_owned()];
    if let Some(operation_id) = operation_id {
        args.extend(["--op".to_owned(), operation_id.to_owned()]);
    }
    if let Some(project) = project {
        args.extend([
            "--project".to_owned(),
            project.to_string_lossy().into_owned(),
        ]);
    }
    if let Some(app_scope) = app_scope {
        args.extend(["--app-scope".to_owned(), app_scope.to_owned()]);
    }
    if let Some(approve_identity) = approve_identity {
        args.extend([
            "--approve-identity".to_owned(),
            serde_json::to_string(approve_identity).expect("serialize identity approval"),
        ]);
    }
    if let Some(approve_grant) = approve_grant {
        args.extend([
            "--approve-grant".to_owned(),
            serde_json::to_string(approve_grant).expect("serialize grant approval"),
        ]);
    }
    if let Some(provide) = provide {
        args.extend([
            "--provide".to_owned(),
            serde_json::to_string(provide).expect("serialize provided facts"),
        ]);
    }
    if query {
        args.push("--query".to_owned());
    }
    args
}

fn run_context_cli(
    fixture: &Fixture,
    pane: Option<&Pane>,
    operation_id: Option<&str>,
    project: Option<&Path>,
    app_scope: Option<&str>,
    approve_identity: Option<&Value>,
    approve_grant: Option<&Value>,
    provide: Option<&Value>,
    query: bool,
) -> std::process::Output {
    let args = context_args(
        operation_id,
        project,
        app_scope,
        approve_identity,
        approve_grant,
        provide,
        query,
    );
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    fixture.command(&refs, pane)
}

fn spawn_context_cli(
    fixture: &Fixture,
    pane: Option<&Pane>,
    operation_id: Option<&str>,
    project: Option<&Path>,
    app_scope: Option<&str>,
    approve_identity: Option<&Value>,
    approve_grant: Option<&Value>,
    provide: Option<&Value>,
    query: bool,
) -> std::process::Child {
    let args = context_args(
        operation_id,
        project,
        app_scope,
        approve_identity,
        approve_grant,
        provide,
        query,
    );
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    fixture.spawn_command(&refs, pane)
}

fn success_payload(output: &std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "expected successful public context: {}",
        text(output)
    );
    let payload = json_stdout(output);
    assert_eq!(payload["ok"], true, "{payload}");
    assert_eq!(payload["result"]["outcome"], "completed", "{payload}");
    payload
}

fn typed_failure(output: &std::process::Output) -> Value {
    let payload = json_stdout(output);
    assert_eq!(payload["ok"], false, "{payload}");
    payload
}

fn canonical_root(fixture: &Fixture) -> String {
    fs::canonicalize(&fixture.root)
        .expect("canonical fixture root")
        .to_string_lossy()
        .into_owned()
}

fn tmux_identity_facts(fixture: &Fixture, pane: &Pane) -> Value {
    let output = tmux(
        &fixture.tmux_socket,
        &[
            "display-message",
            "-p",
            "-t",
            &pane.pane_id,
            "#{session_id}\t#{pane_id}\t#{pane_pid}",
        ],
    );
    let line = String::from_utf8(output.stdout).expect("tmux pane facts are UTF-8");
    let mut fields = line.trim_end().split('\t');
    let tmux_session_id = fields.next().expect("tmux session id").to_owned();
    let observed_pane_id = fields.next().expect("tmux pane id");
    assert_eq!(observed_pane_id, pane.pane_id);
    let pane_pid = fields
        .next()
        .expect("tmux pane pid")
        .parse::<u64>()
        .expect("tmux pane pid is numeric");
    json!({
        "session_id": pane.session_anchor,
        "thread_id": pane.thread_anchor,
        // The fixture pins CODEX_INTERNAL_ORIGINATOR_OVERRIDE to "Codex TUI",
        // so the CLI identity fact collector always adds this namespace. The
        // approval digest binds every observed fact, including this one.
        "namespace": "codex_tui",
        "tmux": {
            "endpoint": {
                "socket_path": fixture.tmux_socket.to_string_lossy(),
                "server_pid": pane.server_pid,
                "tmux_session_id": tmux_session_id,
                "pane_id": pane.pane_id,
                "pane_pid": pane_pid,
                "codex_session_id": pane.session_anchor,
                "codex_thread_id": pane.thread_anchor
            },
            "cwd": canonical_root(fixture)
        }
    })
}

fn approval_digest(approval: &Value, facts: &Value) -> String {
    let mut approval = approval.clone();
    if let Some(object) = approval.as_object_mut() {
        object.remove("intent_digest");
        object.remove("decided_by");
        object.remove("approved_at_ms");
    }
    let normalized = json!({"approval": approval, "facts": facts});
    format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&normalized).expect("serialize approval digest"))
    )
}

#[derive(Clone)]
struct IdentityFence {
    worker_id: String,
    binding_id: String,
    endpoint_generation: u64,
    identity_path: PathBuf,
    stale_identity_bytes: Vec<u8>,
}

fn seed_stale_identity(
    fixture: &Fixture,
    pane: &Pane,
    operation_id: &str,
    with_grant: bool,
) -> IdentityFence {
    let seed = fixture.context(Some(operation_id), Some(pane));
    assert_eq!(seed["result"]["outcome"], "completed", "{seed}");
    if with_grant {
        fixture.run_ok(
            &[
                "master",
                "promote",
                "--approval",
                "fixture approved grant seed",
            ],
            Some(pane),
        );
    }
    let worker_id = seed["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("seed snapshot has worker id")
        .to_owned();
    let binding = &seed["result"]["snapshot"]["binding"];
    let binding_id = binding["binding_id"]
        .as_str()
        .expect("seed snapshot has binding id")
        .to_owned();
    let endpoint_generation = binding["endpoint_generation"]
        .as_u64()
        .expect("seed snapshot has endpoint generation");
    let identity_path = fixture
        .host_state
        .join("identities")
        .join(&worker_id)
        .join("identity.json");
    let mut identity: Value = serde_json::from_slice(
        &fs::read(&identity_path).expect("read isolated fixture identity cache"),
    )
    .expect("isolated fixture identity cache is JSON");
    let token = identity["token"]
        .as_str()
        .expect("isolated fixture identity has token")
        .to_owned();
    identity["token"] = json!(format!("stale-{token}"));
    let stale_identity_bytes =
        serde_json::to_vec_pretty(&identity).expect("serialize isolated fixture identity cache");
    fs::write(&identity_path, &stale_identity_bytes)
        .expect("write isolated fixture identity cache");
    IdentityFence {
        worker_id,
        binding_id,
        endpoint_generation,
        identity_path,
        stale_identity_bytes,
    }
}

fn identity_approval(fixture: &Fixture, pane: &Pane, fence: &IdentityFence, action: &str) -> Value {
    let mut approval = json!({
        "decision": "approved",
        "decided_by": "user",
        "target_identity": fence.worker_id,
        "project_scope": canonical_root(fixture),
        "app_scope_id": "appserver-cli",
        "action": action,
        "expected_incumbent": {
            "binding_id": fence.binding_id,
            "endpoint_generation": fence.endpoint_generation
        },
        "intent_digest": "sha256:placeholder",
        "approved_at_ms": 1770000000000_i64
    });
    approval["intent_digest"] = json!(approval_digest(
        &approval,
        &tmux_identity_facts(fixture, pane)
    ));
    approval
}

fn grant_approval(
    fixture: &Fixture,
    pane: &Pane,
    fence: &IdentityFence,
    expected_grant: &Value,
) -> Value {
    let mut approval = json!({
        "decision": "approved",
        "decided_by": "user",
        "target_identity": fence.worker_id,
        "project_scope": canonical_root(fixture),
        "app_scope_id": "appserver-cli",
        "action": "replace_master_grant",
        "expected_grant": expected_grant,
        "intent_digest": "sha256:placeholder",
        "approved_at_ms": 1770000000000_i64
    });
    approval["intent_digest"] = json!(approval_digest(
        &approval,
        &tmux_identity_facts(fixture, pane)
    ));
    approval
}

fn master_status(fixture: &Fixture, pane: &Pane) -> Value {
    let status = fixture.run_ok(&["master", "status"], Some(pane));
    status["master"].clone()
}

fn binding_event_count(path: &Path, binding_id: &str, endpoint_generation: u64) -> usize {
    read_lines(path)
        .into_iter()
        .filter(|record| {
            record["ev"] == "GlobalRuntimeBound"
                && record["binding"]["binding_id"] == binding_id
                && record["binding"]["endpoint_generation"] == json!(endpoint_generation)
        })
        .count()
}

fn grant_event_count(path: &Path, binding_id: &str) -> usize {
    read_lines(path)
        .into_iter()
        .filter(|record| {
            record["ev"] == "GlobalMasterGranted" && record["grant"]["binding_id"] == binding_id
        })
        .count()
}

fn grant_replacement_start_count(path: &Path, operation_id: &str) -> usize {
    read_lines(path)
        .into_iter()
        .filter(|record| {
            record["ev"] == "GlobalMasterGrantReplacementStarted"
                && record["intent"]["operation_id"] == operation_id
        })
        .count()
}

fn grant_replacement_completed_count(path: &Path, operation_id: &str) -> usize {
    read_lines(path)
        .into_iter()
        .filter(|record| {
            record["ev"] == "GlobalMasterGrantReplacementCompleted"
                && record["receipt"]["operation_id"] == operation_id
        })
        .count()
}

fn thread_route_event_count(path: &Path) -> usize {
    read_lines(path)
        .into_iter()
        .filter(|record| record["ev"] == "GlobalCurrentThreadRouteSet")
        .count()
}

#[test]
fn b01_automatic_context_persists_proof_before_effect_and_redacts_capability() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-b01");
    let result = fixture.context(None, Some(&pane));
    let operation_id = result["operation_id"]
        .as_str()
        .expect("automatic context returns its durable operation id");
    let body = &result["result"];
    assert_eq!(body["outcome"], "completed", "{result}");
    assert_eq!(body["snapshot"]["registered"], true, "{result}");
    assert_eq!(
        body["committed_phases"],
        json!([
            "nested_register",
            "route",
            "credential",
            "lease",
            "context_complete"
        ]),
        "{result}"
    );

    let proof = fixture.proof(operation_id);
    let capability = proof["query_capability"]
        .as_str()
        .expect("private query capability exists");
    let expected_hash = format!("sha256:{:x}", Sha256::digest(capability.as_bytes()));
    let host = operation_records(&fixture.host_journal(), operation_id);
    assert_eq!(host.len(), 5, "admission plus four transitions: {host:?}");
    assert_eq!(host[0]["phase"], "admitted");
    assert_eq!(host[0]["query_capability_hash"], expected_hash);
    assert!(
        fs::metadata(fixture.proof_path(operation_id))
            .expect("proof metadata")
            .len()
            > 0,
        "the CLI must persist a query proof before the daemon can admit the operation"
    );
    for (label, path) in [
        ("host journal", fixture.host_journal()),
        ("project journal", fixture.project_journal()),
        ("host log", fixture.host_log()),
        ("host events", fixture.host_events()),
    ] {
        if let Ok(bytes) = fs::read(path) {
            assert_no_secret_text(label, &String::from_utf8_lossy(&bytes), capability);
        }
    }
}

#[test]
fn b02_same_key_retry_is_read_only_and_changed_facts_conflict() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-b02");
    let operation_id = "ctxop-v23-b02-same-key";
    let first = fixture.context(Some(operation_id), Some(&pane));
    assert_eq!(first["result"]["outcome"], "completed", "{first}");
    let before = stable_surfaces(&fixture, &pane, operation_id);

    let retry = fixture.context(Some(operation_id), Some(&pane));
    assert_eq!(retry["result"]["outcome"], "completed", "{retry}");
    assert_eq!(stable_surfaces(&fixture, &pane, operation_id), before);

    let other = second_pane(&fixture, "b02", pane.server_pid);
    let conflict = fixture.command(&["context", "--op", operation_id], Some(&other));
    let conflict_text = text(&conflict);
    assert!(
        !conflict.status.success(),
        "changed facts must fail: {conflict_text}"
    );
    assert!(
        conflict_text.contains("IDENTITY_OPERATION_INTENT_CONFLICT")
            || conflict_text.contains("IDENTITY_FACT_CONFLICT"),
        "changed facts must produce a typed conflict: {conflict_text}"
    );
    assert_eq!(stable_surfaces(&fixture, &pane, operation_id), before);
}

#[test]
fn b03_discarded_response_recovers_same_nested_receipt_after_restart() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("collab-v23-b03");
    let operation_id = "ctxop-v23-b03-lost-response";
    let discarded = fixture.command(&["context", "--op", operation_id], Some(&pane));
    assert!(
        discarded.status.success(),
        "first public request failed: {}",
        text(&discarded)
    );
    drop(discarded); // Model a caller that committed the invocation but lost its response.

    let initial = operation_records(&fixture.host_journal(), operation_id);
    assert_eq!(initial.last().unwrap()["phase"], "completed", "{initial:?}");
    let nested = initial
        .iter()
        .find(|record| record["nested_command_id"].is_string())
        .expect("durable validating phase binds nested receipt IDs");
    let command_id = nested["nested_command_id"].as_str().unwrap().to_owned();
    let nested_operation_id = nested["nested_operation_id"].as_str().unwrap().to_owned();
    assert!(initial
        .iter()
        .filter(|row| row["nested_command_id"].is_string())
        .all(|row| {
            row["nested_command_id"] == command_id
                && row["nested_operation_id"] == nested_operation_id
        }));

    fixture.restart_daemon();
    let before = fs::read(fixture.host_journal()).unwrap();
    let queried = fixture.context_query(operation_id, Some(&pane));
    assert_eq!(
        queried["result"]["queried_operation"]["nested_command_id"],
        command_id
    );
    assert_eq!(
        queried["result"]["queried_operation"]["nested_operation_id"],
        nested_operation_id
    );
    assert_eq!(
        queried["result"]["queried_operation"]["nested_receipt"]["command_id"],
        command_id
    );
    assert_eq!(
        queried["result"]["queried_operation"]["nested_receipt"]["operation_id"],
        nested_operation_id
    );
    assert_eq!(fs::read(fixture.host_journal()).unwrap(), before);
}

#[test]
fn b04_restart_query_replays_every_durable_phase_without_mutation() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let cases = [
        ("admitted", "unknown", 1usize),
        ("validating", "unknown", 2),
        ("inner_dispatched", "unknown", 3),
        ("effect_observed", "partial", 4),
        ("completed", "completed", 5),
    ];
    for (target_phase, expected_outcome, allowed_records) in cases {
        let label = format!("collab-v23-b04-{target_phase}");
        let (mut fixture, seed_pane) = Fixture::isolated(&label);
        let seed = fixture.context(Some("ctxop-v23-b04-seed"), Some(&seed_pane));
        assert_eq!(seed["result"]["outcome"], "completed", "{seed}");
        let seed_worker = seed["result"]["snapshot"]["identity"]["worker_id"].clone();
        let seed_operation = operation_records(&fixture.host_journal(), "ctxop-v23-b04-seed");
        let seed_receipt = seed_operation.last().expect("completed seed operation");
        let seed_nested_command = seed_receipt["nested_command_id"].clone();
        let seed_nested_operation = seed_receipt["nested_operation_id"].clone();
        let project_effects_before_reuse =
            project_register_effect_counts(&fixture.project_journal());
        for index in 0..100 {
            let host_len = fs::metadata(fixture.host_journal()).unwrap().len();
            let project_len = fs::metadata(fixture.project_journal()).unwrap().len();
            if host_len > project_len + 16 * 1024 {
                break;
            }
            let operation_id = format!("ctxop-v23-b04-pad-{index:04}");
            let output = fixture.command(&["context", "--op", &operation_id], Some(&seed_pane));
            let typed = json_stdout(&output);
            assert_eq!(typed["result"]["outcome"], "completed", "{typed}");
            assert_eq!(
                typed["result"]["snapshot"]["identity"]["worker_id"], seed_worker,
                "padding contexts must reuse the registered identity: {typed}"
            );
            if index == 0 {
                let padding_records = operation_records(&fixture.host_journal(), &operation_id);
                assert_eq!(
                    padding_records.last().unwrap()["nested_command_id"],
                    seed_nested_command,
                    "same-peer reuse must bind the exact committed Register command"
                );
                assert_eq!(
                    padding_records.last().unwrap()["nested_operation_id"],
                    seed_nested_operation,
                    "same-peer reuse must bind the exact committed Register operation"
                );
                assert_eq!(
                    padding_records
                        .iter()
                        .filter(|record| record["phase"] == "inner_dispatched")
                        .count(),
                    1,
                    "receipt reuse promotes the new outer operation once"
                );
                assert_eq!(
                    padding_records.last().unwrap()["business_receipts"],
                    json!([
                        "nested_register",
                        "route",
                        "credential",
                        "lease",
                        "context_complete"
                    ]),
                    "the new outer operation records only read-back-proven owner receipts"
                );
            }
        }
        assert_eq!(
            project_register_effect_counts(&fixture.project_journal()),
            project_effects_before_reuse,
            "same-peer context reuse must not repeat Register, binding, or route effects"
        );
        let host_len = fs::metadata(fixture.host_journal()).unwrap().len();
        let project_len = fs::metadata(fixture.project_journal()).unwrap().len();
        assert!(
            host_len > project_len + 16 * 1024,
            "outer journal must exceed the project journal before RLIMIT_FSIZE: host={host_len} project={project_len}"
        );

        let calibration_id = "ctxop-v23-b04-calibrate";
        let calibration_pane = second_pane(&fixture, "calibration", seed_pane.server_pid);
        let calibrated = fixture.context(Some(calibration_id), Some(&calibration_pane));
        assert_eq!(calibrated["result"]["outcome"], "completed", "{calibrated}");
        let calibration_lengths = operation_line_lengths(&fixture.host_journal(), calibration_id);
        assert_eq!(calibration_lengths.len(), 5, "{calibration_lengths:?}");

        let target_pane = second_pane(&fixture, target_phase, seed_pane.server_pid);
        let target_id = "ctxop-v23-b04-target000";
        assert_eq!(calibration_id.len(), target_id.len());
        let journal_len = fs::metadata(fixture.host_journal()).unwrap().len();
        let file_limit = journal_len + calibration_lengths[..allowed_records].iter().sum::<u64>();
        let project_len = fs::metadata(fixture.project_journal()).unwrap().len();
        assert!(
            file_limit > project_len + 4 * 1024,
            "outer journal cap must leave room for a public project Register append: limit={file_limit} project={project_len}"
        );
        fixture.restart_daemon_with_file_limit(file_limit);

        let attempt = fixture.command(&["context", "--op", target_id], Some(&target_pane));
        let typed = json_stdout(&attempt);
        assert_eq!(
            typed["result"]["outcome"],
            expected_outcome,
            "fault boundary for {target_phase}: {}",
            text(&attempt)
        );
        assert_eq!(
            fs::metadata(fixture.host_journal()).unwrap().len(),
            file_limit,
            "the daemon must reach the calibrated OS host-journal boundary for {target_phase}"
        );
        let committed = operation_records(&fixture.host_journal(), target_id);
        assert_eq!(
            committed.len(),
            allowed_records,
            "{target_phase}: {committed:?}"
        );
        assert_eq!(committed.last().unwrap()["phase"], target_phase);

        fixture.restart_daemon();
        let before = (
            fs::read(fixture.host_journal()).unwrap(),
            fs::read(fixture.host_routes()).unwrap_or_default(),
            fs::read(fixture.project_journal()).unwrap(),
        );
        let query = fixture.context_query(target_id, Some(&target_pane));
        assert_eq!(
            query["result"]["queried_operation"]["phase"], target_phase,
            "replayed phase {target_phase}: {query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["outcome"], expected_outcome,
            "replayed outcome for {target_phase}: {query}"
        );
        let expected_committed: Vec<&str> = match target_phase {
            "admitted" => vec![],
            "validating" => vec!["validating"],
            "inner_dispatched" => vec!["validating", "inner_dispatched"],
            "effect_observed" => vec!["validating", "inner_dispatched", "effect_observed"],
            "completed" => vec![
                "validating",
                "inner_dispatched",
                "effect_observed",
                "completed",
            ],
            _ => unreachable!(),
        };
        assert_eq!(
            query["result"]["queried_operation"]["committed_phases"],
            json!(expected_committed)
        );
        if target_phase == "validating" {
            assert_eq!(
                query["result"]["queried_operation"]["nested_receipt"]["state"], "unavailable",
                "incomplete Register replay must stay queryable as degraded evidence: {query}"
            );
            assert!(
                query["result"]["queried_operation"]["nested_receipt"]["error"]
                    .as_str()
                    .is_some_and(|error| error.contains("PROJECT_OWNER_READBACK_UNAVAILABLE")),
                "degraded query preserves its owner replay failure: {query}"
            );
        } else if target_phase == "inner_dispatched" {
            assert_eq!(
                query["result"]["queried_operation"]["nested_receipt"]["command_id"],
                query["result"]["queried_operation"]["nested_command_id"],
                "replayed Register receipt is tied to the exact outer command ID: {query}"
            );
            assert_eq!(
                query["result"]["queried_operation"]["nested_receipt"]["operation_id"],
                query["result"]["queried_operation"]["nested_operation_id"],
                "replayed Register receipt is tied to the exact outer operation ID: {query}"
            );
        }
        assert_eq!(
            (
                fs::read(fixture.host_journal()).unwrap(),
                fs::read(fixture.host_routes()).unwrap_or_default(),
                fs::read(fixture.project_journal()).unwrap(),
            ),
            before,
            "operation query must not mutate either journal or the host route projection"
        );
        if target_phase == "validating" {
            fixture.stop_degraded_daemon();
        }
    }
}

#[test]
fn b05_query_remains_available_when_real_project_journal_is_unavailable() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("collab-v23-b05");
    let operation_id = "ctxop-v23-b05-degraded";
    let completed = fixture.context(Some(operation_id), Some(&pane));
    assert_eq!(completed["result"]["outcome"], "completed", "{completed}");
    let project_bytes = fs::read(fixture.project_journal()).expect("real journal exists");
    let host_before = fs::read(fixture.host_journal()).unwrap();

    fixture.stop_daemon();
    fixture.hide_project_journal();
    assert!(fixture.project_journal().is_dir());
    fixture.restart_daemon();
    let query = fixture.context_query(operation_id, Some(&pane));
    assert_eq!(query["result"]["outcome"], "completed", "{query}");
    assert_eq!(
        query["result"]["queried_operation"]["operation_id"],
        operation_id
    );
    assert_eq!(fs::read(fixture.host_journal()).unwrap(), host_before);
    assert!(fixture.project_journal().is_dir());

    fixture.stop_degraded_daemon();
    fixture.restore_project_journal();
    assert_eq!(fs::read(fixture.project_journal()).unwrap(), project_bytes);
}

#[test]
fn b06_mcp_query_uses_retained_capability_and_rejects_wrong_one_without_leaks() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-b06");
    let operation_id = "ctxop-v23-b06-capability";
    let completed = fixture.context(Some(operation_id), Some(&pane));
    assert_eq!(completed["result"]["outcome"], "completed", "{completed}");
    let proof = fixture.proof(operation_id);
    let capability = proof["query_capability"].as_str().unwrap().to_owned();
    let before = stable_surfaces(&fixture, &pane, operation_id);
    let operation_before = operation_records(&fixture.project_journal(), operation_id);

    let mut mcp = Mcp::start(&fixture, Some(&pane));
    let result = mcp.call_context(6, json!({"operation_id": operation_id, "query": true}));
    assert_eq!(
        result["isError"], false,
        "correct capability query failed: {result}"
    );
    let mcp_text = result["content"][0]["text"]
        .as_str()
        .expect("MCP query includes typed result text");
    let mcp_payload: Value = serde_json::from_str(mcp_text).expect("MCP typed result is JSON");
    assert_eq!(
        mcp_payload["result"]["queried_operation"]["operation_id"],
        operation_id
    );
    assert_no_secret_text("MCP result", mcp_text, &capability);
    mcp.finish();

    fixture.set_proof_capability(operation_id, "wrong-capability-test-value");
    let denied = fixture.command(&["context", "--op", operation_id, "--query"], Some(&pane));
    let denied_text = text(&denied);
    assert!(
        !denied.status.success(),
        "wrong capability must be denied: {denied_text}"
    );
    assert!(
        denied_text.contains("IDENTITY_OPERATION_QUERY_DENIED"),
        "wrong capability must produce the typed denial: {denied_text}"
    );
    assert!(!denied_text.contains("wrong-capability-test-value"));
    fixture.set_proof_capability(operation_id, &capability);
    let after = stable_surfaces(&fixture, &pane, operation_id);
    assert_eq!(
        after.0, before.0,
        "query changed the host operation journal"
    );
    assert_eq!(after.1, before.1, "query changed host route state");
    assert_eq!(
        operation_records(&fixture.project_journal(), operation_id),
        operation_before,
        "query changed this operation's durable project journal records"
    );
    assert_eq!(after.3, before.3, "query changed notification state");
    for path in [
        fixture.host_journal(),
        fixture.project_journal(),
        fixture.host_log(),
        fixture.host_events(),
    ] {
        if let Ok(bytes) = fs::read(path) {
            assert_no_secret_text(
                "persisted surface",
                &String::from_utf8_lossy(&bytes),
                &capability,
            );
        }
    }
}

#[test]
fn b07_missing_facts_are_exact_and_fresh_supplement_registers_through_public_cli() {
    let fixture = Fixture::without_tmux("collab-v23-b07");
    let operation_id = "ctxop-v23-b07-supplement";
    let session_id = format!("test-session-{}", std::process::id());
    let thread_id = format!("test-thread-{}", std::process::id());
    let appserver = TestAppServer::start(&fixture.root, &session_id, &thread_id);

    let missing_output = fixture.command(&["context", "--op", operation_id], None);
    assert_eq!(missing_output.status.code(), Some(2), "{missing_output:?}");
    let missing = json_stdout(&missing_output);
    assert_eq!(missing["result"]["outcome"], "missing_facts", "{missing}");
    assert_eq!(missing["result"]["requires"]["kind"], "identity_facts");
    assert_eq!(
        missing["result"]["requires"]["fields"],
        json!(["session_id", "thread_id", "endpoint"])
    );
    assert_eq!(
        missing["result"]["requires"]["sources"]["session_id"],
        "Current runtime session identifier"
    );
    assert_eq!(
        missing["result"]["requires"]["sources"]["thread_id"],
        "Current native thread identifier"
    );
    assert_eq!(
        missing["result"]["requires"]["sources"]["endpoint"],
        "Current native AppServer unix socket endpoint"
    );
    assert_eq!(
        missing["result"]["requires"]["repair_invocation"],
        "collab context --provide '<JSON containing required_fields>'"
    );
    assert!(operation_records(&fixture.host_journal(), operation_id).is_empty());
    assert!(operation_records(&fixture.project_journal(), operation_id).is_empty());

    let facts = json!({
        "session_id": session_id,
        "thread_id": thread_id,
        "endpoint": appserver.endpoint(),
        "namespace": "codex_tui"
    });
    let completed = fixture.context_provide(operation_id, &facts, None);
    assert_eq!(
        completed["result"]["invocation"], "supplement",
        "{completed}"
    );
    assert_eq!(completed["result"]["outcome"], "completed", "{completed}");
    assert_eq!(
        completed["result"]["snapshot"]["registered"], true,
        "{completed}"
    );
    let queried = fixture.context_query(operation_id, None);
    assert_eq!(
        queried["result"]["queried_operation"]["phase"], "completed",
        "{queried}"
    );
    drop(appserver);
}

#[test]
fn b08_os_append_failure_is_typed_and_never_projects_durable_completion() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("collab-v23-b08");
    let seed = fixture.context(Some("ctxop-v23-b08-seed"), Some(&pane));
    assert_eq!(seed["result"]["outcome"], "completed", "{seed}");
    let journal_path = fixture.host_journal();
    let valid_prefix = fs::metadata(&journal_path).unwrap().len();
    fixture.restart_daemon_with_file_limit(valid_prefix);

    let failed_id = "ctxop-v23-b08-failed";
    let failed = fixture.command(&["context", "--op", failed_id], Some(&pane));
    let failed_text = text(&failed);
    assert!(
        failed_text.contains("IDENTITY_OPERATION_DURABILITY_FAILED"),
        "the public request must surface the real append failure: {failed_text}"
    );
    assert!(operation_records(&journal_path, failed_id).is_empty());

    let query = fixture.command(&["context", "--op", failed_id, "--query"], Some(&pane));
    let query_text = text(&query);
    assert!(
        !query.status.success(),
        "unknown operation must not appear successful: {query_text}"
    );
    assert!(
        query_text.contains("IDENTITY_OPERATION_UNKNOWN"),
        "query must return the typed unknown state: {query_text}"
    );
    assert!(operation_records(&journal_path, failed_id).is_empty());
    assert!(
        wait_until(Duration::from_secs(2), || fixture
            .host_state
            .join("server.sock")
            .exists()),
        "fixture daemon remains the owner after append failure"
    );
}

#[test]
fn c03_unapproved_stale_recovery_requires_approval_without_mutation() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c03");
    let operation_id = "ctxop-v23-c03-unapproved";
    let fence = seed_stale_identity(&fixture, &pane, operation_id, false);
    let host_before = fs::read(fixture.host_journal()).expect("host journal before recovery");
    let routes_before = fs::read(fixture.host_routes()).unwrap_or_default();
    let project_before =
        fs::read(fixture.project_journal()).expect("project journal before recovery");
    let recovery_id = "ctxop-v23-c03-recovery";

    let automatic = run_context_cli(
        &fixture,
        Some(&pane),
        Some(recovery_id),
        None,
        None,
        None,
        None,
        None,
        false,
    );
    let automatic_payload = typed_failure(&automatic);
    let automatic_text = text(&automatic);
    assert!(
        automatic_text.contains("IDENTITY_APPROVAL_REQUIRED"),
        "unapproved stale recovery must be typed: {automatic_text}"
    );
    assert_eq!(
        automatic_payload["result"]["outcome"], "denied",
        "{automatic_payload}"
    );
    assert_eq!(
        automatic_payload["result"]["phase"],
        Value::Null,
        "approval preflight must not admit an operation: {automatic_payload}"
    );
    assert_eq!(
        automatic_payload["result"]["requires"]["kind"], "approval",
        "{automatic_payload}"
    );
    let approval = &automatic_payload["result"]["requires"]["approval"];
    assert_eq!(
        approval["target_identity"], fence.worker_id,
        "{automatic_payload}"
    );
    assert_eq!(
        approval["project_scope"],
        canonical_root(&fixture),
        "{automatic_payload}"
    );
    assert_eq!(
        approval["app_scope_id"], "appserver-cli",
        "{automatic_payload}"
    );
    assert_eq!(
        approval["expected_incumbent"]["binding_id"], fence.binding_id,
        "{automatic_payload}"
    );
    assert_eq!(
        approval["expected_incumbent"]["endpoint_generation"], fence.endpoint_generation,
        "{automatic_payload}"
    );
    let repair = automatic_payload["result"]["requires"]["repair_invocation"]
        .as_str()
        .expect("approval result must carry an executable repair invocation");
    assert!(repair.contains("--approve-identity"), "{automatic_payload}");
    let repair_identity = identity_approval(&fixture, &pane, &fence, "replace_binding");
    assert_eq!(
        approval["intent_digest"], repair_identity["intent_digest"],
        "the returned approval must be directly executable for this invocation: {automatic_payload}"
    );
    assert!(
        !automatic_text.contains("stale-"),
        "approval result must not leak the stale credential: {automatic_text}"
    );
    assert!(operation_records(&fixture.host_journal(), recovery_id).is_empty());
    assert_eq!(fs::read(fixture.host_journal()).unwrap(), host_before);
    assert_eq!(
        fs::read(fixture.host_routes()).unwrap_or_default(),
        routes_before
    );
    assert_eq!(fs::read(fixture.project_journal()).unwrap(), project_before);
    assert_eq!(
        fs::read(&fence.identity_path).expect("stale cache remains"),
        fence.stale_identity_bytes
    );
}

#[test]
fn c04_approved_identity_recovery_retires_exact_incumbent_without_grant_change() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c04");
    let operation_id = "ctxop-v23-c04-seed";
    let fence = seed_stale_identity(&fixture, &pane, operation_id, false);
    let recovery_id = "ctxop-v23-c04-recovery";
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");
    let grant_before = master_status(&fixture, &pane);
    let before_events = fs::read(fixture.host_journal()).unwrap();

    let recovered = run_context_cli(
        &fixture,
        Some(&pane),
        Some(recovery_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let payload = success_payload(&recovered);
    assert_eq!(
        payload["result"]["invocation"], "approved_recovery",
        "{payload}"
    );
    assert_eq!(
        payload["result"]["committed_phases"],
        json!([
            "approval_decision",
            "nested_register",
            "route",
            "credential",
            "lease",
            "context_complete"
        ]),
        "{payload}"
    );
    let new_binding = &payload["result"]["snapshot"]["binding"];
    let new_binding_id = new_binding["binding_id"]
        .as_str()
        .expect("approved recovery returns real binding id");
    let new_endpoint_generation = new_binding["endpoint_generation"]
        .as_u64()
        .expect("approved recovery returns an endpoint generation");
    assert_eq!(
        new_binding_id, fence.binding_id,
        "approved recovery must retain the stable binding id: {payload}"
    );
    assert!(
        new_endpoint_generation > fence.endpoint_generation,
        "{payload}"
    );
    assert_eq!(
        master_status(&fixture, &pane),
        grant_before,
        "identity recovery without grant approval must not create or replace master authority: {payload}"
    );
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            &fence.binding_id,
            fence.endpoint_generation
        ),
        1,
        "the exact old binding version must be retired once"
    );
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            new_binding_id,
            new_endpoint_generation
        ),
        1,
        "exactly one newer binding version must commit"
    );
    assert!(
        fs::read(&fixture.host_journal()).unwrap().len() > before_events.len(),
        "recovery writes the new durable binding"
    );
    assert_eq!(
        fs::read(&fence.identity_path).expect("daemon overwrites the fixture cache")
            != fence.stale_identity_bytes,
        true,
        "the stale local credential must be replaced by the daemon receipt"
    );
}

#[test]
fn c05_changed_incumbent_is_stale_conflict_before_any_owner_commit() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c05");
    let operation_id = "ctxop-v23-c05-seed";
    let fence = seed_stale_identity(&fixture, &pane, operation_id, false);
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");
    let recovery_id = "ctxop-v23-c05-recovery";

    let replacement_id = "ctxop-v23-c05-replacement";
    let replacement = run_context_cli(
        &fixture,
        Some(&pane),
        Some(replacement_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let replacement = success_payload(&replacement);
    assert_eq!(
        replacement["result"]["outcome"], "completed",
        "{replacement}"
    );
    let before = (
        fs::read(fixture.host_journal()).unwrap(),
        fs::read(fixture.host_routes()).unwrap_or_default(),
        fs::read(fixture.project_journal()).unwrap(),
    );
    let events_before = before.0.clone();
    let identity_before = fs::read(&fence.identity_path).expect("identity after replacement");
    let changed = run_context_cli(
        &fixture,
        Some(&pane),
        Some(recovery_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let changed_text = text(&changed);
    assert!(
        !changed.status.success(),
        "changed incumbent must be refused: {changed_text}"
    );
    assert!(
        changed_text.contains("APPROVAL_STALE_CONFLICT")
            || changed_text.contains("IDENTITY_APPROVAL_STALE_INCUMBENT"),
        "stale fence must be typed: {changed_text}"
    );
    let payload = typed_failure(&changed);
    assert_eq!(payload["result"]["outcome"], "denied", "{payload}");
    assert_eq!(fs::read(fixture.host_journal()).unwrap(), events_before);
    assert_eq!(
        (
            fs::read(fixture.host_journal()).unwrap(),
            fs::read(fixture.host_routes()).unwrap_or_default(),
            fs::read(fixture.project_journal()).unwrap(),
        ),
        before
    );
    assert_eq!(
        fs::read(&fence.identity_path).expect("stale cache remains"),
        identity_before
    );
}

#[test]
fn c06_grant_separation_requires_independent_grant_approval_and_preserves_grant_without_it() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c06");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c06-seed", true);
    let grant_before = master_status(&fixture, &pane);
    let grant_id = grant_before["grant_id"]
        .as_str()
        .expect("seed master status has grant id")
        .to_owned();
    let grant_generation = grant_before["grant_generation"]
        .as_u64()
        .expect("seed master status has grant generation");
    let recovery_id = "ctxop-v23-c06-identity-only";
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");
    let identity_only = run_context_cli(
        &fixture,
        Some(&pane),
        Some(recovery_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let payload = success_payload(&identity_only);
    assert_eq!(
        payload["result"]["invocation"], "approved_recovery",
        "{payload}"
    );
    let recovered_binding = &payload["result"]["snapshot"]["binding"];
    let recovered_binding_id = recovered_binding["binding_id"]
        .as_str()
        .expect("identity-only recovery returns a binding id");
    let recovered_endpoint_generation = recovered_binding["endpoint_generation"]
        .as_u64()
        .expect("identity-only recovery returns an endpoint generation");
    assert_eq!(
        recovered_binding_id, fence.binding_id,
        "identity-only recovery must retain the stable binding id: {payload}"
    );
    assert!(
        recovered_endpoint_generation > fence.endpoint_generation,
        "identity-only recovery must commit a greater endpoint generation: {payload}"
    );
    let grant_after_identity = master_status(&fixture, &pane);
    assert_eq!(
        grant_after_identity["grant_id"], grant_id,
        "identity approval alone must not replace the grant resource: {grant_after_identity}"
    );
    assert_eq!(
        grant_after_identity["grant_generation"],
        json!(grant_generation),
        "{grant_after_identity}"
    );
    assert_eq!(
        grant_after_identity["binding_id"], recovered_binding["binding_id"],
        "identity approval alone must advance only the endpoint fence: {grant_after_identity}"
    );
    assert_eq!(
        grant_after_identity["endpoint_generation"], recovered_binding["endpoint_generation"],
        "identity approval alone must advance only the endpoint fence: {grant_after_identity}"
    );
    let mut expected_identity_grant = grant_before.clone();
    expected_identity_grant["binding_id"] = recovered_binding["binding_id"].clone();
    expected_identity_grant["endpoint_generation"] =
        recovered_binding["endpoint_generation"].clone();
    assert_eq!(
        grant_after_identity, expected_identity_grant,
        "identity-only recovery must preserve principal, scope, authority, and approval metadata"
    );

    // The second invocation carries both approvals. The identity approval alone
    // never authorized the grant owner; only this distinct grant approval does.
    let current_fence = identity_fence_from_status(&fixture, &pane);
    let combined_identity = identity_approval(&fixture, &pane, &current_fence, "replace_binding");
    let current_grant = grant_fence(&grant_after_identity);
    let combined_grant = grant_approval(&fixture, &pane, &current_fence, &current_grant);
    let combined_id = "ctxop-v23-c06-combined";
    let combined = run_context_cli(
        &fixture,
        Some(&pane),
        Some(combined_id),
        None,
        None,
        Some(&combined_identity),
        Some(&combined_grant),
        None,
        false,
    );
    let combined_payload = success_payload(&combined);
    assert_eq!(
        combined_payload["result"]["invocation"], "approved_recovery",
        "{combined_payload}"
    );
    let combined_binding = &combined_payload["result"]["snapshot"]["binding"];
    let combined_binding_id = combined_binding["binding_id"]
        .as_str()
        .expect("combined recovery returns a binding id");
    let combined_endpoint_generation = combined_binding["endpoint_generation"]
        .as_u64()
        .expect("combined recovery returns an endpoint generation");
    assert_eq!(
        combined_binding_id, current_fence.binding_id,
        "combined recovery must retain the stable binding id: {combined_payload}"
    );
    assert!(
        combined_endpoint_generation > current_fence.endpoint_generation,
        "combined recovery must commit a greater endpoint generation: {combined_payload}"
    );
    let grant_after_replacement = master_status(&fixture, &pane);
    assert_eq!(
        grant_after_replacement["grant_id"], grant_id,
        "the separately approved replacement must retain the master grant resource: {grant_after_replacement}"
    );
    assert_eq!(
        grant_after_replacement["grant_generation"],
        json!(grant_generation + 1),
        "the independent grant approval must advance the grant generation exactly once: {grant_after_replacement}"
    );
    assert_eq!(
        grant_after_replacement["binding_id"], combined_binding["binding_id"],
        "the replacement must commit the recovered endpoint fence: {grant_after_replacement}"
    );
    assert_eq!(
        grant_after_replacement["endpoint_generation"], combined_binding["endpoint_generation"],
        "the replacement must commit the recovered endpoint fence: {grant_after_replacement}"
    );

    // Grant approval without the identity owner fence is still refused, and it
    // creates no outer operation record.
    let grant_only_fence = identity_fence_from_status(&fixture, &pane);
    let grant_only_grant = grant_fence(&grant_after_replacement);
    let grant_only_approval = grant_approval(&fixture, &pane, &grant_only_fence, &grant_only_grant);
    let grant_only_id = "ctxop-v23-c06-grant-only";
    let grant_only_output = run_context_cli(
        &fixture,
        Some(&pane),
        Some(grant_only_id),
        None,
        None,
        None,
        Some(&grant_only_approval),
        None,
        false,
    );
    let grant_only_text = text(&grant_only_output);
    assert!(
        !grant_only_output.status.success(),
        "grant approval alone must not replace the identity owner: {grant_only_text}"
    );
    assert!(
        grant_only_text.contains("IDENTITY_APPROVAL_REQUIRED"),
        "grant separation must name the missing identity approval: {grant_only_text}"
    );
    assert!(operation_records(&fixture.host_journal(), grant_only_id).is_empty());
    assert_eq!(
        master_status(&fixture, &pane),
        grant_after_replacement,
        "grant-only refusal must not mutate authority"
    );
}

#[test]
fn c07_same_key_approved_recovery_replays_projection_without_duplicate_effects() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c07");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c07-seed", false);
    let operation_id = "ctxop-v23-c07-replay";
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");

    let first = run_context_cli(
        &fixture,
        Some(&pane),
        Some(operation_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let first_payload = success_payload(&first);
    let first_journal = fs::read(fixture.host_journal()).expect("host journal");
    let first_events = journal_operations(&fixture.host_journal(), operation_id);
    let first_binding = first_payload["result"]["snapshot"]["binding"]["binding_id"]
        .as_str()
        .expect("recovered binding id")
        .to_owned();
    let first_binding_generation = first_payload["result"]["snapshot"]["binding"]
        ["endpoint_generation"]
        .as_u64()
        .expect("recovered binding generation");

    let replay = run_context_cli(
        &fixture,
        Some(&pane),
        Some(operation_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let replay_payload = success_payload(&replay);
    assert_eq!(
        replay_payload["result"]["outcome"], "completed",
        "{replay_payload}"
    );
    assert_eq!(
        replay_payload["result"]["operation_id"],
        json!(operation_id),
        "same-key replay must return the same committed projection: {replay_payload}"
    );
    assert_eq!(
        replay_payload["result"]["phase"], first_payload["result"]["phase"],
        "same-key replay must retain the original terminal phase: {replay_payload}"
    );
    assert_eq!(
        replay_payload["result"]["committed_phases"], first_payload["result"]["committed_phases"],
        "same-key replay must retain the original business receipts: {replay_payload}"
    );
    assert_eq!(
        fs::read(fixture.host_journal()).expect("host journal after replay"),
        first_journal,
        "same-key replay must not append another durable operation transition"
    );
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            &first_binding,
            first_binding_generation
        ),
        1,
        "same-key replay must not bind the recovered identity twice"
    );
    assert_eq!(
        operation_records(&fixture.host_journal(), operation_id),
        first_events,
        "same-key replay must not add duplicate operation records"
    );
}

#[test]
fn c08_same_key_changed_intent_conflicts_before_side_effects_and_preserves_original() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c08");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c08-seed", false);
    let operation_id = "ctxop-v23-c08-key";
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");

    let original = run_context_cli(
        &fixture,
        Some(&pane),
        Some(operation_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let original_payload = success_payload(&original);
    let original_binding = original_payload["result"]["snapshot"]["binding"]["binding_id"]
        .as_str()
        .expect("recovered binding id")
        .to_owned();
    let original_binding_generation = original_payload["result"]["snapshot"]["binding"]
        ["endpoint_generation"]
        .as_u64()
        .expect("recovered binding generation");
    let original_records = operation_records(&fixture.host_journal(), operation_id);
    let original_journal = fs::read(fixture.host_journal()).expect("host journal before conflict");

    let changed_pane = second_pane(&fixture, "c08", pane.server_pid);
    let changed = run_context_cli(
        &fixture,
        Some(&changed_pane),
        Some(operation_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let changed_text = text(&changed);
    assert!(
        changed_text.contains("IDENTITY_OPERATION_INTENT_CONFLICT"),
        "changed intent must be typed before side effects: {changed_text}"
    );
    let changed_payload = typed_failure(&changed);
    assert_eq!(
        changed_payload["result"]["outcome"], "denied",
        "{changed_payload}"
    );
    assert_eq!(
        fs::read(fixture.host_journal()).expect("host journal after conflict"),
        original_journal,
        "intent conflict must not append durable state"
    );
    assert_eq!(
        operation_records(&fixture.host_journal(), operation_id),
        original_records,
        "the original admitted operation must remain unchanged"
    );
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            &original_binding,
            original_binding_generation
        ),
        1,
        "the original binding must not be rebound after a conflicting key"
    );

    let query = fixture.context_query(operation_id, Some(&pane));
    assert_eq!(
        query["result"]["queried_operation"]["outcome"], "completed",
        "{query}"
    );
    assert_eq!(
        query["result"]["queried_operation"]["operation_id"],
        operation_id
    );
}

#[test]
fn c09_query_after_retired_credential_uses_only_retained_local_capability() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c09");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c09-seed", false);
    let operation_id = "ctxop-v23-c09-recovered";
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");
    let recovered = run_context_cli(
        &fixture,
        Some(&pane),
        Some(operation_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let recovered_payload = success_payload(&recovered);
    let durable_outcome = recovered_payload["result"]["outcome"].clone();
    let durable_binding = recovered_payload["result"]["snapshot"]["binding"]["binding_id"]
        .as_str()
        .expect("recovered binding id")
        .to_owned();
    let durable_binding_generation = recovered_payload["result"]["snapshot"]["binding"]
        ["endpoint_generation"]
        .as_u64()
        .expect("recovered binding generation");
    let proof = fixture.proof(operation_id);
    let capability = proof["query_capability"]
        .as_str()
        .expect("proof retains query capability")
        .to_owned();
    let before = stable_surfaces(&fixture, &pane, operation_id);

    let cli_query = fixture.command(&["context", "--op", operation_id, "--query"], Some(&pane));
    let cli_payload = success_payload(&cli_query);
    assert_eq!(
        cli_payload["result"]["invocation"], "query",
        "{cli_payload}"
    );
    assert_eq!(cli_payload["result"]["action"], "query", "{cli_payload}");
    assert_eq!(
        cli_payload["result"]["queried_operation"]["outcome"], durable_outcome,
        "{cli_payload}"
    );
    assert_eq!(
        cli_payload["result"]["queried_operation"]["operation_id"],
        operation_id
    );
    let cli_text = text(&cli_query);
    assert_no_secret_text("CLI query stdout", &cli_text, &capability);
    assert_no_public_secret_surface("CLI query result", &cli_payload);
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            &durable_binding,
            durable_binding_generation
        ),
        1,
        "query must not repeat the committed binding effect"
    );

    let mut mcp = Mcp::start(&fixture, Some(&pane));
    let mcp_result = mcp.call_context(9, json!({"operation_id": operation_id, "query": true}));
    assert_eq!(mcp_result["isError"], false, "{mcp_result}");
    let mcp_text = mcp_result["content"][0]["text"]
        .as_str()
        .expect("MCP query returns typed payload text");
    let mcp_payload: Value = serde_json::from_str(mcp_text).expect("MCP query payload is JSON");
    assert_eq!(
        mcp_payload["result"]["queried_operation"]["outcome"], durable_outcome,
        "{mcp_payload}"
    );
    assert_eq!(
        mcp_payload["result"]["queried_operation"]["operation_id"],
        operation_id
    );
    assert_no_secret_text("MCP query text", mcp_text, &capability);
    assert_no_public_secret_surface("MCP query result", &mcp_payload);
    mcp.finish();

    fixture.set_proof_capability(operation_id, "wrong-capability-c09");
    let denied = fixture.command(&["context", "--op", operation_id, "--query"], Some(&pane));
    let denied_text = text(&denied);
    assert!(
        denied_text.contains("IDENTITY_OPERATION_QUERY_DENIED"),
        "wrong capability must be typed without disclosure: {denied_text}"
    );
    assert!(!denied_text.contains("wrong-capability-c09"));
    fixture.set_proof_capability(operation_id, &capability);
    assert_eq!(stable_surfaces(&fixture, &pane, operation_id), before);
}

#[test]
fn c10_master_identity_restore_preserves_existing_grant() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c10");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c10-seed", true);
    let grant_before = master_status(&fixture, &pane);
    let grant_id = grant_before["grant_id"].clone();
    let grant_generation = grant_before["grant_generation"].clone();
    let recovery_id = "ctxop-v23-c10-restore";
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");
    let recovered = run_context_cli(
        &fixture,
        Some(&pane),
        Some(recovery_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let payload = success_payload(&recovered);
    assert_eq!(
        payload["result"]["invocation"], "approved_recovery",
        "{payload}"
    );
    let recovered_binding = &payload["result"]["snapshot"]["binding"];
    let recovered_binding_id = recovered_binding["binding_id"]
        .as_str()
        .expect("master identity restore returns a binding id");
    let recovered_endpoint_generation = recovered_binding["endpoint_generation"]
        .as_u64()
        .expect("master identity restore returns an endpoint generation");
    assert_eq!(
        recovered_binding_id, fence.binding_id,
        "master identity restore must retain the stable binding id: {payload}"
    );
    assert!(
        recovered_endpoint_generation > fence.endpoint_generation,
        "master identity restore must commit a greater endpoint generation: {payload}"
    );
    let grant_after = master_status(&fixture, &pane);
    assert_eq!(
        grant_after["grant_id"], grant_id,
        "master identity restore must retain the existing grant resource: {grant_after}"
    );
    assert_eq!(
        grant_after["grant_generation"], grant_generation,
        "master identity restore must retain the existing grant version: {grant_after}"
    );
    assert_eq!(
        grant_after["binding_id"], recovered_binding["binding_id"],
        "master identity restore must publish the recovered endpoint fence: {grant_after}"
    );
    assert_eq!(
        grant_after["endpoint_generation"], recovered_binding["endpoint_generation"],
        "master identity restore must publish the recovered endpoint fence: {grant_after}"
    );
    let mut expected_grant = grant_before.clone();
    expected_grant["binding_id"] = recovered_binding["binding_id"].clone();
    expected_grant["endpoint_generation"] = recovered_binding["endpoint_generation"].clone();
    assert_eq!(
        grant_after, expected_grant,
        "master identity restore must preserve principal, scope, authority, and approval metadata: {grant_after}"
    );

    let query = fixture.context_query(recovery_id, Some(&pane));
    assert_eq!(
        query["result"]["queried_operation"]["outcome"], "completed",
        "{query}"
    );
    assert_eq!(
        query["result"]["queried_operation"]["operation_id"], recovery_id,
        "the retained query must resolve the exact recovered operation: {query}"
    );
    assert_eq!(
        master_status(&fixture, &pane),
        expected_grant,
        "the retained authority readback must show the recovered endpoint with unchanged authority: {query}"
    );
}

#[test]
fn c11_dual_approval_commits_identity_and_grant_once_without_duplicate_effects() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c11");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c11-seed", true);
    let grant_before = master_status(&fixture, &pane);
    let grant_before_id = grant_before["grant_id"].clone();
    let grant_before_generation = grant_before["grant_generation"].clone();
    let operation_id = "ctxop-v23-c11-dual";
    let identity = identity_approval(&fixture, &pane, &fence, "replace_binding");
    let grant = grant_approval(&fixture, &pane, &fence, &grant_fence(&grant_before));
    let route_before = thread_route_event_count(&fixture.project_journal());

    let output = run_context_cli(
        &fixture,
        Some(&pane),
        Some(operation_id),
        None,
        None,
        Some(&identity),
        Some(&grant),
        None,
        false,
    );
    let payload = success_payload(&output);
    assert_eq!(
        payload["result"]["invocation"], "approved_recovery",
        "{payload}"
    );
    assert_eq!(
        payload["result"]["committed_phases"],
        json!([
            "approval_decision",
            "nested_register",
            "route",
            "credential",
            "grant",
            "lease",
            "context_complete"
        ]),
        "{payload}"
    );
    let new_binding = &payload["result"]["snapshot"]["binding"];
    let new_binding_id = new_binding["binding_id"]
        .as_str()
        .expect("dual approval returns a binding id")
        .to_owned();
    let new_endpoint_generation = new_binding["endpoint_generation"]
        .as_u64()
        .expect("dual approval returns an endpoint generation");
    assert_eq!(
        new_binding_id, fence.binding_id,
        "dual approval must retain the stable binding id: {payload}"
    );
    assert!(
        new_endpoint_generation > fence.endpoint_generation,
        "dual approval must commit a greater endpoint generation: {payload}"
    );

    let grant_after = master_status(&fixture, &pane);
    assert_eq!(
        grant_after["grant_id"], grant_before_id,
        "the grant owner must retain the same grant resource: {grant_after}"
    );
    assert_eq!(
        grant_after["grant_generation"],
        json!(grant_before_generation.as_u64().unwrap() + 1),
        "the grant owner must advance the grant version exactly once: {grant_after}"
    );
    assert_eq!(
        grant_after["binding_id"], new_binding_id,
        "the grant owner must publish the recovered endpoint fence: {grant_after}"
    );
    assert_eq!(
        grant_after["endpoint_generation"],
        json!(new_endpoint_generation),
        "the grant owner must publish the recovered endpoint fence: {grant_after}"
    );
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            &fence.binding_id,
            fence.endpoint_generation
        ),
        1,
        "the exact approved incumbent binding version must be retired once"
    );
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            &new_binding_id,
            new_endpoint_generation
        ),
        1,
        "exactly one newer binding version must commit"
    );
    assert_eq!(
        thread_route_event_count(&fixture.project_journal()),
        route_before + 1,
        "dual approval must publish the recovered route once"
    );
    let records = operation_records(&fixture.host_journal(), operation_id);
    let completed_records: Vec<_> = records
        .iter()
        .filter(|record| record["phase"] == "completed")
        .collect();
    assert_eq!(
        completed_records.len(),
        1,
        "the outer lifecycle must complete once: {records:?}"
    );
    let business_receipts = completed_records[0]["business_receipts"]
        .as_array()
        .expect("completed outer record has business receipts");
    assert_eq!(
        business_receipts.last().and_then(Value::as_str),
        Some("context_complete"),
        "the final business receipt must be context_complete: {records:?}"
    );
    assert_eq!(
        business_receipts
            .iter()
            .filter(|receipt| receipt.as_str() == Some("context_complete"))
            .count(),
        1,
        "context_complete must be recorded exactly once: {records:?}"
    );
    assert_eq!(
        completed_records[0]["business_receipts"],
        json!([
            "approval_decision",
            "nested_register",
            "route",
            "credential",
            "grant",
            "lease",
            "context_complete"
        ]),
        "the completed record must preserve the ordered business receipts: {records:?}"
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record["phase"] == "admitted")
            .count(),
        1,
        "admission must be durable once: {records:?}"
    );
}

/// Stale the identity produced by an already-completed seed so an approved
/// recovery can be staged against the exact committed incumbent fence.
fn stale_seeded_identity(fixture: &Fixture, seed: &Value) -> IdentityFence {
    let worker_id = seed["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("seed snapshot has worker id")
        .to_owned();
    let binding = &seed["result"]["snapshot"]["binding"];
    let binding_id = binding["binding_id"]
        .as_str()
        .expect("seed snapshot has binding id")
        .to_owned();
    let endpoint_generation = binding["endpoint_generation"]
        .as_u64()
        .expect("seed snapshot has endpoint generation");
    let identity_path = fixture
        .host_state
        .join("identities")
        .join(&worker_id)
        .join("identity.json");
    let mut identity: Value = serde_json::from_slice(
        &fs::read(&identity_path).expect("read isolated fixture identity cache"),
    )
    .expect("isolated fixture identity cache is JSON");
    let token = identity["token"]
        .as_str()
        .expect("isolated fixture identity has token")
        .to_owned();
    identity["token"] = json!(format!("stale-{token}"));
    let stale_identity_bytes =
        serde_json::to_vec_pretty(&identity).expect("serialize isolated fixture identity cache");
    fs::write(&identity_path, &stale_identity_bytes)
        .expect("write isolated fixture identity cache");
    IdentityFence {
        worker_id,
        binding_id,
        endpoint_generation,
        identity_path,
        stale_identity_bytes,
    }
}

/// Cancel one CLI invocation at the pre-send boundary. The fixture holds the
/// process before the first mutation byte, sends SIGINT, waits until the
/// process acknowledges it consumed the signal, and only then releases the
/// barrier so the local decision is never raced by the release.
fn cli_pre_send_cancel(
    fixture: &Fixture,
    pane: &Pane,
    harness: &mut CancellationHarness,
    operation_id: &str,
    approval: Option<&Value>,
    provide: Option<&Value>,
) -> std::process::Output {
    harness.arm("client_pre_send", operation_id);
    harness.arm("client_pre_send_signalled", operation_id);
    let mut child = spawn_context_cli(
        fixture,
        Some(pane),
        Some(operation_id),
        None,
        None,
        approval,
        None,
        provide,
        false,
    );
    let cli_pid = child.id() as i32;
    let barrier =
        match harness.try_wait_for("client_pre_send", operation_id, Duration::from_secs(30)) {
            Ok(barrier) => barrier,
            Err(error) => {
                let _ = child.kill();
                let output = child
                    .wait_with_output()
                    .expect("collect failed context child");
                panic!(
                    "{error}; context child status={:?}: {}",
                    output.status.code(),
                    text(&output)
                );
            }
        };
    assert_eq!(unsafe { libc::kill(cli_pid, libc::SIGINT) }, 0);
    harness
        .wait_for(
            "client_pre_send_signalled",
            operation_id,
            Duration::from_secs(30),
        )
        .release();
    barrier.release();
    wait_child_output(&mut child, Duration::from_secs(30))
}

/// Cancel one already-admitted CLI invocation exactly at a daemon-owned safe
/// boundary and return its drained public output. The durable cancellation is
/// observed before execution resumes, so the daemon cannot race past the
/// boundary with an un-cancelled owner start.
fn cli_cancel_at(
    fixture: &Fixture,
    pane: &Pane,
    harness: &mut CancellationHarness,
    boundary: &str,
    operation_id: &str,
    approval: Option<&Value>,
    provide: Option<&Value>,
) -> std::process::Output {
    harness.arm(boundary, operation_id);
    let mut child = spawn_context_cli(
        fixture,
        Some(pane),
        Some(operation_id),
        None,
        None,
        approval,
        None,
        provide,
        false,
    );
    let cli_pid = child.id() as i32;
    let barrier = harness.wait_for(boundary, operation_id, Duration::from_secs(30));
    assert_eq!(unsafe { libc::kill(cli_pid, libc::SIGINT) }, 0);
    assert!(
        wait_until(Duration::from_secs(30), || {
            operation_records(&fixture.host_journal(), operation_id)
                .iter()
                .any(|record| record["phase"] == "cancelled")
        }),
        "the daemon must durably cancel operation {operation_id} before the {boundary} boundary releases"
    );
    barrier.release();
    wait_child_output(&mut child, Duration::from_secs(30))
}

#[test]
fn c12_sigint_before_admission_returns_local_cancelled_without_daemon_operation() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let mut harness = CancellationHarness::start(&unique_root("c12-hook"));
    let (mut fixture, pane) = Fixture::isolated("collab-v23-c12");
    fixture.enable_cancellation_hooks(harness.socket());
    let seed = fixture.context(Some("ctxop-v23-c12-seed"), Some(&pane));
    assert_eq!(seed["result"]["outcome"], "completed", "{seed}");
    let supplement = json!({"session_id": pane.session_anchor});

    // 1) Pre-admission local cancellation before the first mutation byte for
    //    automatic, supplement and approved recovery through the public CLI.
    // An approved-recovery invocation only reaches the pre-send decision when
    // it carries a well-formed approval object; the content is never validated
    // because the local cancellation wins before any mutation byte is sent.
    let placeholder_approval = json!({
        "decision": "approved",
        "decided_by": "user",
        "target_identity": "peer-fixture",
        "project_scope": canonical_root(&fixture),
        "app_scope_id": "appserver-cli",
        "action": "replace_binding",
        "expected_incumbent": {"binding_id": "bind-placeholder", "endpoint_generation": 1},
        "intent_digest": "sha256:placeholder",
        "approved_at_ms": 1770000000000_i64
    });
    let variants = [
        ("automatic", "ctxop-v23-c12-auto", None, None),
        (
            "supplement",
            "ctxop-v23-c12-supp",
            None,
            Some(supplement.clone()),
        ),
        (
            "approved_recovery",
            "ctxop-v23-c12-approved",
            Some(placeholder_approval.clone()),
            None,
        ),
    ];
    for (label, operation_id, approval, provide) in variants {
        let output = cli_pre_send_cancel(
            &fixture,
            &pane,
            &mut harness,
            operation_id,
            approval.as_ref(),
            provide.as_ref(),
        );
        assert_eq!(output.status.code(), Some(2), "{label}: {}", text(&output));
        let payload = typed_failure(&output);
        assert_eq!(
            payload["result"]["outcome"], "cancelled",
            "{label}: {payload}"
        );
        assert_eq!(
            payload["result"]["phase"],
            Value::Null,
            "{label}: {payload}"
        );
        assert_eq!(
            payload["result"]["committed_phases"],
            json!([]),
            "{label}: {payload}"
        );
        assert!(
            operation_records(&fixture.host_journal(), operation_id).is_empty(),
            "{label}: a pre-admission cancellation must not create a daemon operation record"
        );
    }

    // 2) MCP standard cancellation notification before admission: the targeted
    //    tool result is suppressed and no daemon operation is created.
    let mcp_operation = "ctxop-v23-c12-mcp";
    harness.arm("client_pre_send", mcp_operation);
    harness.arm("client_pre_send_signalled", mcp_operation);
    let mut mcp = McpReader::start(&fixture, Some(&pane));
    let request_id = 41_u64;
    mcp.send_raw(&json!({
        "jsonrpc":"2.0",
        "id":request_id,
        "method":"tools/call",
        "params":{"name":"collab_context","arguments":{"operation_id":mcp_operation}}
    }));
    let barrier = harness.wait_for("client_pre_send", mcp_operation, Duration::from_secs(30));
    mcp.cancel(request_id);
    harness
        .wait_for(
            "client_pre_send_signalled",
            mcp_operation,
            Duration::from_secs(30),
        )
        .release();
    barrier.release();
    assert!(
        mcp.recv(Duration::from_secs(5)).is_err(),
        "a local pre-send MCP cancellation must not produce a tool result"
    );
    assert!(
        operation_records(&fixture.host_journal(), mcp_operation).is_empty(),
        "a local pre-send MCP cancellation must not create a daemon operation"
    );
    mcp.finish();

    // 3) After durable admission, before the first owner: the daemon wins the
    //    arbitration and returns the durable cancelled projection with no
    //    committed phases.
    let admitted_operation = "ctxop-v23-c12-admitted";
    let admitted = cli_cancel_at(
        &fixture,
        &pane,
        &mut harness,
        "admitted_before_owner",
        admitted_operation,
        None,
        None,
    );
    assert_eq!(admitted.status.code(), Some(2), "{}", text(&admitted));
    let admitted_payload = typed_failure(&admitted);
    assert_eq!(
        admitted_payload["result"]["outcome"], "cancelled",
        "{admitted_payload}"
    );
    assert_eq!(
        admitted_payload["result"]["committed_phases"],
        json!([]),
        "{admitted_payload}"
    );
    let admitted_records = operation_records(&fixture.host_journal(), admitted_operation);
    assert_eq!(
        admitted_records
            .iter()
            .filter(|record| record["phase"] == "cancelled")
            .count(),
        1,
        "admission-then-cancellation must be durable exactly once: {admitted_records:?}"
    );

    // 4) After proven committed owner work, before the next owner: cancellation
    //    preserves exactly the committed receipts and performs no next effect.
    let between_operation = "ctxop-v23-c12-between";
    let between = cli_cancel_at(
        &fixture,
        &pane,
        &mut harness,
        "between_owners",
        between_operation,
        None,
        None,
    );
    assert_eq!(between.status.code(), Some(2), "{}", text(&between));
    let between_payload = typed_failure(&between);
    assert_eq!(
        between_payload["result"]["outcome"], "cancelled",
        "{between_payload}"
    );
    assert_eq!(
        between_payload["result"]["committed_phases"],
        json!(["nested_register", "route", "credential", "lease"]),
        "{between_payload}"
    );
    let between_records = operation_records(&fixture.host_journal(), between_operation);
    assert_eq!(
        between_records
            .iter()
            .filter(|record| record["phase"] == "cancelled")
            .count(),
        1,
        "{between_records:?}"
    );

    // 5) Restart + query: both admitted cancellations survive exactly, while
    //    the pre-admission cancellations remain unknown.
    fixture.stop_daemon();
    fixture.restart_daemon();
    for (operation_id, expected_receipts) in [
        (admitted_operation, json!([])),
        (
            between_operation,
            json!(["nested_register", "route", "credential", "lease"]),
        ),
    ] {
        let query = fixture.context_query(operation_id, Some(&pane));
        assert_eq!(
            query["result"]["queried_operation"]["outcome"], "cancelled",
            "{operation_id}: {query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["business_receipts"], expected_receipts,
            "{operation_id}: {query}"
        );
    }
    for operation_id in [
        "ctxop-v23-c12-auto",
        "ctxop-v23-c12-supp",
        "ctxop-v23-c12-approved",
        mcp_operation,
    ] {
        let query = fixture.command(&["context", "--op", operation_id, "--query"], Some(&pane));
        let query_text = text(&query);
        assert!(
            !query.status.success(),
            "{operation_id} must not become queryable: {query_text}"
        );
        assert!(
            query_text.contains("IDENTITY_OPERATION_UNKNOWN"),
            "{operation_id}: {query_text}"
        );
    }
}

#[test]
fn c13_cancellation_after_owner_begins_never_claims_cancelled() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();

    // 1) Register owner already started: the exact nested `CommandStarted` has
    //    synced before the boundary is acknowledged. Cancellation is refused
    //    for this invocation, so the operation must finish truthfully instead
    //    of ever claiming cancellation.
    {
        let mut harness = CancellationHarness::start(&unique_root("c13-register-hook"));
        let (mut fixture, pane) = Fixture::isolated("collab-v23-c13-register");
        fixture.enable_cancellation_hooks(harness.socket());
        let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c13-register-seed", false);
        let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");
        let operation_id = "ctxop-v23-c13-register";
        harness.arm("register_start_synced", operation_id);
        let mut child = spawn_context_cli(
            &fixture,
            Some(&pane),
            Some(operation_id),
            None,
            None,
            Some(&approval),
            None,
            None,
            false,
        );
        let cli_pid = child.id() as i32;
        let barrier = harness.wait_for(
            "register_start_synced",
            operation_id,
            Duration::from_secs(30),
        );
        // The outer Validating record already carries the exact nested IDs
        // bound before consume; the owner transaction is in flight.
        let bound = operation_records(&fixture.host_journal(), operation_id);
        assert!(
            bound.iter().any(|record| {
                record["phase"] == "validating"
                    && record["nested_command_id"].as_str().is_some()
                    && record["nested_operation_id"].as_str().is_some()
            }),
            "Register owner must persist its nested IDs before the Start barrier: {bound:?}"
        );
        let nested_command_id = barrier
            .nested_command_id
            .clone()
            .expect("Start barrier identifies the exact Register command");
        let nested_operation_id = barrier
            .nested_operation_id
            .clone()
            .expect("Start barrier identifies the exact Register operation");
        let validating = bound
            .iter()
            .find(|record| record["phase"] == "validating")
            .expect("durable validating projection");
        assert_eq!(validating["nested_command_id"], nested_command_id);
        assert_eq!(validating["nested_operation_id"], nested_operation_id);
        assert_eq!(unsafe { libc::kill(cli_pid, libc::SIGINT) }, 0);
        barrier.release();
        let output = wait_child_output(&mut child, Duration::from_secs(30));
        assert_eq!(output.status.code(), Some(0), "{}", text(&output));
        let payload = json_stdout(&output);
        assert_eq!(
            payload["result"]["outcome"], "completed",
            "cancellation after Register Start must not win: {payload}"
        );
        assert_eq!(
            payload["result"]["committed_phases"],
            json!([
                "approval_decision",
                "nested_register",
                "route",
                "credential",
                "lease",
                "context_complete"
            ]),
            "{payload}"
        );
        let records = operation_records(&fixture.host_journal(), operation_id);
        assert!(
            records.iter().all(|record| record["phase"] != "cancelled"),
            "a started owner must never record a cancelled phase: {records:?}"
        );
        assert_eq!(
            records
                .iter()
                .filter(|record| record["phase"] == "admitted")
                .count(),
            1,
            "admission must be durable exactly once: {records:?}"
        );
        assert_eq!(
            records
                .iter()
                .filter(|record| record["phase"] == "inner_dispatched")
                .count(),
            1,
            "the outer promotion must be appended exactly once: {records:?}"
        );
        let admission = records
            .iter()
            .find(|record| record["phase"] == "admitted")
            .expect("admitted outer record");
        assert!(
            admission["approval_evidence"].is_object(),
            "the durable admission must retain immutable approval evidence: {admission}"
        );
        let completed = records
            .iter()
            .find(|record| record["phase"] == "completed")
            .expect("completed outer record");
        assert_eq!(completed["nested_command_id"], nested_command_id);
        assert_eq!(completed["nested_operation_id"], nested_operation_id);
        assert_eq!(
            completed["business_receipts"],
            json!([
                "approval_decision",
                "nested_register",
                "route",
                "credential",
                "lease",
                "context_complete"
            ]),
            "approval_decision must be promoted only after Start sync: {completed}"
        );
        let new_binding = &payload["result"]["snapshot"]["binding"];
        let new_binding_id = new_binding["binding_id"]
            .as_str()
            .expect("recovery returns a binding id");
        let new_endpoint_generation = new_binding["endpoint_generation"]
            .as_u64()
            .expect("recovery returns an endpoint generation");
        assert_eq!(
            binding_event_count(
                &fixture.project_journal(),
                &fence.binding_id,
                fence.endpoint_generation
            ),
            1,
            "the incumbent must be retired exactly once"
        );
        assert_eq!(
            binding_event_count(
                &fixture.project_journal(),
                new_binding_id,
                new_endpoint_generation
            ),
            1,
            "the recovered binding must commit exactly once"
        );

        fixture.stop_daemon();
        fixture.restart_daemon();
        let journal_before = fs::read(fixture.host_journal()).expect("host journal before query");
        let project_before =
            fs::read(fixture.project_journal()).expect("project journal before query");
        let query = fixture.context_query(operation_id, Some(&pane));
        assert_eq!(
            query["result"]["queried_operation"]["outcome"], "completed",
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["phase"], "completed",
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["nested_command_id"], nested_command_id,
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["nested_operation_id"], nested_operation_id,
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["business_receipts"],
            json!([
                "approval_decision",
                "nested_register",
                "route",
                "credential",
                "lease",
                "context_complete"
            ]),
            "{query}"
        );
        assert_eq!(
            fs::read(fixture.host_journal()).expect("host journal after query"),
            journal_before,
            "restart query must not append to the operation journal"
        );
        assert_eq!(
            fs::read(fixture.project_journal()).expect("project journal after query"),
            project_before,
            "restart query must not replay the Register owner"
        );
    }

    // 2) Register is proven committed, then the required grant owner fails
    //    before its Start. Preserve only the proven Register receipt prefix.
    {
        let mut harness = CancellationHarness::start(&unique_root("c13-partial-hook"));
        let (mut fixture, pane) = Fixture::isolated("collab-v23-c13-partial");
        fixture.enable_cancellation_hooks(harness.socket());
        let identity_fence =
            seed_stale_identity(&fixture, &pane, "ctxop-v23-c13-partial-seed", true);
        let status = master_status(&fixture, &pane);
        let expected_grant = grant_fence(&status);
        let identity_approval =
            identity_approval(&fixture, &pane, &identity_fence, "replace_binding");
        let grant_approval = grant_approval(&fixture, &pane, &identity_fence, &expected_grant);
        let operation_id = "ctxop-v23-c13-partial";
        harness.arm("before_grant_owner", operation_id);
        let mut child = spawn_context_cli(
            &fixture,
            Some(&pane),
            Some(operation_id),
            None,
            None,
            Some(&identity_approval),
            Some(&grant_approval),
            None,
            false,
        );
        let barrier = harness.wait_for("before_grant_owner", operation_id, Duration::from_secs(30));
        let before_release = operation_records(&fixture.host_journal(), operation_id);
        assert_eq!(
            before_release
                .iter()
                .filter(|record| record["phase"] == "inner_dispatched")
                .count(),
            1,
            "Register Start promotes the outer operation exactly once before later owners: {before_release:?}"
        );
        assert_eq!(
            grant_replacement_start_count(&fixture.project_journal(), operation_id),
            0,
            "the grant owner has not started at this boundary"
        );
        barrier.respond("fail_owner");
        let output = wait_child_output(&mut child, Duration::from_secs(30));
        assert_ne!(output.status.code(), Some(0), "{}", text(&output));
        let payload = typed_failure(&output);
        let proven_receipts = json!([
            "approval_decision",
            "nested_register",
            "route",
            "credential",
            "lease"
        ]);
        assert_eq!(payload["result"]["outcome"], "partial", "{payload}");
        assert_eq!(
            payload["result"]["committed_phases"], proven_receipts,
            "{payload}"
        );
        assert!(!payload["result"]["committed_phases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|receipt| receipt == "context_complete"));
        assert_eq!(
            grant_replacement_start_count(&fixture.project_journal(), operation_id),
            0,
            "failed next owner must have no Start or effect"
        );
        assert_eq!(
            grant_replacement_completed_count(&fixture.project_journal(), operation_id),
            0,
            "failed next owner must have no completion receipt"
        );
        let records = operation_records(&fixture.host_journal(), operation_id);
        assert_eq!(
            records
                .iter()
                .filter(|record| record["phase"] == "inner_dispatched")
                .count(),
            1,
            "{records:?}"
        );
        assert_eq!(
            records.last().unwrap()["business_receipts"],
            proven_receipts,
            "{records:?}"
        );
        assert_eq!(
            records.last().unwrap()["committed_phases"],
            json!([
                "validating",
                "inner_dispatched",
                "effect_observed",
                "partial"
            ]),
            "{records:?}"
        );
        fixture.stop_daemon();
        fixture.restart_daemon();
        let host_before =
            fs::read(fixture.host_journal()).expect("host journal before partial query");
        let project_before =
            fs::read(fixture.project_journal()).expect("project journal before partial query");
        let query = fixture.context_query(operation_id, Some(&pane));
        assert_eq!(
            query["result"]["queried_operation"]["outcome"], "partial",
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["business_receipts"], proven_receipts,
            "{query}"
        );
        assert_eq!(fs::read(fixture.host_journal()).unwrap(), host_before);
        assert_eq!(fs::read(fixture.project_journal()).unwrap(), project_before);
    }

    // 3) Register Start syncs but outer promotion fails. No Register business
    //    event or owner completion may follow; query and restart stay read-only.
    {
        let mut harness = CancellationHarness::start(&unique_root("c13-promotion-fault"));
        let (mut fixture, pane) = Fixture::isolated("collab-v23-c13-promotion-fault");
        fixture.enable_cancellation_hooks(harness.socket());
        let identity_fence =
            seed_stale_identity(&fixture, &pane, "ctxop-v23-c13-promotion-seed", false);
        let approval = identity_approval(&fixture, &pane, &identity_fence, "replace_binding");
        let operation_id = "ctxop-v23-c13-promotion-fault";
        harness.arm("register_start_synced", operation_id);
        let mut child = spawn_context_cli(
            &fixture,
            Some(&pane),
            Some(operation_id),
            None,
            None,
            Some(&approval),
            None,
            None,
            false,
        );
        let barrier = harness.wait_for(
            "register_start_synced",
            operation_id,
            Duration::from_secs(30),
        );
        let command_id = barrier
            .nested_command_id
            .clone()
            .expect("Register Start reports nested command id");
        let nested_operation_id = barrier
            .nested_operation_id
            .clone()
            .expect("Register Start reports nested operation id");
        barrier.respond("fail_promotion");
        let output = wait_child_output(&mut child, Duration::from_secs(30));
        assert_ne!(output.status.code(), Some(0), "{}", text(&output));
        let payload = typed_failure(&output);
        assert_eq!(payload["result"]["outcome"], "unknown", "{payload}");
        assert!(payload["result"]["phase"].is_null(), "{payload}");
        assert_eq!(
            payload["result"]["committed_phases"],
            json!([]),
            "{payload}"
        );
        let records = operation_records(&fixture.host_journal(), operation_id);
        assert!(
            records.iter().all(|record| {
                !matches!(
                    record["phase"].as_str(),
                    Some(
                        "inner_dispatched"
                            | "effect_observed"
                            | "completed"
                            | "partial"
                            | "cancelled"
                    )
                )
            }),
            "promotion failure cannot claim owner work or cancellation: {records:?}"
        );
        let latest = records.last().expect("durable validating record");
        assert_eq!(latest["nested_command_id"], command_id);
        assert_eq!(latest["nested_operation_id"], nested_operation_id);
        let project_records = read_lines(&fixture.project_journal());
        assert_eq!(
            project_records
                .iter()
                .filter(|record| {
                    record["ev"] == "CommandStarted"
                        && record["command_id"] == command_id
                        && record["operation_id"] == nested_operation_id
                })
                .count(),
            1,
            "the exact nested Register Start must be observable: {project_records:?}"
        );
        assert_eq!(
            project_records
                .iter()
                .filter(|record| {
                    record["ev"] == "CommandCompleted"
                        && record["command_id"] == command_id
                        && record["operation_id"] == nested_operation_id
                })
                .count(),
            0,
            "promotion failure precedes Register business and completion"
        );
        let host_before =
            fs::read(fixture.host_journal()).expect("host journal before unknown query");
        let project_before =
            fs::read(fixture.project_journal()).expect("project journal before unknown query");
        let query = fixture.context_query(operation_id, Some(&pane));
        assert_eq!(
            query["result"]["queried_operation"]["outcome"], "unknown",
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["phase"], "validating",
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["nested_command_id"], command_id,
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["nested_operation_id"], nested_operation_id,
            "{query}"
        );
        assert_eq!(fs::read(fixture.host_journal()).unwrap(), host_before);
        assert_eq!(fs::read(fixture.project_journal()).unwrap(), project_before);
        fixture.stop_degraded_daemon();
        fixture.restart_daemon();
        let host_after_restart =
            fs::read(fixture.host_journal()).expect("host journal before restart query");
        let project_after_restart =
            fs::read(fixture.project_journal()).expect("project journal before restart query");
        let query = fixture.context_query(operation_id, Some(&pane));
        assert_eq!(
            query["result"]["queried_operation"]["outcome"], "unknown",
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["phase"], "validating",
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["nested_command_id"], command_id,
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["nested_operation_id"], nested_operation_id,
            "{query}"
        );
        assert_eq!(
            query["result"]["queried_operation"]["nested_receipt"]["state"], "unavailable",
            "restart query must retain the explicit Register owner readback failure: {query}"
        );
        assert!(
            query["result"]["queried_operation"]["nested_receipt"]["error"]
                .as_str()
                .is_some_and(|error| error.contains("PROJECT_OWNER_READBACK_UNAVAILABLE")),
            "restart query must explain why the nested receipt cannot be read: {query}"
        );
        assert_eq!(
            fs::read(fixture.host_journal()).unwrap(),
            host_after_restart,
            "query must not repair the outer operation"
        );
        assert_eq!(
            fs::read(fixture.project_journal()).unwrap(),
            project_after_restart,
            "query must not replay or repeat Register"
        );
        fixture.stop_degraded_daemon();
    }

    // 4) Grant owner already started: the grant Start is durable in the
    //    resident authority journal before the boundary is acknowledged, so
    //    cancellation is refused and the replacement completes exactly once.
    {
        let mut harness = CancellationHarness::start(&unique_root("c13-grant-hook"));
        let (mut fixture, pane) = Fixture::isolated("collab-v23-c13-grant");
        fixture.enable_cancellation_hooks(harness.socket());
        let identity_fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c13-grant-seed", true);
        let status = master_status(&fixture, &pane);
        let expected_grant = grant_fence(&status);
        let identity_approval =
            identity_approval(&fixture, &pane, &identity_fence, "replace_binding");
        let grant_approval = grant_approval(&fixture, &pane, &identity_fence, &expected_grant);
        let operation_id = "ctxop-v23-c13-grant";
        harness.arm("grant_start_synced", operation_id);
        let mut child = spawn_context_cli(
            &fixture,
            Some(&pane),
            Some(operation_id),
            None,
            None,
            Some(&identity_approval),
            Some(&grant_approval),
            None,
            false,
        );
        let cli_pid = child.id() as i32;
        let barrier = harness.wait_for("grant_start_synced", operation_id, Duration::from_secs(30));
        assert_eq!(
            grant_replacement_start_count(&fixture.project_journal(), operation_id),
            1,
            "the grant owner must commit its durable Start before the barrier"
        );
        assert_eq!(unsafe { libc::kill(cli_pid, libc::SIGINT) }, 0);
        barrier.release();
        let output = wait_child_output(&mut child, Duration::from_secs(30));
        assert_eq!(output.status.code(), Some(0), "{}", text(&output));
        let payload = json_stdout(&output);
        assert_eq!(
            payload["result"]["outcome"], "completed",
            "cancellation after the grant Start must not win: {payload}"
        );
        assert_eq!(
            grant_replacement_start_count(&fixture.project_journal(), operation_id),
            1,
            "the grant Start must be durable exactly once"
        );
        assert_eq!(
            grant_replacement_completed_count(&fixture.project_journal(), operation_id),
            1,
            "the grant completion must be durable exactly once"
        );
        let grant_after = master_status(&fixture, &pane);
        assert_eq!(
            grant_after["grant_id"], expected_grant["grant_id"],
            "the replacement must retain the stable grant resource: {grant_after}"
        );
        assert_eq!(
            grant_after["grant_generation"],
            json!(
                expected_grant["generation"]
                    .as_u64()
                    .expect("grant generation")
                    + 1
            ),
            "the grant owner must advance the resource version exactly once: {grant_after}"
        );
        let records = operation_records(&fixture.host_journal(), operation_id);
        assert!(
            records.iter().all(|record| record["phase"] != "cancelled"),
            "a started grant owner must never record a cancelled phase: {records:?}"
        );

        fixture.stop_daemon();
        fixture.restart_daemon();
        let journal_before = fs::read(fixture.host_journal()).expect("host journal before query");
        let project_before =
            fs::read(fixture.project_journal()).expect("project journal before query");
        let query = fixture.context_query(operation_id, Some(&pane));
        assert_eq!(
            query["result"]["queried_operation"]["outcome"], "completed",
            "{query}"
        );
        assert_eq!(
            fs::read(fixture.host_journal()).expect("host journal after query"),
            journal_before,
            "restart query must not append to the operation journal"
        );
        assert_eq!(
            fs::read(fixture.project_journal()).expect("project journal after query"),
            project_before,
            "restart query must not replay the grant owner"
        );
    }
}

#[test]
fn c14_lost_first_response_queries_same_local_key_after_restart_without_remutation() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("collab-v23-c14");
    let fence = seed_stale_identity(&fixture, &pane, "ctxop-v23-c14-seed", false);
    let operation_id = "ctxop-v23-c14-lost";
    let approval = identity_approval(&fixture, &pane, &fence, "replace_binding");
    let mut first = spawn_context_cli(
        &fixture,
        Some(&pane),
        Some(operation_id),
        None,
        None,
        Some(&approval),
        None,
        None,
        false,
    );
    let lost = wait_child_output(&mut first, Duration::from_secs(30));
    let committed_payload = success_payload(&lost);
    let committed_binding = committed_payload["result"]["snapshot"]["binding"]["binding_id"]
        .as_str()
        .expect("lost-response recovery committed a binding")
        .to_owned();
    let committed_binding_generation = committed_payload["result"]["snapshot"]["binding"]
        ["endpoint_generation"]
        .as_u64()
        .expect("lost-response recovery committed an endpoint generation");
    let journal_after_commit = fs::read(fixture.host_journal()).expect("host journal after commit");
    let proof_before_restart = fixture.proof(operation_id);
    let binding_events = binding_event_count(
        &fixture.project_journal(),
        &committed_binding,
        committed_binding_generation,
    );
    let operation_events = operation_records(&fixture.host_journal(), operation_id);

    fixture.restart_daemon();
    let query = fixture.context_query(operation_id, Some(&pane));
    assert_eq!(
        query["result"]["queried_operation"]["outcome"], "completed",
        "{query}"
    );
    assert_eq!(
        fs::read(fixture.host_journal()).expect("host journal after query"),
        journal_after_commit,
        "query after adapter restart must be a pure read"
    );
    assert_eq!(
        binding_event_count(
            &fixture.project_journal(),
            &committed_binding,
            committed_binding_generation
        ),
        binding_events,
        "query must not repeat the committed binding effect"
    );
    assert_eq!(
        operation_records(&fixture.host_journal(), operation_id),
        operation_events,
        "query must not append a second operation"
    );
    assert_eq!(
        fixture.proof(operation_id)["operation_id"],
        proof_before_restart["operation_id"],
        "the key must come from the retained local proof record, not a lost response"
    );
}

fn identity_fence_from_status(fixture: &Fixture, pane: &Pane) -> IdentityFence {
    let status = master_status(fixture, pane);
    IdentityFence {
        worker_id: status["worker_id"]
            .as_str()
            .expect("master status has worker id")
            .to_owned(),
        binding_id: status["binding_id"]
            .as_str()
            .expect("master status has binding id")
            .to_owned(),
        endpoint_generation: status["endpoint_generation"]
            .as_u64()
            .expect("master status has endpoint generation"),
        identity_path: PathBuf::new(),
        stale_identity_bytes: Vec::new(),
    }
}

fn grant_fence(status: &Value) -> Value {
    json!({
        "grant_id": status["grant_id"],
        "generation": status["grant_generation"]
    })
}

fn wait_child_output(child: &mut std::process::Child, timeout: Duration) -> std::process::Output {
    assert!(
        wait_until(timeout, || {
            child
                .try_wait()
                .expect("poll exact fixture CLI child")
                .is_some()
        }),
        "fixture CLI child did not exit within {timeout:?}"
    );
    let stdout = child
        .stdout
        .take()
        .expect("fixture CLI child stdout is piped");
    let stderr = child
        .stderr
        .take()
        .expect("fixture CLI child stderr is piped");
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::BufReader::new(stdout), &mut stdout_bytes)
        .expect("read fixture CLI stdout");
    std::io::Read::read_to_end(&mut std::io::BufReader::new(stderr), &mut stderr_bytes)
        .expect("read fixture CLI stderr");
    let status = child.wait().expect("wait for exact fixture CLI child");
    std::process::Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    }
}

// C1: an unfinished task owned by the peer blocks Close; the typed refusal
// names the exact task id and no archive effect reaches the host.
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
