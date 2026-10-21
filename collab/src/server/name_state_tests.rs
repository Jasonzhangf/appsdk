use super::*;
use crate::server::name_state::{
    AgentActivity, AgentActivityRecord, AgentPresence, AgentPresenceRecord, NameBindingReference,
    NameOccupancy, NameOccupancyRef, NameRecoveryDecision, NameRecoveryOutcome,
    NameRecoveryPending, NameRecoveryProgress, NameRecoveryRequest, NameRecoveryResult,
    NameRecoveryVerdict, SchedulingReflowDelivery, TaskAssignment, TaskSchedulingReflow,
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

fn recovery(project_scope: &str, operation_id: &str, target_name: &str) -> NameRecoveryPending {
    NameRecoveryPending {
        operation_id: operation_id.into(),
        project_scope: project_scope.into(),
        target_name: target_name.into(),
        occupancy_ref: NameOccupancyRef {
            project_scope: project_scope.into(),
            name: target_name.into(),
            worker_id: "worker-old".into(),
            session_id: "session-old".into(),
            occupancy_revision: 4,
        },
        request: NameRecoveryRequest {
            invocation: "context-once".into(),
            requested_name: Some(target_name.into()),
            caller_worker_id: "worker-old".into(),
            caller_session_id: "session-old".into(),
            caller_binding: NameBindingReference {
                app_scope_id: "appserver-cli".into(),
                binding_id: "binding-old".into(),
                endpoint_generation: 3,
            },
            replace_confirmed: false,
            requested_ms: 21,
        },
        progress: NameRecoveryProgress::AdjudicationRequested,
        decision: Some(NameRecoveryDecision {
            verdict: NameRecoveryVerdict::Offline,
            evidence: "shell prompt visible".into(),
            decided_by: "Jenny".into(),
            decided_ms: 30,
        }),
        result: Some(NameRecoveryResult {
            outcome: NameRecoveryOutcome::Applied,
            detail: Some("current caller inherits the target name".into()),
            recorded_ms: 31,
        }),
        created_ms: 21,
        updated_ms: 31,
    }
}

#[test]
fn old_journal_without_name_state_fields_replays_with_empty_new_projections() {
    let root = name_state_test_root("legacy");
    let journal = root.join(".agent-collab/server/journal.jsonl");
    std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
    std::fs::write(
        &journal,
        "{\"ev\":\"WorkerNamed\",\"worker_id\":\"worker-1\",\"name\":\"Alice\"}\n",
    )
    .unwrap();

    let replayed = crate::server::replay(&root).expect("old journal must replay");

    assert_eq!(replayed.worker_names["worker-1"], "Alice");
    assert!(replayed.name_occupancies.is_empty());
    assert!(replayed.task_assignments.is_empty());
    assert!(replayed.name_recoveries.is_empty());
    assert!(replayed.agent_presence.is_empty());
    assert!(replayed.agent_activity.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn name_revision_retains_released_epochs_across_legacy_replay_and_snapshots() {
    let mut state = State::default();
    // Pre-watermark journals contain only the committed name events.
    for (name, revision) in [("Jenny", 9), ("Alice", 1)] {
        state
            .apply_checked(&Event::NameResolutionCommitted {
                project_scope: "/project".into(),
                occupancy: NameOccupancy {
                    name: name.into(),
                    worker_id: "worker".into(),
                    session_id: "session".into(),
                    occupancy_revision: revision,
                    updated_ms: 1,
                },
                assignments: vec![],
            })
            .unwrap();
    }
    assert!(state.name_occupancy("/project", "Jenny").is_none());
    assert_eq!(state.name_occupancy_revision("/project", "Jenny"), 9);
    assert_eq!(state.name_occupancy_revision("/other", "Jenny"), 0);
    for _ in 0..2 {
        let mut replay = State::default();
        for event in state.snapshot_events() {
            let encoded = serde_json::to_string(&event).unwrap();
            replay
                .apply_checked(&serde_json::from_str(&encoded).unwrap())
                .unwrap();
        }
        assert!(replay.name_occupancy("/project", "Jenny").is_none());
        assert_eq!(replay.name_occupancy_revision("/project", "Jenny"), 9);
        state = replay;
    }
    super::super::project_reset::clear_project(&mut state, "/project", false, &[]);
    assert_eq!(state.name_occupancy_revision("/project", "Jenny"), 0);
}

#[test]
fn name_revision_recovers_legacy_snapshot_retained_handoff_and_pending_references() {
    let mut state = State::default();
    // An older compacted snapshot may have released Jenny and retain only
    // handoff/recovery evidence for its epoch. Neither recreates an occupant.
    let event: Event = serde_json::from_value(serde_json::json!({
        "ev":"MailboxReassigned", "handoff":{
            "project_scope":"/project", "name":"Jenny", "occupancy_revision":12,
            "session_id":"s", "message_id":"m", "previous_recipient":"old",
            "recipient":"new", "wake_attempt_count":1,
            "previous_subscription":null,"accepted_ms":null,"evidence":null,"failure":null
        }
    }))
    .unwrap();
    state.apply_checked(&event).unwrap();
    let mut pending = recovery("/project", "pending", "Jenny");
    pending.occupancy_ref.occupancy_revision = 14;
    state
        .apply_checked(&Event::NameRecoveryUpdated { recovery: pending })
        .unwrap();
    // Replaying lower retained evidence cannot reduce the watermark.
    state.apply_checked(&event).unwrap();
    assert_eq!(state.name_occupancy_revision("/project", "Jenny"), 14);
    assert!(state.name_occupancy("/project", "Jenny").is_none());
    let mut replay = State::default();
    for event in state.snapshot_events() {
        replay.apply_checked(&event).unwrap();
    }
    assert_eq!(replay.name_occupancy_revision("/project", "Jenny"), 14);
}

#[test]
fn name_resolution_atomically_replays_with_assignments_and_snapshot_records() {
    let mut rejected = State::default();
    let error = rejected
        .apply_checked(&Event::NameResolutionCommitted {
            project_scope: "/project".into(),
            occupancy: NameOccupancy {
                name: "Alice".into(),
                worker_id: "worker-1".into(),
                session_id: "session-2".into(),
                occupancy_revision: 6,
                updated_ms: 19,
            },
            assignments: vec![assignment("task-a", "Bob", "session-2", 6)],
        })
        .unwrap_err();
    assert!(error.contains("does not match occupancy"), "{error}");
    assert!(rejected.name_occupancies.is_empty());
    assert!(rejected.task_assignments.is_empty());

    let mut state = State::default();
    state.apply(&Event::NameResolutionCommitted {
        project_scope: "/project".into(),
        occupancy: NameOccupancy {
            name: "Alice".into(),
            worker_id: "worker-1".into(),
            session_id: "session-2".into(),
            occupancy_revision: 7,
            updated_ms: 20,
        },
        assignments: vec![
            assignment("task-a", "Alice", "session-2", 7),
            assignment("task-b", "Alice", "session-2", 7),
        ],
    });
    state.apply(&Event::NameRecoveryUpdated {
        recovery: recovery("/project", "operation-name-1", "Bob"),
    });
    state.apply(&Event::AgentPresenceObserved {
        presence: AgentPresenceRecord {
            project_scope: "/project".into(),
            worker_id: "worker-1".into(),
            session_id: "session-2".into(),
            presence: AgentPresence::Online,
            reason: "codex-visible".into(),
            observed_ms: 32,
        },
    });
    state.apply(&Event::AgentActivityObserved {
        activity: AgentActivityRecord {
            project_scope: "/project".into(),
            worker_id: "worker-1".into(),
            session_id: "session-2".into(),
            activity: AgentActivity::Busy,
            background: false,
            reason: "generation-active".into(),
            observed_ms: 32,
        },
    });

    assert_eq!(
        state.name_occupancies["/project"]["Alice"].session_id,
        "session-2"
    );
    assert_eq!(state.task_assignments["task-a"].session_id, "session-2");
    assert_eq!(state.task_assignments["task-b"].session_id, "session-2");

    let replayed = state
        .snapshot_events()
        .into_iter()
        .map(|event| serde_json::from_value(serde_json::to_value(event).unwrap()).unwrap())
        .fold(State::default(), |mut replayed, event| {
            replayed.apply(&event);
            replayed
        });

    assert_eq!(replayed.name_occupancies, state.name_occupancies);
    assert_eq!(replayed.task_assignments, state.task_assignments);
    assert_eq!(replayed.name_recoveries, state.name_recoveries);
    assert_eq!(replayed.agent_presence, state.agent_presence);
    assert_eq!(replayed.agent_activity, state.agent_activity);
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

    let replayed = state
        .snapshot_events()
        .into_iter()
        .map(|event| serde_json::from_value(serde_json::to_value(event).unwrap()).unwrap())
        .fold(State::default(), |mut replayed, event| {
            replayed.apply(&event);
            replayed
        });
    assert_eq!(replayed.name_occupancies, state.name_occupancies);
    assert_eq!(replayed.task_assignments, state.task_assignments);
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
        },
        assignments: vec![assignment("task-reset", "Alice", "session-new", 5)],
    });
    state.apply(&Event::NameRecoveryUpdated {
        recovery: recovery("/project", "operation-reset", "Bob"),
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
        },
    });

    crate::server::project_reset::clear_project(&mut state, "/project", true, &[]);

    assert!(state.name_occupancies.is_empty());
    assert!(state.task_assignments.is_empty());
    assert!(state.name_recoveries.is_empty());
    assert!(state.agent_presence.is_empty());
    assert!(state.agent_activity.is_empty());
}
