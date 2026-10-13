include!("support/context_operation_fixture.rs");

const CONSUMER_READ_SCRIPT: &str = r#"set -e
if [ -n "${PROVIDE:-}" ]; then
  "$COLLAB_BINARY" context --op "$OP_ID" --provide "$PROVIDE" >"$PANE_STDOUT" 2>"$PANE_STDERR" || true
else
  "$COLLAB_BINARY" context --op "$OP_ID" ${CONTEXT_OPTION:-} >"$PANE_STDOUT" 2>"$PANE_STDERR" || true
fi
IFS= read -r challenge
case "$challenge" in
  COLLAB_REGISTER_CHALLENGE\ {*) ;;
  *) exit 3 ;;
esac
CHALLENGE="${challenge#COLLAB_REGISTER_CHALLENGE }"
case "$CHALLENGE" in
  *"'"*) exit 4 ;;
esac
printf '%s' "$CHALLENGE" >"$CHALLENGE_FILE"
"#;

const CONSUMER_RETURN_SCRIPT: &str = r#"
"$COLLAB_BINARY" context --op "$OP_ID" --return-challenge "$CHALLENGE" >"$RETURN_OUT" 2>"$RETURN_ERR" || true
"$COLLAB_BINARY" context --op "$OP_ID" --return-challenge "$CHALLENGE" >"$DUPLICATE_OUT" 2>"$DUPLICATE_ERR" || true
printf '%s' "$CHALLENGE" >"$RETURNED_FILE"
sleep 600
"#;

fn registration_effects(fixture: &Fixture) -> [usize; 2] {
    std::fs::read_to_string(fixture.project_journal())
        .expect("read the real project journal for business effects")
        .lines()
        .fold([0, 0], |mut counts, line| {
            let value: Value = serde_json::from_str(line).unwrap();
            match value["ev"].as_str() {
                Some("Registered") => counts[0] += 1,
                Some("GlobalRuntimeBound") => counts[1] += 1,
                _ => {}
            }
            counts
        })
}

#[test]
fn second_pending_register_survives_first_binding_classification() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, first) = Fixture::isolated("gate-classification");
    let pane_id = String::from_utf8(
        tmux(
            &fixture.tmux_socket,
            &["new-window", "-d", "-P", "-F", "#{pane_id}", "sleep 600"],
        )
        .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    let second = Pane {
        server_pid: first.server_pid,
        session_anchor: format!("session-{pane_id}"),
        thread_anchor: format!("thread-{pane_id}"),
        pane_id,
    };
    let start = |pane: &Pane, name: &str| {
        let script = format!(
            r#"set -e
"$COLLAB_BINARY" context --op '{name}' >"$PANE_STDOUT" 2>"$PANE_STDERR" || true
IFS= read -r challenge
CHALLENGE="${{challenge#COLLAB_REGISTER_CHALLENGE }}"
printf '%s' "$CHALLENGE" >'{root}/{name}.received'
read -r _ <'{root}/{name}.fifo'
"$COLLAB_BINARY" context --return-challenge "$CHALLENGE" >'{root}/{name}.returned' 2>'{root}/{name}.err' || true
sleep 600
"#,
            root = fixture.root.display()
        );
        assert!(Command::new("mkfifo")
            .arg(fixture.root.join(format!("{name}.fifo")))
            .status()
            .unwrap()
            .success());
        fixture.run_in_pane(pane, name, &script)
    };
    let a = start(&first, "first-pending");
    assert!(
        wait_until(Duration::from_secs(15), || fixture
            .root
            .join("first-pending.received")
            .exists()),
        "{}",
        a.output()
    );
    let b = start(&second, "second-pending");
    assert!(
        wait_until(Duration::from_secs(15), || fixture
            .root
            .join("second-pending.received")
            .exists()),
        "{}",
        b.output()
    );
    let frozen = challenge_record(&fixture, "second-pending")["prepared"].clone();
    let release = |name: &str| {
        std::fs::write(fixture.root.join(format!("{name}.release")), b"release").unwrap();
        writeln!(
            std::fs::OpenOptions::new()
                .write(true)
                .open(fixture.root.join(format!("{name}.fifo")))
                .unwrap(),
            "release"
        )
        .unwrap();
    };
    release("first-pending");
    assert!(
        wait_until(Duration::from_secs(15), || std::fs::read_to_string(
            fixture.project_journal()
        )
        .unwrap()
        .contains("GlobalRuntimeBindingLedgerClassified")),
        "normal first-binding classification event missing"
    );
    assert_eq!(registration_effects(&fixture), [1, 1]);
    release("second-pending");
    assert!(wait_until(Duration::from_secs(15), || std::fs::read(
        fixture.root.join("second-pending.returned")
    )
    .is_ok_and(|b| !b.is_empty())));
    let returned: Value = serde_json::from_slice(
        &std::fs::read(fixture.root.join("second-pending.returned")).unwrap(),
    )
    .unwrap();
    assert_eq!(returned["result"]["outcome"], "completed", "{returned}");
    assert_eq!(
        challenge_record(&fixture, "second-pending")["prepared"],
        frozen
    );
    assert_eq!(registration_effects(&fixture), [2, 2]);
}

