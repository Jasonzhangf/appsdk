use super::binding_slot::{
    BindingControlSet, BindingValue, CommitOutcome, IdentityBindingKey, IdentityBindingSlot,
    IdentityStatus, PendingEffect, PendingEffectKind, SlotError,
};
use crate::identity::{AgentId, AppServerId, BindingId, NativeThreadId, RuntimeId, SessionId};
use crate::proto::{IdentityFacts, RexCandidate, RexEndpoint, SelectedTransport, TransportKind};
use crate::scope::ProjectScopeId;
use crate::server::global_state::RuntimeBinding;

fn key(name: &str) -> IdentityBindingKey {
    key_in("/tmp/collab-binding-slot", name)
}

fn key_in(project: &str, name: &str) -> IdentityBindingKey {
    let name = match name {
        "alice" => "Alice",
        "bob" => "Bob",
        name => name,
    };
    IdentityBindingKey::new(
        ProjectScopeId::new(project).unwrap(),
        AppServerId::new("appserver-cli").unwrap(),
        name,
    )
}

fn value(key: &IdentityBindingKey, agent: &str, runtime: &str, endpoint: &str) -> BindingValue {
    let session_id = SessionId::new(format!("{runtime}-session")).unwrap();
    let native_thread_id = NativeThreadId::new(format!("{runtime}-thread")).unwrap();
    let binding = RuntimeBinding::new_with_session(
        key.project_scope.clone(),
        key.app_scope_id.clone(),
        AgentId::new(agent).unwrap(),
        RuntimeId::new(runtime).unwrap(),
        BindingId::new(format!("binding-{runtime}")).unwrap(),
        Some(session_id.clone()),
        Some(native_thread_id.clone()),
    )
    .unwrap();
    BindingValue::new(
        binding,
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some(endpoint.to_owned()),
            namespace: Some("test".to_owned()),
            session_id: Some(session_id.as_str().to_owned()),
            thread_id: Some(native_thread_id.as_str().to_owned()),
            tmux_endpoint: None,
            rex_endpoint: None,
            capabilities: vec!["test".to_owned()],
            self_check: "ok".to_owned(),
        },
    )
}

fn rex_value(
    key: &IdentityBindingKey,
    agent: &str,
    runtime: &str,
    session: &str,
    block: &str,
) -> BindingValue {
    rex_value_on_server(
        key,
        agent,
        runtime,
        session,
        block,
        "unix:///tmp/rex-test.sock",
    )
}

fn rex_value_on_server(
    key: &IdentityBindingKey,
    agent: &str,
    runtime: &str,
    session: &str,
    block: &str,
    server: &str,
) -> BindingValue {
    let binding = RuntimeBinding::new_with_session(
        key.project_scope.clone(),
        key.app_scope_id.clone(),
        AgentId::new(agent).unwrap(),
        RuntimeId::new(runtime).unwrap(),
        BindingId::new(format!("binding-{runtime}")).unwrap(),
        Some(SessionId::new(session).unwrap()),
        None,
    )
    .unwrap();
    BindingValue::new(
        binding,
        SelectedTransport {
            kind: TransportKind::Rex,
            endpoint: None,
            namespace: None,
            session_id: Some(session.into()),
            thread_id: None,
            tmux_endpoint: None,
            rex_endpoint: Some(RexEndpoint {
                server: server.into(),
                session_id: "rex-session".into(),
                block_id: block.into(),
                host_id: Some("host".into()),
                instance_id: "instance".into(),
                cwd: key.project_scope.as_str().into(),
            }),
            capabilities: vec!["send_message_to_block".into()],
            self_check: "ok".into(),
        },
    )
}

