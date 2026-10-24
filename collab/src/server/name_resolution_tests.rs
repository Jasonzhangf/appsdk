use super::name_resolution::*;
use super::name_state::*;
use super::*;

/// One agent session has one current name carrier. A later commit releases any
/// other worker's name in that session, while task assignments and closed work
/// history stay addressable. Journal replay applies the same rule in event
/// order, so the last committer is the one replayed.
#[test]
fn same_session_name_commits_keep_one_carrier_and_replay_in_commit_order() {
    let alice = NameOccupancy {
        name: "Alice".into(),
        worker_id: "old-a".into(),
        session_id: "s1".into(),
        occupancy_revision: 1,
        updated_ms: 1,
    };
    let assignment = TaskAssignment {
        task_id: "task-a".into(),
        name: "Alice".into(),
        session_id: "s1".into(),
        assignment_revision: 1,
        reflow: None,
        updated_ms: 1,
    };
    let bob = NameOccupancy {
        name: "Bob".into(),
        worker_id: "old-b".into(),
        session_id: "s1".into(),
        occupancy_revision: 1,
        updated_ms: 2,
    };
    let events = vec![
        Event::NameResolutionCommitted {
            project_scope: "/project".into(),
            occupancy: alice,
            assignments: vec![assignment.clone()],
        },
        Event::NameResolutionCommitted {
            project_scope: "/project".into(),
            occupancy: bob,
            assignments: vec![],
        },
    ];
    let mut state = State::default();
    for event in &events {
        state.apply_checked(event).unwrap();
    }
    assert!(state.name_occupancy("/project", "Alice").is_none());
    assert_eq!(
        state.name_occupancy("/project", "Bob").unwrap().worker_id,
        "old-b"
    );
    assert_eq!(
        state.task_assignment("task-a"),
        Some(&assignment),
        "removing an old session carrier must not delete its task history"
    );

    let mut replayed = State::default();
    for event in &events {
        replayed.apply_checked(event).unwrap();
    }
    assert!(replayed.name_occupancy("/project", "Alice").is_none());
    assert_eq!(
        replayed.name_occupancy("/project", "Bob").unwrap().worker_id,
        "old-b"
    );
    assert_eq!(replayed.task_assignment("task-a"), Some(&assignment));

    // Another session keeps its own name and does not become a replay
    // ambiguity for the first session.
    let other_session = NameOccupancy {
        name: "Carol".into(),
        worker_id: "other-session-worker".into(),
        session_id: "s2".into(),
        occupancy_revision: 1,
        updated_ms: 3,
    };
    state
        .apply_checked(&Event::NameResolutionCommitted {
            project_scope: "/project".into(),
            occupancy: other_session,
            assignments: vec![],
        })
        .unwrap();
    assert_eq!(
        state.name_occupancy("/project", "Bob").unwrap().session_id,
        "s1"
    );
    assert_eq!(
        state.name_occupancy("/project", "Carol").unwrap().session_id,
        "s2"
    );
}

