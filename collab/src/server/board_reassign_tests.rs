use super::name_state::{AgentActivity, AgentActivityRecord};
use super::name_stewardship::{identity_transition_events, scheduling_events, NameScheduleTransition};
use std::sync::{mpsc, Arc};
use std::time::Duration;

fn due_automatic_invitation() -> (Server, std::path::PathBuf, RouteScope, u64) {
    let (server, root, route) = identity_return_fixture();
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load().unwrap();
    let revision = {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(&mut state, &route, "old", AgentPresence::Offline, AgentActivity::Unknown);
        identity_return_observation(&mut state, &route, "idle", AgentPresence::Online, AgentActivity::Idle);
        let name = state.task_assignment("named-work").unwrap().name.clone();
        state.apply_checked(&Event::NameReassignmentScheduled {
            project_scope: route.project_scope_id.as_str().into(),
            name: name.clone(),
            schedule: name_state::eligible_after_retirement(&name, 1_000_000_000),
        }).unwrap();
        let prepared = scheduling_events(&server, &state, &control, 1_000_000_000).unwrap();
        server.commit_locked_checked_with_control(&mut state, &prepared.events, &control).unwrap();
        assert_eq!(state.board_details["named-work"].invitation.as_ref().unwrap().peer_id, "idle");
        state.board_details["named-work"].revision
    };
    (server, root, route, revision)
}

