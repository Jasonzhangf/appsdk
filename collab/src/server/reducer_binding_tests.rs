use super::*;

#[test]
fn migration_commit_evidence_survives_journal_replay_and_checkpoint() {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "collab-migration-evidence-replay-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(server_dir.join("journal.jsonl"))
        .unwrap();
    let server = Server {
        config: crate::config::Config::default(),
        root: root.clone(),
        storage_root: root.clone(),
        journal_path: root.join(".agent-collab/server/journal.jsonl"),
        host_paths: HostPaths::for_state_root(root.join("host-state")).unwrap(),
        state: Mutex::new(State::default()),
        journal: Mutex::new(journal),
        appserver_candidate_check: Arc::new(|candidate| {
            Ok(SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some(candidate.endpoint.clone()),
                namespace: Some(candidate.namespace.clone()),
                session_id: Some(candidate.session_id.clone()),
                thread_id: Some(candidate.thread_id.clone()),
                tmux_endpoint: None,
                capabilities: vec!["send_message_to_thread".into()],
                self_check: "test appserver".into(),
            })
        }),
        appserver_notification_sink: Arc::new(|_, _, _, _, _, _| {
            Ok(serde_json::json!({"accepted": true}))
        }),
        appserver_thread_status: Arc::new(|_, thread_id| {
            Ok(serde_json::json!({
                "thread": {
                    "id": thread_id,
                    "status": {"type": "idle"},
                    "canAcceptDirectInput": true
                }
            }))
        }),
        appserver_thread_archive: Arc::new(|_, _| Ok(serde_json::json!({"archived": true}))),
        mailbox_notify: Notify::new(),
    };
    let repeated_events = (0..99)
        .map(|_| Event::KeepaliveUpdated {
            worker_id: "worker".into(),
            record: crate::server::keepalive::Record::default(),
        })
        .collect::<Vec<_>>();
    server.commit_checked(&repeated_events).unwrap();

    let evidence = crate::server::global_state::MigrationCommitEvidence::new(
        "migration-1",
        "project-1",
        1,
        "sha256:source",
        OperationId::new("migration-op-1").unwrap(),
        None,
        7,
        100,
    )
    .unwrap();

    server
        .commit_checked(&[Event::GlobalMigrationCommitEvidence {
            evidence: evidence.clone(),
        }])
        .unwrap();
    let snapshot = server.state.lock().unwrap().snapshot_events();
    assert!(snapshot.iter().any(|event| {
        matches!(
            event,
            Event::GlobalMigrationCommitEvidence { evidence: observed }
                if observed == &evidence
        )
    }));
    assert_eq!(
        server
            .state
            .lock()
            .unwrap()
            .global
            .migration_commit_evidence
            .len(),
        1
    );

    let replayed = replay(&root).unwrap();
    assert_eq!(
        replayed
            .global
            .migration_commit_evidence
            .get("migration-op-1"),
        Some(&evidence)
    );
    replayed.global.validate().unwrap();

    let mut checkpointed = State::default();
    for (line, event) in snapshot.iter().enumerate() {
        apply_replayed_event(&mut checkpointed, event, line + 1).unwrap();
    }
    assert_eq!(
        checkpointed
            .global
            .migration_commit_evidence
            .get("migration-op-1"),
        Some(&evidence)
    );
    checkpointed.global.validate().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn task_register_wires_worktree_binding_and_replays_it() {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::path::PathBuf::from(format!(
        "/tmp/collab-r2-binding-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(server_dir.join("journal.jsonl"))
        .unwrap();
    let external_base = root.with_file_name(format!(
        "{}-external",
        root.file_name().unwrap().to_string_lossy()
    ));
    std::fs::create_dir_all(&external_base).unwrap();
    let mut config = crate::config::Config::default();
    config.worktree.base = Some(external_base.clone());
    let server = Server {
        config,
        root: root.clone(),
        storage_root: root.clone(),
        journal_path: root.join(".agent-collab/server/journal.jsonl"),
        host_paths: HostPaths::for_state_root(root.join("host-state")).unwrap(),
        state: Mutex::new(State::default()),
        journal: Mutex::new(journal),
        appserver_candidate_check: Arc::new(|candidate| {
            Ok(peer_tests::test_appserver_transport(&candidate.thread_id))
        }),
        appserver_notification_sink: default_appserver_notification_sink(),
        appserver_thread_status: default_appserver_thread_status(),
        appserver_thread_archive: default_appserver_thread_archive(),
        mailbox_notify: tokio::sync::Notify::new(),
    };
    peer_tests::register(&server, "worker", "thread-worker");
    let worktree = external_base
        .canonicalize()
        .unwrap()
        .join(crate::server::configured_project_key(&root).unwrap())
        .join("task-1")
        .display()
        .to_string();
    let registered = handle_task_register(
        &server,
        "worker".into(),
        "token-worker".into(),
        "task-1".into(),
        None,
        None,
        Some(worktree.clone()),
        Some("feature/branch".into()),
        Some("abc123".into()),
        "p2".into(),
    );
    assert!(registered.ok, "{registered:?}");
    let canonical_worktree = worktree.clone();
    {
        let state = server.state.lock().unwrap();
        assert_eq!(
            state.worktree_bindings["binding-task-task-1"].task_id,
            "task-1"
        );
        assert_eq!(
            state.worktree_bindings["binding-task-task-1"].worktree_root,
            canonical_worktree
        );
    }
    let replayed = replay(&root).unwrap();
    assert_eq!(
        replayed.worktree_bindings["binding-task-task-1"].task_id,
        "task-1"
    );
    assert_eq!(
        replayed.worktree_bindings["binding-task-task-1"].worktree_root,
        canonical_worktree
    );
    std::fs::remove_dir_all(external_base).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn task_register_journal_failures_are_explicit_and_read_only_after_normal_write() {
    use TaskRegisterJournalFault::{Append, Sync};

    for fault in [Append, Sync] {
        let (server, root) = peer_tests::test_server();
        assert!(peer_tests::register(&server, "worker", "%worker").ok);
        let mailbox_dir = root.join(".agent-collab/mailbox");
        std::fs::create_dir_all(&mailbox_dir).unwrap();
        let mailbox_sentinel = mailbox_dir.join("sentinel");
        std::fs::write(&mailbox_sentinel, b"unchanged").unwrap();
        let journal_path = root.join(".agent-collab/server/journal.jsonl");
        let journal_before = std::fs::read(&journal_path).unwrap();
        let (sequence_before, revision_before) = {
            let state = server.state.lock().unwrap();
            (state.sequence, state.revision)
        };
        let server = Arc::new(server);

        inject_task_register_journal_fault(fault);
        let response = handle_task_register(
            server.as_ref(),
            "worker".into(),
            "token-worker".into(),
            "task-register-fault".into(),
            None,
            Some("feature-register-fault".into()),
            None,
            None,
            None,
            "p2".into(),
        );
        assert!(!response.ok, "{fault:?}: {response:?}");
        assert!(response
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("TASK_DURABILITY_FAILED: journal ")));

        let state = server.state.lock().unwrap();
        assert!(!state.tasks.contains_key("task-register-fault"));
        assert_eq!(state.sequence, sequence_before);
        assert_eq!(state.revision, revision_before);
        assert!(state
            .journal_poison
            .as_deref()
            .is_some_and(|error| error.contains("injected task register journal")));
        drop(state);
        assert_eq!(std::fs::read(&mailbox_sentinel).unwrap(), b"unchanged");
        if matches!(fault, Append) {
            assert_eq!(std::fs::read(&journal_path).unwrap(), journal_before);
        }

        let read_only = dispatch(&server, Req::TaskStatus { task_id: None });
        assert!(read_only.ok, "{fault:?}: {read_only:?}");
        assert_eq!(read_only.data["tasks"].as_array().unwrap().len(), 0);

        let retry = handle_task_register(
            server.as_ref(),
            "worker".into(),
            "token-worker".into(),
            "task-register-retry".into(),
            None,
            Some("feature-register-retry".into()),
            None,
            None,
            None,
            "p2".into(),
        );
        assert!(!retry.ok, "{fault:?}: poisoned writes must fail closed");
        assert!(retry
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("TASK_DURABILITY_FAILED: journal ")));
        assert_eq!(std::fs::read(&mailbox_sentinel).unwrap(), b"unchanged");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn task_register_journal_failures_are_explicit_and_preserve_resource_holder() {
    use TaskRegisterJournalFault::{Append, Sync};

    for fault in [Append, Sync] {
        let (server, root) = peer_tests::test_server();
        assert!(peer_tests::register(&server, "holder", "%holder").ok);
        assert!(peer_tests::register(&server, "waiter", "%waiter").ok);
        let holder = handle_task_register(
            &server,
            "holder".into(),
            "token-holder".into(),
            "held-task".into(),
            None,
            Some("shared-resource".into()),
            None,
            None,
            None,
            "p2".into(),
        );
        assert!(holder.ok, "{holder:?}");
        let mailbox_dir = root.join(".agent-collab/mailbox");
        std::fs::create_dir_all(&mailbox_dir).unwrap();
        let mailbox_sentinel = mailbox_dir.join("sentinel");
        std::fs::write(&mailbox_sentinel, b"unchanged").unwrap();
        let journal_path = root.join(".agent-collab/server/journal.jsonl");
        let journal_before = std::fs::read(&journal_path).unwrap();
        let (sequence_before, revision_before) = {
            let state = server.state.lock().unwrap();
            (state.sequence, state.revision)
        };
        let server = Arc::new(server);

        inject_task_register_journal_fault(fault);
        let response = handle_task_register(
            server.as_ref(),
            "waiter".into(),
            "token-waiter".into(),
            "blocked-task".into(),
            None,
            Some("shared-resource".into()),
            None,
            None,
            None,
            "p2".into(),
        );
        assert!(!response.ok, "{fault:?}: {response:?}");
        assert!(response
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("TASK_DURABILITY_FAILED: journal ")));

        let state = server.state.lock().unwrap();
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.tasks["held-task"].status, "working");
        assert!(!state.tasks.contains_key("blocked-task"));
        assert_eq!(state.sequence, sequence_before);
        assert_eq!(state.revision, revision_before);
        assert!(state
            .journal_poison
            .as_deref()
            .is_some_and(|error| error.contains("injected task register journal")));
        drop(state);
        if matches!(fault, Append) {
            assert_eq!(std::fs::read(&journal_path).unwrap(), journal_before);
        }
        assert_eq!(std::fs::read(&mailbox_sentinel).unwrap(), b"unchanged");

        let read_only = dispatch(
            &server,
            Req::TaskConflicts {
                feature_id: Some("shared-resource".into()),
                worktree_path: None,
            },
        );
        assert!(read_only.ok, "{fault:?}: {read_only:?}");
        assert_eq!(read_only.data["conflicts"].as_array().unwrap().len(), 1);
        assert_eq!(read_only.data["conflicts"][0]["id"], "held-task");

        let retry = handle_task_register(
            server.as_ref(),
            "waiter".into(),
            "token-waiter".into(),
            "blocked-retry".into(),
            None,
            Some("another-resource".into()),
            None,
            None,
            None,
            "p2".into(),
        );
        assert!(!retry.ok, "{fault:?}: poisoned writes must fail closed");
        assert!(retry
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("TASK_DURABILITY_FAILED: journal ")));
        assert_eq!(std::fs::read(&mailbox_sentinel).unwrap(), b"unchanged");
        std::fs::remove_dir_all(root).unwrap();
    }
}
