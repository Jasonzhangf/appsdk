use super::*;

#[test]
fn timer_state_execution_schedule_requires_typed_running_phase() {
    use ticket_state::{TicketPhaseKind, TicketPhaseRecord, TicketPhaseStatus};
    let mut state = State::default();
    let task: TaskRec = serde_json::from_value(json!({
        "id":"timer-start", "owner":"owner", "created_by":"master",
        "status":"invited", "created_ms":1, "updated_ms":1
    }))
    .unwrap();
    let mut details = crate::board::BoardTaskDetails::legacy(&task.id, true);
    details.invitation = Some(
        serde_json::from_value(json!({
            "peer_id":"invitee", "binding_id":"binding", "message_id":"invite",
            "created_ms":1
        }))
        .unwrap(),
    );
    assert!(
        task_schedule_selection(&state, &task, &details).is_none(),
        "a pending invitation must not select an Execute timer owner"
    );
    let mut task = task;
    task.status = "working".into();
    details.invitation = None;
    let mut phase = TicketPhaseRecord {
        task_id: task.id.clone(),
        phase: TicketPhaseKind::Execute,
        batch: 1,
        priority: "p2".into(),
        status: TicketPhaseStatus::Claimed,
        slot_name: Some("Alice".into()),
        binding_generation: 1,
        enqueue_seq: 1,
        attempt_generation: 2,
        depends_on: vec![],
        route_group: None,
        wait_reason: None,
        result: None,
    };
    state.ticket_phases.push(phase.clone());
    assert!(
        task_schedule_selection(&state, &task, &details).is_none(),
        "acceptance and claim are not typed start"
    );
    phase.status = TicketPhaseStatus::Running;
    state.ticket_phases[0] = phase;
    assert!(matches!(task_schedule_selection(&state, &task, &details),
        Some(TaskScheduleSelection::Name(name)) if name == "Alice"));
    for status in [
        TicketPhaseStatus::WaitingAssignment,
        TicketPhaseStatus::Queued,
        TicketPhaseStatus::Completed,
        TicketPhaseStatus::Rework,
        TicketPhaseStatus::Revoked,
    ] {
        state.ticket_phases[0].status = status;
        assert!(task_schedule_selection(&state, &task, &details).is_none());
    }
    let mut older = state.ticket_phases[0].clone();
    older.batch = 0;
    older.status = TicketPhaseStatus::Running;
    state.ticket_phases.push(older);
    assert!(
        task_schedule_selection(&state, &task, &details).is_none(),
        "a revoked latest batch must not expose an older Running phase"
    );
    state.ticket_phases.clear();
    assert!(
        task_schedule_selection(&state, &task, &details).is_none(),
        "legacy working status alone must not start execution cadence"
    );
}

fn edit_worker(state: &mut State, id: &str) {
    state.workers.insert(
        id.into(),
        WorkerRec {
            id: id.into(),
            token: format!("token-{id}"),
            cwd: "/tmp/goal-edit-fixture".into(),
            registered_ms: now_ms(),
            transport: Some(peer_tests::test_appserver_transport(id)),
        },
    );
}

