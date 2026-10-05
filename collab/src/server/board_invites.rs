fn board_unstarted_resources(state: &State, task: &TaskRec) -> Result<(), Resp> {
    if task.worktree_path.is_some() || task.branch.is_some() || task.base_commit.is_some()
        || state.worktree_bindings.values().any(|binding| binding.task_id == task.id)
        || state.task_lifecycle.contains_key(&task.id) || state.cleanup_receipts.contains_key(&task.id)
        || state.pending_merges.contains_key(&task.id) || task.wait.is_some()
    {
        return Err(Resp::err("BOARD_RESOURCE_OWNER_CONFLICT: unaccepted tasks must have no execution resources or evidence"));
    }
    Ok(())
}

fn board_invite(server: &Server, actor: String, token: String, id: String, to: String, expected_revision: u64) -> Resp {
    let (peer, expected_binding) = {
        let state = server.state.lock().unwrap();
        if let Err(error) = board_check_actor(&state, &actor, &token)
            .and_then(|()| board_check_master_locked(server, &state, &actor)) { return error; }
        if to == actor || is_managed_subagent(&state, &to) { return Resp::err("BOARD_ORDINARY_PEER_REQUIRED: invite a different ordinary peer"); }
        if let Err(error) = board_check_revision(&state, &id, expected_revision) { return error; }
        let Some(peer) = state.workers.get(&to).cloned() else { return Resp::err("peer not registered"); };
        let route = match server_route_scope(server, &state) {
            Ok(Some(route)) => route,
            Ok(None) => return Resp::err("project route is not ready"),
            Err(error) => return Resp::err(error),
        };
        let binding = match authoritative_send_binding(&state, &route, &to) {
            Ok(binding) => binding.clone(), Err(error) => return Resp::err(error),
        };
        (peer, binding)
    };
    if worker_presence(server, &peer) != IdentityPresence::Present { return Resp::err("BOARD_PEER_NOT_LIVE: invite a live peer"); }
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, &actor, &token)
        .and_then(|()| board_check_master_locked(server, &state, &actor)) { return error; }
    let mut details = match board_check_revision(&state, &id, expected_revision) { Ok(details) => details, Err(error) => return error };
    let Some(mut task) = state.tasks.get(&id).cloned() else { return Resp::err("task not found"); };
    if task.status != "pending" { return Resp::err("only a pending task may be invited"); }
    if task.owner != task.created_by { return Resp::err("pending task owner must be its publisher"); }
    if let Err(error) = board_unstarted_resources(&state, &task) { return error; }
    let Some(current_peer) = state.workers.get(&to) else { return Resp::err("peer is no longer registered"); };
    if current_peer.token != peer.token || current_peer.registered_ms != peer.registered_ms || current_peer.cwd != peer.cwd {
        return Resp::err("BOARD_STALE_PEER: recipient identity changed during admission");
    }
    let route = match server_route_scope(server, &state) { Ok(Some(route)) => route, _ => return Resp::err("project route is not ready") };
    let binding = match authoritative_send_binding(&state, &route, &to) { Ok(binding) => binding, Err(error) => return Resp::err(error) };
    if binding.binding_id != expected_binding.binding_id || binding.endpoint_generation != expected_binding.endpoint_generation {
        return Resp::err("BOARD_STALE_PEER: recipient runtime binding changed during admission");
    }
    if board_peer_has_responsibility(&state, &to, None) { return Resp::err("BOARD_PEER_BUSY: peer owns an unfinished task or another invitation"); }
    let now = now_ms();
    let message_id = format!("board-invite-{}-{}", id, details.revision + 1);
    details.invitation = Some(crate::board::BoardInvitation {
        peer_id: to.clone(), binding_id: binding.binding_id.as_str().into(),
        endpoint_generation: binding.endpoint_generation, message_id: message_id.clone(), created_ms: now,
    });
    details.last_response = None;
    details.revision += 1;
    task.status = "invited".into(); task.updated_ms = now;
    let message = Message {
        id: message_id.clone(), from: actor, to: to.clone(), mtype: "request".into(),
        subject: Some(format!("task-invitation:{id}")),
        body: format!("{}\n{}\nDelivery condition: {}\nTest condition: {}\nRead the durable message with collab recv, then observe your invitation with collab board show. Respond using collab board respond <task-id> --accept|--decline --expected-revision <observed-revision>, taking the task ID and revision from the typed task view, not this message. You may decline; an invitation alone does not transfer ownership.", details.title, details.description, details.delivery_condition, details.test_condition),
        in_reply_to: None, created_ms: now, state: "pending".into(),
        wake_attempt_count: 0, last_wake_attempt_ms: 0, retry_attempted: false,
    };
    let subscription = state.matching_subscription(&to, "direct-message", None, now).cloned();
    let method = notification_method_for_worker(&state, &to);
    let mut events = vec![
        Event::TaskUpdated { task },
        Event::BoardDetailsChanged { task_id: id.clone(), details },
        Event::Sent { msg: message },
        Event::DeliveryMode { msg_id: message_id.clone(), mode: "explicit-notification".into(), source_thread_id: None },
    ];
    if let Some(subscription) = &subscription { events.push(Event::WakeBound { message_id: message_id.clone(), subscription_id: subscription.id.clone() }); }
    if let Err(error) = server.commit_locked_checked(&mut state, &events) { return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}")); }
    let data = json!({ "task": state.tasks.get(&id).and_then(|task| board_task_view(&state, task)), "msg_id": message_id, "consumed": false });
    drop(state);
    let attempt = match &subscription {
        Some(subscription) => attempt_notification_detailed_with_at(server, &message_id, &subscription.id, now_ms()),
        None => NotificationAttempt::NotAttempted(NOTIFICATION_SUBSCRIPTION_MISSING_ERROR.into()),
    };
    notification_send_response(data, subscription.is_some(), method, &attempt)
}

