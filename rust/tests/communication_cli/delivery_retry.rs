use super::*;

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
fn communication_store_rejects_cross_project_root_writes() {
    let root = temp_root("root-binding");
    let foreign = temp_root("foreign-project");
    let foreign_root = foreign.to_str().unwrap();
    let rejected = call_error(
        &root,
        json!({
            "op": "register_runtime",
            "runtime": {
                "runtimeId": "foreign-runtime",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://foreign",
                "projectRoot": foreign_root,
                "processId": std::process::id()
            }
        }),
    );
    assert!(rejected.contains("project_root_mismatch"), "{rejected}");
    assert!(!root.join(".appsdk-host/runtimes.jsonl").exists());

    let project_root = root.canonicalize().unwrap().to_str().unwrap().to_owned();
    call(
        &root,
        json!({
            "op": "register_runtime",
            "runtime": {
                "runtimeId": "local-runtime",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://local",
                "projectRoot": project_root,
                "processId": std::process::id()
            }
        }),
    );
    let scope_rejected = call_error(
        &root,
        json!({
            "op": "register_scope",
            "scope": {
                "scopeId": "foreign-scope",
                "appserverId": "app",
                "namespace": "codex_tui",
                "endpoint": "mock://foreign",
                "projectRoot": foreign_root,
                "sessionIds": ["master"],
                "runtimeId": "local-runtime"
            }
        }),
    );
    assert!(
        scope_rejected.contains("project_root_mismatch"),
        "{scope_rejected}"
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert!(!raw.contains("foreign-scope"));
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(foreign).unwrap();
}

#[test]
fn communication_store_rejects_noncanonical_and_symlink_roots() {
    let root = temp_root("root-path");
    let name = root.file_name().unwrap().to_owned();
    let noncanonical = root.join("..").join(&name);
    let rejected = call_error(&noncanonical, json!({ "op": "status" }));
    assert!(
        rejected.contains("communication_root_not_canonical"),
        "{rejected}"
    );
    fs::remove_dir_all(&root).unwrap();

    let root = temp_root("root-dot-path");
    let name = root.file_name().unwrap().to_owned();
    let embedded_dot = root.parent().unwrap().join(".").join(&name);
    let rejected = call_error(&embedded_dot, json!({ "op": "status" }));
    assert!(
        rejected.contains("communication_root_not_canonical"),
        "{rejected}"
    );
    fs::remove_dir_all(&root).unwrap();

    let target = temp_root("root-symlink-target");
    let alias = target.with_file_name("appsdk-communication-root-alias");
    #[cfg(unix)]
    symlink(&target, &alias).unwrap();
    #[cfg(unix)]
    {
        let rejected = call_error(&alias, json!({ "op": "status" }));
        assert!(
            rejected.contains("communication_path_symlink"),
            "{rejected}"
        );
        fs::remove_file(&alias).unwrap();
    }
    fs::remove_dir_all(target).unwrap();
}

#[cfg(unix)]
#[test]
fn communication_store_rejects_mailbox_lock_and_parent_symlinks() {
    let parent_target = temp_root("parent-symlink-target");
    let parent_root = temp_root("parent-symlink-root");
    fs::remove_dir_all(&parent_root).unwrap();
    symlink(&parent_target, &parent_root).unwrap();
    let parent_error = call_error(&parent_root, json!({ "op": "status" }));
    assert!(
        parent_error.contains("communication_path_symlink"),
        "{parent_error}"
    );
    fs::remove_file(&parent_root).unwrap();
    fs::remove_dir_all(parent_target).unwrap();

    let mailbox_root = temp_root("mailbox-symlink-root");
    let mailbox_dir = mailbox_root.join(".appsdk-control/communication");
    fs::create_dir_all(&mailbox_dir).unwrap();
    let mailbox_target = temp_root("mailbox-symlink-target").join("mailbox.jsonl");
    fs::write(&mailbox_target, "").unwrap();
    symlink(&mailbox_target, mailbox_dir.join("mailbox.jsonl")).unwrap();
    let mailbox_error = call_error(&mailbox_root, json!({ "op": "status" }));
    assert!(
        mailbox_error.contains("communication_path_symlink"),
        "{mailbox_error}"
    );
    fs::remove_dir_all(mailbox_root).unwrap();
    fs::remove_file(&mailbox_target).unwrap();
    fs::remove_dir_all(mailbox_target.parent().unwrap()).unwrap();

    let lock_root = temp_root("lock-symlink-root");
    let lock_dir = lock_root.join(".appsdk-control/communication");
    fs::create_dir_all(&lock_dir).unwrap();
    fs::write(lock_dir.join("mailbox.jsonl"), "").unwrap();
    let lock_target = temp_root("lock-symlink-target").join("mailbox.jsonl.lock");
    fs::write(&lock_target, "").unwrap();
    symlink(&lock_target, lock_dir.join("mailbox.jsonl.lock")).unwrap();
    let lock_error = call_error(&lock_root, json!({ "op": "status" }));
    assert!(
        lock_error.contains("communication_path_symlink"),
        "{lock_error}"
    );
    fs::remove_dir_all(lock_root).unwrap();
    fs::remove_file(&lock_target).unwrap();
    fs::remove_dir_all(lock_target.parent().unwrap()).unwrap();
}

#[test]
fn communication_replay_rejects_empty_lines_and_replays_valid_events() {
    let root = temp_root("replay-empty-line");
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    fs::create_dir_all(mailbox.parent().unwrap()).unwrap();
    fs::write(&mailbox, "\n").unwrap();
    let empty_error = call_error(&root, json!({ "op": "status" }));
    assert!(empty_error.contains("journal_corrupt"), "{empty_error}");
    assert!(empty_error.contains("line 1"), "{empty_error}");
    fs::remove_dir_all(&root).unwrap();

    let root = temp_root("replay-invalid-envelope");
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    fs::create_dir_all(mailbox.parent().unwrap()).unwrap();
    fs::write(&mailbox, "{}\n").unwrap();
    let envelope_error = call_error(&root, json!({ "op": "status" }));
    assert!(
        envelope_error.contains("journal_corrupt"),
        "{envelope_error}"
    );
    assert!(envelope_error.contains("line 1"), "{envelope_error}");
    fs::remove_dir_all(&root).unwrap();

    let root = temp_root("replay-invalid-envelope-fields");
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    fs::create_dir_all(mailbox.parent().unwrap()).unwrap();
    let event = json!({
        "protocol": "appsdk-comm/v1",
        "eventId": "",
        "at": "not-a-timestamp",
        "kind": "adapter.registered",
        "data": {
            "adapterId": "replay-adapter",
            "kind": "mailbox",
            "target": null,
            "enabled": true,
            "execute": false,
            "recipient": null,
            "registeredAt": "2026-01-01T00:00:00Z"
        }
    });
    fs::write(
        &mailbox,
        format!("{}\n", serde_json::to_string(&event).unwrap()),
    )
    .unwrap();
    let fields_error = call_error(&root, json!({ "op": "status" }));
    assert!(fields_error.contains("journal_corrupt"), "{fields_error}");
    assert!(fields_error.contains("line 1"), "{fields_error}");
    fs::remove_dir_all(&root).unwrap();

    let root = temp_root("replay-valid-event");
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "replay-adapter",
                "kind": "mailbox",
                "target": "local"
            }
        }),
    );
    let replayed = call(&root, json!({ "op": "status" }));
    assert!(replayed["adapters"]
        .as_array()
        .unwrap()
        .iter()
        .any(|adapter| adapter["adapterId"] == "replay-adapter"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn communication_replay_rejects_duplicate_event_ids_missing_final_newline_and_unknown_fields() {
    let root = temp_root("replay-duplicate-event-id");
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "replay-adapter",
                "kind": "mailbox",
                "target": "local"
            }
        }),
    );
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let first = contents.lines().next().unwrap();
    fs::write(&mailbox, format!("{contents}{first}\n")).unwrap();
    let duplicate_error = call_error(&root, json!({ "op": "status" }));
    assert!(
        duplicate_error.contains("journal_corrupt"),
        "{duplicate_error}"
    );
    assert!(duplicate_error.contains("duplicate"), "{duplicate_error}");
    fs::remove_dir_all(&root).unwrap();

    let root = temp_root("replay-missing-final-newline");
    call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "replay-adapter",
                "kind": "mailbox",
                "target": "local"
            }
        }),
    );
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let mut contents = fs::read_to_string(&mailbox).unwrap();
    assert!(contents.ends_with('\n'));
    contents.pop();
    fs::write(&mailbox, contents).unwrap();
    let newline_error = call_error(&root, json!({ "op": "status" }));
    assert!(newline_error.contains("journal_corrupt"), "{newline_error}");
    assert!(newline_error.contains("newline"), "{newline_error}");
    fs::remove_dir_all(&root).unwrap();

    let root = temp_root("replay-unknown-envelope-field");
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    fs::create_dir_all(mailbox.parent().unwrap()).unwrap();
    let event = json!({
        "protocol": "appsdk-comm/v1",
        "eventId": "event-with-extra-field",
        "at": "2026-01-01T00:00:00Z",
        "kind": "adapter.registered",
        "data": {
            "adapterId": "replay-adapter",
            "kind": "mailbox",
            "target": "local",
            "enabled": true,
            "execute": false,
            "recipient": null,
            "registeredAt": "2026-01-01T00:00:00Z"
        },
        "unexpected": true
    });
    fs::write(
        &mailbox,
        format!("{}\n", serde_json::to_string(&event).unwrap()),
    )
    .unwrap();
    let fields_error = call_error(&root, json!({ "op": "status" }));
    assert!(fields_error.contains("journal_corrupt"), "{fields_error}");
    assert!(
        fields_error.contains("unknown") || fields_error.contains("unexpected"),
        "{fields_error}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn adapters_are_explicit_and_receipts_are_replayed() {
    let root = temp_root("adapters");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);

    let appserver = call(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "desktop-host",
                "kind": "appserver",
                "target": "mock://scope",
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
                "adapterId": "desktop-host",
                "messageId": "message-appserver-preview"
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
    assert_eq!(emitted[0]["transportReceipt"]["adapterId"], "desktop-host");
    assert_eq!(emitted[0]["transportReceipt"]["state"], "intent");
    assert_eq!(
        emitted[0]["transportReceipt"]["evidence"]["hostMustExecute"],
        true
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
    let error = call_error(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "legacy-rejected",
                "kind": "tmux",
                "target": "legacy-pane",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert!(error.contains("invalid_adapter_kind"), "{error}");

    let status = call(&root, json!({ "op": "status" }));
    let pending = status["notificationProjection"]["pending"]
        .as_array()
        .unwrap();
    assert!(pending.is_empty());
    assert!(status["notificationProjection"]["emitted"]
        .as_array()
        .unwrap()
        .is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn direct_and_p0_failures_are_never_downgraded_to_idle_batch() {
    let root = temp_root("direct-p0-batch-failure");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "master", "master", "master", None);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let rejected = call_error(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "legacy-rejected",
                "kind": "tmux",
                "target": "legacy-pane",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert!(rejected.contains("invalid_adapter_kind"), "{rejected}");

    for (message_id, priority, delivery_mode) in [
        ("direct-failure", "p1", "direct"),
        ("p0-failure", "p0", "idle"),
    ] {
        call(
            &root,
            json!({
                "op": "send",
                "message": {
                    "from": { "scopeId": "scope", "sessionId": "worker" },
                    "to": { "scopeId": "scope", "sessionId": "master" },
                    "title": message_id,
                    "priority": priority,
                    "body": "delivery remains direct",
                    "deliveryMode": delivery_mode,
                    "messageId": message_id,
                    "createdAt": "2026-01-01T00:00:00Z"
                }
            }),
        );
    }

    let flush = Command::new(binary())
        .args([
            "communication",
            root.to_str().unwrap(),
            "--json",
            &serde_json::to_string(&json!({
                "op": "flush_notifications",
                "now": "2026-01-01T00:10:00Z"
            }))
            .unwrap(),
        ])
        .env("APPSDK_HOME", root.join(".appsdk-host"))
        .output()
        .unwrap();
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    let events: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let batch_attempts: Vec<&Value> = events
        .iter()
        .filter(|event| {
            event["kind"] == "notification.delivery_attempt"
                && event["data"]["attempt"]["operation"] == "notification.batch_emitted"
        })
        .collect();
    assert!(
        batch_attempts.is_empty(),
        "flush must not attempt a batch for direct or p0 failures; stderr={}",
        String::from_utf8_lossy(&flush.stderr)
    );
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
        2
    );
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
                "kind": "appserver",
                "target": "mock://scope",
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
                "adapterId": "appserver-preview",
                "kind": "appserver",
                "target": "mock://scope",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    for (message_id, adapter_id) in [
        ("mailbox-message", "mailbox"),
        ("appserver-message", "appserver-preview"),
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
    // Master registration reconciles the persisted idle edge itself.  A
    // later repeated state observation is a no-op and must not emit a second
    // response or append another message fact.
    assert!(retry["notification"].is_null());
    let message_id = call(&root, json!({ "op": "status" }))["messages"][0]["messageId"]
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
fn master_registration_reconciles_existing_worker_idle_edge_once() {
    let root = temp_root("master-registration-idle-reconcile");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let error = call_error(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:00:00Z"
        }),
    );
    assert!(error.contains("master_not_registered"), "{error}");

    register_agent(&root, "scope", "master", "master", "master", None);
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        status["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        status["notificationProjection"]["pending"][0]["messageId"],
        status["messages"][0]["messageId"]
    );

    register_agent(&root, "scope", "master", "master", "master", None);
    let repeated = call(&root, json!({ "op": "status" }));
    assert_eq!(repeated["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        repeated["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"message.created\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_registration_retry_reconciles_agent_registered_prefix() {
    let root = temp_root("master-registration-prefix-retry");
    register_scope(&root, "scope", "app", "/project", &["master", "worker"]);
    register_agent(&root, "scope", "worker", "worker", "peer", None);
    let error = call_error(
        &root,
        json!({
            "op": "set_agent_state",
            "address": { "scopeId": "scope", "sessionId": "worker" },
            "state": "idle",
            "at": "2026-01-01T00:00:00Z"
        }),
    );
    assert!(error.contains("master_not_registered"), "{error}");

    register_agent(&root, "scope", "master", "master", "master", None);
    // Simulate a crash immediately after agent.registered: the durable
    // master identity remains, while reconciliation facts are absent.
    retain_mailbox_through_occurrence(&root, "agent.registered", 2);
    let retry = call(
        &root,
        json!({
            "op": "register_agent",
            "agent": {
                "scopeId": "scope",
                "sessionId": "master",
                "agentId": "master",
                "role": "master",
                "masterGrant": "user approved master for this scope",
                "runtimeId": "runtime-scope"
            }
        }),
    );
    assert_eq!(retry["idempotent"], true);
    assert_eq!(retry["reconciledIdle"].as_array().unwrap().len(), 1);
    let status = call(&root, json!({ "op": "status" }));
    assert_eq!(status["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        status["notificationProjection"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        status["notificationProjection"]["pending"][0]["messageId"],
        status["messages"][0]["messageId"]
    );
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(raw.matches("\"kind\":\"agent.registered\"").count(), 2);
    assert_eq!(raw.matches("\"kind\":\"message.created\"").count(), 1);
    assert_eq!(raw.matches("\"kind\":\"notification.queued\"").count(), 1);
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
    let rejected = call_error(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "legacy-rejected",
                "kind": "tmux",
                "target": "legacy-pane",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert!(rejected.contains("invalid_adapter_kind"), "{rejected}");
    for (message_id, body) in [("direct-1", "first"), ("direct-2", "second")] {
        call(
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
                    "messageId": message_id
                }
            }),
        );
    }
    let status = call(&root, json!({ "op": "status" }));
    let pending = status["notificationProjection"]["pending"]
        .as_array()
        .unwrap();
    assert!(pending.is_empty());
    assert_eq!(
        status["notificationProjection"]["emitted"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
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
    let rejected = call_error(
        &root,
        json!({
            "op": "register_adapter",
            "adapter": {
                "adapterId": "legacy-rejected",
                "kind": "tmux",
                "target": "legacy-pane",
                "recipient": { "scopeId": "scope", "sessionId": "master" }
            }
        }),
    );
    assert!(rejected.contains("invalid_adapter_kind"), "{rejected}");
    let request = json!({
        "op": "send",
        "message": {
            "from": { "scopeId": "scope", "sessionId": "worker" },
            "to": { "scopeId": "scope", "sessionId": "master" },
            "title": "retry direct",
            "priority": "p1",
            "body": "known transport failure",
            "deliveryMode": "direct",
            "messageId": "retry-direct"
        }
    });
    let first = call(&root, request.clone());
    let second = call(&root, request);
    assert_eq!(first["idempotent"], false);
    assert_eq!(second["idempotent"], true);

    let status = call(&root, json!({ "op": "status" }));
    assert!(status["notificationProjection"]["pending"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(status["notificationProjection"]["unknown"]
        .as_array()
        .unwrap()
        .is_empty());
    let raw = fs::read_to_string(root.join(".appsdk-control/communication/mailbox.jsonl")).unwrap();
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_attempt\"")
            .count(),
        1
    );
    assert_eq!(
        raw.matches("\"kind\":\"notification.delivery_failed\"")
            .count(),
        0
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
