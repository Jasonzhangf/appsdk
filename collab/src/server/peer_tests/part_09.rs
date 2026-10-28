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
    let worktree = candidate_worktree(&root, "task");
    git_ok(&worktree, &["commit", "--allow-empty", "-q", "-m", "candidate"]);
    let candidate = current_head(&worktree);
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate commit and gates passed".into()),
            Some(worktree.to_string_lossy().into_owned()),
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

    git_ok(&root, &["merge", "--ff-only", &candidate]);
    assert!(
        handle_task_integrated(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            candidate,
            "main integration verified".into(),
        )
        .ok
    );
    git_ok(&root, &["worktree", "remove", worktree.to_str().unwrap()]);
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

#[test]
fn task_lifecycle_public_read_only_delivery_reaches_no_merge_and_owner_close() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "owner", "thread-read-only").ok);
    assert!(create_task(&server, "owner", "read-only-task", "feature").ok);
    let server = Arc::new(server);

    let delivered = dispatch(
        &server,
        Req::TaskDeliver {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "read-only-task".into(),
            evidence: Some("documentation and source audit complete".into()),
            worktree: None,
            no_candidate: true,
        },
    );
    assert!(delivered.ok, "{delivered:?}");
    {
        let state = server.state.lock().unwrap();
        let lifecycle = &state.task_lifecycle["read-only-task"];
        assert_eq!(
            lifecycle.delivery_kind,
            crate::server::state::TaskDeliveryKind::ReadOnly
        );
        assert_eq!(lifecycle.delivery_commit, None);
    }

    let reviewed = dispatch(
        &server,
        Req::TaskReview {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "read-only-task".into(),
            accept: true,
            rework: false,
            evidence: "review accepted".into(),
        },
    );
    assert!(reviewed.ok, "{reviewed:?}");
    assert_eq!(reviewed.data["status"], "accepted");
    assert_eq!(reviewed.data["merge_pending"], false);
    assert_eq!(reviewed.data["disposition"], "no_merge_required");
    assert!(
        server
            .state
            .lock()
            .unwrap()
            .pending_merges
            .get("read-only-task")
            .is_none()
    );

    let closed = dispatch(
        &server,
        Req::TaskClose {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "read-only-task".into(),
            force: false,
            reason: None,
        },
    );
    assert!(closed.ok, "{closed:?}");
    assert_eq!(closed.data["disposition"], "no_merge_required");
    drop(server);

    let replayed = replay(&root).unwrap();
    assert_eq!(replayed.tasks["read-only-task"].status, "closed");
    assert_eq!(
        replayed.task_lifecycle["read-only-task"].delivery_kind,
        crate::server::state::TaskDeliveryKind::ReadOnly
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_lifecycle_public_masterless_code_integration_requires_exact_candidate() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "owner", "thread-code").ok);
    assert!(create_task(&server, "owner", "code-task", "feature").ok);
    let worktree = candidate_worktree(&root, "code-task");
    git_ok(&worktree, &["commit", "--allow-empty", "-q", "-m", "code candidate"]);
    let candidate = current_head(&worktree);
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "code-task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    let delivered = handle_task_deliver(
        &server,
        "owner".into(),
        "token-owner".into(),
        "code-task".into(),
        Some("code candidate delivered".into()),
        Some(worktree.to_string_lossy().into_owned()),
    );
    assert!(delivered.ok, "{delivered:?}");
    let reviewed = handle_task_review(
        &server,
        "owner".into(),
        "token-owner".into(),
        "code-task".into(),
        true,
        false,
        "code review accepted".into(),
    );
    assert!(reviewed.ok, "{reviewed:?}");
    git_ok(&root, &["worktree", "remove", worktree.to_str().unwrap()]);
    git_ok(&root, &["commit", "--allow-empty", "-q", "-m", "unrelated main"]);
    let unrelated_main = current_head(&root);
    assert_ne!(candidate, unrelated_main);
    let server = Arc::new(server);

    let rejected = dispatch(
        &server,
        Req::TaskIntegrated {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "code-task".into(),
            commit: unrelated_main,
            evidence: "main moved without the delivered candidate".into(),
        },
    );
    assert!(!rejected.ok, "{rejected:?}");
    assert_eq!(
        rejected.error.as_deref(),
        Some("TASK_INTEGRATION_CANDIDATE_MISMATCH")
    );
    assert_eq!(rejected.data["candidate_commit"], candidate);
    assert_eq!(
        server.state.lock().unwrap().tasks["code-task"].status,
        "accepted"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_lifecycle_public_unresolved_code_delivery_is_not_read_only() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "owner", "thread-unresolved").ok);
    assert!(create_task(&server, "owner", "unresolved-task", "feature").ok);
    let server = Arc::new(server);

    let delivered = dispatch(
        &server,
        Req::TaskDeliver {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "unresolved-task".into(),
            evidence: Some("code delivery without a resolvable candidate".into()),
            worktree: Some("/definitely-not-a-worktree".into()),
            no_candidate: false,
        },
    );
    assert!(delivered.ok, "{delivered:?}");
    {
        let state = server.state.lock().unwrap();
        let lifecycle = &state.task_lifecycle["unresolved-task"];
        assert_eq!(
            lifecycle.delivery_kind,
            crate::server::state::TaskDeliveryKind::Code
        );
        assert_eq!(lifecycle.delivery_commit, None);
    }

    let reviewed = dispatch(
        &server,
        Req::TaskReview {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "unresolved-task".into(),
            accept: true,
            rework: false,
            evidence: "review accepted".into(),
        },
    );
    assert!(reviewed.ok, "{reviewed:?}");
    assert_eq!(reviewed.data["disposition"], "integration_required");
    let closed = dispatch(
        &server,
        Req::TaskClose {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "unresolved-task".into(),
            force: false,
            reason: None,
        },
    );
    assert!(!closed.ok, "{closed:?}");
    assert_eq!(
        closed.error.as_deref(),
        Some("TASK_DELIVERY_CANDIDATE_UNRESOLVED")
    );
    std::fs::remove_dir_all(root).ok();
}
#[cfg(test)]
fn same_session_appserver_transport(session: &str, thread: &str) -> SelectedTransport {
    SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/collab-session-carrier.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some(session.into()),
        thread_id: Some(thread.into()),
        tmux_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "same session carrier fixture".into(),
    }
}

