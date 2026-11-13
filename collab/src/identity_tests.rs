use super::*;
use crate::proto::IdentityFacts;
use crate::server::binding_slot::{BindingControlSet, BindingValue, IdentityBindingKey};
use crate::server::binding_store::BindingControlStore;

fn test_root(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{prefix}-{}-{}", std::process::id(), hex(8)))
}

fn runtime_identity(binding: &str) -> RuntimeIdentity {
    RuntimeIdentity {
        agent_id: AgentId::new("agent-1").unwrap(),
        runtime_id: RuntimeId::new("runtime-1").unwrap(),
        appserver_id: AppServerId::new(CLI_APP_SERVER_ID).unwrap(),
        binding_id: BindingId::new(binding).unwrap(),
        session_id: Some(SessionId::new("session-1").unwrap()),
        native_thread_id: Some(NativeThreadId::new("thread-1").unwrap()),
    }
}

fn selected() -> SelectedTransport {
    SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/codex.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some("session-1".into()),
        thread_id: Some("thread-1".into()),
        tmux_endpoint: None,
        rex_endpoint: None,
        capabilities: vec!["send_message".into()],
        self_check: "verified fixture".into(),
    }
}

#[test]
fn rex_registration_uses_an_agent_session_and_a_separate_terminal_endpoint() {
    let mut transport = selected();
    transport.kind = TransportKind::Rex;
    transport.endpoint = None;
    transport.namespace = None;
    transport.thread_id = None;
    transport.rex_endpoint = Some(crate::proto::RexEndpoint {
        server: "unix:///tmp/rex.sock".into(),
        session_id: "session:11111111-1111-1111-1111-111111111111".into(),
        block_id: "block:22222222-2222-2222-2222-222222222222".into(),
        host_id: Some("host".into()),
        instance_id: "instance".into(),
        cwd: "/remote/project".into(),
    });
    let mut runtime = runtime_identity("binding-rex");
    runtime.native_thread_id = None;
    validate_registration_transport(&transport, &runtime).unwrap();
    runtime.native_thread_id = Some(NativeThreadId::new("wrong-thread").unwrap());
    assert!(validate_registration_transport(&transport, &runtime).is_err());
}

fn credential(paths: &HostPaths, scope: &Scope) -> Identity {
    let mut identity = draft_identity_with_id(scope, "agent-1").unwrap();
    persist_current_binding_at(
        paths,
        scope,
        &mut identity,
        runtime_identity("binding-1"),
        selected(),
    )
    .unwrap();
    identity
}

fn mapped(paths: &HostPaths, identity: &Identity) {
    let runtime = identity.runtime.as_ref().unwrap();
    let project = identity.project_scope.clone().unwrap();
    let binding = crate::server::global_state::RuntimeBinding::new_with_session(
        project.clone(),
        runtime.appserver_id.clone(),
        runtime.agent_id.clone(),
        runtime.runtime_id.clone(),
        runtime.binding_id.clone(),
        runtime.session_id.clone(),
        runtime.native_thread_id.clone(),
    )
    .unwrap();
    let key = IdentityBindingKey::new(project, runtime.appserver_id.clone(), "Alice");
    let mut control = BindingControlSet::new();
    control
        .commit(&key, BindingValue::new(binding, selected()), None)
        .unwrap();
    BindingControlStore::new(paths.state_root())
        .save(&control)
        .unwrap();
}

