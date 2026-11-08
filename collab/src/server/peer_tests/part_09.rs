#[test]
fn removed_role_and_dispatch_commands_fail_fast() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(
        handle_task_dispatch(&server, "peer".into(), "token-peer".into())
            .error
            .unwrap()
            .contains("deprecated")
    );
    assert!(handle_task_claim(
        &server,
        "peer".into(),
        "token-peer".into(),
        "legacy-task".into(),
    )
    .error
    .unwrap()
    .contains("deprecated"));
    let server = Arc::new(server);
    for request in [
        Req::Role {
            worker_id: "peer".into(),
        },
        Req::TransferMaster {
            worker_id: "peer".into(),
            token: "token-peer".into(),
            target_id: "peer".into(),
        },
    ] {
        assert!(!dispatch(&server, request).ok);
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn lifecycle_cannot_bypass_delivery_or_review() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    register(&server, "supervisor", "%supervisor");
    promote_master(&server, "supervisor", "isolated lifecycle block approval");
    assert!(create_task(&server, "peer", "task", "feature").ok);
    assert!(create_task(&server, "peer", "blocked-task", "blocked-feature").ok);
    assert!(create_task(&server, "peer", "working-task", "working-feature").ok);
    let early_review = handle_task_review(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        true,
        false,
        "not delivered".into(),
    );
    assert!(!early_review.ok);
    assert_eq!(
        early_review.error.as_deref(),
        Some("task task must be delivered before review (current: working)")
    );
    let missing_evidence_delivery = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "working-task".into(),
        None,
        Some("/tmp/worktree".into()),
    );
    assert_eq!(
        missing_evidence_delivery.error.as_deref(),
        Some("task deliver requires non-empty --evidence")
    );
    let working_delivery = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "working-task".into(),
        Some("working task candidate verified".into()),
        Some("/tmp/worktree".into()),
    );
    assert!(working_delivery.ok, "{working_delivery:?}");
    assert_eq!(
        server.state.lock().unwrap().tasks["working-task"].status,
        "delivered"
    );
    assert!(
        handle_task_update(
            &server,
            "peer".into(),
            "token-peer".into(),
            "blocked-task".into(),
            Some("blocked".into()),
            None,
        )
        .ok
    );
    assert_eq!(server.state.lock().unwrap().tasks["blocked-task"].status, "working");
    let approval = handle_task_update_full(
        &server,
        "supervisor".into(),
        "token-supervisor".into(),
        "blocked-task".into(),
        None,
        None,
        Some("approve".into()),
        Some("confirmed isolated lifecycle blocker".into()),
    );
    assert!(approval.ok, "{approval:?}");
    let blocked_delivery = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "blocked-task".into(),
        Some("blocked task candidate verified".into()),
        Some("/tmp/worktree".into()),
    );
    assert_eq!(
        blocked_delivery.error.as_deref(),
        Some(
            "task blocked-task must be owned and working, verifying, reviewed, or rework before delivery (current: blocked)"
        )
    );
    assert!(
        handle_task_update(
            &server,
            "peer".into(),
            "token-peer".into(),
            "task".into(),
            Some("verifying".into()),
            None,
        )
        .ok
    );
    let skipped_delivery = handle_task_update(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        Some("merged".into()),
        None,
    );
    assert_eq!(
        skipped_delivery.error.as_deref(),
        Some("use collab task review/integrated for integration-owned lifecycle transitions")
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["task"].status,
        "verifying"
    );
    let delivered = handle_task_deliver(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        Some("candidate verified".into()),
        Some("/tmp/worktree".into()),
    );
    assert!(delivered.ok, "{delivered:?}");
    let accepted = handle_task_review(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        true,
        false,
        "review passed".into(),
    );
    assert!(accepted.ok, "{accepted:?}");
    assert_eq!(
        server.state.lock().unwrap().tasks["task"].status,
        "accepted"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn legacy_accepted_candidate_can_record_merge() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "accepted-task", "feature").ok);
    {
        let mut state = server.state.lock().unwrap();
        let mut task = state.tasks.remove("accepted-task").unwrap();
        task.status = "accepted".into();
        state.tasks.insert(task.id.clone(), task);
    }
    let merged = handle_task_update(
        &server,
        "peer".into(),
        "token-peer".into(),
        "accepted-task".into(),
        Some("merged".into()),
        Some("integration recorded".into()),
    );
    assert!(
        merged.ok,
        "legacy accepted task must be mergeable: {merged:?}"
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["accepted-task"].status,
        "merged"
    );
    assert_eq!(
        current_state::load(&server.state_path, false).unwrap().tasks["accepted-task"].status,
        "merged"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn merged_task_without_lifecycle_edges_cannot_close() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "accepted-task", "feature").ok);
    {
        let mut state = server.state.lock().unwrap();
        let mut task = state.tasks.remove("accepted-task").unwrap();
        task.status = "accepted".into();
        state.tasks.insert(task.id.clone(), task);
    }
    assert!(
        handle_task_update(
            &server,
            "peer".into(),
            "token-peer".into(),
            "accepted-task".into(),
            Some("merged".into()),
            Some("legacy merge compatibility".into()),
        )
        .ok
    );
    let closed = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "accepted-task".into(),
        false,
        None,
    );
    assert_eq!(
        closed.error.as_deref(),
        Some("task accepted-task cannot close before delivery, review, and integration evidence")
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["accepted-task"].status,
        "merged"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_integrated_accepts_the_main_tip() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");
    initialize_main(&root);
    let head = current_head(&root);

    let integrated = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        head.clone(),
        "main tip integration".into(),
    );
    assert!(integrated.ok, "{integrated:?}");
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["task"].status, "merged");
    assert_eq!(
        state.task_lifecycle["task"].integration_commit.as_deref(),
        Some(head.as_str())
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_integrated_accepts_a_real_merge_commit_and_the_merged_candidate() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");
    initialize_main(&root);

    git_ok(&root, &["checkout", "-q", "-b", "candidate"]);
    git_ok(&root, &["commit", "--allow-empty", "-q", "-m", "candidate"]);
    let candidate = current_head(&root);
    git_ok(&root, &["checkout", "-q", "main"]);
    git_ok(
        &root,
        &[
            "merge",
            "--no-ff",
            "-q",
            "candidate",
            "-m",
            "merge candidate",
        ],
    );
    let merge_commit = current_head(&root);
    assert_ne!(candidate, merge_commit);
    // Main keeps moving after the merge, so neither the merge commit nor the
    // candidate is the current main tip when integration is recorded.
    git_ok(
        &root,
        &["commit", "--allow-empty", "-q", "-m", "main moves on"],
    );
    let main_tip = current_head(&root);
    assert_eq!(rev_parse(&root, "refs/heads/main"), main_tip);
    assert_ne!(main_tip, merge_commit);

    let via_merge = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        merge_commit.clone(),
        "merge commit integration".into(),
    );
    assert!(via_merge.ok, "{via_merge:?}");
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["task"]
            .integration_commit
            .as_deref(),
        Some(merge_commit.as_str())
    );

    // The same accepted task can be re-recorded with the candidate SHA that
    // the merge brought in, because it is reachable from refs/heads/main.
    {
        let mut state = server.state.lock().unwrap();
        let mut task = state.tasks.remove("task").unwrap();
        task.status = "accepted".into();
        state.tasks.insert(task.id.clone(), task);
    }
    let via_candidate = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        candidate.clone(),
        "candidate SHA integration".into(),
    );
    assert!(via_candidate.ok, "{via_candidate:?}");
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["task"]
            .integration_commit
            .as_deref(),
        Some(candidate.as_str())
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_integrated_rejects_a_commit_not_reachable_from_main_with_an_actionable_error() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");
    initialize_main(&root);
    let main_head = current_head(&root);

    git_ok(&root, &["checkout", "-q", "--orphan", "orphan"]);
    git_ok(&root, &["commit", "--allow-empty", "-q", "-m", "orphan"]);
    let orphan = current_head(&root);
    git_ok(&root, &["checkout", "-q", "main"]);

    let rejected = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        orphan.clone(),
        "orphan integration".into(),
    );
    assert!(!rejected.ok, "{rejected:?}");
    assert_eq!(
        rejected.error.as_deref(),
        Some("TASK_INTEGRATION_COMMIT_MISMATCH")
    );
    assert_eq!(rejected.data["provided"], orphan);
    assert_eq!(rejected.data["main_head"], main_head);
    assert!(
        rejected.data["expected"]
            .as_str()
            .unwrap_or_default()
            .contains("reachable from refs/heads/main"),
        "expected value must name the reachability contract: {rejected:?}"
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["task"].status,
        "accepted"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_integrated_is_correct_when_root_is_not_checked_out_on_main() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    accept_task(&server, "owner", "task");
    initialize_main(&root);
    let main_head = current_head(&root);

    git_ok(&root, &["checkout", "-q", "-b", "topic"]);
    git_ok(&root, &["commit", "--allow-empty", "-q", "-m", "topic"]);
    let topic_head = current_head(&root);
    assert_ne!(topic_head, main_head);
    assert_eq!(rev_parse(&root, "refs/heads/main"), main_head);

    // The recorded commit is a real ancestor of main, so reachability must be
    // judged from refs/heads/main rather than from the current checkout.
    let integrated = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        topic_head.clone(),
        "integration judged from refs/heads/main".into(),
    );
    assert!(
        !integrated.ok,
        "a topic-only commit must not be accepted: {integrated:?}"
    );
    assert_eq!(
        integrated.error.as_deref(),
        Some("TASK_INTEGRATION_COMMIT_MISMATCH")
    );
    assert!(integrated.data["expected"]
        .as_str()
        .unwrap_or_default()
        .contains("reachable from refs/heads/main"));
    assert_eq!(
        server.state.lock().unwrap().tasks["task"].status,
        "accepted"
    );

    let merged = handle_task_integrated(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        main_head.clone(),
        "main ancestor accepted while checked out on topic".into(),
    );
    assert!(merged.ok, "{merged:?}");
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["task"]
            .integration_commit
            .as_deref(),
        Some(main_head.as_str())
    );
    // The checkout must not be moved by the integration record.
    assert_eq!(current_head(&root), topic_head);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn delivery_review_and_exact_main_integration_are_durable() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "outsider", "%outsider");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    assert!(
        handle_task_update(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("verifying".into()),
            None,
        )
        .ok
    );
    assert!(
        handle_task_update(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("reviewed".into()),
            None,
        )
        .ok
    );
    let worktree = candidate_worktree(&root, "task");
    git_ok(&worktree, &["commit", "--allow-empty", "-q", "-m", "candidate"]);
    let candidate = current_head(&worktree);
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("candidate commit and gates passed".into()),
            Some(worktree.to_string_lossy().into_owned()),
        )
        .ok
    );
    let denied = handle_task_review(
        &server,
        "outsider".into(),
        "token-outsider".into(),
        "task".into(),
        true,
        false,
        "outsider review".into(),
    );
    assert!(!denied.ok);
    assert!(
        handle_task_review(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            true,
            false,
            "review gates passed".into(),
        )
        .ok
    );

    git_ok(&root, &["merge", "--ff-only", &candidate]);
    assert!(
        handle_task_integrated(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            candidate,
            "main integration verified".into(),
        )
        .ok
    );
    git_ok(&root, &["worktree", "remove", worktree.to_str().unwrap()]);
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["task"].status, "merged");
    let lifecycle = &state.task_lifecycle["task"];
    assert_eq!(
        lifecycle.delivery_evidence.as_deref(),
        Some("candidate commit and gates passed")
    );
    assert_eq!(lifecycle.reviewer.as_deref(), Some("owner"));
    assert_eq!(
        lifecycle.integration_evidence.as_deref(),
        Some("main integration verified")
    );
    drop(state);
    let replayed = current_state::load(&server.state_path, false).unwrap();
    assert_eq!(replayed.tasks["task"].status, "merged");
    assert_eq!(
        replayed.task_lifecycle["task"]
            .integration_evidence
            .as_deref(),
        Some("main integration verified")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn accepted_task_can_return_to_rework_and_redeliver() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    assert!(create_task(&server, "owner", "task", "feature").ok);
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("first candidate".into()),
            Some("candidate".into()),
        )
        .ok
    );
    assert!(
        handle_task_review(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            true,
            false,
            "accepted".into(),
        )
        .ok
    );
    let direct_verifying = handle_task_update(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        Some("verifying".into()),
        None,
    );
    assert_eq!(
        direct_verifying.error.as_deref(),
        Some("invalid task transition accepted -> verifying")
    );
    let direct_merge = handle_task_update(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        Some("merged".into()),
        None,
    );
    assert_eq!(
        direct_merge.error.as_deref(),
        Some("use collab task review/integrated for integration-owned lifecycle transitions")
    );
    assert!(
        handle_task_update(
            &server,
            "owner".into(),
            "token-owner".into(),
            "task".into(),
            Some("rework".into()),
            Some("address review findings".into()),
        )
        .ok
    );
    for status in ["working", "verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    let redelivery = handle_task_deliver(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task".into(),
        Some("corrected candidate".into()),
        Some("candidate".into()),
    );
    assert!(redelivery.ok, "{}", redelivery.error.unwrap_or_default());
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["task"]
            .delivery_evidence
            .as_deref(),
        Some("corrected candidate")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_lifecycle_public_read_only_delivery_reaches_no_merge_and_owner_close() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "owner", "thread-read-only").ok);
    assert!(create_task(&server, "owner", "read-only-task", "feature").ok);
    let server = Arc::new(server);

    let delivered = dispatch(
        &server,
        Req::TaskDeliver {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "read-only-task".into(),
            evidence: Some("documentation and source audit complete".into()),
            attempt: None,
            worktree: None,
            no_candidate: true,
        },
    );
    assert!(delivered.ok, "{delivered:?}");
    {
        let state = server.state.lock().unwrap();
        let lifecycle = &state.task_lifecycle["read-only-task"];
        assert_eq!(
            lifecycle.delivery_kind,
            crate::server::state::TaskDeliveryKind::ReadOnly
        );
    }

    let reviewed = dispatch(
        &server,
        Req::TaskReview {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "read-only-task".into(),
            accept: true,
            rework: false,
            evidence: "review accepted".into(),
        },
    );
    assert!(reviewed.ok, "{reviewed:?}");
    assert_eq!(reviewed.data["status"], "accepted");
    assert_eq!(reviewed.data["merge_pending"], false);
    assert_eq!(reviewed.data["disposition"], "no_merge_required");
    assert!(
        server
            .state
            .lock()
            .unwrap()
            .pending_merges
            .get("read-only-task")
            .is_none()
    );

    let closed = dispatch(
        &server,
        Req::TaskClose {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "read-only-task".into(),
            force: false,
            reason: None,
        },
    );
    assert!(closed.ok, "{closed:?}");
    assert_eq!(closed.data["disposition"], "no_merge_required");
    let replayed = current_state::load(&server.state_path, false).unwrap();
    drop(server);
    assert_eq!(replayed.tasks["read-only-task"].status, "closed");
    assert_eq!(
        replayed.task_lifecycle["read-only-task"].delivery_kind,
        crate::server::state::TaskDeliveryKind::ReadOnly
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_lifecycle_public_masterless_code_integration_requires_main_reachable_commit() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "owner", "thread-code").ok);
    assert!(create_task(&server, "owner", "code-task", "feature").ok);
    let worktree = candidate_worktree(&root, "code-task");
    git_ok(&worktree, &["commit", "--allow-empty", "-q", "-m", "code candidate"]);
    let candidate = current_head(&worktree);
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "code-task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    let delivered = handle_task_deliver(
        &server,
        "owner".into(),
        "token-owner".into(),
        "code-task".into(),
        Some("code candidate delivered".into()),
        Some(worktree.to_string_lossy().into_owned()),
    );
    assert!(delivered.ok, "{delivered:?}");
    let reviewed = handle_task_review(
        &server,
        "owner".into(),
        "token-owner".into(),
        "code-task".into(),
        true,
        false,
        "code review accepted".into(),
    );
    assert!(reviewed.ok, "{reviewed:?}");
    git_ok(&root, &["worktree", "remove", worktree.to_str().unwrap()]);
    git_ok(&root, &["commit", "--allow-empty", "-q", "-m", "later main"]);
    let main_head = current_head(&root);
    assert_ne!(candidate, main_head);
    let server = Arc::new(server);

    // A supplied commit that is not reachable from refs/heads/main is refused,
    // because the record must describe an actual integration.
    let rejected = dispatch(
        &server,
        Req::TaskIntegrated {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "code-task".into(),
            commit: candidate.clone(),
            evidence: "feature branch sha before it lands on main".into(),
        },
    );
    assert!(!rejected.ok, "{rejected:?}");
    assert_eq!(
        rejected.error.as_deref(),
        Some("TASK_INTEGRATION_COMMIT_MISMATCH")
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["code-task"].status,
        "accepted"
    );

    // A supplied commit already reachable from refs/heads/main is accepted.
    let integrated = dispatch(
        &server,
        Req::TaskIntegrated {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "code-task".into(),
            commit: main_head.clone(),
            evidence: "main head integration recorded".into(),
        },
    );
    assert!(integrated.ok, "{integrated:?}");
    assert_eq!(integrated.data["status"], "merged");
    assert_eq!(
        server.state.lock().unwrap().task_lifecycle["code-task"]
            .integration_evidence
            .as_deref(),
        Some("main head integration recorded")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn task_lifecycle_public_code_delivery_requires_integration_before_close() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "owner", "thread-unresolved").ok);
    assert!(create_task(&server, "owner", "unresolved-task", "feature").ok);
    let server = Arc::new(server);

    let delivered = dispatch(
        &server,
        Req::TaskDeliver {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "unresolved-task".into(),
            evidence: Some("code delivery without a resolvable candidate".into()),
            attempt: None,
            worktree: Some("/definitely-not-a-worktree".into()),
            no_candidate: false,
        },
    );
    assert!(delivered.ok, "{delivered:?}");
    {
        let state = server.state.lock().unwrap();
        let lifecycle = &state.task_lifecycle["unresolved-task"];
        assert_eq!(
            lifecycle.delivery_kind,
            crate::server::state::TaskDeliveryKind::Code
        );
    }

    let reviewed = dispatch(
        &server,
        Req::TaskReview {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "unresolved-task".into(),
            accept: true,
            rework: false,
            evidence: "review accepted".into(),
        },
    );
    assert!(reviewed.ok, "{reviewed:?}");
    assert_eq!(reviewed.data["disposition"], "integration_required");
    let closed = dispatch(
        &server,
        Req::TaskClose {
            worker_id: "owner".into(),
            token: "token-owner".into(),
            task_id: "unresolved-task".into(),
            force: false,
            reason: None,
        },
    );
    // An accepted code task with no pending merge still cannot close before
    // integration; it is never treated as read-only.
    assert!(!closed.ok, "{closed:?}");
    assert!(
        closed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("must be merged by its owner before close")),
        "{closed:?}"
    );
    std::fs::remove_dir_all(root).ok();
}