fn tmux_value_on_socket(
    key: &IdentityBindingKey,
    agent: &str,
    runtime: &str,
    socket: &str,
    session: &str,
    pane: &str,
) -> BindingValue {
    let mut value = value(key, agent, runtime, "endpoint");
    let endpoint = crate::proto::TmuxEndpoint {
        socket_path: socket.into(),
        server_pid: 11,
        tmux_session_id: session.into(),
        pane_id: pane.into(),
        pane_pid: 22,
        codex_session_id: value
            .binding
            .session_id
            .as_ref()
            .map(|id| id.as_str().into()),
        codex_thread_id: None,
    };
    value.binding.tmux_endpoint = Some(endpoint.clone());
    value.transport = SelectedTransport {
        kind: TransportKind::Tmux,
        endpoint: Some(socket.into()),
        namespace: Some(session.into()),
        session_id: value
            .binding
            .session_id
            .as_ref()
            .map(|id| id.as_str().into()),
        thread_id: None,
        tmux_endpoint: Some(endpoint),
        rex_endpoint: None,
        capabilities: vec!["send_message_to_pane".into()],
        self_check: "ok".into(),
    };
    value
}

fn committed_slot(key: IdentityBindingKey, value: BindingValue) -> IdentityBindingSlot {
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(&key, value, Some(0)).unwrap();
    slot
}

fn current_effect(
    key: &IdentityBindingKey,
    effect_id: &str,
    kind: PendingEffectKind,
    target_worker_id: &str,
    target_session_id: &str,
) -> PendingEffect {
    PendingEffect {
        kind,
        role: None,
        effect_id: effect_id.into(),
        operation_id: format!("operation-{effect_id}"),
        project_scope: key.project_scope.clone(),
        app_scope_id: key.app_scope_id.clone(),
        name: key.name.clone(),
        previous_worker_id: None,
        previous_session_id: None,
        target_worker_id: target_worker_id.into(),
        target_session_id: target_session_id.into(),
        occupancy_revision: 1,
        source_names: Vec::new(),
        moves_existing_names: false,
        pane_anchor: None,
        created_ms: 1,
    }
}

#[test]
fn first_binding_becomes_current_without_previous() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());

    assert_eq!(
        slot.commit(
            &key,
            value(&key, "alice", "runtime-a", "endpoint-a"),
            Some(0)
        )
        .unwrap(),
        CommitOutcome::Committed { generation: 1 }
    );

    assert_eq!(slot.identity_status(), IdentityStatus::Active);
    assert_eq!(slot.generation(), 1);
    assert!(slot.reference_previous().is_none());
    slot.validate().unwrap();
}

#[test]
fn current_lookup_selects_unique_session_before_an_occupied_rex_block() {
    let alice = key("alice");
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            value(&alice, "alice", "runtime-alice", "endpoint-alice"),
            None,
        )
        .unwrap();
    control
        .commit(
            &bob,
            rex_value(&bob, "bob", "runtime-bob", "bob-session", "block-bob"),
            None,
        )
        .unwrap();
    let facts = IdentityFacts {
        session_id: Some("runtime-alice-session".into()),
        rex: Some(RexCandidate {
            server: "unix:///tmp/rex-test.sock".into(),
            session_id: "rex-session".into(),
            block_id: "block-bob".into(),
            cwd: "/tmp/collab-binding-slot".into(),
        }),
        ..Default::default()
    };

    let selected = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap()
        .expect("the unique agent session selects the caller for a later block replacement");
    assert_eq!(selected.identity_key, alice);
}

#[test]
fn current_rex_anchor_disambiguates_a_reused_agent_session() {
    let alice = key("alice");
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            rex_value(
                &alice,
                "alice",
                "runtime-alice",
                "reused-agent-session",
                "block-alice",
            ),
            None,
        )
        .unwrap();
    control
        .commit(
            &bob,
            rex_value(
                &bob,
                "bob",
                "runtime-bob",
                "reused-agent-session",
                "block-bob",
            ),
            None,
        )
        .unwrap();

    let facts = IdentityFacts {
        session_id: Some("reused-agent-session".into()),
        rex: Some(RexCandidate {
            server: "unix:///tmp/rex-test.sock".into(),
            session_id: "rex-session".into(),
            block_id: "block-bob".into(),
            cwd: "/tmp/collab-binding-slot".into(),
        }),
        ..Default::default()
    };

    let selected = control
        .lookup(&bob.project_scope, &bob.app_scope_id, &facts, None)
        .unwrap()
        .expect("the exact current Rex block disambiguates duplicate session history");
    assert_eq!(selected.identity_key, bob);
}

