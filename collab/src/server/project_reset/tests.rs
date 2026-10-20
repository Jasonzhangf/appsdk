use super::super::*;

fn fixture() -> (
    Arc<Server>,
    PathBuf,
    Arc<ProjectRuntimeManager>,
    ProjectContext,
) {
    let (server, root) = peer_tests::test_server();
    let server = Arc::new(server);
    server
        .commit_checked(&[
            Event::Registered {
                worker: state::WorkerRec {
                    id: "reset-master".into(),
                    token: "reset-token".into(),
                    cwd: root.display().to_string(),
                    registered_ms: now_ms(),
                    transport: None,
                },
            },
            Event::MasterAssigned {
                worker_id: "reset-master".into(),
                assigned_by: "test".into(),
                approval: None,
                assigned_ms: now_ms(),
            },
        ])
        .unwrap();
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let context = ProjectContext::for_registered_root_with_app(
        &root,
        AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
    )
    .unwrap();
    manager.install_runtime(
        &ProjectRuntimeManager::route_key(&context),
        server.clone(),
        None,
    );
    let binding = RuntimeBinding::new_with_session(
        context.project_scope.clone(),
        context.app_scope_id.clone(),
        AgentId::new("reset-master").unwrap(),
        RuntimeId::new("reset-runtime").unwrap(),
        BindingId::new("reset-binding").unwrap(),
        1,
        Some(crate::identity::SessionId::new("reset-session").unwrap()),
        Some(NativeThreadId::new("reset-thread").unwrap()),
    )
    .unwrap();
    let grant = global_state::MasterGrant::new(
        context.project_scope.clone(),
        context.app_scope_id.clone(),
        binding.agent_id.clone(),
        "project",
        "test",
        "test authorized master",
        binding.binding_id.clone(),
        1,
        now_ms(),
    )
    .unwrap();
    server
        .commit_checked(&[
            Event::GlobalProjectRegistered {
                registration: ProjectRegistration::new(
                    context.project_scope.clone(),
                    context.app_scope_id.clone(),
                )
                .unwrap(),
            },
            Event::GlobalRuntimeBound {
                binding: binding.clone(),
            },
            Event::GlobalCurrentThreadRouteSet { binding },
            Event::GlobalMasterGranted { grant },
        ])
        .unwrap();
    (server, root, manager, context)
}

