include!("support/context_operation_fixture.rs");

fn registration_effects(fixture: &Fixture) -> [usize; 2] {
    std::fs::read_to_string(fixture.project_journal())
        .unwrap()
        .lines()
        .fold([0, 0], |mut n, line| {
            let event: Value = serde_json::from_str(line).unwrap();
            match event["ev"].as_str() {
                Some("Registered") => n[0] += 1,
                Some("GlobalRuntimeBound") => n[1] += 1,
                _ => {}
            }
            n
        })
}

#[test]
fn initial_tmux_registers_once_without_participant_return() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("gate-no-return");
    let output = fixture.command(&["context", "--op", "gate-no-return"], Some(&pane));
    let response: Value = serde_json::from_slice(&output.stdout).expect("typed context result");
    assert!(output.status.success(), "{response}");
    assert_eq!(response["result"]["outcome"], "completed", "{response}");
    let patrol_contract = "For your own ongoing long-horizon goals, create and maintain finite patrol tasks and subscriptions. On each new master assignment, evaluate the goal, dependencies, blockers, priorities and resources, and update existing patrol scope as part of that assessment; add only missing scope and retire obsolete owned scope without duplicating timers. Subscription housekeeping does not gate ready authorized work.";
    let closes_idle_patrol = |snapshot: &Value| {
        snapshot["role_brief"]["responsibilities"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .any(|rule| {
                rule.contains("no authorized next action remains")
                    && rule.contains("request next work or explicit idle approval before stopping")
                    && rule.contains("Close unused owned patrol tasks and timed subscriptions")
                    && rule.contains("keep direct-message reception")
            })
    };
    let owns_goal = |snapshot: &Value| {
        snapshot["role_brief"]["responsibilities"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .any(|rule| {
                rule.contains("register your own accepted goal")
                    && rule.contains("Never ask master to proxy-register a peer goal")
                    && rule.contains("Stable name owns the goal")
            })
    };
    assert!(
        owns_goal(&response["result"]["snapshot"]),
        "registration must explain self-owned goals: {response}"
    );
    assert!(
        closes_idle_patrol(&response["result"]["snapshot"]),
        "{response}"
    );
    assert!(
        response["result"]["snapshot"]["role_brief"]["responsibilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule == patrol_contract),
        "registration must explain peer-owned patrol and assignment reassessment: {response}"
    );
    assert_eq!(registration_effects(&fixture), [1, 1], "{response}");
    assert!(!fixture
        .host_state
        .join("register-reply-evidence.jsonl")
        .exists());
    let replay = fixture.context(Some("gate-no-return"), Some(&pane));
    assert_eq!(replay["result"]["outcome"], "completed", "{replay}");
    assert!(
        owns_goal(&replay["result"]["snapshot"]),
        "recovery must retain self-owned goals: {replay}"
    );
    assert!(
        closes_idle_patrol(&replay["result"]["snapshot"]),
        "{replay}"
    );
    assert!(
        replay["result"]["snapshot"]["role_brief"]["responsibilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule == patrol_contract),
        "recovery must retain the peer patrol contract: {replay}"
    );
    assert_eq!(registration_effects(&fixture), [1, 1]);
    let before = std::fs::read(fixture.project_journal()).unwrap();
    let query = fixture.context_query("gate-no-return", Some(&pane));
    assert_eq!(
        query["result"]["queried_operation"]["phase"], "completed",
        "{query}"
    );
    assert_eq!(std::fs::read(fixture.project_journal()).unwrap(), before);
}

#[test]
fn tmux_without_agent_session_requires_facts_before_registration() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("tmux-missing-agent-session");
    let output = fixture
        .configured_command(&["context", "--op", "missing-agent-session"], Some(&pane))
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_THREAD")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["outcome"], "missing_facts");
    assert_eq!(
        response["result"]["requires"]["fields"],
        json!(["session_id"])
    );
    assert_eq!(response["result"]["committed_phases"], json!([]));
    assert_eq!(registration_effects(&fixture), [0, 0]);
}

#[test]
fn valid_tmux_ignores_unselected_appserver_facts() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    for (label, mut facts) in [
        (
            "tmux-partial-native",
            json!({"endpoint":"unix:///unused.sock"}),
        ),
        ("tmux-invalid-native", json!({"endpoint":"https://unused"})),
    ] {
        let (fixture, pane) = Fixture::isolated(label);
        facts["session_id"] = json!(pane.session_anchor);
        let output = fixture
            .configured_command(
                &["context", "--op", label, "--provide", &facts.to_string()],
                Some(&pane),
            )
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["result"]["outcome"], "completed", "{response}");
        assert_eq!(
            response["result"]["snapshot"]["identity"]["transport"]["kind"], "tmux",
            "{response}"
        );
        assert_eq!(registration_effects(&fixture), [1, 1]);
        assert!(!fixture
            .host_state
            .join("register-reply-evidence.jsonl")
            .exists());
    }
}

