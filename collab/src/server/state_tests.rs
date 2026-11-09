use super::*;

#[test]
fn legacy_notification_pane_decodes_without_weakening_required_target() {
    let legacy = serde_json::json!({"ev":"NotificationSubscribed","subscription":{
        "id":"legacy-sub","worker_id":"legacy-peer","event":"direct-message",
        "subject":null,"pane":"%1","method":"tmux","trigger_ms":null,
        "expires_ms":1,"status":"armed","created_ms":0,"updated_ms":0
    }});
    let event: Event = serde_json::from_value(legacy.clone()).unwrap();
    let canonical = serde_json::to_value(event).unwrap();
    assert_eq!(canonical["subscription"]["target"], "%1");
    assert!(canonical["subscription"].get("pane").is_none());
    let _: Event = serde_json::from_value(canonical).unwrap();
    let mut missing = legacy;
    missing["subscription"]
        .as_object_mut()
        .unwrap()
        .remove("pane");
    assert!(serde_json::from_value::<Event>(missing).is_err());
}
use crate::identity::{AgentId, AppServerId, BindingId, NativeThreadId, RuntimeId, SessionId};
use crate::scope::ProjectScopeId;
use crate::server::global_state::{ProjectRegistration, RuntimeBinding};

#[test]
fn pending_merge_survives_current_state_and_resolution_removes_it() {
    let mut state = State::default();
    state.apply(&Event::MergeRequested {
        request: PendingMerge {
            task_id: "task-a".into(),
            owner: "owner".into(),
            requested_by: "owner".into(),
            requested_ms: 10,
            candidate_commit: None,
        },
    });
    state.apply(&Event::MergeRequested {
        request: PendingMerge {
            task_id: "task-b".into(),
            owner: "owner".into(),
            requested_by: "owner".into(),
            requested_ms: 20,
            candidate_commit: None,
        },
    });
    state.apply(&Event::MergeResolved {
        task_id: "task-b".into(),
        resolved_by: "master".into(),
        reason: Some("integrated into main".into()),
        at_ms: 30,
    });

    let replayed: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();

    assert_eq!(
        replayed.pending_merges.keys().collect::<Vec<_>>(),
        vec!["task-a"],
        "replay must preserve the unresolved obligation and drop the resolved one"
    );
    assert_eq!(replayed.pending_merges["task-a"].requested_ms, 10);
}

#[test]
fn dropping_a_message_retires_its_notification_delivery_evidence() {
    let mut state = State::default();
    let message_id = "message-pruned";
    state.notification_delivery_failures.insert(
        message_id.into(),
        NotificationDeliveryFailure {
            message_id: message_id.into(),
            operation: "notification.emitted".into(),
            error: "TMUX_ENTER_SUBMIT_FAILED".into(),
            failed_ms: 10,
            retryable: false,
        },
    );
    state
        .notification_delivery_accepted
        .insert(message_id.into(), 11);
    state
        .notification_delivery_evidence
        .insert(message_id.into(), serde_json::json!({"consumed": false}));

    state.drop_message(message_id);

    assert!(!state
        .notification_delivery_failures
        .contains_key(message_id));
    assert!(!state
        .notification_delivery_accepted
        .contains_key(message_id));
    assert!(!state
        .notification_delivery_evidence
        .contains_key(message_id));
}

#[test]
fn notification_delivery_failure_survives_current_state() {
    let mut state = State::default();
    let event = Event::NotificationDeliveryFailed {
        message_id: "msg-persisted-failure".into(),
        operation: "notification.emitted".into(),
        error: "ADAPTER_ROUTE_UNAVAILABLE: persisted failure".into(),
        failed_ms: 42,
        retryable: true,
    };
    state.apply(&event);
    assert_eq!(
        state.notification_delivery_failures["msg-persisted-failure"],
        NotificationDeliveryFailure {
            message_id: "msg-persisted-failure".into(),
            operation: "notification.emitted".into(),
            error: "ADAPTER_ROUTE_UNAVAILABLE: persisted failure".into(),
            failed_ms: 42,
            retryable: true,
        }
    );

    let replayed: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(
        replayed.notification_delivery_failures,
        state.notification_delivery_failures
    );
}

