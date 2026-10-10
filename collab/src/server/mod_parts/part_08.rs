/// The single daemon-owned scope projection shared by `status`, `context`, the
/// board, the panel and the authority-change receipts. It reports the exact
/// route scope as `{ "project_scope", "app_scope_id" }`, or `null` when no
/// route resolves. It never guesses a default app scope, so an unregistered
/// route stays explicit instead of looking like a whole empty project.
pub(crate) fn scope_view(route_scope: Option<&RouteScope>) -> serde_json::Value {
    match route_scope {
        Some(route_scope) => json!({
            "project_scope": route_scope.project_scope_id.as_str(),
            "app_scope_id": route_scope.app_scope_id,
        }),
        None => serde_json::Value::Null,
    }
}

/// The single daemon-owned authority projection shared by `status`, `context`
/// and the board. It reads only the current typed grant for the exact route
/// scope and reports the holder, that scope, and the grant metadata. The
/// transport observation is supplied by the caller and can never change the
/// authority status: an unknown or unreachable holder stays assigned.
pub(crate) fn master_authority_view(
    state: &State,
    route_scope: Option<&RouteScope>,
    transport_live: bool,
) -> serde_json::Value {
    let Some(grant) = current_master_grant(state, route_scope) else {
        return serde_json::Value::Null;
    };
    json!({
        "worker_id": grant.agent_id.as_str(),
        "scope": scope_view(route_scope),
        "endpoint_live": transport_live,
        "grant_id": grant.resource_id(),
        "grant_generation": grant.resource_generation(),
        "assigned_by": grant.granted_by,
        "approval": grant.approval,
        "assigned_ms": grant.granted_at_ms,
        "binding_id": grant.binding_id,
        "endpoint_generation": grant.endpoint_generation,
        "boundary": grant.boundary,
        "master_wake": state.master_wake,
    })
}

