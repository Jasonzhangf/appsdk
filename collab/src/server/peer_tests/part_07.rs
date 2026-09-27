#[test]
fn finalize_requires_a_closed_task_and_authorized_caller() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "owner", "open-task", "feature").ok);
    let not_closed = handle_task_finalize_cleanup(
        &server,
        "owner".into(),
        "token-owner".into(),
        "open-task".into(),
    );
    assert!(!not_closed.ok);
    assert!(
        not_closed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("must be closed")),
        "{not_closed:?}"
    );
    drop(server);
    std::fs::remove_dir_all(root).ok();
}

/// Build a real project with a clean, merged feature worktree/branch, a
/// force-closed holder task that declared them, and an armed default lease on
/// the holder. This is the fixture the ownership and lease regressions use.
fn force_closed_real_worktree_holder(server: &Server, root: &Path) -> (String, String) {
    let playground = root.join("playground");
    std::fs::create_dir_all(&playground).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(root)
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
    assert!(git(&["branch", "-M", "main"]).status.success());
    assert!(git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "feature",
        "playground/held-wt"
    ])
    .status
    .success());
    std::fs::write(root.join("playground/held-wt/feature.txt"), "work\n").unwrap();
    assert!(git(&["-C", "playground/held-wt", "add", "feature.txt"])
        .status
        .success());
    assert!(
        git(&["-C", "playground/held-wt", "commit", "-q", "-m", "feature"])
            .status
            .success()
    );
    // Merged into main so cleanup is otherwise allowed to remove it.
    assert!(git(&["merge", "-q", "feature"]).status.success());

    register(server, "holder", "%holder");
    register(server, "master", "%master");
    promote_master(server, "master", "user approved finalize ownership test");
    let now = now_ms();
    let canonical_wt = root
        .join("playground/held-wt")
        .canonicalize()
        .unwrap()
        .display()
        .to_string();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "held".into(),
            owner: "holder".into(),
            created_by: "holder".into(),
            feature_id: None,
            worktree_path: Some(canonical_wt.clone()),
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
    let forced = handle_task_close(
        server,
        "master".into(),
        "token-master".into(),
        "held".into(),
        true,
        Some("holder abandoned the worktree; force closing".into()),
    );
    assert!(forced.ok, "{}", forced.error.unwrap_or_default());
    assert_eq!(forced.data["cleanup"]["result"], "unverified");
    let lease_armed = server.state.lock().unwrap().notification_subscriptions
        [&crate::server::mailbox::default_direct_message_id("holder")]
        .status
        .clone();
    (canonical_wt, lease_armed)
}

