fn dispatch_with_route_context(
    server: &Arc<Server>,
    req: Req,
    project_context: Option<ProjectContext>,
) -> Resp {
    if mutation_blocked_during_migration(&req) && server.state.lock().unwrap().admission_frozen() {
        return Resp::err(
            "MIGRATION_ADMISSION_FROZEN: only identity rebind, read queries, daemon restart, and migration verify are allowed",
        );
    }
    let unaccepted_resource_task = match &req {
        Req::TaskRelocate { task_id, .. } | Req::TaskWait { task_id, .. }
        | Req::TaskDeliver { task_id, .. } | Req::TaskReview { task_id, .. }
        | Req::TaskIntegrated { task_id, .. } | Req::TaskClose { task_id, .. }
        | Req::TaskFinalizeCleanup { task_id, .. } => Some(task_id),
        _ => None,
    };
    if let Some(task_id) = unaccepted_resource_task {
        let state = server.state.lock().unwrap();
        if state.tasks.get(task_id).is_some_and(|task| matches!(task.status.as_str(), "pending" | "invited")) {
            return Resp::err("BOARD_ACCEPT_REQUIRED: unaccepted tasks cannot bind execution resources or lifecycle evidence");
        }
    }
    let app_scope = project_context
        .as_ref()
        .map(|context| context.app_scope_id.clone());
    match req {
        Req::IdentityContext { .. } => Resp::err("IDENTITY_CONTEXT_HOST_REQUIRED: identity reconciliation belongs to the host daemon"),
        Req::BoardShow => handle_board_show(server),
        Req::Board { worker_id, token, command } => handle_board_command(server, worker_id, token, command),
        Req::SubagentObserve { id, snapshot_lines } => {
            match crate::subagent::observe(server, id.as_deref(), snapshot_lines) {
                Ok(mut value) => {
                    value["notification_channel"] = json!("none");
                    value["next_action"] = json!("No push channel for a non-App-Server agent. Check subagent status/mailbox yourself; request snapshot explicitly when useful.");
                    Resp::data(value)
                }
                Err(error) => Resp::err(error.to_string()),
            }
        }
        Req::Subagent {
            worker_id,
            token,
            command,
            launch_env: _,
        } => crate::subagent::handle_with_env(
            server,
            &worker_id,
            &token,
            command,
            std::collections::BTreeMap::new(),
        ),
        Req::Register {
            worker_id,
            token,
            cwd,
            candidates,
            ..
        } => {
            let recover_existing = project_context.as_ref().is_some_and(|context| {
                is_provisional_cli_runtime(context, &worker_id)
                    || context.runtime_context.as_ref().is_some_and(|runtime| {
                        runtime.agent_id.as_str() == worker_id
                            && server
                                .state
                                .lock()
                                .unwrap()
                                .workers
                                .get(&worker_id)
                                .is_some_and(|worker| worker.token != token)
                    })
            });
            handle_register_with_app_scope_inner(
                server,
                worker_id,
                token,
                cwd,
                app_scope,
                candidates,
                recover_existing,
            )
        }
        Req::Send {
            from,
            worker_id,
            token,
            command,
            to,
            mtype,
            subject,
            body,
            in_reply_to,
            delivery,
        } => {
            let Some(worker_id) = worker_id else {
                return Resp::err(
                    "LEGACY_SEND_REJECTED: authenticated sender binding and route scope are required",
                );
            };
            let Some(token) = token else {
                return Resp::err(
                    "LEGACY_SEND_REJECTED: authenticated sender binding and route scope are required",
                );
            };
            handle_authenticated_send_with_app_scope(
                server,
                from,
                worker_id,
                token,
                command,
                to,
                mtype,
                subject,
                body,
                in_reply_to,
                delivery,
                app_scope,
            )
        }
        Req::LiveClosureDaemonSend {
            worker_id,
            token,
            to,
            path,
            subject,
            body,
            restart_replay_pending,
        } => handle_live_closure_daemon_send(
            server,
            worker_id,
            token,
            to,
            path,
            subject,
            body,
            restart_replay_pending,
        ),
        Req::CrossProjectSend { .. } => Resp::err(
            "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: cross-project wire requests require independently verified source and target route owners",
        ),
        Req::NotificationMethods => Resp::data(json!({
            "methods": ["appserver"],
            "priority": ["appserver"],
            "events": NOTIFICATION_EVENTS,
            "one_shot": false,
            "max_lifetime_attempts": MAX_WAKE_ATTEMPTS,
            "max_repeat_count": crate::server::state::MAX_NOTIFICATION_REPEATS,
            "max_active_subscriptions_per_agent": MAX_ACTIVE_SUBSCRIPTIONS_PER_WORKER,
            "max_ttl_seconds": MAX_NOTIFICATION_TTL_SECONDS,
        })),
        Req::NotificationSubscribe {
            worker_id,
            token,
            event,
            subject,
            trigger_ms,
            trigger_times_ms,
            interval_ms,
            repeat_count,
            ttl_seconds,
        } => handle_notification_subscribe(
            server,
            worker_id,
            token,
            event,
            subject,
            trigger_ms,
            trigger_times_ms,
            interval_ms,
            repeat_count,
            ttl_seconds,
        ),
        Req::NotificationStatus { worker_id, token } => {
            handle_notification_status(server, worker_id, token)
        }
        Req::NotificationUnsubscribe {
            worker_id,
            token,
            subscription_id,
        } => handle_notification_unsubscribe(server, worker_id, token, subscription_id),
        Req::Poll { .. } => {
            Resp::err("Poll is only handled by the async daemon connection path; use collab recv")
        }
        Req::Ack {
            worker_id,
            token,
            ids,
        } => {
            let mut st = server.state.lock().unwrap();
            if let Err(e) = verify(&st, &worker_id, &token) {
                return e;
            }
            let mut acked = Vec::new();
            let mut already_acked = Vec::new();
            let mut not_found = Vec::new();
            let mut restored_msgs = Vec::new();

            if ids.is_empty() {
                for m in st.msgs.values() {
                    if m.to == worker_id {
                        if m.state == "delivered" || m.state == "pending" {
                            acked.push(m.id.clone());
                        } else if m.state == "read" {
                            already_acked.push(m.id.clone());
                        }
                    }
                }
            } else {
                for id in ids {
                    let in_mem = st.msgs.get(&id).cloned();
                    let msg = in_mem.or_else(|| {
                        let path = server
                            .storage_root
                            .join(".agent-collab")
                            .join("mailbox")
                            .join(format!("{}.json", id));
                        std::fs::read_to_string(&path)
                            .ok()
                            .and_then(|s| serde_json::from_str::<Message>(&s).ok())
                    });
                    match msg {
                        Some(m) if m.to == worker_id => {
                            if !st.msgs.contains_key(&id) {
                                restored_msgs.push(m.clone());
                            }
                            if m.state == "delivered" || m.state == "pending" {
                                acked.push(id);
                            } else {
                                already_acked.push(id);
                            }
                        }
                        _ => {
                            not_found.push(id);
                        }
                    }
                }
            }

            let mut events = Vec::new();
            for m in restored_msgs {
                events.push(Event::Sent { msg: m });
            }
            if !acked.is_empty() {
                events.push(Event::Acked { ids: acked.clone() });
            }

            // An explicit or bulk ACK from an authenticated worker proves
            // the worker is responsive and active. Clear keepalive unacked counter.
            if let Some(record) = st.keepalives.get(&worker_id).cloned() {
                if record.unacked > 0 || record.last_notice_id.is_some() || record.suspected_offline
                {
                    let mut updated = record;
                    let now = crate::server::state::now_ms();
                    updated.unacked = 0;
                    updated.last_notice_id = None;
                    updated.suspected_offline = false;
                    updated.activity_ms = now;
                    events.push(Event::KeepaliveUpdated {
                        worker_id: worker_id.clone(),
                        record: updated,
                    });
                }
            }

            if !events.is_empty() {
                if let Err(error) = server.commit_locked(&mut st, &events) {
                    drop(st);
                    return Resp::err(format!("ACK_DURABILITY_FAILED: {error}"));
                }
            }
            drop(st);
            Resp::data(json!({
                "acked": acked,
                "already_acked": already_acked,
                "not_found": not_found,
            }))
        }
        Req::Inbox { worker_id, token } => {
            let st = server.state.lock().unwrap();
            if let Err(e) = verify(&st, &worker_id, &token) {
                return e;
            }
            let inbox: Vec<&Message> = st.inbox_of(&worker_id);
            let items: Vec<serde_json::Value> = inbox
                .iter()
                .map(|m| {
                    json!({
                        "id": m.id, "from": m.from, "type": m.mtype,
                        "subject": m.subject,
                        "state": m.state, "created_at": iso(m.created_ms),
                        "body": m.body,
                    })
                })
                .collect();
            Resp::data(json!({"unread": items.len(), "messages": items}))
        }
        Req::Context { worker_id, token } => handle_context(server, worker_id, token),
        Req::RouteResolve { .. } | Req::RouteResolvePaneRecovery { .. } | Req::RouteResolveNative { .. } => {
            Resp::err("RouteResolve/RouteResolveNative are only handled by the host daemon connection path")
        }
        Req::MsgStatus { msg_id } => {
            let st = server.state.lock().unwrap();
            let in_mem = st.msgs.get(&msg_id).cloned();
            let transport_evidence = st.notification_delivery_evidence.get(&msg_id).cloned();
            let msg = in_mem.or_else(|| {
                let path = server
                    .storage_root
                    .join(".agent-collab")
                    .join("mailbox")
                    .join(format!("{}.json", msg_id));
                std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<Message>(&s).ok())
            });
            let answered = st.answered(&msg_id);
            let consumed = st.receive_receipts.values().any(|receipt| {
                receipt.message_ids.iter().any(|received| received == &msg_id)
            });
            drop(st);
            match msg {
                Some(m) => Resp::data(json!({
                    "id": m.id, "from": m.from, "to": m.to, "type": m.mtype,
                    "subject": m.subject, "body": m.body,
                    "state": m.state, "wake_attempts": m.wake_attempt_count,
                    "created_at": iso(m.created_ms), "answered": answered,
                    "wake_transport_evidence": transport_evidence,
                    "consumed_by_recv": consumed,
                })),
                None => Resp::err(format!("message {} not found", msg_id)),
            }
        }
        Req::TaskRegister {
            worker_id,
            token,
            task_id,
            owner,
            feature_id,
            worktree_path,
            branch,
            base_commit,
            priority,
            next_step,
            goal_prompt,
        } => handle_task_register_with_next(
            server,
            worker_id,
            token,
            task_id,
            owner,
            feature_id,
            worktree_path,
            branch,
            base_commit,
            priority,
            next_step,
            goal_prompt,
        ),
        Req::TaskRelocate {
            worker_id,
            token,
            task_id,
            worktree_path,
            branch,
            base_commit,
        } => handle_task_relocate(
            server,
            worker_id,
            token,
            task_id,
            worktree_path,
            branch,
            base_commit,
        ),
        Req::TaskUpdate {
            worker_id,
            token,
            task_id,
            status,
            next_step,
        } => handle_task_update(server, worker_id, token, task_id, status, next_step),
        Req::TaskAccept {
            worker_id,
            token,
            task_id,
        } => handle_task_accept(server, worker_id, token, task_id),
        Req::TaskClaim {
            worker_id,
            token,
            task_id,
        } => handle_task_claim(server, worker_id, token, task_id),
        Req::TaskWait {
            worker_id,
            token,
            task_id,
            blocking_task_id,
        } => handle_task_wait(server, worker_id, token, task_id, blocking_task_id),
        Req::TaskDeliver {
            worker_id,
            token,
            task_id,
            evidence,
            worktree,
        } => handle_task_deliver(server, worker_id, token, task_id, evidence, worktree),
        Req::TaskReview {
            worker_id,
            token,
            task_id,
            accept,
            rework,
            evidence,
        } => handle_task_review(server, worker_id, token, task_id, accept, rework, evidence),
        Req::TaskIntegrated {
            worker_id,
            token,
            task_id,
            commit,
            evidence,
        } => handle_task_integrated(server, worker_id, token, task_id, commit, evidence),
        Req::TaskClose {
            worker_id,
            token,
            task_id,
            force,
            reason,
        } => handle_task_close(server, worker_id, token, task_id, force, reason),
        Req::TaskFinalizeCleanup {
            worker_id,
            token,
            task_id,
        } => handle_task_finalize_cleanup(server, worker_id, token, task_id),
        Req::TaskDispatch { worker_id, token } => handle_task_dispatch(server, worker_id, token),
        Req::TaskStatus { task_id } => {
            let st = server.state.lock().unwrap();
            match task_id {
                Some(id) => st
                    .tasks
                    .get(&id)
                    .map(|task| task_view(&st, task))
                    .map(Resp::data)
                    .unwrap_or_else(|| Resp::err(format!("task {} not found", id))),
                None => Resp::data(
                    json!({"tasks": st.tasks.values().map(|task| task_view(&st, task)).collect::<Vec<_>>() }),
                ),
            }
        }
        Req::TaskConflicts {
            feature_id,
            worktree_path,
        } => task_conflicts(server, feature_id, worktree_path),
        Req::MigrationInspect { worker_id, token } => {
            handle_migration_inspect(server, worker_id, token)
        }
        Req::MigrationPlan { worker_id, token } => handle_migration_plan(server, worker_id, token),
        Req::MigrationApply { worker_id, token } => {
            handle_migration_apply(server, worker_id, token)
        }
        Req::MigrationVerify { worker_id, token } => {
            handle_migration_verify(server, worker_id, token)
        }
        Req::MasterPromote {
            worker_id,
            token,
            approval,
        } => handle_master_promote(server, worker_id, token, approval),
        Req::MasterDelegate {
            worker_id,
            token,
            target_id,
        } => handle_master_delegate(server, worker_id, token, target_id),
        Req::MasterStatus => handle_master_status(server),
        Req::Role { worker_id: _ } => {
            Resp::err("declared roles are removed; use collab who/context for peer identity")
        }
        Req::Workers => {
            record_ordinary_peer_presence_edges(server, None);
            let (workers_rec, tasks_map, msgs_map, keepalives_map, role_briefs) = {
                let st = server.state.lock().unwrap();
                let role_briefs = st
                    .workers
                    .keys()
                    .map(|worker_id| (worker_id.clone(), role_brief(server, &st, worker_id)))
                    .collect::<std::collections::HashMap<_, _>>();
                (
                    st.workers.values().cloned().collect::<Vec<_>>(),
                    st.tasks.clone(),
                    st.msgs.clone(),
                    st.keepalives.clone(),
                    role_briefs,
                )
            };
            let mut workers: Vec<serde_json::Value> = workers_rec
                .iter()
                .map(|w| {
                    worker_status_summary_with_maps(
                        server,
                        &tasks_map,
                        &msgs_map,
                        &keepalives_map,
                        role_briefs.get(&w.id).cloned().unwrap_or_default(),
                        w,
                    )
                })
                .collect();
            workers.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
            Resp::data(json!({
                "workers": workers,
                "count": workers.len()
            }))
        }
        Req::WorkerClose {
            worker_id,
            token,
            target_id,
            reason,
        } => handle_worker_close(server, worker_id, token, target_id, reason),
        Req::WorkerStatus { worker_id } => {
            record_ordinary_peer_presence_edges(server, worker_id.as_deref());
            let (workers_rec, tasks_map, msgs_map, keepalives_map, role_briefs) = {
                let st = server.state.lock().unwrap();
                let role_briefs = st
                    .workers
                    .keys()
                    .map(|worker_id| (worker_id.clone(), role_brief(server, &st, worker_id)))
                    .collect::<std::collections::HashMap<_, _>>();
                (
                    st.workers.values().cloned().collect::<Vec<_>>(),
                    st.tasks.clone(),
                    st.msgs.clone(),
                    st.keepalives.clone(),
                    role_briefs,
                )
            };
            let mut workers: Vec<serde_json::Value> = workers_rec
                .iter()
                .filter(|w| worker_id.as_ref().is_none_or(|id| id == &w.id))
                .map(|w| {
                    worker_status_summary_with_maps(
                        server,
                        &tasks_map,
                        &msgs_map,
                        &keepalives_map,
                        role_briefs.get(&w.id).cloned().unwrap_or_default(),
                        w,
                    )
                })
                .collect();
            workers.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
            Resp::data(json!({
                "workers": workers,
                "count": workers.len()
            }))
        }
        Req::WorkerSnapshot {
            worker_id,
            token,
            target_id,
            lines,
        } => handle_worker_snapshot(server, worker_id, token, target_id, lines),
        Req::MasterId => handle_master_status(server),
        Req::MasterRecover {
            worker_id: _,
            token: _,
            session: _,
        } => Resp::err("master recovery is deprecated; re-register the peer identity"),
        Req::TransferMaster {
            worker_id: _,
            token: _,
            target_id: _,
        } => Resp::err("master transfer is deprecated; authority is task-scoped"),
        Req::RemoveWorker {
            worker_id: _,
            token: _,
            target_id: _,
            force: _,
        } => Resp::err("remove-worker is deprecated; use task-owner cleanup and migration verify"),
        Req::ResetBindings { confirm: _ } => Resp::err(
            "binding reset is deprecated; preserve journal/mailbox and use migration rebind",
        ),
        Req::Shutdown { operator } if operator => Resp::data(json!({
            "authorized": true,
            "capability": "daemon-operator",
        })),
        Req::Shutdown { .. } => Resp::err("shutdown requires an explicit daemon-operator action"),
        Req::Ping => {
            let st = server.state.lock().unwrap();
            Resp::data(json!({
                "workers": st.workers.len(),
                "messages": st.msgs.len(),
                "tasks": st.tasks.len(),
                "now": iso(now_ms()),
            }))
        }
        Req::StatusAll => {
            record_ordinary_peer_presence_edges(server, None);
            let (
                workers_rec,
                tasks,
                subagents,
                msgs_len,
                tasks_map,
                msgs_map,
                keepalives_map,
                master_wake,
                pending_merges,
                now,
                role_briefs,
            ) = {
                let st = server.state.lock().unwrap();
                let workers_rec: Vec<WorkerRec> = st.workers.values().cloned().collect();
                let role_briefs = st
                    .workers
                    .keys()
                    .map(|worker_id| (worker_id.clone(), role_brief(server, &st, worker_id)))
                    .collect::<std::collections::HashMap<_, _>>();
                let mut tasks: Vec<serde_json::Value> =
                    st.tasks.values().map(|task| task_view(&st, task)).collect();
                tasks.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
                let mut subagents: Vec<crate::subagent::Record> =
                    st.subagents.values().cloned().collect();
                subagents.sort_by(|a, b| a.id.cmp(&b.id));
                let msgs_len = st.msgs.len();
                let now = now_ms();
                let pending_merges = pending_merge_views(&st);
                (
                    workers_rec,
                    tasks,
                    subagents,
                    msgs_len,
                    st.tasks.clone(),
                    st.msgs.clone(),
                    st.keepalives.clone(),
                    st.master_wake.clone(),
                    pending_merges,
                    now,
                    role_briefs,
                )
            };
            let mut workers: Vec<serde_json::Value> = workers_rec
                .iter()
                .map(|w| {
                    worker_status_summary_with_maps(
                        server,
                        &tasks_map,
                        &msgs_map,
                        &keepalives_map,
                        role_briefs.get(&w.id).cloned().unwrap_or_default(),
                        w,
                    )
                })
                .collect();
            workers.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));

            Resp::data(json!({
                "summary": {
                    "workers": workers.len(),
                    "messages": msgs_len,
                    "tasks": tasks.len(),
                    "subagents": subagents.len(),
                    "now": iso(now),
                },
                "master_wake": master_wake,
                "pending_merges": pending_merges,
                "workers": workers,
                "tasks": tasks,
                "subagents": subagents,
            }))
        }
        Req::MailboxRead {
            all,
            sort,
            worker_id,
        } => {
            let st = server.state.lock().unwrap();
            let mut msgs: Vec<Message> = st
                .msgs
                .values()
                .filter(|m| {
                    if all {
                        true
                    } else if let Some(wid) = &worker_id {
                        &m.to == wid || &m.from == wid
                    } else {
                        true
                    }
                })
                .cloned()
                .collect();
            let recipient_messages = worker_id.as_deref().map(|recipient| {
                st.msgs
                    .values()
                    .filter(|message| message.to == recipient)
                    .cloned()
                    .collect::<Vec<_>>()
            });
            let sort_order = sort.as_deref().unwrap_or("time-asc");
            if sort_order == "time-desc" {
                msgs.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
            } else {
                msgs.sort_by(|a, b| a.created_ms.cmp(&b.created_ms));
            }
            let count = msgs.len();
            drop(st);
            let projection = worker_id.as_deref().and_then(|recipient| {
                let path = server
                    .storage_root
                    .join(".agent-collab/mailbox")
                    .join(format!("recipient-{recipient}.jsonl"));
                match read_recipient_mailbox(&path, recipient) {
                    Ok(read) => {
                        let missing = missing_recipient_projection_messages(
                            recipient_messages.as_deref().unwrap_or_default(),
                            recipient,
                            &read,
                        );
                        let status = if read.partial_tail {
                            "partial-tail"
                        } else if !read.recoverable_errors.is_empty() {
                            "recoverable-error"
                        } else if missing.is_empty() {
                            "ok"
                        } else {
                            "incomplete"
                        };
                        let mut exact_errors = read.recoverable_errors.clone();
                        if !missing.is_empty() {
                            exact_errors.push(format!(
                                "recipient JSONL is missing message records: {}",
                                missing.join(",")
                            ));
                        }
                        let exact_error =
                            (!exact_errors.is_empty()).then(|| exact_errors.join(" | "));
                        Some(json!({
                            "status": status,
                            "partial_tail": read.partial_tail,
                            "recoverable_errors": read.recoverable_errors,
                            "missing_message_ids": missing,
                            "exact_error": exact_error,
                            "records": read.records,
                        }))
                    }
                    Err(error) => Some(json!({
                        "status": "error",
                        "exact_error": error,
                        "records": [],
                    })),
                }
            });
            let mut response = json!({
                "count": count,
                "sort": sort_order,
                "messages": msgs,
            });
            if let Some(projection) = projection {
                response["recipient_jsonl"] = projection;
            }
            Resp::data(response)
        }
    }
}

