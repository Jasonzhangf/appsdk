use super::*;

#[test]
fn unauthenticated_directory_query_empty_and_free_of_presence_side_effects() {
    let fixture = AppFixture::new();

    // Directory query before any peers are registered
    let output = fixture.command_without_native_facts(&["directory"]);
    assert!(
        output.status.success(),
        "unauthenticated directory query must succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let dir: Value = serde_json::from_slice(&output.stdout).expect("directory emits JSON");
    assert_eq!(dir["count"], 0);
    assert!(
        dir["targets"].as_array().is_some_and(|arr| arr.is_empty()),
        "empty directory must return empty array: {dir}"
    );

    // Journal/routes must not have any registered routes or presence edges
    let routes_path = fixture.host_state.join("routes.jsonl");
    assert!(
        !routes_path.exists(),
        "unauthenticated directory query must not create a host route or presence side-effects"
    );
}

#[test]
fn directory_query_projects_whitelist_without_sensitive_facts_or_presence_records() {
    let fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);

    assert_eq!(alice["name"], "Alice");
    assert_eq!(bob["name"], "Bob");

    let journal_path = fixture.name_journal();
    let events_before: usize = std::fs::read_to_string(&journal_path)
        .unwrap_or_default()
        .lines()
        .count();

    // Query directory from an unauthenticated caller
    let output = fixture.command_without_native_facts(&["directory"]);
    assert!(
        output.status.success(),
        "directory query failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let dir: Value = serde_json::from_slice(&output.stdout).expect("directory emits JSON");
    assert_eq!(dir["count"], 2);
    let arr = dir["targets"]
        .as_array()
        .expect("directory targets is a JSON array");
    assert_eq!(arr.len(), 2, "must contain Alice and Bob: {dir}");

    for entry in arr {
        // Whitelist projected fields
        assert!(entry.get("worker_id").and_then(Value::as_str).is_some());
        let display_name = entry
            .get("display_name")
            .and_then(Value::as_str)
            .expect("must have display_name");
        assert!(display_name == "Alice" || display_name == "Bob");
        assert!(entry.get("session_id").and_then(Value::as_str).is_some());
        assert!(entry.get("transport").and_then(Value::as_object).is_some());
        assert_eq!(entry.get("identity_valid"), Some(&json!(true)));
        assert_eq!(entry.get("endpoint_live"), Some(&Value::Null));

        // Sensitive / internal fields MUST NOT leak
        assert!(entry.get("token").is_none());
        assert!(entry.get("auth_token").is_none());
        assert!(entry.get("endpoint_generation").is_none());
        assert!(entry.get("tmux_endpoint").is_none());
        assert!(entry.get("query_capability").is_none());
        assert!(entry.get("tasks").is_none());
    }

    // Verify no presence or mutation was written to journal by directory query
    let events_after: usize = std::fs::read_to_string(&journal_path)
        .unwrap_or_default()
        .lines()
        .count();
    assert_eq!(
        events_before, events_after,
        "directory query must be strictly read-only and not append any journal records"
    );
}

#[test]
fn anonymous_send_unauthenticated_and_inherited_agent_env() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);

    // 1. Unauthenticated anonymous send (no agent environment)
    let output = fixture.command_without_native_facts(&[
        "sendmessage",
        "--unknown-sender",
        "--to",
        "Bob",
        "--subject",
        "Hello from unauthenticated",
        "Body from unauthenticated sender",
    ]);
    assert!(
        output.status.success(),
        "anonymous send failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let sent: Value = serde_json::from_slice(&output.stdout).expect("send result JSON");
    let msg_id = sent
        .get("msg_id")
        .or_else(|| sent.get("message_id"))
        .and_then(Value::as_str)
        .expect("send result has msg_id");

    // Bob inspects the message
    let msg = fixture.run_public(&["msg", msg_id], THREAD_B);
    assert_eq!(msg["from"], "unknown", "must be fixed to 'unknown'");
    assert_eq!(msg["subject"], "Hello from unauthenticated");
    assert_eq!(msg["body"], "Body from unauthenticated sender");

    // Verify directory does not contain 'unknown' as a peer
    let dir_out = fixture.command_without_native_facts(&["directory"]);
    let dir: Value = serde_json::from_slice(&dir_out.stdout).unwrap();
    let names: Vec<&str> = dir["targets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["display_name"].as_str())
        .collect();
    assert!(
        !names.contains(&"unknown"),
        "unknown sender must not create a peer/worker in directory"
    );

    // 2. Anonymous send inheriting agent environment (running with THREAD_A in env)
    let output_pane = fixture.command_public(
        &[
            "sendmessage",
            "--unknown-sender",
            "--to",
            "Bob",
            "--subject",
            "Hello from Alice's environment",
            "Body ignoring inherited agent env",
        ],
        THREAD_A,
    );
    assert!(
        output_pane.status.success(),
        "anonymous send in pane failed: {}",
        String::from_utf8_lossy(&output_pane.stderr)
    );
    let sent_pane: Value = serde_json::from_slice(&output_pane.stdout).expect("send result JSON");
    let msg_id_pane = sent_pane
        .get("msg_id")
        .or_else(|| sent_pane.get("message_id"))
        .and_then(Value::as_str)
        .expect("send result has msg_id");

    let msg_pane = fixture.run_public(&["msg", msg_id_pane], THREAD_B);
    assert_eq!(
        msg_pane["from"], "unknown",
        "explicit anonymous send must override inherited agent env"
    );

    // 3. Anonymous send rejects reply obligations
    let rejected = fixture.command_without_native_facts(&[
        "sendmessage",
        "--unknown-sender",
        "--to",
        "Bob",
        "--subject",
        "Bad reply request",
        "--reply-required",
        "Obligation not allowed",
        "Body text",
    ]);
    assert!(
        !rejected.status.success(),
        "anonymous send with reply obligation must fail"
    );
    let err_msg = format!(
        "{} {}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(
        err_msg.contains("cannot be used with") || err_msg.contains("reply obligation"),
        "error message must reject reply obligation: {err_msg}"
    );
}

#[test]
fn target_session_constraint_cli_and_delivery_enforcement() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);

    // 1. Target session matches Bob's session (THREAD_B)
    let output_match = fixture.command_without_native_facts(&[
        "sendmessage",
        "--unknown-sender",
        "--to",
        "Bob",
        "--target-session",
        THREAD_B,
        "--subject",
        "Session Match",
        "Delivered to exact session",
    ]);
    assert!(
        output_match.status.success(),
        "matching target session must succeed: {}",
        String::from_utf8_lossy(&output_match.stderr)
    );

    // 2. Target session mismatches Bob's session
    let output_mismatch = fixture.command_without_native_facts(&[
        "sendmessage",
        "--unknown-sender",
        "--to",
        "Bob",
        "--target-session",
        "01a0d699-mismatch-session-000000000000",
        "--subject",
        "Session Mismatch",
        "Must be rejected",
    ]);
    assert!(
        !output_mismatch.status.success(),
        "mismatched target session must fail"
    );
    let err = format!(
        "{} {}",
        String::from_utf8_lossy(&output_mismatch.stdout),
        String::from_utf8_lossy(&output_mismatch.stderr)
    );
    assert!(
        err.contains("TARGET_SESSION_MISMATCH") || err.contains("target session"),
        "error must indicate target session mismatch: {err}"
    );
}

