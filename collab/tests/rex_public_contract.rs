include!("support/rex_public_fixture.rs");

fn snapshot(value: &Value) -> &Value {
    value
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .unwrap_or(value)
}

fn worker_id(snapshot: &Value) -> String {
    snapshot["identity"]["worker_id"]
        .as_str()
        .unwrap_or_else(|| panic!("context snapshot has no worker id: {snapshot}"))
        .to_owned()
}

fn selected_transport(snapshot: &Value) -> Value {
    snapshot["identity"]["transport"].clone()
}

fn direct_subscription(status: &Value, worker_id: &str) -> Value {
    status["subscriptions"]
        .as_array()
        .unwrap_or_else(|| panic!("notify status lists subscriptions: {status}"))
        .iter()
        .find(|subscription| {
            subscription["worker_id"] == worker_id && subscription["event"] == "direct-message"
        })
        .cloned()
        .unwrap_or_else(|| panic!("direct-message subscription not found: {status}"))
}

#[test]
#[ignore = "requires an explicitly selected Rex server"]
fn rex_public_context_automatically_discovers_current_block() {
    let mut fixture = RexLiveFixture::new("auto-context");
    let agent = fixture.create_receiver_session("auto-context");
    fixture.wait_receiver_ready(&agent, Duration::from_secs(20));
    fixture.start_daemon();

    let operation = format!("ctx-rex-auto-{}", fixture.unique_token());
    let output = fixture
        .command(
            &["context", "--op", operation.as_str()],
            Some(&agent.agent_session_id),
        )
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
        .env("REX_SERVER", &fixture.rex_server)
        .env("REX_SESSION", &agent.session_id)
        .env("REX_BLOCK", &agent.block_id)
        .output()
        .expect("run ordinary public Context in the test-owned Rex block");
    assert!(
        output.status.success(),
        "ordinary Rex context failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("ordinary Rex context returns JSON");
    assert_eq!(response["result"]["outcome"], "completed", "{response}");
    let context = &response["result"]["snapshot"];
    assert_eq!(
        context["identity"]["transport"]["kind"], "rex",
        "{response}"
    );
    assert_eq!(
        context["identity"]["transport"]["session_id"], agent.agent_session_id,
        "the agent session remains distinct from its Rex address"
    );
    assert_eq!(
        context["identity"]["transport"]["rex_endpoint"]["server"],
        fixture.rex_server
    );
    assert_eq!(
        context["identity"]["transport"]["rex_endpoint"]["session_id"],
        agent.session_id
    );
    assert_eq!(
        context["identity"]["transport"]["rex_endpoint"]["block_id"],
        agent.block_id
    );
    fixture.evidence(
        "ordinary_context_auto_registered",
        json!({
            "operation": operation,
            "agent_session_id": agent.agent_session_id,
            "rex_session_id": agent.session_id,
            "rex_block_id": agent.block_id,
            "snapshot": context,
        }),
    );

    let errors = fixture.finish();
    assert!(
        errors.is_empty(),
        "owned Rex session cleanup failed: {errors:?}"
    );
    eprintln!("Rex live evidence: {}", fixture.evidence_path.display());
}

#[test]
#[ignore = "requires an explicitly selected Rex server"]
fn rex_public_block_binding_replaces_current_owner_and_keeps_blocks_independent() {
    let mut fixture = RexLiveFixture::new("block-binding");
    let alice = fixture.create_receiver_session("binding-alice");
    let bob = fixture.create_receiver_session("binding-bob");
    fixture.wait_receiver_ready(&alice, Duration::from_secs(20));
    fixture.wait_receiver_ready(&bob, Duration::from_secs(20));
    fixture.start_daemon();

    let alice_context = fixture.context_provide(&alice);
    assert_eq!(
        alice_context["result"]["outcome"], "completed",
        "{alice_context}"
    );
    let bob_context = fixture.context_provide(&bob);
    assert_eq!(
        bob_context["result"]["outcome"], "completed",
        "{bob_context}"
    );
    let alice_snapshot = snapshot(&alice_context);
    let bob_snapshot = snapshot(&bob_context);
    let alice_name = alice_snapshot["name"]
        .as_str()
        .unwrap_or_else(|| panic!("Alice snapshot has no name: {alice_snapshot}"))
        .to_owned();
    let bob_name = bob_snapshot["name"]
        .as_str()
        .unwrap_or_else(|| panic!("Bob snapshot has no name: {bob_snapshot}"))
        .to_owned();
    assert_ne!(alice_name, bob_name);
    assert_ne!(alice.block_id, bob.block_id);

    let initial_owners = fixture.rex_block_current_owners();
    assert_eq!(
        initial_owners.get(&alice.block_id),
        Some(&vec![alice_name.clone()]),
        "Alice must initially own her distinct block alone: {initial_owners:?}"
    );
    assert_eq!(
        initial_owners.get(&bob.block_id),
        Some(&vec![bob_name.clone()]),
        "Bob must initially own his distinct block alone: {initial_owners:?}"
    );

    let replacement = fixture.context_provide_on_block(&alice, &bob);
    assert_eq!(
        replacement["result"]["outcome"],
        "completed",
        "the known agent session must replace the occupied block owner without a manual takeover: {replacement}"
    );
    let replacement_snapshot = snapshot(&replacement);
    assert_eq!(
        selected_transport(replacement_snapshot)["rex_endpoint"]["block_id"],
        bob.block_id,
        "{replacement}"
    );
    assert_eq!(
        selected_transport(replacement_snapshot)["session_id"],
        alice.agent_session_id,
        "the agent session must stay distinct from its Rex address: {replacement}"
    );

    let owners = fixture.rex_block_current_owners();
    assert_eq!(
        owners.get(&bob.block_id),
        Some(&vec![alice_name.clone()]),
        "the later successful binding must be the only current owner of Bob's block: {owners:?}"
    );
    assert!(
        owners
            .get(&alice.block_id)
            .is_none_or(|owners| owners.is_empty()),
        "Alice's old block must not remain a current delivery target: {owners:?}"
    );
    assert!(
        !owners.values().flatten().any(|owner| owner == &bob_name),
        "Bob must lose current ownership of the replaced block without being deleted: {owners:?}"
    );
    fixture.evidence(
        "block_binding_replacement",
        json!({
            "alice": {"agent_session_id": alice.agent_session_id, "block_id": alice.block_id},
            "bob": {"agent_session_id": bob.agent_session_id, "block_id": bob.block_id},
            "current_owners": owners,
        }),
    );

    let errors = fixture.finish();
    assert!(
        errors.is_empty(),
        "owned Rex session cleanup failed: {errors:?}"
    );
    eprintln!("Rex live evidence: {}", fixture.evidence_path.display());
}

#[test]
#[ignore = "requires an explicitly selected Rex server"]
fn rex_public_block_binding_directs_delivery_to_the_current_owner_only() {
    let mut fixture = RexLiveFixture::new("block-delivery");
    let sender = fixture.create_receiver_session("binding-sender");
    let old_owner = fixture.create_receiver_session("binding-old-owner");
    let new_owner = fixture.create_receiver_session("binding-new-owner");
    for owned in [&sender, &old_owner, &new_owner] {
        fixture.wait_receiver_ready(owned, Duration::from_secs(20));
    }
    fixture.start_daemon();

    for owned in [&sender, &old_owner, &new_owner] {
        let context = fixture.context_provide(owned);
        assert_eq!(context["result"]["outcome"], "completed", "{context}");
    }
    let sender_id = worker_id(snapshot(&fixture.context_provide(&sender)));
    let old_id = worker_id(snapshot(&fixture.context_provide(&old_owner)));
    let new_id = worker_id(snapshot(&fixture.context_provide(&new_owner)));

    let replacement = fixture.context_provide_on_block(&new_owner, &old_owner);
    assert_eq!(
        replacement["result"]["outcome"], "completed",
        "the later successful binding must replace the old owner: {replacement}"
    );
    assert_eq!(
        snapshot(&replacement)["identity"]["transport"]["rex_endpoint"]["block_id"],
        old_owner.block_id,
        "{replacement}"
    );

    let old_body = "old owner body must not reach the replaced block";
    let old_output = fixture
        .command(
            &[
                "sendmessage",
                "--to",
                old_id.as_str(),
                "--subject",
                "rex block binding old owner",
                old_body,
            ],
            Some(&sender.agent_session_id),
        )
        .output()
        .expect("send a durable message to the displaced identity");
    assert_eq!(
        old_output.status.code(),
        Some(1),
        "the displaced identity has no current Rex delivery target: stdout={} stderr={}",
        String::from_utf8_lossy(&old_output.stdout),
        String::from_utf8_lossy(&old_output.stderr)
    );
    let old_stderr = String::from_utf8_lossy(&old_output.stderr);
    assert!(
        old_stderr.contains("REX_CURRENT_BINDING_MISSING"),
        "the notification failure must identify the absent current binding: {old_stderr}"
    );
    let old_response = old_stderr
        .lines()
        .find_map(|line| line.strip_prefix("collab response: "))
        .unwrap_or_else(|| panic!("CLI omitted the durable response: {old_stderr}"));
    let old_sent: Value =
        serde_json::from_str(old_response).expect("decode durable notification failure");
    assert_eq!(old_sent["durable"], true, "{old_sent}");
    assert_eq!(
        old_sent["notification"], "subscribed-not-sent",
        "{old_sent}"
    );
    assert_eq!(
        old_sent["failure"], "notification_delivery_failed",
        "{old_sent}"
    );
    let old_message_id = old_sent["msg_id"]
        .as_str()
        .or_else(|| old_sent["message_id"].as_str())
        .unwrap_or_else(|| panic!("old-owner send has no durable message id: {old_sent}"))
        .to_owned();

    let new_body = "new owner body must reach the current block";
    let new_sent = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            new_id.as_str(),
            "--subject",
            "rex block binding new owner",
            new_body,
        ],
        Some(&sender.agent_session_id),
    );
    let new_message_id = new_sent["msg_id"]
        .as_str()
        .or_else(|| new_sent["message_id"].as_str())
        .unwrap_or_else(|| panic!("new-owner send has no durable message id: {new_sent}"))
        .to_owned();

    // The old identity keeps its mailbox but no longer owns a current Rex
    // target. Prove its rejected wake did not write the shared block.
    fixture.wait_capture_contains(&old_owner, &new_message_id, Duration::from_secs(20));
    let block_capture = fixture
        .capture(&old_owner)
        .expect("capture the shared test-owned block");
    assert!(
        !block_capture.contains(&old_message_id),
        "the displaced owner's send must not write the shared block: {block_capture}"
    );

    let receive_id = format!("rex-block-binding-recv-{}", fixture.unique_token());
    let received = fixture.run_ok(
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            receive_id.as_str(),
        ],
        Some(&new_owner.agent_session_id),
    );
    assert_eq!(received["count"], 1, "{received}");
    assert_eq!(received["messages"][0]["id"], new_message_id);
    assert_eq!(received["messages"][0]["to"], new_id);
    assert_eq!(received["messages"][0]["from"], sender_id);
    assert_eq!(received["messages"][0]["body"], new_body);

    let new_message = fixture.run_ok(
        &["msg", new_message_id.as_str()],
        Some(&sender.agent_session_id),
    );
    assert_eq!(new_message["state"], "read");
    assert_eq!(new_message["consumed_by_recv"], true);
    assert_eq!(new_message["to"], new_id);

    let old_message = fixture.run_ok(
        &["msg", old_message_id.as_str()],
        Some(&sender.agent_session_id),
    );
    assert_eq!(old_message["state"], "pending");
    assert_eq!(old_message["consumed_by_recv"], false);
    assert_eq!(
        old_message["to"], old_id,
        "mailbox ownership must not transfer"
    );
    fixture.evidence(
        "block_binding_delivery",
        json!({
            "sender": sender_id,
            "old_owner": {"worker_id": old_id, "block_id": old_owner.block_id, "message_id": old_message_id},
            "new_owner": {"worker_id": new_id, "block_id": old_owner.block_id, "message_id": new_message_id},
            "received": received,
        }),
    );

    let errors = fixture.finish();
    assert!(
        errors.is_empty(),
        "owned Rex session cleanup failed: {errors:?}"
    );
    eprintln!("Rex live evidence: {}", fixture.evidence_path.display());
}

