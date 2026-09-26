#[test]
fn context_rearms_missing_default_lease_for_reused_registration() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);

    // Simulate the reused-registration path: the peer is already registered,
    // so `collab context` never re-enters the Register command, while the
    // system-owned default lease was lost. Context must restore it.
    server.commit(&[Event::NotificationStatus {
        subscription_id: "sub-default-direct-message-peer".into(),
        status: "cancelled".into(),
        updated_ms: now_ms(),
    }]);
    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.notification_subscriptions["sub-default-direct-message-peer"].status,
            "cancelled"
        );
    }

    let context = handle_context(&server, "peer".into(), "token-peer".into());
    assert!(context.ok, "{}", context.error.unwrap_or_default());

    let state = server.state.lock().unwrap();
    let lease = &state.notification_subscriptions["sub-default-direct-message-peer"];
    assert_eq!(lease.status, "armed");
    assert_eq!(lease.worker_id, "peer");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn context_rearm_never_overrides_a_concurrent_explicit_unsubscribe() {
    use std::sync::Arc as StdArc;

    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);
    let server = StdArc::new(server);

    // Race an explicit owner unsubscribe against the context rearm. The
    // unsubscribe must always be the final durable state: the rearm runs
    // under the same state lock, so it can never apply a NotificationSubscribed
    // event after the newer explicit-unsubscribe suppression.
    let unsub_server = StdArc::clone(&server);
    let unsubscribe = std::thread::spawn(move || {
        handle_notification_unsubscribe(
            &unsub_server,
            "peer".into(),
            "token-peer".into(),
            "sub-default-direct-message-peer".into(),
        )
    });
    let context = handle_context(&server, "peer".into(), "token-peer".into());
    assert!(context.ok, "{}", context.error.unwrap_or_default());
    let cancelled = unsubscribe.join().unwrap();
    assert!(cancelled.ok, "{}", cancelled.error.unwrap_or_default());

    // After both operations settle, one more context must still respect the
    // explicit unsubscribe and leave the lease stopped.
    let context = handle_context(&server, "peer".into(), "token-peer".into());
    assert!(context.ok, "{}", context.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    let lease = &state.notification_subscriptions["sub-default-direct-message-peer"];
    assert_eq!(lease.status, "cancelled");
    assert_eq!(lease.status_reason.as_deref(), Some("explicit-unsubscribe"));
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn context_keeps_explicit_unsubscribe_stopped() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);
    let cancelled = handle_notification_unsubscribe(
        &server,
        "peer".into(),
        "token-peer".into(),
        "sub-default-direct-message-peer".into(),
    );
    assert!(cancelled.ok, "{}", cancelled.error.unwrap_or_default());

    let context = handle_context(&server, "peer".into(), "token-peer".into());
    assert!(context.ok, "{}", context.error.unwrap_or_default());

    let state = server.state.lock().unwrap();
    let lease = &state.notification_subscriptions["sub-default-direct-message-peer"];
    assert_eq!(lease.status, "cancelled");
    assert_eq!(lease.status_reason.as_deref(), Some("explicit-unsubscribe"));
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn daemon_replay_restores_default_lease_for_registered_peer() {
    let mut state = State::default();
    state.apply(&Event::Registered {
        worker: WorkerRec {
            id: "peer".into(),
            token: "token-peer".into(),
            cwd: "/tmp".into(),
            registered_ms: 1,
            transport: Some(test_tmux_transport("thread-peer")),
        },
    });

    let events = registered_peer_default_events(&state, 10_000);
    assert!(events.iter().any(|event| matches!(
        event,
        Event::NotificationSubscribed { subscription }
            if subscription.id == "sub-default-direct-message-peer"
    )));
}

