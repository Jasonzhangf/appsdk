#[test]
fn role_contract_is_identical_across_context_workers_and_status() {
    use crate::subagent::Record;

    let (server, root) = test_server();
    register(&server, "master", "%master");
    register(&server, "worker", "%worker");
    register(&server, "child", "%child");
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
fn absent_tmux_master_allows_approved_peer_promotion() {
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
    assert!(status.data["master"].is_null(), "{status:?}");
    assert_eq!(status.data["recorded_unusable"]["worker_id"], "peer-a");
    let promoted = super::handle_master_promote(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "user approved peer-b after the previous master runtime died".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());
    assert_eq!(
        super::live_master_id(&server, &server.state.lock().unwrap()).unwrap(),
        Some("peer-b".into())
    );
    let live = super::handle_master_status(&server);
    assert_eq!(live.data["master"]["worker_id"], "peer-b");
    assert!(live.data["recorded_unusable"].is_null());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_tmux_master_cannot_be_superseded_by_approved_peer_promotion() {
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
        super::live_master_id(&server, &server.state.lock().unwrap()).unwrap(),
        Some("peer-a".into())
    );
    let context = handle_context(&server, "peer-b".into(), "token-peer-b".into());
    assert_eq!(context.data["master"]["worker_id"], "peer-a", "{context:?}");

    let promoted = super::handle_master_promote(
        &server,
        "peer-b".into(),
        "token-peer-b".into(),
        "user approved peer-b after the previous master thread became unusable".into(),
    );
    assert!(!promoted.ok, "a live tmux master must retain ownership");
    assert_eq!(
        super::live_master_id(&server, &server.state.lock().unwrap()).unwrap(),
        Some("peer-a".into())
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cross_project_send_requires_master_endpoints_on_both_sides() {
    let (server, root) = test_server();
    register(&server, "target-master", "%target-master");
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
        .contains("target to be a live master"));

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

fn register_appserver(server: &mut Server, id: &str, thread_id: &str) -> Resp {
    register(server, id, thread_id)
}

#[test]
fn init_registration_result_exposes_persisted_runtime_identity() {
    let (mut server, root) = test_server();
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
fn master_promotion_requires_live_tmux_pane() {
    let (mut server, root) = test_server();
    register(&server, "peer-a", "thread-a");
    retire_registered_test_pane(&registered_binding(&server, "peer-a"));
    let denied = super::handle_master_promote(
        &server,
        "peer-a".into(),
        "token-peer-a".into(),
        "user approved peer-a as collab master".into(),
    );
    assert!(!denied.ok);
    assert!(denied.error.unwrap().contains("live registered transport"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_promotion_allows_live_tmux_peer() {
    let (mut server, root) = test_server();
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
        super::live_master_id(&server, &server.state.lock().unwrap()).unwrap(),
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
fn recover_worker_on_new_thread(server: &mut Server, id: &str, thread_id: &str) -> Resp {
    register(server, id, thread_id)
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
fn wire_master_recovery_is_rejected_while_master_is_live() {
    let (mut server, root) = test_server();
    let registered = register_appserver(&mut server, "live-master", "thread-live-master-old");
    assert!(registered.ok, "{registered:?}");
    promote_master(&server, "live-master", "user approved live-master");
    let previous = registered_binding(&server, "live-master");

    let recovered =
        recover_worker_on_new_thread(&mut server, "live-master", "thread-live-master-new");
    assert!(!recovered.ok, "{recovered:?}");
    assert!(recovered
        .error
        .as_deref()
        .unwrap()
        .starts_with("MASTER_RECOVERY_BLOCKED_LIVE:"));
    let state = server.state.lock().unwrap();
    let current = state
        .global
        .lookup_binding_for(&previous.route_scope(), &previous.binding_id)
        .unwrap();
    assert_eq!(current.endpoint_generation, previous.endpoint_generation);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn wire_master_recovery_is_rejected_when_liveness_is_unknown() {
    let (mut server, root) = test_server();
    let registered = register_appserver(&mut server, "unknown-master", "thread-unknown-master-old");
    assert!(registered.ok, "{registered:?}");
    promote_master(&server, "unknown-master", "user approved unknown-master");
    stop_registered_test_tmux_server();

    let recovered =
        recover_worker_on_new_thread(&mut server, "unknown-master", "thread-unknown-master-new");
    assert!(!recovered.ok, "{recovered:?}");
    assert!(recovered
        .error
        .as_deref()
        .unwrap()
        .starts_with("MASTER_RECOVERY_BLOCKED_UNKNOWN:"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn wire_peer_recover_does_not_revoke_unrelated_master() {
    let (mut server, root) = test_server();
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
    let registered = register_appserver(&mut server, "peer-appserver", "thread-appserver");
    assert!(registered.ok, "{registered:?}");
    let promoted = super::handle_master_promote(
        &server,
        "peer-appserver".into(),
        "token-peer-appserver".into(),
        "user approved peer-appserver as collab master".into(),
    );
    assert!(promoted.ok, "{}", promoted.error.unwrap_or_default());

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
    assert_eq!(grant.granted_by, "peer-appserver");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_master_assignment_before_runtime_rebind_does_not_regrant_authority() {
    use std::io::Write;

    let (mut server, root) = test_server();
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
    assert_eq!(
        register(&server, "peer-a", "%peer-a").data["identity_kind"],
        "peer"
    );
    assert_eq!(
        register(&server, "peer-b", "%peer-b").data["identity_kind"],
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
    assert!(register(&server, "peer", "%peer").ok);
    assert!(register(&server, "peer", "%peer").ok);
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
    assert!(register(&server, "sender", "thread-sender").ok);
    assert!(register(&server, "recipient", "thread-recipient").ok);
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
                    },
                }],
            )
            .expect("commit legacy appserver lease");
    }

    assert!(register(&server, "recipient", "thread-recipient").ok);
    let lease = server.state.lock().unwrap().notification_subscriptions
        ["sub-default-direct-message-recipient"]
        .clone();
    assert_eq!(lease.method, "tmux");
    assert_eq!(lease.target, "thread-recipient");
    assert_eq!(lease.status, "armed");

    server.appserver_notification_sink = Arc::new(|transport, _, _, _, _, _mode| {
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
fn cancelled_default_lease_stays_suppressed_until_explicit_subscribe() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);

    let cancelled = handle_notification_unsubscribe(
        &server,
        "peer".into(),
        "token-peer".into(),
        "sub-default-direct-message-peer".into(),
    );
    assert!(cancelled.ok, "{}", cancelled.error.unwrap_or_default());
    assert!(register(&server, "peer", "%peer").ok);

    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["sub-default-direct-message-peer"].status,
        "cancelled"
    );
    assert!(default_direct_message_events(
        &state,
        "peer",
        &test_tmux_transport("thread-peer"),
        now_ms()
    )
    .is_empty());
    drop(state);

    let explicit = handle_notification_subscribe(
        &server,
        "peer".into(),
        "token-peer".into(),
        "direct-message".into(),
        None,
        None,
        Vec::new(),
        None,
        1,
        3_600,
    );
    assert!(explicit.ok, "{}", explicit.error.unwrap_or_default());
    let explicit_id = explicit.data["subscription"]["id"].as_str().unwrap();
    assert_ne!(explicit_id, "sub-default-direct-message-peer");
    assert_eq!(
        explicit.data["subscription"]["status"],
        serde_json::Value::String("armed".into())
    );

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn legacy_cancelled_default_lease_is_rearmed_but_explicit_unsubscribe_stays_suppressed() {
    let (server, root) = test_server();
    assert!(register(&server, "peer", "%peer").ok);

    // Simulate a lease cancelled by the removed automatic close path: no
    // explicit-unsubscribe reason was recorded.
    server.commit(&[Event::NotificationStatus {
        subscription_id: "sub-default-direct-message-peer".into(),
        status: "cancelled".into(),
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

    // An explicit owner unsubscribe is a durable stop and must not be re-armed
    // by registration or daemon replay.
    let cancelled = handle_notification_unsubscribe(
        &server,
        "peer".into(),
        "token-peer".into(),
        "sub-default-direct-message-peer".into(),
    );
    assert!(cancelled.ok, "{}", cancelled.error.unwrap_or_default());
    assert!(register(&server, "peer", "%peer").ok);
    let state = server.state.lock().unwrap();
    assert_eq!(
        state.notification_subscriptions["sub-default-direct-message-peer"]
            .status_reason
            .as_deref(),
        Some("explicit-unsubscribe")
    );
    assert!(default_direct_message_events(
        &state,
        "peer",
        &test_tmux_transport("thread-peer"),
        now_ms()
    )
    .is_empty());
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
