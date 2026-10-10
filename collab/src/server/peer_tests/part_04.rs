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
        worker_presence(&server, &target),
        IdentityPresence::Missing,
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
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate verified".into()),
            Some("/tmp/lifecycle-r1-worktree".into()),
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
    initialize_main(&root);
    assert!(
        handle_task_integrated(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            current_head(&root),
            "main verified".into(),
        )
        .ok
    );

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

#[test]
fn worker_snapshot_rejects_tmux_without_writing_a_receipt() {
    let (server, root) = test_server();
    let tmux = IsolatedTmux::start(&root);
    let endpoints = tmux.endpoints();
    assert_eq!(endpoints.len(), 2);
    // The master must carry an explicit AppServer session and thread: a
    // tmux-only binding is Unknown and therefore holds no live master authority.
    let mut master_endpoint = endpoints[0].clone();
    master_endpoint.codex_session_id = Some("session-snapshot-master".into());
    master_endpoint.codex_thread_id = Some("thread-snapshot-master".into());
    assert!(register_tmux(&server, "snapshot-master", master_endpoint).ok);
    assert!(register_tmux(&server, "snapshot-peer", endpoints[1].clone()).ok);
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
        40,
    );
    assert!(!response.ok);
    assert!(response
        .error
        .unwrap()
        .starts_with("WORKER_SNAPSHOT_UNSUPPORTED:"));
    let state = server.state.lock().unwrap();
    assert!(!state.worker_snapshots.contains_key("snapshot-peer"));
    assert_eq!(state.revision, revision_before);
    drop(state);
    assert_eq!(std::fs::read(&server.journal_path).unwrap(), journal_before);

    drop(tmux);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ordinary_worker_requires_its_own_snapshot_and_closes_idempotently() {
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

    let outsider_snapshot = super::handle_worker_snapshot(
        &server,
        "ordinary".into(),
        "token-ordinary".into(),
        "master".into(),
        40,
    );
    assert!(!outsider_snapshot.ok);
    assert!(outsider_snapshot
        .error
        .unwrap()
        .contains("master authority required"));

    let revision_before_snapshot = server.state.lock().unwrap().revision;
    let unsupported_snapshot = super::handle_worker_snapshot(
        &server,
        "master".into(),
        "token-master".into(),
        "ordinary".into(),
        40,
    );
    assert!(!unsupported_snapshot.ok);
    assert!(unsupported_snapshot
        .error
        .unwrap()
        .starts_with("WORKER_SNAPSHOT_UNSUPPORTED:"));
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
    let worker_registration = register(&server, "peer-a", "%a");
    let target_registration = register(&server, "peer-b", "%b");
    assert_eq!(worker_registration.data["role_brief"]["role"], "worker");
    assert_eq!(target_registration.data["role_brief"]["role"], "worker");
    assert!(worker_registration.data["role_brief"]["role_task"]
        .as_str()
        .unwrap()
        .contains("independent task"));
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
    assert!(promoted.data["role_brief"]["responsibilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line.as_str().unwrap().contains("assign tasks")));
    let context = handle_context(&server, "peer-a".into(), "token-peer-a".into());
    assert_eq!(context.data["master"]["worker_id"], "peer-a");
    assert_eq!(context.data["role_brief"]["role"], "master");
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
