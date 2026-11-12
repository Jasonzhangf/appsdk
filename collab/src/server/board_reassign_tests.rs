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

/// Current committed session fixture. Public recovery is covered by the map
/// consumer tests; this fixture exercises the Board's business handoff owner.
fn refresh_board_fixture(server: &Server, route: &RouteScope, worker: &str, session: &str) {
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap();
    let key = control.key_for_worker(&route.project_scope_id, &route.app_scope_id, worker).unwrap().unwrap();
    let mut transport = control.slot(key).unwrap().active_binding().unwrap().transport.clone();
    transport.session_id = Some(session.into());
    if let Some(endpoint) = transport.tmux_endpoint.as_mut() {
        endpoint.codex_session_id = Some(session.into());
    }
    let response = crate::server::peer_tests::seed_committed_owner(
        server, worker, &format!("token-{worker}"), &transport,
        &server.root.to_string_lossy(), Some(route.app_scope_id.clone()),
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
        let replayed: State = serde_json::from_slice(&serde_json::to_vec(&*state).unwrap()).unwrap();
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
fn stale_executor_handoff_failure_preserves_task_and_schedule() {
    use crate::server::ticket_state::{TicketPhaseKind, TicketPhaseRecord, TicketPhaseStatus};
    let (server, root, _) = reassign_fixture("working");
    {
        let mut state = server.state.lock().unwrap();
        let details = state.board_details.get_mut("named-work").unwrap();
        details.admitted = true;
        details.long_horizon = Some(crate::board::LongHorizonTaskSpec {
            content: "continue existing handoff".into(),
            interval_ms: 1_200_000,
            repeat_count: 100,
            ttl_seconds: 172_800,
        });
        let events = task_goal_events(&server, &state, &state.tasks["named-work"],
            &state.board_details["named-work"], Some("current"), false, false).unwrap().0;
        for event in events { state.apply_checked(&event).unwrap(); }
        state.notification_subscriptions.values_mut().find(|sub| {
            sub.subject.as_deref() == Some("goal:task:named-work")
        }).unwrap().fired_count = 7;
        state.ticket_phases.push(TicketPhaseRecord {
            task_id: "named-work".into(), phase: TicketPhaseKind::Execute,
            batch: 1, priority: "p2".into(), status: TicketPhaseStatus::Running,
            slot_name: Some("Alice".into()), binding_generation: 1,
            enqueue_seq: 1, attempt_generation: 1, depends_on: vec![],
            route_group: None, wait_reason: None, result: None,
        });
        state.workers.remove("old");
    }
    let before = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    // Existing deterministic persistence-failure mechanism: rename cannot
    // publish a state document over a directory. No daemon or timing needed.
    if server.state_path.exists() { std::fs::remove_file(&server.state_path).unwrap(); }
    std::fs::create_dir(&server.state_path).unwrap();
    let response = board_respond(&server, "current".into(), "token-current".into(),
        "named-work".into(), 1, true, None);
    assert!(!response.ok, "{response:?}");
    assert!(response.error.as_deref().unwrap().contains("BOARD_DURABILITY_FAILED"), "{response:?}");
    let after = serde_json::to_value(&*server.state.lock().unwrap()).unwrap();
    assert_eq!(after, before, "failed acceptance must publish no partial state");
    std::fs::remove_dir(&server.state_path).unwrap();
    let response = board_respond(&server, "current".into(), "token-current".into(),
        "named-work".into(), 1, true, None);
    assert!(response.ok, "retry without persistence fault: {response:?}");
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
    let control =
        binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap();
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
        state.task_lifecycle["named-work"].delivery_evidence.as_deref(),
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
