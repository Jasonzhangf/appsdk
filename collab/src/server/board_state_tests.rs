use super::*;
use crate::server::peer_tests::test_server;

include!("board_name_tests.rs");
include!("board_reassign_tests.rs");

/// Register a state-level worker without a transport fixture. These tests
/// cover board projection and current state, not transport selection.
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

fn described_owner_task(
    id: &str,
    owner: &str,
    status: &str,
) -> (TaskRec, crate::board::BoardTaskDetails) {
    let task = TaskRec {
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
        isolation_mode: None,
        created_ms: now_ms(),
        updated_ms: now_ms(),
    };
    let details = crate::board::BoardTaskDetails {
        title: "Legacy state ticket".into(),
        description: "state reducer legacy admission".into(),
        delivery_condition: "admission creates one claimed phase".into(),
        test_condition: "state reducer admission test".into(),
        public_visibility: true,
        admitted: false,
        revision: 1,
        ..crate::board::BoardTaskDetails::default()
    };
    (task, details)
}

#[test]
fn legacy_admission_rejects_any_phase_history_before_mutation() {
    use crate::server::ticket_state::{TicketPhaseKind, TicketPhaseRecord, TicketPhaseStatus};
    let mut state = State::default();
    let (task, details) = described_owner_task("legacy-state-phase", "owner", "rework");
    state
        .apply_checked(&Event::TaskCreated { task: task.clone() })
        .unwrap();
    state
        .apply_checked(&Event::BoardDetailsChanged {
            task_id: task.id.clone(),
            details: details.clone(),
        })
        .unwrap();
    // The described rework task is eligible while it has no phase history.
    assert!(super::legacy_admission_eligibility(&state, &details, &task).is_ok());
    // Any phase record, including revoked history, blocks admission.
    state
        .apply_checked(&Event::TicketPhaseRecorded {
            phase: TicketPhaseRecord {
                task_id: task.id.clone(),
                phase: TicketPhaseKind::Execute,
                batch: 0,
                priority: "p2".into(),
                status: TicketPhaseStatus::Revoked,
                slot_name: None,
                binding_generation: 0,
                enqueue_seq: 1,
                attempt_generation: 0,
                depends_on: Vec::new(),
                route_group: None,
                wait_reason: None,
                result: None,
            },
        })
        .unwrap();
    let rejection = super::legacy_admission_eligibility(&state, &details, &task).unwrap_err();
    assert!(
        rejection
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("TICKET_ADMISSION_PHASE_CONFLICT"),
        "{rejection:?}"
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
        isolation_mode: None,
        created_ms: now_ms(),
        updated_ms: now_ms(),
    }
}

