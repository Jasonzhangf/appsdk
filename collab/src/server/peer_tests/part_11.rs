#[test]
fn first_offline_status_sets_baseline_then_online_transition_notifies_once() {
    let (server, root) = test_server();
    register(&server, "master-worker", "thread-master");
    register(&server, "cold-worker", "thread-cold");
    kill_registered_worker_pane(&server, "cold-worker");
    let server_arc = std::sync::Arc::new(server);
    let promote_resp = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            approval: "approved".into(),
        },
    );
    assert!(promote_resp.ok);

    let offline = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("cold-worker".into()),
        },
    );
    assert!(offline.ok, "{offline:?}");
    assert_eq!(offline.data["workers"][0]["status"], "lost");
    {
        let state = server_arc.state.lock().unwrap();
        assert_eq!(state.keepalives["cold-worker"].notified_presence, "offline");
        assert!(state.msgs.values().all(|m| {
            !(m.to == "master-worker"
                && m.subject == Some("worker-unresponsive: cold-worker".into()))
        }));
    }

    assert!(register(&server_arc, "cold-worker", "thread-cold").ok);
    let online = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("cold-worker".into()),
        },
    );
    assert!(online.ok, "{online:?}");
    assert_eq!(online.data["workers"][0]["endpoint_live"], true);
    assert_eq!(online.data["workers"][0]["agent_state"], "unknown");
    let repeated = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("cold-worker".into()),
        },
    );
    assert!(repeated.ok, "{repeated:?}");
    let state = server_arc.state.lock().unwrap();
    assert_eq!(
        state
            .msgs
            .values()
            .filter(|m| {
                m.to == "master-worker" && m.subject == Some("worker-recovered: cold-worker".into())
            })
            .count(),
        1,
        "offline->online status edge must notify exactly once"
    );
    assert_eq!(state.keepalives["cold-worker"].notified_presence, "online");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_presence_probe_after_reregister_does_not_notify_or_mutate_new_worker() {
    let (server, root) = test_server();
    register(&server, "master-worker", "thread-master");
    register(&server, "edge-worker", "thread-stale");
    let server_arc = std::sync::Arc::new(server);
    let promote_resp = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            approval: "approved".into(),
        },
    );
    assert!(promote_resp.ok);
    let stale_worker = server_arc.state.lock().unwrap().workers["edge-worker"].clone();
    kill_registered_worker_pane(&server_arc, "edge-worker");
    assert!(register(&server_arc, "edge-worker", "thread-stale").ok);
    assert_eq!(
        worker_identity_presence(&server_arc, &stale_worker),
        IdentityPresence::Missing
    );
    server_arc.commit(&[Event::KeepaliveUpdated {
        worker_id: "edge-worker".into(),
        record: crate::server::keepalive::Record {
            notified_presence: "online".into(),
            ..Default::default()
        },
    }]);

    let status = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("edge-worker".into()),
        },
    );
    assert!(status.ok, "{status:?}");
    assert_eq!(status.data["workers"][0]["presence"], "present");
    assert_eq!(status.data["workers"][0]["agent_state"], "unknown");
    let state = server_arc.state.lock().unwrap();
    assert_eq!(state.workers["edge-worker"].token, "token-edge-worker");
    assert_eq!(state.keepalives["edge-worker"].notified_presence, "online");
    assert!(state.msgs.values().all(|m| {
        !(m.to == "master-worker" && m.subject == Some("worker-unresponsive: edge-worker".into()))
    }));
    assert!(!state
        .master_wake
        .unresponsive_workers
        .contains(&"edge-worker".into()));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn presence_edge_records_tmux_peer_presence() {
    let (server, root) = test_server();
    let tmux = IsolatedTmux::start(&root);
    let endpoints = tmux.endpoints();
    assert!(register_tmux(&server, "master-worker", endpoints[0].clone()).ok);
    assert!(register_tmux(&server, "edge-worker", endpoints[1].clone()).ok);
    let mut server_arc = std::sync::Arc::new(server);
    let promote_resp = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            approval: "approved".into(),
        },
    );
    assert!(promote_resp.ok);

    let status = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("edge-worker".into()),
        },
    );
    assert!(status.ok, "{status:?}");
    assert_eq!(status.data["workers"][0]["presence"], "present");
    assert_eq!(
        server_arc.state.lock().unwrap().keepalives["edge-worker"].notified_presence,
        "online"
    );
    drop(server_arc);
    drop(tmux);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn closed_and_delivered_tasks_do_not_trigger_keepalives() {
    let (server, root) = test_server();
    register(&server, "worker-a", "thread-worker-a");
    let server_arc = std::sync::Arc::new(server);
    let base = now_ms();

    // Create a task that is delivered
    server_arc.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "task-delivered".into(),
            owner: "worker-a".into(),
            created_by: "worker-a".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            status: "delivered".into(),
            next_step: None,
            wait: None,
            created_ms: base,
            updated_ms: base,
        },
    }]);

    // Tick scheduler - delivered task must NOT generate keepalive.
    crate::server::keepalive::tick_at(&server_arc, base + 900_000);
    assert!(server_arc.state.lock().unwrap().msgs.is_empty());

    // Now update task to closed
    server_arc.commit(&[Event::TaskUpdated {
        task: TaskRec {
            id: "task-delivered".into(),
            owner: "worker-a".into(),
            created_by: "worker-a".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            status: "closed".into(),
            next_step: None,
            wait: None,
            created_ms: base,
            updated_ms: base,
        },
    }]);

    // Tick scheduler - closed task must NOT generate keepalive.
    crate::server::keepalive::tick_at(&server_arc, base + 1_800_000);
    assert!(server_arc.state.lock().unwrap().msgs.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn status_all_aggregates_workers_tasks_subagents_and_summary() {
    let (server, root) = test_server();
    let server_arc = Arc::new(server);

    // Register a worker
    server_arc.commit(&[Event::Registered {
        worker: WorkerRec {
            id: "worker-1".into(),
            token: "tok-1".into(),
            cwd: root.display().to_string(),
            registered_ms: 1000,
            transport: Some(test_appserver_transport("thread-worker-1")),
        },
    }]);

    // Create a task
    server_arc.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "task-1".into(),
            owner: "worker-1".into(),
            created_by: "master".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "normal".into(),
            status: "working".into(),
            next_step: Some("implementing".into()),
            wait: None,
            created_ms: 1000,
            updated_ms: 1000,
        },
    }]);

    let resp = dispatch(&server_arc, Req::StatusAll);
    assert!(resp.ok);
    assert_eq!(resp.data["summary"]["workers"], 1);
    assert_eq!(resp.data["summary"]["tasks"], 1);
    assert_eq!(resp.data["workers"].as_array().unwrap().len(), 1);
    assert_eq!(resp.data["workers"][0]["id"], "worker-1");
    assert_eq!(resp.data["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(resp.data["tasks"][0]["id"], "task-1");

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mailbox_read_all_chronological_sort_asc_and_desc() {
    let (server, root) = test_server();
    let server_arc = Arc::new(server);

    // Send messages with different timestamps
    server_arc.commit(&[
        Event::Sent {
            msg: Message {
                id: "m-1".into(),
                from: "alice".into(),
                to: "bob".into(),
                mtype: "notify".into(),
                subject: Some("first".into()),
                body: "first body".into(),
                in_reply_to: None,
                created_ms: 1000,
                state: "delivered".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::Sent {
            msg: Message {
                id: "m-2".into(),
                from: "bob".into(),
                to: "alice".into(),
                mtype: "notify".into(),
                subject: Some("second".into()),
                body: "second body".into(),
                in_reply_to: None,
                created_ms: 2000,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::Sent {
            msg: Message {
                id: "m-3".into(),
                from: "charlie".into(),
                to: "bob".into(),
                mtype: "notify".into(),
                subject: Some("third".into()),
                body: "third body".into(),
                in_reply_to: None,
                created_ms: 3000,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
    ]);

    // Test time-asc (default)
    let asc_resp = dispatch(
        &server_arc,
        Req::MailboxRead {
            all: true,
            sort: Some("time-asc".into()),
            worker_id: None,
        },
    );
    assert!(asc_resp.ok);
    assert_eq!(asc_resp.data["count"], 3);
    let asc_msgs = asc_resp.data["messages"].as_array().unwrap();
    assert_eq!(asc_msgs[0]["id"], "m-1");
    assert_eq!(asc_msgs[1]["id"], "m-2");
    assert_eq!(asc_msgs[2]["id"], "m-3");

    // Test time-desc
    let desc_resp = dispatch(
        &server_arc,
        Req::MailboxRead {
            all: true,
            sort: Some("time-desc".into()),
            worker_id: None,
        },
    );
    assert!(desc_resp.ok);
    let desc_msgs = desc_resp.data["messages"].as_array().unwrap();
    assert_eq!(desc_msgs[0]["id"], "m-3");
    assert_eq!(desc_msgs[1]["id"], "m-2");
    assert_eq!(desc_msgs[2]["id"], "m-1");

    // Test worker filter
    let worker_resp = dispatch(
        &server_arc,
        Req::MailboxRead {
            all: false,
            sort: Some("time-asc".into()),
            worker_id: Some("charlie".into()),
        },
    );
    assert!(worker_resp.ok);
    assert_eq!(worker_resp.data["count"], 1);
    assert_eq!(worker_resp.data["messages"][0]["id"], "m-3");

    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn recv_clears_keepalive_unacked_counter() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
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
    let mut keepalive = crate::server::keepalive::Record::default();
    keepalive.unacked = 3;
    keepalive.last_notice_id = Some("stale-notice".into());
    keepalive.suspected_offline = true;
    server.commit(&[Event::KeepaliveUpdated {
        worker_id: "peer".into(),
        record: keepalive,
    }]);
    let server = Arc::new(server);
    let response = handle_poll_async(server.clone(), "peer".into(), 100).await;
    assert!(response.ok);
    let record = server.state.lock().unwrap().keepalives["peer"].clone();
    assert_eq!(record.unacked, 0);
    assert_eq!(record.last_notice_id, None);
    assert!(!record.suspected_offline);
    std::fs::remove_dir_all(root).ok();
}