#[test]
fn current_lookup_rejects_reused_agent_session_without_matching_anchor() {
    let alice = key("alice");
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            rex_value(
                &alice,
                "alice",
                "runtime-alice",
                "reused-agent-session",
                "block-alice",
            ),
            None,
        )
        .unwrap();
    control
        .commit(
            &bob,
            rex_value(
                &bob,
                "bob",
                "runtime-bob",
                "reused-agent-session",
                "block-bob",
            ),
            None,
        )
        .unwrap();
    let before = serde_json::to_value(&control).unwrap();
    let facts = IdentityFacts {
        session_id: Some("reused-agent-session".into()),
        ..Default::default()
    };

    let error = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap_err();
    assert!(error.starts_with("IDENTITY_MAP_AMBIGUOUS:"), "{error}");
    assert_eq!(serde_json::to_value(&control).unwrap(), before);
}

#[test]
fn current_lookup_rejects_anchor_outside_reused_session_matches() {
    let alice = key("alice");
    let bob = key("bob");
    let carol = key("Carol");
    let mut control = BindingControlSet::new();
    for (key, agent, runtime, block) in [
        (&alice, "alice", "runtime-alice", "block-alice"),
        (&bob, "bob", "runtime-bob", "block-bob"),
    ] {
        control
            .commit(
                key,
                rex_value(key, agent, runtime, "reused-agent-session", block),
                None,
            )
            .unwrap();
    }
    control
        .commit(
            &carol,
            rex_value(
                &carol,
                "carol",
                "runtime-carol",
                "carol-session",
                "block-carol",
            ),
            None,
        )
        .unwrap();
    let before = serde_json::to_value(&control).unwrap();
    let facts = IdentityFacts {
        session_id: Some("reused-agent-session".into()),
        rex: Some(RexCandidate {
            server: "unix:///tmp/rex-test.sock".into(),
            session_id: "rex-session".into(),
            block_id: "block-carol".into(),
            cwd: "/tmp/collab-binding-slot".into(),
        }),
        ..Default::default()
    };

    let error = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap_err();
    assert!(error.starts_with("IDENTITY_ANCHOR_CONFLICT:"), "{error}");
    assert_eq!(serde_json::to_value(&control).unwrap(), before);
}

#[test]
fn current_lookup_uses_unique_session_when_a_rex_block_has_seeded_duplicate_currents() {
    let alice = key("alice");
    let bob = key("bob");
    let alice_value = rex_value(
        &alice,
        "alice",
        "runtime-alice",
        "alice-session",
        "shared-block",
    );
    let bob_value = rex_value(&bob, "bob", "runtime-bob", "bob-session", "shared-block");
    let control = BindingControlSet::from_parts(
        vec![
            committed_slot(alice.clone(), alice_value),
            committed_slot(bob, bob_value),
        ],
        Vec::new(),
    );
    let facts = IdentityFacts {
        session_id: Some("alice-session".into()),
        rex: Some(RexCandidate {
            server: "unix:///tmp/rex-test.sock".into(),
            session_id: "rex-session".into(),
            block_id: "shared-block".into(),
            cwd: "/tmp/collab-binding-slot".into(),
        }),
        ..Default::default()
    };

    let selected = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap()
        .expect(
            "the known session repairs the seeded duplicate without choosing an arbitrary owner",
        );
    assert_eq!(selected.identity_key, alice);
}

#[test]
fn current_lookup_matches_the_full_rex_server_and_block() {
    let alice = key("alice");
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            rex_value_on_server(
                &alice,
                "alice",
                "runtime-alice",
                "alice-session",
                "shared-block",
                "unix:///tmp/rex-a.sock",
            ),
            None,
        )
        .unwrap();
    control
        .commit(
            &bob,
            rex_value_on_server(
                &bob,
                "bob",
                "runtime-bob",
                "bob-session",
                "shared-block",
                "unix:///tmp/rex-b.sock",
            ),
            None,
        )
        .unwrap();
    let facts = IdentityFacts {
        session_id: Some("alice-session".into()),
        rex: Some(RexCandidate {
            server: "unix:///tmp/rex-a.sock".into(),
            session_id: "rex-session".into(),
            block_id: "shared-block".into(),
            cwd: "/tmp/collab-binding-slot".into(),
        }),
        ..Default::default()
    };

    let selected = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap()
        .expect("the full server scope selects only the matching principal");
    assert_eq!(selected.identity_key, alice);
}

