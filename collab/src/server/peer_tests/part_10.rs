#[test]
fn context_is_read_only_and_does_not_consume_notifications() {
    let (mut server, root) = test_server();
    register(&server, "peer", "%peer");
    register(&server, "peer-two", "%peer-two");
    let expected_tmux_endpoint = server.state.lock().unwrap().workers["peer"]
        .transport
        .as_ref()
        .unwrap()
        .tmux_endpoint
        .clone()
        .unwrap();
    let worktree = configured_test_worktree(&mut server, &root, "peer-task");
    let other_worktree = configured_test_worktree(&mut server, &root, "other-task");
    assert!(
        handle_task_register(
            &server,
            "peer".into(),
            "token-peer".into(),
            "task".into(),
            None,
            Some("feature".into()),
            Some(worktree.display().to_string()),
            Some("peer-branch".into()),
            Some("peer-base".into()),
            default_priority(),
        )
        .ok
    );
    assert!(
        handle_task_register(
            &server,
            "peer-two".into(),
            "token-peer-two".into(),
            "other-task".into(),
            None,
            Some("other-feature".into()),
            Some(other_worktree.display().to_string()),
            Some("other-branch".into()),
            Some("other-base".into()),
            default_priority(),
        )
        .ok
    );
    let message_id = "notification".to_string();
    server.commit(&[Event::Sent {
        msg: Message {
            id: message_id.clone(),
            from: "peer-two".into(),
            to: "peer".into(),
            mtype: "notify".into(),
            subject: Some("released:task".into()),
            body: "RESOURCE_RELEASED task=task".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);

    let context = handle_context(&server, "peer".into(), "token-peer".into());
    assert!(context.ok);
    assert_eq!(context.data["schema_version"], 1);
    assert_eq!(context.data["registered"], true);
    assert_eq!(context.data["registration"]["status"], "registered");
    assert!(context.data["master"].is_null());
    assert!(context.data["recorded_unusable"].is_null());
    assert_eq!(context.data["authority"]["must_obey_master"], false);
    assert_eq!(context.data["authority"]["may_decline_master_invite"], true);
    assert_eq!(context.data["inbox"]["unread"], 1);
    assert_eq!(context.data["inbox"]["messages"][0]["id"], message_id);
    assert_eq!(
        context.data["inbox"]["messages"][0]["body"],
        "RESOURCE_RELEASED task=task"
    );
    assert_eq!(context.data["identity"]["role"], "worker");
    let operations = context.data["operations"].as_array().expect("operations");
    assert!(
        !operations.is_empty(),
        "worker context must expose operations"
    );
    assert!(
        operations.iter().any(|operation| {
            operation["kind"] == "promote_master"
                && operation["requires_approval"] == true
                && operation["action"]
                    .as_str()
                    .is_some_and(|action| action.contains("collab master promote --approval"))
        }),
        "no-live-master context must expose the user-approved promote operation"
    );
    assert!(operations.iter().any(|operation| {
        operation["kind"] == "next_action"
            && operation["action"] == context.data["role_brief"]["next_action"]
    }));
    assert_eq!(
        context.data["identity"]["transport"]["tmux_endpoint"],
        serde_json::to_value(expected_tmux_endpoint).unwrap()
    );
    assert_eq!(context.data["agent"]["thread_state"], "unknown");
    assert!(context.data["agent"]["can_accept_direct_input"].is_null());
    assert_eq!(context.data["tasks"][0]["id"], "task");
    assert_eq!(context.data["worktrees"].as_array().unwrap().len(), 1);
    assert_eq!(context.data["worktrees"][0]["task_id"], "task");
    for peer in context.data["peers"].as_array().unwrap() {
        assert!(peer.get("tasks").is_none());
        assert!(peer.get("transport").is_none());
        assert!(peer.get("agent").is_none());
    }
    assert!(!context.data.to_string().contains("other-task"));
    assert!(!context.data.to_string().contains("other-branch"));
    assert!(!context.data.to_string().contains("other-base"));
    assert!(context.data["subscriptions"].is_array());
    assert_eq!(context.data["daemon"]["live"], true);
    assert_eq!(
        context.data["daemon"]["pid"],
        serde_json::json!(std::process::id())
    );
    let state = server.state.lock().unwrap();
    assert_eq!(state.msgs[&message_id].state, "pending");
    drop(state);
    std::fs::remove_dir_all(server.config.worktree.base.as_ref().unwrap()).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn context_gives_an_idle_master_one_canonical_scheduling_action() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user-approved");

    let context = handle_context(&server, "master".into(), "token-master".into());

    assert!(context.ok, "{}", context.error.unwrap_or_default());
    assert_eq!(context.data["identity"]["role"], "master");
    assert_eq!(context.data["master"]["worker_id"], "master");
    // The grant and reachable pane are separate from unknown agent activity.
    assert_eq!(context.data["master"]["endpoint_live"], true);
    assert_eq!(context.data["agent"]["thread_state"], "unknown");
    assert!(context.data["recorded_unusable"].is_null());
    assert_eq!(
        context.data["next_actions"].as_array().unwrap().len(),
        1
    );
    let next_action = context.data["next_actions"][0].as_str().unwrap();
    let next_action_lower = next_action.to_ascii_lowercase();
    assert!(
        next_action_lower.contains("long-horizon goals"),
        "{next_action}"
    );
    assert!(
        next_action_lower.contains("regardless of whether other peers are busy"),
        "{next_action}"
    );
    assert!(
        next_action_lower.contains("protected_holds"),
        "{next_action}"
    );
    assert!(
        next_action_lower.contains("re-arm"),
        "{next_action}"
    );
    let operations = context.data["operations"].as_array().expect("operations");
    assert!(
        !operations.is_empty(),
        "master context must expose operations"
    );
    assert!(
        operations
            .iter()
            .any(|operation| operation["kind"] == "next_action"
                && operation["action"].as_str().is_some_and(|action| {
                    let action = action.to_ascii_lowercase();
                    action.contains("regardless of whether other peers are busy")
                        && action.contains("protected_holds")
                        && action.contains("re-arm")
                }))
    );
    assert!(context.data["role_brief"]["responsibilities"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("Delivery, merge, or a review verdict is not a lifecycle endpoint; drive review/integration/cleanup/close and assign the next ready P0/P1 task.")));
    assert!(context.data["role_brief"]["responsibilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| {
            line.as_str().is_some_and(|line| {
                line.contains("converges owned worktrees") && line.contains("PROTECTED_HOLD")
            })
        }));
    assert_eq!(
        context.data["role_brief"]["next_action"],
        context.data["next_actions"][0]
    );
    assert!(!context
        .data
        .to_string()
        .contains("saturate live present peers first"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn master_context_routes_work_by_task_kind() {
    let (server, root) = test_server();
    register_synthetic_peer(&server, "master", "%master");
    promote_master(&server, "master", "user-approved");

    let context = handle_context(&server, "master".into(), "token-master".into());

    assert!(context.ok, "{}", context.error.unwrap_or_default());
    assert_eq!(context.data["identity"]["role"], "master");
    assert_eq!(
        context.data["next_actions"],
        serde_json::json!([context.data["role_brief"]["next_action"].clone()])
    );
    let responsibilities = context.data["role_brief"]["responsibilities"]
        .as_array()
        .expect("master responsibilities")
        .iter()
        .filter_map(|line| line.as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    assert!(responsibilities.contains("long-horizon goals"));
    assert!(responsibilities.contains("bounded subagent tasks"));
    assert!(responsibilities.contains("regardless of whether other peers are busy"));
    assert!(responsibilities.contains("codex exec --profile gcm"));
    assert!(!responsibilities.contains("saturate every live present peer first"));
    assert!(!context
        .data
        .to_string()
        .contains("saturate live present peers first"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn board_invitation_uses_authoritative_binding_when_identity_projection_is_stale() {
    let (server, root) = test_server();
    let server = Arc::new(server);
    register(&server, "master", "%master");
    register(&server, "peer", "%peer");
    promote_master(&server, "master", "user-approved");

    let peer_binding = {
        let state = server.state.lock().unwrap();
        state
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .find(|binding| binding.agent_id.as_str() == "peer")
            .cloned()
            .expect("peer authoritative binding")
    };
    server
        .commit_checked(&[Event::GlobalCurrentThreadRouteRetired {
            binding: peer_binding,
        }])
        .expect("retire only the current route projection");

    let context = handle_context(&server, "peer".into(), "token-peer".into());
    assert!(context.ok, "{:?}", context.error);
    assert_eq!(context.data["registered"], true);
    assert_eq!(context.data["liveness"]["live"], true);

    let status = dispatch_with_route_context(&server, Req::StatusAll, None);
    assert!(status.ok, "{:?}", status.error);
    let peer_status = status.data["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worker| worker["id"] == "peer")
        .expect("peer status");
    assert_eq!(peer_status["identity_valid"], true);
    assert_ne!(peer_status["status"], "identity-mismatch");
    assert_eq!(peer_status["presence"], "present");
    assert_eq!(peer_status["endpoint_live"], true);

    let published = handle_board_command(
        &server,
        "master".into(),
        "token-master".into(),
        crate::board::BoardCommand::Publish {
            id: "stale-route-invite".into(),
            title: "stale route invite".into(),
            description: "durable invitation admission".into(),
            delivery_condition: "invite and acceptance are defined".into(),
            test_condition: "two peer regression passes".into(),
            priority: "p1".into(),
            work_kind: None,
            parent_task: None,
            prefer_role: None,
            executor: None,
            reviewer: None,
        },
    );
    assert!(published.ok, "{:?}", published.error);

    let invited = handle_board_command(
        &server,
        "master".into(),
        "token-master".into(),
        crate::board::BoardCommand::Invite {
            id: "stale-route-invite".into(),
            to: "peer".into(),
            expected_revision: 1,
        },
    );
    assert!(invited.ok, "{:?}", invited.error);
    assert_eq!(invited.data["task"]["status"], "invited");

    let accepted = handle_board_command(
        &server,
        "peer".into(),
        "token-peer".into(),
        crate::board::BoardCommand::Respond {
            id: "stale-route-invite".into(),
            accept: true,
            decline: false,
            expected_revision: 2,
            reason: None,
        },
    );
    assert!(accepted.ok, "{:?}", accepted.error);
    assert_eq!(accepted.data["task"]["owner"], "peer");
    assert_eq!(accepted.data["task"]["status"], "working");

    let accepted_context = handle_context(&server, "peer".into(), "token-peer".into());
    assert!(accepted_context.ok, "{:?}", accepted_context.error);
    assert_eq!(
        accepted_context.data["role_brief"]["role"], "worker",
        "the durable invitation must not be revoked by stale status identity"
    );
    assert!(
        accepted_context.data["role_brief"]["next_action"]
            .as_str()
            .unwrap_or_default()
            .contains("accepted goal's long-horizon orchestrator"),
        "{}",
        accepted_context.data["role_brief"]
    );

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn binding_projection_is_scoped_before_same_named_binding_uniqueness() {
    let (server, root) = test_server();
    register(&server, "scoped-peer", "%scoped-peer");
    let (worker, binding) = {
        let state = server.state.lock().unwrap();
        (state.workers["scoped-peer"].clone(), state.global.projects.values()
            .flat_map(|project| project.runtime_bindings.values())
            .find(|binding| binding.agent_id.as_str() == "scoped-peer").unwrap().clone())
    };
    let foreign_root = root.join("foreign-project");
    std::fs::create_dir_all(&foreign_root).unwrap();
    let mut foreign = binding.clone();
    foreign.project_scope = GlobalState::canonical_project_scope(&foreign_root).unwrap();
    foreign.binding_id = BindingId::new("foreign-binding").unwrap();
    let mut state = server.state.lock().unwrap();
    state.apply_checked(&Event::GlobalProjectRegistered {
        registration: global_state::ProjectRegistration::new(foreign.project_scope.clone(), foreign.app_scope_id.clone()).unwrap(),
    }).unwrap();
    state.apply_checked(&Event::GlobalRuntimeBound { binding: foreign }).unwrap();
    drop(state);
    assert_eq!(worker_identity_presence(&server, &worker), IdentityPresence::Present,
        "a same-named foreign binding must not invalidate the local identity");
    let task = TaskRec {
        id: "scoped-owner-task".into(), owner: worker.id.clone(), created_by: worker.id.clone(),
        feature_id: None, worktree_path: None, branch: None, base_commit: None,
        priority: "p1".into(), status: "working".into(), next_step: None, wait: None,
        created_ms: now_ms(), updated_ms: now_ms(),
    };
    let mut state = server.state.lock().unwrap();
    state.apply_checked(&Event::TaskCreated { task: task.clone() }).unwrap();
    state.apply_checked(&Event::BoardDetailsChanged { task_id: task.id.clone(), details:
        crate::board::BoardTaskDetails { public_visibility: true, revision: 1, ..Default::default() }
    }).unwrap();
    assert_eq!(board_task_view(&state, &task).unwrap().owner, worker.id);
    let mut other_app = binding.clone();
    other_app.app_scope_id = AppServerId::new("other-app").unwrap();
    other_app.binding_id = BindingId::new("other-app-binding").unwrap();
    other_app.runtime_id = RuntimeId::new("other-app-runtime").unwrap();
    state.apply_checked(&Event::GlobalProjectRegistered {
        registration: global_state::ProjectRegistration::new(other_app.project_scope.clone(), other_app.app_scope_id.clone()).unwrap(),
    }).unwrap();
    state.apply_checked(&Event::GlobalRuntimeBound { binding: other_app.clone() }).unwrap();
    assert_eq!(board_task_view_with_master(&state, &task, None, None, Some(&binding.route_scope())).unwrap().owner, worker.id,
        "a same-named binding in another app must not invalidate the requested board owner");
    state.global.projects.get_mut(binding.project_scope.as_str()).unwrap()
        .runtime_bindings.remove(other_app.binding_id.as_str());
    state.global.projects.get_mut(binding.project_scope.as_str()).unwrap()
        .runtime_bindings.remove(binding.binding_id.as_str());
    drop(state);
    assert_eq!(worker_identity_presence(&server, &worker), IdentityPresence::Missing,
        "a foreign-only binding must not prove a local identity");
    let mut state = server.state.lock().unwrap();
    assert_eq!(board_task_view(&state, &task).unwrap().owner, "");
    state.apply_checked(&Event::GlobalRuntimeBound { binding: binding.clone() }).unwrap();
    let mut duplicate = binding.clone();
    duplicate.binding_id = BindingId::new("duplicate-local-binding").unwrap();
    duplicate.runtime_id = RuntimeId::new("duplicate-local-runtime").unwrap();
    state.apply_checked(&Event::GlobalRuntimeBound { binding: duplicate }).unwrap();
    assert_eq!(board_task_view(&state, &task).unwrap().owner, "");
    drop(state);
    assert_eq!(worker_identity_presence(&server, &worker), IdentityPresence::Missing,
        "multiple applicable local bindings must not be guessed");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn context_does_not_project_retired_appserver_thread_or_turn_state() {
    let (mut server, root) = test_server();
    let registration = register(&server, "state-peer", "thread-state-peer");
    assert!(
        registration.ok,
        "{}",
        registration.error.unwrap_or_default()
    );
    server.appserver_thread_status = Arc::new(|_, _| {
        Err("APP_SERVER_RETIRED: the registered AppServer endpoint is gone".into())
    });
    let context = handle_context(&server, "state-peer".into(), "token-state-peer".into());
    assert_eq!(context.data["agent"]["thread_state"], "unknown");
    let status = dispatch(&Arc::new(server), Req::WorkerStatus { worker_id: None });
    assert_eq!(status.data["workers"][0]["agent_state"], "unknown");
    // A retired AppServer callback cannot change the committed tmux identity
    // or its reachable address, and cannot fabricate agent activity.
    assert_eq!(status.data["workers"][0]["presence"], "present");
    assert_eq!(status.data["workers"][0]["identity_valid"], true);
    assert_eq!(status.data["workers"][0]["endpoint_live"], true);

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn retired_appserver_status_callback_leaves_a_tmux_binding_unknown() {
    let (mut server, root) = test_server();
    let registration = register(&server, "timeout-peer", "thread-timeout-peer");
    assert!(
        registration.ok,
        "{}",
        registration.error.unwrap_or_default()
    );
    server.appserver_thread_status = Arc::new(|_, _| {
        Err("APP_SERVER_RETIRED: the registered AppServer endpoint is gone".into())
    });

    let context = handle_context(&server, "timeout-peer".into(), "token-timeout-peer".into());
    assert_eq!(context.data["agent"]["thread_state"], "unknown");

    let status = dispatch(&Arc::new(server), Req::WorkerStatus { worker_id: None });
    assert_eq!(status.data["workers"][0]["presence"], "present");
    assert_eq!(status.data["workers"][0]["endpoint_live"], true);
    assert_eq!(status.data["workers"][0]["identity_valid"], true);
    assert_eq!(status.data["workers"][0]["agent_state"], "unknown");

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn retired_appserver_route_error_leaves_a_tmux_binding_unknown_not_missing() {
    let (mut server, root) = test_server();
    let registration = register(&server, "missing-peer", "thread-missing-peer");
    assert!(
        registration.ok,
        "{}",
        registration.error.unwrap_or_default()
    );
    server.appserver_thread_status =
        Arc::new(|_, _| Err("APP_SERVER_ROUTE_RETIRED: the registered route is gone".into()));

    let status = dispatch(&Arc::new(server), Req::WorkerStatus { worker_id: None });
    // The retired AppServer route has no authority over this tmux identity,
    // address or activity projection.
    assert_eq!(status.data["workers"][0]["presence"], "present");
    assert_eq!(status.data["workers"][0]["endpoint_live"], true);
    assert_eq!(status.data["workers"][0]["identity_valid"], true);
    assert_eq!(status.data["workers"][0]["agent_state"], "unknown");

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn tmux_presence_never_consults_the_appserver_candidate_check() {
    let (mut server, root) = test_server();
    let registration = register(&server, "timeout-identity", "thread-timeout-identity");
    assert!(
        registration.ok,
        "{}",
        registration.error.unwrap_or_default()
    );
    server.appserver_candidate_check =
        Arc::new(|_| panic!("tmux presence must not call the AppServer candidate checker"));

    // The pane address is reachable and the identity is committed. Runtime
    // activity stays unknown; the AppServer oracle is never consulted.
    let status = dispatch(&Arc::new(server), Req::WorkerStatus { worker_id: None });
    assert_eq!(status.data["workers"][0]["presence"], "present");
    assert_eq!(status.data["workers"][0]["endpoint_live"], true);
    assert_eq!(status.data["workers"][0]["identity_valid"], true);
    assert_eq!(status.data["workers"][0]["agent_state"], "unknown");

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_tmux_pane_id_recorded_as_the_codex_thread_is_not_liveness() {
    let (mut server, root) = test_server();
    // A tmux registration copies CODEX_SESSION_ID and CODEX_THREAD_ID from the
    // caller's environment, so a worker can record its own pane address as the
    // thread. The production AppServer oracle must refuse a tmux transport
    // instead of answering from the pane.
    server.appserver_thread_status = default_appserver_thread_status();
    let tmux = IsolatedTmux::start_single(&root);
    let mut endpoint = tmux.endpoints().remove(0);
    endpoint.codex_session_id = Some("session-pane-address".into());
    endpoint.codex_thread_id = Some(endpoint.pane_id.clone());
    let registration = register_tmux(&server, "pane-address-worker", endpoint);
    assert!(registration.ok, "{registration:?}");

    let worker = server.state.lock().unwrap().workers["pane-address-worker"].clone();
    assert_eq!(worker_presence(&server, &worker), IdentityPresence::Present);

    let status = dispatch(&Arc::new(server), Req::WorkerStatus { worker_id: None });
    assert_eq!(status.data["workers"][0]["presence"], "present");
    assert_eq!(status.data["workers"][0]["endpoint_live"], true);
    assert_eq!(status.data["workers"][0]["identity_valid"], true);
    assert_eq!(status.data["workers"][0]["agent_state"], "unknown");
    drop(tmux);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn architecture_source_has_no_live_declared_role_or_dispatch_owner() {
    let server_source = include_str!("../mod.rs");
    let state_source = include_str!("../state.rs");
    let mcp_source = include_str!("../../bin/collab-mcp.rs");
    for removed in [
        "#[cfg(any())]",
        "fn default_role",
        "fn handle_transfer_master",
        "fn idle_worker_ids",
        "TASK_OFFER",
        "TASK_DELIVERED",
        "master_notified",
    ] {
        assert!(
            !server_source.contains(removed),
            "removed runtime semantic remains: {removed}"
        );
    }
    assert!(!state_source.contains("pub role:"));
    assert!(!state_source.contains("pub goal_prompt:"));
    assert!(!state_source.contains("pub goal_busy:"));
    assert!(!state_source.contains("pub nudge_count:"));
    assert!(!state_source.contains("pub last_nudge_ms:"));
    for removed_tool in [
        "\"collab_role\"",
        "\"collab_root\"",
        "\"collab_task_claim\"",
        "\"collab_task_dispatch\"",
        "\"project_root\"",
        "args.get(\"pane\")",
    ] {
        assert!(
            !mcp_source.contains(removed_tool),
            "removed MCP tool remains: {removed_tool}"
        );
    }
}

#[test]
fn active_lifecycle_manifest_binds_every_registered_call_edge_to_source() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/collab-v1-lifecycle.manifest.json")).unwrap();
    let call_map: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/mainline-call-map.json")).unwrap();
    assert_eq!(manifest["status"], "active");
    assert_eq!(call_map["status"], "active");
    assert_eq!(manifest["lifecycle_id"], call_map["lifecycle_id"]);

    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = String::new();
    let mut stack = vec![project_root.join("src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                sources.push_str(&std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    for edge in call_map["edges"].as_array().unwrap() {
        for field in ["caller", "callee"] {
            let symbol = edge[field].as_str().unwrap();
            assert!(
                sources.contains(symbol),
                "{field} {symbol} is not bound in src/"
            );
        }
    }
    for path in manifest["canonical_docs"].as_array().unwrap() {
        assert!(
            project_root.join(path.as_str().unwrap()).is_file(),
            "missing canonical doc: {path}"
        );
    }
}

#[test]
fn cleanup_rejects_unmerged_then_removes_only_merged_clean_worktree() {
    let root = std::env::temp_dir().join(format!(
        "collab-close-git-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let playground = root.join("playground");
    std::fs::create_dir_all(&playground).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&root)
            .args(args)
            .output()
            .unwrap()
    };
    assert!(git(&["init", "-q", "--initial-branch=main"])
        .status
        .success());
    assert!(git(&["config", "user.email", "test@example.com"])
        .status
        .success());
    assert!(git(&["config", "user.name", "collab test"])
        .status
        .success());
    std::fs::write(root.join("README.md"), "base\n").unwrap();
    assert!(git(&["add", "README.md"]).status.success());
    assert!(git(&["commit", "-q", "-m", "base"]).status.success());
    assert!(
        git(&["worktree", "add", "-q", "-b", "feature", "playground/wt"])
            .status
            .success()
    );
    std::fs::write(root.join("playground/wt/feature.txt"), "work\n").unwrap();
    assert!(git(&["-C", "playground/wt", "add", "feature.txt"])
        .status
        .success());
    assert!(
        git(&["-C", "playground/wt", "commit", "-q", "-m", "feature"])
            .status
            .success()
    );

    let refused = close_task_resources(&root, &crate::config::Config::default(), Some("playground/wt"), Some("feature"), None);
    assert!(refused.unwrap_err().contains("not merged"));
    assert!(playground.join("wt").is_dir());

    assert!(git(&["merge", "-q", "feature"]).status.success());
    assert!(close_task_resources(&root, &crate::config::Config::default(), Some("playground/wt"), Some("feature"), None).is_ok());
    assert!(!playground.join("wt").exists());
    assert!(!git(&["rev-parse", "--verify", "feature"]).status.success());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn close_task_resources_accepts_squash_only_with_reachable_integration_commit() {
    let root = std::env::temp_dir().join(format!(
        "collab-close-squash-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let playground = root.join("playground");
    std::fs::create_dir_all(&playground).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&root)
            .args(args)
            .output()
            .unwrap()
    };
    assert!(git(&["init", "-q", "--initial-branch=main"])
        .status
        .success());
    assert!(git(&["config", "user.email", "test@example.com"])
        .status
        .success());
    assert!(git(&["config", "user.name", "collab test"])
        .status
        .success());
    std::fs::write(root.join("README.md"), "base\n").unwrap();
    assert!(git(&["add", "README.md"]).status.success());
    assert!(git(&["commit", "-q", "-m", "base"]).status.success());
    assert!(
        git(&["worktree", "add", "-q", "-b", "feature", "playground/wt"])
            .status
            .success()
    );
    std::fs::write(root.join("playground/wt/feature.txt"), "work\n").unwrap();
    assert!(git(&["-C", "playground/wt", "add", "feature.txt"])
        .status
        .success());
    assert!(
        git(&["-C", "playground/wt", "commit", "-q", "-m", "feature"])
            .status
            .success()
    );
    let feature_tip = String::from_utf8_lossy(&git(&["rev-parse", "feature"]).stdout)
        .trim()
        .to_string();
    assert!(git(&["merge", "--squash", "feature"]).status.success());
    assert!(git(&["commit", "-q", "-m", "squash feature"]).status.success());
    let squash = String::from_utf8_lossy(&git(&["rev-parse", "main"]).stdout)
        .trim()
        .to_string();

    // No evidence and evidence outside main both keep the refusal.
    assert!(close_task_resources(
        &root,
        &crate::config::Config::default(),
        Some("playground/wt"),
        Some("feature"),
        None,
    )
    .unwrap_err()
    .contains("not merged"));
    assert!(close_task_resources(
        &root,
        &crate::config::Config::default(),
        Some("playground/wt"),
        Some("feature"),
        Some(&feature_tip),
    )
    .unwrap_err()
    .contains("not reachable from refs/heads/main"));
    assert!(playground.join("wt").is_dir());
    assert!(git(&["rev-parse", "--verify", "feature"]).status.success());

    // The reachable squash commit proves the branch work is on main.
    assert!(close_task_resources(
        &root,
        &crate::config::Config::default(),
        Some("playground/wt"),
        Some("feature"),
        Some(&squash),
    )
    .is_ok());
    assert!(!playground.join("wt").exists());
    assert!(!git(&["rev-parse", "--verify", "feature"]).status.success());
    std::fs::remove_dir_all(root).ok();
}

/// Build a task whose branch was squash-merged into `main`. The squash commit
/// replaces the branch tip, so the branch is not an ancestor of `main`. With
/// `force_close` the task is closed manually without cleanup; otherwise the
/// task is seeded as fully integrated and awaiting the normal close path.
/// Returns the squash commit and the unreachable feature tip.
fn squash_merged_holder(
    server: &Server,
    root: &Path,
    task_id: &str,
    recorded_integration: bool,
    force_close: bool,
) -> (String, String) {
    let playground = root.join("playground");
    std::fs::create_dir_all(&playground).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap()
    };
    assert!(git(&["init", "-q", "--initial-branch=main"])
        .status
        .success());
    assert!(git(&["config", "user.email", "test@example.com"])
        .status
        .success());
    assert!(git(&["config", "user.name", "collab test"])
        .status
        .success());
    std::fs::write(root.join("README.md"), "base\n").unwrap();
    assert!(git(&["add", "README.md"]).status.success());
    assert!(git(&["commit", "-q", "-m", "base"]).status.success());
    assert!(
        git(&["worktree", "add", "-q", "-b", "feature", "playground/squash-wt"])
            .status
            .success()
    );
    std::fs::write(root.join("playground/squash-wt/feature.txt"), "work\n").unwrap();
    assert!(git(&["-C", "playground/squash-wt", "add", "feature.txt"])
        .status
        .success());
    assert!(
        git(&["-C", "playground/squash-wt", "commit", "-q", "-m", "feature"])
            .status
            .success()
    );
    let feature_tip = String::from_utf8_lossy(&git(&["rev-parse", "feature"]).stdout)
        .trim()
        .to_string();
    assert!(git(&["merge", "--squash", "feature"]).status.success());
    assert!(git(&["commit", "-q", "-m", "squash feature"]).status.success());
    let squash = String::from_utf8_lossy(&git(&["rev-parse", "main"]).stdout)
        .trim()
        .to_string();
    assert_ne!(feature_tip, squash);

    register_synthetic_peer(server, "holder", "%holder");
    register_synthetic_peer(server, "master", "%master");
    promote_master(server, "master", "user approved squash cleanup test");
    let canonical_wt = root
        .join("playground/squash-wt")
        .canonicalize()
        .unwrap()
        .display()
        .to_string();
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: task_id.into(),
            owner: "holder".into(),
            created_by: "holder".into(),
            feature_id: None,
            worktree_path: Some(canonical_wt),
            branch: Some("feature".into()),
            base_commit: None,
            priority: default_priority(),
            status: "working".into(),
            next_step: None,
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
    if force_close {
        let forced = handle_task_close(
            server,
            "master".into(),
            "token-master".into(),
            task_id.into(),
            true,
            Some("holder abandoned the worktree; force closing".into()),
        );
        assert!(forced.ok, "{}", forced.error.unwrap_or_default());
        assert_eq!(forced.data["cleanup"]["result"], "unverified");
        if recorded_integration {
            server.commit(&[Event::TaskLifecycleUpdated {
                task_id: task_id.into(),
                record: crate::server::state::TaskLifecycleRecord {
                    integration_commit: Some(squash.clone()),
                    integration_evidence: Some("squash commit recorded on main".into()),
                    integrated_ms: Some(now_ms()),
                    ..Default::default()
                },
            }]);
        }
    } else {
        let updated_ms = now_ms();
        let mut merged = server.state.lock().unwrap().tasks[task_id].clone();
        merged.status = "merged".into();
        merged.updated_ms = updated_ms;
        server.commit(&[
            Event::TaskUpdated { task: merged },
            Event::TaskLifecycleUpdated {
                task_id: task_id.into(),
                record: crate::server::state::TaskLifecycleRecord {
                    delivery_kind: crate::server::state::TaskDeliveryKind::Code,
                    delivery_evidence: Some("candidate delivered".into()),
                    delivered_ms: Some(updated_ms),
                    delivery_commit: Some(feature_tip.clone()),
                    review_evidence: Some("review pass".into()),
                    reviewer: Some("master".into()),
                    reviewed_ms: Some(updated_ms),
                    integration_commit: Some(squash.clone()),
                    integration_evidence: Some("squash commit recorded on main".into()),
                    integrated_ms: Some(updated_ms),
                    ..Default::default()
                },
            },
        ]);
    }
    (squash, feature_tip)
}

#[test]
fn finalize_accepts_squash_merged_branch_from_recorded_integration() {
    let (server, root) = test_server();
    squash_merged_holder(&server, &root, "squash-held", true, true);
    let finalized = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "squash-held".into(),
        None,
    );
    assert!(finalized.ok, "{finalized:?}");
    assert_eq!(finalized.data["cleanup"]["result"], "verified");
    assert!(!root.join("playground/squash-wt").exists());
    assert!(!Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--verify", "refs/heads/feature"])
        .output()
        .unwrap()
        .status
        .success());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn finalize_keeps_refusal_without_integration_evidence() {
    let (server, root) = test_server();
    squash_merged_holder(&server, &root, "squash-held", false, true);
    let refused = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "squash-held".into(),
        None,
    );
    assert!(!refused.ok, "{refused:?}");
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|error| error.contains("not merged")),
        "{refused:?}"
    );
    assert!(root.join("playground/squash-wt").is_dir());
    assert!(Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--verify", "refs/heads/feature"])
        .output()
        .unwrap()
        .status
        .success());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn finalize_rejects_unreachable_and_fabricated_commits_then_accepts_reachable_commit() {
    let (server, root) = test_server();
    let (squash, feature_tip) = squash_merged_holder(&server, &root, "squash-held", false, true);

    // The feature tip resolves but never landed on main: fail closed.
    let unreachable = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "squash-held".into(),
        Some(feature_tip.clone()),
    );
    assert!(!unreachable.ok, "{unreachable:?}");
    assert_eq!(
        unreachable.error.as_deref(),
        Some("CLEANUP_FINALIZE_COMMIT_NOT_IN_MAIN")
    );
    assert_eq!(unreachable.data["provided"], feature_tip);
    assert!(unreachable.data["main_head"].is_string());
    assert_eq!(
        unreachable.data["expected"],
        "a commit reachable from refs/heads/main"
    );
    assert!(root.join("playground/squash-wt").is_dir());

    // A commit that does not exist cannot authorize deletion.
    let fabricated = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "squash-held".into(),
        Some("0123456789012345678901234567890123456789".into()),
    );
    assert!(!fabricated.ok, "{fabricated:?}");
    assert!(
        fabricated
            .error
            .as_deref()
            .is_some_and(|error| error.contains("TASK_INTEGRATION_COMMIT_UNRESOLVED")),
        "{fabricated:?}"
    );
    assert!(root.join("playground/squash-wt").is_dir());

    // An empty value is not evidence either.
    let empty = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "squash-held".into(),
        Some("   ".into()),
    );
    assert!(!empty.ok, "{empty:?}");
    assert_eq!(
        empty.error.as_deref(),
        Some("CLEANUP_FINALIZE_COMMIT_INVALID")
    );
    assert!(root.join("playground/squash-wt").is_dir());

    // The recorded squash commit authorizes the cleanup.
    let finalized = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "squash-held".into(),
        Some(squash.clone()),
    );
    assert!(finalized.ok, "{finalized:?}");
    assert_eq!(finalized.data["cleanup"]["result"], "verified");
    assert!(!root.join("playground/squash-wt").exists());
    assert!(!Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--verify", "refs/heads/feature"])
        .output()
        .unwrap()
        .status
        .success());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn close_accepts_squash_integrated_branch_from_recorded_integration() {
    let (server, root) = test_server();
    squash_merged_holder(&server, &root, "squash-close", true, false);
    let closed = handle_task_close(
        &server,
        "holder".into(),
        "token-holder".into(),
        "squash-close".into(),
        false,
        None,
    );
    assert!(closed.ok, "{closed:?}");
    assert_eq!(closed.data["cleanup"]["result"], "verified");
    assert!(!root.join("playground/squash-wt").exists());
    assert!(!Command::new("git")
        .current_dir(&root)
        .args(["rev-parse", "--verify", "refs/heads/feature"])
        .output()
        .unwrap()
        .status
        .success());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn codex_subagents_exchange_messages() {
    use crate::subagent::{Action, Record};
    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "first-peer", "%first");
    register(&server, "codex-peer", "%codex");
    let now = now_ms();
    let first = bind_test_subagent_record(&server, Record {
        id: "first-rt".into(),
        parent: "parent".into(),
        peer: "first-peer".into(),
        status: "idle".into(),
        thread_id: Some("thread-first".into()),
        profile: None,
        created_ms: now,
        ready_deadline_ms: now + 90_000,
        last_message: None,
        error: None,
        probe_failures: Vec::new(),
        runtime: Some("codex".into()),
        create_operation_id: None,
        binding_id: None,
        endpoint_generation: None,
    });
    let codex = bind_test_subagent_record(&server, Record {
        id: "codex-rt".into(),
        parent: "parent".into(),
        peer: "codex-peer".into(),
        status: "idle".into(),
        thread_id: Some("thread-codex".into()),
        profile: None,
        created_ms: now,
        ready_deadline_ms: now + 90_000,
        last_message: None,
        error: None,
        probe_failures: Vec::new(),
        runtime: Some("codex".into()),
        create_operation_id: None,
        binding_id: None,
        endpoint_generation: None,
    });
    server.commit(&[
        Event::SubagentUpdated {
            subagent: first.clone(),
        },
        Event::SubagentUpdated {
            subagent: codex.clone(),
        },
    ]);
    let to_codex = handle_send(
        &server,
        "first-peer".into(),
        "codex-peer".into(),
        "notify".into(),
        Some("first-to-codex".into()),
        "ping from first runtime".into(),
        None,
        "immediate".into(),
    );
    assert!(to_codex.ok, "{}", to_codex.error.unwrap_or_default());
    let to_first = handle_send(
        &server,
        "codex-peer".into(),
        "first-peer".into(),
        "notify".into(),
        Some("codex-to-first".into()),
        "pong from codex runtime".into(),
        None,
        "immediate".into(),
    );
    assert!(to_first.ok, "{}", to_first.error.unwrap_or_default());
    let to_parent = handle_send(
        &server,
        "first-peer".into(),
        "parent".into(),
        "notify".into(),
        Some("first-result".into()),
        "first finished".into(),
        None,
        "immediate".into(),
    );
    assert!(to_parent.ok, "{}", to_parent.error.unwrap_or_default());
    let from_parent_first = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "first-rt".into(),
            subject: "assign-first".into(),
            body: "task for first".into(),
        },
    );
    assert!(
        from_parent_first.ok,
        "{}",
        from_parent_first.error.unwrap_or_default()
    );
    assert!(
        crate::subagent::handle(
            &server,
            "first-peer",
            "token-first-peer",
            Action::Working {
                id: "first-rt".into()
            }
        )
        .ok
    );
    assert!(
        crate::subagent::handle(
            &server,
            "first-peer",
            "token-first-peer",
            Action::Ready {
                id: "first-rt".into()
            }
        )
        .ok
    );
    let from_parent_codex = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "codex-rt".into(),
            subject: "assign-codex".into(),
            body: "task for codex".into(),
        },
    );
    assert!(
        from_parent_codex.ok,
        "{}",
        from_parent_codex.error.unwrap_or_default()
    );
    let first_status = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Status {
            id: "first-rt".into(),
        },
    );
    let codex_status = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Status {
            id: "codex-rt".into(),
        },
    );
    assert!(first_status.ok);
    assert!(codex_status.ok);
    assert_eq!(first_status.data["subagent"]["runtime"], "codex");
    assert_eq!(codex_status.data["subagent"]["runtime"], "codex");
    assert_eq!(first_status.data["next_check"], "status");
    assert_eq!(first_status.data["progress"], "snapshot");
    assert_eq!(first_status.data["close_required"], false);
    let msgs: Vec<_> = server
        .state
        .lock()
        .unwrap()
        .msgs
        .values()
        .cloned()
        .collect();
    assert!(
        msgs.iter().any(|m| m.from == "first-peer"
            && m.to == "codex-peer"
            && m.subject.as_deref() == Some("first-to-codex")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m.from == "codex-peer"
            && m.to == "first-peer"
            && m.subject.as_deref() == Some("codex-to-first")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter().any(|m| m.from == "first-peer"
            && m.to == "parent"
            && m.subject.as_deref() == Some("first-result")),
        "{msgs:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_status_query_exposes_unknown_liveness_for_a_tmux_only_peer() {
    let (server, root) = test_server();
    let tmux = IsolatedTmux::start(&root);
    register_tmux(&server, "status-worker", tmux.endpoints().remove(0));
    let resp = dispatch(&Arc::new(server), Req::WorkerStatus { worker_id: None });
    assert!(resp.ok);
    let workers = resp.data["workers"].as_array().unwrap();
    assert_eq!(workers.len(), 1);
    let w = &workers[0];
    assert_eq!(w["id"], "status-worker");
    assert_eq!(w["transport"]["kind"], "tmux");
    // The durable binding proves identity, and the pane proves address
    // reachability. Neither fact proves agent activity.
    assert_eq!(w["endpoint_live"], true);
    assert_eq!(w["identity_valid"], true);
    assert_eq!(w["presence"], "present");
    assert_eq!(w["agent_state"], "unknown");
    assert_eq!(w["status"], "unknown");
    assert_eq!(w["transport_view"]["transport"], "tmux");
    assert_eq!(w["unacked_notifications"], 0);
    assert_eq!(w["notifications_paused"], false);
    drop(tmux);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_status_keeps_tmux_liveness_unknown_despite_offline_keepalive_hint() {
    let (server, root) = test_server();
    let tmux = IsolatedTmux::start_single(&root);
    register_tmux(&server, "unresponsive-worker", tmux.endpoints().remove(0));
    let worker = server
        .state
        .lock()
        .unwrap()
        .workers
        .get("unresponsive-worker")
        .unwrap()
        .clone();
    let keepalives = std::collections::HashMap::from([(
        worker.id.clone(),
        crate::server::keepalive::Record {
            suspected_offline: true,
            ..Default::default()
        },
    )]);
    let summary = worker_status_summary_with_maps(
        &server,
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
        &keepalives,
        json!({"role": "peer"}),
        &worker,
    );
    assert_eq!(summary["status"], "unknown");
    assert_eq!(summary["agent_state"], "unknown");
    assert_eq!(summary["diagnostic"], serde_json::Value::Null);
    drop(tmux);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_status_senses_registered_appserver_routes() {
    let (server, root) = test_server();
    server.state.lock().unwrap().workers.insert(
        "status-appserver".into(),
        crate::server::state::WorkerRec {
            id: "status-appserver".into(),
            token: "token-status-appserver".into(),
            cwd: server.root.display().to_string(),
            registered_ms: now_ms(),
            transport: Some(test_appserver_transport("thread-status-appserver")),
        },
    );
    let resp = dispatch(&Arc::new(server), Req::WorkerStatus { worker_id: None });
    assert!(resp.ok);
    let workers = resp.data["workers"].as_array().unwrap();
    assert_eq!(workers.len(), 1);
    let w = &workers[0];
    assert_eq!(w["id"], "status-appserver");
    assert_eq!(w["transport"]["kind"], "appserver");
    assert_eq!(w["transport"]["thread_id"], "thread-status-appserver");
    assert_eq!(w["endpoint_live"], serde_json::Value::Bool(true));
    assert_eq!(w["identity_valid"], serde_json::Value::Bool(true));
    assert_eq!(w["agent_state"], "idle");
    assert_eq!(w["presence"], "present");
    assert_eq!(w["status"], "idle");
    assert_eq!(w["transport_view"]["thread_state"], "idle");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_status_uses_appserver_status_not_candidate_check_for_presence() {
    let (mut server, root) = test_server();
    server.state.lock().unwrap().workers.insert(
        "lost-appserver".into(),
        crate::server::state::WorkerRec {
            id: "lost-appserver".into(),
            token: "token-lost-appserver".into(),
            cwd: server.root.display().to_string(),
            registered_ms: now_ms(),
            transport: Some(test_appserver_transport("thread-lost-appserver")),
        },
    );
    server.appserver_candidate_check =
        Arc::new(|_| panic!("legacy AppServer presence must not probe the AppServer adapter"));
    let resp = dispatch(&Arc::new(server), Req::WorkerStatus { worker_id: None });
    assert!(resp.ok);
    let workers = resp.data["workers"].as_array().unwrap();
    assert_eq!(workers.len(), 1);
    let w = &workers[0];
    assert_eq!(w["transport"]["kind"], "appserver");
    assert_eq!(w["endpoint_live"], serde_json::Value::Bool(true));
    assert_eq!(w["agent_state"], "idle");
    assert_eq!(w["status"], "idle");
    assert_eq!(w["presence"], "present");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bulk_ack_with_empty_ids_acknowledges_all_inbox_messages() {
    let (server, root) = test_server();
    register(&server, "sender-worker", "%test-sender-worker");
    register(&server, "bulk-worker", "%test-bulk-worker");
    let server_arc = Arc::new(server);

    // Send 2 messages to bulk-worker
    for i in 1..=2 {
        dispatch(
            &server_arc,
            Req::Send {
                from: "sender-worker".into(),
                worker_id: Some("sender-worker".into()),
                token: Some("token-sender-worker".into()),
                command: Some(send_command(&root, "sender-worker")),
                to: "bulk-worker".into(),
                mtype: "notify".into(),
                subject: Some(format!("test-{i}")),
                body: format!("body {i}"),
                in_reply_to: None,
                reply_policy: None,
                delivery: "immediate".into(),
            },
        );
    }

    // Deliver them
    let ids: Vec<String> = server_arc
        .state
        .lock()
        .unwrap()
        .msgs
        .values()
        .map(|m| m.id.clone())
        .collect();
    server_arc.commit(&[Event::Delivered { ids: ids.clone() }]);

    // Bulk ack with empty ids
    let resp = dispatch(
        &server_arc,
        Req::Ack {
            worker_id: "bulk-worker".into(),
            token: "token-bulk-worker".into(),
            ids: vec![],
        },
    );
    assert!(resp.ok);
    assert_eq!(resp.data["acked"].as_array().unwrap().len(), 2);

    // Verify all messages are read
    let state = server_arc.state.lock().unwrap();
    assert!(state.msgs.values().all(|m| m.state == "read"));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_master_wake_delivery_marks_accumulator_notified() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user-approved");
    let now = now_ms();
    server.commit(&[
        Event::NotificationSubscribed {
            subscription: crate::server::state::NotificationSubscription {
                id: "sub-master".into(),
                worker_id: "master".into(),
                target: "thread-master".into(),
                event: "worker-idle".into(),
                subject: None,
                method: "appserver".into(),
                status: "armed".into(),
                status_reason: None,
                created_ms: now,
                updated_ms: now,
                fired_count: 0,
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                expires_ms: now + 60_000,
            },
        },
        Event::MasterWakeSignal {
            signal: crate::server::state::MasterWakeSignal::WorkerIdle {
                worker_id: "worker".into(),
            },
            at_ms: now,
        },
        Event::Sent {
            msg: crate::server::state::Message {
                id: "master-wake".into(),
                from: "collab-server".into(),
                to: "master".into(),
                mtype: "notify".into(),
                subject: Some("worker-idle: worker".into()),
                body: "wake".into(),
                in_reply_to: None,
                created_ms: now,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::NotificationSubscribed {
            subscription: crate::server::state::NotificationSubscription {
                id: "sub-master".into(),
                worker_id: "master".into(),
                event: "direct-message".into(),
                subject: None,
                target: "thread-master".into(),
                method: "appserver".into(),
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                fired_count: 0,
                expires_ms: now.saturating_add(300_000),
                status: "armed".into(),
                created_ms: now,
                updated_ms: now,
                status_reason: None,
            },
        },
        Event::WakeBound {
            message_id: "master-wake".into(),
            subscription_id: "sub-master".into(),
        },
    ]);
    assert_eq!(
        server.state.lock().unwrap().master_wake.delivery_state,
        "pending"
    );

    server.commit(&[Event::Delivered {
        ids: vec!["master-wake".into()],
    }]);

    assert_eq!(
        server.state.lock().unwrap().master_wake.delivery_state,
        "notified_unconsumed"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_master_wake_delivery_does_not_depend_on_legacy_master_projection() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user-approved");
    let now = now_ms();
    server.commit(&[
        Event::MasterWakeSignal {
            signal: crate::server::state::MasterWakeSignal::WorkerIdle {
                worker_id: "worker".into(),
            },
            at_ms: now,
        },
        Event::Sent {
            msg: crate::server::state::Message {
                id: "master-wake-typed".into(),
                from: "collab-server".into(),
                to: "master".into(),
                mtype: "notify".into(),
                subject: Some("worker-idle: worker".into()),
                body: "wake".into(),
                in_reply_to: None,
                created_ms: now,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::WakeBound {
            message_id: "master-wake-typed".into(),
            subscription_id: "sub-master".into(),
        },
    ]);
    {
        let mut state = server.state.lock().unwrap();
        state.master_worker_id = None;
        state.notification_subscriptions.insert(
            "sub-master".into(),
            crate::server::state::NotificationSubscription {
                id: "sub-master".into(),
                worker_id: "master".into(),
                target: "thread-master".into(),
                event: "worker-idle".into(),
                subject: None,
                method: "appserver".into(),
                status: "armed".into(),
                status_reason: None,
                created_ms: now,
                updated_ms: now,
                fired_count: 1,
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                expires_ms: now + 60_000,
            },
        );
    }

    server.commit(&[Event::Delivered {
        ids: vec!["master-wake-typed".into()],
    }]);

    assert_eq!(
        server.state.lock().unwrap().master_wake.delivery_state,
        "notified_unconsumed"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn external_or_operator_sender_can_send_without_registration() {
    let (server, root) = test_server();
    register(&server, "recipient-worker", "%recipient");
    let resp = handle_send(
        &server,
        "external-operator".into(),
        "recipient-worker".into(),
        "notify".into(),
        Some("test-topic".into()),
        "hello from outside a registered peer".into(),
        None,
        "immediate".into(),
    );
    assert!(resp.ok);
    assert_eq!(resp.data["durable"].as_bool(), Some(true));
    let msg_id = resp.data["msg_id"].as_str().unwrap();

    let state = server.state.lock().unwrap();
    let msg = state.msgs.get(msg_id).unwrap();
    assert_eq!(msg.from, "external-operator");
    assert_eq!(msg.to, "recipient-worker");
    assert_eq!(msg.body, "hello from outside a registered peer");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_freed_transitions_notify_live_master() {
    let (server, root) = test_server();
    register_synthetic_peer(&server, "master-worker", "thread-master");
    register_synthetic_peer(&server, "task-worker", "thread-worker");
    let server_arc = std::sync::Arc::new(server);
    let promote_resp = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            approval: "approved".into(),
        },
    );
    assert!(promote_resp.ok);

    let now = now_ms();
    server_arc.commit(&[
        Event::SubagentUpdated {
            subagent: subagent_record("managed-worker", "working", "task-worker"),
        },
        Event::KeepaliveUpdated {
            worker_id: "task-worker".into(),
            record: crate::server::keepalive::Record {
                observed: "working".into(),
                idle_since_ms: now - 1,
                ..Default::default()
            },
        },
        Event::TaskCreated {
            task: TaskRec {
                id: "task-worker-release".into(),
                owner: "task-worker".into(),
                created_by: "master-worker".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: default_priority(),
                status: "working".into(),
                next_step: None,
                wait: None,
                created_ms: now,
                updated_ms: now,
            },
        },
    ]);
    crate::server::keepalive::tick_at(&server_arc, now + 1);
    server_arc.commit(&[
        Event::TaskUpdated {
            task: TaskRec {
                id: "task-worker-release".into(),
                owner: "task-worker".into(),
                created_by: "master-worker".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: default_priority(),
                status: "closed".into(),
                next_step: None,
                wait: None,
                created_ms: now,
                updated_ms: now + 2,
            },
        },
        Event::SubagentUpdated {
            subagent: subagent_record("managed-worker", "idle", "task-worker"),
        },
    ]);

    crate::server::keepalive::tick_at(&server_arc, now + 60_002);

    let state = server_arc.state.lock().unwrap();
    let idle_alert = state
        .msgs
        .values()
        .find(|m| m.to == "master-worker" && m.subject == Some("subagent-status".into()));
    assert!(
        idle_alert.is_some(),
        "expected managed subagent-status alert sent to master"
    );
    let alert = idle_alert.unwrap();
    assert!(alert.body.contains("newly_idle=subagent:managed-worker"));
    assert!(alert.body.contains("live_idle=subagent:managed-worker"));
    assert!(alert.body.contains("long-horizon goals"));
    assert!(alert.body.contains("one-shot execution or observation"));
    assert!(alert.body.contains("codex exec --profile gcm"));
    assert!(alert.body.contains("authorized peers"));
    assert!(alert
        .body
        .contains("regardless of whether other peers are busy"));
    assert!(!alert.body.contains("saturate live present peers first"));
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_working_to_idle_leaves_reminder_to_the_master_idle_timer() {
    let (server, root) = test_server();
    register(&server, "master-worker", "thread-master");
    let server_arc = std::sync::Arc::new(server);
    let promote_resp = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            approval: "approved".into(),
        },
    );
    assert!(promote_resp.ok);

    let now = now_ms();
    server_arc.commit(&[
        Event::SubagentUpdated {
            subagent: subagent_record("managed-master", "working", "master-worker"),
        },
        Event::KeepaliveUpdated {
            worker_id: "master-worker".into(),
            record: crate::server::keepalive::Record {
                observed: "working".into(),
                idle_since_ms: now - 1,
                ..Default::default()
            },
        },
        Event::SubagentUpdated {
            subagent: subagent_record("managed-master", "idle", "master-worker"),
        },
    ]);

    crate::server::keepalive::tick_at(&server_arc, now + 1);

    let state = server_arc.state.lock().unwrap();
    assert_eq!(
        state
            .msgs
            .values()
            .filter(|m| m.to == "master-worker")
            .count(),
        0,
        "keepalive must not self-wake a master idle transition; the master-idle timer owns that contract"
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn master_idle_requires_live_master_armed_subscription_and_empty_backlog() {
    let (server, root) = test_server();
    register(&server, "unpromoted", "thread-unpromoted");
    let server_arc = std::sync::Arc::new(server);
    let now = now_ms();
    let initial_rec = crate::server::keepalive::Record {
        observed: "working".into(),
        idle_since_ms: now - 1,
        ..Default::default()
    };
    server_arc.commit(&[
        Event::SubagentUpdated {
            subagent: subagent_record("managed-unpromoted", "working", "unpromoted"),
        },
        Event::KeepaliveUpdated {
            worker_id: "unpromoted".into(),
            record: initial_rec.clone(),
        },
        Event::SubagentUpdated {
            subagent: subagent_record("managed-unpromoted", "idle", "unpromoted"),
        },
    ]);
    crate::server::keepalive::tick_at(&server_arc, now + 1);
    assert!(!server_arc
        .state
        .lock()
        .unwrap()
        .msgs
        .values()
        .any(|m| m.subject == Some("master-idle: unpromoted".into())));

    let promote_resp = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "unpromoted".into(),
            token: "token-unpromoted".into(),
            approval: "approved".into(),
        },
    );
    assert!(promote_resp.ok);
    server_arc.commit(&[
        Event::KeepaliveUpdated {
            worker_id: "unpromoted".into(),
            record: initial_rec.clone(),
        },
        Event::NotificationStatus {
            subscription_id: "sub-default-direct-message-unpromoted".into(),
            status: "consumed".into(),
            updated_ms: 2001,
        },
    ]);
    crate::server::keepalive::tick_at(&server_arc, now + 2);
    assert!(!server_arc
        .state
        .lock()
        .unwrap()
        .msgs
        .values()
        .any(|m| m.subject == Some("master-idle: unpromoted".into())));
    std::fs::remove_dir_all(root).unwrap();

    let (server, root) = test_server();
    register(&server, "busy-master", "thread-master");
    let server_arc = std::sync::Arc::new(server);
    assert!(
        dispatch(
            &server_arc,
            Req::MasterPromote {
                worker_id: "busy-master".into(),
                token: "token-busy-master".into(),
                approval: "approved".into(),
            }
        )
        .ok
    );
    create_task(&server_arc, "busy-master", "task-actionable", "feature");
    server_arc.commit(&[Event::KeepaliveUpdated {
        worker_id: "busy-master".into(),
        record: initial_rec,
    }]);
    crate::server::keepalive::tick_at(&server_arc, now + 3);
    assert!(!server_arc
        .state
        .lock()
        .unwrap()
        .msgs
        .values()
        .any(|m| m.subject == Some("master-idle: busy-master".into())));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_unresponsive_notifies_live_master_to_check_durable_work() {
    let (server, root) = test_server();
    register(&server, "master-worker", "thread-master");
    register(&server, "stuck-worker", "thread-stuck");
    let server_arc = std::sync::Arc::new(server);
    let promote_resp = dispatch(
        &server_arc,
        Req::MasterPromote {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            approval: "approved".into(),
        },
    );
    assert!(promote_resp.ok);

    // Seed the durable notification baseline, independent of identity and
    // current pane activity, to exercise the real offline notification edge.
    server_arc.commit(&[Event::KeepaliveUpdated {
        worker_id: "stuck-worker".into(),
        record: crate::server::keepalive::Record {
            notified_presence: "online".into(),
            ..Default::default()
        },
    }]);

    let baseline = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
    );
    assert!(baseline.ok, "{baseline:?}");
    // The pane is a reachable address but not a runtime, so the peer must not
    // be re-derived as an online agent and no notification may fire.
    assert_eq!(baseline.data["workers"][0]["presence"], "present");
    assert_eq!(baseline.data["workers"][0]["identity_valid"], true);
    assert_eq!(baseline.data["workers"][0]["endpoint_live"], true);
    assert_eq!(baseline.data["workers"][0]["agent_state"], "unknown");
    assert_eq!(
        server_arc.state.lock().unwrap().keepalives["stuck-worker"].notified_presence,
        "online"
    );
    assert!(server_arc.state.lock().unwrap().msgs.values().all(|m| {
        !(m.to == "master-worker" && m.subject == Some("worker-unresponsive: stuck-worker".into()))
    }));

    kill_registered_worker_pane(&server_arc, "stuck-worker");
    let status = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
    );
    assert!(status.ok, "{status:?}");
    assert_eq!(status.data["workers"][0]["presence"], "present");
    assert_eq!(status.data["workers"][0]["identity_valid"], true);
    assert_eq!(status.data["workers"][0]["endpoint_live"], false);
    assert_eq!(status.data["workers"][0]["status"], "lost");

    let state = server_arc.state.lock().unwrap();
    let offline_alerts: Vec<_> = state
        .msgs
        .values()
        .filter(|m| {
            m.to == "master-worker" && m.subject == Some("worker-unresponsive: stuck-worker".into())
        })
        .collect();
    assert_eq!(offline_alerts.len(), 1);
    assert!(offline_alerts[0]
        .body
        .contains("durable tasks with `collab task status`"));
    assert!(offline_alerts[0]
        .body
        .contains("mailbox with `collab inbox`"));
    assert!(!offline_alerts[0].body.contains("snapshot"));
    assert_eq!(
        state.keepalives["stuck-worker"].notified_presence,
        "offline"
    );
    assert!(state
        .master_wake
        .unresponsive_workers
        .contains(&"stuck-worker".into()));
    drop(state);

    for request in [
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
        Req::StatusAll,
        Req::Workers,
    ] {
        let response = dispatch(&server_arc, request);
        assert!(response.ok, "{response:?}");
    }
    let ack = dispatch(
        &server_arc,
        Req::Ack {
            worker_id: "master-worker".into(),
            token: "token-master-worker".into(),
            ids: vec![],
        },
    );
    assert!(ack.ok, "{ack:?}");
    let unchanged = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
    );
    assert!(unchanged.ok, "{unchanged:?}");
    assert_eq!(
        server_arc
            .state
            .lock()
            .unwrap()
            .msgs
            .values()
            .filter(|m| {
                m.to == "master-worker"
                    && m.subject == Some("worker-unresponsive: stuck-worker".into())
            })
            .count(),
        1,
        "unchanged offline status, status-all, workers, and ack must not duplicate"
    );

    // Re-creating the pane restores an address only. The agent runtime stays
    // unknown, so it must not fabricate an online recovery or clear the offline
    // baseline.
    assert!(register(&server_arc, "stuck-worker", "thread-stuck").ok);
    let recreated = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
    );
    assert!(recreated.ok, "{recreated:?}");
    assert_eq!(recreated.data["workers"][0]["presence"], "present");
    assert_eq!(recreated.data["workers"][0]["identity_valid"], true);
    assert_eq!(recreated.data["workers"][0]["endpoint_live"], true);
    assert_eq!(recreated.data["workers"][0]["agent_state"], "unknown");
    let state = server_arc.state.lock().unwrap();
    assert_eq!(
        state
            .msgs
            .values()
            .filter(|m| {
                m.to == "master-worker"
                    && m.subject == Some("worker-recovered: stuck-worker".into())
            })
            .count(),
        0,
        "a recreated pane must not fabricate an online recovery notification"
    );
    assert_eq!(
        state.keepalives["stuck-worker"].notified_presence,
        "offline"
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
