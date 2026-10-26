use super::*;
use crate::server::peer_tests::test_server;

include!("board_name_tests.rs");
include!("board_reassign_tests.rs");

/// Register a state-level worker without a transport fixture. These tests
/// cover board projection and replay, not transport selection.
fn register_worker(state: &mut State, id: &str) {
    state.workers.insert(
        id.into(),
        crate::server::state::WorkerRec {
            id: id.into(),
            token: format!("token-{id}"),
            cwd: "/tmp/board-state-fixture".into(),
            registered_ms: now_ms(),
            transport: None,
        },
    );
}

fn register_managed_child(state: &mut State, id: &str, parent: &str, peer: &str) {
    state
        .apply_checked(&Event::SubagentUpdated {
            subagent: crate::subagent::Record {
                id: id.into(),
                parent: parent.into(),
                peer: peer.into(),
                status: "idle".into(),
                thread_id: Some(format!("thread-{peer}")),
                profile: None,
                created_ms: now_ms(),
                ready_deadline_ms: now_ms() + 60_000,
                last_message: None,
                error: None,
                probe_failures: Vec::new(),
                runtime: Some("codex".into()),
                create_operation_id: None,
                binding_id: None,
                endpoint_generation: None,
            },
        })
        .unwrap();
}

fn task(id: &str, owner: &str) -> TaskRec {
    TaskRec {
        id: id.into(),
        owner: owner.into(),
        created_by: owner.into(),
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
    }
}

