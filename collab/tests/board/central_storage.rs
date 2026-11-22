use super::*;

fn find_task<'a>(board: &'a Value, id: &str) -> &'a Value {
    board["tasks"]
        .as_array()
        .expect("board lists tasks")
        .iter()
        .find(|task| task["id"] == id)
        .unwrap_or_else(|| panic!("board contains task {id}: {board}"))
}

fn point_route_at(fixture: &AppFixture, storage_root: &std::path::Path) {
    let path = fixture.host_state.join("routes.json");
    let mut routes: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let project = fixture.project.canonicalize().unwrap();
    let selected: Vec<_> = routes
        .iter_mut()
        .filter(|route| {
            route["project_scope"].as_str() == project.to_str()
                && route["app_scope_id"] == "appserver-cli"
        })
        .collect();
    assert_eq!(selected.len(), 1);
    selected.into_iter().next().unwrap()["storage_root"] = json!(storage_root);
    std::fs::write(path, serde_json::to_vec(&routes).unwrap()).unwrap();
}

fn remove_host_routes(fixture: &AppFixture) {
    let path = fixture.host_state.join("routes.json");
    let mut routes: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let project = fixture.project.canonicalize().unwrap();
    let before = routes.len();
    routes.retain(|route| {
        !(route["project_scope"].as_str() == project.to_str()
            && route["app_scope_id"] == "appserver-cli")
    });
    assert_eq!(before - routes.len(), 1);
    std::fs::write(path, serde_json::to_vec(&routes).unwrap()).unwrap();
}

fn command_public_in(
    fixture: &AppFixture,
    args: &[&str],
    thread: &str,
    cwd: &std::path::Path,
) -> Output {
    Command::new(binary())
        .args(args)
        .current_dir(cwd)
        .env("COLLAB_STATE_DIR", &fixture.host_state)
        .env("HOME", fixture.root.join("home"))
        .env("COLLAB_APPSERVER_SOCKET", &fixture.app_socket)
        .env_remove("CODEX_APP_SERVER_SOCKET")
        .env_remove("COLLAB_APPSERVER_NAMESPACE")
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
        .env("CODEX_SESSION_ID", thread)
        .env("CODEX_THREAD_ID", thread)
        .env_remove("REX_SERVER")
        .env_remove("REX_SESSION")
        .env_remove("REX_BLOCK")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("COLLAB_WORKER")
        .env("CODEX_HOME", fixture.root.join("home"))
        .output()
        .expect("run collab CLI in selected project")
}

fn run_public_in(
    fixture: &AppFixture,
    args: &[&str],
    thread: &str,
    cwd: &std::path::Path,
) -> Value {
    let output = command_public_in(fixture, args, thread, cwd);
    assert!(
        output.status.success(),
        "collab {:?} in {} failed: stdout={} stderr={}",
        args,
        cwd.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("collab CLI emits JSON");
    value
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .cloned()
        .unwrap_or(value)
}

fn start_daemon_from(fixture: &mut AppFixture, daemon_root: &std::path::Path) {
    let daemon_log =
        std::fs::File::create(fixture.root.join("test-daemon-host.log")).expect("create serve log");
    let mut command = Command::new(binary());
    command
        .arg("serve")
        .current_dir(daemon_root)
        .env("COLLAB_STATE_DIR", &fixture.host_state)
        .env("HOME", fixture.root.join("home"))
        .env("CODEX_HOME", fixture.root.join("home"))
        .env_remove("COLLAB_APPSERVER_SOCKET")
        .env_remove("CODEX_APP_SERVER_SOCKET")
        .env_remove("COLLAB_APPSERVER_NAMESPACE")
        .env_remove("REX_SERVER")
        .env_remove("REX_SESSION")
        .env_remove("REX_BLOCK")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("COLLAB_WORKER")
        .stdin(std::process::Stdio::null())
        .stdout(daemon_log.try_clone().expect("clone serve log"))
        .stderr(daemon_log);
    fixture.daemon = Some(command.spawn().expect("spawn owned serve child"));
    super::daemon_readiness::wait_for_ping(
        fixture.daemon.as_mut().unwrap(),
        &fixture.host_state.join("server.sock"),
        Duration::from_secs(5),
    );
}

#[test]
fn retired_local_subscription_history_does_not_override_current_central_runtime() {
    let mut fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    let first_id = first["identity"]["worker_id"].as_str().unwrap().to_owned();
    fixture.stop_daemon();
    let legacy_journal = fixture.project.join(".agent-collab/server/journal.jsonl");
    std::fs::create_dir_all(legacy_journal.parent().unwrap()).unwrap();
    std::fs::write(
        &legacy_journal,
        b"incompatible-retired-version\nlegacy-pane-sub\n",
    )
    .unwrap();
    let legacy_before = std::fs::read(&legacy_journal).unwrap();
    remove_host_routes(&fixture);
    let project = fixture.project.clone();
    start_daemon_from(&mut fixture, &project);
    let restored = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(restored["identity"]["worker_id"], first_id);
    let second = fixture.run_public(&["context"], THREAD_B);
    let second_id = second["identity"]["worker_id"].as_str().unwrap();
    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            second_id,
            "--subject",
            "legacy replay",
            "message after migration",
        ],
        THREAD_A,
    );
    assert_eq!(sent["durable"], true);
    assert_eq!(std::fs::read(&legacy_journal).unwrap(), legacy_before);
}

