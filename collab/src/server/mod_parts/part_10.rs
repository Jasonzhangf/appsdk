fn poll_messages_with_context(
    server: &Server,
    worker_id: &str,
    token: Option<&str>,
    project_context: Option<&ProjectContext>,
    receive_id: Option<&str>,
) -> Option<Resp> {
    let ids: Vec<String>;
    let mut st = server.state.lock().unwrap();
    match (token, project_context) {
        (Some(token), Some(project_context)) => {
            if let Err(response) = project_route_actor(&st, project_context, worker_id, token) {
                return Some(response);
            }
        }
        (None, None) => {}
        _ => return Some(Resp::err(
            "PROJECT_CONTEXT_REQUIRED: poll runtime admission requires token and project context",
        )),
    }
    let route_scope = project_context.map(|context| RouteScope {
        app_scope_id: context.app_scope_id.clone(),
        project_scope_id: context.project_scope.clone(),
    });
    if let Some(receive_id) = receive_id {
        if let Err(error) = crate::identity::validate_id_for_protocol(receive_id) {
            return Some(Resp::err(format!("RECEIVE_IDENTITY_INVALID: {error}")));
        }
        if let Some(receipt) = st.receive_receipts.get(receive_id).cloned() {
            if receipt.worker_id != worker_id {
                return Some(Resp::err(
                    "RECEIVE_IDENTITY_MISMATCH: receive identity belongs to another actor",
                ));
            }
            if receipt.route_scope != route_scope {
                return Some(Resp::err(
                    "RECEIVE_ROUTE_MISMATCH: receive identity belongs to another project route",
                ));
            }
            // The receipt stores identities, not a second copy of the body.
            // Retention owns message lifetime, so a purged batch reports its
            // expiry instead of resurrecting or retaining expired payload.
            let Some(messages) = receipt
                .message_ids
                .iter()
                .map(|id| st.msgs.get(id).cloned())
                .collect::<Option<Vec<_>>>()
            else {
                return Some(Resp::err_data(
                    "RECEIVE_BATCH_EXPIRED: the committed receive batch is no longer retained",
                    json!({
                        "receive_id": receipt.receive_id,
                        "expired": true,
                        "count": 0,
                    }),
                ));
            };
            return Some(Resp::data(json!({
                "messages": messages,
                "count": receipt.message_ids.len(),
                "receive_id": receipt.receive_id,
                "replayed": true,
                "fetched_at": iso(receipt.received_ms),
            })));
        }
    }
    let unread = st.inbox_of(worker_id);
    if unread.is_empty() {
        return None;
    }
    ids = unread.iter().map(|m| m.id.clone()).collect();
    // recv is an explicit read operation: deliver and consume the same batch
    // atomically so a successful read cannot leave a new ACK obligation. The
    // receive receipt commits in the same transaction, so a lost socket
    // response replays the exact batch instead of stranding it.
    let received_ms = now_ms();
    let mut events = Vec::new();
    if let Some(receive_id) = receive_id {
        events.push(Event::ReceiveCommitted {
            receipt: state::ReceiveReceipt {
                receive_id: receive_id.to_owned(),
                worker_id: worker_id.to_owned(),
                route_scope,
                message_ids: ids.clone(),
                received_ms,
            },
            ids: ids.clone(),
        });
    } else {
        events.push(Event::Delivered { ids: ids.clone() });
        events.push(Event::Acked { ids: ids.clone() });
    }
    if let Some(record) = st.keepalives.get(worker_id).cloned() {
        if record.unacked > 0 || record.last_notice_id.is_some() || record.suspected_offline {
            let mut updated = record;
            updated.unacked = 0;
            updated.last_notice_id = None;
            updated.suspected_offline = false;
            updated.activity_ms = now_ms();
            events.push(Event::KeepaliveUpdated {
                worker_id: worker_id.to_owned(),
                record: updated,
            });
        }
    }
    if let Err(error) = server.commit_locked(&mut st, &events) {
        drop(st);
        return Some(Resp::err(format!("RECV_DURABILITY_FAILED: {error}")));
    }
    // Project from the committed reducer state so the first response and a
    // receipt replay answer from the same owner. Retention still owns the
    // body; the receipt keeps only the message identities.
    let Some(msgs) = ids
        .iter()
        .map(|id| st.msgs.get(id).cloned())
        .collect::<Option<Vec<_>>>()
    else {
        return Some(Resp::err_data(
            "RECEIVE_BATCH_EXPIRED: the committed receive batch is no longer retained",
            json!({
                "receive_id": receive_id,
                "expired": true,
                "count": 0,
            }),
        ));
    };
    Some(Resp::data(json!({
        "messages": msgs,
        "count": msgs.len(),
        "receive_id": receive_id,
        "replayed": false,
        "fetched_at": iso(received_ms),
    })))
}