fn challenge_record(fixture: &Fixture, operation_id: &str) -> Value {
    let path = fixture.host_state.join("register-reply-evidence.jsonl");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("register reply evidence is JSON"))
        .filter(|record| record["operation_id"] == operation_id)
        .last()
        .unwrap_or_else(|| panic!("no register reply evidence for {operation_id}"))
}

fn run_consumer(fixture: &Fixture, pane: &Pane, label: &str, operation_id: &str) -> PaneConsumer {
    run_controlled_consumer(fixture, pane, label, operation_id, "", "")
}

fn run_controlled_consumer(
    fixture: &Fixture,
    pane: &Pane,
    label: &str,
    operation_id: &str,
    prelude: &str,
    before_return: &str,
) -> PaneConsumer {
    let script = format!(
        "OP_ID='{operation_id}'\nCHALLENGE_FILE='{}'\nRETURN_OUT='{}'\nRETURN_ERR='{}'\nRETURNED_FILE='{}'\nDUPLICATE_OUT='{}'\nDUPLICATE_ERR='{}'\n{prelude}\n{CONSUMER_READ_SCRIPT}\n{before_return}\n{CONSUMER_RETURN_SCRIPT}",
        fixture.root.join("challenge.json").display(),
        fixture.root.join("challenge-return.out").display(),
        fixture.root.join("challenge-return.err").display(),
        fixture.root.join("challenge-returned.json").display(),
        fixture.root.join("challenge-duplicate.out").display(),
        fixture.root.join("challenge-duplicate.err").display(),
    );
    fixture.run_in_pane(pane, label, &script)
}

fn wait_for_challenge(fixture: &Fixture, consumer: &PaneConsumer) -> Value {
    assert!(
        wait_until(std::time::Duration::from_secs(15), || fixture
            .root
            .join("challenge.json")
            .exists()),
        "selected-pane consumer did not receive the challenge: {}",
        consumer.output()
    );
    let bytes =
        std::fs::read(fixture.root.join("challenge.json")).expect("read selected-pane challenge");
    serde_json::from_slice(&bytes).expect("challenge is JSON")
}

fn wait_for_return(fixture: &Fixture, consumer: &PaneConsumer) -> Value {
    assert!(
        wait_until(std::time::Duration::from_secs(15), || fixture
            .root
            .join("challenge-returned.json")
            .exists()),
        "selected-pane participant did not execute the typed return: {}",
        consumer.output()
    );
    let result: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join("challenge-return.out")).unwrap())
            .unwrap();
    assert_eq!(result["ok"], true, "{result}");
    std::fs::read(fixture.root.join("challenge-returned.json"))
        .map(|bytes| serde_json::from_slice(&bytes).expect("returned challenge is JSON"))
        .expect("read returned challenge")
}

