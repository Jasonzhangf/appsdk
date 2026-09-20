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