#[test]
fn skipped_busy_master_wake_current_state_keeps_zero_generation() {
    let mut state = State::default();
    state.master_worker_id = Some("master".into());
    state.apply(&Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-master-deadline".into(),
            worker_id: "master".into(),
            event: "deadline".into(),
            subject: Some("periodic:master".into()),
            target: "thread-master".into(),
            method: "appserver".into(),
            trigger_ms: Some(10),
            trigger_times_ms: Vec::new(),
            interval_ms: Some(60_000),
            repeat_count: 3,
            fired_count: 0,
            expires_ms: 60_000,
            status: "armed".into(),
            created_ms: 1,
            updated_ms: 1,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    });
    state.apply(&Event::NotificationSkipped {
        subscription_id: "sub-master-deadline".into(),
        reason: "deadline-master-busy-skipped".into(),
        due_ms: 10,
        skipped_ms: 12,
    });
    assert_eq!(state.master_wake.generation, 0);
    assert_eq!(state.master_wake.delivery_state, "skipped-busy");

    let replayed: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(replayed.master_wake.generation, 0);
    assert_eq!(
        replayed.master_wake.delivery_state,
        state.master_wake.delivery_state
    );
}

#[test]
fn failed_master_wake_current_state_keeps_zero_generation() {
    let mut state = State::default();
    state.master_worker_id = Some("master".into());
    state.apply(&Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-master-deadline".into(),
            worker_id: "master".into(),
            event: "deadline".into(),
            subject: Some("periodic:master".into()),
            target: "thread-master".into(),
            method: "appserver".into(),
            trigger_ms: Some(10),
            trigger_times_ms: Vec::new(),
            interval_ms: Some(60_000),
            repeat_count: 3,
            fired_count: 0,
            expires_ms: 60_000,
            status: "armed".into(),
            created_ms: 1,
            updated_ms: 1,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    });
    state.apply(&Event::Sent {
        msg: Message {
            id: "master-deadline-message".into(),
            from: "collab-server".into(),
            to: "master".into(),
            mtype: "notification".into(),
            subject: Some("deadline:periodic:master".into()),
            body: "DEADLINE_REACHED subject=periodic:master".into(),
            in_reply_to: None,
            created_ms: 11,
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    });
    state.apply(&Event::WakeBound {
        message_id: "master-deadline-message".into(),
        subscription_id: "sub-master-deadline".into(),
    });
    state.apply(&Event::NotificationDeliveryFailed {
        message_id: "master-deadline-message".into(),
        operation: "notification.emitted".into(),
        error: "ADAPTER_ROUTE_UNAVAILABLE: failed".into(),
        failed_ms: 12,
        retryable: true,
    });
    assert_eq!(state.master_wake.generation, 0);
    assert_eq!(state.master_wake.delivery_state, "delivery_failed");

    let replayed: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(replayed.master_wake.generation, 0);
    assert_eq!(
        replayed.master_wake.delivery_state,
        state.master_wake.delivery_state
    );
}

#[test]
fn master_wake_hold_survives_current_state() {
    use crate::server::notification_state::{MasterWakeAccumulator, WakeHold};

    let mut state = State::default();
    state.apply(&Event::MasterWakeUpdated {
        accumulator: MasterWakeAccumulator {
            wake_hold: Some(WakeHold {
                reason: "external gate".into(),
                held_by: "master".into(),
                held_ms: 1_000,
                until_ms: 2_000,
            }),
            ..Default::default()
        },
    });
    assert_eq!(
        state.master_wake.wake_hold.as_ref().unwrap().reason,
        "external gate"
    );

    let replayed: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(replayed.master_wake.wake_hold, state.master_wake.wake_hold);
}

