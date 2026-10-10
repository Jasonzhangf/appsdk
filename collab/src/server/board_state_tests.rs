use super::*;
use crate::server::peer_tests::test_server;

/// Register a state-level worker without a transport fixture. These tests
/// cover board projection and replay, not transport selection.
fn register_worker(state: &mut State, id: &str) {
    state.workers.insert(
        id.into(),
        crate::server::state::WorkerRec {
            id: id.into(),
            token: format!("token-{id}"),
            cwd: "/tmp/board-state-fixture".into(),
            registered_ms: now_ms(),
            transport: None,
        },
    );
}

fn task(id: &str, owner: &str) -> TaskRec {
    TaskRec {
        id: id.into(),
        owner: owner.into(),
        created_by: owner.into(),
        feature_id: None,
        worktree_path: None,
        branch: None,
        base_commit: None,
        priority: "p2".into(),
        status: "working".into(),
        next_step: None,
        wait: None,
        created_ms: now_ms(),
        updated_ms: now_ms(),
    }
}

#[test]
fn compact_snapshot_preserves_public_history_and_exact_revision() {
    let (server, root) = test_server();
    let mut state = server.state.lock().unwrap();
    register_worker(&mut state, "peer");
    let mut task = task("public-history", "peer");
    state
        .apply_checked(&Event::TaskCreated { task: task.clone() })
        .unwrap();
    for next in ["实施", "验证", "交付"] {
        task.next_step = Some(next.into());
        state
            .apply_checked(&Event::TaskUpdated { task: task.clone() })
            .unwrap();
    }
    state
        .apply_checked(&Event::WorkerClosed {
            worker_id: "peer".into(),
            closed_by: "peer".into(),
            reason: "finished fixture".into(),
            snapshot_captured_ms: None,
            at_ms: now_ms(),
        })
        .unwrap();
    assert!(!state.workers.contains_key("peer"));
    assert_eq!(board_task_view(&state, &task).unwrap().revision, 4);
    let events = state.snapshot_events();
    let mut restored = State::default();
    for event in events {
        restored.apply_checked(&event).unwrap();
    }
    let view = board_task_view(&restored, &restored.tasks["public-history"]).unwrap();
    assert_eq!(view.revision, 4);
    assert_eq!(view.owner, "peer");
    assert_eq!(view.next_step.as_deref(), Some("交付"));
    assert!(!restored.workers.contains_key("peer"));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unknown_legacy_actor_is_not_public_after_compaction() {
    let mut state = State::default();
    let task = task("unknown-history", "unknown-actor");
    state
        .apply_checked(&Event::TaskCreated { task: task.clone() })
        .unwrap();
    assert!(board_task_view(&state, &task).is_none());
    let mut restored = State::default();
    for event in state.snapshot_events() {
        restored.apply_checked(&event).unwrap();
    }
    assert!(board_task_view(&restored, &restored.tasks["unknown-history"]).is_none());
}

#[test]
fn compact_snapshot_preserves_invitation_and_its_capacity_reservation() {
    let (server, root) = test_server();
    let mut state = server.state.lock().unwrap();
    register_worker(&mut state, "publisher");
    register_worker(&mut state, "peer");
    let mut task = task("offer", "publisher");
    task.status = "invited".into();
    state.apply_checked(&Event::TaskCreated { task }).unwrap();
    state
        .apply_checked(&Event::BoardDetailsChanged {
            task_id: "offer".into(),
            details: crate::board::BoardTaskDetails {
                revision: 17,
                public_visibility: true,
                invitation: Some(crate::board::BoardInvitation {
                    peer_id: "peer".into(),
                    binding_id: "test-binding".into(),
                    endpoint_generation: 8,
                    message_id: "offer-message".into(),
                    created_ms: now_ms(),
                }),
                ..crate::board::BoardTaskDetails::default()
            },
        })
        .unwrap();
    assert!(board_peer_has_responsibility(&state, "peer", None));
    assert!(!keepalive::actionable("invited"));
    assert!(!task_resource_active("invited"));
    let mut restored = State::default();
    for event in state.snapshot_events() {
        restored.apply_checked(&event).unwrap();
    }
    assert_eq!(restored.board_details["offer"].revision, 17);
    assert_eq!(
        restored.board_details["offer"]
            .invitation
            .as_ref()
            .unwrap()
            .endpoint_generation,
        8
    );
    assert_eq!(restored.tasks["offer"].owner, "publisher");
    assert!(board_peer_has_responsibility(&restored, "peer", None));
    assert!(board_check_revision(&restored, "offer", 16).is_err());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