#[test]
#[ignore = "requires an explicitly selected Rex server"]
fn rex_public_context_send_recv_reply_restart_and_body_fidelity() {
    let mut fixture = RexLiveFixture::new("contract");
    let receiver = fixture.create_receiver_session("receiver");
    let sender = fixture.create_receiver_session("sender");
    fixture.wait_receiver_ready(&receiver, Duration::from_secs(20));
    fixture.wait_receiver_ready(&sender, Duration::from_secs(20));

    // This framed probe proves that the receiver returns a complete input ACK,
    // which is distinct from terminal echo.
    let probe_case = format!("probe-{}", fixture.unique_token());
    let probe_payload = "probe 中文 trailing  ";
    fixture.write_frame_and_enter(&receiver, &probe_case, probe_payload);
    let probe_ack = fixture.wait_receiver_document(&receiver, Duration::from_secs(20), |value| {
        value["kind"] == "test_receiver_ack" && value["case_id"] == probe_case
    });
    assert_eq!(probe_ack["received"].as_str(), Some(probe_payload));
    assert_eq!(
        probe_ack["byte_count"].as_u64(),
        Some(probe_payload.as_bytes().len() as u64)
    );
    fixture.evidence("receiver_probe_ack", probe_ack);

    fixture.start_daemon();
    let sender_context = fixture.context_provide(&sender);
    let receiver_context = fixture.context_provide(&receiver);
    assert_eq!(
        sender_context["result"]["outcome"], "completed",
        "{sender_context}"
    );
    assert_eq!(
        receiver_context["result"]["outcome"], "completed",
        "{receiver_context}"
    );

    let sender_snapshot = snapshot(&sender_context);
    let receiver_snapshot = snapshot(&receiver_context);
    let sender_id = worker_id(sender_snapshot);
    let receiver_id = worker_id(receiver_snapshot);
    let sender_transport = selected_transport(sender_snapshot);
    let receiver_transport = selected_transport(receiver_snapshot);
    assert_eq!(sender_transport["kind"], "rex", "{sender_context}");
    assert_eq!(receiver_transport["kind"], "rex", "{receiver_context}");
    assert_eq!(
        sender_transport["session_id"].as_str(),
        Some(sender.agent_session_id.as_str()),
        "the agent session id must stay distinct from the Rex session id"
    );
    assert_eq!(
        receiver_transport["session_id"].as_str(),
        Some(receiver.agent_session_id.as_str())
    );
    assert_eq!(
        sender_transport["rex_endpoint"]["server"].as_str(),
        Some(fixture.rex_server.as_str())
    );
    assert_eq!(
        sender_transport["rex_endpoint"]["session_id"].as_str(),
        Some(sender.session_id.as_str())
    );
    assert_eq!(
        sender_transport["rex_endpoint"]["block_id"].as_str(),
        Some(sender.block_id.as_str())
    );
    assert_eq!(
        receiver_transport["rex_endpoint"]["session_id"].as_str(),
        Some(receiver.session_id.as_str())
    );
    assert_eq!(
        receiver_transport["rex_endpoint"]["block_id"].as_str(),
        Some(receiver.block_id.as_str())
    );
    assert!(sender_transport["rex_endpoint"]["instance_id"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
    assert!(sender_transport["endpoint"].is_null());
    assert!(sender_transport["namespace"].is_null());
    assert!(sender_transport["tmux_endpoint"].is_null());
    fixture.evidence(
        "context_registered",
        json!({
            "sender": sender_snapshot,
            "receiver": receiver_snapshot,
        }),
    );

    fixture.write_frame_and_enter(
        &receiver,
        "osc-status",
        "state=working:id=collab-live:app=codex",
    );
    let busy = fixture.wait_agent_state(&receiver, "working");
    assert_eq!(busy["activity"], "busy");
    assert_eq!(busy["agent_presence"], "online");
    fixture.write_frame_and_enter(
        &receiver,
        "osc-status",
        "state=blocked:kind=question:id=collab-live:app=codex",
    );
    let question = fixture.wait_agent_state(&receiver, "waiting_user");
    assert_eq!(question["activity"], "waiting_user");
    fixture.write_frame_and_enter(
        &receiver,
        "osc-status",
        "state=done:id=collab-live:app=codex",
    );
    // A valid done record for the current foreground owner is not working.
    // Under the fixed projection it reports idle instead of the old unknown.
    let done = fixture.wait_agent_state(&receiver, "idle");
    assert_eq!(done["activity"], "idle");
    fixture.write_frame_and_enter(
        &receiver,
        "osc-status",
        "state=error:id=collab-live:app=codex",
    );
    let error_state = fixture.wait_agent_state(&receiver, "idle");
    assert_eq!(error_state["activity"], "idle");
    fixture.write_frame_and_enter(
        &receiver,
        "osc-status",
        "state=future_state:id=collab-live:app=codex",
    );
    let future_state = fixture.wait_agent_state(&receiver, "idle");
    assert_eq!(future_state["activity"], "idle");

    let notify_status = fixture.run_ok(&["notify", "status"], Some(&receiver.agent_session_id));
    let lease = direct_subscription(&notify_status, &receiver_id);
    assert_eq!(lease["method"], "rex");
    assert_eq!(lease["status"], "armed");
    let expected_target = json!([
        fixture.rex_server.as_str(),
        receiver.session_id.as_str(),
        receiver.block_id.as_str(),
    ])
    .to_string();
    assert_eq!(
        lease["target"].as_str(),
        Some(expected_target.as_str()),
        "the lease must retain the full service-scoped terminal target"
    );
    fixture.evidence("default_lease", lease);

    let body = "persist before consume 中文 body";
    let sent = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &receiver_id,
            "--subject",
            "rex live contract",
            body,
        ],
        Some(&sender.agent_session_id),
    );
    let message_id = sent["msg_id"]
        .as_str()
        .or_else(|| sent["message_id"].as_str())
        .unwrap_or_else(|| panic!("sendmessage has no durable message id: {sent}"))
        .to_owned();
    assert_eq!(sent["consumed"], false);
    assert_eq!(sent["durable"], true);
    fixture.evidence("message_sent", sent.clone());

    fixture.wait_capture_contains(&receiver, &message_id, Duration::from_secs(20));
    let notification =
        fixture.wait_receiver_document(&receiver, Duration::from_secs(30), |value| {
            value["kind"] == "test_receiver_raw_input"
                && value["received"].as_str().is_some_and(|received| {
                    received.contains(&message_id) && received.contains(body)
                })
        });
    let notification_text = notification["received"]
        .as_str()
        .unwrap_or_else(|| panic!("receiver raw input has no text: {notification}"));
    assert!(
        notification_text.starts_with(&format!("COLLAB_NOTIFY {message_id} [notification-batch] ")),
        "notification must carry the message id in the public batch envelope: {notification_text}"
    );
    assert!(
        notification_text.contains(&format!("entries={message_id}:")),
        "notification batch must name the durable message id: {notification_text}"
    );
    assert!(
        notification_text.contains("rex live contract"),
        "notification batch must preserve the subject: {notification_text}"
    );
    assert!(
        notification_text.contains(body),
        "notification body must preserve the original text: {notification_text}"
    );
    assert_eq!(
        notification["byte_count"].as_u64(),
        Some(notification_text.as_bytes().len() as u64)
    );
    fixture.evidence(
        "notification_body",
        json!({
            "message_id": message_id,
            "body": body,
            "notification": notification,
        }),
    );

    let pending = fixture.run_ok(&["msg", &message_id], Some(&sender.agent_session_id));
    assert_eq!(pending["state"], "pending");
    assert_eq!(pending["consumed_by_recv"], false);
    fixture.evidence("message_pending", pending);

    let receive_id = format!("rex-live-recv-{}", fixture.unique_token());
    let received = fixture.run_ok(
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            receive_id.as_str(),
        ],
        Some(&receiver.agent_session_id),
    );
    assert_eq!(received["count"], 1, "{received}");
    assert_eq!(received["messages"][0]["id"], message_id);
    assert_eq!(received["messages"][0]["body"], body);
    assert_eq!(received["messages"][0]["from"], sender_id);
    assert!(received["messages"][0]["in_reply_to"].is_null());
    assert_eq!(received["receive_id"], receive_id);
    assert_eq!(received["replayed"], false);
    fixture.evidence("message_received", received.clone());

    let consumed = fixture.run_ok(&["msg", &message_id], Some(&sender.agent_session_id));
    assert_eq!(consumed["state"], "read");
    assert_eq!(consumed["consumed_by_recv"], true);

    let replayed = fixture.run_ok(
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            receive_id.as_str(),
        ],
        Some(&receiver.agent_session_id),
    );
    assert_eq!(replayed["replayed"], true, "{replayed}");
    assert_eq!(replayed["receive_id"], receive_id);
    assert_eq!(replayed["count"], 1);
    assert_eq!(replayed["messages"][0]["id"], message_id);
    fixture.evidence("message_receive_replayed", replayed);

    let reply_body = "reply body 中文 association";
    let reply = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &sender_id,
            "--subject",
            "rex live reply",
            "--in-reply-to",
            &message_id,
            reply_body,
        ],
        Some(&receiver.agent_session_id),
    );
    let reply_id = reply["msg_id"]
        .as_str()
        .or_else(|| reply["message_id"].as_str())
        .unwrap_or_else(|| panic!("reply has no durable message id: {reply}"))
        .to_owned();
    assert_eq!(reply["consumed"], false);
    fixture.evidence("reply_sent", reply.clone());

    fixture.wait_capture_contains(&sender, &reply_id, Duration::from_secs(20));
    let reply_notification =
        fixture.wait_receiver_document(&sender, Duration::from_secs(30), |value| {
            value["kind"] == "test_receiver_raw_input"
                && value["received"].as_str().is_some_and(|received| {
                    received.contains(&reply_id) && received.contains(reply_body)
                })
        });

    let reply_pending = fixture.run_ok(&["msg", &reply_id], Some(&receiver.agent_session_id));
    assert_eq!(reply_pending["consumed_by_recv"], false);
    fixture.evidence("reply_notification", reply_notification);

    let survive_body = "survive restart unread 中文";
    let survive = fixture.run_ok(
        &[
            "sendmessage",
            "--to",
            &receiver_id,
            "--subject",
            "rex live restart",
            survive_body,
        ],
        Some(&sender.agent_session_id),
    );
    let survive_id = survive["msg_id"]
        .as_str()
        .or_else(|| survive["message_id"].as_str())
        .unwrap_or_else(|| panic!("survive send has no durable message id: {survive}"))
        .to_owned();
    fixture.wait_capture_contains(&receiver, &survive_id, Duration::from_secs(20));
    fixture.wait_receiver_document(&receiver, Duration::from_secs(30), |value| {
        value["kind"] == "test_receiver_raw_input"
            && value["received"].as_str().is_some_and(|received| {
                received.contains(&survive_id) && received.contains(survive_body)
            })
    });

    let sender_identity_before = sender_snapshot["identity"].clone();
    let sender_binding_before = sender_snapshot["binding"].clone();
    let receiver_identity_before = receiver_snapshot["identity"].clone();
    let receiver_binding_before = receiver_snapshot["binding"].clone();
    fixture.restart_daemon();

    let receiver_restored = fixture.context_provide(&receiver);
    let receiver_restored_snapshot = snapshot(&receiver_restored);
    assert_eq!(
        receiver_restored_snapshot["identity"]["worker_id"], receiver_id,
        "receiver identity must survive the test-owned daemon restart"
    );
    assert_eq!(
        receiver_restored_snapshot["identity"],
        receiver_identity_before
    );
    assert_eq!(
        receiver_restored_snapshot["binding"],
        receiver_binding_before
    );

    let sender_restored = fixture.context_provide(&sender);
    let sender_restored_snapshot = snapshot(&sender_restored);
    assert_eq!(sender_restored_snapshot["identity"]["worker_id"], sender_id);
    assert_eq!(sender_restored_snapshot["identity"], sender_identity_before);
    assert_eq!(sender_restored_snapshot["binding"], sender_binding_before);

    let survive_receive_id = format!("rex-live-survive-{}", fixture.unique_token());
    let survived = fixture.run_ok(
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            survive_receive_id.as_str(),
        ],
        Some(&receiver.agent_session_id),
    );
    assert_eq!(survived["count"], 1, "{survived}");
    assert_eq!(survived["messages"][0]["id"], survive_id);
    assert_eq!(survived["messages"][0]["body"], survive_body);
    assert_eq!(survived["messages"][0]["from"], sender_id);
    fixture.evidence("unread_message_survived_restart", survived);

    let reply_receive_id = format!("rex-live-reply-{}", fixture.unique_token());
    let reply_received = fixture.run_ok(
        &[
            "recv",
            "--timeout",
            "0",
            "--receive-id",
            reply_receive_id.as_str(),
        ],
        Some(&sender.agent_session_id),
    );
    assert_eq!(reply_received["count"], 1, "{reply_received}");
    assert_eq!(reply_received["messages"][0]["id"], reply_id);
    assert_eq!(reply_received["messages"][0]["body"], reply_body);
    assert_eq!(reply_received["messages"][0]["from"], receiver_id);
    assert_eq!(reply_received["messages"][0]["in_reply_to"], message_id);
    fixture.evidence("reply_survived_restart", reply_received);

    let cleanup_errors = fixture.finish();
    assert!(
        cleanup_errors.is_empty(),
        "owned Rex session cleanup failed: {cleanup_errors:?}"
    );
    eprintln!("Rex live evidence: {}", fixture.evidence_path.display());
}