#[test]
fn resource_release_failure_does_not_poison_master_wake_delivery_state() {
    let mut state = State::default();
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::GoalDue { revision: 7 },
        at_ms: 10,
    });
    assert_eq!(state.master_wake.delivery_state, "pending");
    state.apply(&Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-release".into(),
            worker_id: "worker".into(),
            event: "resource-released".into(),
            subject: Some("task-1".into()),
            target: "thread-worker".into(),
            method: "appserver".into(),
            trigger_ms: None,
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: 60_000,
            status: "armed".into(),
            created_ms: 1,
            updated_ms: 1,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    });
    state.apply(&Event::Sent {
        msg: Message {
            id: "release-message".into(),
            from: "collab-server".into(),
            to: "worker".into(),
            mtype: "notification".into(),
            subject: Some("released:task-1".into()),
            body: "RESOURCE_RELEASED subject=task-1".into(),
            in_reply_to: None,
            created_ms: 11,
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    });
    state.apply(&Event::WakeBound {
        message_id: "release-message".into(),
        subscription_id: "sub-release".into(),
    });
    state.apply(&Event::NotificationDeliveryFailed {
        message_id: "release-message".into(),
        operation: "notification.emitted".into(),
        error: "ADAPTER_UNKNOWN: native frame exceeds maximum size".into(),
        failed_ms: 12,
        retryable: false,
    });

    assert_eq!(state.master_wake.delivery_state, "pending");
    assert!(state
        .notification_delivery_failures
        .contains_key("release-message"));
}

#[test]
fn worker_deadline_failure_does_not_poison_master_wake_delivery_state() {
    let mut state = State::default();
    let initial_delivery_state = state.master_wake.delivery_state.clone();
    state.apply(&Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-worker-deadline".into(),
            worker_id: "worker".into(),
            event: "deadline".into(),
            subject: Some("worker-periodic".into()),
            target: "thread-worker".into(),
            method: "appserver".into(),
            trigger_ms: Some(10),
            trigger_times_ms: Vec::new(),
            interval_ms: Some(60_000),
            repeat_count: 3,
            fired_count: 0,
            expires_ms: 60_000,
            status: "armed".into(),
            created_ms: 1,
            updated_ms: 1,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    });
    state.apply(&Event::Sent {
        msg: Message {
            id: "worker-deadline-message".into(),
            from: "collab-server".into(),
            to: "worker".into(),
            mtype: "notification".into(),
            subject: Some("deadline:worker-periodic".into()),
            body: "DEADLINE_REACHED subject=worker-periodic".into(),
            in_reply_to: None,
            created_ms: 11,
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    });
    state.apply(&Event::WakeBound {
        message_id: "worker-deadline-message".into(),
        subscription_id: "sub-worker-deadline".into(),
    });
    state.apply(&Event::NotificationDeliveryFailed {
        message_id: "worker-deadline-message".into(),
        operation: "notification.emitted".into(),
        error: "ADAPTER_UNKNOWN: native frame exceeds maximum size".into(),
        failed_ms: 12,
        retryable: false,
    });

    assert_eq!(state.master_wake.delivery_state, initial_delivery_state);
    assert_eq!(state.master_wake.generation, 0);
}

#[test]
fn worker_deadline_skip_does_not_poison_master_wake_delivery_state() {
    let mut state = State::default();
    let initial_delivery_state = state.master_wake.delivery_state.clone();
    state.apply(&Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-worker-deadline".into(),
            worker_id: "worker".into(),
            event: "deadline".into(),
            subject: Some("worker-periodic".into()),
            target: "thread-worker".into(),
            method: "appserver".into(),
            trigger_ms: Some(10),
            trigger_times_ms: Vec::new(),
            interval_ms: Some(60_000),
            repeat_count: 3,
            fired_count: 0,
            expires_ms: 60_000,
            status: "armed".into(),
            created_ms: 1,
            updated_ms: 1,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    });
    state.apply(&Event::NotificationSkipped {
        subscription_id: "sub-worker-deadline".into(),
        reason: "deadline-worker-busy-skipped".into(),
        due_ms: 10,
        skipped_ms: 12,
    });

    assert_eq!(state.master_wake.delivery_state, initial_delivery_state);
    assert_eq!(state.master_wake.generation, 0);
    let subscription = &state.notification_subscriptions["sub-worker-deadline"];
    assert_eq!(subscription.status, "armed");
    assert_eq!(subscription.fired_count, 1);
}