fn dispatch(server: &Arc<Server>, req: Req) -> Resp {
    dispatch_with_route_context(server, req, None)
}

fn request_requires_project_context(req: &Req) -> bool {
    !matches!(
        req,
        Req::Ping | Req::RouteResolve { .. } | Req::RouteResolvePaneRecovery { .. } | Req::RouteResolveNative { .. }
    )
}

enum WireRoutePrincipal<'a> {
    Authenticated { worker_id: &'a str },
    Selected { worker_id: &'a str },
}

fn wire_route_principals(req: &Req) -> Result<Vec<WireRoutePrincipal<'_>>, String> {
    match req {
        Req::Board { worker_id, .. }
        | Req::Subagent { worker_id, .. }
        | Req::Register { worker_id, .. }
        | Req::LiveClosureDaemonSend { worker_id, .. }
        | Req::NotificationSubscribe { worker_id, .. }
        | Req::NotificationStatus { worker_id, .. }
        | Req::NotificationUnsubscribe { worker_id, .. }
        | Req::Poll { worker_id, .. }
        | Req::Ack { worker_id, .. }
        | Req::Inbox { worker_id, .. }
        | Req::Context { worker_id, .. }
        | Req::TaskRegister { worker_id, .. }
        | Req::TaskRelocate { worker_id, .. }
        | Req::TaskUpdate { worker_id, .. }
        | Req::TaskAccept { worker_id, .. }
        | Req::TaskClaim { worker_id, .. }
        | Req::TaskWait { worker_id, .. }
        | Req::TaskDeliver { worker_id, .. }
        | Req::TaskReview { worker_id, .. }
        | Req::TaskIntegrated { worker_id, .. }
        | Req::TaskClose { worker_id, .. }
        | Req::TaskFinalizeCleanup { worker_id, .. }
        | Req::TaskDispatch { worker_id, .. }
        | Req::MigrationInspect { worker_id, .. }
        | Req::MigrationPlan { worker_id, .. }
        | Req::MigrationApply { worker_id, .. }
        | Req::MigrationVerify { worker_id, .. }
        | Req::MasterPromote { worker_id, .. }
        | Req::MasterDelegate { worker_id, .. }
        | Req::WorkerClose { worker_id, .. }
        | Req::WorkerSnapshot { worker_id, .. }
        | Req::MasterRecover { worker_id, .. }
        | Req::TransferMaster { worker_id, .. }
        | Req::RemoveWorker { worker_id, .. } => {
            Ok(vec![WireRoutePrincipal::Authenticated { worker_id }])
        }
        Req::Send {
            worker_id, token, ..
        } => match (worker_id.as_deref(), token.as_deref()) {
            (Some(worker_id), Some(_)) => Ok(vec![WireRoutePrincipal::Authenticated { worker_id }]),
            (None, None) => Ok(Vec::new()),
            _ => Err(
                "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: wire send requires worker_id and token together"
                    .into(),
            ),
        },
        Req::Role { worker_id } => Ok(vec![WireRoutePrincipal::Selected { worker_id }]),
        Req::WorkerStatus {
            worker_id: Some(worker_id),
        }
        | Req::MailboxRead {
            worker_id: Some(worker_id),
            ..
        } => Ok(vec![WireRoutePrincipal::Selected { worker_id }]),
        _ => Ok(Vec::new()),
    }
}

