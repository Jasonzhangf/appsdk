use super::binding_slot::{
    BindingControlSet, BindingValue, IdentityBindingKey, PendingEffect, PendingEffectKind,
};
use super::binding_store::{BindingControlStore, ControlChange, Durability, StoreChangeOutcome};
use super::{project_reset::delete_route, HostRouteRecord};
use crate::identity::{AgentId, AppServerId, BindingId, NativeThreadId, RuntimeId, SessionId};
use crate::proto::{SelectedTransport, TransportKind};
use crate::scope::ProjectScopeId;
use crate::server::global_state::RuntimeBinding;

fn key() -> IdentityBindingKey {
    key_in("/tmp/collab-binding-store", "alice")
}

fn key_in(project: &str, name: &str) -> IdentityBindingKey {
    IdentityBindingKey::new(
        ProjectScopeId::new(project).unwrap(),
        AppServerId::new("appserver-cli").unwrap(),
        name,
    )
}

fn value(key: &IdentityBindingKey, agent: &str, runtime: &str) -> BindingValue {
    value_at(key, agent, runtime, 1)
}

fn value_at(key: &IdentityBindingKey, agent: &str, runtime: &str, generation: u64) -> BindingValue {
    let session_id = SessionId::new(format!("{runtime}-session")).unwrap();
    let thread_id = NativeThreadId::new(format!("{runtime}-thread")).unwrap();
    let binding = RuntimeBinding::new_with_session(
        key.project_scope.clone(),
        key.app_scope_id.clone(),
        AgentId::new(agent).unwrap(),
        RuntimeId::new(runtime).unwrap(),
        BindingId::new(format!("binding-{runtime}")).unwrap(),
        generation,
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
fn save_then_load_round_trips_without_a_journal() {
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
    assert_eq!(content.lines().count(), 1);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn corrupt_file_fails_closed_instead_of_returning_empty() {
    let root = temp_root("corrupt");
    let store = BindingControlStore::new(&root);
    std::fs::write(store.path(), b"{\"slots\": not-json").unwrap();
    let error = store.load().unwrap_err();
    assert!(error.contains("BINDING_CONTROL_DECODE_FAILED"), "{error}");
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
            "kind": "binding_control",
            "slots": [
                {"identity_key": key, "identity_status": "unregistered", "generation": 0, "current": null, "previous": null},
                {"identity_key": key, "identity_status": "unregistered", "generation": 0, "current": null, "previous": null}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let error = store.load().unwrap_err();
    assert!(error.contains("BINDING_CONTROL_INVALID"), "{error}");
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

    // Make the journal directory unwritable so the temp file cannot be
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
fn startup_loads_bindings_directly_without_scanning_project_journals() {
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
fn binding_control_is_one_line_in_the_shared_route_file() {
    let root = temp_root("shared-route-file");
    let store = BindingControlStore::new(&root);
    let route = serde_json::json!({
        "version": 1,
        "op": "register",
        "app_scope_id": "appserver-cli",
        "project_scope": "/tmp/project",
        "canonical_root": "/tmp/project",
        "storage_root": "/tmp/project",
        "registered_ms": 1
    });
    std::fs::write(
        store.path(),
        format!("{}\n", serde_json::to_string(&route).unwrap()),
    )
    .unwrap();

    let key = key();
    let mut set = BindingControlSet::new();
    set.commit(&key, value(&key, "alice", "runtime-a"), Some(0))
        .unwrap();
    store.save(&set).unwrap();

    let content = std::fs::read_to_string(store.path()).unwrap();
    assert_eq!(
        content
            .lines()
            .filter(|line| line.contains("\"kind\":\"binding_control\""))
            .count(),
        1
    );
    assert!(content.contains("\"op\":\"register\""));
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
            value: value_at(&key, "alice-s2", "runtime-b", 2),
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
            value: value_at(&alice, "bob-s2", "runtime-c", 2),
            source_names: vec!["bob".to_owned()],
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

    delete_route(store.path(), &route_record(&alice)).unwrap();

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
        version: 1,
        op: "register".into(),
        app_scope_id: key.app_scope_id.as_str().to_owned(),
        project_scope: key.project_scope.as_str().to_owned(),
        canonical_root: key.project_scope.as_str().to_owned(),
        storage_root: key.project_scope.as_str().to_owned(),
        registered_ms: 1,
    }
}

fn effect_for(key: &IdentityBindingKey, effect_id: &str, target_session_id: &str) -> PendingEffect {
    PendingEffect {
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
fn route_writers_surface_unknown_publication_central() {
    check_route_writer_unknown("central");
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
    store.publish_shared(&initial, &control).unwrap();
    store.fail_parent_sync_on_next_publish();
    let result = match writer {
        "append" => super::append_host_route_record(store.path(), &record),
        "central" => super::publish_central_route_record(store.path(), record.clone()),
        _ => delete_route(store.path(), &record),
    };
    let (routes, visible_control) = store.load_shared().unwrap();
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
        delete_route(store.path(), &record).unwrap();
        super::append_host_route_record(store.path(), &record).unwrap();
    } else if writer == "central" {
        super::publish_central_route_record(store.path(), record).unwrap();
    } else {
        delete_route(store.path(), &record).unwrap();
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