#[test]
fn timer_state_running_transition_arms_once_and_clears_execution_residue() {
    use ticket_state::{TicketPhaseKind, TicketPhaseRecord, TicketPhaseStatus};
    let (server, root) = peer_tests::test_server();
    let registered = peer_tests::register_synthetic_peer(&server, "timer-owner", "timer-owner");
    assert!(registered.ok, "{registered:?}");
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let mut before = server.state.lock().unwrap().clone();
    let route = server_route_scope(&server, &before).unwrap().unwrap();
    let slot = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "timer-owner")
        .unwrap()
        .unwrap();
    let task: TaskRec = serde_json::from_value(json!({
        "id":"timer-lifecycle", "owner":"timer-owner", "created_by":"timer-owner",
        "status":"working", "created_ms":1, "updated_ms":1
    }))
    .unwrap();
    before
        .apply_checked(&Event::TaskCreated { task: task.clone() })
        .unwrap();
    let mut details = before.board_details[&task.id].clone();
    details.long_horizon = Some(crate::board::LongHorizonTaskSpec {
        content: "Run the current attempt".into(),
        interval_ms: 60_000,
        repeat_count: 5,
        ttl_seconds: 600,
    });
    before
        .apply_checked(&Event::BoardDetailsChanged {
            task_id: task.id.clone(),
            details: details.clone(),
        })
        .unwrap();
    let mut phase = TicketPhaseRecord {
        task_id: task.id.clone(),
        phase: TicketPhaseKind::Execute,
        batch: 1,
        priority: "p2".into(),
        status: TicketPhaseStatus::Claimed,
        slot_name: Some(slot.identity_key.name.clone()),
        binding_generation: slot.generation,
        enqueue_seq: 1,
        attempt_generation: 2,
        depends_on: vec![],
        route_group: None,
        wait_reason: None,
        result: None,
    };
    before
        .apply_checked(&Event::TicketPhaseRecorded {
            phase: phase.clone(),
        })
        .unwrap();
    let true_schedule = edit_subscription("true-goal", "timer-owner", "armed", now_ms());
    before
        .notification_subscriptions
        .insert(true_schedule.id.clone(), true_schedule.clone());
    let unaffected = before.notification_subscriptions.clone();
    let claimed = server
        .checked_candidate_with_goal(
            &before,
            &[Event::TicketPhaseRecorded {
                phase: phase.clone(),
            }],
            Some(&control),
        )
        .unwrap();
    assert_eq!(claimed.notification_subscriptions, unaffected);
    phase.status = TicketPhaseStatus::Running;
    let running = server
        .checked_candidate_with_goal(
            &claimed,
            &[Event::TicketPhaseRecorded {
                phase: phase.clone(),
            }],
            Some(&control),
        )
        .unwrap();
    let receipt = task_goal_receipt(&running, &task.id);
    assert_eq!(receipt["armed"], true);
    let id = receipt["subscription"]["id"].as_str().unwrap();
    let repeated = server
        .checked_candidate_with_goal(
            &running,
            &[Event::TicketPhaseRecorded {
                phase: phase.clone(),
            }],
            Some(&control),
        )
        .unwrap();
    assert_eq!(
        repeated.notification_subscriptions,
        running.notification_subscriptions
    );
    assert_eq!(repeated.ticket_phases, running.ticket_phases);

    let mut with_residue = running.clone();
    let message: Message = serde_json::from_value(json!({
        "id":"execution-residue", "from":"daemon", "to":"timer-owner", "type":"notify",
        "subject":"deadline:goal:task:timer-lifecycle", "body":"old execution notice",
        "created_ms":1, "state":"pending"
    }))
    .unwrap();
    with_residue
        .apply_checked(&Event::Sent { msg: message })
        .unwrap();
    with_residue
        .apply_checked(&Event::WakeBound {
            message_id: "execution-residue".into(),
            subscription_id: id.into(),
        })
        .unwrap();
    for terminal in ["decline", "withdraw", "revoke", "closed", "cancelled"] {
        let mut task = task.clone();
        let mut changes = Vec::new();
        match terminal {
            "decline" | "withdraw" => {
                task.status = "pending".into();
                changes.push(Event::TaskUpdated { task: task.clone() });
                changes.push(Event::BoardDetailsChanged {
                    task_id: task.id.clone(),
                    details: details.clone(),
                });
            }
            "revoke" => {
                let mut revoked = phase.clone();
                revoked.status = TicketPhaseStatus::Revoked;
                changes.push(Event::TicketPhaseRecorded { phase: revoked });
            }
            status => {
                task.status = status.into();
                changes.push(Event::TaskUpdated { task: task.clone() });
            }
        }
        let ended = server
            .checked_candidate_with_goal(&with_residue, &changes, Some(&control))
            .unwrap();
        assert_eq!(
            ended.notification_subscriptions[id].status, "cancelled",
            "{terminal}"
        );
        assert!(
            ended
                .msgs
                .get("execution-residue")
                .is_none_or(|msg| msg.state != "pending"),
            "{terminal}"
        );
        for (id, original) in &unaffected {
            assert_eq!(
                &ended.notification_subscriptions[id], original,
                "{terminal}: {id}"
            );
        }
        let retry = server
            .checked_candidate_with_goal(&ended, &changes, Some(&control))
            .unwrap();
        assert_eq!(
            retry.notification_subscriptions, ended.notification_subscriptions,
            "{terminal}"
        );
        if terminal == "revoke" {
            phase.batch += 1;
            let restarted = server
                .checked_candidate_with_goal(
                    &ended,
                    &[Event::TicketPhaseRecorded {
                        phase: phase.clone(),
                    }],
                    Some(&control),
                )
                .unwrap();
            assert_eq!(task_goal_receipt(&restarted, &task.id)["armed"], true);
            assert_eq!(
                restarted
                    .notification_subscriptions
                    .values()
                    .filter(
                        |sub| sub.subject.as_deref() == Some("goal:task:timer-lifecycle")
                            && sub.status == "armed"
                    )
                    .count(),
                1
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

fn edit_subscription(
    id: &str,
    worker: &str,
    status: &str,
    created_ms: i64,
) -> NotificationSubscription {
    NotificationSubscription {
        id: id.into(),
        worker_id: worker.into(),
        event: "deadline".into(),
        subject: Some("goal:goal-edit".into()),
        target: format!("thread-{worker}"),
        method: "appserver".into(),
        trigger_ms: Some(created_ms + 60_000),
        trigger_times_ms: Vec::new(),
        interval_ms: Some(60_000),
        repeat_count: 5,
        fired_count: 2,
        expires_ms: created_ms + 600_000,
        status: status.into(),
        created_ms,
        updated_ms: created_ms,
        status_reason: None,
        patrol_idle_since_ms: None,
        patrol_last_push_ms: 0,
        periodic_declared: true,
        goal_content: Some(GoalScheduleBinding {
            source: Some(GoalScheduleContent::Inline {
                content: "old content".into(),
            }),
            ttl_seconds: 600,
        }),
    }
}

fn terminal_edit_change() -> GoalScheduleChange {
    GoalScheduleChange {
        goal_id: "goal-edit".into(),
        goal_owner: "edit-worker".into(),
        desired: "active".into(),
        trigger_ms: None,
        ttl_seconds: None,
        goal_subject: None,
        expected_subscription_id: None,
        reason: None,
        interval_ms: Some(90_000),
        repeat_count: Some(4),
        content: Some(GoalScheduleContent::Inline {
            content: "edited content".into(),
        }),
        operation: GoalScheduleOperation::Edit,
    }
}

fn content_edit_change(content: &str) -> GoalScheduleChange {
    GoalScheduleChange {
        goal_id: "goal-edit".into(),
        goal_owner: "edit-worker".into(),
        desired: "active".into(),
        trigger_ms: None,
        ttl_seconds: None,
        goal_subject: None,
        expected_subscription_id: None,
        reason: None,
        interval_ms: None,
        repeat_count: None,
        content: Some(GoalScheduleContent::Inline {
            content: content.into(),
        }),
        operation: GoalScheduleOperation::Edit,
    }
}

fn edited_subscription(events: &[Event], expected_id: &str) -> NotificationSubscription {
    let subscriptions: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::NotificationSubscribed { subscription } => Some(subscription),
            _ => None,
        })
        .collect();
    assert_eq!(subscriptions.len(), 1, "{events:?}");
    assert_eq!(subscriptions[0].id, expected_id);
    subscriptions[0].clone()
}

#[test]
fn goal_edit_terminal_subscription_updates_metadata_without_rearm_red() {
    let now = now_ms();
    let mut state = State::default();
    edit_worker(&mut state, "edit-worker");
    let terminal = edit_subscription("sub-terminal", "edit-worker", "consumed", now - 120_000);
    state
        .notification_subscriptions
        .insert(terminal.id.clone(), terminal.clone());

    let (events, response) =
        prepare_goal_schedule(&state, "edit-worker", &terminal_edit_change(), now)
            .unwrap_or_else(|error| panic!("terminal edit failed: {:?}", error.error));
    let edited = edited_subscription(&events, &terminal.id);
    assert_eq!(edited.status, "consumed");
    assert_eq!(edited.fired_count, terminal.fired_count);
    assert_eq!(edited.trigger_ms, terminal.trigger_ms);
    assert_eq!(edited.expires_ms, terminal.expires_ms);
    assert_eq!(edited.interval_ms, Some(90_000));
    assert_eq!(edited.repeat_count, 4);
    assert_eq!(
        edited
            .goal_content
            .as_ref()
            .and_then(|binding| binding.source.as_ref()),
        Some(&GoalScheduleContent::Inline {
            content: "edited content".into(),
        })
    );
    assert_eq!(response.data["status"], "consumed");

    for event in &events {
        state.apply_checked(event).unwrap();
    }
    let (repeated_events, repeated_response) =
        prepare_goal_schedule(&state, "edit-worker", &terminal_edit_change(), now).unwrap();
    assert!(repeated_events.is_empty(), "{repeated_events:?}");
    assert_eq!(repeated_response.data["deduplicated"], true);
}

#[test]
fn goal_edit_current_batch_keeps_identity_progress_and_is_idempotent() {
    let now = now_ms();
    let mut state = State::default();
    edit_worker(&mut state, "edit-worker");
    let current = edit_subscription("sub-current", "edit-worker", "armed", now - 60_000);
    state
        .notification_subscriptions
        .insert(current.id.clone(), current.clone());

    let change = content_edit_change("edited content");
    let (events, response) = prepare_goal_schedule(&state, "edit-worker", &change, now).unwrap();
    let edited = edited_subscription(&events, &current.id);
    assert_eq!(edited.status, "armed");
    assert_eq!(edited.fired_count, current.fired_count);
    assert_eq!(edited.trigger_ms, current.trigger_ms);
    assert_eq!(edited.expires_ms, current.expires_ms);
    assert_eq!(edited.interval_ms, current.interval_ms);
    assert_eq!(edited.repeat_count, current.repeat_count);
    assert_eq!(response.data["status"], "armed");
    assert_eq!(
        state.notification_subscriptions.len(),
        1,
        "edit must not create a replacement batch"
    );

    for event in &events {
        state.apply_checked(event).unwrap();
    }
    let (repeated_events, repeated_response) =
        prepare_goal_schedule(&state, "edit-worker", &change, now).unwrap();
    assert!(repeated_events.is_empty(), "{repeated_events:?}");
    assert_eq!(repeated_response.data["deduplicated"], true);
}

#[test]
fn repeat_only_edit_retracts_unsubmitted_reminder_and_preserves_other_receipts() {
    let now = now_ms();
    let mut state = State::default();
    edit_worker(&mut state, "edit-worker");
    let before = edit_subscription("sub-edit-repeat", "edit-worker", "armed", now);
    state.notification_subscriptions.insert(before.id.clone(), before.clone());
    for (id, attempts) in [("unsent", 0), ("uncertain", 1), ("accepted", 1)] {
        state.apply(&Event::Sent { msg: Message {
            id: id.into(), from: "collab-server".into(), to: "edit-worker".into(),
            mtype: "notification".into(), subject: Some("deadline:goal:goal-edit".into()),
            body: "Cycle 3/5".into(), in_reply_to: None, created_ms: now,
            state: "pending".into(), wake_attempt_count: attempts,
            last_wake_attempt_ms: now, retry_attempted: false,
        }});
        state.apply(&Event::WakeBound { message_id: id.into(), subscription_id: before.id.clone() });
        state.apply(&Event::NotificationOccurrenceIssued {
            message_id: id.into(), subscription_id: before.id.clone(),
            trigger_ms: before.trigger_ms.unwrap(), idle_triggered: true,
        });
    }
    state.notification_delivery_accepted.insert("accepted".into(), now);
    let mut change = content_edit_change("old content");
    change.content = None;
    change.repeat_count = Some(7);
    let (events, response) = prepare_goal_schedule(&state, "edit-worker", &change, now + 1).unwrap();
    assert_eq!(response.data["superseded_message_ids"], json!(["unsent"]));
    for event in &events {
        state.apply(event);
    }
    let after = &state.notification_subscriptions[&before.id];
    assert_eq!(after.repeat_count, 7);
    assert_eq!(after.fired_count, before.fired_count);
    assert_eq!(after.trigger_ms, before.trigger_ms);
    assert_eq!(state.msgs["unsent"].state, "superseded");
    assert_eq!(state.msgs["uncertain"].state, "pending");
    assert_eq!(state.msgs["accepted"].state, "pending");
}

#[test]
fn goal_edit_does_not_rearm_a_lost_or_elapsed_batch() {
    let now = now_ms();
    for (created_ms, status) in [(now - 60_000, "transport-lost"), (now - 610_000, "expired")] {
        let mut state = State::default();
        edit_worker(&mut state, "edit-worker");
        let current = edit_subscription("sub-current", "edit-worker", "transport-lost", created_ms);
        state
            .notification_subscriptions
            .insert(current.id.clone(), current.clone());
        let (events, response) = prepare_goal_schedule(
            &state,
            "edit-worker",
            &content_edit_change("updated while unavailable"),
            now,
        )
        .unwrap();
        let edited = edited_subscription(&events, &current.id);
        assert_eq!(edited.status, status);
        assert_eq!(response.data["status"], status);
        assert_eq!(edited.trigger_ms, current.trigger_ms);
        assert_eq!(edited.expires_ms, current.expires_ms);
        assert_eq!(edited.fired_count, current.fired_count);
        assert_eq!(edited.target, current.target);
    }
}

#[test]
fn goal_edit_prefers_the_current_batch_over_renewal_history() {
    let now = now_ms();
    let mut state = State::default();
    edit_worker(&mut state, "edit-worker");
    let terminal = edit_subscription("sub-terminal", "edit-worker", "consumed", now - 180_000);
    let current = edit_subscription("sub-current", "edit-worker", "armed", now - 60_000);
    for subscription in [terminal.clone(), current.clone()] {
        state
            .notification_subscriptions
            .insert(subscription.id.clone(), subscription);
    }

    let (events, _) = prepare_goal_schedule(
        &state,
        "edit-worker",
        &content_edit_change("current content"),
        now,
    )
    .unwrap();
    let edited = edited_subscription(&events, &current.id);
    assert_eq!(edited.status, "armed");
    assert_eq!(edited.fired_count, current.fired_count);
    assert_eq!(
        state.notification_subscriptions[&terminal.id], terminal,
        "editing the current batch must preserve renewal history"
    );
}

#[test]
fn goal_edit_terminal_fallback_selects_the_newest_batch_only() {
    let now = now_ms();
    let mut state = State::default();
    edit_worker(&mut state, "edit-worker");
    let older = edit_subscription("sub-older", "edit-worker", "consumed", now - 180_000);
    let newer = edit_subscription("sub-newer", "edit-worker", "expired", now - 60_000);
    for subscription in [older.clone(), newer.clone()] {
        state
            .notification_subscriptions
            .insert(subscription.id.clone(), subscription);
    }

    let (events, response) = prepare_goal_schedule(
        &state,
        "edit-worker",
        &content_edit_change("terminal content"),
        now,
    )
    .unwrap();
    let edited = edited_subscription(&events, &newer.id);
    assert_eq!(edited.status, "expired");
    assert_eq!(edited.trigger_ms, newer.trigger_ms);
    assert_eq!(edited.expires_ms, newer.expires_ms);
    assert_eq!(response.data["status"], "expired");
    assert_eq!(state.notification_subscriptions[&older.id], older);
}

#[test]
fn goal_edit_missing_target_and_unknown_operation_are_explicit() {
    let now = now_ms();
    let mut state = State::default();
    edit_worker(&mut state, "edit-worker");
    let missing =
        prepare_goal_schedule(&state, "edit-worker", &content_edit_change("missing"), now)
            .unwrap_err();
    assert_eq!(
        missing.error.as_deref(),
        Some("GOAL_SCHEDULE_TARGET_NOT_FOUND: edit requires an existing exact goal")
    );
    let unknown = match GoalScheduleOperation::from_request(Some("renew".into())) {
        Ok(_) => panic!("unknown operation must be rejected"),
        Err(error) => error,
    };
    assert_eq!(
        unknown.error.as_deref(),
        Some("GOAL_SCHEDULE_OPERATION_INVALID: unknown operation renew")
    );
}

#[test]
fn goal_edit_task_details_and_schedule_commit_together() {
    use crate::board::LongHorizonTaskSpec;

    let (server, root) = peer_tests::test_server();
    let registered = peer_tests::register_synthetic_peer(&server, "task-edit", "task-edit");
    assert!(registered.ok, "{registered:?}");
    let mut state = server.state.lock().unwrap();
    let task = TaskRec {
        id: "task-edit".into(),
        owner: "task-edit".into(),
        created_by: "task-edit".into(),
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
    };
    state.apply_checked(&Event::TaskCreated { task }).unwrap();
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let route = server_route_scope(&server, &state).unwrap().unwrap();
    let slot = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "task-edit")
        .unwrap()
        .unwrap();
    state.ticket_phases.push(ticket_state::TicketPhaseRecord {
        task_id: "task-edit".into(),
        phase: ticket_state::TicketPhaseKind::Execute,
        batch: 1,
        priority: "p2".into(),
        status: ticket_state::TicketPhaseStatus::Running,
        slot_name: Some(slot.identity_key.name.clone()),
        binding_generation: slot.generation,
        enqueue_seq: 1,
        attempt_generation: 1,
        depends_on: vec![],
        route_group: None,
        wait_reason: None,
        result: None,
    });
    let mut details = state.board_details["task-edit"].clone();
    details.long_horizon = Some(LongHorizonTaskSpec {
        content: "original task goal".into(),
        interval_ms: 60_000,
        repeat_count: 5,
        ttl_seconds: 600,
    });
    state
        .apply_checked(&Event::BoardDetailsChanged {
            task_id: "task-edit".into(),
            details,
        })
        .unwrap();
    let created = commit_goal_schedule(
        &server,
        &mut state,
        "task-edit",
        GoalScheduleChange {
            goal_id: "task:task-edit".into(),
            goal_owner: "task-edit".into(),
            desired: "active".into(),
            trigger_ms: None,
            ttl_seconds: Some(600),
            goal_subject: None,
            expected_subscription_id: None,
            reason: None,
            interval_ms: Some(60_000),
            repeat_count: Some(5),
            content: Some(GoalScheduleContent::BoardTask {
                task_id: "task-edit".into(),
            }),
            operation: GoalScheduleOperation::Explicit,
        },
    );
    assert!(created.ok, "{created:?}");
    let subscription_id = state
        .notification_subscriptions
        .values()
        .find(|subscription| subscription.subject.as_deref() == Some("goal:task:task-edit"))
        .unwrap()
        .id
        .clone();
    let edited = commit_goal_schedule(
        &server,
        &mut state,
        "task-edit",
        GoalScheduleChange {
            goal_id: "task:task-edit".into(),
            goal_owner: "task-edit".into(),
            desired: "active".into(),
            trigger_ms: None,
            ttl_seconds: None,
            goal_subject: None,
            expected_subscription_id: None,
            reason: None,
            interval_ms: Some(90_000),
            repeat_count: Some(4),
            content: Some(GoalScheduleContent::Inline {
                content: "edited task goal".into(),
            }),
            operation: GoalScheduleOperation::Edit,
        },
    );
    assert!(edited.ok, "{edited:?}");
    assert_eq!(
        state.board_details["task-edit"]
            .long_horizon
            .as_ref()
            .unwrap()
            .content,
        "edited task goal"
    );
    let edited_subscription = &state.notification_subscriptions[&subscription_id];
    assert_eq!(edited_subscription.interval_ms, Some(90_000));
    assert_eq!(edited_subscription.repeat_count, 4);
    assert_eq!(
        edited_subscription.goal_content.as_ref().unwrap().source,
        Some(GoalScheduleContent::BoardTask {
            task_id: "task-edit".into(),
        })
    );
    let details_before_failure = state.board_details["task-edit"].clone();
    let subscription_before_failure = edited_subscription.clone();

    let rejected = commit_goal_schedule(
        &server,
        &mut state,
        "task-edit",
        GoalScheduleChange {
            goal_id: "task:task-edit".into(),
            goal_owner: "task-edit".into(),
            desired: "active".into(),
            trigger_ms: None,
            ttl_seconds: None,
            goal_subject: None,
            expected_subscription_id: None,
            reason: None,
            interval_ms: Some(0),
            repeat_count: Some(2),
            content: Some(GoalScheduleContent::Inline {
                content: "must not commit".into(),
            }),
            operation: GoalScheduleOperation::Edit,
        },
    );
    assert!(!rejected.ok);
    assert_eq!(
        serde_json::to_value(&state.board_details["task-edit"]).unwrap(),
        serde_json::to_value(&details_before_failure).unwrap()
    );
    assert_eq!(
        state.notification_subscriptions[&subscription_id],
        subscription_before_failure
    );
    let pending_id = "pending-task-edit";
    server.commit_locked_checked(&mut state, &[
        Event::Sent { msg: Message {
            id: pending_id.into(), from: "collab-server".into(), to: "task-edit".into(),
            mtype: "notification".into(), subject: Some("deadline:goal:task:task-edit".into()),
            body: "edited task goal".into(), in_reply_to: None, created_ms: now_ms(),
            state: "pending".into(), wake_attempt_count: 0, last_wake_attempt_ms: 0,
            retry_attempted: false,
        }},
        Event::WakeBound { message_id: pending_id.into(), subscription_id: subscription_id.clone() },
        Event::NotificationOccurrenceIssued {
            message_id: pending_id.into(), subscription_id: subscription_id.clone(),
            trigger_ms: subscription_before_failure.trigger_ms.unwrap(), idle_triggered: true,
        },
    ]).unwrap();
    let mut described = details_before_failure.clone();
    described.revision += 1;
    described.delivery_condition = "Verify the revised acceptance condition.".into();
    assert!(state.notification_subscriptions[&subscription_id].periodic_declared);
    assert!(notification_provably_unsubmitted(&state, pending_id));
    assert_eq!(retractable_messages_for_subscription(&state, &subscription_id), vec![pending_id]);
    server.commit_locked_checked(&mut state, &[Event::BoardDetailsChanged {
        task_id: "task-edit".into(), details: described.clone(),
    }]).unwrap();
    assert_eq!(state.msgs[pending_id].state, "superseded");
    assert_eq!(state.board_details["task-edit"].delivery_condition, described.delivery_condition);
    assert_eq!(state.notification_subscriptions[&subscription_id], subscription_before_failure);
    let details_before_failure = described;
    state.tasks.get_mut("task-edit").unwrap().status = "closed".into();
    let mut closed_edit = content_edit_change("must not change closed task");
    closed_edit.goal_id = "task:task-edit".into();
    closed_edit.goal_owner = "task-edit".into();
    let closed = commit_goal_schedule(&server, &mut state, "task-edit", closed_edit);
    assert!(closed
        .error
        .as_deref()
        .unwrap()
        .contains("GOAL_SCHEDULE_TASK_TERMINAL"));
    assert_eq!(
        serde_json::to_value(&state.board_details["task-edit"]).unwrap(),
        serde_json::to_value(&details_before_failure).unwrap()
    );
    assert_eq!(
        state.notification_subscriptions[&subscription_id],
        subscription_before_failure
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn goal_edit_multiple_current_batches_are_ambiguous_and_non_mutating_red() {
    let now = now_ms();
    let mut state = State::default();
    edit_worker(&mut state, "edit-worker");
    for (id, created) in [
        ("sub-current-a", now - 120_000),
        ("sub-current-b", now - 60_000),
    ] {
        let subscription = edit_subscription(id, "edit-worker", "armed", created);
        state
            .notification_subscriptions
            .insert(subscription.id.clone(), subscription);
    }
    let before = state.notification_subscriptions.clone();

    let result = prepare_goal_schedule(&state, "edit-worker", &terminal_edit_change(), now);
    let error = result
        .err()
        .expect("multiple current batches must be an explicit ambiguity denial");
    assert_eq!(
        error.error.as_deref(),
        Some("GOAL_SCHEDULE_TARGET_AMBIGUOUS: edit requires one current goal")
    );
    assert_eq!(state.notification_subscriptions, before);
}

#[test]
fn explicit_task_goal_renewal_creates_a_new_batch_and_preserves_terminal_history() {
    let now = now_ms();
    let mut state = State::default();
    edit_worker(&mut state, "edit-worker");
    let mut terminal = edit_subscription("sub-terminal", "edit-worker", "consumed", now - 120_000);
    terminal.subject = Some("goal:task:renew-me".into());
    terminal.goal_content = Some(GoalScheduleBinding {
        source: Some(GoalScheduleContent::BoardTask {
            task_id: "renew-me".into(),
        }),
        ttl_seconds: 600,
    });
    state
        .notification_subscriptions
        .insert(terminal.id.clone(), terminal.clone());

    let mut derived = terminal_edit_change();
    derived.goal_id = "task:renew-me".into();
    derived.operation = GoalScheduleOperation::Derived;
    derived.content = terminal.goal_content.as_ref().unwrap().source.clone();
    let mut elapsed = terminal.clone();
    elapsed.status = "armed".into();
    elapsed.expires_ms = now - 1;
    state
        .notification_subscriptions
        .insert(elapsed.id.clone(), elapsed);
    let (derived_events, _) = prepare_goal_schedule(&state, "edit-worker", &derived, now).unwrap();
    assert!(derived_events.is_empty(), "{derived_events:?}");
    state
        .notification_subscriptions
        .insert(terminal.id.clone(), terminal.clone());

    let (events, response) = prepare_goal_schedule(
        &state,
        "edit-worker",
        &GoalScheduleChange {
            goal_id: "task:renew-me".into(),
            goal_owner: "edit-worker".into(),
            desired: "active".into(),
            trigger_ms: None,
            ttl_seconds: Some(600),
            goal_subject: None,
            expected_subscription_id: None,
            reason: None,
            interval_ms: Some(60_000),
            repeat_count: Some(3),
            content: Some(GoalScheduleContent::BoardTask {
                task_id: "renew-me".into(),
            }),
            operation: GoalScheduleOperation::Explicit,
        },
        now,
    )
    .unwrap();
    let renewals: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::NotificationSubscribed { subscription } => Some(subscription),
            _ => None,
        })
        .collect();
    assert_eq!(renewals.len(), 1, "{events:?}");
    let renewed = renewals[0];
    assert_ne!(renewed.id, terminal.id);
    assert_eq!(renewed.status, "armed");
    assert_eq!(renewed.fired_count, 0);
    assert_eq!(response.data["subscription"]["id"], renewed.id);

    for event in &events {
        state.apply_checked(event).unwrap();
    }
    assert_eq!(state.notification_subscriptions[&terminal.id], terminal);
    assert_eq!(state.notification_subscriptions.len(), 2);
}

