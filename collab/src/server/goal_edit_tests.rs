use super::*;

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