#[test]
fn central_storage_survives_restart_and_stays_under_private_host() {
    let mut fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    let second = fixture.run_public(&["context"], THREAD_B);
    let first_id = first["identity"]["worker_id"].as_str().unwrap().to_owned();
    let second_id = second["identity"]["worker_id"].as_str().unwrap().to_owned();
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "central storage acceptance",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "board",
            "publish",
            "central-task",
            "--title",
            "central storage",
            "--description",
            "survive restart",
            "--delivery-condition",
            "central current state active",
            "--test-condition",
            "public CLI round trip",
            "--priority",
            "p1",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "sendmessage",
            "--to",
            &first_id,
            "--subject",
            "central-message",
            "durable mailbox row",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "central:subscription",
            "--every-ms",
            "100",
            "--repeat-count",
            "2",
            "--ttl-seconds",
            "30",
        ],
        THREAD_B,
    );

    let current_path = fixture
        .project_state_for(&fixture.project, "appserver-cli")
        .canonicalize()
        .unwrap();
    assert!(current_path.starts_with(fixture.host_state.canonicalize().unwrap().join("projects")));
    assert!(
        !fixture
            .project
            .join(".agent-collab/server/state.json")
            .exists(),
        "the project marker must not own current runtime state"
    );

    fixture.restart_daemon();

    let restored_first = fixture.run_public(&["context"], THREAD_A);
    let restored_second = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(restored_first["identity"]["worker_id"], first_id);
    assert_eq!(restored_second["identity"]["worker_id"], second_id);
    assert_eq!(restored_first["identity"]["role"], "master");
    let board = fixture.run_public(&["board", "show"], THREAD_B);
    let task = find_task(&board, "central-task");
    assert_eq!(task["publisher"], first_id);
    assert_eq!(task["status"], "pending");
    let inbox = fixture.run_public(&["inbox"], THREAD_A);
    assert!(
        inbox["messages"]
            .as_array()
            .expect("inbox lists messages")
            .iter()
            .any(|message| message["subject"] == "central-message"),
        "mailbox row must survive restart: {inbox}"
    );
    let notifications = fixture.run_public(&["notify", "status"], THREAD_B);
    assert!(
        notifications["subscriptions"]
            .as_array()
            .expect("notification status lists subscriptions")
            .iter()
            .any(|subscription| {
                subscription["subject"] == "central:subscription"
                    && subscription["status"] == "armed"
            }),
        "finite subscription must survive restart: {notifications}"
    );
}

