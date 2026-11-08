fn tmux_session_name(fixture: &Fixture, pane: &Pane) -> String {
    String::from_utf8(
        tmux(
            &fixture.tmux_socket,
            &[
                "display-message",
                "-p",
                "-t",
                pane.pane_id.as_str(),
                "#{session_name}",
            ],
        )
        .stdout,
    )
    .expect("tmux session name is UTF-8")
    .trim()
    .to_owned()
}

fn project_tmux_name(cwd: &Path, name: &str) -> String {
    let slot = if name == "Jenny" {
        1
    } else {
        [
            "Alice", "Bob", "Carol", "David", "Emma", "Frank", "Grace", "Henry", "Iris", "Jack",
            "Kate", "Leo", "Mia", "Noah", "Olivia", "Peter", "Quinn", "Rose", "Sam", "Tina",
        ]
        .iter()
        .position(|candidate| *candidate == name)
        .unwrap()
            + 2
    };
    format!(
        "{}_{}_{}",
        cwd.file_name().unwrap().to_str().unwrap(),
        slot,
        name
    )
}

#[test]
fn identity_map_links_fixed_manifest_position_and_refuses_nonfirst_master_promotion() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("identity-map-layout-pane-five");
    let path = fixture
        .root
        .join("home/.local/state/tm-iterm/manifest.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let manifest = serde_json::to_vec(&json!([{
        "binding_id":"tm_fixed_five", "tab_id":"project_alpha", "pane_order":"5",
        "cwd":fixture.root, "tmux_session":tmux_session_name(&fixture, &pane),
        "profile":"fixed-iterm-profile", "codex_session_id":"old-display-session"
    }, {
        "binding_id":"overview_five", "tab_id":"overview", "pane_order":"1",
        "cwd":fixture.root, "tmux_session":tmux_session_name(&fixture, &pane)
    }]))
    .unwrap();
    fs::write(&path, &manifest).unwrap();
    let registered = fixture.registered_context("layout-pane-five-register", &pane);
    let worker = registered["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap();
    let map_path = fixture.host_state.join("identity-map.json");
    let map: Value = serde_json::from_slice(&fs::read(&map_path).unwrap()).unwrap();
    let current = &map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["current"]["binding"]["agent_id"] == worker)
        .unwrap()["current"];
    assert_eq!(current["layout_binding_id"], "tm_fixed_five");
    assert_eq!(tmux_session_name(&fixture, &pane), "project_alpha_5_Alice");
    let before = fs::read(&map_path).unwrap();
    let refused = fixture.command(
        &[
            "master",
            "promote",
            "--approval",
            "user requests fixed pane five promotion",
        ],
        Some(&pane),
    );
    assert!(
        !refused.status.success(),
        "only fixed position one may become Jenny: {}",
        text(&refused)
    );
    assert!(
        text(&refused).contains("MASTER_SLOT_REQUIRED"),
        "{}",
        text(&refused)
    );
    assert_eq!(tmux_session_name(&fixture, &pane), "project_alpha_5_Alice");
    assert_eq!(
        fs::read(&map_path).unwrap(),
        before,
        "refused promotion cannot publish identity changes"
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        manifest,
        "registration and role changes cannot rewrite fixed layout"
    );
    let restored = fixture.registered_context("layout-pane-five-restore", &pane);
    assert_eq!(
        restored["result"]["snapshot"]["identity"]["worker_id"],
        worker
    );
    assert_eq!(restored["result"]["snapshot"]["name"], "Alice");
}

#[test]
fn identity_map_fixed_first_position_can_promote_without_changing_layout() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("identity-map-layout-pane-one");
    let path = fixture
        .root
        .join("home/.local/state/tm-iterm/manifest.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let manifest = serde_json::to_vec(&json!([{
        "binding_id":"tm_fixed_one", "tab_id":"project_alpha", "pane_order":"1",
        "cwd":fixture.root, "tmux_session":tmux_session_name(&fixture, &pane),
        "profile":"fixed-iterm-profile"
    }, {
        "binding_id":"overview_project_alpha", "tab_id":"overview", "pane_order":"4",
        "cwd":fixture.root, "tmux_session":tmux_session_name(&fixture, &pane),
        "profile":"overview-display-only"
    }]))
    .unwrap();
    fs::write(&path, &manifest).unwrap();
    fixture.registered_context("layout-pane-one-register", &pane);
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user promotes fixed pane one",
        ],
        Some(&pane),
    );
    assert_eq!(tmux_session_name(&fixture, &pane), "project_alpha_1_Jenny");
    assert_eq!(fs::read(&path).unwrap(), manifest);
    let restored = fixture.registered_context("layout-pane-one-restore", &pane);
    assert_eq!(restored["result"]["snapshot"]["name"], "Jenny");
    assert_eq!(
        restored["result"]["snapshot"]["layout"]["binding_id"],
        "tm_fixed_one"
    );
    let worker = restored["result"]["snapshot"]["identity"]["worker_id"].clone();
    let session_id = String::from_utf8(tmux(&fixture.tmux_socket,
        &["display-message", "-p", "-t", &pane.pane_id, "#{session_id}"]).stdout).unwrap();
    tmux(&fixture.tmux_socket, &["rename-session", "-t", session_id.trim(), "unstructured-current-label"]);
    let continued = fixture.registered_context("layout-mutable-name", &pane);
    assert_eq!(continued["result"]["snapshot"]["identity"]["worker_id"], worker);
    assert_eq!(continued["result"]["snapshot"]["layout"]["binding_id"], "tm_fixed_one");
    assert_eq!(tmux_session_name(&fixture, &pane), "project_alpha_1_Jenny");

    // Seed the actual legacy overview-ID condition in this isolated map.
    // The manifest now references the current label, as in the live evidence.
    let mut current_manifest: Value = serde_json::from_slice(&manifest).unwrap();
    for entry in current_manifest.as_array_mut().unwrap() {
        entry["tmux_session"] = json!(tmux_session_name(&fixture, &pane));
    }
    fs::write(&path, serde_json::to_vec(&current_manifest).unwrap()).unwrap();
    fixture.stop_daemon();
    let map_path = fixture.host_state.join("identity-map.json");
    let mut map: Value = serde_json::from_slice(&fs::read(&map_path).unwrap()).unwrap();
    for slot in map["slots"].as_array_mut().unwrap() {
        if slot["current"]["binding"]["agent_id"] == worker {
            slot["current"]["layout_binding_id"] = json!("overview_project_alpha");
        }
    }
    fs::write(&map_path, serde_json::to_vec(&map).unwrap()).unwrap();
    fixture.restart_daemon();
    let corrected = fixture.registered_context("layout-overview-id", &pane);
    assert_eq!(corrected["result"]["snapshot"]["identity"]["worker_id"], worker);
    assert_eq!(corrected["result"]["snapshot"]["name"], "Jenny");
    assert_eq!(corrected["result"]["snapshot"]["layout"]["binding_id"], "tm_fixed_one");
    let stable = fs::read(&map_path).unwrap();
    fixture.registered_context("layout-overview-id-repeat", &pane);
    assert_eq!(fs::read(&map_path).unwrap(), stable);
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(), current_manifest);
}

#[test]
fn identity_map_rejects_duplicate_project_positions_with_or_without_saved_binding() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("identity-map-duplicate-layout");
    let path = fixture.root.join("home/.local/state/tm-iterm/manifest.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::create_dir(fixture.root.join("foreign-project")).unwrap();
    let mut manifest = json!([
        {"binding_id":"project_one","tab_id":"project_alpha","pane_order":1,"cwd":fixture.root,"tmux_session":tmux_session_name(&fixture,&pane)},
        {"binding_id":"overview_one","tab_id":"overview","pane_order":4,"cwd":fixture.root,"tmux_session":tmux_session_name(&fixture,&pane)},
        {"binding_id":"foreign_one","tab_id":"foreign","pane_order":5,"cwd":fixture.root.join("foreign-project"),"tmux_session":tmux_session_name(&fixture,&pane)}
    ]);
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let initial = fixture.registered_context("duplicate-layout-seed", &pane);
    assert_eq!(initial["result"]["snapshot"]["layout"]["binding_id"], "project_one");
    let worker = initial["result"]["snapshot"]["identity"]["worker_id"].clone();
    let physical = tmux_session_name(&fixture,&pane);
    for entry in manifest.as_array_mut().unwrap() { entry["tmux_session"] = json!(physical); }
    let mut duplicate = manifest[0].clone();
    duplicate["binding_id"] = json!("competing_project_five");
    duplicate["pane_order"] = json!(5);
    manifest.as_array_mut().unwrap().push(duplicate);
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let map_path = fixture.host_state.join("identity-map.json");
    for saved in [true, false] {
        if !saved {
            fixture.stop_daemon();
            let mut map: Value = serde_json::from_slice(&fs::read(&map_path).unwrap()).unwrap();
            for slot in map["slots"].as_array_mut().unwrap() {
                if slot["current"]["binding"]["agent_id"] == worker {
                    slot["current"]["layout_binding_id"] = Value::Null;
                }
            }
            fs::write(&map_path, serde_json::to_vec(&map).unwrap()).unwrap();
            fixture.restart_daemon();
        }
        let before = fs::read(&map_path).unwrap();
        let refused = fixture.command(&["master","promote","--approval","fixture user requests project master"],Some(&pane));
        assert!(!refused.status.success());
        assert!(text(&refused).contains("TM_LAYOUT_AMBIGUOUS"),"{}",text(&refused));
        assert_eq!(fs::read(&map_path).unwrap(),before);
        assert_eq!(tmux_session_name(&fixture,&pane),physical);
    }
}

#[test]
fn identity_map_registration_publishes_one_json_control_resource() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("identity-map-register");
    let registered = fixture.registered_context("identity-map-register", &pane);
    let path = fixture.host_state.join("identity-map.json");
    assert!(
        path.is_file(),
        "registration must publish the sole identity map: {}",
        path.display()
    );
    let map: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(map["schema_version"], 1);
    let slots = map["slots"].as_array().expect("one JSON slots array");
    let slot = slots
        .iter()
        .find(|slot| slot["identity_status"] == "active")
        .unwrap();
    assert_eq!(slot["identity_key"]["name"], "Alice");
    assert_eq!(slot["display_slot"], 2);
    assert_eq!(
        slot["current"]["binding"]["agent_id"],
        registered["result"]["snapshot"]["identity"]["worker_id"]
    );
    assert_eq!(
        tmux_session_name(&fixture, &pane),
        format!(
            "{}_2_Alice",
            fixture.root.file_name().unwrap().to_str().unwrap()
        )
    );
    let routes: Vec<Value> =
        serde_json::from_slice(&fs::read(fixture.host_routes()).unwrap()).unwrap();
    assert!(
        routes.iter().all(|route| route.get("kind").is_none()),
        "current routes cannot contain a second identity control set"
    );
}

#[test]
fn identity_map_session_only_binding_does_not_block_daemon_startup() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("identity-map-session-only");
    let first = fixture.registered_context("session-only-seed", &pane);
    let worker = first["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture.stop_daemon();
    let map_path = fixture.host_state.join("identity-map.json");
    let mut map: Value = serde_json::from_slice(&fs::read(&map_path).unwrap()).unwrap();
    let current = map["slots"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|slot| slot["current"]["binding"]["agent_id"] == worker)
        .unwrap();
    let current = &mut current["current"];
    current["binding"]
        .as_object_mut()
        .unwrap()
        .remove("tmux_endpoint");
    current["binding"]
        .as_object_mut()
        .unwrap()
        .remove("native_thread_id");
    current["transport"]
        .as_object_mut()
        .unwrap()
        .remove("tmux_endpoint");
    let binding = current["binding"].clone();
    let transport = current["transport"].clone();
    fs::write(&map_path, serde_json::to_vec(&map).unwrap()).unwrap();
    let path = fixture.project_state();
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let scope = binding["project_scope"].as_str().unwrap();
    let binding_id = binding["binding_id"].as_str().unwrap();
    state["state"]["global"]["projects"][scope]["runtime_bindings"][binding_id] = binding.clone();
    state["state"]["global"]["current_thread_routes"] = json!([]);
    state["state"]["workers"][&worker]["transport"] = transport.clone();
    fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
    let path = fixture
        .host_state
        .join("identities")
        .join(&worker)
        .join("identity.json");
    let mut credential: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    credential["runtime"]
        .as_object_mut()
        .unwrap()
        .remove("native_thread_id");
    credential["transport"] = transport;
    fs::write(path, serde_json::to_vec(&credential).unwrap()).unwrap();

    fixture.restart_daemon();
    let board = lifecycle_wire(&fixture, &credential, json!({"op":"BoardShow"}));
    assert_eq!(board["ok"], true, "{board}");
    let saved: Value = serde_json::from_slice(&fs::read(&map_path).unwrap()).unwrap();
    assert_eq!(saved["slots"][0]["current"]["binding"], binding);
}