#[test]
fn master_wake_accumulator_coalesces_generated_signals_until_decision() {
    let mut state = State::default();
    let project_scope = ProjectScopeId::new("/master-wake-project").unwrap();
    let app_scope = AppServerId::new("app-one").unwrap();
    let agent = AgentId::new("master").unwrap();
    let binding = RuntimeBinding::new_with_session(
        project_scope.clone(),
        app_scope.clone(),
        agent.clone(),
        RuntimeId::new("runtime-master-wake").unwrap(),
        BindingId::new("binding-master-wake").unwrap(),
        Some(SessionId::new("session-thread-master").unwrap()),
        Some(NativeThreadId::new("thread-master").unwrap()),
    )
    .unwrap();
    let grant = crate::server::global_state::MasterGrant::new(
        project_scope.clone(),
        app_scope,
        agent,
        "project",
        "operator",
        "approved",
        binding.binding_id.clone(),
        1,
    )
    .unwrap();
    state
        .global
        .register_project(
            ProjectRegistration::new(project_scope, binding.app_scope_id.clone()).unwrap(),
        )
        .unwrap();
    state.global.bind_runtime(binding).unwrap();
    state.apply(&Event::GlobalMasterGranted { grant });
    let generated = |id: &str, subject: &str, created_ms: i64| Message {
        id: id.into(),
        from: "collab-server".into(),
        to: "master".into(),
        mtype: "notify".into(),
        subject: Some(subject.into()),
        body: "durable detail".into(),
        in_reply_to: None,
        created_ms,
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::WorkerIdle {
            worker_id: "worker".into(),
        },
        at_ms: 10,
    });
    state.apply(&Event::Sent {
        msg: generated("idle-1", "worker-idle: worker", 10),
    });
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::WorkerIdle {
            worker_id: "worker".into(),
        },
        at_ms: 20,
    });
    state.apply(&Event::Sent {
        msg: generated("idle-duplicate", "worker-idle: worker", 20),
    });
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::WorkerUnresponsive {
            worker_id: "offline".into(),
        },
        at_ms: 30,
    });
    state.apply(&Event::Sent {
        msg: generated("unresponsive", "worker-unresponsive: offline", 30),
    });
    assert_eq!(state.master_wake.generation, 1);
    assert_eq!(state.master_wake.first_pending_ms, 10);
    assert_eq!(state.master_wake.last_updated_ms, 30);
    assert_eq!(state.master_wake.idle_workers, vec!["worker"]);
    assert_eq!(state.master_wake.unresponsive_workers, vec!["offline"]);
    assert_eq!(state.master_wake.delivery_state, "pending");

    let explicit = Message {
        from: "peer".into(),
        ..generated("explicit", "worker-idle: ignored", 40)
    };
    state.apply(&Event::Sent { msg: explicit });
    assert_eq!(state.master_wake.idle_workers, vec!["worker"]);
    state.apply(&Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-master".into(),
            worker_id: "master".into(),
            event: "direct-message".into(),
            subject: None,
            target: "thread-master".into(),
            method: "appserver".into(),
            trigger_ms: None,
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: 100,
            status: "armed".into(),
            created_ms: 1,
            updated_ms: 1,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    });
    state.apply(&Event::WakeBound {
        message_id: "idle-1".into(),
        subscription_id: "sub-master".into(),
    });
    state.notification_subscriptions.insert(
        "sub-master".into(),
        NotificationSubscription {
            id: "sub-master".into(),
            worker_id: "master".into(),
            event: "worker-idle".into(),
            subject: None,
            target: "thread-master".into(),
            method: "appserver".into(),
            trigger_ms: None,
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: 60_000,
            status: "armed".into(),
            created_ms: 1,
            updated_ms: 1,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    );
    state.apply(&Event::Delivered {
        ids: vec!["idle-1".into()],
    });
    assert_eq!(state.master_wake.delivery_state, "notified_unconsumed");
    state.apply(&Event::Acked {
        ids: vec!["idle-1".into()],
    });
    assert_eq!(state.master_wake.generation, 1);
    assert_eq!(state.master_wake.delivery_state, "notified_unconsumed");
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::WorkerWorking {
            worker_id: "worker".into(),
        },
        at_ms: 50,
    });
    assert!(state.master_wake.idle_workers.is_empty());
    assert_eq!(state.master_wake.delivery_state, "pending");
}