#[test]
fn goal_rearm_commits_one_due_replacement_and_retry_is_read_only() {
    let now = now_ms();
    let mut state = State::default();
    edit_worker(&mut state, "edit-worker");
    let mut due = edit_subscription("sub-due", "edit-worker", "armed", now - 70_000);
    due.periodic_declared = false;
    due.interval_ms = None;
    due.repeat_count = 1;
    due.fired_count = 0;
    state
        .notification_subscriptions
        .insert(due.id.clone(), due.clone());
    let mut rearm = content_edit_change("rearm due goal");
    rearm.operation = GoalScheduleOperation::Rearm;
    rearm.trigger_ms = Some(now + 60_000);
    rearm.ttl_seconds = Some(600);
    let (events, result) = prepare_goal_schedule(&state, "edit-worker", &rearm, now).unwrap();
    let replacement = result.data["subscription"]["id"].as_str().unwrap();
    assert_ne!(replacement, due.id);
    assert_eq!(result.data["one_shot"], true);
    assert_eq!(
        result.data["retired_subscription_ids"],
        serde_json::json!([due.id])
    );
    for event in &events {
        state.apply_checked(event).unwrap();
    }
    let (retry_events, retry) = prepare_goal_schedule(&state, "edit-worker", &rearm, now).unwrap();
    assert!(retry_events.is_empty());
    assert_eq!(retry.data["subscription"]["id"], replacement);
}