fn board_respond(server: &Server, actor: String, token: String, id: String, expected_revision: u64, accept: bool, reason: Option<String>) -> Resp {
    if !accept && reason.as_deref().is_none_or(|reason| reason.trim().is_empty()) { return Resp::err("decline requires a nonempty --reason"); }
    let peer = {
        let state = server.state.lock().unwrap();
        if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
        state.workers.get(&actor).cloned().expect("verified actor")
    };
    if accept && worker_presence(server, &peer) != IdentityPresence::Present { return Resp::err("BOARD_PEER_NOT_LIVE: accepting peer must be live"); }
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
    let mut details = match board_check_revision(&state, &id, expected_revision) { Ok(details) => details, Err(error) => return error };
    let Some(invite) = details.invitation.clone() else { return Resp::err("task has no pending invitation"); };
    if invite.peer_id != actor { return Resp::err("only the invited peer may respond"); }
    let Some(mut task) = state.tasks.get(&id).cloned() else { return Resp::err("task not found"); };
    if task.status != "invited" || task.owner != task.created_by { return Resp::err("task invitation is not awaiting a response"); }
    if let Err(error) = board_unstarted_resources(&state, &task) { return error; }
    if accept {
        let binding = match board_execution_gate(server, &state, &actor, &id) { Ok(binding) => binding, Err(error) => return error };
        if binding.binding_id.as_str() != invite.binding_id || binding.endpoint_generation != invite.endpoint_generation {
            return Resp::err("BOARD_STALE_INVITATION: recipient rebound; master must withdraw and issue a new invitation");
        }
        task.owner = actor.clone(); task.status = "working".into();
        details.last_response = Some(format!("accepted by {actor}"));
    } else {
        task.status = "pending".into();
        details.last_response = Some(format!("declined by {actor}: {}", reason.unwrap()));
    }
    task.updated_ms = now_ms(); details.revision += 1; details.invitation = None;
    let mut events = vec![Event::TaskUpdated { task }, Event::BoardDetailsChanged { task_id: id.clone(), details }];
    if !accept { events.push(Event::Superseded { ids: vec![invite.message_id] }); }
    if let Err(error) = server.commit_locked_checked(&mut state, &events) { return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}")); }
    board_task_response(&state, &id)
}

fn board_withdraw(server: &Server, actor: String, token: String, id: String, expected_revision: u64, reason: String) -> Resp {
    if reason.trim().is_empty() { return Resp::err("withdraw requires a nonempty reason"); }
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, &actor, &token)
        .and_then(|()| board_check_master_locked(server, &state, &actor)) { return error; }
    let mut details = match board_check_revision(&state, &id, expected_revision) { Ok(details) => details, Err(error) => return error };
    let Some(invite) = details.invitation.take() else { return Resp::err("task has no pending invitation"); };
    let Some(mut task) = state.tasks.get(&id).cloned() else { return Resp::err("task not found"); };
    if task.status != "invited" { return Resp::err("only an unaccepted invitation may be withdrawn"); }
    task.status = "pending".into(); task.updated_ms = now_ms();
    details.revision += 1; details.last_response = Some(format!("withdrawn by {actor}: {reason}"));
    if let Err(error) = server.commit_locked_checked(&mut state, &[
        Event::TaskUpdated { task }, Event::BoardDetailsChanged { task_id: id.clone(), details },
        Event::Superseded { ids: vec![invite.message_id] },
    ]) { return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}")); }
    board_task_response(&state, &id)
}
