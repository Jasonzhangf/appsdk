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
    assert!(
        !operations.iter().any(|operation| {
            matches!(
                operation["kind"].as_str(),
                Some("responsibility" | "next_action" | "completion_action" | "notification_rule")
            )
        }),
        "operations must not mirror static role_brief text: {operations:?}"
    );
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
fn idle_master_keeps_static_guidance_in_role_brief_without_fake_task_actions() {
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
    assert_eq!(context.data["next_actions"], serde_json::json!([]));
    let next_action = context.data["role_brief"]["next_action"]
        .as_str()
        .expect("master next action");
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
        operations
            .iter()
            .any(|operation| operation["kind"] == "peer_lifecycle"),
        "master context must retain the fact-triggered peer lifecycle operation"
    );
    assert!(
        !operations.iter().any(|operation| {
            matches!(
                operation["kind"].as_str(),
                Some("responsibility" | "next_action" | "completion_action" | "notification_rule")
            )
        }),
        "operations must contain current facts, not copied role_brief text: {operations:?}"
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
    assert!(!context
        .data
        .to_string()
        .contains("saturate live present peers first"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn master_context_routes_work_by_task_kind() {
    let (server, root) = test_server();
    register(&server, "master", "%master");
    promote_master(&server, "master", "user-approved");

    let context = handle_context(&server, "master".into(), "token-master".into());

    assert!(context.ok, "{}", context.error.unwrap_or_default());
    assert_eq!(context.data["identity"]["role"], "master");
    assert_eq!(context.data["next_actions"], serde_json::json!([]));
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
            depends_on: Vec::new(),
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
        isolation_mode: None,
        created_ms: now_ms(), updated_ms: now_ms(),
    };
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap();
    let mut state = server.state.lock().unwrap();
    state.apply_checked(&Event::TaskCreated { task: task.clone() }).unwrap();
    state.apply_checked(&Event::BoardDetailsChanged { task_id: task.id.clone(), details:
        crate::board::BoardTaskDetails { public_visibility: true, revision: 1, ..Default::default() }
    }).unwrap();
    assert_eq!(board_task_view(&state, &control, &task).unwrap().unwrap().owner, worker.id);
    let mut other_app = binding.clone();
    other_app.app_scope_id = AppServerId::new("other-app").unwrap();
    other_app.binding_id = BindingId::new("other-app-binding").unwrap();
    other_app.runtime_id = RuntimeId::new("other-app-runtime").unwrap();
    state.apply_checked(&Event::GlobalProjectRegistered {
        registration: global_state::ProjectRegistration::new(other_app.project_scope.clone(), other_app.app_scope_id.clone()).unwrap(),
    }).unwrap();
    state.apply_checked(&Event::GlobalRuntimeBound { binding: other_app.clone() }).unwrap();
    assert_eq!(board_task_view_with_master(&state, &control, &task, None, None, Some(&binding.route_scope())).unwrap().unwrap().owner, worker.id,
        "a same-named binding in another app must not invalidate the requested board owner");
    state.global.projects.get_mut(binding.project_scope.as_str()).unwrap()
        .runtime_bindings.remove(other_app.binding_id.as_str());
    state.global.projects.get_mut(binding.project_scope.as_str()).unwrap()
        .runtime_bindings.remove(binding.binding_id.as_str());
    drop(state);
    assert_eq!(worker_identity_presence(&server, &worker), IdentityPresence::Missing,
        "a foreign-only binding must not prove a local identity");
    let mut state = server.state.lock().unwrap();
    assert_eq!(board_task_view(&state, &control, &task).unwrap().unwrap().owner, "");
    state.apply_checked(&Event::GlobalRuntimeBound { binding: binding.clone() }).unwrap();
    let mut duplicate = binding.clone();
    duplicate.binding_id = BindingId::new("duplicate-local-binding").unwrap();
    duplicate.runtime_id = RuntimeId::new("duplicate-local-runtime").unwrap();
    state.apply_checked(&Event::GlobalRuntimeBound { binding: duplicate }).unwrap();
    assert_eq!(board_task_view(&state, &control, &task).unwrap().unwrap().owner, "");
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
    register(server, "master", "%master");
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
            isolation_mode: None,
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
fn invalid_workspace_close_preserves_same_named_git_branch_and_local_directory() {
    let (server, root) = test_server();
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
    assert!(git(&["branch", "retired-workspace"]).status.success());
    let side_directory = playground.join("retired-workspace");
    std::fs::create_dir_all(&side_directory).unwrap();
    std::fs::write(side_directory.join("user-data.txt"), "preserve\n").unwrap();

    register(&server, "holder", "%holder");
    let now = now_ms();
    let task = TaskRec {
        id: "invalid-close".into(),
        owner: "holder".into(),
        created_by: "holder".into(),
        feature_id: None,
        worktree_path: Some("retired:demo/retired-workspace".into()),
        branch: Some("retired-workspace".into()),
        base_commit: None,
        priority: default_priority(),
        status: "merged".into(),
        next_step: None,
        wait: None,
        isolation_mode: None,
        created_ms: now,
        updated_ms: now,
    };
    server.commit(&[
        Event::TaskCreated { task: task.clone() },
        Event::TaskLifecycleUpdated {
            task_id: "invalid-close".into(),
            record: crate::server::state::TaskLifecycleRecord {
                delivery_kind: crate::server::state::TaskDeliveryKind::ReadOnly,
                delivery_evidence: Some("read-only delivery evidence".into()),
                delivered_ms: Some(now),
                review_evidence: Some("independent review pass".into()),
                reviewer: Some("holder".into()),
                reviewed_ms: Some(now),
                ..Default::default()
            },
        },
        Event::CleanupVerified {
            receipt: CleanupReceipt {
                id: "cleanup-invalid-close".into(),
                task_id: "invalid-close".into(),
                worktree_path: Some("retired:demo/retired-workspace".into()),
                branch: Some("retired-workspace".into()),
                verified_ms: now,
                verification: CleanupVerification::Verified,
                manual_reason: None,
            },
        },
    ]);
    let before_close = server.state.lock().unwrap().tasks["invalid-close"].clone();
    let close = handle_task_close(
        &server,
        "holder".into(),
        "token-holder".into(),
        "invalid-close".into(),
        false,
        None,
    );
    assert!(
        !close.ok,
        "a non-Git workspace reference must not close: {close:?}"
    );
    let state = server.state.lock().unwrap();
    assert_task_record_unchanged(&before_close, &state.tasks["invalid-close"]);
    assert_eq!(
        state.cleanup_receipts["invalid-close"].verification,
        CleanupVerification::Verified
    );
    assert!(side_directory.join("user-data.txt").is_file());
    assert!(git(&["rev-parse", "--verify", "refs/heads/retired-workspace"])
        .status
        .success());
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn invalid_workspace_finalize_keeps_unverified_receipt_and_local_resources() {
    let (server, root) = test_server();
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
    assert!(git(&["branch", "retired-finalize"]).status.success());
    let side_directory = playground.join("retired-finalize");
    std::fs::create_dir_all(&side_directory).unwrap();
    std::fs::write(side_directory.join("user-data.txt"), "preserve\n").unwrap();

    register(&server, "holder", "%holder");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved invalid workspace finalize test");
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "invalid-finalize".into(),
            owner: "holder".into(),
            created_by: "holder".into(),
            feature_id: None,
            worktree_path: Some("retired:demo/retired-finalize".into()),
            branch: Some("retired-finalize".into()),
            base_commit: None,
            priority: default_priority(),
            status: "working".into(),
            next_step: None,
            wait: None,
            isolation_mode: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
    let forced = handle_task_close(
        &server,
        "master".into(),
        "token-master".into(),
        "invalid-finalize".into(),
        true,
        Some("fixture leaves cleanup for finalize".into()),
    );
    assert!(forced.ok, "{forced:?}");
    let before_finalize = server.state.lock().unwrap().tasks["invalid-finalize"].clone();
    let finalized = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "invalid-finalize".into(),
        None,
    );
    assert!(
        !finalized.ok,
        "finalize must not verify an invalid workspace reference: {finalized:?}"
    );
    let state = server.state.lock().unwrap();
    assert_task_record_unchanged(&before_finalize, &state.tasks["invalid-finalize"]);
    assert_eq!(
        state.cleanup_receipts["invalid-finalize"].verification,
        CleanupVerification::Unverified
    );
    assert!(side_directory.join("user-data.txt").is_file());
    assert!(git(&["rev-parse", "--verify", "refs/heads/retired-finalize"])
        .status
        .success());
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn invalid_workspace_close_cannot_reuse_a_verified_receipt_for_a_missing_path() {
    let (server, root) = test_server();
    register(&server, "holder", "%holder");
    let now = now_ms();
    server.commit(&[
        Event::TaskCreated {
            task: TaskRec {
                id: "missing-workspace-close".into(),
                owner: "holder".into(),
                created_by: "holder".into(),
                feature_id: None,
                worktree_path: Some("retired:demo/missing-workspace".into()),
                branch: None,
                base_commit: None,
                priority: default_priority(),
                status: "merged".into(),
                next_step: None,
                wait: None,
                isolation_mode: None,
                created_ms: now,
                updated_ms: now,
            },
        },
        Event::TaskLifecycleUpdated {
            task_id: "missing-workspace-close".into(),
            record: crate::server::state::TaskLifecycleRecord {
                delivery_kind: crate::server::state::TaskDeliveryKind::ReadOnly,
                delivery_evidence: Some("read-only delivery evidence".into()),
                delivered_ms: Some(now),
                review_evidence: Some("independent review pass".into()),
                reviewer: Some("holder".into()),
                reviewed_ms: Some(now),
                ..Default::default()
            },
        },
        Event::CleanupVerified {
            receipt: CleanupReceipt {
                id: "cleanup-missing-workspace-close".into(),
                task_id: "missing-workspace-close".into(),
                worktree_path: Some("retired:demo/missing-workspace".into()),
                branch: None,
                verified_ms: now,
                verification: CleanupVerification::Verified,
                manual_reason: None,
            },
        },
    ]);
    assert!(!root.join("playground/missing-workspace").exists());
    let before = server.state.lock().unwrap().tasks["missing-workspace-close"].clone();
    let closed = handle_task_close(
        &server,
        "holder".into(),
        "token-holder".into(),
        "missing-workspace-close".into(),
        false,
        None,
    );
    assert!(
        !closed.ok,
        "a verified receipt must not bypass workspace validation: {closed:?}"
    );
    let state = server.state.lock().unwrap();
    assert_task_record_unchanged(&before, &state.tasks["missing-workspace-close"]);
    assert_eq!(
        state.cleanup_receipts["missing-workspace-close"].verification,
        CleanupVerification::Verified
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn invalid_workspace_finalize_cannot_reuse_a_verified_receipt_for_a_missing_path() {
    let (server, root) = test_server();
    register(&server, "holder", "%holder");
    register(&server, "master", "%master");
    promote_master(
        &server,
        "master",
        "user approved invalid workspace receipt finalize test",
    );
    let now = now_ms();
    server.commit(&[
        Event::TaskCreated {
            task: TaskRec {
                id: "missing-workspace-finalize".into(),
                owner: "holder".into(),
                created_by: "holder".into(),
                feature_id: None,
                worktree_path: Some("retired:demo/missing-finalize".into()),
                branch: None,
                base_commit: None,
                priority: default_priority(),
                status: "closed".into(),
                next_step: None,
                wait: None,
                isolation_mode: None,
                created_ms: now,
                updated_ms: now,
            },
        },
        Event::CleanupVerified {
            receipt: CleanupReceipt {
                id: "cleanup-missing-workspace-finalize".into(),
                task_id: "missing-workspace-finalize".into(),
                worktree_path: Some("retired:demo/missing-finalize".into()),
                branch: None,
                verified_ms: now,
                verification: CleanupVerification::Verified,
                manual_reason: None,
            },
        },
    ]);
    assert!(!root.join("playground/missing-finalize").exists());
    let before = server.state.lock().unwrap().tasks["missing-workspace-finalize"].clone();
    let finalized = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "missing-workspace-finalize".into(),
        None,
    );
    assert!(
        !finalized.ok,
        "a verified receipt must not bypass workspace validation in finalize: {finalized:?}"
    );
    let state = server.state.lock().unwrap();
    assert_task_record_unchanged(&before, &state.tasks["missing-workspace-finalize"]);
    assert_eq!(
        state.cleanup_receipts["missing-workspace-finalize"].verification,
        CleanupVerification::Verified
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn invalid_workspace_handoff_mention_delivers_without_claiming_a_worktree() {
    let (server, root) = test_server();
    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "retired-handoff-task".into(),
            owner: "handoff-owner".into(),
            created_by: "handoff-owner".into(),
            feature_id: None,
            worktree_path: Some("retired:demo/retired-handoff".into()),
            branch: Some("retired-handoff".into()),
            base_commit: None,
            priority: default_priority(),
            status: "working".into(),
            next_step: None,
            wait: None,
            isolation_mode: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        },
    }]);
    assert!(!root.join("retired:demo/retired-handoff").exists());
    let prose = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("retired workspace prose mention".into()),
        "see retired:demo/retired-handoff for context".into(),
        None,
        "immediate".into(),
    );
    assert!(prose.ok, "{prose:?}");
    let explicit = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("retired workspace explicit mention".into()),
        "preflight implementation lives at ./retired:demo/retired-handoff".into(),
        None,
        "immediate".into(),
    );
    assert!(
        explicit.ok,
        "a retired scheme is prose text, not a project worktree claim: {explicit:?}"
    );
    assert_eq!(explicit.data["durable"], true);
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

fn assert_task_record_unchanged(before: &TaskRec, after: &TaskRec) {
    assert_eq!(before.id, after.id);
    assert_eq!(before.owner, after.owner);
    assert_eq!(before.created_by, after.created_by);
    assert_eq!(before.feature_id, after.feature_id);
    assert_eq!(before.worktree_path, after.worktree_path);
    assert_eq!(before.branch, after.branch);
    assert_eq!(before.base_commit, after.base_commit);
    assert_eq!(before.priority, after.priority);
    assert_eq!(before.status, after.status);
    assert_eq!(before.next_step, after.next_step);
    assert_eq!(before.isolation_mode, after.isolation_mode);
    assert_eq!(before.created_ms, after.created_ms);
    assert_eq!(before.updated_ms, after.updated_ms);
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
        &binding_store::BindingControlStore::new(server.host_paths.state_root()).load().unwrap(),
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
        &keepalives,
        json!({"role": "peer"}),
        &worker,
    ).unwrap();
    assert_eq!(summary["status"], "unknown");
    assert_eq!(summary["agent_state"], "unknown");
    assert_eq!(summary["diagnostic"], serde_json::Value::Null);
    drop(tmux);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn worker_status_senses_registered_appserver_routes() {
    let (server, root) = test_server();
    register_appserver_worker(&server, &root, "status-appserver", "thread-status-appserver");
    let resp = dispatch(&Arc::new(server), Req::WorkerStatus { worker_id: None });
    assert!(resp.ok, "{resp:?}");
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
    register_appserver_worker(&server, &root, "lost-appserver", "thread-lost-appserver");
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
                target_session: None,
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
                patrol_idle_since_ms: None,
                patrol_last_push_ms: 0,
                periodic_declared: false,
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
                patrol_idle_since_ms: None,
                patrol_last_push_ms: 0,
                periodic_declared: false,
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
                patrol_idle_since_ms: None,
                patrol_last_push_ms: 0,
                periodic_declared: false,
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
    register(&server, "master-worker", "thread-master");
    register_synthetic_peer(&server, "task-worker", "thread-worker");
    let server_arc = std::sync::Arc::new(server);
    promote_master(&server_arc, "master-worker", "approved");
    let scope = server_arc
        .root
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    server_arc.commit(&[Event::AgentPresenceObserved {
        presence: crate::server::name_state::AgentPresenceRecord {
            project_scope: scope,
            worker_id: "master-worker".into(),
            session_id: "session-thread-master".into(),
            presence: crate::server::name_state::AgentPresence::Online,
            reason: "fixture".into(),
            observed_ms: 1,
        },
    }]);

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
                isolation_mode: None,
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
                isolation_mode: None,
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
    promote_master(&server_arc, "master-worker", "approved");

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

    promote_master(&server_arc, "unpromoted", "approved");
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
    promote_master(&server_arc, "busy-master", "approved");
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
    promote_master(&server_arc, "master-worker", "approved");

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

#[test]
fn colon_prefixed_relative_alias_worktree_is_a_valid_local_git_path() {
    let (mut server, root) = test_server();
    let base = root.with_file_name(format!(
        "{}-colon-external",
        root.file_name().unwrap().to_string_lossy()
    ));
    let project_key = crate::server::configured_project_key(&root).unwrap();
    let project_dir = base.join(&project_key);
    let direct_target = project_dir.join("colon-direct");
    let dotted_target = project_dir.join("colon-dotted");
    std::fs::create_dir_all(&project_dir).unwrap();
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
    assert!(git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "colon-direct",
        direct_target.to_str().unwrap(),
    ])
    .status
    .success());
    assert!(git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "colon-dotted",
        dotted_target.to_str().unwrap(),
    ])
    .status
    .success());

    // A project-root directory alias can point at the configured external
    // worktree base. The colon is part of a legal POSIX directory name, not a
    // URI scheme.
    let alias = root.join("a:b");
    std::os::unix::fs::symlink(&base, &alias).unwrap();
    server.config.worktree.base = Some(base.canonicalize().unwrap());

    let direct = format!(
        "a:b/{project_key}/{}",
        direct_target.file_name().unwrap().to_string_lossy()
    );
    let dotted = format!(
        "./a:b/{project_key}/{}",
        dotted_target.file_name().unwrap().to_string_lossy()
    );
    let direct_expected = direct_target.canonicalize().unwrap();
    let dotted_expected = dotted_target.canonicalize().unwrap();

    // The old path owner validated only the final slug and then resolved the
    // relative path. Prove both spellings resolve to the configured target
    // before exercising the public registration handler.
    assert!(validate_worktree_slug(&direct).is_ok());
    assert!(validate_worktree_slug(&dotted).is_ok());
    assert_eq!(
        resolve_worktree_path(&root, &server.config, &direct, false).unwrap(),
        direct_expected
    );
    assert_eq!(
        resolve_worktree_path(&root, &server.config, &dotted, false).unwrap(),
        dotted_expected
    );

    register(&server, "owner", "%owner");
    let direct_registered = handle_task_register(
        &server,
        "owner".into(),
        "token-owner".into(),
        "colon-direct-task".into(),
        None,
        Some("colon-direct".into()),
        Some(direct),
        Some("colon-direct".into()),
        None,
        default_priority(),
    );
    assert!(
        direct_registered.ok,
        "a colon-prefixed relative alias must be a valid local worktree: {direct_registered:?}"
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["colon-direct-task"]
            .worktree_path
            .as_deref(),
        Some(direct_expected.to_string_lossy().as_ref())
    );

    let dotted_registered = handle_task_register(
        &server,
        "owner".into(),
        "token-owner".into(),
        "colon-dotted-task".into(),
        None,
        Some("colon-dotted".into()),
        Some(dotted),
        Some("colon-dotted".into()),
        None,
        default_priority(),
    );
    assert!(
        dotted_registered.ok,
        "the equivalent ./ spelling must remain valid: {dotted_registered:?}"
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["colon-dotted-task"]
            .worktree_path
            .as_deref(),
        Some(dotted_expected.to_string_lossy().as_ref())
    );

    std::fs::remove_dir_all(base).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn scheme_shaped_worktree_registration_fails_without_deleting_local_data() {
    let (server, root) = test_server();
    let side_directory = root.join("playground/retired-workspace");
    std::fs::create_dir_all(&side_directory).unwrap();
    std::fs::write(side_directory.join("user-data.txt"), "preserve\n").unwrap();
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
    assert!(git(&["branch", "retired-workspace"]).status.success());

    register(&server, "owner", "%owner");
    let response = handle_task_register(
        &server,
        "owner".into(),
        "token-owner".into(),
        "retired-task".into(),
        None,
        Some("retired-feature".into()),
        Some("retired:demo/retired-workspace".into()),
        Some("retired-workspace".into()),
        None,
        default_priority(),
    );
    assert!(!response.ok, "{response:?}");
    assert_eq!(
        response.error.as_deref(),
        Some("new worktree requires a configured external worktree base")
    );
    assert!(!server
        .state
        .lock()
        .unwrap()
        .tasks
        .contains_key("retired-task"));
    assert!(side_directory.join("user-data.txt").is_file());
    assert!(git(&["rev-parse", "--verify", "refs/heads/retired-workspace"])
        .status
        .success());

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn local_colon_worktree_reference_keeps_the_real_contract_error() {
    let (mut server, root) = test_server();

    // A colon in the first component of a POSIX path is legal. This reference
    // is a real local directory, not a URI scheme, so its validation error is
    // a real contract error and must not be rewritten as a scheme rejection.
    let local_task = root.join("retired:demo/red-task");
    std::fs::create_dir_all(&local_task).unwrap();
    std::fs::write(local_task.join("user-data.txt"), "preserve\n").unwrap();

    register(&server, "holder", "%holder");
    let bindings_before = server.state.lock().unwrap().worktree_bindings.len();

    // Missing configured base: the real error from validate_worktree_path
    // must survive declare_task_workspace instead of being replaced.
    let missing_base = handle_task_register(
        &server,
        "holder".into(),
        "token-holder".into(),
        "colon-missing-base-task".into(),
        None,
        Some("colon-missing-base".into()),
        Some("retired:demo/red-task".into()),
        Some("red-task".into()),
        None,
        default_priority(),
    );
    assert!(!missing_base.ok, "{missing_base:?}");
    assert_eq!(
        missing_base.error.as_deref(),
        Some("new worktree requires a configured external worktree base")
    );

    // Layout mismatch against a configured external base keeps its own error.
    let base = root.with_file_name(format!(
        "{}-colon-error",
        root.file_name().unwrap().to_string_lossy()
    ));
    std::fs::create_dir_all(&base).unwrap();
    server.config.worktree.base = Some(base.canonicalize().unwrap());
    let layout_mismatch = handle_task_register(
        &server,
        "holder".into(),
        "token-holder".into(),
        "colon-layout-mismatch-task".into(),
        None,
        Some("colon-layout-mismatch".into()),
        Some("retired:demo/red-task".into()),
        Some("red-task".into()),
        None,
        default_priority(),
    );
    assert!(!layout_mismatch.ok, "{layout_mismatch:?}");
    assert_eq!(
        layout_mismatch.error.as_deref(),
        Some("worktree path must match the configured worktree base/layout")
    );

    // A rejected declaration must not create a task or change a binding.
    let state = server.state.lock().unwrap();
    assert!(!state.tasks.contains_key("colon-missing-base-task"));
    assert!(!state.tasks.contains_key("colon-layout-mismatch-task"));
    assert_eq!(state.worktree_bindings.len(), bindings_before);
    drop(state);

    // The real local colon path and its data stay untouched.
    assert!(local_task.join("user-data.txt").is_file());

    std::fs::remove_dir_all(base).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn historical_colon_alias_does_not_authorize_close_or_finalize_on_a_real_worktree() {
    let (mut server, root) = test_server();
    let base = root.with_file_name(format!(
        "{}-colon-history",
        root.file_name().unwrap().to_string_lossy()
    ));
    let project_key = crate::server::configured_project_key(&root).unwrap();
    let project_dir = base.join(&project_key);
    std::fs::create_dir_all(&project_dir).unwrap();
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

    let close_target = project_dir.join("history-close");
    assert!(git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "history-close",
        close_target.to_str().unwrap(),
    ])
    .status
    .success());
    let close_user_file = close_target.join("user-data.txt");
    std::fs::write(&close_user_file, "preserve close\n").unwrap();
    assert!(Command::new("git")
        .current_dir(&close_target)
        .args(["add", "user-data.txt"])
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new("git")
        .current_dir(&close_target)
        .args(["commit", "-q", "-m", "close user file"])
        .output()
        .unwrap()
        .status
        .success());
    assert!(git(&["merge", "-q", "--no-edit", "history-close"])
        .status
        .success());

    let finalize_target = project_dir.join("history-finalize");
    assert!(git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "history-finalize",
        finalize_target.to_str().unwrap(),
    ])
    .status
    .success());
    let finalize_user_file = finalize_target.join("user-data.txt");
    std::fs::write(&finalize_user_file, "preserve finalize\n").unwrap();
    assert!(Command::new("git")
        .current_dir(&finalize_target)
        .args(["add", "user-data.txt"])
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new("git")
        .current_dir(&finalize_target)
        .args(["commit", "-q", "-m", "finalize user file"])
        .output()
        .unwrap()
        .status
        .success());
    assert!(git(&["merge", "-q", "--no-edit", "history-finalize"])
        .status
        .success());

    for target in [&close_target, &finalize_target] {
        let status = Command::new("git")
            .arg("-C")
            .arg(target)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(status.status.success());
        assert!(
            status.stdout.is_empty(),
            "fixture worktree must be clean so cleanup would otherwise be allowed: {}",
            String::from_utf8_lossy(&status.stdout)
        );
    }

    let alias = root.join("retired:demo");
    std::os::unix::fs::symlink(base.canonicalize().unwrap(), &alias).unwrap();
    server.config.worktree.base = Some(base.canonicalize().unwrap());

    let close_raw = format!("retired:demo/{project_key}/history-close");
    let finalize_raw = format!("retired:demo/{project_key}/history-finalize");
    let close_canonical = close_target.canonicalize().unwrap();
    let finalize_canonical = finalize_target.canonicalize().unwrap();

    // The colon-prefixed alias and its ./ spelling reach the same real Git
    // storage through the existing path owner. That equivalence is not
    // ownership proof for a historical string stored in a task or receipt.
    assert_eq!(
        resolve_worktree_path(&server.root, &server.config, &close_raw, true).unwrap(),
        close_canonical
    );
    assert_eq!(
        resolve_worktree_path(
            &server.root,
            &server.config,
            &format!("./{close_raw}"),
            true,
        )
        .unwrap(),
        close_canonical
    );
    assert_eq!(
        resolve_worktree_path(&server.root, &server.config, &finalize_raw, true).unwrap(),
        finalize_canonical
    );
    assert_eq!(
        resolve_worktree_path(
            &server.root,
            &server.config,
            &format!("./{finalize_raw}"),
            true,
        )
        .unwrap(),
        finalize_canonical
    );
    let declared_close = declare_task_workspace(
        &server.root,
        &server.config,
        &format!("./{close_raw}"),
    )
    .unwrap();
    assert_eq!(PathBuf::from(&declared_close), close_canonical);
    assert_ne!(&declared_close, &close_raw);

    register(&server, "holder", "%holder");
    register(&server, "master", "%master");
    promote_master(
        &server,
        "master",
        "user approved historical colon alias finalize safety test",
    );

    let now = now_ms();
    let close_binding = WorktreeBinding {
        worktree_root: close_raw.clone(),
        owning_project_scope: server.root.display().to_string(),
        task_id: "historical-colon-close".into(),
        owner_agent_id: "holder".into(),
        binding_id: "binding-historical-colon-close".into(),
        base_commit: String::new(),
        isolation_mode: Some(crate::server::ISOLATION_WORKTREE.into()),
    };
    server.commit(&[
        Event::TaskCreated {
            task: TaskRec {
                id: "historical-colon-close".into(),
                owner: "holder".into(),
                created_by: "holder".into(),
                feature_id: None,
                worktree_path: Some(close_raw.clone()),
                branch: Some("history-close".into()),
                base_commit: None,
                priority: default_priority(),
                status: "merged".into(),
                next_step: None,
                wait: None,
                isolation_mode: Some(crate::server::ISOLATION_WORKTREE.into()),
                created_ms: now,
                updated_ms: now,
            },
        },
        Event::WorktreeBound {
            binding: close_binding.clone(),
        },
        Event::TaskLifecycleUpdated {
            task_id: "historical-colon-close".into(),
            record: crate::server::state::TaskLifecycleRecord {
                delivery_kind: crate::server::state::TaskDeliveryKind::ReadOnly,
                delivery_evidence: Some("read-only delivery evidence".into()),
                delivered_ms: Some(now),
                review_evidence: Some("independent review pass".into()),
                reviewer: Some("holder".into()),
                reviewed_ms: Some(now),
                ..Default::default()
            },
        },
        Event::CleanupVerified {
            receipt: CleanupReceipt {
                id: "cleanup-historical-colon-close".into(),
                task_id: "historical-colon-close".into(),
                worktree_path: Some(close_raw.clone()),
                branch: Some("history-close".into()),
                verified_ms: now,
                verification: CleanupVerification::Verified,
                manual_reason: None,
            },
        },
    ]);

    let before_close = server
        .state
        .lock()
        .unwrap()
        .tasks["historical-colon-close"]
        .clone();
    let closed = handle_task_close(
        &server,
        "holder".into(),
        "token-holder".into(),
        "historical-colon-close".into(),
        false,
        None,
    );
    assert!(
        !closed.ok,
        "historical colon alias with a real worktree and verified receipt must not close: {closed:?}"
    );
    let state = server.state.lock().unwrap();
    assert_task_record_unchanged(&before_close, &state.tasks["historical-colon-close"]);
    assert_eq!(
        state.worktree_bindings["binding-historical-colon-close"],
        close_binding
    );
    assert_eq!(
        state.cleanup_receipts["historical-colon-close"].verification,
        CleanupVerification::Verified
    );
    drop(state);
    assert!(close_user_file.is_file());
    assert!(git(&["rev-parse", "--verify", "refs/heads/history-close"])
        .status
        .success());

    let finalize_binding = WorktreeBinding {
        worktree_root: finalize_raw.clone(),
        owning_project_scope: server.root.display().to_string(),
        task_id: "historical-colon-finalize".into(),
        owner_agent_id: "holder".into(),
        binding_id: "binding-historical-colon-finalize".into(),
        base_commit: String::new(),
        isolation_mode: Some(crate::server::ISOLATION_WORKTREE.into()),
    };
    server.commit(&[
        Event::TaskCreated {
            task: TaskRec {
                id: "historical-colon-finalize".into(),
                owner: "holder".into(),
                created_by: "holder".into(),
                feature_id: None,
                worktree_path: Some(finalize_raw.clone()),
                branch: Some("history-finalize".into()),
                base_commit: None,
                priority: default_priority(),
                status: "closed".into(),
                next_step: None,
                wait: None,
                isolation_mode: Some(crate::server::ISOLATION_WORKTREE.into()),
                created_ms: now,
                updated_ms: now,
            },
        },
        Event::WorktreeBound {
            binding: finalize_binding.clone(),
        },
        Event::CleanupVerified {
            receipt: CleanupReceipt {
                id: "cleanup-historical-colon-finalize".into(),
                task_id: "historical-colon-finalize".into(),
                worktree_path: Some(finalize_raw.clone()),
                branch: Some("history-finalize".into()),
                verified_ms: now,
                verification: CleanupVerification::Verified,
                manual_reason: None,
            },
        },
    ]);
    let before_finalize = server
        .state
        .lock()
        .unwrap()
        .tasks["historical-colon-finalize"]
        .clone();
    let finalized = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "historical-colon-finalize".into(),
        None,
    );
    assert!(
        !finalized.ok,
        "historical colon alias with a real worktree and verified receipt must not finalize: {finalized:?}"
    );
    let state = server.state.lock().unwrap();
    assert_task_record_unchanged(
        &before_finalize,
        &state.tasks["historical-colon-finalize"],
    );
    assert_eq!(
        state.worktree_bindings["binding-historical-colon-finalize"],
        finalize_binding
    );
    assert_eq!(
        state.cleanup_receipts["historical-colon-finalize"].verification,
        CleanupVerification::Verified
    );
    drop(state);
    assert!(finalize_user_file.is_file());
    assert!(git(&["rev-parse", "--verify", "refs/heads/history-finalize"])
        .status
        .success());
    assert!(alias.symlink_metadata().is_ok());

    std::fs::remove_dir_all(base).ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn handoff_bare_colon_alias_matches_registered_worktree_without_claiming_prose() {
    let (mut server, root) = test_server();
    let base = root.with_file_name(format!(
        "{}-colon-handoff-external",
        root.file_name().unwrap().to_string_lossy()
    ));
    let project_key = crate::server::configured_project_key(&root).unwrap();
    let project_dir = base.join(&project_key);
    std::fs::create_dir_all(&project_dir).unwrap();
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

    let worktree = project_dir.join("handoff-colon");
    assert!(git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "handoff-colon",
        worktree.to_str().unwrap(),
    ])
    .status
    .success());
    let canonical_worktree = worktree.canonicalize().unwrap();
    server.config.worktree.base = Some(base.canonicalize().unwrap());
    let alias = root.join("handoff:alias");
    std::os::unix::fs::symlink(&base, &alias).unwrap();
    let alias_ref = format!("handoff:alias/{project_key}/handoff-colon");
    assert_eq!(
        resolve_worktree_path(&server.root, &server.config, &alias_ref, true).unwrap(),
        canonical_worktree
    );

    register(&server, "handoff-sender", "%handoff-sender");
    register(&server, "handoff-owner", "%handoff-owner");
    let registered = handle_task_register(
        &server,
        "handoff-owner".into(),
        "token-handoff-owner".into(),
        "handoff-colon-task".into(),
        None,
        Some("handoff-colon".into()),
        Some(canonical_worktree.display().to_string()),
        Some("handoff-colon".into()),
        None,
        default_priority(),
    );
    assert!(registered.ok, "{registered:?}");
    assert_eq!(
        server.state.lock().unwrap().tasks["handoff-colon-task"]
            .worktree_path
            .as_deref(),
        Some(canonical_worktree.to_string_lossy().as_ref())
    );
    let task_count = server.state.lock().unwrap().tasks.len();

    let url_body = "reference https://example.com/docs/handoff-colon for background";
    let url = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("ordinary URL".into()),
        url_body.into(),
        None,
        "immediate".into(),
    );
    assert!(url.ok, "{url:?}");
    let url_id = url.data["msg_id"].as_str().unwrap().to_owned();
    assert_eq!(
        server.state.lock().unwrap().msgs[&url_id].body,
        url_body
    );

    let historical_body = "see retired:demo/handoff-missing for context";
    let historical = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("historical ambiguous token".into()),
        historical_body.into(),
        None,
        "immediate".into(),
    );
    assert!(historical.ok, "{historical:?}");
    let historical_id = historical.data["msg_id"].as_str().unwrap().to_owned();
    assert_eq!(
        server.state.lock().unwrap().msgs[&historical_id].body,
        historical_body
    );
    assert_eq!(server.state.lock().unwrap().tasks.len(), task_count);

    let valid_alias = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("valid alias".into()),
        format!("worktree {alias_ref}"),
        None,
        "immediate".into(),
    );
    assert!(valid_alias.ok, "{valid_alias:?}");

    std::fs::remove_dir_all(&worktree).unwrap();
    let message_count = server.state.lock().unwrap().msgs.len();
    let missing_alias = handle_send(
        &server,
        "handoff-sender".into(),
        "handoff-owner".into(),
        "notify".into(),
        Some("missing alias target".into()),
        format!("worktree {alias_ref}"),
        None,
        "immediate".into(),
    );
    assert!(
        !missing_alias.ok,
        "a registered canonical worktree alias must be matched even when its target is missing: {missing_alias:?}"
    );
    assert_eq!(missing_alias.data["error_code"], "HANDOFF_TARGET_UNRESOLVED");
    assert_eq!(missing_alias.data["reason"], "worktree_path_missing");
    assert_eq!(missing_alias.data["recorded"], false);
    assert_eq!(server.state.lock().unwrap().msgs.len(), message_count);

    std::fs::remove_dir_all(base).ok();
    std::fs::remove_dir_all(root).ok();
}