#[test]
fn offline_incumbent_name_change_does_not_block_current_messages() {
    let (mut fixture, _old_host, next_host, old, next) =
        lifecycle_native_pair_fixture("offline-name-effect", "ack_only");
    let sender_host = TestAppServer::start_with_effect_at(
        &fixture.root, "sender.sock", "sender-session", "sender-thread", "ack_only",
    );
    let seeded = fixture.context_provide("offline-effect-sender", &json!({
        "session_id":"sender-session", "thread_id":"sender-thread",
        "endpoint":sender_host.endpoint(), "namespace":"codex_tui"
    }), None);
    let sender = lifecycle_identity(&fixture, &seeded);
    let old_worker = old["worker_id"].as_str().unwrap();
    let next_worker = next["worker_id"].as_str().unwrap();
    let queued = fixture.configured_command(&[
        "send", "--to", sender["worker_id"].as_str().unwrap(), "--subject", "retain-current-message",
        "--content", "retain this message through credential write failure",
    ], None)
        .env("CODEX_APP_SERVER_SOCKET", next_host.endpoint().strip_prefix("unix://").unwrap())
        .env("CODEX_SESSION_ID", "lifecycle-child-session")
        .env("CODEX_THREAD_ID", "lifecycle-child-thread")
        .output().unwrap();
    assert!(queued.status.success(), "{}", String::from_utf8_lossy(&queued.stderr));
    let queued: Value = serde_json::from_slice(&queued.stdout).unwrap();
    let queued_id = queued["msg_id"].as_str().unwrap();
    let task = lifecycle_wire(&fixture, &sender, json!({"op":"TaskRegister",
        "worker_id":sender["worker_id"], "token":sender["token"], "task_id":"retained-current-work",
        "owner":null, "feature_id":null, "worktree_path":null, "branch":null,
        "base_commit":null, "priority":"p2", "next_step":"continue the same task"}));
    assert_eq!(task["ok"], true, "{task}");

    fixture.stop_daemon();
    let map_path = fixture.host_state.join("identity-map.json");
    let mut map: Value = serde_json::from_slice(&fs::read(&map_path).unwrap()).unwrap();
    let value = &mut map["slots"].as_array_mut().unwrap().iter_mut()
        .find(|slot| slot["current"]["binding"]["agent_id"] == old_worker).unwrap()["current"];
    // A current retained identity can lack delivery facts. This is not a new
    // registration and must not borrow an old pane to appear reachable.
    value["transport"]["kind"] = json!("tmux");
    value["transport"].as_object_mut().unwrap().remove("tmux_endpoint");
    let transport = value["transport"].clone();
    fs::write(&map_path, serde_json::to_vec(&map).unwrap()).unwrap();
    let state_path = fixture.project_state();
    let mut state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    state["state"]["workers"][old_worker]["transport"] = transport.clone();
    fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    let credential_path = fixture.host_state.join("identities").join(old_worker).join("identity.json");
    let mut credential: Value = serde_json::from_slice(&fs::read(&credential_path).unwrap()).unwrap();
    credential["transport"] = transport;
    fs::write(&credential_path, serde_json::to_vec(&credential).unwrap()).unwrap();
    fixture.restart_daemon();

    let promote = || fixture.configured_command(&[
        "context", "--approve-grant",
        r#"{"action":"restore_master","authorization":"user replaces the offline incumbent","replace_confirmed":true}"#,
    ], None)
        .env("CODEX_APP_SERVER_SOCKET", next_host.endpoint().strip_prefix("unix://").unwrap())
        .env("CODEX_SESSION_ID", "lifecycle-child-session")
        .env("CODEX_THREAD_ID", "lifecycle-child-thread")
        .output().unwrap();
    let retained_credential = credential_path.with_extension("retained");
    fs::rename(&credential_path, &retained_credential).unwrap();
    fs::create_dir(&credential_path).unwrap();
    let refused = promote();
    let failure = format!("{} {}", String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr));
    assert!(!refused.status.success(), "a real credential replacement failure cannot succeed");
    assert!(failure.contains("PENDING_EFFECT_CREDENTIAL_FAILED"), "{failure}");
    assert!(!failure.contains("selected tmux transport has no tmux endpoint"), "{failure}");
    let blocked_recv = fixture.configured_command(&["recv", "--timeout", "1"], None)
        .env("CODEX_APP_SERVER_SOCKET", sender_host.endpoint().strip_prefix("unix://").unwrap())
        .env("CODEX_SESSION_ID", "sender-session")
        .env("CODEX_THREAD_ID", "sender-thread")
        .output().unwrap();
    assert!(!blocked_recv.status.success());
    let pending: Value = serde_json::from_slice(&fs::read(&map_path).unwrap()).unwrap();
    assert!(!pending["effects"].as_array().unwrap().is_empty());
    let pending_state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    assert_ne!(pending_state["state"]["msgs"][queued_id]["state"], "read");
    fs::remove_dir(&credential_path).unwrap();
    fs::rename(&retained_credential, &credential_path).unwrap();
    let promoted = promote();
    drop(promote);
    let sent = fixture.configured_command(&[
        "send", "--to", next_worker, "--subject", "current-after-takeover", "--content",
        "current message must not depend on the offline incumbent address",
    ], None)
        .env("CODEX_APP_SERVER_SOCKET", sender_host.endpoint().strip_prefix("unix://").unwrap())
        .env("CODEX_SESSION_ID", "sender-session")
        .env("CODEX_THREAD_ID", "sender-thread")
        .output().unwrap();
    assert!(sent.status.success(), "current sender was blocked: {} ; takeover: {} {}",
        String::from_utf8_lossy(&sent.stderr), String::from_utf8_lossy(&promoted.stdout),
        String::from_utf8_lossy(&promoted.stderr));
    assert!(promoted.status.success(), "{} {}", String::from_utf8_lossy(&promoted.stdout),
        String::from_utf8_lossy(&promoted.stderr));
    let received = fixture.configured_command(&["recv", "--timeout", "1"], None)
        .env("CODEX_APP_SERVER_SOCKET", next_host.endpoint().strip_prefix("unix://").unwrap())
        .env("CODEX_SESSION_ID", "lifecycle-child-session")
        .env("CODEX_THREAD_ID", "lifecycle-child-thread")
        .output().unwrap();
    assert!(received.status.success(), "{}", String::from_utf8_lossy(&received.stderr));
    let received: Value = serde_json::from_slice(&received.stdout).unwrap();
    assert!(received["messages"].as_array().unwrap().iter()
        .any(|message| message["subject"] == "current-after-takeover"));
    let saved: Value = serde_json::from_slice(&fs::read(&map_path).unwrap()).unwrap();
    assert!(saved["effects"].as_array().unwrap().is_empty());
    let retained = saved["slots"].as_array().unwrap().iter().find(|slot|
        slot["identity_status"] == "active" && slot["current"]["binding"]["agent_id"] == old_worker
    ).expect("the offline identity must remain in the current map");
    assert_ne!(retained["identity_key"]["name"], "Jenny");
    assert!(retained["current"]["transport"].get("tmux_endpoint").is_none());
    let master = lifecycle_wire(&fixture, &next, json!({"op":"MasterStatus"}));
    assert_eq!(master["master"]["worker_id"], next["worker_id"]);
    let workers = lifecycle_wire(&fixture, &next, json!({"op":"Workers"}));
    let old_view = workers["workers"].as_array().unwrap().iter()
        .find(|worker| worker["id"] == old_worker).unwrap();
    assert_ne!(old_view["endpoint_live"], true);
    let task = lifecycle_wire(&fixture, &sender, json!({"op":"TaskStatus","task_id":"retained-current-work"}));
    assert_eq!(task["ok"], true, "{task}");
    assert_eq!(task["owner"], sender["worker_id"]);
    assert_eq!(task["status"], "working");
    let sender_recv = fixture.configured_command(&["recv", "--timeout", "1"], None)
        .env("CODEX_APP_SERVER_SOCKET", sender_host.endpoint().strip_prefix("unix://").unwrap())
        .env("CODEX_SESSION_ID", "sender-session")
        .env("CODEX_THREAD_ID", "sender-thread")
        .output().unwrap();
    assert!(sender_recv.status.success(), "{}", String::from_utf8_lossy(&sender_recv.stderr));
    let consumed: Value = serde_json::from_slice(&sender_recv.stdout).unwrap();
    assert!(consumed["messages"].as_array().unwrap().iter().any(|message| message["id"] == queued_id));
    let receive_id = consumed["receive_id"].as_str().unwrap();
    fixture.restart_daemon();
    let replayed = fixture.configured_command(&["recv", "--receive-id", receive_id], None)
        .env("CODEX_APP_SERVER_SOCKET", sender_host.endpoint().strip_prefix("unix://").unwrap())
        .env("CODEX_SESSION_ID", "sender-session")
        .env("CODEX_THREAD_ID", "sender-thread")
        .output().unwrap();
    assert!(replayed.status.success(), "{}", String::from_utf8_lossy(&replayed.stderr));
    let replayed: Value = serde_json::from_slice(&replayed.stdout).unwrap();
    assert_eq!(replayed["replayed"], true);
    assert_eq!(replayed["messages"], consumed["messages"]);
    let missing = fixture.command(&["context", "--provide",
        r#"{"session_id":"new-session-without-address","thread_id":"new-thread-without-address","namespace":"codex_tui"}"#,
    ], None);
    assert_eq!(missing.status.code(), Some(2), "{} {}", String::from_utf8_lossy(&missing.stdout),
        String::from_utf8_lossy(&missing.stderr));
    let mut forged = sender.clone();
    forged["token"] = json!("wrong-token");
    let refused = lifecycle_wire(&fixture, &forged, json!({"op":"Inbox",
        "worker_id":sender["worker_id"],"token":"wrong-token"}));
    assert_eq!(refused["ok"], false);
}

#[test]
fn identity_map_corruption_is_not_ignored_for_history_recovery() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("identity-map-corrupt");
    fixture.registered_context("identity-map-corrupt-seed", &pane);
    let path = fixture.host_state.join("identity-map.json");
    fs::write(&path, b"{broken-map").unwrap();
    let result = run_context_cli(
        &fixture,
        Some(&pane),
        Some("identity-map-corrupt-restore"),
        None,
        None,
        None,
        None,
        None,
        false,
    );
    assert!(
        !result.status.success(),
        "map corruption cannot be ignored in favor of old state: {}",
        text(&result)
    );
    assert!(
        text(&result).contains("identity-map.json"),
        "{}",
        text(&result)
    );
    assert_eq!(fs::read(&path).unwrap(), b"{broken-map");
}

