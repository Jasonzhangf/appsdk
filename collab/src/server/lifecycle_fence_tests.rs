use super::*;

const TARGET: &str = "target";

fn fence_server() -> (Arc<Server>, PathBuf) {
    let (server, root) = peer_tests::test_server();
    (Arc::new(server), root)
}

fn exact_fence(worker_id: &str, operation_id: &str) -> state::ResponsibilityFence {
    state::ResponsibilityFence {
        operation_id: operation_id.into(),
        worker_id: worker_id.into(),
        project_scope: "/tmp/lifecycle-fence-project".into(),
        app_scope: "tui-default".into(),
        binding_id: format!("binding-{worker_id}"),
        endpoint_generation: 1,
        snapshot: state::ResponsibilitySnapshot::default(),
        state: "closing".into(),
        created_ms: 1,
    }
}

fn task(owner: &str, id: &str, status: &str) -> TaskRec {
    TaskRec {
        id: id.into(),
        owner: owner.into(),
        created_by: owner.into(),
        feature_id: None,
        worktree_path: None,
        branch: None,
        base_commit: None,
        priority: "p2".into(),
        status: status.into(),
        next_step: None,
        wait: None,
        created_ms: 1,
        updated_ms: 1,
    }
}

fn message(to: &str, id: &str, mtype: &str) -> state::Message {
    state::Message {
        id: id.into(),
        from: "master".into(),
        to: to.into(),
        mtype: mtype.into(),
        subject: None,
        body: "payload".into(),
        in_reply_to: None,
        created_ms: 1,
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    }
}

fn subscription(worker_id: &str, id: &str, status: &str) -> state::NotificationSubscription {
    state::NotificationSubscription {
        id: id.into(),
        worker_id: worker_id.into(),
        event: "direct-message".into(),
        subject: None,
        target: "thread-target".into(),
        method: "appserver".into(),
        trigger_ms: None,
        trigger_times_ms: Vec::new(),
        interval_ms: None,
        repeat_count: 1,
        fired_count: 0,
        expires_ms: i64::MAX,
        status: status.into(),
        created_ms: 1,
        updated_ms: 1,
        status_reason: None,
    }
}

fn repeating_subscription(
    worker_id: &str,
    id: &str,
    status: &str,
    fired_count: u32,
    repeat_count: u32,
) -> state::NotificationSubscription {
    state::NotificationSubscription {
        repeat_count,
        fired_count,
        interval_ms: Some(10),
        ..subscription(worker_id, id, status)
    }
}

fn fence_with_snapshot(server: &Server, worker_id: &str, operation_id: &str) -> Event {
    let snapshot = state::responsibility_snapshot(&server.state.lock().unwrap(), worker_id);
    Event::ResponsibilityFenceSet {
        fence: state::ResponsibilityFence {
            snapshot,
            ..exact_fence(worker_id, operation_id)
        },
    }
}

fn delivery_message(to: &str, id: &str, state: &str) -> state::Message {
    state::Message {
        state: state.into(),
        ..message(to, id, "notify")
    }
}

fn subagent(peer: &str, id: &str, status: &str) -> crate::subagent::Record {
    crate::subagent::Record {
        id: id.into(),
        parent: "master".into(),
        peer: peer.into(),
        status: status.into(),
        thread_id: None,
        profile: None,
        created_ms: 1,
        ready_deadline_ms: 1,
        last_message: None,
        error: None,
        probe_failures: Vec::new(),
        runtime: None,
        create_operation_id: None,
        binding_id: None,
        endpoint_generation: None,
    }
}

fn public_task(owner: &str, id: &str, status: &str) -> TaskRec {
    TaskRec {
        id: id.into(),
        owner: owner.into(),
        created_by: owner.into(),
        feature_id: None,
        worktree_path: None,
        branch: None,
        base_commit: None,
        priority: "p2".into(),
        status: status.into(),
        next_step: Some("public-next-step".into()),
        wait: None,
        created_ms: 1,
        updated_ms: 1,
    }
}

fn public_details() -> crate::board::BoardTaskDetails {
    crate::board::BoardTaskDetails {
        title: "public-title".into(),
        description: "public-description".into(),
        delivery_condition: "public-delivery".into(),
        test_condition: "public-test".into(),
        revision: 42,
        public_visibility: true,
        invitation: Some(crate::board::BoardInvitation {
            peer_id: "invitee".into(),
            binding_id: "invite-binding".into(),
            endpoint_generation: 13,
            message_id: "offer-message".into(),
            created_ms: 3,
        }),
        last_response: Some("public-response".into()),
    }
}

