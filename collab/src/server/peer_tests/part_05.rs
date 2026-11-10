#[test]
fn role_contract_is_identical_across_context_workers_and_status() {
    use crate::subagent::Record;

    let (server, root) = test_server();
    let server = Arc::new(server);
    register(&server, "master", "%master");
    register_synthetic_peer(&server, "worker", "%worker");
    register_synthetic_peer(&server, "child", "%child");
    let context_for = |id: &str| {
        let binding = registered_binding(&server, id);
        let identity = crate::identity::RuntimeIdentity {
            agent_id: binding.agent_id, runtime_id: binding.runtime_id, appserver_id: binding.app_scope_id,
            binding_id: binding.binding_id, session_id: binding.session_id, native_thread_id: binding.native_thread_id,
        };
        ProjectContext::for_registered_route(&root, &identity).unwrap()
    };
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let context = context_for("master");
    manager.install_runtime(&(context.app_scope_id.as_str().into(), context.project_scope.as_str().into()), server.clone(), None);
    let map = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let before_map = map.load().unwrap();
    let before_state = std::fs::read(&server.state_path).unwrap();
    let (_, rejected) = manager.dispatch_sync(Some(context), Req::MasterPromote {
        worker_id: "master".into(), token: "token-master".into(), approval: "  ".into(),
    });
    assert!(!rejected.ok, "{rejected:?}");
    assert!(rejected.error.as_deref().unwrap().contains("explicit user approval"), "{rejected:?}");
    assert_eq!(map.load().unwrap(), before_map);
    assert_eq!(std::fs::read(&server.state_path).unwrap(), before_state);
    promote_master(&server, "master", "user approved master");
    server.commit(&[Event::SubagentUpdated {
        subagent: Record {
            id: "managed-role".into(),
            parent: "master".into(),
            peer: "child".into(),
            status: "idle".into(),
            thread_id: Some("thread-child".into()),
            profile: None,
            created_ms: now_ms(),
            ready_deadline_ms: 0,
            last_message: None,
            error: None,
            probe_failures: Vec::new(),
            runtime: None,
            create_operation_id: None,
            binding_id: None,
        },
    }]);
    let cases = [
        ("master", "master"),
        ("worker", "worker"),
        ("child", "managed-subagent"),
    ];
    for (worker_id, expected_role) in cases {
        let context = handle_context(&server, worker_id.into(), format!("token-{worker_id}"));
        assert!(context.ok, "{context:?}");
        assert_eq!(context.data["identity"]["role"], expected_role);
        assert_eq!(context.data["role_brief"]["role"], expected_role);
        assert_eq!(
            context.data["authority"],
            context.data["role_brief"]["authority"]
        );
        assert_eq!(
            context.data["role_brief"]["derivation"]["parent"].as_str(),
            (expected_role == "managed-subagent").then_some("master")
        );
        let role_task = context.data["role_brief"]["role_task"]
            .as_str()
            .expect("role task is explicit")
            .to_lowercase();
        let responsibilities = context.data["role_brief"]["responsibilities"]
            .as_array()
            .expect("role responsibilities are explicit")
            .iter()
            .filter_map(|line| line.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if expected_role != "managed-subagent" {
            assert!(!responsibilities.is_empty());
            let workflow = &context.data["role_brief"]["workflow_template"];
            assert_eq!(workflow["base_version"], serde_json::json!(1), "{workflow}");
            assert_eq!(
                workflow["identity"]["target"],
                if expected_role == "master" {
                    serde_json::json!("master")
                } else {
                    serde_json::json!("peer")
                },
                "{workflow}"
            );
            assert_eq!(workflow["identity"]["source"], "base", "{workflow}");
            assert_eq!(workflow["identity"]["base_version"], 1, "{workflow}");
            assert_eq!(workflow["identity"]["revision"], 0, "{workflow}");
            assert_eq!(workflow["specialties"], serde_json::json!([]), "{workflow}");
            assert!(
                workflow["identity"].get("template").is_none(),
                "workflow_template references its source instead of repeating its text: {workflow}"
            );
            for field in [
                "role_task",
                "responsibilities",
                "next_action",
                "completion_action",
            ] {
                assert!(
                    !context.data["role_brief"][field].is_null(),
                    "{field} must be projected once through role_brief: {workflow}"
                );
            }
        } else {
            assert!(
                context.data["role_brief"].get("workflow_template").is_none(),
                "the legacy managed-subagent branch keeps its existing brief"
            );
        }
        match expected_role {
            "master" => {
                assert!(role_task.contains("long-horizon"));
                assert!(role_task.contains("bounded"));
                assert!(role_task.contains("orchestrator"));
                let next_action = context.data["role_brief"]["next_action"]
                    .as_str()
                    .expect("role next action is explicit")
                    .to_lowercase();
                assert!(next_action.contains(
                    "invite suitable, available and authorized peers first"
                ));
                assert!(next_action.contains("no suitable authorized peer can take the work"));
                assert!(responsibilities.contains("codex exec --profile gcm"));
            }
            "worker" => {
                assert!(role_task.contains("long-horizon"));
                assert!(role_task.contains("orchestrator"));
                assert!(responsibilities.contains("codex exec --profile gcm"));
                assert_eq!(
                    context.data["role_brief"]["authority"],
                    serde_json::json!({
                        "managed_subagent": false,
                        "must_obey_master": false,
                        "may_decline_master_invite": true
                    })
                );
                assert_eq!(context.data["role_brief"]["derivation"]["kind"], "peer");
            }
            "managed-subagent" => {
                assert!(role_task.contains("legacy"));
                assert!(role_task.contains("no production execution authority"));
                assert!(responsibilities.contains("compatibility"));
                assert!(responsibilities.contains("do not accept new work"));
                assert_eq!(
                    context.data["role_brief"]["authority"],
                    serde_json::json!({
                        "managed_subagent": false,
                        "must_obey_master": false,
                        "may_decline_master_invite": false
                    })
                );
            }
            other => panic!("unexpected role {other}"),
        }
    }

    let workers = dispatch_with_route_context(&server, Req::Workers, None);
    assert!(workers.ok, "{workers:?}");
    let status = dispatch_with_route_context(&server, Req::StatusAll, None);
    assert!(status.ok, "{status:?}");
    for worker in workers.data["workers"].as_array().unwrap() {
        let id = worker["id"].as_str().unwrap();
        let status_worker = status.data["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|candidate| candidate["id"] == id)
            .unwrap();
        assert_eq!(worker["role"], worker["role_brief"]["role"]);
        assert_eq!(status_worker["role_brief"], worker["role_brief"]);
    }
    let before_map = map.load().unwrap();
    let before_state = std::fs::read(&server.state_path).unwrap();
    let (_, rejected) = manager.dispatch_sync(Some(context_for("worker")), Req::MasterDelegate {
        worker_id: "worker".into(), token: "token-worker".into(), target_id: "child".into(),
    });
    assert!(!rejected.ok, "{rejected:?}");
    assert!(rejected.error.as_deref().unwrap().starts_with("MASTER_DELEGATE_DENIED:"), "{rejected:?}");
    assert_eq!(map.load().unwrap(), before_map);
    assert_eq!(std::fs::read(&server.state_path).unwrap(), before_state);
    std::fs::remove_dir_all(root).unwrap();
}

/// A typed user-confirmed restore replaces an online current Jenny carrier.
#[test]
fn explicit_typed_restore_master_replaces_an_online_incumbent() {
    use super::name_state::{AgentPresence, AgentPresenceRecord};
    let (server, root) = test_server();
    for id in ["peer-a", "peer-b", "unrelated"] {
        let transport = test_appserver_transport(&format!("thread-{id}"));
        let registered = seed_committed_owner(&server, id, &format!("token-{id}"),
            &transport, &root.display().to_string(), Some(AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap()));
        assert!(registered.ok, "{registered:?}");
    }
    promote_master(&server, "peer-a", "user approved peer-a as collab master");
    let binding = registered_binding(&server, "peer-a");
    server
        .commit_checked(&[Event::AgentPresenceObserved {
            presence: AgentPresenceRecord {
                project_scope: binding.project_scope.as_str().into(),
                worker_id: "peer-a".into(),
                session_id: binding.session_id.unwrap().to_string(),
                presence: AgentPresence::Online,
                reason: "controlled current-agent online observation".into(),
                observed_ms: now_ms(),
            },
        }])
        .unwrap();
    let app = binding.app_scope_id.clone();
    let scope = binding.project_scope.clone();
    let store = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let control = store.load().unwrap();
    let unrelated = control.current_for_worker(&scope, &app, "unrelated").unwrap().unwrap().clone();
    let transport = server.state.lock().unwrap().workers["peer-b"].transport.clone().unwrap();
    let facts = crate::proto::IdentityFacts {
        session_id: transport.session_id.clone(), thread_id: transport.thread_id.clone(),
        endpoint: transport.endpoint.clone(), namespace: transport.namespace.clone(), ..Default::default()
    };
    let mut request = crate::proto::IdentityContextRequest::legacy(facts.clone());
    request.operation_id = "restore-online-jenny".into();
    request.grant_approval = Some(json!({"action":"restore_master", "decision":"approved",
        "authorization":"user explicitly restores peer-b as Jenny", "replace_confirmed":true}));
    let server = Arc::new(server);
    let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
    let context = ProjectContext::for_registered_root_with_app(&root, app.clone()).unwrap();
    manager.install_runtime(&(app.as_str().into(), scope.as_str().into()), server.clone(), None);
    let (_, restored) = manager.dispatch_sync(Some(context), Req::IdentityContext { facts, identity_context: Some(request) });
    assert!(restored.ok, "{restored:?}");
    let control = store.load().unwrap();
    assert_eq!(control.current_for_name(&scope, &app, "Jenny").unwrap().unwrap()
        .active_binding().unwrap().binding.agent_id.as_str(), "peer-b");
    assert_eq!(control.current_for_worker(&scope, &app, "unrelated").unwrap().unwrap(), &unrelated);
    assert_eq!(
        super::current_master_holder(&server, &server.state.lock().unwrap()).unwrap(),
        Some("peer-b".into())
    );
    let state = server.state.lock().unwrap();
    assert!(!state.global.lookup_project(&scope).unwrap().master_grants.values().any(|grant| grant.agent_id.as_str() == "peer-a"));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cross_project_send_queues_to_a_live_tmux_master_without_route_projection() {
    let (server, root) = test_server();
    register(&server, "target-master", "%target-master");
    promote_master(&server, "target-master", "user approved target-master");
    let binding = server
        .state
        .lock()
        .unwrap()
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| binding.agent_id.as_str() == "target-master")
        .cloned()
        .expect("target master binding");
    let worker = server.state.lock().unwrap().workers["target-master"].clone();
    assert_eq!(
        worker_presence(&server, &worker),
        IdentityPresence::Present,
        "a fresh registration has a verified identity and route"
    );
    // Model the observed production drift: the pane address stays live and the
    // typed master grant stays valid, but the runtime route projection no
    // longer claims the pane. The project binding and durable mailbox address
    // remain valid independently of that secondary route projection.
    {
        let mut st = server.state.lock().unwrap();
        st.global
            .current_thread_routes
            .retain(|_, route| route.binding_id != binding.binding_id);
        assert_eq!(
            current_master_holder(&server, &st).unwrap().as_deref(),
            Some("target-master"),
            "the typed grant survives the route projection loss"
        );
    }
    assert_eq!(
        worker_presence(&server, &worker),
        IdentityPresence::Present,
        "the project binding remains valid without the secondary route projection"
    );
    assert_eq!(
        worker_identity_and_endpoint(&server, &worker).1,
        Some(true),
        "the exact target pane must still be reachable"
    );

    let delivered = super::handle_cross_project_send(
        &server,
        "appsdk-master",
        "/tmp/appsdk".into(),
        Some("thread-appsdk-master".into()),
        "target-master",
        "feature",
        "durable tmux master message",
        None,
    );
    assert!(delivered.ok, "{}", delivered.error.unwrap_or_default());
    let msg_id = delivered.data["msg_id"].as_str().unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[msg_id].to, "target-master");
    assert_eq!(state.msgs[msg_id].from, "appsdk-master@/tmp/appsdk");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cross_project_send_rejects_a_definitively_missing_tmux_endpoint() {
    let (server, root) = test_server();
    register(&server, "target-master", "%target-master");
    promote_master(&server, "target-master", "user approved target-master");
    let binding = registered_binding(&server, "target-master");
    retire_registered_test_pane(&binding);
    let worker = server.state.lock().unwrap().workers["target-master"].clone();
    assert_eq!(
        worker_identity_and_endpoint(&server, &worker).1,
        Some(false),
        "the pane probe must prove the address is gone"
    );
    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            current_master_holder(&server, &state).unwrap().as_deref(),
            Some("target-master"),
            "the typed grant is independent of the dead address"
        );
    }

    let denied = super::handle_cross_project_send(
        &server,
        "appsdk-master",
        "/tmp/appsdk".into(),
        Some("thread-appsdk-master".into()),
        "target-master",
        "feature",
        "must reject a dead address",
        None,
    );
    assert!(!denied.ok);
    assert!(
        denied
            .error
            .unwrap_or_default()
            .contains("reachable target binding"),
        "the reject must name the dead address, not master authority"
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn register_appserver(server: &Arc<Server>, id: &str, thread_id: &str) -> Resp {
    register_public(server, id, thread_id)
}

#[test]
fn init_registration_result_exposes_persisted_runtime_identity() {
    let (mut server, root) = test_server();
    let mut server = Arc::new(server);
    let registration = register_appserver(&mut server, "peer-init", "thread-init");
    assert!(registration.ok, "{registration:?}");
    let runtime =
        runtime_from_registration_receipt(&registration.data, "peer-init", &root).unwrap();
    assert_eq!(registration.data["transport_selected"]["kind"], "tmux");
    assert_eq!(
        runtime.session_id.as_ref().unwrap().as_str(),
        registration.data["transport_selected"]["session_id"]
            .as_str()
            .unwrap()
    );
    assert_eq!(
        runtime.native_thread_id.as_ref().unwrap().as_str(),
        registration.data["transport_selected"]["thread_id"]
            .as_str()
            .unwrap()
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// Register the same peer's next agent session/thread in its existing pane.
fn recover_worker_on_new_thread(server: &Arc<Server>, id: &str, thread_id: &str) -> Resp {
    register_public(server, id, thread_id)
}

fn registered_binding(server: &Server, id: &str) -> crate::server::global_state::RuntimeBinding {
    server
        .state
        .lock()
        .unwrap()
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| binding.agent_id.as_str() == id)
        .cloned()
        .unwrap_or_else(|| panic!("{id} has no runtime binding"))
}

#[test]
fn wire_master_recovery_preserves_current_grant_and_approval() {
    let (mut server, root) = test_server();
    let mut server = Arc::new(server);
    let registered = register_appserver(&mut server, "recover-master", "thread-recover-master-old");
    assert!(registered.ok, "{registered:?}");
    promote_master(&server, "recover-master", "user approved recover-master");

    let previous = registered_binding(&server, "recover-master");
    let original_grant = server
        .state
        .lock()
        .unwrap()
        .global
        .lookup_master_grant_for(&previous.route_scope(), &previous.binding_id)
        .unwrap()
        .clone();
    assert!(matches!(crate::client::adapters::tmux::probe(previous.tmux_endpoint.as_ref().unwrap()),
        Ok(crate::client::adapters::tmux::PanePresence::Present)));
    let recovered =
        recover_worker_on_new_thread(&mut server, "recover-master", "thread-recover-master-new");
    assert!(recovered.ok, "{recovered:?}");

    let state = server.state.lock().unwrap();
    let route_scope = previous.route_scope();
    let current = state
        .global
        .lookup_binding_for(&route_scope, &previous.binding_id)
        .cloned()
        .expect("recovery keeps the binding id");
    let grant = state
        .global
        .lookup_master_grant_for(&route_scope, &previous.binding_id)
        .expect("same-principal recovery must reissue the master grant");
    assert_eq!(grant.binding_id, current.binding_id);
    assert_eq!(current.agent_id, previous.agent_id);
    assert_eq!(grant.agent_id, original_grant.agent_id);
    assert_eq!(grant.granted_by, original_grant.granted_by);
    assert_eq!(grant.approval, original_grant.approval);
    assert_eq!(grant.granted_at_ms, original_grant.granted_at_ms);
    assert_eq!(state.global.lookup_project_for_route(&route_scope).unwrap().master_grants.len(), 1);
    assert_eq!(
        state
            .global
            .role_for_route(&route_scope, &current.binding_id),
        crate::server::global_state::PeerRole::Master
    );
    drop(state);

    let status = super::handle_master_status(&server);
    assert!(status.ok, "{status:?}");
    assert_eq!(status.data["master"]["worker_id"], "recover-master");
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap();
    let mapped = &control.current_for_name(&current.project_scope, &current.app_scope_id, "Jenny").unwrap().unwrap()
        .active_binding().unwrap().binding;
    assert_eq!(mapped, &current);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn wire_peer_recover_does_not_revoke_unrelated_master() {
    let (mut server, root) = test_server();
    let mut server = Arc::new(server);
    let master = register_appserver(&mut server, "recover-keep-master", "thread-keep-master");
    assert!(master.ok, "{master:?}");
    let peer = recover_worker_on_new_thread(&mut server, "recover-plain-peer", "thread-plain-peer");
    assert!(peer.ok, "{peer:?}");
    promote_master(
        &server,
        "recover-keep-master",
        "user approved recover-keep-master",
    );
    let master_binding = registered_binding(&server, "recover-keep-master");
    let map = binding_store::BindingControlStore::new(server.host_paths.state_root());
    let before_map = map.load().unwrap();
    let before_master = before_map.current_for_name(&master_binding.project_scope, &master_binding.app_scope_id, "Jenny").unwrap().unwrap().clone();
    let original_grant = server.state.lock().unwrap().global.lookup_master_grant_for(&master_binding.route_scope(), &master_binding.binding_id).unwrap().clone();

    let recovered =
        recover_worker_on_new_thread(&mut server, "recover-plain-peer", "thread-plain-peer-new");
    assert!(recovered.ok, "{recovered:?}");

    let state = server.state.lock().unwrap();
    let route_scope = master_binding.route_scope();
    let grant = state
        .global
        .lookup_master_grant_for(&route_scope, &master_binding.binding_id)
        .expect("an unrelated peer recovery must not remove the master grant");
    assert_eq!(grant, &original_grant);
    assert_eq!(
        state
            .global
            .role_for_route(&route_scope, &master_binding.binding_id),
        crate::server::global_state::PeerRole::Master
    );
    let grants: Vec<_> = state
        .global
        .projects
        .values()
        .flat_map(|project| project.master_grants.values())
        .collect();
    assert_eq!(
        grants.len(),
        1,
        "recovery must not create a duplicate grant"
    );
    assert_eq!(grants[0].binding_id, master_binding.binding_id);
    drop(state);
    assert_eq!(map.load().unwrap().current_for_name(&master_binding.project_scope, &master_binding.app_scope_id, "Jenny").unwrap().unwrap(), &before_master);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovered_master_keeps_goal_deadline_scheduling() {
    let (mut server, root) = test_server();
    let mut server = Arc::new(server);
    let registered = register_appserver(&mut server, "recover-deadline", "thread-deadline-old");
    assert!(registered.ok, "{registered:?}");
    promote_master(
        &server,
        "recover-deadline",
        "user approved recover-deadline",
    );
    let previous = registered_binding(&server, "recover-deadline");
    let recovered =
        recover_worker_on_new_thread(&mut server, "recover-deadline", "thread-deadline-new");
    assert!(recovered.ok, "{recovered:?}");

    let current = registered_binding(&server, "recover-deadline");
    assert_eq!(current.agent_id, previous.agent_id);
    assert_eq!(current.binding_id, previous.binding_id);
    assert_eq!(current.session_id.as_ref().unwrap().as_str(), "session-thread-deadline-new");
    assert_eq!(current.native_thread_id.as_ref().unwrap().as_str(), "thread-deadline-new");
    assert_eq!(current.tmux_endpoint.as_ref().unwrap().pane_id, previous.tmux_endpoint.as_ref().unwrap().pane_id);
    let trigger = now_ms() + 60_000;
    let scheduled = handle_notification_subscribe(
        &server,
        "recover-deadline".into(),
        "token-recover-deadline".into(),
        "deadline".into(),
        Some("goal:recover-deadline".into()),
        Some(trigger),
        Vec::new(),
        None,
        1,
        86_400,
    );
    assert!(
        scheduled.ok,
        "recovered master must still schedule goal deadlines: {scheduled:?}"
    );
    let state = server.state.lock().unwrap();
    let deadlines: Vec<_> = state.notification_subscriptions.values()
        .filter(|lease| lease.worker_id == "recover-deadline" && lease.subject.as_deref() == Some("goal:recover-deadline"))
        .collect();
    assert_eq!(deadlines.len(), 1);
    assert_eq!(deadlines[0].trigger_ms, Some(trigger));
    assert_eq!(deadlines[0].status, "armed");
    assert!(state.global.lookup_master_grant_for(&current.route_scope(), &current.binding_id).is_some());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn first_and_later_registration_are_equal_peers() {
    let (server, root) = test_server();
    let server = Arc::new(server);
    assert_eq!(
        register_public(&server, "peer-a", "%peer-a").data["identity_kind"],
        "peer"
    );
    assert_eq!(
        register_public(&server, "peer-b", "%peer-b").data["identity_kind"],
        "peer"
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.workers.len(), 2);
    assert!(state.master_worker_id.is_none());
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn registration_creates_one_finite_default_direct_message_subscription() {
    let (server, root) = test_server();
    let server = Arc::new(server);
    assert!(register_public(&server, "peer", "%peer").ok);
    assert!(register_public(&server, "peer", "%peer").ok);
    let state = server.state.lock().unwrap();
    let subscriptions: Vec<_> = state
        .notification_subscriptions
        .values()
        .filter(|subscription| subscription.worker_id == "peer")
        .collect();
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0].event, "direct-message");
    let transport = selected_transport_for_worker(&state.workers["peer"]).unwrap();
    assert_eq!(subscriptions[0].method, transport.kind.as_str());
    assert_eq!(
        subscriptions[0].target,
        notification_target(&transport).unwrap().as_ref()
    );
    assert_eq!(subscriptions[0].status, "armed");
    assert!(subscriptions[0].expires_ms > now_ms());
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn daemon_maintenance_replaces_a_persisted_appserver_default_wake_lease() {
    let (mut server, root) = test_server();
    let mut server = Arc::new(server);
    assert!(register_public(&server, "sender", "thread-sender").ok);
    assert!(register_public(&server, "recipient", "thread-recipient").ok);
    {
        let mut state = server.state.lock().unwrap();
        server
            .commit_locked(
                &mut state,
                &[Event::NotificationSubscribed {
                    subscription: NotificationSubscription {
                        id: "sub-default-direct-message-recipient".into(),
                        worker_id: "recipient".into(),
                        event: "direct-message".into(),
                        subject: None,
                        target: "legacy-thread-recipient".into(),
                        method: "appserver".into(),
                        trigger_ms: None,
                        trigger_times_ms: Vec::new(),
                        interval_ms: None,
                        repeat_count: 1,
                        fired_count: 0,
                        expires_ms: now_ms() + 60_000,
                        status: "armed".into(),
                        created_ms: now_ms(),
                        updated_ms: now_ms(),
                        status_reason: None,
                        patrol_idle_since_ms: None,
                        patrol_last_push_ms: 0,
                        periodic_declared: false,
                        goal_content: None,
                    },
                }],
            )
            .expect("commit legacy appserver lease");
    }

    restore_registered_peer_default_leases(&server);
    let lease = server.state.lock().unwrap().notification_subscriptions
        ["sub-default-direct-message-recipient"]
        .clone();
    assert_eq!(lease.method, "tmux");
    assert_eq!(lease.target, "thread-recipient");
    assert_eq!(lease.status, "armed");
    assert_eq!(serde_json::to_value(&current_state::load(&server.state_path, false).unwrap()
        .notification_subscriptions[&lease.id]).unwrap(), serde_json::to_value(&lease).unwrap());

    Arc::get_mut(&mut server).unwrap().appserver_notification_sink = Arc::new(|transport, _, _, _, _, _mode| {
        if transport.kind != TransportKind::Tmux {
            return Err("expected tmux wake transport".into());
        }
        Ok(serde_json::json!({"text_submitted": true, "enter_submitted": true, "consumed": false}))
    });
    let sent = handle_send_with_task(
        &server,
        "sender".into(),
        "recipient".into(),
        "notify".into(),
        Some("upgrade wake".into()),
        "legacy lease must not suppress tmux wake".into(),
        None,
        "immediate".into(),
        false,
        None,
    );
    assert!(sent.ok, "{sent:?}");
    assert_eq!(sent.data["notification"], "tmux-input-submitted");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_cancelled_default_lease_is_repaired_by_registration() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);

    server.commit(&[Event::NotificationSuppressed {
        subscription_id: "sub-default-direct-message-peer".into(),
        status: "cancelled".into(),
        reason: "explicit-unsubscribe".into(),
        updated_ms: now_ms(),
    }]);
    let events = default_direct_message_events(
        &server.state.lock().unwrap(),
        "peer",
        &test_tmux_transport("thread-peer"),
        now_ms(),
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::NotificationSubscribed { subscription }
                if subscription.id == "sub-default-direct-message-peer"
                    && subscription.status == "armed"
        )),
        "historical explicit-unsubscribe must not disable base reception: {events:?}"
    );
    server.commit(&events);
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["sub-default-direct-message-peer"].status,
        "armed"
    );
    assert_eq!(
        state.notification_subscriptions["sub-default-direct-message-peer"]
            .status_reason
            .as_deref(),
        None
    );

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn historical_cancelled_default_lease_is_rearmed_without_reviving_goal_schedules() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);

    server.commit(&[Event::NotificationSuppressed {
        subscription_id: "sub-default-direct-message-peer".into(),
        status: "cancelled".into(),
        reason: "explicit-unsubscribe".into(),
        updated_ms: now_ms(),
    }]);
    {
        let state = server.state.lock().unwrap();
        let events = default_direct_message_events(
            &state,
            "peer",
            &test_tmux_transport("thread-peer"),
            now_ms(),
        );
        assert!(
            events.iter().any(|event| matches!(
                event,
                Event::NotificationSubscribed { subscription }
                    if subscription.id == "sub-default-direct-message-peer"
                        && subscription.status == "armed"
            )),
            "a reason-less legacy cancellation must be recoverable: {events:?}"
        );
    }
    assert!(register(&server, "peer", "%peer").ok);
    let project_scope = GlobalState::canonical_project_scope(&server.root).unwrap();
    server.commit(&[Event::NameResolutionCommitted {
        project_scope: project_scope.as_str().to_owned(),
        occupancy: crate::server::name_state::NameOccupancy {
            name: "peer".into(),
            worker_id: "peer".into(),
            session_id: "session-peer".into(),
            occupancy_revision: 1,
            updated_ms: now_ms(),
            pane_anchor: None,
        },
        assignments: Vec::new(),
    }]);

    let ended = handle_goal_schedule_reconcile(
        &server,
        "peer".into(),
        "token-peer".into(),
        "finished-goal".into(),
        "Alice".into(),
        "ended".into(),
        None,
        None,
        None,
        Some("goal-finished".into()),
    );
    assert!(ended.ok, "{}", ended.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["sub-default-direct-message-peer"].status,
        "armed"
    );
    assert!(
        state
            .notification_subscriptions
            .values()
            .all(|subscription| subscription.subject.as_deref() != Some("goal:finished-goal")),
        "ending an absent goal must not resurrect or create a goal occurrence"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn pending_unattempted_direct_message_is_recovered_once_and_not_replayed() {
    let (mut server, root) = test_server();
    server.config.notifications.mode = "immediate".into();
    assert!(register_synthetic_peer(&server, "peer", "thread-peer").ok);
    let default_id = "sub-default-direct-message-peer";
    let now = now_ms();
    server.commit(&[Event::NotificationSuppressed {
        subscription_id: default_id.into(),
        status: "cancelled".into(),
        reason: "explicit-unsubscribe".into(),
        updated_ms: now,
    }]);
    let message = |id: &str, mtype: &str, subject: Option<&str>, state: &str, attempts: u32| {
        Message {
            id: id.into(),
            from: "sender".into(),
            to: "peer".into(),
            mtype: mtype.into(),
            subject: subject.map(str::to_owned),
            body: "report".into(),
            in_reply_to: None,
            created_ms: now,
            state: state.into(),
            wake_attempt_count: attempts,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        }
    };
    // A real ended scheduler occurrence is a typed notification bound to a
    // cancelled deadline subscription, not a user-controlled subject prefix.
    server.commit(&[Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-goal-ended".into(),
            worker_id: "peer".into(),
            event: "deadline".into(),
            subject: Some("goal:ended".into()),
            target: "thread-peer".into(),
            method: "tmux".into(),
            trigger_ms: Some(now - 1),
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: now - 1,
            status: "cancelled".into(),
            created_ms: now - 1000,
            updated_ms: now - 1000,
            status_reason: Some("goal-schedule-ended".into()),
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    }]);
    server.commit(&[
        Event::Sent {
            msg: message("m-recover", "notify", Some("report"), "pending", 0),
        },
        Event::Sent {
            msg: message(
                "m-deadline-report",
                "notify",
                Some("deadline:report"),
                "pending",
                0,
            ),
        },
        Event::Sent {
            msg: message(
                "m-idle-report",
                "notify",
                Some("master-idle:report"),
                "pending",
                0,
            ),
        },
        Event::Sent {
            msg: message(
                "m-ended",
                "notification",
                Some("deadline:goal:ended"),
                "pending",
                0,
            ),
        },
        Event::Sent {
            msg: message("m-scheduler-bound", "notify", Some("report"), "pending", 0),
        },
        Event::WakeBound {
            message_id: "m-ended".into(),
            subscription_id: "sub-goal-ended".into(),
        },
        Event::WakeBound {
            message_id: "m-scheduler-bound".into(),
            subscription_id: "sub-goal-ended".into(),
        },
        Event::Sent {
            msg: message("m-attempted", "notify", Some("report"), "pending", 1),
        },
        Event::Sent {
            msg: message("m-accepted", "notify", Some("report"), "pending", 0),
        },
        Event::Sent {
            msg: message("m-consumed", "notify", Some("report"), "read", 1),
        },
        Event::NotificationDeliveryAccepted {
            message_id: "m-accepted".into(),
            accepted_ms: now,
            evidence: None,
        },
    ]);

    let events = default_direct_message_events(
        &server.state.lock().unwrap(),
        "peer",
        &test_tmux_transport("thread-peer"),
        now,
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::NotificationSubscribed { subscription }
                if subscription.id == default_id && subscription.status == "armed"
        )),
        "historical cancellation must be repaired: {events:?}"
    );
    let mut bound = events
        .iter()
        .filter_map(|event| match event {
            Event::WakeBound {
                message_id,
                subscription_id,
            } if subscription_id == default_id => Some(message_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    bound.sort();
    assert_eq!(
        bound,
        vec!["m-deadline-report", "m-idle-report", "m-recover"],
        "ordinary direct messages must recover regardless of subject prefix: {events:?}"
    );
    server.commit(&events);

    let attempt_results: Vec<_> = ["m-recover", "m-deadline-report", "m-idle-report"]
        .iter()
        .map(|message_id| {
            crate::server::attempt_notification_with_default(
                &server,
                message_id,
                default_id,
                &|_| true,
                &|_, _| true,
            )
        })
        .collect();
    // One shared batch owns the three pending ordinary records. Each record
    // receives exactly one accepted attempt; only the first seed call may
    // report a newly issued attempt, and repeated seeds must not submit again.
    assert_eq!(
        attempt_results,
        [true, false, false],
        "ordinary direct messages must recover regardless of subject prefix"
    );

    {
        let state = server.state.lock().unwrap();
        for message_id in ["m-recover", "m-deadline-report", "m-idle-report"] {
            assert_eq!(state.msgs[message_id].wake_attempt_count, 1);
            assert!(state.notification_delivery_accepted.contains_key(message_id));
        }
        // A typed ended occurrence keeps its scheduler ownership. A notify
        // record bound to a scheduler subscription is not ordinary payload
        // merely because its subject has no reserved prefix.
        for message_id in ["m-ended", "m-scheduler-bound"] {
            assert_eq!(
                state.wake_bindings.get(message_id).map(String::as_str),
                Some("sub-goal-ended"),
                "{message_id} must not be rebound to the default direct lease"
            );
            assert_eq!(state.msgs[message_id].wake_attempt_count, 0);
            assert_eq!(state.msgs[message_id].state, "pending");
        }
        assert!(state.wake_bindings.get("m-attempted").is_none());
        assert_eq!(state.msgs["m-attempted"].wake_attempt_count, 1);
        assert!(state.wake_bindings.get("m-accepted").is_none());
        assert_eq!(state.msgs["m-accepted"].wake_attempt_count, 0);
        assert!(state.wake_bindings.get("m-consumed").is_none());
    }

    let repeated = default_direct_message_events(
        &server.state.lock().unwrap(),
        "peer",
        &test_tmux_transport("thread-peer"),
        now + 1,
    );
    assert!(
        !repeated
            .iter()
            .any(|event| matches!(event, Event::WakeBound { .. })),
        "recovery must not bind again after acceptance: {repeated:?}"
    );
    server.commit(&repeated);
    assert!(!crate::server::attempt_notification_with_default(
        &server,
        "m-recover",
        default_id,
        &|_| true,
        &|_, _| panic!("accepted recovery must not submit a second attempt"),
    ));
    assert_eq!(
        server.state.lock().unwrap().msgs["m-recover"].wake_attempt_count,
        1
    );

    let replayed = current_state::load(&server.state_path, false)
        .expect("current state keeps recovery facts");
    assert_eq!(replayed.msgs["m-recover"].wake_attempt_count, 1);
    assert!(replayed
        .notification_delivery_accepted
        .contains_key("m-recover"));
    assert!(
        !default_direct_message_events(
            &replayed,
            "peer",
            &test_tmux_transport("thread-peer"),
            now + 2,
        )
        .iter()
        .any(|event| matches!(event, Event::WakeBound { .. }))
    );

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn maintenance_tick_refreshes_idle_default_lease_without_recipient_context() {
    let (mut server, root) = test_server();
    server.config.notifications.mode = "immediate".into();
    let submitted = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    {
        let submitted = Arc::clone(&submitted);
        server.appserver_notification_sink =
            Arc::new(move |_transport, _, _, message_id, _, _mode| {
                submitted.lock().unwrap().push(message_id.to_string());
                Ok(serde_json::json!({"accepted": true}))
            });
    }
    let server = Arc::new(server);

    assert!(register_synthetic_peer(&server, "peer", "thread-peer").ok);
    let now = now_ms();
    // Seed the daemon-owned default lease as armed but already past expiry.
    // The recipient has no later context, registration, or goal timer.
    server.commit(&[Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-default-direct-message-peer".into(),
            worker_id: "peer".into(),
            event: "direct-message".into(),
            subject: None,
            target: "thread-peer".into(),
            method: "tmux".into(),
            trigger_ms: None,
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: now - 1000,
            status: "armed".into(),
            created_ms: now - DEFAULT_DIRECT_MESSAGE_TTL_SECONDS as i64 * 1000,
            updated_ms: now - DEFAULT_DIRECT_MESSAGE_TTL_SECONDS as i64 * 1000,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    }]);

    // One ordinary maintenance tick must refresh the lease before the timer
    // pass can mark it expired.
    crate::server::timers::tick_with_idle_at(&server, now, &|_| true);
    {
        let state = server.state.lock().unwrap();
        let lease = &state.notification_subscriptions["sub-default-direct-message-peer"];
        assert_eq!(lease.status, "armed", "{lease:?}");
        assert!(lease.expires_ms > now, "idle lease must be refreshed: {lease:?}");
    }

    // The recipient stays idle. Public sendmessage must reach the default
    // lease with exactly one automatic attempt and no recipient context.
    let sent = handle_send_with_task(
        &server,
        "sender".into(),
        "peer".into(),
        "notify".into(),
        Some("deadline:report".into()),
        "ordinary direct report".into(),
        None,
        "immediate".into(),
        false,
        None,
    );
    assert!(sent.ok, "{sent:?}");
    let message_id = sent.data["msg_id"].as_str().unwrap().to_owned();
    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.msgs[&message_id].wake_attempt_count,
            1,
            "the refreshed default lease must deliver exactly once"
        );
        assert!(state
            .notification_delivery_accepted
            .contains_key(&message_id));
        assert_eq!(
            state.wake_bindings.get(&message_id).map(String::as_str),
            Some("sub-default-direct-message-peer")
        );
    }
    assert_eq!(
        submitted.lock().unwrap().as_slice(),
        [message_id.as_str()],
        "the idle recipient must be reached without context or re-registration"
    );

    // Later maintenance must not resubmit the accepted attempt.
    crate::server::timers::tick_with_idle_at(&server, now + 1, &|_| true);
    assert_eq!(submitted.lock().unwrap().len(), 1);
    {
        let state = server.state.lock().unwrap();
        assert_eq!(state.msgs[&message_id].wake_attempt_count, 1);
        assert!(state.notification_subscriptions["sub-default-direct-message-peer"]
            .expires_ms
            > now + 1);
    }

    drop(server);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn maintenance_tick_refreshes_default_lease_with_timers_disabled() {
    let (mut server, root) = test_server();
    server.config.notifications.mode = "immediate".into();
    server.config.timers.enabled = false;
    let submitted = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    {
        let submitted = Arc::clone(&submitted);
        server.appserver_notification_sink =
            Arc::new(move |_transport, _, _, message_id, _, _mode| {
                submitted.lock().unwrap().push(message_id.to_string());
                Ok(serde_json::json!({"accepted": true}))
            });
    }
    let server = Arc::new(server);

    assert!(register_synthetic_peer(&server, "peer", "thread-peer").ok);
    let now = now_ms();
    server.commit(&[Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-default-direct-message-peer".into(),
            worker_id: "peer".into(),
            event: "direct-message".into(),
            subject: None,
            target: "thread-peer".into(),
            method: "tmux".into(),
            trigger_ms: None,
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: now - 1000,
            status: "armed".into(),
            created_ms: now - DEFAULT_DIRECT_MESSAGE_TTL_SECONDS as i64 * 1000,
            updated_ms: now - DEFAULT_DIRECT_MESSAGE_TTL_SECONDS as i64 * 1000,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    }]);

    // Disabling finite patrol timers must not close daemon-owned base
    // reception. Maintenance still reconciles the default lease before the
    // lifecycle pass sees it as expired.
    crate::server::timers::tick_with_idle_at(&server, now, &|_| true);
    {
        let state = server.state.lock().unwrap();
        let lease = &state.notification_subscriptions["sub-default-direct-message-peer"];
        assert_eq!(
            lease.status, "armed",
            "disabling finite timers must not close base reception: {lease:?}"
        );
        assert!(
            lease.expires_ms > now,
            "an idle default lease must refresh even when timers are disabled: {lease:?}"
        );
    }

    let sent = handle_send_with_task(
        &server,
        "sender".into(),
        "peer".into(),
        "notify".into(),
        Some("deadline:report".into()),
        "ordinary direct report".into(),
        None,
        "immediate".into(),
        false,
        None,
    );
    assert!(sent.ok, "{sent:?}");
    let message_id = sent.data["msg_id"].as_str().unwrap().to_owned();
    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.msgs[&message_id].wake_attempt_count, 1,
            "a timer-disabled recipient must still receive one automatic attempt"
        );
        assert!(state
            .notification_delivery_accepted
            .contains_key(&message_id));
        assert_eq!(
            state.wake_bindings.get(&message_id).map(String::as_str),
            Some("sub-default-direct-message-peer")
        );
    }
    assert_eq!(
        submitted.lock().unwrap().as_slice(),
        [message_id.as_str()],
        "timer-disabled maintenance must reach the idle recipient exactly once"
    );

    crate::server::timers::tick_with_idle_at(&server, now + 1, &|_| true);
    assert_eq!(submitted.lock().unwrap().len(), 1);

    drop(server);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn recovered_immediate_direct_message_reaches_automatic_attempt_once() {
    let (mut server, root) = test_server();
    let submitted = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    {
        let submitted = Arc::clone(&submitted);
        server.appserver_notification_sink =
            Arc::new(move |_transport, _, _, message_id, _, _mode| {
                submitted.lock().unwrap().push(message_id.to_string());
                Ok(serde_json::json!({"accepted": true}))
            });
    }
    let server = Arc::new(server);

    assert!(register_synthetic_peer(&server, "peer", "thread-peer").ok);
    server.commit(&[Event::NotificationSuppressed {
        subscription_id: "sub-default-direct-message-peer".into(),
        status: "cancelled".into(),
        reason: "explicit-unsubscribe".into(),
        updated_ms: now_ms(),
    }]);

    let sent = handle_send_with_task(
        &server,
        "sender".into(),
        "peer".into(),
        "notify".into(),
        Some("report".into()),
        "recovered report".into(),
        None,
        "immediate".into(),
        false,
        None,
    );
    assert!(sent.ok, "{sent:?}");
    let message_id = sent.data["msg_id"].as_str().unwrap().to_owned();
    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.delivery_modes.get(&message_id).map(String::as_str),
            Some("immediate"),
            "public sendmessage must persist delivery=immediate"
        );
        assert_eq!(state.msgs[&message_id].wake_attempt_count, 0);
        assert!(state.wake_bindings.get(&message_id).is_none());
    }

    // Restore the daemon-owned default lease through the real reconciliation.
    restore_registered_peer_default_leases(&server);
    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.notification_subscriptions["sub-default-direct-message-peer"].status,
            "armed"
        );
        assert_eq!(
            state.wake_bindings.get(&message_id).map(String::as_str),
            Some("sub-default-direct-message-peer")
        );
    }

    let now = now_ms();
    crate::server::timers::tick_with_idle_at(&server, now, &|_| true);
    assert_eq!(
        submitted.lock().unwrap().as_slice(),
        [message_id.as_str()],
        "the recovered immediate report must reach the adaptor exactly once"
    );
    assert_eq!(
        server.state.lock().unwrap().msgs[&message_id].wake_attempt_count,
        1
    );
    assert!(server
        .state
        .lock()
        .unwrap()
        .notification_delivery_accepted
        .contains_key(&message_id));

    // Repeated reconciliation and tick must not resubmit the accepted attempt.
    restore_registered_peer_default_leases(&server);
    crate::server::timers::tick_with_idle_at(&server, now + 1, &|_| true);
    assert_eq!(submitted.lock().unwrap().len(), 1);
    assert_eq!(
        server.state.lock().unwrap().msgs[&message_id].wake_attempt_count,
        1
    );

    drop(server);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn goal_reconcile_retires_retained_basename_subject_without_arbitrary_ids() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);
    let project_scope = GlobalState::canonical_project_scope(&server.root).unwrap();
    server.commit(&[Event::NameResolutionCommitted {
        project_scope: project_scope.as_str().to_owned(),
        occupancy: crate::server::name_state::NameOccupancy {
            name: "peer".into(),
            worker_id: "peer".into(),
            session_id: "session-peer".into(),
            occupancy_revision: 1,
            updated_ms: now_ms(),
            pane_anchor: None,
        },
        assignments: Vec::new(),
    }]);
    let now = now_ms();
    server.commit(&[Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-legacy-goal".into(),
            worker_id: "peer".into(),
            event: "deadline".into(),
            subject: Some("goal:long-task.md".into()),
            target: "thread-peer".into(),
            method: "tmux".into(),
            trigger_ms: Some(now + 60_000),
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: now + 86_400_000,
            status: "armed".into(),
            created_ms: now,
            updated_ms: now,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    }]);

    let arbitrary = handle_goal_schedule_reconcile(
        &server,
        "peer".into(),
        "token-peer".into(),
        "different-id".into(),
        "Alice".into(),
        "ended".into(),
        None,
        None,
        Some("sub-legacy-goal".into()),
        Some("fixture".into()),
    );
    assert!(
        !arbitrary.ok,
        "a subscription ID must never be accepted as a goal subject"
    );

    let ended = handle_goal_schedule_reconcile(
        &server,
        "peer".into(),
        "token-peer".into(),
        "different-id".into(),
        "Alice".into(),
        "ended".into(),
        None,
        None,
        Some("goal:long-task.md".into()),
        Some("goal-finished".into()),
    );
    assert!(ended.ok, "{}", ended.error.unwrap_or_default());
    assert_eq!(
        ended.data["retired_subscription_ids"],
        serde_json::json!(["sub-legacy-goal"])
    );
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["sub-legacy-goal"].status,
        "cancelled"
    );
    assert!(
        state
            .notification_subscriptions
            .values()
            .all(|subscription| subscription.subject.as_deref() != Some("goal:different-id")),
        "retained subject retirement must not create a canonical duplicate"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn registration_adds_default_lease_when_only_short_direct_message_lease_exists() {
    let mut state = State::default();
    let now = 10_000;
    state.apply(&Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: "sub-short".into(),
            worker_id: "peer".into(),
            event: "direct-message".into(),
            subject: None,
            target: "thread-peer".into(),
            method: "appserver".into(),
            trigger_ms: None,
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: now + 600_000,
            status: "armed".into(),
            created_ms: now,
            updated_ms: now,
            status_reason: None,
            patrol_idle_since_ms: None,
            patrol_last_push_ms: 0,
            periodic_declared: false,
            goal_content: None,
        },
    });

    let events =
        default_direct_message_events(&state, "peer", &test_tmux_transport("thread-peer"), now);
    let default = events
        .iter()
        .find_map(|event| match event {
            Event::NotificationSubscribed { subscription } => Some(subscription),
            _ => None,
        })
        .expect("a short explicit lease must not suppress the default peer lease");
    assert_eq!(default.id, "sub-default-direct-message-peer");
    assert_eq!(
        default.expires_ms,
        now + DEFAULT_DIRECT_MESSAGE_TTL_SECONDS as i64 * 1000
    );

    for event in &events {
        state.apply(event);
    }
    assert_eq!(
        state.notification_subscriptions["sub-short"].status,
        "rebound"
    );
    assert!(default_direct_message_events(
        &state,
        "peer",
        &test_tmux_transport("thread-peer"),
        now + 1
    )
    .is_empty());
}

