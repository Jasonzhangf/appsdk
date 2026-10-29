use super::*;
use crate::server::peer_tests::{register_synthetic_peer, test_server};

#[test]
fn absence_of_a_probe_fact_does_not_fabricate_periodic_unknown() {
    let (server, root, _) = fixture();
    tick_checked(&server, 100).unwrap();
    assert!(server.state.lock().unwrap().name_recoveries.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

fn tick_checked(server: &Server, now: i64) -> Result<(), String> {
    super::tick_with_notifier(server, now, &|_, _, _, _| {})
}

fn fixture() -> (Server, std::path::PathBuf, RouteScope) {
    let (server, root) = test_server();
    for id in ["old", "new", "jenny", "next-jenny"] {
        assert!(register_synthetic_peer(&server, id, id).ok);
    }
    let mut state = server.state.lock().unwrap();
    let route = server_route_scope(&server, &state).unwrap().unwrap();
    // Registration supplies names and default leases. This fixture isolates
    // one occupancy and starts without a notification subscription.
    state.name_occupancies.clear();
    state.notification_subscriptions.clear();
    state.msgs.clear();
    state
        .apply_checked(&Event::NameResolutionCommitted {
            project_scope: route.project_scope_id.as_str().into(),
            assignments: vec![],
            occupancy: NameOccupancy {
                name: "Alice".into(),
                worker_id: "old".into(),
                session_id: "session-old".into(),
                occupancy_revision: 1,
                updated_ms: 1,
            },
        })
        .unwrap();
    for id in ["one", "two", "closed"] {
        let task: TaskRec = serde_json::from_value(
            json!({"id":id,"owner":"old","created_by":"old", "priority":"p2",
            "status":if id == "closed" {"closed"} else {"working"}, "created_ms":1,"updated_ms":1}),
        )
        .unwrap();
        state.apply_checked(&Event::TaskCreated { task }).unwrap();
        state
            .apply_checked(&Event::TaskAssignmentUpdated {
                assignment: TaskAssignment {
                    task_id: id.into(),
                    name: "Alice".into(),
                    session_id: "session-old".into(),
                    assignment_revision: 1,
                    reflow: None,
                    updated_ms: 1,
                },
            })
            .unwrap();
    }
    drop(state);
    (server, root, route)
}

fn presence(server: &Server, route: &RouteScope, value: AgentPresence, time: i64) {
    server
        .state
        .lock()
        .unwrap()
        .apply_checked(&Event::AgentPresenceObserved {
            presence: AgentPresenceRecord {
                project_scope: route.project_scope_id.as_str().into(),
                worker_id: "old".into(),
                session_id: "session-old".into(),
                presence: value,
                reason: "deterministic test observation".into(),
                observed_ms: time,
            },
        })
        .unwrap();
}

fn master(server: &Server, route: &RouteScope, worker: &str) {
    let mut state = server.state.lock().unwrap();
    let grant =
        master_grant_for_worker(&state, route, worker, "user", "test authorization").unwrap();
    let events = master_authority_transfer_events(&state, route, grant);
    server.commit_locked_checked(&mut state, &events).unwrap();
}

fn subscribe(server: &Server, worker: &str) {
    server
        .state
        .lock()
        .unwrap()
        .apply_checked(&Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: format!("sub-{worker}"),
                worker_id: worker.into(),
                event: "direct-message".into(),
                subject: None,
                target: format!("thread-{worker}"),
                method: "appserver".into(),
                trigger_ms: None,
                trigger_times_ms: vec![],
                interval_ms: None,
                repeat_count: 100,
                fired_count: 0,
                expires_ms: i64::MAX,
                status: "armed".into(),
                created_ms: 1,
                updated_ms: 1,
                status_reason: None,
                patrol_idle_since_ms: None,
                patrol_last_push_ms: 0,
            },
        })
        .unwrap();
}