#[test]
fn finalize_refuses_a_worktree_taken_over_by_another_open_task() {
    let (server, root) = test_server();
    let (worktree, _) = force_closed_real_worktree_holder(&server, &root);
    // A peer legitimately claims the same worktree after the force close,
    // because the closed task no longer counts as an active resource holder.
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "taken-over", "peer-feature").ok);
    {
        let mut state = server.state.lock().unwrap();
        let mut taken = state.tasks["taken-over"].clone();
        taken.worktree_path = Some(worktree.clone());
        taken.branch = Some("feature".into());
        state.tasks.insert("taken-over".into(), taken);
    }

    let refused = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(!refused.ok, "{refused:?}");
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("CLEANUP_FINALIZE_REFUSED")),
        "{refused:?}"
    );
    assert_eq!(refused.data["competing_task"], "taken-over");
    assert_eq!(refused.data["finalized"], false);
    // The competing task's resource must still exist untouched.
    assert!(
        root.join("playground/held-wt").is_dir(),
        "finalize must not destroy a resource another open task owns"
    );
    assert!(
        Command::new("git")
            .current_dir(&root)
            .args(["rev-parse", "--verify", "refs/heads/feature"])
            .output()
            .unwrap()
            .status
            .success(),
        "finalize must not delete a branch another open task owns"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn finalize_removes_the_worktree_when_the_closed_task_still_owns_it() {
    let (server, root) = test_server();
    let (worktree, _) = force_closed_real_worktree_holder(&server, &root);
    let finalized = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(finalized.ok, "{}", finalized.error.unwrap_or_default());
    assert_eq!(finalized.data["cleanup"]["result"], "verified");
    assert_eq!(finalized.data["cleanup"]["worktree"], worktree);
    assert!(!root.join("playground/held-wt").exists());
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
fn finalize_keeps_the_owner_default_lease_armed_on_last_responsibility() {
    let (server, root) = test_server();
    let (_, lease_armed) = force_closed_real_worktree_holder(&server, &root);
    assert_eq!(
        lease_armed, "armed",
        "registration must arm the owner's default lease for this test to mean anything"
    );
    let finalized = handle_task_finalize_cleanup(
        &server,
        "master".into(),
        "token-master".into(),
        "held".into(),
    );
    assert!(finalized.ok, "{}", finalized.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    let lease = &state.notification_subscriptions
        [&crate::server::mailbox::default_direct_message_id("holder")];
    assert_eq!(
        lease.status, "armed",
        "finalize must keep the owner's default direct-message lease armed"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn non_master_force_close_is_rejected() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    register(&server, "peer", "%peer");
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "stuck".into(),
            owner: "owner".into(),
            created_by: "owner".into(),
            feature_id: None,
            worktree_path: Some("playground/stuck".into()),
            branch: Some("codex/stuck".into()),
            base_commit: Some("base".into()),
            priority: default_priority(),
            status: "working".into(),
            next_step: None,
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
    let resp = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "stuck".into(),
        true,
        Some("not authorized".into()),
    );
    assert!(!resp.ok);
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["stuck"].status, "working");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn orphan_force_close_defers_when_owner_appserver_probe_is_unknown() {
    let (mut server, root) = test_server();
    register(&server, "owner", "thread-owner");
    register(&server, "peer", "thread-peer");
    assert!(create_task(&server, "owner", "working", "feature").ok);
    server.appserver_candidate_check = Arc::new(|candidate| {
        if candidate.thread_id == "thread-owner" {
            Err("owner route probe unknown".into())
        } else {
            Ok(test_appserver_transport(&candidate.thread_id))
        }
    });
    let resp = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "working".into(),
        true,
        Some("owner route probe is unknown; defer orphan close".into()),
    );
    assert!(!resp.ok);
    assert!(resp.error.as_deref().is_some_and(|error| {
        error.contains("not authorized")
            || error.contains("owner route probe is unknown")
            || error.contains("live master")
    }));
    assert_eq!(
        server.state.lock().unwrap().tasks["working"].status,
        "working"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn orphan_force_close_refuses_a_cold_owner_because_cold_is_not_dead() {
    let (mut server, root) = test_server();
    register(&server, "owner", "thread-owner");
    register(&server, "peer", "thread-peer");
    assert!(create_task(&server, "owner", "orphan", "feature").ok);
    server.appserver_thread_status = Arc::new(|_, thread_id| {
        Ok(serde_json::json!({
            "thread": {
                "id": thread_id,
                "status": {"type": if thread_id == "thread-owner" {"notLoaded"} else {"idle"}},
                "canAcceptDirectInput": thread_id != "thread-owner"
            }
        }))
    });

    // A cold thread is not evidence that its owner is dead: the owner may
    // simply be idle on the endpoint, so force close stays refused.
    let refused = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "orphan".into(),
        true,
        Some("owner native thread is not loaded".into()),
    );
    assert!(!refused.ok, "{refused:?}");
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|error| error.contains("not authorized")),
        "{refused:?}"
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["orphan"].status,
        "working"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn owner_force_close_when_no_live_master_is_allowed() {
    let (server, root) = test_server();
    register(&server, "owner", "%owner");
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "orphan".into(),
            owner: "owner".into(),
            created_by: "owner".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: default_priority(),
            status: "blocked".into(),
            next_step: None,
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
    let resp = handle_task_close(
        &server,
        "owner".into(),
        "token-owner".into(),
        "orphan".into(),
        true,
        Some("master unreachable; owner closes".into()),
    );
    assert!(resp.ok, "{}", resp.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["orphan"].status, "closed");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn registered_peer_force_closes_orphaned_owner_with_no_live_master() {
    let (server, root) = test_server();
    register(&server, "owner", "thread-owner");
    register(&server, "peer", "thread-peer");
    assert!(create_task(&server, "owner", "orphan", "feature").ok);
    kill_registered_worker_pane(&server, "owner");
    let resp = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "orphan".into(),
        true,
        Some("owner route lost; no live master; peer closes orphan".into()),
    );
    assert!(resp.ok, "{}", resp.error.unwrap_or_default());
    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["orphan"].status, "closed");
    assert_eq!(
        state.cleanup_receipts["orphan"].manual_reason.as_deref(),
        Some("owner route lost; no live master; peer closes orphan"),
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn repeated_orphan_force_close_is_idempotent_after_journal_replay() {
    let (server, root) = test_server();
    register(&server, "owner", "thread-owner");
    register(&server, "peer", "thread-peer");
    assert!(create_task(&server, "owner", "orphan", "feature").ok);
    kill_registered_worker_pane(&server, "owner");
    let reason = "owner route lost; replay closes the same orphan";
    let first = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "orphan".into(),
        true,
        Some(reason.into()),
    );
    assert!(first.ok, "{}", first.error.unwrap_or_default());
    let receipt_id = first.data["receipt_id"].as_str().unwrap().to_owned();
    assert_eq!(first.data["cleanup"]["result"], "unverified");
    assert_eq!(
        first.data["next_action"],
        "manual close recorded; worktree/branch cleanup remains unverified"
    );
    let task_updated_ms = server.state.lock().unwrap().tasks["orphan"].updated_ms;
    let journal_path = root.join(".agent-collab/server/journal.jsonl");
    let journal_after_first = std::fs::read_to_string(&journal_path).unwrap();
    *server.state.lock().unwrap() = replay(&root).unwrap();

    let second = handle_task_close(
        &server,
        "peer".into(),
        "token-peer".into(),
        "orphan".into(),
        true,
        Some(reason.into()),
    );
    assert!(second.ok, "{}", second.error.unwrap_or_default());
    assert_eq!(second.data["idempotent"], true);
    assert_eq!(second.data["receipt_id"], receipt_id);
    assert_eq!(second.data["cleanup"]["result"], "unverified");
    assert_eq!(
        second.data["next_action"],
        "manual close recorded; worktree/branch cleanup remains unverified"
    );
    assert_eq!(
        server.state.lock().unwrap().tasks["orphan"].updated_ms,
        task_updated_ms
    );
    assert_eq!(
        std::fs::read_to_string(&journal_path).unwrap(),
        journal_after_first
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn legacy_manual_cleanup_receipt_replays_as_unverified() {
    let receipt: CleanupReceipt = serde_json::from_value(json!({
        "id": "cleanup-manual-legacy-1",
        "task_id": "legacy-task",
        "worktree_path": "playground/legacy-task",
        "branch": "codex/legacy-task",
        "verified_ms": 1,
        "manual_reason": "legacy force close without verified cleanup",
    }))
    .unwrap();
    assert_eq!(receipt.verification, CleanupVerification::Unverified);
}

#[test]
fn worktree_claim_requires_cleanup_and_cannot_cancel() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "task".into(),
            owner: "peer".into(),
            created_by: "peer".into(),
            feature_id: Some("feature".into()),
            worktree_path: Some("playground/task-wt".into()),
            branch: Some("codex/task-wt".into()),
            base_commit: Some("base".into()),
            priority: default_priority(),
            status: "working".into(),
            next_step: None,
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
    let cancelled = handle_task_update(
        &server,
        "peer".into(),
        "token-peer".into(),
        "task".into(),
        Some("cancelled".into()),
        None,
    );
    assert!(!cancelled.ok);
    assert_eq!(
        cancelled.error.as_deref(),
        Some(
            "CLEANUP_REQUIRED_BEFORE_CANCEL: task owns a worktree; close only after merged cleanup"
        )
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn merged_worktree_without_cleanup_receipt_fails_audit() {
    let (server, root) = test_server();
    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "merged-task".into(),
            owner: "peer".into(),
            created_by: "peer".into(),
            feature_id: None,
            worktree_path: Some("playground/merged-wt".into()),
            branch: Some("codex/merged-wt".into()),
            base_commit: None,
            priority: default_priority(),
            status: "merged".into(),
            next_step: None,
            wait: None,
            created_ms: now,
            updated_ms: now,
        },
    }]);
    let issues = migration_issues(&server, &server.state.lock().unwrap());
    assert!(issues
        .iter()
        .any(|issue| issue.starts_with("TASK_CLEANUP_INCOMPLETE:merged-task:")));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn conflict_is_durable_and_wait_targets_resource_holder() {
    let (server, root) = test_server();
    register(&server, "holder", "%holder");
    register(&server, "waiter", "%waiter");
    assert!(create_task(&server, "holder", "held", "shared-feature").ok);
    let conflict = create_task(&server, "waiter", "waiting", "shared-feature");
    assert!(!conflict.ok);
    assert_eq!(conflict.error.as_deref(), Some("TASK_RESOURCE_CONFLICT"));
    assert_eq!(conflict.data["responsible_actor"], "holder");
    assert_eq!(server.state.lock().unwrap().msgs.len(), 0);
    assert_eq!(
        conflict.data["notification"],
        "none; use explicit sendmessage when coordination is needed"
    );

    let waiting = handle_task_wait(
        &server,
        "waiter".into(),
        "token-waiter".into(),
        "waiting".into(),
        "held".into(),
    );
    assert!(waiting.ok);
    let state = server.state.lock().unwrap();
    let wait = state.tasks["waiting"].wait.as_ref().unwrap();
    assert_eq!(wait.waiter, "waiter");
    assert_eq!(wait.responsible_actor, "holder");
    assert!(wait.deadline_ms > now_ms());
    assert!(wait.resume_on.contains(&"resource_released".into()));
    assert_eq!(wait.escalation, "resource_owner_and_waiter_recheck");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn self_registration_cannot_duplicate_the_owners_active_task() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "authoritative", "shared-feature").ok);

    let duplicate = create_task(&server, "peer", "duplicate", "shared-feature");
    assert!(!duplicate.ok, "{duplicate:?}");
    let error = duplicate.error.as_deref().unwrap_or_default();
    assert!(
        error.starts_with("TASK_OWNER_ALREADY_HOLDS_RESOURCE:"),
        "error={error}"
    );
    assert!(error.contains("authoritative"), "error={error}");
    assert_ne!(error, "TASK_RESOURCE_CONFLICT");
    let state = server.state.lock().unwrap();
    assert!(!state.tasks.contains_key("duplicate"));
    assert_eq!(state.tasks["authoritative"].status, "working");
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn scheduler_assignment_cannot_self_conflict_with_a_duplicate_registration() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    let now = now_ms();
    server.commit(&[
        Event::TaskCreated {
            task: TaskRec {
                id: "task-scheduler-authoritative".into(),
                owner: "peer".into(),
                created_by: "master".into(),
                feature_id: Some("shared-feature".into()),
                worktree_path: Some("playground/shared".into()),
                branch: Some("fix/shared".into()),
                base_commit: None,
                priority: default_priority(),
                status: "assigned".into(),
                next_step: None,
                wait: None,
                created_ms: now,
                updated_ms: now,
            },
        },
        Event::SchedulerAdmission {
            admission: crate::server::state::SchedulerAdmissionRecord {
                request_id: "request-authoritative".into(),
                decision: "use-registered-peer".into(),
                worker_id: "master".into(),
                managed_subagent_id: None,
                message_id: "scheduler-request-authoritative".into(),
                task_id: "task-scheduler-authoritative".into(),
                status: "succeeded".into(),
                error: None,
                created_ms: now,
                updated_ms: now,
            },
        },
    ]);

    let duplicate = create_task(&server, "peer", "duplicate", "shared-feature");
    assert!(!duplicate.ok, "{duplicate:?}");
    let error = duplicate.error.as_deref().unwrap_or_default();
    assert!(
        error.starts_with("TASK_OWNER_ALREADY_HOLDS_RESOURCE:"),
        "error={error}"
    );
    assert!(
        error.contains("task-scheduler-authoritative"),
        "error={error}"
    );
    assert_ne!(error, "TASK_RESOURCE_CONFLICT");
    let state = server.state.lock().unwrap();
    assert!(!state.tasks.contains_key("duplicate"));
    assert_eq!(
        state.tasks["task-scheduler-authoritative"].status,
        "assigned"
    );
    assert!(
        state.tasks.values().all(|task| task.next_step.as_deref()
            != Some("RESOURCE_CONFLICT=task-scheduler-authoritative")),
        "no task may block against its own scheduler assignment"
    );
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn holder_close_persists_release_only_for_waiter() {
    let (server, root) = test_server();
    register(&server, "holder", "%holder");
    register(&server, "waiter", "%waiter");
    assert!(create_task(&server, "holder", "held", "shared-feature").ok);
    assert!(!create_task(&server, "waiter", "waiting", "shared-feature").ok);
    assert!(
        handle_task_wait(
            &server,
            "waiter".into(),
            "token-waiter".into(),
            "waiting".into(),
            "held".into(),
        )
        .ok
    );
    assert!(
        handle_notification_subscribe(
            &server,
            "waiter".into(),
            "token-waiter".into(),
            "resource-released".into(),
            Some("held".into()),
            None,
            Vec::new(),
            None,
            1,
            60,
        )
        .ok
    );
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                &server,
                "holder".into(),
                "token-holder".into(),
                "held".into(),
                Some(status.into()),
                Some(format!("continue {status}")),
            )
            .ok
        );
    }
    assert!(
        handle_task_deliver(
            &server,
            "holder".into(),
            "token-holder".into(),
            "held".into(),
            Some("candidate verified".into()),
            Some("/tmp/holder-worktree".into()),
        )
        .ok
    );
    assert!(
        handle_task_review(
            &server,
            "holder".into(),
            "token-holder".into(),
            "held".into(),
            true,
            false,
            "reviewed candidate".into(),
        )
        .ok
    );
    initialize_main(&root);
    assert!(
        handle_task_integrated(
            &server,
            "holder".into(),
            "token-holder".into(),
            "held".into(),
            current_head(&root),
            "main verified".into(),
        )
        .ok
    );
    assert!(
        handle_task_close(
            &server,
            "holder".into(),
            "token-holder".into(),
            "held".into(),
            false,
            None,
        )
        .ok
    );

    let state = server.state.lock().unwrap();
    assert_eq!(state.tasks["waiting"].status, "blocked");
    assert!(state.tasks["waiting"].wait.is_none());
    assert!(state.tasks["waiting"]
        .next_step
        .as_deref()
        .unwrap()
        .starts_with("RESOURCE_RELEASED=held"));
    let releases: Vec<&Message> = state
        .msgs
        .values()
        .filter(|message| message.body.starts_with("RESOURCE_RELEASED "))
        .collect();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].to, "waiter");
    assert!(state
        .msgs
        .values()
        .all(|message| !message.body.starts_with("TASK_CLOSED ")));
    drop(state);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn direct_two_peer_and_three_peer_wait_cycles_fail_closed() {
    let (server, root) = test_server();
    for (id, thread) in [("a", "%a"), ("b", "%b"), ("c", "%c")] {
        register(&server, id, thread);
    }
    assert!(create_task(&server, "a", "a-task", "a-feature").ok);
    let direct = handle_task_wait(
        &server,
        "a".into(),
        "token-a".into(),
        "a-task".into(),
        "a-task".into(),
    );
    assert!(!direct.ok);
    assert_eq!(direct.error.as_deref(), Some("WAIT_CYCLE_DETECTED"));

    let now = now_ms();
    let make = |id: &str, owner: &str, waiting_for: Option<&str>| TaskRec {
        id: id.into(),
        owner: owner.into(),
        created_by: owner.into(),
        feature_id: Some("shared".into()),
        worktree_path: None,
        branch: None,
        base_commit: None,
        priority: default_priority(),
        status: if waiting_for.is_some() {
            "waiting"
        } else {
            "blocked"
        }
        .into(),
        next_step: None,
        wait: waiting_for.map(|blocking| WaitSpec {
            waiter: owner.into(),
            waiting_for: blocking.into(),
            responsible_actor: "a".into(),
            reason: "resource_conflict".into(),
            deadline_ms: now + 60_000,
            resume_on: vec!["resource_released".into()],
            escalation: "resource_owner_and_waiter_recheck".into(),
        }),
        created_ms: now,
        updated_ms: now,
    };
    server.commit(&[
        Event::TaskUpdated {
            task: make("a-task", "a", None),
        },
        Event::TaskCreated {
            task: make("b-task", "b", Some("a-task")),
        },
    ]);
    let two = handle_task_wait(
        &server,
        "a".into(),
        "token-a".into(),
        "a-task".into(),
        "b-task".into(),
    );
    assert_eq!(two.error.as_deref(), Some("WAIT_CYCLE_DETECTED"));

    server.commit(&[
        Event::TaskUpdated {
            task: make("b-task", "b", Some("c-task")),
        },
        Event::TaskCreated {
            task: make("c-task", "c", Some("a-task")),
        },
    ]);
    let three = handle_task_wait(
        &server,
        "a".into(),
        "token-a".into(),
        "a-task".into(),
        "b-task".into(),
    );
    assert_eq!(three.error.as_deref(), Some("WAIT_CYCLE_DETECTED"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn terminal_or_delivered_task_cannot_wait() {
    let (server, root) = test_server();
    register(&server, "a", "%a");
    register(&server, "b", "%b");
    assert!(create_task(&server, "a", "a-task", "a-feature").ok);
    assert!(create_task(&server, "b", "b-task", "b-feature").ok);
    server
        .state
        .lock()
        .unwrap()
        .tasks
        .get_mut("a-task")
        .unwrap()
        .status = "delivered".into();
    let response = handle_task_wait(
        &server,
        "a".into(),
        "token-a".into(),
        "a-task".into(),
        "b-task".into(),
    );
    assert!(!response.ok);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn peer_migration_freezes_snapshot_and_resumes_after_verify() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(handle_migration_inspect(&server, "peer".into(), "token-peer".into()).ok);
    assert!(handle_migration_plan(&server, "peer".into(), "token-peer".into()).ok);
    let applied = handle_migration_apply(&server, "peer".into(), "token-peer".into());
    assert!(applied.ok);
    assert!(applied.data["admission_frozen"].as_bool().unwrap());
    let verified = handle_migration_verify(&server, "peer".into(), "token-peer".into());
    assert!(verified.ok);
    assert!(verified.data["verified"].as_bool().unwrap());
    assert!(!server.state.lock().unwrap().admission_frozen());
    let repeated = handle_migration_verify(&server, "peer".into(), "token-peer".into());
    assert!(repeated.ok);
    assert_eq!(repeated.data["verified"], true);
    assert_eq!(repeated.data["idempotent"], true);
    assert_eq!(repeated.data["resumed"], false);
    assert!(repeated.data["next"]
        .as_str()
        .unwrap()
        .contains("do not rerun"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn migration_transaction_lease_rejects_second_peer() {
    let (server, root) = test_server();
    register(&server, "peer-a", "%peer-a");
    register(&server, "peer-b", "%peer-b");
    assert!(handle_migration_plan(&server, "peer-a".into(), "token-peer-a".into()).ok);
    let second = handle_migration_plan(&server, "peer-b".into(), "token-peer-b".into());
    assert!(!second.ok);
    assert_eq!(
        second.error.as_deref(),
        Some("MIGRATION_TRANSACTION_HELD_BY_ANOTHER_PEER")
    );
    assert_eq!(second.data["holder"], "peer-a");
    assert_eq!(second.data["requester"], "peer-b");
    assert_eq!(second.data["retry_allowed"], false);
    assert!(second.data["next"]
        .as_str()
        .unwrap()
        .contains("do not retry"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn migration_verify_rejection_exposes_current_state_and_stops_retry() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    let response = handle_migration_verify(&server, "peer".into(), "token-peer".into());
    assert!(!response.ok);
    assert_eq!(
        response.error.as_deref(),
        Some("no migration record to verify")
    );
    assert_eq!(response.data["retry_allowed"], false);
    assert!(response.data["next"].as_str().unwrap().contains("inspect"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn migration_rejects_wait_without_matching_active_resource_holder() {
    let (server, root) = test_server();
    register(&server, "holder", "%holder");
    register(&server, "waiter", "%waiter");
    assert!(create_task(&server, "holder", "held", "shared-feature").ok);
    assert!(!create_task(&server, "waiter", "waiting", "shared-feature").ok);
    assert!(
        handle_task_wait(
            &server,
            "waiter".into(),
            "token-waiter".into(),
            "waiting".into(),
            "held".into(),
        )
        .ok
    );
    server
        .state
        .lock()
        .unwrap()
        .tasks
        .get_mut("held")
        .unwrap()
        .status = "closed".into();

    let inspected = handle_migration_inspect(&server, "waiter".into(), "token-waiter".into());
    assert!(inspected.ok);
    assert!(inspected.data["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue
            .as_str()
            .unwrap()
            .contains("inactive blocking task held")));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn changed_migration_snapshot_remains_frozen() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(handle_migration_plan(&server, "peer".into(), "token-peer".into()).ok);
    assert!(handle_migration_apply(&server, "peer".into(), "token-peer".into()).ok);

    let now = now_ms();
    server.commit(&[Event::TaskCreated {
        task: TaskRec {
            id: "tampered".into(),
            owner: "peer".into(),
            created_by: "peer".into(),
            feature_id: Some("tampered".into()),
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
    }]);
    let verified = handle_migration_verify(&server, "peer".into(), "token-peer".into());
    assert!(verified.ok);
    assert!(!verified.data["verified"].as_bool().unwrap());
    assert!(verified.data["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue.as_str().unwrap().contains("snapshot hash mismatch")));
    assert!(server.state.lock().unwrap().admission_frozen());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn migration_freeze_rejects_mutations_but_allows_rebind_and_reads() {
    let (server, root) = test_server();
    register(&server, "peer", "%peer");
    assert!(create_task(&server, "peer", "task", "feature").ok);
    assert!(handle_migration_plan(&server, "peer".into(), "token-peer".into()).ok);
    assert!(handle_migration_apply(&server, "peer".into(), "token-peer".into()).ok);
    let server = Arc::new(server);

    let mutations = vec![
        Req::Send {
            from: "peer".into(),
            worker_id: Some("peer".into()),
            token: Some("token-peer".into()),
            command: Some(send_command(&root, "peer-freeze")),
            to: "peer".into(),
            mtype: "notify".into(),
            subject: Some("release".into()),
            body: "RESOURCE_RELEASED feature".into(),
            in_reply_to: None,
            delivery: "immediate".into(),
        },
        Req::Poll {
            worker_id: "peer".into(),
            token: "token-peer".into(),
            timeout_ms: 1,
            receive_id: None,
        },
        Req::Ack {
            worker_id: "peer".into(),
            token: "token-peer".into(),
            ids: vec!["message".into()],
        },
        Req::TaskUpdate {
            worker_id: "peer".into(),
            token: "token-peer".into(),
            task_id: "task".into(),
            status: Some("verifying".into()),
            next_step: None,
        },
        Req::MigrationPlan {
            worker_id: "peer".into(),
            token: "token-peer".into(),
        },
        Req::MigrationApply {
            worker_id: "peer".into(),
            token: "token-peer".into(),
        },
    ];
    for request in mutations {
        let response = dispatch(&server, request);
        assert_eq!(
            response.error.as_deref(),
            Some(
                "MIGRATION_ADMISSION_FROZEN: only identity rebind, read queries, daemon restart, and migration verify are allowed"
            )
        );
    }

    let read = dispatch(
        &server,
        Req::TaskStatus {
            task_id: Some("task".into()),
        },
    );
    assert!(read.ok);
    let endpoint = server.state.lock().unwrap().workers["peer"]
        .transport
        .as_ref()
        .unwrap()
        .tmux_endpoint
        .clone()
        .unwrap();
    let rebound = dispatch(
        &server,
        Req::register("peer".into(),
             "token-peer".into(),
             root.display().to_string(),
             Some(crate::proto::TransportCandidates {
                appserver: None,
                tmux: Some(crate::proto::TmuxCandidate {
                    endpoint: endpoint.clone(),
                    cwd: root.display().to_string(),
                }),
            })),
    );
    assert!(rebound.ok);
    let new_identity = dispatch(
        &server,
        Req::register("new-peer".into(),
             "token-new-peer".into(),
             root.display().to_string(),
             Some(crate::proto::TransportCandidates {
                appserver: None,
                tmux: Some(crate::proto::TmuxCandidate {
                    endpoint,
                    cwd: root.display().to_string(),
                }),
            })),
    );
    assert_eq!(
        new_identity.error.as_deref(),
        Some("MIGRATION_ADMISSION_FROZEN: only an existing identity may rebind")
    );
    assert_eq!(server.state.lock().unwrap().workers.len(), 1);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_uses_registered_cwd_for_authoritative_route_scope() {
    let (server, root) = test_server();
    assert!(register(&server, "sender", "thread-sender").ok);
    assert!(register(&server, "recipient", "%recipient").ok);

    let mut request = authenticated_send(&root, "sender", "recipient", "scope");
    if let Req::Send {
        command: Some(command),
        ..
    } = &mut request
    {
        command.scope = crate::scope::RouteScope::for_registered_project(
            crate::identity::AppServerId::new("appserver-cli").unwrap(),
            &root,
        )
        .unwrap();
    }
    let response = dispatch(&Arc::new(server), request);
    assert!(!response.ok);
    assert!(response
        .error
        .as_deref()
        .is_some_and(|error| error.starts_with("SEND_BINDING_REJECTED:")));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn authenticated_send_returns_typed_durability_failure_before_wake() {
    let (server, root) = test_server();
    assert!(register(&server, "sender", "%sender").ok);
    assert!(register(&server, "recipient", "%recipient").ok);
    *server.journal.lock().unwrap() =
        std::fs::File::open(root.join(".agent-collab/server/journal.jsonl")).unwrap();

    let response = dispatch(
        &Arc::new(server),
        authenticated_send(&root, "sender", "recipient", "durability"),
    );
    assert!(!response.ok);
    assert!(response
        .error
        .as_deref()
        .is_some_and(|error| error.starts_with("SEND_DURABILITY_FAILED:")));
    assert!(response.error.as_deref().unwrap().contains("journal"));
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn duplicate_daemon_rejection_preserves_authoritative_pid() {
    let _startup_test_lock = startup_test_lock();
    let root = PathBuf::from(format!(
        "/tmp/collab-sd-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let scope = Scope { root: root.clone() };
    let first = tokio::spawn(run(Scope { root: root.clone() }));
    for _ in 0..100 {
        if scope.sock_path().exists() && scope.server_dir().join("server.pid").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(scope.sock_path().exists());
    let pid_path = scope.server_dir().join("server.pid");
    let authoritative_pid = std::fs::read_to_string(&pid_path).unwrap();

    let error = run(Scope { root: root.clone() })
        .await
        .err()
        .expect("second daemon must be rejected");
    assert!(error.to_string().contains("server already running"));
    assert_eq!(
        std::fs::read_to_string(&pid_path).unwrap(),
        authoritative_pid
    );

    first.abort();
    let _ = first.await;
    std::fs::remove_dir_all(root).ok();
}