#[test]
fn identity_map_role_change_and_restore_survive_restart() {
    let (mut fixture, host, original) =
        lifecycle_native_fixture_effect("map-no-journal", "ack_only");
    let promoted = fixture
        .configured_command(
            &[
                "master",
                "promote",
                "--approval",
                "current user authorization",
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
        promoted.status.success(),
        "{}",
        String::from_utf8_lossy(&promoted.stderr)
    );
    fixture.restart_daemon();
    let restored = fixture.context_provide(
        "map-no-journal-restore",
        &json!({
            "session_id":"lifecycle-session", "thread_id":"lifecycle-thread",
            "endpoint":host.endpoint(), "namespace":"codex_tui"
        }),
        None,
    );
    assert_eq!(
        restored["result"]["snapshot"]["identity"]["worker_id"],
        original["worker_id"]
    );
    assert_eq!(restored["result"]["snapshot"]["name"], "Jenny");
}

#[test]
fn current_state_restart_preserves_identity_roles_messages_and_open_tasks() {
    let (mut fixture, master_host, child_host, master, child) =
        lifecycle_native_pair_fixture("current-state-restart", "ack_only");
    let native = |args: &[&str], host: &TestAppServer, session: &str, thread: &str| {
        fixture
            .configured_command(args, None)
            .env(
                "CODEX_APP_SERVER_SOCKET",
                host.endpoint().strip_prefix("unix://").unwrap(),
            )
            .env("CODEX_SESSION_ID", session)
            .env("CODEX_THREAD_ID", thread)
            .output()
            .unwrap()
    };
    let recipient = child["worker_id"].as_str().unwrap();
    let first = native(
        &[
            "send",
            "--to",
            recipient,
            "--subject",
            "current-consumed",
            "consumed-current-message",
        ],
        &master_host,
        "lifecycle-master-session",
        "lifecycle-master-thread",
    );
    assert!(
        first.status.success(),
        "send must succeed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    let first_id = first["msg_id"].as_str().unwrap();
    let ack = lifecycle_wire(
        &fixture,
        &child,
        json!({"op":"Ack", "worker_id":recipient,
        "token":child["token"], "ids":[first_id]}),
    );
    assert_eq!(ack["ok"], true);
    let second = native(
        &[
            "send",
            "--to",
            recipient,
            "--subject",
            "current-pending",
            "pending-current-message",
        ],
        &master_host,
        "lifecycle-master-session",
        "lifecycle-master-thread",
    );
    assert!(
        second.status.success(),
        "send must succeed: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second: Value = serde_json::from_slice(&second.stdout).unwrap();
    let second_id = second["msg_id"].as_str().unwrap();

    // A real replacement failure must leave the rejected task out of memory.
    let state_path = fixture.project_state();
    let retained = state_path.with_extension("retained");
    fs::rename(&state_path, &retained).unwrap();
    fs::create_dir(&state_path).unwrap();
    let refused = native(
        &[
            "task",
            "register",
            "rejected-current-task",
            "--next",
            "must remain absent",
        ],
        &child_host,
        "lifecycle-child-session",
        "lifecycle-child-thread",
    );
    fs::remove_dir(&state_path).unwrap();
    fs::rename(&retained, &state_path).unwrap();
    assert!(
        !refused.status.success(),
        "a failed current-state replacement cannot succeed"
    );
    let task = native(
        &[
            "task",
            "register",
            "open-current-task",
            "--next",
            "continue current work",
        ],
        &child_host,
        "lifecycle-child-session",
        "lifecycle-child-thread",
    );
    assert!(
        task.status.success(),
        "task must register: {}",
        String::from_utf8_lossy(&task.stderr)
    );
    let current: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    assert!(
        current["state"]["tasks"]
            .get("rejected-current-task")
            .is_none(),
        "a rejected mutation must not leak into the next successful save"
    );
    assert_eq!(
        current["state"]["tasks"]["open-current-task"]["owner"],
        recipient
    );
    assert_eq!(current["state"]["msgs"][first_id]["state"], "read");
    assert_ne!(current["state"]["msgs"][second_id]["state"], "read");
    let mut revision = current["state"]["board_details"]["open-current-task"]["revision"]
        .as_u64()
        .unwrap();
    for description in ["superseded-board-description", "latest-board-description"] {
        let described = lifecycle_wire(
            &fixture,
            &child,
            json!({"op":"Board", "worker_id":recipient, "token":child["token"],
                "command":{"action":"describe", "id":"open-current-task",
                    "expected_revision":revision, "title":"Current board task",
                    "description":description, "delivery_condition":"Retain the latest task",
                    "test_condition":"Read the same task after restart"}}),
        );
        assert_eq!(described["ok"], true, "{described}");
        revision = described["task"]["revision"].as_u64().unwrap();
    }
    for next in ["superseded-board-progress", "latest-board-progress"] {
        let updated = lifecycle_wire(
            &fixture,
            &child,
            json!({"op":"Board", "worker_id":recipient, "token":child["token"],
                "command":{"action":"update", "id":"open-current-task",
                    "expected_revision":revision, "status":null, "next":next}}),
        );
        assert_eq!(updated["ok"], true, "{updated}");
        revision = updated["task"]["revision"].as_u64().unwrap();
    }
    let saved = fs::read_to_string(&state_path).unwrap();
    assert!(!saved.contains("superseded-board-description"));
    assert!(!saved.contains("superseded-board-progress"));
    drop(native);

    fixture.stop_daemon();
    let mut obsolete_fields: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    obsolete_fields["state"]["migration"] = json!({"phase":"applied", "admission_frozen":true});
    obsolete_fields["state"]["global"]["migration_commit_evidence"] = json!({"unused":"not a current proof"});
    fs::write(&state_path, serde_json::to_vec(&obsolete_fields).unwrap()).unwrap();
    fixture.start_daemon();
    let restored = fixture.context_provide(
        "current-child-restore",
        &json!({
            "session_id":"lifecycle-child-session", "thread_id":"lifecycle-child-thread",
            "endpoint":child_host.endpoint(), "namespace":"codex_tui"
        }),
        None,
    );
    assert_eq!(
        restored["result"]["snapshot"]["identity"]["worker_id"],
        child["worker_id"]
    );
    let current_identity = lifecycle_identity(&fixture, &restored);
    assert!(
        current_identity["token"] == child["token"],
        "restart must preserve the current credential"
    );
    let inbox = lifecycle_wire(
        &fixture,
        &current_identity,
        json!({"op":"Inbox",
        "worker_id":recipient, "token":current_identity["token"]}),
    );
    assert_eq!(inbox["ok"], true);
    let messages = inbox["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| message["id"] == second_id));
    assert!(!messages.iter().any(|message| message["id"] == first_id));
    let master_restored = fixture.context_provide(
        "current-master-restore",
        &json!({
            "session_id":"lifecycle-master-session", "thread_id":"lifecycle-master-thread",
            "endpoint":master_host.endpoint(), "namespace":"codex_tui"
        }),
        None,
    );
    assert_eq!(master_restored["result"]["snapshot"]["name"], "Jenny");
    assert_eq!(
        master_restored["result"]["snapshot"]["identity"]["worker_id"],
        master["worker_id"]
    );
    let current: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(
        current["state"]["tasks"]["open-current-task"]["status"],
        "working"
    );
    assert_eq!(current["state"]["msgs"][first_id]["state"], "read");
    let board = lifecycle_wire(&fixture, &current_identity, json!({"op":"BoardShow"}));
    assert_eq!(board["ok"], true, "{board}");
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "open-current-task")
        .expect("the current task must remain on the public board after restart");
    assert_eq!(task["title"], "Current board task");
    assert_eq!(task["description"], "latest-board-description");
    assert_eq!(task["next_step"], "latest-board-progress");
    assert_eq!(task["revision"], revision);
    let updated = lifecycle_wire(
        &fixture,
        &current_identity,
        json!({"op":"TaskUpdate", "worker_id":recipient, "token":current_identity["token"],
            "task_id":"open-current-task", "status":"verifying", "next_step":"current mutation after restart"}),
    );
    assert_eq!(updated["ok"], true, "obsolete inner fields must not freeze current admission: {updated}");
    let saved: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    assert!(saved["state"].get("migration").is_none());
    assert!(saved["state"]["global"].get("migration_commit_evidence").is_none());
    assert_eq!(saved["state"]["tasks"]["open-current-task"]["status"], "verifying");
    assert_eq!(saved["state"]["msgs"][first_id]["state"], "read");
    assert_ne!(saved["state"]["msgs"][second_id]["state"], "read");
}

#[test]
fn identity_map_missing_does_not_recover_history_or_transfer_its_work() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("identity-map-missing-work");
    let first = fixture.registered_context("identity-map-missing-work-seed", &pane);
    let old = first["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    fixture.run_ok(
        &[
            "task",
            "register",
            "identity-map-original-task",
            "--next",
            "keep original owner",
        ],
        Some(&pane),
    );
    fs::remove_file(fixture.host_state.join("identity-map.json")).unwrap();
    let restored = success_payload(&context_with_agent_session(
        &fixture,
        &fixture.root,
        "identity-map-missing-work-new-registration",
        &pane,
    ));
    let new = restored["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap();
    assert_ne!(new, old, "a missing map cannot restore a historical worker");
    assert!(restored["result"]["snapshot"]["master"].is_null());
    let task = fixture.run_ok(
        &["task", "status", "identity-map-original-task"],
        Some(&pane),
    );
    assert_eq!(task["owner"], old,
        "fresh registration without a map must not use old business occupancy to inherit work: {task}");
}

#[test]
fn project_prefixed_names_coexist_and_restore_on_one_tmux_server() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, first_pane) = Fixture::isolated("project-prefix");
    let second_root = fixture.root.join("other-project");
    seed_baseline(&second_root);
    let second_id = String::from_utf8(
        tmux(
            &fixture.tmux_socket,
            &[
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-s",
                "second-project",
                "sleep 600",
            ],
        )
        .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    fixture.track_tmux_session("second-project");
    let second_pane = Pane {
        server_pid: first_pane.server_pid,
        pane_id: second_id,
        session_anchor: "second-project-session".into(),
        thread_anchor: "second-project-thread".into(),
    };
    let first = registered_context_in(&fixture, &fixture.root, "prefix-first", &first_pane);
    let second = registered_context_in(&fixture, &second_root, "prefix-second", &second_pane);
    assert_eq!(first["result"]["snapshot"]["name"], "Alice");
    assert_eq!(second["result"]["snapshot"]["name"], "Alice");
    assert_eq!(
        tmux_session_name(&fixture, &first_pane),
        project_tmux_name(&fixture.root, "Alice")
    );
    assert_eq!(
        tmux_session_name(&fixture, &second_pane),
        "other-project_2_Alice"
    );
    for (root, pane, label) in [
        (&fixture.root, &first_pane, "first-promote"),
        (&second_root, &second_pane, "second-promote"),
    ] {
        let output = current_pane_command(
            &fixture,
            root,
            label,
            pane,
            &[
                "master",
                "promote",
                "--approval",
                "user approved isolated master",
            ],
        );
        assert!(output.status.success(), "{}", text(&output));
    }
    let first_physical = project_tmux_name(&fixture.root, "Jenny");
    assert_eq!(tmux_session_name(&fixture, &first_pane), first_physical);
    assert_eq!(
        tmux_session_name(&fixture, &second_pane),
        "other-project_1_Jenny"
    );
    let restored = pane_identity_context(
        &fixture,
        &first_pane,
        "prefix-restore",
        &fixture.root,
        "prefix-successor-session",
        "prefix-successor-thread",
        "prefix-restore",
    );
    assert_eq!(restored["result"]["snapshot"]["name"], "Jenny");
    assert_eq!(
        restored["result"]["snapshot"]["identity"]["worker_id"],
        first["result"]["snapshot"]["identity"]["worker_id"]
    );
    assert_eq!(
        tmux_session_name(&fixture, &second_pane),
        "other-project_1_Jenny"
    );
    // A foreign physical prefix cannot select this project's Jenny.
    tmux(
        &fixture.tmux_socket,
        &[
            "rename-session",
            "-t",
            &second_pane.pane_id,
            "foreign_Jenny",
        ],
    );
    let restored_second = pane_identity_context(
        &fixture,
        &second_pane,
        "prefix-second-restore",
        &second_root,
        "second-project-session",
        "second-project-thread",
        "prefix-second-restore",
    );
    assert_eq!(restored_second["result"]["snapshot"]["name"], "Jenny");
    assert_eq!(
        tmux_session_name(&fixture, &second_pane),
        "other-project_1_Jenny"
    );
    assert_eq!(tmux_session_name(&fixture, &first_pane), first_physical);
}

fn route_records(content: &str) -> Vec<Value> {
    serde_json::from_str(content).expect("current route array is JSON")
}

#[test]
fn public_context_fixes_the_tmux_session_name_and_restores_by_it() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("session-name-anchor");
    let original = tmux_session_name(&fixture, &pane);
    assert_ne!(original, "Alice");

    let first = success_payload(&run_context_cli(
        &fixture,
        Some(&pane),
        Some("ctxop-session-name-first"),
        None,
        None,
        None,
        None,
        None,
        false,
    ));
    let snapshot = &first["result"]["snapshot"];
    let role = snapshot["name"]
        .as_str()
        .expect("first context assigns a role name")
        .to_owned();
    let worker = snapshot["identity"]["worker_id"]
        .as_str()
        .expect("first context returns a worker id")
        .to_owned();
    let physical = project_tmux_name(&fixture.root, &role);
    assert_eq!(tmux_session_name(&fixture, &pane), physical);

    let repeated = success_payload(&run_context_cli(
        &fixture,
        Some(&pane),
        Some("ctxop-session-name-first"),
        None,
        None,
        None,
        None,
        None,
        false,
    ));
    assert_eq!(repeated["result"]["snapshot"]["name"], role);
    assert_eq!(
        repeated["result"]["snapshot"]["identity"]["worker_id"],
        worker
    );
    assert_eq!(tmux_session_name(&fixture, &pane), physical);

    // Legacy bare names help recovery but must migrate to the scoped label.
    tmux(
        &fixture.tmux_socket,
        &["rename-session", "-t", &pane.pane_id, &role],
    );

    fixture.restart_daemon();
    let resumed_pane = Pane {
        session_anchor: "session-after-daemon-restart".into(),
        thread_anchor: "thread-after-daemon-restart".into(),
        ..pane.clone()
    };
    let resumed = success_payload(&run_context_cli(
        &fixture,
        Some(&resumed_pane),
        Some("ctxop-session-name-restart"),
        None,
        None,
        None,
        None,
        None,
        false,
    ));
    assert_eq!(
        resumed["result"]["snapshot"]["identity"]["worker_id"],
        worker
    );
    assert_eq!(resumed["result"]["snapshot"]["name"], role);
    assert_eq!(tmux_session_name(&fixture, &pane), physical);

    let moved_pane_id = String::from_utf8(
        tmux(
            &fixture.tmux_socket,
            &[
                "split-window",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-t",
                pane.pane_id.as_str(),
                "sleep 600",
            ],
        )
        .stdout,
    )
    .expect("split pane id is UTF-8")
    .trim()
    .to_owned();
    fixture.start_pane_runner_on_socket(&fixture.tmux_socket, &moved_pane_id, "sleep 600");
    let moved_pane = Pane {
        pane_id: moved_pane_id,
        session_anchor: "session-after-pane-move".into(),
        thread_anchor: "thread-after-pane-move".into(),
        ..pane.clone()
    };
    assert_eq!(tmux_session_name(&fixture, &moved_pane), physical);
    let moved = success_payload(&run_context_cli(
        &fixture,
        Some(&moved_pane),
        Some("ctxop-session-name-pane-move"),
        None,
        None,
        None,
        None,
        None,
        false,
    ));
    assert_eq!(moved["result"]["snapshot"]["identity"]["worker_id"], worker);
    assert_eq!(moved["result"]["snapshot"]["name"], role);
}

#[test]
fn public_context_rename_failure_keeps_the_original_session_and_name_uncommitted() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("session-name-conflict");
    let original = tmux_session_name(&fixture, &pane);
    let occupied = project_tmux_name(&fixture.root, "Alice");
    tmux(
        &fixture.tmux_socket,
        &["new-session", "-d", "-s", &occupied, "sleep 600"],
    );
    fixture.track_tmux_session(&occupied);
    assert!(!fixture.host_state.join("identities").exists());
    assert!(!fixture.host_state.join("identity-map.json").exists());

    let output = run_context_cli(
        &fixture,
        Some(&pane),
        Some("ctxop-session-name-conflict"),
        None,
        None,
        None,
        None,
        None,
        false,
    );
    assert!(!output.status.success(), "{}", text(&output));
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    let response = stderr.lines().find_map(|line| line.strip_prefix("collab response: "))
        .expect("rename refusal carries the daemon's typed error");
    let typed: Value = serde_json::from_str(response).unwrap();
    assert_eq!(typed["ok"], false);
    assert_eq!(
        typed["error"],
        format!("TMUX_SESSION_RENAME_FAILED: duplicate session: {occupied}"),
        "{}",
        text(&output)
    );
    assert_eq!(tmux_session_name(&fixture, &pane), original);
    assert!(!fixture.host_state.join("identity-map.json").exists(),
        "a failed physical rename cannot publish the first identity");
    let identities = fixture.host_state.join("identities");
    if identities.exists() {
        for directory in fs::read_dir(identities).unwrap() {
            assert!(!directory.unwrap().path().join("identity.json").exists(),
                "failed registration retained its staged credential");
        }
    }
    let state = fixture.current_state();
    for field in ["workers", "notification_subscriptions", "tasks", "msgs", "receive_receipts"] {
        assert!(state[field].as_object().unwrap().is_empty(), "rename refusal must leave {field} empty");
    }
    assert!(state["global"]["projects"].as_object().unwrap().values().all(|project|
        project["runtime_bindings"].as_object().unwrap().is_empty()));
    assert!(state["global"]["current_thread_routes"].as_array().unwrap().is_empty());
    assert_eq!(String::from_utf8(tmux(&fixture.tmux_socket,
        &["display-message", "-p", "-t", &occupied, "#{session_name}"]).stdout).unwrap().trim(), occupied);
}

