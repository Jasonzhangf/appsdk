use super::*;
use crate::server::peer_tests::{register_synthetic_peer, test_server};

#[test]
fn confirmed_loss_starts_one_fixed_deadline_and_repeats_keep_it() {
    let (server, root, route) = fixture();
    let scope = route.project_scope_id.as_str().to_owned();
    let first = name_state::start_return_wait(None, "Alice:old:gen-1", 1_000);
    assert_eq!(
        first,
        name_state::NameReassignmentSchedule::WaitingReturn {
            lost_relation_ref: "Alice:old:gen-1".into(),
            started_ms: 1_000,
            deadline_ms: 1_000 + name_state::IDENTITY_RETURN_WAIT_MS,
        }
    );
    let deadline = 1_000 + name_state::IDENTITY_RETURN_WAIT_MS;
    assert!(!first.is_due(deadline - 1));
    assert!(first.is_due(deadline));

    let mut state = server.state.lock().unwrap();
    state
        .apply_checked(&Event::NameReassignmentScheduled {
            project_scope: scope.clone(),
            name: "Alice".into(),
            schedule: first.clone(),
        })
        .unwrap();
    let repeated = name_state::start_return_wait(
        state.name_reassignment(&scope, "Alice"),
        "Alice:old:gen-2",
        deadline + 5_000,
    );
    assert_eq!(
        repeated, first,
        "a repeated loss observation must keep the first deadline"
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_retirement_is_immediately_eligible_and_replaces_a_wait() {
    let (server, root, route) = fixture();
    let scope = route.project_scope_id.as_str().to_owned();
    let retired = name_state::eligible_after_retirement("Alice:retired:gen-1", 2_000);
    assert!(retired.is_due(2_000), "retirement must not wait 900000ms");

    let mut state = server.state.lock().unwrap();
    state
        .apply_checked(&Event::NameReassignmentScheduled {
            project_scope: scope.clone(),
            name: "Alice".into(),
            schedule: name_state::start_return_wait(None, "Alice:old:gen-1", 1_000),
        })
        .unwrap();
    state
        .apply_checked(&Event::NameReassignmentScheduled {
            project_scope: scope.clone(),
            name: "Alice".into(),
            schedule: retired.clone(),
        })
        .unwrap();
    assert_eq!(state.name_reassignment(&scope, "Alice"), Some(&retired));
    assert!(state
        .name_reassignment(&scope, "Alice")
        .unwrap()
        .is_due(2_000));

    state
        .apply_checked(&Event::NameReassignmentCleared {
            project_scope: scope.clone(),
            name: "Alice".into(),
        })
        .unwrap();
    assert!(state.name_reassignment(&scope, "Alice").is_none());
    assert!(state.name_reassignments.is_empty());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_state_without_reassignment_field_decodes_as_empty() {
    let state: State = serde_json::from_str("{}").expect("legacy state decodes");
    assert!(state.name_reassignments.is_empty());
}

#[test]
fn absence_of_a_probe_fact_does_not_fabricate_periodic_unknown() {
    let (server, root, _) = fixture();
    let before = server.state.lock().unwrap().task_assignments.clone();
    let notifications = std::sync::atomic::AtomicUsize::new(0);
    super::tick_with_notifier(&server, 100, &|_, _, _, _| {
        notifications.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    })
    .unwrap();
    let state = server.state.lock().unwrap();
    for id in ["one", "two", "closed"] {
        assert!(state.task_assignment(id).unwrap().reflow.is_none());
        assert_eq!(
            state.task_assignment(id).unwrap().assignment_revision,
            before[id].assignment_revision
        );
    }
    assert!(state.msgs.is_empty());
    assert!(state.agent_presence.is_empty());
    assert!(state.agent_activity.is_empty());
    assert_eq!(notifications.load(std::sync::atomic::Ordering::SeqCst), 0);
    drop(state);
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
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let current = control
        .active_values()
        .find(|value| value.binding.agent_id.as_str() == "old")
        .unwrap();
    let source = control
        .key_for_worker(
            &current.binding.project_scope,
            &current.binding.app_scope_id,
            "old",
        )
        .unwrap()
        .unwrap();
    assert_eq!(source.name, "Alice");
    let session = current.binding.session_id.as_ref().unwrap().to_string();
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
                session_id: session.clone(),
                occupancy_revision: 1,
                updated_ms: 1,
                pane_anchor: None,
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
                    session_id: session.clone(),
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
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .unwrap();
    let session = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, "old")
        .unwrap()
        .unwrap()
        .active_binding()
        .unwrap()
        .binding
        .session_id
        .as_ref()
        .unwrap()
        .to_string();
    server
        .state
        .lock()
        .unwrap()
        .apply_checked(&Event::AgentPresenceObserved {
            presence: AgentPresenceRecord {
                project_scope: route.project_scope_id.as_str().into(),
                worker_id: "old".into(),
                session_id: session,
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
                periodic_declared: false,
                goal_content: None,
            },
        })
        .unwrap();
}

#[test]
fn reflow_delivery_progress_keeps_the_persisted_assignment_generation() {
    let (server, root, route) = fixture();
    master(&server, &route, "jenny");
    subscribe(&server, "jenny");
    {
        let mut state = server.state.lock().unwrap();
        for id in ["one", "two"] {
            let mut assignment = state.task_assignment(id).unwrap().clone();
            assignment.assignment_revision = 7;
            state
                .apply_checked(&Event::TaskAssignmentUpdated { assignment })
                .unwrap();
        }
    }
    presence(&server, &route, AgentPresence::Offline, 2);
    tick_checked(&server, 3).unwrap();
    let message_ids = {
        let mut state = server.state.lock().unwrap();
        let ids: Vec<_> = ["one", "two"]
            .into_iter()
            .map(|id| {
                let assignment = state.task_assignment(id).unwrap();
                assert_eq!(assignment.assignment_revision, 7);
                assignment
                    .reflow
                    .as_ref()
                    .unwrap()
                    .message_id
                    .clone()
                    .unwrap()
            })
            .collect();
        for id in &ids {
            state
                .apply_checked(&Event::NotificationDeliveryAccepted {
                    message_id: id.clone(),
                    accepted_ms: 4,
                    evidence: None,
                })
                .unwrap();
        }
        ids
    };
    tick_checked(&server, 5).unwrap();
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&server.state_path).unwrap()).unwrap();
    let restored: State = serde_json::from_value(saved["state"].clone()).unwrap();
    for (id, message_id) in ["one", "two"].into_iter().zip(message_ids) {
        let assignment = restored.task_assignment(id).unwrap();
        assert_eq!(assignment.assignment_revision, 7);
        let reflow = assignment.reflow.as_ref().unwrap();
        assert_eq!(reflow.delivery, SchedulingReflowDelivery::Delivered);
        assert_eq!(reflow.message_id.as_deref(), Some(message_id.as_str()));
    }
    assert_eq!(
        restored.msgs.len(),
        2,
        "delivery progress must reuse the original messages"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn offline_reflow_is_once_per_task_and_recovery_starts_new_episode() {
    let (server, root, route) = fixture();
    presence(&server, &route, AgentPresence::Offline, 2);
    tick_checked(&server, 3).unwrap();
    let before = std::fs::read(&server.state_path).unwrap();
    tick_checked(&server, 4).unwrap();
    assert_eq!(std::fs::read(&server.state_path).unwrap(), before);
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
