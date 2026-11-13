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
    board_invite_with_selection(server,actor,token,id,to,expected_revision,None,None)
}

pub(super) struct PreparedBoardInvitation {
    pub(super) events: Vec<Event>,
    pub(super) message_id: String,
    pub(super) subscription: Option<NotificationSubscription>,
    pub(super) method: &'static str,
}

pub(super) struct BoardManagerNotification {
    data: serde_json::Value,
    message_id: String,
    subscription: Option<NotificationSubscription>,
    method: &'static str,
}

pub(super) enum BoardManagerOutcome {
    Reply(Resp),
    Notify(BoardManagerNotification),
}

impl BoardManagerOutcome {
    pub(super) fn finish(self, server: &Server) -> Resp {
        match self {
            Self::Reply(response) => response,
            Self::Notify(notification) => {
                let attempt = match &notification.subscription {
                    Some(subscription) => attempt_notification_detailed_with_at(
                        server,
                        &notification.message_id,
                        &subscription.id,
                        now_ms(),
                    ),
                    None => NotificationAttempt::NotAttempted(
                        NOTIFICATION_SUBSCRIPTION_MISSING_ERROR.into(),
                    ),
                };
                notification_send_response(
                    notification.data,
                    notification.subscription.is_some(),
                    notification.method,
                    &attempt,
                )
            }
        }
    }
}

fn board_task_response_with_control(
    server: &Server,
    state: &State,
    control: &binding_slot::BindingControlSet,
    task_id: &str,
    viewer: &str,
) -> Resp {
    let route = match server_route_scope(server, state) {
        Ok(route) => route,
        Err(error) => return Resp::err(error),
    };
    let master = current_master_worker_id(state, route.as_ref());
    match state
        .tasks
        .get(task_id)
        .map(|task| {
            board_task_view_with_master(
                state,
                control,
                task,
                Some(viewer),
                master.as_deref(),
                route.as_ref(),
            )
        })
        .transpose()
    {
        Ok(Some(Some(task))) => Resp::data(json!({
            "task": task,
            "notification": "none",
            "goal_schedule": task_goal_receipt(state, task_id),
        })),
        Ok(_) => Resp::err("BOARD_TASK_NOT_PUBLIC: task does not belong to the public task board"),
        Err(error) => Resp::err(error),
    }
}

struct PreparedBoardResponse {
    events: Vec<Event>,
    rejected: Option<Resp>,
}