#[test]
fn public_context_refresh_rename_failure_preserves_the_current_binding() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, first) = Fixture::isolated("session-refresh-name-conflict");
    let registered = success_payload(&run_context_cli(&fixture, Some(&first),
        Some("refresh-conflict-register"), None, None, None, None, None, false));
    let worker = registered["result"]["snapshot"]["identity"]["worker_id"].as_str().unwrap();
    let occupied = tmux_session_name(&fixture, &first);
    let mut moved = second_pane(&fixture, "refresh-conflict-incoming", first.server_pid);
    moved.session_anchor = first.session_anchor.clone();
    fixture.start_pane_runner_on_socket(&fixture.tmux_socket, &moved.pane_id, "sleep 600");
    let previous = tmux_session_name(&fixture, &moved);
    let baseline = fixture.current_resources();
    let output = run_context_cli(&fixture, Some(&moved), Some("refresh-conflict-current"),
        None, None, None, None, None, false);
    assert!(!output.status.success(), "{}", text(&output));
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    let response = stderr.lines().find_map(|line| line.strip_prefix("collab response: "))
        .expect("refresh rename refusal must carry the original daemon error");
    let response: Value = serde_json::from_str(response).unwrap();
    assert_eq!(response["error"], format!("TMUX_SESSION_RENAME_FAILED: duplicate session: {occupied}"));
    assert_eq!(tmux_session_name(&fixture, &first), occupied);
    assert_eq!(tmux_session_name(&fixture, &moved), previous);
    assert_eq!(fixture.current_resources(), baseline,
        "failed refresh rename must preserve the mapped worker {worker}, credential and business state");
}

#[test]
fn public_context_bootstraps_an_empty_resident_route() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("resident-route-empty");
    let root = canonical_root(&fixture);
    let output = fixture.command(
        &["context", "--op", "ctxop-resident-route-empty"],
        Some(&pane),
    );
    assert!(
        output.status.success(),
        "fresh public context must bootstrap the resident route: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let records = fs::read_to_string(fixture.host_routes())
        .expect("fresh public context must publish current routes");
    let records = route_records(&records);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["project_scope"], root);
    assert_eq!(records[0]["canonical_root"], root);
    let storage = std::path::Path::new(records[0]["storage_root"].as_str().unwrap());
    assert!(storage.starts_with(
        std::fs::canonicalize(&fixture.host_state)
            .unwrap()
            .join("projects")
    ));
    assert!(storage.join(".agent-collab/server/state.json").is_file());
}

#[test]
fn public_context_registers_a_non_git_workspace_without_existing_baseline() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("non-git-bootstrap");
    let root = canonical_root(&fixture);
    fs::remove_dir_all(fixture.root.join(".agent-collab"))
        .expect("remove seeded baseline to exercise first registration");

    let output = fixture.command(&["context", "--op", "ctxop-non-git-bootstrap"], Some(&pane));
    let payload = success_payload(&output);
    assert_eq!(payload["bootstrap"]["project_root_resolution"], "cwd");
    assert_eq!(payload["bootstrap"]["baseline_created"], true);
    assert_eq!(payload["bootstrap"]["registered"], true);
    assert!(fixture.root.join(".agent-collab").is_dir());

    let records = fs::read_to_string(fixture.host_routes())
        .expect("first registration must publish current routes");
    let records = route_records(&records);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["project_scope"], root);
    assert_eq!(records[0]["canonical_root"], root);
}

#[test]
fn public_context_keeps_query_fail_closed_in_non_git_workspace_without_baseline() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("non-git-query");
    fs::remove_dir_all(fixture.root.join(".agent-collab"))
        .expect("remove seeded baseline to exercise query without baseline");

    let output = fixture.command(
        &["context", "--query", "--op", "ctxop-non-git-query"],
        Some(&pane),
    );
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("COLLAB_CONTEXT_UNRESOLVED:"),
        "{}",
        text(&output)
    );
    assert!(
        !fixture.root.join(".agent-collab").exists(),
        "a query must not create a baseline in a non-Git workspace"
    );
}

#[test]
fn public_context_keeps_git_subdirectories_fail_closed_without_route_or_baseline() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("non-git-git-subdir");
    git_init_main(&fixture.root);
    fs::remove_dir_all(fixture.root.join(".agent-collab"))
        .expect("remove seeded baseline to exercise first registration");
    let subdir = fixture.root.join("nested");
    fs::create_dir(&subdir).expect("create Git subdirectory");

    let output = fixture
        .command_on_socket(
            &fixture.tmux_socket,
            &["context", "--op", "ctxop-git-subdir"],
            &pane,
        )
        .current_dir(&subdir)
        .output()
        .expect("run context from Git subdirectory");
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("COLLAB_CONTEXT_UNRESOLVED:"),
        "{}",
        text(&output)
    );
    assert!(
        !subdir.join(".agent-collab").exists(),
        "a failed Git subdirectory resolution must not create a baseline"
    );
}

#[test]
fn public_context_keeps_plain_directories_fail_closed_without_route_or_baseline() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("plain-dir");
    let worktree = fixture.root.join("parent/plain-dir/task");
    fs::create_dir_all(&worktree).expect("create plain directory");

    let output = fixture
        .command_on_socket(
            &fixture.tmux_socket,
            &["context", "--op", "ctxop-plain-dir"],
            &pane,
        )
        .current_dir(&worktree)
        .output()
        .expect("run context from plain directory");
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("COLLAB_CONTEXT_UNRESOLVED"),
        "{}",
        text(&output)
    );
    assert!(
        !worktree.join(".agent-collab").exists(),
        "a failed resolution must not create a baseline"
    );
}

#[test]
fn public_context_rebuilds_a_missing_resident_route_after_restart() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("resident-route-bootstrap");
    let seed = fixture.registered_context("ctxop-resident-route-seed", &pane);
    let worker_id = seed["result"]["snapshot"]["identity"]["worker_id"].clone();
    let root = canonical_root(&fixture);

    fixture.stop_daemon();
    fs::remove_file(fixture.host_routes()).expect("remove isolated current route file");
    fixture.restart_daemon();

    let recovered = success_payload(&fixture.command(
        &["context", "--op", "ctxop-resident-route-recover"],
        Some(&pane),
    ));
    assert_eq!(
        recovered["result"]["snapshot"]["identity"]["worker_id"],
        worker_id
    );
    let records = fs::read_to_string(fixture.host_routes())
        .expect("public context must recreate current routes");
    let records = route_records(&records);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["project_scope"], root);
    assert_eq!(records[0]["canonical_root"], root);
    let storage = std::path::Path::new(records[0]["storage_root"].as_str().unwrap());
    assert!(storage.starts_with(
        std::fs::canonicalize(&fixture.host_state)
            .unwrap()
            .join("projects")
    ));
    assert!(storage.join(".agent-collab/server/state.json").is_file());

    fixture.restart_daemon();
    let replay = success_payload(&fixture.command(
        &["context", "--op", "ctxop-resident-route-recover"],
        Some(&pane),
    ));
    assert_eq!(
        replay["result"]["snapshot"]["identity"]["worker_id"],
        worker_id
    );
}

#[test]
fn persisted_context_uses_daemon_observation_without_adjacent_admission() {
    use std::os::unix::fs::PermissionsExt;
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (mut fixture, pane) = Fixture::isolated("restore-offline");
    let seed = fixture.registered_context("ctxop-offline-seed", &pane);
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user assigned offline master",
        ],
        Some(&pane),
    );
    let master = master_status(&fixture, &pane);
    let binding = seed["result"]["snapshot"]["binding"].clone();
    let worker = seed["result"]["snapshot"]["identity"]["worker_id"].as_str().unwrap();
    let credential = fixture.host_state.join("identities").join(worker).join("identity.json");
    let original_credential: Value = serde_json::from_slice(&fs::read(&credential).unwrap()).unwrap();
    let original_state = fixture.current_state();
    let original_worker = original_state["workers"][worker].clone();
    let original_grants = original_state["global"]["projects"][canonical_root(&fixture)]["master_grants"].clone();
    let original_map: Value = serde_json::from_slice(&fs::read(fixture.host_state.join("identity-map.json")).unwrap()).unwrap();
    let original_slot = original_map["slots"].as_array().unwrap().iter().find(|slot|
        slot["identity_status"] == "active" && slot["current"]["binding"]["agent_id"] == worker).unwrap().clone();
    let subscription_routes = |state: &Value| state["notification_subscriptions"].as_object().unwrap().iter()
        .map(|(id, subscription)| json!([id, subscription["worker_id"], subscription["target"], subscription["method"]])).collect::<Vec<_>>();
    let original_subscriptions = subscription_routes(&original_state);
    let wrapper_dir = fixture.root.join("failed-display");
    fs::create_dir(&wrapper_dir).unwrap();
    let wrapper = wrapper_dir.join("tmux");
    fs::write(
        &wrapper,
        "#!/bin/sh\necho 'fixture display/capture I/O failed' >&2\nexit 71\n",
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    for step in 0..2 {
        if step == 1 {
            fixture.restart_daemon();
        }
        let op = format!("ctxop-offline-{step}");
        let output = fixture
            .configured_command(&["context", "--op", &op], Some(&pane))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    wrapper_dir.display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env(
                "CODEX_APP_SERVER_SOCKET",
                "/tmp/unavailable-adjacent-adaptor.sock",
            )
            .output()
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("fixture display/capture I/O failed"),
            "caller PATH must not replace the daemon's placement observer"
        );
        let result = success_payload(&output);
        assert_eq!(result["result"]["outcome"], "completed", "{result}");
        assert_eq!(result["result"]["snapshot"]["binding"], binding);
        assert_eq!(
            result["result"]["snapshot"]["master"]["worker_id"],
            master["worker_id"]
        );
        assert!(result["result"]["nested_command_id"].is_null());
        assert_eq!(serde_json::from_slice::<Value>(&fs::read(&credential).unwrap()).unwrap(), original_credential);
        let state = fixture.current_state();
        assert_eq!(state["workers"][worker], original_worker);
        assert_eq!(state["global"]["projects"][canonical_root(&fixture)]["master_grants"], original_grants);
        assert_eq!(subscription_routes(&state), original_subscriptions);
        let map: Value = serde_json::from_slice(&fs::read(fixture.host_state.join("identity-map.json")).unwrap()).unwrap();
        assert!(map["slots"].as_array().unwrap().contains(&original_slot));
        assert_eq!(master_status(&fixture, &pane), master);
    }
}

