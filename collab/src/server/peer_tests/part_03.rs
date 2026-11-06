#[test]
fn typed_dispatch_rejects_stale_revision_without_mutation() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();
    let first = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    server.typed_dispatch(first).unwrap();
    let mut stale = server
        .typed_register_envelope("worker-2", "token-worker-2", "%worker-2", cwd)
        .unwrap();
    stale.envelope.command_id = crate::identity::CommandId::new("register-stale").unwrap();
    stale.envelope.operation_id = crate::identity::OperationId::new("register-stale-op").unwrap();
    stale.envelope.expected_revision = Some(0);
    let before = std::fs::read(&server.state_path).unwrap();
    let error = server.typed_dispatch(stale).unwrap_err();
    assert!(error
        .to_string()
        .contains("CAS_MISMATCH:"), "{error}");
    assert_eq!(std::fs::read(&server.state_path).unwrap(), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_dispatch_rejects_wrong_principal_and_scope() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();
    let before = std::fs::read(&server.state_path).unwrap();
    let mut wrong_principal = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    if let crate::server::state::TypedCommand::RegisterWorker { binding, .. } =
        &mut wrong_principal.command
    {
        binding.agent_id = crate::identity::AgentId::new("other-worker").unwrap();
    }
    let principal_error = server.typed_dispatch(wrong_principal).unwrap_err();
    assert!(principal_error
        .to_string()
        .contains("runtime binding agent does not match worker identity"));

    let mut wrong_scope = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    wrong_scope.envelope.scope.project_scope_id =
        crate::scope::ProjectScopeId::new("/other-project").unwrap();
    let scope_error = server.typed_dispatch(wrong_scope).unwrap_err();
    assert!(scope_error
        .to_string()
        .contains("envelope scope does not match binding route scope"));
    assert_eq!(std::fs::read(&server.state_path).unwrap(), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_token_rotation_rejects_dual_key_session_mismatch() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();
    let project_scope = crate::server::global_state::GlobalState::canonical_project_scope(
        std::path::Path::new(cwd),
    )
    .unwrap();
    let app_scope = crate::identity::AppServerId::new("tui-default").unwrap();
    let shared_thread = "thread-dual-key-worker";
    let transport_first = SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some(format!("session-{shared_thread}")),
        thread_id: Some(shared_thread.into()),
        tmux_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    };
    let first = server
        .typed_register_envelope_for_scope(
            "dual-key-worker",
            "old-token-dual-key-worker",
            &transport_first,
            project_scope.clone(),
            cwd,
            app_scope.clone(),
            false,
        )
        .unwrap();
    server
        .typed_dispatch(first)
        .expect("initial typed register must succeed");
    let before_state = std::fs::read(&server.state_path).unwrap();
    let before_global = server.state.lock().unwrap().global.clone();

    let other_session = "session-dual-key-worker-other";
    let transport_second = SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
        namespace: Some("codex_tui".into()),
        session_id: Some(other_session.into()),
        thread_id: Some(shared_thread.into()),
        tmux_endpoint: None,
        capabilities: vec!["send_message_to_thread".into()],
        self_check: "test appserver".into(),
    };
    let mut second = server
        .typed_register_envelope_for_scope(
            "dual-key-worker",
            "new-token-dual-key-worker",
            &transport_second,
            project_scope,
            cwd,
            app_scope,
            false,
        )
        .unwrap();
    if let crate::server::state::TypedCommand::RegisterWorker {
        binding, worker, ..
    } = &mut second.command
    {
        binding.session_id = SessionId::new(other_session).ok();
        if let Some(transport) = worker.transport.as_mut() {
            transport.session_id = Some(other_session.into());
        }
    }

    let error = server.typed_dispatch(second).expect_err(
        "a same-thread register with a different session must be rejected as a dual-key mismatch",
    );
    let error_message = error.to_string();
    assert!(
        error_message.contains("worker token does not belong to the registered runtime identity"),
        "unexpected error: {error_message}"
    );
    assert_eq!(
        std::fs::read(&server.state_path).unwrap(), before_state,
        "a rejected dual-key mismatch must not change current state"
    );
    assert_eq!(
        server.state.lock().unwrap().global,
        before_global,
        "a rejected dual-key mismatch must not mutate the global reducer"
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn subagent_record(id: &str, status: &str, peer: &str) -> crate::subagent::Record {
    crate::subagent::Record {
        id: id.into(),
        parent: "parent".into(),
        peer: peer.into(),
        status: status.into(),
        thread_id: Some(format!("thread-{peer}")),
        profile: None,
        created_ms: now_ms(),
        ready_deadline_ms: now_ms() + 90_000,
        last_message: None,
        error: None,
        probe_failures: Vec::new(),
        runtime: Some("codex".into()),
        create_operation_id: None,
        binding_id: None,
    }
}

fn subagent_req(command: crate::subagent::Action) -> Req {
    Req::Subagent {
        worker_id: "parent".into(),
        token: "token-parent".into(),
        command,
        launch_env: Default::default(),
    }
}

#[test]
fn missing_subagent_retires_without_snapshot_when_responsibilities_are_resolved() {
    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    server.commit(&[Event::SubagentUpdated {
        subagent: subagent_record("missing-retire", "idle", "child"),
    }]);
    kill_registered_worker_pane(&server, "child");
    let child = server.state.lock().unwrap().workers["child"].clone();
    assert_eq!(
        worker_identity_and_endpoint(&server, &child),
        (true, Some(false)),
        "the committed identity remains valid while the owned address is definitively missing"
    );
    let server = Arc::new(server);
    let response = dispatch(
        &server,
        subagent_req(crate::subagent::Action::Close {
            id: "missing-retire".into(),
        }),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.data["subagent"]["status"], "closed");
    assert!(response.data["snapshot_captured_ms"].is_null());
    assert_eq!(
        server.state.lock().unwrap().subagents["missing-retire"].status,
        "closed"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_tmux_subagent_still_requires_a_snapshot_before_close() {
    let (mut server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    server.commit(&[Event::SubagentUpdated {
        subagent: subagent_record("cold-retire", "idle", "child"),
    }]);
    server.appserver_thread_status = Arc::new(|_, thread_id| {
        Ok(serde_json::json!({
            "thread": {
                "id": thread_id,
                "status": {"type": "idle"},
                "canAcceptDirectInput": true,
                "turns": [{"status": "completed"}]
            },
            "thread_state": "idle",
            "transport": "tmux"
        }))
    });
    let child = server.state.lock().unwrap().workers["child"].clone();
    assert_eq!(
        worker_identity_and_endpoint(&server, &child),
        (true, Some(true)),
        "the reachable address and committed identity do not provide the required snapshot"
    );
    let server = Arc::new(server);
    let response = dispatch(
        &server,
        subagent_req(crate::subagent::Action::Close {
            id: "cold-retire".into(),
        }),
    );
    assert!(!response.ok, "{response:?}");
    assert!(response
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("requires a successful snapshot"));
    assert_eq!(
        server.state.lock().unwrap().subagents["cold-retire"].status,
        "idle"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_subagent_with_unresolved_responsibility_still_requires_a_snapshot() {
    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    server.commit(&[
        Event::SubagentUpdated {
            subagent: subagent_record("missing-busy", "idle", "child"),
        },
        Event::TaskCreated {
            task: TaskRec {
                id: "task-missing-busy".into(),
                owner: "child".into(),
                created_by: "parent".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: "p1".into(),
                status: "working".into(),
                next_step: None,
                wait: None,
                isolation_mode: None,
                created_ms: now_ms(),
                updated_ms: now_ms(),
            },
        },
    ]);
    kill_registered_worker_pane(&server, "child");
    let child = server.state.lock().unwrap().workers["child"].clone();
    assert_eq!(worker_identity_and_endpoint(&server, &child), (true, Some(false)));
    let server = Arc::new(server);
    let response = dispatch(
        &server,
        subagent_req(crate::subagent::Action::Close {
            id: "missing-busy".into(),
        }),
    );
    assert!(!response.ok, "{response:?}");
    assert!(response
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("requires a successful snapshot"));
    let state = server.state.lock().unwrap();
    assert_eq!(state.subagents["missing-busy"].status, "idle");
    assert_eq!(state.tasks["task-missing-busy"].status, "working");
    drop(state);
    // Resolve the task through its real owner, then retain a distinct unread
    // mailbox responsibility. Missing address alone still cannot retire it.
    let closed_task = handle_task_close(&server, "child".into(), "token-child".into(),
        "task-missing-busy".into(), true, Some("owner resolves the fixture task before unread-only responsibility check".into()));
    assert!(closed_task.ok, "{closed_task:?}");
    assert_eq!(server.state.lock().unwrap().tasks["task-missing-busy"].status, "closed");
    let sent = handle_send(&server, "parent".into(), "child".into(), "notify".into(),
        Some("unread responsibility".into()), "This child still owns an unread result.".into(),
        None, "queued".into());
    assert!(sent.ok, "{sent:?}");
    let message_id = sent.data["msg_id"].as_str().unwrap();
    let state_before = std::fs::read(&server.state_path).unwrap();
    let unread_refusal = dispatch(&server, subagent_req(crate::subagent::Action::Close {
        id: "missing-busy".into(),
    }));
    assert!(!unread_refusal.ok, "{unread_refusal:?}");
    assert!(unread_refusal.error.as_deref().unwrap_or_default().contains("requires a successful snapshot"));
    assert_eq!(std::fs::read(&server.state_path).unwrap(), state_before);
    let state = server.state.lock().unwrap();
    assert_eq!(state.subagents["missing-busy"].status, "idle");
    assert_eq!(state.msgs[message_id].state, "pending");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_subagent_still_requires_a_snapshot_before_close() {
    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register_appserver_worker(&server, &root, "child", "thread-child");
    server.commit(&[Event::SubagentUpdated {
        subagent: subagent_record("unknown-retire", "idle", "child"),
    }]);
    let child = server.state.lock().unwrap().workers["child"].clone();
    assert_eq!(
        worker_presence(&server, &child),
        IdentityPresence::Present,
        "a natively registered App Server peer whose thread answers is the presence authority"
    );
    let server = Arc::new(server);
    let response = dispatch(
        &server,
        subagent_req(crate::subagent::Action::Close {
            id: "unknown-retire".into(),
        }),
    );
    assert!(!response.ok, "{response:?}");
    assert!(response
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("requires a successful snapshot"));
    assert_eq!(
        server.state.lock().unwrap().subagents["unknown-retire"].status,
        "idle"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn subagent_close_rechecks_observed_owner_and_actor_but_allows_unrelated_writes() {
    for changed in ["child", "parent", "unrelated"] {
        let (mut server, root) = test_server();
        register(&server, "parent", "thread-parent");
        register_appserver_worker(&server, &root, "child", "thread-child");
        server.commit(&[Event::SubagentUpdated {
            subagent: subagent_record("observed-close", "idle", "child"),
        }]);
        let slot = Arc::new(std::sync::Mutex::new(None::<std::sync::Weak<Server>>));
        let observed_state = Arc::new(std::sync::Mutex::new(None::<Vec<u8>>));
        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let callback_slot = slot.clone();
        let callback_state = observed_state.clone();
        let callback_fired = fired.clone();
        server.appserver_thread_status = Arc::new(move |_, thread| {
            assert_eq!(thread, "thread-child");
            if !callback_fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
                let server = callback_slot.lock().unwrap().as_ref().unwrap().upgrade().unwrap();
                if changed == "unrelated" {
                    server.commit(&[Event::KeepaliveUpdated {
                        worker_id: "parent".into(), record: Default::default(),
                    }]);
                } else {
                    let worker = server.state.lock().unwrap().workers[changed].clone();
                    let result = seed_committed_owner(&server, changed,
                        &format!("replacement-token-{changed}"), worker.transport.as_ref().unwrap(),
                        &server.root.display().to_string(), Some(AppServerId::new("tui-default").unwrap()));
                    assert!(result.ok, "{result:?}");
                }
                *callback_state.lock().unwrap() = Some(std::fs::read(&server.state_path).unwrap());
            }
            Err("APP_SERVER_THREAD_MISSING: controlled observed endpoint absence".into())
        });
        let server = Arc::new(server);
        *slot.lock().unwrap() = Some(Arc::downgrade(&server));
        let result = dispatch(&server, subagent_req(crate::subagent::Action::Close {
            id: "observed-close".into(),
        }));
        assert!(fired.load(std::sync::atomic::Ordering::SeqCst), "real existing observer must execute");
        if changed == "unrelated" {
            assert!(result.ok, "unrelated write must not reject exact owner: {result:?}");
            assert_eq!(server.state.lock().unwrap().subagents["observed-close"].status, "closed");
        } else {
            assert!(!result.ok, "changed {changed} must be refused: {result:?}");
            let error = result.error.as_deref().unwrap_or_default();
            assert!(if changed == "child" { error.contains("SUBAGENT_CLOSE_TARGET_CHANGED") }
                else { error.contains("authentication failed") }, "{result:?}");
            assert_eq!(server.state.lock().unwrap().subagents["observed-close"].status, "idle");
            assert_eq!(std::fs::read(&server.state_path).unwrap(),
                observed_state.lock().unwrap().as_ref().unwrap().clone());
            assert_eq!(current_state::load(&server.state_path, false).unwrap().subagents["observed-close"].status, "idle");
        }
        drop(server);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn subagent_start_fails_explicitly_without_creating_a_codex_thread() {
    let (server, root) = test_server();
    let tmux = IsolatedTmux::start(&root);
    register_tmux(&server, "parent", tmux.endpoints().remove(0));
    let server = Arc::new(server);
    let result = dispatch(
        &server,
        subagent_req(crate::subagent::Action::Start {
            id: Some("tmux-child".into()),
            runtime: Some("codex".into()),
        }),
    );
    assert_eq!(
        result.error.as_deref(),
        Some("MANAGED_SUBAGENT_UNSUPPORTED: tmux cannot create a Codex thread; start the peer in its own tmux pane and register that pane")
    );
    let state = server.state.lock().unwrap();
    assert!(state.subagents.is_empty());
    drop(state);
    assert!(current_state::load(&server.state_path, false).unwrap().subagents.is_empty());
    drop(server);
    drop(tmux);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_subagent_is_authenticated_persistent_and_replayable() {
    use crate::subagent::{Action, Record};
    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    register(&server, "other", "%other");
    let record = bind_test_subagent_record(&server, Record {
        id: "managed".into(),
        parent: "parent".into(),
        peer: "child".into(),
        status: "starting".into(),
        thread_id: Some("thread-child".into()),
        profile: None,
        created_ms: now_ms(),
        ready_deadline_ms: now_ms() + 90000,
        last_message: None,
        error: None,
        probe_failures: Vec::new(),
        runtime: None,
        create_operation_id: None,
        binding_id: None,
    });
    let event = Event::SubagentUpdated { subagent: record };
    let encoded = serde_json::to_string(&event).unwrap();
    let mut replay = State::default();
    replay.apply(&serde_json::from_str(&encoded).unwrap());
    assert_eq!(replay.subagents["managed"].status, "starting");
    server.commit(&[event]);
    let child_ctx = handle_context(&server, "child".into(), "token-child".into());
    assert_eq!(
        child_ctx.data["role_brief"]["role_task"]
            .as_str()
            .unwrap()
            .starts_with("Legacy compatibility record only."),
        true
    );
    assert_eq!(
        child_ctx.data["authority"]["managed_subagent"],
        false
    );
    assert_eq!(child_ctx.data["authority"]["must_obey_master"], false);
    assert_eq!(
        child_ctx.data["authority"]["may_decline_master_invite"],
        false
    );
    let parent_ctx = handle_context(&server, "parent".into(), "token-parent".into());
    assert_eq!(
        parent_ctx.data["authority"]["may_decline_master_invite"],
        true
    );
    assert!(
        !crate::subagent::handle(
            &server,
            "other",
            "token-other",
            Action::Close {
                id: "managed".into()
            }
        )
        .ok
    );
    assert!(!crate::subagent::handle(&server, "parent", "wrong", Action::List).ok);
    assert!(
        !crate::subagent::handle(
            &server,
            "parent",
            "token-parent",
            Action::Ready {
                id: "managed".into()
            }
        )
        .ok
    );
    assert!(
        crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Ready {
                id: "managed".into()
            }
        )
        .ok
    );
    let count = server.state.lock().unwrap().msgs.len();
    assert!(
        crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Ready {
                id: "managed".into()
            }
        )
        .ok
    );
    assert_eq!(server.state.lock().unwrap().msgs.len(), count);
    let no_assigned = crate::subagent::handle(
        &server,
        "child",
        "token-child",
        Action::Working {
            id: "managed".into(),
        },
    );
    assert!(!no_assigned.ok);
    assert_eq!(
        no_assigned.error.as_deref(),
        Some("no assigned task to accept")
    );
    let unknown = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Status {
            id: "missing-managed".into(),
        },
    );
    assert!(!unknown.ok);
    assert!(
        crate::subagent::handle(
            &server,
            "parent",
            "token-parent",
            Action::Send {
                id: "managed".into(),
                subject: "test".into(),
                body: "task".into()
            }
        )
        .ok
    );
    let message_id = server.state.lock().unwrap().subagents["managed"]
        .last_message
        .clone()
        .unwrap();
    assert_eq!(
        server.state.lock().unwrap().tasks[&format!("task-{message_id}")].status,
        "assigned"
    );
    let server = Arc::new(server);
    let message = dispatch(&server, Req::MsgStatus { msg_id: message_id });
    assert_eq!(message.data["body"], "task");
    assert!(
        !crate::subagent::handle(
            &server,
            "parent",
            "token-parent",
            Action::Send {
                id: "managed".into(),
                subject: "test".into(),
                body: "task".into()
            }
        )
        .ok
    );
    let still_working = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Send {
            id: "managed".into(),
            subject: "must-wait".into(),
            body: "active task still owns the child".into(),
        },
    );
    assert!(!still_working.ok);
    assert_eq!(
        still_working.error.as_deref(),
        Some("subagent is not idle; query status instead of resending")
    );
    assert!(
        crate::subagent::handle(
            &server,
            "child",
            "token-child",
            Action::Working {
                id: "managed".into()
            }
        )
        .ok
    );
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .tasks
            .values()
            .next()
            .unwrap()
            .status,
        "working"
    );
    let observed = dispatch(
        &server,
        Req::SubagentObserve {
            id: Some("managed".into()),
            snapshot_lines: None,
        },
    );
    assert!(observed.ok);
    assert_eq!(observed.data["notification_channel"], "none");
    assert!(observed.data.get("screen_tail").is_none());
    assert!(observed.data["tasks"].as_array().unwrap().len() == 1);
    let snapshot = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Snapshot {
            id: "managed".into(),
            lines: 40,
        },
    );
    assert_eq!(
        snapshot.error.as_deref(),
        Some("SUBAGENT_SNAPSHOT_UNSUPPORTED: tmux panes do not expose durable Codex thread history; inspect the peer's durable mailbox and task state")
    );
    let ready = crate::subagent::handle(
        &server,
        "child",
        "token-child",
        Action::Ready {
            id: "managed".into(),
        },
    );
    assert!(!ready.ok);
    assert!(ready
        .error
        .as_deref()
        .unwrap_or_default()
        .starts_with("TMUX_NOTIFICATION_REJECTED:"));
    assert_eq!(
        server.state.lock().unwrap().subagents["managed"].status,
        "idle"
    );
    server.commit(&[Event::SubagentSnapshotCaptured {
        subagent_id: "managed".into(),
        thread_id: "thread-child".into(),
        captured_ms: now_ms(),
    }]);
    assert!(
        crate::subagent::handle(
            &server,
            "parent",
            "token-parent",
            Action::Close {
                id: "managed".into()
            }
        )
        .ok
    );
    assert!(
        crate::subagent::handle(
            &server,
            "parent",
            "token-parent",
            Action::Close {
                id: "managed".into()
            }
        )
        .ok
    );
    std::fs::remove_dir_all(root).unwrap();
}
