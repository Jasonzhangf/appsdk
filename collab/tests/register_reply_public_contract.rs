include!("support/context_operation_fixture.rs");

fn registered_slot(fixture: &Fixture, snapshot: &Value) -> Value {
    let map: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let slots: Vec<_> = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|slot| {
            slot["identity_status"] == "active"
                && slot["identity_key"]["project_scope"] == snapshot["scope"]["project_scope"]
                && slot["identity_key"]["app_scope_id"] == snapshot["scope"]["app_scope_id"]
                && slot["current"]["binding"]["agent_id"] == snapshot["identity"]["worker_id"]
        })
        .collect();
    assert_eq!(
        slots.len(),
        1,
        "one active current identity for the exact scope and worker"
    );
    let slot = slots[0];
    assert_eq!(slot["identity_key"]["name"], snapshot["name"]);
    assert_eq!(
        slot["current"]["binding"]["session_id"],
        snapshot["binding"]["session_id"]
    );
    assert_eq!(
        slot["current"]["transport"],
        snapshot["identity"]["transport"]
    );
    slot.clone()
}

fn credential(fixture: &Fixture, snapshot: &Value) -> Value {
    serde_json::from_slice(
        &std::fs::read(
            fixture
                .host_state
                .join("identities")
                .join(snapshot["identity"]["worker_id"].as_str().unwrap())
                .join("identity.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn initial_tmux_registers_once_without_participant_return() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("gate-no-return");
    let response = fixture.context(Some("gate-no-return"), Some(&pane));
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
                    && rule
                        .contains("must not stop to report to the master and wait for a dispatch")
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
                rule.contains("Manage your own accepted goal")
                    && rule.contains("dispatch atomically starts the target peer's subscription")
                    && rule.contains("The target stable name retains ownership")
            })
    };
    let first = &response["result"]["snapshot"];
    assert!(
        owns_goal(first),
        "registration must explain stable goal ownership and delegated activation: {response}"
    );
    assert!(closes_idle_patrol(first), "{response}");
    assert!(first["role_brief"]["responsibilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|rule| rule == patrol_contract));
    let first_slot = registered_slot(&fixture, first);
    let first_credential = credential(&fixture, first);
    assert_eq!(
        fixture.current_state()["workers"][first["identity"]["worker_id"].as_str().unwrap()]
            ["token"],
        first_credential["token"]
    );
    assert!(!fixture
        .host_state
        .join("register-reply-evidence.jsonl")
        .exists());

    let restored = fixture.context(Some("gate-no-return"), Some(&pane));
    assert_eq!(restored["result"]["outcome"], "completed", "{restored}");
    let current = &restored["result"]["snapshot"];
    assert!(owns_goal(current));
    assert!(closes_idle_patrol(current));
    assert!(current["role_brief"]["responsibilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|rule| rule == patrol_contract));
    assert_eq!(current["identity"], first["identity"]);
    assert_eq!(current["binding"], first["binding"]);
    assert_eq!(current["master"], first["master"]);
    assert_eq!(registered_slot(&fixture, current), first_slot);
    assert_eq!(credential(&fixture, current), first_credential);

    let before = fixture.current_resources();
    let query = fixture.context_query("gate-no-return", Some(&pane));
    assert_eq!(query["result"]["outcome"], "completed", "{query}");
    for field in ["identity", "binding", "master", "name"] {
        assert_eq!(
            query["result"]["snapshot"][field], current[field],
            "query must preserve {field}"
        );
    }
    assert_eq!(fixture.current_resources(), before);
}

#[test]
fn tmux_without_agent_session_requires_facts_before_registration() {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("tmux-missing-agent-session");
    let before = fixture.current_resources();
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
    assert_eq!(fixture.current_resources(), before);
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
        let snapshot = &response["result"]["snapshot"];
        assert_eq!(snapshot["identity"]["transport"]["kind"], "tmux");
        let slot = registered_slot(&fixture, snapshot);
        assert_eq!(
            slot["current"]["binding"]["session_id"],
            pane.session_anchor
        );
        assert_eq!(
            slot["current"]["binding"]["tmux_endpoint"]["pane_id"],
            pane.pane_id
        );
        assert_eq!(
            slot["current"]["binding"]["tmux_endpoint"]["socket_path"],
            fixture.tmux_socket.to_str().unwrap()
        );
        assert!(!fixture
            .host_state
            .join("register-reply-evidence.jsonl")
            .exists());
    }
}

#[test]
fn selected_appserver_still_requires_valid_native_facts() {
    let fixture = Fixture::without_tmux("native-invalid");
    let before = fixture.current_resources();
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
    assert_eq!(fixture.current_resources(), before);
}

#[test]
fn agy_tmux_with_inherited_cross_project_codex_registers_real_agy_session_and_subsequent_commands_agree(
) {
    let _guard = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("agy-tmux-cross-project");
    let foreign_codex_session = "01a1590b-7d1c-78a0-bbb8-25b83f1ad578";
    let agy_conversation = "72bc1b6a-9d2e-4edf-a784-4e220b514f11";
    let foreign_worker = "codex-foreign-appsdk";
    let foreign_identity = json!({
        "worker_id":foreign_worker, "token":"token-foreign-appsdk", "project_scope":"project-foreign-appsdk-scope",
        "runtime":{"agent_id":foreign_worker,"runtime_id":"runtime-foreign-appsdk","appserver_id":"appserver-cli",
            "binding_id":"binding-foreign-appsdk","session_id":foreign_codex_session,"native_thread_id":foreign_codex_session},
        "transport":{"kind":"tmux","endpoint":"/tmp/foreign.sock","namespace":"$1",
            "session_id":foreign_codex_session,"thread_id":foreign_codex_session,
            "tmux_endpoint":{"socket_path":"/tmp/foreign.sock","server_pid":11111,"tmux_session_id":"$1",
                "pane_id":"%1","pane_pid":11112,"codex_session_id":foreign_codex_session,"codex_thread_id":foreign_codex_session},
            "capabilities":[],"self_check":"foreign transport"}
    });
    let foreign_dir = fixture.host_state.join("identities").join(foreign_worker);
    std::fs::create_dir_all(&foreign_dir).unwrap();
    let foreign_bytes = serde_json::to_vec_pretty(&foreign_identity).unwrap();
    std::fs::write(foreign_dir.join("identity.json"), &foreign_bytes).unwrap();

    let agy_output = fixture
        .agy_command_on_socket(&["context", "--op", "agy-reg-cross"], &pane)
        .env("ANTIGRAVITY_CONVERSATION_ID", agy_conversation)
        .env("CODEX_SESSION_ID", foreign_codex_session)
        .env("CODEX_THREAD_ID", foreign_codex_session)
        .output()
        .unwrap();
    assert!(
        agy_output.status.success(),
        "AGY registration failed: {agy_output:?}"
    );
    let response: Value = serde_json::from_slice(&agy_output.stdout).unwrap();
    assert_eq!(response["result"]["outcome"], "completed", "{response}");
    let snapshot = &response["result"]["snapshot"];
    assert_eq!(snapshot["session_id"], agy_conversation);
    assert_eq!(snapshot["binding"]["session_id"], agy_conversation);
    assert_eq!(
        snapshot["identity"]["transport"]["tmux_endpoint"]["codex_session_id"],
        agy_conversation
    );
    let slot = registered_slot(&fixture, snapshot);
    assert_eq!(slot["current"]["host"]["host_type"], "agy_tui");
    let saved_credential = credential(&fixture, snapshot);
    for args in [
        vec!["context", "--op", "agy-restore"],
        vec!["context", "--op", "agy-restore", "--query"],
    ] {
        let output = fixture
            .agy_command_on_socket(&args, &pane)
            .env("ANTIGRAVITY_CONVERSATION_ID", agy_conversation)
            .env("CODEX_SESSION_ID", foreign_codex_session)
            .env("CODEX_THREAD_ID", foreign_codex_session)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let restored: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(restored["result"]["outcome"], "completed");
        let current = &restored["result"]["snapshot"];
        assert_eq!(
            current["identity"]["worker_id"],
            snapshot["identity"]["worker_id"]
        );
        assert_eq!(current["binding"]["session_id"], agy_conversation);
        assert_eq!(registered_slot(&fixture, current), slot);
        assert_eq!(credential(&fixture, current), saved_credential);
    }
    for args in [vec!["who"], vec!["inbox"], vec!["board", "show"]] {
        let output = fixture
            .agy_command_on_socket(&args, &pane)
            .env("ANTIGRAVITY_CONVERSATION_ID", agy_conversation)
            .env("CODEX_SESSION_ID", foreign_codex_session)
            .env("CODEX_THREAD_ID", foreign_codex_session)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        if args == ["who"] {
            assert!(String::from_utf8_lossy(&output.stdout)
                .contains(snapshot["identity"]["worker_id"].as_str().unwrap()));
        }
    }
    assert_eq!(
        std::fs::read(foreign_dir.join("identity.json")).unwrap(),
        foreign_bytes
    );
}