fn same_session_pane(fixture: &Fixture, label: &str, server_pid: u32, base: &Pane) -> Pane {
    let session_id = String::from_utf8(
        tmux(
            &fixture.tmux_socket,
            &[
                "display-message",
                "-p",
                "-t",
                &base.pane_id,
                "#{session_id}",
            ],
        )
        .stdout,
    )
    .expect("tmux session id is UTF-8")
    .trim()
    .to_owned();
    let output = tmux(
        &fixture.tmux_socket,
        &[
            "new-window",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-t",
            &session_id,
            "sleep 600",
        ],
    );
    let pane_id = String::from_utf8(output.stdout)
        .expect("same-session pane id is UTF-8")
        .trim()
        .to_owned();
    Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: format!("session-{label}-{pane_id}"),
        thread_anchor: format!("thread-{label}-{pane_id}"),
    }
}

fn registered_context_in(
    fixture: &Fixture,
    project_root: &Path,
    operation_id: &str,
    pane: &Pane,
) -> Value {
    let done = fixture
        .root
        .join(format!("{operation_id}.participant-done"));
    let result_path = fixture
        .root
        .join(format!("{operation_id}.participant-result"));
    let script = format!(
        "cd {}\n\"$COLLAB_BINARY\" context --op {} >{} 2>\"$PANE_STDERR\"\nprintf done >{}\nsleep 600\n",
        shell_quote(&project_root.to_string_lossy()),
        shell_quote(operation_id),
        shell_quote(&result_path.to_string_lossy()),
        shell_quote(&done.to_string_lossy())
    );
    let consumer = fixture.run_in_pane(pane, operation_id, &script);
    assert!(
        wait_until(Duration::from_secs(15), || done.exists()),
        "external project participant did not complete: {} {}",
        consumer.output(),
        std::fs::read_to_string(&result_path).unwrap_or_default()
    );
    let result: Value = serde_json::from_slice(
        &std::fs::read(result_path).expect("read external participant result"),
    )
    .expect("external participant emits JSON");
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["result"]["outcome"], "completed", "{result}");
    result
}

fn context_with_agent_session(
    fixture: &Fixture,
    project_root: &Path,
    operation_id: &str,
    pane: &Pane,
) -> std::process::Output {
    current_pane_command(
        fixture,
        project_root,
        operation_id,
        pane,
        &["context", "--op", operation_id],
    )
}

fn current_pane_command(
    fixture: &Fixture,
    project_root: &Path,
    label: &str,
    pane: &Pane,
    args: &[&str],
) -> std::process::Output {
    use std::os::unix::process::ExitStatusExt;
    let done = fixture.root.join(format!("{label}.exit"));
    let result = fixture.root.join(format!("{label}.result"));
    if done.exists() {
        fs::remove_file(&done).unwrap();
    }
    let command = args
        .iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        "cd {}\nunset CODEX_THREAD_ID\n\"$COLLAB_BINARY\" {} >{} 2>\"$PANE_STDERR\"\nprintf '%s' \"$?\" >{}\nsleep 600\n",
        shell_quote(&project_root.to_string_lossy()), command,
        shell_quote(&result.to_string_lossy()), shell_quote(&done.to_string_lossy()),
    );
    let consumer = fixture.run_in_pane(pane, label, &script);
    assert!(
        wait_until(Duration::from_secs(15), || fs::read_to_string(&done)
            .is_ok_and(|value| !value.is_empty())),
        "pane command did not finish: {}",
        consumer.output()
    );
    let code: i32 = fs::read_to_string(done).unwrap().parse().unwrap();
    std::process::Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: fs::read(result).unwrap(),
        stderr: fs::read(consumer.stderr).unwrap_or_default(),
    }
}

fn identity_tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn visit(root: &Path, path: &Path, entries: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(path).expect("read isolated identity tree") {
            let entry = entry.expect("read isolated identity entry");
            let entry_path = entry.path();
            if entry.file_type().expect("identity entry type").is_dir() {
                visit(root, &entry_path, entries);
            } else {
                entries.push((
                    entry_path
                        .strip_prefix(root)
                        .expect("identity entry under root")
                        .to_string_lossy()
                        .into_owned(),
                    fs::read(&entry_path).expect("read isolated identity entry"),
                ));
            }
        }
    }

    let root = root.join("identities");
    let mut entries = Vec::new();
    visit(&root, &root, &mut entries);
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn seed_shared_session_identities(fixture: &Fixture, pane: &Pane) {
    let facts = tmux_identity_facts(fixture, pane);
    let endpoint = facts["tmux"]["endpoint"].clone();
    for suffix in ["ambiguous-a", "ambiguous-b"] {
        let worker_id = format!("codex-session-fixture-{suffix}");
        let identity_path = fixture
            .host_state
            .join("identities")
            .join(&worker_id)
            .join("identity.json");
        fs::create_dir_all(identity_path.parent().unwrap())
            .expect("create isolated fixture identity");
        fs::write(
            &identity_path,
            serde_json::to_vec_pretty(&json!({
                "worker_id": worker_id,
                "token": format!("fixture-token-{suffix}"),
                "project_scope": canonical_root(fixture),
                "transport": {
                    "kind": "tmux",
                    "endpoint": fixture.tmux_socket.to_string_lossy(),
                    "namespace": endpoint["tmux_session_id"],
                    "tmux_endpoint": endpoint,
                    "capabilities": ["send_message_to_pane", "probe_pane"],
                    "self_check": "isolated ambiguity fixture"
                }
            }))
            .expect("serialize isolated fixture identity"),
        )
        .expect("write isolated fixture identity");
    }
}

fn seed_matching_historical_identities(
    fixture: &Fixture,
    project_scope: &str,
    session_id: &str,
    thread_id: &str,
    suffixes: &[&str],
    matching_endpoint: Option<&Value>,
) {
    for suffix in suffixes {
        let worker_id = format!("codex-session-history-{suffix}");
        let identity_path = fixture
            .host_state
            .join("identities")
            .join(&worker_id)
            .join("identity.json");
        fs::create_dir_all(identity_path.parent().unwrap())
            .expect("create isolated historical identity");
        let mut transport = json!({
            "kind": "appserver",
            "endpoint": "unix:///tmp/session-recovery-history.sock",
            "namespace": "codex_tui",
            "session_id": session_id,
            "thread_id": thread_id,
            "capabilities": [],
            "self_check": "isolated historical identity fixture"
        });
        // One archive may still carry the pane this caller now occupies.
        // Historical placement is not ownership evidence, so the fixture keeps
        // the endpoint bytes to prove recovery does not select by placement.
        if *suffix == suffixes[0] {
            if let Some(endpoint) = matching_endpoint {
                transport["kind"] = json!("tmux");
                transport["endpoint"] = endpoint["socket_path"].clone();
                transport["namespace"] = endpoint["tmux_session_id"].clone();
                transport["tmux_endpoint"] = endpoint.clone();
            }
        }
        fs::write(
            &identity_path,
            serde_json::to_vec_pretty(&json!({
                "worker_id": worker_id,
                "token": format!("fixture-history-token-{suffix}"),
                "project_scope": project_scope,
                "runtime": {
                    "agent_id": worker_id,
                    "runtime_id": format!("runtime-history-{suffix}"),
                    "appserver_id": "appserver-cli",
                    "endpoint_generation": 1,
                    "binding_id": format!("binding-history-{suffix}"),
                    "session_id": session_id,
                    "native_thread_id": thread_id
                },
                "transport": transport
            }))
            .expect("serialize historical identity"),
        )
        .expect("write historical identity");
    }
}

#[test]
fn session_recovery_ignores_duplicate_historical_archives_without_current_workers() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("session-recovery-history-only");
    let root = canonical_root(&fixture);
    let matching_endpoint = tmux_identity_facts(&fixture, &pane)["tmux"]["endpoint"].clone();
    seed_matching_historical_identities(
        &fixture,
        &root,
        &pane.session_anchor,
        &pane.thread_anchor,
        &["a", "b"],
        Some(&matching_endpoint),
    );
    let before = identity_tree(&fixture.host_state);

    let first = success_payload(&context_with_agent_session(
        &fixture,
        &fixture.root,
        "session-recovery-history-first",
        &pane,
    ));
    let worker = first["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("recovered worker id");
    assert_ne!(
        worker, "codex-session-history-a",
        "archive a cannot select the worker"
    );
    assert_ne!(
        worker, "codex-session-history-b",
        "archive b cannot select the worker"
    );
    let map: Value =
        serde_json::from_slice(&fs::read(fixture.host_state.join("identity-map.json")).unwrap())
            .unwrap();
    assert!(
        map["slots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|slot| slot["identity_status"] == "active"
                && slot["current"]["binding"]["agent_id"] == worker),
        "the new current mapping must own the real registered worker"
    );
    assert!(
        first["result"]["snapshot"]["master"].is_null(),
        "historical archives must not grant master authority: {first}"
    );

    let second = success_payload(&context_with_agent_session(
        &fixture,
        &fixture.root,
        "session-recovery-history-second",
        &pane,
    ));
    assert_eq!(
        second["result"]["snapshot"]["identity"]["worker_id"], worker,
        "the derived carrier must remain stable across recovery calls"
    );

    let after = identity_tree(&fixture.host_state);
    for (path, bytes) in before {
        let saved = after
            .iter()
            .find(|(candidate, _)| candidate == &path)
            .unwrap_or_else(|| panic!("historical identity {path} disappeared"));
        assert_eq!(saved.1, bytes, "historical identity {path} changed");
    }
}

#[test]
fn history_free_recovery_authorized_master_does_not_read_ambiguous_archives_first() {
    let fixture = Fixture::without_tmux("authorized-history-master");
    let session = "authorized-history-session";
    let thread = "authorized-history-thread";
    let host = TestAppServer::start_with_effect_at(
        &fixture.root,
        "authorized.sock",
        session,
        thread,
        "ack_only",
    );
    let root = canonical_root(&fixture);
    seed_matching_historical_identities(
        &fixture,
        &root,
        session,
        thread,
        &["authorized-a", "authorized-b"],
        None,
    );
    let before = identity_tree(&fixture.host_state);
    let output = run_context_cli(
        &fixture,
        None,
        Some("authorized-history-restore"),
        None,
        None,
        None,
        Some(&json!({"action":"restore_master", "authorization":"user requires current Jenny"})),
        Some(&json!({"session_id":session, "thread_id":thread,
            "endpoint":host.endpoint(), "namespace":"codex_tui"})),
        false,
    );
    let restored = success_payload(&output);
    assert_eq!(restored["result"]["outcome"], "completed", "{restored}");
    assert_eq!(
        restored["result"]["snapshot"]["name"], "Jenny",
        "{restored}"
    );
    let after = identity_tree(&fixture.host_state);
    for (path, bytes) in before {
        assert_eq!(
            after.iter().find(|(p, _)| p == &path).map(|(_, b)| b),
            Some(&bytes)
        );
    }
}

#[test]
fn history_free_recovery_registered_native_session_reuses_its_adaptor() {
    let fixture = Fixture::without_tmux("registered-native-restore");
    let session = "registered-native-session";
    let thread = "registered-native-thread";
    let host = TestAppServer::start_with_effect_at(
        &fixture.root,
        "native.sock",
        session,
        thread,
        "ack_only",
    );
    let seed = fixture.context_provide(
        "registered-native-seed",
        &json!({
            "session_id":session, "thread_id":thread,
            "endpoint":host.endpoint(), "namespace":"codex_tui"
        }),
        None,
    );
    assert_eq!(seed["result"]["outcome"], "completed", "{seed}");
    let output = run_context_cli(
        &fixture,
        None,
        Some("registered-native-restore"),
        None,
        None,
        None,
        None,
        Some(&json!({"session_id":session, "thread_id":thread})),
        false,
    );
    let restored = success_payload(&output);
    assert_eq!(restored["result"]["outcome"], "completed", "{restored}");
    assert_eq!(
        restored["result"]["snapshot"]["identity"]["worker_id"],
        seed["result"]["snapshot"]["identity"]["worker_id"]
    );
    assert_eq!(
        restored["result"]["snapshot"]["binding"],
        seed["result"]["snapshot"]["binding"]
    );
}

