use super::*;
use name_resolution::{NameResolutionDecision, NameResolutionInput};
use name_state::{NameBindingReference, NameOccupancy, NameRecoveryRequest, TaskAssignment};

fn setup() -> (Server, PathBuf) {
    let (server, root) = peer_tests::test_server();
    for id in ["a", "b", "c"] {
        let transport = peer_tests::test_appserver_transport(id);
        let registered = peer_tests::seed_committed_owner(
            &server,
            id,
            id,
            &transport,
            root.to_str().unwrap(),
            Some(AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap()),
        );
        assert!(registered.ok, "{registered:?}");
    }
    let promoted = handle_master_promote(
        &server,
        "a".into(),
        "a".into(),
        "original user approval".into(),
    );
    assert!(promoted.ok, "{promoted:?}");
    (server, root)
}

fn replay(server: &Server) -> State {
    let mut state = State::default();
    for line in std::fs::read_to_string(&server.journal_path)
        .unwrap()
        .lines()
    {
        state
            .apply_checked(&serde_json::from_str::<Event>(line).unwrap())
            .unwrap();
    }
    state
}

#[test]
fn role_name_closure_delegate_transfers_work_and_mailbox_with_authority() {
    let (server, root) = setup();
    let scope = server_route_scope(&server, &server.state.lock().unwrap())
        .unwrap()
        .unwrap();
    {
        let mut state = server.state.lock().unwrap();
        let mut events = Vec::new();
        for (id, status) in [("open", "working"), ("closed", "closed")] {
            events.push(Event::TaskCreated {
                task: TaskRec {
                    id: id.into(),
                    owner: "a".into(),
                    created_by: "a".into(),
                    feature_id: None,
                    worktree_path: Some("/resource".into()),
                    branch: Some("branch".into()),
                    base_commit: None,
                    priority: "p2".into(),
                    status: status.into(),
                    next_step: Some("preserve".into()),
                    wait: None,
                    created_ms: 1,
                    updated_ms: 1,
                },
            });
            events.push(Event::TaskAssignmentUpdated {
                assignment: TaskAssignment {
                    task_id: id.into(),
                    name: "Jenny".into(),
                    session_id: "a".into(),
                    assignment_revision: 1,
                    reflow: None,
                    updated_ms: 1,
                },
            });
        }
        events.push(Event::WorktreeBound {
            binding: WorktreeBinding {
                worktree_root: "/resource".into(),
                owning_project_scope: scope.project_scope_id.as_str().into(),
                task_id: "open".into(),
                owner_agent_id: "a".into(),
                binding_id: "resource".into(),
                base_commit: "base".into(),
            },
        });
        for (id, status) in [("pending", "pending"), ("read", "read")] {
            events.push(Event::Sent {
                msg: Message {
                    id: id.into(),
                    from: "c".into(),
                    to: "a".into(),
                    mtype: "notify".into(),
                    subject: Some(id.into()),
                    body: "original body".into(),
                    in_reply_to: None,
                    created_ms: 1,
                    state: status.into(),
                    wake_attempt_count: 0,
                    last_wake_attempt_ms: 0,
                    retry_attempted: false,
                },
            });
        }
        server.commit_locked_checked(&mut state, &events).unwrap();
    }
    let before = std::fs::read(&server.journal_path).unwrap();
    for (actor, token, target) in [("a", "wrong", "b"), ("b", "b", "c"), ("a", "a", "missing")] {
        assert!(!handle_master_delegate(&server, actor.into(), token.into(), target.into()).ok);
    }
    assert_eq!(std::fs::read(&server.journal_path).unwrap(), before);
    let delegated = handle_master_delegate(&server, "a".into(), "a".into(), "b".into());
    assert!(delegated.ok, "{delegated:?}");
    for state in [&*server.state.lock().unwrap(), &replay(&server)] {
        assert_eq!(
            current_master_worker_id(state, Some(&scope)).as_deref(),
            Some("b")
        );
        assert_eq!(
            state
                .name_occupancy(scope.project_scope_id.as_str(), "Jenny")
                .unwrap()
                .worker_id,
            "b"
        );
        assert_eq!(worker_display_name(state, "b", true), "Jenny");
        assert_eq!(state.tasks["open"].owner, "b");
        assert_eq!(state.task_assignments["open"].session_id, "session-b");
        assert_eq!(state.worktree_bindings["resource"].owner_agent_id, "b");
        assert_eq!(state.tasks["closed"].owner, "a");
        assert_eq!(state.task_assignments["closed"].session_id, "a");
        assert_eq!(state.msgs["pending"].to, "b");
        assert_eq!(state.msgs["read"].to, "a");
        assert_eq!(state.msgs["pending"].body, "original body");
        assert_eq!(state.msgs["pending"].from, "c");
        let grant = current_master_grant(state, Some(&scope)).unwrap();
        assert_eq!(grant.granted_by, "a");
        assert_eq!(grant.approval, "delegated by the current master");
    }
    assert!(!handle_master_delegate(&server, "a".into(), "a".into(), "c".into()).ok);
    let cleared = handle_master_clear(&server, "c".into(), "c".into(), "user clear".into());
    assert!(cleared.ok, "{cleared:?}");
    assert!(current_master_worker_id(&replay(&server), Some(&scope)).is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn role_name_closure_clear_revokes_misaligned_legacy_grant_in_one_commit() {
    let (server, root) = setup();
    let scope = server_route_scope(&server, &server.state.lock().unwrap())
        .unwrap()
        .unwrap();
    {
        let mut state = server.state.lock().unwrap();
        let grant = master_grant_for_worker(&state, &scope, "b", "a", "legacy delegation").unwrap();
        let events = master_authority_transfer_events(&state, &scope, grant);
        server.commit_locked_checked(&mut state, &events).unwrap();
    }
    let cleared = handle_master_clear(&server, "b".into(), "b".into(), "user clear".into());
    assert!(cleared.ok, "{cleared:?}");
    let state = replay(&server);
    assert!(current_master_worker_id(&state, Some(&scope)).is_none());
    assert!(state
        .name_occupancy(scope.project_scope_id.as_str(), "Jenny")
        .is_none());
    assert!(state.workers.contains_key("a") && state.workers.contains_key("b"));
    let again = handle_master_clear(&server, "b".into(), "b".into(), "user clear".into());
    assert!(again.ok, "{again:?}");
    assert_eq!(again.data["was_empty"], true);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn role_name_closure_default_peer_ignores_historical_jenny_worker_and_session() {
    let mut state = State::default();
    state
        .apply_checked(&Event::NameResolutionCommitted {
            project_scope: "/project".into(),
            occupancy: NameOccupancy {
                name: "Jenny".into(),
                worker_id: "old".into(),
                session_id: "s1".into(),
                occupancy_revision: 1,
                updated_ms: 1,
            },
            assignments: vec![],
        })
        .unwrap();
    for (worker, session) in [("old", "s2"), ("new", "s1")] {
        let input = NameResolutionInput {
            project_scope: "/project".into(),
            operation_id: "ordinary".into(),
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
                requested_ms: 2,
            },
            intent: None,
            is_current_master: false,
        };
        let plan = name_resolution::plan_name_resolution(&state, &input).unwrap();
        assert!(!plan.requires_master_authorization);
        assert!(
            matches!(plan.decision, NameResolutionDecision::Committed { occupancy, .. } if occupancy.name == "Alice")
        );
        assert_eq!(
            state.name_occupancy("/project", "Jenny").unwrap().worker_id,
            "old"
        );
    }
}
