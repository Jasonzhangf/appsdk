include!("support/tmux_cli_fixture.rs");

#[test]
fn collab_recv_cli_subprocess_commits_queryable_receipt_over_isolated_daemon() {
    let root = unique_root();
    let host_state = root.join("h");
    let tmux_socket = root.join("t.sock");
    std::fs::create_dir_all(&root).expect("create isolated project root");
    std::fs::create_dir_all(&host_state).expect("create isolated host state root");
    seed_baseline(&root);
    let mut fixture = Fixture {
        binary: collab_test_binary(),
        root: root.clone(),
        host_state,
        tmux_socket: tmux_socket.clone(),
        initialized: false,
    };

    tmux(
        &tmux_socket,
        &[
            "new-session",
            "-d",
            "-s",
            "collab-tmux-recv-e2e",
            "sleep 600",
        ],
    );
    tmux(
        &tmux_socket,
        &[
            "split-window",
            "-d",
            "-t",
            "collab-tmux-recv-e2e:0",
            "sleep 600",
        ],
    );
    let server_pid =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pid}"]).stdout)
            .expect("tmux server pid is utf8")
            .trim()
            .parse::<u32>()
            .expect("tmux server pid is numeric");
    let pane_ids =
        String::from_utf8(tmux(&tmux_socket, &["list-panes", "-a", "-F", "#{pane_id}"]).stdout)
            .expect("pane list is utf8");
    let pane_ids = pane_ids.lines().collect::<Vec<_>>();
    assert_eq!(pane_ids.len(), 2, "fixture owns exactly two panes");
    let pane = |pane_id: &str| Pane {
        server_pid,
        pane_id: pane_id.to_owned(),
        session_anchor: format!("session-{pane_id}"),
        thread_anchor: format!("thread-{pane_id}"),
    };
    let sender = pane(pane_ids[0]);
    let receiver = pane(pane_ids[1]);

    fixture.initialized = true;
    let unknown_host =
        fixture.command_with_originator(&["context"], Some(&sender), "Codex future host");
    assert!(
        unknown_host.status.success(),
        "context with a verified tmux candidate must return a snapshot or classified terminal: stdout={} stderr={}",
        String::from_utf8_lossy(&unknown_host.stdout),
        String::from_utf8_lossy(&unknown_host.stderr)
    );
    let unknown_host_registration: Value =
        serde_json::from_slice(&unknown_host.stdout).expect("collab CLI emits JSON");
    assert_eq!(
        unknown_host_registration["identity"]["transport"]["kind"],
        "tmux"
    );
    let receiver_context = fixture.run_context(&["context"], Some(&receiver));
    let sender_context = fixture.run_context(&["context"], Some(&sender));
    let receiver_id = receiver_context["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names receiver worker")
        .to_owned();
    let sender_id = sender_context["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names sender worker")
        .to_owned();

    let route = fixture.run_ok(
        &["route", "resolve", "--pane-id", &sender.pane_id],
        Some(&sender),
    );
    assert_eq!(
        route["tmux_endpoint"]["socket_path"],
        tmux_socket.to_string_lossy().as_ref()
    );
    assert_eq!(route["tmux_endpoint"]["server_pid"], server_pid);
    assert_eq!(route["tmux_endpoint"]["pane_id"], sender.pane_id);
    assert_eq!(route["session_id"], sender.session_anchor);
    assert_eq!(route["native_thread_id"], sender.thread_anchor);

    let sent = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &receiver_id,
            "--subject",
            "tmux recv e2e",
            "persist before consume",
        ],
        Some(&sender),
    );
    let message_id = sent["msg_id"]
        .as_str()
        .or_else(|| sent["message_id"].as_str())
        .expect("send response has durable message id")
        .to_owned();
    assert_eq!(
        sent["consumed"], false,
        "wake acceptance is not consumption"
    );

    let received = fixture.run_ok(
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "tmux-e2e-receive-1",
        ],
        Some(&receiver),
    );
    assert_eq!(received["count"], 1);
    assert_eq!(received["messages"][0]["id"], message_id);
    assert_eq!(received["receive_id"], "tmux-e2e-receive-1");

    let status = fixture.run_ok(&["msg", &message_id], Some(&sender));
    assert_eq!(status["consumed_by_recv"], true);
    assert_eq!(status["state"], "read");
    assert_eq!(sender_context["identity"]["worker_id"], sender_id);
    assert_eq!(receiver_context["identity"]["worker_id"], receiver_id);

    // Register an owned task and leave one durable message unread so the
    // restart proves tasks and mailbox survive, not only the identity.
    let task = fixture.run_ok(
        &[
            "task",
            "register",
            "tmux-e2e-task",
            "--next",
            "survive restart",
        ],
        Some(&sender),
    );
    assert_eq!(task["task"], "tmux-e2e-task");
    let preserved = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &receiver_id,
            "--subject",
            "preserve across restart",
            "durable mailbox",
        ],
        Some(&sender),
    );
    let preserved_id = preserved["msg_id"]
        .as_str()
        .or_else(|| preserved["message_id"].as_str())
        .expect("preserved send has a durable message id")
        .to_owned();

    // Daemon restart and lost local receipt: the same actual tmux anchor must
    // return the same worker and generation without a fresh identity.
    let sender_identity = sender_context["identity"].clone();
    let sender_generation = sender_context["binding"]["endpoint_generation"].clone();
    let down = fixture.command_without_pane(&["down"]);
    assert!(down.status.success(), "isolated fixture down must succeed");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && fixture.host_state.join("server.sock").exists() {
        thread::sleep(Duration::from_millis(50));
    }
    std::fs::remove_dir_all(fixture.host_state.join("identities")).unwrap();
    let up = fixture.command_without_pane(&["up"]);
    assert!(up.status.success(), "isolated fixture up must succeed");
    let restart_context = fixture.run_context(&["context"], Some(&sender));
    assert_eq!(restart_context["identity"]["worker_id"], sender_id);
    assert_eq!(
        restart_context["binding"]["endpoint_generation"],
        sender_generation
    );
    assert_eq!(
        restart_context["identity"], sender_identity,
        "same tmux anchor must restore the same identity"
    );
    assert!(
        restart_context["subscriptions"]
            .as_array()
            .expect("context lists the caller's subscriptions")
            .iter()
            .any(|subscription| subscription["event"] == "direct-message"
                && subscription["status"] == "armed"),
        "the default direct-message lease must be armed again after restart: {restart_context}"
    );

    // Mailbox and tasks are daemon state, not receipt state: they survive the
    // restart together with the recovered identity.
    let tasks = restart_context["tasks"]
        .as_array()
        .expect("context lists the caller's tasks");
    assert!(
        tasks
            .iter()
            .any(|task| task["id"] == "tmux-e2e-task" && task["owner"] == sender_id),
        "the owned task must survive the daemon restart: {restart_context}"
    );
    let preserved_recv = fixture.run_ok(
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "tmux-e2e-receive-2",
        ],
        Some(&receiver),
    );
    let preserved_ids = preserved_recv["messages"]
        .as_array()
        .expect("recv returns messages")
        .iter()
        .filter_map(|message| message["id"].as_str())
        .collect::<Vec<_>>();
    assert!(
        preserved_ids.contains(&preserved_id.as_str()),
        "unread mailbox must survive the daemon restart: {preserved_recv}"
    );

    // No `collab context` display path may leak the private credential or the
    // internal receipt: not the first registration, not the receipt-derived
    // reads, and not the lost-receipt restart rebuild.
    assert_context_display_is_public("unknown-host context", &unknown_host);
    assert_context_display_is_public(
        "sender context",
        &fixture.command(&["context"], Some(&sender)),
    );
    assert_context_display_is_public(
        "receiver context",
        &fixture.command(&["context"], Some(&receiver)),
    );
    assert_context_display_is_public(
        "restart context",
        &fixture.command(&["context"], Some(&sender)),
    );
}

