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
fn presence_edge_skips_a_tmux_only_peer() {
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
    assert!(promote_resp.ok, "{promote_resp:?}");

    let status = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("edge-worker".into()),
        },
    );
    assert!(status.ok, "{status:?}");
    // A tmux-only binding registers no AppServer session or thread, so the pane
    // probe can only prove reachability. The peer stays Unknown, and an Unknown
    // peer records no presence edge at all: it is neither announced online nor
    // declared offline.
    assert_eq!(status.data["workers"][0]["presence"], "unknown");
    assert!(
        !server_arc
            .state
            .lock()
            .unwrap()
            .keepalives
            .contains_key("edge-worker"),
        "an Unknown peer must not record a presence edge"
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

/// `collab context` is the single agent bootstrap read, so the projections that
/// `collab who` (`count`) and `collab status --all` (`summary`, `master_wake`,
/// `subagents`) expose must be part of the same snapshot. Without them the agent
/// has to issue three more calls to answer "who is here, and what is the
/// scheduling state".
#[test]
fn context_snapshot_carries_peer_status_and_scheduling_state() {
    let (server, root) = test_server();
    for (id, pane) in [("peer-a", "%1"), ("peer-b", "%2")] {
        let response = register(&server, id, pane);
        assert!(response.ok, "registering {id} failed: {:?}", response.error);
    }

    let response = handle_context(&server, "peer-a".into(), "token-peer-a".into());
    assert!(response.ok, "context failed: {:?}", response.error);
    let data = &response.data;

    assert_eq!(data["registered"], true);
    assert_eq!(data["peer_count"], 2);
    assert_eq!(data["peers"].as_array().unwrap().len(), 2);
    assert_eq!(data["summary"]["workers"], 2);
    assert_eq!(data["summary"]["tasks"], 0);
    assert_eq!(data["summary"]["subagents"], 0);
    assert!(data["summary"]["now"].is_string(), "{data}");
    assert!(
        data["master_wake"].is_object(),
        "the scheduling projection must travel with the context snapshot: {data}"
    );
    assert!(data["subagents"].is_array(), "{data}");

    std::fs::remove_dir_all(root).ok();
}

/// `collab context` is the single bootstrap read for an agent, so it is also
/// where a peer reads back its own runtime binding. A peer that loses the
/// register receipt cannot address its route again otherwise: the generation
/// then appears only inside a rejection message, and recovering from there
/// would mean inferring a control value from an error.
#[test]
fn context_returns_the_callers_own_runtime_binding_receipt() {
    let (server, root) = test_server();
    let registered = register(&server, "receipt-worker", "thread-receipt");
    assert!(registered.ok, "registering the peer failed: {registered:?}");
    let receipt = registered.data["command"]["binding"].clone();
    assert!(
        receipt.is_object(),
        "the register response must carry the typed binding: {registered:?}"
    );

    let response = handle_context(
        &server,
        "receipt-worker".into(),
        "token-receipt-worker".into(),
    );
    assert!(response.ok, "context failed: {response:?}");
    assert_eq!(
        response.data["binding"], receipt,
        "context must return the caller's own binding so a peer that lost the receipt can address the route again"
    );

    std::fs::remove_dir_all(root).ok();
}

/// The read-back is scoped to the caller. A peer that received another peer's
/// binding could address a route it does not own, so the projection must never
/// fall back to "some binding in this project".
#[test]
fn context_never_returns_another_peers_binding() {
    let (server, root) = test_server();
    let first = register(&server, "receipt-first", "thread-first");
    let second = register(&server, "receipt-second", "thread-second");
    assert!(first.ok, "{first:?}");
    assert!(second.ok, "{second:?}");
    let first_receipt = first.data["command"]["binding"].clone();
    let second_receipt = second.data["command"]["binding"].clone();
    assert_ne!(first_receipt, second_receipt, "{first:?} {second:?}");

    let response = handle_context(&server, "receipt-first".into(), "token-receipt-first".into());
    assert!(response.ok, "{response:?}");
    assert_eq!(
        response.data["binding"], first_receipt,
        "context must return the caller's binding, never another peer's"
    );

    std::fs::remove_dir_all(root).ok();
}

/// One root can hold several app scopes. The resolved route scope then names no
/// single scope, and a read-back that first resolved it returned null even
/// though the caller's own binding was unambiguous. The caller's receipt must
/// not depend on that resolution.
#[test]
fn context_returns_the_callers_binding_when_the_root_holds_several_app_scopes() {
    let (server, root) = test_server();
    let registered = register(&server, "receipt-multi", "thread-multi");
    assert!(registered.ok, "registering the peer failed: {registered:?}");
    let receipt = registered.data["command"]["binding"].clone();
    assert!(receipt.is_object(), "{registered:?}");

    // A second app scope on the same root makes the resolved route scope
    // ambiguous. Nothing about the caller's own binding becomes ambiguous.
    let project_scope = crate::server::GlobalState::canonical_project_scope(&server.root).unwrap();
    let other = crate::server::global_state::RuntimeBinding::new_with_session(
        project_scope.clone(),
        AppServerId::new("appserver-cli").unwrap(),
        crate::identity::AgentId::new("agent-other-scope").unwrap(),
        RuntimeId::new("runtime-other-scope").unwrap(),
        BindingId::new("binding-other-scope").unwrap(),
        1,
        Some(SessionId::new("session-other-scope").unwrap()),
        Some(crate::identity::NativeThreadId::new("thread-other-scope").unwrap()),
    )
    .unwrap();
    server
        .commit_checked(&[
            Event::GlobalProjectRegistered {
                registration: crate::server::global_state::ProjectRegistration::new(
                    project_scope,
                    AppServerId::new("appserver-cli").unwrap(),
                )
                .unwrap(),
            },
            Event::GlobalRuntimeBound { binding: other },
        ])
        .expect("adding a second app scope for this root must commit");
    {
        let state = server.state.lock().unwrap();
        assert!(
            server_route_scope(&server, &state).is_err(),
            "the fixture must make the resolved route scope ambiguous"
        );
    }

    let response = handle_context(&server, "receipt-multi".into(), "token-receipt-multi".into());
    assert!(response.ok, "context failed: {response:?}");
    assert_eq!(
        response.data["binding"], receipt,
        "an ambiguous route scope must not hide the caller's own binding: {response:?}"
    );

    std::fs::remove_dir_all(root).ok();
}

/// `collab context` replaces `collab who` / `collab status --all` for agents,
/// and `Workers` / `StatusAll` were the calls that recorded ordinary-peer
/// presence edges. Driving the same transition through the consolidated entry
/// must still emit the recovery notification, or the wake loop silently
/// degrades as soon as agents stop calling the demoted commands.
#[test]
fn context_records_peer_presence_transitions_like_status_all() {
    let (server, root) = test_server();
    register(&server, "master-worker", "thread-master");
    register(&server, "cold-worker", "thread-cold");
    kill_registered_worker_pane(&server, "cold-worker");
    let server_arc = std::sync::Arc::new(server);
    let promoted = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            approval: "approved".into(),
        },
    );
    assert!(promoted.ok, "{promoted:?}");

    // The first read only establishes the offline baseline; it must not notify.
    let baseline = handle_context(&server_arc, "master-worker".into(), "token-master-worker".into());
    assert!(baseline.ok, "{baseline:?}");
    {
        let state = server_arc.state.lock().unwrap();
        assert_eq!(state.keepalives["cold-worker"].notified_presence, "offline");
        assert!(state.msgs.values().all(|m| {
            !(m.to == "master-worker"
                && m.subject == Some("worker-unresponsive: cold-worker".into()))
        }));
    }

    assert!(register(&server_arc, "cold-worker", "thread-cold").ok);
    let recovered =
        handle_context(&server_arc, "master-worker".into(), "token-master-worker".into());
    assert!(recovered.ok, "{recovered:?}");
    let repeated = handle_context(&server_arc, "master-worker".into(), "token-master-worker".into());
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
        "the offline->online edge must still notify exactly once through collab context"
    );
    assert_eq!(state.keepalives["cold-worker"].notified_presence, "online");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