async fn poll_messages_async_with_context(
    server: Arc<Server>,
    worker_id: &str,
    token: Option<String>,
    project_context: Option<ProjectContext>,
    receive_id: Option<String>,
) -> Option<Resp> {
    let worker_id = worker_id.to_owned();
    tokio::task::spawn_blocking(move || {
        poll_messages_with_context(
            &server,
            &worker_id,
            token.as_deref(),
            project_context.as_ref(),
            receive_id.as_deref(),
        )
    })
    .await
    .unwrap_or_else(|error| Some(Resp::err(format!("poll handler join error: {}", error))))
}

async fn handle_poll_async(server: Arc<Server>, worker_id: String, timeout_ms: u64) -> Resp {
    handle_poll_async_with_context(server, worker_id, None, timeout_ms, None, None).await
}

async fn handle_poll_async_with_context(
    server: Arc<Server>,
    worker_id: String,
    token: Option<String>,
    timeout_ms: u64,
    project_context: Option<ProjectContext>,
    receive_id: Option<String>,
) -> Resp {
    let timeout_ms = timeout_ms.min(MAX_POLL_MS);
    let mut notified = Box::pin(server.mailbox_notify.notified());
    let timeout = tokio::time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(timeout);
    loop {
        notified.as_mut().enable();
        if let Some(response) = poll_messages_async_with_context(
            server.clone(),
            &worker_id,
            token.clone(),
            project_context.clone(),
            receive_id.clone(),
        )
        .await
        {
            return response;
        }
        tokio::select! {
            _ = notified.as_mut() => {
                notified.set(server.mailbox_notify.notified());
            }
            _ = &mut timeout => {
                return Resp::data(json!({"messages": [], "count": 0, "timeout": true}));
            }
        }
    }
}

fn task_conflicts(
    server: &Server,
    feature_id: Option<String>,
    worktree_path: Option<String>,
) -> Resp {
    let st = server.state.lock().unwrap();
    let conflicts: Vec<serde_json::Value> = st
        .tasks
        .values()
        .filter(|task| {
            task_resource_active(&task.status)
                && ((feature_id.is_some() && task.feature_id == feature_id)
                    || (worktree_path.is_some() && task.worktree_path == worktree_path))
        })
        .map(|task| task_view(&st, task))
        .collect();
    Resp::data(json!({"conflicts": conflicts}))
}