#[test]
fn context_recovers_archived_master_in_the_same_live_pane() {
    let root = unique_root();
    let host_state = root.join("h");
    let tmux_socket = root.join("t.sock");
    std::fs::create_dir_all(&host_state).unwrap();
    seed_baseline(&root);
    let mut fixture = Fixture {
        binary: collab_test_binary(),
        root: root.clone(),
        host_state,
        tmux_socket: tmux_socket.clone(),
        initialized: false,
    };
    tmux(
        &tmux_socket,
        &[
            "new-session",
            "-d",
            "-s",
            "collab-pane-recovery",
            "sleep 600",
        ],
    );
    let server_pid =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pid}"]).stdout)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
    let pane_id =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pane_id}"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
    let old = Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: "session-before-restart".into(),
        thread_anchor: "thread-before-restart".into(),
    };
    fixture.initialized = true;
    // The daemon owns the worker id; the fixture must not assume a naming
    // scheme. Read the concrete worker from the registration receipt.
    let init = fixture.run_ok(&["init"], Some(&old));
    let worker_id = init["worker_id"]
        .as_str()
        .expect("init receipt names the daemon-owned worker")
        .to_owned();
    fixture.run_ok(
        &[
            "master",
            "promote",
            "--approval",
            "user approved isolated master",
        ],
        Some(&old),
    );
    let archive = fixture
        .host_state
        .join("archives/identities-retired-1")
        .join(&worker_id);
    std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
    std::fs::rename(
        fixture.host_state.join("identities").join(&worker_id),
        &archive,
    )
    .unwrap();
    let new = Pane {
        session_anchor: "session-after-restart".into(),
        thread_anchor: "thread-after-restart".into(),
        ..old
    };
    let recovered = fixture.run_ok(&["context"], Some(&new));
    assert_eq!(recovered["identity"]["worker_id"], worker_id);
    assert_eq!(recovered["identity"]["role"], "master");
    assert_eq!(recovered["registered"], true);
    let again = fixture.run_ok(&["context"], Some(&new));
    assert_eq!(again["identity"]["worker_id"], worker_id);
    assert_eq!(
        again["binding"]["endpoint_generation"],
        recovered["binding"]["endpoint_generation"]
    );

    let sent = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &worker_id,
            "--subject",
            "recovery",
            "consume me",
        ],
        Some(&new),
    );
    let message_id = sent["msg_id"]
        .as_str()
        .or_else(|| sent["message_id"].as_str())
        .unwrap();
    let received = fixture.run_ok(
        &["recv", "--timeout", "0", "--receive-id", "recovery-recv-1"],
        Some(&new),
    );
    assert_eq!(received["messages"][0]["id"], message_id);
    let status = fixture.run_ok(&["msg", message_id], Some(&new));
    assert_eq!(status["consumed_by_recv"], true);
}