fn approve_promotion(server: &Arc<Server>, worker_id: &str) -> Resp {
    dispatch(
        server,
        Req::MasterPromote {
            worker_id: worker_id.into(),
            token: format!("token-{worker_id}"),
            approval: "user approval".into(),
        },
    )
}

/// A pane has one owner, so the later registrant on the recorded master's pane
/// closes the master and retires its binding. The owner's explicit approval
/// must still be able to name the new authority, otherwise the project is left
/// with no authority at all.
#[test]
fn user_approved_promotion_replaces_a_master_after_its_pane_is_taken() {
    let (server, root) = test_server();
    let tmux = IsolatedTmux::start_single(&root);
    let server = Arc::new(server);

    let mut endpoint = tmux.endpoints().remove(0);
    endpoint.codex_session_id = Some("session-old-master".into());
    endpoint.codex_thread_id = Some("thread-old-master".into());
    assert!(register_tmux(&server, "old-master", endpoint.clone()).ok);
    let master_binding = registered_binding(&server, "old-master");
    server
        .commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: master_binding,
        }])
        .unwrap();
    assert!(approve_promotion(&server, "old-master").ok);
    assert_eq!(
        dispatch(&server, Req::MasterStatus).data["master"]["worker_id"],
        "old-master"
    );

    // A later Codex thread takes the same pane. The pane shell keeps its pid,
    // so the recorded master still probes present.
    let mut taker = endpoint.clone();
    taker.codex_session_id = Some("session-taker".into());
    taker.codex_thread_id = Some("thread-taker".into());
    assert!(register_tmux(&server, "taker", taker).ok);
    let taker_binding = registered_binding(&server, "taker");
    server
        .commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: taker_binding,
        }])
        .unwrap();

    // The later registrant owns the pane, so the recorded master is closed and
    // its authority is gone. The pane never makes a retired claimant live.
    assert_ne!(
        dispatch(&server, Req::MasterStatus).data["master"]["worker_id"],
        "old-master"
    );
    assert!(!server
        .state
        .lock()
        .unwrap()
        .workers
        .contains_key("old-master"));

    let promoted = approve_promotion(&server, "taker");
    assert!(promoted.ok, "{promoted:?}");
    assert_eq!(
        dispatch(&server, Req::MasterStatus).data["master"]["worker_id"],
        "taker"
    );
    drop(tmux);
    std::fs::remove_dir_all(root).ok();
}

