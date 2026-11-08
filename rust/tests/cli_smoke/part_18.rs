fn dagpipe_message_delivery_attempt_event(event_id: &str, at: &str, message_id: &str) -> Value {
    serde_json::json!({
        "protocol": "appsdk-comm/v1",
        "eventId": event_id,
        "at": at,
        "kind": "message.delivery_attempt",
        "data": {
            "messageId": message_id,
            "attempt": {
                "attemptId": format!("attempt-{message_id}"),
                "messageId": message_id,
                "operation": "message.delivery",
                "adapterId": "adapter-1",
                "runtimeId": "runtime-1",
                "runtimeFingerprint": "fingerprint-1",
                "target": "recipient",
                "nonce": "nonce-1",
                "startedAt": at
            }
        }
    })
}

fn dagpipe_notification_failed_event(
    event_id: &str,
    at: &str,
    keys: &[&str],
    operation: &str,
    attempt_id: &str,
) -> Value {
    serde_json::json!({
        "protocol": "appsdk-comm/v1",
        "eventId": event_id,
        "at": at,
        "kind": "notification.delivery_failed",
        "data": {
            "keys": keys,
            "adapterId": "adapter-1",
            "operation": operation,
            "error": {
                "code": "transport_unavailable",
                "message": "retry later",
                "context": {},
                "at": at
            },
            "attemptId": attempt_id
        }
    })
}

fn dagpipe_notification_failed_event_without_attempt(
    event_id: &str,
    at: &str,
    keys: &[&str],
    operation: &str,
) -> Value {
    serde_json::json!({
        "protocol": "appsdk-comm/v1",
        "eventId": event_id,
        "at": at,
        "kind": "notification.delivery_failed",
        "data": {
            "keys": keys,
            "adapterId": "adapter-1",
            "operation": operation,
            "error": {
                "code": "transport_unavailable",
                "message": "retry later",
                "context": {},
                "at": at
            }
        }
    })
}

#[test]
fn dagpipe_validate_notifications_reads_real_mailbox_objects() {
    let root = temp_root("dagpipe-notification-objects");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:00:02.5Z",
                "attempt-1",
                &["notification-1"],
                "notification.emitted",
            ),
            dagpipe_notification_emitted_event(
                "event-emitted",
                "2026-01-01T00:00:03Z",
                "attempt-1",
                &["notification-1"],
            ),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["single_source_single_sink"], Value::Bool(true));
    assert_eq!(result["objects"].as_array().unwrap().len(), 1);
    assert_eq!(result["objects"][0]["key"], "notification-1");
    assert_eq!(
        result["objects"][0]["terminal"]["kind"],
        "notification.emitted"
    );
    fs::remove_dir_all(root).unwrap();
}

fn write_dagpipe_notification_fixture(root: &std::path::Path, events: &[Value]) {
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    let mut raw = events
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    if !raw.is_empty() {
        raw.push('\n');
    }
    fs::write(communication.join("mailbox.jsonl"), raw).unwrap();
}

fn dagpipe_notification_graph(steps: &[&str], omit_terminal: bool) -> Value {
    let mut current_input = "notification_object";
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        let (operator, step_output) = match *step {
            "source_validate" => (
                "appsdk.communication.notification_source_validate",
                "notification_source_checked",
            ),
            "delivery_validate" => (
                "appsdk.communication.notification_delivery_validate",
                "notification_delivery_checked",
            ),
            "terminal_accept" => (
                "appsdk.communication.notification_terminal_accept",
                "notification_terminal",
            ),
            other => panic!("unknown notification graph step {other}"),
        };
        let output = if index + 1 == steps.len() {
            "notification_terminal"
        } else {
            step_output
        };
        let output_selector = if omit_terminal && index + 1 == steps.len() {
            serde_json::json!({"include": [], "exclude": ["terminal"], "predicate": null})
        } else {
            serde_json::json!({"include": [], "exclude": [], "predicate": null})
        };
        nodes.push(serde_json::json!({
            "id": step,
            "operator": operator,
            "operator_version": "1",
            "inputs": [current_input],
            "output": {"id": output, "schema": "Any"},
            "input_selector": {"include": [], "exclude": [], "predicate": null},
            "output_selector": output_selector,
            "iterator": "Whole"
        }));
        if index > 0 {
            edges.push(serde_json::json!({
                "from": steps[index - 1],
                "to": step,
                "arc_id": current_input
            }));
        }
        current_input = output;
    }
    serde_json::json!({
        "id": "appsdk-notification-object",
        "version": "0.2.0",
        "inputs": [{"id": "notification_object", "schema": "Any"}],
        "nodes": nodes,
        "edges": edges,
        "outputs": [current_input]
    })
}