/// The implicit bootstrap path — `collab context` run from a pane with no
/// `--worker` and no `COLLAB_WORKER` — resolves its durable identity from the
/// live anchor and can then be rejected by the daemon. The terminal must name
/// that resolved identity: before, it echoed the caller's optional `--worker`,
/// which is absent on this path, so the agent got `worker_id: null` plus a
/// `<worker_id>` placeholder and could not run the repair it was handed.
#[test]
fn implicit_context_names_the_resolved_identity_when_the_daemon_rejects_its_token() {
    let root = unique_root();
    let host_state = root.join("h");
    let tmux_socket = root.join("t.sock");
    std::fs::create_dir_all(&root).expect("create isolated project root");
    std::fs::create_dir_all(&host_state).expect("create isolated host state root");
    seed_baseline(&root);
    let mut fixture = Fixture {
        binary: collab_test_binary(),
        root: root.clone(),
        host_state: host_state.clone(),
        tmux_socket: tmux_socket.clone(),
        initialized: false,
    };
    tmux(
        &tmux_socket,
        &[
            "new-session",
            "-d",
            "-s",
            "collab-implicit-context",
            "sleep 600",
        ],
    );
    let server_pid =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pid}"]).stdout)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
    let pane_id =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pane_id}"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
    let pane = Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: "session-implicit".into(),
        thread_anchor: "thread-implicit".into(),
    };
    fixture.initialized = true;
    // The daemon owns the worker id; read the concrete value from the init
    // receipt instead of re-deriving the old naming scheme.
    let init = fixture.run_ok(&["init"], Some(&pane));
    let worker_id = init["worker_id"]
        .as_str()
        .expect("init receipt names the daemon-owned worker")
        .to_owned();

    // Durable identity exists and is registered; now make its token unusable so
    // the daemon rejects the first authenticated call.
    let identity_path = host_state
        .join("identities")
        .join(&worker_id)
        .join("identity.json");
    let mut identity: Value =
        serde_json::from_slice(&std::fs::read(&identity_path).unwrap()).unwrap();
    identity["token"] = Value::String("not-the-recorded-token".into());
    std::fs::write(
        &identity_path,
        serde_json::to_vec_pretty(&identity).unwrap(),
    )
    .unwrap();

    // Implicit: no `--worker`, and COLLAB_WORKER is deliberately removed while
    // the tmux anchor stays, so the identity is resolved from the anchor alone.
    let output = Command::new(&fixture.binary)
        .arg("context")
        .current_dir(&fixture.root)
        .env("COLLAB_STATE_DIR", &fixture.host_state)
        .env(
            "TMUX",
            format!("{},{},0", tmux_socket.display(), pane.server_pid),
        )
        .env("TMUX_PANE", &pane.pane_id)
        .env("CODEX_SESSION_ID", &pane.session_anchor)
        .env("CODEX_THREAD_ID", &pane.thread_anchor)
        .output()
        .expect("run implicit collab context");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "a rejected stored credential must fail: stdout={stdout} stderr={stderr}"
    );
    assert!(stderr.contains("TOKEN_MISMATCH:"), "{stderr}");
    assert!(
        stderr.contains(&worker_id),
        "the error must name the rejected identity: {stderr}"
    );
    assert!(
        !stderr.contains("IDENTITY_INFORMATION_REQUIRED"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("not-the-recorded-token"),
        "the rejected credential must not leak"
    );
    let after: Value = serde_json::from_slice(&std::fs::read(&identity_path).unwrap()).unwrap();
    assert_eq!(
        after["token"], identity["token"],
        "the daemon must not replace a rejected credential"
    );
}