fn prune_master_wake_idle_capacity(server: &Server) {
    let (live_idle_workers, live_idle_subagents) = {
        let state = server.state.lock().unwrap();
        let live_idle_subagents = state
            .subagents
            .values()
            .filter(|record| record.status == "idle")
            .filter(|record| {
                state.workers.get(&record.peer).is_some_and(|worker| {
                    worker_presence(server, worker) == IdentityPresence::Present
                })
            })
            .filter(|record| {
                !state
                    .tasks
                    .values()
                    .any(|task| task.owner == record.peer && keepalive::actionable(&task.status))
            })
            .map(|record| record.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let live_idle_workers = state
            .workers
            .values()
            .filter(|worker| worker_presence(server, worker) == IdentityPresence::Present)
            .filter(|worker| {
                state
                    .keepalives
                    .get(&worker.id)
                    .is_some_and(|record| record.observed == "idle")
            })
            .filter(|worker| {
                !state
                    .tasks
                    .values()
                    .any(|task| task.owner == worker.id && keepalive::actionable(&task.status))
            })
            .map(|worker| worker.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        (live_idle_workers, live_idle_subagents)
    };
    let mut state = server.state.lock().unwrap();
    let changed = notification_state::retain_live_idle_workers(
        &mut state.master_wake,
        &live_idle_workers,
        &live_idle_subagents,
    );
    if changed {
        let accumulator = state.master_wake.clone();
        server.commit_locked_reporting(&mut state, &[Event::MasterWakeUpdated { accumulator }]);
    }
}

fn handle_master_status(server: &Server) -> Resp {
    prune_master_wake_idle_capacity(server);
    let state = server.state.lock().unwrap();
    // The typed grant is the only authority source. A route, reducer or scope
    // failure is an explicit error; a transport observation is reported
    // separately and never removes the holder.
    let route_scope = match server_route_scope(server, &state) {
        Ok(route_scope) => route_scope,
        Err(error) => return Resp::err(error),
    };
    let transport_live = current_master_grant(&state, route_scope.as_ref())
        .and_then(|grant| state.workers.get(grant.agent_id.as_str()).cloned())
        .is_some_and(|worker| worker_presence(server, &worker) == IdentityPresence::Present);
    let master = master_authority_view(&state, route_scope.as_ref(), transport_live);
    Resp::data(json!({
        "master": master,
        "scope": scope_view(route_scope.as_ref()),
        "recorded_unusable": serde_json::Value::Null,
    }))
}

pub(crate) fn handle_send(
    server: &Server,
    from: String,
    to: String,
    mtype: String,
    subject: Option<String>,
    body: String,
    in_reply_to: Option<String>,
    delivery_mode: String,
) -> Resp {
    if mtype != "notify" {
        return Resp::err("peer messaging requires type notify");
    }
    handle_send_with_task(
        server,
        from,
        to,
        mtype,
        subject,
        body,
        in_reply_to,
        delivery_mode,
        false,
        None,
    )
}

fn authoritative_send_binding<'a>(
    state: &'a State,
    route_scope: &RouteScope,
    worker_id: &str,
) -> Result<&'a RuntimeBinding, &'static str> {
    let Some(project) = state.global.lookup_project(&route_scope.project_scope_id) else {
        return Err("SEND_BINDING_REJECTED: authoritative runtime binding is missing");
    };
    let mut bindings = project.runtime_bindings.values().filter(|binding| {
        binding.app_scope_id == route_scope.app_scope_id && binding.agent_id.as_str() == worker_id
    });
    let Some(binding) = bindings.next() else {
        return Err("SEND_BINDING_REJECTED: authoritative runtime binding is missing");
    };
    if bindings.next().is_some() {
        return Err("SEND_BINDING_REJECTED: authoritative runtime binding is ambiguous");
    }
    Ok(binding)
}

fn handle_authenticated_send_with_app_scope(
    server: &Server,
    raw_from: String,
    worker_id: String,
    token: String,
    command: Option<crate::proto::CommandEnvelope>,
    to: String,
    mtype: String,
    subject: Option<String>,
    body: String,
    in_reply_to: Option<String>,
    delivery_mode: String,
    app_scope: Option<AppServerId>,
) -> Resp {
    let st = server.state.lock().unwrap();
    let Some(worker) = st.workers.get(&worker_id).cloned() else {
        return Resp::err(format!("worker {} not registered", worker_id));
    };
    if worker.token != token {
        return Resp::err("token mismatch: identity does not own this worker_id");
    }
    let Some(command) = command else {
        return Resp::err(
            "LEGACY_SEND_REJECTED: authenticated sender binding and route scope are required",
        );
    };
    let (runtime, registered_scope) = match app_scope {
        Some(context_app_scope) => {
            if command.scope.app_scope_id != context_app_scope {
                return Resp::err(
                    "SEND_BINDING_REJECTED: command app scope does not match request context",
                );
            }
            let worker_project_scope =
                match GlobalState::canonical_project_scope(Path::new(&worker.cwd)) {
                    Ok(scope) => scope,
                    Err(error) => return Resp::err(format!("SEND_BINDING_REJECTED: {error}")),
                };
            if command.scope.project_scope_id != worker_project_scope {
                return Resp::err(
                    "SEND_BINDING_REJECTED: command project scope does not match registered worker cwd",
                );
            }
            let Some(binding) = st
                .global
                .lookup_binding_for(&command.scope, &command.actor_binding_id)
                .cloned()
            else {
                return Resp::err(
                    "SEND_BINDING_REJECTED: authenticated sender binding is not registered for the request route",
                );
            };
            if binding.agent_id.as_str() != worker.id {
                return Resp::err(
                    "SEND_BINDING_REJECTED: authenticated sender binding belongs to another worker",
                );
            }
            let runtime = crate::identity::RuntimeIdentity {
                agent_id: binding.agent_id.clone(),
                runtime_id: binding.runtime_id.clone(),
                appserver_id: binding.app_scope_id.clone(),
                endpoint_generation: binding.endpoint_generation,
                binding_id: binding.binding_id.clone(),
                session_id: binding.session_id.clone(),
                native_thread_id: binding.native_thread_id.clone(),
            };
            (runtime, binding.route_scope())
        }
        None => {
            // Direct in-process callers predate the wire ProjectContext and
            // are bound to the compatibility tui-default route established by
            // handle_register. Wire requests never use this branch: they are
            // admitted with an explicit app scope above.
            let appserver_id = match crate::identity::AppServerId::new("tui-default") {
                Ok(id) => id,
                Err(error) => return Resp::err(error.to_string()),
            };
            let registered_scope =
                match RouteScope::for_registered_project(appserver_id, Path::new(&worker.cwd)) {
                    Ok(scope) => scope,
                    Err(error) => return Resp::err(error.to_string()),
                };
            let binding = match authoritative_send_binding(&st, &registered_scope, &worker_id) {
                Ok(binding) => binding.clone(),
                Err(error) => return Resp::err(error),
            };
            if command.actor_binding_id != binding.binding_id {
                return Resp::err(format!(
                    "SEND_BINDING_REJECTED: actor binding mismatch: expected {}, observed {}",
                    binding.binding_id, command.actor_binding_id
                ));
            }
            if command.endpoint_generation != binding.endpoint_generation {
                return Resp::err(format!(
                    "SEND_BINDING_REJECTED: stale endpoint generation: expected {}, observed {}",
                    binding.endpoint_generation, command.endpoint_generation
                ));
            }
            if command.scope != registered_scope {
                return Resp::err(
                    "SEND_BINDING_REJECTED: envelope scope does not match authoritative route scope",
                );
            }
            let runtime = crate::identity::RuntimeIdentity {
                agent_id: binding.agent_id.clone(),
                runtime_id: binding.runtime_id.clone(),
                appserver_id: binding.app_scope_id.clone(),
                endpoint_generation: binding.endpoint_generation,
                binding_id: binding.binding_id.clone(),
                session_id: binding.session_id.clone(),
                native_thread_id: binding.native_thread_id.clone(),
            };
            (runtime, registered_scope)
        }
    };
    if let Err(error) = command.validate_for(&runtime, &registered_scope) {
        return Resp::err(format!("SEND_BINDING_REJECTED: {error}"));
    }
    if raw_from != worker_id {
        return Resp::err("sender identity is derived from the authenticated binding");
    }
    if mtype == "notification" {
        return Resp::err("peer messaging requires type notify");
    }
    drop(st);
    handle_send_with_task(
        server,
        worker_id,
        to,
        mtype,
        subject,
        body,
        in_reply_to,
        delivery_mode,
        false,
        None,
    )
}

pub(crate) fn handle_send_with_task(
    server: &Server,
    from: String,
    to: String,
    mtype: String,
    subject: Option<String>,
    body: String,
    in_reply_to: Option<String>,
    delivery_mode: String,
    assign_task: bool,
    managed_subagent_id: Option<&str>,
) -> Resp {
    if !matches!(delivery_mode.as_str(), "immediate" | "queued") {
        return Resp::err("delivery mode must be immediate or queued");
    }
    let Some(subject) = subject.filter(|subject| !subject.trim().is_empty()) else {
        return Resp::err("MESSAGE_SUBJECT_REQUIRED: sendmessage requires --subject");
    };
    if !MSG_TYPES.contains(&mtype.as_str()) {
        return Resp::err(format!(
            "invalid type {}; must be one of {:?}",
            mtype, MSG_TYPES
        ));
    }
    let mut st = server.state.lock().unwrap();
    let managed_child = if assign_task {
        let Some(id) = managed_subagent_id else {
            return Resp::err("managed task requires an explicit subagent binding");
        };
        let Some(child) = st.subagents.get(id).cloned() else {
            return Resp::err(format!("unknown managed subagent {}", id));
        };
        if child.parent != from || child.peer != to {
            return Resp::err("managed subagent owner mismatch");
        }
        if child.status != "idle" {
            return Resp::err("subagent is not idle; query status instead of resending");
        }
        if st
            .tasks
            .values()
            .any(|task| task.owner == child.peer && task_resource_active(&task.status))
        {
            return Resp::err("managed subagent already has an active task");
        }
        Some(child)
    } else {
        if managed_subagent_id.is_some() {
            return Resp::err("unassigned peer message cannot bind a managed subagent");
        }
        None
    };
    if from.trim().is_empty() {
        return Resp::err("sender cannot be empty");
    }
    // A recipient is a handoff target, so reporting it as unresolved must
    // not depend on message dedup state.  Path references are still probed
    // later, after dedup, so a previously recorded handoff is never blocked
    // by a worktree that disappeared after delivery.
    if !st.workers.contains_key(&to) {
        let (error, data) = handoff_unresolved_recipient(&to);
        return Resp::err_data(error, data);
    }
    let Some(recipient) = st.workers.get(&to).cloned() else {
        return Resp::err(format!("recipient {} not registered", to));
    };
    // Presence probes open an AppServer RPC connection, so run them outside
    // the global state lock to avoid stalling unrelated daemon commands.
    drop(st);
    if worker_presence(server, &recipient) == IdentityPresence::Missing {
        // Missing is a transport failure, not a durable mailbox failure. The
        // message is committed below and the attempted wake carries the exact
        // missing endpoint error, so recovery can preserve and later receive it.
    }
    let mut st = server.state.lock().unwrap();
    // The presence probe runs outside the lock, so a concurrent
    // WorkerClosed/rebind can remove or replace the recipient while it is in
    // flight.  Commit only against the exact recipient that was probed.
    match st.workers.get(&to) {
        Some(current) if current == &recipient => {}
        Some(_) => {
            return Resp::err(format!(
                "recipient {} transport changed while verifying liveness; retry the send",
                to
            ));
        }
        None => {
            let (error, data) = handoff_unresolved_recipient(&to);
            return Resp::err_data(error, data);
        }
    }
    if let Some(ref rid) = in_reply_to {
        if !st.msgs.contains_key(rid) {
            return Resp::err(format!("in_reply_to message {} not found", rid));
        }
    }
    if mtype == "request" {
        if let Some((existing_id, existing)) = st.recent_live_request(&from, &to, now_ms()) {
            let retry_at = iso(existing.created_ms + state::REQUEST_COOLDOWN_MS);
            return Resp::err(format!(
                "request cooldown active: existing_request_id={}, retry_at={}",
                existing_id, retry_at
            ));
        }
    }
    let superseded_ids = match (mtype.as_str(), in_reply_to.as_deref()) {
        ("reply", Some(request_id)) => st.superseded_replies(request_id),
        _ => Vec::new(),
    };
    let msg = Message {
        id: gen_msg_id(),
        from: from.clone(),
        to: to.clone(),
        mtype: mtype.clone(),
        subject: Some(subject),
        body,
        in_reply_to,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    if let Some(existing) = st.msgs.values().find(|m| {
        m.from == from
            && m.to == to
            && m.mtype == mtype
            && m.subject == msg.subject
            && m.body == msg.body
            && m.state == "pending"
    }) {
        let managed_duplicate = assign_task
            && managed_child.as_ref().is_some_and(|child| {
                child.last_message.as_deref() == Some(existing.id.as_str())
                    && st
                        .tasks
                        .get(&format!("task-{}", existing.id))
                        .is_some_and(|task| task.owner == child.peer && task.created_by == from)
            });
        if !assign_task || managed_duplicate {
            let existing_id = existing.id.clone();
            let subscription = st
                .matching_subscription(&to, "direct-message", None, now_ms())
                .cloned();
            let explicit_retry = explicit_retry_refusal(&st, &existing_id).is_none();
            drop(st);
            let notification = subscription
                .as_ref()
                .map(|subscription| {
                    attempt_notification_detailed_with_mode_at(
                        server,
                        &existing_id,
                        &subscription.id,
                        now_ms(),
                        explicit_retry,
                    )
                })
                .unwrap_or_else(|| {
                    NotificationAttempt::NotAttempted("no notification subscription".into())
                });
            return notification_send_response(
                json!({
                    "msg_id": existing_id,
                    "deduplicated": true,
                }),
                subscription.is_some(),
                {
                    let state = server.state.lock().unwrap();
                    notification_method_for_worker(&state, &to)
                },
                &notification,
            );
        }
    }
    if let Err((error, data)) = resolve_handoff_target(server, &st, &to, &msg.body) {
        return Resp::err_data(error, data);
    }
    let mid = msg.id.clone();
    let task_id = assign_task.then(|| format!("task-{mid}"));
    let subscription = st
        .matching_subscription(&to, "direct-message", None, now_ms())
        .cloned();
    let mut events = if let Some(task_id) = &task_id {
        let task = TaskRec {
            id: task_id.clone(),
            owner: to.clone(),
            created_by: from.clone(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            status: "assigned".into(),
            next_step: Some(format!(
                "Read collab msg {mid}; accept via subagent working; bind a worktree with task relocate before code edits."
            )),
            wait: None,
            created_ms: now_ms(),
            updated_ms: now_ms(),
        };
        scheduler_assignment_events(
            msg,
            task,
            Some(managed_child.expect("managed child validated above")),
        )
    } else {
        vec![Event::Sent { msg }]
    };
    events.push(Event::DeliveryMode {
        msg_id: mid.clone(),
        mode: delivery_mode,
        source_thread_id: None,
    });
    if let Some(subscription) = &subscription {
        events.push(Event::WakeBound {
            message_id: mid.clone(),
            subscription_id: subscription.id.clone(),
        });
    }
    if !superseded_ids.is_empty() {
        events.push(Event::Superseded {
            ids: superseded_ids,
        });
    }
    if let Err(error) = server.commit_locked_checked(&mut st, &events) {
        return Resp::err(format!("SEND_DURABILITY_FAILED: {error}"));
    }
    drop(st);
    let notification = subscription
        .as_ref()
        .map(|subscription| {
            attempt_notification_detailed_with_at(server, &mid, &subscription.id, now_ms())
        })
        .unwrap_or_else(|| {
            NotificationAttempt::NotAttempted("no notification subscription".into())
        });
    notification_send_response(
        json!({
            "msg_id": mid,
            "task_id": task_id,
        }),
        subscription.is_some(),
        {
            let state = server.state.lock().unwrap();
            notification_method_for_worker(&state, &to)
        },
        &notification,
    )
}

fn handle_live_closure_daemon_send(
    server: &Server,
    worker_id: String,
    token: String,
    to: String,
    path: String,
    subject: String,
    body: String,
    restart_replay_pending: bool,
) -> Resp {
    if !matches!(
        path.as_str(),
        "daemon_to_peer" | "daemon_to_master" | "restart_replay"
    ) {
        return Resp::err(format!("COLLAB_LIVE_CLOSURE_PROBE_INVALID_PATH:{path}"));
    }
    if restart_replay_pending && path != "restart_replay" {
        return Resp::err("COLLAB_LIVE_CLOSURE_RESTART_REPLAY_PENDING_REQUIRES_RESTART_PATH");
    }
    if subject.trim().is_empty() || body.trim().is_empty() || subject != body {
        return Resp::err("COLLAB_LIVE_CLOSURE_CHALLENGE_MISMATCH");
    }

    let restart_replay_pending = path == "restart_replay";
    let mut st = server.state.lock().unwrap();
    if let Err(error) = verify(&st, &worker_id, &token) {
        return error;
    }
    // Route role is the current grant holder. Whether the recipient address is
    // actually reachable is checked separately below and stays a send gate.
    let master_holder = match current_master_holder(server, &st) {
        Ok(master) => master,
        Err(error) => return Resp::err(error),
    };
    if path == "daemon_to_master" && master_holder.as_deref() != Some(to.as_str()) {
        return Resp::err("COLLAB_LIVE_CLOSURE_MASTER_ROUTE_MISMATCH");
    }
    if matches!(path.as_str(), "daemon_to_peer" | "restart_replay")
        && master_holder.as_deref() == Some(to.as_str())
    {
        return Resp::err("COLLAB_LIVE_CLOSURE_PEER_ROUTE_MISMATCH");
    }
    let Some(recipient) = st.workers.get(&to).cloned() else {
        return Resp::err(format!("recipient {} not registered", to));
    };
    drop(st);
    if worker_presence(server, &recipient) != IdentityPresence::Present {
        return Resp::err("COLLAB_LIVE_CLOSURE_TARGET_ROUTE_UNAVAILABLE");
    }
    let mut st = server.state.lock().unwrap();
    let msg = Message {
        id: gen_msg_id(),
        from: "collab-server".into(),
        to: to.clone(),
        mtype: "notification".into(),
        subject: Some(subject),
        body,
        in_reply_to: None,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    let mid = msg.id.clone();
    let subscription = st
        .matching_subscription(&to, "direct-message", None, now_ms())
        .cloned();
    let delivery_mode = if restart_replay_pending {
        RESTART_REPLAY_PENDING_MODE
    } else {
        DAEMON_LIVE_CLOSURE_MODE
    };
    let mut events = vec![
        Event::Sent { msg },
        Event::DeliveryMode {
            msg_id: mid.clone(),
            mode: delivery_mode.into(),
            source_thread_id: None,
        },
    ];
    if let Some(subscription) = &subscription {
        events.push(Event::WakeBound {
            message_id: mid.clone(),
            subscription_id: subscription.id.clone(),
        });
    }
    if let Err(error) = server.commit_locked_checked(&mut st, &events) {
        return Resp::err(format!("SEND_DURABILITY_FAILED: {error}"));
    }
    drop(st);

    let notification = subscription
        .as_ref()
        .map(|subscription| {
            attempt_notification_detailed_with_at(server, &mid, &subscription.id, now_ms())
        })
        .unwrap_or_else(|| {
            NotificationAttempt::NotAttempted("no notification subscription".into())
        });
    notification_send_response(
        json!({
            "msg_id": mid,
            "daemon_sender": true,
            "from": "collab-server",
            "path": path,
            "delivery_mode": delivery_mode,
            "restart_replay_pending": restart_replay_pending,
        }),
        subscription.is_some(),
        {
            let state = server.state.lock().unwrap();
            notification_method_for_worker(&state, &to)
        },
        &notification,
    )
}

fn handle_cross_project_send(
    server: &Server,
    from: String,
    from_project: String,
    source_thread_id: String,
    source_master_assigned_by: String,
    source_master_approval: Option<String>,
    source_master_assigned_ms: i64,
    to: String,
    subject: String,
    body: String,
    in_reply_to: Option<String>,
) -> Resp {
    if from_project.trim().is_empty() || source_master_assigned_by.trim().is_empty() {
        return Resp::err(
            "cross-project send requires source project and master assignment evidence",
        );
    }
    if source_master_assigned_ms <= 0 {
        return Resp::err("cross-project send requires source master assignment timestamp");
    }
    if source_master_approval
        .as_deref()
        .is_none_or(|v| v.trim().is_empty())
        && source_master_assigned_by == from
    {
        return Resp::err(
            "cross-project send requires user approval evidence for self-promoted source master",
        );
    }
    let mut st = server.state.lock().unwrap();
    let target_master = match current_master_holder(server, &st) {
        Ok(master) => master,
        Err(error) => return Resp::err(error),
    };
    if target_master.as_deref() != Some(to.as_str()) {
        return Resp::err("cross-project communication requires the target to be the project master");
    }
    let Some(recipient) = st.workers.get(&to).cloned() else {
        return Resp::err(format!("recipient {} not registered", to));
    };
    drop(st);
    if worker_presence(server, &recipient) != IdentityPresence::Present {
        return Resp::err(
            "cross-project communication requires a live target identity on its server-selected transport",
        );
    }
    let mut st = server.state.lock().unwrap();
    if subject.trim().is_empty() {
        return Resp::err("MESSAGE_SUBJECT_REQUIRED: cross-project send requires --subject");
    }
    let msg = Message {
        id: gen_msg_id(),
        from: format!("{}@{}", from, from_project),
        to: to.clone(),
        mtype: "notify".into(),
        subject: Some(subject),
        body,
        in_reply_to,
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    let mid = msg.id.clone();
    let subscription = st
        .matching_subscription(&to, "direct-message", None, now_ms())
        .cloned();
    let mut events = vec![
        Event::Sent { msg },
        Event::DeliveryMode {
            msg_id: mid.clone(),
            mode: "explicit-notification".into(),
            source_thread_id: Some(source_thread_id),
        },
    ];
    if let Some(subscription) = &subscription {
        events.push(Event::WakeBound {
            message_id: mid.clone(),
            subscription_id: subscription.id.clone(),
        });
    }
    if let Err(error) = server.commit_locked(&mut st, &events) {
        drop(st);
        return Resp::err(format!("CROSS_PROJECT_SEND_DURABILITY_FAILED: {error}"));
    }
    drop(st);
    let notification = subscription
        .as_ref()
        .map(|subscription| {
            attempt_notification_detailed_with_at(server, &mid, &subscription.id, now_ms())
        })
        .unwrap_or_else(|| {
            NotificationAttempt::NotAttempted("no notification subscription".into())
        });
    notification_send_response(
        json!({
            "msg_id": mid,
            "cross_project": true,
            "source_master": from,
            "target_master": to,
        }),
        subscription.is_some(),
        {
            let state = server.state.lock().unwrap();
            notification_method_for_worker(&state, &to)
        },
        &notification,
    )
}

#[cfg(test)]
fn handle_task_register(
    server: &Server,
    worker_id: String,
    token: String,
    task_id: String,
    owner: Option<String>,
    feature_id: Option<String>,
    worktree_path: Option<String>,
    branch: Option<String>,
    base_commit: Option<String>,
    priority: String,
) -> Resp {
    handle_task_register_with_next(
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
        None,
        None,
    )
}

fn handle_task_register_with_next(
    server: &Server,
    worker_id: String,
    token: String,
    task_id: String,
    owner: Option<String>,
    feature_id: Option<String>,
    mut worktree_path: Option<String>,
    branch: Option<String>,
    base_commit: Option<String>,
    priority: String,
    next_step: Option<String>,
    goal_prompt: Option<String>,
) -> Resp {
    let mut st = server.state.lock().unwrap();
    let Some(worker) = st.workers.get(&worker_id).cloned() else {
        return Resp::err(format!("worker {} not registered", worker_id));
    };
    if worker.token != token {
        return Resp::err("token mismatch: identity does not own this worker_id");
    }
    if st.tasks.contains_key(&task_id) {
        return Resp::err(format!("task {} already registered", task_id));
    }
    if goal_prompt.is_some() {
        return Resp::err(
            "/goal registration is deferred; register the peer-owned task without --goal-prompt",
        );
    }
    if owner.as_deref().is_some_and(|owner| owner != worker_id) {
        return Resp::err("peer may register only its own task; omit --owner or use its worker_id");
    }
    if !matches!(priority.as_str(), "p0" | "p1" | "p2" | "p3" | "p4") {
        return Resp::err(format!(
            "invalid priority {}; must be p0, p1, p2, p3, or p4",
            priority
        ));
    }
    let task_owner = worker_id.clone();
    if let Some(path) = &worktree_path {
        let canonical = match validate_worktree_path(&server.root, &server.config, path) {
            Ok(path) => path,
            Err(error) => return Resp::err(error),
        };
        worktree_path = Some(canonical.display().to_string());
    }
    if let Some(existing) = st
        .tasks
        .values()
        .find(|task| {
            task.owner == task_owner
                && task_resource_active(&task.status)
                && (feature_id.is_some() && task.feature_id == feature_id
                    || worktree_path.is_some() && task.worktree_path == worktree_path)
        })
        .cloned()
    {
        return Resp::err(format!(
            "TASK_OWNER_ALREADY_HOLDS_RESOURCE: task {} already owns this feature/worktree for owner {}; continue {} instead of registering a parallel task, or use `collab task relocate {}` to change its worktree",
            existing.id, task_owner, existing.id, existing.id
        ));
    }
    if let Some(existing) = st
        .tasks
        .values()
        .find(|task| {
            task_resource_active(&task.status)
                && (feature_id.is_some() && task.feature_id == feature_id
                    || worktree_path.is_some() && task.worktree_path == worktree_path)
        })
        .cloned()
    {
        let now = now_ms();
        let blocked_task = TaskRec {
            id: task_id.clone(),
            owner: worker_id.clone(),
            created_by: worker_id.clone(),
            feature_id: feature_id.clone(),
            worktree_path: worktree_path.clone(),
            branch: branch.clone(),
            base_commit: base_commit.clone(),
            priority: priority.clone(),
            status: "blocked".into(),
            next_step: Some(format!("RESOURCE_CONFLICT={}", existing.id)),
            wait: None,
            created_ms: now,
            updated_ms: now,
        };
        if let Err(error) =
            server.commit_locked_checked(&mut st, &[Event::TaskCreated { task: blocked_task }])
        {
            return Resp::err(format!("TASK_DURABILITY_FAILED: {error}"));
        }
        return Resp::err_data(
            "TASK_RESOURCE_CONFLICT",
            json!({
                "requested_task": task_id,
                "blocking_task": existing.id,
                "responsible_actor": existing.owner,
                "status": "blocked",
                "notification": "none; use explicit sendmessage when coordination is needed",
            }),
        );
    }
    let now = now_ms();
    let task = TaskRec {
        id: task_id.clone(),
        owner: task_owner,
        created_by: worker_id,
        feature_id,
        worktree_path,
        branch,
        base_commit,
        priority,
        status: "working".to_string(),
        next_step,
        wait: None,
        created_ms: now,
        updated_ms: now,
    };
    let mut events = vec![Event::TaskCreated { task: task.clone() }];
    if let Some(binding) = worktree_binding_for_task(server, &task) {
        events.push(Event::WorktreeBound { binding });
    }
    if let Err(error) = server.commit_locked_checked(&mut st, &events) {
        return Resp::err(format!("TASK_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "task": task.id,
        "owner": task.owner,
        "status": task.status,
        "cleanup": if task.worktree_path.is_some() { "pending" } else { "not_required" },
    }))
}

fn handle_task_relocate(
    server: &Server,
    worker_id: String,
    token: String,
    task_id: String,
    worktree_path: String,
    branch: Option<String>,
    base_commit: Option<String>,
) -> Resp {
    let mut st = server.state.lock().unwrap();
    let Some(worker) = st.workers.get(&worker_id).cloned() else {
        return Resp::err(format!("worker {} not registered", worker_id));
    };
    if worker.token != token {
        return Resp::err("token mismatch: identity does not own this worker_id");
    }
    let canonical_worktree = match validate_worktree_path(&server.root, &server.config, &worktree_path) {
        Ok(path) => path,
        Err(error) => return Resp::err(error),
    };
    let worktree_path = canonical_worktree.display().to_string();
    let Some(mut task) = st.tasks.get(&task_id).cloned() else {
        return Resp::err(format!("task {} not found", task_id));
    };
    if task.owner != worker_id {
        return Resp::err("only the task owner may relocate its worktree");
    }
    if matches!(task.status.as_str(), "closed" | "cancelled") {
        return Resp::err("terminal tasks cannot be relocated");
    }
    if let Some(existing) = st.tasks.values().find(|other| {
        other.id != task_id
            && task_resource_active(&other.status)
            && other.worktree_path.as_deref() == Some(worktree_path.as_str())
    }) {
        return Resp::err(format!(
            "worktree is already declared by task {}",
            existing.id
        ));
    }
    let old_worktree = task.worktree_path.clone();
    task.worktree_path = Some(worktree_path.clone());
    if branch.is_some() {
        task.branch = branch;
    }
    if base_commit.is_some() {
        task.base_commit = base_commit;
    }
    task.updated_ms = now_ms();
    let mut events = vec![Event::TaskUpdated { task: task.clone() }];
    if let Some(binding) = worktree_binding_for_task(server, &task) {
        events.push(Event::WorktreeBound { binding });
    }
    if let Err(error) = server.commit_locked(&mut st, &events) {
        drop(st);
        return Resp::err(format!("TASK_RELOCATE_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "task": task.id,
        "relocated": true,
        "old_worktree": old_worktree,
        "worktree": task.worktree_path,
        "branch": task.branch,
        "base_commit": task.base_commit,
        "status": task.status,
        "next": "verify git worktree list and continue the existing claim; evidence remains attached"
    }))
}

fn handle_task_dispatch(server: &Server, worker_id: String, token: String) -> Resp {
    let st = server.state.lock().unwrap();
    if let Err(error) = verify(&st, &worker_id, &token) {
        return error;
    }
    Resp::err("central task dispatch is deprecated; each peer registers and owns its task")
}

fn handle_task_claim(server: &Server, worker_id: String, token: String, task_id: String) -> Resp {
    let st = server.state.lock().unwrap();
    if let Err(error) = verify(&st, &worker_id, &token) {
        return error;
    }
    Resp::err(format!(
        "task claim is deprecated; peer must self-register task {}",
        task_id
    ))
}

fn handle_task_accept(server: &Server, worker_id: String, token: String, task_id: String) -> Resp {
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
    if task.status == "invited" {
        return Resp::err("BOARD_INVITATION_REVISION_REQUIRED: use collab task accept --expected-revision N or collab board respond --accept --expected-revision N");
    }
    if task.owner != worker_id {
        return Resp::err("only the task owner may accept its assignment");
    }
    let Some(admission) = st
        .scheduler_admissions
        .values()
        .find(|admission| admission.task_id == task.id && admission.status == "succeeded")
    else {
        return Resp::err(
            "task assignment provenance is missing; accept only scheduler assignments",
        );
    };
    if admission.managed_subagent_id.is_some() {
        return Resp::err("managed assignment must be accepted with collab subagent working");
    }
    if task.status == "working" {
        return Resp::data(json!({
            "task": task.id,
            "status": task.status,
            "owner": task.owner,
            "accepted": true,
            "idempotent": true,
            "notification": "none",
            "next_action": task.next_step,
        }));
    }
    if task.status != "assigned" {
        return Resp::err(format!(
            "task {} is not assigned; current status is {}",
            task.id, task.status
        ));
    }
    if let Err(error) = board_execution_gate(server, &st, &worker_id, &task_id) {
        return error;
    }
    task.status = "working".into();
    task.wait = None;
    task.updated_ms = now_ms();
    if let Err(error) = server.commit_locked_checked(&mut st, &[Event::TaskUpdated { task: task.clone() }]) {
        return Resp::err(format!("TASK_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "task": task.id,
        "status": task.status,
        "owner": task.owner,
        "accepted": true,
        "notification": "none",
        "next_action": task.next_step,
    }))
}

fn handle_task_update(
    server: &Server,
    worker_id: String,
    token: String,
    task_id: String,
    status: Option<String>,
    next_step: Option<String>,
) -> Resp {
    let mut st = server.state.lock().unwrap();
    task_update_locked(server, &mut st, worker_id, token, task_id, status, next_step)
}

fn task_update_locked(
    server: &Server,
    mut st: &mut State,
    worker_id: String,
    token: String,
    task_id: String,
    status: Option<String>,
    next_step: Option<String>,
) -> Resp {
    let Some(worker) = st.workers.get(&worker_id).cloned() else {
        return Resp::err(format!("worker {} not registered", worker_id));
    };
    if worker.token != token {
        return Resp::err("token mismatch: identity does not own this worker_id");
    }
    let Some(mut task) = st.tasks.get(&task_id).cloned() else {
        return Resp::err(format!("task {} not found", task_id));
    };
    if task.owner != worker_id {
        return Resp::err("only the task owner may update its lifecycle");
    }
    if let Some(new_status) = status {
        if !TASK_STATUSES.contains(&new_status.as_str()) {
            return Resp::err(format!(
                "invalid status {}; must be one of {:?}",
                new_status, TASK_STATUSES
            ));
        }
        if new_status == "closed" {
            return Resp::err("use collab task close after owner merge and cleanup verification");
        }
        if new_status == "delivered" {
            return Resp::err(
                "use collab task deliver to complete a claim; direct status mutation is rejected",
            );
        }
        if new_status == "working" && task.status == "assigned" {
            return Resp::err("use collab task accept to accept an assigned task");
        }
        // Pre-review producers persisted accepted candidates without a lifecycle
        // record. Keep their owner-local merge transition replayable while new
        // review records continue through the evidence-bearing integrated path.
        let legacy_accepted_merge = new_status == "merged"
            && task.status == "accepted"
            && !st.task_lifecycle.contains_key(&task.id);
        if new_status == "accepted" || (new_status == "merged" && !legacy_accepted_merge) {
            return Resp::err(
                "use collab task review/integrated for integration-owned lifecycle transitions",
            );
        }
        if new_status == "waiting" {
            return Resp::err("use collab task wait so responsibility and deadline are durable");
        }
        if new_status == "cancelled" && task.worktree_path.is_some() {
            return Resp::err(
                "CLEANUP_REQUIRED_BEFORE_CANCEL: task owns a worktree; close only after merged cleanup",
            );
        }
        if !task_transition_allowed(&task.status, &new_status) {
            return Resp::err(format!(
                "invalid task transition {} -> {}",
                task.status, new_status
            ));
        }
        task.status = new_status.clone();
        if new_status != "waiting" {
            task.wait = None;
        }
    }
    if next_step.is_some() {
        task.next_step = next_step;
    }
    task.updated_ms = now_ms();
    let mut events = vec![Event::TaskUpdated { task: task.clone() }];
    // A registered merge obligation exists iff the task is still awaiting the
    // merge of an accepted candidate. Any other transition (rework, cancel,
    // legacy merge) ends that obligation in the same transaction.
    if task.status != "accepted" && st.pending_merges.contains_key(&task_id) {
        let stale_notices = pending_merge_notice_ids(&st, &task_id);
        events.push(Event::MergeResolved {
            task_id: task_id.clone(),
            resolved_by: worker_id.clone(),
            reason: Some(format!("task no longer awaits merge (status={})", task.status)),
            at_ms: task.updated_ms,
        });
        if !stale_notices.is_empty() {
            events.push(Event::Superseded { ids: stale_notices });
        }
    }
    if let Err(error) = server.commit_locked_checked(&mut st, &events) {
        return Resp::err(format!("TASK_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "task": task.id,
        "status": task.status,
        "owner": task.owner,
        "notification": "none",
        "next_action": task.next_step,
    }))
}

fn stale_worker_views(
    st: &State,
    presence: &dyn Fn(&WorkerRec) -> IdentityPresence,
) -> Vec<serde_json::Value> {
    st.workers
        .values()
        .filter(|worker| {
            presence(worker) == IdentityPresence::Missing
        })
        .map(|worker| {
            let active_tasks: Vec<String> = st
                .tasks
                .values()
                .filter(|task| task.owner == worker.id && task_resource_active(&task.status))
                .map(|task| task.id.clone())
                .collect();
            json!({
                "worker": worker.id,
                "transport_kind": worker.transport.as_ref().map(|transport| transport.kind.as_str()),
                "active_tasks": active_tasks,
                "action": "peer owns cleanup; daemon operator may inspect during migration"
            })
        })
        .collect()
}

fn close_task_resources(
    root: &Path,
    config: &crate::config::Config,
    worktree_path: Option<&str>,
    branch: Option<&str>,
) -> Result<(), String> {
    if let Some(branch) = branch {
        let branch_ref = format!("refs/heads/{branch}");
        let branch_exists = Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "--verify", &branch_ref])
            .output()
            .map_err(|e| format!("cannot verify branch {branch}: {e}"))?;
        if branch_exists.status.success() {
            let main_ref = "refs/heads/main";
            let merged = Command::new("git")
                .current_dir(root)
                .args(["merge-base", "--is-ancestor", &branch_ref, main_ref])
                .output()
                .map_err(|e| format!("cannot verify branch {branch}: {e}"))?;
            if !merged.status.success() {
                return Err(format!(
                    "branch {branch} is not merged into {main_ref}; refusing delete"
                ));
            }
        }
    }
    if let Some(relative) = worktree_path {
        let worktree = cleanup_worktree_path(root, config, relative)?;
        if worktree.exists() {
            let dirty = Command::new("git")
                .arg("-C")
                .arg(&worktree)
                .args(["status", "--porcelain"])
                .output()
                .map_err(|e| format!("cannot inspect worktree {}: {e}", relative))?;
            if !dirty.status.success() {
                return Err(format!(
                    "cannot verify clean worktree {}: {}",
                    relative,
                    String::from_utf8_lossy(&dirty.stderr).trim()
                ));
            }
            if !dirty.stdout.is_empty() {
                return Err(format!("worktree {} has uncommitted changes", relative));
            }

            let removed = Command::new("git")
                .current_dir(root)
                .args(["worktree", "remove", &worktree.display().to_string()])
                .output()
                .map_err(|e| format!("cannot remove worktree {}: {e}", relative))?;
            if !removed.status.success() {
                return Err(format!(
                    "worktree cleanup failed for {}: {}",
                    relative,
                    String::from_utf8_lossy(&removed.stderr).trim()
                ));
            }
        }
    }

    if let Some(branch) = branch {
        let branch_ref = format!("refs/heads/{branch}");
        let branch_exists = Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "--verify", &branch_ref])
            .output()
            .map_err(|e| format!("cannot verify branch {branch}: {e}"))?;
        if branch_exists.status.success() {
            let deleted = Command::new("git")
                .current_dir(root)
                .args(["branch", "-D", branch])
                .output()
                .map_err(|e| format!("cannot delete branch {branch}: {e}"))?;
            if !deleted.status.success() {
                return Err(format!(
                    "branch cleanup failed for {branch}: {}",
                    String::from_utf8_lossy(&deleted.stderr).trim()
                ));
            }
        }
    }
    Ok(())
}

fn cleanup_receipt_is_reusable(root: &Path, receipt: &CleanupReceipt) -> bool {
    if let Some(path) = receipt.worktree_path.as_deref() {
        let worktree = Path::new(path);
        let worktree = if worktree.is_absolute() {
            worktree.to_path_buf()
        } else {
            root.join(worktree.strip_prefix("./").unwrap_or(worktree))
        };
        if worktree.exists() {
            return false;
        }
    }
    if let Some(branch) = receipt.branch.as_deref() {
        let branch_exists = Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "--verify", branch])
            .output()
            .is_ok_and(|output| output.status.success());
        if branch_exists {
            return false;
        }
    }
    true
}

