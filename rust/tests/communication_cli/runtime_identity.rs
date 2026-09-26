use super::*;

#[test]
fn communication_runtime_identity_registration_is_required_bound_and_replayed() {
    let root = temp_root("runtime-identity");
    let project_root = root.canonicalize().unwrap().to_str().unwrap().to_owned();
    let missing_runtime = call_error(
        &root,
        json!({
            "op": "register_scope",
            "scope": {
                "scopeId": "scope",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://scope",
                "projectRoot": project_root,
                "sessionIds": ["master"]
            }
        }),
    );
    assert!(missing_runtime.contains("runtime_registration_required"));

    let runtime = call(
        &root,
        json!({
            "op": "register_runtime",
            "runtime": {
                "runtimeId": "runtime-a",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://scope",
                "projectRoot": project_root,
                "processId": std::process::id()
            }
        }),
    );
    assert_eq!(runtime["receipt"]["idempotent"], false);
    assert!(runtime["receipt"]["fingerprint"]
        .as_str()
        .unwrap()
        .starts_with("runtime-"));

    let conflict = call_error(
        &root,
        json!({
            "op": "register_runtime",
            "runtime": {
                "runtimeId": "runtime-a",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://forged",
                "projectRoot": project_root,
                "processId": std::process::id()
            }
        }),
    );
    assert!(conflict.contains("runtime_identity_conflict"), "{conflict}");

    let scope_mismatch = call_error(
        &root,
        json!({
            "op": "register_scope",
            "scope": {
                "scopeId": "scope",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://forged",
                "projectRoot": project_root,
                "sessionIds": ["master"],
                "runtimeId": "runtime-a"
            }
        }),
    );
    assert!(
        scope_mismatch.contains("runtime_scope_mismatch"),
        "{scope_mismatch}"
    );

    call(
        &root,
        json!({
            "op": "register_scope",
            "scope": {
                "scopeId": "scope",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://scope",
                "projectRoot": project_root,
                "sessionIds": ["master"],
                "runtimeId": "runtime-a"
            }
        }),
    );
    let agent_mismatch = call_error(
        &root,
        json!({
            "op": "register_agent",
            "agent": {
                "scopeId": "scope",
                "sessionId": "master",
                "agentId": "master",
                "role": "master",
                "masterGrant": "user approved master",
                "runtimeId": "runtime-forged"
            }
        }),
    );
    assert!(
        agent_mismatch.contains("runtime_agent_mismatch"),
        "{agent_mismatch}"
    );
    call(
        &root,
        json!({
            "op": "register_agent",
            "agent": {
                "scopeId": "scope",
                "sessionId": "master",
                "agentId": "master",
                "role": "master",
                "masterGrant": "user approved master",
                "runtimeId": "runtime-a"
            }
        }),
    );

    let replayed = call(&root, json!({ "op": "status" }));
    assert_eq!(replayed["scopes"][0]["runtimeId"], "runtime-a");
    assert_eq!(replayed["agents"][0]["runtimeId"], "runtime-a");
    let host_registry = root.join(".appsdk-host/runtimes.jsonl");
    assert_eq!(
        fs::read_to_string(host_registry).unwrap().lines().count(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn communication_runtime_identity_delivery_receipts_are_monotonic() {
    let root = temp_root("delivery-receipt");
    let project_root = root.canonicalize().unwrap().to_str().unwrap().to_owned();
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let sent = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master" },
                "to": { "scopeId": "scope", "sessionId": "worker" },
                "title": "execute task",
                "priority": "p1",
                "body": "run the assigned verification"
            }
        }),
    );
    let message_id = sent["message"]["messageId"].as_str().unwrap();
    let attempt_id = sent["deliveryAttempt"]["attemptId"].as_str().unwrap();
    let nonce = sent["deliveryAttempt"]["nonce"].as_str().unwrap();
    let forged = call_error(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-forged",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "forged" }
            }
        }),
    );
    assert!(forged.contains("delivery_runtime_mismatch"), "{forged}");

    let empty_evidence = call_error(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": {}
            }
        }),
    );
    assert!(
        empty_evidence.contains("delivery_evidence_required"),
        "{empty_evidence}"
    );

    let delivered = call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "target-received" }
            }
        }),
    );
    assert_eq!(delivered["message"]["state"], "delivered");
    call(
        &root,
        json!({
            "op": "register_runtime",
            "runtime": {
                "runtimeId": "runtime-scope",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://scope",
                "projectRoot": project_root,
                "processId": std::process::id() + 1
            }
        }),
    );
    let duplicate = call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "target-received" }
            }
        }),
    );
    assert_eq!(duplicate["idempotent"], true);
    call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "executed",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "target-executed" }
            }
        }),
    );
    let unknown_after_delivery = call_error(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "unknown",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "delivery-observation-lost" }
            }
        }),
    );
    assert!(
        unknown_after_delivery.contains("delivery_state_regression"),
        "{unknown_after_delivery}"
    );
    let regression = call_error(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "too-early" }
            }
        }),
    );
    assert!(
        regression.contains("delivery_state_regression"),
        "{regression}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn communication_record_delivery_rejects_empty_shell_receipt_before_state_change() {
    let root = temp_root("delivery-receipt-shape");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let sent = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master" },
                "to": { "scopeId": "scope", "sessionId": "worker" },
                "title": "shape receipt",
                "priority": "p1",
                "body": "delivery evidence must match adapter contract"
            }
        }),
    );
    let message_id = sent["message"]["messageId"].as_str().unwrap();
    let attempt_id = sent["deliveryAttempt"]["attemptId"].as_str().unwrap();
    let nonce = sent["deliveryAttempt"]["nonce"].as_str().unwrap();
    let rejected = call_error(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "receiptId": "fake" }
            }
        }),
    );
    assert!(rejected.contains("delivery_evidence_invalid"), "{rejected}");
    let status = call(&root, json!({ "op": "status" }));
    let message = status["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["messageId"] == message_id)
        .unwrap();
    assert_eq!(message["state"], "accepted");

    let delivered = call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "accepted" }
            }
        }),
    );
    assert_eq!(delivered["message"]["state"], "delivered");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn communication_record_delivery_uses_adapter_kind_receipt_contract() {
    let root = temp_root("delivery-receipt-adapter-kinds");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let rejected = call_error(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "legacy-tmux",
                "kind": "tmux",
                "target": "legacy-pane",
                "execute": false,
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert!(rejected.contains("invalid_adapter_kind"), "{rejected}");
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "desktop-bound",
                "kind": "appserver",
                "target": "mock://scope",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );

    let appserver_intent = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "worker" },
                "to": { "scopeId": "scope", "sessionId": "master" },
                "title": "appserver receipt",
                "priority": "p1",
                "body": "preview",
                "deliveryMode": "direct",
                "adapterId": "desktop-bound",
                "messageId": "appserver-receipt-intent"
            }
        }),
    );
    let intent_message_id = appserver_intent["message"]["messageId"].as_str().unwrap();
    let intent_attempt_id = appserver_intent["deliveryAttempt"]["attemptId"]
        .as_str()
        .unwrap();
    let intent_nonce = appserver_intent["deliveryAttempt"]["nonce"]
        .as_str()
        .unwrap();
    let intent_only = call_error(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": intent_message_id,
                "attemptId": intent_attempt_id,
                "nonce": intent_nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": {
                    "hostMustExecute": true,
                    "capability": "send_message_to_thread",
                    "runtimeId": "runtime-scope"
                }
            }
        }),
    );
    assert!(
        intent_only.contains("independent host execution evidence"),
        "{intent_only}"
    );
    let intent_delivered = call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": intent_message_id,
                "attemptId": intent_attempt_id,
                "nonce": intent_nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": {
                    "hostMustExecute": true,
                    "hostExecuted": true,
                    "capability": "send_message_to_thread",
                    "runtimeId": "runtime-scope"
                }
            }
        }),
    );
    assert_eq!(intent_delivered["message"]["state"], "delivered");

    let appserver = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "worker" },
                "to": { "scopeId": "scope", "sessionId": "master" },
                "title": "appserver receipt",
                "priority": "p1",
                "body": "host must execute",
                "deliveryMode": "direct",
                "adapterId": "desktop-bound",
                "messageId": "appserver-receipt"
            }
        }),
    );
    let appserver_message_id = appserver["message"]["messageId"].as_str().unwrap();
    let appserver_attempt_id = appserver["deliveryAttempt"]["attemptId"].as_str().unwrap();
    let appserver_nonce = appserver["deliveryAttempt"]["nonce"].as_str().unwrap();
    let intent_only = call_error(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": appserver_message_id,
                "attemptId": appserver_attempt_id,
                "nonce": appserver_nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": {
                    "hostMustExecute": true,
                    "capability": "send_message_to_thread",
                    "runtimeId": "runtime-scope"
                }
            }
        }),
    );
    assert!(
        intent_only.contains("independent host execution evidence"),
        "{intent_only}"
    );

    let appserver_delivered = call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": appserver_message_id,
                "attemptId": appserver_attempt_id,
                "nonce": appserver_nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": {
                    "hostMustExecute": true,
                    "hostExecuted": true,
                    "capability": "send_message_to_thread",
                    "runtimeId": "runtime-scope"
                }
            }
        }),
    );
    assert_eq!(appserver_delivered["message"]["state"], "delivered");

    let executed_intent_only = call_error(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": appserver_message_id,
                "attemptId": appserver_attempt_id,
                "nonce": appserver_nonce,
                "state": "executed",
                "runtimeId": "runtime-scope",
                "evidence": {
                    "hostMustExecute": true,
                    "capability": "send_message_to_thread",
                    "runtimeId": "runtime-scope"
                }
            }
        }),
    );
    assert!(
        executed_intent_only.contains("independent host execution evidence"),
        "{executed_intent_only}"
    );

    let executed = call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": appserver_message_id,
                "attemptId": appserver_attempt_id,
                "nonce": appserver_nonce,
                "state": "executed",
                "runtimeId": "runtime-scope",
                "evidence": {
                    "hostMustExecute": true,
                    "hostExecuted": true,
                    "capability": "send_message_to_thread",
                    "runtimeId": "runtime-scope"
                }
            }
        }),
    );
    assert_eq!(executed["message"]["state"], "executed");

    let read_intent_only = call_error(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": appserver_message_id,
                "attemptId": appserver_attempt_id,
                "nonce": appserver_nonce,
                "state": "read",
                "runtimeId": "runtime-scope",
                "evidence": {
                    "hostMustExecute": true,
                    "capability": "send_message_to_thread",
                    "runtimeId": "runtime-scope"
                }
            }
        }),
    );
    assert!(
        read_intent_only.contains("independent host execution evidence"),
        "{read_intent_only}"
    );

    let read = call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": appserver_message_id,
                "attemptId": appserver_attempt_id,
                "nonce": appserver_nonce,
                "state": "read",
                "runtimeId": "runtime-scope",
                "evidence": {
                    "hostMustExecute": true,
                    "hostExecuted": true,
                    "capability": "send_message_to_thread",
                    "runtimeId": "runtime-scope"
                }
            }
        }),
    );
    assert_eq!(read["message"]["state"], "read");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn communication_record_delivery_appserver_replied_and_consumed_require_host_execution() {
    let root = temp_root("appserver-replied-consumed-host-execution");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "desktop-bound",
                "kind": "appserver",
                "target": "mock://scope",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );

    for (message_id, state) in [
        ("appserver-replied-direct", "replied"),
        ("appserver-consumed-direct", "consumed"),
    ] {
        let sent = call(
            &root,
            json!({
                "op": "send",
                "message": {
                    "from": { "scopeId": "scope", "sessionId": "worker" },
                    "to": { "scopeId": "scope", "sessionId": "master" },
                    "title": "appserver terminal receipt",
                    "priority": "p1",
                    "body": "host execution is required before terminal success",
                    "deliveryMode": "direct",
                    "adapterId": "desktop-bound",
                    "messageId": message_id
                }
            }),
        );
        let sent_message_id = sent["message"]["messageId"].as_str().unwrap();
        let attempt_id = sent["deliveryAttempt"]["attemptId"].as_str().unwrap();
        let nonce = sent["deliveryAttempt"]["nonce"].as_str().unwrap();
        let intent_only = call_error(
            &root,
            json!({
                "op": "record_delivery",
                "delivery": {
                    "messageId": sent_message_id,
                    "attemptId": attempt_id,
                    "nonce": nonce,
                    "state": state,
                    "runtimeId": "runtime-scope",
                    "evidence": {
                        "hostMustExecute": true,
                        "capability": "send_message_to_thread",
                        "runtimeId": "runtime-scope"
                    }
                }
            }),
        );
        assert!(
            intent_only.contains("independent host execution evidence"),
            "{intent_only}"
        );
        let status = call(&root, json!({ "op": "status" }));
        let message = status["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["messageId"] == sent_message_id)
            .unwrap();
        assert_eq!(message["state"], "accepted");

        let executed = call(
            &root,
            json!({
                "op": "record_delivery",
                "delivery": {
                    "messageId": sent_message_id,
                    "attemptId": attempt_id,
                    "nonce": nonce,
                    "state": state,
                    "runtimeId": "runtime-scope",
                    "evidence": {
                        "hostMustExecute": true,
                        "hostExecuted": true,
                        "capability": "send_message_to_thread",
                        "runtimeId": "runtime-scope"
                    }
                }
            }),
        );
        assert_eq!(executed["message"]["state"], state);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn communication_record_delivery_appserver_unknown_allows_missing_host_execution() {
    let root = temp_root("appserver-unknown-without-host-execution");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "desktop-bound",
                "kind": "appserver",
                "target": "mock://scope",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    let sent = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "worker" },
                "to": { "scopeId": "scope", "sessionId": "master" },
                "title": "appserver unknown",
                "priority": "p1",
                "body": "an unobserved host result remains unknown",
                "deliveryMode": "direct",
                "adapterId": "desktop-bound",
                "messageId": "appserver-unknown"
            }
        }),
    );
    let message_id = sent["message"]["messageId"].as_str().unwrap();
    let attempt_id = sent["deliveryAttempt"]["attemptId"].as_str().unwrap();
    let nonce = sent["deliveryAttempt"]["nonce"].as_str().unwrap();
    let unknown = call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "unknown",
                "runtimeId": "runtime-scope",
                "evidence": {
                    "hostMustExecute": true,
                    "capability": "send_message_to_thread",
                    "runtimeId": "runtime-scope"
                }
            }
        }),
    );
    assert_eq!(unknown["message"]["state"], "unknown");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn communication_replay_rejects_record_delivery_receipt_shape() {
    let root = temp_root("delivery-replay-receipt-shape");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let sent = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master" },
                "to": { "scopeId": "scope", "sessionId": "worker" },
                "title": "replay shape",
                "priority": "p1",
                "body": "replay must share the adapter receipt validator"
            }
        }),
    );
    let message_id = sent["message"]["messageId"].as_str().unwrap();
    let attempt_id = sent["deliveryAttempt"]["attemptId"].as_str().unwrap();
    let nonce = sent["deliveryAttempt"]["nonce"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "accepted" }
            }
        }),
    );

    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let mut tampered = Vec::new();
    for line in contents.lines() {
        let mut event: Value = serde_json::from_str(line).unwrap();
        if event["kind"] == "message.state"
            && event["data"]["messageId"] == message_id
            && event["data"]["state"] == "delivered"
        {
            event["data"]["evidence"]["details"]["receipt"] = json!({ "receiptId": "fake" });
        }
        tampered.push(serde_json::to_string(&event).unwrap());
    }
    fs::write(&mailbox, format!("{}\n", tampered.join("\n"))).unwrap();
    let replay_error = call_error(&root, json!({ "op": "status" }));
    assert!(replay_error.contains("journal_corrupt"), "{replay_error}");
    assert!(
        replay_error.contains("mailbox delivery evidence"),
        "{replay_error}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn communication_replay_rejects_appserver_intent_without_host_execution() {
    let root = temp_root("appserver-replay-intent");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "desktop-bound",
                "kind": "appserver",
                "target": "mock://scope",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    let sent = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "worker" },
                "to": { "scopeId": "scope", "sessionId": "master" },
                "title": "appserver replay",
                "priority": "p1",
                "body": "replay must reject intent as execution evidence",
                "deliveryMode": "direct",
                "adapterId": "desktop-bound",
                "messageId": "appserver-replay-intent"
            }
        }),
    );
    let message_id = sent["message"]["messageId"].as_str().unwrap();
    let attempt_id = sent["deliveryAttempt"]["attemptId"].as_str().unwrap();
    let nonce = sent["deliveryAttempt"]["nonce"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": {
                    "hostMustExecute": true,
                    "hostExecuted": true,
                    "capability": "send_message_to_thread",
                    "runtimeId": "runtime-scope"
                }
            }
        }),
    );

    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let mut tampered = Vec::new();
    for line in contents.lines() {
        let mut event: Value = serde_json::from_str(line).unwrap();
        if event["kind"] == "message.state"
            && event["data"]["messageId"] == message_id
            && event["data"]["state"] == "delivered"
        {
            event["data"]["evidence"]["details"]["receipt"] = json!({
                "hostMustExecute": true,
                "capability": "send_message_to_thread",
                "runtimeId": "runtime-scope"
            });
        }
        tampered.push(serde_json::to_string(&event).unwrap());
    }
    fs::write(&mailbox, format!("{}\n", tampered.join("\n"))).unwrap();
    let replay_error = call_error(&root, json!({ "op": "status" }));
    assert!(replay_error.contains("journal_corrupt"), "{replay_error}");
    assert!(
        replay_error.contains("independent host execution evidence"),
        "{replay_error}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn communication_replay_rejects_appserver_replied_and_consumed_without_host_execution() {
    let root = temp_root("appserver-replay-replied-consumed");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "desktop-bound",
                "kind": "appserver",
                "target": "mock://scope",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );

    for (message_id, state) in [
        ("appserver-replay-replied", "replied"),
        ("appserver-replay-consumed", "consumed"),
    ] {
        let sent = call(
            &root,
            json!({
                "op": "send",
                "message": {
                    "from": { "scopeId": "scope", "sessionId": "worker" },
                    "to": { "scopeId": "scope", "sessionId": "master" },
                    "title": "appserver replay terminal receipt",
                    "priority": "p1",
                    "body": "replay must reject intent-only terminal success",
                    "deliveryMode": "direct",
                    "adapterId": "desktop-bound",
                    "messageId": message_id
                }
            }),
        );
        let sent_message_id = sent["message"]["messageId"].as_str().unwrap();
        let attempt_id = sent["deliveryAttempt"]["attemptId"].as_str().unwrap();
        let nonce = sent["deliveryAttempt"]["nonce"].as_str().unwrap();
        let terminal = call(
            &root,
            json!({
                "op": "record_delivery",
                "delivery": {
                    "messageId": sent_message_id,
                    "attemptId": attempt_id,
                    "nonce": nonce,
                    "state": state,
                    "runtimeId": "runtime-scope",
                    "evidence": {
                        "hostMustExecute": true,
                        "hostExecuted": true,
                        "capability": "send_message_to_thread",
                        "runtimeId": "runtime-scope"
                    }
                }
            }),
        );
        assert_eq!(terminal["message"]["state"], state);
    }

    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let mut tampered = Vec::new();
    for line in contents.lines() {
        let mut event: Value = serde_json::from_str(line).unwrap();
        if event["kind"] == "message.state"
            && (event["data"]["messageId"] == "appserver-replay-replied"
                || event["data"]["messageId"] == "appserver-replay-consumed")
        {
            event["data"]["evidence"]["details"]["receipt"] = json!({
                "hostMustExecute": true,
                "capability": "send_message_to_thread",
                "runtimeId": "runtime-scope"
            });
        }
        tampered.push(serde_json::to_string(&event).unwrap());
    }
    fs::write(&mailbox, format!("{}\n", tampered.join("\n"))).unwrap();
    let replay_error = call_error(&root, json!({ "op": "status" }));
    assert!(replay_error.contains("journal_corrupt"), "{replay_error}");
    assert!(
        replay_error.contains("independent host execution evidence"),
        "{replay_error}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn communication_runtime_identity_replay_rejects_forged_delivery_receipt() {
    let root = temp_root("delivery-replay-forged");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let sent = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master" },
                "to": { "scopeId": "scope", "sessionId": "worker" },
                "title": "replay receipt",
                "priority": "p1",
                "body": "verify receipt replay"
            }
        }),
    );
    let message_id = sent["message"]["messageId"].as_str().unwrap();
    let attempt_id = sent["deliveryAttempt"]["attemptId"].as_str().unwrap();
    let nonce = sent["deliveryAttempt"]["nonce"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "worker-received" }
            }
        }),
    );

    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let mut tampered = Vec::new();
    for line in contents.lines() {
        let mut event: Value = serde_json::from_str(line).unwrap();
        if event["kind"] == "message.state"
            && event["data"]["messageId"] == message_id
            && event["data"]["state"] == "delivered"
        {
            event["data"]["evidence"]["details"]["runtimeId"] = json!("runtime-forged");
        }
        tampered.push(serde_json::to_string(&event).unwrap());
    }
    fs::write(&mailbox, format!("{}\n", tampered.join("\n"))).unwrap();
    let replay_error = call_error(&root, json!({ "op": "status" }));
    assert!(replay_error.contains("journal_corrupt"), "{replay_error}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn message_delivery_receipt_requires_persisted_attempt_and_exact_identity() {
    let root = temp_root("delivery-attempt-contract");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let sent = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master" },
                "to": { "scopeId": "scope", "sessionId": "worker" },
                "title": "attempt contract",
                "priority": "p1",
                "body": "receipt must bind to a persisted attempt",
                "messageId": "attempt-contract"
            }
        }),
    );
    let message_id = sent["message"]["messageId"].as_str().unwrap();
    let attempt_id = sent["deliveryAttempt"]["attemptId"].as_str().unwrap();
    let nonce = sent["deliveryAttempt"]["nonce"].as_str().unwrap();

    remove_mailbox_events(&root, "message.delivery_attempt");
    let missing = call_error(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "missing-attempt" }
            }
        }),
    );
    assert!(missing.contains("delivery_attempt_required"), "{missing}");

    let restored = call(
        &root,
        json!({ "op": "send", "message": {
            "from": { "scopeId": "scope", "sessionId": "master" },
            "to": { "scopeId": "scope", "sessionId": "worker" },
            "title": "attempt contract",
            "priority": "p1",
            "body": "receipt must bind to a persisted attempt",
            "messageId": "attempt-contract"
        }}),
    );
    let restored_attempt_id = restored["deliveryAttempt"]["attemptId"].as_str().unwrap();
    let restored_nonce = restored["deliveryAttempt"]["nonce"].as_str().unwrap();
    assert_ne!(restored_attempt_id, attempt_id);
    assert_ne!(restored_nonce, nonce);

    let wrong_attempt = call_error(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": "attempt-forged",
                "nonce": restored_nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "wrong-attempt" }
            }
        }),
    );
    assert!(
        wrong_attempt.contains("delivery_attempt_mismatch"),
        "{wrong_attempt}"
    );

    let wrong_nonce = call_error(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": restored_attempt_id,
                "nonce": "nonce-forged",
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "wrong-nonce" }
            }
        }),
    );
    assert!(
        wrong_nonce.contains("delivery_attempt_nonce_mismatch"),
        "{wrong_nonce}"
    );

    let delivered = call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": restored_attempt_id,
                "nonce": restored_nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "valid" }
            }
        }),
    );
    assert_eq!(delivered["message"]["state"], "delivered");
    assert_eq!(
        delivered["message"]["evidence"][1]["details"]["attemptId"],
        restored_attempt_id
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_delivery_receipt_replays_and_retry_binds_a_new_attempt() {
    let root = temp_root("legacy-delivery-replay");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let sent = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master" },
                "to": { "scopeId": "scope", "sessionId": "worker" },
                "title": "legacy receipt",
                "priority": "p1",
                "body": "replay a receipt written before persisted attempts",
                "messageId": "legacy-receipt"
            }
        }),
    );
    let message_id = sent["message"]["messageId"].as_str().unwrap();
    assert_eq!(sent["message"]["deliveryAttemptRequired"], true);
    let attempt_id = sent["deliveryAttempt"]["attemptId"].as_str().unwrap();
    let nonce = sent["deliveryAttempt"]["nonce"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "legacy-compatible" }
            }
        }),
    );

    // Model a mailbox written by the pre-attempt binary: the message marker,
    // persisted attempt event and new receipt identity fields did not exist.
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let mut rewritten = Vec::new();
    for line in contents.lines() {
        let mut event: Value = serde_json::from_str(line).unwrap();
        if event["kind"] == "message.delivery_attempt" && event["data"]["messageId"] == message_id {
            continue;
        }
        if event["kind"] == "message.created" && event["data"]["messageId"] == message_id {
            event["data"]
                .as_object_mut()
                .unwrap()
                .remove("deliveryAttemptRequired");
        }
        if event["kind"] == "message.state"
            && event["data"]["messageId"] == message_id
            && event["data"]["state"] == "delivered"
        {
            let data = event["data"].as_object_mut().unwrap();
            data.remove("attemptId");
            data.remove("nonce");
            data.get_mut("evidence")
                .and_then(Value::as_object_mut)
                .and_then(|evidence| evidence.get_mut("details"))
                .and_then(Value::as_object_mut)
                .map(|details| {
                    details.remove("attemptId");
                    details.remove("nonce");
                    details.remove("adapterId");
                    details.remove("target");
                });
        }
        rewritten.push(serde_json::to_string(&event).unwrap());
    }
    fs::write(&mailbox, format!("{}\n", rewritten.join("\n"))).unwrap();

    let replayed = call(&root, json!({ "op": "status" }));
    let replayed_message = replayed["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["messageId"] == message_id)
        .unwrap();
    assert_eq!(replayed_message["state"], "delivered");
    assert!(replayed["messageDeliveryAttempts"]
        .as_array()
        .unwrap()
        .is_empty());

    // Retrying the legacy message establishes a fresh persisted attempt. The
    // new receipt path remains strict even though the message itself is legacy.
    let retried = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master" },
                "to": { "scopeId": "scope", "sessionId": "worker" },
                "title": "legacy receipt",
                "priority": "p1",
                "body": "replay a receipt written before persisted attempts",
                "messageId": message_id
            }
        }),
    );
    let retry_attempt_id = retried["deliveryAttempt"]["attemptId"].as_str().unwrap();
    let retry_nonce = retried["deliveryAttempt"]["nonce"].as_str().unwrap();
    assert_ne!(retry_attempt_id, attempt_id);
    assert_ne!(retry_nonce, nonce);
    let progressed = call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": retry_attempt_id,
                "nonce": retry_nonce,
                "state": "executed",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "legacy-retry-executed" }
            }
        }),
    );
    assert_eq!(progressed["message"]["state"], "executed");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn new_delivery_receipt_missing_attempt_identity_is_rejected_on_replay() {
    let root = temp_root("new-delivery-replay-missing-attempt");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let sent = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master" },
                "to": { "scopeId": "scope", "sessionId": "worker" },
                "title": "strict receipt",
                "priority": "p1",
                "body": "new receipts require attempt identity",
                "messageId": "strict-receipt"
            }
        }),
    );
    let message_id = sent["message"]["messageId"].as_str().unwrap();
    let attempt_id = sent["deliveryAttempt"]["attemptId"].as_str().unwrap();
    let nonce = sent["deliveryAttempt"]["nonce"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "strict-receipt" }
            }
        }),
    );

    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let mut rewritten = Vec::new();
    for line in contents.lines() {
        let mut event: Value = serde_json::from_str(line).unwrap();
        if event["kind"] == "message.state"
            && event["data"]["messageId"] == message_id
            && event["data"]["state"] == "delivered"
        {
            let data = event["data"].as_object_mut().unwrap();
            data.remove("attemptId");
            data.remove("nonce");
            data.get_mut("evidence")
                .and_then(Value::as_object_mut)
                .and_then(|evidence| evidence.get_mut("details"))
                .and_then(Value::as_object_mut)
                .map(|details| {
                    details.remove("attemptId");
                    details.remove("nonce");
                });
        }
        rewritten.push(serde_json::to_string(&event).unwrap());
    }
    fs::write(&mailbox, format!("{}\n", rewritten.join("\n"))).unwrap();
    let replay_error = call_error(&root, json!({ "op": "status" }));
    assert!(replay_error.contains("journal_corrupt"), "{replay_error}");
    assert!(
        replay_error.contains("external delivery evidence attemptId is missing"),
        "{replay_error}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tampered_message_delivery_attempt_is_rejected_on_replay() {
    let root = temp_root("delivery-attempt-replay-tamper");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let sent = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master" },
                "to": { "scopeId": "scope", "sessionId": "worker" },
                "title": "tampered attempt",
                "priority": "p1",
                "body": "replay must reject a changed nonce",
                "messageId": "tampered-attempt"
            }
        }),
    );
    let message_id = sent["message"]["messageId"].as_str().unwrap();
    let attempt_id = sent["deliveryAttempt"]["attemptId"].as_str().unwrap();
    let nonce = sent["deliveryAttempt"]["nonce"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "valid" }
            }
        }),
    );
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let mut tampered = Vec::new();
    for line in contents.lines() {
        let mut event: Value = serde_json::from_str(line).unwrap();
        if event["kind"] == "message.delivery_attempt" && event["data"]["messageId"] == message_id {
            event["data"]["attempt"]["nonce"] = json!("nonce-tampered");
        }
        tampered.push(serde_json::to_string(&event).unwrap());
    }
    fs::write(&mailbox, format!("{}\n", tampered.join("\n"))).unwrap();
    let replay_error = call_error(&root, json!({ "op": "status" }));
    assert!(replay_error.contains("journal_corrupt"), "{replay_error}");
    fs::remove_dir_all(root).unwrap();
}
