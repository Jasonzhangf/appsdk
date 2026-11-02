#[test]
fn task_lifecycle_closes_without_git_or_resource_side_effects() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "peer", "thread-peer").ok);
    assert!(create_task(&server, "peer", "no-git-task", "feature").ok);
    let delivered = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "no-git-task".into(),
        Some("evidence without worktree or commit".into()),
        None,
    );
    assert!(delivered.ok, "{delivered:?}");
    assert_eq!(
        server.state.lock().unwrap().tasks["no-git-task"].status,
        "delivered"
    );
    let accepted = handle_task_review(
        &server,
        "peer".into(),
        "token-peer".into(),
        "no-git-task".into(),
        true,
        false,
        "reviewed without git".into(),
    );
    assert!(accepted.ok, "{accepted:?}");
    let closed = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "no-git-task".into(),
        false,
        None,
    );
    assert!(closed.ok, "{closed:?}");
    assert_eq!(
        server.state.lock().unwrap().tasks["no-git-task"].status,
        "closed"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn retired_git_task_entries_report_disabled_without_side_effects() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "peer", "thread-peer").ok);
    assert!(create_task(&server, "peer", "retired-git", "feature").ok);
    let before = server.state.lock().unwrap().tasks["retired-git"].status.clone();
    let integrated = handle_task_integrated(
        &server,
        "peer".into(),
        "token-peer".into(),
        "retired-git".into(),
        "deadbeef".into(),
        "legacy integration".into(),
    );
    assert!(!integrated.ok);
    assert_eq!(
        integrated.error.as_deref(),
        Some("COLLAB_GIT_OWNERSHIP_DISABLED: task integration, finalize-cleanup and worktree/branch cleanup are retired; deliver, review and close carry task responsibility without git or resource operations")
    );
    let finalized = handle_task_finalize_cleanup(
        &server,
        "peer".into(),
        "token-peer".into(),
        "retired-git".into(),
        None,
    );
    assert!(!finalized.ok);
    assert_eq!(
        finalized.error.as_deref(),
        Some("COLLAB_GIT_OWNERSHIP_DISABLED: task integration, finalize-cleanup and worktree/branch cleanup are retired; deliver, review and close carry task responsibility without git or resource operations")
    );
    let after = server.state.lock().unwrap().tasks["retired-git"].status.clone();
    assert_eq!(before, after);
    assert_eq!(after, "working");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn legacy_worktree_task_closes_without_deleting_anything() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "peer", "thread-peer").ok);
    assert!(create_task(&server, "peer", "legacy-wt", "feature").ok);
    {
        let mut state = server.state.lock().unwrap();
        let mut task = state.tasks.get("legacy-wt").unwrap().clone();
        task.worktree_path = Some("./playground/legacy-wt".into());
        task.branch = Some("task/legacy-wt".into());
        state.tasks.insert(task.id.clone(), task);
    }
    let delivered = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "legacy-wt".into(),
        Some("evidence only".into()),
        None,
    );
    assert!(delivered.ok, "{delivered:?}");
    let accepted = handle_task_review(
        &server,
        "peer".into(),
        "token-peer".into(),
        "legacy-wt".into(),
        true,
        false,
        "reviewed".into(),
    );
    assert!(accepted.ok, "{accepted:?}");
    let closed = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "legacy-wt".into(),
        false,
        None,
    );
    assert!(closed.ok, "{closed:?}");
    assert_eq!(
        server.state.lock().unwrap().tasks["legacy-wt"].status,
        "closed"
    );
    std::fs::remove_dir_all(root).ok();
}
