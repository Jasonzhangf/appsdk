use super::*;

fn runtime_journals(root: &std::path::Path) -> Vec<PathBuf> {
    fn visit(path: &std::path::Path, found: &mut Vec<PathBuf>) {
        let entries = match std::fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => panic!("read {}: {error}", path.display()),
        };
        for entry in entries {
            let path = entry.expect("read directory entry").path();
            if path.is_dir() {
                visit(&path, found);
            } else if path.file_name().is_some_and(|name| name == "journal.jsonl")
                && path
                    .parent()
                    .and_then(std::path::Path::file_name)
                    .is_some_and(|name| name == "server")
            {
                found.push(path);
            }
        }
    }

    let mut found = Vec::new();
    visit(root, &mut found);
    found.sort();
    found
}

fn find_task<'a>(board: &'a Value, id: &str) -> &'a Value {
    board["tasks"]
        .as_array()
        .expect("board lists tasks")
        .iter()
        .find(|task| task["id"] == id)
        .unwrap_or_else(|| panic!("board contains task {id}: {board}"))
}

fn move_runtime_data(source: &std::path::Path, destination: &std::path::Path) {
    std::fs::create_dir_all(destination).expect("create legacy runtime root");
    for name in ["server", "mailbox"] {
        let from = source.join(".agent-collab").join(name);
        if !from.exists() {
            continue;
        }
        let to = destination.join(".agent-collab").join(name);
        if to.exists() {
            std::fs::remove_dir_all(&to).expect("remove destination runtime directory");
        }
        std::fs::rename(&from, &to).expect("move runtime data");
    }
}

