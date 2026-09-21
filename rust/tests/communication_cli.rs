use chrono::{DateTime, Duration, SecondsFormat};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_appsdk"))
}

fn temp_root(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("appsdk-communication-{name}-{nonce}"));
    fs::create_dir_all(&root).unwrap();
    root
}

fn call(root: &Path, request: Value) -> Value {
    call_with_host(root, &root.join(".appsdk-host"), request)
}

fn call_with_host(root: &Path, host: &Path, request: Value) -> Value {
    let output = Command::new(binary())
        .args([
            "communication",
            root.to_str().unwrap(),
            "--json",
            &serde_json::to_string(&request).unwrap(),
        ])
        .env("APPSDK_HOME", host)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "request failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn call_error(root: &Path, request: Value) -> String {
    call_error_with_host(root, &root.join(".appsdk-host"), request)
}

fn call_error_with_host(root: &Path, host: &Path, request: Value) -> String {
    let output = Command::new(binary())
        .args([
            "communication",
            root.to_str().unwrap(),
            "--json",
            &serde_json::to_string(&request).unwrap(),
        ])
        .env("APPSDK_HOME", host)
        .output()
        .unwrap();
    assert!(!output.status.success());
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn delivery_attempt_fields(root: &Path, message_id: &str) -> (String, String) {
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(mailbox).unwrap();
    contents
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find_map(|event| {
            (event["kind"] == "message.delivery_attempt"
                && event["data"]["messageId"] == message_id)
                .then(|| {
                    (
                        event["data"]["attempt"]["attemptId"]
                            .as_str()
                            .unwrap()
                            .to_owned(),
                        event["data"]["attempt"]["nonce"]
                            .as_str()
                            .unwrap()
                            .to_owned(),
                    )
                })
        })
        .unwrap()
}

fn count_events(events: &[Value], kind: &str) -> usize {
    events.iter().filter(|event| event["kind"] == kind).count()
}

fn retain_mailbox_through(root: &Path, event_kind: &str) {
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    let marker = format!("\"kind\":\"{event_kind}\"");
    let end = lines
        .iter()
        .position(|line| line.contains(&marker))
        .unwrap();
    let retained = lines[..=end].join("\n");
    fs::write(mailbox, format!("{retained}\n")).unwrap();
}

fn retain_mailbox_through_last(root: &Path, event_kind: &str) {
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    let marker = format!("\"kind\":\"{event_kind}\"");
    let end = lines
        .iter()
        .rposition(|line| line.contains(&marker))
        .unwrap();
    let retained = lines[..=end].join("\n");
    fs::write(mailbox, format!("{retained}\n")).unwrap();
}

fn retain_mailbox_through_occurrence(root: &Path, event_kind: &str, occurrence: usize) {
    assert!(occurrence > 0);
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    let marker = format!("\"kind\":\"{event_kind}\"");
    let end = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains(&marker))
        .nth(occurrence - 1)
        .map(|(index, _)| index)
        .unwrap();
    let retained = lines[..=end].join("\n");
    fs::write(mailbox, format!("{retained}\n")).unwrap();
}

fn remove_mailbox_events(root: &Path, event_kind: &str) {
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let marker = format!("\"kind\":\"{event_kind}\"");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let retained: Vec<&str> = contents
        .lines()
        .filter(|line| !line.contains(&marker))
        .collect();
    fs::write(mailbox, format!("{}\n", retained.join("\n"))).unwrap();
}

fn register_scope(
    root: &Path,
    scope_id: &str,
    appserver_id: &str,
    _project_root: &str,
    sessions: &[&str],
) {
    register_scope_with_host(
        root,
        &root.join(".appsdk-host"),
        scope_id,
        appserver_id,
        sessions,
    );
}

fn register_scope_with_host(
    root: &Path,
    host: &Path,
    scope_id: &str,
    appserver_id: &str,
    sessions: &[&str],
) {
    let runtime_id = format!("runtime-{scope_id}");
    let bound_project_root = root.canonicalize().unwrap();
    let bound_project_root = bound_project_root.to_str().unwrap();
    call_with_host(
        root,
        host,
        json!({
            "op": "register_runtime",
            "runtime": {
                "runtimeId": runtime_id,
                "appserverId": appserver_id,
                "namespace": "codex_tui",
                "endpoint": format!("mock://{scope_id}"),
                "projectRoot": bound_project_root,
                "capabilities": ["send_message_to_thread"],
                "processId": std::process::id()
            }
        }),
    );
    call_with_host(
        root,
        host,
        json!({
            "op": "register_scope",
            "scope": {
                "scopeId": scope_id,
                "appserverId": appserver_id,
                "namespace": "codex_tui",
                "endpoint": format!("mock://{scope_id}"),
                "projectRoot": bound_project_root,
                "sessionIds": sessions,
                "runtimeId": format!("runtime-{scope_id}")
            }
        }),
    );
}

fn register_agent(
    root: &Path,
    scope_id: &str,
    session_id: &str,
    agent_id: &str,
    role: &str,
    parent: Option<Value>,
) {
    register_agent_with_host(
        root,
        &root.join(".appsdk-host"),
        scope_id,
        session_id,
        agent_id,
        role,
        parent,
    );
}

fn register_agent_with_host(
    root: &Path,
    host: &Path,
    scope_id: &str,
    session_id: &str,
    agent_id: &str,
    role: &str,
    parent: Option<Value>,
) {
    let mut agent = json!({
        "scopeId": scope_id,
        "sessionId": session_id,
        "agentId": agent_id,
        "role": role,
        "runtimeId": format!("runtime-{scope_id}")
    });
    if role == "master" {
        agent["masterGrant"] = json!("user approved master for this scope");
    }
    if let Some(parent) = parent {
        agent["parent"] = parent;
    }
    call_with_host(
        root,
        host,
        json!({ "op": "register_agent", "agent": agent }),
    );
}

fn register_agent_with_lease(
    root: &Path,
    scope_id: &str,
    session_id: &str,
    agent_id: &str,
    role: &str,
    lease_ms: u64,
) -> Value {
    let mut agent = json!({
        "scopeId": scope_id,
        "sessionId": session_id,
        "agentId": agent_id,
        "role": role,
        "leaseMs": lease_ms,
        "runtimeId": format!("runtime-{scope_id}")
    });
    if role == "master" {
        agent["masterGrant"] = json!("user approved master for this scope");
    }
    call(root, json!({ "op": "register_agent", "agent": agent }))
}

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

fn after(timestamp: &str, seconds: i64) -> String {
    (DateTime::parse_from_rfc3339(timestamp).unwrap() + Duration::seconds(seconds))
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[test]
fn routing_requires_explicit_master_and_respects_scope_boundaries() {
    let root = temp_root("routing");
    register_scope(
        &root,
        "scope-a",
        "app-a",
        "/project",
        &["master-a", "peer-a", "peer-b", "child-a"],
    );
    register_scope(
        &root,
        "scope-b",
        "app-b",
        "/project",
        &["master-b", "peer-c"],
    );
    register_agent(&root, "scope-a", "master-a", "master-a", "master", None);
    register_agent(&root, "scope-a", "peer-a", "peer-a", "peer", None);
    register_agent(&root, "scope-a", "peer-b", "peer-b", "peer", None);
    register_agent(
        &root,
        "scope-a",
        "child-a",
        "child-a",
        "subagent",
        Some(json!({ "scopeId": "scope-a", "sessionId": "peer-a" })),
    );
    register_agent(&root, "scope-b", "master-b", "master-b", "master", None);
    register_agent(&root, "scope-b", "peer-c", "peer-c", "peer", None);

    let local_peer = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "peer-a" },
                "to": { "scopeId": "scope-a", "sessionId": "peer-b" },
                "title": "local peer",
                "priority": "p2",
                "body": "same appserver and project"
            }
        }),
    );
    assert_eq!(local_peer["route"]["mode"], "same-scope-peer");
    assert_eq!(local_peer["message"]["state"], "accepted");

    let bound_child = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "peer-a" },
                "to": { "scopeId": "scope-a", "sessionId": "child-a" },
                "title": "bound child",
                "priority": "p2",
                "body": "parent scoped"
            }
        }),
    );
    assert_eq!(bound_child["route"]["mode"], "same-scope-parent");

    let cross_master = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "master-a" },
                "to": { "scopeId": "scope-b", "sessionId": "master-b" },
                "title": "cross master",
                "priority": "p1",
                "body": "cross appserver same project"
            }
        }),
    );
    assert_eq!(cross_master["route"]["mode"], "cross-scope-master");

    let peer_cross = call_error(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "peer-a" },
                "to": { "scopeId": "scope-b", "sessionId": "master-b" },
                "title": "forbidden",
                "priority": "p2",
                "body": "peer cannot cross scope"
            }
        }),
    );
    assert!(
        peer_cross.contains("cross_scope_master_required"),
        "{peer_cross}"
    );

    let unbound_child = call_error(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "peer-b" },
                "to": { "scopeId": "scope-a", "sessionId": "child-a" },
                "title": "forbidden child",
                "priority": "p2",
                "body": "wrong parent"
            }
        }),
    );
    assert!(
        unbound_child.contains("subagent_parent_required"),
        "{unbound_child}"
    );

    let auto_role = call_error(
        &root,
        json!({
            "op": "register_agent",
            "agent": {
                "scopeId": "scope-a",
                "sessionId": "auto",
                "agentId": "auto",
                "role": "auto"
            }
        }),
    );
    assert!(auto_role.contains("role_auto_forbidden"), "{auto_role}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cross_project_master_messages_use_shared_host_discovery_and_replay_target_mailbox() {
    let project_a = temp_root("cross-project-a");
    let project_b = temp_root("cross-project-b");
    let host = temp_root("cross-project-host");

    register_scope_with_host(&project_a, &host, "scope-a", "app-a", &["master-a"]);
    register_scope_with_host(&project_b, &host, "scope-b", "app-b", &["master-b"]);
    register_agent_with_host(
        &project_a, &host, "scope-a", "master-a", "master-a", "master", None,
    );
    register_agent_with_host(
        &project_b, &host, "scope-b", "master-b", "master-b", "master", None,
    );

    let a_to_b = call_with_host(
        &project_a,
        &host,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "master-a" },
                "to": { "scopeId": "scope-b", "sessionId": "master-b" },
                "title": "cross project request",
                "priority": "p1",
                "body": "A asks B to coordinate"
            }
        }),
    );
    assert_eq!(a_to_b["route"]["mode"], "cross-scope-master");
    assert_eq!(a_to_b["route"]["sameAppserver"], false);
    assert_eq!(a_to_b["route"]["sameProject"], false);
    assert_eq!(a_to_b["message"]["state"], "accepted");

    let b_to_a = call_with_host(
        &project_b,
        &host,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-b", "sessionId": "master-b" },
                "to": { "scopeId": "scope-a", "sessionId": "master-a" },
                "title": "cross project response",
                "priority": "p1",
                "body": "B confirms coordination"
            }
        }),
    );
    assert_eq!(b_to_a["route"]["mode"], "cross-scope-master");

    let status_a = call_with_host(&project_a, &host, json!({ "op": "status" }));
    assert_eq!(status_a["messages"].as_array().unwrap().len(), 1);
    assert_eq!(status_a["messages"][0]["to"]["sessionId"], "master-b");
    let status_b = call_with_host(&project_b, &host, json!({ "op": "status" }));
    assert_eq!(status_b["messages"].as_array().unwrap().len(), 1);
    assert_eq!(status_b["messages"][0]["to"]["sessionId"], "master-a");

    let registry = fs::read_to_string(host.join("communication.jsonl")).unwrap();
    assert_eq!(registry.lines().count(), 4);
    assert!(registry.contains("\"scopeId\":\"scope-a\""));
    assert!(registry.contains("\"scopeId\":\"scope-b\""));

    fs::remove_dir_all(project_a).unwrap();
    fs::remove_dir_all(project_b).unwrap();
    fs::remove_dir_all(host).unwrap();
}

#[test]
fn cross_project_peer_and_unknown_target_remain_fail_closed_after_discovery() {
    let project_a = temp_root("cross-project-peer-a");
    let project_b = temp_root("cross-project-peer-b");
    let host = temp_root("cross-project-peer-host");

    register_scope_with_host(
        &project_a,
        &host,
        "scope-a",
        "app-a",
        &["master-a", "peer-a"],
    );
    register_scope_with_host(
        &project_b,
        &host,
        "scope-b",
        "app-b",
        &["master-b", "peer-b"],
    );
    register_agent_with_host(
        &project_a, &host, "scope-a", "master-a", "master-a", "master", None,
    );
    register_agent_with_host(
        &project_a, &host, "scope-a", "peer-a", "peer-a", "peer", None,
    );
    register_agent_with_host(
        &project_b, &host, "scope-b", "master-b", "master-b", "master", None,
    );
    register_agent_with_host(
        &project_b, &host, "scope-b", "peer-b", "peer-b", "peer", None,
    );

    let peer_cross = call_error_with_host(
        &project_a,
        &host,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "peer-a" },
                "to": { "scopeId": "scope-b", "sessionId": "master-b" },
                "title": "forbidden cross project peer",
                "priority": "p2",
                "body": "peer cannot cross scope"
            }
        }),
    );
    assert!(
        peer_cross.contains("cross_scope_master_required"),
        "{peer_cross}"
    );

    let unknown = call_error_with_host(
        &project_a,
        &host,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "master-a" },
                "to": { "scopeId": "scope-z", "sessionId": "master-z" },
                "title": "unknown target",
                "priority": "p2",
                "body": "must fail closed"
            }
        }),
    );
    assert!(unknown.contains("agent_not_registered"), "{unknown}");

    let status_a = call_with_host(&project_a, &host, json!({ "op": "status" }));
    assert!(status_a["messages"].as_array().unwrap().is_empty());
    fs::remove_dir_all(project_a).unwrap();
    fs::remove_dir_all(project_b).unwrap();
    fs::remove_dir_all(host).unwrap();
}

#[test]
fn missing_host_discovery_index_is_unavailable_not_unknown_agent() {
    let project_a = temp_root("missing-host-index-a");
    let host = temp_root("missing-host-index-host");

    register_scope_with_host(&project_a, &host, "scope-a", "app-a", &["master-a"]);
    register_agent_with_host(
        &project_a, &host, "scope-a", "master-a", "master-a", "master", None,
    );

    fs::remove_file(host.join("communication.jsonl")).unwrap();

    let missing = call_error_with_host(
        &project_a,
        &host,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "master-a" },
                "to": { "scopeId": "scope-z", "sessionId": "master-z" },
                "title": "target registry missing",
                "priority": "p2",
                "body": "must not be classified as unknown agent"
            }
        }),
    );
    assert!(
        missing.contains("GLOBAL_COMMUNICATION_REGISTRY_UNAVAILABLE"),
        "{missing}"
    );
    assert!(!missing.contains("agent_not_registered"), "{missing}");

    register_scope_with_host(&project_a, &host, "scope-a", "app-a", &["master-a"]);
    register_agent_with_host(
        &project_a, &host, "scope-a", "master-a", "master-a", "master", None,
    );

    let unknown = call_error_with_host(
        &project_a,
        &host,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope-a", "sessionId": "master-a" },
                "to": { "scopeId": "scope-z", "sessionId": "master-z" },
                "title": "unknown target",
                "priority": "p2",
                "body": "valid unknown agent must stay fail closed"
            }
        }),
    );
    assert!(unknown.contains("agent_not_registered"), "{unknown}");
    assert!(
        !unknown.contains("GLOBAL_COMMUNICATION_REGISTRY_UNAVAILABLE"),
        "{unknown}"
    );

    fs::remove_dir_all(project_a).unwrap();
    fs::remove_dir_all(host).unwrap();
}

