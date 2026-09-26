use jsonschema::validator_for;
use serde_json::{json, Value};

const SCHEMA_SOURCE: &str =
    include_str!("../../contracts/communication/communication-event.schema.json");

fn validator() -> jsonschema::Validator {
    let schema: Value =
        serde_json::from_str(SCHEMA_SOURCE).expect("communication event schema must parse");
    validator_for(&schema).expect("schema must be a valid Draft 2020-12 document")
}

fn scope_record() -> Value {
    json!({
        "scopeId": "scope",
        "appserverId": "appserver",
        "namespace": "codex_app",
        "endpoint": "appserver://runtime",
        "projectRoot": "/project",
        "sessionIds": ["session"],
        "registeredAt": "2026-09-20T00:00:00Z",
        "lastObservedAt": "2026-09-20T00:00:00Z",
        "masterSessionId": null,
        "runtimeId": "runtime"
    })
}

fn agent_record() -> Value {
    json!({
        "scopeId": "scope",
        "sessionId": "session",
        "agentId": "agent",
        "role": "peer",
        "masterGrant": null,
        "parent": null,
        "leaseMs": 1000,
        "registeredAt": "2026-09-20T00:00:00Z",
        "lastObservedAt": "2026-09-20T00:00:00Z",
        "expiresAt": "2026-09-21T00:00:00Z",
        "state": "working",
        "lastStateAt": "2026-09-20T00:00:00Z",
        "runtimeId": "runtime"
    })
}

fn adapter_record() -> Value {
    json!({
        "adapterId": "mailbox",
        "kind": "mailbox",
        "target": null,
        "enabled": true,
        "execute": false,
        "recipient": null,
        "registeredAt": "2026-09-20T00:00:00Z"
    })
}

fn message_record() -> Value {
    json!({
        "protocol": "appsdk-comm/v1",
        "messageId": "message",
        "conversationId": "conversation",
        "from": {"scopeId": "scope", "sessionId": "sender"},
        "to": {"scopeId": "scope", "sessionId": "recipient"},
        "title": "title",
        "priority": "p1",
        "body": "body",
        "deliveryMode": "direct",
        "coalesceKey": null,
        "issueId": null,
        "adapterId": "mailbox",
        "deliveryAttemptRequired": true,
        "createdAt": "2026-09-20T00:00:00Z",
        "state": "created",
        "evidence": [],
        "route": {
            "mode": "local",
            "sameAppserver": true,
            "sameProject": true,
            "sourceRole": "peer",
            "targetRole": "peer"
        },
        "lastError": null
    })
}

fn loop_record() -> Value {
    json!({
        "loopId": "loop",
        "kind": "task",
        "owner": {"scopeId": "scope", "sessionId": "owner"},
        "trigger": "manual",
        "work": "work",
        "gate": "gate",
        "state": "state",
        "stop": "stop",
        "maxIterations": 1,
        "deadlineAt": null,
        "phase": "discover",
        "status": "active",
        "iteration": 0,
        "createdAt": "2026-09-20T00:00:00Z",
        "updatedAt": "2026-09-20T00:00:00Z"
    })
}

fn wakeup_record() -> Value {
    json!({
        "address": {"scopeId": "scope", "sessionId": "master"},
        "idleSince": "2026-09-20T00:00:00Z",
        "remindersSent": 0,
        "nextDueAt": "2026-09-20T00:02:00Z",
        "stopped": false,
        "lastReminderAt": null
    })
}

fn notification_record() -> Value {
    json!({
        "notificationId": "notification",
        "messageId": "message",
        "generation": 0,
        "recipient": {"scopeId": "scope", "sessionId": "master"},
        "title": "title",
        "priority": "p1",
        "issueId": null,
        "coalesceKey": null,
        "body": "body",
        "createdAt": "2026-09-20T00:00:00Z",
        "availableAt": "2026-09-20T00:00:00Z",
        "status": "pending",
        "emittedAt": null,
        "adapterId": "mailbox",
        "transportReceipt": null,
        "lastError": null
    })
}

fn bug_record() -> Value {
    json!({
        "bugId": "bug",
        "scopeId": "scope",
        "title": "title",
        "priority": "p1",
        "description": "description",
        "reporter": {"scopeId": "scope", "sessionId": "reporter"},
        "status": "active",
        "worktreeId": null,
        "loopId": "bug-loop-bug",
        "createdAt": "2026-09-20T00:00:00Z",
        "updatedAt": "2026-09-20T00:00:00Z",
        "resolutionEvidence": null
    })
}

fn event(kind: &str, data: Value) -> Value {
    json!({
        "protocol": "appsdk-comm/v1",
        "eventId": "event-1",
        "at": "2026-09-20T00:00:00Z",
        "kind": kind,
        "data": data
    })
}

