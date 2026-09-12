use chrono::{DateTime, Duration, SecondsFormat};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
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
    let output = Command::new(binary())
        .args([
            "communication",
            root.to_str().unwrap(),
            "--json",
            &serde_json::to_string(&request).unwrap(),
        ])
        .env("APPSDK_HOME", root.join(".appsdk-host"))
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
    let output = Command::new(binary())
        .args([
            "communication",
            root.to_str().unwrap(),
            "--json",
            &serde_json::to_string(&request).unwrap(),
        ])
        .env("APPSDK_HOME", root.join(".appsdk-host"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    String::from_utf8_lossy(&output.stderr).into_owned()
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
    project_root: &str,
    sessions: &[&str],
) {
    let runtime_id = format!("runtime-{scope_id}");
    call(
        root,
        json!({
            "op": "register_runtime",
            "runtime": {
                "runtimeId": runtime_id,
                "appserverId": appserver_id,
                "namespace": "codex_tui",
                "endpoint": format!("mock://{scope_id}"),
                "projectRoot": project_root,
                "processId": std::process::id()
            }
        }),
    );
    call(
        root,
        json!({
            "op": "register_scope",
            "scope": {
                "scopeId": scope_id,
                "appserverId": appserver_id,
                "namespace": "codex_tui",
                "endpoint": format!("mock://{scope_id}"),
                "projectRoot": project_root,
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
    call(root, json!({ "op": "register_agent", "agent": agent }));
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
    let missing_runtime = call_error(
        &root,
        json!({
            "op": "register_scope",
            "scope": {
                "scopeId": "scope",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://scope",
                "projectRoot": "/project",
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
                "projectRoot": "/project",
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
                "projectRoot": "/project",
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
                "projectRoot": "/project",
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
                "projectRoot": "/project",
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
    let forged = call_error(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "state": "delivered",
                "runtimeId": "runtime-forged",
                "evidence": { "receiptId": "forged" }
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
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "receiptId": "target-received" }
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
                "projectRoot": "/project",
                "tmuxSession": "tui-scope",
                "tmuxPane": "%7",
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
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "receiptId": "target-received" }
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
                "state": "executed",
                "runtimeId": "runtime-scope",
                "evidence": { "receiptId": "target-executed" }
            }
        }),
    );
    let unknown_after_delivery = call_error(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "state": "unknown",
                "runtimeId": "runtime-scope",
                "evidence": { "receiptId": "delivery-observation-lost" }
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
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "receiptId": "too-early" }
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
    call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "receiptId": "worker-received" }
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
    call(
        &root,
        json!({
            "op": "record_delivery",
            "delivery": {
                "messageId": message_id,
                "state": "delivered",
                "runtimeId": "runtime-scope",
                "evidence": { "receiptId": "master-received" }
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
fn adapters_are_explicit_and_receipts_are_replayed() {
    let root = temp_root("adapters");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);

    let registered = call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "tmux-preview",
                "kind": "tmux",
                "target": "preview-pane",
                "execute": false,
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert_eq!(registered["adapter"]["kind"], "tmux");
    let appserver = call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "desktop-host",
                "kind": "appserver",
                "target": "appserver://desktop",
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
                "adapterId": "tmux-preview",
                "messageId": "message-tmux-preview"
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
    assert_eq!(emitted[0]["transportReceipt"]["adapterId"], "tmux-preview");
    assert_eq!(emitted[0]["transportReceipt"]["state"], "intent");
    assert_eq!(
        emitted[0]["transportReceipt"]["evidence"]["executed"],
        false
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
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "tmux-execute",
                "kind": "tmux",
                "target": "missing-pane",
                "execute": true,
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
                "title": "will fail",
                "priority": "p1",
                "body": "target does not exist",
                "deliveryMode": "direct",
                "adapterId": "tmux-execute",
                "messageId": "message-tmux-failure"
            }
        }),
    );
    assert!(error.contains("tmux_delivery_failed"), "{error}");

    let status = call(&root, json!({ "op": "status" }));
    let pending = status["notificationProjection"]["pending"]
        .as_array()
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["lastError"]["code"], "tmux_delivery_failed");
    assert!(status["notificationProjection"]["emitted"]
        .as_array()
        .unwrap()
        .is_empty());
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
                "kind": "tmux",
                "target": "preview-pane",
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
                "adapterId": "tmux-preview",
                "kind": "tmux",
                "target": "preview-pane",
                "execute": false,
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    for (message_id, adapter_id) in [
        ("mailbox-message", "mailbox"),
        ("tmux-message", "tmux-preview"),
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
    assert_eq!(retry["notification"]["message"]["state"], "accepted");
    let message_id = retry["notification"]["message"]["messageId"]
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
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "tmux-execute",
                "kind": "tmux",
                "target": "missing-pane",
                "execute": true,
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    for (message_id, body) in [("direct-1", "first"), ("direct-2", "second")] {
        let error = call_error(
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
                    "adapterId": "tmux-execute",
                    "messageId": message_id
                }
            }),
        );
        assert!(error.contains("tmux_delivery_failed"), "{error}");
    }
    let status = call(&root, json!({ "op": "status" }));
    let pending = status["notificationProjection"]["pending"]
        .as_array()
        .unwrap();
    assert_eq!(pending.len(), 2);
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
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "tmux-execute",
                "kind": "tmux",
                "target": "missing-pane",
                "execute": true,
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    let request = json!({
        "op": "send",
        "message": {
            "from": { "scopeId": "scope", "sessionId": "worker" },
            "to": { "scopeId": "scope", "sessionId": "master" },
            "title": "retry direct",
            "priority": "p1",
            "body": "known transport failure",
            "deliveryMode": "direct",
            "adapterId": "tmux-execute",
            "messageId": "retry-direct"
        }
    });
    let first = call_error(&root, request.clone());
    assert!(first.contains("tmux_delivery_failed"), "{first}");
    let second = call_error(&root, request);
    assert!(second.contains("tmux_delivery_failed"), "{second}");

    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(
        status["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(status["notificationProjection"]["unknown"]
        .as_array()
        .unwrap()
        .is_empty());
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        2
    );
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_failed\"")
            .count(),
        2
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
                "adapterId": "master-pane",
                "kind": "tmux",
                "target": "master-pane",
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
                "adapterId": "master-pane"
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