#[test]
fn reviewer_rebind_keeps_delivered_task_stable_and_updates_next_step() {
    let (server, root) = test_server();
    {
        let mut state = server.state.lock().unwrap();
        register_worker(&mut state, "author");
        register_worker(&mut state, "master");
        let mut delivered = task("reviewer-rebind", "author");
        delivered.created_by = "master".into();
        delivered.status = "delivered".into();
        delivered.next_step =
            Some("await review by Bob; use collab task review --accept or --rework".into());
        state
            .apply_checked(&Event::TaskCreated { task: delivered })
            .unwrap();
        state
            .board_details
            .get_mut("reviewer-rebind")
            .unwrap()
            .work_policy
            .reviewer_name = Some("Bob".into());
        state.task_lifecycle.insert(
            "reviewer-rebind".into(),
            state::TaskLifecycleRecord {
                required_reviewer_name: Some("Bob".into()),
                delivery_kind: state::TaskDeliveryKind::ReadOnly,
                delivery_evidence: Some("delivery evidence".into()),
                delivered_ms: Some(now_ms()),
                review_subject_revision: 2,
                ..Default::default()
            },
        );
    }

    let patch = crate::board::WorkPolicy {
        reviewer_name: Some("Carol".into()),
        ..Default::default()
    };
    let response = board_work_policy(
        &server,
        "author".into(),
        "token-author".into(),
        "task".into(),
        Some("reviewer-rebind".into()),
        patch,
        Some(1),
    );
    assert!(response.ok, "{:?}", response.error);

    let state = server.state.lock().unwrap();
    let task = &state.tasks["reviewer-rebind"];
    assert_eq!(task.status, "delivered");
    assert_eq!(
        task.next_step.as_deref(),
        Some("await review by Carol; use collab task review --accept or --rework")
    );
    let lifecycle = &state.task_lifecycle["reviewer-rebind"];
    assert_eq!(lifecycle.required_reviewer_name.as_deref(), Some("Carol"));
    assert!(lifecycle.reviewer.is_none());
    assert!(lifecycle.review_evidence.is_none());
    assert_eq!(lifecycle.review_subject_revision, 3);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_lists_only_open_tasks_without_deleting_history() {
    let (server, root) = test_server();
    let control = binding_slot::BindingControlSet::default();
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
        board_task_view(&state, &control, &state.tasks["closed"])
            .unwrap()
            .unwrap()
            .status,
        "closed"
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_state_preserves_public_history_and_exact_revision() {
    let (server, root) = test_server();
    let control = binding_slot::BindingControlSet::default();
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
    assert_eq!(
        board_task_view(&state, &control, &task)
            .unwrap()
            .unwrap()
            .revision,
        4
    );
    let restored: State = serde_json::from_slice(&serde_json::to_vec(&*state).unwrap()).unwrap();
    let view = board_task_view(&restored, &control, &restored.tasks["public-history"])
        .unwrap()
        .unwrap();
    assert_eq!(view.revision, 4);
    assert_eq!(view.owner, "");
    assert_eq!(view.owner_display_name, "");
    assert_eq!(restored.tasks["public-history"].owner, "peer");
    let mut closed = restored.tasks["public-history"].clone();
    closed.status = "closed".into();
    assert_eq!(
        board_task_view(&restored, &control, &closed)
            .unwrap()
            .unwrap()
            .owner,
        "peer"
    );
    assert_eq!(view.next_step.as_deref(), Some("交付"));
    assert!(!restored.workers.contains_key("peer"));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unknown_actor_is_not_public_after_current_state_round_trip() {
    let mut state = State::default();
    let control = binding_slot::BindingControlSet::default();
    let task = task("unknown-history", "unknown-actor");
    state
        .apply_checked(&Event::TaskCreated { task: task.clone() })
        .unwrap();
    assert!(board_task_view(&state, &control, &task).unwrap().is_none());
    let restored: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert!(
        board_task_view(&restored, &control, &restored.tasks["unknown-history"])
            .unwrap()
            .is_none()
    );
}

#[test]
fn current_state_preserves_invitation_and_its_capacity_reservation() {
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
                    message_id: "offer-message".into(),
                    created_ms: now_ms(),
                    original_status: None,
                    original_owner: None,
                    automatic_source: None,
                }),
                ..crate::board::BoardTaskDetails::default()
            },
        })
        .unwrap();
    assert!(board_peer_has_execution_reservation(&state, "peer", None));
    assert!(!keepalive::actionable("invited"));
    assert!(!task_resource_active("invited"));
    let restored: State = serde_json::from_slice(&serde_json::to_vec(&*state).unwrap()).unwrap();
    assert_eq!(restored.board_details["offer"].revision, 17);
    assert_eq!(
        restored.board_details["offer"]
            .invitation
            .as_ref()
            .unwrap()
            .binding_id,
        "test-binding"
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
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let route = control
        .active_values()
        .find(|value| value.binding.agent_id.as_str() == "parent")
        .unwrap()
        .binding
        .route_scope();
    let mut state = server.state.lock().unwrap();
    register_managed_child(&mut state, "child", "parent", "child-peer");

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
    assert!(board_task_view(&state, &control, &state.tasks[&private.id])
        .unwrap()
        .is_none());
    assert!(
        board_task_view_for(&state, &control, &state.tasks[&private.id], Some("parent"))
            .unwrap()
            .is_some()
    );
    assert!(board_task_view_for(
        &state,
        &control,
        &state.tasks[&private.id],
        Some("child-peer")
    )
    .unwrap()
    .is_some());
    assert!(board_task_view_for(
        &state,
        &control,
        &state.tasks[&private.id],
        Some("other-peer")
    )
    .unwrap()
    .is_none());

    let public_view = board_task_view(&state, &control, &state.tasks["public-claimed-by-child"])
        .unwrap()
        .unwrap();
    assert_eq!(public_view.owner, "parent");
    assert_eq!(public_view.publisher, "parent");
    assert!(!serde_json::to_string(&public_view)
        .unwrap()
        .contains("child-peer"));
    assert_eq!(public_view.owner_display_name, "Alice");
    assert_ne!(
        public_view.owner_display_name,
        worker_display_name(&control, Some(&route), "child-peer").unwrap()
    );
    let child_name = worker_display_name(&control, Some(&route), "child-peer").unwrap();
    assert!(!serde_json::to_string(&public_view)
        .unwrap()
        .contains(&serde_json::to_string(&child_name).unwrap()));
    let private_parent = board_task_view_for(
        &state,
        &control,
        &state.tasks["child-private"],
        Some("parent"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(private_parent.owner, "child-peer");
    assert_eq!(private_parent.publisher, "child-peer");

    let restored: State = serde_json::from_slice(&serde_json::to_vec(&*state).unwrap()).unwrap();
    assert_eq!(
        restored.tasks["public-claimed-by-child"].owner,
        "child-peer"
    );
    assert!(restored.board_details["public-claimed-by-child"].public_visibility);
    assert_eq!(
        board_task_view(
            &restored,
            &control,
            &restored.tasks["public-claimed-by-child"]
        )
        .unwrap()
        .unwrap()
        .owner,
        "parent"
    );
    assert!(!restored.board_details["child-private"].public_visibility);
    assert!(board_task_view_for(
        &restored,
        &control,
        &restored.tasks["child-private"],
        Some("parent")
    )
    .unwrap()
    .is_some());
    assert!(
        board_task_view(&restored, &control, &restored.tasks["child-private"])
            .unwrap()
            .is_none()
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
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

#[test]
fn internal_safety_blocked_writes_never_create_business_approval_records() {
    let (server, root) = test_server();
    let mut state = server.state.lock().unwrap();
    register_worker(&mut state, "owner");
    register_worker(&mut state, "dep");
    // Resource-conflict creation (part_08) emits `TaskCreated` directly in the
    // blocked state for safety isolation.
    let mut conflict = task("resource-conflict", "owner");
    conflict.status = "blocked".into();
    conflict.next_step = Some("RESOURCE_CONFLICT=base".into());
    state
        .apply_checked(&Event::TaskCreated { task: conflict })
        .unwrap();
    // Dependency release (part_09) rewrites a waiting dependent directly to
    // blocked; it must stay a safety state, not an approved business blocker.
    let mut base = task("base", "owner");
    base.status = "closed".into();
    state
        .apply_checked(&Event::TaskCreated { task: base })
        .unwrap();
    let mut waiter = task("waiter", "dep");
    waiter.status = "waiting".into();
    waiter.wait = Some(WaitSpec {
        waiter: "dep".into(),
        waiting_for: "base".into(),
        responsible_actor: "base".into(),
        reason: "resource depends on base".into(),
        deadline_ms: now_ms() + 60_000,
        resume_on: Vec::new(),
        escalation: "master".into(),
    });
    state
        .apply_checked(&Event::TaskCreated { task: waiter })
        .unwrap();
    release_dependents_of_closed_task(&server, &mut state, "base");
    assert_eq!(state.tasks["waiter"].status, "blocked");
    assert_eq!(state.tasks["resource-conflict"].status, "blocked");
    assert!(
        state.task_block_requests.is_empty(),
        "internal resource/safety isolation must never fabricate an approved business block"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn pending_block_request_survives_current_state_round_trip() {
    let request = TaskBlockRequest {
        task_id: "task-block".into(),
        requester: "owner".into(),
        reason: Some("external dependency".into()),
        requested_at_ms: 10,
        task_revision: 4,
        state: TaskBlockRequestState::Pending,
        decided_by: None,
        decided_at_ms: None,
        decision_reason: None,
    };
    let mut state = State::default();
    state
        .apply_checked(&Event::TaskBlockRequestUpdated {
            request: request.clone(),
        })
        .unwrap();

    let restored: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();

    assert_eq!(restored.task_block_requests["task-block"], request);
}
