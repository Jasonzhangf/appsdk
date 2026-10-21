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
        endpoint_generation: binding.endpoint_generation,
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
        let mut state = server.state.lock().unwrap();
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
        let mut old = state
            .name_occupancy(route.project_scope_id.as_str(), "Alice")
            .unwrap()
            .clone();
        old.session_id = "old-refreshed".into();
        old.occupancy_revision += 1;
        for event in name_resolution::build_name_resolution_events(
            &state,
            route.project_scope_id.as_str(),
            &old,
        ) {
            state.apply_checked(&event).unwrap();
        }
        assert_eq!(
            state.task_assignment("named-work").unwrap().session_id,
            "session-current"
        );
        let mut new = state
            .name_occupancy(route.project_scope_id.as_str(), "Bob")
            .unwrap()
            .clone();
        new.session_id = "current-refreshed".into();
        new.occupancy_revision += 1;
        for event in name_resolution::build_name_resolution_events(
            &state,
            route.project_scope_id.as_str(),
            &new,
        ) {
            state.apply_checked(&event).unwrap();
        }
        assert_eq!(
            state.task_assignment("named-work").unwrap().session_id,
            "current-refreshed"
        );
        assert_eq!(state.tasks["named-work"].status, status);
        let mut replayed = State::default();
        for event in state.snapshot_events() {
            replayed.apply_checked(&event).unwrap();
        }
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
    let before = serde_json::to_value(server.state.lock().unwrap().snapshot_events()).unwrap();
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
        serde_json::to_value(server.state.lock().unwrap().snapshot_events()).unwrap(),
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