fn point_route_at(fixture: &AppFixture, storage_root: &std::path::Path) {
    let path = fixture.host_state.join("routes.jsonl");
    let mut records = std::fs::read_to_string(&path)
        .expect("read route journal")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("parse route record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1, "{records:?}");
    records[0]["storage_root"] = Value::String(storage_root.to_string_lossy().into_owned());
    let mut body = records
        .into_iter()
        .map(|record| serde_json::to_string(&record).expect("serialize route record"))
        .collect::<Vec<_>>()
        .join("\n");
    body.push('\n');
    std::fs::write(path, body).expect("write route journal fixture");
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
        .env("COLLAB_APPSERVER_SOCKET", &fixture.app_socket)
        .env_remove("CODEX_APP_SERVER_SOCKET")
        .env_remove("COLLAB_APPSERVER_NAMESPACE")
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
        .env("CODEX_SESSION_ID", thread)
        .env("CODEX_THREAD_ID", thread)
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
        .env("CODEX_HOME", fixture.root.join("home"))
        .env_remove("COLLAB_APPSERVER_SOCKET")
        .env_remove("CODEX_APP_SERVER_SOCKET")
        .env_remove("COLLAB_APPSERVER_NAMESPACE")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("COLLAB_WORKER")
        .stdin(std::process::Stdio::null())
        .stdout(daemon_log.try_clone().expect("clone serve log"))
        .stderr(daemon_log);
    fixture.daemon = Some(command.spawn().expect("spawn owned serve child"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.host_state.join("server.sock").exists() && Instant::now() < deadline {
        if let Some(status) = fixture.daemon.as_mut().unwrap().try_wait().unwrap() {
            panic!(
                "test-owned serve exited before readiness: {status}: {}",
                std::fs::read_to_string(fixture.root.join("test-daemon-host.log"))
                    .unwrap_or_default()
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
    assert!(
        fixture.host_state.join("server.sock").exists(),
        "owned serve did not create socket"
    );
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
            "central journal active",
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

    let host_journals = runtime_journals(&fixture.host_state);
    assert!(
        !host_journals.is_empty(),
        "host-central runtime journal is absent under {}",
        fixture.host_state.display()
    );
    assert!(
        host_journals
            .iter()
            .all(|path| path.starts_with(fixture.host_state.join("projects"))),
        "runtime journals must use the host project namespace: {host_journals:?}"
    );
    assert_eq!(
        runtime_journals(&fixture.project),
        Vec::<PathBuf>::new(),
        "the project marker must not own runtime journals"
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
fn external_legacy_without_host_route_migrates_before_fresh_central_runtime() {
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

    let original_journal = runtime_journals(&fixture.host_state)
        .into_iter()
        .find(|journal| {
            std::fs::read_to_string(journal).is_ok_and(|body| body.contains("external-legacy-task"))
        })
        .expect("central journal before external migration");
    let central_root = original_journal
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .expect("central storage root")
        .to_path_buf();
    fixture.stop_daemon();
    move_runtime_data(&central_root, &fixture.project);
    let legacy_journal = fixture.project.join(".agent-collab/server/journal.jsonl");
    let legacy_before = std::fs::read(&legacy_journal).expect("read external legacy journal");
    std::fs::remove_file(fixture.host_state.join("routes.jsonl"))
        .expect("remove host route before first admission");
    std::fs::remove_dir_all(&central_root).expect("remove central destination");

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

    let migrated_journal = runtime_journals(&fixture.host_state)
        .into_iter()
        .find(|journal| {
            std::fs::read_to_string(journal).is_ok_and(|body| body.contains("external-legacy-task"))
        })
        .expect("central journal after external migration");
    assert!(std::fs::read(&migrated_journal)
        .expect("read migrated central journal")
        .starts_with(&legacy_before));
    assert_eq!(
        std::fs::read(&legacy_journal).expect("read preserved external legacy journal"),
        legacy_before,
        "the external legacy source must remain unchanged"
    );
}

#[test]
fn legacy_runtime_migrates_without_becoming_a_second_writer() {
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

    let original_journal = runtime_journals(&fixture.host_state)
        .into_iter()
        .next()
        .expect("central journal before migration");
    let central_root = original_journal
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .expect("central storage root")
        .to_path_buf();
    fixture.stop_daemon();
    move_runtime_data(&central_root, &fixture.project);
    point_route_at(&fixture, &fixture.project);
    std::fs::remove_dir_all(&central_root).expect("remove pre-migration destination");
    std::fs::create_dir_all(central_root.join(".agent-collab/server"))
        .expect("seed interrupted migration destination");
    std::fs::write(
        central_root.join(".agent-collab/server/journal.jsonl"),
        b"{this is an unactivated partial destination}\n",
    )
    .expect("seed partial destination journal");
    let legacy_journal = fixture.project.join(".agent-collab/server/journal.jsonl");
    let legacy_before = std::fs::read(&legacy_journal).expect("read legacy journal");

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

    let migrated = runtime_journals(&fixture.host_state)
        .into_iter()
        .next()
        .expect("central journal after migration");
    assert!(
        migrated.starts_with(fixture.host_state.join("projects")),
        "migrated journal is outside the private host: {}",
        migrated.display()
    );
    let routes = std::fs::read_to_string(fixture.host_state.join("routes.jsonl"))
        .expect("read migrated route");
    let route: Value = serde_json::from_str(routes.trim()).expect("parse migrated route");
    let canonical_host =
        std::fs::canonicalize(&fixture.host_state).expect("canonicalize private host root");
    let canonical_route = std::fs::canonicalize(route["storage_root"].as_str().unwrap())
        .expect("canonicalize migrated route");
    assert!(
        canonical_route.starts_with(canonical_host.join("projects")),
        "migration must activate the central route before writes: {route}"
    );
    let migrated_before_writes = std::fs::read(&migrated).expect("read migrated journal");
    assert!(
        migrated_before_writes.starts_with(&legacy_before),
        "migration must preserve the original journal history byte for byte"
    );
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
    assert_ne!(
        std::fs::read(&migrated).expect("read migrated journal after writes"),
        migrated_before_writes,
        "new writes must reach the central journal"
    );
    assert_eq!(
        std::fs::read(&legacy_journal).expect("read preserved legacy journal"),
        legacy_before,
        "the legacy source must not receive a second writer"
    );

    fixture.run_public(&["reset", "--project"], THREAD_A);
    let after_reset = std::fs::read_to_string(&migrated).unwrap_or_default();
    assert!(
        !after_reset.contains("legacy-task")
            && !after_reset.contains("legacy-message")
            && !after_reset.contains("legacy:subscription"),
        "scoped reset must clear this project's centralized runtime: {}",
        migrated.display()
    );
    assert!(
        fixture.host_state.join("daemon.lock").exists()
            && fixture.host_state.join("server.sock").exists(),
        "scoped reset must preserve the host daemon"
    );
}
