#[test]
fn send_fails_closed_when_recipient_rebound_during_presence_probe() {
    let (mut server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    {
        let mut state = server.state.lock().unwrap();
        state.workers.get_mut("recipient").unwrap().transport =
            Some(test_appserver_transport("thread-recipient"));
    }
    let slot: std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<crate::server::Server>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let probe_slot = slot.clone();
    server.appserver_thread_status = std::sync::Arc::new(move |_, thread_id| {
        if let Some(server) = probe_slot.lock().unwrap().as_ref() {
            server
                .state
                .lock()
                .unwrap()
                .workers
                .get_mut("recipient")
                .unwrap()
                .transport = Some(test_appserver_transport("thread-recipient-other"));
        }
        Ok(serde_json::json!({
            "thread": {
                "id": thread_id,
                "status": {"type": "idle"},
                "canAcceptDirectInput": true
            }
        }))
    });
    let server = std::sync::Arc::new(server);
    *slot.lock().unwrap() = Some(server.clone());

    let response = handle_send_with_task(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready.".into(),
        None,
        "immediate".into(),
        false,
        None,
    );
    assert!(!response.ok, "{response:?}");
    assert!(
        response
            .error
            .as_deref()
            .is_some_and(|error| error.contains("transport changed")),
        "{response:?}"
    );
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn recipient_jsonl_records_latest_delivery_and_journal_replay() {
    let (server, root) = test_server();
    register(&server, "recipient", "%recipient");
    let id = "jsonl-message";
    server.commit(&[
        Event::Sent {
            msg: Message {
                id: id.into(),
                from: "sender".into(),
                to: "recipient".into(),
                mtype: "notify".into(),
                subject: Some("progress".into()),
                body: "task-jsonl progress".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::Delivered {
            ids: vec![id.into()],
        },
        Event::Acked {
            ids: vec![id.into()],
        },
    ]);
    let path = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    let records = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|record| record["message"]["id"] == id)
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 3, "Sent/Delivered/Acked append snapshots");
    for record in &records {
        assert_eq!(record["schema_version"], 1);
        assert_eq!(record["record_type"], "message");
        assert_eq!(record["recipient"], "recipient");
        assert_eq!(record["category"], "progress");
        assert!(record["task_ids"].is_array());
        assert!(record["created_ms"].is_i64());
        assert!(record["window_start_ms"].is_null());
        assert!(record["window_end_ms"].is_null());
        assert!(record["exact_error"]
            .as_str()
            .is_some_and(|error| { error.starts_with("MAILBOX_SCOPE_BINDING_UNAVAILABLE:") }));
    }
    assert_eq!(records.last().unwrap()["message"]["state"], "read");
    assert_eq!(replay(&root).unwrap().msgs[id].state, "read");
    let path = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    std::fs::write(
        &path,
        format!("{}{{\"partial\":", std::fs::read_to_string(&path).unwrap()),
    )
    .unwrap();
    let projection = read_recipient_mailbox(&path, "recipient").unwrap();
    assert_eq!(projection.records.len(), 3);
    assert!(projection.partial_tail);
    assert!(projection.unterminated_tail);
    std::fs::write(&path, "{\"bad\":true}\nnot-json\n").unwrap();
    assert!(read_recipient_mailbox(&path, "recipient").is_err());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn recipient_jsonl_failure_is_logged_without_panicking() {
    let (server, root) = test_server();
    register(&server, "recipient", "%recipient");
    std::fs::create_dir_all(root.join(".agent-collab/mailbox/recipient-recipient.jsonl")).unwrap();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "jsonl-failure".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("failure".into()),
            body: "preserve journal truth".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    assert!(
        std::fs::read_to_string(root.join(".agent-collab/server/log.txt"))
            .unwrap()
            .contains("MAILBOX_JSONL_WRITE_FAILED")
    );
    assert_eq!(
        replay(&root).unwrap().msgs["jsonl-failure"].state,
        "pending"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn recipient_jsonl_accepts_legacy_bare_message_before_new_append() {
    let (server, root) = test_server();
    register(&server, "recipient", "%recipient");
    let path = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let legacy = Message {
        id: "legacy-message".into(),
        from: "sender".into(),
        to: "recipient".into(),
        mtype: "notify".into(),
        subject: Some("progress".into()),
        body: "legacy record".into(),
        in_reply_to: None,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&legacy).unwrap()),
    )
    .unwrap();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "new-message".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "new record".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let projection = read_recipient_mailbox(&path, "recipient").unwrap();
    assert_eq!(projection.records.len(), 2);
    assert_eq!(projection.records[0]["schema_version"], 1);
    assert_eq!(projection.records[0]["window_source"], "legacy-message");
    assert_eq!(projection.records[1]["message"]["id"], "new-message");
    assert_eq!(replay(&root).unwrap().msgs["new-message"].state, "pending");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn recipient_jsonl_separates_complete_unterminated_legacy_record() {
    let (server, root) = test_server();
    register(&server, "recipient", "%recipient");
    let path = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let legacy = Message {
        id: "legacy-without-newline".into(),
        from: "sender".into(),
        to: "recipient".into(),
        mtype: "notify".into(),
        subject: Some("progress".into()),
        body: "complete record without separator".into(),
        in_reply_to: None,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    std::fs::write(&path, serde_json::to_string(&legacy).unwrap()).unwrap();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "after-legacy-without-newline".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "new record".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let projection = read_recipient_mailbox(&path, "recipient").unwrap();
    assert_eq!(projection.records.len(), 2);
    assert!(!projection.partial_tail);
    assert!(!projection.unterminated_tail);
    assert_eq!(
        projection.records[0]["message"]["id"],
        "legacy-without-newline"
    );
    assert_eq!(
        projection.records[1]["message"]["id"],
        "after-legacy-without-newline"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn partial_recipient_tail_is_repaired_before_append_and_replay_preserves_assignment() {
    let (server, root) = test_server();
    register(&server, "recipient", "%recipient");
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "partial-assigned-task".into(),
            owner: "recipient".into(),
            created_by: "sender".into(),
            feature_id: Some("partial-mailbox".into()),
            worktree_path: None,
            branch: None,
            base_commit: Some("partial-base".into()),
            priority: "p0".into(),
            status: "working".into(),
            next_step: Some("replay after restart".into()),
            wait: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        },
    }]);
    let path = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "before-partial".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "before partial tail".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let mut content = std::fs::read_to_string(&path).unwrap();
    content.push_str("{\"partial\":");
    std::fs::write(&path, content).unwrap();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "after-partial".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "after partial tail".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let projection = read_recipient_mailbox(&path, "recipient").unwrap();
    assert_eq!(projection.records.len(), 2);
    assert!(!projection.partial_tail);
    assert!(!projection.unterminated_tail);
    assert_eq!(projection.records[0]["message"]["id"], "before-partial");
    assert_eq!(projection.records[1]["message"]["id"], "after-partial");
    drop(server);
    let replayed = replay(&root).unwrap();
    assert_eq!(replayed.msgs["before-partial"].state, "pending");
    assert_eq!(replayed.msgs["after-partial"].state, "pending");
    assert_eq!(replayed.tasks["partial-assigned-task"].owner, "recipient");
    assert_eq!(
        replayed.tasks["partial-assigned-task"]
            .feature_id
            .as_deref(),
        Some("partial-mailbox")
    );
    assert_eq!(
        replayed.tasks["partial-assigned-task"].next_step.as_deref(),
        Some("replay after restart")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn malformed_recipient_jsonl_does_not_block_future_append_or_journal_replay() {
    let (server, root) = test_server();
    register(&server, "recipient", "%recipient");
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "assigned-task".into(),
            owner: "recipient".into(),
            created_by: "sender".into(),
            feature_id: Some("mailbox-envelope".into()),
            worktree_path: None,
            branch: None,
            base_commit: Some("base-commit".into()),
            priority: "p0".into(),
            status: "working".into(),
            next_step: Some("consume mailbox".into()),
            wait: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        },
    }]);
    let path = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{\"bad\":true}\n").unwrap();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "after-malformed".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "journal remains authoritative".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.lines().any(|line| line.contains("after-malformed")));
    assert_eq!(content.matches("{\"bad\":true}").count(), 1);
    let projection = read_recipient_mailbox(&path, "recipient").unwrap();
    assert_eq!(projection.records.len(), 1);
    assert_eq!(projection.records[0]["message"]["id"], "after-malformed");
    assert!(
        std::fs::read_to_string(root.join(".agent-collab/server/log.txt"))
            .unwrap()
            .contains("MAILBOX_JSONL_RECOVERABLE")
    );
    drop(server);
    let replayed = replay(&root).unwrap();
    assert_eq!(replayed.msgs["after-malformed"].state, "pending");
    let assignment = &replayed.tasks["assigned-task"];
    assert_eq!(assignment.owner, "recipient");
    assert_eq!(assignment.feature_id.as_deref(), Some("mailbox-envelope"));
    assert_eq!(assignment.base_commit.as_deref(), Some("base-commit"));
    assert_eq!(assignment.status, "working");
    assert_eq!(assignment.next_step.as_deref(), Some("consume mailbox"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn interior_malformed_recipient_jsonl_preserves_bad_line_and_appends_later_message() {
    let (server, root) = test_server();
    register(&server, "recipient", "%recipient");
    let path = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "interior-assigned-task".into(),
            owner: "recipient".into(),
            created_by: "sender".into(),
            feature_id: Some("interior-mailbox-recovery".into()),
            worktree_path: Some("./playground/interior-mailbox-recovery".into()),
            branch: Some("codex/interior-mailbox-recovery".into()),
            base_commit: Some("interior-base".into()),
            priority: "p1".into(),
            status: "working".into(),
            next_step: Some("consume preserved mailbox".into()),
            wait: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        },
    }]);

    server.commit(&[Event::Sent {
        msg: Message {
            id: "before-interior-malformed".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "before malformed interior record".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let malformed_line = "not-json-interior-record-preserve-this-line";
    server.commit(&[Event::Sent {
        msg: Message {
            id: "later-valid-record".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "later valid record".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);

    let content = std::fs::read_to_string(&path).unwrap();
    let (first, remaining) = content.split_once('\n').unwrap();
    std::fs::write(&path, format!("{first}\n{malformed_line}\n{remaining}")).unwrap();

    server.commit(&[Event::Sent {
        msg: Message {
            id: "after-interior-malformed".into(),
            from: "sender".into(),
            to: "recipient".into(),
            mtype: "notify".into(),
            subject: Some("progress".into()),
            body: "new append after malformed interior record".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.lines().any(|line| line == malformed_line));
    let projection = read_recipient_mailbox(&path, "recipient").unwrap();
    assert_eq!(
        projection
            .records
            .iter()
            .filter_map(|record| record["message"]["id"].as_str())
            .collect::<Vec<_>>(),
        vec![
            "before-interior-malformed",
            "later-valid-record",
            "after-interior-malformed"
        ]
    );
    assert_eq!(projection.recoverable_errors.len(), 1);
    assert!(projection.recoverable_errors[0].contains("record 2"));

    let server = Arc::new(server);
    let response = dispatch(
        &server,
        Req::MailboxRead {
            all: false,
            sort: Some("time-asc".into()),
            worker_id: Some("recipient".into()),
        },
    );
    assert!(response.ok);
    assert_eq!(
        response.data["recipient_jsonl"]["status"],
        "recoverable-error"
    );
    assert!(response.data["recipient_jsonl"]["exact_error"]
        .as_str()
        .unwrap()
        .contains("record 2"));
    assert_eq!(
        response.data["recipient_jsonl"]["records"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    drop(server);
    let replayed = replay(&root).unwrap();
    assert_eq!(
        replayed.msgs["later-valid-record"].body,
        "later valid record"
    );
    assert_eq!(
        replayed.msgs["after-interior-malformed"].body,
        "new append after malformed interior record"
    );
    let assignment = &replayed.tasks["interior-assigned-task"];
    assert_eq!(assignment.owner, "recipient");
    assert_eq!(
        assignment.feature_id.as_deref(),
        Some("interior-mailbox-recovery")
    );
    assert_eq!(
        assignment.worktree_path.as_deref(),
        Some("./playground/interior-mailbox-recovery")
    );
    assert_eq!(
        assignment.branch.as_deref(),
        Some("codex/interior-mailbox-recovery")
    );
    assert_eq!(assignment.base_commit.as_deref(), Some("interior-base"));
    assert_eq!(assignment.status, "working");
    assert_eq!(
        assignment.next_step.as_deref(),
        Some("consume preserved mailbox")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn removed_role_and_dispatch_commands_fail_fast() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(
        handle_task_dispatch(&server, "peer".into(), "token-peer".into())
            .error
            .unwrap()
            .contains("deprecated")
    );
    assert!(handle_task_claim(
        &server,
        "peer".into(),
        "token-peer".into(),
        "legacy-task".into(),
    )
    .error
    .unwrap()
    .contains("deprecated"));
    let server = Arc::new(server);
    for request in [
        Req::Role {
            worker_id: "peer".into(),
        },
        Req::TransferMaster {
            worker_id: "peer".into(),
            token: "token-peer".into(),
            target_id: "peer".into(),
        },
    ] {
        assert!(!dispatch(&server, request).ok);
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn lifecycle_cannot_bypass_delivery_or_review() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "task", "feature").ok);
    assert!(create_task(&server, "peer", "blocked-task", "blocked-feature").ok);
    assert!(create_task(&server, "peer", "working-task", "working-feature").ok);
    let early_review = handle_task_review(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        true,
        false,
        "not delivered".into(),
    );
    assert!(!early_review.ok);
    assert_eq!(
        early_review.error.as_deref(),
        Some("task task must be delivered before review (current: working)")
    );
    let missing_evidence_delivery = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "working-task".into(),
        None,
        Some("/tmp/worktree".into()),
    );
    assert_eq!(
        missing_evidence_delivery.error.as_deref(),
        Some("task deliver requires non-empty --evidence")
    );
    let working_delivery = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "working-task".into(),
        Some("working task candidate verified".into()),
        Some("/tmp/worktree".into()),
    );
    assert!(working_delivery.ok, "{working_delivery:?}");
    assert_eq!(
        server.state.lock().unwrap().tasks["working-task"].status,
        "delivered"
    );
    assert!(
        handle_task_update(
            &server,
            "peer".into(),
            "token-peer".into(),
            "blocked-task".into(),
            Some("blocked".into()),
            None,
        )
        .ok
    );
    let blocked_delivery = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "blocked-task".into(),
        Some("blocked task candidate verified".into()),
        Some("/tmp/worktree".into()),
    );
    assert_eq!(
        blocked_delivery.error.as_deref(),
        Some(
            "task blocked-task must be owned and working, verifying, reviewed, or rework before delivery (current: blocked)"
        )
    );
    assert!(
        handle_task_update(
            &server,
            "peer".into(),
            "token-peer".into(),
            "task".into(),
            Some("verifying".into()),
            None,
        )
        .ok
    );
    let skipped_delivery = handle_task_update(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        Some("merged".into()),
        None,
    );
    assert_eq!(
        skipped_delivery.error.as_deref(),
        Some("use collab task review/integrated for integration-owned lifecycle transitions")
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["task"].status,
        "verifying"
    );
    let delivered = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        Some("candidate verified".into()),
        Some("/tmp/worktree".into()),
    );
    assert!(delivered.ok, "{delivered:?}");
    let accepted = handle_task_review(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        true,
        false,
        "review passed".into(),
    );
    assert!(accepted.ok, "{accepted:?}");
    assert_eq!(
        server.state.lock().unwrap().tasks["task"].status,
        "accepted"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn legacy_accepted_candidate_can_record_merge() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "accepted-task", "feature").ok);
    {
        let mut state = server.state.lock().unwrap();
        let mut task = state.tasks.remove("accepted-task").unwrap();
        task.status = "accepted".into();
        state.tasks.insert(task.id.clone(), task);
    }
    let merged = handle_task_update(
        &server,
        "peer".into(),
        "token-peer".into(),
        "accepted-task".into(),
        Some("merged".into()),
        Some("integration recorded".into()),
    );
    assert!(
        merged.ok,
        "legacy accepted task must be mergeable: {merged:?}"
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["accepted-task"].status,
        "merged"
    );
    assert_eq!(
        replay(&root).unwrap().tasks["accepted-task"].status,
        "merged"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn merged_task_without_lifecycle_edges_cannot_close() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "accepted-task", "feature").ok);
    {
        let mut state = server.state.lock().unwrap();
        let mut task = state.tasks.remove("accepted-task").unwrap();
        task.status = "accepted".into();
        state.tasks.insert(task.id.clone(), task);
    }
    assert!(
        handle_task_update(
            &server,
            "peer".into(),
            "token-peer".into(),
            "accepted-task".into(),
            Some("merged".into()),
            Some("legacy merge compatibility".into()),
        )
        .ok
    );
    let closed = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "accepted-task".into(),
        false,
        None,
    );
    assert_eq!(
        closed.error.as_deref(),
        Some("task accepted-task cannot close before delivery, review, and integration evidence")
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["accepted-task"].status,
        "merged"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_integrated_accepts_the_main_tip() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");
    initialize_main(&root);
    let head = current_head(&root);

    let integrated = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        head.clone(),
        "main tip integration".into(),
    );
    assert!(integrated.ok, "{integrated:?}");
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["task"].status, "merged");
    assert_eq!(
        state.task_lifecycle["task"].integration_commit.as_deref(),
        Some(head.as_str())
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_integrated_accepts_a_real_merge_commit_and_the_merged_candidate() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");
    initialize_main(&root);

    git_ok(&root, &["checkout", "-q", "-b", "candidate"]);
    git_ok(&root, &["commit", "--allow-empty", "-q", "-m", "candidate"]);
    let candidate = current_head(&root);
    git_ok(&root, &["checkout", "-q", "main"]);
    git_ok(
        &root,
        &[
            "merge",
            "--no-ff",
            "-q",
            "candidate",
            "-m",
            "merge candidate",
        ],
    );
    let merge_commit = current_head(&root);
    assert_ne!(candidate, merge_commit);
    // Main keeps moving after the merge, so neither the merge commit nor the
    // candidate is the current main tip when integration is recorded.
    git_ok(
        &root,
        &["commit", "--allow-empty", "-q", "-m", "main moves on"],
    );
    let main_tip = current_head(&root);
    assert_eq!(rev_parse(&root, "refs/heads/main"), main_tip);
    assert_ne!(main_tip, merge_commit);

    let via_merge = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        merge_commit.clone(),
        "merge commit integration".into(),
    );
    assert!(via_merge.ok, "{via_merge:?}");
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["task"]
            .integration_commit
            .as_deref(),
        Some(merge_commit.as_str())
    );

    // The same accepted task can be re-recorded with the candidate SHA that
    // the merge brought in, because it is reachable from refs/heads/main.
    {
        let mut state = server.state.lock().unwrap();
        let mut task = state.tasks.remove("task").unwrap();
        task.status = "accepted".into();
        state.tasks.insert(task.id.clone(), task);
    }
    let via_candidate = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        candidate.clone(),
        "candidate SHA integration".into(),
    );
    assert!(via_candidate.ok, "{via_candidate:?}");
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["task"]
            .integration_commit
            .as_deref(),
        Some(candidate.as_str())
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_integrated_rejects_a_commit_not_reachable_from_main_with_an_actionable_error() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");
    initialize_main(&root);
    let main_head = current_head(&root);

    git_ok(&root, &["checkout", "-q", "--orphan", "orphan"]);
    git_ok(&root, &["commit", "--allow-empty", "-q", "-m", "orphan"]);
    let orphan = current_head(&root);
    git_ok(&root, &["checkout", "-q", "main"]);

    let rejected = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        orphan.clone(),
        "orphan integration".into(),
    );
    assert!(!rejected.ok, "{rejected:?}");
    assert_eq!(
        rejected.error.as_deref(),
        Some("TASK_INTEGRATION_COMMIT_MISMATCH")
    );
    assert_eq!(rejected.data["provided"], orphan);
    assert_eq!(rejected.data["main_head"], main_head);
    assert!(
        rejected.data["expected"]
            .as_str()
            .unwrap_or_default()
            .contains("reachable from refs/heads/main"),
        "expected value must name the reachability contract: {rejected:?}"
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["task"].status,
        "accepted"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_integrated_is_correct_when_root_is_not_checked_out_on_main() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");
    initialize_main(&root);
    let main_head = current_head(&root);

    git_ok(&root, &["checkout", "-q", "-b", "topic"]);
    git_ok(&root, &["commit", "--allow-empty", "-q", "-m", "topic"]);
    let topic_head = current_head(&root);
    assert_ne!(topic_head, main_head);
    assert_eq!(rev_parse(&root, "refs/heads/main"), main_head);

    // The recorded commit is a real ancestor of main, so reachability must be
    // judged from refs/heads/main rather than from the current checkout.
    let integrated = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        topic_head.clone(),
        "integration judged from refs/heads/main".into(),
    );
    assert!(
        !integrated.ok,
        "a topic-only commit must not be accepted: {integrated:?}"
    );
    assert_eq!(
        integrated.error.as_deref(),
        Some("TASK_INTEGRATION_COMMIT_MISMATCH")
    );
    assert!(integrated.data["expected"]
        .as_str()
        .unwrap_or_default()
        .contains("reachable from refs/heads/main"));
    assert_eq!(
        server.state.lock().unwrap().tasks["task"].status,
        "accepted"
    );

    let merged = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        main_head.clone(),
        "main ancestor accepted while checked out on topic".into(),
    );
    assert!(merged.ok, "{merged:?}");
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["task"]
            .integration_commit
            .as_deref(),
        Some(main_head.as_str())
    );
    // The checkout must not be moved by the integration record.
    assert_eq!(current_head(&root), topic_head);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn delivery_review_and_exact_main_integration_are_durable() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "outsider", "%outsider");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    assert!(
        handle_task_update(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("verifying".into()),
            None,
        )
        .ok
    );
    assert!(
        handle_task_update(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("reviewed".into()),
            None,
        )
        .ok
    );
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate commit and gates passed".into()),
            Some("candidate".into()),
        )
        .ok
    );
    let denied = handle_task_review(
        &server,
        "outsider".into(),
        "token-outsider".into(),
        "task".into(),
        true,
        false,
        "outsider review".into(),
    );
    assert!(!denied.ok);
    assert!(
        handle_task_review(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            true,
            false,
            "review gates passed".into(),
        )
        .ok
    );

    for args in [
        ["init", "-q"].as_slice(),
        ["config", "user.email", "test@example.com"].as_slice(),
        ["config", "user.name", "Collab Test"].as_slice(),
        ["commit", "--allow-empty", "-q", "-m", "main"].as_slice(),
        ["branch", "-M", "main"].as_slice(),
    ] {
        assert!(Command::new("git")
            .current_dir(&root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let head = String::from_utf8(
        Command::new("git")
            .current_dir(&root)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    assert!(
        handle_task_integrated(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            head,
            "main integration verified".into(),
        )
        .ok
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["task"].status, "merged");
    let lifecycle = &state.task_lifecycle["task"];
    assert_eq!(
        lifecycle.delivery_evidence.as_deref(),
        Some("candidate commit and gates passed")
    );
    assert_eq!(lifecycle.reviewer.as_deref(), Some("owner"));
    assert_eq!(
        lifecycle.integration_evidence.as_deref(),
        Some("main integration verified")
    );
    drop(state);
    let replayed = replay(&root).unwrap();
    assert_eq!(replayed.tasks["task"].status, "merged");
    assert_eq!(
        replayed.task_lifecycle["task"]
            .integration_evidence
            .as_deref(),
        Some("main integration verified")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn accepted_task_can_return_to_rework_and_redeliver() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("first candidate".into()),
            Some("candidate".into()),
        )
        .ok
    );
    assert!(
        handle_task_review(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            true,
            false,
            "accepted".into(),
        )
        .ok
    );
    let direct_verifying = handle_task_update(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        Some("verifying".into()),
        None,
    );
    assert_eq!(
        direct_verifying.error.as_deref(),
        Some("invalid task transition accepted -> verifying")
    );
    let direct_merge = handle_task_update(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        Some("merged".into()),
        None,
    );
    assert_eq!(
        direct_merge.error.as_deref(),
        Some("use collab task review/integrated for integration-owned lifecycle transitions")
    );
    assert!(
        handle_task_update(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("rework".into()),
            Some("address review findings".into()),
        )
        .ok
    );
    for status in ["working", "verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    let redelivery = handle_task_deliver(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        Some("corrected candidate".into()),
        Some("candidate".into()),
    );
    assert!(redelivery.ok, "{}", redelivery.error.unwrap_or_default());
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["task"]
            .delivery_evidence
            .as_deref(),
        Some("corrected candidate")
    );
    std::fs::remove_dir_all(root).ok();
}