fn write_dagpipe_notification_graph(root: &std::path::Path, graph: &Value) -> String {
    let path = root.join("notification.graph.json");
    fs::write(&path, serde_json::to_vec_pretty(graph).unwrap()).unwrap();
    "notification.graph.json".to_owned()
}

fn dagpipe_valid_notification_events() -> Vec<Value> {
    vec![
        dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
        dagpipe_message_state_event(
            "event-accepted",
            "2026-01-01T00:00:01Z",
            "message-1",
            "accepted",
        ),
        dagpipe_notification_queued(
            "event-queued",
            "2026-01-01T00:00:02Z",
            "notification-1",
            "notification-id-1",
            "message-1",
            0,
        ),
        dagpipe_notification_attempt_event(
            "event-attempt",
            "2026-01-01T00:00:03Z",
            "attempt-1",
            &["notification-1"],
            "notification.emitted",
        ),
        dagpipe_notification_emitted_event(
            "event-emitted",
            "2026-01-01T00:00:04Z",
            "attempt-1",
            &["notification-1"],
        ),
    ]
}

#[test]
fn dagpipe_validate_notifications_reports_default_and_explicit_evidence_scope() {
    let root = temp_root("dagpipe-notification-evidence-scope");
    write_dagpipe_notification_fixture(&root, &dagpipe_valid_notification_events());
    let default = run(&["dagpipe", "validate-notifications", root.to_str().unwrap()]);
    assert!(
        default.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&default.stderr)
    );
    let default: Value = serde_json::from_slice(&default.stdout).unwrap();
    assert_eq!(default["evidence_scope"], "sdk_default_contract");
    assert_eq!(default["graph"]["version"], "0.2.0");

    let graph = dagpipe_notification_graph(
        &["source_validate", "delivery_validate", "terminal_accept"],
        false,
    );
    let graph_path = write_dagpipe_notification_graph(&root, &graph);
    let explicit = run(&[
        "dagpipe",
        "validate-notifications",
        root.to_str().unwrap(),
        "--graph",
        &graph_path,
    ]);
    assert!(
        explicit.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&explicit.stderr)
    );
    let explicit: Value = serde_json::from_slice(&explicit.stdout).unwrap();
    assert_eq!(explicit["evidence_scope"], "selected_project_program");
    assert_eq!(explicit["objects"][0]["terminal"], default["objects"][0]["terminal"]);
}

#[test]
fn dagpipe_validate_notifications_graph_order_changes_first_business_error() {
    let root = temp_root("dagpipe-notification-double-error");
    write_dagpipe_notification_fixture(
        &root,
        &[
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_emitted_event(
                "event-emitted",
                "2026-01-01T00:00:04Z",
                "attempt-1",
                &["notification-1"],
            ),
        ],
    );
    let a = dagpipe_notification_graph(
        &["source_validate", "delivery_validate", "terminal_accept"],
        false,
    );
    let b = dagpipe_notification_graph(
        &["delivery_validate", "source_validate", "terminal_accept"],
        false,
    );
    let a_path = write_dagpipe_notification_graph(&root, &a);
    let a_output = run(&[
        "dagpipe",
        "validate-notifications",
        root.to_str().unwrap(),
        "--graph",
        &a_path,
    ]);
    assert_eq!(a_output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&a_output.stderr)
            .contains("NOTIFICATION_OBJECT_MESSAGE_ACCEPT_MISSING:notification-1")
    );

    let b_path = write_dagpipe_notification_graph(&root, &b);
    let b_output = run(&[
        "dagpipe",
        "validate-notifications",
        root.to_str().unwrap(),
        "--graph",
        &b_path,
    ]);
    assert_eq!(b_output.status.code(), Some(1));
    let b_error = String::from_utf8_lossy(&b_output.stderr);
    assert!(b_error.contains("NOTIFICATION_TERMINAL_INVALID:notification-1"));
    assert!(b_error.contains("attempt_missing"));
    assert!(!b_error.contains("NOTIFICATION_OBJECT_MESSAGE_ACCEPT_MISSING"));
}

