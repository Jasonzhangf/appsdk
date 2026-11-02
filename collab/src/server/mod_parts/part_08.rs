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
/// endpoint observation is supplied by the caller and can never change the
/// authority status: an unknown or unreachable holder stays assigned, and the
/// endpoint field carries the independent reachability value (`true`, `false`
/// or `null` when the probe could not decide).
pub(crate) fn master_authority_view_with_endpoint(
    state: &State,
    route_scope: Option<&RouteScope>,
    endpoint_live: Option<bool>,
) -> serde_json::Value {
    let Some(grant) = current_master_grant(state, route_scope) else {
        return serde_json::Value::Null;
    };
    json!({
        "worker_id": grant.agent_id.as_str(),
        "display_name": worker_display_name(state, grant.agent_id.as_str(), true),
        "scope": scope_view(route_scope),
        "endpoint_live": endpoint_live,
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

/// The single read-only workspace-idle projection shared by `master status`
/// and `ceo progress`.
///
/// It reports the trusted continuous online-idle fact from the typed activity
/// owner. The canonical project root comes from the daemon's registered
/// project scope, never from the caller's cwd. When there is no master, no
/// current session match, or no typed activity, the fact is `unknown` with a
/// null idle start and a zero duration. The projection never mutates state, so
/// reading it cannot advance the CEO report watermark.
pub(crate) fn workspace_idle_view(
    server: &Server,
    state: &State,
    master_id: Option<&str>,
    now: i64,
) -> serde_json::Value {
    let project_root = GlobalState::canonical_project_scope(&server.root)
        .ok()
        .map(|scope| scope.as_str().to_owned());
    let Some(project_root) = project_root else {
        return unknown_workspace_view(None);
    };
    let Some(master_id) = master_id else {
        return unknown_workspace_view(Some(project_root));
    };
    let Some(worker) = state.workers.get(master_id) else {
        return unknown_workspace_view(Some(project_root));
    };
    let (presence, activity) = worker_agent_facts(server, state, worker);
    let presence = presence.map(|fact| fact.presence);
    let activity_kind = activity.map(|fact| fact.activity);
    let label = match (presence, activity_kind) {
        (Some(crate::server::name_state::AgentPresence::Offline), _) => "unknown",
        (_, Some(crate::server::name_state::AgentActivity::Busy)) => "busy",
        (_, Some(crate::server::name_state::AgentActivity::Idle)) => "idle",
        (_, Some(crate::server::name_state::AgentActivity::WaitingUser)) => "waiting_user",
        _ => "unknown",
    };
    let trusted_idle = presence == Some(crate::server::name_state::AgentPresence::Online)
        && activity_kind == Some(crate::server::name_state::AgentActivity::Idle);
    let idle_since_ms = trusted_idle
        .then(|| {
            activity
                .and_then(|fact| fact.idle_since_ms)
                .filter(|start| *start > 0 && *start <= now)
        })
        .flatten();
    let idle_duration_ms = idle_since_ms
        .map(|start| now.saturating_sub(start))
        .unwrap_or(0);
    json!({
        "project_root": project_root,
        "activity": label,
        "idle_since_ms": idle_since_ms,
        "idle_duration_ms": idle_duration_ms,
        "observed_ms": activity.map(|fact| fact.observed_ms),
    })
}

fn unknown_workspace_view(project_root: Option<String>) -> serde_json::Value {
    json!({
        "project_root": project_root,
        "activity": "unknown",
        "idle_since_ms": serde_json::Value::Null,
        "idle_duration_ms": 0,
        "observed_ms": serde_json::Value::Null,
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
                    worker_presence_in_state(server, &state, worker) == IdentityPresence::Present
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
            .filter(|worker| {
                worker_presence_in_state(server, &state, worker) == IdentityPresence::Present
            })
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
    // The typed grant is the only authority source. A route, reducer or scope
    // failure is an explicit error; a transport observation is reported
    // separately and never removes the holder. The endpoint probe must run
    // without the reducer lock.
    let (route_scope, holder) = {
        let state = server.state.lock().unwrap();
        let route_scope = match server_route_scope(server, &state) {
            Ok(route_scope) => route_scope,
            Err(error) => return Resp::err(error),
        };
        let holder = current_master_grant(&state, route_scope.as_ref())
            .and_then(|grant| state.workers.get(grant.agent_id.as_str()).cloned());
        (route_scope, holder)
    };
    // Identity presence proves the grant holder, not communication. The
    // endpoint value comes from the same independent observation the worker
    // status uses, and it is read outside the reducer lock.
    let endpoint_live = holder
        .as_ref()
        .and_then(|worker| worker_identity_and_endpoint(server, worker).1);
    let state = server.state.lock().unwrap();
    let workspace = workspace_idle_view(
        server,
        &state,
        holder.as_ref().map(|worker| worker.id.as_str()),
        now_ms(),
    );
    let mut master =
        master_authority_view_with_endpoint(&state, route_scope.as_ref(), endpoint_live);
    if let Some(master_object) = master.as_object_mut() {
        master_object.insert("workspace".into(), workspace);
    }
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

#[cfg(test)]
#[path = "../name_address_tests.rs"]
mod name_address_tests;

#[cfg(test)]
#[path = "../workspace_idle_tests.rs"]
mod workspace_idle_tests;

fn resolve_send_name_address(
    server: &Server,
    state: &State,
    target: String,
) -> Result<(String, Option<name_mailbox::MailboxNameAddress>), String> {
    let route = server_route_scope(server, state)?.ok_or_else(|| {
        "NAME_ADDRESS_SCOPE_MISSING: current project route is unavailable".to_string()
    })?;
    let project_scope = route.project_scope_id.as_str();
    let current_key = canonical_project_key(&server.root).ok_or_else(|| {
        "NAME_ADDRESS_SCOPE_MISSING: current project key is unavailable".to_string()
    })?;
    // `sendmessage`, `default` and channel-inbound routing share one address
    // owner. It resolves `ceo`/bare project keys through the typed grant and
    // rejects case-folded ambiguity instead of selecting the first holder.
    match resolve_named_target(state, project_scope, &current_key, &target)? {
        Some(resolved) => {
            authoritative_send_binding(state, &route, &resolved.worker_id)
                .map_err(str::to_owned)?;
            Ok((
                resolved.worker_id,
                Some(name_mailbox::MailboxNameAddress {
                    project_scope: project_scope.into(),
                    name: resolved.name,
                }),
            ))
        }
        // Raw worker IDs keep their legacy direct-send meaning. Conversation
        // defaults reject them because inbound has no stable way to re-resolve
        // an arbitrary ID.
        None => Ok((target, None)),
    }
}

/// Canonical project key used by list/send/default addresses. It is the
/// registered project root's file name, never derived from cwd or display text.
fn canonical_project_key(root: &std::path::Path) -> Option<String> {
    std::fs::canonicalize(root)
        .ok()?
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// Stable name currently occupied by a worker in this project, if any.
fn stable_name_for_worker(state: &State, project_scope: &str, worker_id: &str) -> Option<String> {
    state
        .name_occupancies
        .get(project_scope)
        .and_then(|names| names.values().find(|holder| holder.worker_id == worker_id))
        .map(|holder| holder.name.clone())
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

fn validate_channel_envelope_for_inbound(
    envelope: &crate::server::channel_envelope::ChannelEnvelope,
) -> Result<(), String> {
    if envelope.source_channel != "qqbot" {
        return Err("CHANNEL_ENVELOPE_CHANNEL_UNSUPPORTED: only qqbot inbound envelopes are admitted".into());
    }
    if envelope.channel_instance.trim().is_empty()
        || envelope.external_user.trim().is_empty()
        || envelope.external_conversation.trim().is_empty()
        || envelope.platform_message_id.trim().is_empty()
        || envelope.reply_route.trim().is_empty()
    {
        return Err(
            "CHANNEL_ENVELOPE_INCOMPLETE: inbound envelope is missing a required channel identity or reply route"
                .into(),
        );
    }
    Ok(())
}

/// A QQ reply is legal only for the original recipient. Stable-name handoff
/// keeps the right to answer when the same name moves to a new session.
fn channel_reply_authorized(state: &State, original: &Message, from: &str) -> bool {
    if original.to == from {
        return true;
    }
    state
        .mailbox_name_addresses
        .get(&original.id)
        .and_then(|address| {
            state
                .name_occupancies
                .get(address.project_scope.as_str())
                .and_then(|names| names.get(address.name.as_str()))
        })
        .is_some_and(|holder| holder.worker_id == from)
}

/// Commit one typed channel-inbound message and its envelope in the same
/// durable batch. A duplicate platform event resolves to the first message
/// instead of enqueuing a second one.
pub(crate) fn handle_channel_inbound(
    server: &Server,
    from: String,
    to: String,
    subject: String,
    body: String,
    envelope: crate::server::channel_envelope::ChannelEnvelope,
) -> Resp {
    let mut st = server.state.lock().unwrap();
    let intent = match channel_inbound_address_intent(
        &st,
        &envelope.source_channel,
        &envelope.channel_instance,
        &envelope.external_conversation,
        &body,
        &to,
    ) {
        Ok(intent) => intent,
        Err(error) => return Resp::err(error),
    };
    let project_key = canonical_project_key(&server.root);
    let is_ceo = intent.address.eq_ignore_ascii_case("ceo")
        || project_key
            .as_deref()
            .is_some_and(|key| intent.address.eq_ignore_ascii_case(key));
    let local_grant = if is_ceo {
        st.ceo_grant.clone()
    } else {
        None
    };
    let mut response = channel_inbound_commit_with_state(
        server,
        &mut st,
        from,
        subject,
        envelope,
        local_grant.as_ref(),
        &intent,
    );
    drop(st);
    channel_inbound_finish(server, &mut response);
    response
}

/// Lock-free inbound admission used by the host-aware manager path. The caller
/// holds `target_state` (and, when the target differs from the host, holds the
/// host state first). This commit parses the authoritative holder under the
/// same guard and never re-locks the target state.
#[allow(clippy::too_many_arguments)]
pub(crate) fn channel_inbound_commit_with_state(
    server: &Server,
    target_state: &mut State,
    from: String,
    subject: String,
    envelope: crate::server::channel_envelope::ChannelEnvelope,
    ceo_grant: Option<&crate::ceo::CeoGrant>,
    intent: &crate::server::ChannelInboundAddressIntent,
) -> Resp {
    let route = match server_route_scope(server, target_state) {
        Ok(Some(route)) => route,
        Ok(None) => {
            return Resp::err(
                "CHANNEL_ROUTE_SCOPE_MISSING: the current project route is unavailable",
            )
        }
        Err(error) => return Resp::err(error.to_owned()),
    };
    let project_scope = route.project_scope_id.as_str();
    let project_key = match canonical_project_key(&server.root) {
        Some(key) => key,
        None => {
            return Resp::err(
                "CHANNEL_ROUTE_SCOPE_MISSING: the current project key is unavailable",
            )
        }
    };
    let (worker_id, recipient_address, body_after_address) = if let Some(grant) = ceo_grant {
        let worker_id = match resolve_ceo_holder_from_grant(target_state, project_scope, grant) {
            Ok(worker_id) => worker_id,
            Err(error) => return Resp::err(error),
        };
        if let Err(error) = authoritative_send_binding(target_state, &route, &worker_id) {
            return Resp::err(error);
        }
        (worker_id, intent.address.clone(), intent.body.clone())
    } else {
        let worker_id = if intent.explicit_to && target_state.workers.contains_key(&intent.address) {
            intent.address.clone()
        } else {
            match resolve_address(target_state, &route, &project_key, &intent.address) {
                Ok(worker_id) => worker_id,
                Err(error) => return Resp::err(error),
            }
        };
        (
            worker_id,
            intent.address.clone(),
            intent.body.clone(),
        )
    };
    let mut envelope = envelope;
    envelope.recipient_address = recipient_address.clone();
    let projected =
        crate::server::channel_envelope::readable_projection(&envelope, &body_after_address);
    channel_inbound_commit_locked(
        server,
        target_state,
        from,
        worker_id,
        subject,
        projected,
        envelope,
    )
}

/// Commit the durable message and envelope while `target_state` is already
/// held. The caller must release the guard before notification.
#[allow(clippy::too_many_arguments)]
fn channel_inbound_commit_locked(
    server: &Server,
    st: &mut State,
    from: String,
    to: String,
    subject: String,
    body: String,
    envelope: crate::server::channel_envelope::ChannelEnvelope,
) -> Resp {
    let channel_envelope = Some(envelope);
    if let Err(error) = validate_channel_envelope_for_inbound(
        channel_envelope.as_ref().expect("channel envelope supplied"),
    ) {
        return Resp::err(error);
    }
    let key = crate::server::channel_envelope::platform_event_key(
        &channel_envelope.as_ref().expect("channel envelope supplied").channel_instance,
        &channel_envelope.as_ref().expect("channel envelope supplied").external_conversation,
        &channel_envelope.as_ref().expect("channel envelope supplied").platform_message_id,
    );
    if let Some(message_id) = st.platform_event_index.get(&key) {
        return Resp::data(json!({
            "msg_id": message_id,
            "deduplicated": true,
            "channel": channel_envelope.as_ref().expect("channel envelope supplied").source_channel,
            "platform_message_id": channel_envelope.as_ref().expect("channel envelope supplied").platform_message_id,
        }));
    }
    if !st.workers.contains_key(&to) {
        let (error, data) = handoff_unresolved_recipient(&to);
        return Resp::err_data(error, data);
    }
    let msg = Message {
        id: gen_msg_id(),
        from,
        to: to.clone(),
        mtype: "notify".into(),
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
    let mut events = vec![
        Event::Sent { msg },
        Event::DeliveryMode {
            msg_id: mid.clone(),
            mode: "immediate".into(),
            source_thread_id: None,
        },
    ];
    let envelope = channel_envelope.expect("channel envelope supplied");
    events.push(Event::ChannelEnvelopeRecorded {
        message_id: mid.clone(),
        idempotency_key: key,
        envelope,
    });
    if let Some(subscription) = &subscription {
        events.push(Event::WakeBound {
            message_id: mid.clone(),
            subscription_id: subscription.id.clone(),
        });
    }
    if let Err(error) = server.commit_locked(st, &events) {
        return Resp::err(format!("SEND_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "msg_id": mid,
        "notification": subscription.is_some().then(|| "queued").unwrap_or("mailbox-only-no-subscription"),
        "durable": true,
    }))
}

/// Notification runs only after the inbound commit released the state guard.
fn channel_inbound_finish(server: &Server, response: &mut Resp) {
    if response.data.get("deduplicated").and_then(|value| value.as_bool()) == Some(true) {
        return;
    }
    let Some(msg_id) = response
        .data
        .as_object()
        .and_then(|data| data.get("msg_id"))
        .and_then(|value| value.as_str())
        .map(str::to_owned)
    else {
        return;
    };
    let (subscription, to) = {
        let st = server.state.lock().unwrap();
        (
            st.wake_bindings.get(&msg_id)
                .and_then(|id| st.notification_subscriptions.get(id))
                .cloned(),
            st.msgs.get(&msg_id).map(|message| message.to.clone()),
        )
    };
    let Some(subscription) = subscription else {
        if let Some(to) = to {
            if let Some(data) = response.data.as_object_mut() {
                data.insert(
                    "notification".into(),
                    json!("mailbox-only-no-subscription"),
                );
                data.insert("notification_error".into(), json!(NOTIFICATION_SUBSCRIPTION_MISSING_ERROR));
                data.insert("failure".into(), json!("notification_subscription_missing"));
                data.insert("repair_required".into(), json!(true));
                data.insert("escalation".into(), json!(MAILBOX_ONLY_ESCALATION));
            }
        }
        return;
    };
    let notification = attempt_notification_detailed_with_at(server, &msg_id, &subscription.id, now_ms());
    let method = {
        let st = server.state.lock().unwrap();
        notification_method_for_worker(&st, &to.unwrap_or_default())
    };
    *response = notification_send_response(
        response.data.clone(),
        true,
        method,
        &notification,
    );
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
    reply_policy: Option<crate::proto::ReplyPolicy>,
    target_session: Option<String>,
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
    handle_send_with_task_policy(
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
        reply_policy,
        target_session,
        None,
    )
}

/// Backwards-compatible entry for callers that do not declare a reply
/// obligation. All ordinary sends run through here with no policy.
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
    handle_send_with_task_policy(
        server,
        from,
        to,
        mtype,
        subject,
        body,
        in_reply_to,
        delivery_mode,
        assign_task,
        managed_subagent_id,
        None,
        None,
        None,
    )
}

/// The real ordinary-send owner. `reply_policy` is the only source of a reply
/// obligation; a linked reply validates and clears the original obligation in
/// the same committed batch as the reply message.
#[allow(clippy::too_many_arguments)]
fn handle_send_with_task_policy(
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
    reply_policy: Option<crate::proto::ReplyPolicy>,
    target_session: Option<String>,
    channel_envelope: Option<crate::server::channel_envelope::ChannelEnvelope>,
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
    if let Some(envelope) = channel_envelope.as_ref() {
        if let Err(error) = validate_channel_envelope_for_inbound(envelope) {
            return Resp::err(error);
        }
        let key = crate::server::channel_envelope::platform_event_key(
            &envelope.channel_instance,
            &envelope.external_conversation,
            &envelope.platform_message_id,
        );
        if let Some(message_id) = st.platform_event_index.get(&key) {
            return Resp::data(json!({
                "msg_id": message_id,
                "deduplicated": true,
                "channel": envelope.source_channel,
                "platform_message_id": envelope.platform_message_id,
            }));
        }
    }
    let mut reply_route: Option<crate::server::channel_envelope::ChannelReplyRoute> = None;
    // Resolve and retain the original name under the durable admission lock.
    // A typed QQ reply uses the immutable envelope route and does not require a
    // local worker target; a normal send still resolves through directory facts.
    let (to, name_address) = if let Some(ref rid) = in_reply_to {
        let Some(original) = st.msgs.get(rid).cloned() else {
            return Resp::err(format!("in_reply_to message {} not found", rid));
        };
        match st.channel_envelopes.get(rid).cloned() {
            Some(envelope) => {
                if !channel_reply_authorized(&st, &original, &from) {
                    return Resp::err(
                        "CHANNEL_REPLY_FORBIDDEN: only the original recipient may reply through the original channel route",
                    );
                }
                if to.trim().is_empty() {
                    let route = crate::server::channel_envelope::ChannelReplyRoute::from_envelope(&envelope, rid);
                    let target = route.reply_route.clone();
                    reply_route = Some(route);
                    (target, None)
                } else if to == envelope.reply_route {
                    let route = crate::server::channel_envelope::ChannelReplyRoute::from_envelope(&envelope, rid);
                    reply_route = Some(route);
                    (to, None)
                } else {
                    return Resp::err(
                        "CHANNEL_REPLY_ROUTE_MISMATCH: explicit --to does not match the original channel reply route",
                    );
                }
            }
            None => match resolve_send_name_address(server, &st, to) {
                Ok(resolved) => resolved,
                Err(error) => return Resp::err(error),
            },
        }
    } else if to.trim().is_empty() {
        return Resp::err("SEND_TARGET_REQUIRED: sendmessage requires --to or a positional address");
    } else {
        match resolve_send_name_address(server, &st, to) {
            Ok(resolved) => resolved,
            Err(error) => return Resp::err(error),
        }
    };
    let managed_child = if assign_task {
        let Some(id) = managed_subagent_id else {
            return Resp::err("managed task requires an explicit subagent binding");
        };
        let Some(child) = st.subagents.get(id).cloned() else {
            return Resp::err(format!("unknown managed subagent {}", id));
        };
        let master = match current_master_holder(server, &st) {
            Ok(master) => master,
            Err(error) => return Resp::err(error),
        };
        if (child.parent != from && master.as_deref() != Some(from.as_str())) || child.peer != to {
            return Resp::err("managed subagent owner mismatch");
        }
        if child.status != "idle" {
            return Resp::err("subagent is not idle; query status instead of resending");
        }
        if board_peer_has_execution_reservation(&st, &child.peer, None) {
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
    let is_channel_reply = reply_route.is_some();
    if !is_channel_reply && !st.workers.contains_key(&to) {
        let (error, data) = handoff_unresolved_recipient(&to);
        return Resp::err_data(error, data);
    }
    if is_channel_reply && target_session.is_some() {
        return Resp::err(
            "CHANNEL_REPLY_TARGET_SESSION_UNSUPPORTED: a typed channel reply uses the original external route",
        );
    }
    if let Some(ref required_session) = target_session {
        let recipient_session = st
            .workers
            .get(&to)
            .and_then(|w| w.transport.as_ref())
            .and_then(|t| t.session_id.as_deref());
        if recipient_session != Some(required_session.as_str()) {
            return Resp::err(format!(
                "TARGET_SESSION_MISMATCH: target worker {} current session does not match requested target session {}",
                to, required_session
            ));
        }
    }
    // Durable send admission depends on the recipient record and binding.
    // Endpoint failure is reported by the notification owner after enqueue.
    let mut clear_obligation_id: Option<String> = None;
    if let Some(ref rid) = in_reply_to {
        if !st.msgs.contains_key(rid) {
            return Resp::err(format!("in_reply_to message {} not found", rid));
        }
        // Only an existing obligation adds the reply-identity gate. Legacy
        // linked messages keep their previous behavior.
        if let Some(obligation) = st.reply_obligations.get(rid) {
            if body.trim().is_empty() {
                return Resp::err("REPLY_BODY_REQUIRED: an obligated reply requires a nonempty answer");
            }
            if let Err(error) = reply_obligation::validate_reply(&st, obligation, &from, &to) {
                return Resp::err(error);
            }
            clear_obligation_id = Some(rid.clone());
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
    // A typed channel event is deduplicated only by its platform event
    // identity (checked above). The ordinary body shortcut must not fold two
    // distinct platform events, or one conversation's reply route would be
    // reused for another event with the same text.
    if let Some(existing) = st.msgs.values().find(|m| {
        channel_envelope.is_none()
            && m.from == from
            && m.to == to
            && m.mtype == mtype
            && m.subject == msg.subject
            && m.body == msg.body
            && m.state == "pending"
            && st.mailbox_name_addresses.get(&m.id) == name_address.as_ref()
            // A different link or reply policy is a different obligation even
            // when the body is identical.
            && m.in_reply_to == msg.in_reply_to
            && st.reply_obligations.get(&m.id).map(|o| o.policy) == reply_policy
            && st.target_sessions.get(&m.id) == target_session.as_ref()
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
    if !is_channel_reply {
        if let Err((error, data)) = resolve_handoff_target(server, &st, &to, &msg.body) {
            return Resp::err_data(error, data);
        }
    }
    let mid = msg.id.clone();
    let task_id = assign_task.then(|| format!("task-{mid}"));
    let subscription = st
        .matching_subscription(&to, "direct-message", None, now_ms())
        .filter(|_| !is_channel_reply)
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
    if let Some(envelope) = channel_envelope {
        let idempotency_key = crate::server::channel_envelope::platform_event_key(
            &envelope.channel_instance,
            &envelope.external_conversation,
            &envelope.platform_message_id,
        );
        events.push(Event::ChannelEnvelopeRecorded {
            message_id: mid.clone(),
            idempotency_key,
            envelope,
        });
    }
    if let Some(route) = reply_route {
        events.push(Event::ChannelReplyRouteBound {
            message_id: mid.clone(),
            route,
        });
    }
    // The obligation record and the message commit together. The reply-cleared
    // marker is on the original message id, not this reply.
    if let Some(policy) = reply_policy {
        let Some(project) = reply_obligation::resolve_scope(
            &st,
            &from,
            name_address
                .as_ref()
                .map(|address| address.project_scope.as_str()),
        ) else {
            return Resp::err(
                "SEND_BINDING_REJECTED: reply obligation project scope is unavailable",
            );
        };
        let recipient_name = name_address
            .as_ref()
            .map(|address| address.name.clone())
            .or_else(|| reply_obligation::stable_name(&st, &to));
        let obligation = reply_obligation::build_obligation(
            &st,
            mid.clone(),
            project,
            from.clone(),
            to.clone(),
            recipient_name,
            policy,
        );
        events.push(Event::ReplyObligationSet { obligation });
    }
    if let Some(message_id) = clear_obligation_id {
        events.push(Event::ReplyObligationCleared { message_id });
    }
    if let Some(address) = name_address {
        events.push(Event::MailboxNameAddressed {
            message_id: mid.clone(),
            address,
        });
    }
    events.push(Event::DeliveryMode {
        msg_id: mid.clone(),
        mode: delivery_mode,
        source_thread_id: None,
    });
    if let Some(ref required_session) = target_session {
        events.push(Event::TargetSessionBound {
            message_id: mid.clone(),
            target_session: required_session.clone(),
        });
    }
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
    if worker_identity_and_endpoint(server, &recipient).1 != Some(true) {
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

/// Cross-project role resolved from server-side control state only. The caller
/// never supplies this; a worker named "CEO" or a stale master grant cannot
/// manufacture authority. Priority order matches the user ruling: CEO covers
/// the whole project, then same-level master/master and peer/peer are allowed,
/// then peer->foreign master and master->foreign peer are denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CrossProjectRole {
    Ceo,
    Master,
    Peer,
}

fn cross_project_role(
    server: &Server,
    state: &State,
    ceo_grant: Option<&crate::ceo::CeoGrant>,
    worker_id: &str,
) -> Result<CrossProjectRole, String> {
    let route_scope = server_route_scope(server, state)
        .map_err(|error| format!("CROSS_PROJECT_ROLE_RESOLVE_FAILED: {error}"))?;
    // CEO authority is host-wide; a worker is the CEO only when it currently
    // occupies the grant's stable name inside the grant's workspace and app
    // scope. A same-named worker elsewhere grants nothing.
    if let Some(grant) = ceo_grant {
        let root_matches = grant.workspace == server.root.to_string_lossy().as_ref();
        let app_matches = route_scope
            .as_ref()
            .is_some_and(|scope| scope.app_scope_id.as_str() == grant.app_scope_id);
        if root_matches
            && app_matches
            && state
                .name_occupancy(&grant.workspace, &grant.name)
                .is_some_and(|holder| holder.worker_id == worker_id)
        {
            return Ok(CrossProjectRole::Ceo);
        }
    }
    if let Some(grant) = current_master_grant(state, route_scope.as_ref()) {
        if grant.agent_id.as_str() == worker_id {
            return Ok(CrossProjectRole::Master);
        }
    }
    if state.workers.contains_key(worker_id) {
        return Ok(CrossProjectRole::Peer);
    }
    Err(format!("worker {worker_id} not registered"))
}

/// Server-side-only cross-project authorization. The caller is already
/// authenticated by the runtime dispatch; here we judge the sender role, the
/// target registration, and the role matrix, then commit the durable message
/// under the target state lock. No client master evidence is consulted.
#[derive(Debug)]
pub(crate) struct CrossProjectCommitted {
    pub(crate) msg_id: String,
    pub(crate) subscription: Option<crate::server::state::NotificationSubscription>,
}

/// Authorization and durable commit happen under a single target state guard.
/// `sender_role` is resolved by the caller from current server-side state and
/// is never read from the request. This function never re-locks `server`.
fn commit_cross_project_message(
    server: &Server,
    st: &mut State,
    sender_worker_id: &str,
    source_project: &str,
    sender_role: CrossProjectRole,
    ceo_grant: Option<&crate::ceo::CeoGrant>,
    source_thread_id: Option<String>,
    to: &str,
    subject: &str,
    body: &str,
    in_reply_to: Option<&str>,
) -> Result<CrossProjectCommitted, Resp> {
    if subject.trim().is_empty() {
        return Err(Resp::err(
            "MESSAGE_SUBJECT_REQUIRED: cross-project send requires --subject",
        ));
    }
    let target_role = cross_project_role(server, st, ceo_grant, to).map_err(Resp::err)?;
    let allowed = match (sender_role, target_role) {
        (CrossProjectRole::Ceo, _) => true,
        (CrossProjectRole::Master, CrossProjectRole::Master) => true,
        (CrossProjectRole::Peer, CrossProjectRole::Peer) => true,
        _ => false,
    };
    if !allowed {
        let project_scope = GlobalState::canonical_project_scope(&server.root)
            .ok()
            .map(|scope| scope.as_str().to_owned())
            .unwrap_or_default();
        let target_label = stable_name_for_worker(st, &project_scope, to)
            .unwrap_or_else(|| to.to_owned());
        return Err(Resp::err(format!(
            "CROSS_PROJECT_SEND_REJECTED: a {} cannot send to a {}; the current target is {} ({})",
            role_label(sender_role),
            role_label(target_role),
            target_label,
            to,
        )));
    }
    if !st.workers.contains_key(to) {
        return Err(Resp::err(format!("recipient {} not registered", to)));
    }
    let msg = Message {
        id: gen_msg_id(),
        from: format!("{}@{}", sender_worker_id, source_project),
        to: to.to_owned(),
        mtype: "notify".into(),
        subject: Some(subject.to_owned()),
        body: body.to_owned(),
        in_reply_to: in_reply_to.map(str::to_owned),
        created_ms: now_ms(),
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    let mid = msg.id.clone();
    let subscription = st
        .matching_subscription(to, "direct-message", None, now_ms())
        .cloned();
    let mut events = vec![
        Event::Sent { msg },
        Event::DeliveryMode {
            msg_id: mid.clone(),
            mode: "explicit-notification".into(),
            source_thread_id,
        },
    ];
    if let Some(subscription) = &subscription {
        events.push(Event::WakeBound {
            message_id: mid.clone(),
            subscription_id: subscription.id.clone(),
        });
    }
    server
        .commit_locked(st, &events)
        .map_err(|error| Resp::err(format!("CROSS_PROJECT_SEND_DURABILITY_FAILED: {error}")))?;
    Ok(CrossProjectCommitted {
        msg_id: mid,
        subscription,
    })
}

/// Build the public send response and attempt notification after the commit
/// has released its state guard. Notification never reverses or authorizes the
/// already durable message.
fn notify_cross_project_committed(
    server: &Server,
    committed: CrossProjectCommitted,
    sender_worker_id: &str,
    to: &str,
) -> Resp {
    let CrossProjectCommitted {
        msg_id,
        subscription,
    } = committed;
    let notification = subscription
        .as_ref()
        .map(|subscription| {
            attempt_notification_detailed_with_at(server, &msg_id, &subscription.id, now_ms())
        })
        .unwrap_or_else(|| {
            NotificationAttempt::NotAttempted("no notification subscription".into())
        });
    notification_send_response(
        json!({
            "msg_id": msg_id,
            "cross_project": true,
            "sender": sender_worker_id,
            "recipient": to,
        }),
        subscription.is_some(),
        {
            let state = server.state.lock().unwrap();
            notification_method_for_worker(&state, to)
        },
        &notification,
    )
}

/// Target-side handler kept for direct reducer tests. It resolves the target
/// role, checks endpoint reachability outside the state lock, and commits the
/// message under one target guard.
fn handle_cross_project_send(
    server: &Server,
    sender_worker_id: &str,
    source_project: String,
    sender_role: CrossProjectRole,
    ceo_grant: Option<crate::ceo::CeoGrant>,
    source_thread_id: Option<String>,
    to: &str,
    subject: &str,
    body: &str,
    in_reply_to: Option<&str>,
) -> Resp {
    if source_project.trim().is_empty() {
        return Resp::err("cross-project send requires a resolved source project");
    }
    // Cross-project messages are durable mailbox entries. Identity admission
    // must not turn endpoint uncertainty or activity into an enqueue failure.
    // A tmux peer has a valid address even when its runtime identity remains
    // Unknown; only a definitively dead endpoint blocks enqueue.
    let recipient_snapshot = {
        let state = server.state.lock().unwrap();
        state.workers.get(to).cloned()
    };
    if let Some(recipient) = recipient_snapshot {
        if worker_identity_and_endpoint(server, &recipient).1 == Some(false) {
            return Resp::err(
                "cross-project communication requires a reachable target binding on its server-selected transport",
            );
        }
    }
    let mut st = server.state.lock().unwrap();
    let committed = match commit_cross_project_message(
        server,
        &mut st,
        sender_worker_id,
        &source_project,
        sender_role,
        ceo_grant.as_ref(),
        source_thread_id,
        to,
        subject,
        body,
        in_reply_to,
    ) {
        Ok(committed) => committed,
        Err(response) => return response,
    };
    drop(st);
    notify_cross_project_committed(server, committed, sender_worker_id, to)
}

fn role_label(role: CrossProjectRole) -> &'static str {
    match role {
        CrossProjectRole::Ceo => "CEO",
        CrossProjectRole::Master => "master",
        CrossProjectRole::Peer => "peer",
    }
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
    let canonical_worktree =
        match validate_worktree_path(&server.root, &server.config, &worktree_path) {
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
    if let Err(error)=work_executor_gate(server,&st,&task_id,&worker_id) {return error;}
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
        "next": "continue the existing claim; evidence remains attached"
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
    if let Err(error) = work_executor_gate(server,&st, &task_id, &worker_id) { return error; }
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
    if let Err(error) =
        server.commit_locked_checked(&mut st, &[Event::TaskUpdated { task: task.clone() }])
    {
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
    task_update_locked(
        server, &mut st, worker_id, token, task_id, status, next_step,
    )
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
    let original_status = task.status.clone();
    if status.as_deref().is_some_and(|value|matches!(value,"working"|"verifying"|"reviewed")) {
        if let Err(error)=work_executor_gate(server,st,&task_id,&worker_id) {return error;}
    }
    if let Some(new_status) = status {
        if !TASK_STATUSES.contains(&new_status.as_str()) {
            return Resp::err(format!(
                "invalid status {}; must be one of {:?}",
                new_status, TASK_STATUSES
            ));
        }
        if new_status == "closed" {
            return Resp::err("use collab task close after delivery and valid review");
        }
        if new_status == "delivered" {
            return Resp::err(
                "use collab task deliver to complete a claim; direct status mutation is rejected",
            );
        }
        if new_status == "working" && task.status == "assigned" {
            return Resp::err("use collab task accept to accept an assigned task");
        }
        if new_status == "accepted" {
            return Resp::err("use collab task review to accept a delivered task");
        }
        if new_status == "merged" {
            return Resp::err(
                "COLLAB_GIT_OWNERSHIP_DISABLED: Collab no longer records Git integration; close after delivery and valid review",
            );
        }
        if new_status == "waiting" {
            return Resp::err("use collab task wait so responsibility and deadline are durable");
        }
        if !task_transition_allowed(&task.status, &new_status) {
            return Resp::err(format!(
                "invalid task transition {} -> {}",
                task.status, new_status
            ));
        }
        if task.status == "blocked" && new_status == "working" {
            if let Err(error) = board_resume_gate(&st, &worker_id, &task.id) { return error; }
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
    if original_status != task.status && st.task_lifecycle.contains_key(&task_id)
        && matches!(task.status.as_str(), "working" | "verifying" | "rework" | "cancelled")
    {
        let mut record = st.task_lifecycle[&task_id].clone();
        let reviewer = required_work_reviewer(st, &task_id);
        events.extend(invalidate_work_review(st, &task, &mut record, reviewer));
        events.push(Event::TaskLifecycleUpdated { task_id:task_id.clone(),record });
    }
    if task.status != "accepted" && st.pending_merges.contains_key(&task_id) {
        let stale_notices = pending_merge_notice_ids(&st, &task_id);
        events.push(Event::MergeResolved {
            task_id: task_id.clone(),
            resolved_by: worker_id.clone(),
            reason: Some(format!(
                "task no longer awaits merge (status={})",
                task.status
            )),
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
                "action": "peer owns its declared task responsibility; Collab does not manage Git resources"
            })
        })
        .collect()
}

fn handle_task_deliver(
    server: &Server,
    worker_id: String,
    token: String,
    task_id: String,
    evidence: Option<String>,
    worktree: Option<String>,
) -> Resp {
    handle_task_deliver_with_intent(
        server,
        worker_id,
        token,
        task_id,
        evidence,
        worktree,
        false,
    )
}

fn handle_task_deliver_with_intent(
    server: &Server,
    worker_id: String,
    token: String,
    task_id: String,
    evidence: Option<String>,
    worktree: Option<String>,
    no_candidate: bool,
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
    if let Err(error)=work_executor_gate(server,&st,&task_id,&worker_id) {return error;}

    let Some(evidence) = evidence.filter(|value| !value.trim().is_empty()) else {
        return Resp::err("task deliver requires non-empty --evidence");
    };
    // Worktree values remain optional historical declarations. Delivery is a
    // Collab fact and does not resolve a Git candidate or require a repository.
    let worktree = worktree.filter(|value| !value.trim().is_empty());
    let now = now_ms();
    task.status = "delivered".to_string();
    task.wait = None;
    task.next_step = Some(
        "task owner or live master reviews delivery with collab task review --accept or --rework"
            .to_string(),
    );
    task.updated_ms = now;
    let mut lifecycle = st.task_lifecycle.get(&task.id).cloned().unwrap_or_default();
    let required_reviewer = required_work_reviewer(&st, &task_id);
    let mut events = invalidate_work_review(&st, &task, &mut lifecycle, required_reviewer.clone());
    lifecycle.delivery_kind = if no_candidate {
        state::TaskDeliveryKind::ReadOnly
    } else {
        state::TaskDeliveryKind::Code
    };
    lifecycle.delivery_evidence = Some(evidence.clone());
    lifecycle.delivered_ms = Some(now);
    lifecycle.review_evidence = None;
    lifecycle.reviewer = None;
    lifecycle.reviewed_ms = None;
    lifecycle.integration_commit = None;
    lifecycle.integration_evidence = None;
    lifecycle.integrated_ms = None;
    lifecycle.delivery_commit = None;
    let delivery_kind = lifecycle.delivery_kind;
    let mut review_notice = None;
    let mut reviewer_waiting = None;
    if let Some(name) = required_reviewer.as_deref() {
        task.next_step = Some(format!("await review by {name}; use collab task review --accept or --rework"));
        let route = match server_route_scope(server, &st) {
            Ok(Some(route)) => route,
            _ => return Resp::err("required reviewer project route is not ready"),
        };
        if let Some(recipient) = st.name_occupancy(route.project_scope_id.as_str(), name)
            .map(|entry| entry.worker_id.clone()).filter(|id| st.workers.contains_key(id))
        {
            let message_id = gen_msg_id();
            events.push(Event::Sent { msg: Message {
                id:message_id.clone(),from:worker_id.clone(),to:recipient.clone(),mtype:"request".into(),
                subject:Some(format!("review-required:{task_id}")),
                body:format!("Task {task_id} is delivered and requires your review as {name}. Read its task status and evidence. Submit collab task review with --accept or --rework and your evidence. The author's delivery evidence is: {evidence}"),
                in_reply_to:None,created_ms:now,state:"pending".into(),wake_attempt_count:0,
                last_wake_attempt_ms:0,retry_attempted:false,
            }});
            events.push(Event::DeliveryMode { msg_id:message_id.clone(),mode:"explicit-notification".into(),source_thread_id:None });
            let subscription = st.matching_subscription(&recipient,"direct-message",None,now).cloned();
            if let Some(subscription) = &subscription {
                events.push(Event::WakeBound { message_id:message_id.clone(),subscription_id:subscription.id.clone() });
            }
            review_notice = Some((message_id,subscription,notification_method_for_worker(&st,&recipient)));
        } else { reviewer_waiting = Some(format!("reviewer {name} is currently unavailable")); }
    }
    events.extend([
            Event::TaskUpdated { task: task.clone() },
            Event::TaskLifecycleUpdated {
                task_id: task.id.clone(),
                record: lifecycle,
            },
        ]);
    if let Err(error) = server.commit_locked(&mut st, &events) {
        drop(st);
        return Resp::err(format!("TASK_DELIVER_DURABILITY_FAILED: {error}"));
    }

    drop(st);
    let data = json!({
        "delivered": task.id,
        "status": task.status,
        "evidence": evidence,
        "worktree": worktree,
        "delivery_kind": delivery_kind.as_str(),
        "git_managed": false,
        "notification": "none",
        "required_reviewer": required_reviewer,
        "review_waiting": reviewer_waiting,
        "delivery_complete": false,
        "next_action": task.next_step,
        "identity": {"worker_id": worker.id, "kind": "peer"},
    });
    if let Some((message_id,subscription,method)) = review_notice {
        let attempt = match &subscription {
            Some(subscription) => attempt_notification_detailed_with_at(server,&message_id,&subscription.id,now_ms()),
            None => NotificationAttempt::NotAttempted(NOTIFICATION_SUBSCRIPTION_MISSING_ERROR.into()),
        };
        let mut data = data;
        data["review_message_id"] = json!(message_id);
        notification_send_response(data,subscription.is_some(),method,&attempt)
    } else { Resp::data(data) }
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