/// A later successful Register for the same project/app/session replaces the
/// previous runtime carrier. The old binding stays as retired history, while
/// the new binding becomes the only current worker for that session.
#[test]
fn later_same_session_registration_reclaims_the_previous_carrier_once() {
    let (server, root) = test_server();
    let app = AppServerId::new("tui-default").unwrap();
    let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
    let cwd = root.display().to_string();
    let transport_a = same_session_appserver_transport("shared-session", "thread-a");
    assert!(
        seed_committed_owner(
            &server,
            "carrier-a",
            "token-carrier-a",
            &transport_a,
            &cwd,
            Some(app.clone()),
        )
        .ok
    );
    let first = registered_binding(&server, "carrier-a");
    server
        .commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: first.clone(),
        }])
        .unwrap();
    let carried_name = server
        .state
        .lock()
        .unwrap()
        .name_occupancies
        .get(project_scope.as_str())
        .unwrap()
        .values()
        .find(|occupancy| occupancy.worker_id == "carrier-a")
        .unwrap()
        .name
        .clone();

    let transport_b = same_session_appserver_transport("shared-session", "thread-b");
    let prepared_b = prepare_register_typed(
        &server,
        "carrier-b",
        "token-carrier-b",
        &transport_b,
        &cwd,
        Some(project_scope.clone()),
        Some(app.clone()),
        false,
    )
    .unwrap();
    let registered = consume_prepared_register_typed(
        &server,
        "carrier-b",
        &transport_b,
        prepared_b.clone(),
        None,
    );
    assert!(registered.ok, "{registered:?}");
    let journal_before_retry = std::fs::read(&server.journal_path).unwrap();
    let retried = consume_prepared_register_typed(
        &server,
        "carrier-b",
        &transport_b,
        prepared_b,
        None,
    );
    assert!(retried.ok, "{retried:?}");
    assert_eq!(retried.data["replayed"], true);
    assert_eq!(
        std::fs::read(&server.journal_path).unwrap(),
        journal_before_retry,
        "an identical Register retry must not append a second carrier takeover"
    );

    let state = server.state.lock().unwrap();
    let retired = state
        .global
        .lookup_binding_for(&first.route_scope(), &first.binding_id)
        .expect("the previous carrier binding remains as history");
    assert_eq!(retired.endpoint_generation, first.endpoint_generation + 1);
    assert!(retired.native_thread_id.is_none());
    assert!(retired.tmux_endpoint.is_none());
    assert!(!state.workers.contains_key("carrier-a"));
    assert!(state.workers.contains_key("carrier-b"));
    assert!(state
        .global
        .current_thread_routes
        .values()
        .all(|binding| binding.binding_id != first.binding_id));
    let session_holders = state
        .name_occupancies
        .get(project_scope.as_str())
        .unwrap()
        .values()
        .filter(|occupancy| occupancy.session_id == "shared-session")
        .collect::<Vec<_>>();
    assert_eq!(session_holders.len(), 1);
    assert_eq!(session_holders[0].worker_id, "carrier-b");
    assert_eq!(session_holders[0].name, carried_name);
    drop(state);

    let replayed = replay(&root).unwrap();
    let replayed_holders = replayed
        .name_occupancies
        .get(project_scope.as_str())
        .unwrap()
        .values()
        .filter(|occupancy| occupancy.session_id == "shared-session")
        .collect::<Vec<_>>();
    assert_eq!(
        replayed_holders.len(),
        1,
        "journal replay must keep one name carrier per session"
    );
    assert_eq!(replayed_holders[0].worker_id, "carrier-b");
    assert!(!replayed.workers.contains_key("carrier-a"));
    assert!(replayed
        .global
        .current_thread_routes
        .values()
        .all(|binding| binding.binding_id != first.binding_id));
    std::fs::remove_dir_all(root).unwrap();
}