#[test]
fn current_lookup_does_not_match_same_tmux_session_id_on_a_different_socket() {
    let alice = key("alice");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            tmux_value_on_socket(
                &alice,
                "alice",
                "runtime-alice",
                "/tmp/tmux-a.sock",
                "$1",
                "%1",
            ),
            None,
        )
        .unwrap();
    let facts = IdentityFacts {
        tmux: Some(crate::proto::TmuxCandidate {
            endpoint: crate::proto::TmuxEndpoint {
                socket_path: "/tmp/tmux-b.sock".into(),
                server_pid: 12,
                tmux_session_id: "$1".into(),
                pane_id: "%1".into(),
                pane_pid: 23,
                codex_session_id: None,
                codex_thread_id: None,
            },
            cwd: alice.project_scope.as_str().into(),
        }),
        ..Default::default()
    };

    assert!(
        control
            .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
            .unwrap()
            .is_none(),
        "a tmux session id is local to its socket and cannot recover another socket's identity"
    );
}

#[test]
fn current_lookup_keeps_same_tmux_session_id_on_the_same_socket() {
    let alice = key("alice");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            tmux_value_on_socket(
                &alice,
                "alice",
                "runtime-alice",
                "/tmp/tmux-a.sock",
                "$1",
                "%1",
            ),
            None,
        )
        .unwrap();
    let facts = IdentityFacts {
        tmux: Some(crate::proto::TmuxCandidate {
            endpoint: crate::proto::TmuxEndpoint {
                socket_path: "/tmp/tmux-a.sock".into(),
                server_pid: 12,
                tmux_session_id: "$1".into(),
                pane_id: "%9".into(),
                pane_pid: 23,
                codex_session_id: None,
                codex_thread_id: None,
            },
            cwd: alice.project_scope.as_str().into(),
        }),
        ..Default::default()
    };

    let selected = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap()
        .expect("the same socket and tmux session preserve the identity anchor");
    assert_eq!(selected.identity_key, alice);
}

#[test]
fn current_lookup_allows_same_subject_to_move_to_a_new_rex_block() {
    let alice = key("alice");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice,
            rex_value(
                &alice,
                "alice",
                "runtime-alice",
                "agent-session",
                "old-block",
            ),
            None,
        )
        .unwrap();
    let facts = IdentityFacts {
        session_id: Some("agent-session".into()),
        rex: Some(RexCandidate {
            server: "unix:///tmp/rex-test.sock".into(),
            session_id: "rex-session".into(),
            block_id: "new-block".into(),
            cwd: "/tmp/collab-binding-slot".into(),
        }),
        ..Default::default()
    };

    assert_eq!(
        control
            .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
            .unwrap()
            .unwrap()
            .identity_key,
        alice
    );
}

#[test]
fn current_lookup_rejects_two_matching_names_without_mutation_or_cross_scope_match() {
    let alice = key("alice");
    let bob = key("bob");
    let mut control = BindingControlSet::new();
    control
        .commit(&alice, value(&alice, "carrier", "shared", "endpoint"), None)
        .unwrap();
    let facts = crate::proto::IdentityFacts {
        session_id: Some("shared-session".into()),
        ..Default::default()
    };
    assert_eq!(
        control
            .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
            .unwrap()
            .unwrap()
            .identity_key,
        alice,
    );
    control
        .commit(&bob, value(&bob, "carrier", "shared", "endpoint"), None)
        .unwrap();
    let before = serde_json::to_value(&control).unwrap();
    let error = control
        .lookup(&alice.project_scope, &alice.app_scope_id, &facts, None)
        .unwrap_err();
    assert!(error.starts_with("IDENTITY_MAP_AMBIGUOUS:"), "{error}");
    assert_eq!(serde_json::to_value(&control).unwrap(), before);
    assert!(control
        .lookup(
            &ProjectScopeId::new("/tmp/unrelated-project").unwrap(),
            &alice.app_scope_id,
            &facts,
            None,
        )
        .unwrap()
        .is_none());
    assert!(control
        .lookup(
            &alice.project_scope,
            &AppServerId::new("other-app").unwrap(),
            &facts,
            None,
        )
        .unwrap()
        .is_none());
}

