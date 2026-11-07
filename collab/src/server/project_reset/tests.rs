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
        .commit_checked(&[Event::Registered {
            worker: state::WorkerRec {
                id: "reset-master".into(),
                token: "reset-token".into(),
                cwd: root.display().to_string(),
                registered_ms: now_ms(),
                transport: None,
            },
        }])
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
            Event::GlobalCurrentThreadRouteSet {
                binding: binding.clone(),
            },
            Event::GlobalMasterGranted { grant },
        ])
        .unwrap();
    let mut transport = peer_tests::test_appserver_transport("reset-thread");
    transport.session_id = Some("reset-session".into());
    binding_store::BindingControlStore::new(server.host_paths.state_root())
        .commit(binding_store::ControlChange::Commit {
            key: binding_slot::IdentityBindingKey::new(
                context.project_scope.clone(),
                context.app_scope_id.clone(),
                "Jenny",
            ),
            value: binding_slot::BindingValue::new(binding, transport),
            expected_generation: None,
        })
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
        server.state_path.exists(),
        "shared host current state must survive reset"
    );
    let replayed = current_state::load(&server.state_path, false).unwrap();
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
    let replayed = current_state::load(&server.state_path, false).unwrap();
    assert!(
        replayed.workers.contains_key("other-worker"),
        "workers after loading current state: {:?}",
        replayed.workers.keys()
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
fn interrupted_reset_retains_central_cleanup_targets_until_startup_retry() {
    let (server, root, manager, context) = fixture();
    let central = server.host_paths.app_storage_root(
        context.project_scope.as_str(),
        context.app_scope_id.as_str(),
    );
    current_state::save(
        &central.join(".agent-collab/server/state.json"),
        &State::default(),
    )
    .unwrap();
    for path in [
        central.join(".agent-collab/mailbox/reset-owned-mailbox"),
        central.join(".agent-collab/assets/reset-owned-asset"),
    ] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "reset-owned").unwrap();
    }
    let unrelated_root = root.join("unrelated-project");
    std::fs::create_dir_all(unrelated_root.join(".agent-collab")).unwrap();
    let unrelated_root = std::fs::canonicalize(unrelated_root).unwrap();
    let unrelated_scope = unrelated_root.display().to_string();
    let unrelated_central = server
        .host_paths
        .app_storage_root(&unrelated_scope, "unrelated-app");
    assert!(current_state::save(
        &unrelated_central.join(".agent-collab/server/state.json"),
        &State::default()
    )
    .unwrap());
    let unrelated_asset = unrelated_central.join(".agent-collab/assets/retained");
    std::fs::create_dir_all(unrelated_asset.parent().unwrap()).unwrap();
    std::fs::write(&unrelated_asset, "retained").unwrap();
    for (app_scope, project_scope, canonical_root, storage_root) in [
        (
            context.app_scope_id.as_str(),
            context.project_scope.as_str(),
            context.canonical_root.as_str(),
            central.as_path(),
        ),
        (
            "unrelated-app",
            unrelated_scope.as_str(),
            unrelated_scope.as_str(),
            unrelated_central.as_path(),
        ),
    ] {
        append_host_route_record(
            &manager.route_file,
            &HostRouteRecord {
                app_scope_id: app_scope.into(),
                project_scope: project_scope.into(),
                canonical_root: canonical_root.into(),
                storage_root: std::fs::canonicalize(storage_root)
                    .unwrap()
                    .display()
                    .to_string(),
                registered_ms: now_ms(),
            },
        )
        .unwrap();
    }
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
    let replayed = current_state::load(&server.state_path, false).unwrap();
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
    assert!(
        !central.exists(),
        "startup retry completed with residual central app data"
    );
    assert_eq!(
        std::fs::read_to_string(&unrelated_asset).unwrap(),
        "retained",
        "reset must preserve another project's central app namespace"
    );
    let records = load_host_route_records(&manager.route_file).unwrap();
    assert!(!records
        .iter()
        .any(|record| record.project_scope == context.project_scope.as_str()));
    assert!(records
        .iter()
        .any(|record| record.project_scope == unrelated_scope));
    assert!(server.state.lock().unwrap().workers.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_route_cleanup_is_durable_idempotent_and_retains_unknown_live() {
    let (server, root, manager, _) = fixture();
    let missing = root.join("missing-project");
    let missing_record = HostRouteRecord {
        app_scope_id: "stale-app".into(),
        project_scope: missing.display().to_string(),
        canonical_root: missing.display().to_string(),
        storage_root: missing.display().to_string(),
        registered_ms: now_ms(),
    };
    append_host_route_record(&manager.route_file, &missing_record).unwrap();
    let live = HostRouteRecord {
        app_scope_id: crate::identity::CLI_APP_SERVER_ID.into(),
        project_scope: root.display().to_string(),
        canonical_root: root.display().to_string(),
        storage_root: root.display().to_string(),
        registered_ms: now_ms(),
    };
    append_host_route_record(&manager.route_file, &live).unwrap();
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
    let records = load_host_route_records(&manager.route_file).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].project_scope, root.display().to_string());
    let bytes = std::fs::read(&manager.route_file).unwrap();
    manager.prune_stale_routes().unwrap();
    assert_eq!(bytes, std::fs::read(&manager.route_file).unwrap());
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
fn project_reset_clears_only_owned_channel_defaults_and_message_control() {
    use crate::server::channel_delivery::{ChannelDeliveryResult, ChannelDeliveryStatus};
    use crate::server::channel_envelope::{ChannelEnvelope, ChannelReplyRoute};
    use crate::server::conversation_default::ConversationDefault;

    let mut state = State::default();
    state.apply(&Event::ConversationDefaultSet {
        default: ConversationDefault {
            channel: "qqbot".into(),
            channel_instance: "qqbot".into(),
            conversation: "u1:session".into(),
            project_scope: "/project-a".into(),
            target_address: "project-a.Bob".into(),
            revision: 1,
            updated_ms: 1,
        },
    });
    state.apply(&Event::ConversationDefaultSet {
        default: ConversationDefault {
            channel: "qqbot".into(),
            channel_instance: "qqbot".into(),
            conversation: "u2:session".into(),
            project_scope: "/project-b".into(),
            target_address: "project-b.Bob".into(),
            revision: 1,
            updated_ms: 1,
        },
    });
    state.apply(&Event::Sent {
        msg: state::Message {
            id: "inbound-a".into(),
            from: "external:qqbot".into(),
            to: "project-a.Bob".into(),
            mtype: "notify".into(),
            subject: Some("QQBot inbound".into()),
            body: "hello".into(),
            in_reply_to: None,
            created_ms: 1,
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    });
    state.apply(&Event::ChannelEnvelopeRecorded {
        message_id: "inbound-a".into(),
        idempotency_key: "qqbot:/project-a:u1:session:platform-1".into(),
        envelope: ChannelEnvelope {
            source_channel: "qqbot".into(),
            channel_instance: "qqbot".into(),
            external_user: "u1".into(),
            external_conversation: "u1:session".into(),
            platform_message_id: "platform-1".into(),
            reply_route: "qqbot://u1/session".into(),
            recipient_address: "project-a.Bob".into(),
            attachments: Vec::new(),
        },
    });
    state.apply(&Event::ChannelReplyRouteBound {
        message_id: "outbound-a".into(),
        route: ChannelReplyRoute {
            source_channel: "qqbot".into(),
            channel_instance: "qqbot".into(),
            external_conversation: "u1:session".into(),
            reply_route: "qqbot://u1/session".into(),
            in_reply_to: "inbound-a".into(),
        },
    });
    state.apply(&Event::ChannelDeliveryResultRecorded {
        result: ChannelDeliveryResult {
            message_id: "outbound-a".into(),
            channel_instance: "qqbot".into(),
            status: ChannelDeliveryStatus::Uncertain,
            platform_message_id: None,
            error: Some("response lost".into()),
            updated_ms: 1,
        },
    });

    crate::server::project_reset::clear_project(&mut state, "/project-a", true, &[]);

    assert!(!state
        .conversation_defaults
        .contains_key("qqbot:qqbot:u1:session"));
    assert!(state
        .conversation_defaults
        .contains_key("qqbot:qqbot:u2:session"));
    assert!(state.channel_envelopes.is_empty());
    assert!(state.platform_event_index.is_empty());
    assert!(state.channel_reply_routes.is_empty());
    assert!(state.channel_delivery_results.is_empty());
}

#[test]
fn online_reset_durability_error_does_not_delete_payload() {
    let (server, root, manager, context) = fixture();
    let payload = root.join(".agent-collab/payload");
    std::fs::write(&payload, "must remain").unwrap();
    std::fs::remove_file(&server.state_path).unwrap();
    std::fs::create_dir(&server.state_path).unwrap();
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
fn online_reset_external_project_drops_runtime_and_its_current_state() {
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
        app_scope_id: external_context.app_scope_id.as_str().into(),
        project_scope: external_context.project_scope.as_str().into(),
        canonical_root: external_context.canonical_root.clone(),
        storage_root: external_context.canonical_root.clone(),
        registered_ms: now_ms(),
    };
    append_host_route_record(&manager.route_file, &external_record).unwrap();
    let result = manager.reset_project(&external_context, "reset-master", "reset-token");
    assert!(result.ok, "{result:?}");
    assert!(!external.state_path.exists());
    assert!(!external_root.join(".agent-collab").exists());
    assert!(external.state.lock().unwrap().workers.is_empty());
    assert!(external.state.lock().unwrap().admission_frozen());
    assert!(host.state_path.exists());
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
    assert!(load_host_route_records(&manager.route_file)
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
    let replayed = current_state::load(&host.state_path, false).unwrap();
    assert!(!replayed
        .global
        .current_thread_routes
        .values()
        .any(|b| b.project_scope == other_context.project_scope));
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(other_root).unwrap();
}

fn dsh_fixture() -> (
    Arc<Server>,
    PathBuf,
    Arc<ProjectRuntimeManager>,
    ProjectContext,
    RuntimeBinding,
) {
    let (server, root) = peer_tests::test_server();
    let server = Arc::new(server);
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
    let transport = crate::proto::SelectedTransport {
        kind: crate::proto::TransportKind::Dsh,
        endpoint: Some("unix:///tmp/dsh-gateway-reset-test.sock".into()),
        namespace: Some("gateway-runtime".into()),
        session_id: Some("dsh-session-A".into()),
        thread_id: Some("agentA".into()),
        tmux_endpoint: None,
        capabilities: Vec::new(),
        self_check: String::new(),
    };
    let binding = RuntimeBinding::new_with_session(
        context.project_scope.clone(),
        context.app_scope_id.clone(),
        AgentId::new("dsh-master").unwrap(),
        RuntimeId::new("dsh-runtime").unwrap(),
        BindingId::new("dsh-binding").unwrap(),
        Some(crate::identity::SessionId::new("dsh-session-A").unwrap()),
        Some(NativeThreadId::new("agentA").unwrap()),
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
        now_ms(),
    )
    .unwrap();
    server
        .commit_checked(&[
            Event::Registered {
                worker: state::WorkerRec {
                    id: "dsh-master".into(),
                    token: "dsh-token".into(),
                    cwd: root.display().to_string(),
                    registered_ms: now_ms(),
                    transport: Some(transport.clone()),
                },
            },
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
            Event::GlobalCurrentThreadRouteSet {
                binding: binding.clone(),
            },
            Event::GlobalMasterGranted { grant },
        ])
        .unwrap();
    binding_store::BindingControlStore::new(server.host_paths.state_root())
        .commit(binding_store::ControlChange::Commit {
            key: binding_slot::IdentityBindingKey::new(
                context.project_scope.clone(),
                context.app_scope_id.clone(),
                "Jenny",
            ),
            value: binding_slot::BindingValue::new(binding.clone(), transport),
            expected_generation: None,
        })
        .unwrap();
    let identity_dir = server
        .host_paths
        .state_root()
        .join("identities")
        .join("dsh-master");
    std::fs::create_dir_all(&identity_dir).unwrap();
    std::fs::write(
        identity_dir.join("identity.json"),
        serde_json::to_vec(&crate::identity::Identity {
            worker_id: "dsh-master".into(),
            token: "dsh-token".into(),
            project_scope: Some(context.project_scope.clone()),
            runtime: Some(crate::identity::RuntimeIdentity {
                agent_id: binding.agent_id.clone(),
                runtime_id: binding.runtime_id.clone(),
                appserver_id: binding.app_scope_id.clone(),
                binding_id: binding.binding_id.clone(),
                session_id: binding.session_id.clone(),
                native_thread_id: binding.native_thread_id.clone(),
            }),
            transport: Some(crate::proto::SelectedTransport {
                kind: crate::proto::TransportKind::Dsh,
                endpoint: Some("unix:///tmp/dsh-gateway-reset-test.sock".into()),
                namespace: Some("gateway-runtime".into()),
                session_id: Some("dsh-session-A".into()),
                thread_id: Some("agentA".into()),
                tmux_endpoint: None,
                capabilities: Vec::new(),
                self_check: String::new(),
            }),
        })
        .unwrap(),
    )
    .unwrap();
    (server, root, manager, context, binding)
}

#[test]
fn online_reset_preserves_live_dsh_binding_and_identity() {
    let (server, root, manager, context, binding) = dsh_fixture();
    let scope = context.project_scope.as_str().to_owned();
    append_host_route_record(
        &manager.route_file,
        &HostRouteRecord {
            app_scope_id: context.app_scope_id.as_str().into(),
            project_scope: scope.clone(),
            canonical_root: root.display().to_string(),
            storage_root: root.display().to_string(),
            registered_ms: now_ms(),
        },
    )
    .unwrap();
    populate(&server, "dsh-master", "dsh");

    // A same-scope non-DSH worker is disposable project state.
    server
        .commit_checked(&[Event::Registered {
            worker: state::WorkerRec {
                id: "stale-worker".into(),
                token: "stale-token".into(),
                cwd: scope.clone(),
                registered_ms: now_ms(),
                transport: None,
            },
        }])
        .unwrap();

    // An unrelated project keeps its worker, registration and current route.
    let other_root = root.join("other-project");
    std::fs::create_dir_all(&other_root).unwrap();
    let other_scope = ProjectScopeId::new(other_root.display().to_string()).unwrap();
    let other_app = AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
    let other_binding = RuntimeBinding::new_with_session(
        other_scope.clone(),
        other_app.clone(),
        AgentId::new("other-worker").unwrap(),
        RuntimeId::new("other-runtime").unwrap(),
        BindingId::new("other-binding").unwrap(),
        Some(crate::identity::SessionId::new("other-session").unwrap()),
        Some(NativeThreadId::new("other-thread").unwrap()),
    )
    .unwrap();
    server
        .commit_checked(&[
            Event::Registered {
                worker: state::WorkerRec {
                    id: "other-worker".into(),
                    token: "other-token".into(),
                    cwd: other_scope.as_str().to_owned(),
                    registered_ms: now_ms(),
                    transport: None,
                },
            },
            Event::GlobalProjectRegistered {
                registration: ProjectRegistration::new(other_scope.clone(), other_app).unwrap(),
            },
            Event::GlobalRuntimeBound {
                binding: other_binding.clone(),
            },
            Event::GlobalCurrentThreadRouteSet {
                binding: other_binding.clone(),
            },
        ])
        .unwrap();

    let result = manager.reset_project(&context, "dsh-master", "dsh-token");
    assert!(result.ok, "DSH project reset must succeed: {result:?}");

    {
        let state = server.state.lock().unwrap();
        let worker = state
            .workers
            .get("dsh-master")
            .expect("live DSH gateway worker must survive reset");
        assert_eq!(worker.token, "dsh-token");
        assert_eq!(
            worker
                .transport
                .as_ref()
                .map(|transport| transport.kind.clone()),
            Some(crate::proto::TransportKind::Dsh)
        );
        let project = state
            .global
            .projects
            .get(&scope)
            .expect("DSH project registration must survive reset");
        assert!(project.runtime_bindings.contains_key("dsh-binding"));
        assert!(
            project.master_grants.is_empty(),
            "reset must not revive the old master grant"
        );
        assert!(
            state
                .global
                .current_thread_routes
                .values()
                .any(|current| current == &binding),
            "the DSH current thread route must survive reset"
        );
        assert!(state.tasks.is_empty(), "project tasks must be cleared");
        assert!(state.msgs.is_empty(), "project messages must be cleared");
        assert!(
            state.notification_subscriptions.is_empty(),
            "project subscriptions must be cleared"
        );
        assert!(
            !state.workers.contains_key("stale-worker"),
            "a non-DSH same-scope worker is disposable project state"
        );
        assert!(state.workers.contains_key("other-worker"));
        assert!(state
            .global
            .projects
            .get(other_scope.as_str())
            .is_some_and(|project| project.runtime_bindings.contains_key("other-binding")));
        assert!(state
            .global
            .current_thread_routes
            .values()
            .any(|current| current == &other_binding));
    }

    assert!(
        server
            .host_paths
            .state_root()
            .join("identities")
            .join("dsh-master")
            .join("identity.json")
            .is_file(),
        "retained DSH identity must survive reset for restart recovery"
    );
    assert!(
        root.join(".agent-collab").is_dir(),
        "retained DSH route must stay available after restart"
    );
    let records = load_host_route_records(&manager.route_file).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].app_scope_id, context.app_scope_id.as_str());

    let replayed = current_state::load(&server.state_path, false).unwrap();
    assert!(
        replayed.workers.contains_key("dsh-master"),
        "current state must preserve the live DSH worker"
    );
    assert!(replayed
        .global
        .projects
        .get(&scope)
        .is_some_and(|project| project.runtime_bindings.contains_key("dsh-binding")));
    assert!(replayed.tasks.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}
