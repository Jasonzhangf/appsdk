#[test]
fn managed_subworker_cannot_create_a_patrol_deadline() {
    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    let now = now_ms();
    let record = bind_test_subagent_record(&server, crate::subagent::Record {
        id: "patrol-child".into(), parent: "parent".into(), peer: "child".into(),
        status: "idle".into(), thread_id: Some("thread-child".into()),
        profile: None, created_ms: now, ready_deadline_ms: now + 90_000,
        last_message: None, error: None, probe_failures: Vec::new(), runtime: None,
        create_operation_id: None, binding_id: None,
    });
    server.commit(&[Event::SubagentUpdated { subagent: record }]);
    let response = handle_notification_subscribe(
        &server, "child".into(), "token-child".into(), "deadline".into(),
        Some("patrol:child".into()), None, Vec::new(), Some(100), 2, 30,
    );
    assert!(!response.ok);
    assert!(response.error.unwrap().contains("managed subworkers"));
    assert!(!server.state.lock().unwrap().notification_subscriptions.values()
        .any(|subscription| subscription.event == "deadline"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_direct_dispatch_accepts_master_and_continues_after_blocked() {
    use crate::subagent::{Action, Record};
    let (server, root) = test_server();
    for id in ["parent", "child", "master", "other"] {
        register(&server, id, &format!("%{id}"));
    }
    promote_master(&server, "master", "user approval");
    let now = now_ms();
    let record = bind_test_subagent_record(&server, Record {
        id: "managed".into(), parent: "parent".into(), peer: "child".into(),
        status: "idle".into(), thread_id: Some("thread-child".into()),
        profile: None, created_ms: now, ready_deadline_ms: now + 90_000,
        last_message: None, error: None, probe_failures: Vec::new(), runtime: None,
        create_operation_id: None, binding_id: None,
    });
    server.commit(&[Event::SubagentUpdated { subagent: record }]);
    let send = |actor: &str, subject: &str| crate::subagent::handle(
        &server, actor, &format!("token-{actor}"), Action::Send {
            id: "managed".into(), subject: subject.into(), body: subject.into(),
        });
    assert!(!send("other", "unauthorized").ok);
    let first = send("master", "first");
    assert!(first.ok, "{:?}", first.error);
    let working = crate::subagent::handle(&server, "child", "token-child", Action::Working { id: "managed".into() });
    assert!(working.ok, "{:?}", working.error);
    assert!(!send("parent", "busy").ok);
    let mut task = server.state.lock().unwrap().tasks.values().find(|task| task.owner == "child").unwrap().clone();
    task.status = "blocked".into();
    task.worktree_path = Some(root.join("held-worktree").display().to_string());
    server.commit(&[Event::TaskUpdated { task: task.clone() }]);
    let next = send("parent", "next");
    assert!(next.ok, "{:?}", next.error);
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks[&task.id].owner, task.owner);
    assert_eq!(state.tasks[&task.id].status, "blocked");
    assert_eq!(state.tasks[&task.id].worktree_path, task.worktree_path);
    assert_eq!(state.tasks.values().filter(|task| task.owner == "child" && task.status == "assigned").count(), 1);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_name_uses_current_map_when_read_projection_is_peer() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approval");
    let worker = server.state.lock().unwrap().workers["master"].clone();
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap();
    let summary = worker_status_summary_with_maps(&server, &control, &Default::default(), &Default::default(), &Default::default(), json!({"role":"peer"}), &worker).unwrap();
    assert_eq!(summary["display_name"], "Jenny");
    assert_eq!(summary["role"], "peer");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_waiting_user_projects_to_worker_status_without_becoming_idle() {
    use crate::server::name_state::{AgentActivity, AgentActivityRecord, AgentPresence, AgentPresenceRecord};
    let (mut server, root) = test_server();
    register_appserver_worker(&server, &root, "waiting-peer", "waiting-thread");
    let endpoint_missing = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed_missing = endpoint_missing.clone();
    server.appserver_thread_status = Arc::new(move |_, _| {
        if observed_missing.load(Ordering::SeqCst) {
            Err("APP_SERVER_THREAD_MISSING: controlled missing endpoint".into())
        } else {
            Ok(json!({"thread":{"status":{"type":"idle"}}}))
        }
    });
    let server = Arc::new(server);
    let worker = server.state.lock().unwrap().workers["waiting-peer"].clone();
    let scope = GlobalState::canonical_project_scope(&root).unwrap();
    let session = worker.transport.as_ref().unwrap().session_id.clone().unwrap();
    for (presence, activity, expected) in [
        (AgentPresence::Online, AgentActivity::WaitingUser, "waiting_user"),
        (AgentPresence::Online, AgentActivity::Error, "error"),
        (AgentPresence::Online, AgentActivity::Unknown, "unknown"),
        (AgentPresence::Offline, AgentActivity::WaitingUser, "absent"),
    ] {
        endpoint_missing.store(presence == AgentPresence::Offline, Ordering::SeqCst);
        server.commit(&[
            Event::AgentPresenceObserved { presence: AgentPresenceRecord {
                project_scope: scope.as_str().into(), worker_id: worker.id.clone(),
                session_id: session.clone(), presence, reason: "controlled current-session observation".into(), observed_ms: now_ms(),
            } },
            Event::AgentActivityObserved { activity: AgentActivityRecord {
                project_scope: scope.as_str().into(), worker_id: worker.id.clone(),
                session_id: session.clone(), activity, background: false,
                reason: "controlled approval prompt".into(), observed_ms: now_ms(),
                idle_since_ms: None,
            } },
        ]);
        let response = dispatch(&server, Req::WorkerStatus { worker_id: Some(worker.id.clone()) });
        assert!(response.ok, "{response:?}");
        let summary = &response.data["workers"][0];
        assert_eq!(summary["agent_state"], expected);
        assert_ne!(summary["agent_state"], "idle");
        assert_eq!(summary["status"], if expected == "absent" { "lost" } else { expected });
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_subagent_send_binds_the_selected_child_when_multiple_children_are_assigned() {
    use crate::subagent::{Action, Record};
    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child-a", "%child-a");
    register(&server, "child-b", "%child-b");
    let now = now_ms();
    server.commit(&[
        Event::SubagentUpdated {
            subagent: bind_test_subagent_record(&server, Record {
                id: "managed-a".into(),
                parent: "parent".into(),
                peer: "child-a".into(),
                status: "idle".into(),
                thread_id: Some("thread-child-a".into()),
                profile: None,
                created_ms: now,
                ready_deadline_ms: now + 90_000,
                last_message: None,
                error: None,
                probe_failures: Vec::new(),
                runtime: None,
                create_operation_id: None,
                binding_id: None,
            }),
        },
        Event::SubagentUpdated {
            subagent: bind_test_subagent_record(&server, Record {
                id: "managed-b".into(),
                parent: "parent".into(),
                peer: "child-b".into(),
                status: "idle".into(),
                thread_id: Some("thread-child-b".into()),
                profile: None,
                created_ms: now,
                ready_deadline_ms: now + 90_000,
                last_message: None,
                error: None,
                probe_failures: Vec::new(),
                runtime: None,
                create_operation_id: None,
                binding_id: None,
            }),
        },
    ]);

    let wrong_binding = handle_send_with_task(
        &server,
        "parent".into(),
        "child-a".into(),
        "notify".into(),
        Some("wrong-binding".into()),
        "must fail".into(),
        None,
        "immediate".into(),
        true,
        Some("managed-b"),
    );
    assert!(!wrong_binding.ok);
    assert_eq!(
        wrong_binding.error.as_deref(),
        Some("managed subagent owner mismatch")
    );
    let unknown_binding = handle_send_with_task(
        &server,
        "parent".into(),
        "child-a".into(),
        "notify".into(),
        Some("unknown-binding".into()),
        "must fail".into(),
        None,
        "immediate".into(),
        true,
        Some("missing"),
    );
    assert!(!unknown_binding.ok);
    assert_eq!(
        unknown_binding.error.as_deref(),
        Some("unknown managed subagent missing")
    );

    let first = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "managed-a".into(),
            subject: "same-task".into(),
            body: "same body".into(),
        },
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    let first_message = server.state.lock().unwrap().subagents["managed-a"]
        .last_message
        .clone()
        .unwrap();
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.tasks[&format!("task-{first_message}")].owner, "child-a");
        assert!(state.subagents["managed-b"].last_message.is_none());
        assert!(!state.tasks.values().any(|task| task.owner == "child-b"));
    }
    let second = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "managed-b".into(),
            subject: "same-task".into(),
            body: "same body".into(),
        },
    );
    assert!(second.ok, "{}", second.error.unwrap_or_default());
    let second_message = server.state.lock().unwrap().subagents["managed-b"]
        .last_message
        .clone()
        .unwrap();


    let state = server.state.lock().unwrap();
    let first_message = state.subagents["managed-a"].last_message.clone().unwrap();
    let second_message = state.subagents["managed-b"].last_message.clone().unwrap();
    assert_ne!(first_message, second_message);
    assert_eq!(state.tasks.len(), 2);
    assert_eq!(
        state.tasks[&format!("task-{first_message}")].owner,
        "child-a"
    );
    assert_eq!(
        state.tasks[&format!("task-{second_message}")].owner,
        "child-b"
    );
    drop(state);

    assert!(
        crate::subagent::handle(
            &server,
            "child-a",
            "token-child-a",
            Action::Working {
                id: "managed-a".into(),
            },
        )
        .ok
    );
    assert!(
        crate::subagent::handle(
            &server,
            "child-b",
            "token-child-b",
            Action::Working {
                id: "managed-b".into(),
            },
        )
        .ok
    );
    let replayed = current_state::load(&server.state_path, false).unwrap();
    assert_eq!(replayed.subagents["managed-a"].status, "working");
    assert_eq!(replayed.subagents["managed-b"].status, "working");
    assert_eq!(
        replayed.tasks[&format!("task-{first_message}")].status,
        "working"
    );
    assert_eq!(
        replayed.tasks[&format!("task-{second_message}")].status,
        "working"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_subagent_send_reclaims_working_child_without_an_owned_task() {
    use crate::subagent::{Action, Record};
    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    let now = now_ms();
    server.commit(&[Event::SubagentUpdated {
        subagent: bind_test_subagent_record(&server, Record {
            id: "managed".into(),
            parent: "parent".into(),
            peer: "child".into(),
            // A keepalive runtime observation can leave this stale after the
            // child has reported ready and consumed an empty recv cycle.
            status: "working".into(),
            thread_id: Some("thread-child".into()),
            profile: None,
            created_ms: now,
            ready_deadline_ms: now + 90_000,
            last_message: None,
            error: None,
            probe_failures: Vec::new(),
            runtime: None,
            create_operation_id: None,
            binding_id: None,
        }),
    }]);

    let assigned = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "managed".into(),
            subject: "next-task".into(),
            body: "dispatch after ready and recv".into(),
        },
    );
    assert!(assigned.ok, "{}", assigned.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    assert_eq!(state.subagents["managed"].status, "assigned");
    assert_eq!(state.tasks.len(), 1);
    assert_eq!(state.tasks.values().next().unwrap().owner, "child");
    drop(state);
    assert!(
        crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Working {
                id: "managed".into(),
            },
        )
        .ok
    );
    assert!(
        crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Ready {
                id: "managed".into(),
            },
        )
        .ok
    );
    let before = {
        let state = server.state.lock().unwrap();
        (
            state.tasks.len(),
            state.subagents["managed"].last_message.clone(),
            state.subagents["managed"].status.clone(),
        )
    };
    let idle_with_active_task = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "managed".into(),
            subject: "must-wait-idle".into(),
            body: "idle status still has an active task".into(),
        },
    );
    assert!(!idle_with_active_task.ok);
    assert_eq!(
        idle_with_active_task.error.as_deref(),
        Some("managed subagent already has an active task")
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks.len(), before.0);
    assert_eq!(state.subagents["managed"].last_message, before.1);
    assert_eq!(state.subagents["managed"].status, before.2);
    drop(state);
    let direct_idle_with_active_task = handle_send_with_task(
        &server,
        "parent".into(),
        "child".into(),
        "notify".into(),
        Some("direct-must-wait".into()),
        "direct active task still owns the child".into(),
        None,
        "immediate".into(),
        true,
        Some("managed"),
    );
    assert!(!direct_idle_with_active_task.ok);
    assert_eq!(
        direct_idle_with_active_task.error.as_deref(),
        Some("managed subagent already has an active task")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_subagent_working_requires_existing_owned_assigned_task() {
    use crate::subagent::{Action, Record};

    {
        let (server, root) = test_server();
        register(&server, "parent", "%parent");
        register(&server, "child", "%child");
        let now = now_ms();
        server.commit(&[Event::SubagentUpdated {
            subagent: Record {
                id: "missing-task".into(),
                parent: "parent".into(),
                peer: "child".into(),
                status: "assigned".into(),
                thread_id: Some("thread-child".into()),
                profile: None,
                created_ms: now,
                ready_deadline_ms: now + 90_000,
                last_message: Some("missing".into()),
                error: None,
                probe_failures: Vec::new(),
                runtime: None,
                create_operation_id: None,
                binding_id: None,
            },
        }]);
        let result = crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Working {
                id: "missing-task".into(),
            },
        );
        assert!(!result.ok);
        assert_eq!(
            result.error.as_deref(),
            Some("assigned task task-missing not found")
        );
        assert_eq!(
            server.state.lock().unwrap().subagents["missing-task"].status,
            "assigned"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    {
        let (server, root) = test_server();
        register(&server, "parent", "%parent");
        register(&server, "child", "%child");
        let now = now_ms();
        server.commit(&[
            Event::SubagentUpdated {
                subagent: Record {
                    id: "terminal-task".into(),
                    parent: "parent".into(),
                    peer: "child".into(),
                    status: "assigned".into(),
                    thread_id: Some("thread-child".into()),
                    profile: None,
                    created_ms: now,
                    ready_deadline_ms: now + 90_000,
                    last_message: Some("terminal".into()),
                    error: None,
                    probe_failures: Vec::new(),
                    runtime: None,
                    create_operation_id: None,
                    binding_id: None,
                },
            },
            Event::TaskCreated {
                task: TaskRec {
                    id: "task-terminal".into(),
                    owner: "child".into(),
                    created_by: "parent".into(),
                    feature_id: None,
                    worktree_path: None,
                    branch: None,
                    base_commit: None,
                    priority: "p2".into(),
                    status: "closed".into(),
                    next_step: None,
                    wait: None,
                    isolation_mode: None,
                    created_ms: now,
                    updated_ms: now,
                },
            },
        ]);
        let result = crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Working {
                id: "terminal-task".into(),
            },
        );
        assert!(!result.ok);
        assert_eq!(
            result.error.as_deref(),
            Some("assigned task task-terminal is not in assigned state (status=closed)")
        );
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["terminal-task"].status, "assigned");
        assert_eq!(state.tasks["task-terminal"].status, "closed");
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    {
        let (server, root) = test_server();
        register(&server, "parent", "%parent");
        register(&server, "child", "%child");
        let now = now_ms();
        server.commit(&[
            Event::SubagentUpdated {
                subagent: Record {
                    id: "owner-task".into(),
                    parent: "parent".into(),
                    peer: "child".into(),
                    status: "assigned".into(),
                    thread_id: Some("thread-child".into()),
                    profile: None,
                    created_ms: now,
                    ready_deadline_ms: now + 90_000,
                    last_message: Some("owner".into()),
                    error: None,
                    probe_failures: Vec::new(),
                    runtime: None,
                    create_operation_id: None,
                    binding_id: None,
                },
            },
            Event::TaskCreated {
                task: TaskRec {
                    id: "task-owner".into(),
                    owner: "other".into(),
                    created_by: "parent".into(),
                    feature_id: None,
                    worktree_path: None,
                    branch: None,
                    base_commit: None,
                    priority: "p2".into(),
                    status: "assigned".into(),
                    next_step: None,
                    wait: None,
                    isolation_mode: None,
                    created_ms: now,
                    updated_ms: now,
                },
            },
        ]);
        let result = crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Working {
                id: "owner-task".into(),
            },
        );
        assert!(!result.ok);
        assert_eq!(result.error.as_deref(), Some("task owner mismatch"));
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["owner-task"].status, "assigned");
        assert_eq!(state.tasks["task-owner"].owner, "other");
        assert_eq!(state.tasks["task-owner"].status, "assigned");
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn managed_subagent_working_accepts_assignment_after_probe_race_and_is_idempotent() {
    use crate::subagent::{Action, Record};

    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    let now = now_ms();
    server.commit(&[Event::SubagentUpdated {
        subagent: bind_test_subagent_record(&server, Record {
            id: "managed".into(),
            parent: "parent".into(),
            peer: "child".into(),
            status: "idle".into(),
            thread_id: Some("thread-child".into()),
            profile: None,
            created_ms: now,
            ready_deadline_ms: now + 90_000,
            last_message: None,
            error: None,
            probe_failures: Vec::new(),
            runtime: None,
            create_operation_id: None,
            binding_id: None,
        }),
    }]);

    let sent = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "managed".into(),
            subject: "race-task".into(),
            body: "claim the task after the probe observes working".into(),
        },
    );
    assert!(sent.ok, "{}", sent.error.unwrap_or_default());
    let message_id = server.state.lock().unwrap().subagents["managed"]
        .last_message
        .clone()
        .unwrap();
    let task_id = format!("task-{message_id}");
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["managed"].status, "assigned");
        assert_eq!(state.tasks[&task_id].status, "assigned");
        assert_eq!(state.tasks.len(), 1);
    }

    // Model the keepalive runtime probe winning the race: it observes the child
    // as working and persists that managed status while the task is assigned.
    let mut probed = server.state.lock().unwrap().subagents["managed"].clone();
    probed.status = "working".into();
    server.commit(&[Event::SubagentUpdated { subagent: probed }]);

    let first = crate::subagent::handle(
        &server,
        "child",
        "token-child",
        Action::Working {
            id: "managed".into(),
        },
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["managed"].status, "working");
        assert_eq!(state.tasks[&task_id].status, "working");
        assert_eq!(state.tasks[&task_id].owner, "child");
        assert_eq!(state.tasks.len(), 1);
    }

    // Once both durable records are working, a repeated claim is a harmless
    // replay of the same transition and must not append or create anything.
    let before = {
        let state = server.state.lock().unwrap();
        (serde_json::to_value(&state.tasks).unwrap(), serde_json::to_value(&state.subagents).unwrap(),
            serde_json::to_value(&state.msgs).unwrap())
    };
    let second = crate::subagent::handle(
        &server,
        "child",
        "token-child",
        Action::Working {
            id: "managed".into(),
        },
    );
    assert!(second.ok, "{}", second.error.unwrap_or_default());
    {
        let state = server.state.lock().unwrap();
        assert_eq!((serde_json::to_value(&state.tasks).unwrap(), serde_json::to_value(&state.subagents).unwrap(),
            serde_json::to_value(&state.msgs).unwrap()), before);
    }
    assert_eq!(server.state.lock().unwrap().tasks.len(), 1);

    let replayed = current_state::load(&server.state_path, false).unwrap();
    assert_eq!(replayed.subagents["managed"].status, "working");
    assert_eq!(replayed.tasks[&task_id].status, "working");
    assert_eq!(replayed.tasks[&task_id].owner, "child");
    assert_eq!(replayed.tasks.len(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_close_allows_definitively_missing_worker_without_snapshot() {
    let (server, root) = test_server();
    register(&server, "master", "thread-master");
    register(&server, "missing-peer", "thread-missing-peer");
    promote_master(&server, "master", "user approved master");

    kill_registered_worker_pane(&server, "missing-peer");
    let target = server.state.lock().unwrap().workers["missing-peer"].clone();
    assert_eq!(
        worker_identity_and_endpoint(&server, &target),
        (true, Some(false)),
        "the close path may only bypass snapshot evidence for a definitively Missing address"
    );

    let closed = super::handle_worker_close(
        &server,
        "master".into(),
        "token-master".into(),
        "missing-peer".into(),
        "tmux pane confirmed missing; no live thread to snapshot".into(),
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert!(closed.data["snapshot_captured_ms"].is_null());

    let replayed = current_state::load(&server.state_path, false).unwrap();
    assert!(!replayed.workers.contains_key("missing-peer"));
    assert_eq!(
        replayed.worker_closures["missing-peer"].reason,
        "tmux pane confirmed missing; no live thread to snapshot"
    );
    assert!(replayed.worker_closures["missing-peer"]
        .snapshot_captured_ms
        .is_none());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_close_is_master_only_audited_and_refuses_to_strand_tasks() {
    let (server, root) = test_server();
    register(&server, "peer-a", "%a");
    register(&server, "peer-b", "%b");
    promote_master(&server, "peer-a", "user approved peer-a as collab master");

    // A non-master peer cannot retire another peer.
    let outsider = super::handle_worker_close(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "peer-a".into(),
        "trying to close the master".into(),
    );
    assert!(!outsider.ok);

    // The audit reason is mandatory.
    let no_reason = super::handle_worker_close(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-b".into(),
        "   ".into(),
    );
    assert!(!no_reason.ok);
    assert!(no_reason.error.unwrap().contains("--reason"));

    // Master may not close itself into a headless project.
    let self_close = super::handle_worker_close(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-a".into(),
        "self".into(),
    );
    assert!(!self_close.ok);
    assert!(self_close.error.unwrap().contains("cannot close itself"));

    let missing_snapshot = super::handle_worker_close(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-b".into(),
        "transport looks dead".into(),
    );
    assert!(!missing_snapshot.ok);
    assert!(missing_snapshot
        .error
        .unwrap()
        .contains("requires a successful worker snapshot"));
    server.commit(&[Event::SubagentUpdated {
        subagent: crate::subagent::Record {
            thread_id: Some("thread-b".into()),
            ..subagent_record("managed-peer-b", "idle", "peer-b")
        },
    }]);
    let mismatched_thread = super::handle_worker_close(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-b".into(),
        "transport looks dead".into(),
    );
    assert!(!mismatched_thread.ok);
    assert!(mismatched_thread
        .error
        .unwrap()
        .contains("requires a successful worker snapshot"));

    // A worker holding live work keeps its registration; the task lifecycle
    // has to be resolved first or the worktree is stranded.
    server.commit(&[Event::TaskCreated {
        task: crate::server::state::TaskRec {
            id: "task-b".into(),
            owner: "peer-b".into(),
            created_by: "peer-a".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            status: "working".into(),
            next_step: None,
            wait: None,
            isolation_mode: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        },
    }]);
    let owns_work = super::handle_worker_close(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-b".into(),
        "transport looks dead".into(),
    );
    assert!(!owns_work.ok);
    assert!(owns_work.error.unwrap().contains("task-b"));

    server.commit(&[Event::TaskUpdated {
        task: crate::server::state::TaskRec {
            id: "task-b".into(),
            owner: "peer-b".into(),
            created_by: "peer-a".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            status: "closed".into(),
            next_step: None,
            wait: None,
            isolation_mode: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        },
    }]);
    server.commit(&[Event::WorkerSnapshotCaptured {
        worker_id: "peer-b".into(),
        thread_id: "thread-b".into(),
        captured_ms: now_ms(),
    }]);
    let closed = super::handle_worker_close(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-b".into(),
        "transport dead after snapshot".into(),
    );
    assert!(closed.ok, "{}", closed.error.clone().unwrap_or_default());
    assert_eq!(closed.data["closed"], "peer-b");
    assert_eq!(closed.data["reason"], "transport dead after snapshot");
    assert!(closed.data["snapshot_captured_ms"].is_i64());
    assert!(closed.data.get("archived_thread").is_none());

    let repeated = super::handle_worker_close(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-b".into(),
        "a different reason must not rewrite the receipt".into(),
    );
    assert!(
        repeated.ok,
        "{}",
        repeated.error.clone().unwrap_or_default()
    );
    assert_eq!(repeated.data["closed"], "peer-b");
    assert_eq!(repeated.data["closed_by"], "peer-a");
    assert_eq!(repeated.data["reason"], "transport dead after snapshot");
    assert_eq!(repeated.data["reused"], true);

    let state = server.state.lock().unwrap();
    assert!(!state.workers.contains_key("peer-b"));
    assert!(!state.keepalives.contains_key("peer-b"));
    assert_eq!(
        state.worker_closures["peer-b"].snapshot_captured_ms,
        Some(closed.data["snapshot_captured_ms"].as_i64().unwrap())
    );
    drop(state);

    let replayed = current_state::load(&server.state_path, false).unwrap();
    assert_eq!(replayed.worker_snapshots["peer-b"].thread_id, "thread-b");
    assert_eq!(
        replayed.worker_closures["peer-b"].reason,
        "transport dead after snapshot"
    );
    assert!(replayed.worker_closures["peer-b"]
        .snapshot_captured_ms
        .is_some());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_close_refuses_every_unfinished_task_status() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    register(&server, "peer-b", "%peer-b");
    promote_master(&server, "master", "user approved master");
    server.commit(&[Event::WorkerSnapshotCaptured {
        worker_id: "peer-b".into(),
        thread_id: "thread-peer-b".into(),
        captured_ms: now_ms(),
    }]);

    // A task that raised no keepalive nudge still owns a worktree, branch, and
    // delivery obligation, so worker retirement must refuse it.
    for status in ["blocked", "waiting", "delivered", "accepted", "merged"] {
        server.commit(&[Event::TaskCreated {
            task: TaskRec {
                id: format!("task-{status}"),
                owner: "peer-b".into(),
                created_by: "master".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p2".into(),
                status: status.into(),
                next_step: None,
                wait: None,
                isolation_mode: None,
                created_ms: now_ms(),
                updated_ms: now_ms(),
            },
        }]);
        let refused = super::handle_worker_close(
            &server,
            "master".into(),
            "token-master".into(),
            "peer-b".into(),
            format!("retire while {status}"),
        );
        assert!(
            !refused.ok,
            "worker close must refuse an unfinished {status} task: {:?}",
            refused.data
        );
        assert!(
            refused
                .error
                .unwrap_or_default()
                .contains(&format!("task-{status}")),
            "the refusal must name the blocking task for {status}"
        );
        server.commit(&[Event::TaskUpdated {
            task: TaskRec {
                id: format!("task-{status}"),
                owner: "peer-b".into(),
                created_by: "master".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p2".into(),
                status: "closed".into(),
                next_step: None,
                wait: None,
                isolation_mode: None,
                created_ms: now_ms(),
                updated_ms: now_ms(),
            },
        }]);
    }

    let closed = super::handle_worker_close(
        &server,
        "master".into(),
        "token-master".into(),
        "peer-b".into(),
        "no unfinished task remains".into(),
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn last_task_close_keeps_default_lease_armed_and_preserves_pending_unread_payload() {
    let (server, root) = test_server();
    register(&server, "sender", "%sender");
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);

    let sent = handle_send(
        &server,
        "sender".into(),
        "owner".into(),
        "notify".into(),
        Some("unread while busy".into()),
        "payload that must survive the close".into(),
        None,
        "immediate".into(),
    );
    assert!(sent.ok, "{}", sent.error.clone().unwrap_or_default());
    let msg_id = sent.data["msg_id"].as_str().unwrap().to_string();

    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                Some(format!("continue {status}")),
            )
            .ok
        );
    }
    let worktree = candidate_worktree(&root, "task");
    git_ok(&worktree, &["commit", "--allow-empty", "-q", "-m", "candidate"]);
    let candidate = current_head(&worktree);
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate verified".into()),
            Some(worktree.to_string_lossy().into_owned()),
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
            "review pass".into(),
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
            "main verified".into(),
        )
        .ok
    );
    git_ok(&root, &["worktree", "remove", worktree.to_str().unwrap()]);

    let closed = handle_task_close(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert_eq!(
        closed.data["notification"],
        "subscribed resource waiters only"
    );

    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.notification_subscriptions["sub-default-direct-message-owner"].status,
            "armed",
            "the last task close must not cancel the default direct-message lease"
        );
        assert_eq!(
            state.msgs[&msg_id].state, "pending",
            "automatic lease cancellation must not supersede unread mailbox bytes"
        );
        assert!(state.inbox_of("owner").iter().any(|m| m.id == msg_id));
    }

    // Durable replay must keep both facts: lease armed, payload still owed.
    let replayed = current_state::load(&server.state_path, false).unwrap();
    assert_eq!(
        replayed.notification_subscriptions["sub-default-direct-message-owner"].status,
        "armed"
    );
    assert_eq!(replayed.msgs[&msg_id].state, "pending");

    // An explicit recv after the close still returns the unread payload.
    let recv = poll_messages_with_context(
        &server,
        "owner",
        None,
        None,
        Some("recv-last-close-unread-preserved"),
    )
    .expect("the preserved unread payload must still be delivered");
    assert!(recv.ok, "{:?}", recv.error);
    assert_eq!(recv.data["count"], 1);
    assert_eq!(recv.data["messages"][0]["id"], msg_id.as_str());
    assert_eq!(
        recv.data["messages"][0]["body"],
        "payload that must survive the close"
    );

    // After the last close the default lease is still armed, so a fresh send
    // is a real notification attempt, never a silent mailbox-only degradation.
    let after_close = handle_send(
        &server,
        "sender".into(),
        "owner".into(),
        "notify".into(),
        Some("reachable after last close".into()),
        "this must reach a notification sink, not only mailbox".into(),
        None,
        "immediate".into(),
    );
    assert!(after_close.ok, "{}", after_close.error.clone().unwrap_or_default());
    assert_ne!(
        after_close.data["notification"],
        "mailbox-only-no-subscription",
        "armed default direct-message lease must not degrade to mailbox only"
    );

    std::fs::remove_dir_all(root).ok();
}

