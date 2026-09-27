use super::*;

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
