fn handle_task_review(
    server: &Server,
    worker_id: String,
    token: String,
    task_id: String,
    accept: bool,
    rework: bool,
    evidence: String,
) -> Resp {
    let mut st = server.state.lock().unwrap();
    if let Err(error) = verify(&st, &worker_id, &token) {
        return error;
    }
    if accept == rework {
        return Resp::err("task review requires exactly one of --accept or --rework");
    }
    let evidence = evidence.trim();
    if evidence.is_empty() {
        return Resp::err("task review requires non-empty --evidence");
    }
    let Some(task) = st.tasks.get(&task_id).cloned() else {
        return Resp::err(format!("task {} not found", task_id));
    };
    if task.status != "delivered" {
        return Resp::err(format!(
            "task {} must be delivered before review (current: {})",
            task_id, task.status
        ));
    }
    if !task_integration_authorized(server, &st, &task, &worker_id) {
        return Resp::err("task review requires task owner or live master authority");
    }
    let now = now_ms();
    let mut reviewed = task;
    reviewed.status = if accept { "accepted" } else { "rework" }.into();
    reviewed.next_step = Some(if accept {
        "integrate the accepted candidate on refs/heads/main, then record collab task integrated"
            .into()
    } else {
        format!("address review evidence: {evidence}")
    });
    reviewed.updated_ms = now;
    let mut lifecycle = st.task_lifecycle.get(&task_id).cloned().unwrap_or_default();
    lifecycle.review_evidence = Some(evidence.to_owned());
    lifecycle.reviewer = Some(worker_id.clone());
    lifecycle.reviewed_ms = Some(now);
    let delivery_commit = lifecycle.delivery_commit.clone();
    let mut events = vec![
        Event::TaskUpdated {
            task: reviewed.clone(),
        },
        Event::TaskLifecycleUpdated {
            task_id: task_id.clone(),
            record: lifecycle,
        },
    ];
    let mut pending_notification = None;
    let mut notification_missing = false;
    let mut merge_pending_registered = false;
    if accept {
        // A merge obligation exists only when a live master owns the merge. In
        // a master-less project the owner keeps the plain self-integration
        // lifecycle; registering a pending merge there would deadlock close.
        // An unresolvable master presence is ambiguous authority: fail closed
        // instead of silently downgrading to owner self-integration.
        let master_id = match live_master_id(server, &st) {
            Ok(master_id) => master_id,
            Err(error) => {
                return Resp::err_data(
                    "MASTER_PRESENCE_UNKNOWN",
                    json!({
                        "task_id": task_id,
                        "error": error,
                        "rule": "cannot accept a task while master presence is unknown; probe transport and retry",
                    }),
                );
            }
        };
        if let Some(master_id) = master_id {
            let candidate_commit = delivery_commit;
            let request = state::PendingMerge {
                task_id: task_id.clone(),
                owner: reviewed.owner.clone(),
                requested_by: worker_id.clone(),
                requested_ms: now,
                candidate_commit,
            };
            events.push(Event::MergeRequested { request });
            merge_pending_registered = true;
            if master_id != worker_id {
                let message_id = gen_msg_id();
                events.push(Event::Sent {
                    msg: Message {
                        id: message_id.clone(),
                        from: "collab-server".into(),
                        to: master_id.clone(),
                        mtype: "notify".into(),
                        subject: Some(format!("merge-pending:{}", task_id)),
                        body: format!(
                            "MERGE_PENDING task={} owner={} requested_by={}. Master must integrate the accepted candidate on refs/heads/main and record `collab task integrated --commit <sha> --evidence \"<text>\"` before the task can close.",
                            task_id, reviewed.owner, worker_id
                        ),
                        in_reply_to: None,
                        created_ms: now,
                        state: "pending".into(),
                        wake_attempt_count: 0,
                        last_wake_attempt_ms: 0,
                        retry_attempted: false,
                    },
                });
                // A pending merge is the master's blocking obligation: wake it
                // immediately instead of waiting for the batched window, while
                // pending_merges stays the durable fallback.
                events.push(Event::DeliveryMode {
                    msg_id: message_id.clone(),
                    mode: "immediate".into(),
                    source_thread_id: None,
                });
                if let Some(subscription) =
                    st.matching_subscription(&master_id, "direct-message", None, now)
                {
                    events.push(Event::WakeBound {
                        message_id: message_id.clone(),
                        subscription_id: subscription.id.clone(),
                    });
                    pending_notification = Some((message_id, subscription.id.clone()));
                } else {
                    // The obligation is durable, but a missing direct-message
                    // subscription leaves the master mailbox-only with no wake.
                    // Surface the explicit repair terminal instead of success.
                    notification_missing = true;
                }
            }
        }
    }
    if let Err(error) = server.commit_locked(&mut st, &events) {
        drop(st);
        return Resp::err(format!("TASK_REVIEW_DURABILITY_FAILED: {error}"));
    }
    drop(st);
    let mut notification_attempt_failure: Option<String> = None;
    if let Some((message_id, subscription_id)) = pending_notification {
        // Surface any non-accepted wake attempt as an explicit repair terminal;
        // pending_merges stays durable regardless.
        match attempt_notification_detailed_with_at(
            server,
            &message_id,
            &subscription_id,
            now,
        ) {
            NotificationAttempt::Accepted => {}
            NotificationAttempt::Rejected(error) => {
                notification_attempt_failure = Some(error);
            }
            NotificationAttempt::NotAttempted(error) => {
                notification_attempt_failure = Some(error);
            }
        }
    }
    let mut review_data = json!({
        "task": task_id,
        "status": reviewed.status,
        "reviewer": worker_id,
        "evidence": evidence,
        "merge_pending": merge_pending_registered,
        "next_action": reviewed.next_step,
    });
    if notification_missing {
        apply_mailbox_only_repair_fields(&mut review_data);
        review_data["notification"] = json!("mailbox-only-no-subscription");
    } else if let Some(error) = notification_attempt_failure {
        review_data["durable"] = json!(true);
        review_data["notification"] = json!("subscribed-not-sent");
        review_data["notification_error"] = json!(error);
        review_data["failure"] = json!("notification_delivery_failed");
        review_data["repair_required"] = json!(true);
        review_data["escalation"] = json!(
            "the merge obligation is durable but this wake was not delivered; do not retry this wake, have the master run `collab recv` and rebind the selected wake transport"
        );
    }
    Resp::data(review_data)
}