fn handle_task_wait(
    server: &Server,
    worker_id: String,
    token: String,
    task_id: String,
    blocking_task_id: String,
) -> Resp {
    let mut st = server.state.lock().unwrap();
    let Some(worker) = st.workers.get(&worker_id).cloned() else {
        return Resp::err(format!("worker {} not registered", worker_id));
    };
    if worker.token != token {
        return Resp::err("token mismatch: identity does not own this worker_id");
    }
    let Some(mut task) = st.tasks.get(&task_id).cloned() else {
        return Resp::err(format!("task {} not found", task_id));
    };
    if task.owner != worker_id || !task_claim_held(&task.status) {
        return Resp::err("only an owned active task may enter waiting");
    }
    if matches!(
        task.status.as_str(),
        "delivered" | "accepted" | "merged" | "closed" | "cancelled"
    ) {
        return Resp::err("terminal or delivered task may not enter waiting");
    }
    let Some(blocking) = st.tasks.get(&blocking_task_id).cloned() else {
        return Resp::err(format!("blocking task {} not found", blocking_task_id));
    };
    if task.id == blocking_task_id || wait_cycle(&st.tasks, &task.id, &blocking_task_id) {
        return Resp::err("WAIT_CYCLE_DETECTED");
    }
    let conflict = task_resource_active(&blocking.status)
        && ((task.feature_id.is_some() && task.feature_id == blocking.feature_id)
            || (task.worktree_path.is_some() && task.worktree_path == blocking.worktree_path));
    if !conflict {
        return Resp::err("blocking task does not hold a matching active resource");
    }
    if blocking.owner == worker_id || !st.workers.contains_key(&blocking.owner) {
        return Resp::err("WAIT_RESPONSIBLE_ACTOR_MISSING");
    }
    let responsible_actor = blocking.owner.clone();
    task.status = "waiting".into();
    task.next_step = Some(format!("WAITING_FOR={}", blocking_task_id));
    task.wait = Some(WaitSpec {
        waiter: worker_id.clone(),
        waiting_for: blocking_task_id.clone(),
        responsible_actor: responsible_actor.clone(),
        reason: "resource_conflict".into(),
        deadline_ms: now_ms() + 15 * 60 * 1000,
        resume_on: vec![
            "resource_released".into(),
            "rework".into(),
            "cancelled".into(),
        ],
        escalation: "resource_owner_and_waiter_recheck".into(),
    });
    task.updated_ms = now_ms();
    if let Err(error) = server.commit_locked(&mut st, &[Event::TaskUpdated { task: task.clone() }]) {
        drop(st);
        return Resp::err(format!("TASK_WAIT_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "task": task.id,
        "status": task.status,
        "waiting_for": blocking_task_id,
        "responsible_actor": responsible_actor,
        "deadline_ms": task.wait.as_ref().map(|wait| wait.deadline_ms),
        "notification": "none; subscribe for release/deadline or use explicit sendmessage",
    }))
}

fn worker_status_summary_with_maps(
    server: &Server,
    tasks: &std::collections::HashMap<String, TaskRec>,
    msgs: &std::collections::HashMap<String, Message>,
    keepalives: &std::collections::HashMap<String, crate::server::keepalive::Record>,
    role_brief: serde_json::Value,
    w: &WorkerRec,
) -> serde_json::Value {
    let active = tasks
        .values()
        .find(|task| task.owner == w.id && !matches!(task.status.as_str(), "closed" | "cancelled"));
    let (presence, transport_view) = worker_presence_with_view(server, w);
    let endpoint_live = presence == IdentityPresence::Present;
    let identity_valid = matches!(presence, IdentityPresence::Present | IdentityPresence::Cold);
    let thread_state = transport_view
        .get("thread_state")
        .and_then(serde_json::Value::as_str);
    let agent_state = match (presence, thread_state) {
        (IdentityPresence::Missing, _) => "absent",
        (IdentityPresence::Unknown, _) => "unknown",
        (IdentityPresence::Cold, _) => "cold",
        (IdentityPresence::Present, Some("idle")) => "idle",
        (IdentityPresence::Present, Some("working")) => "working",
        (IdentityPresence::Present, Some("active")) => "working",
        _ => "unknown",
    };
    let unacked_notifications = msgs
        .values()
        .filter(|m| m.to == w.id && m.state == "delivered")
        .count();
    let pending_notifications = msgs
        .values()
        .filter(|m| m.to == w.id && m.state == "pending")
        .count();
    let notifications_paused = false;
    let keepalive = keepalives.get(&w.id);
    let suspected_offline = keepalive.map(|k| k.suspected_offline).unwrap_or(false);
    let unacked_keepalives = keepalive.map(|k| k.unacked).unwrap_or(0);
    let status = if agent_state == "unknown" {
        "unknown"
    } else if !endpoint_live {
        "lost"
    } else if !identity_valid {
        "identity-mismatch"
    } else if suspected_offline {
        "offline"
    } else {
        agent_state
    };
    let diagnostic = if status == "unknown" {
        None
    } else if status == "lost" {
        Some("registered transport is not live; verify the tmux pane binding")
    } else if status == "identity-mismatch" {
        Some("selected transport does not prove the registered identity; verify its binding")
    } else {
        None
    };
    json!({
        "id": w.id,
        "role": role_brief["role"].clone(),
        "role_brief": role_brief,
        "transport": w.transport.as_ref().map(|transport| json!({
            "kind": transport.kind.as_str(),
            "endpoint": transport.endpoint,
            "namespace": transport.namespace,
            "session_id": transport.session_id,
            "thread_id": transport.thread_id,
            "self_check": transport.self_check,
        })),
        "status": status,
        "presence": match presence {
            IdentityPresence::Present => "present",
            IdentityPresence::Cold => "cold",
            IdentityPresence::Missing => "missing",
            IdentityPresence::Unknown => "unknown",
        },
        "endpoint_live": (presence != IdentityPresence::Unknown).then_some(endpoint_live),
        "identity_valid": (presence != IdentityPresence::Unknown).then_some(identity_valid),
        "agent_state": agent_state,
        "transport_view": transport_view,
        "unacked_notifications": unacked_notifications,
        "pending_notifications": pending_notifications,
        "notifications_paused": notifications_paused,
        "unacked_keepalives": unacked_keepalives,
        "suspected_offline": suspected_offline,
        "diagnostic": diagnostic,
        "active_task": active.map(|task| task.id.as_str()),
        "active_status": active.map(|task| task.status.as_str()),
    })
}

