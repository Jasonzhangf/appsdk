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
    IdentityBindingKey::new(
        ProjectScopeId::new(project).unwrap(),
        AppServerId::new("appserver-cli").unwrap(),
        name,
    )
}

fn value(key: &IdentityBindingKey, agent: &str, runtime: &str, endpoint: &str) -> BindingValue {
    let generation = match runtime {
        "runtime-a" => 1,
        "runtime-b" => 2,
        "runtime-c" => 3,
        _ => 1,
    };
    value_at_generation(key, agent, runtime, endpoint, generation)
}

fn value_at_generation(
    key: &IdentityBindingKey,
    agent: &str,
    runtime: &str,
    endpoint: &str,
    generation: u64,
) -> BindingValue {
    let session_id = SessionId::new(format!("{runtime}-session")).unwrap();
    let native_thread_id = NativeThreadId::new(format!("{runtime}-thread")).unwrap();
    let binding = RuntimeBinding::new_with_session(
        key.project_scope.clone(),
        key.app_scope_id.clone(),
        AgentId::new(agent).unwrap(),
        RuntimeId::new(runtime).unwrap(),
        BindingId::new(format!("binding-{runtime}")).unwrap(),
        generation,
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
    assert_eq!(
        slot.active_binding().unwrap().binding.endpoint_generation,
        1
    );
    assert!(slot.reference_previous().is_none());
    slot.validate().unwrap();
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
    repeated.binding.endpoint_generation = 99;

    assert_eq!(
        slot.commit(&key, repeated, Some(1)).unwrap(),
        CommitOutcome::Unchanged { generation: 1 }
    );
    assert_eq!(slot.generation(), 1);
    assert_eq!(
        slot.active_binding().unwrap().binding.endpoint_generation,
        1
    );
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
fn new_session_can_start_its_own_endpoint_generation_at_one() {
    let key = key("alice");
    let mut slot = IdentityBindingSlot::new(key.clone());
    slot.commit(
        &key,
        value_at_generation(&key, "carrier-s1", "runtime-a", "endpoint-a", 1),
        Some(0),
    )
    .unwrap();
    slot.commit(
        &key,
        value_at_generation(&key, "carrier-s2", "runtime-b", "endpoint-b", 1),
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
        value_at_generation(&key, "carrier-s2", "runtime-b", "endpoint-b", 3),
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
            value_at_generation(&key, "carrier-s2", "runtime-b", "endpoint-b", 1),
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
            &["bob".to_owned()],
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
            &["alice".to_owned()],
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