#[test]
fn dagpipe_validate_notifications_single_delivery_error_same_for_a_and_b() {
    let root = temp_root("dagpipe-notification-single-delivery-error");
    write_dagpipe_notification_fixture(
        &root,
        &[
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_emitted_event(
                "event-emitted",
                "2026-01-01T00:00:03Z",
                "attempt-1",
                &["notification-1"],
            ),
        ],
    );
    let a = dagpipe_notification_graph(
        &["source_validate", "delivery_validate", "terminal_accept"],
        false,
    );
    let b = dagpipe_notification_graph(
        &["delivery_validate", "source_validate", "terminal_accept"],
        false,
    );
    for graph in [&a, &b] {
        let graph_path = write_dagpipe_notification_graph(&root, graph);
        let output = run(&[
            "dagpipe",
            "validate-notifications",
            root.to_str().unwrap(),
            "--graph",
            &graph_path,
        ]);
        assert_eq!(output.status.code(), Some(1));
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("NOTIFICATION_TERMINAL_INVALID:notification-1"));
        assert!(error.contains("attempt_missing:attempt-1"));
        assert!(!error.contains("NOTIFICATION_OBJECT_MESSAGE_ACCEPT_MISSING"));
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_explicit_incomplete_graph_success_is_not_default_acceptance() {
    let root = temp_root("dagpipe-notification-incomplete-program");
    write_dagpipe_notification_fixture(
        &root,
        &[
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:00:03Z",
                "attempt-1",
                &["notification-1"],
                "notification.emitted",
            ),
            dagpipe_notification_emitted_event(
                "event-emitted",
                "2026-01-01T00:00:04Z",
                "attempt-1",
                &["notification-1"],
            ),
        ],
    );
    let default = run(&["dagpipe", "validate-notifications", root.to_str().unwrap()]);
    assert_eq!(default.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&default.stderr)
            .contains("NOTIFICATION_OBJECT_MESSAGE_ACCEPT_MISSING:notification-1")
    );

    let incomplete = dagpipe_notification_graph(&["delivery_validate", "terminal_accept"], false);
    let incomplete_path = write_dagpipe_notification_graph(&root, &incomplete);
    let selected = run(&[
        "dagpipe",
        "validate-notifications",
        root.to_str().unwrap(),
        "--graph",
        &incomplete_path,
    ]);
    assert!(
        selected.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let selected: Value = serde_json::from_slice(&selected.stdout).unwrap();
    assert_eq!(selected["evidence_scope"], "selected_project_program");
    assert_eq!(selected["objects"][0]["terminal"]["kind"], "notification.emitted");
}

#[test]
fn dagpipe_validate_notifications_explicit_terminal_rejection_can_be_omitted() {
    let root = temp_root("dagpipe-notification-incomplete-terminal");
    write_dagpipe_notification_fixture(
        &root,
        &[
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
        ],
    );
    let default = run(&["dagpipe", "validate-notifications", root.to_str().unwrap()]);
    assert_eq!(default.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&default.stderr)
            .contains("NOTIFICATION_OBJECT_TERMINAL_MISSING:notification-1")
    );

    let incomplete = dagpipe_notification_graph(&["source_validate", "delivery_validate"], false);
    let incomplete_path = write_dagpipe_notification_graph(&root, &incomplete);
    let selected = run(&[
        "dagpipe",
        "validate-notifications",
        root.to_str().unwrap(),
        "--graph",
        &incomplete_path,
    ]);
    assert!(
        selected.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let selected: Value = serde_json::from_slice(&selected.stdout).unwrap();
    assert_eq!(selected["evidence_scope"], "selected_project_program");
    assert_eq!(selected["objects"][0]["terminal"]["mailbox_only"], true);
    assert_eq!(selected["objects"][0]["terminal"]["repair_required"], true);
}