#[test]
fn session_recovery_explicit_target_bypasses_historical_archive_ambiguity() {
    let fixture = Fixture::without_tmux("session-recovery-explicit-target");
    let session_id = "explicit-target-session";
    let thread_id = "explicit-target-thread";
    let host = TestAppServer::start_with_effect_at(
        &fixture.root,
        "explicit-target.sock",
        session_id,
        thread_id,
        "ack_only",
    );
    let seed = fixture.context_provide(
        "session-recovery-explicit-target-seed",
        &json!({
            "session_id": session_id,
            "thread_id": thread_id,
            "endpoint": host.endpoint(),
            "namespace": "codex_tui"
        }),
        None,
    );
    assert_eq!(seed["result"]["outcome"], "completed", "{seed}");
    let target = seed["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("target worker id")
        .to_owned();
    let target_path = fixture
        .host_state
        .join("identities")
        .join(&target)
        .join("identity.json");
    let target_before = fs::read(&target_path).expect("read target identity");
    let root = canonical_root(&fixture);
    seed_matching_historical_identities(
        &fixture,
        &root,
        session_id,
        thread_id,
        &["explicit-target-a", "explicit-target-b"],
        None,
    );
    let histories_before = identity_tree(&fixture.host_state);

    let output = run_context_cli(
        &fixture,
        None,
        Some("session-recovery-explicit-target"),
        None,
        None,
        Some(&json!({
            "decision": "approved",
            "action": "restore_peer",
            "target_identity": target,
            "authorization": "user explicitly restores this peer"
        })),
        None,
        Some(&json!({"session_id": session_id, "thread_id": thread_id,
            "endpoint": host.endpoint(), "namespace": "codex_tui"})),
        false,
    );
    let payload = success_payload(&output);
    assert_eq!(
        payload["result"]["snapshot"]["identity"]["worker_id"], target,
        "the approved target must win over historical archive ambiguity: {payload}"
    );
    assert!(
        payload["result"]["snapshot"]["master"].is_null(),
        "restore_peer must not grant master: {payload}"
    );
    assert_eq!(
        fs::read(&target_path).expect("read restored target identity"),
        target_before,
        "same-target recovery must preserve the stored credential"
    );
    let histories_after = identity_tree(&fixture.host_state);
    for (path, bytes) in histories_before
        .into_iter()
        .filter(|(path, _)| path != &format!("{target}/identity.json"))
    {
        let saved = histories_after
            .iter()
            .find(|(candidate, _)| candidate == &path)
            .unwrap_or_else(|| panic!("historical identity {path} disappeared"));
        assert_eq!(saved.1, bytes, "historical identity {path} changed");
    }
}

#[test]
fn session_recovery_no_target_peer_intent_uses_current_carrier() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("session-recovery-peer-intent");
    let seed = fixture.registered_context("session-recovery-peer-intent-seed", &pane);
    let worker = seed["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("current carrier id")
        .to_owned();
    let renamed = fixture.run_ok(&[
        "context", "--op", "peer-intent-bob", "--approve-grant",
        r#"{"decision":"approved","action":"restore_name","requested_name":"Bob","authorization":"user assigns Bob"}"#,
    ], Some(&pane));
    assert_eq!(
        renamed["result"]["snapshot"]["identity"]["display_name"],
        "Bob"
    );
    fixture.run_ok(
        &[
            "task",
            "register",
            "peer-intent-task",
            "--next",
            "retain Bob work",
        ],
        Some(&pane),
    );
    let task_before = fixture.run_ok(&["task", "status", "peer-intent-task"], Some(&pane));
    let sent = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            "Bob",
            "--subject",
            "Bob unread work",
            "--delivery",
            "queued",
            "retain Bob message",
        ],
        Some(&pane),
    );
    let message_id = sent["msg_id"].as_str().unwrap();
    let message = fixture.current_state()["msgs"][message_id].clone();
    assert_eq!(message["body"], "retain Bob message");
    let output = fixture.command(
        &[
            "context",
            "--op",
            "session-recovery-peer-intent",
            "--approve-grant",
            r#"{"decision":"approved","action":"restore_peer","authorization":"user restores the ordinary peer"}"#,
        ],
        Some(&pane),
    );
    let payload = success_payload(&output);
    assert_eq!(
        payload["result"]["snapshot"]["identity"]["display_name"], "Bob",
        "ordinary peer recovery must keep Bob even when Alice is vacant: {payload}"
    );
    let task_after = fixture.run_ok(&["task", "status", "peer-intent-task"], Some(&pane));
    assert_eq!(task_after["owner"], task_before["owner"]);
    assert_eq!(task_after["status"], task_before["status"]);
    let unread = payload["result"]["snapshot"]["inbox"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == message["id"])
        .expect("Bob retains unread mail");
    assert_eq!(unread["body"], message["body"]);
    let receipt = fixture.run_ok(&["msg", message["id"].as_str().unwrap()], Some(&pane));
    assert_eq!(receipt["consumed_by_recv"], false);
    assert_eq!(fixture.current_state()["msgs"][message_id], message);
    assert_eq!(
        payload["result"]["snapshot"]["identity"]["worker_id"], worker,
        "a targetless peer intent must select the current carrier: {payload}"
    );
    assert!(
        payload["result"]["snapshot"]["master"].is_null(),
        "restore_peer must not grant master: {payload}"
    );
    let before_denial = identity_tree(&fixture.host_state);
    let before_map = fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    let before_grants = fixture.current_state()["global"]["projects"][canonical_root(&fixture)]["master_grants"].clone();
    let denied = fixture.command(&[
        "context", "--op", "peer-intent-explicit-denial", "--approve-grant",
        r#"{"decision":"denied","action":"restore_master","authorization":"user did not approve master"}"#,
    ], Some(&pane));
    assert!(
        !denied.status.success(),
        "explicit denial must not become approval"
    );
    assert!(text(&denied).contains("NAME_AUTHORIZATION_REQUIRED"));
    assert_eq!(identity_tree(&fixture.host_state), before_denial);
    assert_eq!(fs::read(fixture.host_state.join("identity-map.json")).unwrap(), before_map);
    assert_eq!(fixture.current_state()["global"]["projects"][canonical_root(&fixture)]["master_grants"], before_grants);
}

#[test]
fn session_recovery_rejects_foreign_scope_approval() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("session-recovery-scope-reject");
    let foreign_root = fixture.root.join("foreign-session-recovery");
    seed_baseline(&foreign_root);
    let foreign_pane =
        same_session_pane(&fixture, "session-recovery-foreign", pane.server_pid, &pane);
    let foreign = registered_context_in(
        &fixture,
        &foreign_root,
        "session-recovery-foreign-seed",
        &foreign_pane,
    );
    let foreign_id = foreign["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("foreign worker id")
        .to_owned();
    let foreign_scope = fs::canonicalize(&foreign_root)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let before = identity_tree(&fixture.host_state);

    let approval = json!({
        "decision": "approved",
        "action": "restore_peer",
        "target_identity": foreign_id,
        "project_scope": foreign_scope,
        "authorization": "user targets another project"
    })
    .to_string();
    let output = fixture.command(
        &[
            "context",
            "--op",
            "session-recovery-scope-reject",
            "--approve-identity",
            &approval,
        ],
        Some(&pane),
    );
    let foreign_error = text(&output);
    assert!(
        !output.status.success(),
        "foreign scope must fail: {foreign_error}"
    );
    assert!(
        foreign_error.contains("IDENTITY_APPROVAL_SCOPE_MISMATCH"),
        "foreign scope must produce its typed rejection: {foreign_error}"
    );
    assert_eq!(
        identity_tree(&fixture.host_state),
        before,
        "a rejected scope must not mutate identity state"
    );
}

#[test]
fn public_context_prefers_current_project_for_a_shared_tmux_session() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("shared-tmux-current");
    let current = fixture.registered_context("shared-tmux-current-seed", &pane);
    let current_id = current["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("current project identity")
        .to_owned();

    let foreign_root = fixture.root.join("foreign-project");
    seed_baseline(&foreign_root);
    let foreign_pane = same_session_pane(&fixture, "shared-tmux-foreign", pane.server_pid, &pane);
    let foreign = registered_context_in(
        &fixture,
        &foreign_root,
        "shared-tmux-foreign-seed",
        &foreign_pane,
    );
    let foreign_id = foreign["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .expect("foreign project identity")
        .to_owned();
    assert_ne!(current_id, foreign_id);
    let task = current_pane_command(
        &fixture,
        &foreign_root,
        "foreign-task-create",
        &foreign_pane,
        &[
            "task",
            "register",
            "foreign-preserved-task",
            "--next",
            "keep",
        ],
    );
    assert!(
        task.status.success(),
        "foreign task setup failed: {}",
        String::from_utf8_lossy(&task.stderr)
    );

    let recovered = context_with_agent_session(
        &fixture,
        &fixture.root,
        "shared-tmux-current-restore",
        &pane,
    );
    let recovered = success_payload(&recovered);
    assert_eq!(
        recovered["result"]["snapshot"]["identity"]["worker_id"], current_id,
        "{recovered}"
    );

    let foreign_restore = context_with_agent_session(
        &fixture,
        &foreign_root,
        "shared-tmux-foreign-readback",
        &foreign_pane,
    );
    let foreign_restore = success_payload(&foreign_restore);
    assert_eq!(
        foreign_restore["result"]["snapshot"]["identity"]["worker_id"], foreign_id,
        "{foreign_restore}"
    );
    let status = current_pane_command(
        &fixture,
        &foreign_root,
        "foreign-task-status",
        &foreign_pane,
        &["task", "status", "foreign-preserved-task"],
    );
    assert!(
        status.status.success(),
        "foreign task readback failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status = json_stdout(&status);
    assert_eq!(status["owner"], foreign_id, "{status}");
}

#[test]
fn public_context_restores_current_session_with_historical_identity_duplicates() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("shared-tmux-ambiguous");
    let current = fixture.registered_context("shared-agent-current-seed", &pane);
    seed_shared_session_identities(&fixture, &pane);
    let identities_before = identity_tree(&fixture.host_state);

    let output = context_with_agent_session(
        &fixture,
        &fixture.root,
        "shared-tmux-ambiguous-restore",
        &pane,
    );
    let payload = success_payload(&output);
    assert_eq!(payload["result"]["outcome"], "completed", "{payload}");
    let snapshot = &payload["result"]["snapshot"];
    assert_eq!(snapshot["registered"], true);
    assert!(
        snapshot["master"].is_null(),
        "history must not grant master authority"
    );
    let worker = snapshot["identity"]["worker_id"].as_str().unwrap();
    assert_eq!(
        worker,
        current["result"]["snapshot"]["identity"]["worker_id"]
            .as_str()
            .unwrap(),
        "current agent session must win over historical tmux identity duplicates"
    );
    let selected_path = format!("{worker}/identity.json");
    let identities_after = identity_tree(&fixture.host_state);
    assert_eq!(identities_after.len(), identities_before.len());
    for (path, before) in identities_before {
        let after = &identities_after
            .iter()
            .find(|(candidate, _)| candidate == &path)
            .unwrap()
            .1;
        if path == selected_path {
            let before: Value = serde_json::from_slice(&before).unwrap();
            let after: Value = serde_json::from_slice(after).unwrap();
            assert_eq!(after["token"], before["token"]);
        } else {
            assert_eq!(
                after, &before,
                "unrelated historical identity must remain intact"
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
    assert!(!fixture.host_state.join("identity-map.json").exists());

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
    let current: Value = serde_json::from_slice(&fs::read(fixture.host_state.join("identity-map.json")).unwrap()).unwrap();
    assert_eq!(current["slots"].as_array().unwrap().len(), 1);
    assert_eq!(current["slots"][0]["current"]["binding"]["session_id"], session_id);
    drop(appserver);
}

#[test]
fn c03_unapproved_unique_pane_stale_recovery_repairs_without_approval() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("collab-v23-c03");
    let operation_id = "ctxop-v23-c03-unapproved";
    let fence = seed_stale_identity(&fixture, &pane, operation_id, false);
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
    let automatic_payload = success_payload(&automatic);
    let automatic_text = text(&automatic);
    assert!(
        automatic.status.success(),
        "unique same-pane stale credential must auto-repair: {automatic_text}"
    );
    assert!(
        !automatic_text.contains("IDENTITY_APPROVAL_REQUIRED"),
        "automatic pane recovery must not request approval: {automatic_text}"
    );
    assert_eq!(
        automatic_payload["result"]["outcome"], "completed",
        "{automatic_payload}"
    );
    assert!(
        !automatic_text.contains("stale-"),
        "approval result must not leak the stale credential: {automatic_text}"
    );
    assert_eq!(
        automatic_payload["result"]["snapshot"]["identity"]["worker_id"], fence.worker_id,
        "{automatic_payload}"
    );
    let map: Value = serde_json::from_slice(
        &fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
    ).unwrap();
    let active: Vec<&Value> = map["slots"].as_array().unwrap().iter()
        .filter(|slot| slot["identity_status"] == "active").collect();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0]["current"]["binding"]["agent_id"], fence.worker_id);
    assert_eq!(active[0]["current"]["binding"]["binding_id"], fence.binding_id);
    assert_eq!(
        automatic_payload["result"]["snapshot"]["binding"]["binding_id"],
        fence.binding_id
    );
    assert!(automatic_payload["result"]["snapshot"]["binding"]
        .get("endpoint_generation")
        .is_none());
    assert_eq!(automatic_payload["result"]["snapshot"]["registered"], true);
    assert_ne!(
        fs::read(&fence.identity_path).expect("stale cache replaced"),
        fence.stale_identity_bytes,
        "the daemon must replace the stale local credential"
    );
}

/// Run the real candidate CLI inside the test-owned pane under an explicit
/// agent session and invocation cwd. The daemon observes the caller's live
/// process ancestry for name inheritance, so this runs the binary in the pane
/// instead of submitting tmux facts that would only impersonate observation.
fn pane_identity_context(
    fixture: &Fixture,
    pane: &Pane,
    label: &str,
    cwd: &Path,
    session: &str,
    thread: &str,
    operation_id: &str,
) -> Value {
    use std::os::unix::process::ExitStatusExt;
    let done = fixture.root.join(format!("{label}.exit"));
    let result = fixture.root.join(format!("{label}.result"));
    let stderr = fixture.root.join(format!("{label}.stderr"));
    fs::remove_file(&done).ok();
    let script = format!(
        "cd {cwd}\nexport CODEX_SESSION_ID={session}\nexport CODEX_THREAD_ID={thread}\n\"$COLLAB_BINARY\" context --op {operation_id} >{result} 2>{stderr}\nprintf '%s' \"$?\" >{done}\nsleep 600\n",
        cwd = shell_quote(&cwd.to_string_lossy()),
        session = shell_quote(session),
        thread = shell_quote(thread),
        operation_id = shell_quote(operation_id),
        result = shell_quote(&result.to_string_lossy()),
        stderr = shell_quote(&stderr.to_string_lossy()),
        done = shell_quote(&done.to_string_lossy()),
    );
    let consumer = fixture.run_in_pane(pane, label, &script);
    assert!(
        wait_until(Duration::from_secs(20), || {
            fs::read_to_string(&done).is_ok_and(|value| !value.is_empty())
        }),
        "pane identity context did not finish in its own pane: {}",
        consumer.output()
    );
    let code: i32 = fs::read_to_string(&done).unwrap().parse().unwrap();
    let output = std::process::Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: fs::read(&result).unwrap_or_default(),
        stderr: fs::read(&stderr).unwrap_or_default(),
    };
    success_payload(&output)
}

