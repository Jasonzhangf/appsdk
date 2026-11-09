#![cfg(unix)]

include!("support/federation_public_fixture.rs");

fn snapshot(value: &Value) -> &Value {
    value
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .unwrap_or(value)
}

fn worker_id(value: &Value) -> String {
    snapshot(value)["identity"]["worker_id"]
        .as_str()
        .unwrap_or_else(|| panic!("context snapshot has no worker id: {value}"))
        .to_owned()
}

fn target_for(directory: &Value, worker: &str) -> Value {
    directory["targets"]
        .as_array()
        .unwrap_or_else(|| panic!("directory has no targets: {directory}"))
        .iter()
        .find(|target| target["worker_id"] == worker)
        .cloned()
        .unwrap_or_else(|| panic!("directory has no target for {worker}: {directory}"))
}

fn qualified_address(target: &Value) -> String {
    target["qualified_address"]
        .as_str()
        .unwrap_or_else(|| panic!("target has no qualified address: {target}"))
        .to_owned()
}

fn local_address(target: &Value) -> String {
    target["address"]
        .as_str()
        .unwrap_or_else(|| panic!("target has no local address: {target}"))
        .to_owned()
}

fn register_pair(
    fixture: &mut FederationPublicFixture,
) -> (OwnedRexSession, OwnedRexSession, Value, Value) {
    let alpha_receiver = fixture.alpha.create_receiver_session("alpha-receiver");
    let beta_receiver = fixture.beta.create_receiver_session("beta-receiver");
    fixture
        .alpha
        .wait_receiver_ready(&alpha_receiver, Duration::from_secs(20));
    fixture
        .beta
        .wait_receiver_ready(&beta_receiver, Duration::from_secs(20));
    let alpha_context = fixture.alpha.context_provide(&alpha_receiver);
    let beta_context = fixture.beta.context_provide(&beta_receiver);
    assert_eq!(
        alpha_context["result"]["outcome"], "completed",
        "{alpha_context}"
    );
    assert_eq!(
        beta_context["result"]["outcome"], "completed",
        "{beta_context}"
    );
    fixture.alpha.evidence(
        "registered",
        json!({"alpha": alpha_context.clone(), "beta": beta_context.clone()}),
    );
    (alpha_receiver, beta_receiver, alpha_context, beta_context)
}

fn pair_targets(
    fixture: &FederationPublicFixture,
    alpha_worker: &str,
    beta_worker: &str,
) -> (Value, Value) {
    let alpha_directory = fixture
        .alpha
        .run_ok(&["directory", "--machine", "alpha"], None);
    let beta_directory = fixture
        .alpha
        .run_ok(&["directory", "--machine", "beta"], None);
    (
        target_for(&alpha_directory, alpha_worker),
        target_for(&beta_directory, beta_worker),
    )
}

