use super::*;

#[test]
fn user_confirmed_unknown_jenny_takeover_commits_name_and_master() {
    let mut fixture = AppFixture::new();
    let old = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"fixture user restores Jenny"}"#,
        ],
        THREAD_A,
    );
    fixture.run_public(&["context"], THREAD_B);
    let project = fixture
        .project
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    fixture.update_current_state(|state| {
        state["agent_presence"][&project][old["identity"]["worker_id"].as_str().unwrap()] = json!({
            "project_scope":project,
            "worker_id":old["identity"]["worker_id"], "session_id":THREAD_A,
            "presence":"unknown", "reason":"fixture unresponsive Jenny", "observed_ms":1
        })
    });
    let restored = fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_master","authorization":"fixture user confirms replacing unknown Jenny","replace_confirmed":true}"#], THREAD_B);
    assert_eq!(restored["name"], "Jenny");
    assert_eq!(
        restored["master"]["worker_id"],
        restored["identity"]["worker_id"]
    );
}

#[test]
fn current_name_restores_missing_business_worker_without_losing_ownership() {
    let mut fixture = AppFixture::new();
    fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"fixture user selects supervisor"}"#,
        ],
        THREAD_B,
    );
    let intent = r#"{"action":"restore_name","requested_name":"Iris","authorization":"fixture user restores Iris"}"#;
    let initial = fixture.run_public(&["context", "--approve-grant", intent], THREAD_A);
    assert_eq!(initial["name"], "Iris");
    let worker = initial["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture.run_public(
        &[
            "board",
            "publish",
            "iris-current-work",
            "--title",
            "Iris retained work",
            "--description",
            "current mapped owner",
            "--delivery-condition",
            "preserve ownership",
            "--test-condition",
            "real Context and Board",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "board",
            "claim",
            "iris-current-work",
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    let consumed = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Iris",
            "--subject",
            "consumed",
            "retained consumed body",
        ],
        THREAD_B,
    );
    let received = fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "iris-original-receive",
            "--timeout",
            "0",
        ],
        THREAD_A,
    );
    let pending = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Iris",
            "--subject",
            "pending",
            "retained pending body",
        ],
        THREAD_B,
    );
    let resource = fixture.root.join("iris-current-worktree");
    std::fs::create_dir(&resource).unwrap();
    let scope = std::fs::canonicalize(&fixture.project).unwrap();
    fixture.update_current_state(|state| {
        state["worktree_bindings"]["iris-worktree-binding"] = json!({
            "worktree_root":resource,"owning_project_scope":scope,"task_id":"iris-current-work",
            "owner_agent_id":worker,"binding_id":"iris-worktree-binding","base_commit":"fixture-base"
        });
    });
    let before = fixture.current_state();
    let map_path = fixture.host_state.join("identity-map.json");
    let mapping = std::fs::read(&map_path).unwrap();
    fixture.update_current_state(|state| {
        assert!(state["workers"]
            .as_object_mut()
            .unwrap()
            .remove(&worker)
            .is_some());
    });
    assert_eq!(std::fs::read(&map_path).unwrap(), mapping);
    assert!(fixture.current_state()["workers"].get(&worker).is_none());
    // The existing timer marks the genuinely missing owner once. Compare
    // reconstruction with that settled current state, not with pre-loss data.
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.current_state()["task_assignments"]["iris-current-work"]["reflow"]["reason"]
        != "owner_binding_missing"
    {
        assert!(
            Instant::now() < deadline,
            "missing-owner reflow did not settle"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let token = before["workers"][&worker]["token"].clone();
    let before = fixture.current_state();

    let restored = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(restored["name"], "Iris");
    assert_eq!(restored["identity"]["worker_id"], worker);
    assert_eq!(restored["binding"], initial["binding"]);
    assert_eq!(std::fs::read(&map_path).unwrap(), mapping);
    let after = fixture.current_state();
    assert!(
        token == after["workers"][&worker]["token"],
        "restoration changed the credential"
    );
    for field in [
        "tasks",
        "task_assignments",
        "task_lifecycle",
        "worktree_bindings",
        "receive_receipts",
        "subscriptions",
    ] {
        assert_eq!(after[field], before[field], "restoration changed {field}");
    }
    assert_eq!(
        after["msgs"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        before["msgs"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>()
    );
    for (id, original) in before["msgs"].as_object().unwrap() {
        let mut original = original.clone();
        let mut current = after["msgs"][id].clone();
        // The independent notification timer may deliver an existing pending
        // wake while Context restores the worker. Its observations can advance.
        for observation in ["last_wake_attempt_ms", "wake_attempt_count"] {
            assert!(
                current[observation].as_u64().unwrap() >= original[observation].as_u64().unwrap()
            );
            original.as_object_mut().unwrap().remove(observation);
            current.as_object_mut().unwrap().remove(observation);
        }
        assert_eq!(current, original, "restoration changed message {id}");
    }
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "iris-current-work")
        .unwrap();
    assert_eq!(task["owner_name"], "Iris");
    assert_eq!(task["owner_session_id"], THREAD_A);
    let explicit = fixture.run_public(&["context", "--approve-grant", intent], THREAD_A);
    assert_eq!(explicit["identity"]["worker_id"], worker);
    fixture.restart_daemon();
    assert_eq!(
        fixture.run_public(&["context"], THREAD_A)["identity"]["worker_id"],
        worker
    );
    let replayed = fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "iris-original-receive",
            "--timeout",
            "0",
        ],
        THREAD_A,
    );
    assert_eq!(replayed["messages"], received["messages"]);
    let incoming = fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "iris-restored-receive",
            "--timeout",
            "0",
        ],
        THREAD_A,
    );
    let messages = incoming["messages"].as_array().unwrap();
    assert!(messages
        .iter()
        .any(|message| message["id"] == pending["msg_id"]));
    assert!(messages
        .iter()
        .all(|message| message["id"] != consumed["msg_id"]));
}

