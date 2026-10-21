use super::name_state::{AgentPresence, AgentPresenceRecord, NameOccupancy, TaskAssignment};

fn board_name_fixture() -> (Server, std::path::PathBuf, RouteScope) {
    let (server, root) = test_server();
    for worker in ["old", "current"] {
        let response = crate::server::peer_tests::register_synthetic_peer(&server, worker, worker);
        assert!(response.ok, "{:?}", response.error);
    }
    let mut state = server.state.lock().unwrap();
    let route = server_route_scope(&server, &state).unwrap().unwrap();
    state
        .apply_checked(&Event::TaskCreated {
            task: task("named-work", "old"),
        })
        .unwrap();
    state
        .board_details
        .get_mut("named-work")
        .unwrap()
        .public_visibility = true;
    state
        .apply_task_assignment(&TaskAssignment {
            task_id: "named-work".into(),
            name: "Alice".into(),
            session_id: "session-old".into(),
            assignment_revision: 1,
            reflow: None,
            updated_ms: 1,
        })
        .unwrap();
    state
        .name_occupancies
        .entry(route.project_scope_id.as_str().into())
        .or_default()
        .insert(
            "Alice".into(),
            NameOccupancy {
                name: "Alice".into(),
                worker_id: "old".into(),
                session_id: "session-old".into(),
                occupancy_revision: 1,
                updated_ms: 1,
            },
        );
    drop(state);
    (server, root, route)
}

fn board_name_presence(
    state: &mut State,
    route: &RouteScope,
    worker: &str,
    session: &str,
    presence: AgentPresence,
) {
    state
        .apply_agent_presence(&AgentPresenceRecord {
            project_scope: route.project_scope_id.as_str().into(),
            worker_id: worker.into(),
            session_id: session.into(),
            presence,
            reason: "test observation".into(),
            observed_ms: 2,
        })
        .unwrap();
}