#[test]
fn goal_legacy_one_shot_keeps_its_declared_shape_on_metadata_edit_and_future_rearm() {
    let now = now_ms();
    let mut state = State::default();
    edit_worker(&mut state, "edit-worker");
    let mut legacy = edit_subscription("sub-legacy", "edit-worker", "armed", now);
    legacy.periodic_declared = false;
    legacy.fired_count = 0;
    state
        .notification_subscriptions
        .insert(legacy.id.clone(), legacy.clone());
    let mut edit = content_edit_change("metadata-only edit");
    edit.expected_subscription_id = Some(legacy.id.clone());
    let (edited, receipt) = prepare_goal_schedule(&state, "edit-worker", &edit, now).unwrap();
    assert_eq!(receipt.data["one_shot"], true);
    assert_eq!(
        receipt.data["subscription"]["interval_ms"],
        legacy.interval_ms.unwrap()
    );
    for event in &edited {
        state.apply_checked(event).unwrap();
    }
    let mut rearm = content_edit_change("ignored on future rearm");
    rearm.operation = GoalScheduleOperation::Rearm;
    rearm.trigger_ms = Some(now + 120_000);
    rearm.ttl_seconds = Some(600);
    let (unchanged, receipt) = prepare_goal_schedule(&state, "edit-worker", &rearm, now).unwrap();
    assert!(unchanged.is_empty());
    assert_eq!(receipt.data["one_shot"], true);
    assert_eq!(receipt.data["subscription"]["id"], legacy.id);
}