fn current_map_holder(fixture: &Fixture, name: &str) -> Value {
    let map: Value = serde_json::from_slice(&fs::read(fixture.host_state.join("identity-map.json")).unwrap()).unwrap();
    let slots = map["slots"].as_array().unwrap();
    let slot = slots.iter().find(|slot| slot["identity_status"] == "active"
        && slot["identity_key"]["project_scope"] == canonical_root(fixture)
        && slot["identity_key"]["app_scope_id"] == "appserver-cli"
        && slot["identity_key"]["name"] == name)
        .unwrap_or_else(|| panic!("name {name} has no active current mapping"));
    slot["current"].clone()
}

fn seeded_name_and_worker(payload: &Value) -> (String, String) {
    let snapshot = &payload["result"]["snapshot"];
    (
        snapshot["identity"]["display_name"]
            .as_str()
            .expect("seeded identity has a display name")
            .to_owned(),
        snapshot["identity"]["worker_id"]
            .as_str()
            .expect("seeded identity has a worker id")
            .to_owned(),
    )
}

#[test]
fn same_fixed_tmux_session_name_hands_name_to_new_agent_session() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("pane-inherit-same-pane");
    let cwd = PathBuf::from(canonical_root(&fixture));
    let seed = fixture.registered_context("pane-inherit-same-pane-seed", &pane);
    let (seed_name, seed_worker) = seeded_name_and_worker(&seed);
    assert_eq!(
        seed_name, "Alice",
        "the first ordinary registration takes the pool head: {seed}"
    );
    let inherited = pane_identity_context(
        &fixture,
        &pane,
        "pane-inherit-same-pane-second",
        &cwd,
        "pane-inherit-same-pane-other-session",
        "pane-inherit-same-pane-other-thread",
        "pane-inherit-same-pane-second",
    );
    let snapshot = &inherited["result"]["snapshot"];
    assert_eq!(
        snapshot["name"].as_str(),
        Some(seed_name.as_str()),
        "the fixed tmux session name must hand the first name to a new agent session: {inherited}"
    );
    assert_eq!(
        snapshot["identity"]["display_name"].as_str(),
        Some(seed_name.as_str()),
        "{inherited}"
    );
    assert_eq!(
        snapshot["identity"]["worker_id"].as_str(),
        Some(seed_worker.as_str()),
        "the fixed tmux session name keeps the identity holder and updates its agent session: {inherited}"
    );
    let current = current_map_holder(&fixture, &seed_name);
    assert_eq!(
        current["binding"]["agent_id"], snapshot["identity"]["worker_id"],
        "{current}"
    );
    assert_eq!(
        current["binding"]["session_id"].as_str(),
        Some("pane-inherit-same-pane-other-session"),
        "the fixed name follows its new current agent session: {current}"
    );
    assert_eq!(
        current["binding"]["tmux_endpoint"]["pane_id"].as_str(),
        Some(pane.pane_id.as_str()),
        "the map records the current pane: {current}"
    );
    assert_eq!(
        current["invocation_cwd"].as_str(),
        Some(cwd.to_string_lossy().as_ref()),
        "the map records the current invocation cwd: {current}"
    );
}

#[test]
fn same_agent_session_keeps_name_across_placements() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("pane-inherit-same-session");
    let root = PathBuf::from(canonical_root(&fixture));
    let seed = fixture.registered_context("pane-inherit-same-session-seed", &pane);
    let (seed_name, seed_worker) = seeded_name_and_worker(&seed);
    let other_pane = same_session_pane(
        &fixture,
        "pane-inherit-same-session-other",
        pane.server_pid,
        &pane,
    );
    let other_cwd = root.join("invocation-sub");
    fs::create_dir_all(&other_cwd).expect("create a second invocation directory");

    let resumed = pane_identity_context(
        &fixture,
        &other_pane,
        "pane-inherit-same-session-readback",
        &other_cwd,
        &pane.session_anchor,
        &pane.thread_anchor,
        "pane-inherit-same-session-readback",
    );
    let snapshot = &resumed["result"]["snapshot"];
    assert_eq!(
        snapshot["identity"]["worker_id"].as_str(),
        Some(seed_worker.as_str()),
        "the same agent session resumes the same carrier from a different pane and cwd: {resumed}"
    );
    assert_eq!(
        snapshot["name"].as_str(),
        Some(seed_name.as_str()),
        "one agent session keeps one name regardless of the pane or cwd it calls from: {resumed}"
    );
    let current = current_map_holder(&fixture, &seed_name);
    assert_eq!(
        current["binding"]["agent_id"].as_str(),
        Some(seed_worker.as_str()),
        "{current}"
    );
    assert_eq!(
        current["binding"]["session_id"].as_str(),
        Some(pane.session_anchor.as_str()),
        "{current}"
    );
    assert_eq!(
        tmux_session_name(&fixture, &other_pane),
        project_tmux_name(&other_cwd, &seed_name),
        "moving to another pane in the same tmux session keeps the fixed name"
    );
}

#[test]
fn different_agent_session_does_not_inherit_name_across_panes() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("pane-inherit-other-pane");
    let root = PathBuf::from(canonical_root(&fixture));
    let seed = fixture.registered_context("pane-inherit-other-pane-seed", &pane);
    let (seed_name, seed_worker) = seeded_name_and_worker(&seed);
    let other_pane = second_pane(&fixture, "pane-inherit-other-pane-second", pane.server_pid);

    let fresh = pane_identity_context(
        &fixture,
        &other_pane,
        "pane-inherit-other-pane-readback",
        &root,
        "pane-inherit-other-pane-session",
        "pane-inherit-other-pane-thread",
        "pane-inherit-other-pane-readback",
    );
    let snapshot = &fresh["result"]["snapshot"];
    let fresh_name = snapshot["name"]
        .as_str()
        .expect("fresh registration has a name")
        .to_owned();
    assert_ne!(
        fresh_name, seed_name,
        "a different pane with a different fixed tmux session name must not inherit the first name: {fresh}"
    );
    assert_ne!(
        snapshot["identity"]["worker_id"].as_str(),
        Some(seed_worker.as_str()),
        "{fresh}"
    );
    let current = current_map_holder(&fixture, &fresh_name);
    assert_eq!(
        current["binding"]["session_id"].as_str(),
        Some("pane-inherit-other-pane-session"),
        "{current}"
    );
    assert_eq!(
        current["binding"]["tmux_endpoint"]["pane_id"].as_str(),
        Some(other_pane.pane_id.as_str()),
        "a fresh name records the current pane: {current}"
    );
    assert_eq!(
        current["invocation_cwd"].as_str(),
        Some(root.to_string_lossy().as_ref()),
        "a fresh name records the current invocation cwd: {current}"
    );
    assert_eq!(
        current_map_holder(&fixture, &seed_name)["binding"]["agent_id"].as_str(),
        Some(seed_worker.as_str()),
        "the original holder keeps the first name"
    );
}

#[test]
fn different_agent_session_does_not_inherit_name_across_invocation_cwd() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("pane-inherit-other-cwd");
    let root = PathBuf::from(canonical_root(&fixture));
    let seed = fixture.registered_context("pane-inherit-other-cwd-seed", &pane);
    let (seed_name, seed_worker) = seeded_name_and_worker(&seed);
    let other_cwd = root.join("invocation-sub");
    fs::create_dir_all(&other_cwd).expect("create a second invocation directory");
    let renamed_session = format!("{}-other-cwd", seed_name);
    tmux(
        &fixture.tmux_socket,
        &[
            "rename-session",
            "-t",
            &pane.pane_id,
            renamed_session.as_str(),
        ],
    );

    let fresh = pane_identity_context(
        &fixture,
        &pane,
        "pane-inherit-other-cwd-readback",
        &other_cwd,
        "pane-inherit-other-cwd-session",
        "pane-inherit-other-cwd-thread",
        "pane-inherit-other-cwd-readback",
    );
    let snapshot = &fresh["result"]["snapshot"];
    let fresh_name = snapshot["name"]
        .as_str()
        .expect("fresh registration has a name")
        .to_owned();
    assert_ne!(
        fresh_name, seed_name,
        "a different invocation cwd with a different fixed tmux session name must not inherit the first name: {fresh}"
    );
    assert_ne!(
        snapshot["identity"]["worker_id"].as_str(),
        Some(seed_worker.as_str()),
        "{fresh}"
    );
    let current = current_map_holder(&fixture, &fresh_name);
    assert_eq!(
        current["binding"]["session_id"].as_str(),
        Some("pane-inherit-other-cwd-session"),
        "{current}"
    );
    assert_eq!(
        current["binding"]["tmux_endpoint"]["pane_id"].as_str(),
        Some(pane.pane_id.as_str()),
        "a fresh name records the current pane: {current}"
    );
    assert_eq!(
        current["invocation_cwd"].as_str(),
        Some(other_cwd.to_string_lossy().as_ref()),
        "a fresh name records the current invocation cwd: {current}"
    );
}

/// A name lost by both the fixed tmux session name and the agent session
/// recovers through the daemon-observed pane and invocation cwd, but only
/// when both still match one same-scope occupancy.
#[test]
fn missing_name_recovers_from_joint_pane_and_cwd_match() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("pane-inherit-joint-anchor");
    let root = PathBuf::from(canonical_root(&fixture));
    let seed = fixture.registered_context("pane-inherit-joint-anchor-seed", &pane);
    let (seed_name, _seed_worker) = seeded_name_and_worker(&seed);
    // Rename the fixed session away from the registered name, so neither the
    // current name nor the agent session can select the holder. The pane and
    // the invocation cwd still identify it.
    let renamed_session = format!("{seed_name}-lost-anchor");
    tmux(
        &fixture.tmux_socket,
        &[
            "rename-session",
            "-t",
            &pane.pane_id,
            renamed_session.as_str(),
        ],
    );

    let recovered = pane_identity_context(
        &fixture,
        &pane,
        "pane-inherit-joint-anchor-readback",
        &root,
        "pane-inherit-joint-anchor-session",
        "pane-inherit-joint-anchor-thread",
        "pane-inherit-joint-anchor-readback",
    );
    let snapshot = &recovered["result"]["snapshot"];
    assert_eq!(
        snapshot["name"].as_str(),
        Some(seed_name.as_str()),
        "the joint pane and cwd anchor must recover the lost name: {recovered}"
    );
    let current = current_map_holder(&fixture, &seed_name);
    assert_eq!(
        current["binding"]["session_id"].as_str(),
        Some("pane-inherit-joint-anchor-session"),
        "the current map keeps the name and refreshes its agent session: {current}"
    );
    assert_eq!(
        current["binding"]["tmux_endpoint"]["pane_id"].as_str(),
        Some(pane.pane_id.as_str()),
        "{current}"
    );
}

