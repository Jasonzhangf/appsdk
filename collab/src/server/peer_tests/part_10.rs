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
    assert!(operations
        .iter()
        .any(|operation| operation["kind"] == "next_action"
            && operation["action"]
                == "Resume the registered task or remain available for an explicit dispatch."));
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
    assert_eq!(context.data["master"]["endpoint_live"], true);
    assert!(context.data["recorded_unusable"].is_null());
    assert_eq!(
        context.data["next_actions"],
        serde_json::json!(["run `appsdk longhorizon show`, saturate live peers first, then schedule managed subagents within the configured cap; do not end the scheduling turn while eligible capacity remains idle"])
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
                && operation["action"]
                    == "Run `appsdk longhorizon show`, saturate live peers first, then schedule managed subagents within the configured cap; do not end the scheduling turn while eligible capacity remains idle. Delivery or review triggers review/integration/cleanup/dispatch, not an endpoint.")
    );
    assert!(context.data["role_brief"]["responsibilities"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("Delivery, merge, or a review verdict is not a lifecycle endpoint; drive review/integration/cleanup/close and assign the next ready P0/P1 task.")));
    assert!(context.data["role_brief"]["responsibilities"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("Before ending each scheduling turn, saturate every live present peer first, then schedule managed subagents within the configured cap; never stay idle while eligible capacity remains.")));
    assert_eq!(
        context.data["role_brief"]["next_action"],
        "Run `appsdk longhorizon show`, saturate live peers first, then schedule managed subagents within the configured cap; do not end the scheduling turn while eligible capacity remains idle. Delivery or review triggers review/integration/cleanup/dispatch, not an endpoint."
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn context_does_not_project_retired_appserver_thread_or_turn_state() {
    let (mut server, root) = test_server();
    let registration = register_appserver(&mut server, "state-peer", "thread-state-peer");
    assert!(
        registration.ok,
        "{}",
        registration.error.unwrap_or_default()
    );
    server.appserver_thread_status =
        Arc::new(|_, _| panic!("context must not call retired AppServer status"));
    let server = Arc::new(server);
    let context = handle_context(&server, "state-peer".into(), "token-state-peer".into());
    assert_eq!(context.data["agent"]["thread_state"], "unknown");
    let status = dispatch(&server, Req::WorkerStatus { worker_id: None });
    assert_eq!(status.data["workers"][0]["agent_state"], "unknown");
    assert_eq!(status.data["workers"][0]["presence"], "present");

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn retired_appserver_status_callback_does_not_override_live_tmux_presence() {
    let (mut server, root) = test_server();
    let registration = register_appserver(&mut server, "timeout-peer", "thread-timeout-peer");
    assert!(
        registration.ok,
        "{}",
        registration.error.unwrap_or_default()
    );
    server.appserver_thread_status =
        Arc::new(|_, _| panic!("status must come from the registered tmux pane"));
    let server = Arc::new(server);

    let context = handle_context(&server, "timeout-peer".into(), "token-timeout-peer".into());
    assert_eq!(context.data["agent"]["thread_state"], "unknown");

    let status = dispatch(&server, Req::WorkerStatus { worker_id: None });
    assert_eq!(status.data["workers"][0]["presence"], "present");
    assert_eq!(status.data["workers"][0]["endpoint_live"], true);
    assert_eq!(status.data["workers"][0]["identity_valid"], true);

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn retired_appserver_route_error_does_not_mark_live_tmux_pane_missing() {
    let (mut server, root) = test_server();
    let registration = register_appserver(&mut server, "missing-peer", "thread-missing-peer");
    assert!(
        registration.ok,
        "{}",
        registration.error.unwrap_or_default()
    );
    server.appserver_thread_status =
        Arc::new(|_, _| panic!("status must come from the registered tmux pane"));
    let server = Arc::new(server);

    let status = dispatch(&server, Req::WorkerStatus { worker_id: None });
    assert_eq!(status.data["workers"][0]["presence"], "present");
    assert_eq!(status.data["workers"][0]["endpoint_live"], true);
    assert_eq!(status.data["workers"][0]["identity_valid"], true);

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn retired_appserver_candidate_check_does_not_override_live_tmux_presence() {
    let (mut server, root) = test_server();
    let registration =
        register_appserver(&mut server, "timeout-identity", "thread-timeout-identity");
    assert!(
        registration.ok,
        "{}",
        registration.error.unwrap_or_default()
    );
    server.appserver_candidate_check =
        Arc::new(|_| panic!("tmux presence must not call the retired AppServer candidate checker"));
    let server = Arc::new(server);

    let status = dispatch(&server, Req::WorkerStatus { worker_id: None });
    assert_eq!(status.data["workers"][0]["presence"], "present");
    assert_eq!(status.data["workers"][0]["endpoint_live"], true);
    assert_eq!(status.data["workers"][0]["identity_valid"], true);

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
        assert!(project_root.join(path.as_str().unwrap()).is_file());
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
    assert!(git(&["init", "-q"]).status.success());
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

    let refused = close_task_resources(&root, &crate::config::Config::default(), Some("playground/wt"), Some("feature"));
    assert!(refused.unwrap_err().contains("not merged"));
    assert!(playground.join("wt").is_dir());

    assert!(git(&["merge", "-q", "feature"]).status.success());
    assert!(close_task_resources(&root, &crate::config::Config::default(), Some("playground/wt"), Some("feature")).is_ok());
    assert!(!playground.join("wt").exists());
    assert!(!git(&["rev-parse", "--verify", "feature"]).status.success());
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
    let first = Record {
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
    };
    let codex = Record {
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
    };
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
fn worker_status_query_exposes_liveness_identity_and_notification_pressure() {
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
    register(&server, "master-worker", "thread-master");
    register(&server, "task-worker", "thread-worker");
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

    let baseline = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
    );
    assert!(baseline.ok, "{baseline:?}");
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

    assert!(register(&server_arc, "stuck-worker", "thread-stuck").ok);
    let recovered = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
    );
    assert!(recovered.ok, "{recovered:?}");
    assert_eq!(recovered.data["workers"][0]["endpoint_live"], true);
    assert_eq!(recovered.data["workers"][0]["agent_state"], "unknown");
    {
        let state = server_arc.state.lock().unwrap();
        let recovered_alerts: Vec<_> = state
            .msgs
            .values()
            .filter(|m| {
                m.to == "master-worker"
                    && m.subject == Some("worker-recovered: stuck-worker".into())
            })
            .collect();
        assert_eq!(recovered_alerts.len(), 1);
        assert_eq!(state.keepalives["stuck-worker"].notified_presence, "online");
        assert!(!state
            .master_wake
            .unresponsive_workers
            .contains(&"stuck-worker".into()));
    }
    let duplicate_recovered = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
    );
    assert!(duplicate_recovered.ok, "{duplicate_recovered:?}");
    assert_eq!(
        server_arc
            .state
            .lock()
            .unwrap()
            .msgs
            .values()
            .filter(|m| {
                m.to == "master-worker"
                    && m.subject == Some("worker-recovered: stuck-worker".into())
            })
            .count(),
        1,
        "unchanged online status must not duplicate recovery"
    );

    kill_registered_worker_pane(&server_arc, "stuck-worker");
    let rearmed = dispatch(
        &server_arc,
        Req::WorkerStatus {
            worker_id: Some("stuck-worker".into()),
        },
    );
    assert!(rearmed.ok, "{rearmed:?}");
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
        2,
        "opposite recovery transition must re-arm the next offline notification"
    );
    std::fs::remove_dir_all(root).unwrap();
}