#[test]
fn automatic_invitation_accepts_online_recipient_while_responding() {
    for activity in [AgentActivity::Busy, AgentActivity::Unknown] {
        let (server, root, route, revision) = due_automatic_invitation();
        {
            let mut state = server.state.lock().unwrap();
            identity_return_observation(&mut state, &route, "idle", AgentPresence::Online, activity);
        }
        let response = board_respond(&server, "idle".into(), "token-idle".into(),
            "named-work".into(), revision, true, None);
        assert!(response.ok, "current recipient can actively accept without a conflicting execution slot: {response:?}");
        assert_eq!(server.state.lock().unwrap().tasks["named-work"].owner, "idle");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn automatic_scheduling_ignores_invitation_history_of_closed_tasks() {
    let (server, root, _, _) = due_automatic_invitation();
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap();
    let mut state = server.state.lock().unwrap();
    let mut closed = state.tasks["named-work"].clone();
    closed.id = "closed-invitation-history".into();
    closed.status = "closed".into();
    let historical_details = state.board_details["named-work"].clone();
    state.tasks.insert(closed.id.clone(), closed);
    state.board_details.insert("closed-invitation-history".into(), historical_details);
    state.board_details.get_mut("named-work").unwrap().invitation = None;
    let prepared = scheduling_events(&server, &state, &control, 1_000_000_001).unwrap();
    assert!(prepared.events.iter().any(|event| matches!(event,
        Event::BoardDetailsChanged { task_id, details }
            if task_id == "named-work" && details.invitation.as_ref().is_some_and(|invite| invite.peer_id == "idle")
    )), "a closed task's historical invitation must not reserve an idle peer");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn automatic_invitation_accept_rechecks_other_running_slot() {
    let (server, root, route, revision) = due_automatic_invitation();
    {
        let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap();
        let name = control.key_for_worker(&route.project_scope_id, &route.app_scope_id, "idle")
            .unwrap().unwrap().name.clone();
        let mut state = server.state.lock().unwrap();
        state.ticket_phases.push(ticket_state::TicketPhaseRecord {
            task_id: "other-running-work".into(), phase: ticket_state::TicketPhaseKind::Execute,
            batch: 1, priority: "P1".into(), status: ticket_state::TicketPhaseStatus::Running,
            slot_name: Some(name), binding_generation: 1, enqueue_seq: 1, attempt_generation: 1,
            depends_on: Vec::new(), route_group: None, wait_reason: None, result: None,
        });
    }
    let response = board_respond(&server, "idle".into(), "token-idle".into(),
        "named-work".into(), revision, true, None);
    assert!(!response.ok, "another actual execution must retain its slot: {response:?}");
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["named-work"].owner, "old");
    assert!(state.board_details["named-work"].invitation.is_some());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn identity_return_current_idle_without_duration_can_receive_redispatched_work() {
    let (server, root, route) = identity_return_fixture();
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap();
    let now = 1_000_000_000;
    let revision = {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(&mut state, &route, "old", AgentPresence::Offline, AgentActivity::Unknown);
        identity_return_observation(&mut state, &route, "idle", AgentPresence::Online, AgentActivity::Idle);
        state.agent_activity.get_mut(route.project_scope_id.as_str()).unwrap()
            .get_mut("idle").unwrap().idle_since_ms = None;
        let source_name = state.task_assignment("named-work").unwrap().name.clone();
        let schedule = name_state::eligible_after_retirement(
            &name_state::reassignment_relation_ref(&source_name), now,
        );
        state.apply_checked(&Event::NameReassignmentScheduled {
            project_scope: route.project_scope_id.as_str().into(), name: source_name,
            schedule,
        }).unwrap();
        let prepared = scheduling_events(&server, &state, &control, now).unwrap();
        assert!(prepared.events.iter().any(|event| matches!(event,
            Event::BoardDetailsChanged { task_id, details }
                if task_id == "named-work" && details.invitation.is_some()
        )), "current Online/Idle is sufficient capacity without a duration measurement");
        server.commit_locked_checked_with_control(&mut state, &prepared.events, &control).unwrap();
        state.board_details["named-work"].revision
    };
    let accepted = board_respond(&server, "idle".into(), "token-idle".into(),
        "named-work".into(), revision, true, None);
    assert!(accepted.ok, "current Online/Idle can accept the automatic invitation: {accepted:?}");
    assert_eq!(server.state.lock().unwrap().tasks["named-work"].owner, "idle");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn identity_return_idle_peer_remains_available_while_another_slot_runs() {
    let (server, root, route) = identity_return_fixture();
    let now = 1_000_000_000;
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap();
    let mut state = server.state.lock().unwrap();
    identity_return_observation(&mut state, &route, "old", AgentPresence::Offline, AgentActivity::Unknown);
    identity_return_observation(&mut state, &route, "idle", AgentPresence::Online, AgentActivity::Idle);
    state.apply_checked(&Event::TaskCreated { task: task("idle-peer-existing-work", "idle") }).unwrap();
    let name = state.task_assignment("named-work").unwrap().name.clone();
    state.apply_checked(&Event::NameReassignmentScheduled {
        project_scope: route.project_scope_id.as_str().into(),
        name,
        schedule: name_state::eligible_after_retirement("fixture exact retired relation", now),
    }).unwrap();
    state.ticket_phases.push(ticket_state::TicketPhaseRecord {
        task_id: "other-work".into(), phase: ticket_state::TicketPhaseKind::Execute,
        batch: 1, priority: "P1".into(), status: ticket_state::TicketPhaseStatus::Running,
        slot_name: Some("another-slot".into()), binding_generation: 1,
        enqueue_seq: 1, attempt_generation: 1, depends_on: Vec::new(),
        route_group: None, wait_reason: None, result: None,
    });
    let prepared = scheduling_events(&server, &state, &control, now).unwrap();
    assert!(prepared.events.iter().any(|event| matches!(event,
        Event::BoardDetailsChanged { task_id, details }
            if task_id == "named-work" && details.invitation.is_some()
    )), "an unrelated running slot must not hide a legal idle peer");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn identity_return_invites_only_idle_peers_with_the_required_work_role() {
    let (server, root, route) = identity_return_fixture();
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap();
    let now = 1_000_000_000;
    let mut state = server.state.lock().unwrap();
    identity_return_observation(&mut state, &route, "old", AgentPresence::Offline, AgentActivity::Unknown);
    identity_return_observation(&mut state, &route, "idle", AgentPresence::Online, AgentActivity::Idle);
    state.board_details.get_mut("named-work").unwrap().work_policy.preferred_roles = Some(vec!["designer".into()]);
    let name = state.task_assignment("named-work").unwrap().name.clone();
    let schedule = name_state::eligible_after_retirement(&name_state::reassignment_relation_ref(&name), now);
    state.apply_checked(&Event::NameReassignmentScheduled {
        project_scope: route.project_scope_id.as_str().into(), name, schedule,
    }).unwrap();
    let unqualified = scheduling_events(&server, &state, &control, now).unwrap();
    assert!(unqualified.events.is_empty(), "an idle peer that cannot accept must not reserve this task");
    let idle_name = control.key_for_worker(&route.project_scope_id, &route.app_scope_id, "idle")
        .unwrap().unwrap().name.clone();
    state.apply_checked(&Event::NameWorkRolesUpdated {
        project_scope: route.project_scope_id.as_str().into(),
        record: crate::board::NameWorkRoles { name: idle_name, work_roles: vec!["designer".into()] },
    }).unwrap();
    let qualified = scheduling_events(&server, &state, &control, now).unwrap();
    assert!(qualified.events.iter().any(|event| matches!(event,
        Event::BoardDetailsChanged { task_id, details }
            if task_id == "named-work" && details.invitation.is_some()
    )), "a current idle peer with the required role remains eligible");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

fn reassign_fixture(status: &str) -> (Server, std::path::PathBuf, RouteScope) {
    let (server, root, route) = board_name_fixture();
    let mut state = server.state.lock().unwrap();
    state.tasks.get_mut("named-work").unwrap().status = status.into();
    state
        .name_occupancies
        .get_mut(route.project_scope_id.as_str())
        .unwrap()
        .insert(
            "Bob".into(),
            NameOccupancy {
                name: "Bob".into(),
                worker_id: "current".into(),
                session_id: "session-current".into(),
                occupancy_revision: 1,
                updated_ms: 2,
                pane_anchor: None,
            },
        );
    let binding = authoritative_send_binding(&state, &route, "current")
        .unwrap()
        .clone();
    state
        .board_details
        .get_mut("named-work")
        .unwrap()
        .invitation = Some(crate::board::BoardInvitation {
        peer_id: "current".into(),
        binding_id: binding.binding_id.as_str().into(),
        message_id: "reassign-invite".into(),
        created_ms: 3,
        original_status: Some(status.into()),
        original_owner: Some("old".into()),
        automatic_source: None,
    });
    state.worktree_bindings.insert(
        "resource".into(),
        state::WorktreeBinding {
            worktree_root: "/fixture/worktree".into(),
            owning_project_scope: route.project_scope_id.as_str().into(),
            task_id: "named-work".into(),
            owner_agent_id: "old".into(),
            binding_id: "resource".into(),
            base_commit: "base".into(),
            isolation_mode: None,
        },
    );
    state.worktree_bindings.insert(
        "unrelated".into(),
        state::WorktreeBinding {
            worktree_root: "/fixture/unrelated".into(),
            owning_project_scope: route.project_scope_id.as_str().into(),
            task_id: "other".into(),
            owner_agent_id: "old".into(),
            binding_id: "unrelated".into(),
            base_commit: "other-base".into(),
            isolation_mode: None,
        },
    );
    state.pending_merges.insert(
        "named-work".into(),
        state::PendingMerge {
            task_id: "named-work".into(),
            owner: "old".into(),
            requested_by: "reviewer".into(),
            requested_ms: 10,
            candidate_commit: Some("candidate".into()),
        },
    );
    drop(state);
    (server, root, route)
}

fn identity_return_fixture() -> (Server, std::path::PathBuf, RouteScope) {
    let (server, root) = test_server();
    let app = AppServerId::new("tui-default").unwrap();
    for worker in ["master", "old", "idle"] {
        let response = crate::server::peer_tests::seed_committed_owner(
            &server,
            worker,
            &format!("token-{worker}"),
            &crate::server::peer_tests::test_appserver_transport(&format!("thread-{worker}")),
            &server.root.to_string_lossy(),
            Some(app.clone()),
        );
        assert!(response.ok, "seed committed {worker}: {:?}", response.error);
    }
    crate::server::peer_tests::promote_master(&server, "master", "user-approved");

    let mut state = server.state.lock().unwrap();
    let route = server_route_scope(&server, &state).unwrap().unwrap();
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let key = control
        .key_for_worker(&route.project_scope_id, &route.app_scope_id, "old")
        .unwrap()
        .unwrap();
    let current = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "old")
        .unwrap()
        .unwrap();
    let session = current
        .active_binding()
        .unwrap()
        .binding
        .session_id
        .as_ref()
        .unwrap()
        .to_string();
    state
        .apply_checked(&Event::TaskCreated {
            task: task("named-work", "old"),
        })
        .unwrap();
    {
        let details = state.board_details.get_mut("named-work").unwrap();
        details.public_visibility = true;
        details.long_horizon = Some(crate::board::LongHorizonTaskSpec {
            content: "continue after identity return".into(),
            interval_ms: 3_600_000,
            repeat_count: 100,
            ttl_seconds: 86_400,
        });
    }
    state
        .apply_task_assignment(&TaskAssignment {
            task_id: "named-work".into(),
            name: key.name.clone(),
            session_id: session,
            assignment_revision: 1,
            reflow: None,
            updated_ms: 1,
        })
        .unwrap();
    drop(state);
    (server, root, route)
}

fn identity_return_observation(
    state: &mut State,
    route: &RouteScope,
    worker: &str,
    presence: AgentPresence,
    activity: AgentActivity,
) {
    let session = state
        .name_occupancies
        .get(route.project_scope_id.as_str())
        .and_then(|names| {
            names
                .values()
                .find(|occupancy| occupancy.worker_id == worker)
        })
        .expect("worker occupancy")
        .session_id
        .clone();
    state
        .apply_checked(&Event::AgentPresenceObserved {
            presence: AgentPresenceRecord {
                project_scope: route.project_scope_id.as_str().into(),
                worker_id: worker.into(),
                session_id: session.clone(),
                presence,
                reason: "identity return test observation".into(),
                observed_ms: 1,
            },
        })
        .unwrap();
    state
        .apply_checked(&Event::AgentActivityObserved {
            activity: AgentActivityRecord {
                project_scope: route.project_scope_id.as_str().into(),
                worker_id: worker.into(),
                session_id: session,
                activity,
                background: false,
                reason: "identity return test observation".into(),
                observed_ms: 1,
                idle_since_ms: (activity == AgentActivity::Idle).then_some(1),
            },
        })
        .unwrap();
}

#[test]
fn prepared_claim_commit_retains_admitted_control_across_rebind() {
    let (server, root, route) = identity_return_fixture();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let admitted = store.load().unwrap();
    let idle = admitted
        .current_for_worker(
            &route.project_scope_id,
            &route.app_scope_id,
            "idle",
        )
        .unwrap()
        .unwrap();
    let idle_key = idle.identity_key.clone();
    let idle_name = idle.identity_key.name.clone();
    let idle_generation = idle.generation;
    let idle_session = idle
        .active_binding()
        .unwrap()
        .binding
        .session_id
        .as_ref()
        .unwrap()
        .to_string();

    let events = {
        let mut state = server.state.lock().unwrap();
        state
            .apply_checked(&Event::TicketPhaseRecorded {
                phase: ticket_state::TicketPhaseRecord {
                    task_id: "named-work".into(),
                    phase: ticket_state::TicketPhaseKind::Execute,
                    batch: 1,
                    priority: "P2".into(),
                    status: ticket_state::TicketPhaseStatus::Queued,
                    slot_name: Some(idle_name.clone()),
                    binding_generation: idle_generation,
                    enqueue_seq: 1,
                    attempt_generation: 0,
                    depends_on: Vec::new(),
                    route_group: None,
                    wait_reason: None,
                    result: None,
                },
            })
            .unwrap();
        let mut task = state.tasks["named-work"].clone();
        task.owner = "idle".into();
        task.status = "working".into();
        task.updated_ms = now_ms();
        let mut details = state.board_details["named-work"].clone();
        details.revision += 1;
        vec![
            Event::TaskUpdated { task },
            Event::BoardDetailsChanged {
                task_id: "named-work".into(),
                details,
            },
            Event::TicketPhaseRecorded {
                phase: ticket_state::TicketPhaseRecord {
                    task_id: "named-work".into(),
                    phase: ticket_state::TicketPhaseKind::Execute,
                    batch: 1,
                    priority: "P2".into(),
                    status: ticket_state::TicketPhaseStatus::Claimed,
                    slot_name: Some(idle_name.clone()),
                    binding_generation: idle_generation,
                    enqueue_seq: 1,
                    attempt_generation: 1,
                    depends_on: Vec::new(),
                    route_group: None,
                    wait_reason: None,
                    result: None,
                },
            },
            Event::TaskAssignmentUpdated {
                assignment: name_state::TaskAssignment {
                    task_id: "named-work".into(),
                    name: idle_name.clone(),
                    session_id: idle_session.clone(),
                    assignment_revision: state
                        .task_assignment("named-work")
                        .map_or(1, |assignment| assignment.assignment_revision + 1),
                    reflow: None,
                    updated_ms: now_ms(),
                },
            },
        ]
    };

    let mut rebound = admitted.clone();
    rebound.unregister(&idle_key, None).unwrap();
    store.save(&rebound).unwrap();

    let mut state = server.state.lock().unwrap();
    server
        .commit_locked_checked_with_control(&mut state, &events, &admitted)
        .expect("the prepared claim commit must retain its admitted control");
    assert_eq!(state.tasks["named-work"].owner, "idle");
    let claimed = state
        .ticket_phases
        .iter()
        .find(|phase| phase.task_id == "named-work")
        .expect("claim phase");
    assert_eq!(claimed.status, ticket_state::TicketPhaseStatus::Claimed);
    assert_eq!(claimed.slot_name.as_deref(), Some(idle_name.as_str()));
    let assignment = state
        .task_assignment("named-work")
        .expect("claim assignment");
    assert_eq!(assignment.name, idle_name);
    assert_eq!(assignment.session_id, idle_session);
    let subscription = state
        .notification_subscriptions
        .values()
        .find(|subscription| {
            subscription.subject.as_deref() == Some("goal:task:named-work")
        })
        .expect("claim wake subscription");
    assert_eq!(subscription.worker_id, "idle");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn identity_return_deadline_holds_until_fifteen_minutes_then_prepares_idle_redispatch() {
    let (server, root, route) = identity_return_fixture();
    {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "old",
            AgentPresence::Offline,
            AgentActivity::Unknown,
        );
        identity_return_observation(
            &mut state,
            &route,
            "idle",
            AgentPresence::Online,
            AgentActivity::Idle,
        );
    }
    let server = std::sync::Arc::new(server);
    let base = 1_000_000_000;
    timers::tick_with_idle_at(&server, base, &|_| true);
    assert!(server.state.lock().unwrap().board_details["named-work"]
        .invitation
        .is_none());
    timers::tick_with_idle_at(&server, base + 899_999, &|_| true);
    assert!(server.state.lock().unwrap().board_details["named-work"]
        .invitation
        .is_none());
    timers::tick_with_idle_at(&server, base + 900_000, &|_| true);
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["named-work"].owner, "old");
    let invitation = state.board_details["named-work"]
        .invitation
        .as_ref()
        .expect("deadline must prepare reassignment to an idle peer");
    assert_eq!(invitation.peer_id, "idle");
    assert_eq!(
        state.task_assignment("named-work").unwrap().name,
        state.name_occupancies[route.project_scope_id.as_str()]
            .values()
            .find(|occupancy| occupancy.worker_id == "old")
            .unwrap()
            .name
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn identity_return_deadline_preserves_owner_when_all_peers_are_busy() {
    let (server, root, route) = identity_return_fixture();
    let original_name = server
        .state
        .lock()
        .unwrap()
        .task_assignment("named-work")
        .unwrap()
        .name
        .clone();
    {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "old",
            AgentPresence::Offline,
            AgentActivity::Unknown,
        );
        identity_return_observation(
            &mut state,
            &route,
            "idle",
            AgentPresence::Online,
            AgentActivity::Busy,
        );
    }
    let server = std::sync::Arc::new(server);
    let base = 1_000_000_000;
    timers::tick_with_idle_at(&server, base, &|_| true);
    timers::tick_with_idle_at(&server, base + 900_000, &|_| true);
    let state = server.state.lock().unwrap();
    assert!(state.board_details["named-work"].invitation.is_none());
    assert_eq!(state.tasks["named-work"].owner, "old");
    assert_eq!(
        state.task_assignment("named-work").unwrap().name,
        original_name
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pure_automatic_scheduling_builds_invitation_only_after_fixed_deadline() {
    let (server, root, route) = identity_return_fixture();
    let base = 1_000_000_000;
    let name = {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "old",
            AgentPresence::Offline,
            AgentActivity::Unknown,
        );
        identity_return_observation(
            &mut state,
            &route,
            "idle",
            AgentPresence::Online,
            AgentActivity::Idle,
        );
        let name = state.task_assignment("named-work").unwrap().name.clone();
        state
            .apply_checked(&Event::NameReassignmentScheduled {
                project_scope: route.project_scope_id.as_str().into(),
                name: name.clone(),
                schedule: name_state::start_return_wait(None, &name, base),
            })
            .unwrap();
        name
    };
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    {
        let state = server.state.lock().unwrap();
        let before = scheduling_events(&server, &state, &control, base + 899_999).unwrap();
        assert!(before.events.is_empty());
    }
    {
        let mut state = server.state.lock().unwrap();
        let due = scheduling_events(&server, &state, &control, base + 900_000).unwrap();
        assert!(!due.events.is_empty());
        server
            .commit_locked_checked(&mut state, &due.events)
            .unwrap();
        let invitation = state.board_details["named-work"]
            .invitation
            .as_ref()
            .expect("deadline invitation");
        assert_eq!(invitation.peer_id, "idle");
        assert_eq!(
            invitation
                .automatic_source
                .as_ref()
                .map(|source| source.name.as_str()),
            Some(name.as_str())
        );
        assert_eq!(state.tasks["named-work"].owner, "old");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pure_automatic_scheduling_keeps_owner_when_no_online_idle_candidate_exists() {
    let (server, root, route) = identity_return_fixture();
    let base = 1_000_000_000;
    let name = {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "old",
            AgentPresence::Offline,
            AgentActivity::Unknown,
        );
        identity_return_observation(
            &mut state,
            &route,
            "idle",
            AgentPresence::Online,
            AgentActivity::Error,
        );
        let name = state.task_assignment("named-work").unwrap().name.clone();
        state
            .apply_checked(&Event::NameReassignmentScheduled {
                project_scope: route.project_scope_id.as_str().into(),
                name: name.clone(),
                schedule: name_state::start_return_wait(None, &name, base),
            })
            .unwrap();
        name
    };
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let state = server.state.lock().unwrap();
    let due = scheduling_events(&server, &state, &control, base + 900_000).unwrap();
    assert!(due.events.is_empty());
    assert_eq!(state.tasks["named-work"].owner, "old");
    assert_eq!(state.task_assignment("named-work").unwrap().name, name);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_retirement_builds_immediate_idle_invitation_without_waiting() {
    let (server, root, route) = identity_return_fixture();
    let base = 1_000_000_000;
    let name = {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "old",
            AgentPresence::Offline,
            AgentActivity::Unknown,
        );
        identity_return_observation(
            &mut state,
            &route,
            "idle",
            AgentPresence::Online,
            AgentActivity::Idle,
        );
        state.task_assignment("named-work").unwrap().name.clone()
    };
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    {
        let state = server.state.lock().unwrap();
        let prepared = identity_transition_events(
            &server,
            &state,
            &control,
            NameScheduleTransition::ExplicitRetirement {
                project_scope: route.project_scope_id.as_str().into(),
                name: name.clone(),
            },
            base,
        )
        .unwrap();
        assert!(!prepared.events.is_empty());
        drop(state);
        let mut state = server.state.lock().unwrap();
        server
            .commit_locked_checked(&mut state, &prepared.events)
            .unwrap();
        let invitation = state.board_details["named-work"]
            .invitation
            .as_ref()
            .expect("retirement invitation");
        assert_eq!(invitation.peer_id, "idle");
        assert!(state.tasks["named-work"].owner == "old");
        assert!(state
            .name_reassignment(route.project_scope_id.as_str(), &name)
            .is_some_and(|schedule| schedule.is_due(base)));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn valid_return_clears_wait_and_withdraws_the_automatic_invitation() {
    let (server, root, route) = identity_return_fixture();
    let base = 1_000_000_000;
    let name = {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "old",
            AgentPresence::Offline,
            AgentActivity::Unknown,
        );
        identity_return_observation(
            &mut state,
            &route,
            "idle",
            AgentPresence::Online,
            AgentActivity::Idle,
        );
        state.task_assignment("named-work").unwrap().name.clone()
    };
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    {
        let mut state = server.state.lock().unwrap();
        state
            .apply_checked(&Event::NameReassignmentScheduled {
                project_scope: route.project_scope_id.as_str().into(),
                name: name.clone(),
                schedule: name_state::start_return_wait(None, &name, base),
            })
            .unwrap();
        let prepared = scheduling_events(&server, &state, &control, base + 900_000).unwrap();
        server
            .commit_locked_checked(&mut state, &prepared.events)
            .unwrap();
        assert!(state.board_details["named-work"].invitation.is_some());
        let returned = identity_transition_events(
            &server,
            &state,
            &control,
            NameScheduleTransition::ValidReturn {
                project_scope: route.project_scope_id.as_str().into(),
                name: name.clone(),
            },
            base + 900_001,
        )
        .unwrap();
        server
            .commit_locked_checked(&mut state, &returned.events)
            .unwrap();
        assert!(state.board_details["named-work"].invitation.is_none());
        assert!(state
            .name_reassignment(route.project_scope_id.as_str(), &name)
            .is_none());
        assert_eq!(state.tasks["named-work"].owner, "old");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn changed_assignment_revision_rejects_the_automatic_invitation() {
    let (server, root, route) = identity_return_fixture();
    let base = 1_000_000_000;
    let name = {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "old",
            AgentPresence::Offline,
            AgentActivity::Unknown,
        );
        identity_return_observation(
            &mut state,
            &route,
            "idle",
            AgentPresence::Online,
            AgentActivity::Idle,
        );
        state.task_assignment("named-work").unwrap().name.clone()
    };
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    {
        let mut state = server.state.lock().unwrap();
        state
            .apply_checked(&Event::NameReassignmentScheduled {
                project_scope: route.project_scope_id.as_str().into(),
                name: name.clone(),
                schedule: name_state::start_return_wait(None, &name, base),
            })
            .unwrap();
        let prepared = scheduling_events(&server, &state, &control, base + 900_000).unwrap();
        server
            .commit_locked_checked(&mut state, &prepared.events)
            .unwrap();
        let mut assignment = state.task_assignment("named-work").unwrap().clone();
        assignment.assignment_revision += 1;
        assignment.updated_ms = base + 900_001;
        state
            .apply_checked(&Event::TaskAssignmentUpdated { assignment })
            .unwrap();
    }
    let expected_revision = server
        .state
        .lock()
        .unwrap()
        .board_details["named-work"]
        .revision;
    let response = board_respond(
        &server,
        "idle".into(),
        "token-idle".into(),
        "named-work".into(),
        expected_revision,
        true,
        None,
    );
    assert!(!response.ok, "changed source revision must reject acceptance");
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["named-work"].owner, "old");
    assert_eq!(state.task_assignment("named-work").unwrap().assignment_revision, 2);
    assert!(state.board_details["named-work"].invitation.is_none());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn automatic_scheduling_reserves_one_idle_peer_for_one_task_in_a_round() {
    let (server, root, route) = identity_return_fixture();
    let base = 1_000_000_000;
    let name = {
        let mut state = server.state.lock().unwrap();
        let original_assignment = state.task_assignment("named-work").unwrap().clone();
        let mut second_task = task("second-named-work", "old");
        second_task.status = "working".into();
        state
            .apply_checked(&Event::TaskCreated { task: second_task })
            .unwrap();
        let mut second_details = state.board_details["second-named-work"].clone();
        second_details.public_visibility = true;
        second_details.long_horizon = Some(crate::board::LongHorizonTaskSpec {
            content: "continue the second task".into(),
            interval_ms: 3_600_000,
            repeat_count: 100,
            ttl_seconds: 86_400,
        });
        state
            .apply_checked(&Event::BoardDetailsChanged {
                task_id: "second-named-work".into(),
                details: second_details,
            })
            .unwrap();
        state
            .apply_task_assignment(&TaskAssignment {
                task_id: "second-named-work".into(),
                ..original_assignment.clone()
            })
            .unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "old",
            AgentPresence::Offline,
            AgentActivity::Unknown,
        );
        identity_return_observation(
            &mut state,
            &route,
            "idle",
            AgentPresence::Online,
            AgentActivity::Idle,
        );
        let name = original_assignment.name;
        state
            .apply_checked(&Event::NameReassignmentScheduled {
                project_scope: route.project_scope_id.as_str().into(),
                name: name.clone(),
                schedule: name_state::start_return_wait(None, &name, base),
            })
            .unwrap();
        name
    };
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let state = server.state.lock().unwrap();
    let prepared = scheduling_events(&server, &state, &control, base + 900_000).unwrap();
    let invitations = prepared
        .events
        .iter()
        .filter(|event| matches!(event, Event::BoardDetailsChanged { details, .. } if details.invitation.is_some()))
        .count();
    assert_eq!(invitations, 1, "one idle peer has one invitation reservation");
    assert_eq!(
        state
            .name_reassignment(route.project_scope_id.as_str(), &name)
            .unwrap()
            .round_ms(),
        base
    );
    assert!(prepared.events.len() >= 5);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

/// Current committed session fixture. Public recovery is covered by the map
/// consumer tests; this fixture exercises the Board's business handoff owner.
fn refresh_board_fixture(server: &Server, route: &RouteScope, worker: &str, session: &str) {
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let key = control
        .key_for_worker(&route.project_scope_id, &route.app_scope_id, worker)
        .unwrap()
        .unwrap();
    let mut transport = control
        .slot(key)
        .unwrap()
        .active_binding()
        .unwrap()
        .transport
        .clone();
    transport.session_id = Some(session.into());
    if let Some(endpoint) = transport.tmux_endpoint.as_mut() {
        endpoint.codex_session_id = Some(session.into());
    }
    let response = crate::server::peer_tests::seed_committed_owner(
        server,
        worker,
        &format!("token-{worker}"),
        &transport,
        &server.root.to_string_lossy(),
        Some(route.app_scope_id.clone()),
    );
    assert!(response.ok, "{response:?}");
}

#[test]
fn board_reassign_accept_preserves_status_resources_and_refresh_isolation() {
    for status in [
        "working",
        "rework",
        "delivered",
        "accepted",
        "blocked",
        "waiting",
    ] {
        let (server, root, route) = reassign_fixture(status);
        let response = board_respond(
            &server,
            "current".into(),
            "token-current".into(),
            "named-work".into(),
            1,
            true,
            None,
        );
        assert!(response.ok, "{status}: {:?}", response.error);
        let state = server.state.lock().unwrap();
        assert_eq!(state.tasks["named-work"].status, status);
        assert_eq!(state.tasks["named-work"].owner, "current");
        assert_eq!(state.task_assignment("named-work").unwrap().name, "Bob");
        assert_eq!(
            state.task_assignment("named-work").unwrap().session_id,
            "session-current"
        );
        assert_eq!(
            state.worktree_bindings["resource"].owner_agent_id,
            "current"
        );
        assert_eq!(state.worktree_bindings["unrelated"].owner_agent_id, "old");
        assert_eq!(state.pending_merges["named-work"].owner, "current");
        assert_eq!(
            state.pending_merges["named-work"]
                .candidate_commit
                .as_deref(),
            Some("candidate")
        );
        assert_eq!(state.pending_merges["named-work"].requested_by, "reviewer");
        assert_eq!(state.pending_merges["named-work"].requested_ms, 10);
        assert!(board_resume_gate(&state, "current", "named-work").is_ok());
        assert!(board_resume_gate(&state, "old", "named-work").is_err());
        // A later refresh of the old name must not reclaim this assignment.
        drop(state);
        refresh_board_fixture(&server, &route, "old", "old-refreshed");
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.task_assignment("named-work").unwrap().session_id,
            "session-current"
        );
        drop(state);
        refresh_board_fixture(&server, &route, "current", "current-refreshed");
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.task_assignment("named-work").unwrap().session_id,
            "current-refreshed"
        );
        assert_eq!(state.tasks["named-work"].status, status);
        let replayed: State =
            serde_json::from_slice(&serde_json::to_vec(&*state).unwrap()).unwrap();
        assert_eq!(
            replayed.task_assignment("named-work"),
            state.task_assignment("named-work")
        );
        assert!(board_resume_gate(&replayed, "current", "named-work").is_ok());
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn board_reassign_foreign_resource_fails_without_partial_assignment() {
    let (server, root, _) = reassign_fixture("working");
    server
        .state
        .lock()
        .unwrap()
        .worktree_bindings
        .get_mut("resource")
        .unwrap()
        .owner_agent_id = "third-party".into();
    let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    let response = board_respond(
        &server,
        "current".into(),
        "token-current".into(),
        "named-work".into(),
        1,
        true,
        None,
    );
    assert!(!response.ok);
    assert!(response
        .error
        .unwrap()
        .contains("BOARD_RESOURCE_OWNER_CONFLICT"));
    assert_eq!(
        serde_json::to_value(&*server.state.lock().unwrap()).unwrap(),
        before
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_reassign_closed_rejects_accept() {
    for status in ["closed", "cancelled"] {
        let (server, root, _) = reassign_fixture(status);
        let response = board_respond(
            &server,
            "current".into(),
            "token-current".into(),
            "named-work".into(),
            1,
            true,
            None,
        );
        assert!(!response.ok);
        assert_eq!(
            server.state.lock().unwrap().tasks["named-work"].status,
            status
        );
        // The withdrawal authorization itself is exercised by the CLI test.
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn pending_reassignment_invitation_does_not_transfer_task_ownership() {
    let (server, root, _) = reassign_fixture("working");
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.tasks["named-work"].owner, "old");
        assert_eq!(state.task_assignment("named-work").unwrap().name, "Alice");
        let invitation = state.board_details["named-work"]
            .invitation
            .as_ref()
            .expect("pending invitation");
        assert_eq!(invitation.peer_id, "current");
    }

    let response = board_respond(
        &server,
        "current".into(),
        "token-current".into(),
        "named-work".into(),
        1,
        true,
        None,
    );
    assert!(response.ok, "{:?}", response.error);
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["named-work"].owner, "current");
    assert_eq!(
        state.task_assignment("named-work").unwrap().session_id,
        "session-current"
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pending_reassignment_invitation_does_not_move_the_task_subscription() {
    let (server, root, route) = board_name_fixture();
    let mut state = server.state.lock().unwrap();
    let binding = authoritative_send_binding(&state, &route, "current")
        .unwrap()
        .clone();
    let details = state.board_details.get_mut("named-work").unwrap();
    details.long_horizon = Some(crate::board::LongHorizonTaskSpec {
        content: "continue after reassignment".into(),
        interval_ms: 60_000,
        repeat_count: 8,
        ttl_seconds: 86_400,
    });
    drop(state);

    let now = now_ms();
    server.commit(&[Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "task-subscription".into(),
            worker_id: "old".into(),
            event: "deadline".into(),
            subject: Some("goal:task:named-work".into()),
            target: "old-endpoint".into(),
            method: "tmux".into(),
            trigger_ms: Some(now + 60_000),
            trigger_times_ms: Vec::new(),
            interval_ms: Some(60_000),
            repeat_count: 8,
            fired_count: 2,
            expires_ms: now + 86_400_000,
            status: "armed".into(),
            created_ms: now,
            updated_ms: now,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: true,
            goal_content: Some(state::GoalScheduleBinding {
                source: Some(state::GoalScheduleContent::BoardTask {
                    task_id: "named-work".into(),
                }),
                ttl_seconds: 86_400,
            }),
        },
    }]);

    let mut details = server.state.lock().unwrap().board_details["named-work"].clone();
    details.invitation = Some(crate::board::BoardInvitation {
        peer_id: "current".into(),
        binding_id: binding.binding_id.as_str().into(),
        message_id: "pending-task-invitation".into(),
        created_ms: now,
        original_status: Some("working".into()),
        original_owner: Some("old".into()),
        automatic_source: None,
    });
    details.revision += 1;
    server.commit(&[Event::BoardDetailsChanged {
        task_id: "named-work".into(),
        details,
    }]);

    let state = server.state.lock().unwrap();
    let subscription = &state.notification_subscriptions["task-subscription"];
    assert_eq!(subscription.worker_id, "old");
    assert_eq!(subscription.fired_count, 2);
    assert_eq!(subscription.repeat_count, 8);
    assert_eq!(subscription.interval_ms, Some(60_000));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn identity_return_before_accept_keeps_original_task_and_rejects_late_accept() {
    let (server, root, route) = reassign_fixture("working");
    {
        let mut state = server.state.lock().unwrap();
        state
            .board_details
            .get_mut("named-work")
            .unwrap()
            .invitation
            .as_mut()
            .expect("pending automatic invitation")
            .automatic_source = Some(crate::board::BoardInvitationSource {
            project_scope: route.project_scope_id.as_str().into(),
            name: "Alice".into(),
            assignment_revision: 1,
            schedule_round_ms: 0,
        });
        board_name_presence(
            &mut state,
            &route,
            "old",
            "session-old",
            AgentPresence::Online,
        );
    }

    let response = board_respond(
        &server,
        "current".into(),
        "token-current".into(),
        "named-work".into(),
        1,
        true,
        None,
    );
    assert!(!response.ok, "late accept must be rejected: {response:?}");
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["named-work"].owner, "old");
    assert_eq!(state.task_assignment("named-work").unwrap().name, "Alice");
    assert!(state.board_details["named-work"].invitation.is_none());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_reassign_exclusive_resource_conflict_is_not_a_peer_capacity_limit() {
    let (server, root, _) = reassign_fixture("working");
    {
        let mut state = server.state.lock().unwrap();
        state.tasks.get_mut("named-work").unwrap().feature_id = Some("exclusive-feature".into());
        let mut other = task("other", "current");
        other.feature_id = Some("exclusive-feature".into());
        state
            .apply_checked(&Event::TaskCreated { task: other })
            .unwrap();
    }
    let response = board_respond(
        &server,
        "current".into(),
        "token-current".into(),
        "named-work".into(),
        1,
        true,
        None,
    );
    assert!(!response.ok);
    assert!(response.error.unwrap().contains("TASK_RESOURCE_CONFLICT"));
    assert_eq!(
        server.state.lock().unwrap().tasks["named-work"].owner,
        "old"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_release_starts_fixed_wait_even_when_another_session_is_online() {
    let (server, root, route) = identity_return_fixture();
    let base = 1_000_000_000;
    let name = {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "idle",
            AgentPresence::Online,
            AgentActivity::Idle,
        );
        state.task_assignment("named-work").unwrap().name.clone()
    };
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let binding = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "old")
        .unwrap()
        .unwrap()
        .active_binding()
        .unwrap()
        .binding
        .clone();
    let stale_release = identity_transition_events(
        &server,
        &server.state.lock().unwrap(),
        &control,
        NameScheduleTransition::SourceReleased {
            project_scope: route.project_scope_id.as_str().into(),
            name: name.clone(),
        },
        base,
    )
    .unwrap();
    assert!(stale_release.events.is_empty());
    store.retire_binding(&binding).unwrap();
    let control = store.load().unwrap();
    {
        let mut state = server.state.lock().unwrap();
        let prepared = identity_transition_events(
            &server,
            &state,
            &control,
            NameScheduleTransition::SourceReleased {
                project_scope: route.project_scope_id.as_str().into(),
                name: name.clone(),
            },
            base,
        )
        .unwrap();
        assert_eq!(prepared.events.len(), 1);
        server
            .commit_locked_checked(&mut state, &prepared.events)
            .unwrap();
        assert_eq!(state.name_reassignment(route.project_scope_id.as_str(), &name), Some(
            &name_state::NameReassignmentSchedule::WaitingReturn {
                lost_relation_ref: name.clone(),
                started_ms: base,
                deadline_ms: base + name_state::IDENTITY_RETURN_WAIT_MS,
            }
        ));
        assert_eq!(state.tasks["named-work"].owner, "old");
    }
    let before_deadline = scheduling_events(&server, &server.state.lock().unwrap(), &control, base + name_state::IDENTITY_RETURN_WAIT_MS - 1).unwrap();
    assert!(before_deadline.events.is_empty());
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .task_assignment("named-work")
            .unwrap()
            .name,
        name
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fixed_wait_survives_repeated_loss_and_state_reload() {
    let (server, root, route) = identity_return_fixture();
    let server = std::sync::Arc::new(server);
    let base = 1_000_000_000;
    {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "old",
            AgentPresence::Offline,
            AgentActivity::Unknown,
        );
        identity_return_observation(
            &mut state,
            &route,
            "idle",
            AgentPresence::Online,
            AgentActivity::Idle,
        );
    }
    timers::tick_with_idle_at(&server, base, &|_| true);
    let persisted = {
        let state = server.state.lock().unwrap();
        serde_json::from_value::<State>(serde_json::to_value(&*state).unwrap()).unwrap()
    };
    let deadline = {
        let state = server.state.lock().unwrap();
        state
            .name_reassignment(route.project_scope_id.as_str(), &state.task_assignment("named-work").unwrap().name)
            .and_then(|schedule| match schedule {
                name_state::NameReassignmentSchedule::WaitingReturn { deadline_ms, .. } => Some(*deadline_ms),
                _ => None,
            })
            .expect("first confirmed loss starts a wait")
    };
    {
        let mut state = server.state.lock().unwrap();
        *state = persisted;
    }
    timers::tick_with_idle_at(&server, base + 1_000, &|_| true);
    let observed_deadline = {
        let state = server.state.lock().unwrap();
        let name = state
            .task_assignment("named-work")
            .unwrap()
            .name
            .clone();
        state
            .name_reassignment(route.project_scope_id.as_str(), &name)
            .and_then(|schedule| match schedule {
                name_state::NameReassignmentSchedule::WaitingReturn { deadline_ms, .. } => {
                    Some(*deadline_ms)
                }
                _ => None,
            })
    };
    assert_eq!(
        observed_deadline,
        Some(deadline),
        "repeated loss and state reload must keep the first deadline"
    );
    assert!(server.state.lock().unwrap().board_details["named-work"].invitation.is_none());
    timers::tick_with_idle_at(&server, base + 899_999, &|_| true);
    assert!(server.state.lock().unwrap().board_details["named-work"].invitation.is_none());
    timers::tick_with_idle_at(&server, base + 900_000, &|_| true);
    assert!(server.state.lock().unwrap().board_details["named-work"].invitation.is_some());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_retirement_from_all_busy_remains_eligible_until_idle_capacity_exists() {
    let (server, root, route) = identity_return_fixture();
    let base = 1_000_000_000;
    let name = {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "old",
            AgentPresence::Offline,
            AgentActivity::Unknown,
        );
        identity_return_observation(
            &mut state,
            &route,
            "idle",
            AgentPresence::Online,
            AgentActivity::Busy,
        );
        state.task_assignment("named-work").unwrap().name.clone()
    };
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    {
        let mut state = server.state.lock().unwrap();
        let prepared = identity_transition_events(
            &server,
            &state,
            &control,
            NameScheduleTransition::ExplicitRetirement {
                project_scope: route.project_scope_id.as_str().into(),
                name: name.clone(),
            },
            base,
        )
        .unwrap();
        server
            .commit_locked_checked(&mut state, &prepared.events)
            .unwrap();
        assert!(state.board_details["named-work"].invitation.is_none());
        assert_eq!(state.tasks["named-work"].owner, "old");
        assert!(state
            .name_reassignment(route.project_scope_id.as_str(), &name)
            .is_some_and(|schedule| schedule.is_due(base)));
    }
    {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(&mut state, &route, "idle", AgentPresence::Online, AgentActivity::Idle);
    }
    let mut state = server.state.lock().unwrap();
    let prepared = scheduling_events(&server, &state, &control, base + 1).unwrap();
    server
        .commit_locked_checked(&mut state, &prepared.events)
        .unwrap();
    assert!(state.board_details["named-work"].invitation.is_some());
    assert_eq!(state.board_details["named-work"].invitation.as_ref().unwrap().peer_id, "idle");
    assert_eq!(state.tasks["named-work"].owner, "old");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_retirement_replaces_t2_wait_and_supersedes_its_invitation() {
    let (server, root, route) = identity_return_fixture();
    let base = 1_000_000_000;
    let name = server
        .state
        .lock()
        .unwrap()
        .task_assignment("named-work")
        .unwrap()
        .name
        .clone();
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "old",
            AgentPresence::Offline,
            AgentActivity::Unknown,
        );
        identity_return_observation(
            &mut state,
            &route,
            "idle",
            AgentPresence::Online,
            AgentActivity::Idle,
        );
        state
            .apply_checked(&Event::NameReassignmentScheduled {
                project_scope: route.project_scope_id.as_str().into(),
                name: name.clone(),
                schedule: name_state::start_return_wait(None, &name, base),
            })
            .unwrap();
        let prepared = scheduling_events(&server, &state, &control, base + name_state::IDENTITY_RETURN_WAIT_MS)
            .unwrap();
        server
            .commit_locked_checked(&mut state, &prepared.events)
            .unwrap();
    }
    let t2_invitation = server
        .state
        .lock()
        .unwrap()
        .board_details["named-work"]
        .invitation
        .clone()
        .expect("t2 deadline invitation");
    {
        let mut state = server.state.lock().unwrap();
        let prepared = identity_transition_events(
            &server,
            &state,
            &control,
            NameScheduleTransition::ExplicitRetirement {
                project_scope: route.project_scope_id.as_str().into(),
                name: name.clone(),
            },
            base + name_state::IDENTITY_RETURN_WAIT_MS + 1,
        )
        .unwrap();
        server
            .commit_locked_checked(&mut state, &prepared.events)
            .unwrap();
    }
    let state = server.state.lock().unwrap();
    let e2_invitation = state.board_details["named-work"]
        .invitation
        .as_ref()
        .expect("e2 must retain one automatic invitation");
    assert_ne!(e2_invitation.message_id, t2_invitation.message_id);
    assert_eq!(e2_invitation.peer_id, "idle");
    assert_eq!(e2_invitation.automatic_source.as_ref().unwrap().schedule_round_ms, base + name_state::IDENTITY_RETURN_WAIT_MS + 1);
    assert!(state.name_reassignment(route.project_scope_id.as_str(), &name).is_some_and(|schedule| schedule.is_due(base + name_state::IDENTITY_RETURN_WAIT_MS + 1)));
    assert_eq!(state.tasks["named-work"].owner, "old");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn automatic_invitation_survives_reflow_progress_without_owner_change() {
    let (server, root, route) = reassign_fixture("working");
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let now = 1_000_000_000;
    {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "old",
            AgentPresence::Offline,
            AgentActivity::Unknown,
        );
        identity_return_observation(
            &mut state,
            &route,
            "current",
            AgentPresence::Online,
            AgentActivity::Idle,
        );
        let assignment = state.task_assignment("named-work").unwrap().clone();
        state
            .apply_checked(&Event::NameReassignmentScheduled {
                project_scope: route.project_scope_id.as_str().into(),
                name: assignment.name.clone(),
                schedule: name_state::eligible_after_retirement(&assignment.name, now),
            })
            .unwrap();
        state
            .board_details
            .get_mut("named-work")
            .unwrap()
            .invitation
            .as_mut()
            .unwrap()
            .automatic_source = Some(crate::board::BoardInvitationSource {
            project_scope: route.project_scope_id.as_str().into(),
            name: assignment.name.clone(),
            assignment_revision: assignment.assignment_revision,
            schedule_round_ms: now,
        });
        name_stewardship::tick_locked(&server, &mut state, &control, now + 1).unwrap();
        let progressed = state.task_assignment("named-work").unwrap();
        assert_eq!(progressed.name, assignment.name);
        assert_eq!(progressed.session_id, assignment.session_id);
        assert_eq!(progressed.assignment_revision, assignment.assignment_revision);
        assert!(progressed.reflow.is_some(), "the timer must advance reflow bookkeeping");
    }
    let response = board_respond(
        &server,
        "current".into(),
        "token-current".into(),
        "named-work".into(),
        1,
        true,
        None,
    );
    assert!(
        response.ok,
        "reflow bookkeeping must preserve an invitation whose source owner is unchanged: {response:?}"
    );
    assert_eq!(server.state.lock().unwrap().tasks["named-work"].owner, "current");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn accept_first_keeps_new_owner_after_source_return() {
    let (server, root, route) = reassign_fixture("working");
    let base = 1_000_000_000;
    let name = server
        .state
        .lock()
        .unwrap()
        .task_assignment("named-work")
        .unwrap()
        .name
        .clone();
    {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "current",
            AgentPresence::Online,
            AgentActivity::Idle,
        );
        state
            .apply_checked(&Event::NameReassignmentScheduled {
                project_scope: route.project_scope_id.as_str().into(),
                name: name.clone(),
                schedule: name_state::start_return_wait(None, &name, base),
            })
            .unwrap();
        state
            .board_details
            .get_mut("named-work")
            .unwrap()
            .invitation
            .as_mut()
            .unwrap()
            .automatic_source = Some(crate::board::BoardInvitationSource {
            project_scope: route.project_scope_id.as_str().into(),
            name: name.clone(),
            assignment_revision: 1,
            schedule_round_ms: base,
        });
    }
    let response = board_respond(
        &server,
        "current".into(),
        "token-current".into(),
        "named-work".into(),
        1,
        true,
        None,
    );
    assert!(response.ok, "current invitation must remain valid: {response:?}");
    let mut state = server.state.lock().unwrap();
    assert_eq!(state.tasks["named-work"].owner, "current");
    assert_eq!(state.task_assignment("named-work").unwrap().name, "Bob");
    assert!(state.board_details["named-work"].invitation.is_none());
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let returned = identity_transition_events(
        &server,
        &state,
        &control,
        NameScheduleTransition::ValidReturn {
            project_scope: route.project_scope_id.as_str().into(),
            name: name.clone(),
        },
        base + 1,
    )
    .unwrap();
    server
        .commit_locked_checked(&mut state, &returned.events)
        .unwrap();
    assert!(state.name_reassignment(route.project_scope_id.as_str(), &name).is_none());
    assert_eq!(state.tasks["named-work"].owner, "current");
    assert_eq!(state.task_assignment("named-work").unwrap().name, "Bob");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_map_board_accept_rejects_when_the_current_binding_differs() {
    let (server, root, route) = reassign_fixture("working");
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let runtime = current_map_runtime(&server, &route, "current");
    let control = store.load().unwrap();
    let key = control
        .key_for_worker(&route.project_scope_id, &route.app_scope_id, "current")
        .unwrap()
        .unwrap()
        .clone();
    let slot = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "current")
        .unwrap()
        .unwrap();
    let mut value = slot.active_binding().unwrap().clone();
    value.binding.binding_id = crate::identity::BindingId::new("binding-current-new").unwrap();
    value.binding.session_id =
        Some(crate::identity::SessionId::new("session-current-new").unwrap());
    store
        .commit(binding_store::ControlChange::Commit {
            key,
            value,
            expected_generation: Some(slot.generation()),
        })
        .unwrap();

    let server = Arc::new(server);
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let context = ProjectContext::for_registered_route(&root, &runtime).unwrap();
    manager.install_runtime(
        &ProjectRuntimeManager::route_key(&context),
        server.clone(),
        None,
    );
    let (_, response) = manager.dispatch_sync(
        Some(context),
        Req::Board {
            worker_id: "current".into(),
            token: "token-current".into(),
            command: crate::board::BoardCommand::Respond {
                id: "named-work".into(),
                accept: true,
                decline: false,
                expected_revision: 1,
                reason: None,
            },
        },
    );
    assert_eq!(
        response.error.as_deref(),
        Some(
            "BOARD_STALE_INVITATION: recipient rebound; master must withdraw and issue a new invitation"
        ),
        "a changed current binding must reject accept: {response:?}"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["named-work"].owner, "old");
    assert!(state.board_details["named-work"].invitation.is_some());
    drop(state);
    let control = store.load().unwrap();
    let current = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "current")
        .unwrap()
        .unwrap();
    assert_eq!(
        current.active_binding().unwrap().binding.binding_id.as_str(),
        "binding-current-new"
    );
    drop(current);
    drop(control);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_map_board_accept_rejects_source_handoff_before_state_effect() {
    let (server, root, route) = reassign_fixture("working");
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let key = control
        .slots()
        .find(|slot| {
            slot.identity_key.name == "Alice" && slot.active_binding().is_some()
        })
        .unwrap()
        .identity_key
        .clone();
    let slot = control.slot(&key).unwrap();
    let mut successor = slot.active_binding().unwrap().clone();
    successor.binding.session_id =
        Some(crate::identity::SessionId::new("session-Alice-successor").unwrap());
    store
        .commit(binding_store::ControlChange::Commit {
            key,
            value: successor,
            expected_generation: Some(slot.generation()),
        })
        .unwrap();

    let round_ms = 1_000_000_000;
    {
        let mut state = server.state.lock().unwrap();
        state
            .apply_checked(&Event::NameReassignmentScheduled {
                project_scope: route.project_scope_id.as_str().into(),
                name: "Alice".into(),
                schedule: name_state::start_return_wait(None, "Alice", round_ms),
            })
            .unwrap();
        let invitation = state
            .board_details
            .get_mut("named-work")
            .expect("named work details")
            .invitation
            .as_mut()
            .expect("pending automatic invitation");
        invitation.automatic_source = Some(crate::board::BoardInvitationSource {
            project_scope: route.project_scope_id.as_str().into(),
            name: "Alice".into(),
            assignment_revision: 1,
            schedule_round_ms: round_ms,
        });
    }

    let server = Arc::new(server);
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let context = current_map_context(&server, &root, &route, "current");
    manager.install_runtime(
        &ProjectRuntimeManager::route_key(&context),
        server.clone(),
        None,
    );
    let (_, response) = manager.dispatch_sync(
        Some(context),
        Req::Board {
            worker_id: "current".into(),
            token: "token-current".into(),
            command: crate::board::BoardCommand::Respond {
                id: "named-work".into(),
                accept: true,
                decline: false,
                expected_revision: 1,
                reason: None,
            },
        },
    );
    assert!(
        !response.ok,
        "a changed source holder must reject accept before the State handoff is committed: {response:?}"
    );
    assert_eq!(
        response
            .error
            .as_deref()
            .unwrap_or_default(),
        "BOARD_STALE_AUTOMATIC_INVITATION"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["named-work"].owner, "old");
    assert_eq!(
        state.task_assignment("named-work").unwrap().session_id,
        "session-old"
    );
    assert!(state.board_details["named-work"].invitation.is_none());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_map_board_accept_waits_for_return_first_map_publication() {
    let (server, root, route) = reassign_fixture("working");
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let key = control
        .slots()
        .find(|slot| {
            slot.identity_key.name == "Alice" && slot.active_binding().is_some()
        })
        .unwrap()
        .identity_key
        .clone();
    let slot = control.slot(&key).unwrap();
    let mut successor = slot.active_binding().unwrap().clone();
    successor.binding.session_id =
        Some(crate::identity::SessionId::new("session-Alice-successor").unwrap());

    let round_ms = 1_000_000_000;
    {
        let mut state = server.state.lock().unwrap();
        state
            .apply_checked(&Event::NameReassignmentScheduled {
                project_scope: route.project_scope_id.as_str().into(),
                name: "Alice".into(),
                schedule: name_state::start_return_wait(None, "Alice", round_ms),
            })
            .unwrap();
        let invitation = state
            .board_details
            .get_mut("named-work")
            .expect("named work details")
            .invitation
            .as_mut()
            .expect("pending automatic invitation");
        invitation.automatic_source = Some(crate::board::BoardInvitationSource {
            project_scope: route.project_scope_id.as_str().into(),
            name: "Alice".into(),
            assignment_revision: 1,
            schedule_round_ms: round_ms,
        });
    }

    let server = Arc::new(server);
    let manager = Arc::new(
        ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap(),
    );
    let context = current_map_context(&server, &root, &route, "current");
    manager.install_runtime(
        &ProjectRuntimeManager::route_key(&context),
        server.clone(),
        None,
    );
    let register_gate = manager.register_gate.lock().unwrap();
    let dispatching = manager.clone();
    let (result_tx, result_rx) = mpsc::channel();
    let board_accept = std::thread::spawn(move || {
        let (_, response) = dispatching.dispatch_sync(
            Some(context),
            Req::Board {
                worker_id: "current".into(),
                token: "token-current".into(),
                command: crate::board::BoardCommand::Respond {
                    id: "named-work".into(),
                    accept: true,
                    decline: false,
                    expected_revision: 1,
                    reason: None,
                },
            },
        );
        result_tx.send(response).unwrap();
    });
    assert!(
        result_rx.try_recv().is_err(),
        "late accept must serialize with the return-first identity publication"
    );
    store
        .commit(binding_store::ControlChange::Commit {
            key,
            value: successor,
            expected_generation: Some(slot.generation()),
        })
        .unwrap();
    drop(register_gate);

    let response = result_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("late accept must finish after the return-first map publication");
    assert_eq!(
        response.error.as_deref(),
        Some("BOARD_STALE_AUTOMATIC_INVITATION")
    );
    board_accept.join().unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["named-work"].owner, "old");
    assert_eq!(
        state.task_assignment("named-work").unwrap().session_id,
        "session-old"
    );
    assert!(state.board_details["named-work"].invitation.is_none());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn current_map_board_dispatch_waits_for_the_manager_register_gate() {
    let (server, root, route) = identity_return_fixture();
    {
        let mut state = server.state.lock().unwrap();
        identity_return_observation(
            &mut state,
            &route,
            "idle",
            AgentPresence::Online,
            AgentActivity::Idle,
        );
    }
    let server = Arc::new(server);
    let manager = Arc::new(
        ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap(),
    );
    let context = current_map_context(&server, &root, &route, "master");
    manager.install_runtime(
        &ProjectRuntimeManager::route_key(&context),
        server.clone(),
        None,
    );
    let register_gate = manager.register_gate.lock().unwrap();
    let dispatching = manager.clone();
    let (result_tx, result_rx) = mpsc::channel();
    let board_dispatch = std::thread::spawn(move || {
        let (_, response) = dispatching.dispatch_sync(
            Some(context),
            Req::Board {
                worker_id: "master".into(),
                token: "token-master".into(),
                command: crate::board::BoardCommand::Dispatch {
                    id: "named-work".into(),
                    expected_revision: 1,
                    long_horizon: crate::board::LongHorizonArgs::default(),
                },
            },
        );
        result_tx.send(response).unwrap();
    });
    assert!(
        result_rx.try_recv().is_err(),
        "public Board dispatch must serialize with register_gate before loading the current map"
    );
    drop(register_gate);
    let response = result_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("board dispatch must finish after register_gate is released");
    assert!(response.ok, "{response:?}");
    board_dispatch.join().unwrap();
    let state = server.state.lock().unwrap();
    assert!(state.board_details["named-work"].invitation.is_some());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

fn current_map_context(
    server: &Server,
    root: &std::path::Path,
    route: &RouteScope,
    worker_id: &str,
) -> ProjectContext {
    ProjectContext::for_registered_route(
        root,
        &current_map_runtime(server, route, worker_id),
    )
    .unwrap()
}

fn current_map_runtime(
    server: &Server,
    route: &RouteScope,
    worker_id: &str,
) -> crate::identity::RuntimeIdentity {
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let binding = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, worker_id)
        .unwrap()
        .unwrap()
        .active_binding()
        .unwrap()
        .binding
        .clone();
    crate::identity::RuntimeIdentity {
        agent_id: binding.agent_id,
        runtime_id: binding.runtime_id,
        appserver_id: binding.app_scope_id,
        binding_id: binding.binding_id,
        session_id: binding.session_id,
        native_thread_id: binding.native_thread_id,
    }
}

/// The board projects an unavailable owner as empty and `needs_reassignment`.
/// The current master must be able to repair the task: reassign it by
/// invitation or put it into a durable typed wait, without losing status or
/// evidence.
fn ownerless_fixture(status: &str) -> (Server, std::path::PathBuf, RouteScope) {
    let (server, root, route) = board_name_fixture();
    let response = crate::server::peer_tests::register_synthetic_peer(&server, "master", "master");
    assert!(response.ok, "{:?}", response.error);
    crate::server::peer_tests::promote_master(&server, "master", "user-approved");
    {
        let mut state = server.state.lock().unwrap();
        // Drop the carrier assignment and point the persisted owner at a name
        // with no current binding, so the board projects owner empty.
        state.task_assignments.remove("named-work");
        {
            let task = state.tasks.get_mut("named-work").unwrap();
            task.owner = "ghost".into();
            task.status = status.into();
            task.next_step = Some("preserved next step".into());
        }
        {
            let details = state.board_details.get_mut("named-work").unwrap();
            details.invitation = None;
            details.work_policy.reviewer_name = Some("Grace".into());
            details.long_horizon = Some(crate::board::LongHorizonTaskSpec {
                content: "continue ownerless repair".into(),
                interval_ms: 3_600_000,
                repeat_count: 100,
                ttl_seconds: 86_400,
            });
        }
        state.task_lifecycle.insert(
            "named-work".into(),
            state::TaskLifecycleRecord {
                required_reviewer_name: Some("Grace".into()),
                delivery_kind: state::TaskDeliveryKind::Code,
                delivery_evidence: Some("kept evidence".into()),
                delivered_ms: Some(1),
                review_subject_revision: 1,
                ..Default::default()
            },
        );
        state
            .apply_checked(&Event::TaskCreated {
                task: task("blocking-resource", "current"),
            })
            .unwrap();
        {
            let blocking = state.tasks.get_mut("blocking-resource").unwrap();
            blocking.status = "working".into();
            blocking.feature_id = Some("shared".into());
        }
        state
            .board_details
            .get_mut("blocking-resource")
            .unwrap()
            .public_visibility = true;
    }
    (server, root, route)
}

fn ownerless_board_view(server: &Server) -> serde_json::Value {
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let state = server.state.lock().unwrap();
    let route = server_route_scope(server, &state).unwrap();
    let view = board_task_view_with_master(
        &state,
        &control,
        &state.tasks["named-work"],
        None,
        None,
        route.as_ref(),
    )
    .unwrap()
    .unwrap();
    serde_json::to_value(view).unwrap()
}

#[test]
fn master_can_reassign_ownerless_task_and_preserve_status_and_evidence() {
    let (server, root, _) = ownerless_fixture("rework");
    let view = ownerless_board_view(&server);
    assert_eq!(view["owner"], "");
    assert_eq!(view["needs_reassignment"], true);
    assert_eq!(view["status"], "rework");

    let invited = board_invite(
        &server,
        "master".into(),
        "token-master".into(),
        "named-work".into(),
        "current".into(),
        1,
    );
    assert!(invited.ok, "{:?}", invited.error);
    assert_eq!(invited.data["task"]["owner"], "");
    assert_eq!(invited.data["task"]["status"], "rework");

    let accepted = board_respond(
        &server,
        "current".into(),
        "token-current".into(),
        "named-work".into(),
        2,
        true,
        None,
    );
    assert!(accepted.ok, "{:?}", accepted.error);
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["named-work"].owner, "current");
    assert_eq!(state.tasks["named-work"].status, "rework");
    assert_eq!(
        state.task_lifecycle["named-work"]
            .delivery_evidence
            .as_deref(),
        Some("kept evidence")
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_can_put_ownerless_task_into_durable_wait_with_owner_preserved() {
    let (server, root, _) = ownerless_fixture("rework");
    {
        let mut state = server.state.lock().unwrap();
        state.tasks.get_mut("named-work").unwrap().feature_id = Some("shared".into());
    }
    let response = handle_task_wait(
        &server,
        "master".into(),
        "token-master".into(),
        "named-work".into(),
        "blocking-resource".into(),
    );
    assert!(response.ok, "{:?}", response.error);
    {
        let state = server.state.lock().unwrap();
        let task = &state.tasks["named-work"];
        assert_eq!(task.status, "waiting");
        assert!(task
            .wait
            .as_ref()
            .is_some_and(|wait| wait.waiting_for == "blocking-resource"));
        // The persisted owner row stays intact; the wait is a repair, not a loss.
        assert_eq!(task.owner, "ghost");
        assert_eq!(
            task.next_step.as_deref(),
            Some("WAITING_FOR=blocking-resource")
        );
    }
    // The board no longer exposes owner empty together with wait empty.
    let view = ownerless_board_view(&server);
    assert_eq!(view["owner"], "");
    assert_eq!(view["blocking_task"], "blocking-resource");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn non_master_cannot_repair_ownerless_task() {
    let (server, root, _) = ownerless_fixture("rework");
    let response = handle_task_wait(
        &server,
        "current".into(),
        "token-current".into(),
        "named-work".into(),
        "blocking-resource".into(),
    );
    assert!(
        !response.ok,
        "a non-master peer must not repair an ownerless task"
    );
    std::fs::remove_dir_all(root).unwrap();
}