fn handle_task_integrated(
    server: &Server,
    worker_id: String,
    token: String,
    task_id: String,
    commit: String,
    evidence: String,
) -> Resp {
    let mut st = server.state.lock().unwrap();
    if let Err(error) = verify(&st, &worker_id, &token) {
        return error;
    }
    let commit = commit.trim();
    let evidence = evidence.trim();
    if commit.is_empty() || evidence.is_empty() {
        return Resp::err("task integrated requires non-empty --commit and --evidence");
    }
    let Some(task) = st.tasks.get(&task_id).cloned() else {
        return Resp::err(format!("task {} not found", task_id));
    };
    if task.status != "accepted" {
        return Resp::err(format!(
            "task {} must be accepted before integration (current: {})",
            task_id, task.status
        ));
    }
    if !task_integration_authorized(server, &st, &task, &worker_id) {
        return Resp::err("task integrated requires task owner or live master authority");
    }
    if st.pending_merges.contains_key(&task_id) {
        let request = st.pending_merges.get(&task_id).cloned().unwrap();
        // A pending merge binds the obligation to the exact delivered commit.
        // If that candidate was never resolved, the obligation is unprovable
        // and must fail closed until it is re-bound (rework + re-deliver).
        if request.candidate_commit.is_none() {
            return Resp::err_data(
                "TASK_MERGE_PENDING",
                json!({
                    "task_id": task_id,
                    "status": task.status,
                    "candidate_commit": None::<String>,
                    "rule": "this pending merge has no bound candidate commit; rework the task and re-deliver with a resolvable worktree/branch so the accepted candidate can be proven on refs/heads/main before integration",
                }),
            );
        }
        let is_live_master = live_master_id(server, &st)
            .ok()
            .flatten()
            .as_deref()
            == Some(worker_id.as_str());
        if !is_live_master {
            return Resp::err_data(
                "TASK_MERGE_PENDING",
                json!({
                    "task_id": task_id,
                    "status": task.status,
                    "rule": "an accepted task with a daemon pending merge must be integrated by the live master after the merge lands on refs/heads/main; the owner records evidence after that and closes only after master integrated",
                }),
            );
        }
    }
    let head = match resolve_authoritative_main_head(&server.root) {
        Ok(head) => head,
        Err(error) => return error,
    };
    let integrated = match commit_is_integrated_in_main(&server.root, commit) {
        Ok(integrated) => integrated,
        Err(error) => return error,
    };
    if !integrated {
        return Resp::err_data(
            "TASK_INTEGRATION_COMMIT_MISMATCH",
            json!({
                "provided": commit,
                "main_head": head,
                "expected": format!(
                    "commit already reachable from refs/heads/main (main tip {head})"
                ),
            }),
        );
    }
    // When a pending merge carries the delivered candidate, the recorded
    // integration must prove that candidate itself reached main; an unrelated
    // pre-existing main commit must not satisfy the obligation. The
    // unbound-candidate case already failed closed above, so the candidate is
    // guaranteed to be present here.
    if let Some(candidate) = st
        .pending_merges
        .get(&task_id)
        .and_then(|request| request.candidate_commit.as_deref())
    {
        match commit_is_integrated_in_main(&server.root, candidate) {
            Ok(true) => {}
            Ok(false) => {
                return Resp::err_data(
                    "TASK_MERGE_PENDING",
                    json!({
                        "task_id": task_id,
                        "candidate_commit": candidate,
                        "provided": commit,
                        "rule": "the delivered candidate must be merged onto refs/heads/main before the pending merge can be resolved",
                    }),
                );
            }
            Err(error) => return error,
        }
    }
    let now = now_ms();
    let mut integrated = task;
    integrated.status = "merged".into();
    integrated.next_step = Some("owner cleans the worktree/branch and closes the task".into());
    integrated.updated_ms = now;
    let mut lifecycle = st.task_lifecycle.get(&task_id).cloned().unwrap_or_default();
    lifecycle.integration_commit = Some(commit.to_owned());
    lifecycle.integration_evidence = Some(evidence.to_owned());
    lifecycle.integrated_ms = Some(now);
    let mut events = vec![
        Event::TaskUpdated {
            task: integrated.clone(),
        },
        Event::TaskLifecycleUpdated {
            task_id: task_id.clone(),
            record: lifecycle,
        },
    ];
    if st.pending_merges.contains_key(&task_id) {
        let stale_notices = pending_merge_notice_ids(&st, &task_id);
        events.push(Event::MergeResolved {
            task_id: task_id.clone(),
            resolved_by: worker_id.clone(),
            reason: Some("integrated into main".into()),
            at_ms: now,
        });
        if !stale_notices.is_empty() {
            events.push(Event::Superseded { ids: stale_notices });
        }
    }
    if let Err(error) = server.commit_locked(&mut st, &events) {
        drop(st);
        return Resp::err(format!("TASK_INTEGRATED_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "task": task_id,
        "status": integrated.status,
        "commit": commit,
        "evidence": evidence,
        "next_action": integrated.next_step,
    }))
}