/// Reclaiming is limited to the exact project/app/session. Another session or
/// another project keeps its current carrier, and a same-binding retry stays
/// idempotent instead of reclaiming the registrant itself.
#[test]
fn same_session_carrier_reclaim_is_scoped_and_self_idempotent() {
    let (server, root) = test_server();
    let app = AppServerId::new("tui-default").unwrap();
    let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
    let cwd = root.display().to_string();
    let transport_a = same_session_appserver_transport("session-isolated", "thread-a");
    let prepared_a = prepare_register_typed(
        &server,
        "carrier-a",
        "token-carrier-a",
        &transport_a,
        &cwd,
        Some(project_scope.clone()),
        Some(app.clone()),
        false,
    )
    .unwrap();
    assert!(server
        .consume_prepared_register_with_approval(prepared_a.clone(), None)
        .is_ok());
    let first = registered_binding(&server, "carrier-a");

    let retry_prepared = prepare_register_typed(
        &server,
        "carrier-a",
        "token-carrier-a",
        &transport_a,
        &cwd,
        Some(project_scope.clone()),
        Some(app.clone()),
        true,
    )
    .unwrap();
    assert!(server
        .consume_prepared_register_with_approval(retry_prepared, None)
        .unwrap()
        .replayed);
    assert_eq!(registered_binding(&server, "carrier-a"), first);

    let other_session = same_session_appserver_transport("session-other", "thread-b");
    let prepared_b = prepare_register_typed(
        &server,
        "carrier-b",
        "token-carrier-b",
        &other_session,
        &cwd,
        Some(project_scope.clone()),
        Some(app.clone()),
        false,
    )
    .unwrap();
    assert!(server
        .consume_prepared_register_with_approval(prepared_b, None)
        .is_ok());
    assert_eq!(registered_binding(&server, "carrier-a"), first);
    assert!(server.state.lock().unwrap().workers.contains_key("carrier-a"));

    let other_project = ProjectScopeId::new("/other-project").unwrap();
    let same_session_other_project =
        same_session_appserver_transport("session-isolated", "thread-c");
    let prepared_c = prepare_register_typed(
        &server,
        "carrier-c",
        "token-carrier-c",
        &same_session_other_project,
        &cwd,
        Some(other_project.clone()),
        Some(app),
        false,
    )
    .unwrap();
    assert!(server
        .consume_prepared_register_with_approval(prepared_c, None)
        .is_ok());
    assert_eq!(registered_binding(&server, "carrier-a"), first);
    assert!(server.state.lock().unwrap().workers.contains_key("carrier-a"));
    assert!(server
        .state
        .lock()
        .unwrap()
        .global
        .lookup_project(&other_project)
        .unwrap()
        .runtime_bindings
        .values()
        .any(|binding| binding.agent_id.as_str() == "carrier-c"));
    std::fs::remove_dir_all(root).unwrap();
}

/// A publisher-owned pending task with no bound worktree can be cancelled
/// through the owner-local update path. The `pending -> cancelled` edge is the
/// only legal unaccepted cancellation and must persist through journal replay.
#[test]
fn pending_task_without_worktree_transitions_to_cancelled() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "publisher", "thread-pending-cancel").ok);
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "pending-cancel".into(),
            owner: "publisher".into(),
            created_by: "publisher".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: default_priority(),
            status: "pending".into(),
            next_step: None,
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);

    let cancelled = handle_task_update(
        &server,
        "publisher".into(),
        "token-publisher".into(),
        "pending-cancel".into(),
        Some("cancelled".into()),
        None,
    );
    assert!(cancelled.ok, "{cancelled:?}");
    assert_eq!(cancelled.data["status"], "cancelled");
    assert_eq!(
        server.state.lock().unwrap().tasks["pending-cancel"].status,
        "cancelled"
    );

    let replayed = replay(&root).unwrap();
    assert_eq!(replayed.tasks["pending-cancel"].status, "cancelled");
    std::fs::remove_dir_all(root).ok();
}