#[test]
fn missing_current_worker_keeps_query_pure_and_refuses_invalid_credentials() {
    for case in [
        "missing",
        "malformed",
        "empty_token",
        "foreign_worker",
        "foreign_project",
        "foreign_app",
    ] {
        let mut fixture = AppFixture::new();
        let initial = fixture.run_public(&["context", "--op", "orphan-query"], THREAD_A);
        let worker = initial["identity"]["worker_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let credential_path = fixture
            .host_state
            .join("identities")
            .join(&worker)
            .join("identity.json");
        let original = std::fs::read(&credential_path).unwrap();
        fixture.update_current_state(|state| {
            state["workers"].as_object_mut().unwrap().remove(&worker);
        });
        let mapping = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
        let queried =
            fixture.command_public(&["context", "--query", "--op", "orphan-query"], THREAD_A);
        assert!(
            !queried.status.success(),
            "workerless query must report current unavailability"
        );
        assert!(fixture.current_state()["workers"].get(&worker).is_none());
        assert!(
            std::fs::read(&credential_path).unwrap() == original,
            "query changed credentials"
        );
        fixture.stop_daemon();
        let mut credential: Value = serde_json::from_slice(&original).unwrap();
        match case {
            "missing" => std::fs::remove_file(&credential_path).unwrap(),
            "malformed" => std::fs::write(&credential_path, b"{").unwrap(),
            _ => {
                match case {
                    "empty_token" => credential["token"] = json!(" "),
                    "foreign_worker" => credential["worker_id"] = json!("foreign-worker"),
                    "foreign_project" => credential["project_scope"] = json!("/foreign-project"),
                    "foreign_app" => credential["runtime"]["appserver_id"] = json!("foreign-app"),
                    _ => unreachable!(),
                }
                std::fs::write(&credential_path, serde_json::to_vec(&credential).unwrap()).unwrap();
            }
        }
        fixture.start_daemon();
        let refused = fixture.command_public(&["context"], THREAD_A);
        assert!(!refused.status.success(), "{case} credential was accepted");
        assert!(
            fixture.current_state()["workers"].get(&worker).is_none(),
            "{case} created a worker"
        );
        assert_eq!(
            std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
            mapping
        );
    }
}

#[test]
fn missing_desktop_worker_uses_current_binding_for_explicit_peer_and_master() {
    for action in ["restore_peer", "restore_master"] {
        let mut fixture = AppFixture::new_desktop_managed();
        let initial = fixture.run_ok_desktop(&["context"], THREAD_A);
        let worker = initial["identity"]["worker_id"]
            .as_str()
            .unwrap()
            .to_owned();
        fixture.update_current_state(|state| {
            state["workers"].as_object_mut().unwrap().remove(&worker);
        });
        let approval = json!({"action":action,"authorization":"fixture user selects role after business loss"}).to_string();
        let restored = fixture.run_ok_desktop(&["context", "--approve-grant", &approval], THREAD_A);
        assert_eq!(restored["identity"]["worker_id"], worker);
        assert_eq!(restored["identity"]["transport"]["namespace"], "codex_app");
        if action == "restore_master" {
            assert_eq!(restored["name"], "Jenny");
            assert_eq!(restored["master"]["worker_id"], worker);
        } else {
            assert_eq!(restored["name"], initial["name"]);
            assert!(restored["master"].is_null());
        }
    }
}

#[test]
fn missing_current_worker_retries_a_real_business_write_failure_without_map_changes() {
    let mut fixture = AppFixture::new();
    let initial = fixture.run_public(&["context"], THREAD_A);
    let worker = initial["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture.update_current_state(|state| {
        state["workers"].as_object_mut().unwrap().remove(&worker);
    });
    let map_path = fixture.host_state.join("identity-map.json");
    let mapping = std::fs::read(&map_path).unwrap();
    let state_path = fixture.project_state_for(&fixture.project, "appserver-cli");
    let held = state_path.with_extension("fixture-held");
    std::fs::rename(&state_path, &held).unwrap();
    std::fs::create_dir(&state_path).unwrap();
    let refused = fixture.command_public(&["context"], THREAD_A);
    std::fs::remove_dir(&state_path).unwrap();
    std::fs::rename(&held, &state_path).unwrap();
    assert!(
        !refused.status.success(),
        "failed atomic business publication was accepted"
    );
    assert_eq!(std::fs::read(&map_path).unwrap(), mapping);
    let restored = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(restored["identity"]["worker_id"], worker);
    assert_eq!(std::fs::read(&map_path).unwrap(), mapping);
}