fn handle_task_close(
    server: &Server,
    worker_id: String,
    token: String,
    task_id: String,
    force: bool,
    reason: Option<String>,
) -> Resp {
    let mut st = server.state.lock().unwrap();
    let Some(worker) = st.workers.get(&worker_id).cloned() else {
        return Resp::err(format!("worker {} not registered", worker_id));
    };
    if worker.token != token {
        return Resp::err("token mismatch: identity does not own this worker_id");
    }
    let Some(task) = st.tasks.get(&task_id).cloned() else {
        return Resp::err(format!("task {} not found", task_id));
    };
    if force {
        let reason = reason
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let Some(reason) = reason else {
            return Resp::err("force close requires a non-empty --reason");
        };
        let live_master = match live_master_id(server, &st) {
            Ok(master) => master,
            Err(error) => return Resp::err(error),
        };
        // Only a definitively missing owner identity is treated as dead.  A
        // cold thread may simply be idle on the endpoint and an unknown probe
        // is not evidence of death, so neither authorizes force-closing
        // another peer's task.
        let owner_identity_live = st.workers.get(&task.owner).is_some_and(|owner| {
            !matches!(worker_presence(server, owner), IdentityPresence::Missing)
        });
        let authorized = live_master.as_deref() == Some(worker_id.as_str())
            || (live_master.is_none() && (task.owner == worker_id || !owner_identity_live));
        if !authorized {
            return Resp::err_data(
                "manual force close is not authorized for this caller",
                json!({
                    "live_master": live_master,
                    "task_owner": task.owner,
                    "requester": worker_id,
                    "rule": "live master may close any task; with no live master, the owner may close its task or a registered peer may close an orphaned task whose owner identity is no longer live",
                    "owner_identity_live": owner_identity_live,
                }),
            );
        }
        if task.status == "closed" {
            if let Some(receipt) = st.cleanup_receipts.get(&task.id).filter(|receipt| {
                receipt.task_id == task.id
                    && receipt.worktree_path == task.worktree_path
                    && receipt.branch == task.branch
            }) {
                let cleanup_verified = receipt.verification == CleanupVerification::Verified;
                return Resp::data(json!({
                    "task": task.id,
                    "status": task.status,
                    "owner": task.owner,
                    "manual": true,
                    "reason": receipt.manual_reason,
                    "receipt_id": receipt.id,
                    "cleanup": {
                        "result": if cleanup_verified { "verified" } else { "unverified" },
                        "reason": receipt.manual_reason,
                    },
                    "superseded_pending_keepalives": [],
                    "stale_workers": stale_worker_views(&st, &|worker| {
                        worker_presence(server, worker)
                    }),
                    "idempotent": true,
                    "next_action": if cleanup_verified {
                        "lifecycle complete; keepalives for this task owner stopped"
                    } else {
                        "manual close recorded; worktree/branch cleanup remains unverified"
                    },
                }));
            }
        }
        let mut closed = task;
        closed.status = "closed".into();
        closed.wait = None;
        closed.next_step = Some(format!("manual close: {reason}"));
        closed.updated_ms = now_ms();
        let receipt = CleanupReceipt {
            id: format!("cleanup-manual-{}-{}", closed.id, closed.updated_ms),
            task_id: closed.id.clone(),
            worktree_path: closed.worktree_path.clone(),
            branch: closed.branch.clone(),
            verified_ms: closed.updated_ms,
            verification: CleanupVerification::Unverified,
            manual_reason: Some(reason.clone()),
        };
        let superseded: Vec<String> = st
            .msgs
            .values()
            .filter(|m| {
                m.to == closed.owner
                    && m.mtype == "keepalive"
                    && matches!(m.state.as_str(), "pending" | "delivered")
            })
            .map(|m| m.id.clone())
            .collect();
    let mut events: Vec<Event> = vec![
        Event::CleanupVerified {
            receipt: receipt.clone(),
        },
        Event::TaskUpdated {
            task: closed.clone(),
        },
    ];
    if st.pending_merges.contains_key(&closed.id) {
        let stale_notices = pending_merge_notice_ids(&st, &closed.id);
        events.push(Event::MergeResolved {
            task_id: closed.id.clone(),
            resolved_by: worker_id.clone(),
            reason: Some(format!("force close: {reason}")),
            at_ms: closed.updated_ms,
        });
        if !stale_notices.is_empty() {
            events.push(Event::Superseded { ids: stale_notices });
        }
    }
    if !superseded.is_empty() {
        events.push(Event::Superseded {
            ids: superseded.clone(),
            });
        }
        let other_actionable = st.tasks.values().any(|t| {
            t.id != closed.id
                && t.owner == closed.owner
                && crate::server::keepalive::actionable(&t.status)
        });
        if !other_actionable {
            if let Some(record) = st.keepalives.get(&closed.owner).cloned() {
                if record.unacked > 0 || record.last_notice_id.is_some() {
                    let mut updated = record;
                    updated.unacked = 0;
                    updated.last_notice_id = None;
                    events.push(Event::KeepaliveUpdated {
                        worker_id: closed.owner.clone(),
                        record: updated,
                    });
                }
            }
        }
        if let Err(error) = server.commit_locked(&mut st, &events) {
            drop(st);
            return Resp::err(format!("TASK_CLOSE_DURABILITY_FAILED: {error}"));
        }
        let stale_workers = stale_worker_views(&st, &|worker| worker_presence(server, worker));
        drop(st);
        return Resp::data(json!({
            "task": closed.id,
            "status": closed.status,
            "owner": closed.owner,
            "manual": true,
            "reason": reason,
            "receipt_id": receipt.id,
            "cleanup": {
                "result": "unverified",
                "reason": receipt.manual_reason,
            },
            "superseded_pending_keepalives": superseded,
            "stale_workers": stale_workers,
            "next_action": "manual close recorded; worktree/branch cleanup remains unverified",
        }));
    }
    if task.owner != worker_id {
        let live_master = match live_master_id(server, &st) {
            Ok(master) => master,
            Err(error) => return Resp::err(error),
        };
        if live_master.as_deref() != Some(worker_id.as_str()) {
            return Resp::err("task close requires task owner or live master authority");
        }
    }
    if task.status != "merged" {
        if let Some(request) = st.pending_merges.get(&task_id) {
            return Resp::err_data(
                "TASK_MERGE_PENDING",
                json!({
                    "task_id": task_id,
                    "status": task.status,
                    "requested_by": request.requested_by,
                    "rule": "master must merge the accepted candidate onto refs/heads/main and record `collab task integrated --commit <sha> --evidence \"<text>\"` before close",
                }),
            );
        }
        return Resp::err(format!(
            "task {} must be merged by its owner before close (current: {})",
            task_id, task.status
        ));
    }
    let lifecycle_complete = st.task_lifecycle.get(&task_id).is_some_and(|record| {
        record
            .delivery_evidence
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            && record.delivered_ms.is_some()
            && record
                .review_evidence
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && record
                .reviewer
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && record.reviewed_ms.is_some()
            && record
                .integration_commit
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && record
                .integration_evidence
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && record.integrated_ms.is_some()
    });
    if !lifecycle_complete {
        return Resp::err(format!(
            "task {} cannot close before delivery, review, and integration evidence",
            task_id
        ));
    }
    let receipt_reusable = st
        .cleanup_receipts
        .get(&task.id)
        .filter(|receipt| {
            receipt.task_id == task.id
                && receipt.worktree_path == task.worktree_path
                && receipt.branch == task.branch
        })
        .is_some_and(|receipt| cleanup_receipt_is_reusable(&server.root, receipt));
    if !receipt_reusable {
        if let Err(e) = close_task_resources(
            &server.root,
            &server.config,
            task.worktree_path.as_deref(),
            task.branch.as_deref(),
        ) {
            return Resp::err(e);
        }
    }
    let mut closed = task;
    closed.status = "closed".to_string();
    closed.wait = None;
    closed.next_step = Some("closed after owner merge and cleanup".to_string());
    closed.updated_ms = now_ms();
    let receipt = CleanupReceipt {
        id: format!("cleanup-{}-{}", closed.id, closed.updated_ms),
        task_id: closed.id.clone(),
        worktree_path: closed.worktree_path.clone(),
        branch: closed.branch.clone(),
        verified_ms: closed.updated_ms,
        verification: CleanupVerification::Verified,
        manual_reason: None,
    };
    let superseded: Vec<String> = st
        .msgs
        .values()
        .filter(|m| {
            m.to == closed.owner
                && m.mtype == "keepalive"
                && matches!(m.state.as_str(), "pending" | "delivered")
        })
        .map(|m| m.id.clone())
        .collect();
    let mut close_events: Vec<Event> = vec![
        Event::CleanupVerified {
            receipt: receipt.clone(),
        },
        Event::TaskUpdated {
            task: closed.clone(),
        },
    ];
    if !superseded.is_empty() {
        close_events.push(Event::Superseded { ids: superseded });
    }
    let other_actionable = st.tasks.values().any(|t| {
        t.id != closed.id
            && t.owner == closed.owner
            && crate::server::keepalive::actionable(&t.status)
    });
    if !other_actionable {
        if let Some(record) = st.keepalives.get(&closed.owner).cloned() {
            if record.unacked > 0 || record.last_notice_id.is_some() {
                let mut updated = record;
                updated.unacked = 0;
                updated.last_notice_id = None;
                close_events.push(Event::KeepaliveUpdated {
                    worker_id: closed.owner.clone(),
                    record: updated,
                });
            }
        }
    }
    if let Err(error) = server.commit_locked(&mut st, &close_events) {
        drop(st);
        return Resp::err(format!("TASK_CLOSE_DURABILITY_FAILED: {error}"));
    }

    let subscribed_notifications = release_dependents_of_closed_task(server, &mut st, &closed.id);

    let stale_workers = stale_worker_views(&st, &|worker| worker_presence(server, worker));
    drop(st);
    for (message_id, subscription_id) in subscribed_notifications {
        attempt_notification(server, &message_id, &subscription_id);
    }
    Resp::data(json!({
        "task": closed.id,
        "status": closed.status,
        "owner": closed.owner,
        "cleanup": {
            "worktree": closed.worktree_path,
            "branch": closed.branch,
            "result": "verified",
            "receipt_id": receipt.id,
        },
        "stale_workers": stale_workers,
        "notification": "subscribed resource waiters only",
        "next_action": "lifecycle complete",
    }))
}