fn pending(server: &Server, route: &RouteScope, confirmed: bool) {
    let mut state = server.state.lock().unwrap();
    let binding = authoritative_send_binding(&state, route, "new").unwrap();
    let recovery = NameRecoveryPending {
        operation_id: "restore".into(),
        project_scope: route.project_scope_id.as_str().into(),
        target_name: "Alice".into(),
        occupancy_ref: state
            .name_occupancy_ref(route.project_scope_id.as_str(), "Alice")
            .unwrap(),
        request: NameRecoveryRequest {
            invocation: "context".into(),
            requested_name: Some("Alice".into()),
            caller_worker_id: "new".into(),
            caller_session_id: "old-request-session".into(),
            caller_binding: NameBindingReference {
                app_scope_id: binding.app_scope_id.to_string(),
                binding_id: binding.binding_id.to_string(),
                endpoint_generation: binding.endpoint_generation,
            },
            replace_confirmed: confirmed,
            requested_ms: 1,
        },
        progress: NameRecoveryProgress::Pending,
        decision: None,
        result: None,
        created_ms: 1,
        updated_ms: 1,
    };
    server
        .commit_locked_checked(&mut state, &[Event::NameRecoveryUpdated { recovery }])
        .unwrap();
}

#[test]
fn offline_reflow_is_once_per_task_and_recovery_starts_new_episode() {
    let (server, root, route) = fixture();
    presence(&server, &route, AgentPresence::Offline, 2);
    tick_checked(&server, 3).unwrap();
    let before = std::fs::read(&server.journal_path).unwrap();
    tick_checked(&server, 4).unwrap();
    assert_eq!(std::fs::read(&server.journal_path).unwrap(), before);
    {
        let state = server.state.lock().unwrap();
        for id in ["one", "two"] {
            let flow = state.task_assignment(id).unwrap().reflow.as_ref().unwrap();
            assert_eq!(flow.episode, 1);
            assert_eq!(flow.delivery, SchedulingReflowDelivery::Pending);
            assert!(flow.message_id.is_none());
        }
        assert!(state.task_assignment("closed").unwrap().reflow.is_none());
    }
    presence(&server, &route, AgentPresence::Online, 5);
    tick_checked(&server, 6).unwrap();
    presence(&server, &route, AgentPresence::Offline, 7);
    tick_checked(&server, 8).unwrap();
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .task_assignment("one")
            .unwrap()
            .reflow
            .as_ref()
            .unwrap()
            .episode,
        2
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unknown_pending_survives_replay_and_activity_unknown_does_not_adjudicate() {
    let (server, root, route) = fixture();
    presence(&server, &route, AgentPresence::Online, 2);
    tick_checked(&server, 3).unwrap();
    assert!(server.state.lock().unwrap().name_recoveries.is_empty());
    presence(&server, &route, AgentPresence::Unknown, 4);
    tick_checked(&server, 5).unwrap();
    let events = server.state.lock().unwrap().snapshot_events();
    let mut replay = State::default();
    for event in events {
        replay.apply_checked(&event).unwrap();
    }
    assert_eq!(replay.name_recoveries.len(), 1);
    let recovery = replay.name_recoveries.values().next().unwrap();
    assert_eq!(recovery.progress, NameRecoveryProgress::Pending);
    assert_eq!(recovery.request.invocation, "presence_adjudication");
    *server.state.lock().unwrap() = replay;
    let before = std::fs::read(&server.journal_path).unwrap();
    tick_checked(&server, 6).unwrap();
    assert_eq!(std::fs::read(&server.journal_path).unwrap(), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn offline_adjudication_inherits_all_open_work_using_current_session() {
    let (server, root, route) = fixture();
    master(&server, &route, "jenny");
    pending(&server, &route, false);
    server.state.lock().unwrap().worktree_bindings.insert(
        "resource".into(),
        WorktreeBinding {
            worktree_root: "/fixture/worktree".into(),
            owning_project_scope: route.project_scope_id.as_str().into(),
            task_id: "one".into(),
            owner_agent_id: "old".into(),
            binding_id: "resource".into(),
            base_commit: "base".into(),
        },
    );
    let response = adjudicate(
        &server,
        "jenny",
        "token-jenny",
        "restore",
        NameRecoveryVerdict::Offline,
        "snapshot shows shell",
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["outcome"], "applied");
    let state = server.state.lock().unwrap();
    assert_eq!(
        state
            .name_occupancy(route.project_scope_id.as_str(), "Alice")
            .unwrap()
            .worker_id,
        "new"
    );
    for id in ["one", "two"] {
        assert_eq!(state.task_assignment(id).unwrap().session_id, "session-new");
        assert_eq!(state.tasks[id].owner, "new");
    }
    assert_eq!(state.tasks["closed"].owner, "old");
    assert_eq!(state.worktree_bindings["resource"].owner_agent_id, "new");
    drop(state);
    let before = std::fs::read(&server.journal_path).unwrap();
    assert!(
        adjudicate(
            &server,
            "jenny",
            "token-jenny",
            "restore",
            NameRecoveryVerdict::Offline,
            "same snapshot"
        )
        .ok
    );
    assert_eq!(std::fs::read(&server.journal_path).unwrap(), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn online_needs_confirmation_unknown_retains_and_stale_cannot_take_over() {
    let (server, root, route) = fixture();
    master(&server, &route, "jenny");
    pending(&server, &route, false);
    assert!(
        !adjudicate(
            &server,
            "old",
            "token-old",
            "restore",
            NameRecoveryVerdict::Offline,
            "snapshot"
        )
        .ok
    );
    assert!(
        !adjudicate(
            &server,
            "jenny",
            "wrong",
            "restore",
            NameRecoveryVerdict::Offline,
            "snapshot"
        )
        .ok
    );
    let response = adjudicate(
        &server,
        "jenny",
        "token-jenny",
        "restore",
        NameRecoveryVerdict::Online,
        "snapshot shows agent",
    );
    assert_eq!(response.data["outcome"], "needs_confirmation");
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .name_occupancy(route.project_scope_id.as_str(), "Alice")
            .unwrap()
            .worker_id,
        "old"
    );
    let response = adjudicate(
        &server,
        "jenny",
        "token-jenny",
        "restore",
        NameRecoveryVerdict::Unknown,
        "",
    );
    assert_eq!(response.data["outcome"], "pending");
    let before = std::fs::read(&server.journal_path).unwrap();
    assert_eq!(
        adjudicate(
            &server,
            "jenny",
            "token-jenny",
            "restore",
            NameRecoveryVerdict::Offline,
            ""
        )
        .data["outcome"],
        "pending"
    );
    assert_eq!(
        std::fs::read(&server.journal_path).unwrap(),
        before,
        "repeated insufficient snapshot verdict is idempotent"
    );
    server
        .state
        .lock()
        .unwrap()
        .name_occupancies
        .get_mut(route.project_scope_id.as_str())
        .unwrap()
        .get_mut("Alice")
        .unwrap()
        .occupancy_revision += 1;
    let response = adjudicate(
        &server,
        "jenny",
        "token-jenny",
        "restore",
        NameRecoveryVerdict::Offline,
        "old snapshot",
    );
    assert_eq!(response.data["outcome"], "stale_decision");
    assert_eq!(server.state.lock().unwrap().tasks["one"].owner, "old");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn known_jenny_decision_survives_unknown_but_new_explicit_observation_wins() {
    let (server, root, route) = fixture();
    master(&server, &route, "jenny");
    presence(&server, &route, AgentPresence::Unknown, 2);
    tick_checked(&server, 3).unwrap();
    let op = server
        .state
        .lock()
        .unwrap()
        .name_recoveries
        .keys()
        .next()
        .unwrap()
        .clone();
    let response = adjudicate(
        &server,
        "jenny",
        "token-jenny",
        &op,
        NameRecoveryVerdict::Offline,
        "current shell snapshot",
    );
    assert!(response.ok, "{response:?}");
    let mut probe = server
        .state
        .lock()
        .unwrap()
        .agent_presence(route.project_scope_id.as_str(), "old")
        .unwrap()
        .clone();
    probe.presence = AgentPresence::Unknown;
    probe.observed_ms += 1;
    assert!(retain_adjudicated_presence(
        &server.state.lock().unwrap(),
        &probe
    ));
    // Exercise the actual monitor integration, with no tmux process involved.
    crate::server::name_monitor::tick_with_observer(&server, probe.observed_ms, &|_| {
        Err("temporary snapshot failure".into())
    });
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .agent_presence(route.project_scope_id.as_str(), "old")
            .unwrap()
            .presence,
        AgentPresence::Offline,
        "inconclusive periodic probe must preserve applied Jenny verdict"
    );
    let decision_time = probe.observed_ms - 1;
    crate::server::name_monitor::tick_with_observer(&server, decision_time, &|_| {
        Ok(crate::client::adapters::tmux::AgentObservation {
            agent_presence: crate::client::adapters::tmux::AgentPresence::Offline,
            activity: crate::client::adapters::tmux::AgentActivity::Unknown,
            background: false,
            foreground_command: None,
            reason: "new deterministic shell observation".into(),
        })
    });
    assert!(
        !retain_adjudicated_presence(&server.state.lock().unwrap(), &probe),
        "same-ms deterministic observation must supersede the verdict"
    );
    presence(
        &server,
        &route,
        AgentPresence::Online,
        probe.observed_ms + 1,
    );
    assert!(!retain_adjudicated_presence(
        &server.state.lock().unwrap(),
        &probe
    ));
    presence(
        &server,
        &route,
        AgentPresence::Unknown,
        probe.observed_ms + 2,
    );
    tick_checked(&server, probe.observed_ms + 3).unwrap();
    assert_eq!(
        server.state.lock().unwrap().name_recoveries.len(),
        2,
        "new inconclusive episode needs a new adjudication after explicit evidence"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pending_notifications_follow_current_jenny_and_do_not_claim_consumption() {
    let (server, root, route) = fixture();
    master(&server, &route, "jenny");
    presence(&server, &route, AgentPresence::Offline, 2);
    tick_checked(&server, 3).unwrap();
    assert!(
        server.state.lock().unwrap().msgs.is_empty(),
        "no subscription retains pending facts"
    );
    master(&server, &route, "next-jenny");
    subscribe(&server, "next-jenny");
    let attempts = std::cell::RefCell::new(Vec::new());
    super::tick_with_notifier(&server, 4, &|_, id, sub, _| {
        attempts.borrow_mut().push((id.to_owned(), sub.to_owned()))
    })
    .unwrap();
    assert_eq!(attempts.borrow().len(), 2);
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs.len(), 2);
    assert!(state
        .msgs
        .values()
        .all(|msg| msg.to == "next-jenny" && msg.state == "pending"));
    assert_eq!(state.tasks["one"].owner, "old");
    assert!(state
        .msgs
        .values()
        .all(|msg| msg.body.contains("Jenny 不是执行者")));
    assert_eq!(
        state
            .task_assignment("one")
            .unwrap()
            .reflow
            .as_ref()
            .unwrap()
            .delivery,
        SchedulingReflowDelivery::Pending,
        "enqueue is not adaptor acceptance"
    );
    drop(state);
    let before = std::fs::read(&server.journal_path).unwrap();
    super::tick_with_notifier(&server, 5, &|_, _, _, _| {
        panic!("duplicate adapter attempt")
    })
    .unwrap();
    assert_eq!(std::fs::read(&server.journal_path).unwrap(), before);
    let ids: Vec<_> = server.state.lock().unwrap().msgs.keys().cloned().collect();
    for id in ids {
        server
            .state
            .lock()
            .unwrap()
            .apply_checked(&Event::NotificationDeliveryAccepted {
                message_id: id,
                accepted_ms: 6,
                evidence: None,
            })
            .unwrap();
    }
    tick_checked(&server, 6).unwrap();
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .task_assignment("one")
            .unwrap()
            .reflow
            .as_ref()
            .unwrap()
            .delivery,
        SchedulingReflowDelivery::Delivered
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unknown_request_enqueues_once_and_reassignment_uses_new_name() {
    let (server, root, route) = fixture();
    master(&server, &route, "jenny");
    subscribe(&server, "jenny");
    presence(&server, &route, AgentPresence::Unknown, 2);
    tick_checked(&server, 3).unwrap();
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs.len(), 1);
        assert_eq!(
            state.name_recoveries.values().next().unwrap().progress,
            NameRecoveryProgress::AdjudicationRequested
        );
        assert!(state
            .msgs
            .values()
            .next()
            .unwrap()
            .body
            .contains("operation_id="));
    }
    let before = std::fs::read(&server.journal_path).unwrap();
    tick_checked(&server, 4).unwrap();
    assert_eq!(std::fs::read(&server.journal_path).unwrap(), before);
    {
        let mut state = server.state.lock().unwrap();
        state.task_assignments.get_mut("one").unwrap().name = "Bob".into();
        state
            .apply_name_occupancy(
                route.project_scope_id.as_str(),
                &NameOccupancy {
                    name: "Bob".into(),
                    worker_id: "new".into(),
                    session_id: "session-new".into(),
                    occupancy_revision: 1,
                    updated_ms: 5,
                },
            )
            .unwrap();
        state
            .apply_agent_presence(&AgentPresenceRecord {
                project_scope: route.project_scope_id.as_str().into(),
                worker_id: "new".into(),
                session_id: "session-new".into(),
                presence: AgentPresence::Online,
                reason: "current agent".into(),
                observed_ms: 5,
            })
            .unwrap();
    }
    presence(&server, &route, AgentPresence::Offline, 5);
    tick_checked(&server, 6).unwrap();
    let state = server.state.lock().unwrap();
    assert!(state.task_assignment("one").unwrap().reflow.is_none());
    assert!(state.task_assignment("two").unwrap().reflow.is_some());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn confirmed_online_jenny_handoff_commits_grant_and_name_together() {
    let (server, root, route) = fixture();
    master(&server, &route, "jenny");
    pending(&server, &route, true);
    {
        let mut state = server.state.lock().unwrap();
        let mut occupancy = state
            .name_occupancy(route.project_scope_id.as_str(), "Alice")
            .unwrap()
            .clone();
        occupancy.name = "Jenny".into();
        occupancy.worker_id = "jenny".into();
        occupancy.session_id = "session-jenny".into();
        state
            .apply_name_occupancy(route.project_scope_id.as_str(), &occupancy)
            .unwrap();
        let recovery = state.name_recoveries.get_mut("restore").unwrap();
        recovery.target_name = "Jenny".into();
        recovery.occupancy_ref = occupancy.reference(route.project_scope_id.as_str());
        recovery.request.requested_name = Some("Jenny".into());
    }
    let response = adjudicate(
        &server,
        "jenny",
        "token-jenny",
        "restore",
        NameRecoveryVerdict::Online,
        "snapshot confirms current agent; request already confirmed",
    );
    assert!(response.ok, "{response:?}");
    let state = server.state.lock().unwrap();
    assert_eq!(
        current_master_worker_id(&state, Some(&route)).as_deref(),
        Some("new")
    );
    assert_eq!(
        state
            .name_occupancy(route.project_scope_id.as_str(), "Jenny")
            .unwrap()
            .worker_id,
        "new"
    );
    drop(state);
    assert!(
        !adjudicate(
            &server,
            "jenny",
            "token-jenny",
            "restore",
            NameRecoveryVerdict::Offline,
            "old master"
        )
        .ok
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn persistence_failure_is_explicit_and_does_not_publish_name_or_presence() {
    let (mut server, root, route) = fixture();
    master(&server, &route, "jenny");
    pending(&server, &route, false);
    let before = serde_json::to_value(server.state.lock().unwrap().snapshot_events()).unwrap();
    server.journal = std::sync::Mutex::new(std::fs::File::open(&server.journal_path).unwrap());
    let response = adjudicate(
        &server,
        "jenny",
        "token-jenny",
        "restore",
        NameRecoveryVerdict::Offline,
        "snapshot shell",
    );
    assert!(!response.ok);
    assert!(response
        .error
        .unwrap()
        .contains("NAME_ADJUDICATION_DURABILITY_FAILED"));
    assert_eq!(
        serde_json::to_value(server.state.lock().unwrap().snapshot_events()).unwrap(),
        before
    );
    std::fs::remove_dir_all(root).unwrap();
}