pub(super) fn prepare_board_invitation_locked(
    server: &Server,
    state: &State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    initiator: Option<&str>,
    id: &str,
    to: &str,
    expected_revision: u64,
    selection: Option<(String, String)>,
    long_horizon: Option<crate::board::LongHorizonTaskSpec>,
    automatic_source: Option<crate::board::BoardInvitationSource>,
    now: i64,
) -> Result<PreparedBoardInvitation, Resp> {
    let mut details = match board_check_revision(state, id, expected_revision) {
        Ok(details) => details,
        Err(error) => return Err(error),
    };
    details.long_horizon = long_horizon.or(details.long_horizon);
    details.long_horizon =
        match crate::board::LongHorizonArgs::default().merge(details.long_horizon.as_ref()) {
            Ok(Some(spec)) => Some(spec),
            Ok(None) => {
                return Err(Resp::err(
                    "BOARD_LONG_HORIZON_CONTENT_REQUIRED: fill the task long-horizon declaration before inviting",
                ))
            }
            Err(error) => return Err(Resp::err(error)),
        };
    if !details.public_visibility {
        return Err(Resp::err(
            "BOARD_TASK_NOT_PUBLIC: only public work may be invited",
        ));
    }
    let Some(mut task) = state.tasks.get(id).cloned() else {
        return Err(Resp::err("task not found"));
    };
    if matches!(task.status.as_str(), "closed" | "cancelled") {
        return Err(Resp::err(
            "BOARD_TASK_CLOSED: closed tasks cannot be reassigned",
        ));
    }
    if details.invitation.is_some() || task.status == "invited" {
        return Err(Resp::err("task already has a pending invitation"));
    }
    let first_invitation = task.status == "pending";
    if first_invitation {
        board_unstarted_resources(state, &task)?;
    }

    let to = match control.current_for_name(&route.project_scope_id, &route.app_scope_id, to) {
        Ok(Some(slot)) => slot.active_binding().unwrap().binding.agent_id.as_str().to_owned(),
        Ok(None) => to.to_owned(),
        Err(error) => return Err(Resp::err(error)),
    };
    if initiator.is_some_and(|actor| to == actor)
        || (initiator.is_none() && to == task.owner)
        || is_managed_subagent(state, &to)
    {
        return Err(Resp::err(
            "BOARD_ORDINARY_PEER_REQUIRED: invite a different ordinary peer",
        ));
    }
    if !state.workers.contains_key(&to) {
        return Err(Resp::err("peer not registered"));
    }
    let slot = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, &to)
        .map_err(Resp::err)?
        .ok_or_else(|| Resp::err("peer not registered"))?;
    execution_admission_gate_with_control(state, control, route, id, &to, &slot.identity_key.name)?;
    let current = slot
        .active_binding()
        .ok_or_else(|| Resp::err("peer not registered"))?;
    let binding = authoritative_send_binding(state, route, &to).map_err(Resp::err)?;
    if binding.binding_id != current.binding.binding_id
        || binding.session_id != current.binding.session_id
    {
        return Err(Resp::err(
            "BOARD_STALE_BINDING: recipient runtime binding is not current",
        ));
    }
    let message_id = format!("board-invite-{}-{}", id, details.revision + 1);
    details.invitation = Some(crate::board::BoardInvitation {
        peer_id: to.clone(),
        binding_id: binding.binding_id.as_str().into(),
        message_id: message_id.clone(),
        created_ms: now,
        original_status: Some(task.status.clone()),
        original_owner: Some(task.owner.clone()),
        automatic_source,
    });
    details.last_response = None;
    if let Some((name, reason)) = selection {
        details.dispatch_selection = Some(name);
        details.dispatch_reason = Some(reason);
    }
    details.revision += 1;
    if first_invitation {
        task.status = "invited".into();
    }
    task.updated_ms = now;
    let from = initiator.unwrap_or("daemon").to_owned();
    let message = Message {
        id: message_id.clone(),
        from: from.clone(),
        to: to.clone(),
        mtype: "request".into(),
        subject: Some(format!("task-invitation:{id}")),
        body: format!(
            "{}\n{}\nDelivery condition: {}\nTest condition: {}\nContinue this subgoal from status {} with its existing evidence and resources.\nRespond using collab board respond <task-id> --accept|--decline --expected-revision <observed-revision>, taking the task ID and revision from the typed task view, not this message. Run collab board show only when you do not have the current typed revision. You may decline; an invitation alone does not transfer ownership.",
            details.title,
            details.description,
            details.delivery_condition,
            details.test_condition,
            details
                .invitation
                .as_ref()
                .and_then(|invitation| invitation.original_status.as_deref())
                .unwrap_or("working"),
        ),
        in_reply_to: None,
        created_ms: now,
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    let subscription = state
        .matching_subscription(&to, "direct-message", None, now)
        .cloned();
    let method = notification_method_for_worker(state, &to);
    let mut events = vec![
        Event::TaskUpdated { task },
        Event::BoardDetailsChanged {
            task_id: id.to_owned(),
            details,
        },
        Event::Sent { msg: message },
        Event::DeliveryMode {
            msg_id: message_id.clone(),
            mode: "explicit-notification".into(),
            source_thread_id: None,
        },
    ];
    if let Some(subscription) = &subscription {
        events.push(Event::WakeBound {
            message_id: message_id.clone(),
            subscription_id: subscription.id.clone(),
        });
    }
    Ok(PreparedBoardInvitation {
        events,
        message_id,
        subscription,
        method,
    })
}

fn automatic_invitation_is_stale(
    state: &State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    task_id: &str,
    invitation: &crate::board::BoardInvitation,
) -> Result<bool, Resp> {
    let Some(source) = invitation.automatic_source.as_ref() else {
        return Ok(false);
    };
    let current_revision = state
        .task_assignment(task_id)
        .map_or(0, |assignment| assignment.assignment_revision);
    let schedule_is_stale = state
        .name_reassignment(&source.project_scope, &source.name)
        .is_none_or(|schedule| {
            current_revision != source.assignment_revision
                || !schedule.is_current_for_source(&source.name)
                || schedule.round_ms() != source.schedule_round_ms
        });
    if schedule_is_stale {
        return Ok(true);
    }
    let current = control
        .current_for_name(&route.project_scope_id, &route.app_scope_id, &source.name)
        .map_err(Resp::err)?;
    let Some(current) = current.and_then(|slot| slot.active_binding().cloned()) else {
        return Ok(false);
    };
    if state.task_assignment(task_id).is_some_and(|assignment| {
        current
            .binding
            .session_id
            .as_ref()
            .is_none_or(|session| session.as_str() != assignment.session_id)
    }) {
        return Ok(true);
    }
    Ok(state
        .agent_presence(
            route.project_scope_id.as_str(),
            current.binding.agent_id.as_str(),
        )
        .is_some_and(|presence| {
            presence.presence == name_state::AgentPresence::Online
                && current
                    .binding
                    .session_id
                    .as_ref()
                    .is_some_and(|session| session.as_str() == presence.session_id)
        }))
}

