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
        create_operation_id: None, binding_id: None, endpoint_generation: None,
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
        create_operation_id: None, binding_id: None, endpoint_generation: None,
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
fn master_name_uses_authority_when_read_projection_is_peer() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approval");
    let worker = server.state.lock().unwrap().workers["master"].clone();
    let summary = worker_status_summary_with_maps(&server, &Default::default(), &Default::default(), &Default::default(), json!({"role":"peer"}), &worker);
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
                endpoint_generation: None,
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
                endpoint_generation: None,
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

    let journal: Vec<serde_json::Value> =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
    let first_sent_index = journal
        .iter()
        .position(|event| event["ev"] == "Sent" && event["msg"]["id"] == first_message)
        .unwrap();
    let second_sent_index = journal
        .iter()
        .position(|event| event["ev"] == "Sent" && event["msg"]["id"] == second_message)
        .unwrap();
    assert!(!journal[first_sent_index..second_sent_index]
        .iter()
        .any(|event| event["ev"] == "SubagentUpdated" && event["subagent"]["id"] == "managed-b"),
        "the selected child must be bound only in the message commit");

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
    let replayed = replay(&root).unwrap();
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
            endpoint_generation: None,
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
                endpoint_generation: None,
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
                    endpoint_generation: None,
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
                    endpoint_generation: None,
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
            endpoint_generation: None,
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
    let journal_after_first =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .lines()
            .count();
    let second = crate::subagent::handle(
        &server,
        "child",
        "token-child",
        Action::Working {
            id: "managed".into(),
        },
    );
    assert!(second.ok, "{}", second.error.unwrap_or_default());
    let journal_after_second =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .lines()
            .count();
    assert_eq!(journal_after_second, journal_after_first);
    assert_eq!(server.state.lock().unwrap().tasks.len(), 1);

    let replayed = replay(&root).unwrap();
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
    assert!(
        super::handle_master_promote(
            &server,
            "master".into(),
            "token-master".into(),
            "user approved master".into(),
        )
        .ok
    );

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

    let replayed = replay(&root).unwrap();
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