fn assert_exact_public_details(state: &State, task_id: &str) {
    let details: &crate::board::BoardTaskDetails = state
        .board_details
        .get(task_id)
        .unwrap_or_else(|| panic!("{task_id} details missing"));
    let expected = public_details();
    assert_eq!(details.title, expected.title);
    assert_eq!(details.description, expected.description);
    assert_eq!(details.delivery_condition, expected.delivery_condition);
    assert_eq!(details.test_condition, expected.test_condition);
    assert_eq!(details.revision, expected.revision);
    assert_eq!(details.public_visibility, expected.public_visibility);
    assert_eq!(
        details
            .invitation
            .as_ref()
            .map(|invite| invite.peer_id.as_str()),
        Some("invitee")
    );
    assert_eq!(
        details
            .invitation
            .as_ref()
            .map(|invite| invite.binding_id.as_str()),
        Some("invite-binding")
    );
    assert_eq!(
        details
            .invitation
            .as_ref()
            .map(|invite| invite.endpoint_generation),
        Some(13)
    );
    assert_eq!(
        details
            .invitation
            .as_ref()
            .map(|invite| invite.message_id.as_str()),
        Some("offer-message")
    );
    assert_eq!(
        details.invitation.as_ref().map(|invite| invite.created_ms),
        Some(3)
    );
    assert_eq!(details.last_response, expected.last_response);
}

fn fence(server: &Server, worker_id: &str, operation_id: &str) {
    server
        .commit_checked(&[Event::ResponsibilityFenceSet {
            fence: exact_fence(worker_id, operation_id),
        }])
        .unwrap();
}

fn assert_rejected_before_bytes(server: &Server, root: &Path, events: &[Event]) -> String {
    let journal_path = root.join(".agent-collab/server/journal.jsonl");
    let before = std::fs::read(&journal_path).unwrap();
    let sequence_before = server.state.lock().unwrap().sequence;
    let error = server.commit_checked(events).unwrap_err().to_string();
    assert!(error.contains("target_closing"), "{error}");
    let state = server.state.lock().unwrap();
    assert_eq!(state.sequence, sequence_before);
    drop(state);
    assert_eq!(std::fs::read(&journal_path).unwrap(), before);
    error
}

