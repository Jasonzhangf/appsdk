use super::super::binding_slot::{BindingControlSet, BindingValue, IdentityBindingKey};
use super::super::global_state::RuntimeBinding;
use super::super::name_state::NameOccupancy;
use super::super::state::{Event, Message, ReplyObligation, State, WorkerRec};
use super::{
    build_obligation, current_obligation_recipient_can_read, resolve_scope, stable_name,
    validate_reply,
};
use crate::identity::{AgentId, AppServerId, BindingId, RuntimeId, SessionId};
use crate::proto::{ReplyPolicy, SelectedTransport, TransportKind};
use crate::scope::ProjectScopeId;

fn obligation(
    message_id: &str,
    project: &str,
    from: &str,
    recipient: &str,
    recipient_name: Option<&str>,
) -> ReplyObligation {
    ReplyObligation {
        message_id: message_id.into(),
        project: project.into(),
        from_project: None,
        from_ceo: false,
        from_worker_id: from.into(),
        from_name: None,
        recipient_worker_id: recipient.into(),
        recipient_name: recipient_name.map(str::to_owned),
        policy: ReplyPolicy::Required,
        created_ms: 10,
        reminded_idle_episode: false,
    }
}

fn worker(id: &str, cwd: &str) -> WorkerRec {
    WorkerRec {
        id: id.into(),
        token: "t".into(),
        cwd: cwd.into(),
        registered_ms: 0,
        transport: None,
    }
}

fn message(id: &str, from: &str, to: &str) -> Message {
    Message {
        id: id.into(),
        from: from.into(),
        to: to.into(),
        mtype: "request".into(),
        subject: Some("do the work".into()),
        body: "please".into(),
        in_reply_to: None,
        created_ms: 10,
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    }
}

fn control_with_name(project: &str, app: &str, name: &str, agent: &str) -> BindingControlSet {
    let project = ProjectScopeId::new(project).unwrap();
    let app = AppServerId::new(app).unwrap();
    let key = IdentityBindingKey::new(project.clone(), app.clone(), name);
    let binding = RuntimeBinding::new_with_session(
        project,
        app,
        AgentId::new(agent).unwrap(),
        RuntimeId::new("runtime").unwrap(),
        BindingId::new("binding").unwrap(),
        Some(SessionId::new("session").unwrap()),
        None,
    )
    .unwrap();
    let transport = SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/collab-test.sock".into()),
        namespace: Some("test".into()),
        session_id: Some("session".into()),
        thread_id: None,
        tmux_endpoint: None,
        rex_endpoint: None,
        capabilities: Vec::new(),
        self_check: "ok".into(),
    };
    let mut control = BindingControlSet::new();
    control
        .commit(&key, BindingValue::new(binding, transport), None)
        .unwrap();
    control
}

#[test]
fn current_name_can_read_only_an_open_obligation_in_the_exact_route() {
    let project = "/tmp/collab-reply-project";
    let app = "appserver-cli";
    let project_scope = ProjectScopeId::new(project).unwrap();
    let app_scope = AppServerId::new(app).unwrap();
    let control = control_with_name(project, app, "Alice", "alice-new");
    let open = obligation("m1", project, "requester", "alice-old", Some("Alice"));

    assert!(current_obligation_recipient_can_read(
        &control,
        &project_scope,
        &app_scope,
        "m1",
        Some(&open),
        "alice-new",
    )
    .unwrap());
    assert!(!current_obligation_recipient_can_read(
        &control,
        &project_scope,
        &AppServerId::new("other-app").unwrap(),
        "m1",
        Some(&open),
        "alice-new",
    )
    .unwrap());
    assert!(!current_obligation_recipient_can_read(
        &control,
        &ProjectScopeId::new("/tmp/other-project").unwrap(),
        &app_scope,
        "m1",
        Some(&open),
        "alice-new",
    )
    .unwrap());
    assert!(!current_obligation_recipient_can_read(
        &control,
        &project_scope,
        &app_scope,
        "m1",
        Some(&open),
        "alice-old",
    )
    .unwrap());
    assert!(!current_obligation_recipient_can_read(
        &control,
        &project_scope,
        &app_scope,
        "m1",
        None,
        "alice-new",
    )
    .unwrap());
}

#[test]
fn set_remind_clear_round_trip_and_view_fields() {
    let mut state = State::default();
    state.apply(&Event::Sent {
        msg: message("m1", "requester", "peer"),
    });
    state.apply(&Event::ReplyObligationSet {
        obligation: obligation("m1", "proj", "requester", "peer", None),
    });

    let pending = state.pending_reply_obligations("peer");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].message_id, "m1");
    let view = state.reply_obligation_view("peer");
    assert_eq!(view[0]["message_id"], "m1");
    assert_eq!(view[0]["from"], "requester");
    assert_eq!(view[0]["policy"], "required");
    assert_eq!(view[0]["reminded_idle_episode"], false);

    state.apply(&Event::ReplyObligationReminded {
        message_id: "m1".into(),
        reminded: true,
    });
    assert!(state.pending_reply_obligations("peer")[0].reminded_idle_episode);

    // Consuming and acknowledging the original message are not replies; they
    // must leave the obligation pending.
    state.apply(&Event::Delivered {
        ids: vec!["m1".into()],
    });
    state.apply(&Event::Acked {
        ids: vec!["m1".into()],
    });
    assert_eq!(state.pending_reply_obligations("peer").len(), 1);

    // Only the explicit cleared event removes it.
    state.apply(&Event::ReplyObligationCleared {
        message_id: "m1".into(),
    });
    assert!(state.pending_reply_obligations("peer").is_empty());
    assert!(state.reply_obligation_view("peer").is_empty());
}