#[test]
fn public_goal_schedule_and_pending_wake_move_across_agent_session_to_new_jenny() {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("name-anchor-takeover");
    assert_eq!(fixture.tmux_socket, fixture.root.join("t.sock"));
    assert!(fixture.tmux_socket.exists());

    let old = fixture.registered_context("name-anchor-takeover-old", &pane);
    let (old_ordinary_name, old_worker_id) = seeded_name_and_worker(&old);
    let old_session_id = old["result"]["snapshot"]["binding"]["session_id"]
        .as_str()
        .expect("old identity has a session id")
        .to_owned();
    assert_ne!(old_ordinary_name, "Jenny");

    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "isolated name-anchor takeover old promotes to Jenny",
        ],
        Some(&pane),
    );

    let goal_id = "name-anchor-takeover-goal";
    let trigger_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_millis() as i64
        + 2_000;
    let armed = fixture.run_ok(
        &[
            "goal",
            "reconcile",
            "--goal-id",
            goal_id,
            "--owner",
            "Jenny",
            "--desired",
            "active",
            "--at-ms",
            &trigger_ms.to_string(),
            "--ttl-seconds",
            "300",
        ],
        Some(&pane),
    );
    assert_eq!(armed["status"], "armed", "{armed}");
    let subscription_id = armed["subscription"]["id"]
        .as_str()
        .expect("armed goal has a subscription id")
        .to_owned();

    let pending_id = {
        assert!(
            wait_until(Duration::from_secs(15), || {
                let state = fixture.current_state();
                state["wake_bindings"].as_object().unwrap().iter().any(|(id, subscription)|
                    subscription == &subscription_id && state["msgs"][id]["state"] == "pending")
            }),
            "the real daemon did not commit a pending goal wake"
        );

        let state = fixture.current_state();
        state["wake_bindings"].as_object().unwrap().iter().find(|(id, subscription)|
            *subscription == &subscription_id && state["msgs"][*id]["state"] == "pending")
            .expect("current pending wake names its goal subscription").0.clone()
    };

    let state = fixture.current_state();
    let pending_message = state["msgs"][&pending_id].clone();
    let receive_receipts = state["receive_receipts"].clone();
    assert_eq!(pending_message["state"], "pending");
    assert_eq!(pending_message["to"], old_worker_id);

    let successor = second_pane(&fixture, "name-anchor-takeover-second", pane.server_pid);
    let root = PathBuf::from(canonical_root(&fixture));
    let successor_context = pane_identity_context(
        &fixture,
        &successor,
        "name-anchor-takeover-context",
        &root,
        &format!("session-name-anchor-takeover-second-{}", successor.pane_id),
        &format!("thread-name-anchor-takeover-second-{}", successor.pane_id),
        "name-anchor-takeover-context",
    );
    let successor_snapshot = &successor_context["result"]["snapshot"];
    let successor_worker_id = successor_snapshot["identity"]["worker_id"]
        .as_str()
        .expect("successor context names its worker")
        .to_owned();
    let successor_name = successor_snapshot["name"]
        .as_str()
        .expect("successor names its ordinary name")
        .to_owned();
    let successor_session_id = successor_snapshot["binding"]["session_id"]
        .as_str()
        .expect("successor identity has a session id")
        .to_owned();
    assert_ne!(successor_worker_id, old_worker_id);
    assert_ne!(successor_session_id, old_session_id);
    assert_ne!(successor_name, "Jenny");

    let takeover = fixture.command(
        &[
            "context",
            "--op",
            "name-anchor-takeover-approve",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"isolated name-anchor takeover restore master","replace_confirmed":true}"#,
        ],
        Some(&successor),
    );
    assert!(
        takeover.status.success(),
        "restore_master must take Jenny from the old session: stdout={} stderr={}",
        String::from_utf8_lossy(&takeover.stdout),
        String::from_utf8_lossy(&takeover.stderr)
    );

    let successor_status = fixture.run_ok(&["notify", "status"], Some(&successor));
    let successor_goal = successor_status["subscriptions"]
        .as_array()
        .expect("notify status lists successor subscriptions")
        .iter()
        .find(|subscription| subscription["id"] == subscription_id)
        .expect("successor owns the goal subscription");
    assert_eq!(
        successor_goal["worker_id"], successor_worker_id,
        "{successor_goal}"
    );

    let old_status = fixture.run_ok(&["notify", "status"], Some(&pane));
    assert!(
        !old_status["subscriptions"]
            .as_array()
            .expect("notify status lists old subscriptions")
            .iter()
            .any(|subscription| subscription["id"] == subscription_id),
        "old worker must not retain the goal subscription: {old_status}"
    );

    let state = fixture.current_state();
    let moved = &state["msgs"][&pending_id];
    assert_eq!(moved["id"], pending_message["id"]);
    assert_eq!(moved["body"], pending_message["body"]);
    assert_eq!(moved["from"], pending_message["from"]);
    assert_eq!(moved["state"], "pending");
    assert_eq!(moved["to"], successor_worker_id);
    assert_eq!(state["wake_bindings"][&pending_id], subscription_id);
    assert_eq!(state["receive_receipts"], receive_receipts);

    let denied = fixture.command(
        &[
            "goal",
            "reconcile",
            "--goal-id",
            goal_id,
            "--owner",
            "Jenny",
            "--desired",
            "ended",
        ],
        Some(&pane),
    );
    let denied_text = text(&denied);
    assert!(!denied.status.success(), "{denied_text}");
    assert!(
        denied_text.contains("GOAL_SCHEDULE_OWNER_MISMATCH"),
        "old worker must be rejected as the goal owner: {denied_text}"
    );

    let ended = fixture.run_ok(
        &[
            "goal",
            "reconcile",
            "--goal-id",
            goal_id,
            "--owner",
            "Jenny",
            "--desired",
            "ended",
        ],
        Some(&successor),
    );
    assert_eq!(ended["status"], "ended", "{ended}");
    assert!(
        ended["retired_subscription_ids"]
            .as_array()
            .is_some_and(|ids| {
                ids.iter()
                    .any(|id| id.as_str() == Some(subscription_id.as_str()))
            }),
        "{ended}"
    );
    let successor_status = fixture.run_ok(&["notify", "status"], Some(&successor));
    let successor_goal = successor_status["subscriptions"]
        .as_array()
        .expect("notify status keeps the retired occurrence")
        .iter()
        .find(|subscription| subscription["id"] == subscription_id)
        .expect("the retired goal occurrence remains observable");
    assert_eq!(successor_goal["status"], "cancelled", "{successor_goal}");
}

fn restricted_cwd_context_in_pane(
    fixture: &Fixture,
    pane: &Pane,
    restricted: &Path,
    label: &str,
    operation_id: &str,
    pwd: Option<&str>,
) -> std::process::Output {
    use std::os::unix::process::ExitStatusExt;
    let done = fixture.root.join(format!("{label}.exit"));
    let result = fixture.root.join(format!("{label}.result"));
    let stderr = fixture.root.join(format!("{label}.stderr"));
    fs::remove_file(&done).ok();
    let pwd_step = match pwd {
        Some(pwd) => format!("export PWD={}", shell_quote(pwd)),
        None => "unset PWD".into(),
    };
    // The caller stays inside the real fixture pane so the daemon observes the
    // same session/thread ancestry as the seed registration, while the denied
    // `getcwd` is the exact degraded path the fallback must resolve.
    let script = format!(
        "cd {}\nchmod 000 .\n{}\n\"$COLLAB_BINARY\" context --query --op {} >{} 2>{}\nstatus=$?\nchmod 700 {}\nprintf '%s' \"$status\" >{}\nsleep 600\n",
        shell_quote(&restricted.to_string_lossy()),
        pwd_step,
        shell_quote(operation_id),
        shell_quote(&result.to_string_lossy()),
        shell_quote(&stderr.to_string_lossy()),
        shell_quote(&restricted.to_string_lossy()),
        shell_quote(&done.to_string_lossy()),
    );
    let consumer = fixture.run_in_pane(pane, label, &script);
    assert!(
        wait_until(Duration::from_secs(20), || {
            fs::read_to_string(&done).is_ok_and(|value| !value.is_empty())
        }),
        "restricted cwd context did not finish in its own pane: {}",
        consumer.output()
    );
    let code: i32 = fs::read_to_string(&done).unwrap().parse().unwrap();
    std::process::Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: fs::read(&result).unwrap_or_default(),
        stderr: fs::read(&stderr).unwrap_or_default(),
    }
}

#[test]
fn public_context_restricted_cwd_uses_authenticated_pwd_and_rejects_forged_paths() {
    let _fixture_lock = TMUX_FIXTURE_LOCK.lock().unwrap();
    let (fixture, pane) = Fixture::isolated("restricted-cwd-resolution");
    let seed = fixture.registered_context("restricted-cwd-seed", &pane);
    assert_eq!(seed["ok"], true, "{seed}");

    let restricted = fixture.root.join("restricted-cwd");
    fs::create_dir(&restricted).expect("create restricted cwd");
    let root = canonical_root(&fixture);
    let forged = fixture.root.join("forged-cwd-path");
    fs::remove_dir_all(&forged).ok();
    let seed_worker = seed["result"]["snapshot"]["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let valid = restricted_cwd_context_in_pane(
        &fixture,
        &pane,
        &restricted,
        "ctxop-restricted-valid-pwd",
        "ctxop-restricted-valid-pwd",
        Some(&root),
    );
    assert!(
        valid.status.success(),
        "valid authenticated PWD must resolve: {}",
        text(&valid)
    );
    let valid_payload = json_stdout(&valid);
    assert_eq!(
        valid_payload.pointer("/result/snapshot/identity/worker_id")
            .and_then(Value::as_str),
        Some(seed_worker.as_str()),
        "restricted cwd must resolve to the same registered identity: {valid_payload}"
    );
    assert_eq!(
        valid_payload.pointer("/result/snapshot/binding/project_scope")
            .and_then(Value::as_str),
        Some(root.as_str()),
        "authenticated PWD must resolve to the same registered project: {valid_payload}"
    );

    let missing = restricted_cwd_context_in_pane(
        &fixture,
        &pane,
        &restricted,
        "ctxop-restricted-missing-pwd",
        "ctxop-restricted-missing-pwd",
        None,
    );
    assert!(!missing.status.success(), "{}", text(&missing));
    assert!(
        text(&missing).contains("PWD is unavailable"),
        "{}",
        text(&missing)
    );

    let relative = restricted_cwd_context_in_pane(
        &fixture,
        &pane,
        &restricted,
        "ctxop-restricted-relative-pwd",
        "ctxop-restricted-relative-pwd",
        Some("relative"),
    );
    assert!(!relative.status.success(), "{}", text(&relative));
    assert!(
        text(&relative).contains("PWD is unavailable"),
        "{}",
        text(&relative)
    );

    let forged = restricted_cwd_context_in_pane(
        &fixture,
        &pane,
        &restricted,
        "ctxop-restricted-forged-pwd",
        "ctxop-restricted-forged-pwd",
        Some(forged.to_str().expect("forged path is UTF-8")),
    );
    assert!(!forged.status.success(), "{}", text(&forged));
    assert!(
        text(&forged).contains("PWD cannot be canonicalized"),
        "{}",
        text(&forged)
    );

    let unregistered = unique_root("restricted-cwd-unregistered");
    fs::create_dir_all(&unregistered).expect("create forged but real PWD target");
    let forged_real = restricted_cwd_context_in_pane(
        &fixture,
        &pane,
        &restricted,
        "ctxop-restricted-forged-real-pwd",
        "ctxop-restricted-forged-real-pwd",
        Some(unregistered.to_str().expect("unregistered PWD is UTF-8")),
    );
    fs::remove_dir_all(&unregistered).ok();
    assert!(!forged_real.status.success(), "{}", text(&forged_real));
    assert!(
        text(&forged_real).contains("COLLAB_CONTEXT_UNRESOLVED"),
        "{}",
        text(&forged_real)
    );
}