#[test]
fn delivered_master_wake_uses_current_typed_grant() {
    let project_scope = ProjectScopeId::new("/project").unwrap();
    let app_scope = AppServerId::new("app-one").unwrap();
    let agent = AgentId::new("master").unwrap();
    let binding = RuntimeBinding::new_with_session(
        project_scope.clone(),
        app_scope.clone(),
        agent.clone(),
        RuntimeId::new("runtime-one").unwrap(),
        BindingId::new("binding-master").unwrap(),
        Some(SessionId::new("session-thread-master").unwrap()),
        Some(NativeThreadId::new("thread-master").unwrap()),
    )
    .unwrap();
    let grant = crate::server::global_state::MasterGrant::new(
        project_scope.clone(),
        app_scope,
        agent,
        "project",
        "master",
        "approved",
        binding.binding_id.clone(),
        1,
    )
    .unwrap();

    let mut state = State::default();
    state
        .global
        .register_project(
            ProjectRegistration::new(project_scope, binding.app_scope_id.clone()).unwrap(),
        )
        .unwrap();
    state.global.bind_runtime(binding).unwrap();
    state.apply(&Event::GlobalMasterGranted { grant });
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::WorkerIdle {
            worker_id: "worker".into(),
        },
        at_ms: 10,
    });
    state.apply(&Event::Sent {
        msg: Message {
            from: "collab-server".into(),
            to: "master".into(),
            ..msg("master-wake", "master", "notify")
        },
    });
    state.apply(&Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-master".into(),
            worker_id: "master".into(),
            event: "direct-message".into(),
            subject: None,
            target: "thread-master".into(),
            method: "appserver".into(),
            trigger_ms: None,
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: 100,
            status: "armed".into(),
            created_ms: 1,
            updated_ms: 1,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    });
    state.apply(&Event::WakeBound {
        message_id: "master-wake".into(),
        subscription_id: "sub-master".into(),
    });

    let replayed: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(replayed.master_wake.delivery_state, "pending");

    state.apply(&Event::Delivered {
        ids: vec!["master-wake".into()],
    });
    let replayed: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(state.master_wake.delivery_state, "notified_unconsumed");
    assert_eq!(
        replayed.master_wake.delivery_state,
        state.master_wake.delivery_state
    );
}

#[test]
fn delivered_master_wake_is_scoped_to_subscription_route() {
    let first_scope = ProjectScopeId::new("/project-one").unwrap();
    let second_scope = ProjectScopeId::new("/project-two").unwrap();
    let app_scope = AppServerId::new("app-one").unwrap();
    let first_binding = RuntimeBinding::new_with_session(
        first_scope.clone(),
        app_scope.clone(),
        AgentId::new("first-master").unwrap(),
        RuntimeId::new("runtime-first").unwrap(),
        BindingId::new("binding-first").unwrap(),
        Some(SessionId::new("session-thread-first").unwrap()),
        Some(NativeThreadId::new("thread-first").unwrap()),
    )
    .unwrap();
    let second_binding = RuntimeBinding::new_with_session(
        second_scope.clone(),
        app_scope.clone(),
        AgentId::new("second-master").unwrap(),
        RuntimeId::new("runtime-second").unwrap(),
        BindingId::new("binding-second").unwrap(),
        Some(SessionId::new("session-thread-second").unwrap()),
        Some(NativeThreadId::new("thread-second").unwrap()),
    )
    .unwrap();
    let first_grant = crate::server::global_state::MasterGrant::new(
        first_scope.clone(),
        app_scope.clone(),
        first_binding.agent_id.clone(),
        "project",
        "operator",
        "approved",
        first_binding.binding_id.clone(),
        1,
    )
    .unwrap();
    let second_grant = crate::server::global_state::MasterGrant::new(
        second_scope.clone(),
        app_scope.clone(),
        second_binding.agent_id.clone(),
        "project",
        "operator",
        "approved",
        second_binding.binding_id.clone(),
        1,
    )
    .unwrap();

    let mut state = State::default();
    for (scope, binding, grant) in [
        (first_scope, first_binding, first_grant),
        (second_scope, second_binding, second_grant),
    ] {
        state
            .global
            .register_project(
                ProjectRegistration::new(scope, binding.app_scope_id.clone()).unwrap(),
            )
            .unwrap();
        state.global.bind_runtime(binding).unwrap();
        state.apply(&Event::GlobalMasterGranted { grant });
    }
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::WorkerIdle {
            worker_id: "worker".into(),
        },
        at_ms: 10,
    });
    state.apply(&Event::Sent {
        msg: Message {
            from: "collab-server".into(),
            to: "first-master".into(),
            ..msg("master-wake", "first-master", "notify")
        },
    });
    state.apply(&Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-first-master".into(),
            worker_id: "first-master".into(),
            event: "direct-message".into(),
            subject: None,
            target: "thread-first".into(),
            method: "appserver".into(),
            trigger_ms: None,
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: 100,
            status: "armed".into(),
            created_ms: 1,
            updated_ms: 1,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    });
    state.apply(&Event::WakeBound {
        message_id: "master-wake".into(),
        subscription_id: "sub-first-master".into(),
    });

    state.apply(&Event::Delivered {
        ids: vec!["master-wake".into()],
    });
    assert_eq!(
        state.master_wake.delivery_state, "notified_unconsumed",
        "two project-scoped masters must not make delivery ambiguous"
    );
}