#[test]
fn board_name_offline_binding_projects_empty_without_mutation() {
    let (server, root, route) = board_name_fixture();
    let mut state = server.state.lock().unwrap();
    board_name_presence(
        &mut state,
        &route,
        "old",
        "session-old",
        AgentPresence::Offline,
    );
    assert!(current_worker_binding(
        &state,
        &route.project_scope_id,
        Some(&route.app_scope_id),
        &state.workers["old"]
    )
    .is_some());
    let before = serde_json::to_value(state.snapshot_events()).unwrap();
    let view =
        board_task_view_with_master(&state, &state.tasks["named-work"], None, None, Some(&route))
            .unwrap();
    assert_eq!(view.owner, "");
    assert!(view.needs_reassignment);
    assert_eq!(view.owner_name.as_deref(), Some("Alice"));
    assert_eq!(view.owner_session_id, None);
    assert_eq!(view.reassignment_reason.as_deref(), Some("owner_offline"));
    assert_eq!(
        serde_json::to_value(state.snapshot_events()).unwrap(),
        before
    );
    drop(state);
    let response = handle_board_show(&server, None);
    assert!(response.ok, "{:?}", response.error);
    assert_eq!(response.data["tasks"][0]["owner"], "");
    assert_eq!(
        serde_json::to_value(server.state.lock().unwrap().snapshot_events()).unwrap(),
        before
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_current_holder_and_unknown_presence_preserve_closed_history() {
    let (server, root, route) = board_name_fixture();
    let mut state = server.state.lock().unwrap();
    let occupancy = state
        .name_occupancies
        .get_mut(route.project_scope_id.as_str())
        .unwrap()
        .get_mut("Alice")
        .unwrap();
    occupancy.worker_id = "current".into();
    occupancy.session_id = "session-current".into();
    state
        .task_assignments
        .get_mut("named-work")
        .unwrap()
        .session_id = "session-current".into();
    for presence in [AgentPresence::Online, AgentPresence::Unknown] {
        board_name_presence(&mut state, &route, "current", "session-current", presence);
        let view = board_task_view_with_master(
            &state,
            &state.tasks["named-work"],
            None,
            None,
            Some(&route),
        )
        .unwrap();
        assert_eq!(view.owner, "current");
        assert_eq!(view.owner_display_name, "Alice");
        let json = serde_json::to_value(view).unwrap();
        assert_eq!(json["owner_name"], "Alice");
        assert_eq!(json["owner_session_id"], "session-current");
        assert_eq!(json["needs_reassignment"], false);
    }
    let mut closed = state.tasks["named-work"].clone();
    closed.status = "closed".into();
    board_name_presence(
        &mut state,
        &route,
        "old",
        "session-old",
        AgentPresence::Offline,
    );
    assert_eq!(
        board_task_view_with_master(&state, &closed, None, None, Some(&route))
            .unwrap()
            .owner,
        "old"
    );
    assert_eq!(state.tasks["named-work"].owner, "old");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_claim_commits_current_occupancy_assignment() {
    let (server, root, route) = board_name_fixture();
    {
        let mut state = server.state.lock().unwrap();
        state.tasks.get_mut("named-work").unwrap().status = "pending".into();
        state.task_assignments.clear();
    }
    let response = handle_board_command(
        &server,
        "old".into(),
        "token-old".into(),
        crate::board::BoardCommand::Claim {
            id: "named-work".into(),
            expected_revision: 1,
        },
    );
    assert!(response.ok, "{:?}", response.error);
    let state = server.state.lock().unwrap();
    let assignment = state
        .task_assignments
        .get("named-work")
        .expect("claim persists typed assignment");
    assert_eq!(assignment.name, "Alice");
    assert_eq!(assignment.session_id, "session-old");
    assert_eq!(assignment.assignment_revision, 1);
    assert_eq!(state.tasks["named-work"].status, "working");
    assert_eq!(state.board_details["named-work"].revision, 2);
    let mut restored = State::default();
    for event in state.snapshot_events() {
        restored.apply_checked(&event).unwrap();
    }
    assert_eq!(restored.task_assignment("named-work"), Some(assignment));
    assert_eq!(
        route.project_scope_id.as_str(),
        GlobalState::canonical_project_scope(&root)
            .unwrap()
            .as_str()
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_missing_occupancy_does_not_fall_back_to_old_owner() {
    let (server, root, route) = board_name_fixture();
    let mut state = server.state.lock().unwrap();
    state.name_occupancies.clear();
    let before = serde_json::to_value(state.snapshot_events()).unwrap();
    let view =
        board_task_view_with_master(&state, &state.tasks["named-work"], None, None, Some(&route))
            .unwrap();
    assert_eq!(view.owner, "");
    assert_eq!(view.reassignment_reason.as_deref(), Some("name_unoccupied"));
    assert_eq!(
        serde_json::to_value(state.snapshot_events()).unwrap(),
        before
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_old_session_offline_does_not_hide_current_holder() {
    let (server, root, route) = board_name_fixture();
    let mut state = server.state.lock().unwrap();
    board_name_presence(
        &mut state,
        &route,
        "old",
        "retired-session",
        AgentPresence::Offline,
    );
    let view =
        board_task_view_with_master(&state, &state.tasks["named-work"], None, None, Some(&route))
            .unwrap();
    assert_eq!(view.owner, "old");
    assert!(!view.needs_reassignment);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_legacy_claim_preserves_label_without_inventing_assignment_or_occupancy() {
    let (server, root, _) = board_name_fixture();
    {
        let mut state = server.state.lock().unwrap();
        state.tasks.get_mut("named-work").unwrap().status = "pending".into();
        state.task_assignments.clear();
        state.name_occupancies.clear();
        state
            .worker_names
            .insert("old".into(), "Legacy Alice".into());
    }
    let rejected = board_claim(
        &server,
        "old".into(),
        "token-old".into(),
        "named-work".into(),
        0,
    );
    assert!(!rejected.ok);
    assert!(server.state.lock().unwrap().task_assignments.is_empty());
    let response = board_claim(
        &server,
        "old".into(),
        "token-old".into(),
        "named-work".into(),
        1,
    );
    assert!(response.ok, "{:?}", response.error);
    let state = server.state.lock().unwrap();
    assert!(state.task_assignment("named-work").is_none());
    assert_eq!(response.data["task"]["owner"], "old");
    assert_eq!(response.data["task"]["owner_display_name"], "Legacy Alice");
    assert!(state.name_occupancies.is_empty());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn board_name_legacy_invitation_defaults_original_fields() {
    let invitation: crate::board::BoardInvitation = serde_json::from_value(json!({
        "peer_id": "peer", "binding_id": "binding", "endpoint_generation": 1,
        "message_id": "message", "created_ms": 1,
    }))
    .unwrap();
    assert!(invitation.original_status.is_none());
    assert!(invitation.original_owner.is_none());
}