#[test]
fn rejected_registration_does_not_publish_an_active_identity_without_business_or_credential() {
    let fixture = AppFixture::new();
    let initial = fixture.run_public(&["context"], THREAD_A);
    let map_path = fixture.host_state.join("identity-map.json");
    let mapping = std::fs::read(&map_path).unwrap();
    let workers = fixture.current_state()["workers"].clone();
    let scope = std::fs::canonicalize(&fixture.project).unwrap();
    let request = json!({
        "op":"Register","worker_id":"session-carrier-successor","token":"fixture-successor-token",
        "cwd":scope,"project_context":{"project_scope":scope,"canonical_root":scope,"app_scope_id":"appserver-cli"},
        "candidates":{"appserver":{"endpoint":format!("unix://{}",fixture.app_socket.display()),
            "namespace":"codex_tui","session_id":THREAD_A,"thread_id":THREAD_A,"cwd":scope}}
    });
    let mut stream = UnixStream::connect(fixture.host_state.join("server.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    writeln!(stream, "{request}").unwrap();
    let mut line = String::new();
    std::io::BufReader::new(stream)
        .read_line(&mut line)
        .unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["ok"], false, "duplicate runtime must be refused");
    assert!(response["error"]
        .as_str()
        .unwrap()
        .contains("REGISTER_RUNTIME_RELEASE_REJECTED"));
    assert!(
        std::fs::read(&map_path).unwrap() == mapping,
        "refused registration published a new active identity"
    );
    assert_eq!(fixture.current_state()["workers"], workers);
    assert!(!fixture
        .host_state
        .join("identities/session-carrier-successor/identity.json")
        .exists());
    assert_eq!(
        fixture.run_public(&["context"], THREAD_A)["name"],
        initial["name"]
    );
}

#[test]
fn concurrent_public_registrations_publish_only_the_runtime_winner() {
    let fixture = AppFixture::new();
    let scope = std::fs::canonicalize(&fixture.project).unwrap();
    let start = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut children = Vec::new();
    for worker in ["concurrent-alice", "concurrent-bob"] {
        let start = start.clone();
        let socket = fixture.host_state.join("server.sock");
        let request = json!({
            "op":"Register","worker_id":worker,"token":format!("{worker}-token"),
            "cwd":scope,"project_context":{"project_scope":scope,"canonical_root":scope,"app_scope_id":"appserver-cli"},
            "candidates":{"appserver":{"endpoint":format!("unix://{}",fixture.app_socket.display()),
                "namespace":"codex_tui","session_id":THREAD_A,"thread_id":THREAD_A,"cwd":scope}}
        });
        children.push(std::thread::spawn(move || {
            let mut stream = UnixStream::connect(socket).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            start.wait();
            writeln!(stream, "{request}").unwrap();
            let mut line = String::new();
            std::io::BufReader::new(stream)
                .read_line(&mut line)
                .unwrap();
            (worker, serde_json::from_str::<Value>(&line).unwrap())
        }));
    }
    start.wait();
    let results: Vec<_> = children
        .into_iter()
        .map(|child| child.join().unwrap())
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|(_, response)| response["ok"] == true)
            .count(),
        1,
        "{results:?}"
    );
    let (winner, _) = results
        .iter()
        .find(|(_, response)| response["ok"] == true)
        .unwrap();
    let (loser, response) = results
        .iter()
        .find(|(_, response)| response["ok"] == false)
        .unwrap();
    assert!(response["error"]
        .as_str()
        .unwrap()
        .contains("REGISTER_RUNTIME_RELEASE_REJECTED"));
    let state = fixture.current_state();
    assert!(state["workers"].get(*winner).is_some());
    assert!(state["workers"].get(*loser).is_none());
    let map: Value = serde_json::from_slice(
        &std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    )
    .unwrap();
    let active: Vec<_> = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|slot| slot["current"].is_object())
        .collect();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0]["current"]["binding"]["agent_id"], *winner);
    assert!(map
        .get("effects")
        .is_none_or(|effects| effects.as_array().unwrap().is_empty()));
    assert!(!fixture
        .host_state
        .join("identities")
        .join(loser)
        .join("identity.json")
        .exists());
}