// ---------- dispatch ----------

fn mutation_blocked_during_migration(req: &Req) -> bool {
    match req {
        Req::BoardShow => false,
        Req::Board { command, .. } => !matches!(command, crate::board::BoardCommand::Show),
        Req::SubagentObserve { .. } => false,
        Req::Subagent { command, .. } => !matches!(
            command,
            crate::subagent::Action::List | crate::subagent::Action::Status { .. }
        ),
        Req::Send { .. }
        | Req::CrossProjectSend { .. }
        | Req::LiveClosureDaemonSend { .. }
        | Req::NotificationSubscribe { .. }
        | Req::NotificationUnsubscribe { .. }
        | Req::Poll { .. }
        | Req::Ack { .. }
        | Req::TaskRegister { .. }
        | Req::TaskRelocate { .. }
        | Req::TaskUpdate { .. }
        | Req::TaskAccept { .. }
        | Req::TaskClaim { .. }
        | Req::TaskWait { .. }
        | Req::TaskDeliver { .. }
        | Req::TaskReview { .. }
        | Req::TaskIntegrated { .. }
        | Req::TaskClose { .. }
        | Req::TaskFinalizeCleanup { .. }
        | Req::TaskDispatch { .. }
        | Req::MigrationPlan { .. }
        | Req::MigrationApply { .. }
        | Req::MasterPromote { .. }
        | Req::MasterDelegate { .. }
        | Req::TransferMaster { .. }
        | Req::RemoveWorker { .. }
        | Req::WorkerSnapshot { .. }
        | Req::WorkerClose { .. }
        | Req::ResetBindings { .. } => true,
        Req::Register { .. }
        | Req::RouteResolve { .. }
        | Req::RouteResolvePaneRecovery { .. }
        | Req::RouteResolveNative { .. }
        | Req::NotificationMethods
        | Req::NotificationStatus { .. }
        | Req::Inbox { .. }
        | Req::Context { .. }
        | Req::MsgStatus { .. }
        | Req::TaskStatus { .. }
        | Req::TaskConflicts { .. }
        | Req::MigrationInspect { .. }
        | Req::MigrationVerify { .. }
        | Req::MasterStatus
        | Req::Role { .. }
        | Req::Workers
        | Req::WorkerStatus { .. }
        | Req::MasterId
        | Req::MasterRecover { .. }
        | Req::Shutdown { .. }
        | Req::Ping
        | Req::StatusAll
        | Req::MailboxRead { .. } => false,
    }
}

fn subagent_action_mutates(action: &crate::subagent::Action) -> bool {
    !matches!(
        action,
        crate::subagent::Action::List | crate::subagent::Action::Status { .. }
    )
}

