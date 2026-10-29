#[test]
fn role_contract_is_identical_across_context_workers_and_status() {
    use crate::subagent::Record;

    let (server, root) = test_server();
    register_synthetic_peer(&server, "master", "%master");
    register_synthetic_peer(&server, "worker", "%worker");
    register_synthetic_peer(&server, "child", "%child");
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
            endpoint_generation: None,
        },
    }]);
    let server = Arc::new(server);

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
            assert!(responsibilities.contains("--in-reply-to <message-id>"));
            assert!(responsibilities.contains("resume your own unfinished work"));
            assert!(responsibilities.contains("user-facing summary is not that reply"));
            assert!(responsibilities.contains("register your own accepted goal"));
            assert!(responsibilities.contains("request next work or explicit idle approval"));
            assert!(responsibilities.contains("adjust only affected duties, dependencies or handoffs"));
            assert!(responsibilities.contains("specialty roles, policy and workflow overrides are optional"));
            assert!(!responsibilities.contains("automatically update affected role workflow templates"));
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
                assert!(responsibilities.contains("codex exec --profile gcm"));
                assert!(responsibilities.contains("authorized peers"));
                assert!(responsibilities.contains("regardless of whether other peers are busy"));
                assert!(responsibilities.contains("protected_hold"));
                assert!(responsibilities.contains("only its own project"));
                assert!(responsibilities.contains("non-overlapping"));
                assert!(responsibilities.contains("close-before-open/sese"));
                assert!(responsibilities.contains("canonical intake and deduplication"));
                assert!(responsibilities.contains("owning project contract"));
                assert!(responsibilities.contains("daemon or tmux lifecycle"));
                assert!(responsibilities.contains("re-arm"));
                assert!(responsibilities.contains("no inherited master authority"));
                assert!(!responsibilities.contains("saturate every live present peer first"));
            }
            "worker" => {
                assert!(responsibilities.contains("not routinely to the user"));
                assert!(role_task.contains("long-horizon"));
                assert!(role_task.contains("orchestrator"));
                assert!(responsibilities.contains("codex exec --profile gcm"));
                assert!(responsibilities.contains("non-overlapping bounded scopes"));
                assert!(responsibilities.contains("verify subworker evidence"));
                assert!(responsibilities.contains("resource closure"));
                assert!(responsibilities.contains("close-before-open/sese"));
                assert!(responsibilities.contains("canonical intake and deduplication"));
                assert!(responsibilities.contains("owning project contract"));
                assert!(responsibilities.contains("daemon or tmux lifecycle actions"));
                assert!(responsibilities.contains("do not claim master authority"));
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
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn absent_tmux_master_requires_name_recovery_before_peer_promotion() {
    let (server, root) = test_server();
    register(&server, "peer-a", "thread-a");
    register(&server, "peer-b", "thread-b");
    assert!(
        super::handle_master_promote(
            &server,
            "peer-a".into(),
            "token-peer-a".into(),
            "user approved peer-a as collab master".into(),
        )
        .ok
    );
    peer_tests::kill_registered_worker_pane(&server, "peer-a");
    let status = super::handle_master_status(&server);
    // The recorded grant is the authority. A vanished pane address is a
    // communication fact; it does not unseat the holder.
    assert_eq!(status.data["master"]["worker_id"], "peer-a", "{status:?}");
    assert!(status.data["recorded_unusable"].is_null(), "{status:?}");
    let promoted = super::handle_master_promote(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "user approved peer-b after the previous master runtime died".into(),
    );
    assert!(!promoted.ok, "{promoted:?}");
    assert!(promoted
        .error
        .as_deref()
        .unwrap()
        .starts_with("NAME_RECOVERY_REQUIRED"));
    assert_eq!(promoted.data["outcome"], "pending");
    assert_eq!(
        super::handle_master_status(&server).data["master"]["worker_id"],
        "peer-a"
    );
    confirm_test_jenny_takeover(&server, "peer-b");
    assert_eq!(
        super::current_master_holder(&server, &server.state.lock().unwrap()).unwrap(),
        Some("peer-b".into())
    );
    let live = super::handle_master_status(&server);
    assert_eq!(live.data["master"]["worker_id"], "peer-b");
    assert!(live.data["recorded_unusable"].is_null());
    std::fs::remove_dir_all(root).unwrap();
}

/// Legacy approval alone cannot replace an occupied Jenny. A confirmed name
/// recovery can replace it without requiring the previous executor to stop.
#[test]
fn approved_promotion_supersedes_a_live_tmux_master() {
    use super::name_state::{AgentPresence, AgentPresenceRecord};
    let (server, root) = test_server();
    register(&server, "peer-a", "thread-a");
    register(&server, "peer-b", "thread-b");
    assert!(
        super::handle_master_promote(
            &server,
            "peer-a".into(),
            "token-peer-a".into(),
            "user approved peer-a as collab master".into(),
        )
        .ok
    );
    let status = super::handle_master_status(&server);
    assert_eq!(status.data["master"]["worker_id"], "peer-a", "{status:?}");
    assert_eq!(
        super::current_master_holder(&server, &server.state.lock().unwrap()).unwrap(),
        Some("peer-a".into())
    );
    let context = handle_context(&server, "peer-b".into(), "token-peer-b".into());
    assert_eq!(context.data["master"]["worker_id"], "peer-a", "{context:?}");

    // The name owner consumes current typed observations, not pane existence.
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

    let promoted = super::handle_master_promote(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "user approved peer-b after the previous master thread became unusable".into(),
    );
    assert!(!promoted.ok, "{promoted:?}");
    assert!(promoted
        .error
        .as_deref()
        .unwrap()
        .starts_with("NAME_RECOVERY_REQUIRED"));
    assert_eq!(promoted.data["outcome"], "confirmation_required");
    assert_eq!(
        super::handle_master_status(&server).data["master"]["worker_id"],
        "peer-a"
    );
    confirm_test_jenny_takeover(&server, "peer-b");
    assert_eq!(
        super::current_master_holder(&server, &server.state.lock().unwrap()).unwrap(),
        Some("peer-b".into())
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn confirm_test_jenny_takeover(server: &Server, worker_id: &str) {
    let result = super::name_resolution::resolve_registered_name(
        server,
        worker_id,
        &format!("confirmed-jenny-{worker_id}"),
        "approved_recovery",
        Some(super::name_resolution::NameIntent {
            requested_name: "Jenny".into(),
            user_authorized: true,
            replace_confirmed: true,
        }),
    )
    .unwrap();
    assert_eq!(result["outcome"], "committed", "{result}");
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
        "appsdk-master".into(),
        "/tmp/appsdk".into(),
        "thread-appsdk-master".into(),
        "appsdk-master".into(),
        Some("user approved appsdk-master".into()),
        1,
        "target-master".into(),
        "feature".into(),
        "durable tmux master message".into(),
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
        "appsdk-master".into(),
        "/tmp/appsdk".into(),
        "thread-appsdk-master".into(),
        "appsdk-master".into(),
        Some("user approved appsdk-master".into()),
        1,
        "target-master".into(),
        "feature".into(),
        "must reject a dead address".into(),
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

#[test]
fn cross_project_send_requires_master_endpoints_on_both_sides() {
    let (server, root) = test_server();
    // The target master is a native App Server peer, so its presence is a real
    // runtime fact rather than a tmux address that can only report Unknown.
    register_appserver_worker(&server, &root, "target-master", "thread-target-master");
    register(&server, "target-peer", "%target-peer");
    let promoted = super::handle_master_promote(
        &server,
        "target-master".into(),
        "token-target-master".into(),
        "user approved target-master as collab master".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());

    let denied_peer = super::handle_cross_project_send(
        &server,
        "appsdk-master".into(),
        "/tmp/appsdk".into(),
        "thread-appsdk-master".into(),
        "appsdk-operator".into(),
        None,
        1,
        "target-peer".into(),
        "feature".into(),
        "must reject non-master target".into(),
        None,
    );
    assert!(!denied_peer.ok);
    assert!(denied_peer
        .error
        .unwrap()
        .contains("target to be the project master"));

    let delivered = super::handle_cross_project_send(
        &server,
        "appsdk-master".into(),
        "/tmp/appsdk".into(),
        "thread-appsdk-master".into(),
        "appsdk-operator".into(),
        None,
        1,
        "target-master".into(),
        "feature".into(),
        "master-to-master message".into(),
        None,
    );
    assert!(delivered.ok, "{}", delivered.error.unwrap_or_default());
    let msg_id = delivered.data["msg_id"].as_str().unwrap();
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[msg_id].to, "target-master");
    assert_eq!(state.msgs[msg_id].from, "appsdk-master@/tmp/appsdk");
    drop(state);

    let missing_approval = super::handle_cross_project_send(
        &server,
        "self-promoted".into(),
        "/tmp/appsdk".into(),
        "thread-self-promoted".into(),
        "self-promoted".into(),
        None,
        1,
        "target-master".into(),
        "feature".into(),
        "must reject missing approval".into(),
        None,
    );
    assert!(!missing_approval.ok);
    assert!(missing_approval.error.unwrap().contains("user approval"));
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

#[test]
fn master_promotion_is_not_blocked_by_an_unproven_candidate_transport() {
    let (mut server, root) = test_server();
    register(&server, "peer-a", "thread-a");
    retire_registered_test_pane(&registered_binding(&server, "peer-a"));
    let promoted = super::handle_master_promote(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "user approved peer-a as collab master".into(),
    );
    assert!(promoted.ok, "{promoted:?}");
    assert_eq!(promoted.data["master"], "peer-a");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_promotion_allows_live_tmux_peer() {
    let (mut server, root) = test_server();
    let mut server = Arc::new(server);
    let registered = register_appserver(&mut server, "peer-appserver", "thread-appserver");
    assert!(registered.ok, "{registered:?}");
    assert_eq!(registered.data["transport_selected"]["kind"], "tmux");
    let promoted = super::handle_master_promote(
        &server,
        "peer-appserver".into(),
        "token-peer-appserver".into(),
        "user approved peer-appserver as appserver master".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());
    assert_eq!(
        super::current_master_holder(&server, &server.state.lock().unwrap()).unwrap(),
        Some("peer-appserver".into())
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_assigned_replays_approval_and_live_identity() {
    let event = Event::MasterAssigned {
        worker_id: "peer-a".into(),
        assigned_by: "peer-a".into(),
        approval: Some("user approved peer-a as collab master".into()),
        assigned_ms: 1,
    };
    let encoded = serde_json::to_string(&event).unwrap();
    let mut replay = State::default();
    replay.apply(&serde_json::from_str(&encoded).unwrap());
    assert_eq!(replay.master_worker_id.as_deref(), Some("peer-a"));
    assert_eq!(replay.master_assigned_by.as_deref(), Some("peer-a"));
    assert_eq!(
        replay.master_approval.as_deref(),
        Some("user approved peer-a as collab master")
    );
    assert_eq!(replay.master_assigned_ms, Some(1));
    let legacy = r#"{"ev":"RootAssigned","worker_id":"peer-b","assigned_by":"peer-a","approval":null,"assigned_ms":2}"#;
    let mut legacy_replay = State::default();
    legacy_replay.apply(&serde_json::from_str(legacy).unwrap());
    assert_eq!(legacy_replay.master_worker_id.as_deref(), Some("peer-b"));
}

#[test]
fn master_authority_is_generation_bound_and_replays_from_typed_grant() {
    let (mut server, root) = test_server();
    let mut server = Arc::new(server);
    let registered = register_appserver(&mut server, "peer-appserver", "thread-appserver");
    assert!(registered.ok, "{registered:?}");

    let promoted = super::handle_master_promote(
        &server,
        "peer-appserver".into(),
        "token-peer-appserver".into(),
        "user approved peer-appserver as collab master".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());

    let (scope, generation) = {
        let state = server.state.lock().unwrap();
        let binding = state
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .find(|binding| binding.agent_id.as_str() == "peer-appserver")
            .unwrap()
            .clone();
        let grant = state
            .global
            .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
            .expect("promotion must commit a generation-bound typed master grant");
        assert_eq!(grant.endpoint_generation, binding.endpoint_generation);
        (binding.route_scope(), binding.endpoint_generation)
    };

    retire_registered_test_pane(&registered_binding(&server, "peer-appserver"));

    let reconnected = register_appserver(&mut server, "peer-appserver", "thread-appserver-next");
    assert!(reconnected.ok, "{reconnected:?}");
    {
        let state = server.state.lock().unwrap();
        let binding = state
            .global
            .lookup_binding_for(&scope, &BindingId::new("binding-peer-appserver").unwrap())
            .unwrap();
        assert_eq!(binding.endpoint_generation, generation + 1);
        // A same-principal generation replacement is the recovery path, so
        // the grant is reissued for the new generation in the same
        // transaction instead of being dropped.
        let reissued = state
            .global
            .lookup_master_grant_for(&scope, &binding.binding_id)
            .expect("same-principal reconnect must reissue the master grant");
        assert_eq!(reissued.endpoint_generation, binding.endpoint_generation);
        assert_eq!(
            state
                .global
                .role_for_binding(&scope.project_scope_id, &binding.binding_id),
            crate::server::global_state::PeerRole::Master
        );
    }
    let status = super::handle_master_status(&server);
    assert!(status.ok, "{status:?}");
    assert_eq!(status.data["master"]["worker_id"], "peer-appserver");
    assert!(status.data["recorded_unusable"].is_null(), "{status:?}");

    let replayed = super::replay(&root).unwrap();
    let binding = replayed
        .global
        .lookup_binding_for(&scope, &BindingId::new("binding-peer-appserver").unwrap())
        .unwrap();
    let replayed_grant = replayed
        .global
        .lookup_master_grant_for(&scope, &binding.binding_id)
        .expect("replay must keep the reissued grant");
    assert_eq!(
        replayed_grant.endpoint_generation,
        binding.endpoint_generation
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn same_runtime_registration_recovery_preserves_master_authority() {
    let (mut server, root) = test_server();
    let mut server = Arc::new(server);
    let registered = register_appserver(&mut server, "peer-appserver", "thread-appserver");
    assert!(registered.ok, "{registered:?}");
    let promoted = super::handle_master_promote(
        &server,
        "peer-appserver".into(),
        "token-peer-appserver".into(),
        "user approved peer-appserver as appserver master".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());

    let (scope, generation) = {
        let state = server.state.lock().unwrap();
        let binding = state
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .find(|binding| binding.agent_id.as_str() == "peer-appserver")
            .unwrap()
            .clone();
        (binding.route_scope(), binding.endpoint_generation)
    };

    retire_registered_test_pane(&registered_binding(&server, "peer-appserver"));

    let recovered = register_appserver(&mut server, "peer-appserver", "thread-appserver-recovered");
    assert!(recovered.ok, "{recovered:?}");
    assert_eq!(recovered.data["recovered"], true);
    assert_eq!(recovered.data["role_brief"]["role"], "master");

    {
        let state = server.state.lock().unwrap();
        let binding = state
            .global
            .lookup_binding_for(&scope, &BindingId::new("binding-peer-appserver").unwrap())
            .unwrap();
        assert_eq!(binding.endpoint_generation, generation + 1);
        let grant = state
            .global
            .lookup_master_grant_for(&scope, &binding.binding_id)
            .expect("same-principal recovery must reissue the master grant");
        assert_eq!(grant.endpoint_generation, binding.endpoint_generation);
    }
    let status = super::handle_master_status(&server);
    assert!(status.ok, "{status:?}");
    assert_eq!(status.data["master"]["worker_id"], "peer-appserver");

    let replayed = super::replay(&root).unwrap();
    let binding = replayed
        .global
        .lookup_binding_for(&scope, &BindingId::new("binding-peer-appserver").unwrap())
        .unwrap();
    assert_eq!(binding.endpoint_generation, generation + 1);
    let replayed_grant = replayed
        .global
        .lookup_master_grant_for(&scope, &binding.binding_id)
        .expect("replay must keep the reissued master grant");
    assert_eq!(
        replayed_grant.endpoint_generation,
        binding.endpoint_generation
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// Register the same peer against its next distinct pane/session fixture.
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
fn wire_master_recover_reissues_master_grant_for_new_generation() {
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
    let legacy_promotion_events_before = std::fs::read_to_string(&server.journal_path)
        .unwrap()
        .lines()
        .filter(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .is_ok_and(|event| event["ev"] == "MasterAssigned")
        })
        .count();
    retire_registered_test_pane(&previous);
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
    assert_eq!(
        current.endpoint_generation,
        previous.endpoint_generation + 1
    );
    let grant = state
        .global
        .lookup_master_grant_for(&route_scope, &previous.binding_id)
        .expect("same-principal recovery must reissue the master grant");
    assert_eq!(grant.endpoint_generation, current.endpoint_generation);
    assert_eq!(current.agent_id, previous.agent_id);
    assert_eq!(grant.agent_id, original_grant.agent_id);
    assert_eq!(grant.granted_by, original_grant.granted_by);
    assert_eq!(grant.approval, original_grant.approval);
    assert_eq!(grant.granted_at_ms, original_grant.granted_at_ms);
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
    let legacy_promotion_events_after = std::fs::read_to_string(&server.journal_path)
        .unwrap()
        .lines()
        .filter(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .is_ok_and(|event| event["ev"] == "MasterAssigned")
        })
        .count();
    assert_eq!(
        legacy_promotion_events_after, legacy_promotion_events_before,
        "master recovery must not emit a new promotion event"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn same_principal_master_recovery_keeps_grant_while_incumbent_pane_stays() {
    let (mut server, root) = test_server();
    let mut server = Arc::new(server);
    let registered = register_appserver(&mut server, "live-master", "thread-live-master-old");
    assert!(registered.ok, "{registered:?}");
    promote_master(&server, "live-master", "user approved live-master");
    let previous = registered_binding(&server, "live-master");
    let original_grant = server
        .state
        .lock()
        .unwrap()
        .global
        .lookup_master_grant_for(&previous.route_scope(), &previous.binding_id)
        .unwrap()
        .clone();

    let recovered =
        recover_worker_on_new_thread(&mut server, "live-master", "thread-live-master-new");
    assert!(
        recovered.ok,
        "same-principal recovery must not be fenced by the old pane: {recovered:?}"
    );
    let state = server.state.lock().unwrap();
    let route_scope = previous.route_scope();
    let current = state
        .global
        .lookup_binding_for(&route_scope, &previous.binding_id)
        .unwrap();
    assert_eq!(
        current.endpoint_generation,
        previous.endpoint_generation + 1
    );
    let grant = state
        .global
        .lookup_master_grant_for(&route_scope, &previous.binding_id)
        .expect("same-principal recovery must reissue the grant");
    assert_eq!(grant.endpoint_generation, current.endpoint_generation);
    assert_eq!(grant.approval, original_grant.approval);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn same_principal_master_recovery_is_allowed_when_probe_is_unknown() {
    let (mut server, root) = test_server();
    let mut server = Arc::new(server);
    let registered = register_appserver(&mut server, "unknown-master", "thread-unknown-master-old");
    assert!(registered.ok, "{registered:?}");
    promote_master(&server, "unknown-master", "user approved unknown-master");
    stop_registered_test_tmux_server();

    let recovered =
        recover_worker_on_new_thread(&mut server, "unknown-master", "thread-unknown-master-new");
    assert!(
        recovered.ok,
        "an unknown transport probe must not fence same-principal recovery: {recovered:?}"
    );
    let status = super::handle_master_status(&server);
    assert_eq!(status.data["master"]["worker_id"], "unknown-master", "{status:?}");
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

    let recovered =
        recover_worker_on_new_thread(&mut server, "recover-plain-peer", "thread-plain-peer-new");
    assert!(recovered.ok, "{recovered:?}");

    let state = server.state.lock().unwrap();
    let route_scope = master_binding.route_scope();
    let grant = state
        .global
        .lookup_master_grant_for(&route_scope, &master_binding.binding_id)
        .expect("an unrelated peer recovery must not remove the master grant");
    assert_eq!(
        grant.endpoint_generation,
        master_binding.endpoint_generation
    );
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
    retire_registered_test_pane(&registered_binding(&server, "recover-deadline"));
    let recovered =
        recover_worker_on_new_thread(&mut server, "recover-deadline", "thread-deadline-new");
    assert!(recovered.ok, "{recovered:?}");

    let scheduled = handle_notification_subscribe(
        &server,
        "recover-deadline".into(),
        "token-recover-deadline".into(),
        "deadline".into(),
        Some("goal:recover-deadline".into()),
        Some(now_ms() + 60_000),
        Vec::new(),
        None,
        1,
        86_400,
    );
    assert!(
        scheduled.ok,
        "recovered master must still schedule goal deadlines: {scheduled:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_master_assignment_cannot_override_typed_grant_on_replay() {
    use std::io::Write;

    let (mut server, root) = test_server();
    let mut server = Arc::new(server);
    let registered = register_appserver(&mut server, "peer-appserver", "thread-appserver");
    assert!(registered.ok, "{registered:?}");
    let promoted = super::handle_master_promote(
        &server,
        "peer-appserver".into(),
        "token-peer-appserver".into(),
        "user approved peer-appserver as collab master".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());

    let binding = registered_binding(&server, "peer-appserver");
    let typed_grant = server
        .state
        .lock()
        .unwrap()
        .global
        .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
        .unwrap()
        .clone();

    let legacy = Event::MasterAssigned {
        worker_id: "peer-appserver".into(),
        assigned_by: "legacy-operator".into(),
        approval: Some("legacy approval".into()),
        assigned_ms: 1,
    };
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap();
    writeln!(file, "{}", serde_json::to_string(&legacy).unwrap()).unwrap();
    drop(file);

    let replayed = super::replay(&root).expect("mixed legacy and typed authority must replay");
    assert!(replayed.master_worker_id.is_none());
    let binding = replayed
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| binding.agent_id.as_str() == "peer-appserver")
        .unwrap();
    let grant = replayed
        .global
        .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
        .expect("typed grant must remain authoritative");
    assert_eq!(grant, &typed_grant);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_master_assignment_before_runtime_rebind_does_not_regrant_authority() {
    use std::io::Write;

    let (mut server, root) = test_server();
    let mut server = Arc::new(server);
    let registered = register_appserver(&mut server, "peer-appserver", "thread-appserver");
    assert!(registered.ok, "{registered:?}");

    let legacy = Event::MasterAssigned {
        worker_id: "peer-appserver".into(),
        assigned_by: "legacy-operator".into(),
        approval: Some("legacy approval".into()),
        assigned_ms: 1,
    };
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap();
    writeln!(file, "{}", serde_json::to_string(&legacy).unwrap()).unwrap();
    drop(file);

    let reconnected = register_appserver(&mut server, "peer-appserver", "thread-appserver-next");
    assert!(reconnected.ok, "{reconnected:?}");

    let replayed = super::replay(&root).expect("legacy authority before reconnect must replay");
    assert!(replayed.master_worker_id.is_none());
    let binding = replayed
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| binding.agent_id.as_str() == "peer-appserver")
        .unwrap();
    assert_eq!(binding.endpoint_generation, 2);
    assert!(replayed
        .global
        .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
        .is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_master_assignment_after_runtime_binding_is_migrated() {
    use std::io::Write;

    let (server, root) = test_server();
    register(&server, "peer-a", "thread-peer-a");

    let legacy = Event::MasterAssigned {
        worker_id: "peer-a".into(),
        assigned_by: "peer-a".into(),
        approval: Some("legacy approval".into()),
        assigned_ms: 1,
    };
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap();
    writeln!(file, "{}", serde_json::to_string(&legacy).unwrap()).unwrap();
    drop(file);

    let replayed = super::replay(&root).expect("legacy authority after binding must replay");
    assert!(replayed.master_worker_id.is_none());
    let binding = replayed
        .global
        .projects
        .values()
        .flat_map(|project| project.runtime_bindings.values())
        .find(|binding| binding.agent_id.as_str() == "peer-a")
        .unwrap();
    let grant = replayed
        .global
        .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
        .expect("current legacy authority must migrate to a typed grant");
    assert_eq!(grant.granted_by, "peer-a");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_master_assignment_without_runtime_binding_fails_replay_closed() {
    use crate::identity::AppServerId;
    use crate::server::global_state::ProjectRegistration;
    use std::io::Write;

    let root = std::env::temp_dir().join(format!(
        "collab-legacy-master-missing-binding-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let project_scope =
        crate::server::global_state::GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = AppServerId::new("tui-default").unwrap();
    let registration = ProjectRegistration::new(project_scope, app_scope).unwrap();
    let legacy = Event::MasterAssigned {
        worker_id: "peer-a".into(),
        assigned_by: "peer-a".into(),
        approval: Some("legacy approval".into()),
        assigned_ms: 1,
    };
    let unrelated_binding = crate::server::global_state::RuntimeBinding::new(
        registration.project_scope.clone(),
        registration.app_scope_id.clone(),
        crate::identity::AgentId::new("peer-b").unwrap(),
        crate::identity::RuntimeId::new("runtime-b").unwrap(),
        crate::identity::BindingId::new("binding-b").unwrap(),
        1,
        None,
    )
    .unwrap();
    let journal = server_dir.join("journal.jsonl");
    let mut file = std::fs::File::create(&journal).unwrap();
    for event in [
        Event::GlobalProjectRegistered { registration },
        Event::GlobalRuntimeBound {
            binding: unrelated_binding,
        },
        legacy,
    ] {
        writeln!(file, "{}", serde_json::to_string(&event).unwrap()).unwrap();
    }
    drop(file);

    let error = match replay(&root) {
        Ok(_) => panic!("missing binding must fail replay closed"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("MASTER_AUTHORITY_REQUIRES_RUNTIME_BINDING"),
        "{error:#}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_master_assignment_with_ambiguous_binding_fails_replay_closed() {
    use crate::identity::{AppServerId, BindingId, RuntimeId};
    use crate::server::global_state::{ProjectRegistration, RuntimeBinding};
    use std::io::Write;

    let root = std::env::temp_dir().join(format!(
        "collab-legacy-master-ambiguous-binding-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let project_scope =
        crate::server::global_state::GlobalState::canonical_project_scope(&root).unwrap();
    let app_scope = AppServerId::new("tui-default").unwrap();
    let registration = ProjectRegistration::new(project_scope.clone(), app_scope.clone()).unwrap();
    let first = RuntimeBinding::new(
        project_scope.clone(),
        app_scope.clone(),
        crate::identity::AgentId::new("peer-a").unwrap(),
        RuntimeId::new("runtime-a").unwrap(),
        BindingId::new("binding-a").unwrap(),
        1,
        None,
    )
    .unwrap();
    let second = RuntimeBinding::new(
        project_scope,
        app_scope,
        crate::identity::AgentId::new("peer-a").unwrap(),
        RuntimeId::new("runtime-b").unwrap(),
        BindingId::new("binding-b").unwrap(),
        1,
        None,
    )
    .unwrap();
    let legacy = Event::MasterAssigned {
        worker_id: "peer-a".into(),
        assigned_by: "peer-a".into(),
        approval: Some("legacy approval".into()),
        assigned_ms: 1,
    };
    let journal = server_dir.join("journal.jsonl");
    let mut file = std::fs::File::create(&journal).unwrap();
    for event in [
        Event::GlobalProjectRegistered { registration },
        Event::GlobalRuntimeBound { binding: first },
        Event::GlobalRuntimeBound { binding: second },
        legacy,
    ] {
        writeln!(file, "{}", serde_json::to_string(&event).unwrap()).unwrap();
    }
    drop(file);

    let error = match replay(&root) {
        Ok(_) => panic!("ambiguous binding must fail replay closed"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("MASTER_AUTHORITY_AMBIGUOUS_BINDING"),
        "{error:#}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn journal_root_assigned_rewrites_to_master_on_replay() {
    let root = std::env::temp_dir().join(format!(
        "collab-root-to-master-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let journal = server_dir.join("journal.jsonl");
    std::fs::write(
        &journal,
        r#"{"ev":"RootAssigned","worker_id":"peer-a","assigned_by":"peer-a","approval":"user approved","assigned_ms":1}
"#,
    )
    .unwrap();
    let state = replay(&root).unwrap();
    assert_eq!(state.master_worker_id.as_deref(), Some("peer-a"));
    let rewritten = std::fs::read_to_string(&journal).unwrap();
    assert!(rewritten.contains("MasterAssigned"), "{rewritten}");
    assert!(!rewritten.contains("RootAssigned"), "{rewritten}");
    let again = replay(&root).unwrap();
    assert_eq!(again.master_worker_id.as_deref(), Some("peer-a"));
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
    assert_eq!(subscriptions[0].method, "tmux");
    assert_eq!(subscriptions[0].target, "thread-peer");
    assert_eq!(subscriptions[0].status, "armed");
    assert!(subscriptions[0].expires_ms > now_ms());
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn tmux_reregistration_replaces_a_persisted_appserver_default_wake_lease() {
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
                    },
                }],
            )
            .expect("commit legacy appserver lease");
    }

    assert!(register_public(&server, "recipient", "thread-recipient").ok);
    let lease = server.state.lock().unwrap().notification_subscriptions
        ["sub-default-direct-message-recipient"]
        .clone();
    assert_eq!(lease.method, "tmux");
    assert_eq!(lease.target, "thread-recipient");
    assert_eq!(lease.status, "armed");

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
        },
        assignments: Vec::new(),
    }]);

    let ended = handle_goal_schedule_reconcile(
        &server,
        "peer".into(),
        "token-peer".into(),
        "finished-goal".into(),
        "peer".into(),
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

    let replayed = super::replay(&root).expect("journal replay keeps recovery facts");
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
        },
    }]);

    let arbitrary = handle_goal_schedule_reconcile(
        &server,
        "peer".into(),
        "token-peer".into(),
        "different-id".into(),
        "peer".into(),
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
        "peer".into(),
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
    register_synthetic_peer(&server, "master", "%master");
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
            text.contains("request next work or explicit idle approval"))),
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