#[test]
fn initial_tmux_without_participant_return_never_registers() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("gate-no-return");
    let output = fixture.command(&["context", "--op", "gate-no-return"], Some(&pane));
    let response: Value = serde_json::from_slice(&output.stdout).expect("typed context result");
    assert!(
        !output.status.success(),
        "no participant returned: {response}"
    );
    assert_ne!(response["result"]["outcome"], "completed", "{response}");
    assert_eq!(registration_effects(&fixture), [0, 0], "{response}");
    let record = challenge_record(&fixture, "gate-no-return");
    assert_eq!(record["dispatch_started"], true, "{record}");
    assert!(record["verified"].is_null(), "{record}");
    assert!(
        !std::fs::read_to_string(fixture.host_state.join("register-reply-evidence.jsonl"))
            .unwrap_or_default()
            .is_empty()
    );
}

#[test]
fn selected_pane_reads_challenge_and_typed_return_completes_once() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("gate-selected-return");
    let consumer = run_consumer(&fixture, &pane, "consumer", "gate-selected-return");
    let challenge = wait_for_challenge(&fixture, &consumer);
    assert_eq!(challenge["operation_id"], "gate-selected-return");
    assert_eq!(challenge["purpose"], "initial_register");
    let returned = wait_for_return(&fixture, &consumer);
    assert_eq!(returned, challenge);
    let replay = fixture.context_query("gate-selected-return", Some(&pane));
    assert_eq!(
        replay["result"]["queried_operation"]["phase"], "completed",
        "{replay}"
    );
    let proof: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join("challenge-return.out")).unwrap())
            .unwrap();
    assert_eq!(proof["ok"], true, "{proof}");
    assert_eq!(proof["result"]["outcome"], "completed", "{proof}");
    assert_eq!(
        proof["result"]["committed_phases"],
        serde_json::json!([
            "nested_register",
            "route",
            "credential",
            "lease",
            "context_complete"
        ]),
        "{proof}"
    );
    let record = challenge_record(&fixture, "gate-selected-return");
    assert_eq!(record["verified"]["challenge"], challenge["challenge"]);
    assert_eq!(record["verified"]["anchor"]["pane_id"], pane.pane_id);
    let epochs_before = registration_effects(&fixture);
    assert_eq!(
        epochs_before,
        [1, 1],
        "registration and binding must each commit once"
    );
    let duplicate: Value = serde_json::from_slice(
        &std::fs::read(fixture.root.join("challenge-duplicate.out")).unwrap(),
    )
    .unwrap();
    assert_eq!(duplicate["ok"], true, "{duplicate}");
    assert_eq!(duplicate["result"]["outcome"], "completed", "{duplicate}");
    let replayed = fixture.context_query("gate-selected-return", Some(&pane));
    assert_eq!(
        replayed["result"]["queried_operation"]["phase"],
        "completed"
    );
    assert_eq!(
        registration_effects(&fixture),
        epochs_before,
        "a replay must not append a second registration epoch"
    );
}

#[test]
fn mixed_tmux_and_appserver_facts_select_verified_pane() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("gate-mixed-facts");
    // Both reported capabilities belong to this actual caller. Unrelated
    // native IDs must still be rejected by the CLI's fact merge guard.
    let session_id = pane.session_anchor.clone();
    let thread_id = pane.thread_anchor.clone();
    let appserver = TestAppServer::start(&fixture.root, &session_id, &thread_id);
    let facts = serde_json::json!({
        "session_id": session_id,
        "thread_id": thread_id,
        "endpoint": appserver.endpoint(),
        "namespace": "codex_tui"
    });
    let prelude = format!(
        "PROVIDE={}",
        shell_quote(&serde_json::to_string(&facts).unwrap())
    );
    let consumer = run_controlled_consumer(
        &fixture,
        &pane,
        "mixed-consumer",
        "gate-mixed-facts",
        &prelude,
        "",
    );
    let challenge = wait_for_challenge(&fixture, &consumer);
    assert_eq!(challenge["operation_id"], "gate-mixed-facts");
    wait_for_return(&fixture, &consumer);
    let record = challenge_record(&fixture, "gate-mixed-facts");
    assert!(!record["verified"].is_null(), "{record}");
    assert_eq!(
        record["verified"]["anchor"]["pane_id"], pane.pane_id,
        "{record}"
    );
    assert_eq!(
        appserver.start_request_count(),
        0,
        "unselected AppServer must not run"
    );
    let proof: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join("challenge-return.out")).unwrap())
            .unwrap();
    assert_eq!(proof["ok"], true, "{proof}");
    assert_eq!(
        proof["result"]["snapshot"]["identity"]["transport"]["kind"],
        "tmux"
    );
}