#[test]
fn daemon_restart_restores_default_lease_from_registered_tmux_transport() {
    let mut state = State::default();
    state.apply(&Event::Registered {
        worker: WorkerRec {
            id: "peer".into(),
            token: "token-peer".into(),
            cwd: "/tmp".into(),
            registered_ms: 1,
            transport: Some(test_tmux_transport("thread-current")),
        },
    });

    let lease_events = registered_peer_default_events(&state, 10_000);
    assert!(lease_events.iter().any(|event| matches!(
        event,
        Event::NotificationSubscribed { subscription }
            if subscription.worker_id == "peer" && subscription.target == "thread-current"
    )));
}

#[test]
fn daemon_restart_reuses_existing_deadline_without_recreating_it() {
    let mut state = State::default();
    state.apply(&Event::Registered {
        worker: WorkerRec {
            id: "peer".into(),
            token: "token-peer".into(),
            cwd: "/tmp".into(),
            registered_ms: 1,
            transport: Some(test_tmux_transport("thread-current")),
        },
    });
    let original = NotificationSubscription {
        id: "sub-goal".into(),
        worker_id: "peer".into(),
        event: "deadline".into(),
        subject: Some("goal:sha256:test".into()),
        target: "thread-current".into(),
        method: "appserver".into(),
        trigger_ms: Some(20_000),
        trigger_times_ms: Vec::new(),
        interval_ms: None,
        repeat_count: 1,
        fired_count: 0,
        expires_ms: 60_000,
        status: "armed".into(),
        created_ms: 1,
        updated_ms: 1,
        status_reason: None,
    };
    state.apply(&Event::NotificationSubscribed {
        subscription: original.clone(),
    });

    let events = registered_peer_default_events(&state, 10_000);
    assert!(!events.iter().any(|event| matches!(
        event,
        Event::NotificationSubscribed { subscription }
            if subscription.id == "sub-goal"
    )));
    let rebound = &state.notification_subscriptions["sub-goal"];
    assert_eq!(rebound.id, original.id);
    assert_eq!(rebound.target, "thread-current");
    assert_eq!(rebound.trigger_ms, original.trigger_ms);
    assert_eq!(rebound.expires_ms, original.expires_ms);
    assert_eq!(rebound.status, "armed");
}

#[test]
fn daemon_restart_does_not_restore_without_registered_transport() {
    let mut state = State::default();
    state.apply(&Event::Registered {
        worker: WorkerRec {
            id: "peer".into(),
            token: "token-peer".into(),
            cwd: "/tmp".into(),
            registered_ms: 1,
            transport: None,
        },
    });

    assert!(registered_peer_default_events(&state, 10_000).is_empty());
}