#[test]
fn current_central_recovery_without_host_route_ignores_retired_local_journal() {
    let mut fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    let second = fixture.run_public(&["context"], THREAD_B);
    let first_id = first["identity"]["worker_id"].as_str().unwrap().to_owned();
    let second_id = second["identity"]["worker_id"].as_str().unwrap().to_owned();
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "external legacy migration acceptance",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "board",
            "publish",
            "external-legacy-task",
            "--title",
            "external legacy migration",
            "--description",
            "preserve tasks through first admission",
            "--delivery-condition",
            "legacy state is active",
            "--test-condition",
            "public CLI round trip",
            "--priority",
            "p1",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "sendmessage",
            "--to",
            &first_id,
            "--subject",
            "external-legacy-message",
            "durable legacy mailbox row",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "external-legacy:subscription",
            "--every-ms",
            "100",
            "--repeat-count",
            "2",
            "--ttl-seconds",
            "30",
        ],
        THREAD_B,
    );

    let original_state = fixture.project_state_for(&fixture.project, "appserver-cli");
    assert!(fixture.current_state()["tasks"]["external-legacy-task"].is_object());
    fixture.stop_daemon();
    let legacy_journal = fixture.project.join(".agent-collab/server/journal.jsonl");
    std::fs::create_dir_all(legacy_journal.parent().unwrap()).unwrap();
    std::fs::write(
        &legacy_journal,
        b"incompatible-retired-version\nopaque-external-local-state\n",
    )
    .unwrap();
    let legacy_before = std::fs::read(&legacy_journal).expect("read external legacy journal");
    remove_host_routes(&fixture);

    let host_project = fixture.root.join("host-project");
    std::fs::create_dir_all(host_project.join(".agent-collab"))
        .expect("create host project marker");
    start_daemon_from(&mut fixture, &host_project);

    let restored_first = run_public_in(&fixture, &["context"], THREAD_A, &fixture.project);
    let restored_second = run_public_in(&fixture, &["context"], THREAD_B, &fixture.project);
    assert_eq!(restored_first["identity"]["worker_id"], first_id);
    assert_eq!(restored_second["identity"]["worker_id"], second_id);
    assert_eq!(restored_first["identity"]["role"], "master");

    let board = run_public_in(&fixture, &["board", "show"], THREAD_B, &fixture.project);
    let task = find_task(&board, "external-legacy-task");
    assert_eq!(task["publisher"], first_id);
    let inbox = run_public_in(&fixture, &["inbox"], THREAD_A, &fixture.project);
    assert!(inbox["messages"]
        .as_array()
        .expect("inbox lists messages")
        .iter()
        .any(|message| message["subject"] == "external-legacy-message"));
    let notifications = run_public_in(&fixture, &["notify", "status"], THREAD_B, &fixture.project);
    assert!(notifications["subscriptions"]
        .as_array()
        .expect("notification status lists subscriptions")
        .iter()
        .any(|subscription| {
            subscription["subject"] == "external-legacy:subscription"
                && subscription["status"] == "armed"
        }));

    // This scenario intentionally has no persisted route. The path captured
    // before removing it remains the declared current storage owner.
    let restored: Value = serde_json::from_slice(&std::fs::read(&original_state).unwrap()).unwrap();
    assert!(restored["state"]["tasks"]["external-legacy-task"].is_object());
    assert!(original_state
        .canonicalize()
        .unwrap()
        .starts_with(fixture.host_state.canonicalize().unwrap().join("projects")));
    assert_eq!(
        std::fs::read(&legacy_journal).expect("read preserved external legacy journal"),
        legacy_before,
        "the external legacy source must remain unchanged"
    );
}

