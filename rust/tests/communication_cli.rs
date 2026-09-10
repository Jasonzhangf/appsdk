use serde_json::{json, Value};
use std::fs;
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
        .output()
        .unwrap();
    assert!(!output.status.success());
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn register_scope(
    root: &Path,
    scope_id: &str,
    appserver_id: &str,
    project_root: &str,
    sessions: &[&str],
) {
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
                "sessionIds": sessions
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
        "role": role
    });
    if role == "master" {
        agent["masterGrant"] = json!("user approved master for this scope");
    }
    if let Some(parent) = parent {
        agent["parent"] = parent;
    }
    call(root, json!({ "op": "register_agent", "agent": agent }));
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
    assert_eq!(batch["batches"].as_array().unwrap().len(), 1);
    assert_eq!(batch["batches"][0]["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        batch["batches"][0]["items"][0]["title"],
        "worker idle: worker"
    );
    assert!(batch["batches"][0]["items"][0].get("body").is_none());
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
    assert_eq!(raw.matches("notification.queued").count(), 1);
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
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(
        status["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
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