fn handle_task_deliver(
    server: &Server,
    worker_id: String,
    token: String,
    task_id: String,
    evidence: Option<String>,
    worktree: Option<String>,
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
    if task.owner != worker_id || !task_delivery_allowed(&task.status) {
        return Resp::err(format!(
            "task {} must be owned and working, verifying, reviewed, or rework before delivery (current: {})",
            task_id, task.status
        ));
    }

    let Some(evidence) = evidence.filter(|value| !value.trim().is_empty()) else {
        return Resp::err("task deliver requires non-empty --evidence");
    };
    let Some(worktree) = worktree.filter(|value| !value.trim().is_empty()) else {
        return Resp::err("task deliver requires non-empty --worktree");
    };
    if task
        .worktree_path
        .as_deref()
        .is_some_and(|registered| registered != worktree)
    {
        return Resp::err("task deliver --worktree must match the registered task worktree");
    }
    let now = now_ms();
    task.status = "delivered".to_string();
    task.wait = None;
    task.next_step = Some(
        "task owner or live master reviews delivery with collab task review --accept or --rework"
            .to_string(),
    );
    task.updated_ms = now;
    let mut lifecycle = st.task_lifecycle.get(&task.id).cloned().unwrap_or_default();
    lifecycle.delivery_evidence = Some(evidence.clone());
    lifecycle.delivered_ms = Some(now);
    // Capture the exact candidate commit so a later pending merge can prove
    // the delivered candidate itself reached main, not just some main ref.
    lifecycle.delivery_commit = resolve_candidate_commit(&server.root, &task, &worktree);
    if let Err(error) = server.commit_locked(
        &mut st,
        &[
            Event::TaskUpdated { task: task.clone() },
            Event::TaskLifecycleUpdated {
                task_id: task.id.clone(),
                record: lifecycle,
            },
        ],
    ) {
        drop(st);
        return Resp::err(format!("TASK_DELIVER_DURABILITY_FAILED: {error}"));
    }

    Resp::data(json!({
        "delivered": task.id,
        "status": task.status,
        "evidence": evidence,
        "worktree": worktree,
        "notification": "none",
        "next_action": task.next_step,
        "identity": {"worker_id": worker.id, "kind": "peer"},
    }))
}

