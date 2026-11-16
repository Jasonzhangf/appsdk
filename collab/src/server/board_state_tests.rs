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

fn state_revision(server: &Server, task_id: &str) -> u64 {
    server.state.lock().unwrap().board_details[task_id].revision
}

fn goal_withdraw_fixture(
    status: &str,
    expires_ms: i64,
    fired_count: u32,
) -> (Server, std::path::PathBuf, String, String) {
    let (server, root, route) = board_name_fixture();
    let response = crate::server::peer_tests::register_synthetic_peer(&server, "master", "master");
    assert!(response.ok, "{response:?}");
    crate::server::peer_tests::promote_master(&server, "master", "user-approved");

    let mut state = server.state.lock().unwrap();
    let details = state.board_details.get_mut("named-work").unwrap();
    details.admitted = true;
    details.long_horizon = Some(crate::board::LongHorizonTaskSpec {
        content: "continue existing handoff".into(),
        interval_ms: 1_200_000,
        repeat_count: 3,
        ttl_seconds: 172_800,
    });
    let task = state.tasks["named-work"].clone();
    let details = state.board_details["named-work"].clone();
    let (schedule_events, _) = task_goal_events(
        &server,
        &state,
        &task,
        &details,
        Some("old"),
        false,
        GoalScheduleOperation::Derived,
        None,
    )
    .unwrap();
    for event in schedule_events {
        state.apply_checked(&event).unwrap();
    }
    let subscription_id = state
        .notification_subscriptions
        .values()
        .find(|subscription| subscription.subject.as_deref() == Some("goal:task:named-work"))
        .expect("task-linked goal schedule")
        .id
        .clone();
    let subscription = state
        .notification_subscriptions
        .get_mut(&subscription_id)
        .unwrap();
    subscription.status = status.into();
    subscription.expires_ms = expires_ms;
    subscription.fired_count = fired_count;
    subscription.updated_ms = now_ms();

    let binding = authoritative_send_binding(&state, &route, "current")
        .unwrap()
        .clone();
    let invite_message_id = "withdraw-invite-message".to_owned();
    state
        .board_details
        .get_mut("named-work")
        .unwrap()
        .invitation = Some(crate::board::BoardInvitation {
        peer_id: "current".into(),
        binding_id: binding.binding_id.as_str().into(),
        message_id: invite_message_id.clone(),
        created_ms: now_ms(),
        original_status: Some("working".into()),
        original_owner: Some("old".into()),
        automatic_source: None,
    });
    state
        .apply_checked(&Event::Sent {
            msg: Message {
                id: invite_message_id.clone(),
                from: "master".into(),
                to: "current".into(),
                mtype: "request".into(),
                subject: Some("task-invitation:named-work".into()),
                body: "stale invitation".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        })
        .unwrap();
    let direct_message = Message {
        id: "other-goal-message".to_owned(),
        from: "scheduler".into(),
        to: "other".into(),
        mtype: "notify".into(),
        subject: Some("goal:other-capacity-goal".into()),
        body: "preserve other goal".into(),
        in_reply_to: None,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    state
        .apply_checked(&Event::Sent {
            msg: direct_message,
        })
        .unwrap();
    state
        .apply_checked(&Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: "other-goal-subscription".into(),
                worker_id: "other".into(),
                event: "deadline".into(),
                subject: Some("goal:other-capacity-goal".into()),
                target: "other-target".into(),
                method: "appserver".into(),
                trigger_ms: Some(now_ms() + 60_000),
                trigger_times_ms: Vec::new(),
                interval_ms: Some(60_000),
                repeat_count: 3,
                fired_count: 1,
                expires_ms: now_ms() + 3_600_000,
                status: "armed".into(),
                created_ms: now_ms(),
                updated_ms: now_ms(),
                status_reason: None,
                patrol_idle_since_ms: None,
                patrol_last_push_ms: 0,
                periodic_declared: true,
                goal_content: Some(crate::server::state::GoalScheduleBinding {
                    source: Some(crate::server::state::GoalScheduleContent::Inline {
                        content: "retain other goal".into(),
                    }),
                    ttl_seconds: 3600,
                }),
            },
        })
        .unwrap();
    state
        .apply_checked(&Event::WakeBound {
            message_id: "other-goal-message".into(),
            subscription_id: "other-goal-subscription".into(),
        })
        .unwrap();
    drop(state);
    (server, root, subscription_id, invite_message_id)
}