#[test]
fn current_state_preserves_unsettled_and_reminded_obligation() {
    let mut state = State::default();
    state.apply(&Event::ReplyObligationSet {
        obligation: obligation("m1", "proj", "requester", "peer", None),
    });
    state.apply(&Event::ReplyObligationReminded {
        message_id: "m1".into(),
        reminded: true,
    });

    let replayed: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    let pending = replayed.pending_reply_obligations("peer");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].message_id, "m1");
    assert!(pending[0].reminded_idle_episode);
}

#[test]
fn cleared_obligation_does_not_return_from_current_state() {
    let mut state = State::default();
    state.apply(&Event::ReplyObligationSet {
        obligation: obligation("m1", "proj", "requester", "peer", None),
    });
    state.apply(&Event::ReplyObligationCleared {
        message_id: "m1".into(),
    });

    let replayed: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert!(replayed.pending_reply_obligations("peer").is_empty());
}

#[test]
fn name_handoff_rebinds_current_carrier_and_resets_reminder() {
    let mut state = State::default();
    state.apply(&Event::ReplyObligationSet {
        obligation: obligation("m1", "proj", "requester", "peer-old", Some("peer")),
    });
    state.apply(&Event::ReplyObligationReminded {
        message_id: "m1".into(),
        reminded: true,
    });

    state.apply(&Event::NameResolutionCommitted {
        project_scope: "proj".into(),
        occupancy: NameOccupancy {
            name: "peer".into(),
            worker_id: "peer-new".into(),
            session_id: "session-new".into(),
            occupancy_revision: 2,
            updated_ms: 20,
            pane_anchor: None,
        },
        assignments: Vec::new(),
    });

    // The new session carrier owns the obligation and can reply; the old
    // session no longer can.
    let pending = state.pending_reply_obligations("peer-new");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].recipient_worker_id, "peer-new");
    assert!(!pending[0].reminded_idle_episode);
    assert!(state.pending_reply_obligations("peer-old").is_empty());

    // An unchanged resolution must not reopen the reminder window.
    state.apply(&Event::ReplyObligationReminded {
        message_id: "m1".into(),
        reminded: true,
    });
    state.apply(&Event::NameResolutionCommitted {
        project_scope: "proj".into(),
        occupancy: NameOccupancy {
            name: "peer".into(),
            worker_id: "peer-new".into(),
            session_id: "session-new".into(),
            occupancy_revision: 3,
            updated_ms: 30,
            pane_anchor: None,
        },
        assignments: Vec::new(),
    });
    assert!(state.pending_reply_obligations("peer-new")[0].reminded_idle_episode);
}

#[test]
fn project_reset_clears_only_its_own_scope() {
    let mut state = State::default();
    state.apply(&Event::ReplyObligationSet {
        obligation: obligation("m-a", "proj-a", "a", "b", None),
    });
    state.apply(&Event::ReplyObligationSet {
        obligation: obligation("m-b", "proj-b", "a", "b", None),
    });

    super::super::project_reset::clear_project(&mut state, "proj-a", true, &[]);

    assert!(!state.reply_obligations.contains_key("m-a"));
    assert!(state.reply_obligations.contains_key("m-b"));
}

#[test]
fn linked_reply_must_come_from_recipient_to_sender() {
    let control = crate::server::binding_slot::BindingControlSet::default();
    let app = crate::identity::AppServerId::new("appserver-cli").unwrap();
    let pending = obligation("m1", "proj", "requester", "peer", None);

    // Right recipient, right target.
    assert!(validate_reply(&control, &app, &pending, "peer", "requester").is_ok());
    // Third party cannot answer for the recipient.
    assert!(validate_reply(&control, &app, &pending, "other", "requester").is_err());
    // The recipient cannot answer a different target.
    assert!(validate_reply(&control, &app, &pending, "peer", "other").is_err());
}

#[test]
fn scope_and_name_helpers_prefer_declared_scope_and_stable_name() {
    let (server, root) = crate::server::peer_tests::test_server();
    let registered = crate::server::peer_tests::register_synthetic_peer(&server, "peer", "peer");
    assert!(registered.ok, "{registered:?}");
    let control =
        crate::server::binding_store::BindingControlStore::new(server.host_paths.state_root())
            .load()
            .unwrap();
    let binding = &control
        .active_values()
        .find(|value| value.binding.agent_id.as_str() == "peer")
        .unwrap()
        .binding;
    let name = stable_name(
        &control,
        &binding.project_scope,
        &binding.app_scope_id,
        "peer",
    )
    .unwrap();
    assert_eq!(name.as_deref(), Some("Alice"));
    let state = server.state.lock().unwrap();
    assert_eq!(
        resolve_scope(&state, "peer", Some("declared")).as_deref(),
        Some("declared")
    );

    let built = build_obligation(
        name,
        "m1".into(),
        "declared".into(),
        "peer".into(),
        "requester".into(),
        None,
        ReplyPolicy::OnCompletion,
    );
    assert_eq!(built.from_name.as_deref(), Some("Alice"));
    assert_eq!(built.policy, ReplyPolicy::OnCompletion);
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