/// Old current-state files can still carry the retired `delivery_commit` and
/// `pending_merges.*.candidate_commit` fields. Loading must ignore the retired
/// delivery candidate but keep every other task, lifecycle, pending-merge,
/// resource and message fact intact.
#[test]
fn old_format_candidate_fields_load_without_losing_business_state() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "master", "%master");
    promote_master(&server, "master", "user approved legacy candidate state test");
    initialize_main(&root);
    let worktree = root.join("legacy-wt").display().to_string();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "legacy-task".into(),
            owner: "owner".into(),
            created_by: "owner".into(),
            feature_id: None,
            worktree_path: Some(worktree.clone()),
            branch: Some("feature".into()),
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
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "owner".into(),
                "token-owner".into(),
                "legacy-task".into(),
                Some(status.into()),
                None,
            )
            .ok
        );
    }
    let delivered = handle_task_deliver(
        &server,
        "owner".into(),
        "token-owner".into(),
        "legacy-task".into(),
        Some("legacy code delivery".into()),
        Some(worktree.clone()),
    );
    assert!(delivered.ok, "{delivered:?}");
    let reviewed = handle_task_review(
        &server,
        "master".into(),
        "token-master".into(),
        "legacy-task".into(),
        true,
        false,
        "legacy review accepted".into(),
    );
    assert!(reviewed.ok, "{reviewed:?}");
    assert_eq!(reviewed.data["merge_pending"], json!(true));
    server.commit(&[Event::Sent {
        msg: Message {
            id: "legacy-msg".into(),
            from: "owner".into(),
            to: "master".into(),
            mtype: "notify".into(),
            subject: Some("legacy fact".into()),
            body: "legacy message body".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    }]);

    // Inject the retired candidate fields exactly as an old writer emitted
    // them, then reload the document through the current loader.
    let path = server.state_path.clone();
    let mut document: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    document["state"]["task_lifecycle"]["legacy-task"]["delivery_commit"] =
        json!("legacy-candidate");
    document["state"]["pending_merges"]["legacy-task"]["candidate_commit"] =
        json!("legacy-candidate");
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    let replayed = current_state::load(&path, false).unwrap();

    // The retired lifecycle candidate field is ignored by the current type.
    assert_eq!(
        replayed.pending_merges["legacy-task"].candidate_commit,
        Some("legacy-candidate".to_string())
    );

    // Business facts must be preserved.
    assert_eq!(replayed.tasks["legacy-task"].status, "accepted");
    assert_eq!(
        replayed.tasks["legacy-task"].worktree_path.as_deref(),
        Some(worktree.as_str())
    );
    assert_eq!(
        replayed.tasks["legacy-task"].branch.as_deref(),
        Some("feature")
    );
    let lifecycle = &replayed.task_lifecycle["legacy-task"];
    assert_eq!(
        lifecycle.delivery_kind,
        crate::server::state::TaskDeliveryKind::Code
    );
    assert_eq!(
        lifecycle.delivery_evidence.as_deref(),
        Some("legacy code delivery")
    );
    assert!(lifecycle.delivered_ms.is_some());
    assert_eq!(lifecycle.reviewer.as_deref(), Some("master"));
    assert_eq!(
        lifecycle.review_evidence.as_deref(),
        Some("legacy review accepted")
    );
    let request = &replayed.pending_merges["legacy-task"];
    assert_eq!(request.owner, "owner");
    assert_eq!(request.requested_by, "master");
    assert!(request.requested_ms > 0);
    assert_eq!(replayed.msgs["legacy-msg"].body, "legacy message body");
    assert_eq!(replayed.msgs["legacy-msg"].state, "pending");
    std::fs::remove_dir_all(root).ok();
}