#[test]
fn name_handoff_preserves_goal_schedule_and_terminal_history() {
    let (server, root) = peer_tests::test_server();
    for id in ["old", "new"] {
        assert!(peer_tests::register_synthetic_peer(&server, id, id).ok);
    }
    let mut state = server.state.lock().unwrap();
    let scope = server_route_scope(&server, &state)
        .unwrap()
        .unwrap()
        .project_scope_id
        .as_str()
        .to_owned();
    let old = NameOccupancy {
        name: "Alice".into(),
        worker_id: "old".into(),
        session_id: "s1".into(),
        occupancy_revision: 1,
        updated_ms: 1,
    };
    state
        .name_occupancies
        .entry(scope.clone())
        .or_default()
        .insert("Alice".into(), old.clone());
    let base = state.notification_subscriptions[&mailbox::default_direct_message_id("old")].clone();
    let mut originals = Vec::new();
    for (id, status, event, subject, expires) in [
        ("armed", "armed", "deadline", "goal:owned", i64::MAX),
        ("lost", "transport-lost", "deadline", "goal:owned", i64::MAX),
        ("consumed", "consumed", "deadline", "goal:owned", i64::MAX),
        ("cancelled", "cancelled", "deadline", "goal:owned", i64::MAX),
        ("expired", "armed", "deadline", "goal:owned", 1),
        ("patrol", "armed", "deadline", "patrol:owned", i64::MAX),
        ("master", "armed", "master-idle", "goal:owned", i64::MAX),
    ] {
        let mut sub = base.clone();
        sub.id = id.into();
        sub.status = status.into();
        sub.event = event.into();
        sub.subject = Some(subject.into());
        sub.expires_ms = expires;
        sub.trigger_times_ms = vec![100, 200];
        sub.interval_ms = Some(100);
        sub.repeat_count = 2;
        sub.fired_count = 1;
        sub.created_ms = 7;
        state
            .notification_subscriptions
            .insert(id.into(), sub.clone());
        originals.push(sub);
    }
    state
        .apply_checked(&Event::Sent {
            msg: serde_json::from_value(json!({
                "id":"pending-goal", "from":"collab-server", "to":"old", "type":"notification",
                "subject":"deadline:goal:owned", "body":"original", "in_reply_to":null,
                "created_ms":1, "state":"pending", "wake_attempt_count":0, "last_wake_attempt_ms":0
            }))
            .unwrap(),
        })
        .unwrap();
    state
        .apply_checked(&Event::WakeBound {
            message_id: "pending-goal".into(),
            subscription_id: "armed".into(),
        })
        .unwrap();
    let next = NameOccupancy {
        worker_id: "new".into(),
        session_id: "s2".into(),
        occupancy_revision: 2,
        updated_ms: 2,
        ..old
    };
    let events = build_name_resolution_events(&state, &scope, &next);
    for event in &events {
        state.apply_checked(event).unwrap();
    }
    assert_eq!(
        state.wake_bindings["pending-goal"], "armed",
        "pending goal consumption must close its transferred subscription"
    );
    for mut expected in originals {
        if matches!(expected.id.as_str(), "armed" | "lost" | "consumed") {
            expected.worker_id = "new".into();
            let transport = selected_transport_for_worker(&state.workers["new"]).unwrap();
            expected.method = transport.kind.as_str().into();
            expected.target = notification_target(&transport).unwrap().into();
            expected.updated_ms = 2;
            if expected.id == "lost" {
                expected.status = "armed".into();
                expected.status_reason = None;
            }
        }
        assert_eq!(
            serde_json::to_value(&state.notification_subscriptions[&expected.id]).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
    }
    assert!(!build_name_resolution_events(&state, &scope, &next)
        .iter()
        .any(|e| matches!(e, Event::NotificationSubscribed { .. })));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn same_session_intermediate_name_commit_carries_goal_schedule() {
    let (server, root) = peer_tests::test_server();
    for id in ["old", "new"] {
        assert!(peer_tests::register_synthetic_peer(&server, id, id).ok);
    }
    let mut state = server.state.lock().unwrap();
    let scope = server_route_scope(&server, &state)
        .unwrap()
        .unwrap()
        .project_scope_id
        .as_str()
        .to_owned();
    let previous = NameOccupancy {
        name: "Jenny".into(),
        worker_id: "old".into(),
        session_id: "shared-session".into(),
        occupancy_revision: 1,
        updated_ms: 1,
    };
    state
        .name_occupancies
        .entry(scope.clone())
        .or_default()
        .insert("Jenny".into(), previous);
    let mut goal = state.notification_subscriptions
        [&mailbox::default_direct_message_id("old")]
        .clone();
    goal.id = "goal-schedule".into();
    goal.event = "deadline".into();
    goal.subject = Some("goal:owned".into());
    goal.status = "transport-lost".into();
    goal.expires_ms = i64::MAX;
    state
        .notification_subscriptions
        .insert(goal.id.clone(), goal);
    state
        .apply_checked(&Event::Sent {
            msg: serde_json::from_value(json!({
                "id":"pending-goal-message", "from":"collab-server", "to":"old",
                "type":"notification", "subject":"deadline:goal:owned", "body":"original",
                "in_reply_to":null, "created_ms":1, "state":"pending",
                "wake_attempt_count":0, "last_wake_attempt_ms":0
            }))
            .unwrap(),
        })
        .unwrap();
    state
        .apply_checked(&Event::WakeBound {
            message_id: "pending-goal-message".into(),
            subscription_id: "goal-schedule".into(),
        })
        .unwrap();

    // Registration can first assign an ordinary name; the promotion to Jenny
    // happens after that same session has already released the old occupancy.
    let intermediate = NameOccupancy {
        name: "Bob".into(),
        worker_id: "new".into(),
        session_id: "shared-session".into(),
        occupancy_revision: 2,
        updated_ms: 2,
    };
    let events = build_name_resolution_events(&state, &scope, &intermediate);
    for event in &events {
        state.apply_checked(event).unwrap();
    }
    let moved = state.notification_subscriptions["goal-schedule"].clone();
    assert_eq!(moved.worker_id, "new");
    assert_eq!(moved.status, "armed");
    assert_eq!(state.msgs["pending-goal-message"].to, "new");
    assert_eq!(
        state.wake_bindings["pending-goal-message"],
        "goal-schedule",
        "the pending goal wake must stay bound to its transferred schedule"
    );

    let promoted = NameOccupancy {
        name: "Jenny".into(),
        worker_id: "new".into(),
        session_id: "shared-session".into(),
        occupancy_revision: 3,
        updated_ms: 3,
    };
    let events = build_name_resolution_events(&state, &scope, &promoted);
    for event in &events {
        state.apply_checked(event).unwrap();
    }
    assert_eq!(
        state.notification_subscriptions["goal-schedule"].worker_id,
        "new"
    );
    assert_eq!(state.msgs["pending-goal-message"].to, "new");
    assert_eq!(
        state.wake_bindings["pending-goal-message"],
        "goal-schedule"
    );
    assert!(!state.notification_subscriptions.values().any(|subscription| {
        subscription.worker_id == "old"
            && subscription.event == "deadline"
            && subscription
                .subject
                .as_deref()
                .is_some_and(|subject| subject.starts_with("goal:"))
    }));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn public_handlers_move_goal_schedule_and_pending_wake_across_intermediate_name() {
    let (server, root) = peer_tests::test_server();
    let server = std::sync::Arc::new(server);
    let register = |server: &Server, id: &str, pane: u32, session: &str| {
        peer_tests::register_tmux(
            server,
            id,
            crate::proto::TmuxEndpoint {
                socket_path: format!("/tmp/collab-shared-{id}-{}", std::process::id()),
                server_pid: 1,
                tmux_session_id: format!("${pane}"),
                pane_id: format!("%{pane}"),
                pane_pid: 2,
                codex_session_id: Some(session.into()),
                codex_thread_id: Some(format!("thread-{id}")),
            },
        )
    };
    assert!(register(&server, "old", 1, "shared-session").ok);

    let promoted =
        handle_master_promote(&server, "old".into(), "token-old".into(), "user approved old".into());
    assert!(promoted.ok, "{promoted:?}");

    let subscribed = handle_notification_subscribe(
        &server,
        "old".into(),
        "token-old".into(),
        "deadline".into(),
        Some("goal:shared-migration".into()),
        Some(now_ms() + 5_000),
        Vec::new(),
        None,
        1,
        3_600,
    );
    assert!(subscribed.ok, "{subscribed:?}");
    let goal_id = subscribed.data["subscription"]["id"]
        .as_str()
        .expect("goal subscription id")
        .to_owned();

    // The timer is the public scheduling path that arms one pending wake.
    timers::tick_with_idle_at(&server, now_ms() + 6_000, &|_| true);
    let pending_id = {
        let state = server.state.lock().unwrap();
        state
            .wake_bindings
            .iter()
            .find_map(|(message_id, bound)| {
                (*bound == goal_id && state.msgs[message_id].state == "pending")
                    .then(|| message_id.clone())
            })
            .expect("goal timer produces a pending wake")
    };

    // Registration is the migration entry. It first assigns an ordinary name
    // to the new same-session carrier; promotion to Jenny happens after that
    // intermediate commit has already released the old Jenny occupancy.
    assert!(register(&server, "new", 2, "shared-session").ok);

    let state = server.state.lock().unwrap();
    assert_eq!(state.notification_subscriptions[&goal_id].worker_id, "new");
    assert_eq!(state.msgs[&pending_id].to, "new");
    assert_eq!(state.wake_bindings[&pending_id], goal_id);
    drop(state);

    let promoted =
        handle_master_promote(&server, "new".into(), "token-new".into(), "user approved new".into());
    assert!(promoted.ok, "{promoted:?}");

    let old_denied = handle_goal_schedule_reconcile(
        &server,
        "old".into(),
        "token-old".into(),
        "shared-migration".into(),
        "Jenny".into(),
        "ended".into(),
        None,
        None,
        None,
        Some("fixture".into()),
    );
    assert!(!old_denied.ok, "{old_denied:?}");
    let cancelled = handle_goal_schedule_reconcile(
        &server,
        "new".into(),
        "token-new".into(),
        "shared-migration".into(),
        "Jenny".into(),
        "ended".into(),
        None,
        None,
        None,
        Some("fixture".into()),
    );
    assert!(cancelled.ok, "{cancelled:?}");

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn confirmed_user_jenny_takeover_overrides_unknown_without_ordinary_name_bypass() {
    let mut state = State::default();
    let mut old = input("old", "s1");
    old.is_current_master = true;
    assert_eq!(committed(&mut state, &old).name, "Jenny");
    let mut next = input("new", "s2");
    next.intent = Some(NameIntent {
        requested_name: "Jenny".into(),
        user_authorized: true,
        replace_confirmed: false,
    });
    assert!(matches!(
        plan_name_resolution(&state, &next).unwrap().decision,
        NameResolutionDecision::Pending { .. }
    ));
    next.intent.as_mut().unwrap().replace_confirmed = true;
    let plan = plan_name_resolution(&state, &next).unwrap();
    assert!(plan.requires_master_authorization);
    assert!(
        matches!(plan.decision, NameResolutionDecision::Committed { occupancy, .. }
        if occupancy.name == "Jenny" && occupancy.worker_id == "new" && occupancy.session_id == "s2")
    );
    committed(&mut state, &input("ordinary", "s3"));
    next.intent.as_mut().unwrap().requested_name = "Alice".into();
    assert!(matches!(
        plan_name_resolution(&state, &next).unwrap().decision,
        NameResolutionDecision::Pending { .. }
    ));
}

fn input(worker: &str, session: &str) -> NameResolutionInput {
    NameResolutionInput {
        project_scope: "/project".into(),
        operation_id: format!("context-{worker}-{session}"),
        request: NameRecoveryRequest {
            invocation: "context".into(),
            requested_name: None,
            caller_worker_id: worker.into(),
            caller_session_id: session.into(),
            caller_binding: NameBindingReference {
                app_scope_id: "app".into(),
                binding_id: "binding".into(),
                endpoint_generation: 1,
            },
            replace_confirmed: false,
            requested_ms: 20,
        },
        intent: None,
        is_current_master: false,
    }
}

fn committed(state: &mut State, input: &NameResolutionInput) -> NameOccupancy {
    let plan = plan_name_resolution(state, input).unwrap();
    let NameResolutionDecision::Committed { occupancy, .. } = &plan.decision else {
        panic!("{:?}", plan.decision)
    };
    let occupancy = occupancy.clone();
    for event in plan.events {
        state.apply_checked(&event).unwrap();
    }
    occupancy
}

#[test]
fn fixed_pool_project_isolation_full_and_session_idempotence() {
    let mut state = State::default();
    for (index, name) in ORDINARY_NAME_POOL.iter().enumerate() {
        let i = input(&format!("w{index}"), &format!("s{index}"));
        assert_eq!(&committed(&mut state, &i).name, name);
        let repeat = plan_name_resolution(&state, &i).unwrap();
        assert!(repeat.events.is_empty());
    }
    assert!(plan_name_resolution(&state, &input("extra", "extra"))
        .unwrap_err()
        .contains("NAME_POOL_FULL"));
    let mut other = input("other", "other");
    other.project_scope = "/other".into();
    assert_eq!(committed(&mut state, &other).name, "Alice");
    let mut same = input("recovered-id", "s0");
    same.request.requested_ms = 30;
    assert_eq!(committed(&mut state, &same).name, "Alice");
}

fn task(id: &str, status: &str) -> TaskRec {
    TaskRec {
        id: id.into(),
        owner: "old".into(),
        created_by: "master".into(),
        feature_id: None,
        worktree_path: Some("/resource".into()),
        branch: Some("branch".into()),
        base_commit: None,
        priority: "p2".into(),
        status: status.into(),
        next_step: Some("preserved".into()),
        wait: None,
        created_ms: 1,
        updated_ms: 1,
    }
}

#[test]
fn name_session_refresh_updates_all_open_work_and_preserves_closed_and_reassigned() {
    let mut state = State::default();
    committed(&mut state, &input("old", "s1"));
    for (id, status, name) in [
        ("a", "working", "Alice"),
        ("b", "accepted", "Alice"),
        ("c", "closed", "Alice"),
        ("d", "working", "Bob"),
    ] {
        state
            .apply_checked(&Event::TaskCreated {
                task: task(id, status),
            })
            .unwrap();
        state
            .apply_checked(&Event::TaskAssignmentUpdated {
                assignment: TaskAssignment {
                    task_id: id.into(),
                    name: name.into(),
                    session_id: "s1".into(),
                    assignment_revision: 1,
                    reflow: None,
                    updated_ms: 1,
                },
            })
            .unwrap();
        state
            .board_details
            .insert(id.into(), crate::board::BoardTaskDetails::default());
    }
    let before = serde_json::to_value(&state.tasks["c"]).unwrap();
    committed(&mut state, &input("old", "s2"));
    for id in ["a", "b"] {
        assert_eq!(state.task_assignments[id].session_id, "s2");
        assert_eq!(state.task_assignments[id].assignment_revision, 2);
        assert_eq!(state.tasks[id].worktree_path.as_deref(), Some("/resource"));
        assert_eq!(state.tasks[id].next_step.as_deref(), Some("preserved"));
        assert_eq!(state.board_details[id].revision, 1);
    }
    assert_eq!(state.tasks["b"].status, "accepted");
    assert_eq!(serde_json::to_value(&state.tasks["c"]).unwrap(), before);
    assert_eq!(state.task_assignments["c"].session_id, "s1");
    assert_eq!(state.task_assignments["d"].session_id, "s1");
}

#[test]
fn online_confirmation_unknown_pending_offline_inheritance_and_jenny_plan() {
    let mut state = State::default();
    committed(&mut state, &input("old", "s1"));
    let mut i = input("new", "s2");
    i.intent = Some(NameIntent {
        requested_name: "Alice".into(),
        user_authorized: true,
        replace_confirmed: false,
    });
    for presence in [
        AgentPresence::Unknown,
        AgentPresence::Online,
        AgentPresence::Offline,
    ] {
        state
            .apply_checked(&Event::AgentPresenceObserved {
                presence: AgentPresenceRecord {
                    project_scope: "/project".into(),
                    worker_id: "old".into(),
                    session_id: "s1".into(),
                    presence,
                    reason: "test".into(),
                    observed_ms: 20,
                },
            })
            .unwrap();
        let plan = plan_name_resolution(&state, &i).unwrap();
        match presence {
            AgentPresence::Unknown => {
                let NameResolutionDecision::Pending { recovery } = plan.decision else {
                    panic!()
                };
                assert_eq!(recovery.request.requested_name.as_deref(), Some("Alice"));
                assert_eq!(recovery.occupancy_ref.worker_id, "old");
                assert!(matches!(
                    plan.events.as_slice(),
                    [Event::NameRecoveryUpdated { .. }]
                ));
            }
            AgentPresence::Online => {
                assert!(matches!(
                    plan.decision,
                    NameResolutionDecision::NeedsConfirmation { .. }
                ));
                assert!(plan.events.is_empty());
                let mut confirmed = i.clone();
                confirmed.intent.as_mut().unwrap().replace_confirmed = true;
                assert!(matches!(
                    plan_name_resolution(&state, &confirmed).unwrap().decision,
                    NameResolutionDecision::Committed { .. }
                ));
            }
            AgentPresence::Offline => {
                assert!(matches!(
                    plan.decision,
                    NameResolutionDecision::Committed { .. }
                ));
            }
        }
    }
    i.intent = parse_name_intent(Some(
        &json!({"action":"restore_master","decision":"approved"}),
    ))
    .unwrap();
    let plan = plan_name_resolution(&state, &i).unwrap();
    assert!(plan.requires_master_authorization);
    assert!(
        matches!(plan.decision, NameResolutionDecision::Committed { ref occupancy, .. } if occupancy.name == "Jenny")
    );
    assert!(parse_name_intent(None).unwrap().is_none());
    assert!(
        parse_name_intent(Some(&json!({"action":"replace_master_grant"})))
            .unwrap()
            .is_none()
    );
}

#[test]
fn authorization_text_is_sufficient_but_empty_authorization_is_not() {
    let mut state = State::default();
    let mut i = input("worker", "session");
    i.intent = parse_name_intent(Some(&json!({"action":"restore_master",
        "authorization":"user asked to recover Jenny"})))
    .unwrap();
    assert!(
        plan_name_resolution(&state, &i)
            .unwrap()
            .requires_master_authorization
    );
    i.intent = parse_name_intent(Some(&json!({"action":"restore_master",
        "authorization":"  "})))
    .unwrap();
    assert!(plan_name_resolution(&state, &i)
        .unwrap_err()
        .contains("NAME_AUTHORIZATION_REQUIRED"));
    i.intent = parse_name_intent(Some(&json!({"action":"restore_master",
        "decision":"approved"})))
    .unwrap();
    assert_eq!(committed(&mut state, &i).name, "Jenny");
}

#[test]
fn registered_name_owner_checks_online_confirmation_and_master_clear() {
    let (server, root) = super::peer_tests::test_server();
    let project = std::fs::canonicalize(&root)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    for (worker, session) in [("first", "s1"), ("second", "s2")] {
        let transport = crate::proto::SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/names-fixture.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some(session.into()),
            thread_id: Some(session.into()),
            tmux_endpoint: None,
            capabilities: vec![],
            self_check: "isolated typed binding".into(),
        };
        let registered = super::peer_tests::seed_committed_owner(
            &server,
            worker,
            worker,
            &transport,
            root.to_str().unwrap(),
            Some(AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap()),
        );
        assert!(registered.ok, "{registered:?}");
    }
    let first = handle_master_promote(&server, "first".into(), "first".into(), "user asked".into());
    assert!(first.ok, "{first:?}");
    {
        let mut state = server.state.lock().unwrap();
        server
            .commit_locked_checked(
                &mut state,
                &[Event::AgentPresenceObserved {
                    presence: AgentPresenceRecord {
                        project_scope: project.clone(),
                        worker_id: "first".into(),
                        session_id: "s1".into(),
                        presence: AgentPresence::Online,
                        reason: "observed current agent".into(),
                        observed_ms: 1,
                    },
                }],
            )
            .unwrap();
    }
    let denied = handle_master_promote(
        &server,
        "second".into(),
        "second".into(),
        "user asked".into(),
    );
    assert!(!denied.ok);
    assert_eq!(denied.data["outcome"], "confirmation_required");
    assert!(denied.data["next_invocation"]
        .as_str()
        .unwrap()
        .contains("Jenny"));
    assert_eq!(
        current_master_holder(&server, &server.state.lock().unwrap())
            .unwrap()
            .as_deref(),
        Some("first")
    );
    let confirmed = resolve_registered_name(
        &server,
        "second",
        "confirmed",
        "approved_recovery",
        Some(NameIntent {
            requested_name: "Jenny".into(),
            user_authorized: true,
            replace_confirmed: true,
        }),
    )
    .unwrap();
    assert_eq!(confirmed["outcome"], "committed");
    assert_eq!(
        current_master_holder(&server, &server.state.lock().unwrap())
            .unwrap()
            .as_deref(),
        Some("second")
    );
    let cleared = handle_master_clear(
        &server,
        "second".into(),
        "second".into(),
        "user cleared".into(),
    );
    assert!(cleared.ok, "{cleared:?}");
    let state = server.state.lock().unwrap();
    assert!(current_master_holder(&server, &state).unwrap().is_none());
    assert!(state.name_occupancy(&project, "Jenny").is_none());
    assert!(state.workers.contains_key("first"));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn holder_change_updates_legacy_owner_without_overwriting_reassigned_work() {
    let mut state = State::default();
    committed(&mut state, &input("old", "s1"));
    for id in ["legacy", "assigned", "reassigned", "cancelled"] {
        state
            .apply_checked(&Event::TaskCreated {
                task: task(
                    id,
                    if id == "cancelled" {
                        "cancelled"
                    } else {
                        "working"
                    },
                ),
            })
            .unwrap();
    }
    for (id, name) in [("assigned", "Alice"), ("reassigned", "Bob")] {
        state
            .apply_checked(&Event::TaskAssignmentUpdated {
                assignment: TaskAssignment {
                    task_id: id.into(),
                    name: name.into(),
                    session_id: "s1".into(),
                    assignment_revision: 3,
                    reflow: None,
                    updated_ms: 1,
                },
            })
            .unwrap();
    }
    state.task_lifecycle.insert(
        "assigned".into(),
        state::TaskLifecycleRecord {
            delivery_evidence: Some("evidence".into()),
            ..Default::default()
        },
    );
    let lifecycle = serde_json::to_value(&state.task_lifecycle).unwrap();
    state.worktree_bindings.insert(
        "binding".into(),
        WorktreeBinding {
            worktree_root: "/resource".into(),
            owning_project_scope: "/project".into(),
            task_id: "assigned".into(),
            owner_agent_id: "old".into(),
            binding_id: "binding".into(),
            base_commit: "base".into(),
        },
    );
    state.pending_merges.insert(
        "assigned".into(),
        state::PendingMerge {
            task_id: "assigned".into(),
            owner: "old".into(),
            requested_by: "master".into(),
            requested_ms: 1,
            candidate_commit: Some("candidate".into()),
        },
    );
    let mut i = input("new", "s2");
    i.intent = Some(NameIntent {
        requested_name: "Alice".into(),
        user_authorized: true,
        replace_confirmed: true,
    });
    state
        .apply_checked(&Event::AgentPresenceObserved {
            presence: AgentPresenceRecord {
                project_scope: "/project".into(),
                worker_id: "old".into(),
                session_id: "s1".into(),
                presence: AgentPresence::Online,
                reason: "test".into(),
                observed_ms: 20,
            },
        })
        .unwrap();
    committed(&mut state, &i);
    for id in ["legacy", "assigned"] {
        assert_eq!(state.tasks[id].owner, "new");
        assert_eq!(state.tasks[id].status, "working");
        assert_eq!(state.tasks[id].branch.as_deref(), Some("branch"));
        assert_eq!(state.task_assignments[id].session_id, "s2");
    }
    assert_eq!(state.tasks["reassigned"].owner, "old");
    assert_eq!(state.task_assignments["reassigned"].name, "Bob");
    assert_eq!(state.tasks["cancelled"].owner, "old");
    assert_eq!(state.worktree_bindings["binding"].owner_agent_id, "new");
    assert_eq!(state.worktree_bindings["binding"].base_commit, "base");
    assert_eq!(state.pending_merges["assigned"].owner, "new");
    assert_eq!(
        state.pending_merges["assigned"].candidate_commit.as_deref(),
        Some("candidate")
    );
    assert_eq!(
        serde_json::to_value(&state.task_lifecycle).unwrap(),
        lifecycle
    );
}

#[test]
fn current_occupancy_beats_duplicate_labels_and_internal_id_stays_unchanged() {
    let mut state = State::default();
    committed(&mut state, &input("holder", "s1"));
    state.worker_names.insert("stale".into(), "Alice".into());
    state.worker_names.insert("holder".into(), "Bob".into());
    assert_eq!(worker_display_name(&state, "holder", false), "Alice");
    assert_ne!(worker_display_name(&state, "stale", false), "Alice");
    assert_eq!(
        state.name_occupancy("/project", "Alice").unwrap().worker_id,
        "holder"
    );
}

#[test]
fn name_intent_rejects_unapproved_invalid_and_untyped_requests() {
    for value in [
        json!({"action":"restore_name","requested_name":"Outside","decision":"approved"}),
        json!({"action":"restore_name"}),
        json!({"action":"restore_master","replace_confirmed":"true"}),
    ] {
        assert!(parse_name_intent(Some(&value)).is_err());
    }
    let mut i = input("worker", "s1");
    i.intent = parse_name_intent(Some(
        &json!({"action":"restore_name","requested_name":"Bob","decision":"denied"}),
    ))
    .unwrap();
    assert!(plan_name_resolution(&State::default(), &i)
        .unwrap_err()
        .contains("NAME_AUTHORIZATION_REQUIRED"));
    i.intent = None;
    i.request.requested_name = Some("Bob".into());
    assert!(plan_name_resolution(&State::default(), &i)
        .unwrap_err()
        .contains("NAME_AUTHORIZATION_REQUIRED"));
}

#[test]
fn pending_is_durable_idempotent_and_online_activity_unknown_is_not_pending() {
    let mut state = State::default();
    committed(&mut state, &input("old", "s1"));
    let mut i = input("new", "s2");
    i.intent = Some(NameIntent {
        requested_name: "Alice".into(),
        user_authorized: true,
        replace_confirmed: true,
    });
    let plan = plan_name_resolution(&state, &i).unwrap();
    assert!(matches!(
        plan.decision,
        NameResolutionDecision::Pending { .. }
    ));
    for event in plan.events {
        state.apply_checked(&event).unwrap();
    }
    assert_eq!(
        state.name_occupancy("/project", "Alice").unwrap().worker_id,
        "old"
    );
    assert!(plan_name_resolution(&state, &i).unwrap().events.is_empty());
    state
        .apply_checked(&Event::AgentPresenceObserved {
            presence: AgentPresenceRecord {
                project_scope: "/project".into(),
                worker_id: "old".into(),
                session_id: "stale-session".into(),
                presence: AgentPresence::Offline,
                reason: "stale".into(),
                observed_ms: 20,
            },
        })
        .unwrap();
    assert!(matches!(
        plan_name_resolution(&state, &i).unwrap().decision,
        NameResolutionDecision::Pending { .. }
    ));
    state
        .apply_checked(&Event::AgentPresenceObserved {
            presence: AgentPresenceRecord {
                project_scope: "/project".into(),
                worker_id: "old".into(),
                session_id: "s1".into(),
                presence: AgentPresence::Online,
                reason: "visible".into(),
                observed_ms: 20,
            },
        })
        .unwrap();
    state
        .apply_checked(&Event::AgentActivityObserved {
            activity: AgentActivityRecord {
                project_scope: "/project".into(),
                worker_id: "old".into(),
                session_id: "s1".into(),
                activity: AgentActivity::Unknown,
                background: true,
                reason: "background".into(),
                observed_ms: 20,
            },
        })
        .unwrap();
    i.intent.as_mut().unwrap().replace_confirmed = false;
    assert!(matches!(
        plan_name_resolution(&state, &i).unwrap().decision,
        NameResolutionDecision::NeedsConfirmation { .. }
    ));
}
