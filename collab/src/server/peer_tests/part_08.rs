#[test]
fn long_polls_do_not_starve_ping_on_the_blocking_pool() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::UnixStream;
        use tokio::time::{timeout, Duration};

        let (server, root) = test_server();
        register(&server, "peer", "%peer");
        let server = Arc::new(server);
        let mut poll_clients = Vec::new();
        let mut poll_tasks = Vec::new();

        for _ in 0..8 {
            let (mut client, server_stream) = UnixStream::pair().unwrap();
            poll_tasks.push(tokio::spawn(conn_task(server.clone(), server_stream)));
            let request = serde_json::to_string(&Req::Poll {
                worker_id: "peer".into(),
                token: "token-peer".into(),
                timeout_ms: 10_000,
                receive_id: None,
            })
            .unwrap();
            client.write_all(request.as_bytes()).await.unwrap();
            client.write_all(b"\n").await.unwrap();
            poll_clients.push(client);
        }

        tokio::time::sleep(Duration::from_millis(50)).await;

        let (mut ping_client, server_stream) = UnixStream::pair().unwrap();
        let ping_task = tokio::spawn(conn_task(server.clone(), server_stream));
        let request = serde_json::to_string(&Req::Ping).unwrap();
        ping_client.write_all(request.as_bytes()).await.unwrap();
        ping_client.write_all(b"\n").await.unwrap();
        let mut response = String::new();
        timeout(
            Duration::from_secs(1),
            BufReader::new(&mut ping_client).read_line(&mut response),
        )
        .await
        .expect("Ping must not wait behind long Poll requests")
        .unwrap();
        let response: Resp = serde_json::from_str(response.trim()).unwrap();
        assert!(response.ok);

        ping_task.abort();
        for task in poll_tasks {
            task.abort();
        }
        drop(poll_clients);
        std::fs::remove_dir_all(root).ok();
    });
}