#[test]
fn role_brief_projects_identity_and_sorted_specialty_workflow_templates() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    register_synthetic_peer(&server, "worker", "%worker");
    promote_master(&server, "master", "user approved master");

    let project_scope =
        crate::server::global_state::GlobalState::canonical_project_scope(&server.root)
            .unwrap()
            .as_str()
            .to_owned();
    let worker_name = {
        let state = server.state.lock().unwrap();
        state
            .name_occupancies
            .get(&project_scope)
            .and_then(|names| {
                names
                    .values()
                    .find(|entry| entry.worker_id == "worker")
                    .map(|entry| entry.name.clone())
            })
            .expect("worker stable name")
    };
    let spec = |role_task: &str,
                responsibility: &str,
                next_action: &str,
                completion_action: &str| crate::board::WorkflowTemplateSpec {
        role_task: role_task.into(),
        responsibilities: vec![responsibility.into()],
        next_action: next_action.into(),
        completion_action: completion_action.into(),
    };
    let override_event =
        |target: &str, spec: crate::board::WorkflowTemplateSpec| Event::WorkflowTemplateUpdated {
            project_scope: project_scope.clone(),
            target: target.into(),
            entry: Some(crate::board::WorkflowTemplateOverride {
                base_version: 1,
                revision: 1,
                spec,
                updated_by: "master".into(),
                updated_ms: now_ms(),
            }),
        };
    server.commit(&[
        Event::NameWorkRolesUpdated {
            project_scope: project_scope.clone(),
            record: crate::board::NameWorkRoles {
                name: worker_name,
                work_roles: vec!["reviewer".into(), "designer".into(), "master".into()],
            },
        },
        override_event(
            "peer",
            spec(
                "Peer identity override task",
                "Peer identity override responsibility",
                "Peer identity override next action",
                "Peer identity override completion",
            ),
        ),
        override_event(
            "role:designer",
            spec(
                "Designer specialty task",
                "Designer specialty responsibility",
                "Designer specialty next action",
                "Designer specialty completion",
            ),
        ),
        override_event(
            "role:reviewer",
            spec(
                "Reviewer specialty task",
                "Reviewer specialty responsibility",
                "Reviewer specialty next action",
                "Reviewer specialty completion",
            ),
        ),
        override_event(
            "role:master",
            spec(
                "Master-named specialty task",
                "Master-named specialty responsibility",
                "Master-named specialty next action",
                "Master-named specialty completion",
            ),
        ),
    ]);

    let peer = handle_context(&server, "worker".into(), "token-worker".into());
    assert!(peer.ok, "{peer:?}");
    assert_eq!(peer.data["identity"]["role"], "worker");
    assert_eq!(peer.data["role_brief"]["role"], "worker");
    let workflow = &peer.data["role_brief"]["workflow_template"];
    assert_eq!(workflow["identity"]["target"], "peer", "{workflow}");
    assert_eq!(
        workflow["identity"]["source"], "master_override",
        "{workflow}"
    );
    assert_eq!(workflow["identity"]["revision"], 1, "{workflow}");
    assert!(
        workflow.get("configured_templates").is_none(),
        "peers see only effective cards: {workflow}"
    );
    let targets = workflow["specialties"]
        .as_array()
        .expect("specialty cards")
        .iter()
        .filter_map(|card| card["target"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        vec!["role:designer", "role:master", "role:reviewer"],
        "specialties follow the sorted work-role order: {workflow}"
    );
    assert!(
        workflow["specialties"]
            .as_array()
            .unwrap()
            .iter()
            .all(|card| card.get("template").is_none()),
        "specialty cards reference their text source without embedding it: {workflow}"
    );

    let brief = &peer.data["role_brief"];
    let role_task = brief["role_task"].as_str().expect("role task");
    assert!(role_task.contains("Peer identity override task"), "{role_task}");
    assert!(role_task.contains("Designer specialty task"), "{role_task}");
    assert!(role_task.contains("Reviewer specialty task"), "{role_task}");
    assert!(
        role_task.contains("Master-named specialty task"),
        "a specialty named master cannot replace identity: {role_task}"
    );
    assert_eq!(brief["orchestration_function"], "goal_orchestrator");
    assert!(
        role_task.find("role:designer:").unwrap() < role_task.find("role:reviewer:").unwrap(),
        "specialty text follows the current role order: {role_task}"
    );
    let responsibilities = brief["responsibilities"].as_array().expect("responsibilities");
    for expected in [
        "Peer identity override responsibility",
        "Designer specialty responsibility",
        "Reviewer specialty responsibility",
        "Master-named specialty responsibility",
    ] {
        assert!(
            responsibilities
                .iter()
                .any(|entry| entry.as_str() == Some(expected)),
            "{expected} missing from {brief}"
        );
    }
    let next_action = brief["next_action"].as_str().expect("next action");
    assert!(
        next_action.contains("Peer identity override next action"),
        "{next_action}"
    );
    assert!(
        next_action.contains("Designer specialty next action"),
        "{next_action}"
    );
    assert!(
        next_action.contains("Reviewer specialty next action"),
        "{next_action}"
    );
    assert!(
        responsibilities.iter().any(|line| line.as_str().is_some_and(|text| text.contains("codex exec --profile gcm"))),
        "common execution constraints remain independent of editable next_action"
    );
    let completion = brief["completion_action"]
        .as_str()
        .expect("completion action");
    assert!(
        completion.contains("Peer identity override completion"),
        "{completion}"
    );
    assert!(completion.contains("Designer specialty completion"), "{completion}");
    assert!(
        !completion.contains("Idle handoff:"),
        "completion keeps the selected templates without a fixed suffix: {completion}"
    );
    assert!(
        responsibilities.iter().any(|line| line.as_str().is_some_and(|text|
            text.contains("must not stop to report to the master and wait for a dispatch"))),
        "the common handoff duty remains in responsibilities: {brief}"
    );
    let operations = peer.data["operations"].as_array().expect("operations");
    assert!(
        !operations.iter().any(|operation| {
            matches!(
                operation["kind"].as_str(),
                Some("responsibility" | "next_action" | "completion_action" | "notification_rule")
            )
        }),
        "operations must contain facts-triggered actions, not copied workflow text: {operations:?}"
    );

    let master = handle_context(&server, "master".into(), "token-master".into());
    assert!(master.ok, "{master:?}");
    let master_workflow = &master.data["role_brief"]["workflow_template"];
    assert_eq!(master_workflow["identity"]["target"], "master");
    let catalog = master_workflow["configured_templates"]
        .as_object()
        .expect("master catalog");
    for target in ["peer", "role:designer", "role:master", "role:reviewer"] {
        assert!(catalog.contains_key(target), "{target} missing from {catalog:?}");
        assert!(
            catalog[target].get("template").is_none(),
            "the master catalog must reference templates without embedding them: {catalog:?}"
        );
    }
    let master_completion = master.data["role_brief"]["completion_action"]
        .as_str()
        .expect("master completion");
    let master_completion_lower = master_completion.to_ascii_lowercase();
    assert!(
        master_completion_lower.contains("never to itself")
            || master_completion_lower.contains("idle request to yourself"),
        "master completion keeps the no-self-idle rule: {master_completion}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// A manager's single `collab context` read must answer, for every durable
/// subordinate manager, whether a patrol is armed, whether it is idle, and
/// whether idle is caused by a blocking wait. This is the scope digest a
/// manager needs to schedule its own scope without extra round trips.
#[test]
fn context_scope_digest_reports_subordinate_manager_patrol_idle_and_blockers() {
    use crate::server::name_state::{
        AgentActivity, AgentActivityRecord, AgentPresence, AgentPresenceRecord,
    };
    use crate::server::state::WaitSpec;

    let (server, root) = test_server();
    register(&server, "master", "%master");
    register_synthetic_peer(&server, "peer-a", "%peer-a");
    register_synthetic_peer(&server, "peer-b", "%peer-b");
    promote_master(&server, "master", "user approved master");

    let now = now_ms();
    let scope = GlobalState::canonical_project_scope(&server.root)
        .expect("canonical project scope")
        .as_str()
        .to_owned();
    let session_of = |id: &str| {
        server
            .state
            .lock()
            .unwrap()
            .workers
            .get(id)
            .and_then(|worker| worker.transport.as_ref())
            .and_then(|transport| transport.session_id.clone())
            .expect("registered transport session")
    };
    // peer-a is trusted idle; peer-b is busy. Both are online.
    server.commit(&[
        Event::AgentPresenceObserved {
            presence: AgentPresenceRecord {
                project_scope: scope.clone(),
                worker_id: "peer-a".into(),
                session_id: session_of("peer-a"),
                presence: AgentPresence::Online,
                reason: "test".into(),
                observed_ms: now,
            },
        },
        Event::AgentActivityObserved {
            activity: AgentActivityRecord {
                project_scope: scope.clone(),
                worker_id: "peer-a".into(),
                session_id: session_of("peer-a"),
                activity: AgentActivity::Idle,
                background: false,
                reason: "test".into(),
                observed_ms: now,
                idle_since_ms: Some(now - 60_000),
            },
        },
        Event::AgentPresenceObserved {
            presence: AgentPresenceRecord {
                project_scope: scope.clone(),
                worker_id: "peer-b".into(),
                session_id: session_of("peer-b"),
                presence: AgentPresence::Online,
                reason: "test".into(),
                observed_ms: now,
            },
        },
        Event::AgentActivityObserved {
            activity: AgentActivityRecord {
                project_scope: scope.clone(),
                worker_id: "peer-b".into(),
                session_id: session_of("peer-b"),
                activity: AgentActivity::Busy,
                background: false,
                reason: "test".into(),
                observed_ms: now,
                idle_since_ms: None,
            },
        },
    ]);
    // peer-a armed a board-idle patrol and an owned goal; peer-b armed none.
    server.commit(&[
        Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: "sub-patrol-a".into(),
                worker_id: "peer-a".into(),
                event: "board-idle".into(),
                subject: Some("board-idle-patrol".into()),
                target: "thread-peer-a".into(),
                method: "tmux".into(),
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: Some(900_000),
                repeat_count: 1,
                fired_count: 0,
                expires_ms: now + 3_600_000,
                status: "armed".into(),
                created_ms: now,
                updated_ms: now,
                status_reason: None,
                patrol_idle_since_ms: None,
                patrol_last_push_ms: 0,
                periodic_declared: false,
                goal_content: None,
            },
        },
        Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: "sub-goal-a".into(),
                worker_id: "peer-a".into(),
                event: "deadline".into(),
                subject: Some("goal:g-a".into()),
                target: "thread-peer-a".into(),
                method: "tmux".into(),
                trigger_ms: Some(now + 60_000),
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                fired_count: 0,
                expires_ms: now + 3_600_000,
                status: "armed".into(),
                created_ms: now,
                updated_ms: now,
                status_reason: None,
                patrol_idle_since_ms: None,
                patrol_last_push_ms: 0,
                periodic_declared: false,
                goal_content: None,
            },
        },
    ]);
    // peer-a owns an open task blocked on an external resource; peer-b owns a
    // plain open task.
    let blocked = TaskRec {
        id: "task-blocked".into(),
        owner: "peer-a".into(),
        created_by: "peer-a".into(),
        feature_id: None,
        worktree_path: None,
        branch: None,
        base_commit: None,
        priority: "p2".into(),
        status: "blocked".into(),
        next_step: None,
        wait: Some(WaitSpec {
            waiter: "peer-a".into(),
            waiting_for: "task-upstream".into(),
            responsible_actor: "peer-a".into(),
            reason: "waiting on external dependency".into(),
            deadline_ms: now + 900_000,
            resume_on: vec!["resource_released".into()],
            escalation: "resource_owner_and_waiter_recheck".into(),
        }),
        isolation_mode: None,
        created_ms: now,
        updated_ms: now,
    };
    let plain = TaskRec {
        id: "task-plain".into(),
        owner: "peer-b".into(),
        created_by: "peer-b".into(),
        feature_id: None,
        worktree_path: None,
        branch: None,
        base_commit: None,
        priority: "p2".into(),
        status: "working".into(),
        next_step: None,
        wait: None,
        isolation_mode: None,
        created_ms: now,
        updated_ms: now,
    };
    server.commit(&[
        Event::TaskCreated { task: blocked },
        Event::TaskCreated { task: plain },
    ]);

    let master = handle_context(&server, "master".into(), "token-master".into());
    assert!(master.ok, "{master:?}");
    let digest = master.data["scope_digest"]
        .as_array()
        .expect("scope digest array");
    assert_eq!(digest.len(), 2, "{digest:?}");
    let peer_a = digest
        .iter()
        .find(|row| row["worker_id"] == "peer-a")
        .expect("peer-a digest row");
    assert_eq!(peer_a["activity"], "idle", "{peer_a}");
    assert_eq!(peer_a["patrol"]["armed"], true, "{peer_a}");
    assert_eq!(peer_a["patrol"]["event"], "board-idle", "{peer_a}");
    assert_eq!(peer_a["goal"]["armed"], true, "{peer_a}");
    assert!(
        peer_a["open_tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|task| task == "task-blocked"),
        "{peer_a}"
    );
    assert!(
        !peer_a["blocked"].as_array().unwrap().is_empty(),
        "blocked wait must be visible: {peer_a}"
    );
    let peer_b = digest
        .iter()
        .find(|row| row["worker_id"] == "peer-b")
        .expect("peer-b digest row");
    assert_eq!(peer_b["activity"], "busy", "{peer_b}");
    assert_eq!(peer_b["patrol"]["armed"], false, "{peer_b}");
    assert_eq!(peer_b["goal"]["armed"], false, "{peer_b}");
    assert!(peer_b["blocked"].as_array().unwrap().is_empty(), "{peer_b}");

    // A peer has no durable manager subordinates, so its own digest is empty.
    let peer_view = handle_context(&server, "peer-a".into(), "token-peer-a".into());
    assert!(peer_view.ok, "{peer_view:?}");
    assert_eq!(
        peer_view.data["scope_digest"],
        serde_json::json!([]),
        "a peer digest must stay empty: {peer_view:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}