#[test]
fn goal_due_is_idempotent_per_revision_and_advances_for_new_revision() {
    let mut state = State::default();
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::GoalDue { revision: 7 },
        at_ms: 10,
    });
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::GoalDue { revision: 7 },
        at_ms: 20,
    });
    assert_eq!(state.master_wake.active_goal_revision, Some(7));
    assert_eq!(state.master_wake.last_updated_ms, 10);
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::GoalDue { revision: 8 },
        at_ms: 30,
    });
    assert_eq!(state.master_wake.active_goal_revision, Some(8));
    assert_eq!(state.master_wake.last_updated_ms, 30);
}

#[test]
fn consuming_a_master_goal_deadline_clears_goal_due() {
    let mut state = State::default();
    state.master_worker_id = Some("master".into());
    state.apply(&Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-goal-master".into(),
            worker_id: "master".into(),
            event: "deadline".into(),
            subject: Some("goal:sha256:test".into()),
            target: "thread-master".into(),
            method: "appserver".into(),
            trigger_ms: None,
            trigger_times_ms: vec![9],
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: 100,
            status: "armed".into(),
            created_ms: 1,
            updated_ms: 1,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    });
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::GoalDue { revision: 9 },
        at_ms: 10,
    });
    assert!(state.master_wake.goal_due);

    state.apply(&Event::NotificationConsumed {
        subscription_id: "sub-goal-master".into(),
        message_id: "goal-message".into(),
        consumed_ms: 11,
    });

    assert!(
        !state.master_wake.goal_due,
        "consuming a master goal deadline must clear the master wake goal-due flag"
    );
    assert_eq!(state.master_wake.active_goal_revision, None);
    assert_eq!(
        state.notification_subscriptions["sub-goal-master"].status,
        "consumed"
    );
}

#[test]
fn recv_style_ack_clears_master_goal_due() {
    let mut state = State::default();
    state.master_worker_id = Some("master".into());
    state.apply(&Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-goal-master".into(),
            worker_id: "master".into(),
            event: "deadline".into(),
            subject: Some("goal:sha256:test".into()),
            target: "thread-master".into(),
            method: "appserver".into(),
            trigger_ms: None,
            trigger_times_ms: vec![9],
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: 100,
            status: "armed".into(),
            created_ms: 1,
            updated_ms: 1,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    });
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::GoalDue { revision: 9 },
        at_ms: 10,
    });
    let mut notification = msg("goal-message", "master", "notification");
    notification.from = "collab-server".into();
    notification.subject = Some("deadline:goal:sha256:test".into());
    state.apply(&Event::Sent { msg: notification });
    state.apply(&Event::WakeBound {
        message_id: "goal-message".into(),
        subscription_id: "sub-goal-master".into(),
    });
    assert!(state.master_wake.goal_due);

    // `collab recv` records Delivered + Acked, not NotificationConsumed.
    state.apply(&Event::Delivered {
        ids: vec!["goal-message".into()],
    });
    state.apply(&Event::Acked {
        ids: vec!["goal-message".into()],
    });

    assert!(
        !state.master_wake.goal_due,
        "recv consumption of a master goal deadline must clear the master wake goal-due flag"
    );
    assert_eq!(state.master_wake.active_goal_revision, None);
    assert_eq!(
        state.notification_subscriptions["sub-goal-master"].status,
        "consumed"
    );
}