#[test]
fn echo_only_initial_tmux_input_does_not_register() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("gate-echo-only");
    let script = format!("OP_ID='gate-echo-only'\n\"$COLLAB_BINARY\" context --op \"$OP_ID\" >\"$PANE_STDOUT\" 2>\"$PANE_STDERR\" || true\nIFS= read -r challenge\nprintf '%s\\n' \"$challenge\" >'{}'\nsleep 600", fixture.root.join("echo.txt").display());
    let consumer = fixture.run_in_pane(&pane, "echo-consumer", &script);
    assert!(
        wait_until(Duration::from_secs(15), || fixture
            .root
            .join("echo.txt")
            .exists()),
        "echo-only consumer did not read pane input: {}",
        consumer.output()
    );
    let record = challenge_record(&fixture, "gate-echo-only");
    assert_eq!(record["dispatch_started"], true, "{record}");
    assert!(record["verified"].is_null(), "{record}");
    assert_eq!(registration_effects(&fixture), [0, 0]);
    assert!(
        record["marker"]
            .as_str()
            .is_some_and(|marker| marker.len() == 64),
        "the dispatched challenge must be a nonempty marker: {record}"
    );
    let echoed = std::fs::read_to_string(fixture.root.join("echo.txt")).unwrap();
    assert!(echoed.starts_with("COLLAB_REGISTER_CHALLENGE "), "{echoed}");
    let query = fixture.context_query("gate-echo-only", Some(&pane));
    assert_ne!(
        query["result"]["queried_operation"]["phase"], "completed",
        "{query}"
    );
    assert!(challenge_record(&fixture, "gate-echo-only")["verified"].is_null());
    assert_eq!(registration_effects(&fixture), [0, 0]);
}

#[test]
fn context_query_is_read_only_and_never_discloses_challenge() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("gate-query-readonly");
    let release = fixture.root.join("release-reader");
    let pause = format!(
        "while [ ! -f {} ]; do sleep 0.02; done",
        shell_quote(&release.to_string_lossy())
    );
    let consumer = run_controlled_consumer(
        &fixture,
        &pane,
        "query-consumer",
        "gate-query-readonly",
        "",
        &pause,
    );
    let challenge = wait_for_challenge(&fixture, &consumer);
    let paths = [
        fixture.project_journal(),
        fixture.host_journal(),
        fixture.host_routes(),
        fixture.host_state.join("register-reply-evidence.jsonl"),
    ];
    let before = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap_or_default())
        .collect::<Vec<_>>();
    let query = fixture.context_query("gate-query-readonly", Some(&pane));
    let encoded = serde_json::to_string(&challenge["challenge"]).unwrap();
    assert!(
        !serde_json::to_string(&query).unwrap().contains(&encoded),
        "query disclosed the challenge: {query}"
    );
    assert!(challenge["challenge"].as_str().is_some());
    assert_eq!(registration_effects(&fixture), [0, 0]);
    assert_eq!(
        before,
        paths
            .iter()
            .map(|path| std::fs::read(path).unwrap_or_default())
            .collect::<Vec<_>>(),
        "query must not mutate durable owners"
    );
    std::fs::write(release, b"return").unwrap();
    wait_for_return(&fixture, &consumer);
}