/// Finish the release half of a cleanup completion whose verified receipt may
/// already be durable. A finalize commits the verified receipt first and then
/// releases waiters, so an interrupted run can leave a verified receipt with
/// waiters still blocked. Every caller that observes a verified receipt resumes
/// here, and the operation is idempotent: a released waiter no longer carries a
/// wait on the closed task, so a retry releases nobody and sends no duplicate
/// notification.
fn resume_cleanup_release(server: &Server, st: &mut State, task_id: &str) -> Vec<(String, String)> {
    release_dependents_of_closed_task(server, st, task_id)
}

/// A competing owner of the closed task's declared worktree/branch. A closed
/// task is not a resource holder for scheduling, so another task may
/// legitimately register the same worktree or branch after a forced close.
/// Finalization must never remove a resource that a live task has since taken
/// over, so it refuses while any non-closed task still references the declared
/// path or branch.
fn competing_resource_owner(state: &State, task: &TaskRec) -> Option<String> {
    state
        .tasks
        .values()
        .find(|candidate| {
            candidate.id != task.id
                && candidate.status != "closed"
                && ((task.worktree_path.is_some() && candidate.worktree_path == task.worktree_path)
                    || (task.branch.is_some() && candidate.branch == task.branch))
        })
        .map(|candidate| candidate.id.clone())
}

/// Waiters that blocked on one exactly identified task resource.
fn waiting_dependents_of(state: &State, closed_id: &str) -> Vec<TaskRec> {
    state
        .tasks
        .values()
        .filter(|candidate| {
            candidate.status == "waiting"
                && candidate
                    .wait
                    .as_ref()
                    .is_some_and(|wait| wait.waiting_for == closed_id)
        })
        .cloned()
        .collect()
}