#[test]
fn expired_mailbox_and_journal_are_removed_and_do_not_replay() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);
    let now = now_ms();
    let old_id = "m-old".to_string();
    let fresh_id = "m-fresh".to_string();
    server.commit(&[
        Event::Sent {
            msg: Message {
                id: old_id.clone(),
                from: "peer".into(),
                to: "peer".into(),
                mtype: "notify".into(),
                subject: Some("old".into()),
                body: "expired body".into(),
                in_reply_to: None,
                created_ms: now - 8 * 86_400_000,
                state: "read".into(),
                wake_attempt_count: 1,
                last_wake_attempt_ms: now - 8 * 86_400_000,
                retry_attempted: false,
            },
        },
        Event::Sent {
            msg: Message {
                id: fresh_id.clone(),
                from: "peer".into(),
                to: "peer".into(),
                mtype: "notify".into(),
                subject: Some("new".into()),
                body: "fresh body".into(),
                in_reply_to: None,
                created_ms: now,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::DeliveryMode {
            msg_id: old_id.clone(),
            mode: "explicit-notification".into(),
            source_thread_id: Some("thread-source".into()),
        },
    ]);
    let mailbox = root.join(".agent-collab/mailbox");
    assert!(mailbox.join("m-old.json").exists());
    assert!(mailbox.join("m-fresh.json").exists());
    let journal_before = std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
        .unwrap()
        .lines()
        .count();

    assert_eq!(purge_expired_storage(&server, now), 1);
    let state = server.state.lock().unwrap();
    assert!(!state.msgs.contains_key(&old_id));
    assert!(!state.delivery_source_threads.contains_key(&old_id));
    assert!(state.msgs.contains_key(&fresh_id));
    assert!(state.workers.contains_key("peer"));
    drop(state);
    assert!(!mailbox.join("m-old.json").exists());
    assert!(mailbox.join("m-fresh.json").exists());
    let journal = std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    assert!(!journal.contains("m-old"));
    assert!(journal.contains("m-fresh"));
    assert!(journal.lines().count() < journal_before);

    let replayed = replay(&root).unwrap();
    assert!(!replayed.msgs.contains_key(&old_id));
    assert!(!replayed.delivery_source_threads.contains_key(&old_id));
    assert_eq!(replayed.msgs[&fresh_id].body, "fresh body");
    assert_eq!(
        replayed.workers["peer"]
            .transport
            .as_ref()
            .and_then(|transport| transport.thread_id.as_deref()),
        Some("thread-peer")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn retention_skips_fresh_messages_and_frozen_admission() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);
    let now = now_ms();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "m-keep".into(),
            from: "peer".into(),
            to: "peer".into(),
            mtype: "notify".into(),
            subject: Some("keep".into()),
            body: "keep".into(),
            in_reply_to: None,
            created_ms: now - 86_400_000,
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    assert_eq!(purge_expired_storage(&server, now), 0);
    assert!(server.state.lock().unwrap().msgs.contains_key("m-keep"));

    server.commit(&[Event::Sent {
        msg: Message {
            id: "m-old-frozen".into(),
            from: "peer".into(),
            to: "peer".into(),
            mtype: "notify".into(),
            subject: Some("old".into()),
            body: "old".into(),
            in_reply_to: None,
            created_ms: now - 8 * 86_400_000,
            state: "read".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    server.commit(&[Event::MigrationUpdated {
        migration: MigrationRecord {
            id: "migration".into(),
            from_version: "v1".into(),
            to_version: "v1".into(),
            phase: "applied".into(),
            admission_frozen: true,
            snapshot_hash: None,
            worker_count: 1,
            task_count: 0,
            message_count: 2,
            operator: "peer".into(),
            issues: Vec::new(),
            created_ms: now,
            updated_ms: now,
        },
    }]);
    assert_eq!(purge_expired_storage(&server, now), 0);
    assert!(server
        .state
        .lock()
        .unwrap()
        .msgs
        .contains_key("m-old-frozen"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn fresh_default_does_not_skip_legacy_duplicate_cleanup() {
    let mut state = State::default();
    for event in
        default_direct_message_events(&state, "peer", &test_tmux_transport("thread-one"), 1000)
    {
        state.apply(&event);
    }
    let mut old = state
        .notification_subscriptions
        .values()
        .next()
        .unwrap()
        .clone();
    old.id = "sub-legacy".into();
    state.apply(&Event::NotificationSubscribed { subscription: old });
    for event in
        default_direct_message_events(&state, "peer", &test_tmux_transport("thread-one"), 2000)
    {
        state.apply(&event);
    }
    assert_eq!(
        state
            .notification_subscriptions
            .values()
            .filter(|sub| sub.status == "armed")
            .count(),
        1
    );
    assert_eq!(
        state.notification_subscriptions["sub-legacy"].status,
        "rebound"
    );
    assert!(default_direct_message_events(
        &state,
        "peer",
        &test_tmux_transport("thread-one"),
        2000
    )
    .is_empty());
}

#[test]
fn default_subscription_renews_matching_target_and_keeps_stale_target_visible() {
    let mut state = State::default();
    for event in default_direct_message_events(
        &state,
        "peer",
        &test_appserver_transport("thread-one"),
        1000,
    ) {
        state.apply(&event);
    }
    let id = state
        .notification_subscriptions
        .keys()
        .next()
        .unwrap()
        .clone();
    let ttl = DEFAULT_DIRECT_MESSAGE_TTL_SECONDS as i64 * 1000;
    for (thread_id, time) in [("thread-one", ttl), ("thread-one", ttl * 3)] {
        for event in
            default_direct_message_events(&state, "peer", &test_tmux_transport(thread_id), time)
        {
            state.apply(&event);
        }
        assert_eq!(state.notification_subscriptions.len(), 1);
        let sub = &state.notification_subscriptions[&id];
        assert_eq!(sub.target, thread_id);
        assert_eq!(sub.status, "armed");
        assert_eq!(sub.expires_ms, time + ttl);
    }

    // A changed peer identity is a route change. The system-owned wake lease
    // must move to the newly registered tmux route instead of keeping a stale
    // AppServer-era target.
    let events =
        default_direct_message_events(&state, "peer", &test_tmux_transport("thread-two"), ttl * 4);
    assert!(events.iter().any(|event| matches!(event, Event::NotificationSubscribed { subscription }
        if subscription.id == id && subscription.target == "thread-two" && subscription.method == "tmux")));
    for event in &events {
        state.apply(event);
    }
    assert_eq!(state.notification_subscriptions[&id].target, "thread-two");
    assert_eq!(state.notification_subscriptions[&id].method, "tmux");
    assert_eq!(state.notification_subscriptions.len(), 1);
}

#[test]
fn replay_removes_legacy_declared_roles() {
    let mut state = State::default();
    for (id, role) in [("legacy-master", "master"), ("legacy-worker", "worker")] {
        let json = format!(
            r#"{{"ev":"Registered","worker":{{"id":"{id}","token":"token-{id}","pane":"%{id}","cwd":"/tmp","registered_ms":1,"role":"{role}"}}}}"#
        );
        let event: Event = serde_json::from_str(&json).unwrap();
        state.apply(&event);
    }
    assert!(state
        .workers
        .values()
        .all(|worker| { serde_json::to_value(worker).unwrap().get("role").is_none() }));
}

#[test]
fn only_owner_mutates_and_closes_task() {
    let (server, root) = test_server();
    register(&server, "peer-a", "%peer-a");
    register(&server, "peer-b", "%peer-b");
    assert!(create_task(&server, "peer-a", "task-a", "feature-a").ok);

    let update = handle_task_update(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "task-a".into(),
        Some("verifying".into()),
        None,
    );
    assert!(!update.ok);
    let close = handle_task_close(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "task-a".into(),
        false,
        None,
    );
    assert!(!close.ok);
    assert_eq!(
        server.state.lock().unwrap().tasks["task-a"].status,
        "working"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_completes_local_lifecycle_without_peer_reports() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "task", "feature").ok);
    assert!(
        handle_task_update(
            &server,
            "peer".into(),
            "token-peer".into(),
            "task".into(),
            Some("verifying".into()),
            Some("continue verifying".into()),
        )
        .ok
    );
    let delivered = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        Some("tests and candidate commit verified".into()),
        Some("/tmp/task-worktree".into()),
    );
    assert!(delivered.ok);
    assert_eq!(delivered.data["notification"], "none");
    assert!(
        handle_task_review(
            &server,
            "peer".into(),
            "token-peer".into(),
            "task".into(),
            true,
            false,
            "reviewed candidate".into(),
        )
        .ok
    );
    initialize_main(&root);
    assert!(
        handle_task_integrated(
            &server,
            "peer".into(),
            "token-peer".into(),
            "task".into(),
            current_head(&root),
            "main verified".into(),
        )
        .ok
    );
    let closed = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        false,
        None,
    );
    assert!(closed.ok);
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["task"].status, "closed");
    assert_eq!(state.cleanup_receipts["task"].task_id, "task");
    assert!(
        state.msgs.is_empty(),
        "normal lifecycle must not report to peers"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_close_uses_integrated_main_not_daemon_head_for_cleanup() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    initialize_main(&root);
    let base = current_head(&root);
    git_ok(&root, &["checkout", "-q", "-b", "codex/cleanup-live"]);
    std::fs::write(root.join("cleanup-live.txt"), "merged task\n").unwrap();
    git_ok(&root, &["add", "cleanup-live.txt"]);
    git_ok(&root, &["commit", "-q", "-m", "cleanup task"]);
    git_ok(&root, &["checkout", "-q", "main"]);
    git_ok(&root, &["merge", "--ff-only", "codex/cleanup-live"]);
    let main_commit = rev_parse(&root, "refs/heads/main");
    git_ok(&root, &["checkout", "-q", "-b", "root-snapshot", &base]);
    std::fs::create_dir_all(root.join("playground")).unwrap();
    let worktree = root.join("playground/cleanup-live");
    let worktree_string = worktree.display().to_string();
    git_ok(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            &worktree_string,
            "codex/cleanup-live",
        ],
    );

    let registered = handle_task_register(
        &server,
        "peer".into(),
        "token-peer".into(),
        "cleanup-live".into(),
        None,
        Some("feature".into()),
        Some("playground/cleanup-live".into()),
        Some("codex/cleanup-live".into()),
        Some(base),
        default_priority(),
    );
    assert!(registered.ok, "{}", registered.error.unwrap_or_default());
    let registered_worktree = server.state.lock().unwrap().tasks["cleanup-live"]
        .worktree_path
        .clone()
        .unwrap();
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "peer".into(),
                "token-peer".into(),
                "cleanup-live".into(),
                Some(status.into()),
                Some(format!("continue {status}")),
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "peer".into(),
            "token-peer".into(),
            "cleanup-live".into(),
            Some("candidate verified".into()),
            Some(registered_worktree),
        )
        .ok
    );
    assert!(
        handle_task_review(
            &server,
            "peer".into(),
            "token-peer".into(),
            "cleanup-live".into(),
            true,
            false,
            "review pass".into(),
        )
        .ok
    );
    assert!(
        handle_task_integrated(
            &server,
            "peer".into(),
            "token-peer".into(),
            "cleanup-live".into(),
            main_commit,
            "main verified".into(),
        )
        .ok
    );

    let closed = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "cleanup-live".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert!(!worktree.exists());
    let branch_after_close = Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--verify", "refs/heads/codex/cleanup-live"])
        .output()
        .unwrap();
    assert!(!branch_after_close.status.success());
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["cleanup-live"].status, "closed");
    assert_eq!(
        state.cleanup_receipts["cleanup-live"].verification,
        crate::server::state::CleanupVerification::Verified
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn live_master_closes_merged_task_with_verified_cleanup() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master close test");
    initialize_main(&root);
    let base = current_head(&root);
    git_ok(&root, &["checkout", "-q", "-b", "codex/master-cleanup"]);
    std::fs::write(root.join("master-cleanup.txt"), "merged task\n").unwrap();
    git_ok(&root, &["add", "master-cleanup.txt"]);
    git_ok(&root, &["commit", "-q", "-m", "master cleanup task"]);
    git_ok(&root, &["checkout", "-q", "main"]);
    git_ok(&root, &["merge", "--ff-only", "codex/master-cleanup"]);
    let main_commit = rev_parse(&root, "refs/heads/main");
    std::fs::create_dir_all(root.join("playground")).unwrap();
    let worktree = root.join("playground/master-cleanup");
    let worktree_string = worktree.display().to_string();
    git_ok(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            &worktree_string,
            "codex/master-cleanup",
        ],
    );

    let registered = handle_task_register(
        &server,
        "owner".into(),
        "token-owner".into(),
        "master-cleanup".into(),
        None,
        Some("feature".into()),
        Some("playground/master-cleanup".into()),
        Some("codex/master-cleanup".into()),
        Some(base),
        default_priority(),
    );
    assert!(registered.ok, "{}", registered.error.unwrap_or_default());
    let registered_worktree = server.state.lock().unwrap().tasks["master-cleanup"]
        .worktree_path
        .clone()
        .unwrap();
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "master-cleanup".into(),
                Some(status.into()),
                Some(format!("continue {status}")),
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "master-cleanup".into(),
            Some("candidate verified".into()),
            Some(registered_worktree),
        )
        .ok
    );
    assert!(
        handle_task_review(
            &server,
            "owner".into(),
            "token-owner".into(),
            "master-cleanup".into(),
            true,
            false,
            "review pass".into(),
        )
        .ok
    );
    assert!(
        handle_task_integrated(
            &server,
            "master".into(),
            "token-master".into(),
            "master-cleanup".into(),
            main_commit,
            "main verified".into(),
        )
        .ok
    );

    let closed = handle_task_close(
        &server,
        "master".into(),
        "token-master".into(),
        "master-cleanup".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert!(!worktree.exists());
    let branch_after_close = Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--verify", "refs/heads/codex/master-cleanup"])
        .output()
        .unwrap();
    assert!(!branch_after_close.status.success());
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["master-cleanup"].owner, "owner");
    assert_eq!(state.tasks["master-cleanup"].status, "closed");
    assert_eq!(
        state.cleanup_receipts["master-cleanup"].verification,
        crate::server::state::CleanupVerification::Verified
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_close_records_verified_receipt_after_prior_safe_cleanup() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    initialize_main(&root);
    let base = current_head(&root);
    git_ok(&root, &["checkout", "-q", "-b", "codex/already-clean"]);
    std::fs::write(root.join("already-clean.txt"), "merged task\n").unwrap();
    git_ok(&root, &["add", "already-clean.txt"]);
    git_ok(&root, &["commit", "-q", "-m", "already clean task"]);
    git_ok(&root, &["checkout", "-q", "main"]);
    git_ok(&root, &["merge", "--ff-only", "codex/already-clean"]);
    let main_commit = rev_parse(&root, "refs/heads/main");
    std::fs::create_dir_all(root.join("playground")).unwrap();
    let worktree = root.join("playground/already-clean");
    let worktree_string = worktree.display().to_string();
    git_ok(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            &worktree_string,
            "codex/already-clean",
        ],
    );

    let registered = handle_task_register(
        &server,
        "peer".into(),
        "token-peer".into(),
        "already-clean".into(),
        None,
        Some("feature".into()),
        Some("playground/already-clean".into()),
        Some("codex/already-clean".into()),
        Some(base),
        default_priority(),
    );
    assert!(registered.ok, "{}", registered.error.unwrap_or_default());
    let registered_worktree = server.state.lock().unwrap().tasks["already-clean"]
        .worktree_path
        .clone()
        .unwrap();
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "peer".into(),
                "token-peer".into(),
                "already-clean".into(),
                Some(status.into()),
                Some(format!("continue {status}")),
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "peer".into(),
            "token-peer".into(),
            "already-clean".into(),
            Some("candidate verified".into()),
            Some(registered_worktree),
        )
        .ok
    );
    assert!(
        handle_task_review(
            &server,
            "peer".into(),
            "token-peer".into(),
            "already-clean".into(),
            true,
            false,
            "review pass".into(),
        )
        .ok
    );
    assert!(
        handle_task_integrated(
            &server,
            "peer".into(),
            "token-peer".into(),
            "already-clean".into(),
            main_commit,
            "main verified".into(),
        )
        .ok
    );
    git_ok(&root, &["worktree", "remove", &worktree_string]);
    git_ok(&root, &["branch", "-D", "codex/already-clean"]);

    let closed = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "already-clean".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["already-clean"].status, "closed");
    assert_eq!(
        state.cleanup_receipts["already-clean"].verification,
        crate::server::state::CleanupVerification::Verified
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn master_force_close_skips_owner_and_cleanup_requirements() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved force-close test");
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "stuck".into(),
            owner: "owner".into(),
            created_by: "owner".into(),
            feature_id: None,
            worktree_path: Some("playground/stuck".into()),
            branch: Some("codex/stuck".into()),
            base_commit: Some("base".into()),
            priority: default_priority(),
            status: "blocked".into(),
            next_step: None,
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
    let resp = handle_task_close(
        &server,
        "master".into(),
        "token-master".into(),
        "stuck".into(),
        true,
        Some("worktree dirty and merge blocked; force closing per master".into()),
    );
    assert!(resp.ok, "{}", resp.error.unwrap_or_default());
    assert_eq!(resp.data["status"], "closed");
    assert_eq!(resp.data["cleanup"]["result"], "unverified");
    assert_eq!(
        resp.data["next_action"],
        "manual close recorded; worktree/branch cleanup remains unverified"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["stuck"].status, "closed");
    assert_eq!(
        state.cleanup_receipts["stuck"].manual_reason.as_deref(),
        Some("worktree dirty and merge blocked; force closing per master"),
    );
    assert_eq!(
        state.cleanup_receipts["stuck"].verification,
        crate::server::state::CleanupVerification::Unverified
    );
    let projected = task_view(&state, &state.tasks["stuck"]);
    assert_eq!(projected["cleanup"]["status"], "unverified");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

/// Force-close a holder that declares no worktree/branch, leaving one waiter
/// blocked on it with a resource-released subscription. This is the fixture the
/// finalize regressions build on.
fn force_closed_unverified_holder_with_waiter(server: &Server) -> Vec<String> {
    register(server, "holder", "%holder");
    register(server, "waiter", "%waiter");
    register(server, "master", "%master");
    promote_master(server, "master", "user approved finalize test");
    assert!(create_task(server, "holder", "held", "shared-feature").ok);
    assert!(!create_task(server, "waiter", "waiting", "shared-feature").ok);
    assert!(
        handle_task_wait(
            server,
            "waiter".into(),
            "token-waiter".into(),
            "waiting".into(),
            "held".into(),
        )
        .ok
    );
    assert!(
        handle_notification_subscribe(
            server,
            "waiter".into(),
            "token-waiter".into(),
            "resource-released".into(),
            Some("held".into()),
            None,
            Vec::new(),
            None,
            1,
            60,
        )
        .ok
    );
    let forced = handle_task_close(
        server,
        "master".into(),
        "token-master".into(),
        "held".into(),
        true,
        Some("holder abandoned mid-flight; force closing".into()),
    );
    assert!(forced.ok, "{}", forced.error.unwrap_or_default());
    assert_eq!(forced.data["cleanup"]["result"], "unverified");
    // A force close records the obligation; it must not release the waiter.
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["waiting"].status, "waiting");
    assert_eq!(
        state.cleanup_receipts["held"].verification,
        crate::server::state::CleanupVerification::Unverified
    );
    drop(state);
    Vec::new()
}