fn stale_automatic_invitation_events(
    state: &State,
    task_id: &str,
    invitation: &crate::board::BoardInvitation,
) -> Result<Vec<Event>, Resp> {
    let Some(mut details) = state.board_details.get(task_id).cloned() else {
        return Err(Resp::err("task not found"));
    };
    if details
        .invitation
        .as_ref()
        .is_none_or(|current| current.message_id != invitation.message_id)
    {
        return Err(Resp::err("BOARD_STALE_AUTOMATIC_INVITATION"));
    }
    details.invitation = None;
    details.revision += 1;
    details.last_response = Some("automatic invitation invalidated by changed scheduling source".into());
    let now = now_ms();
    let mut events = Vec::new();
    if let Some(mut task) = state.tasks.get(task_id).cloned() {
        if task.status == "invited" {
            task.status = invitation
                .original_status
                .clone()
                .unwrap_or_else(|| "pending".into());
            task.updated_ms = now;
            events.push(Event::TaskUpdated { task });
        }
    }
    events.extend([
        Event::BoardDetailsChanged {
            task_id: task_id.to_owned(),
            details,
        },
        Event::Superseded {
            ids: vec![invitation.message_id.clone()],
        },
    ]);
    Ok(events)
}

fn board_invite_with_selection(server: &Server, actor: String, token: String, id: String, to: String,
    expected_revision: u64,selection:Option<(String,String)>,long_horizon:Option<crate::board::LongHorizonTaskSpec>) -> Resp {
    // Resolve the name, binding and task revision under the same reducer lock.
    // Notification delivery happens after the durable invitation is committed.
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, &actor, &token)
        .and_then(|()| board_check_master_locked(server, &state, &actor)) { return error; }
    let route = match server_route_scope(server, &state) { Ok(Some(route)) => route, _ => return Resp::err("project route is not ready") };
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
        Ok(control) => control, Err(error) => return Resp::err(error),
    };
    let now = now_ms();
    let prepared = match prepare_board_invitation_locked(
        server,
        &state,
        &control,
        &route,
        Some(&actor),
        &id,
        &to,
        expected_revision,
        selection,
        long_horizon,
        None,
        now,
    ) {
        Ok(prepared) => prepared,
        Err(error) => return error,
    };
    if let Err(error) = server.commit_locked_checked_with_control(&mut state, &prepared.events, &control) {
        return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}"));
    }
    let master = current_master_worker_id(&state, Some(&route));
    let task = match state.tasks.get(&id).map(|task| board_task_view_with_master(&state, &control, task, Some(&actor), master.as_deref(), Some(&route))).transpose() {
        Ok(task) => task.flatten(), Err(error) => return Resp::err(error),
    };
    let data = json!({ "task": task, "msg_id": prepared.message_id, "consumed": false, "goal_schedule": task_goal_receipt(&state, &id) });
    drop(state);
    let attempt = match &prepared.subscription {
        Some(subscription) => attempt_notification_detailed_with_at(server, &prepared.message_id, &subscription.id, now_ms()),
        None => NotificationAttempt::NotAttempted(NOTIFICATION_SUBSCRIPTION_MISSING_ERROR.into()),
    };
    notification_send_response(data, prepared.subscription.is_some(), prepared.method, &attempt)
}