/// A closed task frees its declared resource. Release exactly the waiters that
/// blocked on it and wake only the subscriptions that named its subject. Replay
/// and repeated completion stay idempotent because a released waiter no longer
/// waits on this task.
fn release_dependents_of_closed_task(
    server: &Server,
    st: &mut State,
    closed_id: &str,
) -> Vec<(String, String)> {
    let waiting = waiting_dependents_of(st, closed_id);
    let mut subscribed_notifications = Vec::new();
    for mut waiter_task in waiting {
        waiter_task.status = "blocked".into();
        waiter_task.wait = None;
        waiter_task.next_step = Some(format!(
            "RESOURCE_RELEASED={closed_id} recheck conflicts, then resume only after Server confirms free"
        ));
        waiter_task.updated_ms = now_ms();
        let waiter = waiter_task.owner.clone();
        let subscription = st
            .matching_subscription(&waiter, "resource-released", Some(closed_id), now_ms())
            .cloned();
        let mut events = vec![
            Event::TaskUpdated { task: waiter_task },
            Event::MasterWakeSignal {
                signal: state::MasterWakeSignal::TaskFreed {
                    task_id: closed_id.to_owned(),
                },
                at_ms: now_ms(),
            },
        ];
        if let Some(subscription) = subscription {
            let message_id = gen_msg_id();
            events.extend([
                Event::Sent {
                    msg: Message {
                        id: message_id.clone(),
                        from: "collab-server".into(),
                        to: waiter,
                        mtype: "notification".into(),
                        subject: Some(format!("released:{closed_id}")),
                        body: format!("RESOURCE_RELEASED subject={closed_id}"),
                        in_reply_to: None,
                        created_ms: now_ms(),
                        state: "pending".into(),
                        wake_attempt_count: 0,
                        last_wake_attempt_ms: 0,
                        retry_attempted: false,
                    },
                },
                Event::WakeBound {
                    message_id: message_id.clone(),
                    subscription_id: subscription.id.clone(),
                },
            ]);
            subscribed_notifications.push((message_id, subscription.id));
        }
        server.commit_locked_reporting(st, &events);
    }
    subscribed_notifications
}

/// Forced termination records an explicit manual close without claiming its
/// worktree or branch was cleaned. This is the single completion path for that
/// obligation: it applies the same resource contract as a normal close,
/// replaces the pending receipt with verified evidence, and only then releases
/// dependents and stops the owner's automatic lease.
fn handle_task_finalize_cleanup(
    server: &Server,
    worker_id: String,
    token: String,
    task_id: String,
) -> Resp {
    let mut st = server.state.lock().unwrap();
    let Some(worker) = st.workers.get(&worker_id).cloned() else {
        return Resp::err(format!("worker {} not registered", worker_id));
    };
    if worker.token != token {
        return Resp::err("token mismatch: identity does not own this worker_id");
    }
    let Some(task) = st.tasks.get(&task_id).cloned() else {
        return Resp::err(format!("task {} not found", task_id));
    };
    if task.status != "closed" {
        return Resp::err(format!(
            "task {} must be closed before cleanup finalization (current: {})",
            task_id, task.status
        ));
    }
    let live_master = match live_master_id(server, &st) {
        Ok(master) => master,
        Err(error) => return Resp::err(error),
    };
    // Only a definitively missing owner identity is treated as dead, matching
    // the force-close authorization rule.
    let owner_identity_live = st
        .workers
        .get(&task.owner)
        .is_some_and(|owner| !matches!(worker_presence(server, owner), IdentityPresence::Missing));
    let authorized = task.owner == worker_id
        || live_master.as_deref() == Some(worker_id.as_str())
        || (live_master.is_none() && !owner_identity_live);
    if !authorized {
        return Resp::err_data(
            "cleanup finalization is not authorized for this caller",
            json!({
                "live_master": live_master,
                "task_owner": task.owner,
                "requester": worker_id,
                "owner_identity_live": owner_identity_live,
                "rule": "task owner, live master, or a peer closing an orphaned task may finalize cleanup",
            }),
        );
    }
    let existing = st
        .cleanup_receipts
        .get(&task.id)
        .cloned()
        .filter(|receipt| {
            receipt.task_id == task.id
                && receipt.worktree_path == task.worktree_path
                && receipt.branch == task.branch
        });
    if existing
        .as_ref()
        .is_some_and(|receipt| receipt.verification == CleanupVerification::Verified)
    {
        let receipt_id = existing.map(|receipt| receipt.id);
        let released = waiting_dependents_of(&st, &task.id)
            .into_iter()
            .map(|waiter| waiter.id)
            .collect::<Vec<_>>();
        let notifications = resume_cleanup_release(server, &mut st, &task.id);
        drop(st);
        for (message_id, subscription_id) in notifications {
            attempt_notification(server, &message_id, &subscription_id);
        }
        return Resp::data(json!({
            "task": task.id,
            "owner": task.owner,
            "cleanup": {"result": "verified", "receipt_id": receipt_id},
            "idempotent": true,
            "released_dependents": released,
            "next_action": "lifecycle complete",
        }));
    }
    // A closed task is not a scheduling holder, so another task can take over
    // the same worktree or branch after the force close. Removing it here would
    // destroy a live owner's resource, so refuse before touching the
    // filesystem or git and leave the obligation retryable.
    if let Some(competing) = competing_resource_owner(&st, &task) {
        return Resp::err_data(
            format!(
                "CLEANUP_FINALIZE_REFUSED: task {} declared worktree/branch {} which is owned by non-closed task {}",
                task_id,
                task.worktree_path.as_deref().unwrap_or("-"),
                competing
            ),
            json!({
                "task": task.id,
                "competing_task": competing,
                "cleanup": {
                    "result": "unverified",
                    "reason": existing.and_then(|receipt| receipt.manual_reason),
                },
                "finalized": false,
                "rule": "finalization may remove only resources no non-closed task still references",
            }),
        );
    }
    // The same checks a normal close applies: a clean worktree inside
    // ./playground and a branch already merged into main. Dirty, unmerged, or
    // path-escaping resources are never removed to finish the lifecycle, so a
    // refused finalize leaves the obligation visible and retryable.
    if let Err(error) = close_task_resources(
        &server.root,
        &server.config,
        task.worktree_path.as_deref(),
        task.branch.as_deref(),
    ) {
        return Resp::err_data(
            format!("CLEANUP_FINALIZE_REFUSED: {error}"),
            json!({
                "task": task.id,
                "cleanup": {
                    "result": "unverified",
                    "reason": existing.and_then(|receipt| receipt.manual_reason),
                },
                "finalized": false,
                "next_action": "resolve the reported resource problem, then rerun finalize-cleanup",
            }),
        );
    }
    let now = now_ms();
    let receipt = CleanupReceipt {
        id: format!("cleanup-final-{}-{}", task.id, now),
        task_id: task.id.clone(),
        worktree_path: task.worktree_path.clone(),
        branch: task.branch.clone(),
        verified_ms: now,
        verification: CleanupVerification::Verified,
        manual_reason: existing.and_then(|receipt| receipt.manual_reason),
    };
    if let Err(error) = server.commit_locked(
        &mut st,
        &[Event::CleanupVerified {
            receipt: receipt.clone(),
        }],
    ) {
        drop(st);
        return Resp::err(format!("TASK_CLEANUP_DURABILITY_FAILED: {error}"));
    }
    let released = waiting_dependents_of(&st, &task.id)
        .into_iter()
        .map(|waiter| waiter.id)
        .collect::<Vec<_>>();
    let notifications = resume_cleanup_release(server, &mut st, &task.id);
    let stale_workers = stale_worker_views(&st, &|worker| worker_presence(server, worker));
    drop(st);
    for (message_id, subscription_id) in notifications {
        attempt_notification(server, &message_id, &subscription_id);
    }
    Resp::data(json!({
        "task": task.id,
        "owner": task.owner,
        "cleanup": {
            "worktree": task.worktree_path,
            "branch": task.branch,
            "result": "verified",
            "receipt_id": receipt.id,
            "manual_reason": receipt.manual_reason,
        },
        "released_dependents": released,
        "stale_workers": stale_workers,
        "finalized": true,
        "idempotent": false,
        "next_action": "lifecycle complete",
    }))
}

