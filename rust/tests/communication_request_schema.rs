use jsonschema::validator_for;
use serde_json::{json, Value};

const SCHEMA_SOURCE: &str =
    include_str!("../../contracts/communication/communication-request.schema.json");

fn validator() -> jsonschema::Validator {
    let schema: Value =
        serde_json::from_str(SCHEMA_SOURCE).expect("communication request schema must parse");
    validator_for(&schema).expect("schema must be a valid Draft 2020-12 document")
}

fn address() -> Value {
    json!({ "scopeId": "scope", "sessionId": "agent" })
}

#[test]
fn schema_rejects_unknown_root_fields() {
    let v = validator();
    let request = json!({ "op": "status", "surprise": true });
    let err = v
        .iter_errors(&request)
        .next()
        .expect("unknown root field must fail validation");
    let message = err.to_string();
    let message_lower = message.to_ascii_lowercase();
    assert!(
        message_lower.contains("additional") || message_lower.contains("unknown"),
        "unexpected error: {message}"
    );
}

#[test]
fn schema_rejects_complete_non_boolean() {
    let v = validator();
    let request = json!({
        "op": "advance_loop",
        "loopId": "loop-1",
        "complete": "true"
    });
    let err = v
        .iter_errors(&request)
        .next()
        .expect("non-boolean complete must fail validation");
    let message = err.to_string();
    assert!(message.contains("boolean"), "unexpected error: {message}");
}

#[test]
fn schema_rejects_unknown_status_value() {
    let v = validator();
    let request = json!({
        "op": "update_bug",
        "bugId": "bug-1",
        "status": "pending",
        "actor": address()
    });
    let err = v
        .iter_errors(&request)
        .next()
        .expect("status outside the declared enum must fail validation");
    let message = err.to_string();
    assert!(
        message.contains("active") && message.contains("closed"),
        "unexpected error: {message}"
    );
}

#[test]
fn schema_accepts_update_bug_snake_case_alias() {
    let v = validator();
    let request = json!({
        "op": "update_bug",
        "bug_id": "bug-1",
        "status": "active",
        "actor": address()
    });
    assert!(v.is_valid(&request));
}

#[test]
fn schema_accepts_advance_loop_snake_case_alias() {
    let v = validator();
    let request = json!({
        "op": "advance_loop",
        "loop_id": "loop-1",
        "complete": false
    });
    assert!(v.is_valid(&request));
}

#[test]
fn schema_accepts_record_error_snake_case_loop_alias() {
    let v = validator();
    let request = json!({
        "op": "record_error",
        "code": "boom",
        "message": "failed",
        "loop_id": "loop-1"
    });
    assert!(v.is_valid(&request));
}

#[test]
fn schema_rejects_update_bug_without_identifier() {
    let v = validator();
    let request = json!({
        "op": "update_bug",
        "status": "active",
        "actor": address()
    });
    assert!(!v.is_valid(&request));
}

#[test]
fn schema_rejects_advance_loop_without_identifier() {
    let v = validator();
    assert!(!v.is_valid(&json!({
        "op": "advance_loop",
        "complete": false
    })));
}

#[test]
fn schema_rejects_string_message_for_send() {
    let v = validator();
    assert!(!v.is_valid(&json!({
        "op": "send",
        "message": "not an object"
    })));
}

#[test]
fn schema_rejects_object_message_for_record_error() {
    let v = validator();
    assert!(!v.is_valid(&json!({
        "op": "record_error",
        "code": "boom",
        "message": { "text": "not a string" }
    })));
}

#[test]
fn schema_accepts_register_runtime_in_nested_and_flat_shapes() {
    let v = validator();
    let nested = json!({
        "op": "register_runtime",
        "runtime": {
            "runtimeId": "runtime-1",
            "appserverId": "app",
            "namespace": "codex_app",
            "endpoint": "mock://runtime",
            "projectRoot": "/tmp/project",
            "processId": 1
        }
    });
    assert!(v.is_valid(&nested));
    let flat = json!({
        "op": "register-runtime",
        "runtimeId": "runtime-1",
        "appserverId": "app",
        "namespace": "codex_app",
        "endpoint": "mock://runtime",
        "projectRoot": "/tmp/project",
        "processId": 1
    });
    assert!(v.is_valid(&flat));
}

#[test]
fn schema_accepts_send_in_nested_and_flat_shapes() {
    let v = validator();
    let nested = json!({
        "op": "send",
        "message": {
            "from": address(),
            "to": address(),
            "title": "hi",
            "priority": "p1",
            "body": "hello"
        }
    });
    assert!(v.is_valid(&nested));
    let flat = json!({
        "op": "send",
        "from": address(),
        "to": address(),
        "title": "hi",
        "priority": "p1",
        "body": "hello"
    });
    assert!(v.is_valid(&flat));
}

#[test]
fn schema_accepts_record_delivery_flat_with_required_fields() {
    let v = validator();
    let flat = json!({
        "op": "record_delivery",
        "messageId": "msg-1",
        "attemptId": "attempt-1",
        "nonce": "nonce-1",
        "state": "delivered",
        "runtimeId": "runtime-1",
        "evidence": { "ok": true }
    });
    assert!(v.is_valid(&flat));
}

#[test]
fn schema_accepts_wake_signals_in_nested_and_flat_shapes() {
    let v = validator();
    let nested = json!({
        "op": "accumulate_wake",
        "master": address(),
        "signal": {
            "key": "k",
            "kind": "goal",
            "title": "wake",
            "priority": "p1",
            "summary": "deadline reached"
        }
    });
    assert!(v.is_valid(&nested));
    let flat = json!({
        "op": "record_wake",
        "address": address(),
        "key": "k",
        "kind": "goal",
        "title": "wake",
        "priority": "p1",
        "summary": "deadline reached"
    });
    assert!(v.is_valid(&flat));
}

#[test]
fn schema_rejects_unknown_nested_field_inside_message() {
    let v = validator();
    let bad = json!({
        "op": "send",
        "message": {
            "from": address(),
            "to": address(),
            "title": "hi",
            "priority": "p1",
            "body": "hello",
            "surprise": 1
        }
    });
    assert!(!v.is_valid(&bad));
}

#[test]
fn schema_accepts_status_request_with_no_extra_fields() {
    let v = validator();
    assert!(v.is_valid(&json!({ "op": "status" })));
}
