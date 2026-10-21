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
    assert_eq!(registration_effects(&fixture), [1, 1], "{response}");
    assert!(!fixture
        .host_state
        .join("register-reply-evidence.jsonl")
        .exists());
    let replay = fixture.context(Some("gate-no-return"), Some(&pane));
    assert_eq!(replay["result"]["outcome"], "completed", "{replay}");
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