/// Publisher-owned pending work remains claimable by its publisher, and once
/// claimed is no longer claimable by another registered peer at the advanced
/// revision. This guards `board_claim`, which the fix intentionally leaves
/// unchanged.
#[test]
fn publisher_claims_pending_task_and_non_publisher_cannot_reclaim() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "publisher", "thread-pub-claim").ok);
    assert!(register_synthetic_peer(&server, "other", "thread-other-claim").ok);

    let published = handle_board_command(
        &server,
        "publisher".into(),
        "token-publisher".into(),
        crate::board::BoardCommand::Publish {
            id: "publisher-pending".into(),
            title: "publisher pending".into(),
            description: "claim regression".into(),
            delivery_condition: "claim works".into(),
            test_condition: "green".into(),
            priority: "p1".into(),
            work_kind: None,
            parent_task: None,
            prefer_role: None,
            executor: None,
            reviewer: None,
        },
    );
    assert!(published.ok, "{published:?}");
    assert_eq!(published.data["task"]["status"], "pending");
    assert_eq!(published.data["task"]["owner"], "publisher");

    let claimed = handle_board_command(
        &server,
        "publisher".into(),
        "token-publisher".into(),
        crate::board::BoardCommand::Claim {
            id: "publisher-pending".into(),
            expected_revision: 1,
        },
    );
    assert!(claimed.ok, "{claimed:?}");
    assert_eq!(claimed.data["task"]["status"], "working");
    assert_eq!(claimed.data["task"]["owner"], "publisher");

    let non_publisher = handle_board_command(
        &server,
        "other".into(),
        "token-other".into(),
        crate::board::BoardCommand::Claim {
            id: "publisher-pending".into(),
            expected_revision: 2,
        },
    );
    assert!(!non_publisher.ok, "{non_publisher:?}");
    assert_eq!(
        non_publisher.error.as_deref(),
        Some("BOARD_TASK_NOT_PENDING: only pending publisher-owned work may be claimed")
    );
    std::fs::remove_dir_all(root).ok();
}

/// A private (non-public) pending task owned by its creator is claimable only
/// by its publisher. `board_claim` rejects every other actor with
/// `BOARD_PRIVATE_TASK` before the pending/ownership checks. The task is seeded
/// directly so the claim entry itself is exercised; this guards the privacy
/// branch that the two-line lifecycle fix intentionally leaves unchanged.
#[test]
fn non_publisher_cannot_claim_private_pending_task() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "publisher", "thread-private-claim").ok);
    assert!(register_synthetic_peer(&server, "other", "thread-private-other").ok);
    let now = now_ms();
    server.commit(&[
        Event::TaskCreated {
            task: TaskRec {
                id: "private-pending".into(),
                owner: "publisher".into(),
                created_by: "publisher".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: default_priority(),
                status: "pending".into(),
                next_step: None,
                wait: None,
                created_ms: now,
                updated_ms: now,
            },
        },
        Event::BoardDetailsChanged {
            task_id: "private-pending".into(),
            details: crate::board::BoardTaskDetails {
                public_visibility: false,
                revision: 1,
                ..Default::default()
            },
        },
    ]);

    let rejected = board_claim(
        &server,
        "other".into(),
        "token-other".into(),
        "private-pending".into(),
        1,
    );
    assert!(
        !rejected.ok,
        "non-publisher must not claim private work: {rejected:?}"
    );
    assert_eq!(
        rejected.error.as_deref(),
        Some("BOARD_PRIVATE_TASK: private queue work is claimable only by its publisher")
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["private-pending"].status,
        "pending",
        "a rejected private claim must not mutate the task"
    );

    let claimed = board_claim(
        &server,
        "publisher".into(),
        "token-publisher".into(),
        "private-pending".into(),
        1,
    );
    assert!(
        claimed.ok,
        "publisher must claim own private pending work: {claimed:?}"
    );
    assert_eq!(claimed.data["task"]["status"], "working");
    assert_eq!(claimed.data["task"]["owner"], "publisher");
    assert_eq!(
        server.state.lock().unwrap().tasks["private-pending"].status,
        "working"
    );
    std::fs::remove_dir_all(root).ok();
}
