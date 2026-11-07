use super::*;
use crate::server::name_state::{
    AgentActivity, AgentActivityRecord, AgentPresence, AgentPresenceRecord, NameOccupancy,
    SchedulingReflowDelivery, TaskAssignment, TaskSchedulingReflow,
};
use std::sync::atomic::{AtomicU64, Ordering};

static NAME_STATE_TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn name_state_test_root(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "collab-name-state-{label}-{}-{}",
        std::process::id(),
        NAME_STATE_TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn task(id: &str, owner: &str, status: &str) -> TaskRec {
    TaskRec {
        id: id.into(),
        owner: owner.into(),
        created_by: "master".into(),
        feature_id: None,
        worktree_path: None,
        branch: None,
        base_commit: None,
        priority: "p2".into(),
        status: status.into(),
        next_step: None,
        wait: None,
        isolation_mode: None,
        created_ms: 1,
        updated_ms: 1,
    }
}

fn assignment(task_id: &str, name: &str, session_id: &str, revision: u64) -> TaskAssignment {
    TaskAssignment {
        task_id: task_id.into(),
        name: name.into(),
        session_id: session_id.into(),
        assignment_revision: revision,
        reflow: Some(TaskSchedulingReflow {
            episode: revision,
            reason: "name-current-session".into(),
            delivery: SchedulingReflowDelivery::Pending,
            message_id: None,
            updated_ms: 10 + revision as i64,
        }),
        updated_ms: 10 + revision as i64,
    }
}

#[test]
fn name_resolution_releases_previous_current_name_for_the_same_worker() {
    let mut state = State::default();
    for (project_scope, occupancy) in [
        (
            "/project",
            NameOccupancy {
                name: "Alice".into(),
                worker_id: "worker-1".into(),
                session_id: "session-alice".into(),
                occupancy_revision: 1,
                updated_ms: 10,
                pane_anchor: None,
            },
        ),
        (
            "/project",
            NameOccupancy {
                name: "Bob".into(),
                worker_id: "worker-2".into(),
                session_id: "session-bob".into(),
                occupancy_revision: 1,
                updated_ms: 10,
                pane_anchor: None,
            },
        ),
        (
            "/project",
            NameOccupancy {
                name: "Jenny".into(),
                worker_id: "worker-3".into(),
                session_id: "session-replaced".into(),
                occupancy_revision: 1,
                updated_ms: 10,
                pane_anchor: None,
            },
        ),
        (
            "/other-project",
            NameOccupancy {
                name: "Alice".into(),
                worker_id: "worker-1".into(),
                session_id: "session-other-project".into(),
                occupancy_revision: 1,
                updated_ms: 10,
                pane_anchor: None,
            },
        ),
    ] {
        state.apply(&Event::NameResolutionCommitted {
            project_scope: project_scope.into(),
            occupancy,
            assignments: Vec::new(),
        });
    }
    state.apply(&Event::TaskAssignmentUpdated {
        assignment: assignment("task-alice", "Alice", "session-alice", 1),
    });

    let transition = Event::NameResolutionCommitted {
        project_scope: "/project".into(),
        occupancy: NameOccupancy {
            name: "Jenny".into(),
            worker_id: "worker-1".into(),
            session_id: "session-jenny".into(),
            occupancy_revision: 2,
            updated_ms: 20,
            pane_anchor: None,
        },
        assignments: Vec::new(),
    };
    state.apply(&transition);
    state.apply(&transition);

    assert!(state.name_occupancy("/project", "Alice").is_none());
    assert_eq!(
        state.name_occupancy("/project", "Jenny").unwrap().worker_id,
        "worker-1"
    );
    assert_eq!(
        state.name_occupancy("/project", "Bob").unwrap().worker_id,
        "worker-2"
    );
    assert_eq!(
        state
            .name_occupancy("/other-project", "Alice")
            .unwrap()
            .worker_id,
        "worker-1"
    );
    assert_eq!(state.task_assignment("task-alice").unwrap().name, "Alice");
}

#[test]
fn closed_task_history_is_not_modified_by_name_session_resolution() {
    let mut state = State::default();
    let closed = task("task-closed", "worker-1", "closed");
    let closed_history = serde_json::to_value(&closed).unwrap();
    state.apply(&Event::TaskCreated {
        task: closed.clone(),
    });
    state.apply(&Event::TaskCreated {
        task: task("task-open", "worker-1", "working"),
    });
    state.apply(&Event::TaskAssignmentUpdated {
        assignment: assignment("task-closed", "Alice", "session-old", 3),
    });
    let closed_assignment = state.task_assignments["task-closed"].clone();

    state.apply(&Event::NameResolutionCommitted {
        project_scope: "/project".into(),
        occupancy: NameOccupancy {
            name: "Alice".into(),
            worker_id: "worker-1".into(),
            session_id: "session-new".into(),
            occupancy_revision: 4,
            updated_ms: 40,
            pane_anchor: None,
        },
        assignments: vec![assignment("task-open", "Alice", "session-new", 4)],
    });

    assert_eq!(
        serde_json::to_value(&state.tasks["task-closed"]).unwrap(),
        closed_history
    );
    assert_eq!(state.task_assignments["task-closed"], closed_assignment);
    assert_eq!(
        state.task_assignments["task-open"].session_id,
        "session-new"
    );
}

#[test]
fn project_reset_clears_new_project_state() {
    let mut state = State::default();
    state.apply(&Event::TaskCreated {
        task: task("task-reset", "worker-1", "working"),
    });
    state.apply(&Event::NameResolutionCommitted {
        project_scope: "/project".into(),
        occupancy: NameOccupancy {
            name: "Alice".into(),
            worker_id: "worker-1".into(),
            session_id: "session-new".into(),
            occupancy_revision: 5,
            updated_ms: 50,
            pane_anchor: None,
        },
        assignments: vec![assignment("task-reset", "Alice", "session-new", 5)],
    });
    state.apply(&Event::AgentPresenceObserved {
        presence: AgentPresenceRecord {
            project_scope: "/project".into(),
            worker_id: "worker-1".into(),
            session_id: "session-new".into(),
            presence: AgentPresence::Online,
            reason: "codex-visible".into(),
            observed_ms: 50,
        },
    });
    state.apply(&Event::AgentActivityObserved {
        activity: AgentActivityRecord {
            project_scope: "/project".into(),
            worker_id: "worker-1".into(),
            session_id: "session-new".into(),
            activity: AgentActivity::Busy,
            background: false,
            reason: "generation-active".into(),
            observed_ms: 50,
            idle_since_ms: None,
        },
    });

    crate::server::project_reset::clear_project(&mut state, "/project", true, &[]);

    assert!(state.name_occupancies.is_empty());
    assert!(state.task_assignments.is_empty());
    assert!(state.agent_presence.is_empty());
    assert!(state.agent_activity.is_empty());
}
