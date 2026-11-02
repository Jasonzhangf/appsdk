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
fn context_rearms_historical_explicit_unsubscribe() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);
    server.commit(&[Event::NotificationSuppressed {
        subscription_id: "sub-default-direct-message-peer".into(),
        status: "cancelled".into(),
        reason: "explicit-unsubscribe".into(),
        updated_ms: now_ms(),
    }]);
    let context = handle_context(&server, "peer".into(), "token-peer".into());
    assert!(context.ok, "{}", context.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    let lease = &state.notification_subscriptions["sub-default-direct-message-peer"];
    assert_eq!(lease.status, "armed");
    assert_eq!(lease.status_reason, None);
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn repeated_context_keeps_daemon_default_reception_armed() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);

    for _ in 0..2 {
        let context = handle_context(&server, "peer".into(), "token-peer".into());
        assert!(context.ok, "{}", context.error.unwrap_or_default());
    }

    let state = server.state.lock().unwrap();
    let lease = &state.notification_subscriptions["sub-default-direct-message-peer"];
    assert_eq!(lease.status, "armed");
    assert_eq!(lease.worker_id, "peer");
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
        patrol_idle_since_ms: None,
        patrol_last_push_ms: 0,
                periodic_declared: false,
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
fn reply_obligation_retains_original_until_correlated_reply_clears_it() {
    let (server, root) = test_server();
    let now = now_ms();
    let created_ms = now - 8 * 86_400_000;
    server.commit(&[
        Event::Sent {
            msg: Message {
                id: "retained-reply-request".into(),
                from: "sender".into(),
                to: "recipient".into(),
                mtype: "notify".into(),
                subject: Some("reply required".into()),
                body: "Original requirement must remain readable".into(),
                in_reply_to: None,
                created_ms,
                state: "read".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::ReplyObligationSet {
            obligation: state::ReplyObligation {
                message_id: "retained-reply-request".into(),
                project: root.display().to_string(),
                from_project: None,
                from_ceo: false,
                from_worker_id: "sender".into(),
                from_name: None,
                recipient_worker_id: "recipient".into(),
                recipient_name: None,
                policy: crate::proto::ReplyPolicy::Required,
                created_ms,
                reminded_idle_episode: false,
            },
        },
    ]);
    assert_eq!(purge_expired_storage(&server, now), 0);
    assert!(server.state.lock().unwrap().msgs.contains_key("retained-reply-request"));
    server.commit(&[Event::ReplyObligationCleared {
        message_id: "retained-reply-request".into(),
    }]);
    assert_eq!(purge_expired_storage(&server, now), 1);
    std::fs::remove_dir_all(root).unwrap();
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
    let worktree = candidate_worktree(&root, "task");
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
        Some(worktree.to_string_lossy().into_owned()),
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
    // Git integration is retired: an owner finishes its own task by closing
    // the Collab responsibility after review, without any Git operation.
    git_ok(&root, &["worktree", "remove", worktree.to_str().unwrap()]);
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
    assert!(
        !state.cleanup_receipts.contains_key("task"),
        "a normal Collab close records no Git cleanup receipt"
    );
    assert!(
        state.msgs.is_empty(),
        "normal lifecycle must not report to peers"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_close_leaves_declared_worktree_and_branch_untouched() {
    let (mut server, root) = test_server();
    register(&server, "peer", "%peer");
    initialize_main(&root);
    let base = current_head(&root);
    git_ok(&root, &["checkout", "-q", "-b", "codex/cleanup-live"]);
    std::fs::write(root.join("cleanup-live.txt"), "merged task\n").unwrap();
    git_ok(&root, &["add", "cleanup-live.txt"]);
    git_ok(&root, &["commit", "-q", "-m", "cleanup task"]);
    git_ok(&root, &["checkout", "-q", "main"]);
    git_ok(&root, &["merge", "--ff-only", "codex/cleanup-live"]);
    git_ok(&root, &["checkout", "-q", "-b", "root-snapshot", &base]);
    let worktree = configured_test_worktree(&mut server, &root, "cleanup-live");
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
        Some(worktree_string.clone()),
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

    let closed = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "cleanup-live".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert_eq!(closed.data["cleanup"]["result"], "not_managed");
    assert_eq!(closed.data["cleanup"]["collab_managed"], false);
    // Close records Collab responsibility only; the declared worktree and
    // branch stay as user-owned resources regardless of the recorded main head.
    assert!(worktree.exists());
    let branch_after_close = Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--verify", "refs/heads/codex/cleanup-live"])
        .output()
        .unwrap();
    assert!(branch_after_close.status.success());
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["cleanup-live"].status, "closed");
    assert!(!state.cleanup_receipts.contains_key("cleanup-live"));
    drop(state);
    std::fs::remove_dir_all(server.config.worktree.base.as_ref().unwrap()).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn live_master_closes_task_with_declared_worktree_untouched() {
    let (mut server, root) = test_server();
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
    let worktree = configured_test_worktree(&mut server, &root, "master-cleanup");
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
        Some(worktree_string.clone()),
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

    let closed = handle_task_close(
        &server,
        "master".into(),
        "token-master".into(),
        "master-cleanup".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert_eq!(closed.data["cleanup"]["result"], "not_managed");
    assert_eq!(closed.data["cleanup"]["collab_managed"], false);
    // A live master's close ends the Collab responsibility; the declared
    // worktree and branch are not removed by Collab.
    assert!(worktree.exists());
    let branch_after_close = Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--verify", "refs/heads/codex/master-cleanup"])
        .output()
        .unwrap();
    assert!(branch_after_close.status.success());
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["master-cleanup"].owner, "owner");
    assert_eq!(state.tasks["master-cleanup"].status, "closed");
    assert!(!state.cleanup_receipts.contains_key("master-cleanup"));
    drop(state);
    std::fs::remove_dir_all(server.config.worktree.base.as_ref().unwrap()).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_close_records_no_git_cleanup_receipt() {
    let (mut server, root) = test_server();
    register(&server, "peer", "%peer");
    initialize_main(&root);
    let base = current_head(&root);
    git_ok(&root, &["checkout", "-q", "-b", "codex/already-clean"]);
    std::fs::write(root.join("already-clean.txt"), "merged task\n").unwrap();
    git_ok(&root, &["add", "already-clean.txt"]);
    git_ok(&root, &["commit", "-q", "-m", "already clean task"]);
    git_ok(&root, &["checkout", "-q", "main"]);
    git_ok(&root, &["merge", "--ff-only", "codex/already-clean"]);
    let worktree = configured_test_worktree(&mut server, &root, "already-clean");
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
        Some(worktree_string.clone()),
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

    let closed = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "already-clean".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert_eq!(closed.data["cleanup"]["result"], "not_managed");
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["already-clean"].status, "closed");
    assert!(
        !state.cleanup_receipts.contains_key("already-clean"),
        "Collab close must not fabricate a verified Git cleanup receipt"
    );
    drop(state);
    assert!(worktree.exists(), "user-owned worktree must survive close");
    assert!(Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--verify", "refs/heads/codex/already-clean"])
        .output()
        .unwrap()
        .status
        .success());
    std::fs::remove_dir_all(server.config.worktree.base.as_ref().unwrap()).ok();
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
        "manual close recorded; Collab does not manage Git resources"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["stuck"].status, "closed");
    assert!(
        !state.cleanup_receipts.contains_key("stuck"),
        "a force close records the manual reason on the task, not a cleanup receipt"
    );
    assert_eq!(
        state.tasks["stuck"].next_step.as_deref(),
        Some("manual close: worktree dirty and merge blocked; force closing per master"),
    );
    let projected = task_view(&state, &state.tasks["stuck"]);
    assert_eq!(projected["cleanup"]["status"], "historical_unmanaged");
    assert_eq!(projected["cleanup"]["collab_managed"], false);
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

/// Force-close a holder that declares no worktree/branch, leaving one waiter
/// blocked on it with a resource-released subscription. This is the fixture the
/// finalize regressions build on.
fn force_closed_unverified_holder_with_waiter(server: &Server) {
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
    assert_eq!(
        forced.data["next_action"],
        "manual close recorded; Collab does not manage Git resources"
    );
    // A force close records the manual reason on the task and must not release
    // the waiter or write a Git cleanup receipt.
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["waiting"].status, "waiting");
    assert_eq!(
        state.tasks["held"].next_step.as_deref(),
        Some("manual close: holder abandoned mid-flight; force closing"),
    );
    assert!(!state.cleanup_receipts.contains_key("held"));
    assert!(
        state
            .msgs
            .values()
            .all(|message| !message.body.starts_with("RESOURCE_RELEASED ")),
        "an unverified force close must not release dependents"
    );
    drop(state);
}

#[test]
fn force_close_without_finalize_reports_unverified_and_does_not_release() {
    let (server, root) = test_server();
    force_closed_unverified_holder_with_waiter(&server);
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["held"].status, "closed");
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
fn close_releases_a_waiter_exactly_once() {
    let (server, root) = test_server();
    register(&server, "holder", "%holder");
    register(&server, "waiter", "%waiter");
    assert!(create_task(&server, "holder", "held", "shared-feature").ok);
    assert!(!create_task(&server, "waiter", "waiting", "shared-feature").ok);
    assert!(
        handle_task_wait(
            &server,
            "waiter".into(),
            "token-waiter".into(),
            "waiting".into(),
            "held".into(),
        )
        .ok
    );
    assert!(
        handle_notification_subscribe(
            &server,
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
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "holder".into(),
                "token-holder".into(),
                "held".into(),
                Some(status.into()),
                Some(format!("continue {status}")),
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "holder".into(),
            "token-holder".into(),
            "held".into(),
            Some("candidate verified".into()),
            None,
        )
        .ok
    );
    assert!(
        handle_task_review(
            &server,
            "holder".into(),
            "token-holder".into(),
            "held".into(),
            true,
            false,
            "review passed".into(),
        )
        .ok
    );
    let closed = handle_task_close(
        &server,
        "holder".into(),
        "token-holder".into(),
        "held".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert_eq!(closed.data["cleanup"]["result"], "not_managed");
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["waiting"].status, "blocked");
    assert!(state.tasks["waiting"].wait.is_none());
    assert!(state.tasks["waiting"]
        .next_step
        .as_deref()
        .unwrap()
        .starts_with("RESOURCE_RELEASED=held"));
    let releases = state
        .msgs
        .values()
        .filter(|message| message.body.starts_with("RESOURCE_RELEASED "))
        .collect::<Vec<_>>();
    assert_eq!(releases.len(), 1, "close must release the waiter once");
    assert_eq!(releases[0].to, "waiter");
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
fn finalize_is_disabled_and_does_not_silently_complete() {
    let (server, root) = test_server();
    force_closed_unverified_holder_with_waiter(&server);
    // The force close left the holder closed and the waiter waiting. The
    // retired finalize entry must fail closed without completing silently or
    // touching either task.
    let finalized = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
        None,
    );
    assert!(!finalized.ok, "{finalized:?}");
    assert!(
        finalized
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("COLLAB_GIT_OWNERSHIP_DISABLED")),
        "{finalized:?}"
    );
    assert_eq!(finalized.data["resource_operations"], false);
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["held"].status, "closed");
    assert_eq!(state.tasks["waiting"].status, "waiting");
    assert!(state.tasks["waiting"].wait.is_some());
    assert!(
        state
            .msgs
            .values()
            .all(|message| !message.body.starts_with("RESOURCE_RELEASED ")),
        "the retired finalize must not release dependents"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn close_preserves_unread_direct_message_payloads() {
    let (server, root) = test_server();
    register(&server, "holder", "%holder");
    register(&server, "waiter", "%waiter");
    assert!(create_task(&server, "holder", "held", "shared-feature").ok);
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "holder".into(),
                "token-holder".into(),
                "held".into(),
                Some(status.into()),
                Some(format!("continue {status}")),
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "holder".into(),
            "token-holder".into(),
            "held".into(),
            Some("candidate verified".into()),
            None,
        )
        .ok
    );
    assert!(
        handle_task_review(
            &server,
            "holder".into(),
            "token-holder".into(),
            "held".into(),
            true,
            false,
            "review passed".into(),
        )
        .ok
    );
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
    let closed = handle_task_close(
        &server,
        "holder".into(),
        "token-holder".into(),
        "held".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    let unread = state
        .msgs
        .get("unread-payload")
        .expect("close must not delete unread direct-message payloads");
    assert_eq!(unread.body, "unread direct message body");
    assert_eq!(unread.state, "pending");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

/// A rejected DSH wake is repaired on the recipient's bound gateway, not by
/// making the recipient drain the mailbox with `collab recv` and not by
/// re-sending the same body blindly.
#[test]
fn rejected_dsh_wake_escalation_restores_gateway_native_receive() {
    let response = notification_send_response(
        json!({"msg_id": "msg-dsh-rejected"}),
        true,
        "dsh",
        &NotificationAttempt::Rejected("DSH_GATEWAY_ENQUEUE_TIMEOUT".into()),
    );
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some("DSH_NOTIFICATION_REJECTED: DSH_GATEWAY_ENQUEUE_TIMEOUT")
    );
    assert_eq!(response.data["durable"], true);
    assert_eq!(response.data["failure"], "notification_delivery_failed");
    let escalation = response.data["escalation"]
        .as_str()
        .expect("a rejected dsh wake carries an escalation");
    assert!(escalation.contains("durable"), "{escalation}");
    assert!(
        escalation.contains("restore") && escalation.contains("gateway native receive"),
        "a rejected dsh wake must point at the bound gateway receive loop: {escalation}"
    );
    assert!(
        !escalation.contains("collab recv"),
        "a rejected dsh wake must not redirect the recipient to the CLI reader: {escalation}"
    );
    let lowered = escalation.to_ascii_lowercase();
    assert!(
        !lowered.contains("send a new message") && !lowered.contains("re-send"),
        "a rejected dsh wake must not advise a blind re-send: {escalation}"
    );

    // Other transports keep their CLI-read guidance; the repair differs by
    // transport, so the tmux wording is the control for this counterexample.
    let tmux = notification_send_response(
        json!({"msg_id": "msg-tmux-rejected"}),
        true,
        "tmux",
        &NotificationAttempt::Rejected("TMUX_ENTER_SUBMIT_FAILED: forced".into()),
    );
    let tmux_escalation = tmux.data["escalation"].as_str().unwrap();
    assert!(
        tmux_escalation.contains("collab recv"),
        "a rejected tmux wake keeps the durable-read repair: {tmux_escalation}"
    );
}