#[test]
fn three_distinct_bindings_keep_only_current_and_previous() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    slot.commit(
        &key,
        value(&key, "alice", "runtime-b", "endpoint-b"),
        Some(1),
    )
    .unwrap();
    slot.commit(
        &key,
        value(&key, "alice", "runtime-c", "endpoint-c"),
        Some(2),
    )
    .unwrap();

    let current = slot.active_binding().expect("current binding");
    let previous = slot.reference_previous().expect("previous binding");
    assert_eq!(current.binding.runtime_id.as_str(), "runtime-c");
    assert_eq!(previous.binding.runtime_id.as_str(), "runtime-b");
    assert_eq!(slot.generation(), 3);
    slot.validate().unwrap();
}

#[test]
fn repeated_binding_fact_does_not_roll_or_advance_generation() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();

    let mut repeated = value(&key, "alice", "runtime-a", "endpoint-a");
    repeated.transport.capabilities = vec!["changed".to_owned()];
    repeated.transport.self_check = "changed self check".to_owned();

    assert_eq!(
        slot.commit(&key, repeated, Some(1)).unwrap(),
        CommitOutcome::Unchanged { generation: 1 }
    );
    assert_eq!(slot.generation(), 1);
    assert!(slot.reference_previous().is_none());
    slot.validate().unwrap();
}

#[test]
fn stable_name_keeps_one_slot_across_session_derived_agents() {
    let key = key("alice");
    let mut set = BindingControlSet::new();

    // S1 and S2 carry different session-derived agent ids and transports but
    // the same stable name. They must collapse into one logical slot with the
    // newest carrier as current and the previous carrier retained once.
    set.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    assert_eq!(
        set.commit(
            &key,
            value(&key, "carrier-s2", "runtime-b", "endpoint-b"),
            Some(1),
        )
        .unwrap(),
        CommitOutcome::Committed { generation: 2 }
    );

    assert_eq!(set.slots().count(), 1);
    let slot = set.slot(&key).unwrap();
    assert_eq!(slot.generation(), 2);
    assert_eq!(
        slot.active_binding().unwrap().binding.agent_id.as_str(),
        "carrier-s2"
    );
    assert_eq!(
        slot.reference_previous().unwrap().binding.agent_id.as_str(),
        "alice"
    );
    set.validate().unwrap();
}

#[test]
fn new_session_replaces_the_current_carrier_without_an_endpoint_counter() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "carrier-s1", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    slot.commit(
        &key,
        value(&key, "carrier-s2", "runtime-b", "endpoint-b"),
        Some(1),
    )
    .unwrap();
    assert_eq!(slot.generation(), 2);
    assert_eq!(
        slot.active_binding().unwrap().binding.agent_id.as_str(),
        "carrier-s2"
    );
    assert_eq!(
        slot.reference_previous().unwrap().binding.agent_id.as_str(),
        "carrier-s1"
    );
    slot.validate().unwrap();
}

#[test]
fn returning_to_a_previous_address_rolls_instead_of_swapping() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    slot.commit(
        &key,
        value(&key, "alice", "runtime-b", "endpoint-b"),
        Some(1),
    )
    .unwrap();
    slot.commit(
        &key,
        value(&key, "alice", "runtime-c", "endpoint-a"),
        Some(2),
    )
    .unwrap();

    assert_eq!(
        slot.active_binding().unwrap().binding.runtime_id.as_str(),
        "runtime-c"
    );
    assert_eq!(
        slot.reference_previous()
            .unwrap()
            .binding
            .runtime_id
            .as_str(),
        "runtime-b"
    );
    assert_eq!(slot.generation(), 3);
    slot.validate().unwrap();
}

#[test]
fn stale_expected_generation_is_rejected_without_mutation() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();

    assert_eq!(
        slot.commit(
            &key,
            value(&key, "alice", "runtime-b", "endpoint-b"),
            Some(0)
        ),
        Err(SlotError::GenerationConflict {
            expected: 1,
            observed: 0
        })
    );
    assert_eq!(slot.generation(), 1);
    assert_eq!(
        slot.active_binding().unwrap().binding.runtime_id.as_str(),
        "runtime-a"
    );
    assert!(slot.reference_previous().is_none());
}

