use super::*;

#[test]
fn task_register_preserves_current_worktree_binding() {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::path::PathBuf::from(format!(
        "/tmp/collab-r2-binding-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let server_dir = root.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
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
        state_path: server_dir.join("state.json"),
        host_paths: HostPaths::for_state_root(root.join("host-state")).unwrap(),
        register_gate: Arc::new(Mutex::new(())),
        state: Mutex::new(State::default()),
        appserver_candidate_check: Arc::new(|candidate| {
            Ok(peer_tests::test_appserver_transport(&candidate.thread_id))
        }),
        appserver_notification_sink: default_appserver_notification_sink(),
        appserver_thread_status: default_appserver_thread_status(),
        appserver_thread_archive: default_appserver_thread_archive(),
        rex_monitor: Default::default(),
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
    let current = current_state::load(&server.state_path, false).unwrap();
    assert_eq!(
        current.worktree_bindings["binding-task-task-1"].task_id,
        "task-1"
    );
    assert_eq!(
        current.worktree_bindings["binding-task-task-1"].worktree_root,
        canonical_worktree
    );
    std::fs::remove_dir_all(external_base).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