/// An explicit user approval strips the recorded grant. A different pane is
/// still only an address, so the incumbent cannot veto the replacement.
#[test]
fn user_approved_promotion_replaces_a_master_that_owns_its_anchor() {
    let (server, root) = test_server();
    let tmux = IsolatedTmux::start_single(&root);
    let server = Arc::new(server);

    let mut endpoint = tmux.endpoints().remove(0);
    endpoint.codex_session_id = Some("session-master".into());
    endpoint.codex_thread_id = Some("thread-master".into());
    assert!(register_tmux(&server, "master", endpoint.clone()).ok);
    let master_binding = registered_binding(&server, "master");
    server
        .commit_checked(&[Event::GlobalCurrentThreadRouteSet {
            binding: master_binding,
        }])
        .unwrap();
    assert!(approve_promotion(&server, "master").ok);

    let mut other = tmux.add_session();
    other.codex_session_id = Some("session-other".into());
    other.codex_thread_id = Some("thread-other".into());
    assert!(register_tmux(&server, "other", other).ok);

    let promoted = approve_promotion(&server, "other");
    assert!(promoted.ok, "{promoted:?}");
    assert_eq!(
        dispatch(&server, Req::MasterStatus).data["master"]["worker_id"],
        "other"
    );
    drop(tmux);
    std::fs::remove_dir_all(root).ok();
}