/// `collab context` is the bootstrap read, so it is also where a peer reads
/// back its own runtime binding. A peer that loses the registration receipt
/// otherwise cannot address its route again: the endpoint generation then
/// appears only inside a rejection message, and recovering from there would
/// mean inferring a control value from an error.
#[test]
fn context_returns_the_binding_receipt_that_addresses_the_route() {
    let root = unique_root();
    let host_state = root.join("h");
    let tmux_socket = root.join("t.sock");
    std::fs::create_dir_all(&host_state).unwrap();
    seed_baseline(&root);
    let mut fixture = Fixture {
        binary: collab_test_binary(),
        root: root.clone(),
        host_state,
        tmux_socket: tmux_socket.clone(),
        initialized: false,
    };
    tmux(
        &tmux_socket,
        &[
            "new-session",
            "-d",
            "-s",
            "collab-context-binding",
            "sleep 600",
        ],
    );
    let server_pid =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pid}"]).stdout)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
    let pane_id =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pane_id}"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
    let pane = Pane {
        server_pid,
        pane_id: pane_id.clone(),
        session_anchor: "session-binding".into(),
        thread_anchor: "thread-binding".into(),
    };
    fixture.initialized = true;
    let registered = fixture.run_ok(&["context"], Some(&pane));
    let worker_id = registered["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names the registered worker")
        .to_owned();

    let context = fixture.run_ok(&["context"], Some(&pane));
    let binding = &context["binding"];
    let canonical_root = root.canonicalize().unwrap().to_string_lossy().into_owned();
    assert_eq!(binding["agent_id"], worker_id);
    assert_eq!(binding["project_scope"], canonical_root);
    assert!(
        binding["app_scope_id"]
            .as_str()
            .is_some_and(|scope| !scope.is_empty()),
        "the receipt must name its app scope: {context}"
    );
    assert_eq!(binding["session_id"], pane.session_anchor);
    assert_eq!(binding["native_thread_id"], pane.thread_anchor);
    assert!(
        binding["runtime_id"]
            .as_str()
            .is_some_and(|runtime_id| !runtime_id.is_empty()),
        "the receipt must carry a runtime id"
    );
    assert_eq!(registered["binding"]["runtime_id"], binding["runtime_id"]);
    assert!(
        binding["binding_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "the receipt must name its binding: {context}"
    );
    assert!(
        binding["endpoint_generation"]
            .as_u64()
            .is_some_and(|generation| generation >= 1),
        "the receipt must carry a usable endpoint generation: {context}"
    );

    // The receipt is the daemon's record, not a copy of the caller's local
    // state: reading it again returns the same binding.
    let again = fixture.run_ok(&["context"], Some(&pane));
    assert_eq!(again["binding"], *binding);
}