#[test]
fn pending_restart_keeps_exact_command_and_participant_return_replays_once() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("gate-restart");
    let release = fixture.root.join("release-reader");
    let pause = format!(
        "while [ ! -f {} ]; do sleep 0.02; done",
        shell_quote(&release.to_string_lossy())
    );
    let consumer = run_controlled_consumer(
        &fixture,
        &pane,
        "restart-consumer",
        "gate-restart",
        "",
        &pause,
    );
    wait_for_challenge(&fixture, &consumer);
    let before = std::fs::read(fixture.host_state.join("register-reply-evidence.jsonl")).unwrap();
    let prepared = challenge_record(&fixture, "gate-restart")["prepared"].clone();
    fixture.restart_daemon();
    let query = fixture.context_query("gate-restart", Some(&pane));
    assert_ne!(
        query["result"]["queried_operation"]["phase"], "completed",
        "{query}"
    );
    assert_eq!(
        std::fs::read(fixture.host_state.join("register-reply-evidence.jsonl")).unwrap(),
        before,
        "restart/query must not redispatch"
    );
    std::fs::write(release, b"return").unwrap();
    wait_for_return(&fixture, &consumer);
    assert_eq!(
        challenge_record(&fixture, "gate-restart")["prepared"],
        prepared
    );
    assert_eq!(registration_effects(&fixture), [1, 1]);
    let committed = std::fs::read(fixture.project_journal()).unwrap();
    let verified = std::fs::read(fixture.host_state.join("register-reply-evidence.jsonl")).unwrap();
    fixture.restart_daemon();
    // Startup may append its existing ledger classification/scan receipts.
    // Retain the committed bytes and measure query's read-only boundary after
    // startup, rather than attributing those startup records to query.
    let recovered = std::fs::read(fixture.project_journal()).unwrap();
    assert!(
        recovered.starts_with(&committed),
        "restart changed committed records"
    );
    assert_eq!(registration_effects(&fixture), [1, 1]);
    let query = fixture.context_query("gate-restart", Some(&pane));
    assert_eq!(
        query["result"]["queried_operation"]["phase"], "completed",
        "{query}"
    );
    assert_eq!(std::fs::read(fixture.project_journal()).unwrap(), recovered);
    assert_eq!(
        std::fs::read(fixture.host_state.join("register-reply-evidence.jsonl")).unwrap(),
        verified
    );
}

#[test]
fn cancelled_pending_rejects_actual_participant_return_without_commit() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("gate-cancelled");
    let release = fixture.root.join("release-reader");
    let pause = format!(
        "while [ ! -f {} ]; do sleep 0.02; done",
        shell_quote(&release.to_string_lossy())
    );
    let consumer = run_controlled_consumer(
        &fixture,
        &pane,
        "cancelled-consumer",
        "gate-cancelled",
        "",
        &pause,
    );
    wait_for_challenge(&fixture, &consumer);
    // Cancellation uses the caller's retained capability and public wire
    // control request. It does not inspect or return a pending nonce.
    let proof = fixture.proof("gate-cancelled");
    let root = std::fs::canonicalize(&fixture.root)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let request = json!({"op":"IdentityContext", "project_context":{
        "app_scope_id":"appserver-cli", "canonical_root":root, "project_scope":root},
        "facts":{}, "identity_context":{"operation_id":"gate-cancelled", "invocation":"automatic",
        "action":"cancel", "query_capability":proof["query_capability"]}});
    let mut stream = UnixStream::connect(fixture.host_state.join("server.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(stream, "{request}").unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    let cancelled: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(cancelled["ok"], true, "{cancelled}");
    assert_eq!(
        cancelled["cancellation"]["disposition"], "cancelled",
        "{cancelled}"
    );
    fixture.restart_daemon();
    std::fs::write(release, b"return").unwrap();
    assert!(
        wait_until(Duration::from_secs(15), || fixture
            .root
            .join("challenge-returned.json")
            .exists()),
        "{}",
        consumer.output()
    );
    let result: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join("challenge-return.out")).unwrap())
            .unwrap();
    assert_eq!(result["ok"], false, "{result}");
    assert!(
        result["error"]
            .as_str()
            .unwrap_or_default()
            .contains("IDENTITY_OPERATION_CANCELLED"),
        "{result}"
    );
    assert!(challenge_record(&fixture, "gate-cancelled")["verified"].is_null());
    assert_eq!(registration_effects(&fixture), [0, 0]);
    let query = fixture.context_query("gate-cancelled", Some(&pane));
    assert_eq!(
        query["result"]["queried_operation"]["phase"], "cancelled",
        "{query}"
    );
}