fn task_view(state: &State, task: &TaskRec) -> serde_json::Value {
    let cleanup_required = task.worktree_path.is_some();
    let cleanup_receipt = state.cleanup_receipts.get(&task.id);
    let lifecycle = state.task_lifecycle.get(&task.id);
    json!({
        "id": task.id,
        "owner": task.owner,
        "created_by": task.created_by,
        "feature_id": task.feature_id,
        "worktree": task.worktree_path,
        "branch": task.branch,
        "base_commit": task.base_commit,
        "priority": task.priority,
        "status": task.status,
        "next_step": task.next_step,
        "wait": task.wait,
        "delivery": {
            "evidence": lifecycle.and_then(|record| record.delivery_evidence.clone()),
            "at": lifecycle.and_then(|record| record.delivered_ms.map(iso)),
        },
        "review": {
            "evidence": lifecycle.and_then(|record| record.review_evidence.clone()),
            "reviewer": lifecycle.and_then(|record| record.reviewer.clone()),
            "at": lifecycle.and_then(|record| record.reviewed_ms.map(iso)),
        },
        "integration": {
            "commit": lifecycle.and_then(|record| record.integration_commit.clone()),
            "evidence": lifecycle.and_then(|record| record.integration_evidence.clone()),
            "at": lifecycle.and_then(|record| record.integrated_ms.map(iso)),
        },
        "merge": state.pending_merges.get(&task.id).map(|request| json!({
            "pending": true,
            "requested_by": request.requested_by,
            "requested_at": iso(request.requested_ms),
            "owner": request.owner,
        })).unwrap_or_else(|| json!({"pending": false})),
        "cleanup": {
            "required": cleanup_required,
            "status": if !cleanup_required {
                "not_required"
            } else if cleanup_receipt.is_some_and(|receipt| {
                receipt.verification == CleanupVerification::Verified
            }) {
                "verified"
            } else if cleanup_receipt.is_some() {
                "unverified"
            } else {
                "pending"
            },
            "receipt_id": cleanup_receipt.map(|receipt| receipt.id.clone()),
        },
        "updated_at": iso(task.updated_ms),
        "keepalive": keepalive::view(state, &task.owner),
    })
}

fn pending_merge_views(state: &State) -> Vec<serde_json::Value> {
    let mut views: Vec<_> = state
        .pending_merges
        .values()
        .map(|request| {
            let task = state.tasks.get(&request.task_id);
            json!({
                "task_id": request.task_id,
                "owner": request.owner,
                "requested_by": request.requested_by,
                "requested_at": iso(request.requested_ms),
                "status": task.map(|task| task.status.as_str()).unwrap_or("unknown"),
                "branch": task.and_then(|task| task.branch.clone()),
                "worktree": task.and_then(|task| task.worktree_path.clone()),
            })
        })
        .collect();
    views.sort_by(|a, b| a["task_id"].as_str().cmp(&b["task_id"].as_str()));
    views
}

/// Durable merge-pending notices for a task that may still be readable after a
/// resolution (integrated/rework/cancel). Superseding them prevents a stale P0
/// obligation from surviving in the mailbox once pending_merges is gone.
fn pending_merge_notice_ids(state: &State, task_id: &str) -> Vec<String> {
    let subject = format!("merge-pending:{task_id}");
    state
        .msgs
        .values()
        .filter(|message| {
            message.subject.as_deref() == Some(subject.as_str())
                && matches!(message.state.as_str(), "pending" | "delivered")
        })
        .map(|message| message.id.clone())
        .collect()
}

fn daemon_context_view(server: &Server) -> serde_json::Value {
    let Ok(host_paths) = HostPaths::resolve() else {
        return json!({
            "pid": std::process::id(),
            "socket": null,
            "live": true,
            "reason": "this daemon served the request, but its host state path is unavailable",
            "storage_root": server.storage_root,
        });
    };
    json!({
        "pid": std::process::id(),
        "socket": host_paths.socket_path(),
        "live": true,
        "storage_root": server.storage_root,
    })
}