fn validate_wire_route_principals(
    server: &Server,
    req: &Req,
    route_scope: &RouteScope,
) -> Result<(), String> {
    let principals = wire_route_principals(req)?;
    if principals.is_empty() {
        return Ok(());
    }

    let state = server.state.lock().unwrap();
    for principal in principals {
        let worker_id = match principal {
            WireRoutePrincipal::Authenticated { worker_id, .. }
            | WireRoutePrincipal::Selected { worker_id } => worker_id,
        };
        let bindings = state
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .filter(|binding| binding.agent_id.as_str() == worker_id)
            .collect::<Vec<_>>();

        // A new wire Register is the only request allowed to create its first
        // binding.  A binding in another project remains a route conflict even
        // when the resident legacy worker projection is absent.  A unique
        // binding on the exact incoming route is the recovery shape after a
        // daemon restart, so let the existing register preflight validate its
        // token and runtime identity.
        if matches!(req, Req::Register { .. }) && state.workers.get(worker_id).is_none() {
            if bindings.is_empty() {
                continue;
            }
            if bindings.len() != 1
                || bindings[0].app_scope_id != route_scope.app_scope_id
                || bindings[0].project_scope != route_scope.project_scope_id
            {
                return Err(format!(
                    "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: worker {} is already bound to another project route",
                    worker_id
                ));
            }
            continue;
        }

        let mut routes = std::collections::BTreeSet::new();
        for binding in bindings {
            routes.insert((
                binding.app_scope_id.as_str().to_owned(),
                binding.project_scope.as_str().to_owned(),
            ));
        }
        let Some((app_scope_id, project_scope_id)) = routes.iter().next() else {
            return Err(format!(
                "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: worker {} has no registered runtime binding",
                worker_id
            ));
        };
        if routes.len() != 1
            || app_scope_id != route_scope.app_scope_id.as_str()
            || project_scope_id != route_scope.project_scope_id.as_str()
        {
            return Err(format!(
                "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: worker {} runtime binding route does not match request route",
                worker_id
            ));
        }

        let Some(worker) = state.workers.get(worker_id) else {
            return Err(format!(
                "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: worker {} has no resident identity",
                worker_id
            ));
        };
        let worker_scope =
            GlobalState::canonical_project_scope(Path::new(&worker.cwd)).map_err(|error| {
                format!(
                    "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: worker {} cwd is not a project route: {}",
                    worker_id, error
                )
            })?;
        if worker_scope != route_scope.project_scope_id {
            return Err(format!(
                "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: worker {} cwd does not match request project route",
                worker_id
            ));
        }

        // Authenticated handlers retain the established token check.  This
        // admission only binds the worker identity to the incoming route;
        // direct in-process callers continue to use the legacy handlers.
    }
    Ok(())
}