#[test]
fn online_reset_live_manager_succeeds() {
    let (server, root, manager, context) = fixture();
    populate(&server, "reset-master", "A");
    let pid = std::process::id();
    let result = manager.reset_project(&context, "reset-master", "reset-token");
    assert!(
        result.ok,
        "online live project reset must succeed: {result:?}"
    );
    assert_eq!(pid, std::process::id());
    assert!(server.state.lock().unwrap().workers.is_empty());
    assert!(server.state.lock().unwrap().tasks.is_empty());
    assert!(server.state.lock().unwrap().msgs.is_empty());
    assert!(server
        .state
        .lock()
        .unwrap()
        .notification_subscriptions
        .is_empty());
    assert!(!manager.has_project_route(context.project_scope.as_str()));
    assert!(
        server.journal_path.exists(),
        "shared host journal must survive reset"
    );
    let replayed = replay_from_journal(&root, &server.journal_path).unwrap();
    assert!(replayed.workers.is_empty());
    assert!(replayed
        .global
        .projects
        .get(context.project_scope.as_str())
        .is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn online_reset_preserves_other_project_and_releases_all_app_scopes() {
    let (server, root, manager, context) = fixture();
    let (other, other_root) = peer_tests::test_server();
    let other = Arc::new(other);
    let other_context = ProjectContext::for_registered_root_with_app(
        &other_root,
        AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
    )
    .unwrap();
    manager.install_runtime(
        &ProjectRuntimeManager::route_key(&other_context),
        other.clone(),
        None,
    );
    let other_registration = ProjectRegistration::new(
        other_context.project_scope.clone(),
        other_context.app_scope_id.clone(),
    )
    .unwrap();
    server
        .commit_checked(&[Event::GlobalProjectRegistered {
            registration: other_registration,
        }])
        .unwrap();
    let other_worker = state::WorkerRec {
        id: "other-worker".into(),
        token: "other-token".into(),
        cwd: other_root.display().to_string(),
        registered_ms: now_ms(),
        transport: None,
    };
    server
        .commit_checked(&[Event::Registered {
            worker: other_worker.clone(),
        }])
        .unwrap();
    other
        .commit_checked(&[Event::Registered {
            worker: other_worker,
        }])
        .unwrap();
    populate(&server, "reset-master", "A");
    populate(&other, "other-worker", "B");
    for (label, scope) in [
        ("A", context.project_scope.as_str()),
        ("B", other_context.project_scope.as_str()),
    ] {
        server
            .commit_command(
                &format!("receipt-{label}"),
                &format!("receipt-operation-{label}"),
                &[],
                json!({"command": {"binding": {"project_scope": scope}}}),
            )
            .unwrap();
    }
    manager.install_runtime(
        &("another-app".into(), context.project_scope.as_str().into()),
        server.clone(),
        None,
    );
    let payload = root.join(".agent-collab/payload.bin");
    let other_payload = other_root.join(".agent-collab/payload.bin");
    std::fs::write(&payload, "remove A").unwrap();
    std::fs::write(&other_payload, "keep B").unwrap();
    let old_other = manager.select_runtime(&other_context).unwrap();
    let result = manager.reset_project(&context, "reset-master", "reset-token");
    assert!(result.ok, "{result:?}");
    assert!(!payload.exists());
    assert_eq!(std::fs::read_to_string(&other_payload).unwrap(), "keep B");
    assert!(!manager.has_project_route(context.project_scope.as_str()));
    assert!(Arc::ptr_eq(
        &old_other,
        &manager.select_runtime(&other_context).unwrap()
    ));
    assert!(server
        .state
        .lock()
        .unwrap()
        .workers
        .contains_key("other-worker"));
    assert!(!server
        .state
        .lock()
        .unwrap()
        .global
        .command_receipts
        .contains_key("receipt-A"));
    assert!(server
        .state
        .lock()
        .unwrap()
        .global
        .command_receipts
        .contains_key("receipt-B"));
    assert!(other
        .state
        .lock()
        .unwrap()
        .workers
        .contains_key("other-worker"));
    assert!(other.state.lock().unwrap().tasks.contains_key("task-B"));
    assert!(other.state.lock().unwrap().msgs.contains_key("message-B"));
    assert!(other
        .state
        .lock()
        .unwrap()
        .notification_subscriptions
        .contains_key("subscription-B"));
    let replayed = replay_from_journal(&root, &server.journal_path).unwrap();
    assert!(
        replayed.workers.contains_key("other-worker"),
        "workers after replay: {:?}; journal: {}",
        replayed.workers.keys(),
        std::fs::read_to_string(&server.journal_path).unwrap()
    );
    assert!(replayed
        .global
        .projects
        .contains_key(other_context.project_scope.as_str()));
    assert!(replayed.pending_project_resets.is_empty());
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(other_root).unwrap();
}

fn populate(server: &Server, worker: &str, label: &str) {
    server
        .commit_checked(&[
            Event::TaskCreated {
                task: serde_json::from_value(json!({
                    "id": format!("task-{label}"), "owner": worker, "created_by": worker,
                    "created_ms": now_ms(), "updated_ms": now_ms()
                }))
                .unwrap(),
            },
            Event::Sent {
                msg: serde_json::from_value(json!({
                        "id": format!("message-{label}"), "from": worker, "to": worker,
                "type": "text", "body": "project-owned message", "created_ms": now_ms(),
                        "state": "pending", "last_wake_attempt_ms": 0
                    }))
                .unwrap(),
            },
            Event::NotificationSubscribed {
                subscription: serde_json::from_value(json!({
                    "id": format!("subscription-{label}"), "worker_id": worker,
                    "event": "message", "subject": null, "target": worker, "method": "inbox",
                    "trigger_ms": null, "trigger_times_ms": [], "interval_ms": null,
                    "repeat_count": 1, "fired_count": 0, "expires_ms": now_ms() + 60_000,
                    "status": "active", "created_ms": now_ms(), "updated_ms": now_ms()
                }))
                .unwrap(),
            },
        ])
        .unwrap();
}

#[test]
fn online_reset_nonmaster_cannot_delete() {
    let (server, root, manager, context) = fixture();
    server
        .commit_checked(&[Event::Registered {
            worker: state::WorkerRec {
                id: "peer".into(),
                token: "peer-token".into(),
                cwd: root.display().to_string(),
                registered_ms: now_ms(),
                transport: None,
            },
        }])
        .unwrap();
    let result = manager.reset_project(&context, "peer", "peer-token");
    assert!(!result.ok);
    assert!(result.error.unwrap().contains("RESET_MASTER_REQUIRED"));
    assert!(server
        .state
        .lock()
        .unwrap()
        .workers
        .contains_key("reset-master"));
    assert!(server
        .state
        .lock()
        .unwrap()
        .pending_project_resets
        .is_empty());
    assert!(manager
        .reset_project(&context, "reset-master", "bad-token")
        .error
        .unwrap()
        .contains("token mismatch"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn online_reset_real_storage_error_is_incomplete_and_startup_finishes() {
    let (server, root, manager, context) = fixture();
    // A regular file where the identities owner needs a directory produces
    // a real filesystem error, independent of uid or chmod semantics.
    let identity_root = server.host_paths.state_root().join("identities");
    std::fs::write(&identity_root, "not a directory").unwrap();
    let result = manager.reset_project(&context, "reset-master", "reset-token");
    assert!(!result.ok);
    assert!(result.error.unwrap().contains("RESET_INCOMPLETE"));
    assert!(server
        .state
        .lock()
        .unwrap()
        .pending_project_resets
        .contains_key(context.project_scope.as_str()));
    let replayed = replay_from_journal(&root, &server.journal_path).unwrap();
    assert!(replayed.workers.is_empty());
    assert!(replayed
        .pending_project_resets
        .contains_key(context.project_scope.as_str()));
    std::fs::remove_file(identity_root).unwrap();
    let recovered = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    assert!(server
        .state
        .lock()
        .unwrap()
        .pending_project_resets
        .is_empty());
    assert!(!recovered.has_project_route(context.project_scope.as_str()));
    assert!(server.state.lock().unwrap().workers.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_route_cleanup_is_durable_idempotent_and_retains_unknown_live() {
    let (server, root, manager, _) = fixture();
    let missing = root.join("missing-project");
    let missing_record = HostRouteRecord {
        version: 1,
        op: "register".into(),
        app_scope_id: "stale-app".into(),
        project_scope: missing.display().to_string(),
        canonical_root: missing.display().to_string(),
        storage_root: missing.display().to_string(),
        registered_ms: now_ms(),
    };
    append_host_route_record(&manager.route_journal, &missing_record).unwrap();
    let live = HostRouteRecord {
        version: 1,
        op: "register".into(),
        app_scope_id: crate::identity::CLI_APP_SERVER_ID.into(),
        project_scope: root.display().to_string(),
        canonical_root: root.display().to_string(),
        storage_root: root.display().to_string(),
        registered_ms: now_ms(),
    };
    append_host_route_record(&manager.route_journal, &live).unwrap();
    let stale_key = (
        missing_record.app_scope_id.clone(),
        missing_record.project_scope.clone(),
    );
    manager.install_pending_route(&stale_key, &missing, &missing);
    let failure = manager
        .ensure_runtime(&stale_key, &missing, &missing)
        .err()
        .unwrap();
    assert!(failure.contains("deinitialized project routes were retired"));
    assert!(
        !missing.exists(),
        "stale materialization must not create project storage"
    );
    let records = load_host_route_records(&manager.route_journal).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].project_scope, root.display().to_string());
    let bytes = std::fs::read(&manager.route_journal).unwrap();
    manager.prune_stale_routes().unwrap();
    assert_eq!(bytes, std::fs::read(&manager.route_journal).unwrap());
    assert!(server
        .state
        .lock()
        .unwrap()
        .workers
        .contains_key("reset-master"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn online_reset_removes_host_and_retired_identities_only_for_target_project() {
    let (server, root, manager, context) = fixture();
    let current = server.host_paths.state_root().join("identities");
    let retired = server
        .host_paths
        .state_root()
        .join("archives/identities-retired-test");
    for directory in [&current, &retired] {
        for (worker, scope) in [
            ("worker-A", context.project_scope.as_str()),
            ("worker-B", "/unrelated/project"),
        ] {
            let path = directory.join(worker);
            std::fs::create_dir_all(&path).unwrap();
            let identity = crate::identity::Identity {
                worker_id: worker.into(),
                token: "test-token".into(),
                project_scope: Some(ProjectScopeId::new(scope).unwrap()),
                runtime: None,
                transport: None,
            };
            std::fs::write(
                path.join("identity.json"),
                serde_json::to_vec(&identity).unwrap(),
            )
            .unwrap();
        }
    }
    assert!(
        manager
            .reset_project(&context, "reset-master", "reset-token")
            .ok
    );
    for directory in [&current, &retired] {
        assert!(!directory.join("worker-A").exists());
        assert!(directory.join("worker-B/identity.json").exists());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn resident_route_recovery_and_other_project_reset_have_one_lock_order() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let (_host, root, manager, resident) = fixture();
    let (other, other_root, _other_manager, other_context) = fixture();
    manager.install_runtime(
        &ProjectRuntimeManager::route_key(&other_context),
        other,
        None,
    );
    assert!(
        manager
            .reset_project(&resident, "reset-master", "reset-token")
            .ok
    );
    let materialization = manager.materialization_gate.lock().unwrap();
    let (result_tx, result_rx) = mpsc::channel();
    let resetting = manager.clone();
    let reset_tx = result_tx.clone();
    let reset = std::thread::spawn(move || {
        let result = resetting.reset_project(&other_context, "reset-master", "reset-token");
        reset_tx.send(result.ok).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while manager.identity_gate.try_lock().is_ok() && Instant::now() < deadline {
        std::thread::yield_now();
    }
    let observation = Instant::now() + Duration::from_millis(100);
    let mut publication_held_before_materialization = false;
    while Instant::now() < observation {
        if manager.register_gate.try_lock().is_err() {
            publication_held_before_materialization = true;
            break;
        }
        std::thread::yield_now();
    }
    let recovering = manager.clone();
    let recovery = std::thread::spawn(move || {
        let request: crate::proto::IdentityContextRequest = serde_json::from_value(json!({
            "operation_id":"concurrent-resident-context", "invocation":"automatic", "action":"context"
        })).unwrap();
        result_tx
            .send(
                recovering
                    .ensure_resident_identity_context_route(&resident, &request)
                    .is_ok(),
            )
            .unwrap();
    });
    drop(materialization);
    assert!(result_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("first request must finish"));
    assert!(result_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("second request must finish"));
    reset.join().unwrap();
    recovery.join().unwrap();
    assert!(!publication_held_before_materialization,
        "reset must acquire materialization before route publication, like resident context recovery");
    assert!(manager.host.state.lock().is_ok());
    assert!(manager.dispatch_sync(None, Req::Ping).1.ok);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(other_root).unwrap();
}

#[test]
fn online_reset_durability_error_does_not_delete_payload() {
    let (server, root, manager, context) = fixture();
    let payload = root.join(".agent-collab/payload");
    std::fs::write(&payload, "must remain").unwrap();
    *server.journal.lock().unwrap() = std::fs::File::open(&server.journal_path).unwrap();
    let result = manager.reset_project(&context, "reset-master", "reset-token");
    assert!(!result.ok);
    assert!(result.error.unwrap().contains("RESET_INCOMPLETE"));
    assert!(payload.exists());
    assert!(server
        .state
        .lock()
        .unwrap()
        .workers
        .contains_key("reset-master"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn online_reset_forgets_project_operations_and_reservations_on_replay() {
    let (_server, root, manager, context) = fixture();
    for (id, scope) in [
        ("operation-A", context.project_scope.as_str()),
        ("operation-B", "/unrelated/project"),
    ] {
        manager
            .operation_journal
            .append(operation_journal::OperationAdmission {
                operation_id: id.into(),
                project_scope: scope.into(),
                app_scope_id: crate::identity::CLI_APP_SERVER_ID.into(),
                action: "context".into(),
                invocation: "automatic".into(),
                intent_digest: "test-intent".into(),
                query_capability_hash: operation_journal::capability_hash("test-capability"),
                phase: crate::proto::IdentityOperationPhase::Admitted,
                committed_phases: Vec::new(),
                nested_command_id: None,
                nested_operation_id: None,
                approval_evidence: None,
            })
            .unwrap();
    }
    assert_eq!(
        operation_journal::peek_query_journal(manager.operation_journal.path()).len(),
        2
    );
    assert!(
        manager
            .reset_project(&context, "reset-master", "reset-token")
            .ok
    );
    let replayed = operation_journal::peek_query_journal(manager.operation_journal.path());
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].operation_id, "operation-B");
    let bytes = std::fs::read_to_string(manager.operation_journal.path()).unwrap();
    assert!(
        !bytes.contains("operation-A"),
        "reset must erase historical operation bytes"
    );
    assert!(bytes.contains("operation-B"));
    let reopened =
        operation_journal::OperationJournal::open(manager.operation_journal.path()).unwrap();
    assert_eq!(
        operation_journal::peek_query_journal(reopened.path()).len(),
        1
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn online_reset_external_project_drops_runtime_and_its_journal() {
    let (host, host_root, manager, host_context) = fixture();
    let (external, external_root, _external_manager, external_context) = fixture();
    manager.install_runtime(
        &ProjectRuntimeManager::route_key(&external_context),
        external.clone(),
        None,
    );
    populate(&host, "reset-master", "host");
    populate(&external, "reset-master", "external");
    let external_record = HostRouteRecord {
        version: 1,
        op: "register".into(),
        app_scope_id: external_context.app_scope_id.as_str().into(),
        project_scope: external_context.project_scope.as_str().into(),
        canonical_root: external_context.canonical_root.clone(),
        storage_root: external_context.canonical_root.clone(),
        registered_ms: now_ms(),
    };
    append_host_route_record(&manager.route_journal, &external_record).unwrap();
    let result = manager.reset_project(&external_context, "reset-master", "reset-token");
    assert!(result.ok, "{result:?}");
    assert!(!external.journal_path.exists());
    assert!(!external_root.join(".agent-collab").exists());
    assert!(external.state.lock().unwrap().workers.is_empty());
    assert!(external.state.lock().unwrap().admission_frozen());
    assert!(host.journal_path.exists());
    assert!(host
        .state
        .lock()
        .unwrap()
        .workers
        .contains_key("reset-master"));
    assert!(host.state.lock().unwrap().tasks.contains_key("task-host"));
    assert!(Arc::ptr_eq(
        &host,
        &manager.select_runtime(&host_context).unwrap()
    ));
    assert!(!manager.has_project_route(external_context.project_scope.as_str()));
    assert!(load_host_route_records(&manager.route_journal)
        .unwrap()
        .is_empty());
    std::fs::remove_dir_all(host_root).unwrap();
    std::fs::remove_dir_all(external_root).unwrap();
}

#[test]
fn stale_superseded_route_is_retired_without_touching_live_unknown_route() {
    let (host, root, manager, _) = fixture();
    let (other, other_root, _other_manager, other_context) = fixture();
    let original = other
        .state
        .lock()
        .unwrap()
        .global
        .current_thread_routes
        .values()
        .next()
        .unwrap()
        .clone();
    // Use a distinct address because both fixtures deliberately share simple
    // synthetic native IDs. No transport probe is part of this test.
    let mut original = original;
    original.native_thread_id = Some(NativeThreadId::new("other-thread").unwrap());
    original.session_id = Some(crate::identity::SessionId::new("other-session").unwrap());
    original.endpoint_generation += 1;
    other
        .commit_checked(&[Event::GlobalRuntimeBound {
            binding: original.clone(),
        }])
        .unwrap();
    host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
        binding: original.clone(),
    }])
    .unwrap();
    manager.install_runtime(
        &ProjectRuntimeManager::route_key(&other_context),
        other.clone(),
        None,
    );
    let mut retired = original.clone();
    retired.endpoint_generation += 1;
    retired.native_thread_id = None;
    retired.tmux_endpoint = None;
    other
        .commit_checked(&[Event::GlobalRuntimeBound { binding: retired }])
        .unwrap();
    manager.prune_stale_routes().unwrap();
    let state = host.state.lock().unwrap();
    assert!(!state
        .global
        .current_thread_routes
        .values()
        .any(|b| b.project_scope == other_context.project_scope));
    assert!(state
        .global
        .current_thread_routes
        .values()
        .any(|b| b.project_scope.as_str() == manager.host_root.to_string_lossy()));
    drop(state);
    host.commit_checked(&[Event::GlobalCurrentThreadRouteSet {
        binding: original.clone(),
    }])
    .unwrap();
    other
        .commit_checked(&[Event::ProjectResetStarted {
            project_scope: other_context.project_scope.as_str().into(),
            clear_local: true,
            worker_ids: Vec::new(),
        }])
        .unwrap();
    manager.prune_stale_routes().unwrap();
    let replayed = replay_from_journal(&root, &host.journal_path).unwrap();
    assert!(!replayed
        .global
        .current_thread_routes
        .values()
        .any(|b| b.project_scope == other_context.project_scope));
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(other_root).unwrap();
}