#[test]
fn ended_task_goal_withdrawal_preserves_batch_for_later_reassignment() {
    let now = now_ms();
    let (server, root, subscription_id, invite_message_id) =
        goal_withdraw_fixture("consumed", now - 1, 3);
    let before = {
        let state = server.state.lock().unwrap();
        (
            state.tasks["named-work"].clone(),
            state.board_details["named-work"].clone(),
            state.notification_subscriptions[&subscription_id].clone(),
            serde_json::to_value(&state.notification_subscriptions["other-goal-subscription"])
                .unwrap(),
            serde_json::to_value(&state.msgs["other-goal-message"]).unwrap(),
        )
    };

    let response = board_withdraw(
        &server,
        "master".into(),
        "token-master".into(),
        "named-work".into(),
        before.1.revision,
        "stale invitation blocks reassignment".into(),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["task"]["status"], "working");
    assert_eq!(response.data["task"]["owner"], "old");

    let state = server.state.lock().unwrap();
    let task = &state.tasks["named-work"];
    assert_eq!(task.status, "working");
    assert_eq!(task.owner, "old");
    assert_eq!(task.created_ms, before.0.created_ms);
    assert_eq!(task.created_by, before.0.created_by);
    assert_eq!(task.feature_id, before.0.feature_id);
    assert_eq!(task.worktree_path, before.0.worktree_path);
    assert_eq!(task.branch, before.0.branch);
    assert_eq!(task.base_commit, before.0.base_commit);
    assert_eq!(task.priority, before.0.priority);
    assert_eq!(task.next_step, before.0.next_step);
    assert_eq!(
        serde_json::to_value(&task.wait).unwrap(),
        serde_json::to_value(&before.0.wait).unwrap()
    );
    assert_eq!(task.isolation_mode, before.0.isolation_mode);

    let details = &state.board_details["named-work"];
    assert!(details.invitation.is_none());
    assert_eq!(details.revision, before.1.revision + 1);
    assert_eq!(details.long_horizon, before.1.long_horizon);
    assert_eq!(details.public_visibility, before.1.public_visibility);
    assert_eq!(details.admitted, before.1.admitted);
    assert_eq!(details.title, before.1.title);
    assert_eq!(details.description, before.1.description);
    assert_eq!(details.delivery_condition, before.1.delivery_condition);
    assert_eq!(details.test_condition, before.1.test_condition);
    assert_eq!(
        details.last_response.as_deref(),
        Some("withdrawn by master: stale invitation blocks reassignment")
    );
    assert_eq!(state.msgs[&invite_message_id].state, "superseded");

    let subscription = &state.notification_subscriptions[&subscription_id];
    assert_eq!(subscription.id, before.2.id);
    assert_eq!(subscription.worker_id, before.2.worker_id);
    assert_eq!(subscription.status, "consumed");
    assert_eq!(subscription.fired_count, 3);
    assert_eq!(subscription.repeat_count, 3);
    assert_eq!(subscription.trigger_ms, before.2.trigger_ms);
    assert_eq!(subscription.interval_ms, before.2.interval_ms);
    assert_eq!(subscription.expires_ms, before.2.expires_ms);
    assert_eq!(subscription.goal_content, before.2.goal_content);
    assert_eq!(
        state
            .notification_subscriptions
            .values()
            .filter(|entry| entry.subject.as_deref() == Some("goal:task:named-work"))
            .count(),
        1,
        "withdrawal must retain the existing ended batch for later explicit reassignment"
    );
    assert_eq!(
        serde_json::to_value(&state.notification_subscriptions["other-goal-subscription"]).unwrap(),
        before.3,
        "withdrawal must preserve unrelated goal progress"
    );
    assert_eq!(
        serde_json::to_value(&state.msgs["other-goal-message"]).unwrap(),
        before.4,
        "withdrawal must preserve unrelated messages"
    );
    assert_eq!(
        state
            .wake_bindings
            .get("other-goal-message")
            .map(String::as_str),
        Some("other-goal-subscription")
    );
    assert_eq!(state.task_assignment("named-work").unwrap().name, "Alice");
    assert_eq!(
        state.task_assignment("named-work").unwrap().session_id,
        "session-old"
    );
    let implicit = task_goal_events(
        &server,
        &state,
        &state.tasks["named-work"],
        &state.board_details["named-work"],
        Some("old"),
        false,
        GoalScheduleOperation::Derived,
        None,
    )
    .unwrap();
    assert!(
        implicit.0.is_empty(),
        "withdrawal must not renew an ended batch"
    );
    let withdrawal_revision = state.board_details["named-work"].revision;
    drop(state);

    let invited = board_invite(
        &server,
        "master".into(),
        "token-master".into(),
        "named-work".into(),
        "current".into(),
        withdrawal_revision,
    );
    assert!(invited.ok, "{invited:?}");
    let invited_revision = state_revision(&server, "named-work");
    let accepted = board_respond(
        &server,
        "current".into(),
        "token-current".into(),
        "named-work".into(),
        invited_revision,
        true,
        None,
    );
    assert!(accepted.ok, "{accepted:?}");
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["named-work"].owner, "current");
    assert_eq!(state.tasks["named-work"].status, "working");
    assert_eq!(state.task_assignment("named-work").unwrap().name, "Bob");
    assert_eq!(
        state.task_assignment("named-work").unwrap().session_id,
        "session-current"
    );
    assert_eq!(
        state.notification_subscriptions[&subscription_id].status,
        "consumed"
    );
    assert_eq!(
        state.notification_subscriptions[&subscription_id].fired_count,
        3
    );
    assert_eq!(
        state
            .notification_subscriptions
            .values()
            .filter(|entry| entry.subject.as_deref() == Some("goal:task:named-work"))
            .count(),
        1,
        "acceptance transfers work but only explicit renewal may create a new batch"
    );
    assert_eq!(
        serde_json::to_value(&state.notification_subscriptions["other-goal-subscription"]).unwrap(),
        before.3
    );
    assert_eq!(
        serde_json::to_value(&state.msgs["other-goal-message"]).unwrap(),
        before.4
    );
    drop(state);
    let renew = || {
        handle_goal_schedule_reconcile_with_period(
            &server,
            "current".into(),
            "token-current".into(),
            "task:named-work".into(),
            "Bob".into(),
            "active".into(),
            None,
            Some(before.2.goal_content.as_ref().unwrap().ttl_seconds),
            None,
            None,
            before.2.interval_ms,
            Some(before.2.repeat_count),
        )
    };
    let renewed = renew();
    assert!(renewed.ok, "{renewed:?}");
    let new_id = renewed.data["subscription"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(new_id, subscription_id);
    let repeated = renew();
    assert!(repeated.ok, "{repeated:?}");
    assert_eq!(repeated.data["subscription"]["id"], new_id);
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions[&subscription_id].status,
        "consumed"
    );
    assert_eq!(
        state.notification_subscriptions[&subscription_id].fired_count,
        3
    );
    assert_eq!(state.notification_subscriptions[&new_id].status, "armed");
    assert_eq!(
        state.notification_subscriptions[&new_id].worker_id,
        "current"
    );
    assert_eq!(
        state
            .notification_subscriptions
            .values()
            .filter(|sub| sub.subject.as_deref() == Some("goal:task:named-work"))
            .count(),
        2
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn armed_task_goal_withdrawal_preserves_batch_for_later_reassignment() {
    let now = now_ms();
    let (server, root, subscription_id, invite_message_id) =
        goal_withdraw_fixture("armed", now + 3_600_000, 1);
    let before = {
        let state = server.state.lock().unwrap();
        (
            state.tasks["named-work"].clone(),
            state.notification_subscriptions[&subscription_id].clone(),
            serde_json::to_value(&state.notification_subscriptions["other-goal-subscription"])
                .unwrap(),
        )
    };
    let revision = state_revision(&server, "named-work");
    let response = board_withdraw(
        &server,
        "master".into(),
        "token-master".into(),
        "named-work".into(),
        revision,
        "armed control withdrawal".into(),
    );
    assert!(response.ok, "{response:?}");
    let state = server.state.lock().unwrap();
    assert_eq!(
        serde_json::to_value(&state.tasks["named-work"]).unwrap(),
        serde_json::to_value(&before.0).unwrap()
    );
    assert_eq!(state.msgs[&invite_message_id].state, "superseded");
    let subscription = &state.notification_subscriptions[&subscription_id];
    assert_eq!(subscription.id, before.1.id);
    assert_eq!(subscription.worker_id, before.1.worker_id);
    assert_eq!(subscription.status, "armed");
    assert_eq!(subscription.fired_count, 1);
    assert_eq!(subscription.trigger_ms, before.1.trigger_ms);
    assert_eq!(subscription.interval_ms, before.1.interval_ms);
    assert_eq!(subscription.repeat_count, before.1.repeat_count);
    assert_eq!(subscription.expires_ms, before.1.expires_ms);
    assert_eq!(subscription.goal_content, before.1.goal_content);
    assert_eq!(
        serde_json::to_value(&state.notification_subscriptions["other-goal-subscription"]).unwrap(),
        before.2
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

fn goal_handoff_fixture(
    status: &str,
    expires_ms: i64,
    fired_count: u32,
) -> (
    Server,
    std::path::PathBuf,
    RouteScope,
    String,
    String,
    crate::server::binding_slot::PendingEffect,
) {
    let (server, root, route) = board_name_fixture();
    // Recovery tests need durable owners, not a live pane or tmux probing.
    for worker in ["old", "current"] {
        let transport = crate::server::peer_tests::test_appserver_transport(worker);
        let registered = crate::server::peer_tests::seed_committed_owner(
            &server,
            worker,
            &format!("token-{worker}"),
            &transport,
            &root.display().to_string(),
            Some(route.app_scope_id.clone()),
        );
        assert!(registered.ok, "native handoff fixture: {registered:?}");
    }
    let mut state = server.state.lock().unwrap();
    let details = state.board_details.get_mut("named-work").unwrap();
    details.admitted = true;
    details.long_horizon = Some(crate::board::LongHorizonTaskSpec {
        content: "continue existing handoff".into(),
        interval_ms: 1_200_000,
        repeat_count: 3,
        ttl_seconds: 172_800,
    });
    let task = state.tasks["named-work"].clone();
    let details = state.board_details["named-work"].clone();
    let (schedule_events, _) = task_goal_events(
        &server,
        &state,
        &task,
        &details,
        Some("old"),
        false,
        GoalScheduleOperation::Derived,
        None,
    )
    .unwrap();
    for event in schedule_events {
        state.apply_checked(&event).unwrap();
    }
    let subscription_id = state
        .notification_subscriptions
        .values()
        .find(|subscription| subscription.subject.as_deref() == Some("goal:task:named-work"))
        .expect("task-linked goal schedule")
        .id
        .clone();
    let subscription = state
        .notification_subscriptions
        .get_mut(&subscription_id)
        .unwrap();
    subscription.status = status.into();
    subscription.expires_ms = expires_ms;
    subscription.fired_count = fired_count;
    subscription.updated_ms = now_ms();

    let message_id = "goal-handoff-notice".to_owned();
    state
        .apply_checked(&Event::Sent {
            msg: Message {
                id: message_id.clone(),
                from: "scheduler".into(),
                to: "old".into(),
                mtype: "notify".into(),
                subject: Some("goal:task:named-work".into()),
                body: "continue existing handoff".into(),
                in_reply_to: None,
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        })
        .unwrap();
    state
        .apply_checked(&Event::WakeBound {
            message_id: message_id.clone(),
            subscription_id: subscription_id.clone(),
        })
        .unwrap();
    let previous_revision = state
        .name_occupancy(route.project_scope_id.as_str(), "Alice")
        .expect("current Alice occupancy")
        .occupancy_revision;
    drop(state);

    let control =
        crate::server::binding_store::BindingControlStore::new(server.host_paths.state_root())
            .load()
            .unwrap();
    let current = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "current")
        .unwrap()
        .and_then(|slot| slot.active_binding())
        .expect("current map binding for new carrier");
    let target_session_id = current
        .binding
        .session_id
        .as_ref()
        .expect("new carrier session")
        .as_str()
        .to_owned();
    let effect = crate::server::binding_slot::PendingEffect {
        kind: crate::server::binding_slot::PendingEffectKind::Name,
        role: None,
        effect_id: "goal-handoff-red".into(),
        operation_id: "goal-handoff-red".into(),
        project_scope: route.project_scope_id.clone(),
        app_scope_id: route.app_scope_id.clone(),
        name: "Alice".into(),
        previous_worker_id: Some("old".into()),
        previous_session_id: Some("session-old".into()),
        target_worker_id: "current".into(),
        target_session_id,
        occupancy_revision: previous_revision + 1,
        source_names: Vec::new(),
        moves_existing_names: false,
        pane_anchor: None,
        created_ms: now_ms(),
    };
    (server, root, route, subscription_id, message_id, effect)
}

fn goal_handoff_events(
    server: &Server,
    route: &RouteScope,
    effect: &crate::server::binding_slot::PendingEffect,
) -> Vec<Event> {
    let state = server.state.lock().unwrap();
    let previous = state
        .name_occupancy(route.project_scope_id.as_str(), effect.name.as_str())
        .cloned();
    let occupancy = crate::server::name_state::NameOccupancy {
        name: effect.name.clone(),
        worker_id: effect.target_worker_id.clone(),
        session_id: effect.target_session_id.clone(),
        occupancy_revision: effect.occupancy_revision,
        updated_ms: now_ms(),
        pane_anchor: None,
    };
    name_resolution::build_name_resolution_events_with_rename_from(
        &state,
        route.project_scope_id.as_str(),
        &occupancy,
        &[],
        previous.as_ref(),
    )
}

#[test]
fn same_carrier_name_resolution_is_a_noop_for_an_ended_task_batch() {
    let now = now_ms();
    let (server, root, route, subscription_id, message_id, effect) =
        goal_handoff_fixture("consumed", now - 1, 3);
    let before = {
        let state = server.state.lock().unwrap();
        (
            serde_json::to_value(&state.tasks["named-work"]).unwrap(),
            serde_json::to_value(&state.notification_subscriptions[&subscription_id]).unwrap(),
            serde_json::to_value(&state.msgs[&message_id]).unwrap(),
        )
    };
    let mut same_carrier = effect;
    same_carrier.target_worker_id = "old".into();
    same_carrier.target_session_id = "session-old".into();
    let events = goal_handoff_events(&server, &route, &same_carrier);
    assert!(
        !events.iter().any(|event| match event {
            Event::TaskUpdated { task } => task.id == "named-work",
            Event::BoardDetailsChanged { task_id, .. } => task_id == "named-work",
            Event::MailboxReassigned { handoff } => handoff.message_id == message_id,
            Event::NotificationSubscribed { subscription } =>
                subscription.subject.as_deref() == Some("goal:task:named-work"),
            _ => false,
        }),
        "{events:?}"
    );
    server.commit_checked(&events).unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(
        serde_json::to_value(&state.tasks["named-work"]).unwrap(),
        before.0
    );
    assert_eq!(
        serde_json::to_value(&state.notification_subscriptions[&subscription_id]).unwrap(),
        before.1
    );
    assert_eq!(
        serde_json::to_value(&state.msgs[&message_id]).unwrap(),
        before.2
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

fn assert_scope_local_pending_effect_recovery(status: &str, expires_ms: i64, fired_count: u32) {
    assert_scope_local_pending_effect_recovery_mode(status, expires_ms, fired_count, false);
}

fn assert_scope_local_pending_effect_recovery_mode(
    status: &str,
    expires_ms: i64,
    fired_count: u32,
    ticket_only: bool,
) {
    let now = now_ms();
    let (server, root, route, subscription_id, message_id, mut effect) =
        goal_handoff_fixture(status, expires_ms, fired_count);
    let server = Arc::new(server);
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    manager.install_runtime(
        &(
            route.app_scope_id.as_str().to_owned(),
            route.project_scope_id.as_str().to_owned(),
        ),
        server.clone(),
        None,
    );

    let control_store =
        crate::server::binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = control_store.load().unwrap();
    let alice = crate::server::binding_slot::IdentityBindingKey::new(
        route.project_scope_id.clone(),
        route.app_scope_id.clone(),
        "Alice",
    );
    let displaced_slot = control.slot(&alice).expect("existing Alice slot");
    let displaced = displaced_slot
        .active_binding()
        .expect("existing Alice carrier")
        .clone();
    assert_eq!(displaced.binding.agent_id.as_str(), "old");
    let current = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "current")
        .unwrap()
        .expect("current carrier name")
        .active_binding()
        .expect("current carrier binding")
        .clone();
    let previous_name = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "current")
        .unwrap()
        .expect("current carrier name")
        .identity_key
        .name
        .clone();
    let previous_revision = server
        .state
        .lock()
        .unwrap()
        .name_occupancy_revision(route.project_scope_id.as_str(), "Alice");
    effect.occupancy_revision = previous_revision + 1;
    effect.source_names = vec![previous_name.clone()];
    effect.moves_existing_names = true;
    if ticket_only {
        let mut state = server.state.lock().unwrap();
        state.msgs.get_mut(&message_id).unwrap().to = "current".into();
        state.tasks.get_mut("named-work").unwrap().owner = "current".into();
        state
            .task_assignments
            .get_mut("named-work")
            .unwrap()
            .session_id = "session-current".into();
        state
            .notification_subscriptions
            .get_mut(&subscription_id)
            .unwrap()
            .worker_id = "current".into();
        let occupancy = state
            .name_occupancies
            .get_mut(route.project_scope_id.as_str())
            .unwrap()
            .get_mut("Alice")
            .unwrap();
        occupancy.worker_id = "current".into();
        occupancy.session_id = "session-current".into();
        occupancy.occupancy_revision = effect.occupancy_revision;
        effect.previous_worker_id = Some("current".into());
        effect.previous_session_id = Some("session-current".into());
        state.ticket_phases.push(ticket_state::TicketPhaseRecord {
            task_id: "named-work".into(),
            phase: ticket_state::TicketPhaseKind::Execute,
            batch: 1,
            priority: "p2".into(),
            status: ticket_state::TicketPhaseStatus::Claimed,
            slot_name: Some("Alice".into()),
            binding_generation: displaced_slot.generation(),
            enqueue_seq: 1,
            attempt_generation: 1,
            depends_on: Vec::new(),
            route_group: None,
            wait_reason: None,
            result: None,
        });
    }
    control_store
        .commit(
            crate::server::binding_store::ControlChange::MoveNameWithEffect {
                target: alice,
                value: current,
                source_names: vec![previous_name],
                expected_generation: Some(displaced_slot.generation()),
                effect: effect.clone(),
                companion: None,
            },
        )
        .unwrap();

    let unrelated_subscription = NotificationSubscription {
        id: "recovery-unrelated-goal".into(),
        worker_id: "other".into(),
        event: "deadline".into(),
        subject: Some("goal:other-recovery-goal".into()),
        target: "other-recovery-target".into(),
        method: "appserver".into(),
        trigger_ms: Some(now + 60_000),
        trigger_times_ms: Vec::new(),
        interval_ms: Some(60_000),
        repeat_count: 3,
        fired_count: 1,
        expires_ms: now + 3_600_000,
        status: "armed".into(),
        created_ms: now,
        updated_ms: now,
        status_reason: None,
        patrol_idle_since_ms: None,
        patrol_last_push_ms: 0,
        periodic_declared: true,
        goal_content: Some(crate::server::state::GoalScheduleBinding {
            source: Some(crate::server::state::GoalScheduleContent::Inline {
                content: "preserve unrelated goal".into(),
            }),
            ttl_seconds: 3600,
        }),
    };
    let unrelated_message = Message {
        id: "recovery-unrelated-message".into(),
        from: "scheduler".into(),
        to: "other".into(),
        mtype: "notify".into(),
        subject: Some("goal:other-recovery-goal".into()),
        body: "preserve unrelated message".into(),
        in_reply_to: None,
        created_ms: now,
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    let (
        task_before,
        subscription_before,
        message_before,
        unrelated_subscription_before,
        unrelated_message_before,
    ) = {
        let mut state = server.state.lock().unwrap();
        state
            .apply_checked(&Event::NotificationSubscribed {
                subscription: unrelated_subscription,
            })
            .unwrap();
        state
            .apply_checked(&Event::Sent {
                msg: unrelated_message,
            })
            .unwrap();
        state
            .apply_checked(&Event::WakeBound {
                message_id: "recovery-unrelated-message".into(),
                subscription_id: "recovery-unrelated-goal".into(),
            })
            .unwrap();
        (
            state.tasks["named-work"].clone(),
            state.notification_subscriptions[&subscription_id].clone(),
            state.msgs[&message_id].clone(),
            serde_json::to_value(&state.notification_subscriptions["recovery-unrelated-goal"])
                .unwrap(),
            serde_json::to_value(&state.msgs["recovery-unrelated-message"]).unwrap(),
        )
    };

    let context =
        ProjectContext::for_registered_root_with_app(&server.root, route.app_scope_id.clone())
            .unwrap();
    manager
        .recover_pending_effects_for_scope(&context)
        .expect("scope-local recovery must complete the pending handoff");

    let durable_control = control_store.load().unwrap();
    assert!(
        durable_control.pending_effect(&effect.effect_id).is_none(),
        "the completed effect must be cleared from the control resource"
    );
    let state = crate::server::current_state::load(&server.state_path, false).unwrap();
    let task = &state.tasks["named-work"];
    assert_eq!(task.id, task_before.id);
    assert_eq!(task.owner, "current");
    assert_eq!(task.status, task_before.status);
    assert_eq!(state.task_assignment("named-work").unwrap().name, "Alice");
    assert_eq!(
        state.task_assignment("named-work").unwrap().session_id,
        "session-current"
    );
    let subscription = &state.notification_subscriptions[&subscription_id];
    assert_eq!(subscription.id, subscription_before.id);
    assert_eq!(subscription.worker_id, "current");
    assert_eq!(subscription.status, status);
    assert_eq!(subscription.fired_count, subscription_before.fired_count);
    assert_eq!(subscription.repeat_count, subscription_before.repeat_count);
    assert_eq!(subscription.trigger_ms, subscription_before.trigger_ms);
    assert_eq!(subscription.interval_ms, subscription_before.interval_ms);
    assert_eq!(subscription.expires_ms, subscription_before.expires_ms);
    assert_eq!(subscription.goal_content, subscription_before.goal_content);
    assert_eq!(
        state
            .notification_subscriptions
            .values()
            .filter(|entry| entry.subject.as_deref() == Some("goal:task:named-work"))
            .count(),
        1,
        "actual recovery must not implicitly renew an ended batch"
    );
    let message = &state.msgs[&message_id];
    assert_eq!(message.id, message_before.id);
    assert_eq!(message.body, message_before.body);
    assert_eq!(message.to, "current");
    assert_eq!(message.state, "pending");
    assert_eq!(
        serde_json::to_value(&state.notification_subscriptions["recovery-unrelated-goal"]).unwrap(),
        unrelated_subscription_before,
        "recovery must preserve unrelated goal progress"
    );
    assert_eq!(
        serde_json::to_value(&state.msgs["recovery-unrelated-message"]).unwrap(),
        unrelated_message_before,
        "recovery must preserve unrelated messages"
    );
    assert_eq!(
        state
            .wake_bindings
            .get("recovery-unrelated-message")
            .map(String::as_str),
        Some("recovery-unrelated-goal")
    );
    drop(state);
    if ticket_only {
        let before_retry = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        manager.recover_pending_effects_for_scope(&context).unwrap();
        assert_eq!(
            serde_json::to_value(&*server.state.lock().unwrap()).unwrap(),
            before_retry
        );
        let binding = durable_control
            .current_for_name(&route.project_scope_id, &route.app_scope_id, "Alice")
            .unwrap()
            .unwrap()
            .active_binding()
            .unwrap()
            .binding
            .clone();
        let mut context = context.clone();
        context.runtime_context = Some(crate::identity::RuntimeIdentity {
            agent_id: binding.agent_id,
            runtime_id: binding.runtime_id,
            appserver_id: binding.app_scope_id,
            binding_id: binding.binding_id,
            session_id: binding.session_id,
            native_thread_id: binding.native_thread_id,
        });
        let received = poll_messages_with_context(
            &server,
            "current",
            Some("token-current"),
            Some(&context),
            Some("ended-goal-receive"),
        )
        .expect("pending message must be available");
        assert!(received.ok, "{received:?}");
        assert_eq!(server.state.lock().unwrap().msgs[&message_id].state, "read");
        let replay = poll_messages_with_context(
            &server,
            "current",
            Some("token-current"),
            Some(&context),
            Some("ended-goal-receive"),
        )
        .unwrap();
        assert!(replay.ok, "{replay:?}");
        assert_eq!(replay.data["messages"], received.data["messages"]);
        let revision = state_revision(&server, "named-work");
        let updated = handle_board_command(
            &server,
            "current".into(),
            "token-current".into(),
            crate::board::BoardCommand::Update {
                id: "named-work".into(),
                expected_revision: revision,
                status: None,
                next: Some("continue after terminal batch".into()),
            },
        );
        assert!(updated.ok, "{updated:?}");
        let before_denied = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
        let renew = || GoalScheduleChange {
            goal_id: "task:named-work".into(),
            goal_owner: "Alice".into(),
            desired: "active".into(),
            trigger_ms: None,
            ttl_seconds: None,
            goal_subject: None,
            reason: None,
            interval_ms: None,
            repeat_count: None,
            content: None,
            operation: GoalScheduleOperation::Explicit,
        };
        let denied = handle_goal_schedule_for_actor(&server, "current", "wrong-token", renew());
        assert!(!denied.ok, "{denied:?}");
        assert_eq!(
            serde_json::to_value(&*server.state.lock().unwrap()).unwrap(),
            before_denied
        );
        let renewed = handle_goal_schedule_for_actor(&server, "current", "token-current", renew());
        assert!(renewed.ok, "{renewed:?}");
        let repeated = handle_goal_schedule_for_actor(&server, "current", "token-current", renew());
        assert!(repeated.ok, "{repeated:?}");
        assert_eq!(
            renewed.data["subscription_id"],
            repeated.data["subscription_id"]
        );
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.notification_subscriptions[&subscription_id].status,
            status
        );
        assert_eq!(
            state.notification_subscriptions[&subscription_id].fired_count,
            fired_count
        );
        let batches: Vec<_> = state
            .notification_subscriptions
            .values()
            .filter(|subscription| subscription.subject.as_deref() == Some("goal:task:named-work"))
            .collect();
        assert_eq!(batches.len(), 2);
        assert_eq!(
            batches
                .iter()
                .filter(|subscription| subscription.status == "armed")
                .count(),
            1
        );
        assert_eq!(
            batches
                .iter()
                .find(|subscription| subscription.id != subscription_id)
                .unwrap()
                .fired_count,
            0
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn scope_local_pending_effect_recovery_preserves_consumed_task_batch() {
    assert_scope_local_pending_effect_recovery("consumed", now_ms() - 1, 3);
}

#[test]
fn scope_local_pending_effect_recovery_preserves_expired_task_batch() {
    assert_scope_local_pending_effect_recovery("expired", now_ms() - 1, 1);
}

#[test]
fn scope_local_pending_effect_recovery_preserves_armed_task_batch() {
    assert_scope_local_pending_effect_recovery("armed", now_ms() + 3_600_000, 1);
}

#[test]
fn ended_goal_pending_effect_ticket_recovery_consumed() {
    assert_scope_local_pending_effect_recovery_mode("consumed", now_ms() - 1, 3, true);
}

#[test]
fn ended_goal_pending_effect_ticket_recovery_expired() {
    assert_scope_local_pending_effect_recovery_mode("expired", now_ms() - 1, 1, true);
}

#[test]
fn ended_goal_business_content_update_preserves_terminal_batch() {
    for (status, expired, fired_count) in [
        ("consumed", false, 3),
        ("expired", true, 1),
        ("armed", true, 1),
        ("armed", false, 3),
    ] {
        let expiry = now_ms() + if expired { -1 } else { 3_600_000 };
        let (server, root, _, subscription_id, _, _) =
            goal_handoff_fixture(status, expiry, fired_count);
        let (subscription_before, mut details) = {
            let state = server.state.lock().unwrap();
            (
                serde_json::to_value(&state.notification_subscriptions[&subscription_id]).unwrap(),
                state.board_details["named-work"].clone(),
            )
        };
        details.long_horizon.as_mut().unwrap().content = "updated work remains current".into();
        details.revision += 1;
        server
            .commit_checked(&[Event::BoardDetailsChanged {
                task_id: "named-work".into(),
                details,
            }])
            .expect("business content edit must not require renewal");
        let state = server.state.lock().unwrap();
        assert_eq!(
            serde_json::to_value(&state.notification_subscriptions[&subscription_id]).unwrap(),
            subscription_before
        );
        assert_eq!(
            state
                .notification_subscriptions
                .values()
                .filter(
                    |subscription| subscription.subject.as_deref() == Some("goal:task:named-work")
                )
                .count(),
            1
        );
        assert_eq!(
            state.board_details["named-work"]
                .long_horizon
                .as_ref()
                .unwrap()
                .content,
            "updated work remains current"
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn recovered_same_name_handoff_migrates_stale_owner_before_board_update() {
    let now = now_ms();
    let (server, root, route, subscription_id, message_id, mut effect) =
        goal_handoff_fixture("armed", now + 3_600_000, 1);
    let server = Arc::new(server);
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    manager.install_runtime(
        &(
            route.app_scope_id.as_str().to_owned(),
            route.project_scope_id.as_str().to_owned(),
        ),
        server.clone(),
        None,
    );
    let response = crate::server::peer_tests::register_synthetic_peer(&server, "other", "other");
    assert!(response.ok, "{response:?}");

    let control_store =
        crate::server::binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = control_store.load().unwrap();
    let alice = crate::server::binding_slot::IdentityBindingKey::new(
        route.project_scope_id.clone(),
        route.app_scope_id.clone(),
        "Alice",
    );
    let displaced_slot = control.slot(&alice).expect("existing Alice slot");
    let displaced = displaced_slot
        .active_binding()
        .expect("existing Alice carrier")
        .clone();
    let current = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "current")
        .unwrap()
        .expect("current carrier name")
        .active_binding()
        .expect("current carrier binding")
        .clone();
    let previous_name = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "current")
        .unwrap()
        .expect("current carrier name")
        .identity_key
        .name
        .clone();
    effect.source_names = vec![previous_name.clone()];
    effect.moves_existing_names = true;

    let (task_before, details_before, lifecycle_before, subscription_before, message_before) = {
        let mut state = server.state.lock().unwrap();
        state.tasks.get_mut("named-work").unwrap().next_step =
            Some("preserve the existing handoff".into());
        state.task_assignments.remove("named-work");
        state.task_lifecycle.insert(
            "named-work".into(),
            state::TaskLifecycleRecord {
                review_evidence: Some("preserve reviewed evidence".into()),
                reviewer: Some("Tina".into()),
                required_reviewer_name: Some("Tina".into()),
                ..Default::default()
            },
        );
        let occupancy = state
            .name_occupancies
            .get_mut(route.project_scope_id.as_str())
            .unwrap()
            .get_mut("Alice")
            .unwrap();
        occupancy.worker_id = "current".into();
        occupancy.session_id = "session-current".into();
        occupancy.occupancy_revision = effect.occupancy_revision;
        occupancy.updated_ms = now;
        (
            state.tasks["named-work"].clone(),
            state.board_details["named-work"].clone(),
            serde_json::to_value(&state.task_lifecycle["named-work"]).unwrap(),
            state.notification_subscriptions[&subscription_id].clone(),
            serde_json::to_value(&state.msgs[&message_id]).unwrap(),
        )
    };

    control_store
        .commit(
            crate::server::binding_store::ControlChange::MoveNameWithEffect {
                target: alice,
                value: current,
                source_names: vec![previous_name],
                expected_generation: Some(displaced_slot.generation()),
                effect: effect.clone(),
                companion: None,
            },
        )
        .unwrap();
    let context =
        ProjectContext::for_registered_root_with_app(&server.root, route.app_scope_id.clone())
            .unwrap();
    manager
        .recover_pending_effects_for_scope(&context)
        .expect("scope-local recovery must finish the same-name handoff");

    let state = server.state.lock().unwrap();
    let task = &state.tasks["named-work"];
    assert_eq!(task.id, task_before.id);
    assert_eq!(task.owner, "current");
    assert_eq!(task.status, task_before.status);
    assert_eq!(task.next_step, task_before.next_step);
    let assignment = state
        .task_assignment("named-work")
        .expect("the current name holder must own the unfinished task");
    assert_eq!(assignment.name, "Alice");
    assert_eq!(assignment.session_id, "session-current");
    assert_eq!(
        state.board_details["named-work"].revision,
        details_before.revision + 1
    );
    assert_eq!(
        serde_json::to_value(&state.task_lifecycle["named-work"]).unwrap(),
        lifecycle_before
    );
    let subscription = &state.notification_subscriptions[&subscription_id];
    assert_eq!(subscription.id, subscription_before.id);
    assert_eq!(subscription.worker_id, "current");
    assert_eq!(subscription.status, subscription_before.status);
    assert_eq!(subscription.fired_count, subscription_before.fired_count);
    assert_eq!(subscription.repeat_count, subscription_before.repeat_count);
    assert_eq!(subscription.trigger_ms, subscription_before.trigger_ms);
    assert_eq!(subscription.interval_ms, subscription_before.interval_ms);
    assert_eq!(subscription.expires_ms, subscription_before.expires_ms);
    assert_eq!(subscription.goal_content, subscription_before.goal_content);
    let message = serde_json::to_value(&state.msgs[&message_id]).unwrap();
    assert_eq!(message["id"], message_before["id"]);
    assert_eq!(message["body"], message_before["body"]);
    assert_eq!(message["to"], "current");
    assert_eq!(message["state"], "pending");
    let revision = state.board_details["named-work"].revision;
    drop(state);

    let updated = handle_board_command(
        &server,
        "current".into(),
        "token-current".into(),
        crate::board::BoardCommand::Update {
            id: "named-work".into(),
            expected_revision: revision,
            status: None,
            next: Some("current holder continues the handed-off task".into()),
        },
    );
    assert!(updated.ok, "{updated:?}");
    assert_eq!(updated.data["task"]["owner"], "current");
    assert_eq!(
        updated.data["task"]["next_step"],
        "current holder continues the handed-off task"
    );
    let updated_revision = updated.data["task"]["revision"].as_u64().unwrap();

    let denied = handle_board_command(
        &server,
        "other".into(),
        "token-other".into(),
        crate::board::BoardCommand::Update {
            id: "named-work".into(),
            expected_revision: updated_revision,
            status: None,
            next: Some("a different name must not update the task".into()),
        },
    );
    assert!(!denied.ok);
    assert_eq!(
        denied.error.as_deref(),
        Some("TASK_OWNER_REQUIRED: the task's current stable-name holder may execute it")
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["named-work"].owner, "current");
    assert_eq!(
        state.tasks["named-work"].next_step.as_deref(),
        Some("current holder continues the handed-off task")
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
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
fn task_view_with_identity_follows_current_name_and_preserves_closed_history() {
    let (server, root, route) = board_name_fixture();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    {
        let state = server.state.lock().unwrap();
        let open =
            task_view_with_identity(&state, &control, Some(&route), &state.tasks["named-work"])
                .unwrap();
        assert_eq!(open["owner_name"], "Alice");
        assert_eq!(open["owner"], "old");
        assert_eq!(open["owner_session_id"], "session-old");

        let mut closed = state.tasks["named-work"].clone();
        closed.status = "closed".into();
        let closed = task_view_with_identity(&state, &control, Some(&route), &closed).unwrap();
        assert_eq!(closed["owner_name"], "Alice");
        assert_eq!(closed["owner"], "old");
        assert_eq!(closed["owner_session_id"], "session-old");
    }

    let source = control
        .key_for_worker(&route.project_scope_id, &route.app_scope_id, "current")
        .unwrap()
        .unwrap();
    let value = control
        .slot(source)
        .unwrap()
        .active_binding()
        .unwrap()
        .clone();
    store
        .commit(binding_store::ControlChange::MoveName {
            target: binding_slot::IdentityBindingKey::new(
                route.project_scope_id.clone(),
                route.app_scope_id.clone(),
                "Alice",
            ),
            value,
            source_names: vec![source.name.clone()],
            expected_generation: None,
        })
        .unwrap();
    let control = store.load().unwrap();
    let state = server.state.lock().unwrap();
    let open = task_view_with_identity(&state, &control, Some(&route), &state.tasks["named-work"])
        .unwrap();
    assert_eq!(open["owner_name"], "Alice");
    assert_eq!(open["owner"], "current");
    assert_eq!(open["owner_session_id"], "session-current");

    let mut closed = state.tasks["named-work"].clone();
    closed.status = "closed".into();
    let closed = task_view_with_identity(&state, &control, Some(&route), &closed).unwrap();
    assert_eq!(closed["owner"], "old");
    assert_eq!(closed["owner_session_id"], "session-old");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_update_uses_current_name_when_raw_owner_handoff_is_incomplete() {
    let (server, root) = test_server();
    for worker in ["old", "current"] {
        let response = crate::server::peer_tests::seed_committed_owner(
            &server,
            worker,
            &format!("token-{worker}"),
            &crate::server::peer_tests::test_appserver_transport(worker),
            &server.root.to_string_lossy(),
            Some(AppServerId::new("tui-default").unwrap()),
        );
        assert!(response.ok, "{response:?}");
    }
    let route = {
        let mut state = server.state.lock().unwrap();
        let route = server_route_scope(&server, &state).unwrap().unwrap();
        let mut work = task("named-work", "old");
        work.worktree_path = Some(root.join("retained-work").to_string_lossy().into_owned());
        work.branch = Some("fixture-retained-branch".into());
        state
            .apply_checked(&Event::TaskCreated { task: work })
            .unwrap();
        state
            .apply_checked(&Event::TaskLifecycleUpdated {
                task_id: "named-work".into(),
                record: state::TaskLifecycleRecord {
                    delivery_evidence: Some("retained candidate evidence".into()),
                    ..Default::default()
                },
            })
            .unwrap();
        state
            .board_details
            .get_mut("named-work")
            .unwrap()
            .public_visibility = true;
        state
            .apply_task_assignment(&crate::server::name_state::TaskAssignment {
                task_id: "named-work".into(),
                name: "Alice".into(),
                session_id: "old".into(),
                assignment_revision: 1,
                reflow: None,
                updated_ms: 1,
            })
            .unwrap();
        route
    };
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let source = control
        .key_for_worker(&route.project_scope_id, &route.app_scope_id, "current")
        .unwrap()
        .unwrap();
    let value = control
        .slot(source)
        .unwrap()
        .active_binding()
        .unwrap()
        .clone();
    // Commit only the real map transition. Business handoff has not completed.
    store
        .commit(binding_store::ControlChange::MoveName {
            target: binding_slot::IdentityBindingKey::new(
                route.project_scope_id.clone(),
                route.app_scope_id.clone(),
                "Alice",
            ),
            value,
            source_names: vec![source.name.clone()],
            expected_generation: None,
        })
        .unwrap();
    let control = store.load().unwrap();
    let mut state = server.state.lock().unwrap();
    let before = serde_json::to_value(&*state).unwrap();
    let view = board_task_view_with_master(
        &state,
        &control,
        &state.tasks["named-work"],
        Some("current"),
        None,
        Some(&route),
    )
    .unwrap()
    .unwrap();
    assert_eq!(view.owner, "current");
    assert_eq!(state.tasks["named-work"].owner, "old");
    let old_token = state.workers["old"].token.clone();
    let denied = task_update_locked(
        &server,
        &mut state,
        Some(&control),
        "old".into(),
        old_token,
        "named-work".into(),
        None,
        Some("superseded holder".into()),
        None,
        None,
    );
    assert!(
        !denied.ok,
        "raw owner equality must not authorize: {denied:?}"
    );
    assert_eq!(serde_json::to_value(&*state).unwrap(), before);
    let token = state.workers["current"].token.clone();
    let result = task_update_locked(
        &server,
        &mut state,
        Some(&control),
        "current".into(),
        token,
        "named-work".into(),
        None,
        Some("current name continues".into()),
        None,
        None,
    );
    assert!(
        result.ok,
        "projected current owner must pass the production gate: {result:?}"
    );
    assert_eq!(
        state.tasks["named-work"].next_step.as_deref(),
        Some("current name continues")
    );
    assert_eq!(
        state.tasks["named-work"].owner, "old",
        "update must not fake handoff synchronization"
    );
    let mut expected_task = before["tasks"]["named-work"].clone();
    expected_task["next_step"] = json!("current name continues");
    expected_task["updated_ms"] = json!(state.tasks["named-work"].updated_ms);
    assert_eq!(
        serde_json::to_value(&state.tasks["named-work"]).unwrap(),
        expected_task
    );
    for field in [
        "task_assignments",
        "task_lifecycle",
        "pending_merges",
        "cleanup_receipts",
    ] {
        assert_eq!(
            serde_json::to_value(&*state).unwrap()[field],
            before[field],
            "preserve {field}"
        );
    }
    // A private row uses the same current-name authority as its projection.
    state
        .board_details
        .get_mut("named-work")
        .unwrap()
        .public_visibility = false;
    let token = state.workers["current"].token.clone();
    assert!(board_current_map_update_actor_gate(
        &server,
        &state,
        &control,
        "current",
        &token,
        "named-work"
    )
    .is_ok());
    assert!(board_current_map_update_actor_gate(
        &server,
        &state,
        &control,
        "old",
        &state.workers["old"].token,
        "named-work"
    )
    .is_ok());
    // Visibility alone never grants mutation: the former raw owner was denied
    // by the production lifecycle gate above.
    state
        .board_details
        .get_mut("named-work")
        .unwrap()
        .work_policy
        .executor_name = Some("Bob".into());
    let before_executor = serde_json::to_value(&*state).unwrap();
    let denied = task_update_locked(
        &server,
        &mut state,
        Some(&control),
        "current".into(),
        token.clone(),
        "named-work".into(),
        Some("verifying".into()),
        None,
        None,
        None,
    );
    assert!(!denied.ok, "{denied:?}");
    assert!(denied.error.unwrap().contains("WORK_EXECUTOR_REQUIRED"));
    assert_eq!(serde_json::to_value(&*state).unwrap(), before_executor);
    state
        .board_details
        .get_mut("named-work")
        .unwrap()
        .work_policy
        .executor_name = Some("Alice".into());
    let result = task_update_locked(
        &server,
        &mut state,
        Some(&control),
        "current".into(),
        token,
        "named-work".into(),
        Some("verifying".into()),
        None,
        None,
        None,
    );
    assert!(result.ok, "{result:?}");
    assert_eq!(state.tasks["named-work"].status, "verifying");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn task_view_with_identity_preserves_legacy_and_empty_holder_cases() {
    let (server, root, route) = board_name_fixture();
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let mut state = server.state.lock().unwrap();
    state.task_assignments.remove("named-work");
    let legacy =
        task_view_with_identity(&state, &control, Some(&route), &state.tasks["named-work"])
            .unwrap();
    assert_eq!(legacy["owner"], "old");
    assert!(legacy["owner_name"].is_null());
    assert!(legacy["owner_session_id"].is_null());

    state.task_assignments.insert(
        "named-work".into(),
        crate::server::name_state::TaskAssignment {
            task_id: "named-work".into(),
            name: "Alice".into(),
            session_id: "recorded-session".into(),
            assignment_revision: 1,
            reflow: None,
            updated_ms: 1,
        },
    );
    let empty = task_view_with_identity(
        &state,
        &binding_slot::BindingControlSet::default(),
        Some(&route),
        &state.tasks["named-work"],
    )
    .unwrap();
    assert_eq!(empty["owner_name"], "Alice");
    assert_eq!(empty["owner"], "");
    assert!(empty["owner_session_id"].is_null());

    let missing_route = task_view_with_identity(
        &state,
        &binding_slot::BindingControlSet::default(),
        None,
        &state.tasks["named-work"],
    )
    .unwrap_err();
    assert!(
        missing_route.contains("TASK_ROUTE_SCOPE_REQUIRED"),
        "{missing_route}"
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn task_view_with_identity_propagates_ambiguous_current_map() {
    let (server, root, route) = board_name_fixture();
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let key = control
        .key_for_worker(&route.project_scope_id, &route.app_scope_id, "old")
        .unwrap()
        .unwrap()
        .clone();
    let slot = control.slot(&key).unwrap().clone();
    let ambiguous = binding_slot::BindingControlSet::from_slots(vec![slot.clone(), slot]);
    let state = server.state.lock().unwrap();
    let error =
        task_view_with_identity(&state, &ambiguous, Some(&route), &state.tasks["named-work"])
            .unwrap_err();
    assert!(error.contains("IDENTITY_MAP_AMBIGUOUS"), "{error}");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

fn dispatch_without_state_mutation(server: &Arc<Server>, req: Req, label: &str) -> Resp {
    let state_before = serde_json::to_vec(&*server.state.lock().unwrap()).unwrap();
    let identity_path = server
        .host_paths
        .state_root()
        .join(binding_store::BINDING_CONTROL_FILE);
    let identity_before = std::fs::read(&identity_path).unwrap();

    let response = dispatch_with_route_context(server, req, None);

    let state_after = serde_json::to_vec(&*server.state.lock().unwrap()).unwrap();
    let identity_after = std::fs::read(&identity_path).unwrap();
    assert_eq!(state_after, state_before, "{label} mutated State");
    assert_eq!(
        identity_after, identity_before,
        "{label} mutated identity-map.json"
    );
    response
}

fn assert_named_work_projection(
    view: &serde_json::Value,
    owner_name: &str,
    owner: &str,
    session_id: Option<&str>,
) {
    assert_eq!(view["id"], "named-work", "{view}");
    assert_eq!(view["owner_name"], owner_name, "{view}");
    assert_eq!(view["owner"], owner, "{view}");
    match session_id {
        Some(session_id) => assert_eq!(view["owner_session_id"], session_id, "{view}"),
        None => assert!(view["owner_session_id"].is_null(), "{view}"),
    }
}

fn named_work_list_row(response: &Resp, label: &str) -> serde_json::Value {
    response.data["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "named-work")
        .unwrap_or_else(|| panic!("{label} omitted named-work: {response:?}"))
        .clone()
}

#[test]
fn task_registry_public_queries_do_not_mutate_state() {
    let (server, root, route) = board_name_fixture();
    let server = Arc::new(server);
    for worker in server.state.lock().unwrap().workers.values_mut() {
        worker.transport = None;
    }
    let task_ids = {
        let state = server.state.lock().unwrap();
        let mut task_ids: Vec<String> = state.tasks.keys().cloned().collect();
        task_ids.sort();
        task_ids
    };
    assert_eq!(task_ids, ["named-work"]);

    for task_id in &task_ids {
        let label = format!("current-holder task status {task_id}");
        let response = dispatch_without_state_mutation(
            &server,
            Req::TaskStatus {
                task_id: Some(task_id.clone()),
            },
            &label,
        );
        assert!(response.ok, "{response:?}");
        assert_named_work_projection(&response.data, "Alice", "old", Some("session-old"));
    }

    let response = dispatch_without_state_mutation(
        &server,
        Req::TaskStatus { task_id: None },
        "current-holder task status list",
    );
    assert!(response.ok, "{response:?}");
    assert_named_work_projection(
        &named_work_list_row(&response, "current-holder task status list"),
        "Alice",
        "old",
        Some("session-old"),
    );

    let response =
        dispatch_without_state_mutation(&server, Req::StatusAll, "current-holder status --all");
    assert!(response.ok, "{response:?}");
    assert_named_work_projection(
        &named_work_list_row(&response, "current-holder status --all"),
        "Alice",
        "old",
        Some("session-old"),
    );

    binding_store::BindingControlStore::new(server.host_paths.state_root())
        .commit(binding_store::ControlChange::Unregister {
            key: binding_slot::IdentityBindingKey::new(
                route.project_scope_id.clone(),
                route.app_scope_id.clone(),
                "Alice",
            ),
            expected_generation: None,
        })
        .unwrap();

    for task_id in &task_ids {
        let label = format!("no-holder task status {task_id}");
        let response = dispatch_without_state_mutation(
            &server,
            Req::TaskStatus {
                task_id: Some(task_id.clone()),
            },
            &label,
        );
        assert!(response.ok, "{response:?}");
        assert_named_work_projection(&response.data, "Alice", "", None);
    }

    let response = dispatch_without_state_mutation(
        &server,
        Req::TaskStatus { task_id: None },
        "no-holder task status list",
    );
    assert!(response.ok, "{response:?}");
    assert_named_work_projection(
        &named_work_list_row(&response, "no-holder task status list"),
        "Alice",
        "",
        None,
    );

    let response =
        dispatch_without_state_mutation(&server, Req::StatusAll, "no-holder status --all");
    assert!(response.ok, "{response:?}");
    assert_named_work_projection(
        &named_work_list_row(&response, "no-holder status --all"),
        "Alice",
        "",
        None,
    );

    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn task_conflicts_keeps_membership_and_projects_current_holder() {
    let (server, root, route) = board_name_fixture();
    {
        let mut state = server.state.lock().unwrap();
        state.tasks.get_mut("named-work").unwrap().feature_id = Some("registry-conflict".into());
    }
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let source = control
        .key_for_worker(&route.project_scope_id, &route.app_scope_id, "current")
        .unwrap()
        .unwrap();
    let value = control
        .slot(source)
        .unwrap()
        .active_binding()
        .unwrap()
        .clone();
    store
        .commit(binding_store::ControlChange::MoveName {
            target: binding_slot::IdentityBindingKey::new(
                route.project_scope_id.clone(),
                route.app_scope_id.clone(),
                "Alice",
            ),
            value,
            source_names: vec![source.name.clone()],
            expected_generation: None,
        })
        .unwrap();

    let response = task_conflicts(&server, Some("registry-conflict".into()), None);
    assert!(response.ok, "{:?}", response.error);
    let conflicts = response.data["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1, "{:?}", response.data);
    assert_eq!(conflicts[0]["id"], "named-work");
    assert_eq!(conflicts[0]["owner_name"], "Alice");
    assert_eq!(conflicts[0]["owner"], "current");
    assert_eq!(conflicts[0]["owner_session_id"], "session-current");
    std::fs::remove_dir_all(root).unwrap();
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

fn goal_capacity_renewal_change() -> GoalScheduleChange {
    GoalScheduleChange {
        goal_id: "capacity-renewal".into(),
        goal_owner: "Alice".into(),
        desired: "active".into(),
        trigger_ms: Some(now_ms() + 60_000),
        ttl_seconds: Some(300),
        goal_subject: None,
        reason: None,
        interval_ms: Some(60_000),
        repeat_count: Some(4),
        content: Some(crate::server::state::GoalScheduleContent::Inline {
            content: "Exact retained goal instructions".into(),
        }),
        operation: GoalScheduleOperation::Explicit,
    }
}

fn seed_goal_capacity_subscriptions(
    state: &mut State,
    worker_id: &str,
    transport: &SelectedTransport,
    selected_id: &str,
) {
    let target = notification_target(transport).unwrap().as_ref().to_owned();
    let now = now_ms();
    let subscription =
        |id: &str,
         event: &str,
         subject: Option<&str>,
         trigger: i64,
         interval: Option<i64>,
         repeat: u32,
         fired: u32,
         expires: i64,
         status: &str,
         goal_source: Option<crate::server::state::GoalScheduleContent>| {
            NotificationSubscription {
                id: id.into(),
                worker_id: worker_id.into(),
                event: event.into(),
                subject: subject.map(str::to_owned),
                target: target.clone(),
                method: transport.kind.as_str().into(),
                trigger_ms: Some(trigger),
                trigger_times_ms: Vec::new(),
                interval_ms: interval,
                repeat_count: repeat,
                fired_count: fired,
                expires_ms: expires,
                status: status.into(),
                created_ms: now,
                updated_ms: now,
                status_reason: None,
                patrol_idle_since_ms: None,
                patrol_last_push_ms: 0,
                periodic_declared: interval.is_some(),
                goal_content: goal_source.map(|source| crate::server::state::GoalScheduleBinding {
                    source: Some(source),
                    ttl_seconds: 3600,
                }),
            }
        };
    for record in [
        subscription(
            "capacity-direct-message",
            "direct-message",
            None,
            now + 1,
            None,
            1,
            0,
            now + 3_600_000,
            "armed",
            None,
        ),
        subscription(
            "capacity-project-patrol",
            "deadline",
            Some("project-patrol:old"),
            now + 1_500_000,
            Some(1_500_000),
            100,
            27,
            now + 86_400_000,
            "armed",
            Some(crate::server::state::GoalScheduleContent::Inline {
                content: "standing project patrol".into(),
            }),
        ),
        subscription(
            "capacity-other-goal",
            "deadline",
            Some("goal:other-capacity-goal"),
            now + 60_000,
            Some(60_000),
            3,
            1,
            now + 3_600_000,
            "armed",
            Some(crate::server::state::GoalScheduleContent::Inline {
                content: "retain other goal".into(),
            }),
        ),
        subscription(
            "capacity-other-task-goal",
            "deadline",
            Some("goal:task:other-capacity-task"),
            now + 120_000,
            Some(120_000),
            3,
            1,
            now + 3_600_000,
            "armed",
            Some(crate::server::state::GoalScheduleContent::BoardTask {
                task_id: "other-capacity-task".into(),
            }),
        ),
        subscription(
            selected_id,
            "deadline",
            Some("goal:capacity-renewal"),
            now - 60_000,
            Some(60_000),
            3,
            3,
            now + 3_600_000,
            "consumed",
            Some(crate::server::state::GoalScheduleContent::Inline {
                content: "Exact retained goal instructions".into(),
            }),
        ),
    ] {
        state
            .notification_subscriptions
            .insert(record.id.clone(), record);
    }
}

fn goal_capacity_fixture() -> (
    Server,
    std::path::PathBuf,
    String,
    crate::server::state::WorkerRec,
) {
    let (server, root, _) = board_name_fixture();
    let selected_id = "capacity-selected-consumed".to_owned();
    let worker = {
        let mut state = server.state.lock().unwrap();
        let worker = state.workers["old"].clone();
        let transport = selected_transport_for_worker(&worker).unwrap();
        seed_goal_capacity_subscriptions(&mut state, "old", &transport, selected_id.as_str());
        worker
    };
    (server, root, selected_id, worker)
}

#[test]
fn goal_capacity_missing_runtime_binding_rejects_without_schedule_mutation() {
    let (server, root, _, worker) = goal_capacity_fixture();
    let global = server.state.lock().unwrap().global.clone();
    let positive = handle_goal_schedule_for_actor(
        &server,
        &worker.id,
        &worker.token,
        goal_capacity_renewal_change(),
    );
    assert!(positive.ok, "{positive:?}");
    // Preserve credentials and the current identity map. Lose only the
    // business runtime binding, exactly the earlier SEND_BINDING_REJECTED
    // boundary. No transport observation is part of goal admission.
    for project in server.state.lock().unwrap().global.projects.values_mut() {
        project.runtime_bindings.clear();
    }
    let map = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let map_before = map.load().unwrap();
    let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    let negative = handle_goal_schedule_for_actor(
        &server,
        &worker.id,
        &worker.token,
        goal_capacity_renewal_change(),
    );
    assert_eq!(
        negative.error.as_deref(),
        Some("SEND_BINDING_REJECTED: authoritative runtime binding is missing")
    );
    assert_eq!(
        serde_json::to_value(&*server.state.lock().unwrap()).unwrap(),
        before
    );
    assert_eq!(map.load().unwrap(), map_before);
    server.state.lock().unwrap().global = global;
    let recovered = handle_goal_schedule_for_actor(
        &server,
        &worker.id,
        &worker.token,
        goal_capacity_renewal_change(),
    );
    assert!(recovered.ok, "{recovered:?}");
    assert_eq!(
        recovered.data["subscription"]["id"],
        positive.data["subscription"]["id"]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_capacity_renewal_preserves_other_subscriptions() {
    let (server, root, selected_id, worker) = goal_capacity_fixture();
    let (selected_before, task_before, assignment_before, other_before) = {
        let state = server.state.lock().unwrap();
        (
            state.notification_subscriptions[&selected_id].clone(),
            serde_json::to_value(&state.tasks["named-work"]).unwrap(),
            serde_json::to_value(state.task_assignment("named-work").unwrap()).unwrap(),
            state
                .notification_subscriptions
                .iter()
                .filter(|(id, _)| *id != &selected_id)
                .map(|(id, subscription)| (id.clone(), subscription.clone()))
                .collect::<Vec<_>>(),
        )
    };

    let response = handle_goal_schedule_for_actor(
        &server,
        &worker.id,
        &worker.token,
        goal_capacity_renewal_change(),
    );
    assert!(
        response.ok,
        "authorized renewal must not be blocked by unrelated subscriptions: {response:?}"
    );
    let renewed_id = response.data["subscription"]["id"]
        .as_str()
        .expect("renewal response has subscription id")
        .to_owned();
    assert_ne!(renewed_id, selected_id);

    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions[&selected_id],
        selected_before
    );
    assert_eq!(
        serde_json::to_value(&state.tasks["named-work"]).unwrap(),
        task_before
    );
    assert_eq!(
        serde_json::to_value(state.task_assignment("named-work").unwrap()).unwrap(),
        assignment_before
    );
    for (id, subscription) in other_before {
        assert_eq!(state.notification_subscriptions[&id], subscription);
    }
    let renewed = &state.notification_subscriptions[&renewed_id];
    assert_eq!(renewed.status, "armed");
    assert_eq!(renewed.fired_count, 0);
    assert_eq!(renewed.repeat_count, 4);
    assert_eq!(
        state
            .notification_subscriptions
            .values()
            .filter(|subscription| subscription.subject.as_deref() == Some("goal:capacity-renewal"))
            .count(),
        2,
        "renewal must retain the consumed record and create exactly one armed batch"
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_capacity_active_reconcile_is_idempotent() {
    let (server, root, selected_id, worker) = goal_capacity_fixture();
    let first = handle_goal_schedule_for_actor(
        &server,
        &worker.id,
        &worker.token,
        goal_capacity_renewal_change(),
    );
    assert!(first.ok, "{first:?}");
    let renewed_id = first.data["subscription"]["id"]
        .as_str()
        .expect("renewal response has subscription id")
        .to_owned();
    assert_ne!(renewed_id, selected_id);
    let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();

    let second = handle_goal_schedule_for_actor(
        &server,
        &worker.id,
        &worker.token,
        goal_capacity_renewal_change(),
    );
    assert!(second.ok, "{second:?}");
    assert_eq!(second.data["subscription"]["id"], renewed_id);
    let after = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    assert_eq!(
        after, before,
        "repeat reconcile must not add or mutate state"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_capacity_invalid_schedule_is_non_mutating() {
    let (server, root, _selected_id, worker) = goal_capacity_fixture();
    let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    let mut change = goal_capacity_renewal_change();
    change.trigger_ms = Some(now_ms() + 300_000);
    change.ttl_seconds = Some(60);

    let response = handle_goal_schedule_for_actor(&server, &worker.id, &worker.token, change);
    assert!(!response.ok, "{response:?}");
    assert_eq!(
        response.error.as_deref(),
        Some("goal deadline trigger must be in the future and before expiry")
    );
    let after = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    assert_eq!(after, before, "invalid schedule must not mutate any state");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_capacity_unauthorized_actor_is_non_mutating() {
    let (server, root, _selected_id, _worker) = goal_capacity_fixture();
    let registered =
        crate::server::peer_tests::register_synthetic_peer(&server, "intruder", "intruder");
    assert!(registered.ok, "{registered:?}");
    let intruder = server.state.lock().unwrap().workers["intruder"].clone();
    let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();

    let response = handle_goal_schedule_for_actor(
        &server,
        &intruder.id,
        &intruder.token,
        goal_capacity_renewal_change(),
    );
    assert!(!response.ok, "{response:?}");
    assert_eq!(
        response.error.as_deref(),
        Some("GOAL_SCHEDULE_OWNER_MISMATCH: only the owner, project master or current CEO may manage this goal")
    );
    let after = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    assert_eq!(
        after, before,
        "unauthorized renewal must not mutate any state"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_capacity_keeps_generic_notification_gate() {
    let (server, root, _selected_id, worker) = goal_capacity_fixture();
    let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    let response = handle_notification_subscribe(
        &server,
        worker.id,
        worker.token,
        "direct-message".into(),
        None,
        None,
        Vec::new(),
        None,
        1,
        3600,
    );
    assert!(!response.ok, "{response:?}");
    assert_eq!(
        response.error.as_deref(),
        Some("maximum 3 active subscriptions per agent")
    );
    let after = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    assert_eq!(
        after, before,
        "generic capacity rejection must be non-mutating"
    );
    std::fs::remove_dir_all(root).unwrap();
}