#[tokio::test]
async fn poll_wakes_when_a_message_is_committed() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    let server = Arc::new(server);
    let poll = tokio::spawn(handle_poll_async(server.clone(), "peer".into(), 5_000));

    tokio::time::sleep(Duration::from_millis(10)).await;
    server.commit(&[Event::Sent {
        msg: Message {
            id: "wake-message".into(),
            from: "sender".into(),
            to: "peer".into(),
            mtype: "notify".into(),
            subject: Some("wake".into()),
            body: "message".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);

    let response = tokio::time::timeout(Duration::from_secs(1), poll)
        .await
        .expect("Poll must wake after a durable message commit")
        .unwrap();
    assert!(response.ok);
    assert_eq!(response.data["count"], 1);
    assert_eq!(response.data["messages"][0]["id"], "wake-message");
    assert_eq!(
        server.state.lock().unwrap().msgs["wake-message"].state,
        "read"
    );
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn recv_consumes_messages_without_a_follow_up_ack() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    server.commit(&[Event::Sent {
        msg: Message {
            id: "recv-message".into(),
            from: "sender".into(),
            to: "peer".into(),
            mtype: "notify".into(),
            subject: Some("recv".into()),
            body: "message".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    let server = Arc::new(server);
    let response = handle_poll_async(server.clone(), "peer".into(), 100).await;
    assert!(response.ok);
    assert_eq!(response.data["count"], 1);
    assert_eq!(
        server.state.lock().unwrap().msgs["recv-message"].state,
        "read"
    );
    std::fs::remove_dir_all(root).ok();
}

fn seeded_receive_peer(server: &Server, root: &Path, id: &str, worker: &str) -> String {
    let _ = root;
    register(server, worker, "%peer");
    server.commit(&[Event::Sent {
        msg: Message {
            id: id.into(),
            from: "sender".into(),
            to: worker.into(),
            mtype: "notify".into(),
            subject: Some("receive".into()),
            body: format!("body-{id}"),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);
    root.display().to_string()
}

#[tokio::test]
async fn receive_id_commits_the_batch_and_replays_it_after_a_lost_response() {
    let (server, root) = test_server();
    seeded_receive_peer(&server, &root, "receive-loss-message", "peer");
    let server = Arc::new(server);

    // First poll carries a caller-owned identity and commits the batch. The
    // caller never sees this response: it simulates a lost socket reply.
    let first = handle_poll_async_with_context(
        server.clone(),
        "peer".into(),
        None,
        0,
        None,
        Some("receive-loss-1".into()),
    )
    .await;
    assert!(first.ok, "{first:?}");
    assert_eq!(first.data["receive_id"], "receive-loss-1");
    assert_eq!(first.data["replayed"], false);
    assert_eq!(first.data["messages"][0]["id"], "receive-loss-message");
    assert_eq!(
        server.state.lock().unwrap().msgs["receive-loss-message"].state,
        "read",
        "the receive identity commits the consumption in the same transaction"
    );

    // The same identity must return the exact committed batch, not an empty
    // inbox, so a lost response is recoverable by the same caller.
    let replayed = handle_poll_async_with_context(
        server.clone(),
        "peer".into(),
        None,
        0,
        None,
        Some("receive-loss-1".into()),
    )
    .await;
    assert!(replayed.ok, "{replayed:?}");
    assert_eq!(replayed.data["replayed"], true);
    assert_eq!(replayed.data["count"], 1);
    assert_eq!(replayed.data["messages"][0]["id"], "receive-loss-message");
    assert_eq!(
        replayed.data["messages"][0]["body"],
        "body-receive-loss-message"
    );
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn receive_id_replay_survives_a_truncated_journal_restart() {
    let (server, root) = test_server();
    seeded_receive_peer(&server, &root, "receive-restart-message", "peer");
    let server = Arc::new(server);
    let first = handle_poll_async_with_context(
        server.clone(),
        "peer".into(),
        None,
        0,
        None,
        Some("receive-restart-1".into()),
    )
    .await;
    assert!(first.ok, "{first:?}");
    assert_eq!(first.data["messages"][0]["id"], "receive-restart-message");
    drop(server);

    // A fresh reducer replayed from the committed journal must still answer
    // the same identity with the same batch instead of an unread inbox.
    let restored = replay(&root).expect("replay committed journal");
    let receipt = restored
        .receive_receipts
        .get("receive-restart-1")
        .expect("receive receipt must be durable");
    assert_eq!(receipt.worker_id, "peer");
    assert_eq!(
        receipt.message_ids,
        vec!["receive-restart-message".to_owned()]
    );
    assert!(restored.msgs["receive-restart-message"].state == "read");
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn receive_id_rejects_another_actor_and_another_route() {
    let (server, root) = test_server();
    seeded_receive_peer(&server, &root, "receive-owner-message", "peer");
    register(&server, "other", "%other");
    let server = Arc::new(server);
    let first = handle_poll_async_with_context(
        server.clone(),
        "peer".into(),
        None,
        0,
        None,
        Some("receive-owner-1".into()),
    )
    .await;
    assert!(first.ok, "{first:?}");

    let foreign = handle_poll_async_with_context(
        server.clone(),
        "other".into(),
        None,
        0,
        None,
        Some("receive-owner-1".into()),
    )
    .await;
    assert!(!foreign.ok, "{foreign:?}");
    assert!(
        foreign
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("RECEIVE_IDENTITY_MISMATCH")),
        "{foreign:?}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn legacy_sent_event_replay_classifies_message_without_reply_reference() {
    let (_server, root) = test_server();
    let journal_path = root.join(".agent-collab/server/journal.jsonl");
    std::fs::write(
        &journal_path,
        r#"{"ev":"Sent","msg":{"id":"legacy-message","from":"peer-a","to":"peer-b","type":"request","subject":"legacy","body":"legacy body","created_ms":1,"state":"pending"}}
"#,
    )
    .unwrap();

    let state = replay(&root).expect("legacy Sent event must replay");
    let message = state
        .msgs
        .get("legacy-message")
        .expect("replay must retain the legacy message");
    assert_eq!(message.in_reply_to, None);
    assert_eq!(message.mtype, "request");
    assert_eq!(
        state
            .inbox_of("peer-b")
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["legacy-message"]
    );
    assert!(!state.answered("legacy-message"));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn malformed_journal_replay_fails_fast() {
    let (_server, root) = test_server();
    std::fs::write(
        root.join(".agent-collab/server/journal.jsonl"),
        "{manual-edit\n",
    )
    .unwrap();
    let error = replay(&root).err().expect("malformed journal must fail");
    assert!(error
        .to_string()
        .contains("manual journal edits are unsupported"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn concatenated_journal_events_replay_and_self_heal() {
    let (_server, root) = test_server();
    let journal_path = root.join(".agent-collab/server/journal.jsonl");
    let event1 = json!({"ev":"KeepaliveUpdated","worker_id":"w1","record":{"observed":"unknown","idle_since_ms":100,"activity_ms":50,"last_notice_ms":0,"last_notice_id":null,"unacked":0,"suspected_offline":false}}).to_string();
    let event2 = json!({"ev":"KeepaliveUpdated","worker_id":"w2","record":{"observed":"unknown","idle_since_ms":200,"activity_ms":150,"last_notice_ms":0,"last_notice_id":null,"unacked":0,"suspected_offline":false}}).to_string();
    std::fs::write(&journal_path, format!("{}{}\n", event1, event2)).unwrap();

    let state = replay(&root).expect("concatenated journal events must self-heal and replay");
    assert_eq!(state.keepalives.len(), 2);
    assert_eq!(state.keepalives["w1"].idle_since_ms, 100);
    assert_eq!(state.keepalives["w2"].idle_since_ms, 200);

    let content = std::fs::read_to_string(&journal_path).unwrap();
    let lines: Vec<_> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn worktree_path_budget_accepts_short_slug_and_rejects_escape() {
    let root = std::env::temp_dir().join(format!(
        "collab-worktree-path-{}-{}",
        std::process::id(),
        now_ms()
    ));
    std::fs::create_dir_all(root.join("playground")).unwrap();
    assert!(validate_worktree_path(&root, "./playground/ar03-0828").is_ok());
    assert!(validate_worktree_path(
        &root,
        "./playground/v3-direct-sse-terminal-observability-20260827-long-run-id"
    )
    .is_err());
    assert!(validate_worktree_path(&root, "./playground/../outside").is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/tmp", root.join("playground/link")).unwrap();
        assert!(validate_worktree_path(&root, "./playground/link/escape").is_err());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn tmux_notification_requests_durable_receive() {
    let text = notification_text(&Message {
        id: "message-id".into(),
        from: "sender".into(),
        to: "recipient".into(),
        mtype: "notify".into(),
        subject: Some("release".into()),
        body: "RESOURCE_RELEASED feature=shared".into(),
        in_reply_to: None,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    })
    .unwrap();
    assert_eq!(
        text,
        "COLLAB_NOTIFY message-id [release] RESOURCE_RELEASED feature=shared | P1 ACTION: the resource is free; resume the task that waited on it. Details: run `collab recv` to consume this durable notification; inspect `collab msg message-id` for details. | READ IS NOT DONE: never end your turn on an ACK, a read, or a summary. After handling, resume your current task; if you own none, run `appsdk longhorizon show` and take work."
    );
    assert!(!text.contains("ACK this notice"));
    assert!(text.contains("READ IS NOT DONE"));
}

#[tokio::test]
async fn tmux_peer_send_then_collab_recv_commits_queryable_consumption() {
    let (mut server, root) = test_server();
    let tmux = IsolatedTmux::start(&root);
    let endpoints = tmux.endpoints();
    assert!(register_tmux(&server, "tmux-a", endpoints[0].clone()).ok);
    assert!(register_tmux(&server, "tmux-b", endpoints[1].clone()).ok);
    server.appserver_notification_sink = default_appserver_notification_sink();
    server.config.notifications.enabled = true;

    let subscription = handle_notification_subscribe(
        &server,
        "tmux-b".into(),
        "token-tmux-b".into(),
        "direct-message".into(),
        None,
        None,
        Vec::new(),
        None,
        1,
        3600,
    );
    assert!(subscription.ok, "subscription failed: {subscription:?}");

    let sent = handle_send(
        &server,
        "tmux-a".into(),
        "tmux-b".into(),
        "notify".into(),
        Some("tmux durable receive".into()),
        "execute collab recv for this message".into(),
        None,
        "immediate".into(),
    );
    assert!(sent.ok, "send failed: {sent:?}");
    assert_eq!(sent.data["notification"], "tmux-input-submitted");
    assert_eq!(sent.data["consumed"], false);
    let message_id = sent.data["msg_id"].as_str().expect("message id").to_owned();
    let server = Arc::new(server);
    let route_scope = crate::scope::RouteScope {
        app_scope_id: AppServerId::new("tui-default").unwrap(),
        project_scope_id: crate::server::global_state::GlobalState::canonical_project_scope(&root)
            .unwrap(),
    };
    let binding = server
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(&route_scope, &BindingId::new("binding-tmux-b").unwrap())
        .unwrap()
        .clone();
    let runtime = crate::identity::RuntimeIdentity {
        agent_id: binding.agent_id,
        runtime_id: binding.runtime_id,
        appserver_id: binding.app_scope_id,
        endpoint_generation: binding.endpoint_generation,
        binding_id: binding.binding_id,
        session_id: binding.session_id,
        native_thread_id: binding.native_thread_id,
    };
    let project_context =
        crate::proto::ProjectContext::for_registered_route(&root, &runtime).unwrap();
    let before_recv = dispatch(
        &server,
        Req::MsgStatus {
            msg_id: message_id.clone(),
        },
    );
    assert!(before_recv.ok, "message status failed: {before_recv:?}");
    assert_eq!(before_recv.data["consumed_by_recv"], false);

    // Build the exact Poll request used by the production `collab recv`
    // command; the server handler then commits consumption and its receipt.
    let request = crate::recv_request(
        "tmux-b".into(),
        "token-tmux-b".into(),
        0,
        "tmux-b-recv-1".into(),
    );
    let Req::Poll {
        worker_id,
        token,
        timeout_ms,
        receive_id,
    } = request
    else {
        panic!("collab recv must issue a Poll request");
    };
    assert_eq!(worker_id, "tmux-b");
    assert_eq!(receive_id.as_deref(), Some("tmux-b-recv-1"));
    let received = handle_poll_async_with_context(
        server.clone(),
        worker_id,
        Some(token),
        timeout_ms,
        Some(project_context),
        receive_id,
    )
    .await;
    assert!(received.ok, "collab recv failed: {received:?}");
    assert_eq!(received.data["count"], 1);
    assert_eq!(received.data["messages"][0]["id"], message_id);
    assert_eq!(received.data["receive_id"], "tmux-b-recv-1");

    let consumed = dispatch(&server, Req::MsgStatus { msg_id: message_id });
    assert!(consumed.ok, "message status failed: {consumed:?}");
    assert_eq!(consumed.data["consumed_by_recv"], true);
    assert_eq!(consumed.data["state"], "read");

    drop(server);
    drop(tmux);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn appserver_notification_classifies_priority_and_names_one_action() {
    let message = |from: &str, mtype: &str, subject: &str| {
        notification_text(&Message {
            id: "m1".into(),
            from: from.into(),
            to: "master".into(),
            mtype: mtype.into(),
            subject: Some(subject.into()),
            body: "body".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        })
        .unwrap()
    };
    let notify = |subject: &str| message("peer", "notify", subject);
    let internal = |subject: &str| message("collab-server", "notification", subject);

    assert!(notify("worker-idle: w1").contains("P1 ACTION: dispatch work to this idle capacity"));
    assert!(notify("master-idle: master").contains("P1 ACTION: run the scheduling pass"));
    assert!(notify("worker-unresponsive: w1")
        .contains("P1 ACTION: inspect durable tasks and mailbox for this worker"));
    assert!(notify("task-keepalive 1/3").contains("P1 ACTION: continue your own task"));
    assert!(notify("blocker:task").contains("P1 ACTION:"));
    assert!(notify("unblock:task").contains("P1 ACTION:"));
    assert!(notify("wait-timeout:task").contains("P1 ACTION:"));
    assert!(notify("scheduling-blocker: queue stalled").contains("P1 ACTION:"));
    assert!(compose_notification("batch", "notification-batch", "body").contains("P1 ACTION:"));
    assert!(
        notify("goal:plan.md").contains("P1 ACTION: do the in-scope action the message asks for")
    );
    assert!(notify("goal:<id>").contains("P1 ACTION:"));
    assert!(notify("deadline:<id>").contains("P1 ACTION:"));
    assert!(!notify("goal:plan.md").contains("P0 ACTION:"));
    assert!(!notify("deadline:<id>").contains("P0 ACTION:"));
    let goal = internal("goal:plan.md");
    assert!(goal.starts_with("COLLAB_NOTIFY m1 [goal:plan.md] body | P0 ACTION:"));
    assert!(goal.contains("P0 ACTION: run the long-horizon briefing"));
    assert!(internal("goal:<id>").contains("P0 ACTION: run the long-horizon briefing"));
    assert!(internal("deadline:<id>").contains("P0 ACTION: run the long-horizon briefing"));
    assert!(!internal("goal:plan.md").contains("P1 ACTION:"));
    assert!(!internal("deadline:<id>").contains("P1 ACTION:"));
    assert!(message("peer", "notification", "goal:plan.md").contains("P1 ACTION:"));
    assert!(message("peer", "notification", "deadline:<id>").contains("P1 ACTION:"));
    assert!(message("collab-server", "notify", "goal:plan.md").contains("P1 ACTION:"));
    assert!(message("collab-server", "notify", "deadline:<id>").contains("P1 ACTION:"));
    assert!(internal("goal").contains("P1 ACTION:"));
    assert!(internal("deadline").contains("P1 ACTION:"));
    assert!(notify("goalpost: reached").contains("P1 ACTION:"));
    assert!(notify("deadline-notice: task due").contains("P1 ACTION:"));
    assert!(!notify("goalpost: reached").contains("P0 ACTION:"));
    assert!(!notify("deadline-notice: task due").contains("P0 ACTION:"));
    assert!(notify("Settings delivery recorded").contains("P2 ACTION: note it"));

    // Every class carries the resume protocol, not just the operational ones.
    for subject in [
        "worker-idle: w1",
        "goal:plan.md",
        "Settings delivery recorded",
    ] {
        assert!(notify(subject).contains("resume your current task"));
    }
    assert!(internal("goal:plan.md").contains("resume your current task"));
}

#[test]
fn appserver_notification_truncates_body_without_dropping_the_action_contract() {
    let text = notification_text(&Message {
        id: "message-id".into(),
        from: "sender".into(),
        to: "recipient".into(),
        mtype: "notify".into(),
        subject: Some("release".into()),
        body: "x".repeat(4000),
        in_reply_to: None,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    })
    .unwrap();

    assert!(text.chars().count() <= 1024);
    assert!(text.contains("P1 ACTION:"));
    assert!(text.contains("READ IS NOT DONE"));
    assert!(text.ends_with("run `appsdk longhorizon show` and take work."));
}

#[test]
fn appserver_notification_abbreviates_subject_and_escapes_body_controls() {
    let text = notification_text(&Message {
        id: "message-id".into(),
        from: "sender".into(),
        to: "recipient".into(),
        mtype: "notify".into(),
        subject: Some(
            "this subject is deliberately longer than forty eight visible characters".into(),
        ),
        body: "line one\nline two\t中文".into(),
        in_reply_to: None,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    })
    .unwrap();
    assert_eq!(
        text,
        "COLLAB_NOTIFY message-id [this subject is deliberately longer than forty …] line one\\nline two\\t中文 | P1 ACTION: do the in-scope action the message asks for. Details: run `collab recv` to consume this durable notification; inspect `collab msg message-id` for details. | READ IS NOT DONE: never end your turn on an ACK, a read, or a summary. After handling, resume your current task; if you own none, run `appsdk longhorizon show` and take work."
    );
}

#[test]
fn appserver_notification_long_goal_deadline_subjects_keep_the_typed_prefix() {
    for prefix in ["goal:", "deadline:"] {
        let subject = format!("{prefix}{}", "x".repeat(80));
        let text = notification_text(&Message {
            id: "message-id".into(),
            from: "collab-server".into(),
            to: "master".into(),
            mtype: "notification".into(),
            subject: Some(subject),
            body: "body".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        })
        .unwrap();
        assert!(
            text.starts_with(&format!("COLLAB_NOTIFY message-id [{prefix}")),
            "{text}"
        );
        assert!(text.contains("] body | P0 ACTION: run the long-horizon briefing"));
    }
}

#[test]
fn authenticated_send_cannot_claim_internal_goal_deadline_owner() {
    let (server, root) = test_server();
    let server = Arc::new(server);
    assert!(register(&server, "sender", "%sender").ok);
    assert!(register(&server, "recipient", "%recipient").ok);

    let mut request = authenticated_send(&root, "sender", "recipient", "goal:plan.md");
    if let Req::Send { mtype, .. } = &mut request {
        *mtype = "notification".into();
    }
    let response = dispatch(&server, request);
    assert_eq!(
        response.error.as_deref(),
        Some("peer messaging requires type notify")
    );
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sendmessage_requires_subject_before_state_mutation() {
    let (server, root) = test_server();
    let response = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        None,
        "The candidate is ready.".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some("MESSAGE_SUBJECT_REQUIRED: sendmessage requires --subject")
    );
    assert!(server.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn send_without_subscription_is_mailbox_only_and_deduplicated() {
    let (server, root) = test_server();
    register(&server, "sender", "%collab-missing-sender");
    register(&server, "recipient", "%collab-missing-recipient");
    let subscription_id = server
        .state
        .lock()
        .unwrap()
        .notification_subscriptions
        .values()
        .find(|subscription| subscription.worker_id == "recipient")
        .unwrap()
        .id
        .clone();
    server.commit(&[Event::NotificationStatus {
        subscription_id,
        status: "cancelled".into(),
        updated_ms: now_ms(),
    }]);
    let first = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("occupied".into()),
        "RESOURCE_OCCUPIED feature=shared".into(),
        None,
        "immediate".into(),
    );
    assert!(first.ok);
    let message_id = first.data["msg_id"].as_str().unwrap().to_owned();
    assert_eq!(server.state.lock().unwrap().msgs.len(), 1);
    assert!(root
        .join(".agent-collab/mailbox")
        .join(format!("{message_id}.json"))
        .exists());
    let jsonl = root.join(".agent-collab/mailbox/recipient-recipient.jsonl");
    assert!(std::fs::read_to_string(&jsonl)
        .unwrap()
        .lines()
        .any(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .map(|record| {
                    record["schema_version"] == 1
                        && record["record_type"] == "message"
                        && record["recipient"] == "recipient"
                        && record["category"] == "direct"
                        && record["task_ids"].is_array()
                        && record["created_ms"].is_i64()
                        && record["window_start_ms"].is_null()
                        && record["window_end_ms"].is_null()
                        && record["state"] == "pending"
                        && record["exact_error"].as_str().is_some_and(|error| {
                            error.starts_with("MAILBOX_SCOPE_BINDING_UNAVAILABLE:")
                        })
                        && record["message"]["id"] == message_id
                })
                .unwrap_or(false)
        }));
    assert_eq!(
        replay(&root).unwrap().msgs[&message_id].body,
        "RESOURCE_OCCUPIED feature=shared"
    );
    assert_eq!(first.data["notification"], "mailbox-only-no-subscription");
    assert_eq!(first.data["repair_required"], true);
    assert_eq!(first.data["failure"], "notification_subscription_missing");
    assert_eq!(
        server.state.lock().unwrap().msgs[&message_id].wake_attempt_count,
        0
    );
    assert!(!server.log_path().exists());

    let duplicate = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("occupied".into()),
        "RESOURCE_OCCUPIED feature=shared".into(),
        None,
        "immediate".into(),
    );
    assert!(duplicate.ok);
    assert_eq!(duplicate.data["msg_id"], message_id);
    assert_eq!(duplicate.data["deduplicated"], true);
    assert_eq!(server.state.lock().unwrap().msgs.len(), 1);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_peer_notification_accepts_arbitrary_durable_body() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    let response = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready for your review.".into(),
        None,
        "immediate".into(),
    );
    assert!(response.ok);
    let message_id = response.data["msg_id"].as_str().unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.msgs[message_id].body,
        "The candidate is ready for your review."
    );
    assert_eq!(state.msgs[message_id].subject.as_deref(), Some("review"));
    assert_eq!(state.msgs[message_id].wake_attempt_count, 1);
    assert_eq!(response.data["notification"], "tmux-input-submitted");
    assert_eq!(response.data["consumed"], false);
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_send_reports_tmux_wake_rejection_after_durable_commit() {
    let (mut server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    server.appserver_notification_sink = Arc::new(|_, _, _, _, _, _| {
        Err("TMUX_ENTER_SUBMIT_FAILED: forced send-keys failure".into())
    });

    let response = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready for your review.".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some("TMUX_NOTIFICATION_REJECTED: TMUX_ENTER_SUBMIT_FAILED: forced send-keys failure")
    );
    assert_eq!(response.data["durable"], true);
    assert_eq!(response.data["notification"], "subscribed-not-sent");
    assert_eq!(
        response.data["notification_error"],
        "TMUX_ENTER_SUBMIT_FAILED: forced send-keys failure"
    );
    assert_eq!(response.data["failure"], "notification_delivery_failed");
    assert_eq!(response.data["repair_required"], true);
    let escalation = response.data["escalation"].as_str().unwrap();
    assert!(escalation.contains("collab recv"), "{escalation}");
    assert!(
        escalation.contains("do not retry this wake"),
        "{escalation}"
    );
    assert!(!escalation.contains("retry explicitly"), "{escalation}");
    let message_id = response.data["msg_id"].as_str().unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[message_id].wake_attempt_count, 1);
    assert_eq!(state.msgs[message_id].state, "pending");
    assert_eq!(
        state.notification_delivery_failures[message_id].error,
        "TMUX_ENTER_SUBMIT_FAILED: forced send-keys failure"
    );
    assert!(!state.notification_delivery_failures[message_id].retryable);
    drop(state);
    let replayed = replay(&root).unwrap();
    let failure = &replayed.notification_delivery_failures[message_id];
    assert_eq!(failure.operation, "notification.emitted");
    assert_eq!(
        failure.error,
        "TMUX_ENTER_SUBMIT_FAILED: forced send-keys failure"
    );
    assert!(failure.failed_ms > 0);

    {
        let state = server.state.lock().unwrap();
        server.rewrite_journal_locked(&state).unwrap();
    }
    let compacted = replay(&root).unwrap();
    assert_eq!(
        compacted.notification_delivery_failures[message_id],
        *failure
    );
    assert!(root
        .join(".agent-collab/mailbox")
        .join(format!("{message_id}.json"))
        .exists());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_send_duplicate_after_rejection_retries_the_same_message_once() {
    let (mut server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    let attempts = Arc::new(AtomicU32::new(0));
    let attempts_for_sink = Arc::clone(&attempts);
    server.appserver_notification_sink = Arc::new(move |_, _, _, _, _, _| {
        let attempt = attempts_for_sink.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            Err(
                "ADAPTER_ROUTE_UNAVAILABLE: thread is persisted but not loaded by the App Server"
                    .into(),
            )
        } else {
            Ok(json!({"accepted": true}))
        }
    });

    let first = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready for your review.".into(),
        None,
        "immediate".into(),
    );
    assert!(!first.ok);
    let message_id = first.data["msg_id"].as_str().unwrap().to_owned();
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
        assert!(!state.msgs[&message_id].retry_attempted);
        assert!(state.notification_delivery_failures[&message_id].retryable);
    }

    let duplicate = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready for your review.".into(),
        None,
        "immediate".into(),
    );
    assert!(duplicate.ok, "{duplicate:?}");
    assert_eq!(duplicate.data["msg_id"], message_id);
    assert_eq!(duplicate.data["durable"], true);
    assert_eq!(duplicate.data["deduplicated"], true);
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs.len(), 1);
        assert_eq!(state.msgs[&message_id].wake_attempt_count, 2);
        assert!(state.msgs[&message_id].retry_attempted);
    }

    let third = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready for your review.".into(),
        None,
        "immediate".into(),
    );
    assert!(!third.ok);
    assert_eq!(third.data["msg_id"], message_id);
    assert_eq!(
        third.error.as_deref(),
        Some("TMUX_NOTIFICATION_REJECTED: no notification batch is ready")
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let replayed = replay(&root).unwrap();
    assert_eq!(replayed.msgs[&message_id].wake_attempt_count, 2);
    assert!(replayed.msgs[&message_id].retry_attempted);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_retry_is_refused_for_an_accepted_original_attempt() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    let first = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "accepted once".into(),
        None,
        "immediate".into(),
    );
    assert!(first.ok, "{first:?}");
    let message_id = first.data["msg_id"].as_str().unwrap().to_owned();
    {
        let state = server.state.lock().unwrap();
        assert!(state
            .notification_delivery_accepted
            .contains_key(&message_id));
        assert!(!state.msgs[&message_id].retry_attempted);
    }

    let duplicate = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "accepted once".into(),
        None,
        "immediate".into(),
    );
    assert!(!duplicate.ok, "{duplicate:?}");
    assert_eq!(duplicate.data["msg_id"], message_id);
    assert_eq!(
        duplicate.error.as_deref(),
        Some("TMUX_NOTIFICATION_REJECTED: no notification batch is ready")
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
    assert!(!state.msgs[&message_id].retry_attempted);
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_retry_is_refused_for_an_unknown_original_attempt() {
    let (mut server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    server.appserver_notification_sink =
        Arc::new(|_, _, _, _, _, _| Err("ADAPTER_TIMEOUT: turn/start timed out".into()));
    let first = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "unknown outcome".into(),
        None,
        "immediate".into(),
    );
    assert!(!first.ok);
    let message_id = first.data["msg_id"].as_str().unwrap().to_owned();
    {
        let state = server.state.lock().unwrap();
        assert!(!state.notification_delivery_failures[&message_id].retryable);
    }

    let duplicate = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "unknown outcome".into(),
        None,
        "immediate".into(),
    );
    assert!(!duplicate.ok);
    assert_eq!(
        duplicate.error.as_deref(),
        Some("TMUX_NOTIFICATION_REJECTED: no notification batch is ready")
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
    assert!(!state.msgs[&message_id].retry_attempted);
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_retry_is_refused_for_a_decode_failure() {
    let (mut server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    server.appserver_notification_sink = Arc::new(|_, _, _, _, _, _| {
        Err("ADAPTER_UNKNOWN: decode response: missing result payload".into())
    });
    let first = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "undecodable outcome".into(),
        None,
        "immediate".into(),
    );
    assert!(!first.ok);
    let message_id = first.data["msg_id"].as_str().unwrap().to_owned();
    {
        let state = server.state.lock().unwrap();
        assert!(!state.notification_delivery_failures[&message_id].retryable);
    }

    let duplicate = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "undecodable outcome".into(),
        None,
        "immediate".into(),
    );
    assert!(!duplicate.ok);
    assert_eq!(
        duplicate.error.as_deref(),
        Some("TMUX_NOTIFICATION_REJECTED: no notification batch is ready")
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
    assert!(!state.msgs[&message_id].retry_attempted);
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn reregistration_replaces_a_stale_default_target_with_current_tmux_route() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);
    {
        let mut state = server.state.lock().unwrap();
        state
            .notification_subscriptions
            .get_mut("sub-default-direct-message-peer")
            .unwrap()
            .target = "thread-stale".into();
    }
    assert!(register(&server, "peer", "%peer").ok);
    let state = server.state.lock().unwrap();
    let subscription = &state.notification_subscriptions["sub-default-direct-message-peer"];
    let current_thread = state.workers["peer"]
        .transport
        .as_ref()
        .and_then(|transport| transport.thread_id.as_deref())
        .unwrap();
    assert_eq!(subscription.target, current_thread);
    assert_eq!(subscription.method, "tmux");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_send_reports_when_subscription_appserver_endpoint_mismatches() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    server
        .state
        .lock()
        .unwrap()
        .workers
        .get_mut("recipient")
        .unwrap()
        .transport = Some(test_appserver_transport("mismatched-thread-recipient"));

    let response = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready for your review.".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some(
            "APPSERVER_NOTIFICATION_REJECTED: subscription does not match the selected appserver transport",
        )
    );
    assert_eq!(response.data["notification"], "subscribed-not-sent");
    assert_eq!(
        response.data["notification_error"],
        "subscription does not match the selected appserver transport"
    );
    let message_id = response.data["msg_id"].as_str().unwrap();
    {
        let state = server.state.lock().unwrap();
        let failure = &state.notification_delivery_failures[message_id];
        assert_eq!(failure.operation, "notification.not_attempted");
        assert_eq!(
            failure.error,
            "subscription does not match the selected appserver transport"
        );
    }
    assert_eq!(
        replay(&root).unwrap().notification_delivery_failures[message_id].error,
        "subscription does not match the selected appserver transport"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_send_reports_appserver_wake_rejection_after_durable_commit() {
    let (mut server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "recipient", "%recipient");
    {
        let mut state = server.state.lock().unwrap();
        let transport = test_appserver_transport("thread-recipient-appserver");
        let thread = transport.thread_id.clone().unwrap();
        state.workers.get_mut("recipient").unwrap().transport = Some(transport);
        let subscription = state
            .notification_subscriptions
            .get_mut("sub-default-direct-message-recipient")
            .unwrap();
        subscription.method = "appserver".into();
        subscription.target = thread;
    }
    server.appserver_notification_sink = Arc::new(|_, _, _, _, _, _| {
        Err("ADAPTER_ROUTE_UNAVAILABLE: turn/start forced failure".into())
    });

    let response = handle_send(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("review".into()),
        "The candidate is ready for your review.".into(),
        None,
        "immediate".into(),
    );
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some(
            "APPSERVER_NOTIFICATION_REJECTED: ADAPTER_ROUTE_UNAVAILABLE: turn/start forced failure"
        )
    );
    assert_eq!(response.data["durable"], true);
    assert_eq!(response.data["notification"], "subscribed-not-sent");
    std::fs::remove_dir_all(root).ok();
}
