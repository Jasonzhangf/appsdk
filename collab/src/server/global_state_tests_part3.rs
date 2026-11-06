#[test]
fn an_unregistered_project_incumbent_is_the_declared_non_resident_boundary() {
    let scope = project_scope();
    let mut state = GlobalState::default();
    let incumbent = route_at(&scope, "app-one", "runtime-non-resident", "binding-non-resident",
        "session-nr-old", "thread-nr-old");
    state.set_current_thread_route(incumbent).unwrap();
    let takeover = route_at(&scope, "app-one", "runtime-non-resident-new", "binding-non-resident",
        "session-nr-new", "thread-nr-new");
    state.set_current_thread_route(takeover.clone()).unwrap();
    assert_eq!(state.lookup_current_thread_route(
        takeover.session_id.as_ref().unwrap(), takeover.native_thread_id.as_ref().unwrap()), Some(&takeover));
    state.validate().unwrap();
}

#[test]
fn rebinding_an_address_after_tombstoning_drops_the_stale_tombstone() {
    let scope = project_scope();
    let mut state = GlobalState::default();
    state.register_project(registration(&scope, "app-one")).unwrap();
    let first = route_at(&scope, "app-one", "runtime-one", "binding-one", "session-first", "thread-first");
    let second = route_at(&scope, "app-one", "runtime-one", "binding-one", "session-second", "thread-second");
    state.bind_runtime(first.clone()).unwrap();
    state.set_current_thread_route(first.clone()).unwrap();
    state.bind_runtime(second.clone()).unwrap();
    state.set_current_thread_route(second).unwrap();
    state.bind_runtime(first.clone()).unwrap();
    state.set_current_thread_route(first.clone()).unwrap();
    assert!(state.lookup_current_thread_route_tombstone(
        first.session_id.as_ref().unwrap(), first.native_thread_id.as_ref().unwrap()).is_none());
    state.validate().unwrap();
}

#[test]
fn command_receipts_are_host_wide_and_idempotent() {
    let mut state = GlobalState::default();
    let first = state.record_command(CommandId::new("command-one").unwrap(),
        OperationId::new("operation-one").unwrap(), serde_json::json!({"ok":true})).unwrap();
    let before = state.version();
    let repeated = state.record_command(CommandId::new("command-one").unwrap(),
        OperationId::new("operation-one").unwrap(), serde_json::json!({"ok":true})).unwrap();
    assert_eq!(first, repeated);
    assert_eq!(state.version(), before);
    assert!(matches!(state.record_command(CommandId::new("command-one").unwrap(),
        OperationId::new("operation-two").unwrap(), Value::Null), Err(StateError::CommandIdReuse { .. })));
    state.validate().unwrap();
}

#[test]
fn stale_pane_from_a_previous_tmux_server_does_not_claim_the_reissued_pane() {
    let scope = project_scope();
    let mut state = GlobalState::default();
    state.register_project(registration(&scope, "app-one")).unwrap();
    let mut stale = binding(&scope, "app-one", "agent-one", "runtime-stale", "binding-stale");
    stale.tmux_endpoint = Some(crate::proto::TmuxEndpoint {
        socket_path: "/tmp/collab-pane-reuse.sock".into(), server_pid: 11,
        tmux_session_id: "$1".into(), pane_id: "%2".into(), pane_pid: 33,
        codex_session_id: None, codex_thread_id: None,
    });
    state.bind_runtime(stale.clone()).unwrap();
    state.set_current_thread_route(stale.clone()).unwrap();
    let observed = crate::proto::TmuxEndpoint {
        server_pid: 12, tmux_session_id: "$5".into(), pane_pid: 44,
        ..stale.tmux_endpoint.clone().unwrap()
    };
    assert!(state.tmux_pane_route_claimants(&observed).is_empty());
    assert_eq!(state.lookup_tmux_route(&observed), Some(&stale));
    state.validate().unwrap();
}

#[test]
fn malformed_retired_pane_binding_is_retired_without_a_tombstone() {
    let scope = project_scope();
    let mut state = GlobalState::default();
    state.register_project(registration(&scope, "app-one")).unwrap();
    let mut malformed = binding(&scope, "app-one", "agent-one", "runtime-malformed", "binding-malformed");
    malformed.tmux_endpoint = Some(crate::proto::TmuxEndpoint {
        socket_path: "/tmp/collab-malformed.sock".into(), server_pid: 11,
        tmux_session_id: "$1".into(), pane_id: "%1".into(), pane_pid: 22,
        codex_session_id: None, codex_thread_id: None,
    });
    assert!(malformed.session_id.is_none());
    assert!(malformed.native_thread_id.is_none());
    state.bind_runtime(malformed.clone()).unwrap();
    state.set_current_thread_route(malformed.clone()).unwrap();
    let replacement = route_at(&scope, "app-one", "runtime-malformed", "binding-malformed",
        "session-malformed", "thread-malformed");
    state.bind_runtime(replacement.clone()).unwrap();
    state.set_current_thread_route(replacement.clone()).unwrap();
    assert!(state.current_thread_routes.values().all(|live| live != &malformed));
    assert!(state.current_thread_route_tombstones.is_empty());
    assert_eq!(state.lookup_current_thread_route(
        replacement.session_id.as_ref().unwrap(), replacement.native_thread_id.as_ref().unwrap()), Some(&replacement));
    state.validate().unwrap();
}
