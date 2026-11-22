use super::binding_slot::{
    BindingControlSet, BindingValue, CommitOutcome, IdentityBindingKey, IdentityBindingSlot,
    IdentityStatus, SlotError,
};
use crate::identity::{AgentId, AppServerId, BindingId, NativeThreadId, RuntimeId, SessionId};
use crate::proto::{SelectedTransport, TransportKind};
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
fn current_and_previous_may_reference_the_same_route_fact() {
    let key = key("alice");
    let current = value(&key, "alice", "runtime-a", "endpoint-a");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(&key, current.clone(), Some(0)).unwrap();
    slot.previous = Some(current);

    slot.validate()
        .expect("previous is historical and does not compete with current");
    assert_eq!(slot.active_binding(), slot.reference_previous());
}

#[test]
fn generated_name_rejects_a_fixed_display_slot() {
    let key = key("peer-17");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "peer-17", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();
    slot.display_slot = Some(2);

    assert!(
        slot.validate().is_err(),
        "generated names have no fixed display slot"
    );
}

#[test]
fn fixed_pool_name_allows_a_legacy_null_display_slot() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.display_slot = None;

    slot.validate()
        .expect("historical fixed-name maps may have no display slot");
}

#[test]
fn fixed_pool_name_rejects_a_wrong_display_slot() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.display_slot = Some(3);

    assert!(slot.validate().is_err());
}

#[test]
fn identical_refresh_preserves_registration_number_without_advancing_generation() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    let mut persisted = value(&key, "alice", "runtime-a", "endpoint-a");
    persisted.registration_number = Some(4242);
    slot.commit(&key, persisted, Some(0)).unwrap();

    let mut refresh = value(&key, "alice", "runtime-a", "endpoint-a");
    refresh.transport.capabilities = vec!["rebuilt".to_owned()];
    assert_eq!(
        slot.commit(&key, refresh, Some(1)).unwrap(),
        CommitOutcome::Unchanged { generation: 1 }
    );
    let current = slot.active_binding().unwrap();
    assert_eq!(current.registration_number, Some(4242));
    assert_eq!(slot.generation(), 1);
    assert!(slot.reference_previous().is_none());
}

#[test]
fn same_identity_rebind_carries_registration_number_to_the_new_current() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    let mut persisted = value(&key, "alice", "runtime-a", "endpoint-a");
    persisted.registration_number = Some(4242);
    slot.commit(&key, persisted, Some(0)).unwrap();

    assert_eq!(
        slot.commit(
            &key,
            value(&key, "alice", "runtime-b", "endpoint-b"),
            Some(1)
        )
        .unwrap(),
        CommitOutcome::Committed { generation: 2 }
    );
    let current = slot.active_binding().unwrap();
    assert_eq!(current.binding.runtime_id.as_str(), "runtime-b");
    assert_eq!(current.registration_number, Some(4242));
    assert_eq!(
        slot.reference_previous().unwrap().registration_number,
        Some(4242)
    );
    slot.validate().unwrap();
}

#[test]
fn same_slot_registration_number_is_not_replaced_by_an_incoming_value() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    let mut persisted = value(&key, "alice", "runtime-a", "endpoint-a");
    persisted.registration_number = Some(4242);
    slot.commit(&key, persisted, Some(0)).unwrap();

    let mut replacement = value(&key, "alice", "runtime-b", "endpoint-b");
    replacement.registration_number = Some(7);
    slot.commit(&key, replacement, Some(1)).unwrap();
    assert_eq!(
        slot.active_binding().unwrap().registration_number,
        Some(4242),
        "the logical slot owns its persisted metadata"
    );
}

#[test]
fn registration_number_type_and_scope_errors_are_not_suppressed() {
    let key = key("alice");
    let value = serde_json::json!({
        "identity_key": key.clone(),
        "display_slot": 2,
        "identity_status": "active",
        "generation": 1,
        "current": {
            "binding": {
                "project_scope": "/tmp/collab-binding-slot",
                "app_scope_id": "appserver-cli",
                "agent_id": "alice",
                "runtime_id": "runtime-a",
                "binding_id": "binding-runtime-a"
            },
            "transport": {
                "kind": "appserver",
                "endpoint": "endpoint-a",
                "namespace": "test",
                "session_id": "runtime-a-session",
                "thread_id": "runtime-a-thread",
                "capabilities": ["test"],
                "self_check": "ok"
            },
            "tab_id": {
                "project_scope": "/tmp/collab-binding-slot",
                "app_scope_id": "appserver-cli"
            },
            "host": {"agent_kind": null, "host_type": "native", "profile": null},
            "invocation_cwd": "/tmp/collab-binding-slot",
            "physical_name": null,
            "layout_binding_id": null,
            "registration_number": "not-a-number"
        },
        "previous": null
    });
    let error = serde_json::from_value::<IdentityBindingSlot>(value)
        .expect_err("a string registration number must fail decode");
    assert!(error.to_string().contains("u64"), "{error}");
}

#[test]
fn new_slot_serializes_without_registration_number_but_with_numeric_display_slot() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value(&key, "alice", "runtime-a", "endpoint-a"),
        Some(0),
    )
    .unwrap();

    let value = serde_json::to_value(&slot).unwrap();
    assert_eq!(value["display_slot"], serde_json::json!(2));
    assert!(
        value["current"].get("registration_number").is_none(),
        "an absent registration number must stay absent: {value}"
    );
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