fn send_remote(
    fixture: &RexLiveFixture,
    agent_session_id: &str,
    to: &str,
    subject: &str,
    body: &str,
    operation_id: &str,
    in_reply_to: Option<&str>,
) -> Value {
    let mut args = vec![
        "sendmessage".to_owned(),
        "--to".to_owned(),
        to.to_owned(),
        "--subject".to_owned(),
        subject.to_owned(),
        "--content".to_owned(),
        body.to_owned(),
        "--operation-id".to_owned(),
        operation_id.to_owned(),
    ];
    if let Some(reference) = in_reply_to {
        args.push("--in-reply-to".to_owned());
        args.push(reference.to_owned());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    fixture.run_ok(&refs, Some(agent_session_id))
}

fn recv_messages(fixture: &RexLiveFixture, agent_session_id: &str, receive_id: &str) -> Value {
    fixture.run_ok(
        &["recv", "--timeout", "0", "--receive-id", receive_id],
        Some(agent_session_id),
    )
}

fn run_error(fixture: &RexLiveFixture, args: &[&str], agent_session_id: &str) -> String {
    let output = fixture
        .command(args, Some(agent_session_id))
        .output()
        .expect("run public Collab CLI");
    assert!(
        !output.status.success(),
        "collab {args:?} unexpectedly succeeded: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
#[ignore = "requires COLLAB_TEST_BINARY, COLLAB_REX_TEST_SERVER, and an authorized loopback SSH identity"]
fn federation_public_two_daemon_contract() {
    let mut fixture = FederationPublicFixture::new("contract", false);
    fixture.configure();
    fixture.start();
    let (alpha_receiver, beta_receiver, alpha_context, beta_context) = register_pair(&mut fixture);
    let alpha_worker = worker_id(&alpha_context);
    let beta_worker = worker_id(&beta_context);
    assert_ne!(alpha_worker, beta_worker);

    let alpha_check = fixture.alpha.run_ok(&["machine", "check", "beta"], None);
    assert_eq!(alpha_check["reachable"], true, "{alpha_check}");
    assert_eq!(alpha_check["machine"], "beta", "{alpha_check}");
    assert_eq!(
        alpha_check["protocol"], "collab-federation/v1",
        "{alpha_check}"
    );
    let beta_check = fixture.beta.run_ok(&["machine", "check", "alpha"], None);
    assert_eq!(beta_check["reachable"], true, "{beta_check}");
    assert_eq!(beta_check["machine"], "alpha", "{beta_check}");
    fixture.alpha.evidence(
        "machine_checks",
        json!({"alpha_to_beta": alpha_check, "beta_to_alpha": beta_check}),
    );

    let (alpha_target, beta_target) = pair_targets(&fixture, &alpha_worker, &beta_worker);
    let alpha_local = local_address(&alpha_target);
    let beta_local = local_address(&beta_target);
    let alpha_qualified = qualified_address(&alpha_target);
    let beta_qualified = qualified_address(&beta_target);
    assert_eq!(alpha_qualified, format!("alpha.{alpha_local}"));
    assert_eq!(beta_qualified, format!("beta.{beta_local}"));
    assert_eq!(alpha_target["machine"], "alpha", "{alpha_target}");
    assert_eq!(beta_target["machine"], "beta", "{beta_target}");
    assert_ne!(
        alpha_target["project"], beta_target["project"],
        "each daemon must own a different project scope"
    );
    fixture.alpha.evidence(
        "qualified_directory",
        json!({"alpha": alpha_target.clone(), "beta": beta_target.clone()}),
    );

    let unknown_address = format!("unknown.{beta_local}");
    let unknown_error = run_error(
        &fixture.alpha,
        &[
            "sendmessage",
            "--to",
            unknown_address.as_str(),
            "--subject",
            "unknown machine",
            "--content",
            "must not fall back locally",
        ],
        &alpha_receiver.agent_session_id,
    );
    assert!(
        unknown_error.contains("MACHINE_UNKNOWN"),
        "{unknown_error}\n{}",
        fixture.daemon_logs()
    );
    fixture
        .alpha
        .evidence("unknown_machine_rejected", json!({"error": unknown_error}));

    let body = "federation public 中文 body\nsecond line";
    let operation_id = format!("fed-contract-{}", fixture.alpha.unique_token());
    let sent = send_remote(
        &fixture.alpha,
        &alpha_receiver.agent_session_id,
        &beta_qualified,
        "federation contract",
        body,
        &operation_id,
        None,
    );
    assert_eq!(
        sent["outcome"],
        "accepted",
        "{sent}\n{}",
        fixture.daemon_logs()
    );
    assert_eq!(sent["accepted"], true, "{sent}");
    assert_eq!(sent["durable"], true, "{sent}");
    assert_eq!(sent["consumed"], false, "{sent}");
    let message_ref = sent["message_ref"]
        .as_str()
        .expect("accepted federation send has a message_ref")
        .to_owned();
    let origin_ref = sent["origin_ref"]
        .as_str()
        .expect("accepted federation send has an origin_ref")
        .to_owned();
    let target_reference = sent["reference"].clone();
    let beta_message_id = sent["msg_id"]
        .as_str()
        .expect("accepted federation send has a target message id")
        .to_owned();
    fixture.alpha.evidence("federation_sent", sent.clone());

    let notification =
        fixture
            .beta
            .wait_receiver_document(&beta_receiver, Duration::from_secs(30), |value| {
                value["kind"] == "test_receiver_raw_input"
                    && value["received"].as_str().is_some_and(|received| {
                        received.contains(&beta_message_id)
                            && received.contains(&body.replace('\n', "\\n"))
                    })
            });
    fixture
        .beta
        .evidence("federation_notification", notification);

    let pending = fixture.beta.run_ok(
        &["msg", &beta_message_id],
        Some(&beta_receiver.agent_session_id),
    );
    assert_eq!(pending["state"], "pending", "{pending}");
    assert_eq!(pending["consumed_by_recv"], false, "{pending}");
    fixture.beta.evidence("message_pending", pending);

    let receive_id = format!("fed-contract-recv-{}", fixture.alpha.unique_token());
    let received = recv_messages(&fixture.beta, &beta_receiver.agent_session_id, &receive_id);
    assert_eq!(
        received["count"],
        1,
        "{received}\n{}",
        fixture.daemon_logs()
    );
    assert_eq!(received["replayed"], false, "{received}");
    let received_message = &received["messages"][0];
    assert_eq!(received_message["id"], beta_message_id.as_str());
    assert_eq!(received_message["from"], alpha_qualified.as_str());
    assert_eq!(received_message["to"], beta_worker.as_str());
    assert_eq!(received_message["subject"], "federation contract");
    assert_eq!(received_message["body"], body);
    assert_eq!(
        received_message["federation"]["reference"],
        target_reference
    );
    assert_eq!(received_message["federation"]["origin"]["machine"], "alpha");
    assert_eq!(received_message["federation"]["sender"]["kind"], "peer");
    assert_eq!(
        received_message["federation"]["sender"]["value"]["address"]["machine"],
        "alpha"
    );
    assert_eq!(received_message["federation"]["recipient"]["kind"], "peer");
    assert_eq!(
        received_message["federation"]["recipient"]["value"]["address"]["machine"],
        "beta"
    );
    fixture.beta.evidence("message_received", received.clone());

    let replayed = recv_messages(&fixture.beta, &beta_receiver.agent_session_id, &receive_id);
    assert_eq!(replayed["replayed"], true, "{replayed}");
    assert_eq!(replayed["count"], 1, "{replayed}");
    assert_eq!(replayed["messages"][0]["id"], beta_message_id.as_str());
    fixture.beta.evidence("message_replayed", replayed);

    let beta_receipt = fixture.beta.run_ok(
        &["msg", &message_ref],
        Some(&beta_receiver.agent_session_id),
    );
    assert_eq!(beta_receipt["outcome"], "accepted", "{beta_receipt}");
    assert_eq!(beta_receipt["consumed"], true, "{beta_receipt}");
    assert_eq!(beta_receipt["consumed_by_recv"], true, "{beta_receipt}");
    assert_eq!(
        beta_receipt["receipt"]["notification"], "accepted",
        "{beta_receipt}"
    );
    fixture.beta.evidence("beta_receipt", beta_receipt.clone());

    let alpha_other = fixture.alpha.create_receiver_session("alpha-other");
    fixture
        .alpha
        .wait_receiver_ready(&alpha_other, Duration::from_secs(20));
    let alpha_other_context = fixture.alpha.context_provide(&alpha_other);
    assert_eq!(
        alpha_other_context["result"]["outcome"], "completed",
        "{alpha_other_context}"
    );
    let alpha_other_worker = worker_id(&alpha_other_context);
    assert_ne!(alpha_other_worker, alpha_worker);
    let wrong_participant = run_error(
        &fixture.alpha,
        &["msg", &message_ref],
        &alpha_other.agent_session_id,
    );
    assert!(
        wrong_participant.contains("FEDERATION_PARTICIPANT_MISMATCH"),
        "{wrong_participant}\n{}",
        fixture.daemon_logs()
    );
    fixture.alpha.evidence(
        "cross_machine_wrong_participant_rejected",
        json!({"error": wrong_participant}),
    );

    // An authenticated machine cannot reuse a recipient name from another app scope.
    let mut wrong_scope_sender = received_message["federation"]["recipient"].clone();
    wrong_scope_sender["value"]["project_scope"] = json!("/different/remote-project");
    wrong_scope_sender["value"]["app_scope_id"] = json!("different-remote-app");
    let negative_request = json!({
        "target": "alpha",
        "credential": fs::read_to_string(&fixture.beta_to_alpha_credential).unwrap().trim(),
        "request": {"kind": "put", "value": {
            "origin": {"machine": "beta", "project_scope": "/different/remote-project",
                "app_scope_id": "different-remote-app", "operation_id": format!("wrong-scope-{}", fixture.alpha.unique_token())},
            "sender": wrong_scope_sender,
            "target": {"kind": "named", "value": alpha_target["route"]["local"]},
            "mtype": "message", "subject": "negative scope", "body": "must reject",
            "delivery": "immediate", "in_reply_to": target_reference,
            "original_origin": sent["origin"],
        }},
    });
    let mut private_stream =
        UnixStream::connect(fixture.alpha.host_state.join("federation/rpc.sock")).unwrap();
    private_stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(private_stream, "{negative_request}").unwrap();
    let mut negative_response = String::new();
    BufReader::new(private_stream)
        .read_line(&mut negative_response)
        .unwrap();
    let negative_response: Value = serde_json::from_str(&negative_response).unwrap();
    assert_eq!(negative_response["kind"], "error", "{negative_response}");
    assert!(
        negative_response["value"]["detail"]
            .as_str()
            .unwrap()
            .contains("FEDERATION_REPLY_PARTICIPANT_MISMATCH"),
        "{negative_response}"
    );
    fixture
        .alpha
        .evidence("wrong_reply_scope_rejected", negative_response);

    let local_body = "local omitted-machine body";
    let local_sent = fixture.alpha.run_ok(
        &[
            "sendmessage",
            "--to",
            &alpha_local,
            "--subject",
            "local contract",
            "--content",
            local_body,
        ],
        Some(&alpha_other.agent_session_id),
    );
    assert_eq!(local_sent["durable"], true, "{local_sent}");
    assert_eq!(local_sent["consumed"], false, "{local_sent}");
    assert_eq!(
        local_sent["notification"], "rex-input-submitted",
        "{local_sent}"
    );
    let local_receive_id = format!("fed-local-recv-{}", fixture.alpha.unique_token());
    let local_received = recv_messages(
        &fixture.alpha,
        &alpha_receiver.agent_session_id,
        &local_receive_id,
    );
    assert_eq!(local_received["count"], 1, "{local_received}");
    assert_eq!(local_received["messages"][0]["body"], local_body);
    assert_eq!(local_received["messages"][0]["subject"], "local contract");
    fixture.alpha.evidence(
        "omitted_machine_local_semantics",
        json!({"sent": local_sent, "received": local_received}),
    );

    let reply_body = "federation reply 中文 body";
    let reply_operation_id = format!("fed-reply-{}", fixture.alpha.unique_token());
    let reply = send_remote(
        &fixture.beta,
        &beta_receiver.agent_session_id,
        &alpha_qualified,
        "federation reply",
        reply_body,
        &reply_operation_id,
        Some(&message_ref),
    );
    assert_eq!(
        reply["outcome"],
        "accepted",
        "{reply}\n{}",
        fixture.daemon_logs()
    );
    let reply_ref = reply["message_ref"]
        .as_str()
        .expect("accepted reply has a message_ref")
        .to_owned();
    let reply_reference = reply["reference"].clone();
    let reply_message_id = reply["msg_id"]
        .as_str()
        .expect("accepted reply has a target message id")
        .to_owned();
    fixture.beta.evidence("reply_sent", reply.clone());

    let alpha_notification =
        fixture
            .alpha
            .wait_receiver_document(&alpha_receiver, Duration::from_secs(30), |value| {
                value["kind"] == "test_receiver_raw_input"
                    && value["received"].as_str().is_some_and(|received| {
                        received.contains(&reply_message_id) && received.contains(reply_body)
                    })
            });
    fixture
        .alpha
        .evidence("reply_notification", alpha_notification);

    let alpha_receive_id = format!("fed-contract-alpha-recv-{}", fixture.alpha.unique_token());
    let alpha_received = recv_messages(
        &fixture.alpha,
        &alpha_receiver.agent_session_id,
        &alpha_receive_id,
    );
    assert_eq!(
        alpha_received["count"],
        1,
        "{alpha_received}\n{}",
        fixture.daemon_logs()
    );
    assert_eq!(
        alpha_received["messages"][0]["id"],
        reply_message_id.as_str()
    );
    assert_eq!(alpha_received["messages"][0]["body"], reply_body);
    assert_eq!(
        alpha_received["messages"][0]["federation"]["in_reply_to"],
        target_reference
    );
    assert_eq!(
        alpha_received["messages"][0]["federation"]["reference"],
        reply_reference
    );
    fixture
        .alpha
        .evidence("reply_received", alpha_received.clone());

    let alpha_remote_receipt = fixture.alpha.run_ok(
        &["msg", &message_ref],
        Some(&alpha_receiver.agent_session_id),
    );
    assert_eq!(
        alpha_remote_receipt["outcome"], "accepted",
        "{alpha_remote_receipt}"
    );
    assert_eq!(alpha_remote_receipt["consumed"], true);
    assert_eq!(alpha_remote_receipt["consumed_by_recv"], true);
    fixture
        .alpha
        .evidence("remote_receipt", alpha_remote_receipt);

    let alpha_reply_receipt = fixture
        .alpha
        .run_ok(&["msg", &reply_ref], Some(&alpha_receiver.agent_session_id));
    assert_eq!(
        alpha_reply_receipt["outcome"], "accepted",
        "{alpha_reply_receipt}"
    );
    assert_eq!(alpha_reply_receipt["consumed"], true);
    fixture.alpha.evidence("reply_receipt", alpha_reply_receipt);

    let origin_receipt = fixture.alpha.run_ok(
        &["msg", &origin_ref],
        Some(&alpha_receiver.agent_session_id),
    );
    assert_eq!(origin_receipt["outcome"], "accepted", "{origin_receipt}");
    assert_eq!(origin_receipt["consumed"], true, "{origin_receipt}");
    fixture.alpha.evidence("origin_receipt", origin_receipt);

    let retried = send_remote(
        &fixture.alpha,
        &alpha_receiver.agent_session_id,
        &beta_qualified,
        "federation contract",
        body,
        &operation_id,
        None,
    );
    assert_eq!(
        retried["outcome"],
        "accepted",
        "{retried}\n{}",
        fixture.daemon_logs()
    );
    assert_eq!(retried["message_ref"], message_ref.as_str());
    assert_eq!(retried["msg_id"], beta_message_id.as_str());
    assert_eq!(retried["reference"], target_reference);
    assert_eq!(retried["consumed"], true, "{retried}");
    fixture.alpha.evidence("operation_replayed", retried);

    let body_conflict = run_error(
        &fixture.alpha,
        &[
            "sendmessage",
            "--to",
            beta_qualified.as_str(),
            "--subject",
            "federation contract",
            "--content",
            "different body",
            "--operation-id",
            operation_id.as_str(),
        ],
        &alpha_receiver.agent_session_id,
    );
    assert!(
        body_conflict.contains("FEDERATION_OPERATION_CONFLICT"),
        "{body_conflict}\n{}",
        fixture.daemon_logs()
    );
    fixture
        .alpha
        .evidence("operation_body_conflict", json!({"error": body_conflict}));

    let retried_after_conflict = send_remote(
        &fixture.alpha,
        &alpha_receiver.agent_session_id,
        &beta_qualified,
        "federation contract",
        body,
        &operation_id,
        None,
    );
    assert_eq!(
        retried_after_conflict["outcome"], "accepted",
        "{retried_after_conflict}"
    );
    assert_eq!(retried_after_conflict["reference"], target_reference);
    fixture
        .alpha
        .evidence("operation_replayed_after_conflict", retried_after_conflict);

    let target_session_conflict = run_error(
        &fixture.alpha,
        &[
            "sendmessage",
            "--to",
            beta_qualified.as_str(),
            "--subject",
            "federation contract",
            "--content",
            body,
            "--operation-id",
            operation_id.as_str(),
            "--target-session",
            "different-agent-session",
        ],
        &alpha_receiver.agent_session_id,
    );
    assert!(
        target_session_conflict.contains("FEDERATION_OPERATION_CONFLICT"),
        "{target_session_conflict}\n{}",
        fixture.daemon_logs()
    );
    fixture.alpha.evidence(
        "operation_target_session_conflict",
        json!({"error": target_session_conflict}),
    );

    let empty_receive_id = format!("fed-contract-empty-{}", fixture.alpha.unique_token());
    let empty = recv_messages(
        &fixture.beta,
        &beta_receiver.agent_session_id,
        &empty_receive_id,
    );
    assert_eq!(empty["count"], 0, "{empty}");
    fixture.beta.evidence("no_duplicate_message", empty);

    let alpha_name = snapshot(&alpha_context)["name"]
        .as_str()
        .expect("alpha context has a stable name")
        .to_owned();
    let handoff_operation = format!("fed-name-handoff-{}", fixture.alpha.unique_token());
    let handoff_approval = json!({
        "action": "restore_name",
        "requested_name": alpha_name,
        "authorization": "federation public contract fixture continues the named work",
    })
    .to_string();
    let handoff = fixture.alpha.run_ok(
        &[
            "context",
            "--op",
            handoff_operation.as_str(),
            "--approve-grant",
            handoff_approval.as_str(),
        ],
        Some(&alpha_other.agent_session_id),
    );
    assert_eq!(
        snapshot(&handoff)["name"],
        alpha_name,
        "the requested stable name must move to the new holder: {handoff}"
    );
    assert_eq!(
        worker_id(&handoff),
        alpha_other_worker,
        "name handoff must not change the carrier identity: {handoff}"
    );
    let alpha_directory_after_handoff = fixture
        .alpha
        .run_ok(&["directory", "--machine", "alpha"], None);
    let alpha_target_after_handoff =
        target_for(&alpha_directory_after_handoff, &alpha_other_worker);
    assert_eq!(
        qualified_address(&alpha_target_after_handoff),
        alpha_qualified,
        "the qualified address must remain stable across a name handoff"
    );
    fixture.alpha.evidence(
        "stable_name_handoff",
        json!({
            "handoff": handoff,
            "target": alpha_target_after_handoff,
        }),
    );

    let cleanup_errors = fixture.finish();
    assert!(
        cleanup_errors.is_empty(),
        "owned federation cleanup failed: {cleanup_errors:?}\n{}",
        fixture.daemon_logs()
    );
}

#[test]
#[ignore = "requires COLLAB_TEST_BINARY, COLLAB_REX_TEST_SERVER, and an authorized loopback SSH identity"]
fn federation_public_response_loss_unknown_recovers_without_duplicate() {
    let mut fixture = FederationPublicFixture::new("response-loss", true);
    fixture.configure();
    fixture.start();
    let (alpha_receiver, beta_receiver, alpha_context, beta_context) = register_pair(&mut fixture);
    let alpha_worker = worker_id(&alpha_context);
    let beta_worker = worker_id(&beta_context);
    let (_, beta_target) = pair_targets(&fixture, &alpha_worker, &beta_worker);
    let beta_qualified = qualified_address(&beta_target);

    let body = "response loss unknown 中文 body";
    let operation_id = format!("fed-unknown-{}", fixture.alpha.unique_token());
    let first = send_remote(
        &fixture.alpha,
        &alpha_receiver.agent_session_id,
        &beta_qualified,
        "federation unknown",
        body,
        &operation_id,
        None,
    );
    assert_eq!(
        first["outcome"],
        "unknown",
        "{first}\n{}",
        fixture.daemon_logs()
    );
    assert!(first["accepted"].is_null(), "{first}");
    assert!(first["consumed"].is_null(), "{first}");
    assert_eq!(first["origin_durable"], true, "{first}");
    assert!(
        first["transport_error"]
            .as_str()
            .is_some_and(|error| error.contains("federation response ended before a receipt")),
        "{first}"
    );
    let origin_ref = first["origin_ref"]
        .as_str()
        .expect("unknown federation send has an origin_ref")
        .to_owned();
    fixture
        .alpha
        .evidence("response_lost_unknown", first.clone());

    // The proxy dropped only the response. The target already committed.
    let receive_id = format!("fed-unknown-recv-{}", fixture.alpha.unique_token());
    let received = recv_messages(&fixture.beta, &beta_receiver.agent_session_id, &receive_id);
    assert_eq!(
        received["count"],
        1,
        "{received}\n{}",
        fixture.daemon_logs()
    );
    assert_eq!(received["messages"][0]["body"], body);
    let target_reference = received["messages"][0]["federation"]["reference"].clone();
    let beta_message_id = received["messages"][0]["id"]
        .as_str()
        .expect("received message has an id")
        .to_owned();
    fixture
        .beta
        .evidence("response_loss_target_committed", received.clone());

    // Same operation id and body resolves through Lookup, not a second Put.
    let retried = send_remote(
        &fixture.alpha,
        &alpha_receiver.agent_session_id,
        &beta_qualified,
        "federation unknown",
        body,
        &operation_id,
        None,
    );
    assert_eq!(
        retried["outcome"],
        "accepted",
        "{retried}\n{}",
        fixture.daemon_logs()
    );
    assert_eq!(retried["reference"], target_reference);
    assert_eq!(retried["msg_id"], beta_message_id.as_str());
    assert_eq!(retried["consumed"], true, "{retried}");
    fixture
        .alpha
        .evidence("response_lost_recovered", retried.clone());

    let empty_receive_id = format!("fed-unknown-empty-{}", fixture.alpha.unique_token());
    let empty = recv_messages(
        &fixture.beta,
        &beta_receiver.agent_session_id,
        &empty_receive_id,
    );
    assert_eq!(empty["count"], 0, "{empty}");

    let origin_receipt = fixture.alpha.run_ok(
        &["msg", &origin_ref],
        Some(&alpha_receiver.agent_session_id),
    );
    assert_eq!(origin_receipt["outcome"], "accepted", "{origin_receipt}");
    assert_eq!(origin_receipt["consumed"], true, "{origin_receipt}");
    fixture.alpha.evidence("origin_receipt", origin_receipt);

    let cleanup_errors = fixture.finish();
    assert!(
        cleanup_errors.is_empty(),
        "owned federation cleanup failed: {cleanup_errors:?}\n{}",
        fixture.daemon_logs()
    );
}

#[test]
#[ignore = "requires COLLAB_TEST_BINARY, COLLAB_REX_TEST_SERVER, and an authorized loopback SSH identity"]
fn federation_public_restart_preserves_unread_message_and_identity() {
    let mut fixture = FederationPublicFixture::new("restart", false);
    fixture.configure();
    fixture.start();
    let (alpha_receiver, beta_receiver, alpha_context, beta_context) = register_pair(&mut fixture);
    let alpha_worker = worker_id(&alpha_context);
    let beta_worker = worker_id(&beta_context);
    let (alpha_target, beta_target) = pair_targets(&fixture, &alpha_worker, &beta_worker);
    let alpha_qualified = qualified_address(&alpha_target);
    let beta_qualified = qualified_address(&beta_target);

    let body = "restart unread 中文 body";
    let operation_id = format!("fed-restart-{}", fixture.alpha.unique_token());
    let sent = send_remote(
        &fixture.alpha,
        &alpha_receiver.agent_session_id,
        &beta_qualified,
        "federation restart",
        body,
        &operation_id,
        None,
    );
    assert_eq!(
        sent["outcome"],
        "accepted",
        "{sent}\n{}",
        fixture.daemon_logs()
    );
    let origin_ref = sent["origin_ref"]
        .as_str()
        .expect("accepted federation send has an origin_ref")
        .to_owned();
    let beta_message_id = sent["msg_id"]
        .as_str()
        .expect("accepted federation send has a target message id")
        .to_owned();
    let target_reference = sent["reference"].clone();

    fixture
        .beta
        .wait_receiver_document(&beta_receiver, Duration::from_secs(30), |value| {
            value["kind"] == "test_receiver_raw_input"
                && value["received"].as_str().is_some_and(|received| {
                    received.contains(&beta_message_id) && received.contains(body)
                })
        });

    fixture.alpha.restart_daemon();
    fixture.beta.restart_daemon();
    let alpha_restored = fixture.alpha.context_provide(&alpha_receiver);
    let beta_restored = fixture.beta.context_provide(&beta_receiver);
    assert_eq!(worker_id(&alpha_restored), alpha_worker, "{alpha_restored}");
    assert_eq!(worker_id(&beta_restored), beta_worker, "{beta_restored}");
    fixture.alpha.evidence(
        "restored_identity",
        json!({"alpha": alpha_restored, "beta": beta_restored}),
    );
    let (alpha_target_after, beta_target_after) =
        pair_targets(&fixture, &alpha_worker, &beta_worker);
    assert_eq!(
        qualified_address(&alpha_target_after),
        alpha_qualified,
        "alpha stable address must survive restart"
    );
    assert_eq!(
        qualified_address(&beta_target_after),
        beta_qualified,
        "beta stable address must survive restart"
    );

    let receive_id = format!("fed-restart-recv-{}", fixture.alpha.unique_token());
    let received = recv_messages(&fixture.beta, &beta_receiver.agent_session_id, &receive_id);
    assert_eq!(
        received["count"],
        1,
        "{received}\n{}",
        fixture.daemon_logs()
    );
    assert_eq!(received["messages"][0]["id"], beta_message_id.as_str());
    assert_eq!(received["messages"][0]["body"], body);
    assert_eq!(
        received["messages"][0]["federation"]["reference"],
        target_reference
    );
    fixture
        .beta
        .evidence("unread_message_survived_restart", received);

    let origin_receipt = fixture.alpha.run_ok(
        &["msg", &origin_ref],
        Some(&alpha_receiver.agent_session_id),
    );
    assert_eq!(origin_receipt["outcome"], "accepted", "{origin_receipt}");
    assert_eq!(origin_receipt["consumed"], true, "{origin_receipt}");
    fixture.alpha.evidence("origin_receipt", origin_receipt);

    let cleanup_errors = fixture.finish();
    assert!(
        cleanup_errors.is_empty(),
        "owned federation cleanup failed: {cleanup_errors:?}\n{}",
        fixture.daemon_logs()
    );
}

#[test]
#[ignore = "requires explicit candidate, Rex server and authorized loopback SSH identity"]
fn federation_channel_reply_reaches_the_original_owner_once() {
    let mut fixture = FederationPublicFixture::new("channel", false);
    fixture.configure();
    let capture = fixture.root.join("qq-send-calls.jsonl");
    let helper = fixture.root.join("qq-send-helper.py");
    fs::write(&helper, format!(
        "import json,sys\nrequest=json.load(sys.stdin)\nwith open({},'a') as stream: stream.write(json.dumps(request,ensure_ascii=False)+'\\n')\nprint(json.dumps({{'kind':'accepted','message_id':request['message_id'],'platform_message_id':'fixture-qq-message','timestamp':'2026-11-08T00:00:00Z'}}))\n",
        serde_json::to_string(&capture).unwrap(),
    )).unwrap();
    let config_path = fixture.alpha.root.join("home/.appsdk/config.toml");
    let mut config = fs::OpenOptions::new()
        .append(true)
        .open(config_path)
        .unwrap();
    writeln!(config, "\n[qqbot.delivery]\nnode_program = {}\nhelper_path = {}\nruntime_root = {}\nstate_root = {}\ndeadline_ms = 5000",
        serde_json::to_string(&python_executable()).unwrap(),
        serde_json::to_string(&helper).unwrap(),
        serde_json::to_string(&fixture.root).unwrap(),
        serde_json::to_string(&fixture.root).unwrap()).unwrap();
    drop(config);
    fixture.start();
    let (alpha_receiver, beta_receiver, alpha_context, beta_context) = register_pair(&mut fixture);
    let (alpha_target, beta_target) = pair_targets(
        &fixture,
        &worker_id(&alpha_context),
        &worker_id(&beta_context),
    );
    let beta_address = qualified_address(&beta_target);
    let envelope = json!({
        "source_channel": "qqbot", "channel_instance": "qqbot@test-alpha",
        "external_user": "fixture-user", "external_conversation": "fixture-conversation",
        "platform_message_id": "fixture-inbound-event", "reply_route": "qqbot://c2c/original-fixture-user",
        "recipient_address": beta_address, "attachments": [],
    }).to_string();
    let body = "跨机器入站中文\n第二行正文";
    let inbound = fixture.alpha.run_ok(
        &[
            "sendmessage",
            "--unknown-sender",
            "--to",
            &beta_address,
            "--subject",
            "external channel",
            "--content",
            body,
            "--channel-envelope",
            &envelope,
        ],
        None,
    );
    assert_eq!(inbound["outcome"], "accepted", "{inbound}");
    let reference = inbound["message_ref"].as_str().unwrap().to_owned();
    let received = recv_messages(
        &fixture.beta,
        &beta_receiver.agent_session_id,
        "channel-receive",
    );
    assert_eq!(received["count"], 1, "{received}");
    assert_eq!(received["messages"][0]["body"], body);
    assert_eq!(
        received["messages"][0]["federation"]["sender"]["kind"],
        "external_channel"
    );
    let reply_address = received["messages"][0]["from"].as_str().unwrap();
    let reply_body = "远端 agent 关联回复\n仍回原 QQ 会话";
    let operation = format!("channel-reply-{}", fixture.beta.unique_token());
    let reply = send_remote(
        &fixture.beta,
        &beta_receiver.agent_session_id,
        reply_address,
        "external reply",
        reply_body,
        &operation,
        Some(&reference),
    );
    assert_eq!(reply["outcome"], "accepted", "{reply}");
    assert!(
        capture.is_file(),
        "the original owner accepted the reply but did not invoke its QQ helper: {reply}"
    );
    let calls: Vec<Value> = fs::read_to_string(&capture)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["scope"], "c2c");
    assert_eq!(calls[0]["target_id"], "original-fixture-user");
    assert_eq!(
        calls[0]["content"],
        format!("[{beta_address}]\n{reply_body}")
    );
    assert_eq!(reply["receipt"]["channel_delivery"]["status"], "accepted");
    assert_eq!(
        reply["receipt"]["channel_delivery"]["platform_message_id"],
        "fixture-qq-message"
    );
    assert_eq!(
        reply["consumed"], false,
        "platform acceptance cannot consume a mailbox message"
    );
    let repeated = send_remote(
        &fixture.beta,
        &beta_receiver.agent_session_id,
        reply_address,
        "external reply",
        reply_body,
        &operation,
        Some(&reference),
    );
    assert_eq!(repeated["reference"], reply["reference"]);
    assert_eq!(fs::read_to_string(&capture).unwrap().lines().count(), 1);
    let duplicate = fixture.alpha.run_ok(
        &[
            "sendmessage",
            "--unknown-sender",
            "--to",
            &beta_address,
            "--subject",
            "external channel",
            "--content",
            body,
            "--channel-envelope",
            &envelope,
        ],
        None,
    );
    assert_eq!(duplicate["reference"], inbound["reference"]);
    assert_eq!(
        recv_messages(
            &fixture.beta,
            &beta_receiver.agent_session_id,
            "channel-empty"
        )["count"],
        0
    );
    let mut local_envelope: Value = serde_json::from_str(&envelope).unwrap();
    let alpha_address = local_address(&alpha_target);
    local_envelope["platform_message_id"] = json!("fixture-local-event");
    local_envelope["recipient_address"] = json!(alpha_address);
    fixture.alpha.run_ok(
        &[
            "sendmessage",
            "--unknown-sender",
            "--to",
            &alpha_address,
            "--subject",
            "local external channel",
            "--content",
            "local request",
            "--channel-envelope",
            &local_envelope.to_string(),
        ],
        None,
    );
    let local_received = recv_messages(
        &fixture.alpha,
        &alpha_receiver.agent_session_id,
        "local-channel-recv",
    );
    assert_eq!(local_received["count"], 1, "{local_received}");
    let local_original = &local_received["messages"][0];
    let local_reply_body = "本机 agent 原路回复";
    let local_reply = fixture.alpha.run_ok(
        &[
            "sendmessage",
            "--to",
            local_original["channel"]["reply_route"].as_str().unwrap(),
            "--in-reply-to",
            local_original["id"].as_str().unwrap(),
            "--subject",
            "local reply",
            "--content",
            local_reply_body,
        ],
        Some(&alpha_receiver.agent_session_id),
    );
    assert_eq!(
        local_reply["channel_delivery"]["status"], "accepted",
        "{local_reply}"
    );
    let calls: Vec<Value> = fs::read_to_string(&capture)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1]["target_id"], "original-fixture-user");
    assert_eq!(
        calls[1]["content"],
        format!("[{}]\n{local_reply_body}", qualified_address(&alpha_target))
    );
    let local_status = fixture.alpha.run_ok(
        &["msg", local_reply["msg_id"].as_str().unwrap()],
        Some(&alpha_receiver.agent_session_id),
    );
    assert_eq!(
        local_status["channel_delivery"]["status"], "accepted",
        "{local_status}"
    );
    fixture.alpha.evidence(
        "external_channel_reply",
        json!({"inbound": inbound, "reply": reply, "repeated": repeated, "calls": calls}),
    );
    assert!(fixture.finish().is_empty());
}

