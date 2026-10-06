#[test]
fn pending_merge_requires_the_delivered_candidate_on_main() {
    let (mut server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved candidate merge test");
    initialize_main(&root);

    git_ok(&root, &["checkout", "-q", "-b", "codex/candidate"]);
    std::fs::write(root.join("candidate.txt"), "candidate\n").unwrap();
    git_ok(&root, &["add", "candidate.txt"]);
    git_ok(&root, &["commit", "-q", "-m", "candidate work"]);
    let candidate = current_head(&root);
    git_ok(&root, &["checkout", "-q", "main"]);
    let main_before_merge = current_head(&root);
    // The candidate must be delivered from a real worktree checked out on the
    // registered branch; a branch ref alone is not proof of the delivered
    // commit under the fail-closed binding rule.
    let worktree_dir = configured_test_worktree(&mut server, &root, "candidate");
    git_ok(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            worktree_dir.to_str().unwrap(),
            "codex/candidate",
        ],
    );
    assert_eq!(rev_parse(&worktree_dir, "HEAD"), candidate);

    let registered = handle_task_register(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        None,
        Some("feature".into()),
        Some(worktree_dir.display().to_string()),
        Some("codex/candidate".into()),
        Some(main_before_merge.clone()),
        default_priority(),
    );
    assert!(registered.ok, "{}", registered.error.unwrap_or_default());
    let worktree = server.state.lock().unwrap().tasks["task"]
        .worktree_path
        .clone()
        .unwrap();
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
            Some("candidate delivered".into()),
            Some(worktree),
        )
        .ok
    );
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["task"]
            .delivery_commit
            .as_deref(),
        Some(candidate.as_str()),
        "deliver must bind the obligation to the exact candidate commit"
    );
    assert!(
        handle_task_review(
            &server,
            "master".into(),
            "token-master".into(),
            "task".into(),
            true,
            false,
            "accepted".into(),
        )
        .ok
    );

    // An unrelated pre-existing main commit must not satisfy the obligation.
    let unrelated = handle_task_integrated(
        &server,
        "master".into(),
        "token-master".into(),
        "task".into(),
        main_before_merge.clone(),
        "unrelated main sha".into(),
    );
    assert!(!unrelated.ok, "{unrelated:?}");
    assert_eq!(unrelated.error.as_deref(), Some("TASK_MERGE_PENDING"));
    assert!(
        server.state.lock().unwrap().pending_merges.contains_key("task"),
        "the obligation must survive an unrelated-main integration attempt"
    );

    // Only after the candidate itself lands on main may the master record it.
    git_ok(&root, &["merge", "--no-ff", "-q", "codex/candidate", "-m", "merge candidate"]);
    let merged = handle_task_integrated(
        &server,
        "master".into(),
        "token-master".into(),
        "task".into(),
        candidate.clone(),
        "candidate merged onto main".into(),
    );
    assert!(merged.ok, "{merged:?}");
    assert!(!server.state.lock().unwrap().pending_merges.contains_key("task"));
    std::fs::remove_dir_all(server.config.worktree.base.as_ref().unwrap()).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn pending_merge_binds_worktree_head_not_a_stale_branch_ref() {
    let (mut server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved worktree-head merge test");
    initialize_main(&root);

    // The worktree HEAD is the delivered commit. The registered branch ref is
    // main, deliberately stale relative to the delivered worktree HEAD, so the
    // binding must fail closed instead of trusting the branch ref.
    let worktree_dir = configured_test_worktree(&mut server, &root, "candidate-relative");
    git_ok(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            worktree_dir.to_str().unwrap(),
            "-b",
            "codex/candidate-relative",
            "refs/heads/main",
        ],
    );
    std::fs::write(worktree_dir.join("candidate.txt"), "candidate\n").unwrap();
    git_ok(&worktree_dir, &["add", "candidate.txt"]);
    git_ok(&worktree_dir, &["commit", "-q", "-m", "candidate work"]);
    let candidate = rev_parse(&worktree_dir, "HEAD");
    let main_before_merge = rev_parse(&root, "refs/heads/main");
    assert_ne!(candidate, main_before_merge);

    let registered = handle_task_register(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        None,
        Some("feature".into()),
        Some(worktree_dir.display().to_string()),
        Some("main".into()),
        Some(main_before_merge.clone()),
        default_priority(),
    );
    assert!(registered.ok, "{}", registered.error.unwrap_or_default());
    let registered_worktree = server.state.lock().unwrap().tasks["task"]
        .worktree_path
        .clone()
        .unwrap();
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
            Some("candidate delivered".into()),
            Some(registered_worktree),
        )
        .ok
    );
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["task"]
            .delivery_commit
            .as_deref(),
        None,
        "a stale branch ref must fail closed rather than bind the branch ref"
    );
    std::fs::remove_dir_all(server.config.worktree.base.as_ref().unwrap()).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn review_accept_surfaces_failed_immediate_wake_attempt() {
    let (mut server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master wake attempt test");
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
            Some("candidate commit and gates passed".into()),
            Some("/tmp/candidate".into()),
        )
        .ok
    );

    // Disable notifications so the immediate wake cannot deliver; the
    // obligation must still be durable, but the response must surface the
    // explicit failure instead of pretending success.
    server.config.notifications.enabled = false;
    let response = handle_task_review(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        true,
        false,
        "review passed".into(),
    );
    assert!(response.ok, "{}", response.error.clone().unwrap_or_default());
    assert_eq!(
        response.data["notification"],
        serde_json::json!("subscribed-not-sent")
    );
    assert_eq!(response.data["repair_required"], serde_json::json!(true));
    assert!(
        response.data["notification_error"]
            .as_str()
            .is_some_and(|error| error.contains("notifications are disabled")),
        "{}",
        response.data
    );
    assert!(
        server.state.lock().unwrap().pending_merges.contains_key("task"),
        "the obligation remains durable even when the wake cannot deliver"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn legacy_accepted_update_to_rework_resolves_pending_merge() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved master rework test");
    assert!(create_task(&server, "owner", "task", "feature").ok);

    accept_task(&server, "owner", "task");
    assert!(server.state.lock().unwrap().pending_merges.contains_key("task"));

    assert!(
        handle_task_update(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("rework".into()),
            Some("review requested rework".into()),
        )
        .ok
    );
    let state = server.state.lock().unwrap();
    assert!(
        !state.pending_merges.contains_key("task"),
        "accepted -> rework must resolve the pending merge"
    );
    assert_eq!(state.tasks["task"].status, "rework");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_rejects_missing_binding() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "receiver", "%receiver");
    {
        let mut state = server.state.lock().unwrap();
        let scope = send_command(&root, "sender").scope;
        state
            .global
            .projects
            .get_mut(scope.project_scope_id.as_str())
            .unwrap()
            .runtime_bindings
            .remove("binding-sender");
    }
    let server_arc = Arc::new(server);
    let resp = dispatch(
        &server_arc,
        Req::Send {
            from: "sender".into(),
            worker_id: Some("sender".into()),
            token: Some("token-sender".into()),
            command: Some(send_command(&root, "sender")),
            to: "receiver".into(),
            mtype: "notify".into(),
            subject: Some("test".into()),
            body: "body".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
    );
    assert!(!resp.ok);
    let err = resp.error.unwrap_or_default();
    assert!(
        err.contains("SEND_BINDING_REJECTED")
            && err.contains("authoritative runtime binding is missing"),
        "unexpected error: {err}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_rejects_ambiguous_binding() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "receiver", "%receiver");
    let extra_binding = {
        let state = server.state.lock().unwrap();
        let mut binding = state
            .global
            .lookup_binding_for(
                &send_command(&root, "sender").scope,
                &BindingId::new("binding-sender").unwrap(),
            )
            .unwrap()
            .clone();
        binding.binding_id = BindingId::new("binding-sender-extra").unwrap();
        binding.runtime_id = RuntimeId::new("runtime-sender-extra").unwrap();
        binding
    };
    server.commit(&[Event::GlobalRuntimeBound {
        binding: extra_binding,
    }]);
    let resp = dispatch(
        &Arc::new(server),
        Req::Send {
            from: "sender".into(),
            worker_id: Some("sender".into()),
            token: Some("token-sender".into()),
            command: Some(send_command(&root, "sender")),
            to: "receiver".into(),
            mtype: "notify".into(),
            subject: Some("test".into()),
            body: "body".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
    );
    assert!(!resp.ok);
    let err = resp.error.unwrap_or_default();
    assert!(
        err.contains("SEND_BINDING_REJECTED")
            && err.contains("authoritative runtime binding is ambiguous"),
        "unexpected error: {err}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_rejects_another_workers_binding() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "receiver", "%receiver");
    let resp = dispatch(
        &Arc::new(server),
        Req::Send {
            from: "sender".into(),
            worker_id: Some("sender".into()),
            token: Some("token-sender".into()),
            command: Some(send_command(&root, "receiver")),
            to: "receiver".into(),
            mtype: "notify".into(),
            subject: Some("test".into()),
            body: "body".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
    );
    assert!(!resp.ok);
    let err = resp.error.unwrap_or_default();
    assert!(
        err.contains("SEND_BINDING_REJECTED") && err.contains("actor binding mismatch"),
        "unexpected error: {err}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_rejects_stale_generation() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "receiver", "%receiver");
    let server_arc = Arc::new(server);
    let mut command = send_command(&root, "sender");
    command.endpoint_generation = 0;
    let resp = dispatch(
        &server_arc,
        Req::Send {
            from: "sender".into(),
            worker_id: Some("sender".into()),
            token: Some("token-sender".into()),
            command: Some(command),
            to: "receiver".into(),
            mtype: "notify".into(),
            subject: Some("test".into()),
            body: "body".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
    );
    assert!(!resp.ok);
    let err = resp.error.unwrap_or_default();
    assert!(
        err.contains("SEND_BINDING_REJECTED") && err.contains("stale endpoint generation"),
        "unexpected error: {err}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_enforces_request_cooldown() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "receiver", "%receiver");
    let server_arc = Arc::new(server);
    let command = send_command(&root, "sender");
    let first = dispatch(
        &server_arc,
        Req::Send {
            from: "sender".into(),
            worker_id: Some("sender".into()),
            token: Some("token-sender".into()),
            command: Some(command.clone()),
            to: "receiver".into(),
            mtype: "request".into(),
            subject: Some("cooldown".into()),
            body: "first".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    let second = dispatch(
        &server_arc,
        Req::Send {
            from: "sender".into(),
            worker_id: Some("sender".into()),
            token: Some("token-sender".into()),
            command: Some(command.clone()),
            to: "receiver".into(),
            mtype: "request".into(),
            subject: Some("cooldown".into()),
            body: "second".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
    );
    assert!(!second.ok);
    let err = second.error.unwrap_or_default();
    assert!(
        err.contains("request cooldown active"),
        "unexpected error: {err}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_supersedes_earlier_reply() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "receiver", "%receiver");
    let server_arc = Arc::new(server);
    let command = send_command(&root, "sender");
    let command_receiver = send_command(&root, "receiver");
    let req_resp = dispatch(
        &server_arc,
        Req::Send {
            from: "sender".into(),
            worker_id: Some("sender".into()),
            token: Some("token-sender".into()),
            command: Some(command.clone()),
            to: "receiver".into(),
            mtype: "request".into(),
            subject: Some("supersede".into()),
            body: "ask".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
    );
    assert!(req_resp.ok, "{}", req_resp.error.unwrap_or_default());
    let request_id = req_resp.data["msg_id"].as_str().unwrap().to_owned();
    let first = dispatch(
        &server_arc,
        Req::Send {
            from: "receiver".into(),
            worker_id: Some("receiver".into()),
            token: Some("token-receiver".into()),
            command: Some(command_receiver.clone()),
            to: "sender".into(),
            mtype: "reply".into(),
            subject: Some("supersede".into()),
            body: "first".into(),
            in_reply_to: Some(request_id.clone()),
            delivery: "immediate".into(),
        },
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    let first_id = first.data["msg_id"].as_str().unwrap().to_owned();
    let second = dispatch(
        &server_arc,
        Req::Send {
            from: "receiver".into(),
            worker_id: Some("receiver".into()),
            token: Some("token-receiver".into()),
            command: Some(command_receiver.clone()),
            to: "sender".into(),
            mtype: "reply".into(),
            subject: Some("supersede".into()),
            body: "second".into(),
            in_reply_to: Some(request_id.clone()),
            delivery: "immediate".into(),
        },
    );
    assert!(second.ok, "{}", second.error.unwrap_or_default());
    let state = server_arc.state.lock().unwrap();
    let first_msg = state.msgs.get(&first_id).unwrap();
    assert_eq!(first_msg.state, "superseded");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}
#[test]
fn cancelling_master_idle_subscription_supersedes_pending_wake() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user-approved");
    server.commit(&[
        Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: "sub-master-idle".into(),
                worker_id: "master".into(),
                event: "master-idle".into(),
                subject: Some("master-idle".into()),
                target: "thread-master".into(),
                method: "appserver".into(),
                trigger_ms: Some(now_ms() - 1),
                trigger_times_ms: Vec::new(),
                interval_ms: Some(900_000),
                repeat_count: 3,
                fired_count: 0,
                expires_ms: now_ms() + 86_400_000,
                status: "armed".into(),
                created_ms: now_ms() - 900_000,
                updated_ms: now_ms(),
                status_reason: None,
            },
        },
        Event::Sent {
            msg: Message {
                id: "pending-idle".into(),
                from: "collab-server".into(),
                to: "master".into(),
                mtype: "notification".into(),
                subject: Some("master-idle:master-idle".into()),
                body: "MASTER_IDLE_WAKE scheduling continues".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::WakeBound {
            message_id: "pending-idle".into(),
            subscription_id: "sub-master-idle".into(),
        },
    ]);
    let cancelled = handle_notification_unsubscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "sub-master-idle".into(),
    );
    assert!(cancelled.ok, "{}", cancelled.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["sub-master-idle"].status,
        "cancelled"
    );
    assert_eq!(state.msgs["pending-idle"].state, "superseded");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn deadline_subscription_requires_live_master_authority() {
    let (server, root) = test_server();
    register(&server, "worker", "%worker");
    register(&server, "master", "%master");

    let denied = handle_notification_subscribe(
        &server,
        "worker".into(),
        "token-worker".into(),
        "deadline".into(),
        Some("goal:test".into()),
        None,
        vec![now_ms() + 10_000],
        None,
        1,
        60,
    );
    assert!(!denied.ok);
    assert!(denied.error.unwrap().contains("deadline subscriptions"));

    promote_master(&server, "master", "user approved test master");
    let accepted = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "deadline".into(),
        Some("goal:test".into()),
        None,
        vec![now_ms() + 10_000],
        None,
        1,
        60,
    );
    assert!(accepted.ok, "master should be allowed: {accepted:?}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn async_result_subscription_is_rejected_without_a_producer() {
    let (server, root) = test_server();
    let response = handle_notification_subscribe(
        &server,
        "peer".into(),
        "token-peer".into(),
        "async-result".into(),
        Some("operation-1".into()),
        None,
        Vec::new(),
        None,
        1,
        3600,
    );

    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some("unsupported notification event async-result; expected one of [\"direct-message\", \"resource-released\", \"deadline\", \"master-idle\"]")
    );
    assert!(server
        .state
        .lock()
        .unwrap()
        .notification_subscriptions
        .is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_deadline_rejects_periodic_rearm_options() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user-approved");

    let response = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "deadline".into(),
        Some("goal:inactive".into()),
        None,
        Vec::new(),
        Some(600_000),
        100,
        86_400,
    );
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some("goal deadline subscriptions are one-shot and require one at-ms trigger")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn goal_deadline_registration_deduplicates_same_deadline() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user-approved");
    let trigger_ms = now_ms() + 10_000;
    let first = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "deadline".into(),
        Some("goal:revision-7".into()),
        Some(trigger_ms),
        Vec::new(),
        None,
        1,
        86_400,
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    {
        let mut state = server.state.lock().unwrap();
        let mut legacy = state.notification_subscriptions
            [first.data["subscription"]["id"].as_str().unwrap()]
        .clone();
        legacy.method = "appserver".into();
        legacy.target = "legacy-thread-master".into();
        server
            .commit_locked(
                &mut state,
                &[Event::NotificationSubscribed {
                    subscription: legacy,
                }],
            )
            .expect("commit legacy subscription");
    }
    let second = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "deadline".into(),
        Some("goal:revision-7".into()),
        Some(trigger_ms),
        Vec::new(),
        None,
        1,
        86_400,
    );
    assert!(second.ok, "{}", second.error.unwrap_or_default());
    assert_eq!(second.data["deduplicated"], true);
    assert_eq!(
        second.data["subscription"]["id"],
        first.data["subscription"]["id"]
    );
    assert_eq!(second.data["subscription"]["method"], "tmux");
    assert_eq!(second.data["subscription"]["target"], "thread-master");
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .notification_subscriptions
            .values()
            .filter(|subscription| is_goal_deadline(subscription))
            .count(),
        1
    );

    let next_revision = handle_notification_subscribe(
        &server,
        "master".into(),
        "token-master".into(),
        "deadline".into(),
        Some("goal:revision-8".into()),
        Some(trigger_ms),
        Vec::new(),
        None,
        1,
        86_400,
    );
    assert!(
        next_revision.ok,
        "{}",
        next_revision.error.unwrap_or_default()
    );
    assert!(next_revision.data.get("deduplicated").is_none());
    assert_ne!(
        next_revision.data["subscription"]["id"],
        first.data["subscription"]["id"]
    );
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .notification_subscriptions
            .values()
            .filter(|subscription| is_goal_deadline(subscription))
            .count(),
        2
    );
    std::fs::remove_dir_all(root).ok();
}

fn create_task(server: &Server, owner: &str, id: &str, feature: &str) -> Resp {
    handle_task_register(
        server,
        owner.into(),
        format!("token-{owner}"),
        id.into(),
        None,
        Some(feature.into()),
        None,
        None,
        None,
        default_priority(),
    )
}

fn initialize_main(root: &Path) {
    for args in [
        ["init", "-q"].as_slice(),
        ["config", "user.email", "test@example.com"].as_slice(),
        ["config", "user.name", "Collab Test"].as_slice(),
        ["commit", "--allow-empty", "-q", "-m", "main"].as_slice(),
        ["branch", "-M", "main"].as_slice(),
    ] {
        assert!(Command::new("git")
            .current_dir(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
}

fn current_head(root: &Path) -> String {
    rev_parse(root, "HEAD")
}

/// Drive one task to the accepted state that `task integrated` requires.
fn accept_task(server: &Server, owner: &str, id: &str) {
    let root = server.root.clone();
    let worktree = candidate_worktree(&root, id);
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                server,
                owner.into(),
                format!("token-{owner}"),
                id.into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            server,
            owner.into(),
            format!("token-{owner}"),
            id.into(),
            Some("candidate commit and gates passed".into()),
            Some(worktree.to_string_lossy().into_owned()),
        )
        .ok
    );
    assert!(
        handle_task_review(
            server,
            owner.into(),
            format!("token-{owner}"),
            id.into(),
            true,
            false,
            "review passed".into(),
        )
        .ok
    );
    assert_eq!(server.state.lock().unwrap().tasks[id].status, "accepted");
    git_ok(&root, &["worktree", "remove", worktree.to_str().unwrap()]);
}

/// A daemon pending merge binds the obligation to the exact delivered commit,
/// so tests that later integrate under a live master need a real git worktree
/// whose HEAD resolves. Each call gets a unique path so parallel tests cannot
/// collide on the same worktree lock.
fn candidate_worktree(root: &Path, id: &str) -> PathBuf {
    static WT_SEQ: AtomicU64 = AtomicU64::new(0);
    if !Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--verify", "refs/heads/main"])
        .output()
        .is_ok_and(|output| output.status.success())
    {
        initialize_main(root);
    }
    let seq = WT_SEQ.fetch_add(1, Ordering::Relaxed);
    let worktree = std::env::temp_dir().join(format!(
        "collab-candidate-{}-{seq}-{id}",
        std::process::id()
    ));
    let branch = format!("codex/candidate-{seq}-{id}");
    git_ok(
        root,
        &[
            "worktree",
            "add",
            "-q",
            worktree.to_str().unwrap(),
            "-b",
            &branch,
            "refs/heads/main",
        ],
    );
    worktree
}

fn rev_parse(root: &Path, rev: &str) -> String {
    String::from_utf8(
        Command::new("git")
            .current_dir(root)
            .args(["rev-parse", rev])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned()
}

fn git_ok(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn failed_journal_cannot_apply_a_keepalive_reservation() {
    let (server, root) = test_server();
    *server.journal.lock().unwrap() =
        std::fs::File::open(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    let mut state = State::default();
    let result = server.commit_locked_checked(
        &mut state,
        &[Event::KeepaliveUpdated {
            worker_id: "worker".into(),
            record: crate::server::keepalive::Record::default(),
        }],
    );
    assert!(result.is_err());
    assert!(state.keepalives.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn command_retry_returns_original_outcome_without_reapplying_events() {
    let (server, root) = test_server();
    let event = Event::KeepaliveUpdated {
        worker_id: "worker".into(),
        record: crate::server::keepalive::Record::default(),
    };
    let first = server
        .commit_command(
            "command-1",
            "operation-1",
            std::slice::from_ref(&event),
            json!({"accepted": true}),
        )
        .unwrap();
    assert!(!first.replayed);
    let second = server
        .commit_command(
            "command-1",
            "operation-1",
            &[event],
            json!({"accepted": false}),
        )
        .unwrap();
    assert!(second.replayed);
    assert_eq!(first.receipt, second.receipt);
    assert_eq!(first.operation_id, second.operation_id);
    assert_eq!(second.outcome, json!({"accepted": true}));
    assert!(server
        .state
        .lock()
        .unwrap()
        .global
        .command_receipts
        .contains_key("command-1"));
    assert_eq!(
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .lines()
            .count(),
        3
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_registration_uses_global_binding_and_host_idempotency() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();
    let typed = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    let first = server.typed_dispatch(typed.clone()).unwrap();
    assert!(!first.replayed);
    let project_scope =
        crate::server::global_state::GlobalState::canonical_project_scope(Path::new(cwd)).unwrap();
    let state = server.state.lock().unwrap();
    assert!(state
        .global
        .lookup_registration(
            &project_scope,
            &crate::identity::AppServerId::new("tui-default").unwrap(),
        )
        .is_some());
    assert_eq!(
        state.global.projects[project_scope.as_str()]
            .runtime_bindings
            .len(),
        1
    );
    assert!(state
        .global
        .command_receipts
        .contains_key(first.receipt.command_id.as_str()));
    drop(state);

    let replay = server.typed_dispatch(typed).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.receipt, first.receipt);
    assert_eq!(
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .lines()
            .count(),
        6
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_receipt_revision_is_the_next_cas_and_stale_after_another_mutation() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();

    let first = server
        .typed_dispatch(
            server
                .typed_register_envelope("worker", "token-worker", "%worker", cwd)
                .unwrap(),
        )
        .unwrap();
    let mut second = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    second.envelope.expected_revision = Some(first.receipt.revision);
    let second = server
        .typed_dispatch(second)
        .expect("a receipt revision must be reusable as the next CAS revision");

    let mut stale = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    stale.envelope.expected_revision = Some(second.receipt.revision);
    server
        .commit_checked(&[Event::KeepaliveUpdated {
            worker_id: "other-reducer".into(),
            record: crate::server::keepalive::Record::default(),
        }])
        .unwrap();
    let journal_before_stale =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    let error = server
        .typed_dispatch(stale)
        .expect_err("an intervening reducer mutation must reject the old CAS revision");
    assert!(error
        .to_string()
        .contains("compare-and-swap revision mismatch"));
    assert_eq!(
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap(),
        journal_before_stale,
        "a stale CAS must not append a journal event"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_rebinds_survive_journal_rewrite_and_replay_with_one_version_axis() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();
    let mut previous_revision = 0;
    for _ in 0..3 {
        let mut typed = server
            .typed_register_envelope("worker", "token-worker", "%worker", cwd)
            .unwrap();
        typed.envelope.expected_revision = Some(previous_revision);
        let outcome = server.typed_dispatch(typed).unwrap();
        previous_revision = outcome.receipt.revision;
    }

    let (version, binding_generation, receipts) = {
        let state = server.state.lock().unwrap();
        (
            (state.sequence, state.revision, state.global.version()),
            state
                .global
                .projects
                .values()
                .next()
                .unwrap()
                .runtime_bindings["binding-worker"]
                .endpoint_generation,
            state.global.command_receipts.clone(),
        )
    };
    let state = server.state.lock().unwrap();
    state.global.validate().unwrap();
    server.rewrite_journal_locked(&state).unwrap();
    drop(state);

    let replayed = super::replay(&root).unwrap();
    replayed.global.validate().unwrap();
    assert_eq!(
        (replayed.sequence, replayed.revision),
        (version.0, version.1)
    );
    assert_eq!(replayed.global.version(), version.2);
    assert_eq!(
        replayed
            .global
            .projects
            .values()
            .next()
            .unwrap()
            .runtime_bindings["binding-worker"]
            .endpoint_generation,
        binding_generation
    );
    assert_eq!(replayed.global.command_receipts, receipts);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_command_record_rewrite_and_replay_preserve_checkpoint_version() {
    let (server, root) = test_server();
    let receipt = crate::server::state::CommandReceipt {
        operation_id: "legacy-operation".into(),
        outcome: json!({"accepted": true}),
        sequence: 1,
        revision: 1,
    };
    server
        .commit_checked(&[Event::CommandRecorded {
            command_id: "legacy-command".into(),
            receipt: receipt.clone(),
        }])
        .unwrap();

    let state = server.state.lock().unwrap();
    assert_eq!((state.sequence, state.revision), (1, 1));
    server.rewrite_journal_locked(&state).unwrap();
    drop(state);

    let compacted =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    assert!(compacted.contains("\"ev\":\"CommandRecorded\""));
    assert!(!compacted.contains("\"ev\":\"CommandStarted\""));
    assert!(!compacted.contains("\"ev\":\"CommandCompleted\""));
    let replayed =
        super::replay(&root).expect("a compacted legacy CommandRecorded must replay successfully");
    assert_eq!((replayed.sequence, replayed.revision), (1, 1));
    assert_eq!(replayed.command_receipts["legacy-command"], receipt);
    std::fs::remove_dir_all(root).unwrap();
}
