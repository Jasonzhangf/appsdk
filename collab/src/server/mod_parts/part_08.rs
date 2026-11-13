use crate::proto::FederationResponse;

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
    control: &binding_slot::BindingControlSet,
    route_scope: Option<&RouteScope>,
    endpoint_live: Option<bool>,
) -> Result<serde_json::Value, String> {
    let Some(grant) = current_master_grant(state, route_scope) else {
        return Ok(serde_json::Value::Null);
    };
    Ok(json!({
        "worker_id": grant.agent_id.as_str(),
        "display_name": worker_display_name(control, route_scope, grant.agent_id.as_str())?,
        "scope": scope_view(route_scope),
        "endpoint_live": endpoint_live,
        "grant_id": grant.resource_id(),
        "grant_generation": grant.resource_generation(),
        "assigned_by": grant.granted_by,
        "approval": grant.approval,
        "assigned_ms": grant.granted_at_ms,
        "binding_id": grant.binding_id,
        "boundary": grant.boundary,
        "master_wake": state.master_wake,
    }))
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
        (_, Some(crate::server::name_state::AgentActivity::Error)) => "error",
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

fn live_idle_capacity_keys(
    server: &Server,
    state: &State,
) -> (std::collections::BTreeSet<String>, std::collections::BTreeSet<String>) {
    let live_idle_subagents = state
        .subagents
        .values()
        .filter(|record| record.status == "idle")
        .filter(|record| {
            state.workers.get(&record.peer).is_some_and(|worker| {
                worker_presence_in_state(server, state, worker) == IdentityPresence::Present
            })
        })
        .filter(|record| !board_peer_has_execution_reservation(state, &record.peer, None))
        .map(|record| record.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let live_idle_workers = state
        .workers
        .values()
        .filter(|worker| {
            worker_presence_in_state(server, state, worker) == IdentityPresence::Present
        })
        .filter(|worker| {
            state
                .keepalives
                .get(&worker.id)
                .is_some_and(|record| record.observed == "idle")
        })
        .filter(|worker| !board_peer_has_execution_reservation(state, &worker.id, None))
        .map(|worker| worker.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    (live_idle_workers, live_idle_subagents)
}

pub(crate) fn project_master_wake_idle_capacity(
    server: &Server,
    state: &State,
) -> crate::server::notification_state::MasterWakeAccumulator {
    let (live_idle_workers, live_idle_subagents) = live_idle_capacity_keys(server, state);
    let mut accumulator = state.master_wake.clone();
    notification_state::retain_live_idle_workers(
        &mut accumulator,
        &live_idle_workers,
        &live_idle_subagents,
    );
    accumulator
}

fn handle_master_status(server: &Server) -> Resp {
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
    let projected_master_wake = project_master_wake_idle_capacity(server, &state);
    let workspace = workspace_idle_view(
        server,
        &state,
        holder.as_ref().map(|worker| worker.id.as_str()),
        now_ms(),
    );
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
        Ok(control) => control, Err(error) => return Resp::err(error),
    };
    let mut master = match master_authority_view_with_endpoint(&state, &control, route_scope.as_ref(), endpoint_live) {
        Ok(master) => master, Err(error) => return Resp::err(error),
    };
    if let Some(master_object) = master.as_object_mut() {
        master_object.insert("workspace".into(), workspace);
        master_object.insert("master_wake".into(), json!(projected_master_wake));
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
#[path = "../workspace_idle_tests.rs"]
mod workspace_idle_tests;

fn resolve_send_name_address(
    server: &Server,
    state: &State,
    control: &binding_slot::BindingControlSet,
    target: String,
) -> Result<(String, Option<name_mailbox::MailboxNameAddress>), String> {
    let route = server_route_scope(server, state)?.ok_or_else(|| {
        "NAME_ADDRESS_SCOPE_MISSING: current project route is unavailable".to_string()
    })?;
    let current_key = canonical_project_key(&server.root).ok_or_else(|| {
        "NAME_ADDRESS_SCOPE_MISSING: current project key is unavailable".to_string()
    })?;
    if target.contains('%') {
        let projects = [current_key.clone()];
        let context = crate::address::AddressContext { local_projects: &projects, ..Default::default() };
        let resolved = crate::address::resolve_address(&crate::proto::AddressInput::Legacy(target.clone()), &context)
            .map_err(|error| error.to_string())?;
        if let crate::address::ResolvedAddress::LocalName(address) = resolved {
            if !address.project.eq_ignore_ascii_case(&current_key) {
                return Err("CROSS_PROJECT_ADDRESS_UNSUPPORTED: the encoded address is outside the current project".into());
            }
            let resolved = resolve_name_case_insensitive(control, &route, &address.name)?;
            authoritative_send_binding(state, &route, &resolved.worker_id).map_err(str::to_owned)?;
            return Ok((resolved.worker_id, Some(name_mailbox::MailboxNameAddress {
                project_scope: route.project_scope_id.as_str().into(), name: resolved.name,
            })));
        }
    }
    // `sendmessage`, `default` and channel-inbound routing share one address
    // owner. It resolves `ceo`/bare project keys through the typed grant and
    // rejects case-folded ambiguity instead of selecting the first holder.
    match resolve_named_target(state, control, &route, &current_key, &target)? {
        Some(resolved) => {
            authoritative_send_binding(state, &route, &resolved.worker_id)
                .map_err(str::to_owned)?;
            Ok((
                resolved.worker_id,
                Some(name_mailbox::MailboxNameAddress {
                    project_scope: route.project_scope_id.as_str().into(),
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

pub(crate) fn authoritative_send_binding<'a>(
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
fn channel_reply_authorized(state: &State, control: &binding_slot::BindingControlSet,
    route: &RouteScope, original: &Message, from: &str) -> Result<bool, String> {
    if original.to == from {
        return Ok(true);
    }
    let Some(address) = state.mailbox_name_addresses.get(&original.id) else { return Ok(false); };
    let project = ProjectScopeId::new(&address.project_scope).map_err(|error| error.to_string())?;
    Ok(control.current_for_name(&project, &route.app_scope_id, &address.name)?
        .and_then(|slot| slot.active_binding()).is_some_and(|current| current.binding.agent_id.as_str() == from))
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
    handle_channel_inbound_impl(
        server,
        None,
        from,
        to,
        subject,
        body,
        envelope,
    )
}

pub(crate) fn handle_channel_inbound_with_rex_recovery(
    server: &Arc<Server>,
    from: String,
    to: String,
    subject: String,
    body: String,
    envelope: crate::server::channel_envelope::ChannelEnvelope,
) -> Resp {
    handle_channel_inbound_impl(
        server,
        Some(server),
        from,
        to,
        subject,
        body,
        envelope,
    )
}

fn handle_channel_inbound_impl(
    server: &Server,
    recovery_server: Option<&Arc<Server>>,
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
    channel_inbound_finish(server, recovery_server, &mut response);
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
    let project_key = match canonical_project_key(&server.root) {
        Some(key) => key,
        None => {
            return Resp::err(
                "CHANNEL_ROUTE_SCOPE_MISSING: the current project key is unavailable",
            )
        }
    };
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
        Ok(control) => control, Err(error) => return Resp::err(error),
    };
    let (worker_id, recipient_address, body_after_address) = if let Some(grant) = ceo_grant {
        let worker_id = match resolve_ceo_holder_from_grant(target_state, &control, &route, grant) {
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
            match resolve_address(target_state, &control, &route, &project_key, &intent.address) {
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
fn channel_inbound_finish(
    server: &Server,
    recovery_server: Option<&Arc<Server>>,
    response: &mut Resp,
) {
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
    let notification = match recovery_server {
        Some(server) => attempt_notification_detailed_with_rex_recovery(
            server,
            &msg_id,
            &subscription.id,
            now_ms(),
        ),
        None => attempt_notification_detailed_with_at(
            server,
            &msg_id,
            &subscription.id,
            now_ms(),
        ),
    };
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
    let Some(command) = command else {
        return Resp::err(
            "LEGACY_SEND_REJECTED: authenticated sender binding and route scope are required",
        );
    };
    let (worker, _runtime, _registered_scope) =
        match authenticated_send_admission(&st, &worker_id, &token, &command, app_scope.as_ref()) {
            Ok(values) => values,
            Err(response) => return response,
        };
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

/// Source-side send admission shared by the real send command and the
/// read-only capability check. It stops before recipient resolution and any
/// message, wake, notification, or adapter mutation.
pub(crate) fn authenticated_send_admission(
    state: &State,
    worker_id: &str,
    token: &str,
    command: &crate::proto::CommandEnvelope,
    app_scope: Option<&AppServerId>,
) -> Result<(WorkerRec, crate::identity::RuntimeIdentity, RouteScope), Resp> {
    let Some(worker) = state.workers.get(worker_id).cloned() else {
        return Err(Resp::err(format!("worker {} not registered", worker_id)));
    };
    if worker.token != token {
        return Err(Resp::err("token mismatch: identity does not own this worker_id"));
    }
    let (runtime, registered_scope) = match app_scope {
        Some(context_app_scope) => {
            if command.scope.app_scope_id != *context_app_scope {
                return Err(Resp::err(
                    "SEND_BINDING_REJECTED: command app scope does not match request context",
                ));
            }
            let worker_project_scope =
                match GlobalState::canonical_project_scope(Path::new(&worker.cwd)) {
                    Ok(scope) => scope,
                    Err(error) => return Err(Resp::err(format!("SEND_BINDING_REJECTED: {error}"))),
                };
            if command.scope.project_scope_id != worker_project_scope {
                return Err(Resp::err(
                    "SEND_BINDING_REJECTED: command project scope does not match registered worker cwd",
                ));
            }
            let Some(binding) = state
                .global
                .lookup_binding_for(&command.scope, &command.actor_binding_id)
                .cloned()
            else {
                return Err(Resp::err(
                    "SEND_BINDING_REJECTED: authenticated sender binding is not registered for the request route",
                ));
            };
            if binding.agent_id.as_str() != worker.id {
                return Err(Resp::err(
                    "SEND_BINDING_REJECTED: authenticated sender binding belongs to another worker",
                ));
            }
            let runtime = crate::identity::RuntimeIdentity {
                agent_id: binding.agent_id.clone(),
                runtime_id: binding.runtime_id.clone(),
                appserver_id: binding.app_scope_id.clone(),
                binding_id: binding.binding_id.clone(),
                session_id: binding.session_id.clone(),
                native_thread_id: binding.native_thread_id.clone(),
            };
            (runtime, binding.route_scope())
        }
        None => {
            // Direct in-process callers predate the wire ProjectContext and
            // are bound to the compatibility tui-default route established by
            // handle_register. Wire requests never use this branch.
            let appserver_id = match crate::identity::AppServerId::new("tui-default") {
                Ok(id) => id,
                Err(error) => return Err(Resp::err(error.to_string())),
            };
            let registered_scope =
                match RouteScope::for_registered_project(appserver_id, Path::new(&worker.cwd)) {
                    Ok(scope) => scope,
                    Err(error) => return Err(Resp::err(error.to_string())),
                };
            let binding = match authoritative_send_binding(state, &registered_scope, worker_id) {
                Ok(binding) => binding.clone(),
                Err(error) => return Err(Resp::err(error)),
            };
            if command.actor_binding_id != binding.binding_id {
                return Err(Resp::err(format!(
                    "SEND_BINDING_REJECTED: actor binding mismatch: expected {}, observed {}",
                    binding.binding_id, command.actor_binding_id
                )));
            }
            if command.scope != registered_scope {
                return Err(Resp::err(
                    "SEND_BINDING_REJECTED: envelope scope does not match authoritative route scope",
                ));
            }
            let runtime = crate::identity::RuntimeIdentity {
                agent_id: binding.agent_id.clone(),
                runtime_id: binding.runtime_id.clone(),
                appserver_id: binding.app_scope_id.clone(),
                binding_id: binding.binding_id.clone(),
                session_id: binding.session_id.clone(),
                native_thread_id: binding.native_thread_id.clone(),
            };
            (runtime, registered_scope)
        }
    };
    if let Err(error) = command.validate_for(&runtime, &registered_scope) {
        return Err(Resp::err(format!("SEND_BINDING_REJECTED: {error}")));
    }
    Ok((worker, runtime, registered_scope))
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
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
        Ok(control) => control, Err(error) => return Resp::err(error),
    };
    let mut st = server.state.lock().unwrap();
    let route = match server_route_scope(server, &st) {
        Ok(Some(route)) => route, Ok(None) => return Resp::err("SEND_BINDING_REJECTED: project route is missing"),
        Err(error) => return Resp::err(error),
    };
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
                let authorized = match channel_reply_authorized(&st, &control, &route, &original, &from) {
                    Ok(authorized) => authorized, Err(error) => return Resp::err(error),
                };
                if !authorized {
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
            None => match resolve_send_name_address(server, &st, &control, to) {
                Ok(resolved) => resolved,
                Err(error) => return Resp::err(error),
            },
        }
    } else if to.trim().is_empty() {
        return Resp::err("SEND_TARGET_REQUIRED: sendmessage requires --to or a positional address");
    } else {
        match resolve_send_name_address(server, &st, &control, to) {
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
            if let Err(error) = reply_obligation::validate_reply(&control, &route.app_scope_id, obligation, &from, &to) {
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
            isolation_mode: None,
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
        let recipient_name = match name_address.as_ref() {
            Some(address) => Some(address.name.clone()),
            None => match reply_obligation::stable_name(&control, &route.project_scope_id, &route.app_scope_id, &to) {
                Ok(name) => name, Err(error) => return Resp::err(error),
            },
        };
        let from_name = match reply_obligation::stable_name(&control, &route.project_scope_id, &route.app_scope_id, &from) {
            Ok(name) => name, Err(error) => return Resp::err(error),
        };
        let obligation = reply_obligation::build_obligation(
            from_name,
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

/// A message committed for an authenticated source and an exact target.
#[derive(Debug)]
pub(crate) struct CrossProjectCommitted {
    pub(crate) msg_id: String,
    pub(crate) subscription: Option<crate::server::state::NotificationSubscription>,
}

/// Source authentication precedes this target-side durable commit. Registered
/// callers can send across every role pair. This function never re-locks server.
fn commit_cross_project_message(
    server: &Server,
    st: &mut State,
    sender_worker_id: &str,
    source_project: &str,
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

fn federation_error_response(error: crate::server::federation::FederationError) -> FederationResponse {
    FederationResponse::Error {
        code: error.code.to_owned(),
        detail: error.detail,
    }
}

fn verify_federation_connection(
    connection: &crate::server::federation::AuthenticatedMachine,
    local_machine: &crate::proto::MachineName,
    source_machine: &crate::proto::MachineName,
) -> Result<(), crate::server::federation::FederationError> {
    use crate::server::federation::FederationError;
    if &connection.target != local_machine {
        return Err(FederationError::new(
            "MACHINE_IDENTITY_MISMATCH",
            format!(
                "connection targets {} but this daemon is {local_machine}",
                connection.target
            ),
        ));
    }
    if source_machine != &connection.source {
        return Err(FederationError::new(
            "FEDERATION_SOURCE_MISMATCH",
            format!(
                "request source {source_machine} does not match authenticated source {}",
                connection.source
            ),
        ));
    }
    Ok(())
}

fn federation_consumption_facts(
    st: &State,
    message_id: &str,
) -> crate::server::federation::ConsumptionFacts {
    let consumed = st
        .msgs
        .get(message_id)
        .is_some_and(|message| message.state == "read");
    let accepted = st
        .notification_delivery_accepted
        .contains_key(message_id);
    let known_not_delivered = st
        .notification_delivery_failures
        .contains_key(message_id);
    let attempted = accepted
        || known_not_delivered
        || st.notification_delivery_bindings.contains_key(message_id)
        || st.wake_bindings.contains_key(message_id);
    crate::server::federation::notification_facts(
        consumed,
        accepted,
        known_not_delivered,
        attempted,
    )
}

fn federation_receipt_response(
    st: &State,
    reference: &crate::proto::MessageRef,
    requester: &crate::proto::FederationActor,
) -> FederationResponse {
    let Some(message_id) = st.federation.message_id_for_reference(reference) else {
        if st.federation.expired_origins.values().any(|expired| &expired.reference == reference) {
            return FederationResponse::Expired { reference: reference.clone() };
        }
        return FederationResponse::Error {
            code: "NOT_SEEN".into(),
            detail: "no accepted message for this reference".into(),
        };
    };
    let facts = federation_consumption_facts(st, message_id);
    match crate::server::federation::project_receipt(&st.federation, reference, requester, &facts)
    {
        Ok(mut receipt) => {
            receipt.channel_delivery = channel_delivery::current_result(st, message_id);
            FederationResponse::Message { receipt }
        }
        Err(error) => federation_error_response(error),
    }
}

/// Resolve the exact local target for a federation address before acceptance.
/// It uses the same current stable-name owner as local sends and never caches a
/// remote identity. The returned worker id is the current mailbox carrier.
pub(crate) fn resolve_federated_local_recipient(
    server: &Server,
    state: &State,
    local_machine: &crate::proto::MachineName,
    target: &crate::proto::LocalNameAddress,
) -> Result<crate::server::federation::ResolvedRecipient, crate::server::federation::FederationError>
{
    use crate::server::federation::FederationError;
    let route = server_route_scope(server, state)
        .map_err(|error| FederationError::new("FEDERATION_LOCAL_ROUTE_FAILED", error))?
        .ok_or_else(|| {
            FederationError::new("NAME_ADDRESS_SCOPE_MISSING", "current project route is unavailable")
        })?;
    let project_key = canonical_project_key(&server.root).ok_or_else(|| {
        FederationError::new("NAME_ADDRESS_SCOPE_MISSING", "current project key is unavailable")
    })?;
    if !target.project.eq_ignore_ascii_case(&project_key) {
        return Err(FederationError::new(
            "ADDRESS_PROJECT_MISMATCH",
            format!("{:?} is not the current project key {project_key}", target),
        ));
    }
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .map_err(|error| FederationError::new("FEDERATION_LOCAL_ROUTE_FAILED", error.to_string()))?;
    let resolved = crate::server::resolve_named_target(
        state,
        &control,
        &route,
        &project_key,
        &target.name,
    )
    .map_err(|error| FederationError::new("FEDERATION_LOCAL_ROUTE_FAILED", error))?
    .ok_or_else(|| {
        FederationError::new(
            "FEDERATION_LOCAL_NAME_UNRESOLVED",
            format!("{} in project {}", target.name, target.project),
        )
    })?;
    if !state.workers.contains_key(&resolved.worker_id) {
        return Err(FederationError::new(
            "FEDERATION_LOCAL_NAME_UNRESOLVED",
            format!("resolved worker {} is not registered", resolved.worker_id),
        ));
    }
    authoritative_send_binding(state, &route, &resolved.worker_id)
        .map_err(|error| FederationError::new("SEND_BINDING_REJECTED", error))?;
    Ok(crate::server::federation::ResolvedRecipient {
        worker_id: resolved.worker_id.clone(),
        principal: crate::proto::FederationPrincipal {
            address: crate::proto::QualifiedNameAddress {
                machine: local_machine.clone(),
                local: crate::proto::LocalNameAddress {
                    project: target.project.clone(),
                    name: resolved.name.clone(),
                },
            },
            project_scope: route.project_scope_id.clone(),
            app_scope_id: route.app_scope_id.clone(),
            stable_name: resolved.name.clone(),
        },
        address: crate::proto::LocalNameAddress {
            project: target.project.clone(),
            name: resolved.name,
        },
    })
}

/// Atomic target-side federation acceptance. The caller passes the typed source
/// fact from the auth owner and a local target already resolved by
/// `resolve_federated_local_recipient`. The mailbox row and exact dedup index
/// commit in one current-state batch.
pub(crate) fn accept_federated_message(
    server: &Server,
    local_machine: &crate::proto::MachineName,
    connection: &crate::server::federation::AuthenticatedMachine,
    request: &crate::proto::FederationPut,
    recipient: &crate::server::federation::ResolvedRecipient,
    now: i64,
) -> FederationResponse {
    use crate::server::federation::{AcceptDisposition, FederatedMessageMeta, FederationError};
    let sender_display = crate::server::federation::display_sender(&request.sender);
    let crate::proto::FederationDestination::Named(target) = &request.target else {
        return federation_error_response(FederationError::new(
            "ADDRESS_MISMATCH",
            "external channel inbound cannot declare a channel-owner destination",
        ));
    };
    if &connection.target != local_machine
        || &recipient.principal.address.machine != local_machine
        || !recipient
            .address
            .project
            .eq_ignore_ascii_case(&target.project)
        || !recipient.address.name.eq_ignore_ascii_case(&target.name)
    {
        return federation_error_response(FederationError::new(
            "ADDRESS_MISMATCH",
            "resolved target does not match the federation request target",
        ));
    }

    let mut st = server.state.lock().unwrap();
    let existing_body = st
        .federation
        .message_id_for_origin(&request.origin)
        .and_then(|message_id| st.msgs.get(message_id))
        .map(|message| message.body.as_str());
    let existing_target_session = st
        .federation
        .message_id_for_origin(&request.origin)
        .and_then(|message_id| st.target_sessions.get(message_id))
        .map(String::as_str);
    let disposition = match crate::server::federation::plan_accept(
        &st.federation,
        connection,
        local_machine,
        request,
        existing_body,
        existing_target_session,
    ) {
        Ok(disposition) => disposition,
        Err(error) => return federation_error_response(error),
    };
    match disposition {
        AcceptDisposition::Reuse(reference) => {
            federation_receipt_response(&st, &reference, &request.sender)
        }
        AcceptDisposition::Expired(reference) => FederationResponse::Expired { reference },
        AcceptDisposition::New => {
            let mid = gen_msg_id();
            let msg_id = match crate::identity::MessageId::new(mid.clone()) {
                Ok(msg_id) => msg_id,
                Err(error) => {
                    return federation_error_response(FederationError::new(
                        "MESSAGE_ID_INVALID",
                        error.to_string(),
                    ));
                }
            };
            let reference = crate::proto::MessageRef {
                machine: local_machine.clone(),
                project_scope: recipient.principal.project_scope.clone(),
                app_scope_id: recipient.principal.app_scope_id.clone(),
                msg_id,
            };
            let meta = FederatedMessageMeta {
                origin: request.origin.clone(),
                sender: request.sender.clone(),
                target: request.target.clone(),
                recipient: crate::proto::FederationActor::Peer(recipient.principal.clone()),
                reference: reference.clone(),
                mtype: request.mtype.clone(),
                subject: request.subject.clone(),
                delivery: request.delivery.clone(),
                in_reply_to: request.in_reply_to.clone(),
                original_origin: request.original_origin.clone(),
                reply_policy: request.reply_policy,
                channel: request.channel.clone(),
                accepted_ms: now,
            };
            let msg = Message {
                id: mid.clone(),
                from: sender_display.clone(),
                to: recipient.worker_id.clone(),
                mtype: request.mtype.clone(),
                subject: Some(request.subject.clone()),
                body: request.body.clone(),
                in_reply_to: None,
                created_ms: now,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            };
            let subscription = st
                .matching_subscription(&recipient.worker_id, "direct-message", None, now)
                .cloned();
            let mut events = vec![
                Event::Sent { msg },
                Event::DeliveryMode {
                    msg_id: mid.clone(),
                    mode: request.delivery.clone(),
                    source_thread_id: None,
                },
                Event::MailboxNameAddressed {
                    message_id: mid.clone(),
                    address: name_mailbox::MailboxNameAddress {
                        project_scope: recipient.principal.project_scope.as_str().into(),
                        name: recipient.principal.stable_name.clone(),
                    },
                },
            ];
            if let Some(policy) = request.reply_policy {
                events.push(Event::ReplyObligationSet {
                    obligation: reply_obligation::build_obligation(
                        None, mid.clone(), recipient.principal.project_scope.as_str().into(),
                        sender_display.clone(), recipient.worker_id.clone(),
                        Some(recipient.principal.stable_name.clone()), policy,
                    ),
                });
            }
            if let Some(target_session) = request.target_session.as_ref() {
                events.push(Event::TargetSessionBound {
                    message_id: mid.clone(),
                    target_session: target_session.clone(),
                });
            }
            if let Some(subscription) = &subscription {
                events.push(Event::WakeBound {
                    message_id: mid.clone(),
                    subscription_id: subscription.id.clone(),
                });
            }
            events.push(Event::FederationAccepted {
                message_id: mid.clone(),
                meta,
            });
            if let Err(error) = server.commit_locked(&mut st, &events) {
                return federation_error_response(FederationError::new(
                    "FEDERATION_ACCEPT_DURABILITY_FAILED",
                    error,
                ));
            }
            drop(st);
            notify_cross_project_committed(
                server,
                CrossProjectCommitted {
                    msg_id: mid,
                    subscription,
                },
                &sender_display,
                &recipient.worker_id,
            );
            let st = server.state.lock().unwrap();
            federation_receipt_response(&st, &reference, &request.sender)
        }
    }
}

/// Same-origin `Lookup` recovery. It never creates a new operation id and
/// never invents a mailbox fact.
pub(crate) fn lookup_federated_message(
    server: &Server,
    local_machine: &crate::proto::MachineName,
    connection: &crate::server::federation::AuthenticatedMachine,
    origin: &crate::proto::OriginMessageRef,
    requester: &crate::proto::FederationActor,
    compare: Option<&crate::proto::FederationPut>,
) -> FederationResponse {
    use crate::server::federation::FederationError;
    if let Err(error) = verify_federation_connection(connection, local_machine, &origin.machine) {
        return federation_error_response(error);
    }
    if requester.machine() != connection.source.as_str() {
        return federation_error_response(FederationError::new(
            "FEDERATION_SOURCE_MISMATCH",
            "requester is not the authenticated source machine",
        ));
    }
    let st = server.state.lock().unwrap();
    if let Some(expired) = st
        .federation
        .expired_origins
        .get(&crate::server::federation::origin_key(origin))
    {
        return FederationResponse::Expired {
            reference: expired.reference.clone(),
        };
    }
    let Some(meta) = st.federation.message_for_origin(origin) else {
        return FederationResponse::NotSeen;
    };
    let Some(compare) = compare else {
        return federation_receipt_response(&st, &meta.reference, requester);
    };
    let existing_body = st
        .federation
        .message_id_for_origin(origin)
        .and_then(|message_id| st.msgs.get(message_id))
        .map(|message| message.body.as_str());
    let existing_target_session = st
        .federation
        .message_id_for_origin(origin)
        .and_then(|message_id| st.target_sessions.get(message_id))
        .map(String::as_str);
    match crate::server::federation::compare_accepted_intent(
        &st.federation,
        origin,
        requester,
        compare,
        existing_body,
        existing_target_session,
    ) {
        Ok(reference) => federation_receipt_response(&st, &reference, requester),
        Err(error) => federation_error_response(error),
    }
}

/// Formal receipt query for a target-owned `MessageRef`. Only a participant of
/// the referenced message may read it.
pub(crate) fn federated_message_receipt(
    server: &Server,
    local_machine: &crate::proto::MachineName,
    connection: &crate::server::federation::AuthenticatedMachine,
    reference: &crate::proto::MessageRef,
    requester: &crate::proto::FederationActor,
) -> FederationResponse {
    use crate::server::federation::FederationError;
    let requester_machine = requester.machine().to_owned();
    if let Err(error) = verify_federation_connection(connection, local_machine, &requester_machine) {
        return federation_error_response(error);
    }
    if &reference.machine != local_machine {
        return federation_error_response(FederationError::new(
            "FEDERATION_REFERENCE_MISMATCH",
            "this daemon does not own the requested reference",
        ));
    }
    let st = server.state.lock().unwrap();
    federation_receipt_response(&st, reference, requester)
}

/// Target-side handler kept for direct reducer tests. It checks endpoint
/// reachability outside the state lock and commits the
/// message under one target guard.
fn handle_cross_project_send(
    server: &Server,
    sender_worker_id: &str,
    source_project: String,
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
    worktree_path: Option<String>,
    branch: Option<String>,
    base_commit: Option<String>,
    priority: String,
    next_step: Option<String>,
    goal_prompt: Option<String>,
) -> Resp {
    let mut st = server.state.lock().unwrap();
    handle_task_register_with_next_locked(
        server,
        &mut st,
        None,
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
    )
}

fn handle_task_register_with_next_locked(
    server: &Server,
    st: &mut State,
    control: Option<&binding_slot::BindingControlSet>,
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
        match declare_task_workspace(&server.root, &server.config, path) {
            Ok(declared) => worktree_path = Some(declared),
            Err(error) => return Resp::err(error),
        }
    }
    let branch = match worktree_path.as_deref() {
        Some(path) => match resolve_task_branch(path, branch.as_deref()) {
            Ok(resolved) => resolved,
            Err(error) => return Resp::err(error),
        },
        None => branch,
    };
    let isolation_mode = worktree_path
        .as_deref()
        .map(|path| derive_isolation_mode(Some(path)));
    let now = now_ms();
    let assignment_event = match control {
        Some(control) => {
            let route = match server_route_scope(server, st) {
                Ok(Some(route)) => route,
                Ok(None) => return Resp::err("project route is not ready"),
                Err(error) => return Resp::err(error),
            };
            let slot = match control.current_for_worker(
                &route.project_scope_id,
                &route.app_scope_id,
                &worker_id,
            ) {
                Ok(Some(slot)) => slot,
                Ok(None) => {
                    return Resp::err(
                        "TASK_CURRENT_BINDING_REQUIRED: registration requires the current stable name and session",
                    )
                }
                Err(error) => return Resp::err(error),
            };
            let current = match slot.active_binding() {
                Some(current) => current,
                None => {
                    return Resp::err(
                        "TASK_CURRENT_BINDING_REQUIRED: registration requires the current stable name and session",
                    )
                }
            };
            let Some(session) = current.binding.session_id.as_ref() else {
                return Resp::err(
                    "TASK_CURRENT_BINDING_REQUIRED: registration requires the current stable name and session",
                );
            };
            Some(Event::TaskAssignmentUpdated {
                assignment: name_state::TaskAssignment {
                    task_id: task_id.clone(),
                    name: slot.identity_key.name.clone(),
                    session_id: session.as_str().into(),
                    assignment_revision: st
                        .task_assignment(&task_id)
                        .map_or(1, |assignment| assignment.assignment_revision + 1),
                    reflow: None,
                    updated_ms: now,
                },
            })
        }
        None => None,
    };
    let commit_events = |st: &mut State, events: &[Event]| -> Result<(), String> {
        match control {
            Some(control) => server
                .commit_locked_checked_with_control(st, events, control)
                .map(|_| ())
                .map_err(|error| error.to_string()),
            None => server
                .commit_locked_checked(st, events)
                .map(|_| ())
                .map_err(|error| error.to_string()),
        }
    };
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
            isolation_mode: isolation_mode.clone(),
            created_ms: now,
            updated_ms: now,
        };
        let mut events = vec![Event::TaskCreated { task: blocked_task }];
        if let Some(assignment) = assignment_event.clone() {
            events.push(assignment);
        }
        if let Err(error) = commit_events(st, &events) {
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
        isolation_mode,
        created_ms: now,
        updated_ms: now,
    };
    let mut events = vec![Event::TaskCreated { task: task.clone() }];
    if let Some(assignment) = assignment_event {
        events.push(assignment);
    }
    if let Some(binding) = worktree_binding_for_task(server, &task) {
        events.push(Event::WorktreeBound { binding });
    }
    if let Err(error) = commit_events(st, &events) {
        return Resp::err(format!("TASK_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "task": task.id,
        "owner": task.owner,
        "status": task.status,
        "cleanup": if task.has_owned_resources() { "pending" } else { "not_required" },
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
    {
        let st = server.state.lock().unwrap();
        let Some(worker) = st.workers.get(&worker_id) else {
            return Resp::err(format!("worker {} not registered", worker_id));
        };
        if worker.token != token {
            return Resp::err("token mismatch: identity does not own this worker_id");
        }
        if st.tasks.get(&task_id).is_none() {
            return Resp::err(format!("task {} not found", task_id));
        }
    }
    let worktree_path = match declare_task_workspace(&server.root, &server.config, &worktree_path)
    {
        Ok(declared) => declared,
        Err(error) => return Resp::err(error),
    };
    let mut st = server.state.lock().unwrap();
    task_relocate_locked(
        server,
        &mut st,
        None,
        worker_id,
        token,
        task_id,
        worktree_path,
        branch,
        base_commit,
    )
}

fn task_relocate_locked(
    server: &Server,
    st: &mut State,
    control: Option<&binding_slot::BindingControlSet>,
    worker_id: String,
    token: String,
    task_id: String,
    worktree_path: String,
    branch: Option<String>,
    base_commit: Option<String>,
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
    if let Err(error) =
        task_owner_gate_maybe_with_control(server, st, control, &task_id, &worker_id)
    {
        return error;
    }
    let executor = match control {
        Some(control) => server_route_scope(server, st)
            .map_err(Resp::err)
            .and_then(|route| route.ok_or_else(|| Resp::err("WORK_EXECUTOR_PROJECT_REQUIRED")))
            .and_then(|route| {
                work_executor_gate_with_control(st, control, &route, &task_id, &worker_id)
            }),
        None => work_executor_gate(server, st, &task_id, &worker_id),
    };
    if let Err(error) = executor {
        return error;
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
    let branch = match resolve_task_branch(&worktree_path, branch.as_deref()) {
        Ok(resolved) => resolved,
        Err(error) => return Resp::err(error),
    };
    task.isolation_mode = Some(derive_isolation_mode(Some(&worktree_path)));
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
    let commit = match control {
        Some(control) => server
            .commit_locked_checked_with_control(st, &events, control)
            .map(|_| ())
            .map_err(|error| error.to_string()),
        None => server.commit_locked(st, &events),
    };
    if let Err(error) = commit {
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
        "next": "verify the declared task workspace and continue the existing claim; evidence remains attached"
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
    handle_task_accept_locked(server, &mut st, None, worker_id, token, task_id)
}

fn handle_task_accept_locked(
    server: &Server,
    st: &mut State,
    control: Option<&binding_slot::BindingControlSet>,
    worker_id: String,
    token: String,
    task_id: String,
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
    if task.status == "invited" {
        return Resp::err("BOARD_INVITATION_REVISION_REQUIRED: use collab task accept --expected-revision N or collab board respond --accept --expected-revision N");
    }
    if let Err(error) =
        task_owner_gate_maybe_with_control(server, st, control, &task_id, &worker_id)
    {
        return error;
    }
    let executor = match control {
        Some(control) => server_route_scope(server, st)
            .map_err(Resp::err)
            .and_then(|route| route.ok_or_else(|| Resp::err("WORK_EXECUTOR_PROJECT_REQUIRED")))
            .and_then(|route| {
                work_executor_gate_with_control(st, control, &route, &task_id, &worker_id)
            }),
        None => work_executor_gate(server, st, &task_id, &worker_id),
    };
    if let Err(error) = executor {
        return error;
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
    let execution_gate = match control {
        Some(control) => board_execution_gate_with_control(server, st, control, &worker_id, &task_id),
        None => board_execution_gate(server, st, &worker_id, &task_id),
    };
    if let Err(error) = execution_gate {
        return error;
    }
    task.status = "working".into();
    task.wait = None;
    task.updated_ms = now_ms();
    let events = [Event::TaskUpdated { task: task.clone() }];
    let commit = match control {
        Some(control) => server.commit_locked_checked_with_control(st, &events, control),
        None => server.commit_locked_checked(st, &events),
    };
    if let Err(error) = commit {
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
    handle_task_update_full(
        server,
        worker_id,
        token,
        task_id,
        status,
        next_step,
        None,
        None,
    )
}

/// True when the board projects this task's owner as empty because the owner
/// name or binding is no longer usable. This mirrors the `needs_reassignment`
/// projection so master repair authority matches exactly what the board shows.
fn task_owner_unavailable(server: &Server, state: &State, task: &TaskRec) -> bool {
    let route = match server_route_scope(server, state) {
        Ok(route) => route,
        Err(_) => return false,
    };
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
    {
        Ok(control) => control,
        Err(_) => return false,
    };
    assignment_unavailability(state, &control, task, route.as_ref())
        .ok()
        .flatten()
        .is_some()
}

fn handle_task_update_full(
    server: &Server,
    worker_id: String,
    token: String,
    task_id: String,
    status: Option<String>,
    next_step: Option<String>,
    blocker_action: Option<String>,
    decision_reason: Option<String>,
) -> Resp {
    let mut st = server.state.lock().unwrap();
    task_update_locked(
        server,
        &mut st,
        None,
        worker_id,
        token,
        task_id,
        status,
        next_step,
        blocker_action,
        decision_reason,
    )
}

fn task_update_locked(
    server: &Server,
    mut st: &mut State,
    control: Option<&binding_slot::BindingControlSet>,
    worker_id: String,
    token: String,
    task_id: String,
    status: Option<String>,
    next_step: Option<String>,
    blocker_action: Option<String>,
    decision_reason: Option<String>,
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
    if let Some(action) = blocker_action {
        if status.is_some() {
            return Resp::err(
                "blocker_action cannot be combined with a status update; decide the pending block separately",
            );
        }
        // The supervisor deciding a block request is not necessarily the task
        // owner, so this routes before the owner-only lifecycle guard.
        return task_block_decide_locked(
            server,
            st,
            control,
            worker_id,
            task_id,
            action,
            decision_reason,
        );
    }
    if task.owner != worker_id {
        // A task whose owner name/binding is unavailable projects an empty
        // owner and `needs_reassignment` on the board. The current project
        // master owns that repair path so the task cannot stay stuck without a
        // legal owner or wait state.
        let master_repair = task_owner_unavailable(server, st, &task)
            && board_check_master_locked(server, st, &worker_id).is_ok();
        if !master_repair {
            if let Err(error) =
                task_owner_gate_maybe_with_control(server, st, control, &task_id, &worker_id)
            {
                return error;
            }
        }
    } else if let Err(error) =
        task_owner_gate_maybe_with_control(server, st, control, &task_id, &worker_id)
    {
        return error;
    }
    let original_status = task.status.clone();
    if status.as_deref().is_some_and(|value|matches!(value,"working"|"verifying"|"reviewed")) {
        let result = match control {
            Some(control) => server_route_scope(server, st)
                .map_err(Resp::err)
                .and_then(|route| {
                    route.ok_or_else(|| Resp::err("WORK_EXECUTOR_PROJECT_REQUIRED"))
                })
                .and_then(|route| {
                    work_executor_gate_with_control(st, control, &route, &task_id, &worker_id)
                }),
            None => work_executor_gate(server, st, &task_id, &worker_id),
        };
        if let Err(error) = result {
            return error;
        }
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
            && !st.task_lifecycle.contains_key(&task.id)
            && required_work_reviewer(st, &task.id).is_none();
        if new_status == "accepted" || (new_status == "merged" && !legacy_accepted_merge) {
            return Resp::err(
                "use collab task review/integrated for integration-owned lifecycle transitions",
            );
        }
        if new_status == "waiting" {
            return Resp::err("use collab task wait so responsibility and deadline are durable");
        }
        if new_status == "cancelled" && task.has_owned_resources() {
            return Resp::err(
                "CLEANUP_REQUIRED_BEFORE_CANCEL: task owns a worktree or branch; close only after merged cleanup",
            );
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
        if new_status == "blocked" {
            // A member business block is an application, never an approved
            // blocker. Keep the task status and record one typed pending
            // request for the authorized supervisor or CEO to decide.
            return task_block_apply_locked(
                server,
                st,
                control,
                &task,
                &worker_id,
                next_step.clone(),
            );
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
    if original_status != task.status
        && matches!(task.status.as_str(), "working" | "verifying" | "rework")
    {
        if let Some(event) = ticket_rework_phase_event(server, &st, &task_id) {
            events.push(event);
        }
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
    let commit = match control {
        Some(control) => server.commit_locked_checked_with_control(&mut st, &events, control),
        None => server.commit_locked_checked(&mut st, &events),
    };
    if let Err(error) = commit {
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

/// Record a member's business block application without changing the task
/// status. A repeat application from the same owner refreshes the one pending
/// request instead of stacking a second one.
fn task_block_apply_locked(
    server: &Server,
    st: &mut State,
    control: Option<&binding_slot::BindingControlSet>,
    task: &TaskRec,
    requester: &str,
    reason: Option<String>,
) -> Resp {
    let revision = st
        .board_details
        .get(&task.id)
        .map(|details| details.revision)
        .unwrap_or(0);
    let now = now_ms();
    let request = match st.task_block_requests.get(&task.id).cloned() {
        Some(existing)
            if existing.state == TaskBlockRequestState::Pending
                && existing.requester == requester =>
        {
            let mut existing = existing;
            existing.reason = reason;
            existing.task_revision = revision;
            existing
        }
        _ => TaskBlockRequest {
            task_id: task.id.clone(),
            requester: requester.to_owned(),
            reason,
            requested_at_ms: now,
            task_revision: revision,
            state: TaskBlockRequestState::Pending,
            decided_by: None,
            decided_at_ms: None,
            decision_reason: None,
        },
    };
    let events = [Event::TaskBlockRequestUpdated {
        request: request.clone(),
    }];
    let commit = match control {
        Some(control) => server.commit_locked_checked_with_control(st, &events, control),
        None => server.commit_locked_checked(st, &events),
    };
    if let Err(error) = commit {
        return Resp::err(format!("TASK_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "task": task.id,
        "status": task.status,
        "owner": task.owner,
        "block_request": request,
        "notification": "none",
        "next_action": "business block pending supervisor decision",
    }))
}

/// Decide one pending business block application under the caller-held daemon
/// state lock. Authorization, staleness and atomic state commit all happen
/// here. A decision never grants authority beyond this one request, and the
/// requester cannot decide its own application.
fn task_block_decide_locked(
    server: &Server,
    st: &mut State,
    control: Option<&binding_slot::BindingControlSet>,
    worker_id: String,
    task_id: String,
    decision: String,
    reason: Option<String>,
) -> Resp {
    if !matches!(decision.as_str(), "approve" | "reject") {
        return Resp::err("invalid decision; expected approve or reject");
    }
    let Some(task) = st.tasks.get(&task_id).cloned() else {
        return Resp::err(format!("task {task_id} not found"));
    };
    let Some(mut request) = st.task_block_requests.get(&task_id).cloned() else {
        return Resp::err_data(
            "TASK_BLOCK_REQUEST_NOT_FOUND",
            json!({"task_id": task_id}),
        );
    };
    if request.requester == worker_id {
        return Resp::err_data(
            "TASK_BLOCK_SELF_DECISION_DENIED",
            json!({"task_id": task_id, "requester": request.requester, "decider": worker_id}),
        );
    }
    let route_scope = server_route_scope(server, &st).ok().flatten();
    let master = current_master_worker_id(&st, route_scope.as_ref());
    let requester_is_master = master.as_deref() == Some(request.requester.as_str());
    let authorized = if requester_is_master {
        // A master-owned block application goes to the current CEO; without a
        // current CEO it stays pending instead of granting self-approval.
        is_current_ceo_holder(&st, &worker_id)
    } else {
        master.as_deref() == Some(worker_id.as_str())
    };
    if !authorized {
        return Resp::err_data(
            "TASK_BLOCK_DECISION_UNAUTHORIZED",
            json!({
                "task_id": task_id,
                "decider": worker_id,
                "requester": request.requester,
                "requester_is_master": requester_is_master,
            }),
        );
    }
    let requested_state = if decision == "approve" {
        TaskBlockRequestState::Approved
    } else {
        TaskBlockRequestState::Rejected
    };
    if request.state != TaskBlockRequestState::Pending {
        if request.state == requested_state
            && request.decided_by.as_deref() == Some(worker_id.as_str())
        {
            return Resp::data(json!({
                "task": task.id,
                "status": task.status,
                "owner": task.owner,
                "block_request": request,
                "notification": "none",
                "idempotent": true,
            }));
        }
        return Resp::err_data(
            "TASK_BLOCK_ALREADY_DECIDED",
            json!({"task_id": task_id, "state": request.state}),
        );
    }
    let current_revision = st
        .board_details
        .get(&task_id)
        .map(|details| details.revision)
        .unwrap_or(0);
    if current_revision != request.task_revision {
        return Resp::err_data(
            "BOARD_STALE_REVISION",
            json!({
                "task_id": task_id,
                "expected_revision": request.task_revision,
                "current_revision": current_revision,
            }),
        );
    }
    let now = now_ms();
    request.state = requested_state;
    request.decided_by = Some(worker_id.clone());
    request.decided_at_ms = Some(now);
    request.decision_reason = reason;
    let mut updated_task = task.clone();
    let mut events = vec![Event::TaskBlockRequestUpdated {
        request: request.clone(),
    }];
    if decision == "approve" {
        updated_task.status = "blocked".into();
        updated_task.wait = None;
        updated_task.updated_ms = now;
        events.push(Event::TaskUpdated {
            task: updated_task.clone(),
        });
    }
    let commit = match control {
        Some(control) => server.commit_locked_checked_with_control(st, &events, control),
        None => server.commit_locked_checked(st, &events),
    };
    if let Err(error) = commit {
        return Resp::err(format!("TASK_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "task": updated_task.id,
        "status": updated_task.status,
        "owner": updated_task.owner,
        "block_request": request,
        "notification": "none",
        "next_action": if decision == "approve" {
            "task is blocked; resume only after the blocker is resolved"
        } else {
            "block request rejected; continue work or send a new application"
        },
    }))
}

/// Whether the caller is the worker currently holding the durable CEO grant.
/// Mirrors the holder resolution the CEO owner uses, without inventing a
/// second authority source.
fn is_current_ceo_holder(st: &State, worker_id: &str) -> bool {
    st.ceo_grant.as_ref().is_some_and(|grant| {
        st.name_occupancy(&grant.workspace, &grant.name)
            .is_some_and(|occupancy| occupancy.worker_id == worker_id)
    })
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
                "action": "peer owns cleanup; inspect the current task and cleanup evidence"
            })
        })
        .collect()
}

fn close_task_resources(
    root: &Path,
    config: &crate::config::Config,
    worktree_path: Option<&str>,
    branch: Option<&str>,
    recorded_integration: Option<&str>,
) -> Result<(), String> {
    if worktree_path.is_some_and(is_layerfs_workspace) {
        let worktree_path = worktree_path.unwrap_or_default();
        return close_layerfs_task_workspace(worktree_path);
    }
    if let Some(branch) = branch {
        let branch_ref = format!("refs/heads/{branch}");
        if branch_exists(root, &branch_ref)? {
            let main_ref = "refs/heads/main";
            let merged = run_safe_git(root, &["merge-base", "--is-ancestor", &branch_ref, main_ref])
                .map_err(|e| format!("cannot verify branch {branch}: {e}"))?;
            let merged = match merged.status.code() {
                Some(0) => true,
                Some(1) => false,
                _ => {
                    return Err(format!(
                        "cannot verify whether branch {branch} is an ancestor of {main_ref}: {}",
                        String::from_utf8_lossy(&merged.stderr).trim()
                    ));
                }
            };
            let proven = if merged {
                true
            } else {
                branch_net_content_present_at_integration_commit(
                    root,
                    main_ref,
                    &branch_ref,
                    recorded_integration,
                )?
            };
            if !proven {
                return Err(format!(
                    "branch {branch} is not merged into {main_ref}; refusing delete"
                ));
            }
        }
    }
    if let Some(relative) = worktree_path {
        let worktree = cleanup_worktree_path(root, config, relative)?;
        if worktree.exists() {
            let dirty = run_safe_git(&worktree, &["status", "--porcelain"])
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

            let removed = run_safe_git(
                root,
                &["worktree", "remove", &worktree.display().to_string()],
            )
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
        if branch_exists(root, &branch_ref)? {
            let deleted = run_safe_git(root, &["branch", "-D", branch])
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

fn branch_exists(root: &Path, branch_ref: &str) -> Result<bool, String> {
    let format = run_safe_git(root, &["check-ref-format", branch_ref])
        .map_err(|e| format!("cannot verify branch {branch_ref}: {e}"))?;
    match format.status.code() {
        Some(0) => {}
        Some(1) => {
            return Err(format!(
                "branch {branch_ref} is not a valid ref name; refusing delete"
            ));
        }
        _ => {
            return Err(format!(
                "cannot verify branch {branch_ref}: {}",
                String::from_utf8_lossy(&format.stderr).trim()
            ));
        }
    }

    let exists = run_safe_git(root, &["show-ref", "--verify", "--quiet", branch_ref])
        .map_err(|e| format!("cannot verify branch {branch_ref}: {e}"))?;
    match exists.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "cannot verify branch {branch_ref}: {}",
            String::from_utf8_lossy(&exists.stderr).trim()
        )),
    }
}

/// Release a LayerFS task workspace obligation. Collab owns only the isolation
/// binding: it verifies the declared LayerFS task branch still exists through
/// the generic LayerFS entry and never runs `git worktree remove`, `git branch
/// -D`, `git status`, or any other local-Git operation against the Workspace.
/// Branch sync, publish and Git commit capture remain the LayerFS Skill's
/// contract. A missing entry, missing profile or unreachable Store keeps the
/// original error and leaves the obligation open instead of faking success.
fn close_layerfs_task_workspace(worktree_path: &str) -> Result<(), String> {
    let workspace = parse_layerfs_workspace(worktree_path)?;
    if !layerfs_branch_exists(&workspace.project, &workspace.branch)? {
        return Err(format!(
            "layerfs task branch {} no longer exists in project {}; preserve the site and reconcile through the LayerFS Skill",
            workspace.branch, workspace.project
        ));
    }
    Ok(())
}

/// A squash integration replaces the branch tip with a new commit, so the
/// branch is not an ancestor of main even though its work landed. The recorded
/// `integration.commit` is the owner's integration declaration. For cleanup,
/// require that commit to be reachable from `refs/heads/main` and that the
/// branch's net changes relative to its unique common ancestor are present
/// with identical Git tree content in that commit. This proves net content,
/// not integration causality or exact candidate identity.
fn branch_net_content_present_at_integration_commit(
    root: &Path,
    main_ref: &str,
    branch_ref: &str,
    recorded_integration: Option<&str>,
) -> Result<bool, String> {
    let Some(commit) = recorded_integration
        .map(str::trim)
        .filter(|commit| !commit.is_empty())
    else {
        return Ok(false);
    };
    let main_head = resolve_authoritative_main_head(root)
        .map_err(|error| {
            format!(
                "cannot resolve {main_ref} while checking integration commit {commit}: {}",
                error.error.unwrap_or_default()
            )
        })?;
    match commit_is_integrated_in_main(root, commit) {
        Ok(true) => {}
        Ok(false) => return Err(format!(
            "integration commit {commit} is not reachable from {main_ref} (main_head {main_head}); expected a commit reachable from {main_ref}; refusing delete"
        )),
        Err(error) => return Err(format!(
            "integration commit {commit} cannot be resolved (main_head {main_head}): {}; expected a commit reachable from {main_ref}; refusing delete",
            error.error.unwrap_or_default()
        )),
    }

    let merge_bases = run_safe_git(root, &["merge-base", "--all", branch_ref, commit])
        .map_err(|e| format!("cannot find the common ancestor for {branch_ref} and {commit}: {e}"))?;
    if !merge_bases.status.success() {
        return Err(format!(
            "cannot find the common ancestor for {branch_ref} and {commit}: {}",
            String::from_utf8_lossy(&merge_bases.stderr).trim()
        ));
    }
    let merge_bases = String::from_utf8(merge_bases.stdout)
        .map_err(|error| format!("common ancestor output is not valid UTF-8: {error}"))?;
    let merge_bases = merge_bases
        .lines()
        .map(str::trim)
        .filter(|base| !base.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if merge_bases.len() != 1 {
        return Err(format!(
            "expected one common ancestor for {branch_ref} and {commit}, found {}; refusing delete",
            merge_bases.len()
        ));
    }
    let base = &merge_bases[0];

    let changed = run_safe_git_os(
        root,
        [
            std::ffi::OsStr::new("diff"),
            std::ffi::OsStr::new("--no-ext-diff"),
            std::ffi::OsStr::new("--no-textconv"),
            std::ffi::OsStr::new("--no-renames"),
            std::ffi::OsStr::new("--ignore-submodules=none"),
            std::ffi::OsStr::new("--name-only"),
            std::ffi::OsStr::new("-z"),
            std::ffi::OsStr::new(base),
            std::ffi::OsStr::new(branch_ref),
            std::ffi::OsStr::new("--"),
        ],
    )
    .map_err(|e| format!("cannot list net changed paths for {branch_ref}: {e}"))?;
    if !changed.status.success() {
        return Err(format!(
            "cannot list net changed paths for {branch_ref}: {}",
            String::from_utf8_lossy(&changed.stderr).trim()
        ));
    }
    let paths = parse_nul_os_strings(&changed.stdout, "net changed path")?;
    if paths.is_empty() {
        return Err(format!(
            "branch {branch_ref} has no net changed paths to compare; refusing delete"
        ));
    }

    for path in paths {
        let output = run_safe_git_os(
            root,
            [
                std::ffi::OsStr::new("--literal-pathspecs"),
                std::ffi::OsStr::new("diff"),
                std::ffi::OsStr::new("--no-ext-diff"),
                std::ffi::OsStr::new("--no-textconv"),
                std::ffi::OsStr::new("--no-renames"),
                std::ffi::OsStr::new("--ignore-submodules=none"),
                std::ffi::OsStr::new("--quiet"),
                std::ffi::OsStr::new(branch_ref),
                std::ffi::OsStr::new(commit),
                std::ffi::OsStr::new("--"),
                path.as_os_str(),
            ],
        )
        .map_err(|e| {
            format!(
                "cannot compare net changed path {} for {branch_ref}: {e}",
                path.to_string_lossy()
            )
        })?;
        match output.status.code() {
            Some(0) => {}
            Some(1) => {
                return Err(format!(
                    "net changed path {} differs between {branch_ref} and integration commit {commit}; refusing delete",
                    path.to_string_lossy()
                ));
            }
            _ => {
                return Err(format!(
                    "cannot compare net changed path {} for {branch_ref}: {}",
                    path.to_string_lossy(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
        }
    }

    Ok(true)
}

fn run_safe_git_os<I, S>(root: &Path, args: I) -> std::io::Result<std::process::Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    safe_git_command(root)?.args(args).output()
}

fn parse_nul_os_strings(bytes: &[u8], label: &str) -> Result<Vec<std::ffi::OsString>, String> {
    if !bytes.is_empty() && !bytes.ends_with(&[0]) {
        return Err(format!("{label} output is not NUL-terminated"));
    }
    let mut values = Vec::new();
    for value in bytes.split(|byte| *byte == 0) {
        if value.is_empty() {
            continue;
        }
        #[cfg(unix)]
        let value = {
            use std::os::unix::ffi::OsStrExt;
            std::ffi::OsStr::from_bytes(value).to_os_string()
        };
        #[cfg(not(unix))]
        let value = std::ffi::OsString::from(
            std::str::from_utf8(value)
                .map_err(|error| format!("{label} is not valid UTF-8: {error}"))?,
        );
        values.push(value);
    }
    Ok(values)
}

fn cleanup_receipt_is_reusable(root: &Path, receipt: &CleanupReceipt) -> bool {
    // A LayerFS binding has no local path or Git branch to re-check; always
    // re-verify through the LayerFS entry so a stale receipt cannot skip the
    // branch check with a locally coincidental Git branch name.
    if receipt
        .worktree_path
        .as_deref()
        .is_some_and(is_layerfs_workspace)
    {
        return false;
    }
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
        let branch_exists = run_safe_git(root, &["rev-parse", "--verify", branch])
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
    handle_task_deliver_with_intent(
        server,
        worker_id,
        token,
        task_id,
        evidence,
        None,
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
    attempt: Option<u64>,
    worktree: Option<String>,
    no_candidate: bool,
) -> Resp {
    let mut pending_notice = None;
    let response = {
        let mut st = server.state.lock().unwrap();
        handle_task_deliver_with_intent_locked(
            server,
            &mut st,
            None,
            worker_id,
            token,
            task_id,
            evidence,
            attempt,
            worktree,
            no_candidate,
            &mut pending_notice,
        )
    };
    finish_task_deliver_notification(server, response, pending_notice)
}

struct PendingTaskReviewNotice {
    message_id: String,
    subscription_id: Option<String>,
    method: &'static str,
    now: i64,
}

#[allow(clippy::too_many_arguments)]
fn handle_task_deliver_with_intent_locked(
    server: &Server,
    st: &mut State,
    control: Option<&binding_slot::BindingControlSet>,
    worker_id: String,
    token: String,
    task_id: String,
    evidence: Option<String>,
    attempt: Option<u64>,
    worktree: Option<String>,
    no_candidate: bool,
    pending_notice: &mut Option<PendingTaskReviewNotice>,
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
    if let Err(error) =
        task_owner_gate_maybe_with_control(server, st, control, &task_id, &worker_id)
    {
        return error;
    }
    if !task_delivery_allowed(&task.status) {
        return Resp::err(format!(
            "task {} must be owned and working, verifying, reviewed, or rework before delivery (current: {})",
            task_id, task.status
        ));
    }
    // A pending business block application is an unresolved claim on this
    // task. Delivery must not silently erase it; the supervisor has to decide
    // the exact request first.
    if st
        .task_block_requests
        .get(&task_id)
        .is_some_and(|request| request.state == TaskBlockRequestState::Pending)
    {
        return Resp::err_data(
            "TASK_BLOCK_PENDING_APPROVAL",
            json!({"task_id": task_id}),
        );
    }
    let executor = match control {
        Some(control) => server_route_scope(server, st)
            .map_err(Resp::err)
            .and_then(|route| route.ok_or_else(|| Resp::err("WORK_EXECUTOR_PROJECT_REQUIRED")))
            .and_then(|route| {
                work_executor_gate_with_control(st, control, &route, &task_id, &worker_id)
            }),
        None => work_executor_gate(server, st, &task_id, &worker_id),
    };
    if let Err(error) = executor {
        return error;
    }

    let ticket_result_event = match board_ticket_result_event(
        server,
        &st,
        &worker_id,
        &task_id,
        evidence.as_deref().unwrap_or(""),
        attempt,
    ) {
        Ok(event) => event,
        Err(error) => return error,
    };

    let Some(evidence) = evidence.filter(|value| !value.trim().is_empty()) else {
        return Resp::err("task deliver requires non-empty --evidence");
    };
    let worktree = if no_candidate {
        if worktree.is_some() {
            return Resp::err(
                "task deliver --no-candidate cannot be combined with --worktree",
            );
        }
        None
    } else {
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
        Some(worktree)
    };
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
    if let Some(ticket_event) = ticket_result_event.clone() {
        events.push(ticket_event);
    }
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
    let delivery_kind = lifecycle.delivery_kind;
    let mut review_notice = None;
    let mut reviewer_waiting = None;
    if let Some(name) = required_reviewer.as_deref() {
        task.next_step = Some(format!("await review by {name}; use collab task review --accept or --rework"));
        let route = match server_route_scope(server, &st) {
            Ok(Some(route)) => route,
            _ => return Resp::err("required reviewer project route is not ready"),
        };
        let loaded;
        let control = match control {
            Some(control) => control,
            None => {
                loaded = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
                    Ok(control) => control,
                    Err(error) => return Resp::err(error),
                };
                &loaded
            }
        };
        let recipient = match control.current_for_name(&route.project_scope_id, &route.app_scope_id, name) {
            Ok(slot) => slot.and_then(|slot| slot.active_binding()).map(|current| current.binding.agent_id.as_str().to_owned()),
            Err(error) => return Resp::err(error),
        };
        if let Some(recipient) = recipient.filter(|id| st.workers.contains_key(id))
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
    // A completed execute phase releases the slot. Run the same continuation
    // entry so the next queued item for that slot is claimed in this batch.
    if let Some(Event::TicketPhaseRecorded { phase: completed }) = ticket_result_event.as_ref() {
        if let Ok(Some(route)) = server_route_scope(server, &st) {
            let loaded;
            let control = match control {
                Some(control) => Some(control),
                None => match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
                    Ok(control) => {
                        loaded = control;
                        Some(&loaded)
                    }
                    Err(_) => None,
                },
            };
            if let Some(control) = control {
                events.extend(ticket_continuation_events(
                    &st,
                    control,
                    &route,
                    &worker_id,
                    std::slice::from_ref(completed),
                    false,
                ));
                events.extend(ticket_dependency_release_events(
                    &st,
                    control,
                    &route,
                    completed,
                    &worker_id,
                ));
            }
        }
    }
    events.extend([
            Event::TaskUpdated { task: task.clone() },
            Event::TaskLifecycleUpdated {
                task_id: task.id.clone(),
                record: lifecycle,
            },
        ]);
    let commit = match control {
        Some(control) => server
            .commit_locked_checked_with_control(st, &events, control)
            .map(|_| ())
            .map_err(|error| error.to_string()),
        None => server.commit_locked(st, &events),
    };
    if let Err(error) = commit {
        return Resp::err(format!("TASK_DELIVER_DURABILITY_FAILED: {error}"));
    }

    let data = json!({
        "delivered": task.id,
        "status": task.status,
        "evidence": evidence,
        "worktree": worktree,
        "delivery_kind": delivery_kind.as_str(),
        "notification": "none",
        "required_reviewer": required_reviewer,
        "review_waiting": reviewer_waiting,
        "delivery_complete": false,
        "next_action": task.next_step,
        "identity": {"worker_id": worker.id, "kind": "peer"},
    });
    if let Some((message_id, subscription, method)) = review_notice {
        let mut data = data;
        data["review_message_id"] = json!(message_id);
        *pending_notice = Some(PendingTaskReviewNotice {
            message_id,
            subscription_id: subscription.map(|subscription| subscription.id),
            method,
            now,
        });
        Resp::data(data)
    } else {
        Resp::data(data)
    }
}

fn finish_task_deliver_notification(
    server: &Server,
    response: Resp,
    pending_notice: Option<PendingTaskReviewNotice>,
) -> Resp {
    let Some(notice) = pending_notice else {
        return response;
    };
    let attempt = match notice.subscription_id.as_deref() {
        Some(subscription_id) => {
            attempt_notification_detailed_with_at(server, &notice.message_id, subscription_id, notice.now)
        }
        None => {
            NotificationAttempt::NotAttempted(NOTIFICATION_SUBSCRIPTION_MISSING_ERROR.into())
        }
    };
    notification_send_response(
        response.data,
        notice.subscription_id.is_some(),
        notice.method,
        &attempt,
    )
}

fn task_integration_authorized(
    server: &Server,
    state: &State,
    task: &TaskRec,
    worker_id: &str,
) -> bool {
    task_integration_authorized_with_control(server, state, None, task, worker_id)
}

fn task_integration_authorized_with_control(
    server: &Server,
    state: &State,
    control: Option<&binding_slot::BindingControlSet>,
    task: &TaskRec,
    worker_id: &str,
) -> bool {
    task_owner_gate_maybe_with_control(server, state, control, &task.id, worker_id).is_ok()
        || current_master_holder(server, state)
            .ok()
            .flatten()
            .as_deref()
            == Some(worker_id)
}