#[test]
fn selected_appserver_still_requires_valid_native_facts() {
    let fixture = Fixture::without_tmux("native-invalid");
    let output = fixture.command(
        &[
            "context",
            "--op",
            "native-invalid",
            "--provide",
            r#"{"endpoint":"https://unused","namespace":"codex_tui"}"#,
        ],
        None,
    );
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("IDENTITY_FACT_INVALID: endpoint"),
        "{output:?}"
    );
    assert!(!fixture
        .host_state
        .join("register-reply-evidence.jsonl")
        .exists());
}

fn challenge_record(fixture: &Fixture, operation_id: &str) -> Value {
    std::fs::read_to_string(fixture.host_state.join("register-reply-evidence.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|record| record["operation_id"] == operation_id)
        .last()
        .unwrap()
}

fn manual_restore_case(returns: bool) {
    let label = if returns {
        "gate-manual-success"
    } else {
        "gate-manual-failure"
    };
    let (fixture, pane) = Fixture::isolated(label);
    let seed = fixture.registered_context("manual-seed", &pane);
    let ledger_path = fixture.host_state.join("register-reply-evidence.jsonl");
    let historical = ["initial_register", "desktop_registration"]
        .into_iter()
        .map(|purpose| {
            format!(
                "{}\n",
                json!({
                    "purpose":purpose,
                    "operation_id":format!("retired-{purpose}"),
                    "worker_id":seed["result"]["snapshot"]["identity"]["worker_id"],
                    "transport":seed["result"]["snapshot"]["identity"]["transport"],
                    "prepared":{"historical_registration_intent":true},
                    "continuation":null,
                    "marker":"retired-fixture-challenge",
                    "dispatched_ms":1,
                    "dispatch_started":true,
                    "dispatch_receipt":{"accepted":true},
                    "verified":{"historical_participant_return":true}
                })
            )
        })
        .collect::<String>();
    std::fs::write(&ledger_path, &historical).unwrap();
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user assigned isolated restore master",
        ],
        Some(&pane),
    );
    let identity_path = fixture
        .host_state
        .join("identities")
        .join(
            seed["result"]["snapshot"]["identity"]["worker_id"]
                .as_str()
                .unwrap(),
        )
        .join("identity.json");
    let challenge_file = fixture.root.join("challenge.json");
    let returned_file = fixture.root.join("returned.json");
    let endpoint_file = fixture.root.join("current-endpoint.json");
    let after_file = fixture.root.join("after-endpoint.json");
    let endpoint_output = shell_quote(&endpoint_file.to_string_lossy());
    let finish = format!(
        "{}\n\"$COLLAB_BINARY\" context --op '{}-after' >{} 2>>\"$PANE_STDERR\"\n{}",
        if returns {
            format!("\"$COLLAB_BINARY\" context --return-challenge \"$CHALLENGE\" >{} 2>>\"$PANE_STDERR\"", shell_quote(&returned_file.to_string_lossy()))
        } else {
            String::new()
        },
        label,
        shell_quote(&after_file.to_string_lossy()),
        if returns { "" } else { "exec sleep 600" }
    );
    let script = format!(
        r#"set -e
"$COLLAB_BINARY" context --op 'manual-current-endpoint' >{endpoint_output}
"$COLLAB_BINARY" context --op '{label}' --recover-communication >"$PANE_STDOUT" 2>"$PANE_STDERR" || true
IFS= read -r challenge
case "$challenge" in COLLAB_REGISTER_CHALLENGE\ {{*) ;; *) exit 3 ;; esac
CHALLENGE="${{challenge#COLLAB_REGISTER_CHALLENGE }}"
printf '%s' "$CHALLENGE" >{}
{}
sleep 600
"#,
        shell_quote(&challenge_file.to_string_lossy()),
        finish
    );
    let consumer = fixture.run_interactive_in_pane(&pane, label, &script);
    assert!(
        wait_until(Duration::from_secs(15), || challenge_file.exists()),
        "{}",
        consumer.output()
    );
    let challenge: Value =
        serde_json::from_slice(&std::fs::read(&challenge_file).unwrap()).unwrap();
    assert!(std::fs::read(&ledger_path)
        .unwrap()
        .starts_with(historical.as_bytes()));
    let before: Value = serde_json::from_slice(&std::fs::read(endpoint_file).unwrap()).unwrap();
    let binding = before["result"]["snapshot"]["binding"].clone();
    let identity = before["result"]["snapshot"]["identity"].clone();
    let master = before["result"]["snapshot"]["master"].clone();
    let saved_identity = std::fs::read(&identity_path).unwrap();
    let effects = registration_effects(&fixture);
    assert_eq!(challenge["purpose"], "manual_communication_restore");
    if returns {
        assert!(
            wait_until(Duration::from_secs(15), || std::fs::read(&returned_file)
                .is_ok_and(|bytes| serde_json::from_slice::<Value>(&bytes).is_ok())),
            "{}",
            consumer.output()
        );
        let returned: Value =
            serde_json::from_slice(&std::fs::read(returned_file).unwrap()).unwrap();
        assert_eq!(returned["result"]["outcome"], "completed", "{returned}");
        assert!(!challenge_record(&fixture, label)["verified"].is_null());
        assert_eq!(
            fixture.context_query(label, None)["result"]["queried_operation"]["phase"],
            "completed"
        );
    } else {
        assert!(challenge_record(&fixture, label)["verified"].is_null());
        assert_ne!(
            fixture.context_query(label, None)["result"]["queried_operation"]["phase"],
            "completed"
        );
    }
    assert_eq!(registration_effects(&fixture), effects);
    assert!(
        wait_until(Duration::from_secs(15), || std::fs::read(&after_file)
            .is_ok_and(|bytes| serde_json::from_slice::<Value>(&bytes).is_ok())),
        "{}",
        consumer.output()
    );
    let after: Value = serde_json::from_slice(&std::fs::read(after_file).unwrap()).unwrap();
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
fn manual_restore_still_rejects_corrupt_or_nonretired_records() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    for (label, bytes) in [
        (
            "retired-malformed",
            "{\"purpose\":\"desktop_registration\",}\n",
        ),
        (
            "retired-incomplete",
            "{\"purpose\":\"desktop_registration\"}",
        ),
        ("unknown-purpose", "{\"purpose\":\"future_registration\"}\n"),
        (
            "invalid-manual",
            "{\"purpose\":\"manual_communication_restore\"}\n",
        ),
    ] {
        let (fixture, pane) = Fixture::isolated(label);
        let before = fixture.registered_context("reject-seed", &pane);
        let ledger = fixture.host_state.join("register-reply-evidence.jsonl");
        std::fs::write(&ledger, bytes).unwrap();
        let output = fixture.command(
            &["context", "--op", label, "--recover-communication"],
            Some(&pane),
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(!output.status.success(), "{label}: {result}");
        assert!(
            result.to_string().contains("REGISTER_REPLY_LEDGER_CORRUPT"),
            "{result}"
        );
        assert_eq!(std::fs::read(&ledger).unwrap(), bytes.as_bytes());
        let after = fixture.context(Some("reject-after"), Some(&pane));
        for field in ["identity", "binding", "master"] {
            assert_eq!(
                after["result"]["snapshot"][field], before["result"]["snapshot"][field],
                "{label}: {field}"
            );
        }
        assert_eq!(registration_effects(&fixture), [1, 1]);
    }
}

#[test]
fn agy_tmux_with_inherited_cross_project_codex_registers_real_agy_session_and_subsequent_commands_agree(
) {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("agy-tmux-cross-project");

    let foreign_codex_session = "01a1590b-7d1c-78a0-bbb8-25b83f1ad578";
    let foreign_codex_thread = "01a1590b-7d1c-78a0-bbb8-25b83f1ad578";
    let agy_conversation = "72bc1b6a-9d2e-4edf-a784-4e220b514f11";

    // 1. Seed foreign Project A in the shared host_state/identities directory.
    let foreign_worker = "codex-foreign-appsdk";
    let foreign_identity = json!({
        "worker_id": foreign_worker,
        "token": "token-foreign-appsdk",
        "project_scope": "project-foreign-appsdk-scope",
        "runtime": {
            "agent_id": foreign_worker,
            "runtime_id": "runtime-foreign-appsdk",
            "appserver_id": "appserver-cli",
            "endpoint_generation": 1,
            "binding_id": "binding-foreign-appsdk",
            "session_id": foreign_codex_session,
            "native_thread_id": foreign_codex_thread
        },
        "transport": {
            "kind": "tmux",
            "endpoint": "/tmp/foreign.sock",
            "namespace": "$1",
            "session_id": foreign_codex_session,
            "thread_id": foreign_codex_thread,
            "tmux_endpoint": {
                "socket_path": "/tmp/foreign.sock",
                "server_pid": 11111,
                "tmux_session_id": "$1",
                "pane_id": "%1",
                "pane_pid": 11112,
                "codex_session_id": foreign_codex_session,
                "codex_thread_id": foreign_codex_thread
            },
            "capabilities": [],
            "self_check": "foreign transport"
        }
    });
    let foreign_dir = fixture.host_state.join("identities").join(foreign_worker);
    std::fs::create_dir_all(&foreign_dir).unwrap();
    std::fs::write(
        foreign_dir.join("identity.json"),
        serde_json::to_vec_pretty(&foreign_identity).unwrap(),
    )
    .unwrap();

    // 2. Negative counterexample:
    // A Codex caller in this pane (inheriting foreign_codex_session, but NO ANTIGRAVITY_CONVERSATION_ID)
    // MUST fail with IDENTITY_RESTORE_CROSS_PROJECT.
    let codex_output = fixture
        .configured_command(&["context", "--op", "codex-fail-cross"], Some(&pane))
        .env("CODEX_SESSION_ID", foreign_codex_session)
        .env("CODEX_THREAD_ID", foreign_codex_thread)
        .env_remove("ANTIGRAVITY_CONVERSATION_ID")
        .output()
        .unwrap();
    assert!(
        !codex_output.status.success(),
        "Codex caller inheriting foreign session must fail: stdout={} stderr={}",
        String::from_utf8_lossy(&codex_output.stdout),
        String::from_utf8_lossy(&codex_output.stderr)
    );
    let codex_err = format!(
        "{}{}",
        String::from_utf8_lossy(&codex_output.stderr),
        String::from_utf8_lossy(&codex_output.stdout)
    );
    assert!(
        codex_err.contains("IDENTITY_RESTORE_CROSS_PROJECT"),
        "expected IDENTITY_RESTORE_CROSS_PROJECT for pure Codex, got: {codex_err}"
    );

    // 3. Positive target case (AGY caller with ANTIGRAVITY_CONVERSATION_ID + inherited CODEX_* pollution):
    // In unpatched code: `collab context` will ALSO fail with IDENTITY_RESTORE_CROSS_PROJECT!
    // In patched code: it shields the foreign CODEX_* pollution, uses ANTIGRAVITY_CONVERSATION_ID,
    // and successfully registers in Project B!
    // Uses real process fixture: launches through an `agy` named shell wrapper in the pane.
    let agy_output = fixture
        .agy_command_on_socket(&["context", "--op", "agy-reg-cross"], &pane)
        .env("ANTIGRAVITY_CONVERSATION_ID", agy_conversation)
        .env("CODEX_SESSION_ID", foreign_codex_session)
        .env("CODEX_THREAD_ID", foreign_codex_thread)
        .output()
        .unwrap();

    assert!(
        agy_output.status.success(),
        "AGY registration failed (EXPECTED IN RED TEST): stdout={} stderr={}",
        String::from_utf8_lossy(&agy_output.stdout),
        String::from_utf8_lossy(&agy_output.stderr)
    );
    let response: Value = serde_json::from_slice(&agy_output.stdout).expect("typed context result");
    assert_eq!(response["result"]["outcome"], "completed", "{response}");
    let snapshot = &response["result"]["snapshot"];
    assert_eq!(
        snapshot["session_id"], agy_conversation,
        "snapshot session_id must match real AGY conversation"
    );
    assert_eq!(
        snapshot["binding"]["session_id"], agy_conversation,
        "binding session_id must match real AGY conversation"
    );
    assert_eq!(
        snapshot["identity"]["transport"]["tmux_endpoint"]["codex_session_id"], agy_conversation,
        "transport codex_session_id must match real AGY conversation"
    );
    let registered_worker_id = snapshot["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // 4. Subsequent context call in the same pane restores the exact same identity
    // without re-registering or loopback.
    let restore_output = fixture
        .agy_command_on_socket(&["context", "--op", "agy-restore"], &pane)
        .env("ANTIGRAVITY_CONVERSATION_ID", agy_conversation)
        .env("CODEX_SESSION_ID", foreign_codex_session)
        .env("CODEX_THREAD_ID", foreign_codex_thread)
        .output()
        .unwrap();
    assert!(restore_output.status.success(), "subsequent context failed");
    let restore_resp: Value = serde_json::from_slice(&restore_output.stdout).unwrap();
    assert_eq!(restore_resp["result"]["outcome"], "completed");
    assert_eq!(
        restore_resp["result"]["snapshot"]["identity"]["worker_id"],
        registered_worker_id
    );
    assert_eq!(
        restore_resp["result"]["snapshot"]["binding"]["session_id"],
        agy_conversation
    );

    // 5. Subsequent public read-only CLI command (`collab context --query`)
    // in the same environment resolves to the exact same identity and AGY session.
    let query_output = fixture
        .agy_command_on_socket(&["context", "--op", "agy-restore", "--query"], &pane)
        .env("ANTIGRAVITY_CONVERSATION_ID", agy_conversation)
        .env("CODEX_SESSION_ID", foreign_codex_session)
        .env("CODEX_THREAD_ID", foreign_codex_thread)
        .output()
        .unwrap();
    assert!(
        query_output.status.success(),
        "query failed: stdout={} stderr={}",
        String::from_utf8_lossy(&query_output.stdout),
        String::from_utf8_lossy(&query_output.stderr)
    );

    // 6. Subsequent CLI commands (`collab who`, `collab inbox`, `collab board`)
    // agree with the registered AGY worker identity.
    let who_output = fixture
        .agy_command_on_socket(&["who"], &pane)
        .env("ANTIGRAVITY_CONVERSATION_ID", agy_conversation)
        .env("CODEX_SESSION_ID", foreign_codex_session)
        .env("CODEX_THREAD_ID", foreign_codex_thread)
        .output()
        .unwrap();
    assert!(
        who_output.status.success(),
        "who failed: {}",
        String::from_utf8_lossy(&who_output.stderr)
    );
    let who_text = String::from_utf8_lossy(&who_output.stdout);
    assert!(
        who_text.contains(&registered_worker_id),
        "who output must contain registered worker id: {who_text}"
    );

    let inbox_output = fixture
        .agy_command_on_socket(&["inbox"], &pane)
        .env("ANTIGRAVITY_CONVERSATION_ID", agy_conversation)
        .env("CODEX_SESSION_ID", foreign_codex_session)
        .env("CODEX_THREAD_ID", foreign_codex_thread)
        .output()
        .unwrap();
    assert!(
        inbox_output.status.success(),
        "inbox failed: {}",
        String::from_utf8_lossy(&inbox_output.stderr)
    );

    let board_output = fixture
        .agy_command_on_socket(&["board", "show"], &pane)
        .env("ANTIGRAVITY_CONVERSATION_ID", agy_conversation)
        .env("CODEX_SESSION_ID", foreign_codex_session)
        .env("CODEX_THREAD_ID", foreign_codex_thread)
        .output()
        .unwrap();
    assert!(
        board_output.status.success(),
        "board failed: {}",
        String::from_utf8_lossy(&board_output.stderr)
    );
}

#[test]
fn codex_tmux_cross_project_collision_fails_closed_without_agy_marker() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("codex-tmux-cross-project");

    let foreign_codex_session = "01a1590b-7d1c-78a0-bbb8-25b83f1ad578";
    let foreign_codex_thread = "01a1590b-7d1c-78a0-bbb8-25b83f1ad578";

    let foreign_worker = "codex-foreign-appsdk-isolated";
    let foreign_identity = json!({
        "worker_id": foreign_worker,
        "token": "token-foreign-appsdk-isolated",
        "project_scope": "project-foreign-appsdk-scope",
        "runtime": {
            "agent_id": foreign_worker,
            "runtime_id": "runtime-foreign-appsdk-isolated",
            "appserver_id": "appserver-cli",
            "endpoint_generation": 1,
            "binding_id": "binding-foreign-appsdk-isolated",
            "session_id": foreign_codex_session,
            "native_thread_id": foreign_codex_thread
        },
        "transport": {
            "kind": "tmux",
            "endpoint": "/tmp/foreign.sock",
            "namespace": "$1",
            "session_id": foreign_codex_session,
            "thread_id": foreign_codex_thread,
            "tmux_endpoint": {
                "socket_path": "/tmp/foreign.sock",
                "server_pid": 11111,
                "tmux_session_id": "$1",
                "pane_id": "%1",
                "pane_pid": 11112,
                "codex_session_id": foreign_codex_session,
                "codex_thread_id": foreign_codex_thread
            },
            "capabilities": [],
            "self_check": "foreign transport"
        }
    });
    let foreign_dir = fixture.host_state.join("identities").join(foreign_worker);
    std::fs::create_dir_all(&foreign_dir).unwrap();
    std::fs::write(
        foreign_dir.join("identity.json"),
        serde_json::to_vec_pretty(&foreign_identity).unwrap(),
    )
    .unwrap();

    let output = fixture
        .configured_command(&["context", "--op", "codex-cross-fail"], Some(&pane))
        .env("CODEX_SESSION_ID", foreign_codex_session)
        .env("CODEX_THREAD_ID", foreign_codex_thread)
        .env_remove("ANTIGRAVITY_CONVERSATION_ID")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        err.contains("IDENTITY_RESTORE_CROSS_PROJECT"),
        "pure Codex caller must be rejected across projects: {err}"
    );
}