#[test]
fn unregister_moves_current_to_previous_and_is_idempotent() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();

    assert_eq!(
        slot.unregister(Some(1)).unwrap(),
        CommitOutcome::Committed { generation: 2 }
    );
    assert_eq!(slot.identity_status(), IdentityStatus::Unregistered);
    assert!(slot.active_binding().is_none());
    assert_eq!(
        slot.reference_previous()
            .unwrap()
            .binding
            .runtime_id
            .as_str(),
        "runtime-a"
    );

    assert_eq!(
        slot.unregister(Some(2)).unwrap(),
        CommitOutcome::Unchanged { generation: 2 }
    );
    assert_eq!(slot.generation(), 2);
    assert!(slot.reference_previous().is_some());
    slot.validate().unwrap();
}

#[test]
fn rebuild_keeps_the_registration_before_close_as_previous() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    slot.unregister(Some(1)).unwrap();
    slot.commit(
        &key,
        value(&key, "carrier-s2", "runtime-b", "endpoint-b"),
        Some(2),
    )
    .unwrap();

    assert_eq!(slot.identity_status(), IdentityStatus::Active);
    assert_eq!(
        slot.active_binding().unwrap().binding.runtime_id.as_str(),
        "runtime-b"
    );
    assert_eq!(
        slot.reference_previous()
            .unwrap()
            .binding
            .runtime_id
            .as_str(),
        "runtime-a"
    );
    assert_eq!(slot.generation(), 3);
    slot.validate().unwrap();
}

#[test]
fn rebuild_new_carrier_after_retirement_keeps_last_binding() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    slot.unregister(Some(1)).unwrap();
    assert_eq!(
        slot.commit(
            &key,
            value(&key, "carrier-s2", "runtime-b", "endpoint-b"),
            Some(2)
        )
        .unwrap(),
        CommitOutcome::Committed { generation: 3 }
    );
    assert_eq!(
        slot.reference_previous().unwrap().binding.agent_id.as_str(),
        "alice"
    );
    slot.validate().unwrap();
}

#[test]
fn a_value_from_a_different_scope_cannot_be_committed_into_the_slot() {
    let key = key_in("/tmp/collab-binding-slot", "alice");
    let other = key_in("/tmp/collab-binding-other", "alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    let before = slot.clone();

    assert!(matches!(
        slot.commit(
            &key,
            value(&other, "alice", "runtime-b", "endpoint-b"),
            Some(0)
        ),
        Err(SlotError::KeyMismatch { .. })
    ));
    assert_eq!(slot, before);
}

#[test]
fn slot_round_trips_through_json() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    let encoded = serde_json::to_string(&slot).unwrap();
    let decoded: IdentityBindingSlot = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, slot);
}

#[test]
fn control_set_keeps_identity_slots_independent() {
    let alice = key("alice");
    let bob = key("bob");
    let mut set = BindingControlSet::new();
    set.commit(
        &alice,
        value(&alice, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    set.commit(&bob, value(&bob, "bob", "runtime-b", "endpoint-b"), Some(0))
        .unwrap();

    let active = set.active_values().collect::<Vec<_>>();
    assert_eq!(active.len(), 2);
    set.unregister(&alice, Some(1)).unwrap();

    let active = set.active_values().collect::<Vec<_>>();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].binding.agent_id.as_str(), "bob");
    assert_eq!(
        set.slot(&alice)
            .unwrap()
            .reference_previous()
            .unwrap()
            .binding
            .agent_id
            .as_str(),
        "alice"
    );
    set.validate().unwrap();
}

