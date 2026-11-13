use super::binding_slot::{
    BindingControlSet, BindingValue, CrossScopeSourceRef, IdentityBindingKey, IdentityBindingSlot,
    PendingEffect, PendingEffectKind,
};
use super::binding_store::{BindingControlStore, ControlChange, Durability, StoreChangeOutcome};
use super::{project_reset::delete_route, HostRouteRecord};
use crate::identity::{AgentId, AppServerId, BindingId, NativeThreadId, RuntimeId, SessionId};
use crate::proto::{SelectedTransport, TransportKind};
use crate::scope::ProjectScopeId;
use crate::server::global_state::RuntimeBinding;

fn key() -> IdentityBindingKey {
    key_in("/tmp/collab-binding-store", "Alice")
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

fn value(key: &IdentityBindingKey, agent: &str, runtime: &str) -> BindingValue {
    let session_id = SessionId::new(format!("{runtime}-session")).unwrap();
    let thread_id = NativeThreadId::new(format!("{runtime}-thread")).unwrap();
    let binding = RuntimeBinding::new_with_session(
        key.project_scope.clone(),
        key.app_scope_id.clone(),
        AgentId::new(agent).unwrap(),
        RuntimeId::new(runtime).unwrap(),
        BindingId::new(format!("binding-{runtime}")).unwrap(),
        Some(session_id.clone()),
        Some(thread_id.clone()),
    )
    .unwrap();
    BindingValue::new(
        binding,
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some(format!("{runtime}-endpoint")),
            namespace: Some("test".to_owned()),
            session_id: Some(session_id.as_str().to_owned()),
            thread_id: Some(thread_id.as_str().to_owned()),
            tmux_endpoint: None,
            rex_endpoint: None,
            capabilities: vec!["test".to_owned()],
            self_check: "ok".to_owned(),
        },
    )
}

