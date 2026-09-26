#[test]
fn dagpipe_validate_notifications_rejects_failed_attempt_reused_as_terminal() {
    let root = temp_root("dagpipe-notification-failed-attempt-terminal");
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
            dagpipe_notification_emitted_event(
                "event-emitted-reuses-failed",
                "2026-01-01T00:00:04Z",
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
        String::from_utf8_lossy(&output.stderr)
            .contains("NOTIFICATION_TERMINAL_INVALID:notification-1:notification.emitted:attempt_missing:attempt-1"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_attemptless_failure_after_terminal() {
    let root = temp_root("dagpipe-notification-attemptless-failure-after-terminal");
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
            dagpipe_notification_failed_event_without_attempt(
                "event-attemptless-failed-after-terminal",
                "2026-01-01T00:00:04Z",
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
        String::from_utf8_lossy(&output.stderr)
            .contains("NOTIFICATION_OBJECT_DELIVERY_FAILURE_MISMATCH:notification-1"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_attemptless_failure_while_attempt_pending() {
    let root = temp_root("dagpipe-notification-attemptless-failure-while-pending");
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
            dagpipe_notification_failed_event_without_attempt(
                "event-attemptless-failed-while-pending",
                "2026-01-01T00:00:03Z",
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
        String::from_utf8_lossy(&output.stderr)
            .contains("NOTIFICATION_OBJECT_DELIVERY_FAILURE_MISMATCH:notification-1"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_failure_after_ids_only_batch_terminal() {
    let root = temp_root("dagpipe-notification-ids-only-batch-terminal-failure");
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
                "eventId": "event-ids-only-batch-emitted",
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
                    "notificationIds": ["notification-id-1"],
                    "at": "2026-01-01T00:00:03Z"
                }
            }),
            dagpipe_notification_failed_event(
                "event-failed-after-ids-only-terminal",
                "2026-01-01T00:00:04Z",
                &["notification-1"],
                "notification.batch_emitted",
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

#[test]
fn dagpipe_validate_notifications_rejects_delivery_attempt_after_terminal_sink() {
    let root = temp_root("dagpipe-notification-attempt-after-terminal-sink");
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
            dagpipe_notification_attempt_event(
                "event-attempt-after-terminal",
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
        String::from_utf8_lossy(&output.stderr)
            .contains("NOTIFICATION_OBJECT_DELIVERY_ATTEMPT_AFTER_TERMINAL:notification-1"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_duplicate_event_ids() {
    let root = temp_root("dagpipe-notification-duplicate-event");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        [
            dagpipe_message_created_event("event-duplicate", "2026-01-01T00:00:00Z", "message-1"),
            dagpipe_message_created_event("event-duplicate", "2026-01-01T00:00:01Z", "message-2"),
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
            .contains("COMMUNICATION_MAILBOX_EVENT_DUPLICATE:eventId:event-duplicate"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_folds_same_generation_retry_queue() {
    let root = temp_root("dagpipe-notification-retry-queue");
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
            dagpipe_notification_failed_event(
                "event-failed-1",
                "2026-01-01T00:00:03Z",
                &["notification-1"],
                "notification.emitted",
                "attempt-1",
            ),
            dagpipe_notification_queued(
                "event-queued-retry",
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
                "event-emitted-retry",
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
fn dagpipe_validate_notifications_retries_failed_attempt_without_new_queue() {
    let root = temp_root("dagpipe-notification-retry-attempt-no-queue");
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
                "event-attempt-1",
                "2026-01-01T00:00:02.5Z",
                "attempt-1",
                &["notification-1"],
                "notification.emitted",
            ),
            dagpipe_notification_failed_event(
                "event-failed-1",
                "2026-01-01T00:00:03Z",
                &["notification-1"],
                "notification.emitted",
                "attempt-1",
            ),
            dagpipe_notification_attempt_event(
                "event-attempt-2",
                "2026-01-01T00:00:04Z",
                "attempt-2",
                &["notification-1"],
                "notification.emitted",
            ),
            dagpipe_notification_emitted_event(
                "event-emitted",
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
fn dagpipe_validate_notifications_rejects_schema_invalid_failed_event() {
    let root = temp_root("dagpipe-notification-invalid-failure");
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
                "eventId": "event-failed-invalid",
                "at": "2026-01-01T00:00:03Z",
                "kind": "notification.delivery_failed",
                "data": {
                    "keys": ["notification-1"],
                    "error": {"code": "transport_unavailable", "message": "retry later"}
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
        String::from_utf8_lossy(&output.stderr)
            .contains("COMMUNICATION_MAILBOX_EVENT_INVALID:4:notification.delivery_failed"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dagpipe_validate_notifications_rejects_unknown_event_kinds() {
    let root = temp_root("dagpipe-notification-unknown-kind");
    let root_text = root.to_str().unwrap();
    let communication = root.join(".appsdk-control/communication");
    fs::create_dir_all(&communication).unwrap();
    fs::write(
        communication.join("mailbox.jsonl"),
        serde_json::json!({
            "protocol": "appsdk-comm/v1",
            "eventId": "event-garbage",
            "at": "2026-01-01T00:00:00Z",
            "kind": "garbage",
            "data": {"foo": 1}
        })
        .to_string()
            + "\n",
    )
    .unwrap();

    let output = run(&["dagpipe", "validate-notifications", root_text]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("COMMUNICATION_MAILBOX_EVENT_KIND_INVALID"),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}
