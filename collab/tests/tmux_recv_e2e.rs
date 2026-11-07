include!("support/tmux_cli_fixture.rs");

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

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
    let snapshot = context_snapshot(&value);
    let worker = snapshot["identity"]["worker_id"].as_str().unwrap();
    let app = snapshot["binding"]["app_scope_id"].as_str().unwrap();
    let mapped = fixture.current_mapping_for(&fixture.root, app, worker);
    for field in [
        "agent_id",
        "runtime_id",
        "binding_id",
        "session_id",
        "native_thread_id",
    ] {
        assert_eq!(
            mapped["binding"][field], snapshot["binding"][field],
            "{field}"
        );
    }
    let credential: Value = serde_json::from_slice(
        &std::fs::read(
            fixture
                .host_state
                .join("identities")
                .join(worker)
                .join("identity.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(credential["worker_id"], worker);
    assert_eq!(
        credential["runtime"]["binding_id"],
        mapped["binding"]["binding_id"]
    );
    let state = fixture.current_state_for(&fixture.root, app);
    let lease =
        &state["notification_subscriptions"][format!("sub-default-direct-message-{worker}")];
    assert_eq!(lease["worker_id"], worker);
    assert_eq!(lease["method"], "tmux");
    assert_eq!(lease["status"], "armed");
    let target = mapped["transport"]["thread_id"]
        .as_str()
        .unwrap_or_else(|| {
            mapped["transport"]["tmux_endpoint"]["pane_id"]
                .as_str()
                .unwrap()
        });
    assert_eq!(lease["target"], target);
    snapshot.clone()
}

fn pane_shows_message(visible: &str, message_id: &str) -> bool {
    // tmux wraps long notifications at the pane width, which can split the
    // message id across rows. Compare with whitespace removed so wrapping
    // cannot hide an id that the pane actually rendered.
    let compact: String = visible.chars().filter(|ch| !ch.is_whitespace()).collect();
    compact.contains(message_id)
}

fn wait_for_pane_message(fixture: &Fixture, pane: &Pane, message_id: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let captured = tmux(
            &fixture.tmux_socket,
            &["capture-pane", "-p", "-t", &pane.pane_id, "-S", "-4096"],
        );
        let visible = String::from_utf8_lossy(&captured.stdout);
        if pane_shows_message(&visible, message_id) {
            return;
        }
        if Instant::now() >= deadline {
            // One final capture avoids failing on the last poll boundary: the
            // assertion can otherwise race with the adapter's own publish step.
            let final_captured = tmux(
                &fixture.tmux_socket,
                &["capture-pane", "-p", "-t", &pane.pane_id, "-S", "-4096"],
            );
            let final_visible = String::from_utf8_lossy(&final_captured.stdout);
            if pane_shows_message(&final_visible, message_id) {
                return;
            }
            panic!(
                "message {message_id} was not observed in owned pane {}: {final_visible}",
                pane.pane_id
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
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
    let receiver_name = receiver_context["name"]
        .as_str()
        .expect("context names the receiver's stable occupancy")
        .to_owned();
    // The eventual recipient owns and ends this finite schedule, so the test
    // proves that ending an owner's finite schedule preserves that owner's own
    // daemon-owned direct-message reception. The trigger is close enough that
    // the fixture actually crosses it before asserting termination.
    let receiver_goal_id = "tmux-e2e-finite-goal";
    let finite_trigger_ms = now_epoch_ms() + 5_000;
    let armed_finite = fixture.run_ok(
        &[
            "goal",
            "reconcile",
            "--goal-id",
            receiver_goal_id,
            "--owner",
            &receiver_name,
            "--desired",
            "active",
            "--at-ms",
            &finite_trigger_ms.to_string(),
            "--ttl-seconds",
            "300",
            "--reason",
            "owned finite schedule",
        ],
        Some(&receiver),
    );
    let finite_subscription_id = armed_finite["subscription"]["id"]
        .as_str()
        .expect("an active finite goal has a subscription id")
        .to_owned();
    assert_eq!(armed_finite["status"], "armed");
    let ended_finite = fixture.run_ok(
        &[
            "goal",
            "reconcile",
            "--goal-id",
            receiver_goal_id,
            "--owner",
            &receiver_name,
            "--desired",
            "ended",
            "--reason",
            "owned finite schedule ended",
        ],
        Some(&receiver),
    );
    assert_eq!(ended_finite["status"], "ended");
    assert!(
        ended_finite["retired_subscription_ids"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id == &finite_subscription_id)),
        "the ended receipt must retire the owned occurrence: {ended_finite}"
    );
    let ended_status = fixture.run_ok(&["notify", "status"], Some(&receiver));
    let ended_occurrence = ended_status["subscriptions"]
        .as_array()
        .expect("notify status lists subscriptions")
        .iter()
        .find(|subscription| subscription["id"] == finite_subscription_id)
        .expect("the retired occurrence stays observable");
    assert_eq!(ended_occurrence["status"], "cancelled");
    assert_eq!(
        ended_occurrence["event"], "deadline",
        "the ended schedule must not be a direct-message lease"
    );
    // Cross the bounded trigger window so the assertion cannot pass merely
    // because the run finished before the trigger time.
    let crossed_by = Instant::now() + Duration::from_secs(15);
    while now_epoch_ms() <= finite_trigger_ms + 1_500 {
        assert!(
            Instant::now() < crossed_by,
            "fixture did not cross the bounded goal trigger window"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let state = fixture.current_state_for(&fixture.root, "appserver-cli");
    assert!(!state["wake_bindings"]
        .as_object()
        .unwrap()
        .values()
        .any(|subscription| subscription == &finite_subscription_id));
    let subject = format!("deadline:goal:{receiver_goal_id}");
    assert!(!state["msgs"]
        .as_object()
        .unwrap()
        .values()
        .any(|message| message["subject"] == subject));
    let native = tmux(
        &fixture.tmux_socket,
        &["capture-pane", "-p", "-t", &receiver.pane_id, "-S", "-4096"],
    );
    let native: String = String::from_utf8_lossy(&native.stdout)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(
        !native.contains(&subject),
        "ended goal cannot reach the native pane"
    );

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

    // The default lease is daemon-owned. It satisfies the context receipt
    // without any agent-controlled unsubscribe.
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
    let default_subscription = notify_status["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|subscription| subscription["id"] == default_subscription_id)
        .expect("default direct-message lease")
        .clone();
    assert_eq!(default_subscription["status"], "armed");

    // Daemon restart and lost local receipt: the same actual tmux anchor must
    // return the same worker and current binding without a fresh identity.
    let sender_identity = sender_context["identity"].clone();
    let sender_binding = sender_context["binding"].clone();
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
    assert_eq!(restart_context["binding"], sender_binding);
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
            .any(|subscription| subscription["id"] == default_subscription_id
                && subscription["status"] == "armed"),
        "the daemon-owned default lease must stay armed after restart: {restart_subscriptions:?}"
    );
    // Mailbox and tasks are daemon state, not receipt state: they survive the
    // restart together with the recovered identity. Read the preserved mailbox
    // message here, before the post-goal direct message is consumed, so the
    // two acceptance paths cannot consume each other's durable records.
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

    // Direct-message reception stays daemon-owned after the finite schedule
    // ended and after the same anchor recovered its identity.
    let after_goal = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &receiver_id,
            "--subject",
            "after finite schedule",
            "owned pane body",
        ],
        Some(&sender),
    );
    let after_goal_id = after_goal["msg_id"]
        .as_str()
        .or_else(|| after_goal["message_id"].as_str())
        .expect("a public sendmessage has a durable message id")
        .to_owned();
    assert_eq!(after_goal["notification"], "tmux-input-submitted");
    wait_for_pane_message(&fixture, &receiver, &after_goal_id);
    let after_goal_recv = fixture.run_ok(
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "after-finite-goal-recv",
        ],
        Some(&receiver),
    );
    assert!(
        after_goal_recv["messages"]
            .as_array()
            .expect("recv returns messages")
            .iter()
            .any(|message| message["id"] == after_goal_id),
        "the same message id must be receivable after the finite goal ended: {after_goal_recv}"
    );
    assert_eq!(
        fixture.run_ok(&["msg", &after_goal_id], Some(&sender))["consumed_by_recv"],
        true
    );
    let after_goal_status = fixture.run_ok(&["notify", "status"], Some(&receiver));
    assert!(
        after_goal_status["subscriptions"]
            .as_array()
            .expect("notify status lists subscriptions")
            .iter()
            .find(|subscription| subscription["id"] == finite_subscription_id)
            .is_some_and(|subscription| subscription["status"] == "cancelled"),
        "the ended finite schedule must stay terminal: {after_goal_status}"
    );
    assert!(
        after_goal_status["subscriptions"]
            .as_array()
            .expect("notify status lists subscriptions")
            .iter()
            .find(|subscription| subscription["id"] == finite_subscription_id)
            .is_some_and(|subscription| subscription["worker_id"] == receiver_id),
        "the finite schedule that ended must belong to the receiving owner: {after_goal_status}"
    );
    assert!(
        after_goal_status["subscriptions"]
            .as_array()
            .expect("notify status lists subscriptions")
            .iter()
            .any(|subscription| subscription["event"] == "direct-message"
                && subscription["status"] == "armed"),
        "daemon-owned direct-message reception must remain armed: {after_goal_status}"
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
    assert_eq!(recovered["binding"]["session_id"], new.session_anchor);
    assert_eq!(recovered["binding"]["native_thread_id"], new.thread_anchor);
    let mapped = fixture.current_mapping_for(&fixture.root, "appserver-cli", &worker_id);
    for field in [
        "agent_id",
        "runtime_id",
        "binding_id",
        "session_id",
        "native_thread_id",
    ] {
        assert_eq!(
            mapped["binding"][field], recovered["binding"][field],
            "{field}"
        );
    }
    assert_eq!(mapped["transport"]["tmux_endpoint"]["pane_id"], new.pane_id);
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
    for field in [
        "agent_id",
        "runtime_id",
        "binding_id",
        "session_id",
        "native_thread_id",
    ] {
        assert_eq!(
            receipt["runtime"][field], recovered["binding"][field],
            "{field}"
        );
    }
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
    for field in [
        "agent_id",
        "app_scope_id",
        "project_scope",
        "binding_id",
        "session_id",
        "native_thread_id",
    ] {
        assert_eq!(route[field], recovered["binding"][field], "{field}");
    }
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
    assert_eq!(again["binding"], recovered["binding"]);

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

/// A later agent session in the same live pane and the same exact invocation
/// cwd inherits the pane's current name. The saved transport's old endpoint is
/// not the caller pane; only this invocation's daemon ancestry observation may
/// seed the name anchor.
#[test]
fn same_live_caller_pane_and_exact_cwd_inherits_name_across_session() {
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
        owned_sessions: vec!["collab-pane-inherit".into()],
    };
    tmux(
        &tmux_socket,
        &[
            "new-session",
            "-d",
            "-s",
            "collab-pane-inherit",
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
        session_anchor: "session-before".into(),
        thread_anchor: "thread-before".into(),
    };
    fixture.start_daemon();
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
    assert_eq!(before["name"], "Jenny", "{before}");

    // Force recovery from the archived pane credential so the name anchor is
    // proven from this invocation's live caller pane, not a local session hit.
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

    // A different agent session runs in the same live pane and same cwd. The
    // session anchor is not the identity anchor; the exact pane plus cwd is.
    let new = Pane {
        session_anchor: "session-after".into(),
        thread_anchor: "thread-after".into(),
        ..old
    };
    let envelope = fixture.run_ok(&["context"], Some(&new));
    assert_eq!(envelope["result"]["outcome"], "completed", "{envelope}");
    let recovered = context_snapshot(&envelope);
    assert_eq!(
        recovered["name"], "Jenny",
        "the same live pane plus exact cwd must inherit the name across a session change: {recovered}"
    );
}

/// The saved pane and its durable binding still carry the old name, but this
/// invocation runs in a different live pane with the same exact invocation
/// cwd. Equality between the saved transport and the binding proves only saved
/// state consistency, never the current caller pane; recovering an old
/// identity must not seed the name anchor from the saved endpoint. The caller
/// takes a fresh ordinary name instead of the saved pane name.
#[test]
fn different_live_pane_with_same_cwd_does_not_inherit_saved_pane_name() {
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
        owned_sessions: vec![
            "collab-pane-anchor-saved".into(),
            "collab-pane-anchor-live".into(),
        ],
    };
    tmux(
        &tmux_socket,
        &[
            "new-session",
            "-d",
            "-s",
            "collab-pane-anchor-saved",
            "sleep 600",
        ],
    );
    let live_pane_id = String::from_utf8(
        tmux(
            &tmux_socket,
            &[
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "-s",
                "collab-pane-anchor-live",
                "sleep 600",
            ],
        )
        .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    let server_pid =
        String::from_utf8(tmux(&tmux_socket, &["display-message", "-p", "#{pid}"]).stdout)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
    let saved_pane_id = String::from_utf8(
        tmux(
            &tmux_socket,
            &[
                "display-message",
                "-p",
                "-t",
                "collab-pane-anchor-saved",
                "#{pane_id}",
            ],
        )
        .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    assert_ne!(saved_pane_id, live_pane_id);
    let saved = Pane {
        server_pid,
        pane_id: saved_pane_id,
        session_anchor: "session-saved".into(),
        thread_anchor: "thread-saved".into(),
    };
    let live = Pane {
        server_pid,
        pane_id: live_pane_id,
        session_anchor: "session-live".into(),
        thread_anchor: "thread-live".into(),
    };

    fixture.start_daemon();
    fixture.registered_context(&saved, true);
    let init = fixture.run_ok(&["init"], Some(&saved));
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
        Some(&saved),
    );
    let before = fixture.run_context(&["context"], Some(&saved));
    assert_eq!(before["name"], "Jenny", "{before}");

    // Force recovery from the archived pane credential so the saved pane name
    // must be proven from this invocation's live caller pane, not from the
    // saved transport that still equals the durable binding.
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

    // A different live pane in the same tmux server, same exact cwd. The saved
    // transport (the archived pane) is not the current caller pane.
    let envelope = fixture.run_ok(&["context"], Some(&live));
    assert_eq!(envelope["result"]["outcome"], "completed", "{envelope}");
    let recovered = context_snapshot(&envelope);
    assert_ne!(
        recovered["name"], "Jenny",
        "a different live pane must not inherit the saved pane name: {recovered}"
    );
    assert_ne!(
        recovered["identity"]["worker_id"], worker_id,
        "the archived identity must not be recovered from a different live pane: {recovered}"
    );
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
/// back the current map binding after its local registration receipt is lost.
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

#[test]
fn pane_match_tolerates_message_id_wrapped_at_pane_width() {
    let visible = "COLLAB_NOTIFY m179\n2810151585-1 [notification-batch]";
    assert!(pane_shows_message(visible, "m1792810151585-1"));
    assert!(!pane_shows_message(visible, "m1792810151585-2"));
}