#[test]
fn board_lists_only_open_tasks_without_deleting_history() {
    let (server, root) = test_server();
    let statuses = [
        "pending",
        "working",
        "blocked",
        "merged",
        "closed",
        "cancelled",
    ];
    {
        let mut state = server.state.lock().unwrap();
        register_worker(&mut state, "peer");
        for status in statuses {
            let mut record = task(status, "peer");
            record.status = status.into();
            state
                .apply_checked(&Event::TaskCreated { task: record })
                .unwrap();
        }
    }
    for viewer in [None, Some("peer")] {
        let response = handle_board_show(&server, viewer);
        assert!(response.ok, "{response:?}");
        let ids: Vec<_> = response.data["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|task| task["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["blocked", "merged", "pending", "working"]);
    }
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks.len(), statuses.len());
    assert_eq!(
        board_task_view(&state, &state.tasks["closed"])
            .unwrap()
            .status,
        "closed"
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compact_snapshot_preserves_public_history_and_exact_revision() {
    let (server, root) = test_server();
    let mut state = server.state.lock().unwrap();
    register_worker(&mut state, "peer");
    let mut task = task("public-history", "peer");
    state
        .apply_checked(&Event::TaskCreated { task: task.clone() })
        .unwrap();
    for next in ["实施", "验证", "交付"] {
        task.next_step = Some(next.into());
        state
            .apply_checked(&Event::TaskUpdated { task: task.clone() })
            .unwrap();
    }
    state
        .apply_checked(&Event::WorkerClosed {
            worker_id: "peer".into(),
            closed_by: "peer".into(),
            reason: "finished fixture".into(),
            snapshot_captured_ms: None,
            at_ms: now_ms(),
        })
        .unwrap();
    assert!(!state.workers.contains_key("peer"));
    assert_eq!(board_task_view(&state, &task).unwrap().revision, 4);
    let events = state.snapshot_events();
    let mut restored = State::default();
    for event in events {
        restored.apply_checked(&event).unwrap();
    }
    let view = board_task_view(&restored, &restored.tasks["public-history"]).unwrap();
    assert_eq!(view.revision, 4);
    assert_eq!(view.owner, "");
    assert_eq!(view.owner_display_name, "");
    assert_eq!(restored.tasks["public-history"].owner, "peer");
    let mut closed = restored.tasks["public-history"].clone();
    closed.status = "closed".into();
    assert_eq!(board_task_view(&restored, &closed).unwrap().owner, "peer");
    assert_eq!(view.next_step.as_deref(), Some("交付"));
    assert!(!restored.workers.contains_key("peer"));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unknown_legacy_actor_is_not_public_after_compaction() {
    let mut state = State::default();
    let task = task("unknown-history", "unknown-actor");
    state
        .apply_checked(&Event::TaskCreated { task: task.clone() })
        .unwrap();
    assert!(board_task_view(&state, &task).is_none());
    let mut restored = State::default();
    for event in state.snapshot_events() {
        restored.apply_checked(&event).unwrap();
    }
    assert!(board_task_view(&restored, &restored.tasks["unknown-history"]).is_none());
}

#[test]
fn compact_snapshot_preserves_invitation_and_its_capacity_reservation() {
    let (server, root) = test_server();
    let mut state = server.state.lock().unwrap();
    register_worker(&mut state, "publisher");
    register_worker(&mut state, "peer");
    let mut task = task("offer", "publisher");
    task.status = "invited".into();
    state.apply_checked(&Event::TaskCreated { task }).unwrap();
    state
        .apply_checked(&Event::BoardDetailsChanged {
            task_id: "offer".into(),
            details: crate::board::BoardTaskDetails {
                revision: 17,
                public_visibility: true,
                invitation: Some(crate::board::BoardInvitation {
                    peer_id: "peer".into(),
                    binding_id: "test-binding".into(),
                    endpoint_generation: 8,
                    message_id: "offer-message".into(),
                    created_ms: now_ms(),
                    original_status: None,
                    original_owner: None,
                }),
                ..crate::board::BoardTaskDetails::default()
            },
        })
        .unwrap();
    assert!(board_peer_has_execution_reservation(&state, "peer", None));
    assert!(!keepalive::actionable("invited"));
    assert!(!task_resource_active("invited"));
    let mut restored = State::default();
    for event in state.snapshot_events() {
        restored.apply_checked(&event).unwrap();
    }
    assert_eq!(restored.board_details["offer"].revision, 17);
    assert_eq!(
        restored.board_details["offer"]
            .invitation
            .as_ref()
            .unwrap()
            .endpoint_generation,
        8
    );
    assert_eq!(restored.tasks["offer"].owner, "publisher");
    assert!(board_peer_has_execution_reservation(
        &restored, "peer", None
    ));
    assert!(board_check_revision(&restored, "offer", 16).is_err());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn blocked_and_waiting_keep_responsibility_without_consuming_the_next_execution_slot() {
    let mut state = State::default();
    register_worker(&mut state, "peer");
    let mut blocked = task("blocked", "peer");
    blocked.status = "blocked".into();
    let mut waiting = task("waiting", "peer");
    waiting.status = "waiting".into();
    state
        .apply_checked(&Event::TaskCreated { task: blocked })
        .unwrap();
    state
        .apply_checked(&Event::TaskCreated { task: waiting })
        .unwrap();
    assert!(keepalive::unfinished("blocked"));
    assert!(keepalive::unfinished("waiting"));
    assert!(!board_peer_has_execution_reservation(&state, "peer", None));
}

#[test]
fn managed_binding_does_not_hide_a_task_published_for_public_work() {
    let mut state = State::default();
    register_worker(&mut state, "parent");
    register_worker(&mut state, "child-peer");
    register_managed_child(&mut state, "child", "parent", "child-peer");
    let mut public = task("public-claimed-by-child", "child-peer");
    public.created_by = "parent".into();
    state
        .apply_checked(&Event::TaskCreated { task: public })
        .unwrap();
    state
        .apply_checked(&Event::BoardDetailsChanged {
            task_id: "public-claimed-by-child".into(),
            details: crate::board::BoardTaskDetails {
                title: "公开任务".into(),
                revision: 2,
                public_visibility: true,
                ..crate::board::BoardTaskDetails::default()
            },
        })
        .unwrap();
    let record = state.subagents["child"].clone();
    state
        .apply_checked(&Event::SubagentUpdated { subagent: record })
        .unwrap();
    assert!(state.board_details["public-claimed-by-child"].public_visibility);
}

#[test]
fn authenticated_managed_queue_is_parent_visible_while_public_output_hides_child_identity() {
    let (server, root) = test_server();
    crate::server::peer_tests::register_synthetic_peer(&server, "parent", "parent-thread");
    crate::server::peer_tests::register_synthetic_peer(&server, "child-peer", "child-thread");
    crate::server::peer_tests::register_synthetic_peer(&server, "other-peer", "other-thread");
    let mut state = server.state.lock().unwrap();
    register_managed_child(&mut state, "child", "parent", "child-peer");
    state
        .apply_checked(&Event::WorkerNamed {
            worker_id: "child-peer".into(),
            name: Some("Private child name".into()),
        })
        .unwrap();
    state
        .apply_checked(&Event::WorkerNamed {
            worker_id: "parent".into(),
            name: Some("Parent name".into()),
        })
        .unwrap();

    let mut public = task("public-claimed-by-child", "parent");
    public.created_by = "parent".into();
    state
        .apply_checked(&Event::TaskCreated {
            task: public.clone(),
        })
        .unwrap();
    state
        .apply_checked(&Event::BoardDetailsChanged {
            task_id: public.id.clone(),
            details: crate::board::BoardTaskDetails {
                title: "公开任务".into(),
                revision: 1,
                public_visibility: true,
                ..crate::board::BoardTaskDetails::default()
            },
        })
        .unwrap();
    public.owner = "child-peer".into();
    public.status = "working".into();
    state
        .apply_checked(&Event::TaskUpdated { task: public })
        .unwrap();

    let private = task("child-private", "child-peer");
    state
        .apply_checked(&Event::TaskCreated {
            task: private.clone(),
        })
        .unwrap();
    assert!(board_task_view(&state, &state.tasks[&private.id]).is_none());
    assert!(board_task_view_for(&state, &state.tasks[&private.id], Some("parent")).is_some());
    assert!(board_task_view_for(&state, &state.tasks[&private.id], Some("child-peer")).is_some());
    assert!(board_task_view_for(&state, &state.tasks[&private.id], Some("other-peer")).is_none());

    let public_view = board_task_view(&state, &state.tasks["public-claimed-by-child"]).unwrap();
    assert_eq!(public_view.owner, "parent");
    assert_eq!(public_view.publisher, "parent");
    assert!(!serde_json::to_string(&public_view)
        .unwrap()
        .contains("child-peer"));
    assert_eq!(public_view.owner_display_name, "Alice");
    assert_ne!(
        public_view.owner_display_name,
        worker_display_name(&state, "child-peer", false)
    );
    let child_name = worker_display_name(&state, "child-peer", false);
    assert!(!serde_json::to_string(&public_view)
        .unwrap()
        .contains(&serde_json::to_string(&child_name).unwrap()));
    assert!(!serde_json::to_string(&public_view)
        .unwrap()
        .contains("Private child name"));
    let private_parent =
        board_task_view_for(&state, &state.tasks["child-private"], Some("parent")).unwrap();
    assert_eq!(private_parent.owner, "child-peer");
    assert_eq!(private_parent.publisher, "child-peer");

    let mut restored = State::default();
    for event in state.snapshot_events() {
        restored.apply_checked(&event).unwrap();
    }
    assert_eq!(
        restored.tasks["public-claimed-by-child"].owner,
        "child-peer"
    );
    assert!(restored.board_details["public-claimed-by-child"].public_visibility);
    assert_eq!(
        board_task_view(&restored, &restored.tasks["public-claimed-by-child"])
            .unwrap()
            .owner,
        "parent"
    );
    assert!(!restored.board_details["child-private"].public_visibility);
    assert!(
        board_task_view_for(&restored, &restored.tasks["child-private"], Some("parent")).is_some()
    );
    assert!(board_task_view(&restored, &restored.tasks["child-private"]).is_none());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn name_overrides_survive_compaction_but_reset_and_retirement_remove_them() {
    let mut state = State::default();
    register_worker(&mut state, "peer");
    state
        .apply_checked(&Event::WorkerNamed {
            worker_id: "peer".into(),
            name: Some("Alice".into()),
        })
        .unwrap();
    let restore = |state: &State| {
        let mut restored = State::default();
        for event in state.snapshot_events() {
            restored.apply_checked(&event).unwrap();
        }
        restored
    };
    let mut state = restore(&state);
    assert_eq!(worker_display_name(&state, "peer", true), "Alice");
    state
        .apply_checked(&Event::WorkerNamed {
            worker_id: "peer".into(),
            name: None,
        })
        .unwrap();
    let mut state = restore(&state);
    assert_eq!(worker_display_name(&state, "peer", true), "Jenny");
    state
        .apply_checked(&Event::WorkerNamed {
            worker_id: "peer".into(),
            name: Some("Alice".into()),
        })
        .unwrap();
    state
        .apply_checked(&Event::WorkerClosed {
            worker_id: "peer".into(),
            closed_by: "peer".into(),
            reason: "user unregister".into(),
            snapshot_captured_ms: None,
            at_ms: now_ms(),
        })
        .unwrap();
    let mut state = restore(&state);
    register_worker(&mut state, "peer");
    assert_eq!(worker_display_name(&state, "peer", false), "peer");
}

/// A task that has not reached delivery carries no delivery/review/integration
/// section. Those keys are lifecycle evidence, so an empty default renders as
/// nothing instead of a full block of nulls and `pending: false`.
#[test]
fn task_view_omits_lifecycle_sections_that_hold_no_evidence() {
    let mut state = State::default();
    register_worker(&mut state, "peer");
    let open = task("open-work", "peer");
    state
        .apply_checked(&Event::TaskCreated { task: open.clone() })
        .unwrap();

    let view = task_view(&state, &state.tasks["open-work"]);
    assert_eq!(view["id"], "open-work");
    assert_eq!(view["owner"], "peer");
    assert_eq!(view["status"], "working");
    assert_eq!(view["completion_missing"], json!(["review"]));
    for empty_section in [
        "delivery",
        "review",
        "integration",
        "disposition",
        "merge",
        "cleanup",
    ] {
        assert!(
            view.get(empty_section).is_none(),
            "a task without that evidence must not project an empty {empty_section} section: {view}"
        );
    }
}

/// The facts an actor needs for the next step survive the trimmed projection:
/// a pending merge, the pinned reviewer, and cleanup that is still outstanding.
#[test]
fn task_view_keeps_pending_merge_pinned_reviewer_and_outstanding_cleanup() {
    let (server, root) = test_server();
    let mut state = server.state.lock().unwrap();
    register_worker(&mut state, "peer");
    let mut delivered = task("delivered-work", "peer");
    delivered.worktree_path = Some("/tmp/delivered-work".into());
    delivered.branch = Some("codex/delivered-work".into());
    state
        .apply_checked(&Event::TaskCreated { task: delivered })
        .unwrap();
    state
        .apply_checked(&Event::BoardDetailsChanged {
            task_id: "delivered-work".into(),
            details: crate::board::BoardTaskDetails {
                revision: 2,
                public_visibility: true,
                work_policy: crate::board::WorkPolicy {
                    reviewer_name: Some("Bob".into()),
                    ..crate::board::WorkPolicy::default()
                },
                ..crate::board::BoardTaskDetails::default()
            },
        })
        .unwrap();

    let pending_reviewer = task_view(&state, &state.tasks["delivered-work"]);
    assert_eq!(
        pending_reviewer["review"]["required_reviewer_name"], "Bob",
        "the pinned reviewer is a next-step fact even before delivery: {pending_reviewer}"
    );
    assert!(
        pending_reviewer.get("delivery").is_none(),
        "{pending_reviewer}"
    );
    assert!(
        pending_reviewer.get("integration").is_none(),
        "{pending_reviewer}"
    );

    let mut delivered = state.tasks["delivered-work"].clone();
    delivered.status = "accepted".into();
    state
        .apply_checked(&Event::TaskUpdated { task: delivered })
        .unwrap();
    let record = crate::server::state::TaskLifecycleRecord {
        delivery_kind: crate::server::state::TaskDeliveryKind::Code,
        delivery_evidence: Some("self-check and black-box regression".into()),
        delivered_ms: Some(now_ms()),
        ..crate::server::state::TaskLifecycleRecord::default()
    };
    state
        .apply_checked(&Event::TaskLifecycleUpdated {
            task_id: "delivered-work".into(),
            record,
        })
        .unwrap();
    state
        .apply_checked(&Event::MergeRequested {
            request: crate::server::state::PendingMerge {
                task_id: "delivered-work".into(),
                owner: "peer".into(),
                requested_by: "peer".into(),
                requested_ms: now_ms(),
                candidate_commit: Some("candidate-commit".into()),
            },
        })
        .unwrap();

    let view = task_view(&state, &state.tasks["delivered-work"]);
    assert_eq!(
        view["delivery"]["evidence"], "self-check and black-box regression",
        "{view}"
    );
    assert_eq!(view["disposition"], "integration_required", "{view}");
    assert_eq!(view["merge"]["pending"], true, "{view}");
    assert_eq!(view["merge"]["owner"], "peer", "{view}");
    assert_eq!(
        view["cleanup"]["status"], "pending",
        "outstanding cleanup stays visible: {view}"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}