#[test]
fn nested_codex_under_agy_preserves_codex_identity_and_rejects_cross_project() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("nested-codex-under-agy");

    let foreign_codex_session = "01a1590b-7d1c-78a0-bbb8-25b83f1ad578";
    let foreign_codex_thread = "01a1590b-7d1c-78a0-bbb8-25b83f1ad578";
    let agy_conversation = "72bc1b6a-9d2e-4edf-a784-4e220b514f11";

    let foreign_worker = "codex-foreign-appsdk-nested";
    let foreign_identity = json!({
        "worker_id": foreign_worker,
        "token": "token-foreign-appsdk-nested",
        "project_scope": "project-foreign-appsdk-scope",
        "runtime": {
            "agent_id": foreign_worker,
            "runtime_id": "runtime-foreign-appsdk-nested",
            "appserver_id": "appserver-cli",
            "endpoint_generation": 1,
            "binding_id": "binding-foreign-appsdk-nested",
            "session_id": foreign_codex_session,
            "native_thread_id": foreign_codex_thread
        },
        "transport": {
            "kind": "tmux",
            "endpoint": "/tmp/foreign.sock",
            "namespace": "$1",
            "session_id": foreign_codex_session,
            "thread_id": foreign_codex_thread,
            "tmux_endpoint": {
                "socket_path": "/tmp/foreign.sock",
                "server_pid": 22221,
                "tmux_session_id": "$1",
                "pane_id": "%1",
                "pane_pid": 22222,
                "codex_session_id": foreign_codex_session,
                "codex_thread_id": foreign_codex_thread
            },
            "capabilities": [],
            "self_check": "foreign transport"
        }
    });
    let foreign_dir = fixture.host_state.join("identities").join(foreign_worker);
    std::fs::create_dir_all(&foreign_dir).unwrap();
    std::fs::write(
        foreign_dir.join("identity.json"),
        serde_json::to_vec_pretty(&foreign_identity).unwrap(),
    )
    .unwrap();

    // When the process is explicitly identified as Codex (nested inside AGY environment),
    // Codex takes precedence: foreign Codex session triggers IDENTITY_RESTORE_CROSS_PROJECT.
    // Uses real process fixture: launches through an `agy` shell that spawns a `codex` shell.
    let output = fixture
        .nested_codex_command_on_socket(&["context", "--op", "nested-codex-test"], &pane)
        .env("ANTIGRAVITY_CONVERSATION_ID", agy_conversation)
        .env("CODEX_SESSION_ID", foreign_codex_session)
        .env("CODEX_THREAD_ID", foreign_codex_thread)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        err.contains("IDENTITY_RESTORE_CROSS_PROJECT"),
        "nested codex must reject foreign project: {err}"
    );
}

