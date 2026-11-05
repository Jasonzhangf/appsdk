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
        pane_anchor: None,
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
        pane_anchor: None,
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
        replayed
            .name_occupancy("/project", "Bob")
            .unwrap()
            .worker_id,
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
        pane_anchor: None,
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
        state
            .name_occupancy("/project", "Carol")
            .unwrap()
            .session_id,
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
        pane_anchor: None,
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
        pane_anchor: None,
    };
    state
        .name_occupancies
        .entry(scope.clone())
        .or_default()
        .insert("Jenny".into(), previous);
    let mut goal =
        state.notification_subscriptions[&mailbox::default_direct_message_id("old")].clone();
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
        pane_anchor: None,
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
        state.wake_bindings["pending-goal-message"], "goal-schedule",
        "the pending goal wake must stay bound to its transferred schedule"
    );

    let promoted = NameOccupancy {
        name: "Jenny".into(),
        worker_id: "new".into(),
        session_id: "shared-session".into(),
        occupancy_revision: 3,
        updated_ms: 3,
        pane_anchor: None,
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
    assert_eq!(state.wake_bindings["pending-goal-message"], "goal-schedule");
    assert!(!state
        .notification_subscriptions
        .values()
        .any(|subscription| {
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
        let _ = pane;
        peer_tests::register(server, id, session)
    };
    assert!(register(&server, "old", 1, "shared-session").ok);

    let promoted = handle_master_promote(
        &server,
        "old".into(),
        "token-old".into(),
        "user approved old".into(),
    );
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

    let promoted = handle_master_promote(
        &server,
        "new".into(),
        "token-new".into(),
        "user approved new".into(),
    );
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
        moves_existing_names: true,
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
        tmux_session_name: None,
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
        isolation_mode: None,
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
    // A same-session carrier change is the valid anchor for inheriting the
    // current name and its unfinished work.
    committed(&mut state, &input("new", "s1"));
    for id in ["a", "b"] {
        assert_eq!(state.task_assignments[id].session_id, "s1");
        assert_eq!(state.task_assignments[id].assignment_revision, 2);
        assert_eq!(state.tasks[id].worktree_path.as_deref(), Some("/resource"));
        assert_eq!(state.tasks[id].next_step.as_deref(), Some("preserved"));
        assert_eq!(state.board_details[id].revision, 1);
    }
    assert_eq!(state.tasks["a"].owner, "new");
    assert_eq!(state.tasks["b"].owner, "new");
    assert_eq!(state.tasks["b"].status, "accepted");
    assert_eq!(serde_json::to_value(&state.tasks["c"]).unwrap(), before);
    assert_eq!(state.task_assignments["c"].session_id, "s1");
    assert_eq!(state.task_assignments["d"].session_id, "s1");
}

#[test]
fn same_worker_without_session_or_tmux_name_does_not_inherit_old_name_work_or_mailbox() {
    let mut state = State::default();
    committed(&mut state, &input("w1", "s1"));
    state
        .apply_checked(&Event::TaskCreated {
            task: task("t", "working"),
        })
        .unwrap();
    state.tasks.get_mut("t").unwrap().owner = "w1".into();
    state
        .apply_checked(&Event::TaskAssignmentUpdated {
            assignment: TaskAssignment {
                task_id: "t".into(),
                name: "Alice".into(),
                session_id: "s1".into(),
                assignment_revision: 1,
                reflow: None,
                updated_ms: 1,
            },
        })
        .unwrap();
    state
        .apply_checked(&Event::Sent {
            msg: Message {
                id: "pending".into(),
                from: "sender".into(),
                to: "w1".into(),
                mtype: "notify".into(),
                subject: Some("old-name".into()),
                body: "original body".into(),
                in_reply_to: None,
                created_ms: 1,
                state: "pending".into(),
                wake_attempt_count: 2,
                last_wake_attempt_ms: 1,
                retry_attempted: false,
            },
        })
        .unwrap();
    state
        .apply_checked(&Event::MailboxNameAddressed {
            message_id: "pending".into(),
            address: name_mailbox::MailboxNameAddress {
                project_scope: "/project".into(),
                name: "Alice".into(),
            },
        })
        .unwrap();
    let before_task = serde_json::to_value(&state.tasks["t"]).unwrap();
    let before_assignment = serde_json::to_value(&state.task_assignments["t"]).unwrap();
    let before_message = serde_json::to_value(&state.msgs["pending"]).unwrap();
    let before_responsibility = state::responsibility_snapshot(&state, "w1");

    let occupancy = committed(&mut state, &input("w1", "s2"));
    assert_eq!(occupancy.name, "Bob");
    assert_eq!(occupancy.session_id, "s2");
    assert_eq!(
        serde_json::to_value(&state.tasks["t"]).unwrap(),
        before_task
    );
    assert_eq!(
        serde_json::to_value(&state.task_assignments["t"]).unwrap(),
        before_assignment
    );
    assert_eq!(
        serde_json::to_value(&state.msgs["pending"]).unwrap(),
        before_message
    );
    assert_eq!(
        state.mailbox_name_addresses["pending"].name, "Alice",
        "a fresh ordinary name must not inherit the old name's mailbox"
    );
    assert!(state.mailbox_handoffs.is_empty());
    assert_eq!(
        state::responsibility_snapshot(&state, "w1"),
        before_responsibility,
        "same worker id without an anchor must not move responsibility"
    );
}

#[test]
fn explicit_name_overrides_any_presence_and_jenny_plan() {
    let mut state = State::default();
    committed(&mut state, &input("old", "s1"));
    let mut i = input("new", "s2");
    i.intent = Some(NameIntent {
        requested_name: "Alice".into(),
        user_authorized: true,
        moves_existing_names: true,
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
        assert!(matches!(
            plan.decision,
            NameResolutionDecision::Committed { .. }
        ));
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
fn registered_name_owner_replaces_online_master_with_user_authorization() {
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
    let promoted = handle_master_promote(
        &server,
        "second".into(),
        "second".into(),
        "user asked".into(),
    );
    assert!(!promoted.ok, "{promoted:?}");
    assert!(promoted
        .error
        .as_deref()
        .unwrap()
        .starts_with("NAME_RECOVERY_REQUIRED"));
    assert_eq!(promoted.data["outcome"], "pending", "{promoted:?}");
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
            moves_existing_names: true,
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
            isolation_mode: None,
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
        moves_existing_names: true,
        replace_confirmed: false,
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
fn plan_explicit_name_move_reports_target_and_retired_source_names() {
    let (server, root) = peer_tests::test_server();
    for id in ["old", "new"] {
        assert!(peer_tests::register_synthetic_peer(&server, id, id).ok);
    }
    let scope = {
        let state = server.state.lock().unwrap();
        server_route_scope(&server, &state)
            .unwrap()
            .unwrap()
            .project_scope_id
            .as_str()
            .to_owned()
    };
    {
        let mut state = server.state.lock().unwrap();
        let names = state.name_occupancies.entry(scope).or_default();
        names.insert(
            "Alice".into(),
            NameOccupancy {
                name: "Alice".into(),
                worker_id: "old".into(),
                session_id: "session-old".into(),
                occupancy_revision: 1,
                updated_ms: 1,
                pane_anchor: None,
            },
        );
        names.insert(
            "Bob".into(),
            NameOccupancy {
                name: "Bob".into(),
                worker_id: "new".into(),
                session_id: "session-new".into(),
                occupancy_revision: 1,
                updated_ms: 1,
                pane_anchor: None,
            },
        );
    }

    let moved = plan_explicit_name_move(
        &server,
        "new",
        "names-restore",
        "context",
        &NameIntent {
            requested_name: "Alice".into(),
            user_authorized: true,
            moves_existing_names: true,
            replace_confirmed: false,
        },
        None,
    )
    .unwrap()
    .expect("explicit ordinary rename must plan a control move");
    assert_eq!(moved.target, "Alice");
    assert_eq!(moved.source_names, vec!["Bob".to_owned()]);

    let noop = plan_explicit_name_move(
        &server,
        "new",
        "names-restore",
        "context",
        &NameIntent {
            requested_name: "Bob".into(),
            user_authorized: true,
            moves_existing_names: true,
            replace_confirmed: false,
        },
        None,
    )
    .unwrap();
    assert!(
        noop.is_none(),
        "repeating the carrier's current name is not a move"
    );

    let jenny = plan_explicit_name_move(
        &server,
        "new",
        "names-restore",
        "context",
        &NameIntent {
            requested_name: "Jenny".into(),
            user_authorized: true,
            moves_existing_names: true,
            replace_confirmed: false,
        },
        None,
    )
    .unwrap();
    assert!(
        jenny.is_none(),
        "Jenny takeover stays on the existing grant/confirmation path"
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn plan_automatic_name_keeps_existing_carrier_and_only_pools_a_nameless_one() {
    let (server, root) = peer_tests::test_server();
    // Registration assigns each carrier a pool name, so this mirrors a named
    // current carrier. The automatic planner must return that same name and
    // not allocate a second one.
    assert!(peer_tests::register_synthetic_peer(&server, "keep", "keep").ok);
    let (assigned, scope) = {
        let state = server.state.lock().unwrap();
        let scope = server_route_scope(&server, &state)
            .unwrap()
            .unwrap()
            .project_scope_id
            .as_str()
            .to_owned();
        let name = state
            .name_occupancies
            .get(&scope)
            .and_then(|names| names.values().find(|holder| holder.worker_id == "keep"))
            .map(|holder| holder.name.clone())
            .expect("registration assigns the carrier a name");
        (name, scope)
    };
    let kept = plan_automatic_name(&server, "keep", "names-keep", "context", None)
        .unwrap()
        .expect("an already named carrier has a committed automatic decision");
    assert_eq!(kept, assigned);

    // A legacy identity may carry no occupancy at all. With the pool empty
    // again, the automatic planner allocates the first pool name instead of
    // referencing a name that the carrier does not hold.
    {
        let mut state = server.state.lock().unwrap();
        state
            .name_occupancies
            .get_mut(&scope)
            .expect("name occupancy table exists")
            .retain(|_, holder| holder.worker_id != "keep");
    }
    let allocated = plan_automatic_name(&server, "keep", "names-fresh", "context", None)
        .unwrap()
        .expect("a nameless carrier receives a pool decision");
    assert_eq!(allocated, "Alice");

    std::fs::remove_dir_all(root).unwrap();
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
    ] {
        assert!(parse_name_intent(Some(&value)).is_err());
    }
    assert!(parse_name_intent(Some(
        &json!({"action":"restore_master","replace_confirmed":"true"})
    ))
    .unwrap_err()
    .contains("replace_confirmed must be boolean"));
    assert!(matches!(
        parse_name_intent(Some(&json!({"action":"restore_master"}))).unwrap(),
        Some(NameIntent {
            requested_name,
            user_authorized: false,
            moves_existing_names: true,
            replace_confirmed: false,
        }) if requested_name == "Jenny"
    ));
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

fn planned_name(state: &State, i: &NameResolutionInput) -> String {
    match plan_name_resolution(state, i).unwrap().decision {
        NameResolutionDecision::Committed { occupancy, .. } => occupancy.name,
        other => panic!("unexpected decision: {other:?}"),
    }
}

#[test]
fn explicit_name_overrides_online_holder_without_confirmation() {
    let mut state = State::default();
    committed(&mut state, &input("old", "s1"));
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
    let mut i = input("new", "s2");
    i.intent = Some(NameIntent {
        requested_name: "Alice".into(),
        user_authorized: true,
        moves_existing_names: true,
        replace_confirmed: false,
    });
    let plan = plan_name_resolution(&state, &i).unwrap();
    assert!(
        matches!(&plan.decision, NameResolutionDecision::Committed { occupancy, .. }
            if occupancy.name == "Alice" && occupancy.worker_id == "new")
    );
}

#[test]
fn session_anchor_beats_stale_worker_name() {
    let mut state = State::default();
    committed(&mut state, &input("old", "s1"));
    committed(&mut state, &input("bob", "s2"));
    // The worker id is a carrier, not a name anchor: the current session's
    // name wins over a stale occupancy left on the same worker id.
    assert_eq!(planned_name(&state, &input("old", "s2")), "Bob");
}

#[test]
fn duplicate_session_names_report_conflict_without_selecting_a_holder() {
    let mut state = State::default();
    let names = state.name_occupancies.entry("/project".into()).or_default();
    for (name, worker) in [("Alice", "first"), ("Bob", "second")] {
        names.insert(name.into(), NameOccupancy {
            name: name.into(),
            worker_id: worker.into(),
            session_id: "same-session".into(),
            occupancy_revision: 1,
            updated_ms: 1,
            pane_anchor: None,
        });
    }
    let request = input("new", "same-session");
    let error = match plan_name_resolution(&state, &request) {
        Ok(_) => panic!("ambiguous session must not select a name"),
        Err(error) => error,
    };
    assert!(error.starts_with("NAME_SESSION_CONFLICT:"), "{error}");
    assert_eq!(state.name_occupancies["/project"].len(), 2);
}

/// A one-time migration reads only the legacy record's identity (name plus
/// session). The pane placement stays on the old record and cannot make a later
/// caller inherit the name; a fixed-name caller resumes only the identity.
#[test]
fn legacy_pane_record_migrates_only_its_name_identity() {
    let mut state = State::default();
    state
        .name_occupancies
        .entry("/project".into())
        .or_default()
        .insert(
            "Alice".into(),
            NameOccupancy {
                name: "Alice".into(),
                worker_id: "old".into(),
                session_id: "s1".into(),
                occupancy_revision: 1,
                updated_ms: 1,
                pane_anchor: Some(NamePaneAnchor {
                    pane: crate::proto::TmuxPaneAnchor {
                        socket_path: "/tmp/collab-shared.sock".into(),
                        pane_id: "%3".into(),
                    },
                    cwd: "/project/sub".into(),
                }),
            },
        );
    let mut i = input("new", "s2");
    i.tmux_session_name = None;
    assert_eq!(
        planned_name(&state, &i),
        "Bob",
        "a legacy pane record must not inherit by pane or worker id"
    );
    i.tmux_session_name = Some("Alice".into());
    let plan = plan_name_resolution(&state, &i).unwrap();
    let NameResolutionDecision::Committed { occupancy, .. } = &plan.decision else {
        panic!("{:?}", plan.decision);
    };
    assert_eq!(occupancy.name, "Alice");
    assert_eq!(occupancy.worker_id, "new");
    assert_eq!(occupancy.session_id, "s2");
    assert_eq!(occupancy.pane_anchor, None);
}

#[test]
fn fixed_tmux_session_name_resumes_its_unique_typed_occupancy() {
    let mut state = State::default();
    committed(&mut state, &input("old", "s1"));
    let mut i = input("new", "s2");
    i.tmux_session_name = Some("Alice".into());
    let plan = plan_name_resolution(&state, &i).unwrap();
    let NameResolutionDecision::Committed { occupancy, .. } = &plan.decision else {
        panic!("{:?}", plan.decision);
    };
    assert_eq!(occupancy.name, "Alice");
    assert_eq!(occupancy.worker_id, "new");
    assert_eq!(occupancy.session_id, "s2");
    assert_eq!(occupancy.pane_anchor, None);
}

#[test]
fn fixed_tmux_session_name_conflicts_with_a_different_session_name() {
    let mut state = State::default();
    committed(&mut state, &input("alice", "s1"));
    committed(&mut state, &input("bob", "s2"));
    let mut i = input("alice", "s1");
    i.tmux_session_name = Some("Bob".into());
    let error = plan_name_resolution(&state, &i).unwrap_err();
    assert!(error.starts_with("NAME_TMUX_SESSION_CONFLICT:"), "{error}");
}

#[test]
fn fixed_session_name_inherits_jenny_without_copying_master_grant() {
    let mut state = State::default();
    state
        .name_occupancies
        .entry("/project".into())
        .or_default()
        .insert(
            "Jenny".into(),
            NameOccupancy {
                name: "Jenny".into(),
                worker_id: "old".into(),
                session_id: "s1".into(),
                occupancy_revision: 1,
                updated_ms: 1,
                pane_anchor: None,
            },
        );
    let mut i = input("new", "s2");
    i.tmux_session_name = Some("Jenny".into());
    let plan = plan_name_resolution(&state, &i).unwrap();
    let NameResolutionDecision::Committed { occupancy, .. } = &plan.decision else {
        panic!("{:?}", plan.decision);
    };
    assert_eq!(occupancy.name, "Jenny");
    assert_eq!(occupancy.worker_id, "new");
    assert!(
        !plan.requires_master_authorization,
        "inheriting the Jenny name is not a master grant"
    );
    assert!(!plan.events.iter().any(|event| matches!(
        event,
        Event::GlobalMasterGranted { .. } | Event::GlobalMasterRevoked { .. }
    )));
}

#[test]
fn fresh_ordinary_name_does_not_migrate_same_worker_legacy_tasks() {
    let mut state = State::default();
    committed(&mut state, &input("w1", "s1"));
    let mut record = task("legacy", "working");
    record.owner = "w1".into();
    state
        .apply_checked(&Event::TaskCreated { task: record })
        .unwrap();

    // No session or pane anchor inherits Alice. Falling through to a fresh
    // ordinary name must not use the same worker id as an identity anchor.
    let plan = plan_name_resolution(&state, &input("w1", "s2")).unwrap();
    let NameResolutionDecision::Committed { occupancy, .. } = &plan.decision else {
        panic!("{:?}", plan.decision);
    };
    assert_eq!(occupancy.name, "Bob");
    assert!(
        !plan
            .events
            .iter()
            .any(|event| matches!(event, Event::TaskUpdated { .. })),
        "a fresh ordinary name must not move an unassigned legacy task"
    );
    assert!(
        !plan
            .events
            .iter()
            .any(|event| matches!(event, Event::NameResolutionCommitted { assignments, .. } if !assignments.is_empty())),
        "a fresh ordinary name must not create a task assignment"
    );
}

#[test]
fn explicit_rename_migrates_callers_open_assignments_to_new_name() {
    let mut state = State::default();
    committed(&mut state, &input("w1", "s1"));
    committed(&mut state, &input("w2", "s2"));
    let mut record = task("t", "working");
    record.owner = "w1".into();
    state
        .apply_checked(&Event::TaskCreated { task: record })
        .unwrap();
    state
        .apply_checked(&Event::TaskAssignmentUpdated {
            assignment: TaskAssignment {
                task_id: "t".into(),
                name: "Alice".into(),
                session_id: "s1".into(),
                assignment_revision: 1,
                reflow: None,
                updated_ms: 1,
            },
        })
        .unwrap();
    state
        .apply_checked(&Event::Sent {
            msg: Message {
                id: "pending".into(),
                from: "sender".into(),
                to: "w1".into(),
                mtype: "notify".into(),
                subject: Some("old-name".into()),
                body: "original body".into(),
                in_reply_to: None,
                created_ms: 1,
                state: "pending".into(),
                wake_attempt_count: 4,
                last_wake_attempt_ms: 1,
                retry_attempted: false,
            },
        })
        .unwrap();
    state
        .apply_checked(&Event::MailboxNameAddressed {
            message_id: "pending".into(),
            address: name_mailbox::MailboxNameAddress {
                project_scope: "/project".into(),
                name: "Alice".into(),
            },
        })
        .unwrap();
    let mut i = input("w1", "s1");
    i.intent = Some(NameIntent {
        requested_name: "Bob".into(),
        user_authorized: true,
        moves_existing_names: true,
        replace_confirmed: false,
    });
    assert_eq!(committed(&mut state, &i).name, "Bob");
    assert_eq!(state.tasks["t"].owner, "w1");
    assert_eq!(state.tasks["t"].status, "working");
    assert_eq!(state.task_assignments["t"].name, "Bob");
    assert_eq!(state.task_assignments["t"].session_id, "s1");
    assert_eq!(state.msgs["pending"].to, "w1");
    assert_eq!(state.msgs["pending"].body, "original body");
    assert_eq!(state.msgs["pending"].wake_attempt_count, 4);
    assert_eq!(
        state.mailbox_name_addresses["pending"].name, "Bob",
        "explicit rename carries the caller's unconsumed name mailbox"
    );
}