/// Validate the route carried by a host-daemon request before the legacy
/// project reducer sees it. Registration is host-wide: an initialized project
/// may register its exact canonical root with the resident daemon even when it
/// is not the daemon's startup project. Operations for that route remain
/// explicitly not-ready until a project reducer/journal owner is available.
pub(crate) fn validate_request_context(
    server: &Server,
    req: &Req,
    project_context: Option<&ProjectContext>,
) -> Result<(), String> {
    let Some(project_context) = project_context else {
        if request_requires_project_context(req) {
            return Err(
                "PROJECT_CONTEXT_REQUIRED: canonical project root and scope are required".into(),
            );
        }
        return Ok(());
    };

    project_context
        .validate()
        .map_err(|error| format!("PROJECT_CONTEXT_INVALID: {error}"))?;
    if matches!(req, Req::Shutdown { operator: true }) {
        return validate_host_operator_route(server, project_context);
    }
    let registry = HostRouteRegistry::for_server(server)?;
    let route_scope = RouteScope {
        app_scope_id: project_context.app_scope_id.clone(),
        project_scope_id: project_context.project_scope.clone(),
    };
    match registry.lookup(project_context) {
        Some(HostRouteOwner::ResidentProject { root, .. }) => {
            if project_context.canonical_root != root.to_string_lossy() {
                return Err(format!(
                    "PROJECT_SCOPE_MISMATCH: route root {} does not match context {}",
                    root.display(),
                    project_context.canonical_root
                ));
            }
            if let Req::Register { cwd, .. } = req {
                validate_register_cwd(cwd, root)?;
            }
        }
        Some(HostRouteOwner::RegisteredNotReady { root }) => {
            if let Req::Register { cwd, .. } = req {
                validate_project_registration_cwd(cwd, root)?;
            } else {
                return Err(format!(
                    "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: project route {} is registered but has no migrated reducer/journal owner",
                    root.display()
                ));
            }
        }
        None => {
            // Registration is the one operation that may create the first
            // app route for any initialized project. It still has to carry an
            // explicit app scope, use the exact canonical project root, and
            // prove that the project opted into Collab with its marker.
            if let Req::Register { cwd, .. } = req {
                validate_project_registration_cwd(cwd, Path::new(&project_context.canonical_root))?;
            } else {
                return Err(format!(
                    "PROJECT_SCOPE_UNKNOWN: no host route is registered for {}",
                    project_context.canonical_root
                ));
            }
        }
    }

    if matches!(req, Req::CrossProjectSend { .. }) {
        return Err(
            "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: cross-project wire requests require independently verified source and target route owners".into(),
        );
    }
    validate_wire_route_principals(server, req, &route_scope)?;
    validate_wire_runtime_binding(server, req, project_context)
}