#[test]
fn discovery_registration_failure_replays_from_local_pending_intent() {
    let root = temp_root("discovery-registration-recovery");
    let host = root.join(".appsdk-host");
    let project_root = root.canonicalize().unwrap();
    let project_root = project_root.to_str().unwrap();
    call_with_host(
        &root,
        &host,
        json!({
            "op": "register_runtime",
            "runtime": {
                "runtimeId": "runtime-recovery",
                "appserverId": "app-recovery",
                "namespace": "codex_tui",
                "endpoint": "mock://recovery",
                "projectRoot": project_root,
                "capabilities": ["send_message_to_thread"],
                "processId": std::process::id()
            }
        }),
    );
    let registry_file = host.join("communication.jsonl");
    fs::create_dir_all(&registry_file).unwrap();
    let error = call_error_with_host(
        &root,
        &host,
        json!({
            "op": "register_scope",
            "scope": {
                "scopeId": "scope-recovery",
                "appserverId": "app-recovery",
                "namespace": "codex_tui",
                "endpoint": "mock://recovery",
                "projectRoot": project_root,
                "sessionIds": ["master"],
                "runtimeId": "runtime-recovery"
            }
        }),
    );
    assert!(
        error.contains("communication_discovery_registration_failed"),
        "{error}"
    );
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let raw = fs::read_to_string(&mailbox).unwrap();
    assert!(raw.contains("\"kind\":\"discovery.pending\""));
    assert!(raw.contains("\"kind\":\"scope.registered\""));

    fs::remove_dir_all(&registry_file).unwrap();
    let status = call_with_host(&root, &host, json!({ "op": "status" }));
    assert_eq!(status["scopes"].as_array().unwrap().len(), 1);
    assert_eq!(status["discoveryPending"].as_array().unwrap().len(), 0);
    assert!(host.join("communication.jsonl").is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rebind_failure_replays_from_local_pending_intent() {
    let root = temp_root("discovery-rebind-recovery");
    let host = root.join(".appsdk-host");
    register_scope_with_host(
        &root,
        &host,
        "scope-rebind-recovery",
        "app-rebind-recovery",
        &["master-old", "master-new"],
    );
    register_agent_with_host(
        &root,
        &host,
        "scope-rebind-recovery",
        "master-old",
        "master",
        "master",
        None,
    );

    let registry_file = host.join("communication.jsonl");
    let registry_backup = host.join("communication.jsonl.backup");
    fs::rename(&registry_file, &registry_backup).unwrap();
    fs::create_dir_all(&registry_file).unwrap();
    let error = call_error_with_host(
        &root,
        &host,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope-rebind-recovery", "sessionId": "master-old" },
                "to": { "scopeId": "scope-rebind-recovery", "sessionId": "master-new" },
                "runtimeId": "runtime-scope-rebind-recovery"
            }
        }),
    );
    assert!(
        error.contains("communication_discovery_registration_failed"),
        "{error}"
    );
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let raw = fs::read_to_string(&mailbox).unwrap();
    assert!(raw.contains("\"kind\":\"discovery.pending\""));
    assert!(raw.contains("\"kind\":\"agent.rebound\""));

    fs::remove_dir_all(&registry_file).unwrap();
    fs::rename(&registry_backup, &registry_file).unwrap();
    let status = call_with_host(&root, &host, json!({ "op": "status" }));
    assert_eq!(status["agents"].as_array().unwrap().len(), 1);
    assert_eq!(status["agents"][0]["sessionId"], "master-new");
    assert_eq!(status["discoveryPending"].as_array().unwrap().len(), 0);
    let registry = fs::read_to_string(&registry_file).unwrap();
    assert!(registry.contains("\"event\":\"communication.agent.rebound\""));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rebind_recovery_rejects_projection_key_collision_before_journal_commit() {
    let root = temp_root("discovery-rebind-recovery-collision");
    let host = root.join(".appsdk-host");
    register_scope_with_host(
        &root,
        &host,
        "scope-rebind-recovery-collision",
        "app-rebind-recovery-collision",
        &["master-old", "master-new"],
    );
    register_agent_with_host(
        &root,
        &host,
        "scope-rebind-recovery-collision",
        "master-old",
        "master",
        "master",
        None,
    );
    call_with_host(
        &root,
        &host,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope-rebind-recovery-collision", "sessionId": "master-old" },
            "state": "idle",
            "at": "2026-01-01T00:00:00Z"
        }),
    );

    let registry_file = host.join("communication.jsonl");
    let registry_backup = host.join("communication.jsonl.backup");
    fs::rename(&registry_file, &registry_backup).unwrap();
    fs::create_dir_all(&registry_file).unwrap();
    let error = call_error_with_host(
        &root,
        &host,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope-rebind-recovery-collision", "sessionId": "master-old" },
                "to": { "scopeId": "scope-rebind-recovery-collision", "sessionId": "master-new" },
                "runtimeId": "runtime-scope-rebind-recovery-collision"
            }
        }),
    );
    assert!(
        error.contains("communication_discovery_registration_failed"),
        "{error}"
    );

    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let mut lines: Vec<Value> = contents
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let pending_index = lines
        .iter()
        .rposition(|event| {
            event["kind"] == "discovery.pending"
                && event["data"]["operation"]["operation"] == "rebind"
        })
        .unwrap();
    lines.truncate(pending_index + 1);
    let orphan = json!({
        "protocol": "appsdk-comm/v1",
        "eventId": "event-orphan-recovery-collision",
        "at": "2026-01-01T00:00:00Z",
        "kind": "wakeup.updated",
        "data": {
            "address": { "scopeId": "scope-rebind-recovery-collision", "sessionId": "master-new" },
            "idleSince": null,
            "remindersSent": 0,
            "nextDueAt": null,
            "stopped": false,
            "lastReminderAt": null
        }
    });
    lines.push(orphan.clone());
    let rewritten = lines
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&mailbox, format!("{rewritten}\n")).unwrap();

    fs::remove_dir_all(&registry_file).unwrap();
    fs::rename(&registry_backup, &registry_file).unwrap();
    let error = call_error_with_host(&root, &host, json!({ "op": "status" }));
    assert!(
        error.contains("communication_discovery_recovery_failed")
            && error.contains("duplicate wakeup record"),
        "{error}"
    );
    let after = fs::read_to_string(&mailbox).unwrap();
    assert_eq!(after.matches("\"kind\":\"agent.rebound\"").count(), 0);
    assert!(after.contains("\"kind\":\"discovery.pending\""));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn idle_notifications_are_idempotent_and_batched_after_two_minutes() {
    let root = temp_root("notifications");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let at = "2026-01-01T00:00:00Z";
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": at
        }),
    );
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:00:10Z"
        }),
    );

    let early = call(
        &root,
        json!({ "op": "flush_notifications", "now": "2026-01-01T00:01:59Z" }),
    );
    assert_eq!(early["batches"].as_array().unwrap().len(), 0);
    let batch = call(
        &root,
        json!({ "op": "flush_notifications", "now": "2026-01-01T00:02:00Z" }),
    );
    assert_eq!(batch["batches"].as_array().unwrap().len(), 0);
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": "2026-01-01T00:02:00Z"
        }),
    );
    let wake = call(
        &root,
        json!({ "op": "tick", "now": "2026-01-01T00:02:00Z" }),
    );
    assert_eq!(wake["masterWakeChanged"].as_array().unwrap().len(), 1);
    let again = call(
        &root,
        json!({ "op": "flush_notifications", "now": "2026-01-01T00:04:00Z" }),
    );
    assert_eq!(again["batches"].as_array().unwrap().len(), 0);

    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(
        status["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        status["notificationProjection"]["emitted"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(status["mailboxPath"]
        .as_str()
        .unwrap()
        .ends_with("mailbox.jsonl"));
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("notification.queued").count(), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn idle_window_reopens_after_emitted_without_dropping_update() {
    let root = temp_root("idle-window-reopen");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);

    for (message_id, title, created_at) in [
        ("idle-window-first", "first update", "2026-01-01T00:00:00Z"),
        (
            "idle-window-second",
            "second update",
            "2026-01-01T00:02:01Z",
        ),
    ] {
        call(
            &root,
            json!({
                "op": "send",
                "message": {
                    "from": { "scopeId": "scope", "sessionId": "worker" },
                    "to": { "scopeId": "scope", "sessionId": "master" },
                    "title": title,
                    "priority": "p2",
                    "body": title,
                    "deliveryMode": "idle",
                    "coalesceKey": "project-progress",
                    "messageId": message_id,
                    "createdAt": created_at
                }
            }),
        );
        if message_id == "idle-window-first" {
            let first = call(
                &root,
                json!({
                    "op": "flush_notifications",
                    "now": "2026-01-01T00:02:00Z"
                }),
            );
            assert_eq!(first["batches"].as_array().unwrap().len(), 1);
            assert_eq!(
                first["batches"][0]["items"][0]["messageId"],
                "idle-window-first"
            );
            assert_eq!(first["batches"][0]["items"][0]["generation"], 0);
        }
    }

    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(
        status["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        status["notificationProjection"]["pending"][0]["messageId"],
        "idle-window-second"
    );
    assert_eq!(
        status["notificationProjection"]["pending"][0]["generation"],
        1
    );

    let second = call(
        &root,
        json!({
            "op": "flush_notifications",
            "now": "2026-01-01T00:04:01Z"
        }),
    );
    assert_eq!(second["batches"].as_array().unwrap().len(), 1);
    assert_eq!(
        second["batches"][0]["items"][0]["messageId"],
        "idle-window-second"
    );
    assert_eq!(second["batches"][0]["items"][0]["generation"], 1);

    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 2);
    assert_eq!(
        raw.matches("\"kind\":\"notification.batch_emitted\"")
            .count(),
        2
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn idle_window_retry_of_older_message_keeps_latest_generation() {
    let root = temp_root("idle-window-old-message-retry");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);

    let message_a = json!({
        "from": { "scopeId": "scope", "sessionId": "worker" },
        "to": { "scopeId": "scope", "sessionId": "master" },
        "title": "generation A",
        "priority": "p2",
        "body": "first window",
        "deliveryMode": "idle",
        "coalesceKey": "same-window",
        "messageId": "generation-a",
        "createdAt": "2026-01-01T00:00:00Z"
    });
    call(&root, json!({ "op": "send", "message": message_a.clone() }));
    let first = call(
        &root,
        json!({ "op": "flush_notifications", "now": "2026-01-01T00:02:00Z" }),
    );
    assert_eq!(first["batches"].as_array().unwrap().len(), 1);
    assert_eq!(first["batches"][0]["items"][0]["generation"], 0);

    // Keep the message timestamp equal to A to exercise the tie case.  A
    // fresh send after A is terminal must still open generation one.
    let message_b = json!({
        "from": { "scopeId": "scope", "sessionId": "worker" },
        "to": { "scopeId": "scope", "sessionId": "master" },
        "title": "generation B",
        "priority": "p2",
        "body": "second window",
        "deliveryMode": "idle",
        "coalesceKey": "same-window",
        "messageId": "generation-b",
        "createdAt": "2026-01-01T00:00:00Z"
    });
    call(&root, json!({ "op": "send", "message": message_b.clone() }));
    let pending = call(&root, json!({ "op": "status" }));
    assert_eq!(
        pending["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        pending["notificationProjection"]["pending"][0]["messageId"],
        "generation-b"
    );
    assert_eq!(
        pending["notificationProjection"]["pending"][0]["generation"],
        1
    );

    // Retrying old A while B is pending must return the current projection
    // and append no third queued fact.
    let retry_pending = call(&root, json!({ "op": "send", "message": message_a.clone() }));
    assert_eq!(retry_pending["idempotent"], true);
    let after_pending_retry = call(&root, json!({ "op": "status" }));
    assert_eq!(
        after_pending_retry["notificationProjection"]["pending"][0]["messageId"],
        "generation-b"
    );
    assert_eq!(
        after_pending_retry["notificationProjection"]["pending"][0]["generation"],
        1
    );

    let second = call(
        &root,
        json!({ "op": "flush_notifications", "now": "2026-01-01T00:04:00Z" }),
    );
    assert_eq!(second["batches"].as_array().unwrap().len(), 1);
    assert_eq!(
        second["batches"][0]["items"][0]["messageId"],
        "generation-b"
    );
    assert_eq!(second["batches"][0]["items"][0]["generation"], 1);

    // Retrying old A after B is terminal is still an idempotent read of the
    // latest generation; it must not reopen generation two or batch A again.
    let retry_terminal = call(&root, json!({ "op": "send", "message": message_a }));
    assert_eq!(retry_terminal["idempotent"], true);
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(
        status["notificationProjection"]["emitted"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        status["notificationProjection"]["emitted"][0]["messageId"],
        "generation-b"
    );
    assert_eq!(
        status["notificationProjection"]["emitted"][0]["generation"],
        1
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 2);
    assert_eq!(
        raw.matches("\"kind\":\"notification.batch_emitted\"")
            .count(),
        2
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn same_timestamp_new_idle_message_recovers_after_notification_queue_prefix() {
    let root = temp_root("idle-window-same-time-prefix");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);

    let message_a = json!({
        "from": { "scopeId": "scope", "sessionId": "worker" },
        "to": { "scopeId": "scope", "sessionId": "master" },
        "title": "prefix A",
        "priority": "p2",
        "body": "first window",
        "deliveryMode": "idle",
        "coalesceKey": "same-time-prefix",
        "messageId": "prefix-a",
        "createdAt": "2026-01-01T00:00:00Z"
    });
    call(&root, json!({ "op": "send", "message": message_a }));
    call(
        &root,
        json!({ "op": "flush_notifications", "now": "2026-01-01T00:02:00Z" }),
    );

    let message_b = json!({
        "from": { "scopeId": "scope", "sessionId": "worker" },
        "to": { "scopeId": "scope", "sessionId": "master" },
        "title": "prefix B",
        "priority": "p2",
        "body": "second window",
        "deliveryMode": "idle",
        "coalesceKey": "same-time-prefix",
        "messageId": "prefix-b",
        "createdAt": "2026-01-01T00:00:00Z"
    });
    call(&root, json!({ "op": "send", "message": message_b.clone() }));
    // Keep B's message.created fact but drop its accepted state, attempt and
    // notification queue.  The retry must recognize B as newer by event
    // order even though A and B have identical createdAt values.
    retain_mailbox_through_occurrence(&root, "message.created", 2);
    let recovered = call(&root, json!({ "op": "send", "message": message_b }));
    assert_eq!(recovered["idempotent"], true);
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(
        status["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        status["notificationProjection"]["pending"][0]["messageId"],
        "prefix-b"
    );
    assert_eq!(
        status["notificationProjection"]["pending"][0]["generation"],
        1
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"message.created\"").count(), 2);
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_coalesced_message_fact_fails_closed_before_new_prefix_recovery() {
    let root = temp_root("idle-window-missing-message-fact");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);

    let message_a = json!({
        "from": { "scopeId": "scope", "sessionId": "master" },
        "to": { "scopeId": "scope", "sessionId": "worker" },
        "title": "fact A",
        "priority": "p2",
        "body": "first terminal window",
        "deliveryMode": "idle",
        "coalesceKey": "missing-fact",
        "messageId": "fact-a",
        "createdAt": "2026-01-01T00:00:00Z"
    });
    call(&root, json!({ "op": "send", "message": message_a }));
    call(
        &root,
        json!({
            "op": "flush_notifications",
            "now": "2026-01-01T00:02:00Z"
        }),
    );

    let message_b = json!({
        "from": { "scopeId": "scope", "sessionId": "master" },
        "to": { "scopeId": "scope", "sessionId": "worker" },
        "title": "fact B",
        "priority": "p2",
        "body": "second terminal window",
        "deliveryMode": "idle",
        "coalesceKey": "missing-fact",
        "messageId": "fact-b",
        "createdAt": "2026-01-01T00:00:00Z"
    });
    call(&root, json!({ "op": "send", "message": message_b }));
    call(
        &root,
        json!({
            "op": "flush_notifications",
            "now": "2026-01-01T00:04:00Z"
        }),
    );

    let message_c = json!({
        "from": { "scopeId": "scope", "sessionId": "master" },
        "to": { "scopeId": "scope", "sessionId": "worker" },
        "title": "fact C",
        "priority": "p2",
        "body": "new prefix after B",
        "deliveryMode": "idle",
        "coalesceKey": "missing-fact",
        "messageId": "fact-c",
        "createdAt": "2026-01-01T00:03:00Z"
    });
    call(&root, json!({ "op": "send", "message": message_c.clone() }));

    // Reproduce a malformed crash prefix: B's coalesced notification facts
    // survived, but its message.created fact did not; C's message facts
    // survived while its notification queue prefix did not.  Replay must not
    // use B's generation or timestamp to swallow C.
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let retained: Vec<&str> = contents
        .lines()
        .filter(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            let kind = event["kind"].as_str().unwrap();
            let data = &event["data"];
            match kind {
                "message.created" | "message.state" | "message.delivery_attempt" => {
                    let message_id = data["messageId"]
                        .as_str()
                        .or_else(|| data["message"]["messageId"].as_str())
                        .or_else(|| data["attempt"]["messageId"].as_str());
                    message_id != Some("fact-b")
                }
                "notification.queued" => {
                    data["notification"]["messageId"].as_str() != Some("fact-c")
                }
                _ => true,
            }
        })
        .collect();
    let malformed_prefix = format!("{}\n", retained.join("\n"));
    fs::write(&mailbox, &malformed_prefix).unwrap();

    let error = call_error(&root, json!({ "op": "send", "message": message_c }));
    assert!(error.contains("journal_corrupt"), "{error}");
    assert!(error.contains("fact-b"), "{error}");
    assert!(error.contains("durable creation fact"), "{error}");
    let after = fs::read_to_string(&mailbox).unwrap();
    assert!(after.starts_with(&malformed_prefix));
    assert_eq!(
        after[malformed_prefix.len()..]
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .count(),
        1
    );
    let recorded_error: Value =
        serde_json::from_str(after[malformed_prefix.len()..].trim()).unwrap();
    assert_eq!(recorded_error["kind"], "error.recorded");
    assert_eq!(recorded_error["data"]["code"], "journal_corrupt");
    assert_eq!(
        after.matches("\"kind\":\"message.created\"").count(),
        malformed_prefix
            .matches("\"kind\":\"message.created\"")
            .count()
    );
    assert_eq!(
        after.matches("\"kind\":\"notification.queued\"").count(),
        malformed_prefix
            .matches("\"kind\":\"notification.queued\"")
            .count()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wakeup_is_state_driven_and_stops_after_three_reminders() {
    let root = temp_root("wakeup");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": "2026-01-01T00:00:00Z"
        }),
    );
    for (now, expected) in [
        ("2026-01-01T00:01:59Z", 0),
        ("2026-01-01T00:02:00Z", 1),
        ("2026-01-01T00:04:00Z", 2),
        ("2026-01-01T00:06:00Z", 3),
        ("2026-01-01T00:08:00Z", 3),
    ] {
        let result = call(&root, json!({ "op": "tick", "now": now }));
        assert_eq!(result["wakeup"][0]["remindersSent"], expected);
    }
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["wakeup"][0]["stopped"], true);
    assert_eq!(status["wakeup"][0]["nextDueAt"], Value::Null);
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "working",
            "at": "2026-01-01T00:08:10Z"
        }),
    );
    let reset = call(&root, json!({ "op": "status" }));
    assert_eq!(reset["wakeup"][0]["remindersSent"], 0);
    assert_eq!(reset["wakeup"][0]["stopped"], false);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_idle_state_retry_recovers_current_wakeup_after_agent_prefix() {
    let root = temp_root("master-idle-prefix-recovery");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    let registration =
        register_agent_with_lease(&root, "scope", "master", "master", "master", 600_000);
    let observed_at = registration["agent"]["lastObservedAt"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": observed_at
        }),
    );
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "working",
            "at": after(observed_at, 10)
        }),
    );
    let idle_at = after(observed_at, 20);
    let idle_request = json!({
        "op": "set_agent_state",
        "address": { "scopeId": "scope", "sessionId": "master" },
        "state": "idle",
        "at": idle_at
    });
    call(&root, idle_request.clone());
    retain_mailbox_through_last(&root, "agent.state");

    let recovered = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": after(&idle_at, 1)
        }),
    );
    assert_eq!(recovered["idempotent"], true);
    assert!(recovered["notification"].is_null());
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["wakeup"][0]["idleSince"], idle_at);
    assert_eq!(status["wakeup"][0]["nextDueAt"], after(&idle_at, 120));
    assert_eq!(status["wakeup"][0]["remindersSent"], 0);

    let repeated = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": after(&idle_at, 2)
        }),
    );
    assert_eq!(repeated["idempotent"], true);
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"wakeup.updated\"").count(), 3);

    let due = call(&root, json!({ "op": "tick", "now": after(&idle_at, 120) }));
    assert_eq!(due["changed"].as_array().unwrap().len(), 1);
    assert_eq!(due["wakeup"][0]["remindersSent"], 1);
    let same_due = call(&root, json!({ "op": "tick", "now": after(&idle_at, 120) }));
    assert!(same_due["changed"].as_array().unwrap().is_empty());
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"wakeup.reminder\"").count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_working_state_retry_resets_stale_wakeup_after_agent_prefix() {
    let root = temp_root("master-working-prefix-recovery");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    let registration =
        register_agent_with_lease(&root, "scope", "master", "master", "master", 600_000);
    let observed_at = registration["agent"]["lastObservedAt"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": observed_at
        }),
    );
    for seconds in [120, 240, 360] {
        let result = call(
            &root,
            json!({ "op": "tick", "now": after(observed_at, seconds) }),
        );
        assert_eq!(result["wakeup"][0]["remindersSent"], seconds / 120);
    }
    let working_at = after(observed_at, 361);
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "working",
            "at": working_at
        }),
    );
    retain_mailbox_through_last(&root, "agent.state");

    let recovered = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "working",
            "at": after(&working_at, 1)
        }),
    );
    assert_eq!(recovered["idempotent"], true);
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["wakeup"][0]["idleSince"], Value::Null);
    assert_eq!(status["wakeup"][0]["nextDueAt"], Value::Null);
    assert_eq!(status["wakeup"][0]["remindersSent"], 0);
    assert_eq!(status["wakeup"][0]["stopped"], false);

    let repeated = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "working",
            "at": after(&working_at, 2)
        }),
    );
    assert_eq!(repeated["idempotent"], true);
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"wakeup.updated\"").count(), 2);

    let new_idle_at = after(observed_at, 362);
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": new_idle_at
        }),
    );
    let due = call(
        &root,
        json!({ "op": "tick", "now": after(&new_idle_at, 120) }),
    );
    assert_eq!(due["changed"].as_array().unwrap().len(), 1);
    assert_eq!(due["wakeup"][0]["remindersSent"], 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tick_does_not_wake_an_expired_master_or_consume_reminder_budget() {
    let root = temp_root("expired-master-wakeup");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    let registration =
        register_agent_with_lease(&root, "scope", "master", "master", "master", 1_000);
    let observed_at = registration["agent"]["lastObservedAt"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": observed_at
        }),
    );

    let result = call(
        &root,
        json!({ "op": "tick", "now": after(observed_at, 120) }),
    );
    assert!(result["changed"].as_array().unwrap().is_empty());
    assert_eq!(result["wakeup"][0]["remindersSent"], 0);

    let status = call(&root, json!({ "op": "status" }));
    assert!(status["notificationProjection"]["pending"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(status["notificationProjection"]["emitted"]
        .as_array()
        .unwrap()
        .is_empty());
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert!(!raw.contains("wakeup.reminder"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tick_wakes_a_live_idle_master_within_its_lease() {
    let root = temp_root("live-master-wakeup");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    let registration =
        register_agent_with_lease(&root, "scope", "master", "master", "master", 600_000);
    let observed_at = registration["agent"]["lastObservedAt"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": observed_at
        }),
    );

    let result = call(
        &root,
        json!({ "op": "tick", "now": after(observed_at, 120) }),
    );
    assert_eq!(result["changed"].as_array().unwrap().len(), 1);
    assert_eq!(result["wakeup"][0]["remindersSent"], 1);

    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(
        status["notificationProjection"]["emitted"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    let events: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let tick_events: Vec<&Value> = events
        .iter()
        .filter(|event| {
            matches!(
                event["kind"].as_str(),
                Some(
                    "message.created"
                        | "message.state"
                        | "notification.queued"
                        | "notification.delivery_attempt"
                        | "notification.emitted"
                        | "wakeup.reminder"
                )
            )
        })
        .collect();
    assert_eq!(
        tick_events
            .iter()
            .map(|event| event["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "message.created",
            "message.state",
            "notification.queued",
            "notification.delivery_attempt",
            "notification.emitted",
            "wakeup.reminder"
        ]
    );
    let attempt_id = tick_events[3]["data"]["attemptId"].as_str().unwrap();
    assert_eq!(
        tick_events[4]["data"]["attemptId"].as_str(),
        Some(attempt_id)
    );
    assert_eq!(
        tick_events[5]["data"]["attemptId"].as_str(),
        Some(attempt_id)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wakeup_delivery_receipt_prefix_recovers_when_message_is_already_delivered() {
    let root = temp_root("wakeup-delivery-prefix");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    let registration =
        register_agent_with_lease(&root, "scope", "master", "master", "master", 600_000);
    let observed_at = registration["agent"]["lastObservedAt"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": observed_at
        }),
    );
    let due = after(observed_at, 120);
    call(&root, json!({ "op": "tick", "now": due }));
    retain_mailbox_through(&root, "notification.emitted");
    let status = call(&root, json!({ "op": "status" }));
    let message_id = status["notificationProjection"]["emitted"][0]["messageId"]
        .as_str()
        .unwrap()
        .to_owned();
    let (attempt_id, nonce) = delivery_attempt_fields(&root, &message_id);
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
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "master-received" }
            }
        }),
    );
    let recovered = call(&root, json!({ "op": "tick", "now": due }));
    assert_eq!(recovered["changed"].as_array().unwrap().len(), 1);
    assert_eq!(recovered["wakeup"][0]["remindersSent"], 1);
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert!(raw.contains("\"kind\":\"wakeup.reminder\""));
    assert!(!raw.contains("wakeup_message_state_invalid"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tick_crash_after_attempt_is_unknown_and_does_not_replay_or_consume_budget() {
    let root = temp_root("wakeup-attempt-crash");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    let registration =
        register_agent_with_lease(&root, "scope", "master", "master", "master", 600_000);
    let observed_at = registration["agent"]["lastObservedAt"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": observed_at
        }),
    );
    let due = after(observed_at, 120);
    call(&root, json!({ "op": "tick", "now": due }));
    retain_mailbox_through(&root, "notification.delivery_attempt");

    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(
        status["notificationProjection"]["unknown"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(status["notificationProjection"]["pending"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(status["wakeup"][0]["remindersSent"], 0);

    let replay = call(&root, json!({ "op": "tick", "now": due }));
    assert!(replay["changed"].as_array().unwrap().is_empty());
    assert_eq!(replay["wakeup"][0]["remindersSent"], 0);
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        1
    );
    assert!(!raw.contains("\"kind\":\"notification.emitted\""));
    assert!(!raw.contains("\"kind\":\"wakeup.reminder\""));

    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "working",
            "at": after(observed_at, 121)
        }),
    );
    let new_idle_at = after(observed_at, 122);
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": new_idle_at
        }),
    );
    let new_cycle = call(
        &root,
        json!({ "op": "tick", "now": after(observed_at, 242) }),
    );
    assert_eq!(new_cycle["changed"].as_array().unwrap().len(), 1);
    assert_eq!(new_cycle["wakeup"][0]["remindersSent"], 1);
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        2
    );
    assert_eq!(raw.matches("\"kind\":\"wakeup.reminder\"").count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tick_retry_reuses_stable_cycle_message_and_notification_prefix() {
    let root = temp_root("wakeup-prefix-retry");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    let registration =
        register_agent_with_lease(&root, "scope", "master", "master", "master", 600_000);
    let observed_at = registration["agent"]["lastObservedAt"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": observed_at
        }),
    );
    let due = after(observed_at, 120);
    call(&root, json!({ "op": "tick", "now": due }));
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let raw = fs::read_to_string(&mailbox).unwrap();
    let events: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let first_message_id = events
        .iter()
        .find(|event| event["kind"] == "message.created")
        .and_then(|event| event["data"]["messageId"].as_str())
        .unwrap()
        .to_owned();
    let first_notification_id = events
        .iter()
        .find(|event| event["kind"] == "notification.queued")
        .and_then(|event| event["data"]["notification"]["notificationId"].as_str())
        .unwrap()
        .to_owned();
    retain_mailbox_through(&root, "notification.queued");

    let retry = call(&root, json!({ "op": "tick", "now": due }));
    assert_eq!(retry["changed"].as_array().unwrap().len(), 1);
    assert_eq!(retry["wakeup"][0]["remindersSent"], 1);
    let raw = fs::read_to_string(&mailbox).unwrap();
    assert_eq!(raw.matches("\"kind\":\"message.created\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"message.state\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 1);
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        1
    );
    assert_eq!(raw.matches("\"kind\":\"notification.emitted\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"wakeup.reminder\"").count(), 1);
    let events: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(events
        .iter()
        .filter(|event| event["kind"] == "message.created")
        .all(|event| { event["data"]["messageId"] == first_message_id }));
    assert!(events
        .iter()
        .filter(|event| event["kind"] == "notification.queued")
        .all(|event| event["data"]["notification"]["notificationId"] == first_notification_id));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tick_retry_recovers_created_message_prefix_without_duplicate_facts() {
    let root = temp_root("wakeup-created-prefix");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    let registration =
        register_agent_with_lease(&root, "scope", "master", "master", "master", 600_000);
    let observed_at = registration["agent"]["lastObservedAt"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": observed_at
        }),
    );
    let due = after(observed_at, 120);
    call(&root, json!({ "op": "tick", "now": due }));
    retain_mailbox_through(&root, "message.created");

    let retry = call(&root, json!({ "op": "tick", "now": due }));
    assert_eq!(retry["changed"].as_array().unwrap().len(), 1);
    assert_eq!(retry["wakeup"][0]["remindersSent"], 1);
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"message.created\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"message.state\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 1);
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        1
    );
    assert_eq!(raw.matches("\"kind\":\"notification.emitted\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"wakeup.reminder\"").count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tick_retry_after_emitted_prefix_reuses_attempt_and_finishes_wakeup() {
    let root = temp_root("wakeup-emitted-prefix");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    let registration =
        register_agent_with_lease(&root, "scope", "master", "master", "master", 600_000);
    let observed_at = registration["agent"]["lastObservedAt"].as_str().unwrap();
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": observed_at
        }),
    );
    let due = after(observed_at, 120);
    call(&root, json!({ "op": "tick", "now": due }));
    retain_mailbox_through(&root, "notification.emitted");

    let retry = call(&root, json!({ "op": "tick", "now": due }));
    assert_eq!(retry["changed"].as_array().unwrap().len(), 1);
    assert_eq!(retry["wakeup"][0]["remindersSent"], 1);
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"message.created\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"message.state\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 1);
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        1
    );
    assert_eq!(raw.matches("\"kind\":\"notification.emitted\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"wakeup.reminder\"").count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn active_bugs_are_projected_as_priority_sorted_loops() {
    let root = temp_root("bugs");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    let low = call(
        &root,
        json!({
            "op": "report_bug",
            "bug": {
                "bugId": "bug-low",
                "scopeId": "scope",
                "title": "minor issue",
                "priority": "p3",
                "description": "minor",
                "reporter": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert_eq!(low["bug"]["status"], "active");
    let high = call(
        &root,
        json!({
            "op": "report_bug",
            "bug": {
                "bugId": "bug-high",
                "scopeId": "scope",
                "title": "urgent issue",
                "priority": "p0",
                "description": "urgent",
                "reporter": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert_eq!(high["bug"]["loopId"], "bug-loop-bug-high");
    let status = call(&root, json!({ "op": "status" }));
    let bugs = status["activeBugs"].as_array().unwrap();
    assert_eq!(bugs[0]["bugId"], "bug-high");
    assert_eq!(status["loops"][0]["loopId"], "bug-loop-bug-high");
    assert_eq!(status["loops"][0]["trigger"], "event:bug.reported");
    assert_eq!(status["loops"][0]["phase"], "discover");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupted_mailbox_is_an_explicit_error() {
    let root = temp_root("corrupt");
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    fs::create_dir_all(mailbox.parent().unwrap()).unwrap();
    fs::write(mailbox, "not-json\n").unwrap();
    let error = call_error(&root, json!({ "op": "status" }));
    assert!(error.contains("journal_corrupt"), "{error}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn communication_store_rejects_cross_project_root_writes() {
    let root = temp_root("root-binding");
    let foreign = temp_root("foreign-project");
    let foreign_root = foreign.to_str().unwrap();
    let rejected = call_error(
        &root,
        json!({
            "op": "register_runtime",
            "runtime": {
                "runtimeId": "foreign-runtime",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://foreign",
                "projectRoot": foreign_root,
                "processId": std::process::id()
            }
        }),
    );
    assert!(rejected.contains("project_root_mismatch"), "{rejected}");
    assert!(!root.join(".appsdk-host/runtimes.jsonl").exists());

    let project_root = root.canonicalize().unwrap().to_str().unwrap().to_owned();
    call(
        &root,
        json!({
            "op": "register_runtime",
            "runtime": {
                "runtimeId": "local-runtime",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://local",
                "projectRoot": project_root,
                "processId": std::process::id()
            }
        }),
    );
    let scope_rejected = call_error(
        &root,
        json!({
            "op": "register_scope",
            "scope": {
                "scopeId": "foreign-scope",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://foreign",
                "projectRoot": foreign_root,
                "sessionIds": ["master"],
                "runtimeId": "local-runtime"
            }
        }),
    );
    assert!(
        scope_rejected.contains("project_root_mismatch"),
        "{scope_rejected}"
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert!(!raw.contains("foreign-scope"));
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(foreign).unwrap();
}

#[test]
fn communication_store_rejects_noncanonical_and_symlink_roots() {
    let root = temp_root("root-path");
    let name = root.file_name().unwrap().to_owned();
    let noncanonical = root.join("..").join(&name);
    let rejected = call_error(&noncanonical, json!({ "op": "status" }));
    assert!(
        rejected.contains("communication_root_not_canonical"),
        "{rejected}"
    );
    fs::remove_dir_all(&root).unwrap();

    let root = temp_root("root-dot-path");
    let name = root.file_name().unwrap().to_owned();
    let embedded_dot = root.parent().unwrap().join(".").join(&name);
    let rejected = call_error(&embedded_dot, json!({ "op": "status" }));
    assert!(
        rejected.contains("communication_root_not_canonical"),
        "{rejected}"
    );
    fs::remove_dir_all(&root).unwrap();

    let target = temp_root("root-symlink-target");
    let alias = target.with_file_name("appsdk-communication-root-alias");
    #[cfg(unix)]
    symlink(&target, &alias).unwrap();
    #[cfg(unix)]
    {
        let rejected = call_error(&alias, json!({ "op": "status" }));
        assert!(
            rejected.contains("communication_path_symlink"),
            "{rejected}"
        );
        fs::remove_file(&alias).unwrap();
    }
    fs::remove_dir_all(target).unwrap();
}

#[cfg(unix)]
#[test]
fn communication_store_rejects_mailbox_lock_and_parent_symlinks() {
    let parent_target = temp_root("parent-symlink-target");
    let parent_root = temp_root("parent-symlink-root");
    fs::remove_dir_all(&parent_root).unwrap();
    symlink(&parent_target, &parent_root).unwrap();
    let parent_error = call_error(&parent_root, json!({ "op": "status" }));
    assert!(
        parent_error.contains("communication_path_symlink"),
        "{parent_error}"
    );
    fs::remove_file(&parent_root).unwrap();
    fs::remove_dir_all(parent_target).unwrap();

    let mailbox_root = temp_root("mailbox-symlink-root");
    let mailbox_dir = mailbox_root.join(".appsdk-control/communication");
    fs::create_dir_all(&mailbox_dir).unwrap();
    let mailbox_target = temp_root("mailbox-symlink-target").join("mailbox.jsonl");
    fs::write(&mailbox_target, "").unwrap();
    symlink(&mailbox_target, mailbox_dir.join("mailbox.jsonl")).unwrap();
    let mailbox_error = call_error(&mailbox_root, json!({ "op": "status" }));
    assert!(
        mailbox_error.contains("communication_path_symlink"),
        "{mailbox_error}"
    );
    fs::remove_dir_all(mailbox_root).unwrap();
    fs::remove_file(&mailbox_target).unwrap();
    fs::remove_dir_all(mailbox_target.parent().unwrap()).unwrap();

    let lock_root = temp_root("lock-symlink-root");
    let lock_dir = lock_root.join(".appsdk-control/communication");
    fs::create_dir_all(&lock_dir).unwrap();
    fs::write(lock_dir.join("mailbox.jsonl"), "").unwrap();
    let lock_target = temp_root("lock-symlink-target").join("mailbox.jsonl.lock");
    fs::write(&lock_target, "").unwrap();
    symlink(&lock_target, lock_dir.join("mailbox.jsonl.lock")).unwrap();
    let lock_error = call_error(&lock_root, json!({ "op": "status" }));
    assert!(
        lock_error.contains("communication_path_symlink"),
        "{lock_error}"
    );
    fs::remove_dir_all(lock_root).unwrap();
    fs::remove_file(&lock_target).unwrap();
    fs::remove_dir_all(lock_target.parent().unwrap()).unwrap();
}

#[test]
fn communication_replay_rejects_empty_lines_and_replays_valid_events() {
    let root = temp_root("replay-empty-line");
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    fs::create_dir_all(mailbox.parent().unwrap()).unwrap();
    fs::write(&mailbox, "\n").unwrap();
    let empty_error = call_error(&root, json!({ "op": "status" }));
    assert!(empty_error.contains("journal_corrupt"), "{empty_error}");
    assert!(empty_error.contains("line 1"), "{empty_error}");
    fs::remove_dir_all(&root).unwrap();

    let root = temp_root("replay-invalid-envelope");
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    fs::create_dir_all(mailbox.parent().unwrap()).unwrap();
    fs::write(&mailbox, "{}\n").unwrap();
    let envelope_error = call_error(&root, json!({ "op": "status" }));
    assert!(
        envelope_error.contains("journal_corrupt"),
        "{envelope_error}"
    );
    assert!(envelope_error.contains("line 1"), "{envelope_error}");
    fs::remove_dir_all(&root).unwrap();

    let root = temp_root("replay-invalid-envelope-fields");
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    fs::create_dir_all(mailbox.parent().unwrap()).unwrap();
    let event = json!({
        "protocol": "appsdk-comm/v1",
        "eventId": "",
        "at": "not-a-timestamp",
        "kind": "adapter.registered",
        "data": {
            "adapterId": "replay-adapter",
            "kind": "mailbox",
            "target": null,
            "enabled": true,
            "execute": false,
            "recipient": null,
            "registeredAt": "2026-01-01T00:00:00Z"
        }
    });
    fs::write(
        &mailbox,
        format!("{}\n", serde_json::to_string(&event).unwrap()),
    )
    .unwrap();
    let fields_error = call_error(&root, json!({ "op": "status" }));
    assert!(fields_error.contains("journal_corrupt"), "{fields_error}");
    assert!(fields_error.contains("line 1"), "{fields_error}");
    fs::remove_dir_all(&root).unwrap();

    let root = temp_root("replay-valid-event");
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "replay-adapter",
                "kind": "mailbox",
                "target": "local"
            }
        }),
    );
    let replayed = call(&root, json!({ "op": "status" }));
    assert!(replayed["adapters"]
        .as_array()
        .unwrap()
        .iter()
        .any(|adapter| adapter["adapterId"] == "replay-adapter"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn communication_replay_rejects_duplicate_event_ids_missing_final_newline_and_unknown_fields() {
    let root = temp_root("replay-duplicate-event-id");
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "replay-adapter",
                "kind": "mailbox",
                "target": "local"
            }
        }),
    );
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let first = contents.lines().next().unwrap();
    fs::write(&mailbox, format!("{contents}{first}\n")).unwrap();
    let duplicate_error = call_error(&root, json!({ "op": "status" }));
    assert!(
        duplicate_error.contains("journal_corrupt"),
        "{duplicate_error}"
    );
    assert!(duplicate_error.contains("duplicate"), "{duplicate_error}");
    fs::remove_dir_all(&root).unwrap();

    let root = temp_root("replay-missing-final-newline");
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "replay-adapter",
                "kind": "mailbox",
                "target": "local"
            }
        }),
    );
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let mut contents = fs::read_to_string(&mailbox).unwrap();
    assert!(contents.ends_with('\n'));
    contents.pop();
    fs::write(&mailbox, contents).unwrap();
    let newline_error = call_error(&root, json!({ "op": "status" }));
    assert!(newline_error.contains("journal_corrupt"), "{newline_error}");
    assert!(newline_error.contains("newline"), "{newline_error}");
    fs::remove_dir_all(&root).unwrap();

    let root = temp_root("replay-unknown-envelope-field");
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    fs::create_dir_all(mailbox.parent().unwrap()).unwrap();
    let event = json!({
        "protocol": "appsdk-comm/v1",
        "eventId": "event-with-extra-field",
        "at": "2026-01-01T00:00:00Z",
        "kind": "adapter.registered",
        "data": {
            "adapterId": "replay-adapter",
            "kind": "mailbox",
            "target": "local",
            "enabled": true,
            "execute": false,
            "recipient": null,
            "registeredAt": "2026-01-01T00:00:00Z"
        },
        "unexpected": true
    });
    fs::write(
        &mailbox,
        format!("{}\n", serde_json::to_string(&event).unwrap()),
    )
    .unwrap();
    let fields_error = call_error(&root, json!({ "op": "status" }));
    assert!(fields_error.contains("journal_corrupt"), "{fields_error}");
    assert!(
        fields_error.contains("unknown") || fields_error.contains("unexpected"),
        "{fields_error}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn adapters_are_explicit_and_receipts_are_replayed() {
    let root = temp_root("adapters");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);

    let appserver = call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "desktop-host",
                "kind": "appserver",
                "target": "mock://scope",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert_eq!(appserver["adapter"]["kind"], "appserver");

    let direct = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "worker" },
                "to": { "scopeId": "scope", "sessionId": "master" },
                "title": "preview",
                "priority": "p1",
                "body": "preview only",
                "deliveryMode": "direct",
                "adapterId": "desktop-host",
                "messageId": "message-appserver-preview"
            }
        }),
    );
    assert_eq!(direct["message"]["state"], "accepted");
    assert_eq!(direct["notification"]["title"], "preview");

    let status = call(&root, json!({ "op": "status" }));
    let emitted = status["notificationProjection"]["emitted"]
        .as_array()
        .unwrap();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0]["transportReceipt"]["adapterId"], "desktop-host");
    assert_eq!(emitted[0]["transportReceipt"]["state"], "intent");
    assert_eq!(
        emitted[0]["transportReceipt"]["evidence"]["hostMustExecute"],
        true
    );

    call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "worker" },
                "to": { "scopeId": "scope", "sessionId": "master" },
                "title": "host intent",
                "priority": "p1",
                "body": "desktop must render",
                "deliveryMode": "direct",
                "adapterId": "desktop-host",
                "messageId": "message-appserver-intent"
            }
        }),
    );

    let reopened = call(&root, json!({ "op": "status" }));
    let reopened_emitted = reopened["notificationProjection"]["emitted"]
        .as_array()
        .unwrap();
    assert!(reopened_emitted.iter().any(|notification| {
        notification["transportReceipt"]["kind"] == "appserver"
            && notification["transportReceipt"]["state"] == "intent"
            && notification["transportReceipt"]["evidence"]["hostMustExecute"] == true
    }));
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert!(raw.contains("\"receipt\""));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn adapter_failure_keeps_notification_pending_and_records_error() {
    let root = temp_root("adapter-failure");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let error = call_error(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "legacy-rejected",
                "kind": "tmux",
                "target": "legacy-pane",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert!(error.contains("invalid_adapter_kind"), "{error}");

    let status = call(&root, json!({ "op": "status" }));
    let pending = status["notificationProjection"]["pending"]
        .as_array()
        .unwrap();
    assert!(pending.is_empty());
    assert!(status["notificationProjection"]["emitted"]
        .as_array()
        .unwrap()
        .is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn direct_and_p0_failures_are_never_downgraded_to_idle_batch() {
    let root = temp_root("direct-p0-batch-failure");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let rejected = call_error(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "legacy-rejected",
                "kind": "tmux",
                "target": "legacy-pane",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert!(rejected.contains("invalid_adapter_kind"), "{rejected}");

    for (message_id, priority, delivery_mode) in [
        ("direct-failure", "p1", "direct"),
        ("p0-failure", "p0", "idle"),
    ] {
        call(
            &root,
            json!({
                "op": "send",
                "message": {
                    "from": { "scopeId": "scope", "sessionId": "worker" },
                    "to": { "scopeId": "scope", "sessionId": "master" },
                    "title": message_id,
                    "priority": priority,
                    "body": "delivery remains direct",
                    "deliveryMode": delivery_mode,
                    "messageId": message_id,
                    "createdAt": "2026-01-01T00:00:00Z"
                }
            }),
        );
    }

    let flush = Command::new(binary())
        .args([
            "communication",
            root.to_str().unwrap(),
            "--json",
            &serde_json::to_string(&json!({
                "op": "flush_notifications",
                "now": "2026-01-01T00:10:00Z"
            }))
            .unwrap(),
        ])
        .env("APPSDK_HOME", root.join(".appsdk-host"))
        .output()
        .unwrap();
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    let events: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let batch_attempts: Vec<&Value> = events
        .iter()
        .filter(|event| {
            event["kind"] == "notification.delivery_attempt"
                && event["data"]["attempt"]["operation"] == "notification.batch_emitted"
        })
        .collect();
    assert!(
        batch_attempts.is_empty(),
        "flush must not attempt a batch for direct or p0 failures; stderr={}",
        String::from_utf8_lossy(&flush.stderr)
    );
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(
        status["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        status["notificationProjection"]["emitted"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn disabled_adapter_fails_before_message_persistence() {
    let root = temp_root("adapter-disabled");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "disabled",
                "kind": "appserver",
                "target": "mock://scope",
                "enabled": false,
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    let error = call_error(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "worker" },
                "to": { "scopeId": "scope", "sessionId": "master" },
                "title": "disabled",
                "priority": "p2",
                "body": "adapter is disabled",
                "deliveryMode": "direct",
                "adapterId": "disabled",
                "messageId": "message-disabled"
            }
        }),
    );
    assert!(error.contains("adapter_disabled"), "{error}");
    let status = call(&root, json!({ "op": "status" }));
    assert!(status["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|message| message["messageId"] != "message-disabled"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn idle_flush_is_partitioned_by_adapter() {
    let root = temp_root("adapter-batches");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "appserver-preview",
                "kind": "appserver",
                "target": "mock://scope",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    for (message_id, adapter_id) in [
        ("mailbox-message", "mailbox"),
        ("appserver-message", "appserver-preview"),
    ] {
        call(
            &root,
            json!({
                "op": "send",
                "message": {
                    "from": { "scopeId": "scope", "sessionId": "worker" },
                    "to": { "scopeId": "scope", "sessionId": "master" },
                    "title": message_id,
                    "priority": "p2",
                    "body": "idle update",
                    "deliveryMode": "idle",
                    "coalesceKey": "same-update",
                    "adapterId": adapter_id,
                    "messageId": message_id
                }
            }),
        );
    }
    let result = call(
        &root,
        json!({ "op": "flush_notifications", "now": "2999-01-01T00:02:00Z" }),
    );
    let batches = result["batches"].as_array().unwrap();
    assert_eq!(batches.len(), 2);
    assert_ne!(batches[0]["adapterId"], batches[1]["adapterId"]);
    let status = call(&root, json!({ "op": "status" }));
    let emitted = status["notificationProjection"]["emitted"]
        .as_array()
        .unwrap();
    assert_eq!(emitted.len(), 2);
    assert!(emitted.iter().all(|notification| notification
        .get("transportReceipt")
        .and_then(|receipt| receipt.get("adapterId"))
        .is_some()));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_idle_retries_notification_after_master_registration() {
    let root = temp_root("worker-idle-retry");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let first = call_error(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:00:00Z"
        }),
    );
    assert!(first.contains("master_not_registered"), "{first}");

    register_agent(&root, "scope", "master", "master", "master", None);
    let retry = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:00:10Z"
        }),
    );
    assert_eq!(retry["idempotent"], true);
    // Master registration reconciles the persisted idle edge itself.  A
    // later repeated state observation is a no-op and must not emit a second
    // response or append another message fact.
    assert!(retry["notification"].is_null());
    let message_id = call(&root, json!({ "op": "status" }))["messages"][0]["messageId"]
        .as_str()
        .unwrap()
        .to_owned();
    let repeated = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:00:20Z"
        }),
    );
    assert_eq!(repeated["idempotent"], true);
    assert!(repeated["notification"].is_null());
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(
        status["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(status["messages"].as_array().unwrap().len(), 1);
    assert_eq!(status["messages"][0]["messageId"], message_id);
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"message.created\"").count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_registration_reconciles_existing_worker_idle_edge_once() {
    let root = temp_root("master-registration-idle-reconcile");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let error = call_error(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:00:00Z"
        }),
    );
    assert!(error.contains("master_not_registered"), "{error}");

    register_agent(&root, "scope", "master", "master", "master", None);
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        status["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        status["notificationProjection"]["pending"][0]["messageId"],
        status["messages"][0]["messageId"]
    );

    register_agent(&root, "scope", "master", "master", "master", None);
    let repeated = call(&root, json!({ "op": "status" }));
    assert_eq!(repeated["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        repeated["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"message.created\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_registration_retry_reconciles_agent_registered_prefix() {
    let root = temp_root("master-registration-prefix-retry");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let error = call_error(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:00:00Z"
        }),
    );
    assert!(error.contains("master_not_registered"), "{error}");

    register_agent(&root, "scope", "master", "master", "master", None);
    // Simulate a crash immediately after agent.registered: the durable
    // master identity remains, while reconciliation facts are absent.
    retain_mailbox_through_occurrence(&root, "agent.registered", 2);
    let retry = call(
        &root,
        json!({
            "op": "register_agent",
            "agent": {
                "scopeId": "scope",
                "sessionId": "master",
                "agentId": "master",
                "role": "master",
                "masterGrant": "user approved master for this scope",
                "runtimeId": "runtime-scope"
            }
        }),
    );
    assert_eq!(retry["idempotent"], true);
    assert_eq!(retry["reconciledIdle"].as_array().unwrap().len(), 1);
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        status["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        status["notificationProjection"]["pending"][0]["messageId"],
        status["messages"][0]["messageId"]
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"agent.registered\"").count(), 2);
    assert_eq!(raw.matches("\"kind\":\"message.created\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_idle_replay_after_message_created_prefix_reuses_message_id() {
    let root = temp_root("worker-idle-replay");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let first = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:00:00Z"
        }),
    );
    let message_id = first["notification"]["message"]["messageId"]
        .as_str()
        .unwrap()
        .to_owned();
    retain_mailbox_through(&root, "message.created");

    let recovered = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:00:10Z"
        }),
    );
    assert_eq!(recovered["idempotent"], true);
    assert_eq!(
        recovered["notification"]["message"]["messageId"],
        message_id
    );
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        status["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"message.created\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"message.state\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_idle_second_transition_replays_after_state_prefix_without_reusing_old_bucket() {
    let root = temp_root("worker-idle-second-transition");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);

    let first = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:00:00Z"
        }),
    );
    let first_message_id = first["notification"]["message"]["messageId"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "working",
            "at": "2026-01-01T00:01:00Z"
        }),
    );
    let second = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:02:00Z"
        }),
    );
    let second_message_id = second["notification"]["message"]["messageId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(first_message_id, second_message_id);

    retain_mailbox_through_occurrence(&root, "agent.state", 3);

    let recovered = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:02:10Z"
        }),
    );
    assert_eq!(recovered["idempotent"], true);
    assert_eq!(
        recovered["notification"]["message"]["messageId"],
        second_message_id
    );

    let repeated = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:02:20Z"
        }),
    );
    assert_eq!(repeated["idempotent"], true);
    assert!(repeated["notification"].is_null());

    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["messages"].as_array().unwrap().len(), 2);
    assert!(status["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["messageId"] == first_message_id));
    assert!(status["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["messageId"] == second_message_id));
    assert_eq!(
        status["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"agent.state\"").count(), 3);
    assert_eq!(raw.matches("\"kind\":\"message.created\"").count(), 2);
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_idle_second_transition_replays_after_message_prefix_without_reusing_old_notification() {
    let root = temp_root("worker-idle-second-message-prefix");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);

    let first = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:00:00Z"
        }),
    );
    let first_message_id = first["notification"]["message"]["messageId"]
        .as_str()
        .unwrap()
        .to_owned();
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "working",
            "at": "2026-01-01T00:01:00Z"
        }),
    );
    let second = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:02:00Z"
        }),
    );
    let second_message_id = second["notification"]["message"]["messageId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(first_message_id, second_message_id);

    retain_mailbox_through_occurrence(&root, "message.created", 2);

    let recovered = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:02:10Z"
        }),
    );
    assert_eq!(recovered["idempotent"], true);
    assert_eq!(
        recovered["notification"]["message"]["messageId"],
        second_message_id
    );

    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["messages"].as_array().unwrap().len(), 2);
    assert_eq!(
        status["notificationProjection"]["pending"][0]["messageId"],
        second_message_id
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"agent.state\"").count(), 3);
    assert_eq!(raw.matches("\"kind\":\"message.created\"").count(), 2);
    assert_eq!(raw.matches("\"kind\":\"message.state\"").count(), 2);
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn direct_delivery_failures_keep_each_message_retryable() {
    let root = temp_root("direct-failure-retention");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let rejected = call_error(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "legacy-rejected",
                "kind": "tmux",
                "target": "legacy-pane",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert!(rejected.contains("invalid_adapter_kind"), "{rejected}");
    for (message_id, body) in [("direct-1", "first"), ("direct-2", "second")] {
        call(
            &root,
            json!({
                "op": "send",
                "message": {
                    "from": { "scopeId": "scope", "sessionId": "worker" },
                    "to": { "scopeId": "scope", "sessionId": "master" },
                    "title": message_id,
                    "priority": "p1",
                    "body": body,
                    "deliveryMode": "direct",
                    "messageId": message_id
                }
            }),
        );
    }
    let status = call(&root, json!({ "op": "status" }));
    let pending = status["notificationProjection"]["pending"]
        .as_array()
        .unwrap();
    assert!(pending.is_empty());
    assert_eq!(
        status["notificationProjection"]["emitted"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn idempotent_message_retry_recovers_created_only_prefix() {
    let root = temp_root("message-retry-created");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let request = json!({
        "op": "send",
        "message": {
            "from": { "scopeId": "scope", "sessionId": "worker" },
            "to": { "scopeId": "scope", "sessionId": "master" },
            "title": "recover created",
            "priority": "p1",
            "body": "replay after crash",
            "deliveryMode": "direct",
            "messageId": "recover-created"
        }
    });
    call(&root, request.clone());
    retain_mailbox_through(&root, "message.created");

    let recovered = call(&root, request);
    assert_eq!(recovered["idempotent"], true);
    assert_eq!(recovered["message"]["state"], "accepted");
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        status["notificationProjection"]["emitted"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"message.state\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 1);
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn idempotent_message_retry_recovers_state_prefix_without_notification() {
    let root = temp_root("message-retry-state");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let request = json!({
        "op": "send",
        "message": {
            "from": { "scopeId": "scope", "sessionId": "worker" },
            "to": { "scopeId": "scope", "sessionId": "master" },
            "title": "recover notification",
            "priority": "p1",
            "body": "notification was not queued",
            "deliveryMode": "direct",
            "messageId": "recover-notification"
        }
    });
    call(&root, request.clone());
    retain_mailbox_through(&root, "message.state");

    let recovered = call(&root, request);
    assert_eq!(recovered["idempotent"], true);
    assert_eq!(recovered["message"]["state"], "accepted");
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(
        status["notificationProjection"]["emitted"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"message.state\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn direct_retry_retries_known_failure_without_overwriting_identity() {
    let root = temp_root("direct-retry");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let rejected = call_error(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "legacy-rejected",
                "kind": "tmux",
                "target": "legacy-pane",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert!(rejected.contains("invalid_adapter_kind"), "{rejected}");
    let request = json!({
        "op": "send",
        "message": {
            "from": { "scopeId": "scope", "sessionId": "worker" },
            "to": { "scopeId": "scope", "sessionId": "master" },
            "title": "retry direct",
            "priority": "p1",
            "body": "known transport failure",
            "deliveryMode": "direct",
            "messageId": "retry-direct"
        }
    });
    let first = call(&root, request.clone());
    let second = call(&root, request);
    assert_eq!(first["idempotent"], false);
    assert_eq!(second["idempotent"], true);

    let status = call(&root, json!({ "op": "status" }));
    assert!(status["notificationProjection"]["pending"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(status["notificationProjection"]["unknown"]
        .as_array()
        .unwrap()
        .is_empty());
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        1
    );
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_failed\"")
            .count(),
        0
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unresolved_delivery_attempt_is_unknown_and_is_not_replayed() {
    let root = temp_root("delivery-unknown");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let request = json!({
        "op": "send",
        "message": {
            "from": { "scopeId": "scope", "sessionId": "worker" },
            "to": { "scopeId": "scope", "sessionId": "master" },
            "title": "unknown delivery",
            "priority": "p1",
            "body": "side effect receipt is uncertain",
            "deliveryMode": "direct",
            "messageId": "unknown-delivery"
        }
    });
    call(&root, request.clone());
    retain_mailbox_through(&root, "notification.delivery_attempt");

    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(
        status["notificationProjection"]["unknown"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(status["notificationProjection"]["pending"]
        .as_array()
        .unwrap()
        .is_empty());
    let retry = call(&root, request);
    assert_eq!(retry["idempotent"], true);
    let reopened = call(&root, json!({ "op": "status" }));
    assert_eq!(
        reopened["notificationProjection"]["unknown"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        1
    );
    assert!(!raw.contains("\"kind\":\"notification.emitted\""));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn idempotent_bug_retry_rehydrates_missing_loop_and_notification() {
    let root = temp_root("bug-retry");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    let request = json!({
        "op": "report_bug",
        "bug": {
            "bugId": "bug-recover",
            "scopeId": "scope",
            "title": "recover bug",
            "priority": "p2",
            "description": "notification and loop were interrupted",
            "reporter": { "scopeId": "scope", "sessionId": "master" }
        }
    });
    call(&root, request.clone());
    retain_mailbox_through(&root, "bug.reported");
    remove_mailbox_events(&root, "loop.created");

    let recovered = call(&root, request);
    assert_eq!(recovered["idempotent"], true);
    assert_eq!(recovered["bug"]["bugId"], "bug-recover");
    assert_eq!(recovered["loop"]["loopId"], "bug-loop-bug-recover");
    assert!(recovered["notification"]["notification"].is_object());
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["bugs"].as_array().unwrap().len(), 1);
    assert_eq!(status["loops"].as_array().unwrap().len(), 1);
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"bug.reported\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"loop.created\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_bug_retry_preserves_newer_coalesced_notification_and_window() {
    let root = temp_root("bug-stale-retry");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    let old_request = json!({
        "op": "report_bug",
        "bug": {
            "bugId": "bug-old",
            "scopeId": "scope",
            "title": "old bug summary",
            "priority": "p3",
            "description": "old bug details",
            "reporter": { "scopeId": "scope", "sessionId": "master" }
        }
    });
    call(&root, old_request.clone());
    std::thread::sleep(std::time::Duration::from_millis(2));
    let new_request = json!({
        "op": "report_bug",
        "bug": {
            "bugId": "bug-new",
            "scopeId": "scope",
            "title": "new bug summary",
            "priority": "p1",
            "description": "new bug details",
            "reporter": { "scopeId": "scope", "sessionId": "master" }
        }
    });
    call(&root, new_request);

    let status = call(&root, json!({ "op": "status" }));
    let pending = status["notificationProjection"]["pending"]
        .as_array()
        .unwrap();
    assert_eq!(pending.len(), 1);
    let current = pending
        .iter()
        .find(|notification| notification["issueId"] == "bug-new")
        .unwrap();
    let notification_id = current["notificationId"].as_str().unwrap().to_string();
    let created_at = current["createdAt"].as_str().unwrap().to_string();
    let available_at = current["availableAt"].as_str().unwrap().to_string();
    assert_eq!(current["title"], "bug reported: new bug summary");
    assert_eq!(current["priority"], "p1");

    let retry = call(&root, old_request);
    assert_eq!(retry["idempotent"], true);
    assert_eq!(
        retry["notification"]["notification"]["title"],
        "bug reported: new bug summary"
    );
    assert_eq!(retry["notification"]["notification"]["priority"], "p1");

    let status = call(&root, json!({ "op": "status" }));
    let pending = status["notificationProjection"]["pending"]
        .as_array()
        .unwrap();
    assert_eq!(pending.len(), 1);
    let current = pending
        .iter()
        .find(|notification| notification["issueId"] == "bug-new")
        .unwrap();
    assert_eq!(current["notificationId"], notification_id);
    assert_eq!(current["createdAt"], created_at);
    assert_eq!(current["availableAt"], available_at.clone());
    assert_eq!(current["title"], "bug reported: new bug summary");
    assert_eq!(current["priority"], "p1");

    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": available_at.clone()
        }),
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 2);

    let flushed = call(
        &root,
        json!({ "op": "flush_notifications", "now": available_at }),
    );
    assert!(flushed["batches"].as_array().unwrap().is_empty());
    let wake = call(&root, json!({ "op": "tick", "now": available_at }));
    assert_eq!(wake["masterWakeChanged"].as_array().unwrap().len(), 1);
    assert!(wake["masterWake"][0]["pending"].as_bool().unwrap());
    let status = call(&root, json!({ "op": "status" }));
    assert!(status["notificationProjection"]["pending"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        status["notificationProjection"]["emitted"][0]["title"],
        "master wake: 2 updates (reminder 1/3)"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn idle_coalesce_preserves_first_window_and_latest_summary() {
    let root = temp_root("idle-window");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    for (message_id, title, created_at) in [
        ("idle-1", "first update", "2026-01-01T00:00:00Z"),
        ("idle-2", "latest update", "2026-01-01T00:01:00Z"),
    ] {
        call(
            &root,
            json!({
                "op": "send",
                "message": {
                    "from": { "scopeId": "scope", "sessionId": "worker" },
                    "to": { "scopeId": "scope", "sessionId": "master" },
                    "title": title,
                    "priority": "p2",
                    "body": "progress",
                    "deliveryMode": "idle",
                    "coalesceKey": "progress",
                    "messageId": message_id,
                    "createdAt": created_at
                }
            }),
        );
    }
    let early = call(
        &root,
        json!({ "op": "flush_notifications", "now": "2026-01-01T00:01:59Z" }),
    );
    assert!(early["batches"].as_array().unwrap().is_empty());
    let batch = call(
        &root,
        json!({ "op": "flush_notifications", "now": "2026-01-01T00:02:00Z" }),
    );
    assert_eq!(batch["batches"].as_array().unwrap().len(), 1);
    assert_eq!(batch["batches"][0]["items"][0]["title"], "latest update");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wake_accumulates_worker_idle_and_emits_one_bounded_briefing() {
    let root = temp_root("master-wake-accumulator");
    register_scope(
        &root,
        "scope",
        "app",
        "/project",
        &["master", "worker-a", "worker-b"],
    );
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker-a", "worker-a", "peer", None);
    register_agent(&root, "scope", "worker-b", "worker-b", "peer", None);

    for (session_id, agent_id) in [("worker-a", "worker-a"), ("worker-b", "worker-b")] {
        call(
            &root,
            json!({
                "op": "set_agent_state",
                "address": { "scopeId": "scope", "sessionId": session_id },
                "state": "idle",
                "at": "2026-01-01T00:00:00Z"
            }),
        );
        let repeated = call(
            &root,
            json!({
                "op": "set_agent_state",
                "address": { "scopeId": "scope", "sessionId": session_id },
                "state": "idle",
                "at": "2026-01-01T00:00:01Z"
            }),
        );
        assert_eq!(repeated["idempotent"], true, "{agent_id}");
    }

    let status = call(&root, json!({ "op": "status" }));
    let accumulator = &status["masterWake"][0];
    assert_eq!(accumulator["pending"], true);
    assert_eq!(accumulator["signals"].as_object().unwrap().len(), 2);
    assert_eq!(accumulator["remindersSent"], 0);

    let while_working = call(
        &root,
        json!({ "op": "tick", "now": "2026-01-01T00:02:00Z" }),
    );
    assert!(while_working["masterWakeChanged"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(while_working["changed"].as_array().unwrap().is_empty());

    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": "2026-01-01T00:01:00Z"
        }),
    );
    let wake = call(
        &root,
        json!({ "op": "tick", "now": "2026-01-01T00:02:00Z" }),
    );
    assert_eq!(wake["masterWakeChanged"].as_array().unwrap().len(), 1);
    assert_eq!(wake["masterWake"][0]["remindersSent"], 1);

    let status = call(&root, json!({ "op": "status" }));
    let emitted = status["notificationProjection"]["emitted"]
        .as_array()
        .unwrap();
    assert_eq!(emitted.len(), 1);
    assert!(emitted[0]["title"]
        .as_str()
        .unwrap()
        .starts_with("master wake:"));
    let message = status["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| {
            message["title"]
                .as_str()
                .unwrap()
                .starts_with("master wake:")
        })
        .unwrap();
    let body = message["body"].as_str().unwrap();
    assert!(body.contains("worker-a"));
    assert!(body.contains("worker-b"));
    assert!(body.contains("Idle workers: 2"));
    assert!(body.chars().count() <= 3_600);
    assert!(status["notificationProjection"]["pending"]
        .as_array()
        .unwrap()
        .is_empty());

    let repeated = call(
        &root,
        json!({ "op": "tick", "now": "2026-01-01T00:02:00Z" }),
    );
    assert!(repeated["masterWakeChanged"].as_array().unwrap().is_empty());
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"master_wake.briefing\"").count(), 1);
    assert_eq!(
        raw.matches("\"kind\":\"notification.superseded\"").count(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wake_signal_is_idempotent_and_requires_matching_decision_generation() {
    let root = temp_root("master-wake-signal");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    let request = json!({
        "op": "accumulate_wake",
        "master": { "scopeId": "scope", "sessionId": "master" },
        "signal": {
            "signalId": "goal-1-due",
            "key": "goal:goal-1",
            "kind": "goal_due",
            "title": "goal due",
            "priority": "p1",
            "summary": "goal deadline reached",
            "observedAt": "2026-01-01T00:00:00Z"
        }
    });
    let first = call(&root, request.clone());
    assert_eq!(first["idempotent"], false);
    let second = call(&root, request);
    assert_eq!(second["idempotent"], true);
    assert_eq!(second["masterWake"]["generation"], 1);
    let wrong = call_error(
        &root,
        json!({
            "op": "master_wake_decide",
            "master": { "scopeId": "scope", "sessionId": "master" },
            "generation": 2,
            "action": "handled"
        }),
    );
    assert!(wrong.contains("master_wake_generation_conflict"), "{wrong}");
    let decided = call(
        &root,
        json!({
            "op": "master_wake_decide",
            "master": { "scopeId": "scope", "sessionId": "master" },
            "generation": 1,
            "action": "handled",
            "at": "2026-01-01T00:01:00Z"
        }),
    );
    assert_eq!(decided["masterWake"]["pending"], false);
    assert!(decided["masterWake"]["signals"]
        .as_object()
        .unwrap()
        .is_empty());
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["masterWake"][0]["pending"], false);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wake_consumed_edges_and_hold_are_stable_across_repeated_state_observations() {
    let root = temp_root("master-wake-consumed-edge");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let at = "2026-01-01T00:00:00Z";
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": at
        }),
    );
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": at
        }),
    );
    let due = after(at, 120);
    call(&root, json!({ "op": "tick", "now": due }));
    let generation = call(&root, json!({ "op": "status" }))["masterWake"][0]["generation"]
        .as_u64()
        .unwrap();
    call(
        &root,
        json!({
            "op": "master_wake_decide",
            "master": { "scopeId": "scope", "sessionId": "master" },
            "generation": generation,
            "action": "handled",
            "at": after(&due, 1)
        }),
    );
    let repeated_worker = call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": after(&due, 2)
        }),
    );
    assert!(repeated_worker["notification"].is_null());
    let after_handled = call(&root, json!({ "op": "status" }));
    assert_eq!(after_handled["masterWake"][0]["generation"], generation);
    assert!(!after_handled["masterWake"][0]["pending"].as_bool().unwrap());
    assert_eq!(
        after_handled["masterWake"][0]["consumedSignals"]
            .as_object()
            .unwrap()
            .len(),
        1
    );

    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "working",
            "at": after(&due, 3)
        }),
    );
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": after(&due, 4)
        }),
    );
    let new_cycle = call(&root, json!({ "op": "status" }));
    assert!(new_cycle["masterWake"][0]["pending"].as_bool().unwrap());
    assert!(new_cycle["masterWake"][0]["generation"].as_u64().unwrap() > generation);

    let hold_generation = new_cycle["masterWake"][0]["generation"].as_u64().unwrap();
    call(
        &root,
        json!({
            "op": "master_wake_decide",
            "master": { "scopeId": "scope", "sessionId": "master" },
            "generation": hold_generation,
            "action": "hold",
            "at": after(&due, 5)
        }),
    );
    for offset in [6, 240] {
        call(
            &root,
            json!({
                "op": "set_agent_state",
                "address": { "scopeId": "scope", "sessionId": "master" },
                "state": "idle",
                "at": after(&due, offset)
            }),
        );
        let tick = call(&root, json!({ "op": "tick", "now": after(&due, offset) }));
        assert!(tick["masterWakeChanged"].as_array().unwrap().is_empty());
    }
    let held = call(&root, json!({ "op": "status" }));
    assert_eq!(held["masterWake"][0]["generation"], hold_generation);
    assert_eq!(held["masterWake"][0]["held"], true);
    assert_eq!(held["masterWake"][0]["remindersSent"], 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wake_schedule_after_three_reminders_starts_a_new_generation() {
    let root = temp_root("master-wake-schedule-reset");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let at = "2026-01-01T00:00:00Z";
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": at
        }),
    );
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": at
        }),
    );

    for (index, seconds) in [120, 240, 360].into_iter().enumerate() {
        let wake = call(&root, json!({ "op": "tick", "now": after(at, seconds) }));
        assert_eq!(wake["masterWakeChanged"].as_array().unwrap().len(), 1);
        assert_eq!(wake["masterWake"][0]["remindersSent"], index + 1);
    }

    let stopped = call(&root, json!({ "op": "status" }));
    let old_generation = stopped["masterWake"][0]["generation"].as_u64().unwrap();
    assert_eq!(stopped["masterWake"][0]["remindersSent"], 3);
    assert_eq!(stopped["masterWake"][0]["stopped"], true);

    let rescheduled_at = after(at, 361);
    let scheduled = call(
        &root,
        json!({
            "op": "master_wake_decide",
            "master": { "scopeId": "scope", "sessionId": "master" },
            "generation": old_generation,
            "action": "schedule",
            "at": rescheduled_at.clone()
        }),
    );
    assert_eq!(scheduled["masterWake"]["generation"], old_generation + 1);
    assert_eq!(scheduled["masterWake"]["remindersSent"], 0);
    assert_eq!(scheduled["masterWake"]["stopped"], false);
    assert_eq!(scheduled["masterWake"]["nextDueAt"], rescheduled_at);

    let next = call(&root, json!({ "op": "tick", "now": after(at, 361) }));
    assert_eq!(next["masterWakeChanged"].as_array().unwrap().len(), 1);
    assert_eq!(next["masterWake"][0]["generation"], old_generation + 1);
    assert_eq!(next["masterWake"][0]["remindersSent"], 1);
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"master_wake.briefing\"").count(), 4);
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        4
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wake_window_survives_working_to_idle_transition() {
    let root = temp_root("master-wake-working-window");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let at = "2026-01-01T00:00:00Z";
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": at
        }),
    );
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "working",
            "at": after(at, 1)
        }),
    );
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": after(at, 10)
        }),
    );
    let scheduled = call(&root, json!({ "op": "status" }));
    assert_eq!(scheduled["masterWake"][0]["nextDueAt"], after(at, 120));

    for seconds in [10, 119] {
        let early = call(&root, json!({ "op": "tick", "now": after(at, seconds) }));
        assert!(early["masterWakeChanged"].as_array().unwrap().is_empty());
    }
    let due = call(&root, json!({ "op": "tick", "now": after(at, 120) }));
    assert_eq!(due["masterWakeChanged"].as_array().unwrap().len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wake_terminal_decision_supersedes_pending_notification_before_briefing() {
    let root = temp_root("master-wake-decision-supersedes");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let at = "2026-01-01T00:00:00Z";
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": at
        }),
    );
    let before = call(&root, json!({ "op": "status" }));
    let generation = before["masterWake"][0]["generation"].as_u64().unwrap();
    assert_eq!(
        before["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let decided = call(
        &root,
        json!({
            "op": "master_wake_decide",
            "master": { "scopeId": "scope", "sessionId": "master" },
            "generation": generation,
            "action": "handled",
            "at": after(at, 1)
        }),
    );
    assert_eq!(decided["masterWake"]["pending"], false);
    let replayed = call(&root, json!({ "op": "status" }));
    assert!(replayed["notificationProjection"]["pending"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(replayed["notificationProjection"]["emitted"]
        .as_array()
        .unwrap()
        .is_empty());
    let flushed = call(
        &root,
        json!({ "op": "flush_notifications", "now": after(at, 120) }),
    );
    assert!(flushed["batches"].as_array().unwrap().is_empty());
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(
        raw.matches("\"kind\":\"notification.superseded\"").count(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wake_terminal_decision_replay_stops_legacy_wakeup_without_followup_event() {
    let root = temp_root("master-wake-decision-replay");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let at = "2026-01-01T00:00:00Z";
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": at
        }),
    );
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": at
        }),
    );
    let before = call(&root, json!({ "op": "status" }));
    let generation = before["masterWake"][0]["generation"].as_u64().unwrap();
    call(
        &root,
        json!({
            "op": "master_wake_decide",
            "master": { "scopeId": "scope", "sessionId": "master" },
            "generation": generation,
            "action": "handled",
            "at": after(at, 1)
        }),
    );

    retain_mailbox_through_last(&root, "master_wake.decided");
    let replayed = call(&root, json!({ "op": "status" }));
    assert_eq!(replayed["masterWake"][0]["pending"], false);
    assert_eq!(replayed["wakeup"][0]["stopped"], true);
    assert!(replayed["wakeup"][0]["nextDueAt"].is_null());

    let tick = call(&root, json!({ "op": "tick", "now": after(at, 120) }));
    assert!(tick["masterWakeChanged"].as_array().unwrap().is_empty());
    assert!(tick["changed"].as_array().unwrap().is_empty());
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert!(!raw.contains("\"kind\":\"wakeup.reminder\""));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wake_flush_and_tick_have_one_notification_owner() {
    let root = temp_root("master-wake-flush-order");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let at = "2026-01-01T00:00:00Z";
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": at
        }),
    );
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": at
        }),
    );
    let due = after(at, 120);
    let flushed = call(&root, json!({ "op": "flush_notifications", "now": due }));
    assert!(flushed["batches"].as_array().unwrap().is_empty());
    let wake = call(&root, json!({ "op": "tick", "now": due }));
    assert_eq!(wake["masterWakeChanged"].as_array().unwrap().len(), 1);
    let after_tick = call(&root, json!({ "op": "status" }));
    assert_eq!(
        after_tick["notificationProjection"]["emitted"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let flushed_after = call(
        &root,
        json!({ "op": "flush_notifications", "now": after(&due, 1) }),
    );
    assert!(flushed_after["batches"].as_array().unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn generic_p0_wake_is_direct_and_unknown_is_not_replayed() {
    let root = temp_root("generic-p0-wake");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    let request = json!({
        "op": "record_wake",
        "master": { "scopeId": "scope", "sessionId": "master" },
        "signal": {
            "signalId": "urgent-1",
            "key": "goal:urgent-1",
            "kind": "goal_due",
            "title": "urgent goal",
            "priority": "p0",
            "summary": "interrupt the master now",
            "observedAt": "2026-01-01T00:00:00Z"
        }
    });
    let direct = call(&root, request.clone());
    assert_eq!(direct["masterWake"]["pending"], false);
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(
        status["notificationProjection"]["emitted"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let repeated = call(&root, request.clone());
    assert_eq!(repeated["idempotent"], true);
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        1
    );

    let unknown_root = temp_root("generic-p0-unknown");
    register_scope(&unknown_root, "scope", "app", "/project", &["master"]);
    register_agent(&unknown_root, "scope", "master", "master", "master", None);
    call(&unknown_root, request.clone());
    retain_mailbox_through_last(&unknown_root, "notification.delivery_attempt");
    let unknown = call(&unknown_root, request);
    assert_eq!(unknown["masterWake"]["pending"], true);
    let unknown_status = call(&unknown_root, json!({ "op": "status" }));
    assert_eq!(
        unknown_status["notificationProjection"]["unknown"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let unknown_raw =
        fs::read_to_string(unknown_root.join(".appsdk-control/communication/mailbox.jsonl"))
            .unwrap();
    assert_eq!(
        unknown_raw
            .matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        1
    );
    assert!(!unknown_raw.contains("\"kind\":\"master_wake.briefing\""));
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(unknown_root).unwrap();
}

#[test]
fn peer_owned_loop_error_wakes_scope_master() {
    let root = temp_root("peer-loop-error");
    register_scope(&root, "scope", "app", "/project", &["master", "peer"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "peer", "peer", "peer", None);
    call(
        &root,
        json!({
            "op": "create_loop",
            "loop": {
                "loopId": "peer-loop",
                "kind": "peer",
                "owner": { "scopeId": "scope", "sessionId": "peer" },
                "trigger": "event",
                "work": "peer work",
                "gate": "peer tests",
                "state": "persist",
                "stop": "blocked"
            }
        }),
    );
    let error = call(
        &root,
        json!({
            "op": "record_error",
            "code": "peer_failure",
            "message": "peer loop failed",
            "context": { "source": "peer" },
            "loopId": "peer-loop"
        }),
    );
    assert_eq!(error["loop"]["status"], "blocked");
    assert_eq!(error["masterWake"]["address"]["sessionId"], "master");
    assert_eq!(error["masterWake"]["pending"], true);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wake_reuses_persisted_briefing_body_after_queue_crash() {
    let root = temp_root("master-wake-stable-body");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let at = "2026-01-01T00:00:00Z";
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": at
        }),
    );
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": at
        }),
    );
    let due = after(at, 120);
    call(&root, json!({ "op": "tick", "now": due }));
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    let end = lines
        .iter()
        .rposition(|line| line.contains("\"kind\":\"notification.queued\""))
        .unwrap();
    fs::write(&mailbox, format!("{}\n", lines[..=end].join("\n"))).unwrap();

    call(
        &root,
        json!({
            "op": "create_loop",
            "loop": {
                "loopId": "new-loop-after-crash",
                "kind": "master",
                "owner": { "scopeId": "scope", "sessionId": "master" },
                "trigger": "event",
                "work": "new work after crash",
                "gate": "tests",
                "state": "persist",
                "stop": "done"
            }
        }),
    );
    let recovered = call(&root, json!({ "op": "tick", "now": due }));
    assert_eq!(recovered["masterWakeChanged"].as_array().unwrap().len(), 1);
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(
        status["notificationProjection"]["emitted"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(status["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["title"]
            .as_str()
            .unwrap()
            .starts_with("master wake:")));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wake_briefing_replay_rejects_missing_fields_and_terminal_delivery() {
    let make_root = |name: &str| {
        let root = temp_root(name);
        register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
        register_agent(&root, "scope", "master", "master", "master", None);
        register_agent(&root, "scope", "worker", "worker", "peer", None);
        let at = "2026-01-01T00:00:00Z";
        call(
            &root,
            json!({
                "op": "set_agent_state",
                "address": { "scopeId": "scope", "sessionId": "worker" },
                "state": "idle",
                "at": at
            }),
        );
        call(
            &root,
            json!({
                "op": "set_agent_state",
                "address": { "scopeId": "scope", "sessionId": "master" },
                "state": "idle",
                "at": at
            }),
        );
        call(&root, json!({ "op": "tick", "now": after(at, 120) }));
        root
    };

    let missing = make_root("master-wake-missing-event-field");
    let mailbox = missing.join(".appsdk-control/communication/mailbox.jsonl");
    let mut lines: Vec<Value> = fs::read_to_string(&mailbox)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let event = lines
        .iter_mut()
        .find(|event| event["kind"] == "master_wake.briefing")
        .unwrap();
    event["data"]
        .as_object_mut()
        .unwrap()
        .remove("notification");
    fs::write(
        &mailbox,
        lines
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    let missing_error = call_error(&missing, json!({ "op": "status" }));
    assert!(missing_error.contains("master wake event field is missing: notification"));
    fs::remove_dir_all(missing).unwrap();

    let terminal = make_root("master-wake-missing-terminal");
    let mailbox = terminal.join(".appsdk-control/communication/mailbox.jsonl");
    let terminal_contents = fs::read_to_string(&mailbox).unwrap();
    let retained: Vec<&str> = terminal_contents
        .lines()
        .filter(|line| !line.contains("\"kind\":\"notification.emitted\""))
        .collect();
    fs::write(&mailbox, format!("{}\n", retained.join("\n"))).unwrap();
    let terminal_error = call_error(&terminal, json!({ "op": "status" }));
    assert!(terminal_error.contains("master wake briefing notification"));
    fs::remove_dir_all(terminal).unwrap();
}

#[test]
fn p0_bug_reactivation_is_direct_and_does_not_leave_master_wake_pending() {
    let root = temp_root("p0-bug-reactivation");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    call(
        &root,
        json!({
            "op": "report_bug",
            "bug": {
                "bugId": "bug-p0",
                "scopeId": "scope",
                "title": "urgent failure",
                "priority": "p0",
                "description": "must interrupt the master",
                "reporter": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    let before = call(&root, json!({ "op": "status" }));
    assert_eq!(
        before["notificationProjection"]["emitted"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(before["masterWake"][0]["pending"], false);

    let updated = call(
        &root,
        json!({
            "op": "update_bug",
            "bugId": "bug-p0",
            "status": "active",
            "actor": { "scopeId": "scope", "sessionId": "master" }
        }),
    );
    assert_eq!(updated["notification"]["notification"]["priority"], "p0");
    assert_eq!(updated["masterWake"]["pending"], false);
    let after = call(&root, json!({ "op": "status" }));
    assert_eq!(
        after["notificationProjection"]["emitted"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(after["masterWake"][0]["pending"], false);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wake_crash_after_attempt_remains_unknown_without_replay_or_budget_use() {
    let root = temp_root("master-wake-attempt-crash");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let at = "2026-01-01T00:00:00Z";
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": at
        }),
    );
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": at
        }),
    );
    let due = after(at, 120);
    let first = call(&root, json!({ "op": "tick", "now": due }));
    assert_eq!(first["masterWakeChanged"].as_array().unwrap().len(), 1);
    assert_eq!(first["masterWake"][0]["remindersSent"], 1);
    retain_mailbox_through_last(&root, "notification.delivery_attempt");

    let replayed = call(&root, json!({ "op": "status" }));
    assert_eq!(
        replayed["notificationProjection"]["unknown"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(replayed["masterWake"][0]["remindersSent"], 0);
    let retry = call(&root, json!({ "op": "tick", "now": due }));
    assert!(retry["masterWakeChanged"].as_array().unwrap().is_empty());
    assert_eq!(retry["masterWake"][0]["remindersSent"], 0);
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        1
    );
    assert!(!raw.contains("\"kind\":\"master_wake.briefing\""));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wake_crash_after_queue_reuses_message_and_notification_identity() {
    let root = temp_root("master-wake-queue-crash");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let at = "2026-01-01T00:00:00Z";
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": at
        }),
    );
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": at
        }),
    );
    let due = after(at, 120);
    call(&root, json!({ "op": "tick", "now": due }));
    retain_mailbox_through_last(&root, "notification.queued");

    let retry = call(&root, json!({ "op": "tick", "now": due }));
    assert_eq!(retry["masterWakeChanged"].as_array().unwrap().len(), 1);
    assert_eq!(retry["masterWake"][0]["remindersSent"], 1);
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 2);
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        1
    );
    assert_eq!(raw.matches("\"kind\":\"master_wake.briefing\"").count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn agent_rebind_reconciles_emitted_direct_wake_after_dispatch_mark_crash() {
    let root = temp_root("agent-rebind-emitted-direct-wake");
    register_scope(
        &root,
        "scope",
        "app",
        "/project",
        &["master-old", "master-new"],
    );
    register_agent(&root, "scope", "master-old", "master", "master", None);
    call(
        &root,
        json!({
            "op": "accumulate_wake",
            "master": { "scopeId": "scope", "sessionId": "master-old" },
            "signal": {
                "key": "bug:rebind-direct-crash",
                "kind": "bug",
                "title": "rebind direct crash",
                "priority": "p0",
                "summary": "direct delivery already emitted before the dispatch mark",
                "issueId": "bug-direct-crash",
                "observedAt": "2026-01-01T00:00:00Z"
            }
        }),
    );
    // Crash window: the direct notification reached `emitted`, but the
    // accumulator's `directDispatched` mark was never committed.
    retain_mailbox_through_last(&root, "notification.emitted");

    call(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "master-new" },
                "runtimeId": "runtime-scope"
            }
        }),
    );

    let ticked = call(
        &root,
        json!({ "op": "tick", "now": "2026-01-01T00:02:00Z" }),
    );
    assert!(ticked["masterWake"][0]["signals"]
        .as_object()
        .unwrap()
        .values()
        .any(|signal| signal["directDispatched"] == true));
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    let events: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(count_events(&events, "message.created"), 1);
    assert_eq!(count_events(&events, "notification.emitted"), 1);
    assert_eq!(count_events(&events, "master_wake.briefing"), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn agent_rebind_reuses_worker_sourced_wake_signal_identity() {
    let root = temp_root("agent-rebind-worker-signal-identity");
    register_scope(
        &root,
        "scope",
        "app",
        "/project",
        &["master", "worker-old", "worker-new"],
    );
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker-old", "worker", "peer", None);
    let at = "2026-01-01T00:00:00Z";
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master" },
            "state": "idle",
            "at": at
        }),
    );
    call(
        &root,
        json!({
            "op": "accumulate_wake",
            "master": { "scopeId": "scope", "sessionId": "master" },
            "signal": {
                "key": "worker-idle:rebind",
                "kind": "worker_idle",
                "title": "worker idle: worker",
                "priority": "p0",
                "summary": "worker sourced breakthrough signal",
                "source": { "scopeId": "scope", "sessionId": "worker-old" },
                "observedAt": at
            }
        }),
    );
    // Crash window: the direct delivery was emitted but the accumulator's
    // dispatch mark was not yet committed when the worker rebinds.
    retain_mailbox_through_last(&root, "notification.emitted");

    call(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "worker-old" },
                "to": { "scopeId": "scope", "sessionId": "worker-new" },
                "runtimeId": "runtime-scope"
            }
        }),
    );

    call(&root, json!({ "op": "tick", "now": after(at, 120) }));
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    let events: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(count_events(&events, "message.created"), 1);
    assert_eq!(count_events(&events, "master_wake.briefing"), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn agent_rebind_reuses_queued_wakeup_reminder_identity() {
    let root = temp_root("agent-rebind-queued-wakeup-reminder");
    register_scope(
        &root,
        "scope",
        "app",
        "/project",
        &["master-old", "master-new"],
    );
    register_agent(&root, "scope", "master-old", "master", "master", None);
    let at = "2026-01-01T00:00:00Z";
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master-old" },
            "state": "idle",
            "at": at
        }),
    );
    let due = after(at, 120);
    call(&root, json!({ "op": "tick", "now": due }));
    // Crash after the reminder was queued, before the emit and wakeup advance.
    retain_mailbox_through_last(&root, "notification.queued");

    call(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "master-new" },
                "runtimeId": "runtime-scope"
            }
        }),
    );

    call(&root, json!({ "op": "tick", "now": due }));
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    let events: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    // The reminder was queued pre-rebind; replaying its tick must recover that
    // projection rather than minting a second reminder message for the new address.
    assert_eq!(count_events(&events, "message.created"), 1);
    assert_eq!(count_events(&events, "notification.queued"), 1);
    let message_id = events
        .iter()
        .find(|event| event["kind"] == "message.created")
        .unwrap()["data"]["messageId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(message_id.contains("master-old"), "{message_id}");
    let reminder = events
        .iter()
        .find(|event| event["kind"] == "wakeup.reminder")
        .expect("wakeup reminder committed");
    assert_eq!(
        reminder["data"]["message"]["messageId"],
        message_id.as_str()
    );
    assert_eq!(
        reminder["data"]["wakeup"]["address"]["sessionId"],
        "master-new"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn agent_rebind_reuses_queued_wake_briefing_identity() {
    let root = temp_root("agent-rebind-queued-wake-briefing");
    register_scope(
        &root,
        "scope",
        "app",
        "/project",
        &["master-old", "master-new"],
    );
    register_agent(&root, "scope", "master-old", "master", "master", None);
    let at = "2026-01-01T00:00:00Z";
    call(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "master-old" },
            "state": "idle",
            "at": at
        }),
    );
    call(
        &root,
        json!({
            "op": "accumulate_wake",
            "master": { "scopeId": "scope", "sessionId": "master-old" },
            "signal": {
                "key": "bug:rebind-briefing",
                "kind": "bug",
                "title": "rebind queued briefing",
                "priority": "p1",
                "summary": "queued briefing must keep its identity across rebind",
                "issueId": "bug-queued-briefing",
                "observedAt": at
            }
        }),
    );
    let due = after(at, 120);
    call(&root, json!({ "op": "tick", "now": due }));
    retain_mailbox_through_last(&root, "notification.queued");

    call(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "master-new" },
                "runtimeId": "runtime-scope"
            }
        }),
    );

    let retry = call(&root, json!({ "op": "tick", "now": due }));
    assert_eq!(retry["masterWake"][0]["remindersSent"], 1);
    assert_eq!(retry["masterWake"][0]["address"]["sessionId"], "master-new");
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    let events: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(count_events(&events, "master_wake.briefing"), 1);
    assert_eq!(count_events(&events, "notification.delivery_attempt"), 1);
    assert_eq!(count_events(&events, "notification.emitted"), 1);
    // The briefing was queued before the rebind; replaying its tick must recover
    // that projection instead of minting a second message for the new address.
    assert_eq!(count_events(&events, "message.created"), 1);
    assert_eq!(count_events(&events, "notification.queued"), 1);
    let queued_message_id = events
        .iter()
        .find(|event| event["kind"] == "notification.queued")
        .unwrap()["data"]["notification"]["messageId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        queued_message_id.contains("master-old"),
        "{queued_message_id}"
    );
    let briefing = events
        .iter()
        .find(|event| event["kind"] == "master_wake.briefing")
        .unwrap();
    assert_eq!(
        briefing["data"]["notification"]["recipient"]["sessionId"],
        "master-new"
    );
    assert_eq!(
        briefing["data"]["message"]["messageId"],
        queued_message_id.as_str()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn address_keys_and_message_ids_fail_closed_on_collisions() {
    let root = temp_root("collision");
    register_scope(&root, "a", "app-a", "/project-a", &["b/c"]);
    register_scope(&root, "a/b", "app-b", "/project-b", &["c"]);
    register_agent(&root, "a", "b/c", "agent-one", "peer", None);
    register_agent(&root, "a/b", "c", "agent-two", "peer", None);
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["agents"].as_array().unwrap().len(), 2);

    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "worker" },
                "to": { "scopeId": "scope", "sessionId": "master" },
                "title": "stable",
                "priority": "p2",
                "body": "first",
                "messageId": "same-id"
            }
        }),
    );
    let conflict = call_error(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "worker" },
                "to": { "scopeId": "scope", "sessionId": "master" },
                "title": "stable",
                "priority": "p2",
                "body": "changed",
                "messageId": "same-id"
            }
        }),
    );
    assert!(conflict.contains("message_id_conflict"), "{conflict}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn adapter_binding_and_bug_loop_gates_are_enforced() {
    let root = temp_root("gates");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "master-appserver",
                "kind": "appserver",
                "target": "mock://scope",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    let wrong_target = call_error(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master" },
                "to": { "scopeId": "scope", "sessionId": "worker" },
                "title": "wrong target",
                "priority": "p2",
                "body": "must fail",
                "adapterId": "master-appserver"
            }
        }),
    );
    assert!(
        wrong_target.contains("adapter_recipient_mismatch"),
        "{wrong_target}"
    );

    call(
        &root,
        json!({
            "op": "report_bug",
            "bug": {
                "bugId": "bug-low",
                "scopeId": "scope",
                "title": "low",
                "priority": "p3",
                "description": "low",
                "reporter": { "scopeId": "scope", "sessionId": "worker" }
            }
        }),
    );
    call(
        &root,
        json!({
            "op": "report_bug",
            "bug": {
                "bugId": "bug-high",
                "scopeId": "scope",
                "title": "high",
                "priority": "p0",
                "description": "high",
                "reporter": { "scopeId": "scope", "sessionId": "worker" }
            }
        }),
    );
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["loops"][0]["loopId"], "bug-loop-bug-high");

    let peer_close = call_error(
        &root,
        json!({
            "op": "update_bug",
            "bugId": "bug-high",
            "status": "closed",
            "actor": { "scopeId": "scope", "sessionId": "worker" },
            "evidence": { "fix": "commit", "verification": "tests", "merge": "main" }
        }),
    );
    assert!(
        peer_close.contains("bug_resolution_master_required"),
        "{peer_close}"
    );

    let missing_evidence = call_error(
        &root,
        json!({
            "op": "update_bug",
            "bugId": "bug-high",
            "status": "closed",
            "actor": { "scopeId": "scope", "sessionId": "master" }
        }),
    );
    assert!(
        missing_evidence.contains("bug_resolution_evidence_required"),
        "{missing_evidence}"
    );
    for evidence in [
        json!({ "fix": false, "verification": "tests", "merge": "main" }),
        json!({ "fix": [], "verification": "tests", "merge": "main" }),
        json!({ "fix": {}, "verification": "tests", "merge": "main" }),
        json!({ "fix": { "command": "cargo test" }, "verification": "tests", "merge": "main" }),
        json!({ "fix": { "status": "unknown", "command": "cargo test" }, "verification": "tests", "merge": "main" }),
        json!({ "fix": { "status": "failed", "command": "cargo test" }, "verification": "tests", "merge": "main" }),
        json!({ "fix": { "status": "fail", "command": "cargo test" }, "verification": "tests", "merge": "main" }),
        json!({ "fix": { "status": "timeout", "command": "cargo test" }, "verification": "tests", "merge": "main" }),
        json!({ "fix": { "status": "blocked", "command": "cargo test" }, "verification": "tests", "merge": "main" }),
        json!({ "fix": { "status": "tests passed with warnings", "command": "cargo test" }, "verification": "tests", "merge": "main" }),
        json!({ "fix": { "status": "passed", "result": "failed", "command": "cargo test" }, "verification": "tests", "merge": "main" }),
        json!({ "fix": "commit", "verification": "unknown", "merge": "main" }),
        json!({ "fix": "commit", "verification": "failed", "merge": "main" }),
        json!({ "fix": "commit", "verification": { "status": "blocked" }, "merge": "main" }),
        json!({ "fix": "commit", "verification": "tests", "merge": { "result": "fail" } }),
        json!({ "fix": { "passed": "true" }, "verification": "tests", "merge": "main" }),
    ] {
        let invalid_evidence = call_error(
            &root,
            json!({
                "op": "update_bug",
                "bugId": "bug-high",
                "status": "closed",
                "actor": { "scopeId": "scope", "sessionId": "master" },
                "evidence": evidence
            }),
        );
        assert!(
            invalid_evidence.contains("bug_resolution_evidence_invalid"),
            "invalid resolution evidence must fail closed: {invalid_evidence}"
        );
    }
    let closed = call(
        &root,
        json!({
            "op": "update_bug",
            "bugId": "bug-high",
            "status": "closed",
            "actor": { "scopeId": "scope", "sessionId": "master" },
            "evidence": { "fix": "commit", "verification": "tests", "merge": "main" }
        }),
    );
    assert_eq!(closed["bug"]["status"], "closed");
    assert_eq!(closed["loop"]["status"], "completed");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn loop_completion_requires_gate_and_deadline_wins() {
    let root = temp_root("loop-gate");
    register_scope(&root, "scope", "app", "/project", &["master"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    call(
        &root,
        json!({
            "op": "create_loop",
            "loop": {
                "loopId": "loop-gated",
                "kind": "master",
                "owner": { "scopeId": "scope", "sessionId": "master" },
                "trigger": "event",
                "work": "work",
                "gate": "tests",
                "state": "persist",
                "stop": "done"
            }
        }),
    );
    let missing = call_error(
        &root,
        json!({ "op": "advance_loop", "loopId": "loop-gated", "complete": true }),
    );
    assert!(missing.contains("loop_gate_evidence_required"), "{missing}");

    for evidence in [
        Value::Null,
        json!(false),
        json!([]),
        json!({}),
        json!({ "gate": null }),
        json!({ "gate": false }),
        json!({ "gate": "" }),
        json!({ "gate": "unknown" }),
        json!({ "gate": "fail" }),
        json!({ "gate": "timeout" }),
        json!({ "gate": "blocked" }),
        json!({ "gate": { "command": "cargo test" } }),
        json!({ "gate": { "status": "unknown", "command": "cargo test" } }),
        json!({ "gate": { "status": "failed", "command": "cargo test" } }),
        json!({ "gate": { "status": "fail", "command": "cargo test" } }),
        json!({ "gate": { "status": "timeout", "command": "cargo test" } }),
        json!({ "gate": { "status": "blocked", "command": "cargo test" } }),
        json!({ "gate": { "status": "tests passed with warnings", "command": "cargo test" } }),
        json!({ "gate": { "status": "passed", "result": "failed", "command": "cargo test" } }),
        json!({ "gate": { "status": "passed", "checks": [{ "result": "timeout", "command": "cargo test" }] } }),
        json!({ "gate": { "status": "passed", "passed": "true" } }),
        json!({ "gate": { "status": true } }),
        json!({ "gate": { "passed": false } }),
        json!({ "verification": { "status": "unknown" } }),
        json!({ "verification": { "status": "failed" } }),
        json!({ "gate": { "status": "passed" }, "verification": { "status": "unknown" } }),
        json!({ "gate": { "status": "passed" }, "verification": { "status": "failed" } }),
        json!({ "gate": { "status": "passed" }, "verification": { "status": "timeout" } }),
        json!({ "gate": { "status": "passed" }, "verification": { "status": "blocked" } }),
    ] {
        let invalid = call_error(
            &root,
            json!({
                "op": "advance_loop",
                "loopId": "loop-gated",
                "complete": true,
                "evidence": evidence
            }),
        );
        assert!(
            invalid.contains("loop_gate_evidence_"),
            "invalid evidence must fail closed: {invalid}"
        );
    }

    let completed = call(
        &root,
        json!({
            "op": "advance_loop",
            "loopId": "loop-gated",
            "complete": true,
            "evidence": {
                "gate": { "status": "passed", "command": "cargo test" },
                "verification": "communication_cli"
            }
        }),
    );
    assert_eq!(
        completed["loop"]["completionEvidence"]["gate"]["status"],
        "passed"
    );
    let replayed = call(&root, json!({ "op": "status" }));
    assert_eq!(
        replayed["loops"]
            .as_array()
            .unwrap()
            .iter()
            .find(|loop_record| loop_record["loopId"] == "loop-gated")
            .unwrap()["completionEvidence"]["verification"],
        "communication_cli"
    );

    call(
        &root,
        json!({
            "op": "create_loop",
            "loop": {
                "loopId": "loop-explicit-results",
                "kind": "master",
                "owner": { "scopeId": "scope", "sessionId": "master" },
                "trigger": "event",
                "work": "work",
                "gate": "tests",
                "state": "persist",
                "stop": "done"
            }
        }),
    );
    let explicit_results = call(
        &root,
        json!({
            "op": "advance_loop",
            "loopId": "loop-explicit-results",
            "complete": true,
            "evidence": {
                "gate": {
                    "status": "PASSED",
                    "result": ["pass", { "outcome": "success", "command": "cargo test" }],
                    "state": "verified",
                    "passed": true,
                    "checks": [{ "status": "ok", "command": "cargo test" }]
                },
                "verification": { "status": "true", "verified": true, "command": "communication_cli" }
            }
        }),
    );
    assert_eq!(
        explicit_results["loop"]["completionEvidence"]["gate"]["status"],
        "PASSED"
    );

    call(
        &root,
        json!({
            "op": "create_loop",
            "loop": {
                "loopId": "loop-expired",
                "kind": "master",
                "owner": { "scopeId": "scope", "sessionId": "master" },
                "trigger": "event",
                "work": "work",
                "gate": "tests",
                "state": "persist",
                "stop": "done",
                "deadlineAt": "2020-01-01T00:00:00Z"
            }
        }),
    );
    let expired = call(
        &root,
        json!({
            "op": "advance_loop",
            "loopId": "loop-expired",
            "complete": true,
            "evidence": { "gate": "tests" },
            "now": "2021-01-01T00:00:00Z"
        }),
    );
    assert_eq!(expired["loop"]["status"], "stopped");
    assert_eq!(expired["loop"]["phase"], "deadline");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bug_loop_collisions_fail_closed_without_mutating_unrelated_loops() {
    let root = temp_root("bug-loop-conflict");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);

    let reserved = call_error(
        &root,
        json!({
            "op": "create_loop",
            "loop": {
                "loopId": "bug-loop-report-conflict",
                "kind": "task",
                "owner": { "scopeId": "scope", "sessionId": "master" },
                "trigger": "manual",
                "work": "unrelated work",
                "gate": "other tests",
                "state": "other state",
                "stop": "other stop"
            }
        }),
    );
    assert!(reserved.contains("bug_loop_reserved"), "{reserved}");

    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let report_conflicting_loop = json!({
        "protocol": "appsdk-comm/v1",
        "eventId": "manual-conflicting-report-loop",
        "at": "2026-01-01T00:00:00Z",
        "kind": "loop.created",
        "data": {
            "loopId": "bug-loop-report-conflict",
            "kind": "task",
            "owner": { "scopeId": "scope", "sessionId": "master" },
            "trigger": "manual",
            "work": "unrelated work",
            "gate": "other tests",
            "state": "other state",
            "stop": "other stop",
            "maxIterations": 1,
            "deadlineAt": null,
            "phase": "discover",
            "status": "active",
            "iteration": 0,
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z"
        }
    });
    let mut file = fs::OpenOptions::new().append(true).open(&mailbox).unwrap();
    writeln!(
        file,
        "{}",
        serde_json::to_string(&report_conflicting_loop).unwrap()
    )
    .unwrap();
    drop(file);

    let report_conflict = call_error(
        &root,
        json!({
            "op": "report_bug",
            "bug": {
                "bugId": "report-conflict",
                "scopeId": "scope",
                "title": "collision",
                "priority": "p1",
                "description": "must not reuse unrelated loop",
                "reporter": { "scopeId": "scope", "sessionId": "worker" }
            }
        }),
    );
    assert!(
        report_conflict.contains("bug_loop_conflict"),
        "{report_conflict}"
    );
    let after_report_conflict = call(&root, json!({ "op": "status" }));
    assert!(after_report_conflict["bugs"].as_array().unwrap().is_empty());
    let unrelated = after_report_conflict["loops"]
        .as_array()
        .unwrap()
        .iter()
        .find(|loop_record| loop_record["loopId"] == "bug-loop-report-conflict")
        .unwrap();
    assert_eq!(unrelated["kind"], "task");

    register_scope(
        &root,
        "scope-b",
        "other-app",
        "/other-project",
        &["master-b", "worker-b"],
    );
    register_agent(&root, "scope-b", "master-b", "master-b", "master", None);
    register_agent(&root, "scope-b", "worker-b", "worker-b", "peer", None);
    let cross_scope_loop = json!({
        "protocol": "appsdk-comm/v1",
        "eventId": "manual-cross-scope-loop",
        "at": "2026-01-01T00:00:00Z",
        "kind": "loop.created",
        "data": {
            "loopId": "bug-loop-cross-scope-conflict",
            "kind": "bug",
            "owner": { "scopeId": "scope", "sessionId": "master" },
            "trigger": "event:bug.reported",
            "work": "triage -> fix in an independent worktree",
            "gate": "project verification and review",
            "state": "persist bug evidence and next action",
            "stop": "resolved, merged, and reporter notified",
            "maxIterations": 100,
            "deadlineAt": null,
            "phase": "discover",
            "status": "active",
            "iteration": 0,
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z"
        }
    });
    let mut file = fs::OpenOptions::new().append(true).open(&mailbox).unwrap();
    writeln!(
        file,
        "{}",
        serde_json::to_string(&cross_scope_loop).unwrap()
    )
    .unwrap();
    drop(file);
    let cross_scope_conflict = call_error(
        &root,
        json!({
            "op": "report_bug",
            "bug": {
                "bugId": "cross-scope-conflict",
                "scopeId": "scope-b",
                "title": "cross scope collision",
                "priority": "p1",
                "description": "must not reuse another scope loop",
                "reporter": { "scopeId": "scope-b", "sessionId": "worker-b" }
            }
        }),
    );
    assert!(
        cross_scope_conflict.contains("bug_loop_conflict"),
        "{cross_scope_conflict}"
    );

    call(
        &root,
        json!({
            "op": "report_bug",
            "bug": {
                "bugId": "update-conflict",
                "scopeId": "scope",
                "title": "update collision",
                "priority": "p1",
                "description": "must reject a corrupted bug loop",
                "reporter": { "scopeId": "scope", "sessionId": "worker" }
            }
        }),
    );
    let conflicting_loop = json!({
        "protocol": "appsdk-comm/v1",
        "eventId": "manual-conflicting-loop",
        "at": "2026-01-01T00:00:00Z",
        "kind": "loop.updated",
        "data": {
            "loopId": "bug-loop-update-conflict",
            "kind": "task",
            "owner": { "scopeId": "scope", "sessionId": "master" },
            "trigger": "manual",
            "work": "unrelated work",
            "gate": "other tests",
            "state": "other state",
            "stop": "other stop",
            "maxIterations": 1,
            "deadlineAt": null,
            "phase": "discover",
            "status": "active",
            "iteration": 0,
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z"
        }
    });
    let mut file = fs::OpenOptions::new().append(true).open(mailbox).unwrap();
    writeln!(
        file,
        "{}",
        serde_json::to_string(&conflicting_loop).unwrap()
    )
    .unwrap();
    drop(file);

    let update_conflict = call_error(
        &root,
        json!({
            "op": "update_bug",
            "bugId": "update-conflict",
            "status": "active",
            "actor": { "scopeId": "scope", "sessionId": "master" }
        }),
    );
    assert!(
        update_conflict.contains("bug_loop_conflict"),
        "{update_conflict}"
    );
    let after_update_conflict = call(&root, json!({ "op": "status" }));
    let bug = after_update_conflict["bugs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|bug| bug["bugId"] == "update-conflict")
        .unwrap();
    assert_eq!(bug["status"], "active");
    let corrupted_loop = after_update_conflict["loops"]
        .as_array()
        .unwrap()
        .iter()
        .find(|loop_record| loop_record["loopId"] == "bug-loop-update-conflict")
        .unwrap();
    assert_eq!(corrupted_loop["kind"], "task");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn agent_rebind_preserves_identity_updates_master_and_replays() {
    let root = temp_root("agent-rebind-success");
    register_scope(
        &root,
        "scope",
        "app",
        "/project",
        &["master-old", "master-new"],
    );
    register_agent(&root, "scope", "master-old", "master", "master", None);

    let rebound = call(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "master-new" },
                "runtimeId": "runtime-scope"
            }
        }),
    );
    assert_eq!(rebound["idempotent"], false);
    assert_eq!(rebound["agent"]["agentId"], "master");
    assert_eq!(rebound["agent"]["sessionId"], "master-new");
    assert_eq!(rebound["tombstone"]["address"]["sessionId"], "master-old");
    assert_eq!(rebound["tombstone"]["reboundTo"]["sessionId"], "master-new");

    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["agents"].as_array().unwrap().len(), 1);
    assert_eq!(status["agents"][0]["sessionId"], "master-new");
    assert_eq!(
        status["agents"][0]["masterGrant"],
        "user approved master for this scope"
    );
    assert_eq!(status["scopes"][0]["masterSessionId"], "master-new");
    assert_eq!(status["agentTombstones"].as_array().unwrap().len(), 1);
    assert_eq!(status["agentTombstones"][0]["agentId"], "master");
    assert_eq!(status["agentTombstones"][0]["runtimeId"], "runtime-scope");

    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"agent.rebound\"").count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn agent_rebind_preserves_existing_wakeup_message_delivery_identity() {
    let root = temp_root("agent-rebind-wakeup-message-identity");
    register_scope(
        &root,
        "scope",
        "app",
        "/project",
        &["master-old", "master-new"],
    );
    register_agent(&root, "scope", "master-old", "master", "master", None);
    let emitted = call(
        &root,
        json!({
            "op": "accumulate_wake",
            "master": { "scopeId": "scope", "sessionId": "master-old" },
            "signal": {
                "key": "bug:rebind-p0",
                "kind": "bug",
                "title": "rebind p0",
                "priority": "p0",
                "summary": "preserve message delivery identity",
                "issueId": "bug-p0-rebind",
                "source": { "scopeId": "scope", "sessionId": "master-old" },
                "observedAt": "2026-09-19T00:00:00.000Z"
            }
        }),
    );
    assert_eq!(emitted["idempotent"], false);
    let status = call(&root, json!({ "op": "status" }));
    let message_id = status["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["title"] == "rebind p0")
        .and_then(|message| message["messageId"].as_str())
        .unwrap()
        .to_owned();
    let (attempt_id, nonce) = delivery_attempt_fields(&root, &message_id);

    call(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "master-new" },
                "runtimeId": "runtime-scope"
            }
        }),
    );

    let receipt = call(
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
                    "durable": true,
                    "format": "jsonl",
                    "receiptId": "master-old-receipt"
                }
            }
        }),
    );
    assert_eq!(receipt["message"]["state"], "delivered");
    assert_eq!(receipt["message"]["messageId"], message_id);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn agent_rebind_rekeys_pending_coalesced_notifications_without_losing_identity() {
    let root = temp_root("agent-rebind-coalesced-notification");
    register_scope(
        &root,
        "scope",
        "app",
        "/project",
        &["master-old", "master-new", "peer"],
    );
    register_agent(&root, "scope", "master-old", "master", "master", None);
    register_agent(&root, "scope", "peer", "peer", "peer", None);
    call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "peer" },
                "title": "first pending",
                "priority": "p2",
                "body": "first update",
                "deliveryMode": "idle",
                "coalesceKey": "progress",
                "messageId": "pending-first",
                "createdAt": "2026-09-19T00:00:00.000Z"
            }
        }),
    );

    call(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "master-new" },
                "runtimeId": "runtime-scope"
            }
        }),
    );

    call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master-new" },
                "to": { "scopeId": "scope", "sessionId": "peer" },
                "title": "second pending",
                "priority": "p2",
                "body": "second update",
                "deliveryMode": "idle",
                "coalesceKey": "progress",
                "messageId": "pending-second",
                "createdAt": "2026-09-19T00:01:00.000Z"
            }
        }),
    );

    let status = call(&root, json!({ "op": "status" }));
    let pending = status["notificationProjection"]["pending"]
        .as_array()
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["messageId"], "pending-second");
    assert_eq!(pending[0]["recipient"]["sessionId"], "peer");

    let flushed = call(
        &root,
        json!({
            "op": "flush_notifications",
            "now": "2026-09-19T00:03:00.000Z"
        }),
    );
    assert_eq!(flushed["batches"].as_array().unwrap().len(), 1);
    assert_eq!(flushed["batches"][0]["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        flushed["batches"][0]["items"][0]["messageId"],
        "pending-second"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn agent_rebind_rejects_runtime_mismatch_without_mutation() {
    let root = temp_root("agent-rebind-runtime-mismatch");
    register_scope(
        &root,
        "scope",
        "app",
        "/project",
        &["master-old", "master-new"],
    );
    register_agent(&root, "scope", "master-old", "master", "master", None);
    let error = call_error(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "master-new" },
                "runtimeId": "runtime-forged"
            }
        }),
    );
    assert!(error.contains("agent_rebind_runtime_mismatch"), "{error}");
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["agents"].as_array().unwrap().len(), 1);
    assert_eq!(status["agents"][0]["sessionId"], "master-old");
    assert_eq!(status["scopes"][0]["masterSessionId"], "master-old");
    assert!(status["agentTombstones"].as_array().unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn agent_rebind_rejects_occupied_target_without_mutation() {
    let root = temp_root("agent-rebind-occupied");
    register_scope(&root, "scope", "app", "/project", &["master-old", "target"]);
    register_agent(&root, "scope", "master-old", "master", "master", None);
    register_agent(&root, "scope", "target", "target", "peer", None);
    let error = call_error(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "target" },
                "runtimeId": "runtime-scope"
            }
        }),
    );
    assert!(error.contains("agent_address_occupied"), "{error}");
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["agents"].as_array().unwrap().len(), 2);
    assert_eq!(status["scopes"][0]["masterSessionId"], "master-old");
    assert!(status["agentTombstones"].as_array().unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn agent_rebind_makes_old_address_read_only_and_new_address_sendable() {
    let root = temp_root("agent-rebind-send");
    register_scope(
        &root,
        "scope",
        "app",
        "/project",
        &["master-old", "master-new", "peer"],
    );
    register_agent(&root, "scope", "master-old", "master", "master", None);
    register_agent(&root, "scope", "peer", "peer", "peer", None);
    call(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "master-new" },
                "runtimeId": "runtime-scope"
            }
        }),
    );

    let old_send = call_error(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "peer" },
                "title": "old address",
                "priority": "p1",
                "body": "must be rejected"
            }
        }),
    );
    assert!(old_send.contains("agent_address_rebound"), "{old_send}");

    let new_send = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master-new" },
                "to": { "scopeId": "scope", "sessionId": "peer" },
                "title": "new address",
                "priority": "p1",
                "body": "must be accepted"
            }
        }),
    );
    assert_eq!(new_send["message"]["from"]["sessionId"], "master-new");
    let old_refresh = call_error(
        &root,
        json!({
            "op": "refresh_agent",
            "address": { "scopeId": "scope", "sessionId": "master-old" }
        }),
    );
    assert!(
        old_refresh.contains("agent_address_rebound"),
        "{old_refresh}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn agent_rebind_preserves_existing_subagent_parent_ancestry() {
    let root = temp_root("agent-rebind-parent");
    register_scope(
        &root,
        "scope",
        "app",
        "/project",
        &["master", "parent-old", "parent-new", "child"],
    );
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "parent-old", "parent", "peer", None);
    register_agent(
        &root,
        "scope",
        "child",
        "child",
        "subagent",
        Some(json!({ "scopeId": "scope", "sessionId": "parent-old" })),
    );
    call(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "parent-old" },
                "to": { "scopeId": "scope", "sessionId": "parent-new" },
                "runtimeId": "runtime-scope"
            }
        }),
    );

    let child_to_parent = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "child" },
                "to": { "scopeId": "scope", "sessionId": "parent-new" },
                "title": "child reply",
                "priority": "p1",
                "body": "preserve parent after rebind"
            }
        }),
    );
    assert_eq!(child_to_parent["message"]["state"], "accepted");
    let parent_to_child = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "parent-new" },
                "to": { "scopeId": "scope", "sessionId": "child" },
                "title": "parent dispatch",
                "priority": "p1",
                "body": "preserve child binding after rebind"
            }
        }),
    );
    assert_eq!(parent_to_child["message"]["state"], "accepted");
    let old_parent = call_error(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "parent-old" },
                "to": { "scopeId": "scope", "sessionId": "child" },
                "title": "old parent",
                "priority": "p1",
                "body": "must be rejected"
            }
        }),
    );
    assert!(old_parent.contains("agent_address_rebound"), "{old_parent}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn agent_rebind_migrates_all_active_dag_references() {
    let root = temp_root("agent-rebind-dag");
    register_scope(
        &root,
        "scope",
        "app",
        "/project",
        &["master-old", "master-new", "peer-old", "peer-new"],
    );
    register_agent(&root, "scope", "master-old", "master", "master", None);
    register_agent(&root, "scope", "peer-old", "peer", "peer", None);
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "peer-appserver",
                "kind": "appserver",
                "target": "mock://scope",
                "recipient": { "scopeId": "scope", "sessionId": "peer-old" }
            }
        }),
    );
    call(
        &root,
        json!({
            "op": "create_loop",
            "loop": {
                "loopId": "rebind-loop",
                "kind": "task",
                "owner": { "scopeId": "scope", "sessionId": "master-old" },
                "trigger": "event",
                "work": "work",
                "gate": "tests",
                "state": "persist",
                "stop": "done"
            }
        }),
    );
    let sent = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "peer-old" },
                "title": "rebind message",
                "priority": "p2",
                "body": "recover this message after the session changes",
                "messageId": "rebind-message",
                "coalesceKey": "rebind"
            }
        }),
    );
    assert_eq!(sent["idempotent"], false);
    call(
        &root,
        json!({
            "op": "report_bug",
            "bug": {
                "bugId": "rebind-bug",
                "scopeId": "scope",
                "title": "rebind bug",
                "priority": "p1",
                "description": "reporter must survive a session rebind",
                "reporter": { "scopeId": "scope", "sessionId": "peer-old" }
            }
        }),
    );
    call(
        &root,
        json!({
            "op": "flush_notifications",
            "now": "2999-01-01T00:00:00.000Z"
        }),
    );

    call(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "peer-old" },
                "to": { "scopeId": "scope", "sessionId": "peer-new" },
                "runtimeId": "runtime-scope"
            }
        }),
    );
    call(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "master-new" },
                "runtimeId": "runtime-scope"
            }
        }),
    );

    let advanced = call(
        &root,
        json!({
            "op": "advance_loop",
            "loopId": "rebind-loop",
            "complete": false
        }),
    );
    assert_eq!(advanced["loop"]["owner"]["sessionId"], "master-new");

    let recovered = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master-new" },
                "to": { "scopeId": "scope", "sessionId": "peer-new" },
                "title": "rebind message",
                "priority": "p2",
                "body": "recover this message after the session changes",
                "messageId": "rebind-message",
                "coalesceKey": "rebind"
            }
        }),
    );
    assert_eq!(recovered["idempotent"], true);
    assert_eq!(recovered["message"]["from"]["sessionId"], "master-new");
    assert_eq!(recovered["message"]["to"]["sessionId"], "peer-new");

    let adapter_send = call(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master-new" },
                "to": { "scopeId": "scope", "sessionId": "peer-new" },
                "title": "adapter rebind",
                "priority": "p1",
                "body": "adapter recipient follows the peer",
                "adapterId": "peer-appserver",
                "deliveryMode": "direct"
            }
        }),
    );
    assert_eq!(adapter_send["message"]["to"]["sessionId"], "peer-new");

    let closed = call(
        &root,
        json!({
            "op": "update_bug",
            "bugId": "rebind-bug",
            "status": "closed",
            "actor": { "scopeId": "scope", "sessionId": "master-new" },
            "evidence": { "fix": "commit", "verification": "tests", "merge": "main" }
        }),
    );
    assert_eq!(closed["bug"]["reporter"]["sessionId"], "peer-new");

    let status = call(&root, json!({ "op": "status" }));
    let adapter = status["adapters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|adapter| adapter["adapterId"] == "peer-appserver")
        .unwrap();
    assert_eq!(adapter["recipient"]["sessionId"], "peer-new");
    let notifications: Vec<&Value> = ["pending", "emitted", "unknown"]
        .into_iter()
        .flat_map(|state| status["notificationProjection"][state].as_array().unwrap())
        .collect();
    assert!(notifications
        .iter()
        .all(|notification| { notification["recipient"]["sessionId"] != "peer-old" }));
    assert!(notifications.iter().any(|notification| {
        notification["issueId"] == "rebind-bug"
            && notification["recipient"]["sessionId"] == "peer-new"
    }));
    assert!(status["notificationProjection"]["batches"]
        .as_array()
        .unwrap()
        .iter()
        .all(|batch| batch["recipient"]["sessionId"] != "peer-old"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tampered_agent_rebind_event_fails_closed_on_replay() {
    let root = temp_root("agent-rebind-tamper");
    register_scope(
        &root,
        "scope",
        "app",
        "/project",
        &["master-old", "master-new"],
    );
    register_agent(&root, "scope", "master-old", "master", "master", None);
    call(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "master-new" },
                "runtimeId": "runtime-scope"
            }
        }),
    );
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let mut lines: Vec<Value> = contents
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let rebound = lines
        .iter_mut()
        .find(|event| event["kind"] == "agent.rebound")
        .unwrap();
    rebound["data"]["to"]["agentId"] = json!("forged");
    let rewritten = lines
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&mailbox, format!("{rewritten}\n")).unwrap();
    let error = call_error(&root, json!({ "op": "status" }));
    assert!(error.contains("journal_corrupt"), "{error}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tampered_agent_rebind_address_fails_closed_on_replay() {
    let root = temp_root("agent-rebind-address-tamper");
    register_scope(&root, "scope", "app", "/project", &[]);
    register_agent(&root, "scope", "master-old", "master", "master", None);
    call(
        &root,
        json!({
            "op": "rebind_agent",
            "rebind": {
                "from": { "scopeId": "scope", "sessionId": "master-old" },
                "to": { "scopeId": "scope", "sessionId": "master-new" },
                "runtimeId": "runtime-scope"
            }
        }),
    );
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let mut lines: Vec<Value> = contents
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let rebound = lines
        .iter_mut()
        .find(|event| event["kind"] == "agent.rebound")
        .unwrap();
    rebound["data"]["to"]["sessionId"] = json!("");
    rebound["data"]["tombstone"]["reboundTo"]["sessionId"] = json!("");
    let rewritten = lines
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&mailbox, format!("{rewritten}\n")).unwrap();
    let error = call_error(&root, json!({ "op": "status" }));
    assert!(error.contains("journal_corrupt"), "{error}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_tmux_adapter_is_rejected_without_persisting_messages() {
    let root = temp_root("legacy-tmux-rejected");
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
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert!(rejected.contains("invalid_adapter_kind"), "{rejected}");
    let status = call(&root, json!({ "op": "status" }));
    assert!(status["messages"].as_array().unwrap().is_empty());
    assert!(status["notificationProjection"]["emitted"]
        .as_array()
        .unwrap()
        .is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reset_runtime_registry_cli_archives_legacy_and_registers_v3_runtime() {
    let root = temp_root("runtime-reset-cli");
    let host = root.join(".appsdk-host");
    fs::create_dir_all(&host).unwrap();
    fs::write(
        host.join("runtimes.jsonl"),
        b"not-json legacy tmux bytes\n{\"tmuxSession\":\"old\"}\n",
    )
    .unwrap();
    fs::write(host.join("projects.jsonl"), b"projects-sentinel\n").unwrap();
    fs::write(
        host.join("communication.jsonl"),
        b"communication-sentinel\n",
    )
    .unwrap();

    let no_discard = Command::new(binary())
        .args([
            "communication",
            "reset-runtime-registry",
            "--approval",
            "approved",
        ])
        .env("APPSDK_HOME", &host)
        .output()
        .unwrap();
    assert!(!no_discard.status.success());
    assert!(
        String::from_utf8_lossy(&no_discard.stderr)
            .contains("GLOBAL_RUNTIME_REGISTRY_RESET_REQUIRES_DISCARD_LEGACY"),
        "{}",
        String::from_utf8_lossy(&no_discard.stderr)
    );

    let missing_approval = Command::new(binary())
        .args([
            "communication",
            "reset-runtime-registry",
            "--discard-legacy",
        ])
        .env("APPSDK_HOME", &host)
        .output()
        .unwrap();
    assert!(!missing_approval.status.success());
    assert!(
        String::from_utf8_lossy(&missing_approval.stderr).contains("--approval"),
        "{}",
        String::from_utf8_lossy(&missing_approval.stderr)
    );
    assert_eq!(
        fs::read(host.join("runtimes.jsonl")).unwrap(),
        b"not-json legacy tmux bytes\n{\"tmuxSession\":\"old\"}\n"
    );

    let reset = Command::new(binary())
        .args([
            "communication",
            "reset-runtime-registry",
            "--discard-legacy",
            "--approval",
            "approved reset",
        ])
        .env("APPSDK_HOME", &host)
        .output()
        .unwrap();
    assert!(
        reset.status.success(),
        "{}",
        String::from_utf8_lossy(&reset.stderr)
    );
    let receipt: Value = serde_json::from_slice(&reset.stdout).unwrap();
    assert_eq!(receipt["idempotent"], false);
    assert_eq!(receipt["delivery_verified"], false);
    assert_eq!(fs::read(host.join("runtimes.jsonl")).unwrap(), b"");
    assert_eq!(
        fs::read(host.join("projects.jsonl")).unwrap(),
        b"projects-sentinel\n"
    );
    assert_eq!(
        fs::read(host.join("communication.jsonl")).unwrap(),
        b"communication-sentinel\n"
    );
    let archive_path = receipt["archivePath"].as_str().unwrap();
    assert_eq!(
        fs::read(Path::new(archive_path).join("runtimes.jsonl")).unwrap(),
        b"not-json legacy tmux bytes\n{\"tmuxSession\":\"old\"}\n"
    );

    let project_root = root.canonicalize().unwrap();
    let project_root = project_root.to_str().unwrap();
    call_with_host(
        &root,
        &host,
        json!({
            "op": "register_runtime",
            "runtime": {
                "runtimeId": "runtime-after-reset",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://after-reset",
                "projectRoot": project_root,
                "capabilities": ["send_message_to_thread"],
                "processId": std::process::id()
            }
        }),
    );
    let runtime = fs::read_to_string(host.join("runtimes.jsonl")).unwrap();
    assert!(runtime.contains("runtime.registered"), "{runtime}");
    assert!(!runtime.contains("tmuxSession"), "{runtime}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reset_runtime_registry_preserves_replay_of_retained_mailbox_attempt() {
    let root = temp_root("runtime-reset-retained-mailbox");
    let host = root.join(".appsdk-host");
    fs::create_dir_all(&host).unwrap();
    register_scope_with_host(&root, &host, "scope", "app", &["master", "worker"]);
    register_agent_with_host(&root, &host, "scope", "master", "master", "master", None);
    register_agent_with_host(&root, &host, "scope", "worker", "worker", "peer", None);

    let sent = call_with_host(
        &root,
        &host,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "master" },
                "to": { "scopeId": "scope", "sessionId": "worker" },
                "title": "survive runtime reset",
                "priority": "p1",
                "body": "retained mailbox must replay after the registry baseline reset"
            }
        }),
    );
    let message_id = sent["message"]["messageId"].as_str().unwrap();
    let attempt_id = sent["deliveryAttempt"]["attemptId"].as_str().unwrap();
    let nonce = sent["deliveryAttempt"]["nonce"].as_str().unwrap();
    call_with_host(
        &root,
        &host,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "attemptId": attempt_id,
                "nonce": nonce,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "durable": true, "format": "jsonl", "receiptId": "pre-reset" }
            }
        }),
    );

    let reset = Command::new(binary())
        .args([
            "communication",
            "reset-runtime-registry",
            "--discard-legacy",
            "--approval",
            "approved reset",
        ])
        .env("APPSDK_HOME", &host)
        .output()
        .unwrap();
    assert!(
        reset.status.success(),
        "{}",
        String::from_utf8_lossy(&reset.stderr)
    );
    let reset_receipt: Value = serde_json::from_slice(&reset.stdout).unwrap();
    assert_eq!(reset_receipt["delivery_verified"], false);
    assert!(reset_receipt["archivePath"].as_str().is_some());
    assert!(reset_receipt["transactionId"].as_str().is_some());

    let project_root = root.canonicalize().unwrap();
    let project_root = project_root.to_str().unwrap();
    call_with_host(
        &root,
        &host,
        json!({
            "op": "register_runtime",
            "runtime": {
                "runtimeId": "runtime-scope",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://scope",
                "projectRoot": project_root,
                "capabilities": ["send_message_to_thread"],
                "processId": std::process::id().saturating_add(1)
            }
        }),
    );

    let replayed = call_with_host(&root, &host, json!({ "op": "status" }));
    let message = replayed["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["messageId"] == message_id)
        .unwrap();
    assert_eq!(message["state"], "delivered");
    assert_eq!(message["evidence"][1]["details"]["attemptId"], attempt_id);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn appserver_adapter_requires_endpoint_and_declared_send_capability() {
    let root = temp_root("appserver-runtime-target");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let project_root = root.canonicalize().unwrap();
    let project_root = project_root.to_str().unwrap();

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
                "capabilities": [],
                "processId": std::process::id()
            }
        }),
    );

    let missing_capability = call_error(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "desktop-missing-capability",
                "kind": "appserver",
                "target": "mock://scope",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert!(
        missing_capability.contains("appserver_capability_missing"),
        "{missing_capability}"
    );

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
                "capabilities": ["send_message_to_thread"],
                "processId": std::process::id()
            }
        }),
    );

    let mismatch = call_error(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "desktop-mismatch",
                "kind": "appserver",
                "target": "mock://stale",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert!(mismatch.contains("appserver_target_mismatch"), "{mismatch}");

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
                "capabilities": [],
                "processId": std::process::id()
            }
        }),
    );

    let stale = call_error(
        &root,
        json!({
            "op": "send",
            "message": {
                "from": { "scopeId": "scope", "sessionId": "worker" },
                "to": { "scopeId": "scope", "sessionId": "master" },
                "title": "stale appserver capability",
                "priority": "p1",
                "body": "must not queue without the declared send capability",
                "deliveryMode": "direct",
                "adapterId": "desktop-bound",
                "messageId": "stale-appserver-capability"
            }
        }),
    );
    assert!(stale.contains("appserver_capability_missing"), "{stale}");
    let status = call(&root, json!({ "op": "status" }));
    assert!(status["messages"].as_array().unwrap().is_empty());
    fs::remove_dir_all(root).unwrap();
}