fn board_invite_current_map(
    manager: &ProjectRuntimeManager,
    server: &Server,
    actor: String,
    token: String,
    id: String,
    to: String,
    expected_revision: u64,
) -> Resp {
    let outcome = manager.with_current_state_response(server, |control, state| {
        board_current_map_actor_gate(state, &actor, &token, &id)?;
        board_check_master_locked(server, state, &actor)?;
        let route = server_route_scope(server, state)
            .map_err(Resp::err)?
            .ok_or_else(|| Resp::err("project route is not ready"))?;
        let now = now_ms();
        let prepared = prepare_board_invitation_locked(
            server,
            state,
            control,
            &route,
            Some(&actor),
            &id,
            &to,
            expected_revision,
            None,
            None,
            None,
            now,
        )?;
        server
            .commit_locked_checked_with_control(state, &prepared.events, control)
            .map_err(|error| Resp::err(format!("BOARD_DURABILITY_FAILED: {error}")))?;
        let master = current_master_worker_id(state, Some(&route));
        let task = state
            .tasks
            .get(&id)
            .map(|task| {
                board_task_view_with_master(
                    state,
                    control,
                    task,
                    Some(&actor),
                    master.as_deref(),
                    Some(&route),
                )
            })
            .transpose()
            .map_err(Resp::err)?
            .flatten();
        let data = json!({
            "task": task,
            "msg_id": prepared.message_id,
            "consumed": false,
            "goal_schedule": task_goal_receipt(state, &id),
        });
        Ok(BoardManagerOutcome::Notify(BoardManagerNotification {
            data,
            message_id: prepared.message_id,
            subscription: prepared.subscription,
            method: prepared.method,
        }))
    });
    match outcome {
        Ok(outcome) => outcome.finish(server),
        Err(response) => response,
    }
}

fn board_respond(server: &Server, actor: String, token: String, id: String, expected_revision: u64, accept: bool, reason: Option<String>) -> Resp {
    if !accept && reason.as_deref().is_none_or(|reason| reason.trim().is_empty()) { return Resp::err("decline requires a nonempty --reason"); }
    // The authenticated actor and execution binding are the ownership
    // admission facts. Endpoint activity and `who` projection remain separate.
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
        Ok(control) => control, Err(error) => return Resp::err(error),
    };
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
    let route = match server_route_scope(server, &state) {
        Ok(Some(route)) => route,
        _ => return Resp::err("project route is not ready"),
    };
    let prepared = match prepare_board_response_locked(
        &state,
        &control,
        &route,
        &actor,
        &id,
        expected_revision,
        accept,
        reason,
    ) {
        Ok(prepared) => prepared,
        Err(error) => return error,
    };
    if let Err(error) =
        server.commit_locked_checked_with_control(&mut state, &prepared.events, &control)
    {
        return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}"));
    }
    if let Some(response) = prepared.rejected {
        return response;
    }
    board_task_response(server, &state, &id, &actor)
}

fn board_respond_current_map(
    manager: &ProjectRuntimeManager,
    server: &Server,
    actor: String,
    token: String,
    id: String,
    expected_revision: u64,
    accept: bool,
    reason: Option<String>,
) -> Resp {
    let outcome = manager.with_current_state_response(server, |control, state| {
        board_current_map_actor_gate(state, &actor, &token, &id)?;
        let route = server_route_scope(server, state)
            .map_err(Resp::err)?
            .ok_or_else(|| Resp::err("project route is not ready"))?;
        let prepared =
            prepare_board_response_locked(state, control, &route, &actor, &id, expected_revision, accept, reason)?;
        server
            .commit_locked_checked_with_control(state, &prepared.events, control)
            .map_err(|error| Resp::err(format!("BOARD_DURABILITY_FAILED: {error}")))?;
        if let Some(response) = prepared.rejected {
            return Ok(BoardManagerOutcome::Reply(response));
        }
        Ok(BoardManagerOutcome::Reply(board_task_response_with_control(
            server, state, control, &id, &actor,
        )))
    });
    match outcome {
        Ok(outcome) => outcome.finish(server),
        Err(response) => response,
    }
}

fn board_reassignment_ready(
    state: &State,
    route: &RouteScope,
    task: &TaskRec,
    recipient_name: &str,
    policy: &crate::board::WorkPolicy,
) -> Result<(), Resp> {
    if let Some(roles) = policy.preferred_roles.as_ref() {
        let current_roles = state.name_work_roles(route.project_scope_id.as_str(), recipient_name);
        if !roles.iter().any(|role| current_roles.contains(role)) {
            return Err(Resp::err_data(
                "WORK_ROLE_REQUIRED: task is pinned to a preferred role",
                json!({"task_id":task.id,"required_roles":roles}),
            ));
        }
    }
    if let Some(phase) = state.ticket_phases.iter().filter(|phase| {
        phase.task_id == task.id
            && phase.phase == crate::server::ticket_state::TicketPhaseKind::Execute
            && phase.status != crate::server::ticket_state::TicketPhaseStatus::Revoked
    }).max_by_key(|phase| phase.batch) {
        let dependency_satisfied = |dependency: &str| {
            state.tasks.get(dependency).is_some_and(|task| {
                matches!(task.status.as_str(), "delivered" | "accepted" | "merged" | "closed")
            }) || state.ticket_phases.iter().any(|candidate| {
                candidate.task_id == dependency
                    && candidate.phase == crate::server::ticket_state::TicketPhaseKind::Execute
                    && candidate.status == crate::server::ticket_state::TicketPhaseStatus::Completed
            })
        };
        if !phase.depends_on.iter().all(|dependency| dependency_satisfied(dependency)) {
            return Err(Resp::err("BOARD_DEPENDENCY_UNSATISFIED: task dependencies are incomplete"));
        }
    }
    board_resume_gate(state, &task.owner, &task.id)
}