#[test]
fn registration_persists_exact_scope_and_owner_only_credential() {
    use std::os::unix::fs::PermissionsExt;
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("identity-credential");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = Scope { root: root.clone() };
    let paths = HostPaths::for_state_root(&root.join("state")).unwrap();
    let identity = credential(&paths, &scope);
    let path = identity_path_at(&paths, "agent-1").unwrap();
    assert_eq!(
        serde_json::to_value(read_identity(&path).unwrap().unwrap()).unwrap(),
        serde_json::to_value(&identity).unwrap()
    );
    assert_eq!(
        identity.project_scope.unwrap().as_str(),
        std::fs::canonicalize(&root).unwrap().to_str().unwrap()
    );
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unpublished_registration_credential_restores_exact_original_bytes_or_absence() {
    use std::os::unix::fs::PermissionsExt;
    let root = test_root("identity-staging-rollback");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = Scope { root: root.clone() };
    let paths = HostPaths::for_state_root(&root.join("state")).unwrap();
    let path = identity_path_at(&paths, "agent-1").unwrap();
    for original in [
        None,
        Some(b"original unselected credential bytes\n".to_vec()),
    ] {
        if let Some(bytes) = original.as_ref() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
        }
        let identity = draft_identity_with_id(&scope, "agent-1").unwrap();
        let token = identity.token.clone();
        let staged = stage_registration_credential(
            &paths,
            &scope,
            identity,
            runtime_identity("binding-1"),
            selected(),
        )
        .unwrap();
        assert_eq!(read_identity(&path).unwrap().unwrap().token, token);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        staged.rollback().unwrap();
        assert_eq!(read_identity_bytes(&path).unwrap(), original);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn absent_map_never_recovers_a_credential_archive() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("identity-no-map");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = Scope { root: root.clone() };
    let paths = HostPaths::for_state_root(&root.join("state")).unwrap();
    let identity = credential(&paths, &scope);
    let facts = IdentityFacts {
        session_id: Some("session-1".into()),
        ..Default::default()
    };
    assert!(registered_from_map(&paths, &scope, &facts)
        .unwrap()
        .is_none());
    mapped(&paths, &identity);
    let restored = registered_from_map(&paths, &scope, &facts)
        .unwrap()
        .unwrap();
    assert_eq!(restored.worker_id, identity.worker_id);
    assert_eq!(restored.token, identity.token);
    std::fs::remove_file(BindingControlStore::new(paths.state_root()).path()).unwrap();
    assert!(registered_from_map(&paths, &scope, &facts)
        .unwrap()
        .is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn registered_rex_caller_cannot_borrow_a_successor_session_from_the_same_block() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("identity-rex-current-session");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = Scope { root: root.clone() };
    let paths = HostPaths::for_state_root(&root.join("state")).unwrap();
    let mut transport = selected();
    transport.kind = TransportKind::Rex;
    transport.endpoint = None;
    transport.namespace = None;
    transport.thread_id = None;
    transport.session_id = Some("successor-agent-session".into());
    transport.rex_endpoint = Some(crate::proto::RexEndpoint {
        server: "unix:///tmp/rex-current-session.sock".into(),
        session_id: "session:11111111-1111-1111-1111-111111111111".into(),
        block_id: "block:22222222-2222-2222-2222-222222222222".into(),
        host_id: Some("host".into()),
        instance_id: "instance".into(),
        cwd: root.to_string_lossy().into_owned(),
    });
    let mut runtime = runtime_identity("binding-rex-current-session");
    runtime.session_id = Some(SessionId::new("successor-agent-session").unwrap());
    runtime.native_thread_id = None;
    let mut identity = draft_identity_with_id(&scope, "agent-1").unwrap();
    persist_current_binding_at(&paths, &scope, &mut identity, runtime, transport.clone()).unwrap();
    let runtime = identity.runtime.as_ref().unwrap();
    let project = identity.project_scope.clone().unwrap();
    let binding = crate::server::global_state::RuntimeBinding::new_with_session(
        project.clone(),
        runtime.appserver_id.clone(),
        runtime.agent_id.clone(),
        runtime.runtime_id.clone(),
        runtime.binding_id.clone(),
        runtime.session_id.clone(),
        None,
    )
    .unwrap();
    BindingControlStore::new(paths.state_root())
        .commit(crate::server::binding_store::ControlChange::Commit {
            key: IdentityBindingKey::new(project, runtime.appserver_id.clone(), "Alice"),
            value: BindingValue::new(binding, transport.clone()),
            expected_generation: None,
        })
        .unwrap();
    let endpoint = transport.rex_endpoint.unwrap();
    let mut facts = IdentityFacts {
        session_id: Some("successor-agent-session".into()),
        rex: Some(crate::proto::RexCandidate {
            server: endpoint.server,
            session_id: endpoint.session_id,
            block_id: endpoint.block_id,
            cwd: endpoint.cwd,
        }),
        ..Default::default()
    };
    assert!(registered_from_map(&paths, &scope, &facts)
        .unwrap()
        .is_some());
    facts.session_id = Some("displaced-agent-session".into());
    let stale = registered_from_map(&paths, &scope, &facts);
    assert!(
        stale.is_err(),
        "ordinary commands must not acquire the successor's current identity from its block"
    );
    assert!(stale
        .unwrap_err()
        .to_string()
        .contains("IDENTITY_SESSION_NOT_CURRENT"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mapped_credential_failure_does_not_select_another_worker() {
    let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
    let root = test_root("identity-exact-credential");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let scope = Scope { root: root.clone() };
    let paths = HostPaths::for_state_root(&root.join("state")).unwrap();
    let identity = credential(&paths, &scope);
    mapped(&paths, &identity);
    std::fs::remove_file(identity_path_at(&paths, "agent-1").unwrap()).unwrap();
    let facts = IdentityFacts {
        session_id: Some("session-1".into()),
        ..Default::default()
    };
    assert!(registered_from_map(&paths, &scope, &facts)
        .unwrap_err()
        .to_string()
        .contains("IDENTITY_CREDENTIAL_MISSING"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn identity_writes_use_distinct_temporary_paths() {
    let path = Path::new("/tmp/collab-identity/identity.json");
    assert_ne!(identity_temp_path(path), identity_temp_path(path));
}

#[test]
fn binding_validation_rejects_wrong_binding_without_mutating_inputs() {
    let registered = runtime_identity("binding-1");
    let incoming = runtime_identity("binding-2");
    let before = (registered.clone(), incoming.clone());
    assert!(matches!(
        validate_binding(&registered, &incoming),
        Err(BindingValidationError::Mismatch {
            field: "binding_id",
            ..
        })
    ));
    assert_eq!((registered, incoming), before);
}

#[test]
fn identifier_validation_rejects_empty_and_control_values() {
    assert!(AgentId::new("").is_err());
    assert!(RuntimeId::new("runtime\n1").is_err());
    assert!(DispatchId::new("d".repeat(MAX_ID_LENGTH + 1)).is_err());
}