#[test]
fn authenticated_send_records_real_identity_without_regression() {
    let fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);

    let output = fixture.command_public(
        &[
            "sendmessage",
            "--to",
            "Bob",
            "--subject",
            "Authenticated notice",
            "Authenticated body from Alice",
        ],
        THREAD_A,
    );
    assert!(
        output.status.success(),
        "authenticated send failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let sent: Value = serde_json::from_slice(&output.stdout).expect("send result JSON");
    let msg_id = sent
        .get("msg_id")
        .or_else(|| sent.get("message_id"))
        .and_then(Value::as_str)
        .expect("send result has msg_id");

    let msg = fixture.run_public(&["msg", msg_id], THREAD_B);
    assert_ne!(
        msg["from"], "unknown",
        "authenticated send must not have from='unknown'"
    );
    assert_eq!(
        msg["from"], alice["identity"]["worker_id"],
        "authenticated send must record Alice's worker_id"
    );
    assert_eq!(msg["body"], "Authenticated body from Alice");
}

#[test]
fn pinned_session_survives_public_continuation_and_retention_replay_without_cross_session_delivery()
{
    const NEXT_SESSION: &str = "01a0d641-13f1-7b31-8d84-530f0f2d3fdd";
    let mut fixture = AppFixture::new();
    let receiver = fixture.run_public(&["context"], THREAD_B);
    let worker = receiver["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Hold a real public send in the daemon before any native attempt. This is
    // declared configuration, not a synthesized message or route journal.
    let config = fixture.root.join("home/.appsdk/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    fixture.stop_daemon();
    std::fs::write(&config, "[notifications]\nenabled=false\n").unwrap();
    fixture.start_daemon();
    let pinned = fixture.command_without_native_facts(&[
        "sendmessage",
        "--unknown-sender",
        "--to",
        &worker,
        "--target-session",
        THREAD_B,
        "--delivery",
        "queued",
        "--subject",
        "old-session-pinned",
        "private body for the original session",
    ]);
    assert!(
        !pinned.status.success(),
        "disabled native notifications cannot report delivery"
    );
    let stderr = String::from_utf8_lossy(&pinned.stderr);
    let receipt: Value = serde_json::from_str(
        stderr
            .rsplit_once("collab response: ")
            .map(|(_, value)| value.trim())
            .unwrap_or(""),
    )
    .expect("disabled send returns its durable receipt");
    assert_eq!(receipt["durable"], true, "{receipt}");
    assert_eq!(
        receipt["notification_error"], "notifications are disabled",
        "{receipt}"
    );
    let old_id = receipt["msg_id"].as_str().unwrap().to_owned();
    let before = fixture.run_public(&["msg", &old_id], THREAD_B);
    assert_eq!(before["wake_attempts"], 0);
    assert_eq!(before["consumed_by_recv"], false);

    // Supply an unrelated, expired historical message as retention input. The
    // pinned message and its binding above still come from the real CLI. The
    // daemon must perform its own purge/compaction; no snapshot is synthesized.
    fixture.append_name_events(&[json!({"ev":"Sent", "msg":{
        "id":"m1-1", "from":"unknown", "to":worker,
        "type":"notify", "subject":"expired retention sibling", "body":"old history",
        "in_reply_to":null, "created_ms":1, "state":"pending",
        "wake_attempt_count":0, "last_wake_attempt_ms":0, "retry_attempted":false
    }})]);
    let compacted = std::fs::read_to_string(fixture.name_journal()).unwrap();
    assert!(
        compacted
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|event| event["ev"] == "ReducerSnapshot"),
        "the expired sibling must trigger the daemon's real retention compaction"
    );

    // Only the external native provider's facts change. The real context CLI
    // must perform the identity continuation and persist the new binding.
    fixture
        .state
        .lock()
        .unwrap()
        .get_mut(THREAD_B)
        .unwrap()
        .session_id = Some(NEXT_SESSION.into());
    let continued = fixture.command_public_with_session(&["context"], NEXT_SESSION, THREAD_B);
    assert!(
        continued.status.success(),
        "{}{}",
        String::from_utf8_lossy(&continued.stdout),
        String::from_utf8_lossy(&continued.stderr)
    );
    let continued: Value = serde_json::from_slice(&continued.stdout).unwrap();
    let snapshot = &continued["result"]["snapshot"];
    assert_eq!(snapshot["identity"]["worker_id"], worker);
    assert_eq!(snapshot["binding"]["session_id"], NEXT_SESSION);

    // Replay the actual committed send and continuation, then enable delivery.
    fixture.stop_daemon();
    std::fs::write(&config, "[notifications]\nenabled=true\n").unwrap();
    fixture.start_daemon();
    let replayed = fixture.command_public_with_session(&["msg", &old_id], NEXT_SESSION, THREAD_B);
    assert!(
        replayed.status.success(),
        "{}",
        String::from_utf8_lossy(&replayed.stderr)
    );
    let replayed: Value = serde_json::from_slice(&replayed.stdout).unwrap();
    assert_eq!(
        replayed["target_session"], THREAD_B,
        "retention snapshot replay must retain the original fixed session: {replayed}"
    );
    let fresh = fixture.command_without_native_facts(&[
        "sendmessage",
        "--unknown-sender",
        "--to",
        &worker,
        "--subject",
        "current-session-message",
        "complete body for the current session",
    ]);
    assert!(
        fresh.status.success(),
        "{}{}",
        String::from_utf8_lossy(&fresh.stdout),
        String::from_utf8_lossy(&fresh.stderr)
    );
    let fresh: Value = serde_json::from_slice(&fresh.stdout).unwrap();
    let fresh_id = fresh["msg_id"].as_str().unwrap();

    let deadline = Instant::now() + Duration::from_secs(8);
    let old_status = loop {
        let output = fixture.command_public_with_session(&["msg", &old_id], NEXT_SESSION, THREAD_B);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let status: Value = serde_json::from_slice(&output.stdout).unwrap();
        if status["notification_delivery_failure"]["operation"]
            == "notification.target_session_mismatch"
        {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "pinned failure did not become observable: {status}"
        );
        thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(old_status["target_session"], THREAD_B);
    assert_eq!(
        old_status["notification_delivery_failure"]["retryable"],
        false
    );
    assert_eq!(old_status["consumed_by_recv"], false);
    let native = fixture.state.lock().unwrap();
    let inputs = &native.get(THREAD_B).unwrap().inputs;
    assert!(
        inputs
            .iter()
            .any(|input| native_input_has_message(input, fresh_id)),
        "the current session's native consumer did not receive its wake: {inputs:?}"
    );
    assert!(
        !inputs
            .iter()
            .any(|input| native_input_has_message(input, &old_id)),
        "a replayed pinned message leaked into native input for the new session: {inputs:?}"
    );
    drop(native);

    let received = fixture.command_public_with_session(
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            "session-continuation-consume",
        ],
        NEXT_SESSION,
        THREAD_B,
    );
    assert!(
        received.status.success(),
        "{}{}",
        String::from_utf8_lossy(&received.stdout),
        String::from_utf8_lossy(&received.stderr)
    );
    let received: Value = serde_json::from_slice(&received.stdout).unwrap();
    let messages = received["messages"].as_array().unwrap();
    assert!(
        !messages.iter().any(|message| message["id"] == old_id),
        "{received}"
    );
    assert!(
        !messages
            .iter()
            .any(|message| message["body"] == "private body for the original session"),
        "{received}"
    );
    let current = messages
        .iter()
        .find(|message| message["id"] == fresh_id)
        .expect("the current session consumes its ordinary message through recv");
    assert_eq!(current["from"], "unknown");
    assert_eq!(current["body"], "complete body for the current session");
}