/// The first read and a `receive_id` replay answer from one minimal message
/// projection. Delivery diagnostics stay in the wake ledger: a recipient that
/// consumed the durable body must not receive per-message retry bookkeeping.
#[test]
fn recv_first_read_and_receive_id_replay_share_minimal_message_projection() {
    let (server, root) = test_server();
    server.commit(&[Event::Sent {
        msg: Message {
            id: "msg-minimal-projection".into(),
            from: "writer".into(),
            to: "reader".into(),
            mtype: "notify".into(),
            subject: Some("minimal projection".into()),
            body: "durable body that must not be trimmed".into(),
            in_reply_to: Some("msg-original-request".into()),
            created_ms: 10,
            state: "pending".into(),
            wake_attempt_count: 3,
            last_wake_attempt_ms: 99,
            retry_attempted: true,
        },
    }]);

    let first = poll_messages_with_context(
        &server,
        "reader",
        None,
        None,
        Some("recv-minimal-projection"),
    )
    .expect("an unread durable message must be delivered");
    assert!(first.ok, "{:?}", first.error);
    let view = &first.data["messages"][0];
    assert_eq!(view["id"], "msg-minimal-projection");
    assert_eq!(view["from"], "writer");
    assert_eq!(view["to"], "reader");
    assert_eq!(view["type"], "notify");
    assert_eq!(view["subject"], "minimal projection");
    assert_eq!(view["body"], "durable body that must not be trimmed");
    assert_eq!(view["in_reply_to"], "msg-original-request");
    for diagnostic in [
        "wake_attempt_count",
        "last_wake_attempt_ms",
        "retry_attempted",
    ] {
        assert!(
            view.get(diagnostic).is_none(),
            "recv must not expose per-message delivery diagnostics: {diagnostic} in {view}"
        );
    }

    let replay = poll_messages_with_context(
        &server,
        "reader",
        None,
        None,
        Some("recv-minimal-projection"),
    )
    .expect("the committed receive batch must replay");
    assert_eq!(replay.data["replayed"], true);
    assert_eq!(
        replay.data["messages"], first.data["messages"],
        "a receive_id replay must return the same minimal projection"
    );

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn worker_snapshot_returns_bounded_tmux_buffer_without_side_effects() {
    let (server, root) = test_server();
    let tmux = IsolatedTmux::start(&root);
    let mut endpoints = tmux.endpoints();
    assert_eq!(endpoints.len(), 2);
    let peer_pane_id = endpoints[1].pane_id.clone();
    let output = Command::new("tmux")
        .args([
            "-S",
            &endpoints[1].socket_path,
            "respawn-pane",
            "-k",
            "-t",
            &peer_pane_id,
            "sh",
            "-c",
            "i=1; while [ $i -le 30 ]; do printf 'SNAP-%03d\\n' \"$i\"; i=$((i+1)); done; sleep 60",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "prepare pane buffer: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let capture = Command::new("tmux")
            .args([
                "-S",
                &endpoints[1].socket_path,
                "capture-pane",
                "-p",
                "-t",
                &peer_pane_id,
                "-S",
                "-4096",
            ])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&capture.stdout).contains("SNAP-030") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "snapshot fixture did not publish its terminal line"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    endpoints = tmux.endpoints();
    let peer_endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint.pane_id == peer_pane_id)
        .cloned()
        .unwrap();
    // The master must carry an explicit AppServer session and thread: a
    // tmux-only binding is Unknown and therefore holds no live master authority.
    let mut master_endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint.pane_id != peer_pane_id)
        .cloned()
        .unwrap();
    master_endpoint.codex_session_id = Some("session-snapshot-master".into());
    master_endpoint.codex_thread_id = Some("thread-snapshot-master".into());
    assert!(register_tmux(&server, "snapshot-master", master_endpoint.clone()).ok);
    assert!(register_tmux(&server, "snapshot-peer", peer_endpoint.clone()).ok);
    register_appserver_worker(&server, &root, "snapshot-appserver", "thread-snapshot-appserver");
    promote_master(&server, "snapshot-master", "approved for worker snapshot test");

    let revision_before = server.state.lock().unwrap().revision;
    let state_before = std::fs::read(&server.state_path).unwrap();
    let response = super::handle_worker_snapshot(
        &server,
        "snapshot-master".into(),
        "token-snapshot-master".into(),
        "snapshot-peer".into(),
        5,
    );
    assert!(response.ok, "{}", response.error.unwrap_or_default());
    assert_eq!(response.data["transport"], "tmux");
    assert_eq!(response.data["pane_id"], peer_pane_id);
    assert_eq!(response.data["lines"], 5);
    assert_eq!(response.data["durable"], false);
    let output = response.data["output"].as_str().unwrap();
    let lines = output.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 5, "{output:?}");
    assert_eq!(lines.last().copied(), Some("SNAP-030"));
    assert!(!output.contains("SNAP-025"));

    let one = super::handle_worker_snapshot(
        &server,
        "snapshot-master".into(),
        "token-snapshot-master".into(),
        "snapshot-peer".into(),
        1,
    );
    assert!(one.ok, "{}", one.error.unwrap_or_default());
    assert_eq!(one.data["output"], "SNAP-030");

    let all = super::handle_worker_snapshot(
        &server,
        "snapshot-master".into(),
        "token-snapshot-master".into(),
        "snapshot-peer".into(),
        200,
    );
    assert!(all.ok, "{}", all.error.unwrap_or_default());
    assert_eq!(all.data["output"].as_str().unwrap().lines().count(), 30);

    for invalid in [0, 201] {
        let response = super::handle_worker_snapshot(
            &server,
            "snapshot-master".into(),
            "token-snapshot-master".into(),
            "snapshot-peer".into(),
            invalid,
        );
        assert!(!response.ok);
        assert!(response
            .error
            .unwrap()
            .contains("snapshot lines must be 1..200"));
    }

    for (target, token, expected) in [
        ("snapshot-peer", "wrong-token", "token mismatch"),
        ("missing-peer", "token-snapshot-master", "not registered"),
    ] {
        let response = super::handle_worker_snapshot(
            &server,
            "snapshot-master".into(),
            token.into(),
            target.into(),
            40,
        );
        assert!(!response.ok);
        assert!(
            response.error.as_deref().unwrap_or_default().contains(expected),
            "{target}: {:?}",
            response.error
        );
    }
    assert_eq!(server.state.lock().unwrap().revision, revision_before);
    assert_eq!(std::fs::read(&server.state_path).unwrap(), state_before);
    let resolved = super::handle_worker_snapshot(
        &server,
        "snapshot-master".into(),
        "token-snapshot-master".into(),
        "snapshot-appserver".into(),
        40,
    );
    assert!(resolved.ok, "{resolved:?}");
    assert_eq!(resolved.data["snapshot"], "not_collected");
    assert_eq!(resolved.data["snapshot_collected"], false);
    assert_eq!(resolved.data["judgment_status"], "satisfied");
    server
        .state
        .lock()
        .unwrap()
        .workers
        .get_mut("snapshot-appserver")
        .unwrap()
        .transport
        .as_mut()
        .unwrap()
        .tmux_endpoint = Some(master_endpoint);
    let incidental_tmux = super::handle_worker_snapshot(
        &server,
        "snapshot-master".into(),
        "token-snapshot-master".into(),
        "snapshot-appserver".into(),
        40,
    );
    assert!(!incidental_tmux.ok, "{incidental_tmux:?}");
    assert_eq!(incidental_tmux.error.as_deref(), Some("WORKER_STATUS_CAPTURE_BINDING_CHANGED"));
    server
        .state
        .lock()
        .unwrap()
        .workers
        .get_mut("snapshot-appserver")
        .unwrap()
        .transport = None;
    let no_transport = super::handle_worker_snapshot(
        &server,
        "snapshot-master".into(),
        "token-snapshot-master".into(),
        "snapshot-appserver".into(),
        40,
    );
    assert!(!no_transport.ok);
    assert!(no_transport.error.unwrap().contains("no registered transport"));

    let state = server.state.lock().unwrap();
    assert!(!state.worker_snapshots.contains_key("snapshot-peer"));
    drop(state);

    let killed = Command::new("tmux")
        .args([
            "-S",
            &peer_endpoint.socket_path,
            "kill-pane",
            "-t",
            &peer_pane_id,
        ])
        .output()
        .unwrap();
    assert!(
        killed.status.success(),
        "kill target pane: {}",
        String::from_utf8_lossy(&killed.stderr)
    );
    let missing = super::handle_worker_snapshot(
        &server,
        "snapshot-master".into(),
        "token-snapshot-master".into(),
        "snapshot-peer".into(),
        40,
    );
    assert!(!missing.ok);
    assert!(missing.error.unwrap().starts_with("TMUX_PANE_MISSING:"));

    drop(tmux);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ordinary_worker_snapshots_master_and_master_closes_idempotently() {
    let (server, root) = test_server();
    let tmux = IsolatedTmux::start(&root);
    let endpoints = tmux.endpoints();
    assert_eq!(endpoints.len(), 2);
    // The master must carry an explicit AppServer session and thread: a
    // Transport observation does not change persistent master authority.
    let mut master_endpoint = endpoints[0].clone();
    master_endpoint.codex_session_id = Some("session-master".into());
    master_endpoint.codex_thread_id = Some("thread-master".into());
    assert!(register_tmux(&server, "master", master_endpoint).ok);
    assert!(register_tmux(&server, "ordinary", endpoints[1].clone()).ok);
    promote_master(&server, "master", "user approved master");

    let missing = super::handle_worker_close(
        &server,
        "master".into(),
        "token-master".into(),
        "ordinary".into(),
        "confirmed offline".into(),
    );
    assert!(!missing.ok);
    assert!(missing
        .error
        .unwrap()
        .contains("requires a successful worker snapshot"));

    let revision_before_snapshot = server.state.lock().unwrap().revision;
    let ordinary_snapshot = super::handle_worker_snapshot(
        &server,
        "ordinary".into(),
        "token-ordinary".into(),
        "master".into(),
        40,
    );
    assert!(
        ordinary_snapshot.ok,
        "{}",
        ordinary_snapshot.error.unwrap_or_default()
    );
    assert_eq!(ordinary_snapshot.data["durable"], false);
    assert_eq!(ordinary_snapshot.data["worker_id"], "master");
    assert_eq!(
        server.state.lock().unwrap().revision,
        revision_before_snapshot
    );

    let transient_snapshot = super::handle_worker_snapshot(
        &server,
        "master".into(),
        "token-master".into(),
        "ordinary".into(),
        40,
    );
    assert!(transient_snapshot.ok, "{}", transient_snapshot.error.unwrap_or_default());
    assert_eq!(transient_snapshot.data["durable"], false);
    let state = server.state.lock().unwrap();
    assert!(!state.worker_snapshots.contains_key("ordinary"));
    assert_eq!(state.revision, revision_before_snapshot);
    drop(state);

    server.commit(&[Event::WorkerSnapshotCaptured {
        worker_id: "ordinary".into(),
        thread_id: endpoints[1].pane_id.clone(),
        captured_ms: now_ms(),
    }]);
    let closed = super::handle_worker_close(
        &server,
        "master".into(),
        "token-master".into(),
        "ordinary".into(),
        "confirmed offline after snapshot".into(),
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());
    assert!(closed.data["snapshot_captured_ms"].is_i64());

    let repeated = super::handle_worker_close(
        &server,
        "master".into(),
        "token-master".into(),
        "ordinary".into(),
        "ignored duplicate".into(),
    );
    assert!(repeated.ok, "{}", repeated.error.unwrap_or_default());
    assert_eq!(repeated.data["reused"], true);
    assert_eq!(repeated.data["reason"], "confirmed offline after snapshot");

    let replayed = current_state::load(&server.state_path, false).unwrap();
    assert!(!replayed.workers.contains_key("ordinary"));
    assert_eq!(
        replayed.worker_closures["ordinary"].reason,
        "confirmed offline after snapshot"
    );
    assert_eq!(
        replayed.worker_snapshots["ordinary"].thread_id,
        endpoints[1].pane_id
    );
    drop(tmux);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn worker_snapshot_dispatches_rex_capture_without_writing_input() {
    use crate::client::adapters::rex::{
        set_test_cli, set_test_timeout, TEST_FIXTURE_LOCK,
    };
    use std::os::unix::fs::PermissionsExt;

    let _guard = TEST_FIXTURE_LOCK.lock().await;
    let (server, root) = test_server();
    let fixture_dir = root.join("rex-fixture");
    std::fs::create_dir_all(&fixture_dir).unwrap();
    let fixture = fixture_dir.join("rex-cli");
    std::fs::write(
        &fixture,
        include_str!("../../adapters/rex/fixture_rex.py"),
    )
    .unwrap();
    std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o700)).unwrap();
    set_test_cli(Some(fixture));
    set_test_timeout(Some(Duration::from_secs(5)));
    std::env::set_var("REXPRIMITIVE_LOG_DIR", &fixture_dir);
    std::env::set_var("REXPRIMITIVE_SCENARIO", "capture_lines");

    let transport = crate::proto::SelectedTransport {
        kind: TransportKind::Rex,
        endpoint: None,
        namespace: None,
        session_id: Some("agent-session-rex-snapshot".into()),
        thread_id: None,
        tmux_endpoint: None,
        rex_endpoint: Some(crate::proto::RexEndpoint {
            server: "unix:///tmp/rex-fixture.sock".into(),
            session_id: "session:11111111-1111-1111-1111-111111111111".into(),
            block_id: "block:22222222-2222-2222-2222-222222222222".into(),
            host_id: Some("host-fixture".into()),
            instance_id: "instance-fixture".into(),
            cwd: root.display().to_string(),
        }),
        capabilities: vec!["send_message_to_block".into()],
        self_check: "rex snapshot fixture".into(),
    };
    let registered = seed_committed_owner(
        &server,
        "rex-snapshot-peer",
        "token-rex-snapshot-peer",
        &transport,
        &root.display().to_string(),
        None,
    );
    assert!(registered.ok, "{registered:?}");

    let revision_before = server.state.lock().unwrap().revision;
    let state_before = std::fs::read(&server.state_path).unwrap();
    let server = Arc::new(server);
    let snapshot_server = server.clone();
    let response = tokio::task::spawn_blocking(move || {
        super::handle_worker_snapshot(
            &snapshot_server,
            "rex-snapshot-peer".into(),
            "token-rex-snapshot-peer".into(),
            "rex-snapshot-peer".into(),
            3,
        )
    })
    .await
    .unwrap();
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["transport"], "rex");
    assert_eq!(response.data["worker_id"], "rex-snapshot-peer");
    assert_eq!(response.data["lines"], 3);
    assert_eq!(response.data["durable"], false);
    assert_eq!(response.data["output"], "line-8\nline-9\nline-10");
    assert!(!server
        .state
        .lock()
        .unwrap()
        .worker_snapshots
        .contains_key("rex-snapshot-peer"));
    assert_eq!(server.state.lock().unwrap().revision, revision_before);
    assert_eq!(std::fs::read(&server.state_path).unwrap(), state_before);

    let calls = std::fs::read_to_string(fixture_dir.join("invocations.jsonl")).unwrap();
    let calls: Vec<Value> = calls
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(calls.iter().any(|call| {
        call["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg.as_str() == Some("capture"))
    }));
    assert!(!calls.iter().any(|call| {
        call["argv"].as_array().unwrap().iter().any(|arg| {
            matches!(
                arg.as_str(),
                Some("com.superlogical.terminal.write") | Some("send-key")
            )
        })
    }));

    set_test_cli(None);
    set_test_timeout(None);
    std::env::remove_var("REXPRIMITIVE_LOG_DIR");
    std::env::remove_var("REXPRIMITIVE_SCENARIO");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wake_hold_requires_master_and_replaces_or_releases_the_gate() {
    let (server, root) = test_server();
    register(&server, "gate-master", "thread-gate-master");
    register_synthetic_peer(&server, "gate-peer", "thread-gate-peer");
    promote_master(&server, "gate-master", "user approved gate-master as collab master");

    let rejected = super::handle_master_wake_hold(
        &server,
        "gate-peer".into(),
        "token-gate-peer".into(),
        "peer cannot hold the master ledger".into(),
        60,
    );
    assert!(!rejected.ok, "{rejected:?}");
    assert!(
        rejected
            .error
            .unwrap_or_default()
            .contains("master authority"),
        "non-master hold must be rejected by master authority"
    );

    let before = now_ms();
    let first = super::handle_master_wake_hold(
        &server,
        "gate-master".into(),
        "token-gate-master".into(),
        "waiting on the upstream CI gate".into(),
        120,
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    assert_eq!(first.data["replaced"], false);
    assert_eq!(first.data["held_by"], "gate-master");
    assert_eq!(first.data["reason"], "waiting on the upstream CI gate");
    let first_until = first.data["until_ms"].as_i64().unwrap();
    let first_held = first.data["held_ms"].as_i64().unwrap();
    assert!(first_held >= before);
    assert_eq!(first_until - first_held, 120_000);
    {
        let state = server.state.lock().unwrap();
        let hold = state.master_wake.wake_hold.as_ref().expect("held ledger");
        assert_eq!(hold.held_by, "gate-master");
        assert_eq!(hold.reason, "waiting on the upstream CI gate");
        assert_eq!(hold.until_ms, first_until);
    }
    let status = super::handle_master_status(&server);
    assert_eq!(status.data["master"]["worker_id"], "gate-master");
    assert_eq!(
        status.data["master"]["master_wake"]["wake_hold"]["reason"],
        "waiting on the upstream CI gate"
    );

    let replaced = super::handle_master_wake_hold(
        &server,
        "gate-master".into(),
        "token-gate-master".into(),
        "waiting on the release record".into(),
        30,
    );
    assert!(replaced.ok, "{}", replaced.error.unwrap_or_default());
    assert_eq!(replaced.data["replaced"], true);
    let replaced_until = replaced.data["until_ms"].as_i64().unwrap();
    let replaced_held = replaced.data["held_ms"].as_i64().unwrap();
    assert_eq!(replaced_until - replaced_held, 30_000);
    {
        let state = server.state.lock().unwrap();
        let hold = state.master_wake.wake_hold.as_ref().expect("held ledger");
        assert_eq!(hold.reason, "waiting on the release record");
        assert_eq!(hold.until_ms, replaced_until);
    }

    let released = super::handle_master_wake_release(
        &server,
        "gate-master".into(),
        "token-gate-master".into(),
    );
    assert!(released.ok, "{}", released.error.unwrap_or_default());
    assert_eq!(released.data["was_held"], true);
    assert!(server.state.lock().unwrap().master_wake.wake_hold.is_none());
    let status = super::handle_master_status(&server);
    assert!(status.data["master"]["master_wake"]["wake_hold"].is_null());

    let again = super::handle_master_wake_release(
        &server,
        "gate-master".into(),
        "token-gate-master".into(),
    );
    assert!(again.ok, "{}", again.error.unwrap_or_default());
    assert_eq!(again.data["was_held"], false);

    let empty = super::handle_master_wake_hold(
        &server,
        "gate-master".into(),
        "token-gate-master".into(),
        "   ".into(),
        60,
    );
    assert!(!empty.ok);
    assert!(empty.error.unwrap_or_default().contains("REASON_EMPTY"));
    let zero = super::handle_master_wake_hold(
        &server,
        "gate-master".into(),
        "token-gate-master".into(),
        "external gate".into(),
        0,
    );
    assert!(!zero.ok);
    assert!(zero.error.unwrap_or_default().contains("TTL_INVALID"));
    let too_long = super::handle_master_wake_hold(
        &server,
        "gate-master".into(),
        "token-gate-master".into(),
        "x".repeat(2001),
        60,
    );
    assert!(!too_long.ok);
    assert!(too_long
        .error
        .unwrap_or_default()
        .contains("REASON_TOO_LONG"));

    std::fs::remove_dir_all(root).unwrap();
}
