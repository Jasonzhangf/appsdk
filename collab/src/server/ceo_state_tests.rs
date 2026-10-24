//! Persistence tests for host-owned CEO coordination facts in the shared reducer.
//!
//! These exercise the reducer contract only: the real journal owner still
//! commits the same typed events, and replay uses `snapshot_events`.

use crate::ceo::{CeoBootstrapRecord, CeoGrant};
use crate::proto::ReplyPolicy;
use crate::server::state::{
    responsibility_preflight, Event, ReplyObligation, ResponsibilityFence, ResponsibilitySnapshot,
    State,
};

fn replay(state: &State) -> State {
    state
        .snapshot_events()
        .into_iter()
        .fold(State::default(), |mut replayed, event| {
            replayed.apply(&event);
            replayed
        })
}

fn grant(workspace: &str, granted_ms: i64) -> CeoGrant {
    CeoGrant {
        workspace: workspace.into(),
        app_scope_id: "appsdk".into(),
        name: "ceo".into(),
        granted_by: "user".into(),
        approval: "approved".into(),
        granted_ms,
    }
}

fn bootstrap(operation_id: &str, phase: &str) -> CeoBootstrapRecord {
    CeoBootstrapRecord {
        operation_id: operation_id.into(),
        project: "/tmp/target-project".into(),
        approval: "approved".into(),
        profile: "gcm".into(),
        tmux_socket: "/tmp/target.sock".into(),
        tmux_session: "target".into(),
        requested_by: "ceo".into(),
        requested_workspace: "/workspace".into(),
        requested_app_scope: "appserver-cli".into(),
        phase: phase.into(),
        pane_id: None,
        error: None,
        handshake_message_id: None,
        cancellation_approval: None,
        updated_ms: 1,
    }
}

fn cross_project_obligation() -> ReplyObligation {
    ReplyObligation {
        message_id: "m-1".into(),
        project: "/tmp/target-project".into(),
        from_project: Some("/tmp/origin-project".into()),
        from_ceo: true,
        from_worker_id: "ceo".into(),
        from_name: Some("ceo".into()),
        recipient_worker_id: "master".into(),
        recipient_name: Some("master".into()),
        policy: ReplyPolicy::Required,
        created_ms: 30,
        reminded_idle_episode: false,
    }
}

#[test]
fn ceo_grant_set_replace_clear_survives_snapshot_replay() {
    let mut state = State::default();
    state.apply(&Event::CeoGrantSet {
        grant: Some(grant("/tmp/ws-a", 10)),
    });
    state.apply(&Event::CeoBootstrapSet {
        record: bootstrap("op-1", "prepared"),
    });

    let replayed = replay(&state);
    assert_eq!(
        replayed
            .ceo_grant
            .as_ref()
            .map(|grant| grant.workspace.as_str()),
        Some("/tmp/ws-a")
    );
    assert_eq!(replayed.ceo_bootstraps.len(), 1);

    let mut replaced = replayed;
    replaced.apply(&Event::CeoGrantSet {
        grant: Some(grant("/tmp/ws-b", 20)),
    });
    let replayed = replay(&replaced);
    assert_eq!(
        replayed
            .ceo_grant
            .as_ref()
            .map(|grant| grant.workspace.as_str()),
        Some("/tmp/ws-b")
    );

    let mut cleared = replayed;
    cleared.apply(&Event::CeoGrantSet { grant: None });
    assert!(cleared.ceo_grant.is_none());
    let replayed = replay(&cleared);
    assert!(replayed.ceo_grant.is_none());
    assert_eq!(
        replayed.ceo_bootstraps.len(),
        1,
        "clearing the grant must retain bootstrap history"
    );
}

#[test]
fn ceo_bootstrap_phase_and_cross_project_reply_survive_snapshot_replay() {
    let mut state = State::default();
    state.apply(&Event::CeoBootstrapSet {
        record: bootstrap("op-1", "prepared"),
    });
    state.apply(&Event::CeoBootstrapSet {
        record: CeoBootstrapRecord {
            phase: "launched".into(),
            pane_id: Some("%7".into()),
            updated_ms: 20,
            ..bootstrap("op-1", "launched")
        },
    });
    state.apply(&Event::ReplyObligationSet {
        obligation: cross_project_obligation(),
    });

    let replayed = replay(&state);
    let record = replayed.ceo_bootstraps.get("op-1").unwrap();
    assert_eq!(record.phase, "launched");
    assert_eq!(record.pane_id.as_deref(), Some("%7"));
    assert_eq!(record.updated_ms, 20);
    let obligation = replayed.reply_obligations.get("m-1").unwrap();
    assert_eq!(
        obligation.from_project.as_deref(),
        Some("/tmp/origin-project")
    );
}

#[test]
fn legacy_reply_obligation_without_from_project_decodes() {
    let legacy = serde_json::json!({
        "message_id": "m-1",
        "project": "/tmp/target-project",
        "from_worker_id": "ceo",
        "recipient_worker_id": "master",
        "policy": "required",
        "created_ms": 30
    });
    let obligation: ReplyObligation = serde_json::from_value(legacy).unwrap();
    assert!(obligation.from_project.is_none());

    let wire = serde_json::to_value(&obligation).unwrap();
    assert!(wire.get("from_project").is_none());

    let mut state = State::default();
    state.apply(&Event::ReplyObligationSet { obligation });
    assert!(state.reply_obligations["m-1"].from_project.is_none());
}

#[test]
fn ceo_events_do_not_change_business_task_responsibility() {
    let mut state = State::default();
    state.apply(&Event::ResponsibilityFenceSet {
        fence: ResponsibilityFence {
            operation_id: "op-close".into(),
            worker_id: "closing-peer".into(),
            project_scope: "/tmp/target-project".into(),
            app_scope: "tui-default".into(),
            binding_id: "binding-closing-peer".into(),
            endpoint_generation: 1,
            snapshot: ResponsibilitySnapshot::default(),
            state: "closing".into(),
            created_ms: 1,
        },
    });

    let events = [
        Event::CeoGrantSet {
            grant: Some(grant("/tmp/ws", 10)),
        },
        Event::CeoBootstrapSet {
            record: bootstrap("op-1", "prepared"),
        },
    ];
    assert_eq!(responsibility_preflight(&state, &events), Ok(()));
}