fn handle_context(server: &Server, worker_id: String, token: String) -> Resp {
    let mut st = server.state.lock().unwrap();
    if let Err(e) = verify(&st, &worker_id, &token) {
        return e;
    }
    let Some(worker) = st.workers.get(&worker_id).cloned() else {
        return Resp::err(format!("worker {} not registered", worker_id));
    };
    // `collab context` is the single bootstrap entry, so it must also be the
    // place where the system-owned default direct-message lease is restored.
    // An already-registered peer (RegistrationOutcome::Reused) never re-enters
    // the Register command, so without this the peer would silently degrade to
    // mailbox-only after the lease expired or was lost. Explicit owner
    // unsubscribe stays authoritative; only the legacy/expired shapes re-arm.
    if let Some(transport) = selected_transport_for_worker(&worker) {
        let events = default_direct_message_events(&st, &worker_id, &transport, now_ms());
        if !events.is_empty() {
            if let Err(error) = server.try_commit_locked(&mut st, &events) {
                return Resp::err(format!("default lease rearm failed: {error}"));
            }
        }
    }
    let mut tasks: Vec<serde_json::Value> = st
        .tasks
        .values()
        .filter(|task| task.owner == worker_id)
        .map(|task| task_view(&st, task))
        .collect();
    tasks.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
    let current_role_brief = role_brief(server, &st, &worker_id);
    let role = current_role_brief
        .get("role")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("worker")
        .to_owned();
    let authority = current_role_brief["authority"].clone();
    let route_scope = server_route_scope(server, &st).ok().flatten();
    let current_master = current_master_worker_id(&st, route_scope.as_ref());
    // The caller's own runtime binding is the receipt that addresses this
    // route. `collab context` is the single bootstrap read, so it owns the
    // read-back path for a peer whose local copy of that receipt was lost.
    // Read it from the ledger the daemon validates commands against, so the
    // shape stays the `RuntimeBinding` the Register receipt already carries.
    //
    // This deliberately does not go through the resolved route scope. One root
    // can hold several app scopes, and then no single route scope resolves
    // even though the caller's own binding is unambiguous. Match the caller
    // inside the project this root owns, and report null unless exactly one
    // binding matches, so an ambiguous caller still gets null and not a guess.
    let binding = GlobalState::canonical_project_scope(&server.root)
        .ok()
        .and_then(|project_scope| st.global.lookup_project(&project_scope))
        .and_then(|project| {
            let mut matches = project
                .runtime_bindings
                .values()
                .filter(|binding| binding.agent_id.as_str() == worker_id);
            let binding = matches.next()?.clone();
            matches.next().is_none().then_some(binding)
        });
    let worktrees: Vec<serde_json::Value> = {
        let mut worktrees = st
            .worktree_bindings
            .values()
            .filter(|binding| binding.owner_agent_id == worker_id)
            .map(|binding| {
                let task = st.tasks.get(&binding.task_id);
                json!({
                    "task_id": binding.task_id,
                    "owner": binding.owner_agent_id,
                    "branch": task.and_then(|task| task.branch.clone()),
                    "path": binding.worktree_root,
                    "base_commit": binding.base_commit,
                    "status": task.map(|task| task.status.as_str()).unwrap_or("unknown"),
                    "cleanup": task.map(|task| task_view(&st, task)["cleanup"].clone()),
                })
            })
            .collect::<Vec<_>>();
        worktrees.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
        worktrees
    };
    let subscriptions: Vec<serde_json::Value> = {
        let mut subscriptions = st
            .notification_subscriptions
            .values()
            .filter(|subscription| subscription.worker_id == worker_id)
            .collect::<Vec<_>>();
        subscriptions.sort_by_key(|subscription| (subscription.created_ms, &subscription.id));
        subscriptions
            .into_iter()
            .map(|subscription| {
                serde_json::to_value(subscription).unwrap_or(serde_json::Value::Null)
            })
            .collect()
    };
    let mut peer_snapshots: Vec<_> = st
        .workers
        .values()
        .map(|peer| {
            let peer_role = role_brief(server, &st, &peer.id)
                .get("role")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("worker")
                .to_owned();
            (peer.clone(), peer_role)
        })
        .collect();
    peer_snapshots.sort_by(|left, right| left.0.id.cmp(&right.0.id));
    let master_worker_id = current_master;
    let master_grant = current_master_grant(&st, route_scope.as_ref());
    let master_assigned_by = master_grant.as_ref().map(|grant| grant.granted_by.clone());
    let master_approval = master_grant.as_ref().map(|grant| grant.approval.clone());
    let master_assigned_ms = master_grant.as_ref().map(|grant| grant.granted_at_ms);
    let pending_merges = pending_merge_views(&st);
    // `collab context` is the single agent bootstrap read, so the projections
    // that `collab status --all` and `collab who` expose belong to the same
    // snapshot instead of a second call. They are captured here because `st` is
    // released before the transport probes below. `master_wake` and the message
    // count are deliberately not captured here: the presence recording below can
    // commit a wake signal and worker-unresponsive / worker-recovered messages,
    // so reading them before it would return pre-transition scheduling state.
    let task_count = st.tasks.len();
    let mut subagents: Vec<crate::subagent::Record> = st.subagents.values().cloned().collect();
    subagents.sort_by(|left, right| left.id.cmp(&right.id));
    drop(st);

    // `collab context` replaces `collab who` / `collab status --all` /
    // `collab master status` for agents, and `Workers` / `StatusAll` were the
    // calls that recorded ordinary-peer presence edges. Without this the
    // online/offline transitions would stop emitting KeepaliveUpdated,
    // MasterWakeSignal, worker-unresponsive and worker-recovered as soon as
    // agents follow the consolidated entry. Must run after `drop(st)` because it
    // takes the state lock itself.
    record_ordinary_peer_presence_edges(server, None);

    // That recording can commit KeepaliveUpdated, MasterWakeSignal, and
    // worker-unresponsive / worker-recovered messages, so the scheduling
    // projections are read after it. `StatusAll` records first for the same
    // reason; reading them before would return pre-transition state next to
    // already-updated peer presence.
    let (master_wake, message_count, unread_count, inbox_messages) = {
        let st = server.state.lock().unwrap();
        let unread: Vec<&Message> = st.inbox_of(&worker_id);
        let messages: Vec<serde_json::Value> = unread
            .iter()
            .rev()
            .take(20)
            .map(|message| {
                json!({
                    "id": message.id,
                    "from": message.from,
                    "type": message.mtype,
                    "subject": message.subject,
                    "state": message.state,
                    "created_at": iso(message.created_ms),
                    "body": message.body,
                })
            })
            .collect();
        (
            st.master_wake.clone(),
            st.msgs.len(),
            unread.len(),
            messages,
        )
    };

    let (presence, agent) = worker_presence_with_view(server, &worker);
    let peers: Vec<_> = peer_snapshots
        .into_iter()
        .map(|(peer, peer_role)| {
            let peer_presence = worker_presence(server, &peer);
            json!({
                "worker_id": peer.id,
                "id": peer.id,
                "role": peer_role,
                "presence": match peer_presence {
                    IdentityPresence::Present => "present",
                    IdentityPresence::Cold => "cold",
                    IdentityPresence::Missing => "missing",
                    IdentityPresence::Unknown => "unknown",
                },
                "endpoint_live": peer_presence == IdentityPresence::Present,
            })
        })
        .collect();
    let transport = worker.transport.as_ref().map(|transport| {
        json!({
            "kind": transport.kind.as_str(),
            "endpoint": transport.endpoint,
            "namespace": transport.namespace,
            "thread_id": transport.thread_id,
            "tmux_endpoint": transport.tmux_endpoint,
            "self_check": transport.self_check,
        })
    });
    let registration = json!({
        "status": "registered",
        "project_root": server.root,
        "worker_id": worker.id,
        "cwd": worker.cwd,
        "registered_at": iso(worker.registered_ms),
    });
    let master_peer = master_worker_id
        .as_deref()
        .and_then(|id| peers.iter().find(|peer| peer["worker_id"] == id));
    let master_presence = master_peer
        .and_then(|peer| peer["presence"].as_str())
        .unwrap_or("missing");
    let assignment_view = |endpoint_live: bool| {
        master_worker_id.as_ref().map(|id| {
            json!({
                "worker_id": id,
                "endpoint_live": endpoint_live,
                "assigned_by": master_assigned_by,
                "approval": master_approval,
                "assigned_ms": master_assigned_ms,
                "master_wake": master_wake,
            })
        })
    };
    let (master, recorded_unusable) = match (master_worker_id.as_ref(), master_presence) {
        (None, _) => (serde_json::Value::Null, serde_json::Value::Null),
        (Some(_), "present") => (
            assignment_view(true).unwrap_or(serde_json::Value::Null),
            serde_json::Value::Null,
        ),
        (Some(_), "unknown") => (
            json!({
                "status": "unknown",
                "error": "master identity is unknown; defer authority changes until transport probes succeed",
            }),
            assignment_view(false).unwrap_or(serde_json::Value::Null),
        ),
        (Some(_), _) => (
            serde_json::Value::Null,
            assignment_view(false).unwrap_or(serde_json::Value::Null),
        ),
    };
    let mut next_actions: Vec<String> = tasks
        .iter()
        .filter_map(|task| {
            task.get("next_step")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    if next_actions.is_empty() {
        if master["worker_id"].as_str() == Some(worker_id.as_str()) {
            next_actions.push(
                "run `appsdk longhorizon show`, saturate live peers first, then schedule managed subagents within the configured cap; do not end the scheduling turn while eligible capacity remains idle"
                    .into(),
            );
        } else {
            next_actions
                .push("no assigned task action; remain available for an explicit dispatch".into());
        }
    }
    let operations: Vec<serde_json::Value> = {
        let mut operations = Vec::new();
        let mut push = |kind: &str, value: Option<&str>| {
            if let Some(action) = value.map(str::trim).filter(|value| !value.is_empty()) {
                operations.push(json!({"kind": kind, "action": action}));
            }
        };
        push("next_action", current_role_brief["next_action"].as_str());
        if let Some(responsibilities) = current_role_brief["responsibilities"].as_array() {
            for responsibility in responsibilities {
                push("responsibility", responsibility.as_str());
            }
        }
        push(
            "completion_action",
            current_role_brief["completion_action"].as_str(),
        );
        push(
            "notification_rule",
            current_role_brief["notification_rule"].as_str(),
        );
        let live_master = master
            .get("worker_id")
            .and_then(serde_json::Value::as_str)
            .is_some();
        if live_master && !pending_merges.is_empty() {
            operations.push(json!({
                "kind": "merge_pending",
                "action": "for each pending merge, merge the accepted candidate on refs/heads/main, then record collab task integrated before close",
                "pending_merges": pending_merges,
            }));
        }
        if !live_master {
            operations.push(json!({
                "kind": "promote_master",
                "action": "collab master promote --approval \"<user authorization>\"",
                "requires_approval": true,
                "trigger": "no live master is bound; promotion requires explicit user approval and then auto-completes"
            }));
        }
        operations
    };
    let peer_count = peers.len();
    Resp::data(json!({
        "schema_version": 1,
        "registration": registration,
        "binding": binding,
        "registered": true,
        "project_root": server.root,
        "identity": {
            "worker_id": worker.id,
            "kind": "peer",
            "role": role,
            "transport": transport,
        },
        "role_brief": current_role_brief,
        "liveness": {
            // `live` stays reserved for a thread that is resident now.  A cold
            // thread reports its own presence value below and is labelled
            // "cold" rather than "present" or "missing".
            "live": presence == IdentityPresence::Present,
            "presence": match presence {
                IdentityPresence::Present => "present",
                IdentityPresence::Cold => "cold",
                IdentityPresence::Missing => "missing",
                IdentityPresence::Unknown => "unknown",
            },
            "transport_kind": worker.transport.as_ref().map(|transport| transport.kind.as_str()),
            "endpoint": worker.transport.as_ref().and_then(|transport| transport.endpoint.as_deref()),
            "self_check": worker.transport.as_ref().map(|transport| transport.self_check.clone()),
        },
        "agent": agent,
        "tasks": tasks,
        "pending_merges": pending_merges,
        "worktrees": worktrees,
        "subscriptions": subscriptions,
        "peers": peers,
        "peer_count": peer_count,
        "master_wake": master_wake,
        "subagents": subagents,
        "summary": {
            "workers": peer_count,
            "messages": message_count,
            "tasks": task_count,
            "subagents": subagents.len(),
            "now": iso(now_ms()),
        },
        "inbox": {
            "unread": unread_count,
            "messages": inbox_messages,
        },
        "daemon": daemon_context_view(server),
        "next_actions": next_actions,
        "operations": operations,
        "master": master,
        "recorded_unusable": recorded_unusable,
        "authority": authority,
        "truth": "server journal, mailbox, and live transport probes; `collab context` performs an idempotent bootstrap (canonical root, baseline, daemon, identity, registration) before projecting this snapshot",
    }))
}
