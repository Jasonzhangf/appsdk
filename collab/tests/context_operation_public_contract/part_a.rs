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