/// A publisher-owned pending task with no bound worktree can be cancelled
/// through the owner-local update path. The `pending -> cancelled` edge is the
/// only legal unaccepted cancellation and must persist in current state.
#[test]
fn pending_task_without_worktree_transitions_to_cancelled() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "publisher", "thread-pending-cancel").ok);
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "pending-cancel".into(),
            owner: "publisher".into(),
            created_by: "publisher".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: default_priority(),
            status: "pending".into(),
            next_step: None,
            wait: None,
            isolation_mode: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);

    let cancelled = handle_task_update(
        &server,
        "publisher".into(),
        "token-publisher".into(),
        "pending-cancel".into(),
        Some("cancelled".into()),
        None,
    );
    assert!(cancelled.ok, "{cancelled:?}");
    assert_eq!(cancelled.data["status"], "cancelled");
    assert_eq!(
        server.state.lock().unwrap().tasks["pending-cancel"].status,
        "cancelled"
    );

    let replayed = current_state::load(&server.state_path, false).unwrap();
    assert_eq!(replayed.tasks["pending-cancel"].status, "cancelled");
    std::fs::remove_dir_all(root).ok();
}

/// Publisher-owned pending work remains claimable by its publisher, and once
/// claimed is no longer claimable by another registered peer at the advanced
/// revision. This guards `board_claim`, which the fix intentionally leaves
/// unchanged.
#[test]
fn publisher_claims_pending_task_and_non_publisher_cannot_reclaim() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "publisher", "thread-pub-claim").ok);
    assert!(register_synthetic_peer(&server, "other", "thread-other-claim").ok);

    let published = handle_board_command(
        &server,
        "publisher".into(),
        "token-publisher".into(),
        crate::board::BoardCommand::Publish {
            id: "publisher-pending".into(),
            title: "publisher pending".into(),
            description: "claim regression".into(),
            delivery_condition: "claim works".into(),
            test_condition: "green".into(),
            priority: "p1".into(),
            work_kind: None,
            parent_task: None,
            prefer_role: None,
            executor: None,
            reviewer: None,
            depends_on: Vec::new(),
        },
    );
    assert!(published.ok, "{published:?}");
    assert_eq!(published.data["task"]["status"], "pending");
    assert_eq!(published.data["task"]["owner"], "publisher");

    let claimed = handle_board_command(
        &server,
        "publisher".into(),
        "token-publisher".into(),
        crate::board::BoardCommand::Claim {
            id: "publisher-pending".into(),
            expected_revision: 1,
        },
    );
    assert!(claimed.ok, "{claimed:?}");
    assert_eq!(claimed.data["task"]["status"], "working");
    assert_eq!(claimed.data["task"]["owner"], "publisher");

    let non_publisher = handle_board_command(
        &server,
        "other".into(),
        "token-other".into(),
        crate::board::BoardCommand::Claim {
            id: "publisher-pending".into(),
            expected_revision: 2,
        },
    );
    assert!(!non_publisher.ok, "{non_publisher:?}");
    assert_eq!(
        non_publisher.error.as_deref(),
        Some("BOARD_TASK_NOT_PENDING: only pending publisher-owned work may be claimed")
    );
    std::fs::remove_dir_all(root).ok();
}