#[test]
fn context_missing_facts_and_invalid_supplement_have_no_identity_side_effect() {
    let root = unique_root();
    let host_state = root.join("h");
    std::fs::create_dir_all(&host_state).unwrap();
    seed_baseline(&root);
    let mut fixture = Fixture {
        binary: collab_test_binary(),
        root: root.clone(),
        host_state: host_state.clone(),
        tmux_socket: root.join("unused.sock"),
        initialized: false,
    };
    fixture.initialized = true;

    // A caller with no anchor at all: no tmux pane, no session/thread, and an
    // unsupported originator so no namespace is observed either. The daemon
    // must name every absent factual field and must not guess a worker.
    let no_anchor = fixture.command_with_originator(&["context"], None, "Codex future host");
    assert!(
        no_anchor.status.success(),
        "missing facts are a classified success terminal: stdout={} stderr={}",
        String::from_utf8_lossy(&no_anchor.stdout),
        String::from_utf8_lossy(&no_anchor.stderr)
    );
    let no_anchor_snapshot: Value = serde_json::from_slice(&no_anchor.stdout).unwrap();
    assert_eq!(no_anchor_snapshot["registered"], false);
    let update = &no_anchor_snapshot["requires_identity_update"];
    assert_eq!(update["reason"], "IDENTITY_INFORMATION_REQUIRED");
    assert_eq!(
        update["required_fields"],
        json!(["session_id", "thread_id", "endpoint", "namespace"]),
        "no-anchor context returns every exact missing native fact: {no_anchor_snapshot}"
    );
    assert!(
        update["worker_id"].is_null(),
        "missing-facts terminal must not guess a worker"
    );
    assert_eq!(
        update["action"],
        json!("collab context --provide '<JSON containing required_fields>'")
    );

    // A partial anchor: the runtime is a recognized TUI, so namespace is
    // observed from the originator. The daemon must not re-request observed
    // facts; it asks only for the session, thread and endpoint it still lacks.
    let partial = fixture.command(&["context"], None);
    assert!(
        partial.status.success(),
        "partial native facts are a classified success terminal: stdout={} stderr={}",
        String::from_utf8_lossy(&partial.stdout),
        String::from_utf8_lossy(&partial.stderr)
    );
    let partial_snapshot: Value = serde_json::from_slice(&partial.stdout).unwrap();
    assert_eq!(partial_snapshot["registered"], false);
    assert_eq!(
        partial_snapshot["requires_identity_update"]["required_fields"],
        json!(["session_id", "thread_id", "endpoint"]),
        "observed namespace must not be re-requested: {partial_snapshot}"
    );
    assert!(
        partial_snapshot["requires_identity_update"]["worker_id"].is_null(),
        "partial terminal must not guess a worker: {partial_snapshot}"
    );

    // Unknown, duplicate and empty supplements are rejected before the daemon
    // creates any identity, so a bad claim can never become a registered peer.
    for (label, provide) in [
        ("unknown field", "{\"token\":\"x\"}"),
        (
            "duplicate field",
            "{\"session_id\":\"a\",\"session_id\":\"b\"}",
        ),
        ("empty value", "{\"session_id\":\"\"}"),
    ] {
        let invalid = fixture.command_without_pane(&["context", "--provide", provide]);
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&invalid.stdout),
            String::from_utf8_lossy(&invalid.stderr)
        );
        assert!(
            !invalid.status.success(),
            "{label} supplement must fail closed: {combined}"
        );
        assert!(
            combined.contains("IDENTITY_FACT_INVALID"),
            "{label} supplement must be rejected explicitly: {combined}"
        );
    }

    let identities = host_state.join("identities");
    let created = identities.exists()
        && std::fs::read_dir(&identities)
            .expect("read isolated identities dir")
            .next()
            .is_some();
    assert!(
        !created,
        "rejected supplements must not create an identity: {identities:?}"
    );
}

/// The identity selector and the client-side bootstrap/mint surface are gone.
/// Each removed command or flag must fail at argument parsing, before any
/// daemon or identity side effect.
#[test]
fn removed_identity_commands_and_flags_are_rejected_by_the_cli() {
    let root = unique_root();
    std::fs::create_dir_all(&root).unwrap();
    let binary = collab_test_binary();
    let cases: &[&[&str]] = &[
        &["context", "--worker", "some-worker"],
        &["init", "--worker-id", "some-worker"],
        &["whoami"],
        &["worker", "recover"],
        &["recv", "--worker", "some-worker"],
        &[
            "send",
            "--from",
            "some-worker",
            "--to",
            "x",
            "--subject",
            "y",
            "body",
        ],
        &["inbox", "--worker", "some-worker"],
        &["ack", "m1", "--worker", "some-worker"],
        &["mailbox", "read", "--worker", "some-worker"],
    ];
    for case in cases {
        let output = Command::new(&binary)
            .args(*case)
            .current_dir(&root)
            .env("COLLAB_STATE_DIR", root.join("h"))
            .env("CODEX_HOME", root.join("home"))
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("CODEX_THREAD_ID")
            .output()
            .expect("run collab CLI");
        assert!(
            !output.status.success(),
            "removed identity surface must be rejected: {case:?}"
        );
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            combined.contains("error")
                || combined.contains("unexpected")
                || combined.contains("unrecognized"),
            "removed identity surface must produce a parse rejection: {case:?} => {combined}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