/// Validate the explicit project-registration boundary. The marker is the
/// project owner's opt-in to Collab; without it, an arbitrary canonical path
/// must not create a durable host route. The cwd remains exact and cannot be
/// replaced by a daemon cwd, ancestor, or worktree path.
fn validate_project_registration_cwd(cwd: &str, expected_root: &Path) -> Result<(), String> {
    if !expected_root.join(".agent-collab").is_dir() {
        return Err(format!(
            "PROJECT_SCOPE_UNKNOWN: project {} is not initialized for Collab",
            expected_root.display()
        ));
    }
    reject_linked_worktree_registration(expected_root)?;
    validate_register_cwd(cwd, expected_root)
}

fn reject_linked_worktree_registration(root: &Path) -> Result<(), String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("PROJECT_SCOPE_INVALID: registration root: {error}"))?;
    if crate::scope::is_linked_worktree_root(&root)
        .map_err(|error| format!("PROJECT_SCOPE_INVALID: {error}"))?
    {
        return Err(format!(
            "PROJECT_SCOPE_INVALID: linked worktree {} cannot own Collab identity; register the canonical main checkout",
            root.display()
        ));
    }
    Ok(())
}

/// Admit the explicit CLI host operator route independently of project
/// registration. `collab up` can create a resident daemon before any app
/// route has registered; the operator still needs a way to stop that daemon.
/// Keep this path read-only and exact so it cannot become an unknown-route
/// fallback for project requests or create a peer identity as a side effect.
fn validate_host_operator_route(
    server: &Server,
    project_context: &ProjectContext,
) -> Result<(), String> {
    let resident_scope = GlobalState::canonical_project_scope(&server.root)
        .map_err(|error| format!("PROJECT_SCOPE_UNKNOWN: {error}"))?;
    if project_context.app_scope_id.as_str() != crate::identity::CLI_APP_SERVER_ID {
        return Err(format!(
            "PROJECT_SCOPE_UNKNOWN: host operator route requires app scope {}",
            crate::identity::CLI_APP_SERVER_ID
        ));
    }
    if project_context.canonical_root != resident_scope.as_str()
        || project_context.project_scope != resident_scope
    {
        return Err(format!(
            "PROJECT_SCOPE_UNKNOWN: host operator route must target resident project {}",
            resident_scope.as_str()
        ));
    }
    Ok(())
}

fn validate_register_cwd(cwd: &str, expected_root: &Path) -> Result<(), String> {
    let request_scope = GlobalState::canonical_project_scope(Path::new(cwd))
        .map_err(|error| format!("PROJECT_SCOPE_INVALID: {error}"))?;
    if request_scope.as_str() != expected_root.to_string_lossy() {
        return Err(format!(
            "PROJECT_SCOPE_MISMATCH: register cwd {} is outside {}",
            cwd,
            expected_root.display()
        ));
    }
    Ok(())
}