fn manual_restore_case(returns: bool) {
    let label = if returns {
        "gate-manual-success"
    } else {
        "gate-manual-failure"
    };
    let (fixture, pane) = Fixture::isolated(label);
    let seed = fixture.registered_context("manual-seed", &pane);
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user assigned isolated restore master",
        ],
        Some(&pane),
    );
    let before = fixture.context(Some("manual-before"), Some(&pane));
    let binding = before["result"]["snapshot"]["binding"].clone();
    let identity = before["result"]["snapshot"]["identity"].clone();
    let master = before["result"]["snapshot"]["master"].clone();
    let identity_path = fixture
        .host_state
        .join("identities")
        .join(
            seed["result"]["snapshot"]["identity"]["worker_id"]
                .as_str()
                .unwrap(),
        )
        .join("identity.json");
    let saved_identity = std::fs::read(&identity_path).unwrap();
    let effects = registration_effects(&fixture);
    let consumer = run_controlled_consumer(
        &fixture,
        &pane,
        label,
        label,
        "CONTEXT_OPTION='--recover-communication'",
        if returns { "" } else { "exec sleep 600" },
    );
    let challenge = wait_for_challenge(&fixture, &consumer);
    assert_eq!(challenge["purpose"], "manual_communication_restore");
    if returns {
        wait_for_return(&fixture, &consumer);
        assert!(!challenge_record(&fixture, label)["verified"].is_null());
        let query = fixture.context_query(label, Some(&pane));
        assert_eq!(
            query["result"]["queried_operation"]["phase"], "completed",
            "{query}"
        );
    } else {
        assert!(challenge_record(&fixture, label)["verified"].is_null());
        let query = fixture.context_query(label, Some(&pane));
        assert_ne!(
            query["result"]["queried_operation"]["phase"], "completed",
            "{query}"
        );
    }
    assert_eq!(
        registration_effects(&fixture),
        effects,
        "manual communication restore cannot Register"
    );
    let after = fixture.context(Some("manual-after"), Some(&pane));
    assert_eq!(after["result"]["snapshot"]["identity"], identity);
    assert_eq!(after["result"]["snapshot"]["binding"], binding);
    assert_eq!(after["result"]["snapshot"]["master"], master);
    assert_eq!(std::fs::read(identity_path).unwrap(), saved_identity);
    assert_eq!(registration_effects(&fixture), effects);
}

#[test]
fn manual_communication_restore_success_keeps_identity_role_grant_and_binding() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    manual_restore_case(true);
}

#[test]
fn manual_communication_restore_no_return_keeps_identity_role_grant_and_binding() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    manual_restore_case(false);
}

