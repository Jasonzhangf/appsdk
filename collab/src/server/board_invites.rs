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
    board_invite_with_selection(server,actor,token,id,to,expected_revision,None)
}

fn board_invite_with_selection(server: &Server, actor: String, token: String, id: String, to: String,
    expected_revision: u64,selection:Option<(String,String)>) -> Resp {
    // Resolve the name, binding and task revision under the same reducer lock.
    // Notification delivery happens after the durable invitation is committed.
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, &actor, &token)
        .and_then(|()| board_check_master_locked(server, &state, &actor)) { return error; }
    let mut details = match board_check_revision(&state, &id, expected_revision) { Ok(details) => details, Err(error) => return error };
    if !details.public_visibility { return Resp::err("BOARD_TASK_NOT_PUBLIC: only public work may be invited"); }
    let Some(mut task) = state.tasks.get(&id).cloned() else { return Resp::err("task not found"); };
    if matches!(task.status.as_str(), "closed" | "cancelled") { return Resp::err("BOARD_TASK_CLOSED: closed tasks cannot be reassigned"); }
    if details.invitation.is_some() || task.status == "invited" { return Resp::err("task already has a pending invitation"); }
    let first_invitation = task.status == "pending" && task.owner == task.created_by;
    if first_invitation {
        if let Err(error) = board_unstarted_resources(&state, &task) { return error; }
    }
    let route = match server_route_scope(server, &state) { Ok(Some(route)) => route, _ => return Resp::err("project route is not ready") };
    let to = state.name_occupancy(route.project_scope_id.as_str(), &to)
        .map(|occupancy| occupancy.worker_id.clone()).unwrap_or(to);
    if to == actor || is_managed_subagent(&state, &to) { return Resp::err("BOARD_ORDINARY_PEER_REQUIRED: invite a different ordinary peer"); }
    if !state.workers.contains_key(&to) { return Resp::err("peer not registered"); }
    if let Err(error)=work_executor_gate(server,&state,&id,&to) {return error;}
    let binding = match authoritative_send_binding(&state, &route, &to) { Ok(binding) => binding, Err(error) => return Resp::err(error) };
    let now = now_ms();
    let message_id = format!("board-invite-{}-{}", id, details.revision + 1);
    details.invitation = Some(crate::board::BoardInvitation {
        peer_id: to.clone(), binding_id: binding.binding_id.as_str().into(),
        endpoint_generation: binding.endpoint_generation, message_id: message_id.clone(), created_ms: now,
        original_status: Some(task.status.clone()), original_owner: Some(task.owner.clone()),
    });
    details.last_response = None;
    if let Some((name,reason))=selection {
        details.dispatch_selection=Some(name);details.dispatch_reason=Some(reason);
    }
    details.revision += 1;
    if first_invitation { task.status = "invited".into(); }
    task.updated_ms = now;
    let recipient_brief = role_brief(server, &state, &to);
    let recipient_responsibilities = recipient_brief["responsibilities"]
        .as_array().into_iter().flatten().filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>().join("\n- ");
    let reminder = format!("Role task: {}\nResponsibilities:\n- {}",
        recipient_brief["role_task"].as_str().unwrap_or_default(), recipient_responsibilities);
    let message = Message {
        id: message_id.clone(), from: actor.clone(), to: to.clone(), mtype: "request".into(),
        subject: Some(format!("task-invitation:{id}")),
        body: format!("{}\n{}\nDelivery condition: {}\nTest condition: {}\n{}\nContinue this subgoal from status {} with its existing evidence and resources. Own its decomposition, delegation, resource management, acceptance and delivery.\nRead the durable message with collab recv, then observe your invitation with collab board show. Respond using collab board respond <task-id> --accept|--decline --expected-revision <observed-revision>, taking the task ID and revision from the typed task view, not this message. You may decline; an invitation alone does not transfer ownership.", details.title, details.description, details.delivery_condition, details.test_condition, reminder, details.invitation.as_ref().unwrap().original_status.as_deref().unwrap()),
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
    let master = current_master_worker_id(&state, Some(&route));
    let data = json!({ "task": state.tasks.get(&id).and_then(|task| board_task_view_with_master(&state, task, Some(&actor), master.as_deref(), Some(&route))), "msg_id": message_id, "consumed": false });
    drop(state);
    let attempt = match &subscription {
        Some(subscription) => attempt_notification_detailed_with_at(server, &message_id, &subscription.id, now_ms()),
        None => NotificationAttempt::NotAttempted(NOTIFICATION_SUBSCRIPTION_MISSING_ERROR.into()),
    };
    notification_send_response(data, subscription.is_some(), method, &attempt)
}

