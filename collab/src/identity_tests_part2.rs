use super::*;
#[test]
fn runtime_identity_serializes_typed_fields() {
    let identity = runtime_identity(3, "binding-1");
    let encoded = serde_json::to_value(&identity).unwrap();
    assert_eq!(
        encoded,
        serde_json::json!({
            "agent_id": "agent-1",
            "runtime_id": "runtime-1",
            "appserver_id": "appserver-1",
            "endpoint_generation": 3,
            "binding_id": "binding-1",
            "session_id": "session-1",
            "native_thread_id": "thread-1"
        })
    );
    assert_eq!(
        serde_json::from_value::<RuntimeIdentity>(encoded).unwrap(),
        identity
    );
}

#[test]
fn cli_adapter_identity_has_one_stable_app_scope() {
    let first = RuntimeIdentity::cli_adapter("worker-1").unwrap();
    let second = RuntimeIdentity::cli_adapter("worker-1").unwrap();
    assert_eq!(first, second);
    assert_eq!(first.appserver_id.as_str(), CLI_APP_SERVER_ID);
    assert_eq!(first.endpoint_generation, 0);
}

#[test]
fn registration_receipt_recovers_typed_command_binding() {
    let root = std::env::temp_dir().join(format!(
        "collab-registration-receipt-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let canonical_root = std::fs::canonicalize(&root).unwrap();
    let receipt = serde_json::json!({
        "typed": true,
        "worker_id": "worker-1",
        "runtime": "untrusted-channel-label",
        "transport_selected": {
            "kind": "appserver",
            "endpoint": "unix:///tmp/codex.sock",
            "namespace": "codex_tui",
            "session_id": "session-1",
            "thread_id": "thread-1",
            "capabilities": ["session_status", "read_thread", "send_message_to_thread"],
            "self_check": "server verified"
        },
        "command": {
            "cmd": "RegisterWorker",
            "binding": {
                "project_scope": canonical_root.to_str().unwrap(),
                "app_scope_id": CLI_APP_SERVER_ID,
                "agent_id": "worker-1",
                "runtime_id": "runtime-thread-1",
                "binding_id": "binding-1",
                "endpoint_generation": 4,
                "session_id": "session-1",
                "native_thread_id": "thread-1"
            }
        }
    });

    let runtime = runtime_from_registration_receipt(&receipt, "worker-1", &root).unwrap();
    assert_eq!(runtime.agent_id.as_str(), "worker-1");
    assert_eq!(runtime.runtime_id.as_str(), "runtime-thread-1");
    assert_eq!(runtime.appserver_id.as_str(), CLI_APP_SERVER_ID);
    assert_eq!(runtime.endpoint_generation, 4);
    assert_eq!(runtime.binding_id.as_str(), "binding-1");
    assert_eq!(
        runtime.native_thread_id.as_ref().unwrap().as_str(),
        "thread-1"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn role_brief_receipt_requires_the_complete_contract() {
    let missing_authority = serde_json::json!({
        "role": "managed-subagent",
        "role_task": "Execute the assigned task.",
        "responsibilities": ["Stay in scope."],
        "derivation": {"kind": "managed-subagent", "parent": "parent-1"},
        "blocked_boundary": "Report a concrete blocker.",
        "completion_action": "Return evidence.",
        "next_action": "Continue.",
        "notification_rule": "Handle priority actions."
    });
    let error = role_brief_from_registration_receipt(&serde_json::json!({
        "role_brief": missing_authority
    }))
    .unwrap_err();
    assert!(error.to_string().contains("authority"), "{error}");

    let missing_responsibilities = serde_json::json!({
        "role": "worker",
        "role_task": "Own the task.",
        "authority": {
            "managed_subagent": false,
            "must_obey_master": false,
            "may_decline_master_invite": true
        },
        "derivation": {"kind": "peer", "parent": null},
        "blocked_boundary": "Negotiate conflicts.",
        "completion_action": "Close the task.",
        "next_action": "Resume.",
        "notification_rule": "Handle priority actions."
    });
    let error = role_brief_from_registration_receipt(&serde_json::json!({
        "role_brief": missing_responsibilities
    }))
    .unwrap_err();
    assert!(error.to_string().contains("responsibilities"), "{error}");
}

#[test]
fn registration_receipt_rejects_missing_runtime_binding() {
    let root = std::env::temp_dir().join(format!(
        "collab-registration-missing-binding-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let error = runtime_from_registration_receipt(
        &serde_json::json!({"typed": true, "worker_id": "worker-1"}),
        "worker-1",
        &root,
    )
    .unwrap_err();
    assert!(error.to_string().contains("typed command.binding"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn registration_receipt_rejects_transport_binding_mismatch() {
    let root = std::env::temp_dir().join(format!(
        "collab-registration-mismatch-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let canonical_root = std::fs::canonicalize(&root).unwrap();
    let receipt = serde_json::json!({
        "typed": true,
        "worker_id": "worker-1",
        "transport_selected": {
            "kind": "appserver",
            "endpoint": "unix:///tmp/codex.sock",
            "namespace": "codex_tui",
            "session_id": "session-selected",
            "thread_id": "thread-selected",
            "capabilities": ["send_message_to_thread"],
            "self_check": "server verified"
        },
        "command": {
            "cmd": "RegisterWorker",
            "binding": {
                "project_scope": canonical_root.to_str().unwrap(),
                "app_scope_id": "app-1",
                "agent_id": "worker-1",
                "runtime_id": "runtime-1",
                "binding_id": "binding-1",
                "endpoint_generation": 1,
                "session_id": "session-selected",
                "native_thread_id": "thread-other"
            }
        }
    });
    let error = runtime_from_registration_receipt(&receipt, "worker-1", &root).unwrap_err();
    assert!(error.to_string().contains("thread/pane address"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn appserver_identity_uses_codex_thread() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-appserver");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join(".collab-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let identity =
        load_or_create_resolved_at(&host_paths, &scope, Some("thread-worker".into()), false)
            .unwrap();
    assert_eq!(identity.worker_id, "thread-worker");
    assert_eq!(identity.runtime, None);
    assert_eq!(identity.transport, None);
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn init_preserves_a_persisted_binding_until_registration_replaces_it() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-init");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join(".collab-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let mut ident =
        load_or_create_resolved_at(&host_paths, &scope, Some("codex-thread-1".into()), false)
            .unwrap();
    persist_registration_at(
        &host_paths,
        &scope,
        &mut ident,
        runtime_identity(4, "binding-appserver"),
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-1".into()),
            thread_id: Some("thread-1".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message".into()],
            self_check: "ok".into(),
        },
    )
    .unwrap();

    let previous_worker = std::env::var_os("COLLAB_WORKER");
    let previous_thread = std::env::var_os("CODEX_THREAD_ID");
    std::env::remove_var("CODEX_THREAD_ID");
    std::env::set_var("COLLAB_WORKER", "codex-thread-1");
    let resolved = load_or_create_for_init_at(&host_paths, &scope, None).unwrap();
    assert_eq!(resolved.worker_id, "codex-thread-1");
    assert_eq!(resolved.token, ident.token);
    assert_eq!(resolved.runtime, ident.runtime);
    assert_eq!(resolved.transport, ident.transport);
    match previous_worker {
        Some(value) => std::env::set_var("COLLAB_WORKER", value),
        None => std::env::remove_var("COLLAB_WORKER"),
    }
    match previous_thread {
        Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
        None => std::env::remove_var("CODEX_THREAD_ID"),
    }
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

/// A different thread cannot adopt a persisted peer by project scope alone.
/// tmux recovery requires at least one matching durable session/thread/pane
/// anchor; App Server route liveness is no longer an identity oracle.
#[test]
fn init_rejects_identity_recovery_without_a_matching_anchor() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = short_test_root();
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join("global");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    persist_peer_at(
        &host_paths,
        &scope,
        "other-peer",
        "session-other",
        "thread-other",
        1,
    );

    let adopted = with_current_address("thread-intruder", "session-intruder", || {
        load_or_create_resolved_at(&host_paths, &scope, None, true)
    });

    assert!(adopted
        .unwrap_err()
        .to_string()
        .contains("IDENTITY_REBIND_UNPROVEN"));
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

/// `context` is the one-shot recovery command, so it may mint a fresh peer
/// after stale project identities are present. Ordinary command loading must
/// still fail closed and require an explicit worker selection.
#[test]
fn context_loads_or_creates_without_manual_worker_selection() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = short_test_root();
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join("global");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    persist_peer_at(
        &host_paths,
        &scope,
        "other-peer",
        "session-other",
        "thread-other",
        1,
    );

    let ordinary = with_current_address("thread-new", "session-new", || {
        load_or_create_resolved_at(&host_paths, &scope, None, true)
    });
    assert!(ordinary
        .unwrap_err()
        .to_string()
        .contains("IDENTITY_REBIND_UNPROVEN"));

    let created = with_current_address("thread-new", "session-new", || {
        load_or_create_full(&host_paths, &scope, None, true, true)
    });

    let created = created.unwrap();
    assert_ne!(created.worker_id, "other-peer");
    let persisted = read_identity(&identity_path_at(&host_paths, &created.worker_id).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(persisted.worker_id, created.worker_id);
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

/// A dead App Server route cannot substitute for one of the approved tmux
/// identity anchors; mismatched session/thread must not recover this peer.
#[test]
fn init_rejects_old_route_death_without_a_matching_tmux_anchor() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = short_test_root();
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join("global");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let original = persist_peer_at(
        &host_paths,
        &scope,
        "agent-peer",
        "session-old",
        "thread-old",
        1,
    );
    let _original_token = original.token.clone();

    let restored = with_current_address("thread-new", "session-new", || {
        load_or_create_resolved_at(&host_paths, &scope, None, true)
    });

    assert!(restored
        .unwrap_err()
        .to_string()
        .contains("IDENTITY_REBIND_UNPROVEN"));
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

/// Explicit identity selection is the deterministic recovery path: it needs
/// no liveness proof because the caller named the identity.
#[test]
fn explicit_worker_selection_rebinds_without_a_liveness_probe() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = short_test_root();
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join("global");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let original = persist_peer_at(
        &host_paths,
        &scope,
        "agent-peer",
        "session-old",
        "thread-old",
        1,
    );
    let original_token = original.token.clone();

    // No route authority is listening: explicit selection must not need one.
    let selected = with_current_address("thread-new", "session-new", || {
        load_or_create_resolved_at(&host_paths, &scope, Some("agent-peer".into()), true)
    });

    let selected = selected.unwrap();
    assert_eq!(selected.worker_id, "agent-peer");
    assert_eq!(selected.token, original_token);

    // The recovery skill names the identity through the environment, not
    // through an argument: that path must behave identically.
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    std::env::set_var("COLLAB_WORKER", "agent-peer");
    let selected_env = load_or_create_resolved_at(&host_paths, &scope, None, true);
    match previous_worker {
        Some(value) => std::env::set_var("COLLAB_WORKER", value),
        None => std::env::remove_var("COLLAB_WORKER"),
    }
    let selected_env = selected_env.unwrap();
    assert_eq!(selected_env.worker_id, "agent-peer");
    assert_eq!(selected_env.token, original_token);
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn explicit_worker_selection_rejects_conflicting_identity_anchor() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = short_test_root();
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join("global");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    persist_peer_at(
        &host_paths,
        &scope,
        "anchored-peer",
        "session-a",
        "thread-a",
        1,
    );
    persist_peer_at(
        &host_paths,
        &scope,
        "explicit-peer",
        "session-b",
        "thread-b",
        1,
    );

    let result = with_current_address("thread-a", "session-a", || {
        load_or_create_resolved_at(&host_paths, &scope, Some("explicit-peer".into()), true)
    });

    let error = result.unwrap_err().to_string();
    assert!(error.starts_with("IDENTITY_RESTORE_CONFLICT:"), "{error}");
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

/// With multiple persisted peers and no matching stable anchor, init must
/// fail closed. Anchor ambiguity itself is covered by the tmux recovery
/// tests above.
#[test]
fn init_fails_closed_when_multiple_peers_exist_without_matching_anchor() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = short_test_root();
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join("global");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    persist_peer_at(&host_paths, &scope, "agent-a", "session-a", "thread-a", 1);
    persist_peer_at(&host_paths, &scope, "agent-b", "session-b", "thread-b", 1);

    let restored = with_current_address("thread-new", "session-new", || {
        load_or_create_resolved_at(&host_paths, &scope, None, true)
    });

    let error = restored.unwrap_err().to_string();
    assert!(error.contains("IDENTITY_REBIND_UNPROVEN"), "{error}");
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

/// An old App Server identity without a matching tmux/Codex anchor cannot
/// be recovered by route-death inference.
#[test]
fn ordinary_commands_reject_appserver_only_identity_match() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = short_test_root();
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join("global");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    persist_peer_at(
        &host_paths,
        &scope,
        "agent-peer",
        "session-old",
        "thread-old",
        1,
    );
    let resolved = with_current_address("thread-new", "session-new", || {
        load_or_create_resolved_at(&host_paths, &scope, None, true)
    });
    assert!(resolved
        .unwrap_err()
        .to_string()
        .starts_with("IDENTITY_REBIND_UNPROVEN"));
    assert!(
        !state_root
            .join("identities")
            .join("codex-thread-new")
            .exists(),
        "ordinary commands must not mint a second identity for a rotated address"
    );
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

/// An ordinary command with a persisted project identity but no matching
/// tmux/Codex anchor must not silently mint a second identity.
#[test]
fn ordinary_commands_fail_closed_without_a_matching_tmux_anchor() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = short_test_root();
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join("global");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let victim = persist_peer_at(
        &host_paths,
        &scope,
        "victim-peer",
        "session-victim",
        "thread-victim",
        1,
    );

    let outcome = with_current_address("thread-new", "session-new", || {
        load_or_create_resolved_at(&host_paths, &scope, None, true)
    });

    let error = outcome.unwrap_err().to_string();
    assert!(error.starts_with("IDENTITY_REBIND_UNPROVEN"), "{error}");
    assert!(!error.contains(&victim.token), "{error}");
    assert!(
        !state_root
            .join("identities")
            .join("codex-thread-new")
            .exists(),
        "an unreachable authority must not mint a replacement identity"
    );
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

/// A live App Server route is not consulted or adopted when the current
/// tmux/Codex identity anchors do not match.
#[test]
fn ordinary_commands_do_not_adopt_a_project_peer_without_anchor() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = short_test_root();
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join("global");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let victim = persist_peer_at(
        &host_paths,
        &scope,
        "victim-peer",
        "session-victim",
        "thread-victim",
        1,
    );

    let resolved = with_current_address("thread-intruder", "session-intruder", || {
        load_or_create_resolved_at(&host_paths, &scope, None, true)
    });
    let error = resolved.unwrap_err().to_string();
    assert!(error.starts_with("IDENTITY_REBIND_UNPROVEN"), "{error}");
    assert!(!error.contains(&victim.token));
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn persist_runtime_updates_all_identity_state_after_validation() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-persist");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join(".collab-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let mut identity = Identity {
        worker_id: "agent-1".into(),
        token: "token-1".into(),
        project_scope: None,
        runtime: None,
        transport: None,
    };
    let runtime = runtime_identity(7, "binding-7");
    persist_registration_at(
        &host_paths,
        &scope,
        &mut identity,
        runtime.clone(),
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-1".into()),
            thread_id: Some("thread-1".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message".into()],
            self_check: "server verified".into(),
        },
    )
    .unwrap();
    assert_eq!(identity.runtime, Some(runtime.clone()));
    assert_eq!(
        identity.transport.as_ref().unwrap().kind,
        TransportKind::AppServer
    );
    let persisted = read_identity(&identity_path_at(&host_paths, "agent-1").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(persisted.runtime, Some(runtime));
    std::fs::remove_dir_all(state_root).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn identity_writes_use_distinct_temporary_paths() {
    let path = std::path::Path::new("/tmp/collab-identity/identity.json");
    let first = identity_temp_path(path);
    let second = identity_temp_path(path);
    assert_ne!(first, second);
}

#[test]
fn persisted_registration_records_the_current_project_scope() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = test_root("ci-project-scope");
    std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
    let state_root = root.join(".collab-state");
    std::fs::create_dir_all(&state_root).unwrap();
    let scope = test_scope(root.clone());
    let host_paths = HostPaths::for_state_root(&state_root).unwrap();
    let mut identity = Identity {
        worker_id: "agent-1".into(),
        token: "token-1".into(),
        project_scope: None,
        runtime: None,
        transport: None,
    };

    persist_registration_at(
        &host_paths,
        &scope,
        &mut identity,
        runtime_identity(7, "binding-7"),
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-1".into()),
            thread_id: Some("thread-1".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message".into()],
            self_check: "server verified".into(),
        },
    )
    .unwrap();

    let expected = scope
        .route_scope(AppServerId::new("appserver-1").unwrap())
        .unwrap()
        .project_scope_id;
    assert_eq!(identity.project_scope.as_ref(), Some(&expected));
    let persisted = read_identity(&identity_path_at(&host_paths, "agent-1").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(persisted.project_scope.as_ref(), Some(&expected));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn binding_validation_rejects_stale_generation_without_mutating_inputs() {
    let registered = runtime_identity(4, "binding-1");
    let incoming = runtime_identity(3, "binding-1");
    let registered_before = registered.clone();
    let incoming_before = incoming.clone();

    assert!(matches!(
        validate_binding(&registered, &incoming),
        Err(BindingValidationError::StaleGeneration {
            expected: 4,
            observed: 3
        })
    ));
    assert_eq!(registered, registered_before);
    assert_eq!(incoming, incoming_before);
}

#[test]
fn binding_validation_rejects_wrong_binding_without_mutating_inputs() {
    let registered = runtime_identity(4, "binding-1");
    let incoming = runtime_identity(4, "binding-2");
    let registered_before = registered.clone();
    let incoming_before = incoming.clone();

    assert!(matches!(
        validate_binding(&registered, &incoming),
        Err(BindingValidationError::Mismatch {
            field: "binding_id",
            ..
        })
    ));
    assert_eq!(registered, registered_before);
    assert_eq!(incoming, incoming_before);
}

#[test]
fn identifier_validation_rejects_empty_and_control_values() {
    assert!(AgentId::new("").is_err());
    assert!(RuntimeId::new("runtime\n1").is_err());
    assert!(DispatchId::new("d".repeat(MAX_ID_LENGTH + 1)).is_err());
}

/// A short temp project root: the fake route-authority socket lives under the
/// state root, so the combined path must stay under `sockaddr_un`'s limit.
fn short_test_root() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    // Keep the whole `<root>/global/server.sock` path short: unix socket paths
    // are limited to ~104 bytes and the macOS temp dir already uses ~48.
    std::env::temp_dir().join(format!(
        "cs{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Build one persisted identity whose dual key is the given address.
fn persist_peer_at(
    host_paths: &HostPaths,
    scope: &Scope,
    worker: &str,
    session: &str,
    thread: &str,
    generation: u64,
) -> Identity {
    let mut identity =
        load_or_create_resolved_at(host_paths, scope, Some(worker.into()), false).unwrap();
    let mut runtime = runtime_identity(generation, &format!("binding-{worker}"));
    runtime.session_id = Some(SessionId::new(session).unwrap());
    runtime.native_thread_id = Some(NativeThreadId::new(thread).unwrap());
    persist_registration_at(
        host_paths,
        scope,
        &mut identity,
        runtime,
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/codex.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some(session.into()),
            thread_id: Some(thread.into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message".into()],
            self_check: "server verified".into(),
        },
    )
    .unwrap();
    identity
}
fn with_current_address<T>(thread: &str, session: &str, body: impl FnOnce() -> T) -> T {
    let previous_thread = std::env::var_os("CODEX_THREAD_ID");
    let previous_session = std::env::var_os("CODEX_SESSION_ID");
    let previous_worker = std::env::var_os("COLLAB_WORKER");
    std::env::set_var("CODEX_THREAD_ID", thread);
    std::env::set_var("CODEX_SESSION_ID", session);
    std::env::remove_var("COLLAB_WORKER");
    let result = body();
    match previous_thread {
        Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
        None => std::env::remove_var("CODEX_THREAD_ID"),
    }
    match previous_session {
        Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
        None => std::env::remove_var("CODEX_SESSION_ID"),
    }
    match previous_worker {
        Some(value) => std::env::set_var("COLLAB_WORKER", value),
        None => std::env::remove_var("COLLAB_WORKER"),
    }
    result
}