#[test]
fn unrecognized_process_with_agy_marker_does_not_assume_agy_and_fails_closed() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("unrecognized-host-fail-closed");

    let foreign_codex_session = "01a1590b-7d1c-78a0-bbb8-25b83f1ad578";
    let foreign_codex_thread = "01a1590b-7d1c-78a0-bbb8-25b83f1ad578";
    let agy_conversation = "72bc1b6a-9d2e-4edf-a784-4e220b514f11";

    let foreign_worker = "codex-foreign-appsdk-unrecognized";
    let foreign_identity = json!({
        "worker_id": foreign_worker,
        "token": "token-foreign-appsdk-unrecognized",
        "project_scope": "project-foreign-appsdk-scope",
        "runtime": {
            "agent_id": foreign_worker,
            "runtime_id": "runtime-foreign-appsdk-unrecognized",
            "appserver_id": "appserver-cli",
            "endpoint_generation": 1,
            "binding_id": "binding-foreign-appsdk-unrecognized",
            "session_id": foreign_codex_session,
            "native_thread_id": foreign_codex_thread
        },
        "transport": {
            "kind": "tmux",
            "endpoint": "/tmp/foreign.sock",
            "namespace": "$1",
            "session_id": foreign_codex_session,
            "thread_id": foreign_codex_thread,
            "tmux_endpoint": {
                "socket_path": "/tmp/foreign.sock",
                "server_pid": 22221,
                "tmux_session_id": "$1",
                "pane_id": "%1",
                "pane_pid": 22222,
                "codex_session_id": foreign_codex_session,
                "codex_thread_id": foreign_codex_thread
            },
            "capabilities": [],
            "self_check": "foreign transport"
        }
    });
    let foreign_dir = fixture.host_state.join("identities").join(foreign_worker);
    std::fs::create_dir_all(&foreign_dir).unwrap();
    std::fs::write(
        foreign_dir.join("identity.json"),
        serde_json::to_vec_pretty(&foreign_identity).unwrap(),
    )
    .unwrap();

    // Calling via configured_command (under python/runner) has no agy or codex executable
    // on its ancestor chain. When nearest_agent_host() returns None, host_runtime_facts
    // must NOT assume AGY: foreign Codex session must trigger IDENTITY_RESTORE_CROSS_PROJECT.
    let output = fixture
        .configured_command(&["context", "--op", "unknown-host-fail"], Some(&pane))
        .env("ANTIGRAVITY_CONVERSATION_ID", agy_conversation)
        .env("CODEX_SESSION_ID", foreign_codex_session)
        .env("CODEX_THREAD_ID", foreign_codex_thread)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        err.contains("IDENTITY_RESTORE_CROSS_PROJECT"),
        "unrecognized host must not claim AGY identity and must fail closed: {err}"
    );
}
