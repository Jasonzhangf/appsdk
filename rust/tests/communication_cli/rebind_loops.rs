use super::*;

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