#[test]
fn stale_local_storage_reference_fails_without_importing_retired_data() {
    let mut fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    let second = fixture.run_public(&["context"], THREAD_B);
    let first_id = first["identity"]["worker_id"].as_str().unwrap().to_owned();
    let second_id = second["identity"]["worker_id"].as_str().unwrap().to_owned();
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "legacy migration acceptance",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "board",
            "publish",
            "legacy-task",
            "--title",
            "legacy migration",
            "--description",
            "preserve durable state",
            "--delivery-condition",
            "central route active",
            "--test-condition",
            "source remains unchanged",
            "--priority",
            "p1",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "sendmessage",
            "--to",
            &first_id,
            "--subject",
            "legacy-message",
            "durable legacy mailbox row",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "notify",
            "subscribe",
            "--event",
            "deadline",
            "--subject",
            "legacy:subscription",
            "--every-ms",
            "100",
            "--repeat-count",
            "2",
            "--ttl-seconds",
            "30",
        ],
        THREAD_B,
    );

    let current_path = fixture.project_state_for(&fixture.project, "appserver-cli");
    let current_before = std::fs::read(&current_path).unwrap();
    let routes_path = fixture.host_state.join("routes.json");
    let valid_routes = std::fs::read(&routes_path).unwrap();
    let map_path = fixture.host_state.join("identity-map.json");
    let current_map = std::fs::read(&map_path).unwrap();
    fixture.stop_daemon();
    point_route_at(&fixture, &fixture.project);
    let legacy_journal = fixture.project.join(".agent-collab/server/journal.jsonl");
    std::fs::create_dir_all(legacy_journal.parent().unwrap()).unwrap();
    std::fs::write(
        &legacy_journal,
        b"incompatible-retired-version\nopaque-local-data\n",
    )
    .unwrap();
    let legacy_before = std::fs::read(&legacy_journal).expect("read legacy journal");
    let invalid_routes = std::fs::read(&routes_path).unwrap();
    let log_path = fixture.root.join("invalid-current-route.log");
    let log = std::fs::File::create(&log_path).unwrap();
    fixture.daemon = Some(
        Command::new(binary())
            .arg("serve")
            .current_dir(&fixture.project)
            .env("COLLAB_STATE_DIR", &fixture.host_state)
            .env("HOME", fixture.root.join("home"))
            .env("CODEX_HOME", fixture.root.join("home"))
            .env_remove("COLLAB_APPSERVER_SOCKET")
            .env_remove("CODEX_APP_SERVER_SOCKET")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = fixture.daemon.as_mut().unwrap().try_wait().unwrap() {
            assert!(!status.success(), "local current storage must fail startup");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "invalid current storage did not fail startup"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert!(std::fs::read_to_string(log_path)
        .unwrap()
        .contains("CURRENT_STATE_MIGRATION_REQUIRED"));
    assert!(!fixture.host_state.join("server.sock").exists());
    assert_eq!(std::fs::read(&current_path).unwrap(), current_before);
    assert_eq!(std::fs::read(&routes_path).unwrap(), invalid_routes);
    assert_eq!(std::fs::read(&map_path).unwrap(), current_map);
    assert_eq!(std::fs::read(&legacy_journal).unwrap(), legacy_before);
    fixture.stop_daemon();
    std::fs::write(&routes_path, valid_routes).unwrap();
    fixture.start_daemon();
    let restored_first = fixture.run_public(&["context"], THREAD_A);
    let restored_second = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(restored_first["identity"]["worker_id"], first_id);
    assert_eq!(restored_second["identity"]["worker_id"], second_id);
    assert_eq!(restored_first["identity"]["role"], "master");
    let board = fixture.run_public(&["board", "show"], THREAD_B);
    let task = find_task(&board, "legacy-task");
    assert_eq!(task["publisher"], first_id);
    let inbox = fixture.run_public(&["inbox"], THREAD_A);
    assert!(inbox["messages"]
        .as_array()
        .expect("inbox lists messages")
        .iter()
        .any(|message| message["subject"] == "legacy-message"));
    let notifications = fixture.run_public(&["notify", "status"], THREAD_B);
    assert!(notifications["subscriptions"]
        .as_array()
        .expect("subscriptions list")
        .iter()
        .any(|subscription| {
            subscription["subject"] == "legacy:subscription" && subscription["status"] == "armed"
        }));

    assert_eq!(
        fixture.project_state_for(&fixture.project, "appserver-cli"),
        current_path
    );
    assert!(current_path
        .canonicalize()
        .unwrap()
        .starts_with(fixture.host_state.canonicalize().unwrap().join("projects")));
    fixture.run_public(
        &[
            "board",
            "publish",
            "post-migration-task",
            "--title",
            "post migration",
            "--description",
            "new writer location",
            "--delivery-condition",
            "central only",
            "--test-condition",
            "legacy journal unchanged",
            "--priority",
            "p1",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "sendmessage",
            "--to",
            &second_id,
            "--subject",
            "post-migration-message",
            "central only",
        ],
        THREAD_A,
    );
    let current = fixture.current_state();
    assert!(current["tasks"]["post-migration-task"].is_object());
    assert!(current["msgs"]
        .as_object()
        .unwrap()
        .values()
        .any(|message| message["subject"] == "post-migration-message"
            && message["body"] == "central only"));
    assert_eq!(
        std::fs::read(&legacy_journal).expect("read preserved legacy journal"),
        legacy_before,
        "the legacy source must not receive a second writer"
    );

    fixture.run_public(&["reset", "--project"], THREAD_A);
    match std::fs::read(&current_path) {
        Ok(bytes) => {
            let document: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(document["state"]["tasks"].as_object().unwrap().is_empty());
            assert!(document["state"]["msgs"].as_object().unwrap().is_empty());
            assert!(document["state"]["notification_subscriptions"]
                .as_object()
                .unwrap()
                .is_empty());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("read reset current state: {error}"),
    }
    assert!(
        fixture.host_state.join("daemon.lock").exists()
            && fixture.host_state.join("server.sock").exists(),
        "scoped reset must preserve the host daemon"
    );
}