#[test]
fn every_declared_event_kind_rejects_untyped_empty_data() {
    let validator = validator();
    for kind in [
        "scope.registered",
        "scope.unregistered",
        "adapter.registered",
        "agent.registered",
        "agent.refreshed",
        "agent.rebound",
        "discovery.pending",
        "discovery.reconciled",
        "agent.state",
        "message.created",
        "message.delivery_attempt",
        "message.state",
        "notification.queued",
        "notification.superseded",
        "notification.delivery_attempt",
        "notification.emitted",
        "notification.batch_emitted",
        "notification.delivery_failed",
        "wakeup.updated",
        "wakeup.reminder",
        "master_wake.updated",
        "master_wake.briefing",
        "master_wake.decided",
        "bug.reported",
        "bug.updated",
        "loop.created",
        "loop.updated",
        "error.recorded",
    ] {
        assert!(
            !validator.is_valid(&event(kind, json!({}))),
            "{kind} accepted untyped empty data"
        );
    }
}

#[test]
fn schema_accepts_typed_lifecycle_event_data() {
    let validator = validator();
    let cases = [
        ("scope.registered", scope_record()),
        ("scope.unregistered", json!({"scopeId": "scope"})),
        ("adapter.registered", adapter_record()),
        ("agent.registered", agent_record()),
        ("agent.refreshed", agent_record()),
        (
            "agent.state",
            json!({
                "address": {"scopeId": "scope", "sessionId": "session"},
                "state": "working",
                "at": "2026-09-20T00:00:00Z"
            }),
        ),
        ("message.created", message_record()),
        ("wakeup.updated", wakeup_record()),
        (
            "wakeup.reminder",
            json!({
                "wakeup": wakeup_record(),
                "message": message_record(),
                "notificationKey": "notification-key",
                "notification": notification_record(),
                "attemptId": null,
                "receipt": null
            }),
        ),
        ("bug.reported", bug_record()),
        (
            "bug.updated",
            json!({
                "bug": bug_record(),
                "loop": loop_record(),
                "evidence": null
            }),
        ),
    ];
    for (kind, data) in cases {
        assert!(validator.is_valid(&event(kind, data)), "{kind} rejected");
    }
}

#[test]
fn schema_accepts_discovery_pending_scope_agent_and_rebind() {
    let validator = validator();
    let pending_scope = event(
        "discovery.pending",
        json!({
            "pendingId": "discovery-1",
            "operation": {"operation": "scope", "record": scope_record()},
            "createdAt": "2026-09-20T00:00:00Z"
        }),
    );
    assert!(validator.is_valid(&pending_scope));

    let pending_agent = event(
        "discovery.pending",
        json!({
            "pendingId": "discovery-2",
            "operation": {"operation": "agent", "record": agent_record()},
            "createdAt": "2026-09-20T00:00:00Z"
        }),
    );
    assert!(validator.is_valid(&pending_agent));

    let rebound = json!({
        "from": agent_record(),
        "to": {
            "scopeId": "scope",
            "sessionId": "session-new",
            "agentId": "agent",
            "role": "peer",
            "masterGrant": null,
            "parent": null,
            "leaseMs": 1000,
            "registeredAt": "2026-09-20T00:00:00Z",
            "lastObservedAt": "2026-09-20T00:00:00Z",
            "expiresAt": "2026-09-21T00:00:00Z",
            "state": "working",
            "lastStateAt": "2026-09-20T00:00:00Z",
            "runtimeId": "runtime"
        },
        "tombstone": {
            "address": {"scopeId": "scope", "sessionId": "session"},
            "agentId": "agent",
            "runtimeId": "runtime",
            "reboundTo": {"scopeId": "scope", "sessionId": "session-new"},
            "reboundAt": "2026-09-20T00:00:00Z"
        }
    });
    let pending_rebind = event(
        "discovery.pending",
        json!({
            "pendingId": "discovery-3",
            "operation": {"operation": "rebind", "event": rebound},
            "createdAt": "2026-09-20T00:00:00Z"
        }),
    );
    assert!(validator.is_valid(&pending_rebind));
}

#[test]
fn schema_accepts_discovery_reconciled_and_rejects_unknown_fields() {
    let validator = validator();
    let reconciled = event("discovery.reconciled", json!({"pendingId": "discovery-1"}));
    assert!(validator.is_valid(&reconciled));

    let unknown = event(
        "discovery.reconciled",
        json!({"pendingId": "discovery-1", "unexpected": true}),
    );
    assert!(!validator.is_valid(&unknown));
}

#[test]
fn schema_accepts_both_notification_supersede_reasons() {
    let validator = validator();
    for reason in ["master_wake_briefing", "master_wake_decision"] {
        assert!(
            validator.is_valid(&event(
                "notification.superseded",
                json!({"keys": ["notification-1"], "generation": 0, "reason": reason})
            )),
            "notification.superseded rejected {reason}"
        );
    }
    assert!(!validator.is_valid(&event(
        "notification.superseded",
        json!({"keys": ["notification-1"], "generation": 0, "reason": "other"})
    )));
}
