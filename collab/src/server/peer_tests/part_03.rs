#[test]
fn replay_rejects_a_checkpoint_that_regresses_real_history() {
    let (server, root) = test_server();
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let events = [
        Event::Registered {
            worker: crate::server::state::WorkerRec {
                id: "checkpoint-worker".into(),
                token: "checkpoint-token".into(),
                cwd: "/tmp".into(),
                registered_ms: 1,
                transport: Some(test_appserver_transport("thread-checkpoint-worker")),
            },
        },
        Event::ReducerCheckpoint {
            sequence: 0,
            revision: 0,
        },
    ];
    let body = events
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&journal, &body).unwrap();

    let error = match super::replay(&root) {
        Ok(_) => panic!("a real checkpoint rollback must fail closed"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("reducer checkpoint regresses version"),
        "unexpected error: {error}"
    );
    assert_eq!(std::fs::read_to_string(&journal).unwrap(), body);
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_registration_generation_reaches_max_then_fails_before_journaling_overflow() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();
    let mut first = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    let crate::server::state::TypedCommand::RegisterWorker { binding, .. } = &mut first.command;
    binding.endpoint_generation = u64::MAX - 1;
    first.envelope.endpoint_generation = u64::MAX - 1;
    first.envelope.command_id = crate::identity::CommandId::new("register-max-minus-one").unwrap();
    first.envelope.operation_id =
        crate::identity::OperationId::new("register-op-max-minus-one").unwrap();
    let first = server
        .typed_dispatch(first)
        .expect("MAX-1 binding generation must be accepted");
    assert!(!first.replayed);

    let max = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .unwrap();
    assert_eq!(max.envelope.endpoint_generation, u64::MAX);
    let max = server
        .typed_dispatch(max)
        .expect("MAX binding generation must be accepted");
    assert!(!max.replayed);
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .global
            .projects
            .values()
            .next()
            .unwrap()
            .runtime_bindings["binding-worker"]
            .endpoint_generation,
        u64::MAX
    );

    let journal_before_overflow =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    let receipt_count_before_overflow = server.state.lock().unwrap().global.command_receipts.len();
    let error = server
        .typed_register_envelope("worker", "token-worker", "%worker", cwd)
        .expect_err("MAX binding generation must fail explicitly on increment overflow");
    assert!(
        error.contains("endpoint generation overflow"),
        "unexpected error: {error}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap(),
        journal_before_overflow,
        "generation overflow must happen before any journal append"
    );
    assert_eq!(
        server.state.lock().unwrap().global.command_receipts.len(),
        receipt_count_before_overflow,
        "generation overflow must not masquerade as a replayed receipt"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn replay_rejects_invalid_command_receipt_without_rewriting_the_journal() {
    let (server, root) = test_server();
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let event = Event::CommandRecorded {
        command_id: "invalid-receipt".into(),
        receipt: crate::server::state::CommandReceipt {
            operation_id: "invalid-receipt-operation".into(),
            outcome: json!({"accepted": true}),
            sequence: 0,
            revision: 0,
        },
    };
    let body = format!("{}\n", serde_json::to_string(&event).unwrap());
    std::fs::write(&journal, &body).unwrap();

    let error = match super::replay(&root) {
        Ok(_) => panic!("replay must apply command receipt validation before exposing state"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("command receipt sequence"));
    assert_eq!(std::fs::read_to_string(&journal).unwrap(), body);
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn replay_rejects_invalid_command_completion_receipt_without_rewriting_the_journal() {
    let (server, root) = test_server();
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let events = [
        Event::CommandStarted {
            command_id: "invalid-completion".into(),
            operation_id: "invalid-completion-operation".into(),
        },
        Event::CommandCompleted {
            command_id: "invalid-completion".into(),
            operation_id: "invalid-completion-operation".into(),
            receipt: crate::server::state::CommandReceipt {
                operation_id: "invalid-completion-operation".into(),
                outcome: json!({"accepted": true}),
                sequence: 1,
                revision: 0,
            },
        },
    ];
    let body = events
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&journal, &body).unwrap();

    let error = match super::replay(&root) {
        Ok(_) => panic!("replay must validate a command completion receipt"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("command receipt revision"));
    assert_eq!(std::fs::read_to_string(&journal).unwrap(), body);
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_dispatch_rejects_stale_revision_without_appending() {
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
    let error = server.typed_dispatch(stale).unwrap_err();
    assert!(error
        .to_string()
        .contains("compare-and-swap revision mismatch"));
    assert_eq!(
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .lines()
            .count(),
        6
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_dispatch_rejects_wrong_principal_and_scope() {
    let (server, root) = test_server();
    let cwd = root.to_str().unwrap();
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
    assert!(
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap()
            .trim()
            .is_empty()
    );
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
    let before_journal =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap();
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
    let after_journal =
        std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    assert_eq!(
        after_journal, before_journal,
        "a rejected dual-key mismatch must not append a journal event"
    );
    assert_eq!(
        server.state.lock().unwrap().global,
        before_global,
        "a rejected dual-key mismatch must not mutate the global reducer"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn global_reducer_failure_is_explicit_and_poisoned_after_journal_append() {
    let (server, root) = test_server();
    let binding = crate::server::global_state::RuntimeBinding::new(
        crate::scope::ProjectScopeId::new("/unregistered-project").unwrap(),
        crate::identity::AppServerId::new("tui-default").unwrap(),
        crate::identity::AgentId::new("worker").unwrap(),
        crate::identity::RuntimeId::new("runtime-worker").unwrap(),
        crate::identity::BindingId::new("binding-worker").unwrap(),
        1,
        None,
    )
    .unwrap();
    let error = server
        .commit_checked(&[Event::GlobalRuntimeBound { binding }])
        .unwrap_err();
    assert!(matches!(error, JournalError::Reducer(_)));
    assert!(server.state.lock().unwrap().journal_poison.is_some());
    assert_eq!(server.state.lock().unwrap().global.projects.len(), 0);
    let journal = std::fs::read_to_string(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    assert!(journal.contains("GlobalRuntimeBound"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn command_start_append_and_sync_failures_fail_closed_before_business_apply() {
    for fault in [
        CommandJournalFault::StartAppend,
        CommandJournalFault::StartSync,
    ] {
        let (server, root) = test_server();
        inject_command_journal_fault(fault);
        let result = server.commit_command(
            "command-start-fault",
            "operation-start-fault",
            &[Event::KeepaliveUpdated {
                worker_id: "worker".into(),
                record: crate::server::keepalive::Record::default(),
            }],
            json!({"accepted": true}),
        );
        assert!(result.is_err());
        assert!(server.state.lock().unwrap().keepalives.is_empty());
        let replay = super::replay(&root);
        match fault {
            CommandJournalFault::StartAppend => {
                assert!(replay.is_ok(), "failed first append must leave no command");
            }
            CommandJournalFault::StartSync => {
                assert!(
                    replay.is_err(),
                    "sync failure after start append must poison replay"
                );
            }
            _ => unreachable!(),
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn command_completion_append_failure_leaves_incomplete_replay() {
    let (server, root) = test_server();
    inject_command_journal_fault(CommandJournalFault::CompletionAppend);
    let result = server.commit_command(
        "command-completion-append",
        "operation-completion-append",
        &[Event::KeepaliveUpdated {
            worker_id: "worker".into(),
            record: crate::server::keepalive::Record::default(),
        }],
        json!({"accepted": true}),
    );
    assert!(result.is_err());
    assert!(server.state.lock().unwrap().keepalives.is_empty());
    assert!(super::replay(&root).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn command_completion_sync_failure_is_explicit_and_replayable_if_marker_was_written() {
    let (server, root) = test_server();
    inject_command_journal_fault(CommandJournalFault::CompletionSync);
    let result = server.commit_command(
        "command-completion-sync",
        "operation-completion-sync",
        &[Event::KeepaliveUpdated {
            worker_id: "worker".into(),
            record: crate::server::keepalive::Record::default(),
        }],
        json!({"accepted": true}),
    );
    assert!(result.is_err());
    assert!(server.state.lock().unwrap().keepalives.is_empty());
    let replayed = super::replay(&root).expect("written completion marker must replay");
    assert!(replayed.keepalives.contains_key("worker"));
    assert_eq!(
        replayed.command_receipts["command-completion-sync"].operation_id,
        "operation-completion-sync"
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
        worker_presence(&server, &child),
        IdentityPresence::Missing,
        "route-unavailable identity must classify as definitive Missing"
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
    server.appserver_thread_status =
        Arc::new(|_, _| panic!("tmux close must not inspect AppServer thread status"));
    let child = server.state.lock().unwrap().workers["child"].clone();
    assert_eq!(
        worker_presence(&server, &child),
        IdentityPresence::Present,
        "live tmux pane presence is independent of retired AppServer thread state"
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
                created_ms: now_ms(),
                updated_ms: now_ms(),
            },
        },
    ]);
    kill_registered_worker_pane(&server, "child");
    let child = server.state.lock().unwrap().workers["child"].clone();
    assert_eq!(worker_presence(&server, &child), IdentityPresence::Missing);
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
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn live_subagent_still_requires_a_snapshot_before_close() {
    let (mut server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    server.commit(&[Event::SubagentUpdated {
        subagent: subagent_record("unknown-retire", "idle", "child"),
    }]);
    let child = server.state.lock().unwrap().workers["child"].clone();
    assert_eq!(
        worker_presence(&server, &child),
        IdentityPresence::Present,
        "the registered tmux pane is the presence authority"
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
    assert!(state.journal_poison.is_none());
    drop(state);
    assert!(replay(&root).unwrap().subagents.is_empty());
    drop(server);
    drop(tmux);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn subagent_working_journal_failure_preserves_task_and_subagent_and_poison_rejects_mutation() {
    use crate::server::SubagentJournalFault::{WorkingAppend, WorkingSync};

    for fault in [WorkingAppend, WorkingSync] {
        let (server, root) = test_server();
        register(&server, "parent", "%parent");
        register(&server, "child", "%child");
        let now = now_ms();
        let mut record = subagent_record("working-journal-fault", "assigned", "child");
        record.last_message = Some("working-journal-fault".into());
        server.commit(&[
            Event::SubagentUpdated { subagent: record },
            Event::TaskCreated {
                task: TaskRec {
                    id: "task-working-journal-fault".into(),
                    owner: "child".into(),
                    created_by: "parent".into(),
                    feature_id: None,
                    worktree_path: None,
                    branch: None,
                    base_commit: None,
                    priority: "p2".into(),
                    status: "assigned".into(),
                    next_step: None,
                    wait: None,
                    created_ms: now,
                    updated_ms: now,
                },
            },
        ]);
        let server = Arc::new(server);
        crate::server::inject_subagent_journal_fault(fault);
        let result = dispatch(
            &server,
            Req::Subagent {
                worker_id: "child".into(),
                token: "token-child".into(),
                command: crate::subagent::Action::Working {
                    id: "working-journal-fault".into(),
                },
                launch_env: Default::default(),
            },
        );
        assert!(!result.ok, "{fault:?}: {result:?}");
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["working-journal-fault"].status, "assigned");
        assert_eq!(state.tasks["task-working-journal-fault"].status, "assigned");
        assert!(state.journal_poison.is_some());
        drop(state);
        let rejected = dispatch(
            &server,
            Req::Subagent {
                worker_id: "child".into(),
                token: "token-child".into(),
                command: crate::subagent::Action::Working {
                    id: "working-journal-fault".into(),
                },
                launch_env: Default::default(),
            },
        );
        assert!(!rejected.ok);
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["working-journal-fault"].status, "assigned");
        assert_eq!(state.tasks["task-working-journal-fault"].status, "assigned");
        drop(state);
        assert!(replay(&root).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn subagent_close_first_journal_failure_does_not_remove_external_manifest() {
    use crate::server::SubagentJournalFault::{CloseFirstAppend, CloseFirstSync};

    for fault in [CloseFirstAppend, CloseFirstSync] {
        let (mut server, root) = test_server();
        register(&server, "parent", "%parent");
        register(&server, "child", "%child");
        server.commit(&[Event::SubagentUpdated {
            subagent: subagent_record("close-first-fault", "idle", "child"),
        }]);
        server.commit(&[Event::SubagentSnapshotCaptured {
            subagent_id: "close-first-fault".into(),
            thread_id: "thread-child".into(),
            captured_ms: now_ms(),
        }]);
        let manifest = root.join(".agent-collab/server/launch-close-first-fault.json");
        std::fs::write(&manifest, b"test-only manifest").unwrap();
        let server = Arc::new(server);
        crate::server::inject_subagent_journal_fault(fault);
        let result = dispatch(
            &server,
            subagent_req(crate::subagent::Action::Close {
                id: "close-first-fault".into(),
            }),
        );
        assert!(!result.ok, "{fault:?}: {result:?}");
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["close-first-fault"].status, "idle");
        assert!(state.journal_poison.is_some());
        drop(state);
        assert!(manifest.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn subagent_close_final_journal_failure_reports_unknown_and_stays_open() {
    use crate::server::SubagentJournalFault::{CloseFinalAppend, CloseFinalSync};

    for fault in [CloseFinalAppend, CloseFinalSync] {
        let (mut server, root) = test_server();
        register(&server, "parent", "%parent");
        register(&server, "child", "%child");
        server.commit(&[Event::SubagentUpdated {
            subagent: subagent_record("close-final-fault", "idle", "child"),
        }]);
        server.commit(&[Event::SubagentSnapshotCaptured {
            subagent_id: "close-final-fault".into(),
            thread_id: "thread-child".into(),
            captured_ms: now_ms(),
        }]);
        let server = Arc::new(server);
        crate::server::inject_subagent_journal_fault(fault);
        let result = dispatch(
            &server,
            subagent_req(crate::subagent::Action::Close {
                id: "close-final-fault".into(),
            }),
        );
        assert!(!result.ok, "{fault:?}: {result:?}");
        assert!(result.error.unwrap().contains("outcome unknown"));
        let state = server.state.lock().unwrap();
        assert_eq!(state.subagents["close-final-fault"].status, "closing");
        assert!(state.journal_poison.is_some());
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn replayed_command_is_idempotent_and_operation_conflict_fails_closed() {
    let (server, root) = test_server();
    server
        .commit_command(
            "command-replay",
            "operation-replay",
            &[Event::KeepaliveUpdated {
                worker_id: "worker".into(),
                record: crate::server::keepalive::Record::default(),
            }],
            json!({"accepted": true}),
        )
        .unwrap();
    let restarted = {
        let journal = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join(".agent-collab/server/journal.jsonl"))
            .unwrap();
        Server {
            config: crate::config::Config::default(),
            root: root.clone(),
            storage_root: root.clone(),
            journal_path: root.join(".agent-collab/server/journal.jsonl"),
            host_paths: HostPaths::for_state_root(root.join("host-state")).unwrap(),
            state: Mutex::new(super::replay(&root).unwrap()),
            journal: Mutex::new(journal),
            appserver_candidate_check: crate::server::default_appserver_candidate_check(),
            appserver_notification_sink: crate::server::default_appserver_notification_sink(),
            appserver_thread_status: crate::server::default_appserver_thread_status(),
            appserver_thread_archive: crate::server::default_appserver_thread_archive(),
            mailbox_notify: tokio::sync::Notify::new(),
        }
    };
    let retry = restarted
        .commit_command(
            "command-replay",
            "operation-replay",
            &[Event::KeepaliveUpdated {
                worker_id: "duplicate".into(),
                record: crate::server::keepalive::Record::default(),
            }],
            json!({"accepted": false}),
        )
        .unwrap();
    assert!(retry.replayed);
    assert_eq!(retry.outcome, json!({"accepted": true}));
    let conflict = restarted.commit_command(
        "command-other",
        "operation-replay",
        &[],
        json!({"accepted": true}),
    );
    assert!(matches!(conflict, Err(JournalError::InvalidCommand(_))));
    assert!(!restarted
        .state
        .lock()
        .unwrap()
        .keepalives
        .contains_key("duplicate"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn replay_rejects_business_event_persisted_without_command_completion() {
    let (server, root) = test_server();
    let journal = root.join(".agent-collab/server/journal.jsonl");
    let events = [
        Event::CommandStarted {
            command_id: "incomplete".into(),
            operation_id: "operation-incomplete".into(),
        },
        Event::KeepaliveUpdated {
            worker_id: "worker".into(),
            record: crate::server::keepalive::Record::default(),
        },
    ];
    let body = events
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&journal, format!("{body}\n")).unwrap();
    let error = match super::replay(&root) {
        Ok(_) => panic!("incomplete command replay must fail"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("incomplete"), "unexpected error: {error}");
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_command_record_without_outcome_is_rejected() {
    let (server, root) = test_server();
    let journal = root.join(".agent-collab/server/journal.jsonl");
    std::fs::write(
        &journal,
        r#"{"ev":"CommandRecorded","command_id":"legacy","receipt":{"operation_id":"op"}}
"#,
    )
    .unwrap();
    let error = match super::replay(&root) {
        Ok(_) => panic!("legacy command record without outcome must be rejected"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("outcome"), "unexpected error: {error}");
    drop(server);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn try_commit_reports_journal_failure_without_applying_state() {
    let (server, root) = test_server();
    *server.journal.lock().unwrap() =
        std::fs::File::open(root.join(".agent-collab/server/journal.jsonl")).unwrap();
    let mut state = State::default();
    let error = server
        .try_commit_locked(
            &mut state,
            &[Event::KeepaliveUpdated {
                worker_id: "worker".into(),
                record: crate::server::keepalive::Record::default(),
            }],
        )
        .expect_err("read-only journal must be reported to the caller");
    assert!(error.contains("journal append"));
    assert!(state.keepalives.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn subagent_close_does_not_report_success_when_transition_cannot_persist() {
    use crate::subagent::{Action, Record};

    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    server.commit(&[Event::SubagentUpdated {
        subagent: Record {
            id: "managed-close".into(),
            parent: "parent".into(),
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
    server.commit(&[Event::SubagentSnapshotCaptured {
        subagent_id: "managed-close".into(),
        thread_id: "thread-child".into(),
        captured_ms: now_ms(),
    }]);
    *server.journal.lock().unwrap() =
        std::fs::File::open(root.join(".agent-collab/server/journal.jsonl")).unwrap();

    let response = crate::subagent::handle(
        &server,
        "parent",
        "token-parent",
        Action::Close {
            id: "managed-close".into(),
        },
    );
    assert!(!response.ok);
    assert!(response
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("journal append"));
    assert_eq!(
        server.state.lock().unwrap().subagents["managed-close"].status,
        "idle"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn activity_log_never_copies_launch_credentials() {
    let request = Req::Subagent {
        worker_id: "parent".into(),
        token: "token".into(),
        command: crate::subagent::Action::Start {
            id: None,
            runtime: None,
        },
        launch_env: std::collections::BTreeMap::from([("SECRET".into(), "do-not-log".into())]),
    };
    let log = request_activity(&request, &Resp::data(json!({})));
    assert!(log["request"].get("launch_env").is_none());
    assert!(!log.to_string().contains("do-not-log"));
}

#[test]
fn managed_subagent_is_authenticated_persistent_and_replayable() {
    use crate::subagent::{Action, Record};
    let (server, root) = test_server();
    register(&server, "parent", "%parent");
    register(&server, "child", "%child");
    register(&server, "other", "%other");
    let record = Record {
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
    };
    let event = Event::SubagentUpdated { subagent: record };
    let encoded = serde_json::to_string(&event).unwrap();
    let mut replay = State::default();
    replay.apply(&serde_json::from_str(&encoded).unwrap());
    assert_eq!(replay.subagents["managed"].status, "starting");
    server.commit(&[event]);
    let child_ctx = handle_context(&server, "child".into(), "token-child".into());
    assert_eq!(child_ctx.data["authority"]["must_obey_master"], true);
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