#[test]
fn control_set_stale_update_does_not_clobber_a_sibling_identity() {
    let alice = key("alice");
    let bob = key("bob");
    let mut set = BindingControlSet::new();
    set.commit(
        &alice,
        value(&alice, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    set.commit(&bob, value(&bob, "bob", "runtime-b", "endpoint-b"), Some(0))
        .unwrap();

    assert!(matches!(
        set.commit(
            &alice,
            value(&alice, "alice", "runtime-a2", "endpoint-a2"),
            Some(0)
        ),
        Err(SlotError::GenerationConflict { .. })
    ));

    assert_eq!(
        set.slot(&alice)
            .unwrap()
            .active_binding()
            .unwrap()
            .binding
            .runtime_id
            .as_str(),
        "runtime-a"
    );
    assert_eq!(
        set.slot(&bob)
            .unwrap()
            .active_binding()
            .unwrap()
            .binding
            .runtime_id
            .as_str(),
        "runtime-b"
    );
    set.validate().unwrap();
}

#[test]
fn remove_scope_drops_only_the_requested_project() {
    let alice = key_in("/tmp/collab-binding-scope-a", "alice");
    let bob = key_in("/tmp/collab-binding-scope-b", "bob");
    let mut set = BindingControlSet::new();
    set.commit(
        &alice,
        value(&alice, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    set.commit(&bob, value(&bob, "bob", "runtime-b", "endpoint-b"), Some(0))
        .unwrap();

    let removed = set.remove_scope("/tmp/collab-binding-scope-a");
    assert_eq!(removed, 1);
    assert!(set.slot(&alice).is_none());
    assert!(set.slot(&bob).is_some());
    set.validate().unwrap();
}

#[test]
fn move_name_retires_the_source_slot_and_keeps_the_displaced_holder_as_previous() {
    let alice = key("alice");
    let bob = key("bob");
    let mut set = BindingControlSet::new();
    set.commit(
        &alice,
        value(&alice, "alice-s1", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    set.commit(
        &bob,
        value(&bob, "bob-s1", "runtime-b", "endpoint-b"),
        Some(0),
    )
    .unwrap();

    let moved = set
        .move_name(
            &alice,
            value(&alice, "bob-s2", "runtime-c", "endpoint-c"),
            &[bob.name.clone()],
            Some(1),
        )
        .unwrap();
    assert_eq!(moved.sources_retired, 1);
    assert_eq!(moved.target, CommitOutcome::Committed { generation: 2 });

    let alice_slot = set.slot(&alice).unwrap();
    assert_eq!(
        alice_slot
            .active_binding()
            .unwrap()
            .binding
            .agent_id
            .as_str(),
        "bob-s2"
    );
    assert_eq!(
        alice_slot
            .reference_previous()
            .unwrap()
            .binding
            .agent_id
            .as_str(),
        "alice-s1"
    );
    let bob_slot = set.slot(&bob).unwrap();
    assert_eq!(bob_slot.identity_status(), IdentityStatus::Unregistered);
    assert!(
        bob_slot.active_binding().is_none(),
        "the retired source slot must not expose a current binding"
    );
    assert_eq!(
        bob_slot
            .reference_previous()
            .unwrap()
            .binding
            .agent_id
            .as_str(),
        "bob-s1"
    );
    set.validate().unwrap();
}

#[test]
fn move_name_does_not_retire_the_target_when_source_equals_target() {
    let alice = key("alice");
    let mut set = BindingControlSet::new();
    set.commit(
        &alice,
        value(&alice, "alice-s1", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();

    let moved = set
        .move_name(
            &alice,
            value(&alice, "alice-s2", "runtime-b", "endpoint-b"),
            &[alice.name.clone()],
            Some(1),
        )
        .unwrap();
    assert_eq!(moved.sources_retired, 0);
    assert!(set.slot(&alice).is_some());
    set.validate().unwrap();
}

#[test]
fn rex_block_commit_retires_the_previous_current_and_persists_cleanup() {
    let alice = key("alice");
    let bob = key("bob");
    let mut set = BindingControlSet::new();
    set.commit(
        &alice,
        rex_value(&alice, "alice", "runtime-a", "alice-session", "block-a"),
        None,
    )
    .unwrap();
    set.commit(
        &bob,
        rex_value(&bob, "bob", "runtime-b", "bob-session", "block-b"),
        None,
    )
    .unwrap();

    let winner = rex_value(&alice, "alice", "runtime-c", "alice-session", "block-b");
    set.commit(&alice, winner.clone(), Some(1)).unwrap();

    assert_eq!(set.slot(&alice).unwrap().active_binding(), Some(&winner));
    let bob_slot = set.slot(&bob).unwrap();
    assert_eq!(bob_slot.identity_status(), IdentityStatus::Unregistered);
    assert_eq!(
        bob_slot
            .reference_previous()
            .unwrap()
            .transport
            .rex_endpoint
            .as_ref()
            .unwrap()
            .block_id,
        "block-b"
    );
    assert_eq!(
        set.active_values()
            .filter(|value| value.transport.kind == TransportKind::Rex
                && value
                    .transport
                    .rex_endpoint
                    .as_ref()
                    .is_some_and(|endpoint| endpoint.block_id == "block-b"))
            .count(),
        1
    );
    set.validate().unwrap();
}

#[test]
fn unchanged_rex_winner_still_retires_a_seeded_duplicate_current() {
    let alice = key("alice");
    let bob = key("bob");
    let winner = rex_value(
        &alice,
        "alice",
        "runtime-a",
        "alice-session",
        "shared-block",
    );
    let duplicate = rex_value(&bob, "bob", "runtime-b", "bob-session", "shared-block");
    let mut set = BindingControlSet::from_parts(
        vec![
            committed_slot(alice.clone(), winner.clone()),
            committed_slot(bob.clone(), duplicate),
        ],
        vec![
            current_effect(
                &alice,
                "alice-name",
                PendingEffectKind::Name,
                "alice",
                "alice-session",
            ),
            current_effect(
                &bob,
                "bob-register",
                PendingEffectKind::Register,
                "bob",
                "bob-session",
            ),
            current_effect(
                &bob,
                "bob-unrelated-name",
                PendingEffectKind::Name,
                "other-worker",
                "other-session",
            ),
        ],
    );

    assert_eq!(
        set.commit(&alice, winner.clone(), None).unwrap(),
        CommitOutcome::Unchanged { generation: 1 }
    );
    assert_eq!(set.slot(&alice).unwrap().active_binding(), Some(&winner));
    assert_eq!(
        set.slot(&bob).unwrap().identity_status(),
        IdentityStatus::Unregistered
    );
    assert!(set.pending_effect("alice-name").is_some());
    assert!(set.pending_effect("bob-register").is_none());
    assert!(set.pending_effect("bob-unrelated-name").is_some());
    set.validate().unwrap();
}

#[test]
fn stale_rex_generation_cannot_restore_an_old_block() {
    let alice = key("alice");
    let mut set = BindingControlSet::new();
    let old = rex_value(&alice, "alice", "runtime-a", "alice-session", "old-block");
    let new = rex_value(&alice, "alice", "runtime-b", "alice-session", "new-block");
    set.commit(&alice, old.clone(), Some(0)).unwrap();
    set.commit(&alice, new.clone(), Some(1)).unwrap();

    let error = set.commit(&alice, old, Some(1)).unwrap_err();
    assert!(
        matches!(
            error,
            SlotError::GenerationConflict {
                expected: 2,
                observed: 1
            }
        ),
        "{error}"
    );
    assert_eq!(set.slot(&alice).unwrap().active_binding(), Some(&new));
}

#[test]
fn rex_publication_rejects_an_incomplete_current_without_mutation() {
    let alice = key("alice");
    let mut set = BindingControlSet::new();
    let mut incomplete = rex_value(&alice, "alice", "runtime-a", "alice-session", "block-a");
    incomplete
        .transport
        .rex_endpoint
        .as_mut()
        .unwrap()
        .block_id
        .clear();

    let error = set.commit(&alice, incomplete, Some(0)).unwrap_err();
    assert!(
        matches!(error, SlotError::Invalid(ref message) if message.contains("requires a complete server, session and block")),
        "{error}"
    );
    assert_eq!(set, BindingControlSet::new());
}

#[test]
fn control_set_round_trips_and_rejects_duplicate_keys() {
    let alice = key("alice");
    let mut set = BindingControlSet::new();
    set.commit(
        &alice,
        value(&alice, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    let encoded = serde_json::to_string(&set).unwrap();
    let decoded: BindingControlSet = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, set);

    let single = serde_json::to_value(&IdentityBindingSlot::new(alice.clone())).unwrap();
    let duplicate: BindingControlSet = serde_json::from_value(serde_json::json!({
        "slots": [single, single],
    }))
    .unwrap();
    assert!(matches!(
        duplicate.validate(),
        Err(SlotError::Invariant(message)) if message.contains("duplicate identity key")
    ));
}