/// Return the authenticated actor for a request that can mutate the resident
/// reducer.  Read queries intentionally remain compatible with a context that
/// carries only the project route; the mutation admission below is the single
/// place that requires the actor's current runtime binding.
fn wire_mutation_principal(req: &Req) -> Option<(&str, &str)> {
    match req {
        Req::Subagent {
            worker_id,
            token,
            command,
            ..
        } if subagent_action_mutates(command) => Some((worker_id, token)),
        Req::Board { worker_id, token, .. }
        | Req::Register {
            worker_id, token, ..
        }
        | Req::Send {
            worker_id: Some(worker_id),
            token: Some(token),
            ..
        }
        | Req::LiveClosureDaemonSend {
            worker_id, token, ..
        }
        | Req::NotificationSubscribe {
            worker_id, token, ..
        }
        | Req::NotificationUnsubscribe {
            worker_id, token, ..
        }
        | Req::Poll {
            worker_id, token, ..
        }
        | Req::Ack {
            worker_id, token, ..
        }
        | Req::TaskRegister {
            worker_id, token, ..
        }
        | Req::TaskRelocate {
            worker_id, token, ..
        }
        | Req::TaskUpdate {
            worker_id, token, ..
        }
        | Req::TaskAccept {
            worker_id, token, ..
        }
        | Req::TaskClaim {
            worker_id, token, ..
        }
        | Req::TaskWait {
            worker_id, token, ..
        }
        | Req::TaskDeliver {
            worker_id, token, ..
        }
        | Req::TaskReview {
            worker_id, token, ..
        }
        | Req::TaskIntegrated {
            worker_id, token, ..
        }
        | Req::TaskClose {
            worker_id, token, ..
        }
        | Req::TaskFinalizeCleanup {
            worker_id, token, ..
        }
        | Req::TaskDispatch { worker_id, token }
        | Req::MigrationPlan { worker_id, token }
        | Req::MigrationApply { worker_id, token }
        | Req::MigrationVerify { worker_id, token }
        | Req::MasterPromote {
            worker_id, token, ..
        }
        | Req::MasterDelegate {
            worker_id, token, ..
        }
        | Req::TransferMaster {
            worker_id, token, ..
        }
        | Req::RemoveWorker {
            worker_id, token, ..
        }
        | Req::WorkerClose {
            worker_id, token, ..
        }
        | Req::WorkerSnapshot {
            worker_id, token, ..
        } => Some((worker_id, token)),
        _ => None,
    }
}

fn validate_wire_runtime_binding(
    server: &Server,
    req: &Req,
    project_context: &ProjectContext,
) -> Result<(), String> {
    if let Req::Register { worker_id, .. } = req {
        let state = server.state.lock().unwrap();
        let already_registered = state.workers.contains_key(worker_id)
            || state.global.projects.values().any(|project| {
                project
                    .runtime_bindings
                    .values()
                    .any(|binding| binding.agent_id.as_str() == worker_id)
            });
        drop(state);
        if !already_registered {
            if let Some(runtime) = project_context.runtime_context.as_ref() {
                if runtime.agent_id.as_str() != worker_id {
                    return Err(
                        "RUNTIME_BINDING_REJECTED: runtime identity does not match the registering worker"
                            .into(),
                    );
                }
            }
            return Ok(());
        }

        if let Req::Register {
            worker_id,
            token,
            candidates,
            ..
        } = req
        {
            let token_mismatch = {
                let state = server.state.lock().unwrap();
                state
                    .workers
                    .get(worker_id)
                    .is_some_and(|worker| worker.token != *token)
            };
            // A CLI process may lose its persisted runtime when its App Server
            // thread is recreated. Permit only that recovery shape or an
            // authorized same-thread token rotation to bypass the normal actor
            // check; same-token reconnects remain idempotent.
            if is_provisional_cli_runtime(project_context, worker_id) || token_mismatch {
                return validate_cli_register_rebind(
                    server,
                    project_context,
                    worker_id,
                    token,
                    candidates,
                );
            }
        }
    }

    let Some((worker_id, token)) = wire_mutation_principal(req) else {
        return Ok(());
    };
    let state = server.state.lock().unwrap();
    project_route_actor(&state, project_context, worker_id, token)
        .map(|_| ())
        .map_err(|response| {
            response.error.unwrap_or_else(|| {
                "RUNTIME_BINDING_REJECTED: runtime binding admission failed".into()
            })
        })
}