fn board_respond(server: &Server, actor: String, token: String, id: String, expected_revision: u64, accept: bool, reason: Option<String>) -> Resp {
    if !accept && reason.as_deref().is_none_or(|reason| reason.trim().is_empty()) { return Resp::err("decline requires a nonempty --reason"); }
    // The authenticated actor and execution binding are the ownership
    // admission facts. Endpoint activity and `who` projection remain separate.
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
    let mut details = match board_check_revision(&state, &id, expected_revision) { Ok(details) => details, Err(error) => return error };
    let Some(invite) = details.invitation.clone() else { return Resp::err("task has no pending invitation"); };
    if invite.peer_id != actor { return Resp::err("only the invited peer may respond"); }
    let Some(mut task) = state.tasks.get(&id).cloned() else { return Resp::err("task not found"); };
    if matches!(task.status.as_str(), "closed" | "cancelled") { return Resp::err("BOARD_TASK_CLOSED: closed tasks cannot accept invitations"); }
    let first_invitation = task.status == "invited";
    if first_invitation {
        if task.owner != task.created_by { return Resp::err("task invitation is not awaiting a response"); }
        if let Err(error) = board_unstarted_resources(&state, &task) { return error; }
    }
    let mut events = Vec::new();
    if accept {
        if let Err(error)=work_executor_gate(server,&state,&id,&actor) {return error;}
        // Validate resources before moving their current owner references.
        if let Err(error) = board_resume_gate(&state, &task.owner, &id) { return error; }
        let route = match server_route_scope(server, &state) { Ok(Some(route)) => route, _ => return Resp::err("project route is not ready") };
        let binding = match authoritative_send_binding(&state, &route, &actor) { Ok(binding) => binding, Err(error) => return Resp::err(error) };
        if binding.binding_id.as_str() != invite.binding_id || binding.endpoint_generation != invite.endpoint_generation {
            return Resp::err("BOARD_STALE_INVITATION: recipient rebound; master must withdraw and issue a new invitation");
        }
        let occupancy = state.name_occupancies.get(route.project_scope_id.as_str())
            .and_then(|names| names.values().find(|occupancy| occupancy.worker_id == actor));
        if let Some(occupancy) = occupancy {
            events.push(Event::TaskAssignmentUpdated { assignment: name_state::TaskAssignment {
                task_id: id.clone(), name: occupancy.name.clone(), session_id: occupancy.session_id.clone(),
                assignment_revision: state.task_assignment(&id).map_or(1, |assignment| assignment.assignment_revision + 1),
                reflow: None, updated_ms: now_ms(),
            } });
        } else if state.task_assignment(&id).is_some() {
            return Resp::err("BOARD_TARGET_NAME_REQUIRED: named work requires a current recipient name");
        }
        events.extend(name_resolution::task_resource_owner_events(&state, &task, &actor));
        task.owner = actor.clone();
        if first_invitation { task.status = "working".into(); }
        details.last_response = Some(format!("accepted by {actor}"));
    } else {
        if first_invitation { task.status = "pending".into(); }
        details.last_response = Some(format!("declined by {actor}: {}", reason.unwrap()));
    }
    task.updated_ms = now_ms(); details.revision += 1; details.invitation = None;
    events.extend([Event::TaskUpdated { task }, Event::BoardDetailsChanged { task_id: id.clone(), details }]);
    if !accept { events.push(Event::Superseded { ids: vec![invite.message_id] }); }
    if let Err(error) = server.commit_locked_checked(&mut state, &events) { return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}")); }
    board_task_response(server, &state, &id, &actor)
}

fn board_withdraw(server: &Server, actor: String, token: String, id: String, expected_revision: u64, reason: String) -> Resp {
    if reason.trim().is_empty() { return Resp::err("withdraw requires a nonempty reason"); }
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, &actor, &token)
        .and_then(|()| board_check_master_locked(server, &state, &actor)) { return error; }
    let mut details = match board_check_revision(&state, &id, expected_revision) { Ok(details) => details, Err(error) => return error };
    let Some(invite) = details.invitation.take() else { return Resp::err("task has no pending invitation"); };
    let Some(mut task) = state.tasks.get(&id).cloned() else { return Resp::err("task not found"); };
    let mut events = Vec::new();
    if task.status == "invited" {
        task.status = "pending".into(); task.updated_ms = now_ms();
        events.push(Event::TaskUpdated { task });
    }
    details.revision += 1; details.last_response = Some(format!("withdrawn by {actor}: {reason}"));
    events.extend([
        Event::BoardDetailsChanged { task_id: id.clone(), details },
        Event::Superseded { ids: vec![invite.message_id] },
    ]);
    if let Err(error) = server.commit_locked_checked(&mut state, &events) { return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}")); }
    board_task_response(server, &state, &id, &actor)
}