#[test]
fn delivering_a_master_goal_deadline_clears_goal_due_latch() {
    let mut state = State::default();
    state.master_worker_id = Some("master".into());
    state.apply(&Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-goal-master".into(),
            worker_id: "master".into(),
            event: "deadline".into(),
            subject: Some("goal:sha256:test".into()),
            target: "thread-master".into(),
            method: "appserver".into(),
            trigger_ms: None,
            trigger_times_ms: vec![9],
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: 100,
            status: "armed".into(),
            created_ms: 1,
            updated_ms: 1,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    });
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::GoalDue { revision: 9 },
        at_ms: 10,
    });
    let mut notification = msg("goal-message", "master", "notification");
    notification.from = "collab-server".into();
    notification.subject = Some("deadline:goal:sha256:test".into());
    state.apply(&Event::Sent { msg: notification });
    state.apply(&Event::WakeBound {
        message_id: "goal-message".into(),
        subscription_id: "sub-goal-master".into(),
    });
    assert!(state.master_wake.goal_due);

    state.apply(&Event::NotificationDeliveryAccepted {
        message_id: "goal-message".into(),
        accepted_ms: 11,
        evidence: Some(serde_json::json!({"accepted": true})),
    });

    assert!(
        !state.master_wake.goal_due,
        "adaptor acceptance of a master goal deadline must clear the master wake goal-due latch"
    );
    assert_eq!(
        state.master_wake.active_goal_revision,
        Some(9),
        "acceptance must keep the accepted revision for same-revision dedup"
    );

    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::GoalDue { revision: 9 },
        at_ms: 20,
    });
    assert!(
        !state.master_wake.goal_due,
        "same-revision goal-due replay must not re-raise the latch"
    );

    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::GoalDue { revision: 10 },
        at_ms: 30,
    });
    assert!(
        state.master_wake.goal_due,
        "a higher-revision goal-due signal must re-raise the latch"
    );
    assert_eq!(state.master_wake.active_goal_revision, Some(10));
}

#[test]
fn older_goal_deadline_acceptance_does_not_clear_a_newer_master_goal_due_latch() {
    let mut state = State::default();
    state.master_worker_id = Some("master".into());
    let subscribe = |id: &str, revision: i64| Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: id.into(),
            worker_id: "master".into(),
            event: "deadline".into(),
            subject: Some("goal:sha256:test".into()),
            target: "thread-master".into(),
            method: "appserver".into(),
            trigger_ms: None,
            trigger_times_ms: vec![revision],
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: 200,
            status: "armed".into(),
            created_ms: 1,
            updated_ms: 1,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    };
    let send = |id: &str, subscription_id: &str| {
        let mut notification = msg(id, "master", "notification");
        notification.from = "collab-server".into();
        notification.subject = Some("deadline:goal:sha256:test".into());
        (
            Event::Sent { msg: notification },
            Event::WakeBound {
                message_id: id.into(),
                subscription_id: subscription_id.into(),
            },
        )
    };

    state.apply(&subscribe("sub-goal-old", 9));
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::GoalDue { revision: 9 },
        at_ms: 10,
    });
    let old_notification = send("goal-old", "sub-goal-old");
    state.apply(&old_notification.0);
    state.apply(&old_notification.1);

    state.apply(&subscribe("sub-goal-new", 10));
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::GoalDue { revision: 10 },
        at_ms: 20,
    });
    assert_eq!(state.master_wake.active_goal_revision, Some(10));
    assert!(state.master_wake.goal_due);

    state.apply(&Event::NotificationDeliveryAccepted {
        message_id: "goal-old".into(),
        accepted_ms: 30,
        evidence: Some(serde_json::json!({"accepted": true})),
    });
    assert!(
        state.master_wake.goal_due,
        "an older accepted deadline must not clear the newer master goal-due latch"
    );

    let new_notification = send("goal-new", "sub-goal-new");
    state.apply(&new_notification.0);
    state.apply(&new_notification.1);
    state.apply(&Event::NotificationDeliveryAccepted {
        message_id: "goal-new".into(),
        accepted_ms: 40,
        evidence: Some(serde_json::json!({"accepted": true})),
    });
    assert!(
        !state.master_wake.goal_due,
        "the matching newer deadline acceptance must clear the latch"
    );
    assert_eq!(state.master_wake.active_goal_revision, Some(10));
}