fn validate_cli_register_rebind(
    server: &Server,
    project_context: &ProjectContext,
    worker_id: &str,
    token: &str,
    candidates: &Option<TransportCandidates>,
) -> Result<(), String> {
    let route_scope = RouteScope {
        app_scope_id: project_context.app_scope_id.clone(),
        project_scope_id: project_context.project_scope.clone(),
    };
    let expected_binding_id = BindingId::new(sanitize_identifier(&format!("binding-{worker_id}")))
        .map_err(|error| format!("PROJECT_CONTEXT_INVALID: {error}"))?;
    let orphan_recovery;
    let persisted_thread_id;
    let retired_tombstone;
    {
        let state = server.state.lock().unwrap();
        if state
            .global
            .lookup_project_for_route(&route_scope)
            .is_none()
        {
            return Err(
                "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: worker has no registered runtime route"
                    .into(),
            );
        }
        let bindings = state
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .filter(|binding| binding.agent_id.as_str() == worker_id)
            .collect::<Vec<_>>();
        if bindings.len() != 1 {
            return Err(
                "RUNTIME_BINDING_REJECTED: authoritative runtime binding is ambiguous".into(),
            );
        }
        let binding = bindings[0];
        if binding.app_scope_id != route_scope.app_scope_id
            || binding.project_scope != route_scope.project_scope_id
        {
            return Err(
                "RUNTIME_BINDING_REJECTED: registered worker binding does not match the CLI route"
                    .into(),
            );
        }
        if binding.binding_id != expected_binding_id {
            return Err(
                "RUNTIME_BINDING_REJECTED: registered worker binding does not match the CLI route"
                    .into(),
            );
        }
        if binding.endpoint_generation == 0 {
            return Err(
                "RUNTIME_BINDING_REJECTED: authoritative runtime binding has no live endpoint generation"
                    .into(),
            );
        }
        binding
            .validate()
            .map_err(|error| format!("RUNTIME_BINDING_REJECTED: {error}"))?;
        persisted_thread_id = binding
            .native_thread_id
            .as_ref()
            .map(|thread_id| thread_id.as_str().to_owned());
        // A binding that holds neither an App Server thread nor a pane is an
        // explicit retirement tombstone: the claimant already lost its resource
        // to a later registrant. It owns nothing, so the persisted-identity
        // equality fence below and the orphan pane-owner requirement must not
        // apply to it. The registration still advances the tombstone's
        // generation and the later registrant takes the pane.
        retired_tombstone = binding.native_thread_id.is_none() && binding.tmux_endpoint.is_none();
        let persisted_worker = state.workers.get(worker_id);
        orphan_recovery = persisted_worker.is_none();
        if persisted_worker.is_some_and(|worker| worker.token != token) {
            let Some(candidates) = candidates.as_ref() else {
                return Err(
                    "RUNTIME_BINDING_REJECTED: CLI rebind requires the peer's current transport candidate"
                        .to_owned(),
                );
            };
            if candidates.appserver.is_some() {
                return Err(
                    "TRANSPORT_UNSUPPORTED: App Server rebind is retired; register the peer's current transport instead"
                        .to_owned(),
                );
            }
            let same_runtime_thread = is_provisional_cli_runtime(project_context, worker_id)
                || project_context
                    .runtime_context
                    .as_ref()
                    .is_some_and(|runtime| {
                        runtime.agent_id == binding.agent_id
                            && runtime.native_thread_id == binding.native_thread_id
                    });
            let candidate_owner = if let Some(candidate) = candidates.tmux.as_ref() {
                state
                    .global
                    .lookup_tmux_route(&candidate.endpoint)
                    .map(|binding| binding.agent_id.as_str())
            } else {
                None
            };
            if candidate_owner != Some(worker_id) || !same_runtime_thread {
                return Err(
                    "RUNTIME_BINDING_REJECTED: worker token does not match the registered identity"
                        .to_owned(),
                );
            }
        }
        let worker_cwd = persisted_worker
            .map(|worker| worker.cwd.as_str())
            .unwrap_or(project_context.canonical_root.as_str());
        let worker_scope =
            GlobalState::canonical_project_scope(Path::new(worker_cwd)).map_err(|error| {
                format!("RUNTIME_BINDING_REJECTED: worker cwd is not a project route: {error}")
            })?;
        if worker_scope != route_scope.project_scope_id {
            return Err(
                "RUNTIME_BINDING_REJECTED: worker cwd does not match the requested project route"
                    .into(),
            );
        }
        if orphan_recovery {
            let persisted = crate::identity::read_persisted(&server.host_paths, worker_id)
                .map_err(|error| {
                    format!("RUNTIME_BINDING_REJECTED: read persisted identity: {error}")
                })?
                .ok_or_else(|| {
                    "RUNTIME_BINDING_REJECTED: orphan recovery requires a persisted identity"
                        .to_owned()
                })?;
            let persisted_runtime = persisted.runtime.as_ref().ok_or_else(|| {
                "RUNTIME_BINDING_REJECTED: persisted identity has no registered runtime".to_owned()
            })?;
            let registered_runtime = crate::identity::RuntimeIdentity {
                agent_id: binding.agent_id.clone(),
                runtime_id: binding.runtime_id.clone(),
                appserver_id: binding.app_scope_id.clone(),
                endpoint_generation: binding.endpoint_generation,
                binding_id: binding.binding_id.clone(),
                session_id: binding.session_id.clone(),
                native_thread_id: binding.native_thread_id.clone(),
            };
            if persisted.worker_id != worker_id
                || persisted.token != token
                || persisted.project_scope.as_ref() != Some(&route_scope.project_scope_id)
            {
                return Err(
                    "RUNTIME_BINDING_REJECTED: persisted identity does not own the registered worker"
                        .to_owned(),
                );
            }
            if !retired_tombstone {
                crate::identity::validate_binding(&registered_runtime, persisted_runtime).map_err(
                    |error| {
                        format!(
                            "SESSION_THREAD_BINDING_MISMATCH: persisted identity runtime does not match the registered binding: {error}; preserve the registered binding, obtain the verified host session/thread pair, and explicitly rebind the same identity and runtime; do not edit or infer the binding"
                        )
                    },
                )?;
            }
        }
    }

    let state = server.state.lock().unwrap();
    let candidates = candidates.as_ref().ok_or_else(|| {
        "RUNTIME_BINDING_REJECTED: CLI rebind requires the peer's current transport candidate"
            .to_owned()
    })?;
    let Some(candidate) = candidates.tmux.as_ref() else {
        // A dsh peer arrives here with a dsh candidate and no pane. Its
        // lifecycle belongs to the gateway, so the CLI cannot rebind it; say
        // that instead of sending the operator after a tmux pane that does not
        // exist for this transport.
        return Err(if candidates.dsh.is_some() {
            "RUNTIME_BINDING_REJECTED: a dsh agent's lifecycle is owned by the gateway; collab cannot rebind it from the CLI"
                .to_owned()
        } else {
            "RUNTIME_BINDING_REJECTED: CLI rebind requires the current tmux pane".to_owned()
        });
    };
    let candidate_binding = state.global.lookup_tmux_route(&candidate.endpoint).or_else(|| {
        state
            .global
            .lookup_unique_tmux_pane_route(&candidate.endpoint)
            .filter(|binding| {
                binding.agent_id.as_str() == worker_id
                    && state.workers.get(worker_id).is_some_and(|worker| {
                        worker.token == token
                            && selected_transport_for_worker(worker)
                                .is_some_and(|transport| transport.kind == TransportKind::Tmux)
                    })
            })
    });
    let candidate_thread = candidate
        .endpoint
        .codex_thread_id
        .as_deref()
        .unwrap_or(&candidate.endpoint.pane_id);
    if orphan_recovery && !retired_tombstone {
        let Some(candidate_binding) = candidate_binding else {
            return Err(
                "RUNTIME_BINDING_REJECTED: orphan recovery requires the persisted tmux pane"
                    .to_owned(),
            );
        };
        if candidate_binding.agent_id.as_str() != worker_id
            || candidate_binding.binding_id != expected_binding_id
            || candidate_binding.app_scope_id != route_scope.app_scope_id
            || candidate_binding.project_scope != route_scope.project_scope_id
            || persisted_thread_id.as_deref() != Some(candidate_thread)
        {
            return Err(
                "RUNTIME_BINDING_REJECTED: orphan recovery requires the persisted tmux pane"
                    .to_owned(),
            );
        }
    }
    // A pane has one owner and the later registrant wins it. The typed
    // registration path retires the previous claimant in the same transaction,
    // so this validator must not fence the pane a second time.
    Ok(())
}