#[test]
fn dagpipe_validate_notifications_consumes_runtime_terminal_output() {
    let root = temp_root("dagpipe-notification-runtime-output");
    write_dagpipe_notification_fixture(&root, &dagpipe_valid_notification_events());
    let graph = dagpipe_notification_graph(
        &["source_validate", "delivery_validate", "terminal_accept"],
        true,
    );
    let graph_path = write_dagpipe_notification_graph(&root, &graph);
    let output = run(&[
        "dagpipe",
        "validate-notifications",
        root.to_str().unwrap(),
        "--graph",
        &graph_path,
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("DAGPIPE_NOTIFICATION_TERMINAL_MISSING:notification-1")
    );
}

#[test]
fn dagpipe_validate_notifications_rejects_bad_explicit_graph_without_fallback() {
    let root = temp_root("dagpipe-notification-bad-explicit-graph");
    write_dagpipe_notification_fixture(&root, &[]);
    fs::write(root.join("bad.graph.json"), "{").unwrap();
    let output = run(&[
        "dagpipe",
        "validate-notifications",
        root.to_str().unwrap(),
        "--graph",
        "bad.graph.json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("DAGPIPE_GRAPH_INVALID"));
    assert!(!error.contains("\"evidence_scope\""));
}

#[test]
fn dagpipe_validate_notifications_empty_objects_compiles_and_reports_scope() {
    let root = temp_root("dagpipe-notification-empty-objects");
    write_dagpipe_notification_fixture(&root, &[]);
    let default = run(&["dagpipe", "validate-notifications", root.to_str().unwrap()]);
    assert!(
        default.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&default.stderr)
    );
    let default: Value = serde_json::from_slice(&default.stdout).unwrap();
    assert_eq!(default["evidence_scope"], "sdk_default_contract");
    assert_eq!(default["objects"].as_array().unwrap().len(), 0);

    let graph = dagpipe_notification_graph(
        &["source_validate", "delivery_validate", "terminal_accept"],
        false,
    );
    let graph_path = write_dagpipe_notification_graph(&root, &graph);
    let selected = run(&[
        "dagpipe",
        "validate-notifications",
        root.to_str().unwrap(),
        "--graph",
        &graph_path,
    ]);
    assert!(
        selected.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let selected: Value = serde_json::from_slice(&selected.stdout).unwrap();
    assert_eq!(selected["evidence_scope"], "selected_project_program");
    assert_eq!(selected["objects"].as_array().unwrap().len(), 0);

    let mut invalid_graph = graph;
    invalid_graph["inputs"][0]["schema"] = serde_json::json!("String");
    let invalid_path = write_dagpipe_notification_graph(&root, &invalid_graph);
    let invalid = run(&[
        "dagpipe",
        "validate-notifications",
        root.to_str().unwrap(),
        "--graph",
        &invalid_path,
    ]);
    assert!(
        !invalid.status.success(),
        "a String input cannot satisfy notification operators: stdout={}",
        String::from_utf8_lossy(&invalid.stdout)
    );
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("DAGPIPE_COMPILE_FAILED"));
}

#[test]
fn dagpipe_validate_notifications_rejects_graph_path_escape() {
    let root = temp_root("dagpipe-notification-graph-path-escape");
    write_dagpipe_notification_fixture(&root, &[]);
    let output = run(&[
        "dagpipe",
        "validate-notifications",
        root.to_str().unwrap(),
        "--graph",
        "../notification.graph.json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("DAGPIPE_NOTIFICATION_GRAPH_PATH_INVALID")
    );
}

#[test]
fn dagpipe_validate_notifications_rejects_mailbox_only_objects() {
    let root = temp_root("dagpipe-notification-mailbox-only");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("NOTIFICATION_OBJECT_TERMINAL_MISSING"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_splits_queued_generations_into_objects() {
    let root = temp_root("dagpipe-notification-generations");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created-1", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted-1",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued-1",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt-1",
                "2026-01-01T00:00:02.5Z",
                "attempt-1",
                &["notification-1"],
                "notification.emitted",
            ),
            dagpipe_notification_emitted_event(
                "event-emitted-1",
                "2026-01-01T00:00:03Z",
                "attempt-1",
                &["notification-1"],
            ),
            dagpipe_message_created_event("event-created-2", "2026-01-01T00:01:00Z", "message-2"),
            dagpipe_message_state_event(
                "event-accepted-2",
                "2026-01-01T00:01:01Z",
                "message-2",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued-2",
                "2026-01-01T00:01:02Z",
                "notification-1",
                "notification-id-2",
                "message-2",
                1,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt-2",
                "2026-01-01T00:01:02.5Z",
                "attempt-2",
                &["notification-1"],
                "notification.emitted",
            ),
            dagpipe_notification_emitted_event(
                "event-emitted-2",
                "2026-01-01T00:01:03Z",
                "attempt-2",
                &["notification-1"],
            ),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["objects"].as_array().unwrap().len(), 2);
    assert_eq!(
        result["objects"][0]["terminal"]["kind"],
        "notification.emitted"
    );
    assert_eq!(
        result["objects"][1]["terminal"]["kind"],
        "notification.emitted"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_closes_batch_objects() {
    let root = temp_root("dagpipe-notification-batch");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    let summary1 = dagpipe_notification_summary("notification-id-1", "message-1", 0);
    let summary2 = dagpipe_notification_summary("notification-id-2", "message-2", 0);
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created-1", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted-1",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued-1",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_message_created_event("event-created-2", "2026-01-01T00:01:00Z", "message-2"),
            dagpipe_message_state_event(
                "event-accepted-2",
                "2026-01-01T00:01:01Z",
                "message-2",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued-2",
                "2026-01-01T00:01:02Z",
                "notification-2",
                "notification-id-2",
                "message-2",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:02:00Z",
                "attempt-batch",
                &["notification-1", "notification-2"],
                "notification.batch_emitted",
            ),
            serde_json::json!({
                "protocol": "appsdk-comm/v1",
                "eventId": "event-batch-emitted",
                "at": "2026-01-01T00:02:01Z",
                "kind": "notification.batch_emitted",
                "data": {
                    "attemptId": "attempt-batch",
                    "batch": {
                        "batchId": "batch-attempt-batch",
                        "recipient": dagpipe_address("recipient"),
                        "createdAt": "2026-01-01T00:02:00Z",
                        "adapterId": "adapter-1",
                        "items": [summary1, summary2]
                    },
                    "notificationKeys": ["notification-1", "notification-2"],
                    "at": "2026-01-01T00:02:01Z"
                }
            }),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["objects"].as_array().unwrap().len(), 2);
    for object in result["objects"].as_array().unwrap() {
        assert_eq!(object["terminal"]["kind"], "notification.batch_emitted");
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_batch_terminal_accepts_union_of_keys_and_ids() {
    let root = temp_root("dagpipe-notification-batch-keys-ids-union");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    let summary1 = dagpipe_notification_summary("notification-id-1", "message-1", 0);
    let summary2 = dagpipe_notification_summary("notification-id-2", "message-2", 0);
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created-1", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted-1",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued-1",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_message_created_event("event-created-2", "2026-01-01T00:01:00Z", "message-2"),
            dagpipe_message_state_event(
                "event-accepted-2",
                "2026-01-01T00:01:01Z",
                "message-2",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued-2",
                "2026-01-01T00:01:02Z",
                "notification-2",
                "notification-id-2",
                "message-2",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:02:00Z",
                "attempt-batch-union",
                &["notification-1", "notification-2"],
                "notification.batch_emitted",
            ),
            serde_json::json!({
                "protocol": "appsdk-comm/v1",
                "eventId": "event-batch-emitted-union",
                "at": "2026-01-01T00:02:01Z",
                "kind": "notification.batch_emitted",
                "data": {
                    "attemptId": "attempt-batch-union",
                    "batch": {
                        "batchId": "batch-attempt-batch-union",
                        "recipient": dagpipe_address("recipient"),
                        "createdAt": "2026-01-01T00:02:00Z",
                        "adapterId": "adapter-1",
                        "items": [summary1, summary2]
                    },
                    "notificationKeys": ["notification-2"],
                    "notificationIds": ["notification-id-1", "notification-id-2"],
                    "at": "2026-01-01T00:02:01Z"
                }
            }),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["objects"].as_array().unwrap().len(), 2);
    for object in result["objects"].as_array().unwrap() {
        assert_eq!(object["terminal"]["kind"], "notification.batch_emitted");
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_folds_pending_coalesced_queues() {
    let root = temp_root("dagpipe-notification-coalesce");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created-1", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted-1",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued-1",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_message_created_event("event-created-2", "2026-01-01T00:01:00Z", "message-2"),
            dagpipe_message_state_event(
                "event-accepted-2",
                "2026-01-01T00:01:01Z",
                "message-2",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued-2",
                "2026-01-01T00:01:02Z",
                "notification-1",
                "notification-id-2",
                "message-2",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:02:00Z",
                "attempt-coalesced",
                &["notification-1"],
                "notification.emitted",
            ),
            dagpipe_notification_emitted_event(
                "event-emitted",
                "2026-01-01T00:02:01Z",
                "attempt-coalesced",
                &["notification-1"],
            ),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["objects"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["objects"][0]["terminal"]["kind"],
        "notification.emitted"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_binds_coalesced_queue_to_latest_message_source() {
    let root = temp_root("dagpipe-notification-coalesce-latest-source");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created-1", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted-1",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued-1",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            // The second queued record replaces the object source under the same
            // key/generation but never reaches accepted.
            dagpipe_message_created_event("event-created-2", "2026-01-01T00:01:00Z", "message-2"),
            dagpipe_notification_queued(
                "event-queued-2",
                "2026-01-01T00:01:02Z",
                "notification-1",
                "notification-id-2",
                "message-2",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:02:00Z",
                "attempt-coalesced",
                &["notification-1"],
                "notification.emitted",
            ),
            dagpipe_notification_emitted_event(
                "event-emitted",
                "2026-01-01T00:02:01Z",
                "attempt-coalesced",
                &["notification-1"],
            ),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("NOTIFICATION_OBJECT_MESSAGE_ACCEPT_MISSING"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_terminal_without_delivery_attempt() {
    let root = temp_root("dagpipe-notification-terminal-without-attempt");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_emitted_event(
                "event-emitted",
                "2026-01-01T00:00:03Z",
                "attempt-1",
                &["notification-1"],
            ),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("attempt_missing:attempt-1"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_partially_matched_multi_key_event() {
    let root = temp_root("dagpipe-notification-partial-key-event");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:00:02.5Z",
                "attempt-1",
                &["notification-1"],
                "notification.emitted",
            ),
            dagpipe_notification_emitted_event(
                "event-emitted",
                "2026-01-01T00:00:03Z",
                "attempt-1",
                &["notification-1", "notification-2"],
            ),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(
            "NOTIFICATION_OBJECT_EVENT_UNATTACHED:notification.emitted:key:notification-2"
        ) || String::from_utf8_lossy(&output.stderr)
            .contains("NOTIFICATION_OBJECT_EVENT_UNATTACHED:notification.delivery_attempt"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_attempt_after_terminal() {
    let root = temp_root("dagpipe-notification-attempt-after-terminal");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_emitted_event(
                "event-emitted",
                "2026-01-01T00:00:03Z",
                "attempt-1",
                &["notification-1"],
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:00:04Z",
                "attempt-1",
                &["notification-1"],
                "notification.emitted",
            ),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("attempt_missing:attempt-1"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_emitted_terminal_with_batch_attempt() {
    let root = temp_root("dagpipe-notification-emitted-batch-attempt");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:00:02.5Z",
                "attempt-1",
                &["notification-1"],
                "notification.batch_emitted",
            ),
            dagpipe_notification_emitted_event(
                "event-emitted",
                "2026-01-01T00:00:03Z",
                "attempt-1",
                &["notification-1"],
            ),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("attempt_missing:attempt-1"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_batch_terminal_with_single_attempt() {
    let root = temp_root("dagpipe-notification-batch-single-attempt");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    let summary = dagpipe_notification_summary("notification-id-1", "message-1", 0);
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:00:02.5Z",
                "attempt-1",
                &["notification-1"],
                "notification.emitted",
            ),
            serde_json::json!({
                "protocol": "appsdk-comm/v1",
                "eventId": "event-batch-emitted",
                "at": "2026-01-01T00:00:03Z",
                "kind": "notification.batch_emitted",
                "data": {
                    "attemptId": "attempt-1",
                    "batch": {
                        "batchId": "batch-attempt-1",
                        "recipient": dagpipe_address("recipient"),
                        "createdAt": "2026-01-01T00:00:02Z",
                        "adapterId": "adapter-1",
                        "items": [summary]
                    },
                    "notificationKeys": ["notification-1"],
                    "at": "2026-01-01T00:00:03Z"
                }
            }),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("attempt_missing:attempt-1"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_batch_terminal_with_mismatched_batch_id() {
    let root = temp_root("dagpipe-notification-batch-id-mismatch");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    let summary = dagpipe_notification_summary("notification-id-1", "message-1", 0);
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:00:02.5Z",
                "attempt-1",
                &["notification-1"],
                "notification.batch_emitted",
            ),
            serde_json::json!({
                "protocol": "appsdk-comm/v1",
                "eventId": "event-batch-emitted",
                "at": "2026-01-01T00:00:03Z",
                "kind": "notification.batch_emitted",
                "data": {
                    "attemptId": "attempt-1",
                    "batch": {
                        "batchId": "batch-other",
                        "recipient": dagpipe_address("recipient"),
                        "createdAt": "2026-01-01T00:00:02Z",
                        "adapterId": "adapter-1",
                        "items": [summary]
                    },
                    "notificationKeys": ["notification-1"],
                    "at": "2026-01-01T00:00:03Z"
                }
            }),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("batch_id_mismatch"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_batch_terminal_without_object_item() {
    let root = temp_root("dagpipe-notification-batch-item-missing");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    let summary = dagpipe_notification_summary("notification-id-other", "message-other", 0);
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:00:02.5Z",
                "attempt-1",
                &["notification-1"],
                "notification.batch_emitted",
            ),
            serde_json::json!({
                "protocol": "appsdk-comm/v1",
                "eventId": "event-batch-emitted",
                "at": "2026-01-01T00:00:03Z",
                "kind": "notification.batch_emitted",
                "data": {
                    "attemptId": "attempt-1",
                    "batch": {
                        "batchId": "batch-attempt-1",
                        "recipient": dagpipe_address("recipient"),
                        "createdAt": "2026-01-01T00:00:02Z",
                        "adapterId": "adapter-1",
                        "items": [summary]
                    },
                    "notificationKeys": ["notification-1"],
                    "at": "2026-01-01T00:00:03Z"
                }
            }),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // A batch item that names a different object is already rejected while the
    // event is attached, so the fail-closed evidence may surface either as an
    // unattached claim or as the terminal-level batch-item check.
    assert!(
        stderr.contains("batch_item_missing") || stderr.contains("EVENT_UNATTACHED"),
        "stderr={stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_duplicate_source_after_terminal() {
    let root = temp_root("dagpipe-notification-duplicate-source-after-terminal");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued-first",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt-1",
                "2026-01-01T00:00:02.5Z",
                "attempt-1",
                &["notification-1"],
                "notification.emitted",
            ),
            dagpipe_notification_emitted_event(
                "event-emitted-1",
                "2026-01-01T00:00:03Z",
                "attempt-1",
                &["notification-1"],
            ),
            dagpipe_notification_queued(
                "event-queued-replay",
                "2026-01-01T00:00:04Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt-2",
                "2026-01-01T00:00:04.5Z",
                "attempt-2",
                &["notification-1"],
                "notification.emitted",
            ),
            dagpipe_notification_emitted_event(
                "event-emitted-2",
                "2026-01-01T00:00:05Z",
                "attempt-2",
                &["notification-1"],
            ),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("NOTIFICATION_OBJECT_SOURCE_DUPLICATE:notification-1"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_accepts_system_notification_message_created_accepted() {
    let root = temp_root("dagpipe-notification-system-message");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_system_message_created_event(
                "event-created",
                "2026-01-01T00:00:00Z",
                "message-1",
            ),
            dagpipe_message_delivery_attempt_event(
                "event-delivery-attempt",
                "2026-01-01T00:00:00.5Z",
                "message-1",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:01Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:00:01.5Z",
                "attempt-1",
                &["notification-1"],
                "notification.emitted",
            ),
            dagpipe_notification_emitted_event(
                "event-emitted",
                "2026-01-01T00:00:02Z",
                "attempt-1",
                &["notification-1"],
            ),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["objects"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["objects"][0]["terminal"]["kind"],
        "notification.emitted"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_symlinked_mailbox() {
    let root = temp_root("dagpipe-notification-symlink-root");
    let external = temp_root("dagpipe-notification-symlink-external");
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::create_dir_all(&external).unwrap();
    let external_mailbox = external.join("mailbox.jsonl");
    fs::write(&external_mailbox, "").unwrap();
    std::os::unix::fs::symlink(&external_mailbox, communication.join("mailbox.jsonl")).unwrap();

    let output = run(&["dagpipe", "validate-notifications", root.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("GOVERNANCE_PATH_SYMLINK"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_file(communication.join("mailbox.jsonl")).unwrap();
    fs::remove_file(&external_mailbox).unwrap();
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(external).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_mailbox_missing_trailing_newline() {
    let root = temp_root("dagpipe-notification-no-trailing-newline");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1")
            .to_string(),
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("COMMUNICATION_MAILBOX_EVENT_INVALID:missing_trailing_newline"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_unattached_terminal_events() {
    let root = temp_root("dagpipe-notification-unattached-terminal");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        serde_json::json!({
            "protocol": "appsdk-comm/v1",
            "eventId": "event-emitted",
            "at": "2026-01-01T00:00:03Z",
            "kind": "notification.emitted",
            "data": {
                "attemptId": "attempt-1",
                "keys": ["notification-1"],
                "at": "2026-01-01T00:00:03Z"
            }
        })
        .to_string()
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("NOTIFICATION_OBJECT_EVENT_UNATTACHED:notification.emitted"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_accepts_master_wake_decision_supersede() {
    let root = temp_root("dagpipe-notification-master-wake-decision");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            serde_json::json!({
                "protocol": "appsdk-comm/v1",
                "eventId": "event-superseded",
                "at": "2026-01-01T00:00:03Z",
                "kind": "notification.superseded",
                "data": {
                    "keys": ["notification-1"],
                    "generation": 1,
                    "reason": "master_wake_decision"
                }
            }),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result["objects"][0]["terminal"]["kind"],
        "notification.superseded"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_reports_delivery_failed_retry() {
    let root = temp_root("dagpipe-notification-delivery-failed");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:00:02.5Z",
                "attempt-1",
                &["notification-1"],
                "notification.emitted",
            ),
            dagpipe_notification_failed_event(
                "event-failed",
                "2026-01-01T00:00:03Z",
                &["notification-1"],
                "notification.emitted",
                "attempt-1",
            ),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["objects"][0]["terminal"]["status"], "pending_retry");
    assert_eq!(result["objects"][0]["terminal"]["retry_required"], true);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_reports_attemptless_failure_before_attempt() {
    let root = temp_root("dagpipe-notification-attemptless-failure-before-attempt");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_failed_event_without_attempt(
                "event-attemptless-failed-before-attempt",
                "2026-01-01T00:00:03Z",
                &["notification-1"],
                "notification.batch_emitted",
            ),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["objects"][0]["terminal"]["status"], "pending_retry");
    assert_eq!(result["objects"][0]["terminal"]["retry_required"], true);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_delivery_failure_after_terminal() {
    let root = temp_root("dagpipe-notification-failure-after-terminal");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-created", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_state_event(
                "event-accepted",
                "2026-01-01T00:00:01Z",
                "message-1",
                "accepted",
            ),
            dagpipe_notification_queued(
                "event-queued",
                "2026-01-01T00:00:02Z",
                "notification-1",
                "notification-id-1",
                "message-1",
                0,
            ),
            dagpipe_notification_attempt_event(
                "event-attempt",
                "2026-01-01T00:00:02.5Z",
                "attempt-1",
                &["notification-1"],
                "notification.emitted",
            ),
            dagpipe_notification_emitted_event(
                "event-emitted",
                "2026-01-01T00:00:03Z",
                "attempt-1",
                &["notification-1"],
            ),
            dagpipe_notification_failed_event(
                "event-failed-after-terminal",
                "2026-01-01T00:00:04Z",
                &["notification-1"],
                "notification.emitted",
                "attempt-1",
            ),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("NOTIFICATION_OBJECT_DELIVERY_FAILURE_MISMATCH:notification-1:attempt-1"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}