#[test]
fn stale_master_goal_due_is_reconciled_when_no_armed_goal_deadline_remains() {
    let mut state = State::default();
    state.master_worker_id = Some("master".into());
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::GoalDue { revision: 9 },
        at_ms: 10,
    });
    assert!(state.master_wake.goal_due);

    assert!(state.reconcile_stale_master_goal_due(100));
    assert!(!state.master_wake.goal_due);
    assert_eq!(state.master_wake.active_goal_revision, None);
}

#[test]
fn master_goal_due_is_kept_while_an_armed_goal_deadline_exists() {
    let mut state = State::default();
    state.master_worker_id = Some("master".into());
    state.apply(&Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-goal-master".into(),
            worker_id: "master".into(),
            event: "deadline".into(),
            subject: Some("goal:sha256:test".into()),
            target: "thread-master".into(),
            method: "appserver".into(),
            trigger_ms: None,
            trigger_times_ms: vec![9],
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: 200,
            status: "armed".into(),
            created_ms: 1,
            updated_ms: 1,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    });
    state.apply(&Event::MasterWakeSignal {
        signal: MasterWakeSignal::GoalDue { revision: 9 },
        at_ms: 10,
    });
    assert!(state.master_wake.goal_due);

    assert!(!state.reconcile_stale_master_goal_due(100));
    assert!(state.master_wake.goal_due);
}

fn msg(id: &str, to: &str, mtype: &str) -> Message {
    Message {
        id: id.into(),
        from: "a".into(),
        to: to.into(),
        mtype: mtype.into(),
        subject: Some("test".into()),
        body: "b".into(),
        in_reply_to: None,
        created_ms: 1,
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    }
}

#[test]
fn message_lifecycle() {
    let mut st = State::default();
    st.apply(&Event::Sent {
        msg: msg("m1", "w2", "request"),
    });
    assert_eq!(st.inbox_of("w2").len(), 1);
    assert!(st.inbox_of("w1").is_empty());

    st.apply(&Event::Delivered {
        ids: vec!["m1".into()],
    });
    assert_eq!(st.msgs["m1"].state, "delivered");
    assert_eq!(st.inbox_of("w2").len(), 1);

    st.apply(&Event::Acked {
        ids: vec!["m1".into()],
    });
    assert_eq!(st.msgs["m1"].state, "read");
    assert!(st.inbox_of("w2").is_empty());
}

#[test]
fn legacy_wake_attempt_replay_is_clock_independent() {
    let event: Event = serde_json::from_str(r#"{"ev":"WakeAttempted","ids":["m1"]}"#).unwrap();
    let mut first = State::default();
    let mut second = State::default();
    for state in [&mut first, &mut second] {
        state.apply(&Event::Sent {
            msg: msg("m1", "worker", "system"),
        });
        state.apply(&event);
    }
    assert_eq!(first.msgs["m1"].wake_attempt_count, 1);
    assert_eq!(first.msgs["m1"].last_wake_attempt_ms, 0);
    assert_eq!(
        serde_json::to_value(&first.msgs["m1"]).unwrap(),
        serde_json::to_value(&second.msgs["m1"]).unwrap()
    );
}

#[test]
fn current_state_preserves_worktree_binding() {
    let mut state = State::default();
    let binding = WorktreeBinding {
        worktree_root: "/project/playground/task".into(),
        owning_project_scope: "/project".into(),
        task_id: "task-1".into(),
        owner_agent_id: "worker-1".into(),
        binding_id: "binding-task-1".into(),
        base_commit: "abc123".into(),
        isolation_mode: None,
    };
    state.apply(&Event::WorktreeBound {
            binding: binding.clone(),
    });
    let replayed: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(replayed.worktree_bindings["binding-task-1"], binding);
    assert_eq!(state.worktree_bindings, replayed.worktree_bindings);
}

include!("state_tests_part2.rs");