fn resolve_candidate_commit(root: &Path, task: &TaskRec, worktree: &str) -> Option<String> {
    // The delivered worktree HEAD is the authoritative candidate. A branch
    // ref may point at a stale or unrelated commit; it is only admissible as
    // a consistency cross-check, never as the binding on its own.
    let worktree_dir = if Path::new(worktree).is_absolute() {
        PathBuf::from(worktree)
    } else {
        root.join(worktree)
    };
    let worktree_head = Command::new("git")
        .current_dir(worktree_dir)
        .args(["rev-parse", "--verify", "HEAD^{commit}"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (!head.is_empty()).then_some(head)
        })?;
    if let Some(branch) = task.branch.as_deref() {
        let branch_head = Command::new("git")
            .current_dir(root)
            .args(["rev-parse", "--verify", &format!("refs/heads/{branch}^{{commit}}")])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
                (!head.is_empty()).then_some(head)
            });
        // Fail closed on divergence: the delivered worktree HEAD must agree
        // with the registered branch or the candidate is not provably the
        // delivered commit.
        if branch_head.as_deref() != Some(worktree_head.as_str()) {
            return None;
        }
    }
    Some(worktree_head)
}

fn task_integration_authorized(
    server: &Server,
    state: &State,
    task: &TaskRec,
    worker_id: &str,
) -> bool {
    task.owner == worker_id
        || current_master_holder(server, state)
            .ok()
            .flatten()
            .as_deref()
            == Some(worker_id)
}