#[test]
fn published_initial_registration_completes_its_first_business_transaction_once() {
    let mut fixture = AppFixture::new();
    fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_master","authorization":"fixture initial registration supervisor"}"#], THREAD_B);
    let before = fixture.current_state();
    let initial = fixture.run_public(&["context"], THREAD_A);
    let worker = initial["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let after = fixture.current_state();
    let operation = after["global"]["command_receipts"]
        .as_object()
        .unwrap()
        .values()
        .find(|receipt| {
            receipt["operation_id"]
                .as_str()
                .is_some_and(|op| op.starts_with("register-op-"))
                && !before["global"]["command_receipts"]
                    .as_object()
                    .unwrap()
                    .contains_key(receipt["command_id"].as_str().unwrap())
        })
        .unwrap()["operation_id"]
        .clone();
    // Explicit offline boundary: retain the real published map and credential,
    // but restore business state to immediately before its first consumption.
    fixture.update_current_state(|state| *state = before.clone());
    fixture.stop_daemon();
    let path = fixture.host_state.join("identity-map.json");
    let mut map: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let slot = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["current"]["binding"]["agent_id"] == worker)
        .unwrap();
    map["effects"] = json!([{
        "kind":"register", "role":null,"effect_id":"pending-first-business-register",
        "operation_id":operation,"project_scope":slot["identity_key"]["project_scope"],
        "app_scope_id":"appserver-cli","name":slot["identity_key"]["name"],
        "previous_worker_id":null,"previous_session_id":null,
        "target_worker_id":worker,"target_session_id":THREAD_A,"occupancy_revision":1,
        "source_names":[],"moves_existing_names":false,"pane_anchor":null,"created_ms":1
    }]);
    std::fs::write(&path, serde_json::to_vec(&map).unwrap()).unwrap();
    fixture.start_daemon();
    let restored = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(restored["identity"]["worker_id"], worker);
    assert_eq!(restored["name"], initial["name"]);
    let joined = fixture.run_public(&["recv", "--timeout", "0"], THREAD_B);
    assert_eq!(
        joined["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["subject"] == "peer-registered")
            .count(),
        1,
        "first business consumption lost its registration notice"
    );
    fixture.run_public(&["context"], THREAD_A);
    fixture.stop_daemon();
    fixture.start_daemon();
    fixture.run_public(&["context"], THREAD_A);
    let repeated = fixture.run_public(&["recv", "--timeout", "0"], THREAD_B);
    assert!(!repeated["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["subject"] == "peer-registered"));
    let complete: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(complete
        .get("effects")
        .is_none_or(|effects| effects.as_array().unwrap().is_empty()));
}

#[test]
fn missing_current_worker_completes_pending_name_and_register_without_old_receipt_replay() {
    for case in ["name_role", "register_pending", "register_completed"] {
        let mut fixture = AppFixture::new();
        let initial = fixture.run_public(&["context"], THREAD_A);
        let worker = initial["identity"]["worker_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let state = fixture.current_state();
        let original = state["global"]["command_receipts"]
            .as_object()
            .unwrap()
            .values()
            .find(|receipt| {
                receipt["operation_id"]
                    .as_str()
                    .is_some_and(|op| op.starts_with("register-op-"))
            })
            .unwrap();
        let operation = if case == "register_pending" {
            format!(
                "{}-retry-{}",
                original["operation_id"].as_str().unwrap(),
                state["revision"]
            )
        } else {
            original["operation_id"].as_str().unwrap().to_owned()
        };
        fixture.update_current_state(|state| {
            state["workers"].as_object_mut().unwrap().remove(&worker);
        });
        fixture.stop_daemon();
        let map_path = fixture.host_state.join("identity-map.json");
        let mut map: Value = serde_json::from_slice(&std::fs::read(&map_path).unwrap()).unwrap();
        let slot = map["slots"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|slot| slot["current"]["binding"]["agent_id"] == worker)
            .unwrap();
        let old_name = slot["identity_key"]["name"].clone();
        let scope = slot["identity_key"]["project_scope"].clone();
        let name = if case == "name_role" {
            slot["identity_key"]["name"] = json!("Jenny");
            slot["display_slot"] = json!(1);
            "Jenny".to_owned()
        } else {
            old_name.as_str().unwrap().to_owned()
        };
        let role = (case == "name_role").then(|| {
            json!({"action":"set_master","authorization":{
            "kind":"user","authorization":"fixture pending user grant","granted_by":"user"}})
        });
        map["effects"] = json!([{
            "kind":if case == "name_role" {"name"} else {"register"},
            "role":role,"effect_id":"pending-current-materialization","operation_id":operation,
            "project_scope":scope,"app_scope_id":"appserver-cli","name":name,
            "previous_worker_id":worker,"previous_session_id":THREAD_A,
            "target_worker_id":worker,"target_session_id":THREAD_A,"occupancy_revision":2,
            "source_names":if case == "name_role" {vec![old_name.as_str().unwrap()]} else {Vec::new()},
            "moves_existing_names":false,"pane_anchor":null,"created_ms":1
        }]);
        std::fs::write(&map_path, serde_json::to_vec(&map).unwrap()).unwrap();
        let credential_path = fixture
            .host_state
            .join("identities")
            .join(&worker)
            .join("identity.json");
        let mut credential: Value =
            serde_json::from_slice(&std::fs::read(&credential_path).unwrap()).unwrap();
        credential["runtime"]["runtime_id"] = json!("prior-current-runtime");
        credential["runtime"]["binding_id"] = json!("prior-current-binding");
        credential["runtime"]["session_id"] = json!("prior-agent-session");
        std::fs::write(&credential_path, serde_json::to_vec(&credential).unwrap()).unwrap();
        fixture.start_daemon();
        let restored = fixture.run_public(&["context"], THREAD_A);
        assert_eq!(restored["identity"]["worker_id"], worker, "{case}");
        assert_eq!(restored["name"], name, "{case}");
        assert!(fixture.current_state()["workers"].get(&worker).is_some());
        if case == "name_role" {
            assert_eq!(restored["master"]["worker_id"], worker);
        } else {
            assert!(
                fixture.current_state()["global"]["command_receipts"]
                    .as_object()
                    .unwrap()
                    .values()
                    .any(|receipt| receipt["operation_id"] == operation),
                "pending Register operation was lost"
            );
            assert!(restored["master"].is_null());
        }
        let completed: Value = serde_json::from_slice(&std::fs::read(&map_path).unwrap()).unwrap();
        assert!(
            completed
                .get("effects")
                .is_none_or(|effects| effects.as_array().unwrap().is_empty()),
            "{case}: effect remained pending"
        );
        fixture.run_public(&["context"], THREAD_A);
    }
}