#[test]
fn missing_file_loads_an_empty_set() {
    let root = temp_root("missing");
    let store = BindingControlStore::new(&root);
    assert_eq!(store.load().unwrap(), BindingControlSet::new());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn rex_human_fence_survives_restart_rebuild_and_effect_clear() {
    use super::binding_slot::RexExecutionFence;
    let root = temp_root("human-fence");
    let store = BindingControlStore::new(&root);
    let key = key();
    let mut current = value(&key, "actor", "runtime");
    current.rex_execution_fence = RexExecutionFence::from_start_id("12:999999");
    store
        .commit(ControlChange::Commit {
            key: key.clone(),
            value: current.clone(),
            expected_generation: Some(0),
        })
        .unwrap();
    let restarted = BindingControlStore::new(&root);
    assert_eq!(
        restarted
            .load()
            .unwrap()
            .slot(&key)
            .unwrap()
            .active_binding()
            .unwrap(),
        &current
    );
    let mut rebuilt = current.clone();
    rebuilt.rex_execution_fence = None;
    restarted
        .commit(ControlChange::Commit {
            key: key.clone(),
            value: rebuilt.clone(),
            expected_generation: Some(1),
        })
        .unwrap();
    assert_eq!(
        restarted.load().unwrap().slot(&key).unwrap().generation(),
        1,
        "dropping fence is an identical retry after preservation"
    );
    let effect = effect_for(&key, "fence-effect", "runtime-session");
    restarted
        .commit(ControlChange::CommitWithEffect {
            key: key.clone(),
            value: rebuilt,
            expected_generation: Some(1),
            effect,
        })
        .unwrap();
    restarted
        .commit(ControlChange::ClearEffect {
            effect_id: "fence-effect".into(),
        })
        .unwrap();
    assert_eq!(
        restarted
            .load()
            .unwrap()
            .slot(&key)
            .unwrap()
            .active_binding()
            .unwrap()
            .rex_execution_fence,
        current.rex_execution_fence
    );
    // Old schema is still read without a migration or identity-history scan.
    let mut legacy = serde_json::to_value(current).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("rex_execution_fence");
    assert_eq!(
        serde_json::from_value::<BindingValue>(legacy)
            .unwrap()
            .rex_execution_fence,
        None
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rex_linux_fence_survives_restart_and_rebuild() {
    use super::binding_slot::RexExecutionFence;
    let root = temp_root("linux-fence");
    let store = BindingControlStore::new(&root);
    let key = key();
    let mut current = value(&key, "actor", "runtime");
    current.rex_execution_fence =
        RexExecutionFence::from_start_id("linux:b879bb0a-cf9a-47d1-b889-f63aa872d851:100:34771");
    store
        .commit(ControlChange::Commit {
            key: key.clone(),
            value: current.clone(),
            expected_generation: Some(0),
        })
        .unwrap();

    let restarted = BindingControlStore::new(&root);
    assert_eq!(
        restarted
            .load()
            .unwrap()
            .slot(&key)
            .unwrap()
            .active_binding()
            .unwrap()
            .rex_execution_fence,
        current.rex_execution_fence
    );
    let mut rebuilt = current.clone();
    rebuilt.rex_execution_fence = None;
    restarted
        .commit(ControlChange::Commit {
            key: key.clone(),
            value: rebuilt,
            expected_generation: Some(1),
        })
        .unwrap();
    assert_eq!(
        restarted
            .load()
            .unwrap()
            .slot(&key)
            .unwrap()
            .active_binding()
            .unwrap()
            .rex_execution_fence,
        current.rex_execution_fence
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn save_then_load_round_trips_current_map() {
    let root = temp_root("roundtrip");
    let store = BindingControlStore::new(&root);
    let key = key();
    let mut set = BindingControlSet::new();
    set.commit(&key, value(&key, "alice", "runtime-a"), Some(0))
        .unwrap();
    assert_eq!(store.save(&set).unwrap(), Durability::Confirmed);

    let loaded = store.load().unwrap();
    assert_eq!(loaded, set);
    let slot = loaded.slot(&key).unwrap();
    assert_eq!(slot.generation(), 1);
    assert_eq!(
        slot.active_binding().unwrap().binding.runtime_id.as_str(),
        "runtime-a"
    );
    assert!(slot.reference_previous().is_none());

    let content = std::fs::read_to_string(store.path()).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&content).unwrap()["schema_version"],
        1
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn corrupt_file_fails_closed_instead_of_returning_empty() {
    let root = temp_root("corrupt");
    let store = BindingControlStore::new(&root);
    std::fs::write(store.path(), b"{\"slots\": not-json").unwrap();
    let error = store.load().unwrap_err();
    assert!(error.contains("IDENTITY_MAP_DECODE_FAILED"), "{error}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn invalid_duplicate_set_fails_closed() {
    let root = temp_root("invalid");
    let store = BindingControlStore::new(&root);
    let key = serde_json::to_value(key()).unwrap();
    std::fs::write(
        store.path(),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "slots": [
                {"identity_key": key, "display_slot":2,"identity_status": "unregistered", "generation": 0, "current": null, "previous": null},
                {"identity_key": key, "display_slot":2,"identity_status": "unregistered", "generation": 0, "current": null, "previous": null}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let error = store.load().unwrap_err();
    assert!(error.contains("IDENTITY_MAP_INVALID"), "{error}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn publish_failure_is_returned_and_leaves_no_temporary_file() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_root("publish-failure");
    let store = BindingControlStore::new(&root);
    let key = key();
    let mut set = BindingControlSet::new();
    set.commit(&key, value(&key, "alice", "runtime-a"), Some(0))
        .unwrap();
    store.save(&set).unwrap();

    // Make the map directory unwritable so the temp file cannot be
    // created; the read still succeeds, so this exercises the publish path.
    let original = std::fs::metadata(&root).unwrap().permissions();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
    let error = store.save(&set).unwrap_err();
    std::fs::set_permissions(&root, original).unwrap();
    assert!(error.contains("BINDING_CONTROL_SAVE_FAILED"), "{error}");
    assert!(!std::fs::read_dir(&root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-")
    }));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn startup_loads_bindings_directly_from_current_map() {
    let root = temp_root("startup-direct");
    let store = BindingControlStore::new(&root);
    let key = key();
    let mut set = BindingControlSet::new();
    set.commit(&key, value(&key, "alice", "runtime-a"), Some(0))
        .unwrap();
    store.save(&set).unwrap();

    // Simulate a restart: the only binding input is the control file.
    let restarted_store = BindingControlStore::new(&root);
    let loaded = restarted_store.load().unwrap();
    let active: Vec<_> = loaded.active_values().collect();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].binding.agent_id.as_str(), "alice");
    assert_eq!(active[0].binding.runtime_id.as_str(), "runtime-a");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn identity_map_is_one_json_resource_and_does_not_rewrite_routes() {
    let root = temp_root("shared-route-file");
    let store = BindingControlStore::new(&root);
    let route = serde_json::json!({
        "app_scope_id": "appserver-cli",
        "project_scope": "/tmp/project",
        "canonical_root": "/tmp/project",
        "storage_root": "/tmp/project",
        "registered_ms": 1
    });
    let route_path = root.join("routes.json");
    let route_bytes = serde_json::to_string(&vec![route]).unwrap();
    std::fs::write(&route_path, &route_bytes).unwrap();

    let key = key();
    let mut set = BindingControlSet::new();
    set.commit(&key, value(&key, "alice", "runtime-a"), Some(0))
        .unwrap();
    store.save(&set).unwrap();

    let content = std::fs::read_to_string(store.path()).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&content).unwrap()["schema_version"],
        1
    );
    assert!(!content.contains("binding_control"));
    assert_eq!(std::fs::read_to_string(route_path).unwrap(), route_bytes);
    assert_eq!(store.load().unwrap(), set);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn binding_control_file_is_owner_only_after_publish() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_root("owner-only");
    let store = BindingControlStore::new(&root);
    let key = key();
    let mut set = BindingControlSet::new();
    set.commit(&key, value(&key, "alice", "runtime-a"), Some(0))
        .unwrap();
    store.save(&set).unwrap();

    let mode = std::fs::metadata(store.path())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn store_commit_is_one_entry_point_for_stable_name_handoff() {
    let root = temp_root("commit-entry");
    let store = BindingControlStore::new(&root);
    let key = key();

    let first = store
        .commit(ControlChange::Commit {
            key: key.clone(),
            value: value(&key, "alice-s1", "runtime-a"),
            expected_generation: Some(0),
        })
        .unwrap();
    assert_eq!(first.durability, Durability::Confirmed);
    assert!(matches!(first.outcome, StoreChangeOutcome::Slot(_)));

    let second = store
        .commit(ControlChange::Commit {
            key: key.clone(),
            value: value(&key, "alice-s2", "runtime-b"),
            expected_generation: Some(1),
        })
        .unwrap();
    assert_eq!(second.durability, Durability::Confirmed);

    let loaded = store.load().unwrap();
    assert_eq!(loaded.slots().count(), 1);
    let slot = loaded.slot(&key).unwrap();
    assert_eq!(
        slot.active_binding().unwrap().binding.agent_id.as_str(),
        "alice-s2"
    );
    assert_eq!(
        slot.reference_previous().unwrap().binding.agent_id.as_str(),
        "alice-s1"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn publication_guard_rejects_cross_app_duplicate_names_and_allows_other_projects() {
    let root = temp_root("project-name-publication-guard");
    let store = BindingControlStore::new(&root);
    let project = ProjectScopeId::new("/tmp/collab-binding-store-project-name").unwrap();
    let alice_app_a =
        IdentityBindingKey::new(project.clone(), AppServerId::new("app-a").unwrap(), "Alice");
    let alice_app_b =
        IdentityBindingKey::new(project.clone(), AppServerId::new("app-b").unwrap(), "Alice");
    let other_project_alice = IdentityBindingKey::new(
        ProjectScopeId::new("/tmp/collab-binding-store-project-name-other").unwrap(),
        AppServerId::new("app-b").unwrap(),
        "Alice",
    );
    store
        .commit(ControlChange::Commit {
            key: alice_app_a.clone(),
            value: value(&alice_app_a, "alice-a", "runtime-a"),
            expected_generation: Some(0),
        })
        .unwrap();

    let duplicate = store.commit(ControlChange::Commit {
        key: alice_app_b.clone(),
        value: value(&alice_app_b, "alice-b", "runtime-b"),
        expected_generation: Some(0),
    });
    let error = duplicate.expect_err("a raw Commit cannot bypass project-name uniqueness");
    assert!(error.contains("IDENTITY_PROJECT_NAME_DUPLICATE"), "{error}");
    let loaded = store.load().unwrap();
    assert_eq!(loaded.active_values().count(), 1);
    assert!(loaded
        .slot(&IdentityBindingKey::new(
            project.clone(),
            AppServerId::new("app-b").unwrap(),
            "Alice"
        ))
        .and_then(|slot| slot.active_binding())
        .is_none());

    let bob_app_b =
        IdentityBindingKey::new(project.clone(), AppServerId::new("app-b").unwrap(), "Bob");
    store
        .commit(ControlChange::Commit {
            key: bob_app_b.clone(),
            value: value(&bob_app_b, "bob-b", "runtime-c"),
            expected_generation: Some(0),
        })
        .unwrap();
    let bob_app_a =
        IdentityBindingKey::new(project.clone(), AppServerId::new("app-a").unwrap(), "Bob");
    let duplicate_move = store.commit(ControlChange::MoveName {
        target: bob_app_a.clone(),
        value: value(&bob_app_a, "bob-a", "runtime-d"),
        source_names: Vec::new(),
        expected_generation: Some(0),
    });
    let error = duplicate_move.expect_err("a raw MoveName cannot bypass project-name uniqueness");
    assert!(error.contains("IDENTITY_PROJECT_NAME_DUPLICATE"), "{error}");

    store
        .commit(ControlChange::Commit {
            key: other_project_alice.clone(),
            value: value(&other_project_alice, "other-alice", "runtime-e"),
            expected_generation: Some(0),
        })
        .unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(
        loaded
            .slot(&other_project_alice)
            .and_then(|slot| slot.active_binding())
            .unwrap()
            .binding
            .agent_id
            .as_str(),
        "other-alice"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

fn cross_scope_pending_effect(
    target: &IdentityBindingKey,
    source: CrossScopeSourceRef,
    effect_id: &str,
    target_worker: &str,
    target_session: &str,
) -> PendingEffect {
    cross_scope_pending_effect_with_sources(
        target,
        vec![source],
        effect_id,
        target_worker,
        target_session,
    )
}

fn cross_scope_pending_effect_with_sources(
    target: &IdentityBindingKey,
    sources: Vec<CrossScopeSourceRef>,
    effect_id: &str,
    target_worker: &str,
    target_session: &str,
) -> PendingEffect {
    PendingEffect {
        role: None,
        kind: PendingEffectKind::NameAcrossScopes { sources },
        effect_id: effect_id.to_owned(),
        operation_id: effect_id.to_owned(),
        project_scope: target.project_scope.clone(),
        app_scope_id: target.app_scope_id.clone(),
        name: target.name.clone(),
        previous_worker_id: Some("alice-old".to_owned()),
        previous_session_id: Some("runtime-a-old-session".to_owned()),
        target_worker_id: target_worker.to_owned(),
        target_session_id: target_session.to_owned(),
        occupancy_revision: 1,
        source_names: Vec::new(),
        moves_existing_names: false,
        pane_anchor: None,
        created_ms: 1,
    }
}

fn cross_scope_store_fixture(
    label: &str,
) -> (
    std::path::PathBuf,
    BindingControlStore,
    IdentityBindingKey,
    IdentityBindingKey,
) {
    let root = temp_root(label);
    let store = BindingControlStore::new(&root);
    let project = ProjectScopeId::new("/tmp/collab-binding-store-cross-scope").unwrap();
    let alice_app_a =
        IdentityBindingKey::new(project.clone(), AppServerId::new("app-a").unwrap(), "Alice");
    let frank_app_b = IdentityBindingKey::new(project, AppServerId::new("app-b").unwrap(), "Frank");
    store
        .commit(ControlChange::Commit {
            key: alice_app_a.clone(),
            value: value(&alice_app_a, "alice-old", "runtime-a-old"),
            expected_generation: Some(0),
        })
        .unwrap();
    store
        .commit(ControlChange::Commit {
            key: frank_app_b.clone(),
            value: value(&frank_app_b, "frank-b", "runtime-frank-b"),
            expected_generation: Some(0),
        })
        .unwrap();
    (root, store, alice_app_a, frank_app_b)
}

#[test]
fn cross_scope_takeover_releases_source_and_publishes_target_in_one_candidate() {
    let (root, store, alice_app_a, frank_app_b) = cross_scope_store_fixture("cross-scope-takeover");
    let source_slot = store.load().unwrap().slot(&frank_app_b).cloned().unwrap();
    let source = BindingControlSet::cross_scope_source_ref(&source_slot).unwrap();
    assert_eq!(source.generation, 1);
    let effect = cross_scope_pending_effect(
        &alice_app_a,
        source,
        "cross-scope-takeover:operation",
        "frank-a",
        "runtime-frank-a-session",
    );

    let committed = store
        .commit(ControlChange::MoveNameWithEffect {
            target: alice_app_a.clone(),
            value: value(&alice_app_a, "frank-a", "runtime-frank-a"),
            source_names: Vec::new(),
            expected_generation: Some(1),
            effect: effect.clone(),
            companion: None,
        })
        .unwrap();
    assert_eq!(committed.durability, Durability::Confirmed);

    let loaded = store.load().unwrap();
    let alice = loaded.slot(&alice_app_a).unwrap();
    assert_eq!(
        alice.active_binding().unwrap().binding.agent_id.as_str(),
        "frank-a",
        "the target app publishes the current caller"
    );
    assert_eq!(
        alice
            .reference_previous()
            .unwrap()
            .binding
            .agent_id
            .as_str(),
        "alice-old",
        "the displaced target holder remains exact history"
    );
    let frank = loaded.slot(&frank_app_b).unwrap();
    assert!(
        frank.active_binding().is_none(),
        "the source current is released in the same map candidate"
    );
    assert_eq!(
        frank
            .reference_previous()
            .unwrap()
            .binding
            .binding_id
            .as_str(),
        "binding-runtime-frank-b",
        "the source exact binding remains available for business reconciliation"
    );
    loaded
        .validate_project_name_uniqueness(&alice_app_a.project_scope, "Alice")
        .unwrap();
    assert!(
        loaded
            .project_current_for_name(&alice_app_a.project_scope, "Frank")
            .unwrap()
            .is_none(),
        "no other app keeps Frank current after the takeover"
    );
    assert!(
        loaded
            .pending_effect(&effect.effect_id)
            .is_some_and(|saved| saved.cross_scope_sources() == effect.cross_scope_sources()),
        "the exact source reference remains in the bounded pending effect"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn cross_scope_takeover_converges_historical_duplicate_and_preserves_history() {
    let root = temp_root("cross-scope-history-convergence");
    let store = BindingControlStore::new(&root);
    let project = ProjectScopeId::new("/tmp/collab-binding-store-cross-scope-history").unwrap();
    let alice_app_a =
        IdentityBindingKey::new(project.clone(), AppServerId::new("app-a").unwrap(), "Alice");
    let alice_app_c =
        IdentityBindingKey::new(project.clone(), AppServerId::new("app-c").unwrap(), "Alice");
    let frank_app_b = IdentityBindingKey::new(project, AppServerId::new("app-b").unwrap(), "Frank");
    let mut control = BindingControlSet::new();
    control
        .commit(
            &alice_app_a,
            value(&alice_app_a, "alice-a", "runtime-alice-a"),
            None,
        )
        .unwrap();
    control
        .commit(
            &alice_app_c,
            value(&alice_app_c, "alice-c", "runtime-alice-c"),
            None,
        )
        .unwrap();
    control
        .commit(
            &frank_app_b,
            value(&frank_app_b, "frank-b", "runtime-frank-b"),
            None,
        )
        .unwrap();
    store.save(&control).unwrap();

    let loaded = store.load().unwrap();
    let sources = vec![
        BindingControlSet::cross_scope_source_ref(loaded.slot(&alice_app_c).unwrap()).unwrap(),
        BindingControlSet::cross_scope_source_ref(loaded.slot(&frank_app_b).unwrap()).unwrap(),
    ];
    let effect = cross_scope_pending_effect_with_sources(
        &alice_app_a,
        sources,
        "cross-scope-history:operation",
        "frank-a",
        "runtime-frank-a-session",
    );
    store
        .commit(ControlChange::MoveNameWithEffect {
            target: alice_app_a.clone(),
            value: value(&alice_app_a, "frank-a", "runtime-frank-a"),
            source_names: Vec::new(),
            expected_generation: Some(1),
            effect,
            companion: None,
        })
        .unwrap();

    let loaded = store.load().unwrap();
    assert_eq!(
        loaded
            .project_currents_for_name(&alice_app_a.project_scope, "Alice")
            .len(),
        1,
        "the explicit target app leaves exactly one current Alice"
    );
    assert_eq!(
        loaded
            .slot(&alice_app_a)
            .and_then(IdentityBindingSlot::active_binding)
            .map(|binding| binding.binding.agent_id.as_str()),
        Some("frank-a")
    );
    assert!(loaded
        .slot(&alice_app_c)
        .unwrap()
        .active_binding()
        .is_none());
    assert!(loaded
        .slot(&frank_app_b)
        .unwrap()
        .active_binding()
        .is_none());
    assert_eq!(
        loaded
            .slot(&alice_app_c)
            .unwrap()
            .reference_previous()
            .unwrap()
            .binding
            .binding_id
            .as_str(),
        "binding-runtime-alice-c",
        "the conflicting app keeps its exact business-reference binding"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cross_scope_takeover_rejects_stale_source_generation() {
    let (root, store, alice_app_a, frank_app_b) = cross_scope_store_fixture("cross-scope-stale");
    let source_slot = store.load().unwrap().slot(&frank_app_b).cloned().unwrap();
    let stale_source = BindingControlSet::cross_scope_source_ref(&source_slot).unwrap();
    store
        .commit(ControlChange::Commit {
            key: frank_app_b.clone(),
            value: value(&frank_app_b, "frank-b-new", "runtime-frank-b-new"),
            expected_generation: Some(1),
        })
        .unwrap();
    let before = store.load().unwrap();
    let effect = cross_scope_pending_effect(
        &alice_app_a,
        stale_source,
        "cross-scope-stale:operation",
        "frank-a",
        "runtime-frank-a-session",
    );

    let error = store
        .commit(ControlChange::MoveNameWithEffect {
            target: alice_app_a.clone(),
            value: value(&alice_app_a, "frank-a", "runtime-frank-a"),
            source_names: Vec::new(),
            expected_generation: Some(1),
            effect,
            companion: None,
        })
        .expect_err("a stale cross-scope source cannot be released by a later takeover");
    assert!(error.contains("CROSS_SCOPE_SOURCE_CHANGED"), "{error}");
    let after = store.load().unwrap();
    assert_eq!(after, before, "the rejected candidate publishes nothing");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn cross_scope_source_effect_is_rejected_outside_atomic_move() {
    let (root, store, alice_app_a, frank_app_b) =
        cross_scope_store_fixture("cross-scope-write-entry");
    let source_slot = store.load().unwrap().slot(&frank_app_b).cloned().unwrap();
    let source = BindingControlSet::cross_scope_source_ref(&source_slot).unwrap();
    let effect = cross_scope_pending_effect(
        &alice_app_a,
        source,
        "cross-scope-write-entry:operation",
        "frank-a",
        "runtime-frank-a-session",
    );
    for change in [
        ControlChange::CommitWithEffect {
            key: alice_app_a.clone(),
            value: value(&alice_app_a, "frank-a", "runtime-frank-a"),
            expected_generation: Some(1),
            effect: effect.clone(),
        },
        ControlChange::UnregisterWithEffect {
            key: alice_app_a.clone(),
            effect: effect.clone(),
        },
        ControlChange::SetEffect {
            effect: effect.clone(),
        },
    ] {
        let error = store
            .commit(change)
            .expect_err("only MoveNameWithEffect may release a cross-scope source");
        assert!(
            error.contains("cross-scope source requires MoveNameWithEffect"),
            "{error}"
        );
    }
    std::fs::remove_dir_all(&root).unwrap();
}

/// A retry of an explicit cross-scope takeover continues the remaining effects
/// instead of failing on the source it already released. The exact source ref
/// stays recognizable after release, a successor current is never released by
/// the old intent, and the bounded pending effect stays open for the retry.
#[test]
fn cross_scope_source_release_retry_is_idempotent_and_never_releases_a_successor() {
    let (root, store, alice_app_a, frank_app_b) = cross_scope_store_fixture("cross-scope-retry");
    let source_slot = store.load().unwrap().slot(&frank_app_b).cloned().unwrap();
    let source = BindingControlSet::cross_scope_source_ref(&source_slot).unwrap();
    let effect = cross_scope_pending_effect(
        &alice_app_a,
        source.clone(),
        "cross-scope-retry:operation",
        "frank-a",
        "runtime-frank-a-session",
    );
    store
        .commit(ControlChange::MoveNameWithEffect {
            target: alice_app_a.clone(),
            value: value(&alice_app_a, "frank-a", "runtime-frank-a"),
            source_names: Vec::new(),
            expected_generation: Some(1),
            effect: effect.clone(),
            companion: None,
        })
        .unwrap();

    let mut retry = store.load().unwrap();
    assert!(
        !retry.validate_cross_scope_source(&source).unwrap(),
        "an already released source is a recognized retry, not a changed source"
    );
    assert_eq!(
        retry
            .release_cross_scope_sources(std::slice::from_ref(&source))
            .unwrap(),
        0,
        "retrying the release must not release the source a second time"
    );
    assert!(
        retry.slot(&frank_app_b).unwrap().active_binding().is_none(),
        "the retry keeps the source current released"
    );
    assert!(
        retry
            .pending_effect(&effect.effect_id)
            .is_some_and(|saved| saved.cross_scope_sources() == effect.cross_scope_sources()),
        "the bounded pending effect stays available for the remaining effects"
    );

    store
        .commit(ControlChange::Commit {
            key: frank_app_b.clone(),
            value: value(&frank_app_b, "frank-b-new", "runtime-frank-b-new"),
            expected_generation: Some(2),
        })
        .unwrap();
    let after_successor = store.load().unwrap();
    let error = after_successor
        .validate_cross_scope_source(&source)
        .expect_err("a replacement current is not the same source");
    assert!(error.contains("CROSS_SCOPE_SOURCE_CHANGED"), "{error}");
    assert_eq!(
        after_successor
            .slot(&frank_app_b)
            .and_then(IdentityBindingSlot::active_binding)
            .map(|binding| binding.binding.agent_id.as_str()),
        Some("frank-b-new"),
        "the old retry must not release the successor current"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn move_name_commit_publishes_one_target_slot_and_retires_the_source() {
    let root = temp_root("move-name");
    let store = BindingControlStore::new(&root);
    let alice = key_in("/tmp/collab-binding-store-move", "alice");
    let bob = key_in("/tmp/collab-binding-store-move", "bob");

    store
        .commit(ControlChange::Commit {
            key: alice.clone(),
            value: value(&alice, "alice-s1", "runtime-a"),
            expected_generation: Some(0),
        })
        .unwrap();
    store
        .commit(ControlChange::Commit {
            key: bob.clone(),
            value: value(&bob, "bob-s1", "runtime-b"),
            expected_generation: Some(0),
        })
        .unwrap();

    let moved = store
        .commit(ControlChange::MoveName {
            target: alice.clone(),
            value: value(&alice, "bob-s2", "runtime-c"),
            source_names: vec![bob.name.clone()],
            expected_generation: Some(1),
        })
        .unwrap();
    assert_eq!(moved.durability, Durability::Confirmed);
    assert!(matches!(moved.outcome, StoreChangeOutcome::Moved(_)));

    let loaded = store.load().unwrap();
    assert_eq!(loaded.slots().count(), 2);
    let alice_slot = loaded.slot(&alice).unwrap();
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
    let bob_slot = loaded.slot(&bob).unwrap();
    assert!(
        bob_slot.active_binding().is_none(),
        "the retired source slot must not expose a current binding"
    );
    assert_eq!(loaded.active_values().count(), 1);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn commit_with_effect_publishes_slot_and_handoff_together() {
    let root = temp_root("commit-with-effect");
    let store = BindingControlStore::new(&root);
    let key = key();
    let effect = effect_for(&key, "name:test-op", "runtime-a-session");

    let committed = store
        .commit(ControlChange::CommitWithEffect {
            key: key.clone(),
            value: value(&key, "alice", "runtime-a"),
            expected_generation: Some(0),
            effect,
        })
        .unwrap();
    assert_eq!(committed.durability, Durability::Confirmed);

    let loaded = store.load().unwrap();
    assert_eq!(
        loaded
            .slot(&key)
            .and_then(|slot| slot.active_binding())
            .unwrap()
            .binding
            .runtime_id
            .as_str(),
        "runtime-a"
    );
    let effect = loaded.pending_effect("name:test-op").unwrap();
    assert_eq!(effect.kind, PendingEffectKind::Name);
    assert_eq!(effect.target_worker_id, "alice");
    assert_eq!(effect.target_session_id, "runtime-a-session");
    assert!(store.load_with_presence().unwrap().1);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn clear_effect_keeps_the_slot_and_is_idempotent() {
    let root = temp_root("clear-effect");
    let store = BindingControlStore::new(&root);
    let key = key();
    store
        .commit(ControlChange::CommitWithEffect {
            key: key.clone(),
            value: value(&key, "alice", "runtime-a"),
            expected_generation: Some(0),
            effect: effect_for(&key, "name:test-op", "runtime-a-session"),
        })
        .unwrap();

    assert!(matches!(
        store
            .commit(ControlChange::ClearEffect {
                effect_id: "name:test-op".into(),
            })
            .unwrap()
            .outcome,
        StoreChangeOutcome::Effect { changed: true }
    ));
    assert!(matches!(
        store
            .commit(ControlChange::ClearEffect {
                effect_id: "name:test-op".into(),
            })
            .unwrap()
            .outcome,
        StoreChangeOutcome::Effect { changed: false }
    ));

    let loaded = store.load().unwrap();
    assert!(loaded.slot(&key).is_some());
    assert!(loaded.pending_effect("name:test-op").is_none());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn name_move_publishes_the_displaced_holder_after_releasing_the_source() {
    let root = temp_root("name-companion-source");
    let store = BindingControlStore::new(&root);
    let alice = key();
    let bob = key_in(alice.project_scope.as_str(), "Bob");
    let first = value(&alice, "first", "runtime-first");
    let second = value(&bob, "second", "runtime-second");
    let mut control = BindingControlSet::new();
    control.commit(&alice, first.clone(), None).unwrap();
    control.commit(&bob, second.clone(), None).unwrap();
    store.save(&control).unwrap();
    let mut primary = effect_for(&alice, "name:swap", "runtime-second-session");
    primary.target_worker_id = "second".into();
    primary.source_names = vec!["Bob".into()];
    primary.moves_existing_names = true;
    let mut outgoing = effect_for(&bob, "name:swap:outgoing", "runtime-first-session");
    outgoing.target_worker_id = "first".into();
    outgoing.previous_worker_id = None;
    outgoing.previous_session_id = None;
    store
        .commit(ControlChange::MoveNameWithEffect {
            target: alice.clone(),
            value: second.clone(),
            source_names: vec!["Bob".into()],
            expected_generation: None,
            effect: primary,
            companion: Some((bob.clone(), first.clone(), outgoing)),
        })
        .unwrap();
    let current = store.load().unwrap();
    assert_eq!(
        current.slot(&alice).unwrap().active_binding(),
        Some(&second)
    );
    assert_eq!(current.slot(&bob).unwrap().active_binding(), Some(&first));
    assert_eq!(current.active_values().count(), 2);
    assert!(current.pending_effect("name:swap").is_some());
    assert!(current.pending_effect("name:swap:outgoing").is_some());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn effect_only_control_line_round_trips_until_cleared() {
    let root = temp_root("effect-only");
    let store = BindingControlStore::new(&root);
    let key = key();
    store
        .commit(ControlChange::SetEffect {
            effect: effect_for(&key, "register-op", "runtime-a-session"),
        })
        .unwrap();

    let loaded = store.load().unwrap();
    assert!(loaded.slots().next().is_none());
    assert_eq!(
        loaded.pending_effect("register-op").unwrap().kind,
        PendingEffectKind::Register
    );

    store
        .commit(ControlChange::ClearEffect {
            effect_id: "register-op".into(),
        })
        .unwrap();
    assert_eq!(store.load().unwrap(), BindingControlSet::new());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn post_rename_parent_sync_failure_reports_unknown_and_keeps_published_file() {
    let root = temp_root("sync-unknown");
    let store = BindingControlStore::new(&root);
    let key = key();

    // The rename has already landed; only the parent-directory sync fails.
    // The store must report Durability::Unknown, keep the new file visible,
    // and never roll back to a previous snapshot or leave a temp file.
    store.fail_parent_sync_on_next_publish();
    let commit = store
        .commit(ControlChange::Commit {
            key: key.clone(),
            value: value(&key, "alice", "runtime-a"),
            expected_generation: Some(0),
        })
        .unwrap();
    assert_eq!(commit.durability, Durability::Unknown);

    let loaded = store.load().unwrap();
    assert_eq!(
        loaded
            .slot(&key)
            .and_then(|slot| slot.active_binding())
            .unwrap()
            .binding
            .agent_id
            .as_str(),
        "alice"
    );
    assert!(!std::fs::read_dir(&root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-")
    }));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn project_reset_preserves_current_dsh_binding() {
    let root = temp_root("reset-dsh");
    let store = BindingControlStore::new(&root);
    let dsh = key_in("/tmp/collab-binding-reset-dsh", "Alice");
    let mut binding = value(&dsh, "dsh-worker", "dsh-runtime");
    binding.transport.kind = TransportKind::Dsh;
    binding.transport.endpoint = Some("http://127.0.0.1:12700".into());
    let mut set = BindingControlSet::new();
    set.commit(&dsh, binding.clone(), Some(0)).unwrap();
    store.save(&set).unwrap();
    let routes = vec![route_record(&dsh)];
    let route_path = root.join("routes.json");
    super::binding_store::publish_route_records(&route_path, &routes).unwrap();
    super::project_reset::delete_project_routes_except_dsh(
        &route_path,
        dsh.project_scope.as_str(),
        &std::collections::HashSet::from([dsh.app_scope_id.as_str().to_owned()]),
    )
    .unwrap();
    let loaded_routes = super::load_host_route_records(&route_path).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(
        loaded.slot(&dsh).and_then(|slot| slot.active_binding()),
        Some(&binding)
    );
    assert_eq!(
        loaded_routes.len(),
        1,
        "reset must retain the active DSH route record"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_reset_delete_route_drops_only_its_scope_slots() {
    let root = temp_root("reset-scope");
    let store = BindingControlStore::new(&root);
    let alice = key_in("/tmp/collab-binding-reset-a", "alice");
    let bob = key_in("/tmp/collab-binding-reset-b", "bob");
    let mut set = BindingControlSet::new();
    set.commit(&alice, value(&alice, "alice", "runtime-a"), Some(0))
        .unwrap();
    set.commit(&bob, value(&bob, "bob", "runtime-b"), Some(0))
        .unwrap();
    set.set_effect(effect_for(&alice, "register-reset-a", "runtime-a-session"))
        .unwrap();
    set.set_effect(effect_for(&bob, "register-reset-b", "runtime-b-session"))
        .unwrap();
    store.save(&set).unwrap();

    delete_route(&root.join("routes.json"), &route_record(&alice)).unwrap();

    let loaded = BindingControlStore::new(&root).load().unwrap();
    assert!(
        loaded.slot(&alice).is_none(),
        "reset must remove the control slot for the deleted scope"
    );
    assert!(
        loaded.slot(&bob).is_some(),
        "reset must keep control slots for unrelated scopes"
    );
    assert!(
        loaded.pending_effect("register-reset-a").is_none(),
        "reset must clear pending effects for the deleted scope"
    );
    assert!(
        loaded.pending_effect("register-reset-b").is_some(),
        "reset must keep pending effects for unrelated scopes"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn project_reset_delete_project_routes_keeps_only_retained_dsh_app_scope() {
    let root = temp_root("reset-route-scope");
    let store = BindingControlStore::new(&root);
    let dsh = key_in("/tmp/collab-binding-reset-routes", "Alice");
    let other = IdentityBindingKey::new(
        ProjectScopeId::new("/tmp/collab-binding-reset-routes").unwrap(),
        AppServerId::new("appserver-other").unwrap(),
        "Bob",
    );
    let mut dsh_value = value(&dsh, "dsh-worker", "dsh-runtime");
    dsh_value.transport.kind = TransportKind::Dsh;
    let mut set = BindingControlSet::new();
    set.commit(&dsh, dsh_value, Some(0)).unwrap();
    set.commit(
        &other,
        value(&other, "other-worker", "other-runtime"),
        Some(0),
    )
    .unwrap();
    let mut other_route = route_record(&other);
    other_route.canonical_root = "/tmp/collab-binding-reset-routes-other".into();
    other_route.storage_root = "/tmp/collab-binding-reset-routes-other".into();
    let routes = vec![route_record(&dsh), other_route];
    store.save(&set).unwrap();
    let route_path = root.join("routes.json");
    super::binding_store::publish_route_records(&route_path, &routes).unwrap();

    super::project_reset::delete_project_routes_except_dsh(
        &route_path,
        dsh.project_scope.as_str(),
        &std::collections::HashSet::from([dsh.app_scope_id.as_str().to_owned()]),
    )
    .unwrap();

    let loaded_routes = super::load_host_route_records(&route_path).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded_routes.len(), 1);
    assert_eq!(loaded_routes[0].app_scope_id, dsh.app_scope_id.as_str());
    assert!(loaded.slot(&dsh).is_some(), "active DSH slot must survive");
    assert!(
        loaded.slot(&other).is_none(),
        "non-DSH slot must be retired"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn identical_retry_confirms_directory_sync_instead_of_inventing_durability() {
    let root = temp_root("identical-retry-sync");
    let store = BindingControlStore::new(&root);
    let key = key();
    let change = ControlChange::Commit {
        key: key.clone(),
        value: value(&key, "alice", "runtime-a"),
        expected_generation: None,
    };
    store.fail_parent_sync_on_next_publish();
    assert_eq!(
        store.commit(change.clone()).unwrap().durability,
        Durability::Unknown
    );
    let published = std::fs::read(store.path()).unwrap();

    // The manager constructs a fresh store for each request. A second failed
    // directory sync must remain Unknown even when the binding facts match.
    let retry = BindingControlStore::new(&root);
    retry.fail_parent_sync_on_next_publish();
    assert_eq!(
        retry.commit(change.clone()).unwrap().durability,
        Durability::Unknown
    );
    assert_eq!(std::fs::read(store.path()).unwrap(), published);

    let confirmed = retry.commit(change).unwrap();
    assert_eq!(confirmed.durability, Durability::Confirmed);
    assert!(matches!(
        confirmed.outcome,
        StoreChangeOutcome::Slot(super::binding_slot::CommitOutcome::Unchanged { generation: 1 })
    ));
    assert_eq!(std::fs::read(store.path()).unwrap(), published);
    std::fs::remove_dir_all(root).unwrap();
}

fn route_record(key: &IdentityBindingKey) -> HostRouteRecord {
    HostRouteRecord {
        app_scope_id: key.app_scope_id.as_str().to_owned(),
        project_scope: key.project_scope.as_str().to_owned(),
        canonical_root: key.project_scope.as_str().to_owned(),
        storage_root: key.project_scope.as_str().to_owned(),
        registered_ms: 1,
    }
}

fn effect_for(key: &IdentityBindingKey, effect_id: &str, target_session_id: &str) -> PendingEffect {
    PendingEffect {
        role: None,
        kind: if effect_id.starts_with("register") {
            PendingEffectKind::Register
        } else {
            PendingEffectKind::Name
        },
        effect_id: effect_id.to_owned(),
        operation_id: "operation".to_owned(),
        project_scope: key.project_scope.clone(),
        app_scope_id: key.app_scope_id.clone(),
        name: key.name.clone(),
        previous_worker_id: Some("previous-worker".to_owned()),
        previous_session_id: Some("previous-session".to_owned()),
        target_worker_id: "alice".to_owned(),
        target_session_id: target_session_id.to_owned(),
        occupancy_revision: 1,
        source_names: Vec::new(),
        moves_existing_names: false,
        pane_anchor: None,
        created_ms: 1,
    }
}

#[test]
fn route_writers_surface_unknown_publication_append() {
    check_route_writer_unknown("append");
}

#[test]
fn route_writers_surface_unknown_publication_delete() {
    check_route_writer_unknown("delete");
}

fn check_route_writer_unknown(writer: &str) {
    let root = temp_root(writer);
    let store = BindingControlStore::new(&root);
    let key = key();
    let record = route_record(&key);
    let mut control = BindingControlSet::new();
    control
        .commit(&key, value(&key, "alice", "runtime-a"), Some(0))
        .unwrap();
    let initial = if writer == "delete" {
        vec![record.clone()]
    } else {
        vec![]
    };
    store.save(&control).unwrap();
    let route_path = root.join("routes.json");
    super::binding_store::publish_route_records(&route_path, &initial).unwrap();
    BindingControlStore::at(&route_path).fail_parent_sync_on_next_publish();
    let result = match writer {
        "append" => super::append_host_route_record(&route_path, &record),
        _ => delete_route(&route_path, &record),
    };
    let routes = super::load_host_route_records(&route_path).unwrap();
    let visible_control = store.load().unwrap();
    assert_eq!(routes.len(), usize::from(writer != "delete"));
    assert_eq!(visible_control.slot(&key).is_some(), writer != "delete");
    let error = result.expect_err("published but unconfirmed must not report success");
    assert!(
        error.contains("HOST_ROUTE_DURABILITY_UNKNOWN"),
        "{writer}: {error}"
    );
    assert!(error.contains("published=true"), "{writer}: {error}");
    // A confirmed publication still succeeds through each real caller.
    if writer == "append" {
        delete_route(&route_path, &record).unwrap();
        super::append_host_route_record(&route_path, &record).unwrap();
    } else {
        delete_route(&route_path, &record).unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}

fn temp_root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "collab-binding-store-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}
