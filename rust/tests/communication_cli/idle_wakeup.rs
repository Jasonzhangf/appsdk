use super::*;

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
