include!("support/tmux_cli_fixture.rs");

fn context_snapshot(value: &Value) -> &Value {
    value
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .unwrap_or(value)
}

fn pane_only_context(fixture: &Fixture, pane: &Pane, args: &[&str]) -> Value {
    let provide = json!({"session_id": pane.session_anchor}).to_string();
    let mut args = args.to_vec();
    args.extend(["--provide", provide.as_str()]);
    let output = fixture
        .configured_command(&args, Some(pane), "Codex TUI")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_THREAD")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result"]["outcome"], "completed", "{value}");
    for receipt in ["route", "credential", "lease"] {
        assert!(
            value["result"]["committed_phases"]
                .as_array()
                .unwrap_or_else(|| panic!("missing public committed phases: {value}"))
                .contains(&json!(receipt)),
            "missing {receipt}: {value}"
        );
    }
    context_snapshot(&value).clone()
}

#[test]
fn collab_recv_cli_subprocess_commits_queryable_receipt_over_isolated_daemon() {
    let root = unique_root();
    let host_state = root.join("h");
    let tmux_socket = root.join("t.sock");
    std::fs::create_dir_all(&root).expect("create isolated project root");
    std::fs::create_dir_all(&host_state).expect("create isolated host state root");
    seed_baseline(&root);
    let sender_session = "collab-tmux-recv-sender";
    let receiver_session = "collab-tmux-recv-receiver";
    let mut fixture = Fixture {
        binary: collab_test_binary(),
        root: root.clone(),
        host_state,
        tmux_socket: tmux_socket.clone(),
        daemon: None,
        owned_sessions: vec![sender_session.into(), receiver_session.into()],
    };

    let sender_pane = tmux(
        &tmux_socket,
        &[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            sender_session,
            "sleep 600",
        ],
    );
    let receiver_pane = tmux(
        &tmux_socket,
        &[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            receiver_session,
            "sleep 600",
        ],
    );
    let server_pid =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pid}"]).stdout)
            .expect("tmux server pid is utf8")
            .trim()
            .parse::<u32>()
            .expect("tmux server pid is numeric");
    let pane = |pane_id: &str| Pane {
        server_pid,
        pane_id: pane_id.to_owned(),
        session_anchor: format!("session-{pane_id}"),
        thread_anchor: format!("thread-{pane_id}"),
    };
    let sender_pane_id = String::from_utf8(sender_pane.stdout).unwrap();
    let receiver_pane_id = String::from_utf8(receiver_pane.stdout).unwrap();
    let sender = pane(sender_pane_id.trim());
    let receiver = pane(receiver_pane_id.trim());

    fixture.start_daemon();
    fixture.registered_context_with_originator(&sender, true, "Codex future host");
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
        context_snapshot(&unknown_host_registration)["identity"]["transport"]["kind"],
        "tmux"
    );
    let receiver_context = fixture.registered_context(&receiver, true);
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
    assert_eq!(sent["durable"], true);
    assert_eq!(sent["notification"], "tmux-input-submitted");

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

    let queued = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &receiver_id,
            "--subject",
            "queued recv e2e",
            "--delivery",
            "queued",
            "consume queued body",
        ],
        Some(&sender),
    );
    assert_eq!(queued["durable"], true);
    assert_eq!(queued["notification"], "queued");
    assert_eq!(queued["consumed"], false);
    let queued_id = queued["msg_id"].as_str().unwrap();
    let pending = fixture.run_ok(&["msg", queued_id], Some(&sender));
    assert_eq!(pending["state"], "pending");
    assert_eq!(pending["wake_attempts"], 0);
    assert_eq!(pending["consumed_by_recv"], false);
    let recv = fixture.run_ok(
        &["recv", "--timeout", "0", "--receive-id", "queued-recv-1"],
        Some(&receiver),
    );
    assert_eq!(recv["messages"][0]["id"], queued_id);
    let consumed = fixture.run_ok(&["msg", queued_id], Some(&sender));
    assert_eq!(consumed["state"], "read");
    assert_eq!(consumed["consumed_by_recv"], true);

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

    // An explicit owner re-subscription after unsubscribing the system default
    // must still satisfy the context lease receipt after identity loss.
    let notify_status = fixture.run_ok(&["notify", "status"], Some(&sender));
    let default_subscription_id = notify_status["subscriptions"]
        .as_array()
        .expect("notify status lists subscriptions")
        .iter()
        .find(|subscription| {
            subscription["event"] == "direct-message"
                && subscription["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("sub-default-direct-message-"))
        })
        .and_then(|subscription| subscription["id"].as_str())
        .expect("registered sender has the default direct-message lease")
        .to_owned();
    fixture.run_ok(
        &["notify", "unsubscribe", &default_subscription_id],
        Some(&sender),
    );
    let resubscribed = fixture.run_ok(
        &[
            "notify",
            "subscribe",
            "--event",
            "direct-message",
            "--ttl-seconds",
            "600",
        ],
        Some(&sender),
    );
    let resubscribed_id = resubscribed["subscription"]["id"]
        .as_str()
        .expect("explicit subscription has an id")
        .to_owned();
    assert_ne!(resubscribed_id, default_subscription_id);

    // Daemon restart and lost local receipt: the same actual tmux anchor must
    // return the same worker and generation without a fresh identity.
    let sender_identity = sender_context["identity"].clone();
    let sender_generation = sender_context["binding"]["endpoint_generation"].clone();
    fixture.stop_daemon();
    std::fs::remove_dir_all(fixture.host_state.join("identities")).unwrap();
    fixture.start_daemon();
    let restart_envelope = fixture.run_ok(&["context"], Some(&sender));
    assert_eq!(
        restart_envelope["result"]["outcome"], "completed",
        "{restart_envelope}"
    );
    let restart_context = context_snapshot(&restart_envelope);
    let peer_envelope = fixture.run_ok(&["context"], Some(&receiver));
    assert_eq!(
        peer_envelope["result"]["outcome"], "completed",
        "{peer_envelope}"
    );
    assert_eq!(
        context_snapshot(&peer_envelope)["identity"]["worker_id"],
        receiver_id
    );
    assert_eq!(
        context_snapshot(&peer_envelope)["identity"]["role"],
        receiver_context["identity"]["role"]
    );
    assert_ne!(
        context_snapshot(&peer_envelope)["identity"]["role"],
        "master"
    );
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
        "a direct-message lease must be armed after restart: {restart_context}"
    );
    let restart_subscriptions = fixture.run_ok(&["notify", "status"], Some(&sender));
    let restart_subscriptions = restart_subscriptions["subscriptions"]
        .as_array()
        .expect("notify status lists subscriptions after restart");
    assert!(
        restart_subscriptions
            .iter()
            .any(|subscription| subscription["id"] == resubscribed_id
                && subscription["status"] == "armed"),
        "the explicit direct-message lease must survive restart: {restart_subscriptions:?}"
    );
    assert!(
        restart_subscriptions.iter().any(|subscription| {
            subscription["id"] == default_subscription_id
                && subscription["status"] == "cancelled"
                && subscription["status_reason"] == "explicit-unsubscribe"
        }),
        "the explicit unsubscribe must remain authoritative: {restart_subscriptions:?}"
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
        daemon: None,
        owned_sessions: vec!["collab-pane-recovery".into()],
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
    fixture.start_daemon();
    // The daemon owns the worker id; the fixture must not assume a naming
    // scheme. Read the concrete worker from the registration receipt.
    fixture.registered_context(&old, true);
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
    let before = fixture.run_context(&["context"], Some(&old));
    let grant_before = fixture.run_ok(&["master", "status"], Some(&old));
    let durable_master_fields = |status: &Value| {
        let master = &status["master"];
        json!({
            "worker_id": master["worker_id"],
            "grant_id": master["grant_id"],
            "grant_generation": master["grant_generation"],
            "binding_id": master["binding_id"],
            "endpoint_generation": master["endpoint_generation"],
            "assigned_by": master["assigned_by"],
            "assigned_ms": master["assigned_ms"],
            "boundary": master["boundary"],
            "approval": master["approval"],
            "master_scope": master["scope"],
            "scope": status["scope"],
            "recorded_unusable": status["recorded_unusable"],
        })
    };
    fixture.restart_daemon();
    let restarted = fixture.run_ok(&["context"], Some(&old));
    assert_eq!(restarted["result"]["outcome"], "completed", "{restarted}");
    assert_eq!(context_snapshot(&restarted)["identity"], before["identity"]);
    assert_eq!(context_snapshot(&restarted)["binding"], before["binding"]);
    let grant_restarted = fixture.run_ok(&["master", "status"], Some(&old));
    assert_eq!(
        durable_master_fields(&grant_restarted),
        durable_master_fields(&grant_before),
        "daemon restart must preserve master grant identity"
    );
    // MasterStatus derives this flag from the current holder's identity
    // presence. It does not infer busy/idle or require a transport probe.
    assert_eq!(
        grant_restarted["master"]["endpoint_live"], true,
        "the restored current holder owns its persisted pane binding: {grant_restarted}"
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
        session_anchor: old.session_anchor.clone(),
        thread_anchor: "thread-after-restart".into(),
        ..old
    };
    let envelope = fixture.run_ok(&["context"], Some(&new));
    assert_eq!(envelope["result"]["outcome"], "completed", "{envelope}");
    let recovered = context_snapshot(&envelope);
    assert_eq!(recovered["identity"]["worker_id"], worker_id);
    assert_eq!(recovered["identity"]["role"], "master");
    assert_eq!(recovered["registered"], true);
    assert_eq!(
        recovered["binding"]["binding_id"],
        before["binding"]["binding_id"]
    );
    assert_eq!(
        recovered["binding"]["runtime_id"],
        before["binding"]["runtime_id"]
    );
    assert!(
        recovered["binding"]["endpoint_generation"]
            .as_u64()
            .unwrap()
            > before["binding"]["endpoint_generation"].as_u64().unwrap(),
        "changed current native facts must refresh the delivery endpoint"
    );
    let receipt: Value = serde_json::from_slice(
        &std::fs::read(
            fixture
                .host_state
                .join("identities")
                .join(&worker_id)
                .join("identity.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        receipt["runtime"]["endpoint_generation"],
        recovered["binding"]["endpoint_generation"]
    );
    let route_output = fixture
        .configured_command(
            &["route", "resolve", "--pane-id", &new.pane_id],
            Some(&new),
            "Codex TUI",
        )
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_THREAD")
        .output()
        .unwrap();
    assert!(
        route_output.status.success(),
        "{}",
        String::from_utf8_lossy(&route_output.stderr)
    );
    let route: Value = serde_json::from_slice(&route_output.stdout).unwrap();
    assert_eq!(
        route["endpoint_generation"],
        recovered["binding"]["endpoint_generation"]
    );
    let grant_after = fixture.run_ok(&["master", "status"], Some(&new));
    for field in [
        "worker_id",
        "grant_id",
        "grant_generation",
        "assigned_by",
        "assigned_ms",
        "boundary",
        "approval",
        "scope",
    ] {
        assert_eq!(
            grant_after["master"][field], grant_before["master"][field],
            "master grant {field} must survive recovery"
        );
    }
    assert_eq!(
        grant_after["master"]["endpoint_generation"],
        recovered["binding"]["endpoint_generation"]
    );
    assert_eq!(
        grant_after["master"]["binding_id"],
        recovered["binding"]["binding_id"]
    );
    assert_eq!(grant_after["scope"], grant_before["scope"]);
    assert_eq!(
        grant_after["recorded_unusable"],
        grant_before["recorded_unusable"]
    );
    let again = fixture.run_context(&["context"], Some(&new));
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
    assert!(
        received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == message_id),
        "the receive must contain the sent message"
    );
    let status = fixture.run_ok(&["msg", message_id], Some(&new));
    assert_eq!(status["consumed_by_recv"], true);
}

/// The implicit bootstrap path — `collab context` run from a pane with no
/// `--worker` and no `COLLAB_WORKER` — resolves its durable identity from the
/// pane anchor. A stale local token must synchronize with its unique durable
/// owner in the same invocation, without an approval terminal.
#[test]
fn implicit_context_repairs_a_rejected_same_pane_credential() {
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
        daemon: None,
        owned_sessions: vec!["collab-implicit-context".into()],
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
    fixture.start_daemon();
    // The daemon owns the worker id; read the concrete value from the init
    // receipt instead of re-deriving the old naming scheme.
    fixture.registered_context(&pane, true);
    let init = fixture.run_ok(&["init"], Some(&pane));
    let worker_id = init["worker_id"]
        .as_str()
        .expect("init receipt names the daemon-owned worker")
        .to_owned();

    // Durable identity exists and is registered; now make its token unusable.
    // The same pane restores the daemon's authoritative credential automatically.
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
    let output = fixture
        .configured_command(&["context"], Some(&pane), "Codex TUI")
        .env_remove("COLLAB_WORKER")
        .output()
        .expect("run implicit collab context");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let public_error = format!("{stdout}{stderr}");
    assert!(
        output.status.success(),
        "same-pane credential repair must complete: stdout={stdout} stderr={stderr}"
    );
    let repaired: Value = serde_json::from_slice(&output.stdout).expect("typed context JSON");
    assert_eq!(repaired["result"]["outcome"], "completed", "{repaired}");
    assert_eq!(
        context_snapshot(&repaired)["identity"]["worker_id"],
        worker_id,
        "repair preserves the principal: {repaired}"
    );
    assert!(
        !public_error.contains("not-the-recorded-token"),
        "the rejected credential must not leak: {public_error}"
    );
    let after: Value = serde_json::from_slice(&std::fs::read(&identity_path).unwrap()).unwrap();
    assert_ne!(
        after["token"], identity["token"],
        "the daemon must replace the rejected credential"
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
        daemon: None,
        owned_sessions: vec!["collab-context-binding".into()],
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
    fixture.start_daemon();
    pane_only_context(&fixture, &pane, &["context", "--op", "binding-register"]);
    let registered = pane_only_context(&fixture, &pane, &["context"]);
    let worker_id = registered["identity"]["worker_id"]
        .as_str()
        .expect("daemon receipt names the registered worker")
        .to_owned();

    let context = pane_only_context(&fixture, &pane, &["context"]);
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
    assert_eq!(binding["native_thread_id"], Value::Null);
    assert_eq!(context["identity"]["transport"]["thread_id"], Value::Null);
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
    let again = pane_only_context(&fixture, &pane, &["context"]);
    assert_eq!(again["binding"], *binding);
    let route_output = fixture
        .configured_command(
            &["route", "resolve", "--pane-id", &pane.pane_id],
            Some(&pane),
            "Codex TUI",
        )
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_THREAD")
        .output()
        .unwrap();
    assert!(
        route_output.status.success(),
        "{}",
        String::from_utf8_lossy(&route_output.stderr)
    );
    let route: Value = serde_json::from_slice(&route_output.stdout).unwrap();
    assert_eq!(route["binding_id"], binding["binding_id"]);
    assert_eq!(route["session_id"], pane.session_anchor);
    assert_eq!(route["native_thread_id"], Value::Null);
    let constrained = fixture.command(
        &["route", "resolve", "--pane-id", &pane.pane_id],
        Some(&pane),
    );
    assert!(
        constrained.status.success(),
        "native environment facts must not veto the current project/session route: {}",
        String::from_utf8_lossy(&constrained.stderr)
    );
    let constrained: Value = serde_json::from_slice(&constrained.stdout).unwrap();
    assert_eq!(constrained["binding_id"], binding["binding_id"]);
    fixture.restart_daemon();
    let restarted = pane_only_context(&fixture, &pane, &["context"]);
    assert_eq!(restarted["identity"]["worker_id"], worker_id);
    assert_eq!(restarted["binding"], *binding);
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
        daemon: None,
        owned_sessions: Vec::new(),
    };
    fixture.start_daemon();

    // A caller with no anchor at all: no tmux pane, no session/thread, and an
    // unsupported originator so no namespace is observed either. The daemon
    // must name every absent factual field and must not guess a worker.
    let no_anchor = fixture.command_with_originator(&["context"], None, "Codex future host");
    assert!(
        !no_anchor.status.success(),
        "missing facts are a typed incomplete terminal: stdout={} stderr={}",
        String::from_utf8_lossy(&no_anchor.stdout),
        String::from_utf8_lossy(&no_anchor.stderr)
    );
    let no_anchor_result: Value = serde_json::from_slice(&no_anchor.stdout).unwrap();
    assert_eq!(no_anchor_result["ok"], false);
    assert_eq!(no_anchor_result["result"]["outcome"], "missing_facts");
    let update = &no_anchor_result["result"]["requires"];
    assert_eq!(update["kind"], "identity_facts");
    assert_eq!(
        update["fields"],
        json!(["session_id", "thread_id", "endpoint", "namespace"]),
        "no-anchor context returns every exact missing native fact: {no_anchor_result}"
    );
    assert!(
        update["worker_id"].is_null(),
        "missing-facts terminal must not guess a worker"
    );
    assert_eq!(
        update["repair_invocation"],
        "collab context --provide '<JSON containing required_fields>'"
    );

    // A partial anchor: the runtime is a recognized TUI, so namespace is
    // observed from the originator. The daemon must not re-request observed
    // facts; it asks only for the session, thread and endpoint it still lacks.
    let partial = fixture.command(&["context"], None);
    assert!(
        !partial.status.success(),
        "partial native facts are a typed incomplete terminal: stdout={} stderr={}",
        String::from_utf8_lossy(&partial.stdout),
        String::from_utf8_lossy(&partial.stderr)
    );
    let partial_result: Value = serde_json::from_slice(&partial.stdout).unwrap();
    assert_eq!(partial_result["result"]["outcome"], "missing_facts");
    assert_eq!(
        partial_result["result"]["requires"]["fields"],
        json!(["session_id", "thread_id", "endpoint"]),
        "observed namespace must not be re-requested: {partial_result}"
    );
    assert!(
        partial_result["result"]["requires"]["worker_id"].is_null(),
        "partial terminal must not guess a worker: {partial_result}"
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