#[test]
fn force_close_without_finalize_reports_unverified_and_does_not_release() {
    let (server, root) = test_server();
    force_closed_unverified_holder_with_waiter(&server);
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["held"].status, "closed");
    assert_eq!(
        state.cleanup_receipts["held"].verification,
        crate::server::state::CleanupVerification::Unverified,
        "an unfinalized force close keeps the unverified receipt, not a success"
    );
    assert_eq!(state.tasks["waiting"].status, "waiting");
    assert!(
        state
            .msgs
            .values()
            .all(|message| !message.body.starts_with("RESOURCE_RELEASED ")),
        "an unverified force close must not release dependents"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn finalize_releases_a_waiter_exactly_once() {
    let (server, root) = test_server();
    force_closed_unverified_holder_with_waiter(&server);
    let finalized = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(finalized.ok, "{}", finalized.error.unwrap_or_default());
    assert_eq!(finalized.data["cleanup"]["result"], "verified");
    assert_eq!(finalized.data["finalized"], true);
    assert_eq!(finalized.data["released_dependents"], json!(["waiting"]));
    assert_eq!(
        finalized.data["cleanup"]["manual_reason"], "holder abandoned mid-flight; force closing",
        "the manual reason stays auditable after finalization"
    );

    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.tasks["waiting"].status, "blocked");
        assert!(state.tasks["waiting"].wait.is_none());
        assert_eq!(
            state.cleanup_receipts["held"].verification,
            crate::server::state::CleanupVerification::Verified
        );
        assert_eq!(
            state.cleanup_receipts["held"].manual_reason.as_deref(),
            Some("holder abandoned mid-flight; force closing")
        );
        let releases = state
            .msgs
            .values()
            .filter(|message| message.body.starts_with("RESOURCE_RELEASED "))
            .count();
        assert_eq!(releases, 1, "finalize must release the waiter once");
    }

    // A second finalize must not double-release or duplicate the notification.
    let again = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(again.ok, "{}", again.error.unwrap_or_default());
    assert_eq!(again.data["idempotent"], true);
    assert_eq!(again.data["released_dependents"], json!([]));
    let state = server.state.lock().unwrap();
    let releases = state
        .msgs
        .values()
        .filter(|message| message.body.starts_with("RESOURCE_RELEASED "))
        .count();
    assert_eq!(releases, 1, "a repeated finalize must not double-release");
    assert_eq!(
        state
            .msgs
            .values()
            .filter(|message| message.subject.as_deref() == Some("released:held"))
            .count(),
        1
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn interrupted_finalize_is_resumed_by_a_retry_not_silently_completed() {
    let (server, root) = test_server();
    force_closed_unverified_holder_with_waiter(&server);
    let first = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    assert_eq!(first.data["idempotent"], false);

    // Model interruption after the verified receipt committed but before the
    // release ran by rewinding only the waiter back to its waiting state.
    {
        let mut state = server.state.lock().unwrap();
        let mut waiter = state.tasks["waiting"].clone();
        waiter.status = "waiting".into();
        waiter.wait = Some(crate::server::state::WaitSpec {
            deadline_ms: now_ms() + 60_000,
            escalation: String::new(),
            reason: "recheck after finalize".into(),
            responsible_actor: "holder".into(),
            resume_on: vec![],
            waiter: "waiter".into(),
            waiting_for: "held".into(),
        });
        state.tasks.insert("waiting".into(), waiter);
    }

    let retried = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(retried.ok, "{}", retried.error.unwrap_or_default());
    assert_eq!(
        retried.data["idempotent"], true,
        "a verified receipt with an unfinished release must resume, not re-verify"
    );
    assert_eq!(
        retried.data["released_dependents"],
        json!(["waiting"]),
        "the resumed attempt must finish the remaining release explicitly"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["waiting"].status, "blocked");
    assert!(state.tasks["waiting"].wait.is_none());
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn finalize_preserves_unread_direct_message_payloads() {
    let (server, root) = test_server();
    force_closed_unverified_holder_with_waiter(&server);
    server.commit(&[Event::Sent {
        msg: Message {
            id: "unread-payload".into(),
            from: "holder".into(),
            to: "waiter".into(),
            mtype: "request".into(),
            subject: Some("must survive".into()),
            body: "unread direct message body".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let finalized = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(finalized.ok, "{}", finalized.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    let unread = state
        .msgs
        .get("unread-payload")
        .expect("finalize must not delete unread direct-message payloads");
    assert_eq!(unread.body, "unread direct message body");
    assert_eq!(unread.state, "pending");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}