/// A private (non-public) pending task owned by its creator is claimable only
/// by its publisher. `board_claim` rejects every other actor with
/// `BOARD_PRIVATE_TASK` before the pending/ownership checks. The task is seeded
/// directly so the claim entry itself is exercised; this guards the privacy
/// branch that the two-line lifecycle fix intentionally leaves unchanged.
#[test]
fn non_publisher_cannot_claim_private_pending_task() {
    let (server, root) = test_server();
    assert!(register_synthetic_peer(&server, "publisher", "thread-private-claim").ok);
    assert!(register_synthetic_peer(&server, "other", "thread-private-other").ok);
    let now = now_ms();
    server.commit(&[
        Event::TaskCreated {
            task: TaskRec {
                id: "private-pending".into(),
                owner: "publisher".into(),
                created_by: "publisher".into(),
                feature_id: None,
                worktree_path: None,
                branch: None,
                base_commit: None,
                priority: default_priority(),
                status: "pending".into(),
                next_step: None,
                wait: None,
                isolation_mode: None,
                created_ms: now,
                updated_ms: now,
            },
        },
        Event::BoardDetailsChanged {
            task_id: "private-pending".into(),
            details: crate::board::BoardTaskDetails {
                public_visibility: false,
                revision: 1,
                ..Default::default()
            },
        },
    ]);

    let rejected = board_claim(
        &server,
        "other".into(),
        "token-other".into(),
        "private-pending".into(),
        1,
    );
    assert!(
        !rejected.ok,
        "non-publisher must not claim private work: {rejected:?}"
    );
    assert_eq!(
        rejected.error.as_deref(),
        Some("BOARD_PRIVATE_TASK: private queue work is claimable only by its publisher")
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["private-pending"].status,
        "pending",
        "a rejected private claim must not mutate the task"
    );

    let claimed = board_claim(
        &server,
        "publisher".into(),
        "token-publisher".into(),
        "private-pending".into(),
        1,
    );
    assert!(
        claimed.ok,
        "publisher must claim own private pending work: {claimed:?}"
    );
    assert_eq!(claimed.data["task"]["status"], "working");
    assert_eq!(claimed.data["task"]["owner"], "publisher");
    assert_eq!(
        server.state.lock().unwrap().tasks["private-pending"].status,
        "working"
    );
    std::fs::remove_dir_all(root).ok();
}