#[test]
fn appserver_ack_or_wrong_reply_cannot_waive_initial_gate() {
    for effect in ["register_no_reply", "register_wrong_reply"] {
        let fixture = Fixture::without_tmux(effect);
        let appserver = TestAppServer::start_with_effect(
            &fixture.root,
            "native-session",
            "native-thread",
            effect,
        );
        let facts = json!({"session_id":"native-session", "thread_id":"native-thread",
            "endpoint":appserver.endpoint(),"namespace":"codex_tui"});
        let output = fixture.command(
            &["context", "--op", effect, "--provide", &facts.to_string()],
            None,
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(!output.status.success(), "{result}");
        assert_ne!(result["result"]["outcome"], "completed", "{result}");
        assert!(challenge_record(&fixture, effect)["verified"].is_null());
        assert_eq!(registration_effects(&fixture), [0, 0]);
    }
}

#[test]
fn participant_wrong_operation_command_purpose_nonce_scope_and_anchor_never_commit() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("gate-wrong-return");
    let other_root = fixture.root.join("other-project");
    seed_baseline(&other_root);
    let release = fixture.root.join("release-reader");
    let prelude = format!(
        "OTHER_ROOT={}\nOTHER_SOCKET={}\nRELEASE_FILE={}\nWRONG_DONE={}",
        shell_quote(&other_root.to_string_lossy()),
        shell_quote(&fixture.root.join("other.sock").to_string_lossy()),
        shell_quote(&release.to_string_lossy()),
        shell_quote(&fixture.root.join("wrong-returns-done").to_string_lossy())
    );
    let before_return = r#"
for field in operation_id command_id purpose challenge; do
  bad=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1]); k=sys.argv[2]; d[k]="manual_communication_restore" if k=="purpose" else d[k]+"-wrong"; print(json.dumps(d))' "$CHALLENGE" "$field")
  if "$COLLAB_BINARY" context --return-challenge "$bad" >"$PROJECT_ROOT/wrong-$field.out" 2>"$PROJECT_ROOT/wrong-$field.err"; then exit 9; fi
done
if TMUX_PANE='%999999' "$COLLAB_BINARY" context --return-challenge "$CHALLENGE" >"$PROJECT_ROOT/wrong-pane.out" 2>"$PROJECT_ROOT/wrong-pane.err"; then exit 9; fi
if TMUX="$OTHER_SOCKET,1,0" "$COLLAB_BINARY" context --return-challenge "$CHALLENGE" >"$PROJECT_ROOT/wrong-socket.out" 2>"$PROJECT_ROOT/wrong-socket.err"; then exit 9; fi
if (cd "$OTHER_ROOT" && "$COLLAB_BINARY" context --return-challenge "$CHALLENGE") >"$PROJECT_ROOT/wrong-scope.out" 2>"$PROJECT_ROOT/wrong-scope.err"; then exit 9; fi
printf done >"$WRONG_DONE"
while [ ! -f "$RELEASE_FILE" ]; do sleep 0.02; done
"#;
    let consumer = run_controlled_consumer(
        &fixture,
        &pane,
        "wrong-consumer",
        "gate-wrong-return",
        &prelude,
        before_return,
    );
    wait_for_challenge(&fixture, &consumer);
    assert!(
        wait_until(Duration::from_secs(15), || fixture
            .root
            .join("wrong-returns-done")
            .exists()),
        "{}",
        consumer.output()
    );
    for (field, error) in [
        ("operation_id", "REGISTER_REPLY_PENDING_MISSING"),
        ("command_id", "REGISTER_REPLY_CORRELATION_MISMATCH"),
        ("purpose", "REGISTER_REPLY_CORRELATION_MISMATCH"),
        ("challenge", "REGISTER_REPLY_CORRELATION_MISMATCH"),
        ("pane", "REGISTER_REPLY_TARGET_MISMATCH"),
        ("socket", "REGISTER_REPLY_TARGET_MISMATCH"),
        ("scope", "REGISTER_REPLY_SCOPE_MISMATCH"),
    ] {
        let result: Value = serde_json::from_slice(
            &std::fs::read(fixture.root.join(format!("wrong-{field}.out"))).unwrap(),
        )
        .unwrap();
        assert_eq!(result["ok"], false, "{field}: {result}");
        assert!(
            result["error"].as_str().unwrap_or_default().contains(error),
            "{field}: {result}"
        );
    }
    assert_eq!(registration_effects(&fixture), [0, 0]);
    assert!(challenge_record(&fixture, "gate-wrong-return")["verified"].is_null());
    std::fs::write(release, b"return").unwrap();
    wait_for_return(&fixture, &consumer);
    assert_eq!(registration_effects(&fixture), [1, 1]);
}