/// Every writer class named by the accepted lifecycle contract must be refused
/// before journal bytes change while the exact target is fenced.
#[test]
fn fence_rejects_every_responsibility_writer_class_before_journal_bytes_change() {
    let (server, root) = fence_server();
    fence(&server, TARGET, "close-op");

    // task registration/status and scheduler assignment
    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::TaskCreated {
            task: task(TARGET, "task-new", "pending"),
        }],
    );
    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::TaskUpdated {
            task: task(TARGET, "task-new", "working"),
        }],
    );
    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::SchedulerAdmission {
            admission: state::SchedulerAdmissionRecord {
                request_id: "req-1".into(),
                decision: "admit".into(),
                worker_id: TARGET.into(),
                managed_subagent_id: None,
                message_id: "msg-1".into(),
                task_id: "task-new".into(),
                status: "pending".into(),
                error: None,
                created_ms: 1,
                updated_ms: 1,
            },
        }],
    );

    // managed-child assignment
    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::SubagentUpdated {
            subagent: subagent(TARGET, "child", "assigned"),
        }],
    );

    // board ownership and worktree change
    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::WorktreeBound {
            binding: state::WorktreeBinding {
                worktree_root: "/tmp/wt".into(),
                owning_project_scope: "/tmp/lifecycle-fence-project".into(),
                task_id: "task-new".into(),
                owner_agent_id: TARGET.into(),
                binding_id: "wt-1".into(),
                base_commit: "abc".into(),
            },
        }],
    );
    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::MergeRequested {
            request: state::PendingMerge {
                task_id: "task-new".into(),
                owner: TARGET.into(),
                requested_by: "master".into(),
                requested_ms: 1,
                candidate_commit: None,
            },
        }],
    );

    // worker registration / binding / route publication
    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::Registered {
            worker: state::WorkerRec {
                id: TARGET.into(),
                token: "token".into(),
                cwd: "/tmp".into(),
                registered_ms: 1,
                transport: None,
            },
        }],
    );

    // subscription create plus the message/wake obligations
    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::NotificationSubscribed {
            subscription: subscription(TARGET, "sub-new", "armed"),
        }],
    );
    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::Sent {
            msg: message(TARGET, "msg-request", "request"),
        }],
    );
    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::Sent {
            msg: message(TARGET, "msg-notify", "notify"),
        }],
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fence_allows_safe_message_consumption_and_exact_retirement() {
    let (server, root) = fence_server();
    server
        .commit_checked(&[
            Event::Sent {
                msg: message(TARGET, "msg-1", "request"),
            },
            Event::ResponsibilityFenceSet {
                fence: exact_fence(TARGET, "close-op"),
            },
        ])
        .unwrap();

    // Consumption of an already-durable message and the exact retirement record
    // must commit while the target stays fenced.
    server
        .commit_checked(&[
            Event::Delivered {
                ids: vec!["msg-1".into()],
            },
            Event::Acked {
                ids: vec!["msg-1".into()],
            },
            Event::NotificationConsumed {
                subscription_id: "sub-consumed".into(),
                message_id: "msg-1".into(),
                consumed_ms: 2,
            },
            Event::WorkerClosed {
                worker_id: TARGET.into(),
                closed_by: "master".into(),
                reason: "record-only close".into(),
                snapshot_captured_ms: None,
                at_ms: 2,
            },
        ])
        .unwrap();

    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs["msg-1"].state, "read");
    assert!(state.worker_closures.contains_key(TARGET));
    assert!(state.responsibility_fences["close-op"].is_active());
    drop(state);

    // A new inbound message is a new obligation for the closing peer and is
    // refused even for consumption-style message types.
    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::Sent {
            msg: message(TARGET, "msg-reply", "reply"),
        }],
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fence_allows_retirement_events_but_not_rebinding_while_fenced() {
    let (server, root) = fence_server();
    server
        .commit_checked(&[
            Event::NotificationSubscribed {
                subscription: subscription(TARGET, "sub-1", "armed"),
            },
            Event::ResponsibilityFenceSet {
                fence: exact_fence(TARGET, "close-op"),
            },
        ])
        .unwrap();

    // Retirement of the exact target's obligation stays allowed.
    server
        .commit_checked(&[Event::NotificationStatus {
            subscription_id: "sub-1".into(),
            status: "cancelled".into(),
            updated_ms: 2,
        }])
        .unwrap();

    // Re-arming or rebinding the obligation is refused.
    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::NotificationStatus {
            subscription_id: "sub-1".into(),
            status: "armed".into(),
            updated_ms: 3,
        }],
    );
    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::NotificationRebound {
            subscription_id: "sub-1".into(),
            target: "thread-other".into(),
            updated_ms: 4,
        }],
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// A close admission that writes its own fence cannot be combined in the same
/// batch with an addition for the same target.
#[test]
fn fence_batch_self_admission_rejects_own_target_additions() {
    let (server, root) = fence_server();
    assert_rejected_before_bytes(
        &server,
        &root,
        &[
            Event::ResponsibilityFenceSet {
                fence: exact_fence(TARGET, "close-op"),
            },
            Event::TaskCreated {
                task: task(TARGET, "task-new", "pending"),
            },
        ],
    );

    // The rejected batch leaves no durable fence behind.
    assert!(!server
        .state
        .lock()
        .unwrap()
        .responsibility_fences
        .contains_key("close-op"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fence_survives_replay_and_compaction() {
    let (server, root) = fence_server();
    fence(&server, TARGET, "close-op");
    {
        let state = server.state.lock().unwrap();
        let events = state.snapshot_events();
        let mut checkpointed = State::default();
        for (line, event) in events.iter().enumerate() {
            apply_replayed_event(&mut checkpointed, event, line + 1).unwrap();
        }
        assert!(checkpointed.responsibility_fences["close-op"].is_active());
    }
    let replayed = replay(&root).unwrap();
    assert!(replayed.responsibility_fences["close-op"].is_active());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn close_admission_races_responsibility_writer_without_lost_snapshot() {
    let (server, root) = fence_server();
    let journal_path = root.join(".agent-collab/server/journal.jsonl");
    let barrier = Arc::new(std::sync::Barrier::new(3));

    let close_server = Arc::clone(&server);
    let close_barrier = Arc::clone(&barrier);
    let close = std::thread::spawn(move || {
        close_barrier.wait();
        let mut state = close_server.state.lock().unwrap();
        let snapshot = state::responsibility_snapshot(&state, TARGET);
        let mut record = exact_fence(TARGET, "close-op");
        record.snapshot = snapshot.clone();
        let result = close_server.commit_locked_checked(
            &mut state,
            &[
                Event::ResponsibilityFenceSet { fence: record },
                Event::WorkerClosed {
                    worker_id: TARGET.into(),
                    closed_by: "master".into(),
                    reason: "race".into(),
                    snapshot_captured_ms: None,
                    at_ms: 2,
                },
            ],
        );
        (snapshot, result)
    });

    let writer_server = Arc::clone(&server);
    let writer_barrier = Arc::clone(&barrier);
    let writer = std::thread::spawn(move || {
        writer_barrier.wait();
        writer_server.commit_checked(&[Event::TaskCreated {
            task: task(TARGET, "task-race", "pending"),
        }])
    });

    barrier.wait();
    let (close_snapshot, close_result) = close.join().unwrap();
    let writer_result = writer.join().unwrap();
    close_result.expect("close admission must commit");

    let writer_error = match writer_result {
        Ok(_) => None,
        Err(error) => Some(error.to_string()),
    };
    match &writer_error {
        Some(error) => {
            assert!(error.contains("target_closing"), "{error}");
            assert!(
                !close_snapshot.task_ids.contains(&"task-race".into()),
                "a fence-first admission must snapshot no writer responsibility"
            );
            assert!(!server.state.lock().unwrap().tasks.contains_key("task-race"));
        }
        None => {
            assert!(
                close_snapshot.task_ids.contains(&"task-race".into()),
                "a writer-first admission must be captured in the close snapshot"
            );
            assert!(server.state.lock().unwrap().tasks.contains_key("task-race"));
        }
    }

    let assert_replayed = |replayed: &State, stage: &str| {
        let fence = replayed
            .responsibility_fences
            .get("close-op")
            .unwrap_or_else(|| panic!("{stage}: fence missing"));
        assert!(fence.is_active(), "{stage}: fence inactive");
        assert_eq!(fence.snapshot, close_snapshot, "{stage}: snapshot changed");
        if writer_error.is_some() {
            assert!(
                !replayed.tasks.contains_key("task-race"),
                "{stage}: rejected writer became durable"
            );
        } else {
            assert!(
                replayed.tasks.contains_key("task-race"),
                "{stage}: accepted writer was lost"
            );
        }
    };

    assert_replayed(&replay(&root).unwrap(), "replay");
    server
        .rewrite_journal_locked(&server.state.lock().unwrap())
        .unwrap();
    assert_replayed(&replay(&root).unwrap(), "compaction replay");

    let journal = std::fs::read_to_string(&journal_path).unwrap();
    assert!(journal.contains("ResponsibilityFenceSet"), "{journal}");
    if writer_error.is_some() {
        assert!(!journal.contains("task-race"), "{journal}");
    } else {
        assert!(journal.contains("task-race"), "{journal}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn close_writer_first_compaction_preserves_snapshot_and_checkpoint() {
    let (server, root) = fence_server();
    server
        .commit_checked(&[Event::TaskCreated {
            task: task(TARGET, "task-race", "pending"),
        }])
        .unwrap();
    let snapshot = {
        let state = server.state.lock().unwrap();
        state::responsibility_snapshot(&state, TARGET)
    };
    server
        .commit_checked(&[
            Event::ResponsibilityFenceSet {
                fence: state::ResponsibilityFence {
                    snapshot,
                    ..exact_fence(TARGET, "close-op")
                },
            },
            Event::WorkerClosed {
                worker_id: TARGET.into(),
                closed_by: "master".into(),
                reason: "writer-first close".into(),
                snapshot_captured_ms: None,
                at_ms: 3,
            },
        ])
        .unwrap();
    let version = {
        let state = server.state.lock().unwrap();
        (state.sequence, state.revision)
    };
    assert_eq!(version, (3, 3));

    let pre_rewrite = replay(&root).unwrap();
    let snapshot = pre_rewrite.responsibility_fences["close-op"]
        .snapshot
        .clone();
    let task_id = pre_rewrite.tasks["task-race"].id.clone();
    assert!(snapshot.task_ids.contains(&task_id));
    assert!(pre_rewrite.worker_closures.contains_key(TARGET));

    server
        .rewrite_journal_locked(&server.state.lock().unwrap())
        .unwrap();

    let compacted = replay(&root).unwrap();
    assert_eq!(
        (compacted.sequence, compacted.revision),
        (3, 3),
        "synthetic restoration rows must not advance the original checkpoint"
    );
    assert_eq!(
        compacted.responsibility_fences["close-op"].snapshot,
        snapshot
    );
    assert_eq!(
        compacted.tasks["task-race"].id,
        pre_rewrite.tasks["task-race"].id
    );
    assert_eq!(
        compacted.tasks["task-race"].owner,
        pre_rewrite.tasks["task-race"].owner
    );
    assert_eq!(
        compacted.tasks["task-race"].status,
        pre_rewrite.tasks["task-race"].status
    );
    assert!(compacted.worker_closures.contains_key(TARGET));
    assert!(!compacted.workers.contains_key(TARGET));
    assert!(
        !std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .contains("\"ev\":\"Registered\""),
        "compaction must not register the closed public owner"
    );

    server
        .commit_checked(&[Event::KeepaliveUpdated {
            worker_id: "after-compaction-ordinary".into(),
            record: crate::server::keepalive::Record::default(),
        }])
        .unwrap();
    server
        .commit_command(
            "cmd-after-compaction",
            "op-after-compaction",
            &[Event::KeepaliveUpdated {
                worker_id: "after-compaction-typed".into(),
                record: crate::server::keepalive::Record::default(),
            }],
            serde_json::json!({"accepted": true}),
        )
        .unwrap();
    server
        .rewrite_journal_locked(&server.state.lock().unwrap())
        .unwrap();
    let recompacted = replay(&root).unwrap();
    assert_eq!(
        (recompacted.sequence, recompacted.revision),
        (7, 7),
        "ordinary and typed commits after compaction must keep one version axis"
    );
    assert!(recompacted
        .keepalives
        .contains_key("after-compaction-ordinary"));
    assert!(recompacted
        .keepalives
        .contains_key("after-compaction-typed"));
    assert_eq!(
        recompacted.command_receipts["cmd-after-compaction"].operation_id,
        "op-after-compaction"
    );
    assert_eq!(
        recompacted.responsibility_fences["close-op"].snapshot,
        snapshot
    );
    assert!(recompacted.worker_closures.contains_key(TARGET));
    assert!(!recompacted.workers.contains_key(TARGET));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn close_compaction_preserves_closed_public_owner_details_and_versions() {
    let (server, root) = fence_server();
    server
        .commit_checked(&[
            Event::Registered {
                worker: state::WorkerRec {
                    id: TARGET.into(),
                    token: "token-public".into(),
                    cwd: "/tmp".into(),
                    registered_ms: 1,
                    transport: None,
                },
            },
            Event::TaskCreated {
                task: public_task(TARGET, "public-race", "pending"),
            },
            Event::BoardDetailsChanged {
                task_id: "public-race".into(),
                details: public_details(),
            },
            Event::ResponsibilityFenceSet {
                fence: exact_fence(TARGET, "close-op"),
            },
            Event::WorkerClosed {
                worker_id: TARGET.into(),
                closed_by: "master".into(),
                reason: "public owner finished".into(),
                snapshot_captured_ms: None,
                at_ms: 5,
            },
        ])
        .unwrap();
    let version = server.state.lock().unwrap().sequence;
    assert_eq!(version, 5);
    {
        let state = server.state.lock().unwrap();
        assert_exact_public_details(&state, "public-race");
        assert!(!state.workers.contains_key(TARGET));
        assert!(state.worker_closures.contains_key(TARGET));
    };

    server
        .rewrite_journal_locked(&server.state.lock().unwrap())
        .unwrap();

    let compacted = replay(&root).unwrap();
    assert_eq!(
        (compacted.sequence, compacted.revision),
        (version, version),
        "exact descriptive details must not receive synthetic revisions"
    );
    assert_exact_public_details(&compacted, "public-race");
    assert!(!compacted.workers.contains_key(TARGET));
    assert!(compacted.worker_closures.contains_key(TARGET));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fence_command_batch_is_preflighted_before_command_started() {
    let (server, root) = fence_server();
    fence(&server, TARGET, "close-op");
    let journal_path = root.join(".agent-collab/server/journal.jsonl");
    let before = std::fs::read(&journal_path).unwrap();

    let error = match server.commit_command(
        "cmd-1",
        "op-1",
        &[Event::SubagentUpdated {
            subagent: subagent(TARGET, "child", "assigned"),
        }],
        serde_json::json!({"ok": true}),
    ) {
        Ok(outcome) => panic!("expected rejection, got {outcome:?}"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("target_closing"), "{error}");
    assert_eq!(std::fs::read(&journal_path).unwrap(), before);
    assert!(!server
        .state
        .lock()
        .unwrap()
        .global
        .command_receipts
        .contains_key("cmd-1"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reducer_snapshot_writes_are_rejected_by_both_commit_paths_before_bytes_change() {
    let (server, root) = fence_server();
    let journal_path = root.join(".agent-collab/server/journal.jsonl");
    let snapshot = Event::ReducerSnapshot {
        sequence: 0,
        revision: 0,
        events: Vec::new(),
    };

    let before = std::fs::read(&journal_path).unwrap();
    let sequence_before = server.state.lock().unwrap().sequence;
    let error = server
        .commit_checked(std::slice::from_ref(&snapshot))
        .unwrap_err();
    assert!(error.to_string().contains("replay-only"), "{error}");
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.sequence, sequence_before);
        assert!(state.command_receipts.is_empty());
    }
    assert_eq!(std::fs::read(&journal_path).unwrap(), before);

    let command_error = server
        .commit_command(
            "cmd-snapshot",
            "op-snapshot",
            &[snapshot],
            serde_json::json!({}),
        )
        .unwrap_err();
    assert!(
        command_error.to_string().contains("replay-only"),
        "{command_error}"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.sequence, sequence_before);
    assert!(state.command_receipts.is_empty());
    assert!(state.global.command_receipts.is_empty());
    drop(state);
    assert_eq!(std::fs::read(&journal_path).unwrap(), before);
    std::fs::remove_dir_all(root).unwrap();
}

/// The close operation's own control-plane retirement (a binding whose live
/// endpoint is cleared) stays committable, while a live rebinding of the same
/// agent is refused.
#[test]
fn fence_allows_exact_route_retirement_but_rejects_live_rebinding() {
    let (server, root) = fence_server();
    let project_scope =
        crate::server::global_state::GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = crate::identity::AppServerId::new("tui-default").unwrap();
    let live = crate::server::global_state::RuntimeBinding::new_with_session(
        project_scope.clone(),
        app_scope.clone(),
        crate::identity::AgentId::new(TARGET).unwrap(),
        crate::identity::RuntimeId::new("runtime-target").unwrap(),
        crate::identity::BindingId::new("binding-target").unwrap(),
        1,
        Some(crate::identity::SessionId::new("session-target").unwrap()),
        Some(crate::identity::NativeThreadId::new("thread-target").unwrap()),
    )
    .unwrap();
    server
        .commit_checked(&[Event::GlobalProjectRegistered {
            registration: crate::server::global_state::ProjectRegistration::new(
                project_scope,
                app_scope,
            )
            .unwrap(),
        }])
        .unwrap();
    fence(&server, TARGET, "close-op");

    // The retirement record for the exact generation has no live endpoint.
    let mut retired = live.clone();
    retired.native_thread_id = None;
    retired.tmux_endpoint = None;
    server
        .commit_checked(&[Event::GlobalRuntimeBound { binding: retired }])
        .unwrap();
    assert!(server.state.lock().unwrap().responsibility_fences["close-op"].is_active());

    // A live endpoint for the same agent is a new responsibility.
    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::GlobalRuntimeBound { binding: live }],
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// A `partial` or `unknown` close phase must keep the fence active across
/// replay and keep rejecting new responsibility. B23-F never releases those
/// states; a later lifecycle owner must do so after exact readback.
#[test]
fn partial_and_unknown_phases_stay_fenced_and_block_writers() {
    for phase in ["partial", "unknown"] {
        let (server, root) = fence_server();
        let mut record = exact_fence(TARGET, "close-op");
        record.state = phase.into();
        server
            .commit_checked(&[Event::ResponsibilityFenceSet { fence: record }])
            .unwrap();

        let replayed = replay(&root).unwrap();
        assert!(replayed.responsibility_fences["close-op"].is_active());

        assert_rejected_before_bytes(
            &server,
            &root,
            &[Event::TaskCreated {
                task: task(TARGET, "task-new", "pending"),
            }],
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn ack_reactivating_retired_subscription_is_rejected_before_journal_bytes_change() {
    let (server, root) = fence_server();
    server
        .commit_checked(&[
            Event::NotificationSubscribed {
                subscription: repeating_subscription(TARGET, "sub-retired", "transport-lost", 0, 2),
            },
            Event::Sent {
                msg: delivery_message(TARGET, "msg-retired", "delivered"),
            },
            Event::WakeBound {
                message_id: "msg-retired".into(),
                subscription_id: "sub-retired".into(),
            },
        ])
        .unwrap();
    server
        .commit_checked(&[fence_with_snapshot(&server, TARGET, "close-op")])
        .unwrap();
    let snapshot = server.state.lock().unwrap().responsibility_fences["close-op"]
        .snapshot
        .clone();
    assert!(!snapshot.subscription_ids.contains(&"sub-retired".into()));

    let error = assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::Acked {
            ids: vec!["msg-retired".into()],
        }],
    );
    assert!(error.contains("target_closing"), "{error}");
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["sub-retired"].status,
        "transport-lost"
    );
    assert_eq!(
        state.notification_subscriptions["sub-retired"].fired_count,
        0
    );
    assert_eq!(state.msgs["msg-retired"].state, "delivered");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn receive_committed_reactivating_retired_subscription_is_rejected_before_journal_bytes_change() {
    let (server, root) = fence_server();
    server
        .commit_checked(&[
            Event::NotificationSubscribed {
                subscription: repeating_subscription(TARGET, "sub-retired", "expired", 0, 2),
            },
            Event::Sent {
                msg: delivery_message(TARGET, "msg-retired", "delivered"),
            },
            Event::WakeBound {
                message_id: "msg-retired".into(),
                subscription_id: "sub-retired".into(),
            },
            fence_with_snapshot(&server, TARGET, "close-op"),
        ])
        .unwrap();
    let snapshot = server.state.lock().unwrap().responsibility_fences["close-op"]
        .snapshot
        .clone();
    assert!(!snapshot.subscription_ids.contains(&"sub-retired".into()));

    let error = assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::ReceiveCommitted {
            receipt: state::ReceiveReceipt {
                receive_id: "recv-1".into(),
                worker_id: TARGET.into(),
                route_scope: None,
                message_ids: vec!["msg-retired".into()],
                received_ms: 2,
            },
            ids: vec!["msg-retired".into()],
        }],
    );
    assert!(error.contains("target_closing"), "{error}");
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["sub-retired"].status,
        "expired"
    );
    assert_eq!(
        state.notification_subscriptions["sub-retired"].fired_count,
        0
    );
    assert_eq!(state.msgs["msg-retired"].state, "delivered");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn notification_consumed_reactivating_retired_subscription_is_rejected() {
    let (server, root) = fence_server();
    server
        .commit_checked(&[
            Event::NotificationSubscribed {
                subscription: repeating_subscription(TARGET, "sub-retired", "transport-lost", 0, 2),
            },
            Event::Sent {
                msg: delivery_message(TARGET, "msg-retired", "delivered"),
            },
            Event::WakeBound {
                message_id: "msg-retired".into(),
                subscription_id: "sub-retired".into(),
            },
            fence_with_snapshot(&server, TARGET, "close-op"),
        ])
        .unwrap();

    assert_rejected_before_bytes(
        &server,
        &root,
        &[Event::NotificationConsumed {
            subscription_id: "sub-retired".into(),
            message_id: "msg-retired".into(),
            consumed_ms: 2,
        }],
    );
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["sub-retired"].status,
        "transport-lost"
    );
    assert_eq!(
        state.notification_subscriptions["sub-retired"].fired_count,
        0
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ack_consuming_one_shot_or_counted_subscription_stays_allowed() {
    let (server, root) = fence_server();
    server
        .commit_checked(&[
            Event::NotificationSubscribed {
                subscription: repeating_subscription(TARGET, "sub-counted", "armed", 0, 2),
            },
            Event::Sent {
                msg: delivery_message(TARGET, "msg-counted", "delivered"),
            },
            Event::WakeBound {
                message_id: "msg-counted".into(),
                subscription_id: "sub-counted".into(),
            },
            Event::NotificationSubscribed {
                subscription: repeating_subscription(TARGET, "sub-one-shot", "armed", 0, 1),
            },
            Event::Sent {
                msg: delivery_message(TARGET, "msg-one-shot", "delivered"),
            },
            Event::WakeBound {
                message_id: "msg-one-shot".into(),
                subscription_id: "sub-one-shot".into(),
            },
        ])
        .unwrap();
    let snapshot = state::responsibility_snapshot(&server.state.lock().unwrap(), TARGET);
    assert!(snapshot.subscription_ids.contains(&"sub-counted".into()));
    assert!(snapshot.subscription_ids.contains(&"sub-one-shot".into()));
    server
        .commit_checked(&[Event::ResponsibilityFenceSet {
            fence: state::ResponsibilityFence {
                snapshot,
                ..exact_fence(TARGET, "close-op")
            },
        }])
        .unwrap();

    server
        .commit_checked(&[Event::Acked {
            ids: vec!["msg-counted".into(), "msg-one-shot".into()],
        }])
        .unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["sub-counted"].status,
        "armed"
    );
    assert_eq!(
        state.notification_subscriptions["sub-counted"].fired_count,
        1
    );
    assert_eq!(
        state.notification_subscriptions["sub-one-shot"].status,
        "consumed"
    );
    assert_eq!(
        state.notification_subscriptions["sub-one-shot"].fired_count,
        1
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ack_read_count_at_or_below_fired_count_does_not_reactivate_retired_subscription() {
    // After acknowledging the one unread message, the durable read count is
    // `read_count + 1`. Keep it at the cursor (`fired_count`) so the reducer
    // and preflight agree that no new read occurrence needs consuming.
    for (fired_count, read_count) in [(2, 1), (1, 0)] {
        let (server, root) = fence_server();
        let mut events = vec![
            Event::NotificationSubscribed {
                subscription: repeating_subscription(
                    TARGET,
                    "sub-retired",
                    "expired",
                    fired_count,
                    3,
                ),
            },
            Event::Sent {
                msg: delivery_message(TARGET, "msg-unread", "delivered"),
            },
            Event::WakeBound {
                message_id: "msg-unread".into(),
                subscription_id: "sub-retired".into(),
            },
        ];
        if read_count > 0 {
            events.splice(
                1..1,
                [
                    Event::Sent {
                        msg: delivery_message(TARGET, "msg-read", "read"),
                    },
                    Event::WakeBound {
                        message_id: "msg-read".into(),
                        subscription_id: "sub-retired".into(),
                    },
                ],
            );
        }
        server.commit_checked(&events).unwrap();
        server
            .commit_checked(&[fence_with_snapshot(&server, TARGET, "close-op")])
            .unwrap();

        server
            .commit_checked(&[Event::Acked {
                ids: vec!["msg-unread".into()],
            }])
            .unwrap();
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.notification_subscriptions["sub-retired"].status, "expired",
            "fired_count={fired_count}, read_count={read_count}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn ack_reactivation_followed_by_same_batch_retirement_is_allowed() {
    let (server, root) = fence_server();
    server
        .commit_checked(&[
            Event::NotificationSubscribed {
                subscription: repeating_subscription(TARGET, "sub-retired", "transport-lost", 0, 2),
            },
            Event::Sent {
                msg: delivery_message(TARGET, "msg-retired", "delivered"),
            },
            Event::WakeBound {
                message_id: "msg-retired".into(),
                subscription_id: "sub-retired".into(),
            },
            fence_with_snapshot(&server, TARGET, "close-op"),
        ])
        .unwrap();

    server
        .commit_checked(&[
            Event::Acked {
                ids: vec!["msg-retired".into()],
            },
            Event::NotificationStatus {
                subscription_id: "sub-retired".into(),
                status: "cancelled".into(),
                updated_ms: 2,
            },
        ])
        .unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["sub-retired"].status,
        "cancelled"
    );
    assert_eq!(state.msgs["msg-retired"].state, "read");
    std::fs::remove_dir_all(root).unwrap();
}