#[test]
#[ignore = "requires explicit candidate, Rex server and authorized loopback SSH identity"]
fn federation_accepted_response_preserves_cleanup_error() {
    let mut fixture = FederationPublicFixture::new("accepted-cleanup", false);
    fixture.configure();
    fixture.start();
    let (alpha_receiver, beta_receiver, alpha_context, beta_context) = register_pair(&mut fixture);
    let (alpha_target, beta_target) = pair_targets(
        &fixture,
        &worker_id(&alpha_context),
        &worker_id(&beta_context),
    );
    let holder = OwnedSshStderrHolder::start(&fixture);
    let started = Instant::now();
    let sent = send_remote(
        &fixture.alpha,
        &alpha_receiver.agent_session_id,
        &qualified_address(&beta_target),
        "accepted before cleanup deadline",
        "已接受的正文\n清理错误独立保留",
        "accepted-cleanup-operation",
        None,
    );
    let elapsed = started.elapsed();
    let holder_pid = fs::read_to_string(&holder.pid_file).unwrap();
    drop(holder);
    let cleanup_deadline = Instant::now() + Duration::from_secs(2);
    while Command::new("/bin/kill")
        .args(["-0", holder_pid.trim()])
        .output()
        .unwrap()
        .status
        .success()
    {
        assert!(
            Instant::now() < cleanup_deadline,
            "owned stderr holder remains"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    fixture.alpha.evidence(
        "accepted_cleanup_result",
        json!({"response": sent, "elapsed_ms": elapsed.as_millis()}),
    );
    assert_eq!(sent["outcome"], "accepted", "{sent}");
    assert_eq!(sent["accepted"], true, "{sent}");
    assert!(sent["transport_error"].is_string(), "{sent}");
    assert!(elapsed < Duration::from_secs(17), "{elapsed:?}");
    let received = recv_messages(
        &fixture.beta,
        &beta_receiver.agent_session_id,
        "accepted-cleanup-recv",
    );
    assert_eq!(received["count"], 1, "{received}");
    assert_eq!(
        received["messages"][0]["body"],
        "已接受的正文\n清理错误独立保留",
    );
    let retry = send_remote(
        &fixture.alpha,
        &alpha_receiver.agent_session_id,
        &qualified_address(&beta_target),
        "accepted before cleanup deadline",
        "已接受的正文\n清理错误独立保留",
        "accepted-cleanup-operation",
        None,
    );
    assert_eq!(retry["reference"], sent["reference"], "{retry}");
    let empty = recv_messages(
        &fixture.beta,
        &beta_receiver.agent_session_id,
        "accepted-cleanup-empty",
    );
    assert_eq!(empty["count"], 0, "{empty}");
    assert_eq!(alpha_target["machine"], "alpha");
    assert!(fixture.finish().is_empty());
}

#[test]
#[ignore = "requires explicit candidate, Rex server and authorized loopback SSH identity"]
fn federation_lookup_then_put_shares_one_deadline() {
    let mut fixture = FederationPublicFixture::new("lookup-budget", false);
    fixture.proxy = Some(FederationFaultProxy::start_with_mode(
        fixture.root.join("delayed-proxy.sock"),
        fixture.beta.host_state.join("federation/rpc.sock"),
        FederationFaultMode::DelayOperations(Duration::from_secs(10)),
    ));
    fixture.configure();
    fixture.start();
    let (alpha_receiver, beta_receiver, alpha_context, beta_context) = register_pair(&mut fixture);
    let (_, beta_target) = pair_targets(
        &fixture,
        &worker_id(&alpha_context),
        &worker_id(&beta_context),
    );
    let original_credential = fixture.alpha_to_beta_credential.clone();
    let temporarily_missing = fixture.root.join("temporarily-unavailable-credential");
    fs::rename(&original_credential, &temporarily_missing).unwrap();
    let first = send_remote(
        &fixture.alpha,
        &alpha_receiver.agent_session_id,
        &qualified_address(&beta_target),
        "lookup and put budget",
        "共享期限的正文",
        "lookup-put-budget-operation",
        None,
    );
    fs::rename(&temporarily_missing, &original_credential).unwrap();
    assert_eq!(first["outcome"], "known_not_delivered", "{first}");
    let started = Instant::now();
    let recovered = send_remote(
        &fixture.alpha,
        &alpha_receiver.agent_session_id,
        &qualified_address(&beta_target),
        "lookup and put budget",
        "共享期限的正文",
        "lookup-put-budget-operation",
        None,
    );
    let elapsed = started.elapsed();
    fixture.alpha.evidence(
        "lookup_put_deadline",
        json!({"first": first, "recovered": recovered, "elapsed_ms": elapsed.as_millis()}),
    );
    assert_eq!(recovered["outcome"], "unknown", "{recovered}");
    assert!(elapsed < Duration::from_secs(17), "{elapsed:?}");
    let received = recv_messages(
        &fixture.beta,
        &beta_receiver.agent_session_id,
        "lookup-put-budget-recv",
    );
    assert_eq!(received["count"], 1, "{received}");
    assert_eq!(received["messages"][0]["body"], "共享期限的正文");
    let empty = recv_messages(
        &fixture.beta,
        &beta_receiver.agent_session_id,
        "lookup-put-budget-empty",
    );
    assert_eq!(empty["count"], 0, "{empty}");
    assert!(fixture.finish().is_empty());
}