fn project_route_actor(
    state: &State,
    context: &ProjectContext,
    worker_id: &str,
    token: &str,
) -> Result<WorkerRec, Resp> {
    context
        .validate()
        .map_err(|error| Resp::err(format!("PROJECT_CONTEXT_INVALID: {error}")))?;
    let route_scope = RouteScope {
        app_scope_id: context.app_scope_id.clone(),
        project_scope_id: context.project_scope.clone(),
    };
    let runtime = context.runtime_context.as_ref().ok_or_else(|| {
        Resp::err(
            "PROJECT_CONTEXT_REQUIRED: project-scoped mutation requires typed runtime context",
        )
    })?;
    let worker = verify(state, worker_id, token)?;
    let worker_scope =
        GlobalState::canonical_project_scope(Path::new(&worker.cwd)).map_err(|error| {
            Resp::err(format!(
                "RUNTIME_BINDING_REJECTED: worker cwd is not a project route: {error}"
            ))
        })?;
    if worker_scope != route_scope.project_scope_id {
        return Err(Resp::err(
            "RUNTIME_BINDING_REJECTED: worker cwd does not match request project route",
        ));
    }
    let Some(project) = state.global.lookup_project_for_route(&route_scope) else {
        return Err(Resp::err(
            "RUNTIME_BINDING_REJECTED: authoritative runtime binding is missing",
        ));
    };
    let mut bindings = project.runtime_bindings.values().filter(|binding| {
        binding.project_scope == route_scope.project_scope_id
            && binding.app_scope_id == route_scope.app_scope_id
            && binding.agent_id.as_str() == worker_id
    });
    let Some(binding) = bindings.next() else {
        return Err(Resp::err(
            "RUNTIME_BINDING_REJECTED: authoritative runtime binding is missing",
        ));
    };
    if bindings.next().is_some() {
        return Err(Resp::err(
            "RUNTIME_BINDING_REJECTED: authoritative runtime binding is ambiguous",
        ));
    }
    if binding.endpoint_generation == 0 {
        return Err(Resp::err(
            "RUNTIME_BINDING_REJECTED: authoritative runtime binding has no live endpoint generation",
        ));
    }
    let registered = crate::identity::RuntimeIdentity {
        agent_id: binding.agent_id.clone(),
        runtime_id: binding.runtime_id.clone(),
        appserver_id: binding.app_scope_id.clone(),
        endpoint_generation: binding.endpoint_generation,
        binding_id: binding.binding_id.clone(),
        session_id: binding.session_id.clone(),
        native_thread_id: binding.native_thread_id.clone(),
    };
    registered
        .validate()
        .map_err(|error| Resp::err(format!("RUNTIME_BINDING_REJECTED: {error}")))?;
    crate::identity::validate_binding(&registered, runtime).map_err(|error| {
        Resp::err(format!(
            "SESSION_THREAD_BINDING_MISMATCH: {error}; preserve the registered binding, obtain the verified host session/thread pair, and explicitly rebind the same identity and runtime; do not edit or infer the binding"
        ))
    })?;
    Ok(worker)
}