fn prepare_board_response_locked(
    state: &State,
    control: &binding_slot::BindingControlSet,
    route: &RouteScope,
    actor: &str,
    id: &str,
    expected_revision: u64,
    accept: bool,
    reason: Option<String>,
) -> Result<PreparedBoardResponse, Resp> {
    if !accept && reason.as_deref().is_none_or(|reason| reason.trim().is_empty()) {
        return Err(Resp::err("decline requires a nonempty --reason"));
    }
    let mut details = board_check_revision(state, id, expected_revision)?;
    let Some(invite) = details.invitation.clone() else {
        return Err(Resp::err("task has no pending invitation"));
    };
    if invite.peer_id != actor {
        return Err(Resp::err("only the invited peer may respond"));
    }
    if automatic_invitation_is_stale(state, control, route, id, &invite)? {
        return Ok(PreparedBoardResponse {
            events: stale_automatic_invitation_events(state, id, &invite)?,
            rejected: Some(Resp::err("BOARD_STALE_AUTOMATIC_INVITATION")),
        });
    }
    let Some(mut task) = state.tasks.get(id).cloned() else {
        return Err(Resp::err("task not found"));
    };
    if matches!(task.status.as_str(), "closed" | "cancelled") {
        return Err(Resp::err("BOARD_TASK_CLOSED: closed tasks cannot accept invitations"));
    }
    let first_invitation = matches!(task.status.as_str(), "pending" | "invited");
    if first_invitation {
        board_unstarted_resources(state, &task)?;
    }
    let mut events = Vec::new();
    if accept {
        let policy = state.effective_work_policy(id);
        let current_slot = control
            .current_for_worker(&route.project_scope_id, &route.app_scope_id, actor)
            .map_err(Resp::err)?
            .ok_or_else(|| {
                Resp::err("BOARD_TARGET_NAME_REQUIRED: named work requires a current recipient name")
            })?;
        let current = current_slot.active_binding().ok_or_else(|| {
            Resp::err("BOARD_TARGET_NAME_REQUIRED: named work requires a current recipient name")
        })?;
        execution_admission_gate_with_control(
            state,
            control,
            route,
            id,
            actor,
            &current_slot.identity_key.name,
        )?;
        // Validate resources before moving their current owner references.
        board_resume_gate(state, &task.owner, id)?;
        let Some(session) = current.binding.session_id.as_ref() else {
            return Err(Resp::err("BOARD_TARGET_SESSION_REQUIRED"));
        };
        let binding = authoritative_send_binding(state, route, actor).map_err(Resp::err)?;
        if binding.binding_id != current.binding.binding_id
            || binding.session_id != current.binding.session_id
            || binding.binding_id.as_str() != invite.binding_id
        {
            return Err(Resp::err("BOARD_STALE_INVITATION: recipient rebound; master must withdraw and issue a new invitation"));
        }
        if let Some(executor_name) = policy.executor_name.as_deref() {
            if current_slot.identity_key.name != executor_name {
                return Err(Resp::err_data(
                    "WORK_EXECUTOR_REQUIRED: task is pinned to another stable name",
                    json!({"task_id":id,"executor_name":executor_name,"delivery_complete":false}),
                ));
            }
        }
        if invite.automatic_source.is_some() {
            let online = state
                .agent_presence(route.project_scope_id.as_str(), actor)
                .is_some_and(|presence| {
                    presence.presence == name_state::AgentPresence::Online
                        && presence.session_id == session.as_str()
                });
            if !online {
                return Err(Resp::err(
                    "BOARD_TARGET_NOT_ONLINE: automatic acceptance requires its current online binding",
                ));
            }
            if board_named_execution_reserved(state, actor, &current_slot.identity_key.name, id) {
                return Err(Resp::err(
                    "BOARD_TARGET_BUSY: another invitation or execution reserves the recipient slot",
                ));
            }
        }
        board_reassignment_ready(state, route, &task, &current_slot.identity_key.name, &policy)?;
        events.push(Event::TaskAssignmentUpdated { assignment: name_state::TaskAssignment {
            task_id: id.to_owned(), name: current_slot.identity_key.name.clone(), session_id: session.as_str().into(),
            assignment_revision: state.task_assignment(id).map_or(1, |assignment| assignment.assignment_revision + 1),
            reflow: None, updated_ms: now_ms(),
        } });
            // An invitation accept on an admitted ticket is the compat
            // "specified receiver" entry. It must leave exactly one current
            // execute phase owned by the accepting slot.
            if state.board_details.get(id).is_some_and(|details| details.admitted) {
                if let Some(phase) = state
                    .ticket_phases
                    .iter()
                    .filter(|phase| {
                        phase.task_id == id
                            && phase.phase == crate::server::ticket_state::TicketPhaseKind::Execute
                            && phase.status
                                != crate::server::ticket_state::TicketPhaseStatus::Revoked
                    })
                    .max_by_key(|phase| phase.batch)
                    .cloned()
                {
                    if matches!(
                        phase.status,
                        crate::server::ticket_state::TicketPhaseStatus::Queued
                            | crate::server::ticket_state::TicketPhaseStatus::WaitingAssignment
                    ) {
                        let mut claimed = phase;
                        claimed.status = crate::server::ticket_state::TicketPhaseStatus::Claimed;
                        claimed.slot_name = Some(current_slot.identity_key.name.clone());
                        claimed.binding_generation = current_slot.generation;
                        claimed.attempt_generation = match claimed.attempt_generation.checked_add(1) {
                            Some(attempt) => attempt,
                            None => return Err(Resp::err_data(
                                "TICKET_ATTEMPT_EXHAUSTED: cannot allocate a new claim attempt",
                                json!({"task_id": id, "attempt": claimed.attempt_generation}),
                            )),
                        };
                        claimed.wait_reason = None;
                        events.push(Event::TicketPhaseRecorded { phase: claimed });
                    } else if phase.status
                        == crate::server::ticket_state::TicketPhaseStatus::Claimed
                        && phase.slot_name.as_deref()
                            != Some(current_slot.identity_key.name.as_str())
                    {
                        let batch = match phase.batch.checked_add(1) {
                            Some(batch) => batch,
                            None => return Err(Resp::err_data(
                                "TICKET_BATCH_EXHAUSTED: cannot allocate a reassignment batch",
                                json!({"task_id": id, "batch": phase.batch}),
                            )),
                        };
                        let attempt = match phase.attempt_generation.checked_add(1) {
                            Some(attempt) => attempt,
                            None => return Err(Resp::err_data(
                                "TICKET_ATTEMPT_EXHAUSTED: cannot allocate a reassignment attempt",
                                json!({"task_id": id, "attempt": phase.attempt_generation}),
                            )),
                        };
                        let mut revoked = phase.clone();
                        revoked.status = crate::server::ticket_state::TicketPhaseStatus::Revoked;
                        let mut claimed = phase;
                        claimed.batch = batch;
                        claimed.status = crate::server::ticket_state::TicketPhaseStatus::Claimed;
                        claimed.slot_name = Some(current_slot.identity_key.name.clone());
                        claimed.binding_generation = current_slot.generation;
                        claimed.attempt_generation = attempt;
                        claimed.wait_reason = None;
                        claimed.result = None;
                        events.extend([
                            Event::TicketPhaseRecorded { phase: revoked },
                            Event::TicketPhaseRecorded { phase: claimed },
                        ]);
                    }
                }
            }
        events.extend(name_resolution::task_resource_owner_events(state, &task, actor));
        task.owner = actor.to_owned();
        if first_invitation { task.status = "working".into(); }
        details.last_response = Some(format!("accepted by {actor}"));
    } else {
        if first_invitation { task.status = "pending".into(); }
        details.last_response = Some(format!("declined by {actor}: {}", reason.unwrap()));
    }
    task.updated_ms = now_ms(); details.revision += 1; details.invitation = None;
    events.extend([Event::TaskUpdated { task }, Event::BoardDetailsChanged { task_id: id.to_owned(), details }]);
    if !accept { events.push(Event::Superseded { ids: vec![invite.message_id] }); }
    Ok(PreparedBoardResponse {
        events,
        rejected: None,
    })
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