fn owned_missing_tmux_endpoint(label: &str) -> (PathBuf, crate::proto::TmuxEndpoint, String) {
    let socket = std::env::temp_dir().join(format!(
        "collab-route-less-close-{}-{label}.sock",
        std::process::id()
    ));
    let run = |args: &[&str]| {
        let output = Command::new("tmux")
            .arg("-S")
            .arg(&socket)
            .args(args)
            .output()
            .expect("run owned tmux command");
        assert!(
            output.status.success(),
            "owned tmux {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    run(&["new-session", "-d", "-s", "route-less-keep", "sleep 60"]);
    run(&["new-session", "-d", "-s", "route-less-gone", "sleep 60"]);
    let server_pid = String::from_utf8(
        run(&["display-message", "-p", "#{pid}"]).stdout,
    )
    .unwrap()
    .trim()
    .parse::<u32>()
    .unwrap();
    let fields = String::from_utf8(
        run(&[
            "display-message",
            "-p",
            "-t",
            "route-less-gone",
            "#{session_id}\t#{pane_id}\t#{pane_pid}",
        ])
        .stdout,
    )
    .unwrap();
    let mut fields = fields.trim_end().split('\t');
    let tmux_session_id = fields.next().unwrap().to_owned();
    let pane_id = fields.next().unwrap().to_owned();
    let pane_pid = fields.next().unwrap().parse::<u32>().unwrap();
    run(&["kill-session", "-t", "route-less-gone"]);
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: socket.display().to_string(),
        server_pid,
        tmux_session_id,
        pane_id,
        pane_pid,
        codex_session_id: None,
        codex_thread_id: None,
    };
    (socket, endpoint, "route-less-keep".into())
}

#[test]
fn lifecycle_close_retires_a_route_less_missing_tmux_target() {
    use crate::proto::{PeerLifecycleRequest, PeerLifecycleTarget};

    let (server, root) = test_server();
    let app = AppServerId::new("tui-default").unwrap();
    let cwd = root.display().to_string();
    let master_transport = test_appserver_transport("thread-master");
    let registered = seed_committed_owner(
        &server,
        "master",
        "token-master",
        &master_transport,
        &cwd,
        Some(app.clone()),
    );
    assert!(registered.ok, "{}", registered.error.unwrap_or_default());
    commit_current_thread_route_for_runtime(&server, &server, "master", &cwd, Some(&app)).unwrap();
    promote_master(&server, "master", "user approved lifecycle close");

    let (tmux_socket, endpoint, keep_session) = owned_missing_tmux_endpoint("target");
    let stale_transport = SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some(endpoint.socket_path.clone()),
        namespace: Some(endpoint.tmux_session_id.clone()),
        session_id: Some("route-less-agent-session".into()),
        thread_id: Some("route-less-agent-thread".into()),
        tmux_endpoint: Some(endpoint),
        capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
        self_check: "route-less missing tmux target".into(),
    };
    let registered = seed_committed_owner(
        &server,
        "stale-peer",
        "token-stale-peer",
        &stale_transport,
        &cwd,
        Some(app.clone()),
    );
    assert!(registered.ok, "{}", registered.error.unwrap_or_default());
    // Model a persisted legacy no-session binding explicitly. New registrations
    // require an agent session; this case tests retirement of historical data.
    let mut legacy_binding = registered_binding(&server, "stale-peer");
    legacy_binding.endpoint_generation += 1;
    legacy_binding.session_id = None;
    legacy_binding.native_thread_id = None;
    let mut legacy_worker = server.state.lock().unwrap().workers["stale-peer"].clone();
    let legacy_transport = legacy_worker.transport.as_mut().unwrap();
    legacy_transport.session_id = None;
    legacy_transport.thread_id = None;
    server
        .commit_checked(&[
            Event::GlobalRuntimeBound {
                binding: legacy_binding,
            },
            Event::Registered {
                worker: legacy_worker,
            },
        ])
        .unwrap();
    assert!(
        server
            .state
            .lock()
            .unwrap()
            .global
            .current_thread_routes
            .values()
            .all(|route| route.agent_id.as_str() != "stale-peer"),
        "fixture must leave the target route-less"
    );
    let now = now_ms();
    server.commit(&[Event::NotificationSubscribed {
        subscription: crate::server::state::NotificationSubscription {
            id: "sub-stale-peer".into(),
            worker_id: "stale-peer".into(),
            target: "missing-pane".into(),
            event: "direct-message".into(),
            subject: None,
            method: "tmux".into(),
            status: "armed".into(),
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            created_ms: now,
            updated_ms: now,
            fired_count: 0,
            trigger_ms: None,
            trigger_times_ms: vec![],
            repeat_count: 1,
            interval_ms: None,
            expires_ms: now + 60_000,
        },
    }]);

    let context = ProjectContext::for_registered_root_with_app(&root, app).unwrap();
    let read = super::peer_lifecycle::handle(
        &server,
        Some(&context),
        PeerLifecycleRequest::Read {
            worker_id: "master".into(),
            token: "token-master".into(),
            target_id: Some("stale-peer".into()),
        },
    );
    assert!(read.ok, "{}", read.error.unwrap_or_default());
    assert_eq!(
        read.data["result"]["target"]["worker_id"],
        "stale-peer",
        "{}",
        read.data
    );
    assert_eq!(read.data["result"]["projection"]["endpoint_live"], false);
    assert!(
        read.data["result"]["projection"]["transport_view"]["error"]
            .as_str()
            .unwrap()
            .starts_with("TMUX_CAPTURE_FAILED: can't find pane:"),
        "{}",
        read.data
    );
    let target: PeerLifecycleTarget =
        serde_json::from_value(read.data["result"]["target"].clone()).unwrap();

    let closed = super::peer_lifecycle::handle(
        &server,
        Some(&context),
        PeerLifecycleRequest::Close {
            worker_id: "master".into(),
            token: "token-master".into(),
            operation_id: "route-less-close-op".into(),
            query_capability: "route-less-close-capability".into(),
            target,
            reason: "remove stale no-session binding causing tombstone poison".into(),
        },
    );
    assert!(closed.ok, "{:?}", closed.error);
    assert_eq!(closed.data["result"]["outcome"], "complete", "{}", closed.data);
    assert_eq!(closed.data["result"]["phase"], "complete", "{}", closed.data);
    assert_eq!(
        closed.data["result"]["close"]["runtime_archive"]["state"],
        "missing",
        "{}",
        closed.data
    );
    assert_eq!(
        closed.data["result"]["close"]["worker_retirement"]["state"],
        "verified",
        "{}",
        closed.data
    );
    assert_eq!(
        closed.data["result"]["close"]["binding_retirement"]["state"],
        "verified",
        "{}",
        closed.data
    );
    assert_eq!(
        closed.data["result"]["close"]["subscription_retirement"]["state"],
        "verified",
        "{}",
        closed.data
    );
    assert!(!server.state.lock().unwrap().workers.contains_key("stale-peer"));

    let _ = Command::new("tmux")
        .arg("-S")
        .arg(&tmux_socket)
        .args(["kill-session", "-t", &keep_session])
        .output();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_close_is_master_only_audited_and_refuses_to_strand_tasks() {
    let (server, root) = test_server();
    register(&server, "peer-a", "%a");
    register(&server, "peer-b", "%b");
    assert!(
        super::handle_master_promote(
            &server,
            "peer-a".into(),
            "token-peer-a".into(),
            "user approved peer-a as collab master".into(),
        )
        .ok
    );

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

    let replayed = super::replay(&root).unwrap();
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
    assert!(
        super::handle_master_promote(
            &server,
            "master".into(),
            "token-master".into(),
            "user approved master".into(),
        )
        .ok
    );
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
    let replayed = super::replay(&root).unwrap();
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
    let promoted = super::handle_master_promote(
        &server,
        "snapshot-master".into(),
        "token-snapshot-master".into(),
        "approved for worker snapshot test".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());

    let revision_before = server.state.lock().unwrap().revision;
    let journal_before = std::fs::read(&server.journal_path).unwrap();
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
        (
            "snapshot-appserver",
            "token-snapshot-master",
            "WORKER_SNAPSHOT_UNSUPPORTED",
        ),
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
    assert!(!incidental_tmux.ok);
    assert!(incidental_tmux
        .error
        .unwrap()
        .starts_with("WORKER_SNAPSHOT_UNSUPPORTED:"));
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
    assert_eq!(state.revision, revision_before);
    drop(state);
    assert_eq!(std::fs::read(&server.journal_path).unwrap(), journal_before);

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
    // tmux-only binding is Unknown and therefore holds no live master authority.
    let mut master_endpoint = endpoints[0].clone();
    master_endpoint.codex_session_id = Some("session-master".into());
    master_endpoint.codex_thread_id = Some("thread-master".into());
    assert!(register_tmux(&server, "master", master_endpoint).ok);
    assert!(register_tmux(&server, "ordinary", endpoints[1].clone()).ok);
    assert!(
        super::handle_master_promote(
            &server,
            "master".into(),
            "token-master".into(),
            "user approved master".into(),
        )
        .ok
    );

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

    let replayed = replay(&root).unwrap();
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

#[test]
fn master_promotion_requires_user_approval_and_existing_master_delegates() {
    let (server, root) = test_server();
    let server = Arc::new(server);
    let worker_registration = register_synthetic_peer(&server, "peer-a", "%a");
    let target_registration = register_synthetic_peer(&server, "peer-b", "%b");
    assert_eq!(worker_registration.data["role_brief"]["role"], "worker");
    assert_eq!(
        worker_registration.data["role_brief"]["derivation"]["kind"],
        "peer"
    );
    assert_eq!(target_registration.data["role_brief"]["role"], "worker");
    let role_text = |brief: &Value| {
        let mut text = vec![
            brief["role_task"].as_str().unwrap_or_default(),
            brief["next_action"].as_str().unwrap_or_default(),
        ];
        text.extend(
            brief["responsibilities"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        );
        text.join(" ").to_lowercase()
    };
    let ordinary_text = role_text(&worker_registration.data["role_brief"]);
    assert!(ordinary_text.contains("long-horizon orchestrator"));
    assert!(ordinary_text.contains("prefer assigning code implementation to subagents"));
    assert!(ordinary_text.contains("orchestrate scheduling, acceptance, delivery"));
    assert!(ordinary_text.contains("owned-resource closure"));
    assert!(ordinary_text.contains("codex exec --profile gcm"));
    assert!(ordinary_text.contains("regardless of whether other peers are busy"));
    assert!(ordinary_text.contains("verify subworker evidence"));
    assert!(ordinary_text.contains("contact the current master"));
    assert!(ordinary_text.contains("finite patrol"));
    assert!(ordinary_text.contains("subscription presence does not gate identity"));
    assert!(ordinary_text.contains("do not invent a polling interval"));
    assert!(ordinary_text.contains("tmux"));
    assert!(!ordinary_text.contains("only when every eligible peer is busy"));
    assert!(!ordinary_text.contains("capacity fallback"));
    assert!(!ordinary_text.contains("do not claim master grants"));
    assert_eq!(
        worker_registration.data["role_brief"]["communication_recovery"]["close_only_when"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    let missing =
        super::handle_master_promote(&server, "peer-a".into(), "token-peer-a".into(), "".into());
    assert!(!missing.ok);
    assert!(missing.error.unwrap().contains("approval"));

    let promoted = super::handle_master_promote(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "user approved peer-a as collab master".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());
    assert_eq!(promoted.data["mode"], "user_approved_self_promotion");
    assert_eq!(promoted.data["role_brief"]["role"], "master");
    let master_text = role_text(&promoted.data["role_brief"]);
    assert!(master_text.contains("long-horizon orchestrator"));
    assert!(master_text.contains("prefer assigning code implementation to subagents"));
    assert!(master_text.contains("orchestrate scheduling, acceptance, delivery"));
    assert!(master_text.contains("owned-resource closure"));
    assert!(master_text.contains("codex exec --profile gcm"));
    assert!(master_text.contains("regardless of whether other peers are busy"));
    assert!(master_text.contains("p0/p1"));
    assert!(master_text.contains("peer work progress evidence"));
    assert!(master_text.contains("finite patrol"));
    assert!(master_text.contains("subscription presence does not gate identity"));
    assert!(master_text.contains("do not invent a polling interval"));
    assert!(master_text.contains("tmux"));
    assert!(!master_text.contains("only when every eligible peer is busy"));
    assert!(!master_text.contains("capacity fallback"));
    assert!(!master_text.contains("saturate every live present peer first"));
    let context = handle_context(&server, "peer-a".into(), "token-peer-a".into());
    assert_eq!(context.data["master"]["worker_id"], "peer-a");
    assert_eq!(context.data["role_brief"]["role"], "master");
    assert_eq!(context.data["role_brief"], promoted.data["role_brief"]);
    let status = super::handle_master_status(&server);
    assert_eq!(status.data["master"]["worker_id"], "peer-a");
    assert_eq!(
        status.data["master"]["approval"],
        "user approved peer-a as collab master"
    );

    // A peer cannot take the seat by itself. Only explicit user approval strips
    // the recorded grant, so without approval the incumbent keeps it.
    let unapproved = super::handle_master_promote(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "  ".into(),
    );
    assert!(!unapproved.ok, "{unapproved:?}");
    assert_eq!(
        super::current_master_holder(&server, &server.state.lock().unwrap()).unwrap(),
        Some("peer-a".into())
    );

    let outsider = super::handle_master_delegate(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "peer-a".into(),
    );
    assert!(!outsider.ok);
    assert!(outsider.error.unwrap().contains("master authority"));

    let delegated = super::handle_master_delegate(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "peer-b".into(),
    );
    assert!(delegated.ok, "{}", delegated.error.unwrap_or_default());
    assert_eq!(
        super::current_master_holder(&server, &server.state.lock().unwrap()).unwrap(),
        Some("peer-b".into())
    );
    assert_eq!(delegated.data["role_brief"]["role"], "master");
    let target_context = handle_context(&server, "peer-b".into(), "token-peer-b".into());
    assert_eq!(target_context.data["identity"]["role"], "master");
    assert_eq!(target_context.data["role_brief"]["role"], "master");
    let workers = dispatch_with_route_context(&server, Req::Workers, None);
    let target_worker = workers.data["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "peer-b")
        .unwrap();
    assert_eq!(target_worker["role_brief"]["role"], "master");
    let status = dispatch_with_route_context(&server, Req::StatusAll, None);
    let target_status = status.data["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "peer-b")
        .unwrap();
    assert_eq!(target_status["role_brief"], target_worker["role_brief"]);
    assert_eq!(target_registration.data["role_brief"]["role"], "worker");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_wake_hold_requires_master_and_replaces_or_releases_the_gate() {
    let (server, root) = test_server();
    register_synthetic_peer(&server, "gate-master", "thread-gate-master");
    register_synthetic_peer(&server, "gate-peer", "thread-gate-peer");
    let promoted = super::handle_master_promote(
        &server,
        "gate-master".into(),
        "token-gate-master".into(),
        "user approved gate-master as collab master".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());

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
