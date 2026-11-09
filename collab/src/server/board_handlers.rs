use crate::board::{BoardCommand, BoardMemberView, BoardTaskDetails, BoardTaskView};

fn board_public_task(state: &State, task_id: &str) -> bool {
    state.board_details.get(task_id).is_some_and(|details| details.public_visibility)
}

fn board_parent_for_actor<'a>(state: &'a State, actor: &str) -> Option<&'a str> {
    state.subagents.values().find(|record| record.peer == actor).map(|record| record.parent.as_str())
}

fn board_external_actor(state: &State, actor: &str) -> String {
    board_parent_for_actor(state, actor).unwrap_or(actor).into()
}

#[cfg(test)]
fn board_task_view(state: &State, control: &binding_slot::BindingControlSet, task: &TaskRec) -> Result<Option<BoardTaskView>, String> {
    board_task_view_for(state, control, task, None)
}

#[cfg(test)]
fn board_task_view_for(state: &State, control: &binding_slot::BindingControlSet, task: &TaskRec, viewer: Option<&str>) -> Result<Option<BoardTaskView>, String> {
    let master = current_master_worker_id(state, None);
    let route = state.workers.get(&task.owner).and_then(|worker| {
        let project = GlobalState::canonical_project_scope(Path::new(&worker.cwd)).ok()?;
        current_worker_binding(state, &project, None, worker).map(|binding| binding.route_scope())
    });
    board_task_view_with_master(state, control, task, viewer, master.as_deref(), route.as_ref())
}

fn board_task_visible(state: &State, task: &TaskRec, viewer: Option<&str>) -> bool {
    let Some(details) = state.board_details.get(&task.id) else { return false; };
    if let Some(viewer) = viewer {
        let private_owner = viewer == task.owner
            || board_parent_for_actor(state, &task.owner) == Some(viewer);
        if !details.public_visibility && !private_owner {
            return false;
        }
    } else if !details.public_visibility {
        return false;
    }
    true
}

// Shared typed availability decision. Views and scheduling must use the same
// current occupancy, presence and binding facts, never a projected board row.
fn assignment_unavailability(state: &State, control: &binding_slot::BindingControlSet, task: &TaskRec, route: Option<&RouteScope>) -> Result<Option<&'static str>, String> {
    if matches!(task.status.as_str(), "closed" | "cancelled") { return Ok(None); }
    let assignment = state.task_assignment(&task.id);
    let current = match assignment.zip(route) {
        Some((assignment, route)) => control.current_for_name(&route.project_scope_id, &route.app_scope_id, &assignment.name)?
            .and_then(|slot| slot.active_binding()), None => None,
    };
    if assignment.is_some() && current.is_none() { return Ok(Some("name_unoccupied")); }
    if current.zip(route).is_some_and(|(current, route)| {
        state.agent_presence(route.project_scope_id.as_str(), current.binding.agent_id.as_str())
            .is_some_and(|presence| current.binding.session_id.as_ref().is_some_and(|session| session.as_str() == presence.session_id)
                && presence.presence == name_state::AgentPresence::Offline)
    }) { return Ok(Some("owner_offline")); }
    let owner = current.map_or(task.owner.as_str(), |current| current.binding.agent_id.as_str());
    if route.and_then(|route| state.workers.get(owner).and_then(|worker|
        current_worker_binding(state, &route.project_scope_id, Some(&route.app_scope_id), worker))).is_none() {
        return Ok(Some("owner_binding_missing"));
    }
    Ok(None)
}

fn board_task_view_with_master(state: &State, control: &binding_slot::BindingControlSet, task: &TaskRec, viewer: Option<&str>, _master: Option<&str>, route: Option<&RouteScope>) -> Result<Option<BoardTaskView>, String> {
    if !board_task_visible(state, task, viewer) { return Ok(None); }
    let details = state.board_details.get(&task.id).expect("visible board details");
    let lifecycle = state.task_lifecycle.get(&task.id);
    let cleanup = state.cleanup_receipts.get(&task.id);
    let policy=state.effective_work_policy(&task.id);
    let (completion_delivery_complete,completion_missing)=state.delivery_completion_projection(&task.id);
    let assignment = state.task_assignment(&task.id);
    let historical = matches!(task.status.as_str(), "closed" | "cancelled");
    let current = match assignment.zip(route) {
        Some((assignment, route)) => control.current_for_name(&route.project_scope_id, &route.app_scope_id, &assignment.name)?
            .and_then(|slot| slot.active_binding()), None => None,
    };
    let current_owner = if historical { Some(task.owner.as_str()) }
        else if assignment.is_some() { current.map(|current| current.binding.agent_id.as_str()) }
        else { Some(task.owner.as_str()) };
    let reassignment_reason = assignment_unavailability(state, control, task, route)?.map(str::to_owned);
    let owner = if reassignment_reason.is_some() {
        String::new()
    } else if details.public_visibility {
        board_external_actor(state, current_owner.unwrap_or_default())
    } else {
        current_owner.unwrap_or_default().to_owned()
    };
    let publisher = if details.public_visibility {
        board_external_actor(state, &task.created_by)
    } else {
        task.created_by.clone()
    };
    Ok(Some(BoardTaskView {
        id: task.id.clone(),
        title: details.title.clone(),
        description: details.description.clone(),
        delivery_condition: details.delivery_condition.clone(),
        test_condition: details.test_condition.clone(),
        long_horizon: details.long_horizon.clone(),
        revision: details.revision,
        owner_display_name: if owner.is_empty() { String::new() }
            else if let Some(assignment) = assignment.filter(|_| current_owner == Some(owner.as_str())) { assignment.name.clone() }
            else { worker_display_name(control, route, &owner)? },
        owner_name: assignment.map(|assignment| assignment.name.clone()),
        owner_session_id: if historical { assignment.map(|assignment| assignment.session_id.clone()) }
            else if owner.is_empty() { None }
            else { current.and_then(|current| current.binding.session_id.as_ref().map(ToString::to_string)) },
        needs_reassignment: reassignment_reason.is_some(),
        reassignment_reason,
        publisher_display_name: worker_display_name(control, route, &publisher)?,
        owner,
        publisher,
        priority: task.priority.clone(),
        status: task.status.clone(),
        next_step: task.next_step.clone(),
        work_kind:policy.work_kind,
        parent_task_id:details.parent_task_id.clone(),
        child_work_policies:details.child_work_policies.clone(),
        preferred_roles:policy.preferred_roles.unwrap_or_default(),
        executor_name:policy.executor_name,
        reviewer_name:policy.reviewer_name,
        dispatch_selection:details.dispatch_selection.clone(),
        dispatch_reason:details.dispatch_reason.clone(),
        review_subject_revision:lifecycle.map_or(0,|record|record.review_subject_revision),
        reviewed_subject_revision:lifecycle.and_then(|record|record.reviewed_subject_revision),
        actual_reviewer_name:lifecycle.and_then(|record|record.reviewer_name.clone()),
        completion_delivery_complete,
        completion_missing,
        invited_peer: details.invitation.as_ref().map(|invite| invite.peer_id.clone()),
        last_response: details.last_response.clone(),
        updated_at: iso(task.updated_ms),
        delivery_evidence: lifecycle.and_then(|record| record.delivery_evidence.clone()),
        review_evidence: lifecycle.and_then(|record| record.review_evidence.clone()),
        integration_commit: lifecycle.and_then(|record| record.integration_commit.clone()),
        cleanup_status: if task.worktree_path.is_none() { "not_required" }
            else if cleanup.is_some_and(|receipt| receipt.verification == CleanupVerification::Verified) { "verified" }
            else if cleanup.is_some() { "unverified" } else { "pending" }.into(),
        blocking_task: task.wait.as_ref().map(|wait| {
            if board_public_task(state, &wait.waiting_for) { wait.waiting_for.clone() }
            else { "内部依赖".into() }
        }),
    }))
}

fn board_task_response(server: &Server, state: &State, task_id: &str, viewer: &str) -> Resp {
    let route = match server_route_scope(server, state) { Ok(route) => route, Err(error) => return Resp::err(error) };
    let master = current_master_worker_id(state, route.as_ref());
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
        Ok(control) => control, Err(error) => return Resp::err(error),
    };
    let result = state.tasks.get(task_id).map(|task| board_task_view_with_master(state, &control, task, Some(viewer), master.as_deref(), route.as_ref())).transpose();
    match result {
        Ok(Some(Some(task))) => Resp::data(json!({ "task": task, "notification": "none", "goal_schedule": task_goal_receipt(state, task_id) })),
        Ok(_) => Resp::err("BOARD_TASK_NOT_PUBLIC: task does not belong to the public task board"),
        Err(error) => Resp::err(error),
    }
}

fn handle_board_show(server: &Server, viewer: Option<&str>) -> Resp {
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
        Ok(control) => control, Err(error) => return Resp::err(error),
    };
    // Native observations run outside the reducer lock. They never determine
    // task owner, status, or the persisted invitation reservation.
    let candidates: Vec<_> = {
        let state = server.state.lock().unwrap();
        state.workers.values().filter(|worker| !is_managed_subagent(&state, &worker.id)).cloned().collect()
    };
    let observations: std::collections::HashMap<_, _> = candidates.into_iter().map(|worker| {
        let (status, endpoint_live) = if selected_transport_for_worker(&worker)
            .is_some_and(|transport| transport.kind == TransportKind::Tmux)
        {
            let (identity_valid, endpoint_live) = worker_identity_and_endpoint(server, &worker);
            let status = match (identity_valid, endpoint_live) {
                (false, _) | (_, Some(false)) => "offline",
                (true, Some(true)) | (true, None) => "unknown",
            };
            (status, endpoint_live)
        } else {
            // Preserve the activity owner's resumable/cold distinction on
            // other transports; only tmux presence became identity-only.
            match worker_presence(server, &worker) {
                IdentityPresence::Present => ("online", Some(true)),
                IdentityPresence::Cold => ("cold", Some(true)),
                IdentityPresence::Missing => ("offline", Some(false)),
                IdentityPresence::Unknown => ("unknown", None),
            }
        };
        (worker.id, (status, endpoint_live))
    }).collect();
    let state = server.state.lock().unwrap();
    let route = match server_route_scope(server, &state) {
        Ok(route) => route,
        Err(error) => return Resp::err(error),
    };
    // The board's master role comes from the same typed-grant projection
    // `status` and `context` use. The per-worker status stays a separate
    // transport observation, so a pane address never invents a live agent.
    let holder = current_master_worker_id(&state, route.as_ref());
    let endpoint_live = holder
        .as_deref()
        .and_then(|id| observations.get(id))
        .and_then(|(_, endpoint_live)| *endpoint_live);
    let authority = match master_authority_view_with_endpoint(&state, &control, route.as_ref(), endpoint_live) {
        Ok(authority) => authority, Err(error) => return Resp::err(error),
    };
    let master = authority
        .get("worker_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let mut tasks = Vec::new();
    for task in state.tasks.values().filter(|task| !matches!(task.status.as_str(), "closed" | "cancelled")) {
        match board_task_view_with_master(&state, &control, task, viewer, master.as_deref(), route.as_ref()) {
            Ok(Some(task)) => tasks.push(task), Ok(None) => {}, Err(error) => return Resp::err(error),
        }
    }
    tasks.sort_by(|left, right| left.id.cmp(&right.id));
    let mut workers = Vec::new();
    for worker in state.workers.values().filter(|worker| !is_managed_subagent(&state, &worker.id)) {
        let display_name = match worker_display_name(&control, route.as_ref(), &worker.id) {
            Ok(name) => name, Err(error) => return Resp::err(error),
        };
        let work_roles = match worker_work_roles(&state, &control, route.as_ref(), &worker.id) {
            Ok(roles) => roles, Err(error) => return Resp::err(error),
        };
        workers.push(BoardMemberView {
            id: worker.id.clone(),
            display_name,
            role: if master.as_deref() == Some(worker.id.as_str()) { "master" } else { "peer" }.into(),
            work_roles,
            status: observations.get(&worker.id).map(|(status, _)| *status).unwrap_or("unknown").into(),
            task_ids: tasks.iter().filter(|task| task.owner == worker.id && board_execution_responsibility(&task.status)).map(|task| task.id.clone()).collect(),
            invitation_ids: tasks.iter().filter(|task| task.invited_peer.as_deref() == Some(worker.id.as_str())).map(|task| task.id.clone()).collect(),
        });
    }
    workers.sort_by(|left, right| left.id.cmp(&right.id));
    Resp::data(json!({
        "schema_version": 1,
        "project": server.root.file_name().map(|name| name.to_string_lossy()),
        "observed_at": iso(now_ms()),
        "scope": scope_view(route.as_ref()),
        "master": authority,
        "tasks": tasks,
        "workers": workers,
        "work_policies": state.project_work_policy,
    }))
}

fn board_execution_responsibility(status: &str) -> bool {
    keepalive::unfinished(status)
}

fn board_execution_slot_occupied(status: &str) -> bool {
    board_execution_responsibility(status) && !matches!(status, "blocked" | "waiting")
}

/// One execution-capacity predicate for dispatch, invite, claim, acceptance,
/// and blocked resume. Blocked/waiting remain durable responsibilities in
/// keepalive/resource projections, but they do not consume the next slot.
pub(crate) fn board_peer_has_execution_reservation(state: &State, peer: &str, except_task: Option<&str>) -> bool {
    state.tasks.values().any(|task| {
        Some(task.id.as_str()) != except_task && (
            (task.owner == peer && board_execution_slot_occupied(&task.status))
            || (task.status == "invited" && state.board_details.get(&task.id)
                .and_then(|details| details.invitation.as_ref()).is_some_and(|invite| invite.peer_id == peer))
        )
    })
}

fn board_check_revision(state: &State, task_id: &str, expected: u64) -> Result<BoardTaskDetails, Resp> {
    let details = state.board_details.get(task_id)
        .ok_or_else(|| Resp::err("BOARD_TASK_NOT_FOUND: no described task with this id"))?;
    if details.revision != expected {
        return Err(Resp::err_data("BOARD_STALE_REVISION: reread the board before updating", json!({ "task_id": task_id, "expected_revision": expected, "current_revision": details.revision })));
    }
    Ok(details.clone())
}

/// Read-only ticket projection over the same phase records. The publish
/// response exposes this so callers can observe the admitted queue without a
/// second writable queue source.
fn ticket_projection(state: &State, task_id: &str) -> serde_json::Value {
    let phases: Vec<serde_json::Value> = state
        .ticket_phases
        .iter()
        .filter(|phase| phase.task_id == task_id)
        .map(|phase| {
            json!({
                "phase": phase.phase,
                "batch": phase.batch,
                "priority": phase.priority,
                "status": phase.status,
                "slot": phase.slot_name,
                "binding_generation": phase.binding_generation,
                "enqueue_seq": phase.enqueue_seq,
                "attempt_generation": phase.attempt_generation,
                "depends_on": phase.depends_on,
                "route_group": phase.route_group,
                "wait_reason": phase.wait_reason,
            })
        })
        .collect();
    json!({
        "task_id": task_id,
        "phases": phases,
    })
}

fn board_check_actor(state: &State, actor: &str, token: &str) -> Result<(), Resp> {
    verify(state, actor, token)?;
    Ok(())
}

fn board_check_master_locked(server: &Server, state: &State, actor: &str) -> Result<(), Resp> {
    let route = server_route_scope(server, state).map_err(Resp::err)?;
    if current_master_worker_id(state, route.as_ref()).as_deref() != Some(actor) {
        return Err(Resp::err("BOARD_MASTER_REQUIRED: only the current project master may invite or withdraw"));
    }
    Ok(())
}

fn handle_board_command(server: &Server, actor: String, token: String, command: BoardCommand) -> Resp {
    {
        let state = server.state.lock().unwrap();
        if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
        let task_id = match &command {
            BoardCommand::Show | BoardCommand::Publish { .. } => None,
            BoardCommand::Policy {id,..} => id.as_ref(),
            BoardCommand::Claim { id, .. } | BoardCommand::Start { id, .. }
            | BoardCommand::Update { id, .. }
            | BoardCommand::Describe { id, .. } | BoardCommand::Invite { id, .. }
            | BoardCommand::Respond { id, .. } | BoardCommand::DeclineAssigned { id, .. }
            | BoardCommand::Withdraw { id, .. } | BoardCommand::Dispatch {id,..} => Some(id),
        };
        // Check visibility before reporting revision or lifecycle facts.
        if let Some(task) = task_id.and_then(|id| state.tasks.get(id)) {
            if !board_task_visible(&state, task, Some(&actor)) {
                return Resp::err("BOARD_PRIVATE_TASK: task is outside the caller's board view");
            }
        }
    }
    match command {
        BoardCommand::Show => handle_board_show(server, Some(&actor)),
        BoardCommand::Publish { id, title, description, delivery_condition, test_condition, priority,
            work_kind,parent_task,prefer_role,executor,reviewer,depends_on,long_horizon } => {
            let long_horizon = match long_horizon.merge(None) {
                Ok(spec) => spec, Err(error) => return Resp::err(error),
            };
            if [&id, &title, &description, &delivery_condition, &test_condition].iter().any(|text| text.trim().is_empty()) {
                return Resp::err("BOARD_DESCRIPTION_REQUIRED: provide task id, title, description, delivery condition and test condition");
            }
            if !["p0", "p1", "p2", "p3", "p4"].contains(&priority.as_str()) {
                return Resp::err("invalid priority; expected p0, p1, p2, p3 or p4");
            }
            let mut state = server.state.lock().unwrap();
            if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
            if state.tasks.contains_key(&id) { return Resp::err("task id already exists"); }
            if long_horizon.is_some() {
                if let Err(error) = board_check_master_locked(server, &state, &actor) { return error; }
            }
            if let Some(parent)=parent_task.as_deref() {
                let Some(task)=state.tasks.get(parent) else {return Resp::err("WORK_PARENT_NOT_FOUND");};
                if !board_task_visible(&state,task,Some(&actor)) {return Resp::err("BOARD_PRIVATE_TASK");}
                if parent==id {return Resp::err("WORK_PARENT_CYCLE");}
            }
            for dep in depends_on.iter() {
                if dep == &id { return Resp::err("BOARD_TICKET_SELF_DEPENDENCY: a ticket cannot depend on itself"); }
                if !state.tasks.contains_key(dep) {
                    return Resp::err_data("BOARD_TICKET_DEPENDENCY_NOT_FOUND: dependency task does not exist", json!({"task_id": id, "depends_on": dep}));
                }
            }
            let explicit=match parse_work_policy(work_kind,prefer_role,executor,reviewer,false,false,false) {
                Ok(policy)=>policy,Err(error)=>return error,
            };
            let work_policy=state.materialize_work_policy(explicit.work_kind.as_deref(),parent_task.as_deref(),&explicit);
            let public_visibility = !is_managed_subagent(&state, &actor);
            let now = now_ms();
            let task = TaskRec {
                id: id.clone(), owner: actor.clone(), created_by: actor.clone(),
                feature_id: None, worktree_path: None, branch: None, base_commit: None,
                priority, status: "pending".into(), next_step: None, wait: None,
                isolation_mode: None,
                created_ms: now, updated_ms: now,
            };
            let details = BoardTaskDetails {
                title, description, delivery_condition, test_condition, long_horizon,
                parent_task_id:parent_task,work_policy,
                revision: 1, public_visibility, admitted: public_visibility, ..BoardTaskDetails::default()
            };
            let mut events = vec![
                Event::TaskCreated { task: task.clone() },
                Event::BoardDetailsChanged { task_id: id.clone(), details: details.clone() },
            ];
            if details.public_visibility {
                match board_ticket_admission_events(server, &state, &task, &details, depends_on) {
                    Ok(admission) => events.extend(admission),
                    Err(error) => return error,
                }
            }
            if let Err(error) = server.commit_locked_checked(&mut state, &events) {
                return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}"));
            }
            let mut response = board_task_response(server, &state, &id, &actor);
            if let Some(object) = response.data.as_object_mut() {
                object.insert("ticket".into(), ticket_projection(&state, &id));
            }
            response
        },
        BoardCommand::Claim { id, expected_revision } => board_claim(server, actor, token, id, expected_revision),
        BoardCommand::Start { id, expected_revision, attempt } => board_start(server, actor, token, id, expected_revision, attempt),
        BoardCommand::Update { id, expected_revision, status, next } => {
            let mut state = server.state.lock().unwrap();
            if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
            if let Err(error) = board_check_revision(&state, &id, expected_revision) { return error; }
            if status.is_none() && next.is_none() { return Resp::err("provide --status or --next"); }
            let result = task_update_locked(server, &mut state, actor.clone(), token, id.clone(), status, next, None, None);
            if !result.ok { return result; }
            let block_request = result.data.get("block_request").cloned();
            let mut response = board_task_response(server, &state, &id, &actor);
            if let (Some(request), Some(object)) = (block_request, response.data.as_object_mut()) {
                object.insert("block_request".into(), request);
            }
            response
        },
        BoardCommand::Describe { id, expected_revision, title, description, delivery_condition, test_condition, long_horizon } => {
            let mut state = server.state.lock().unwrap();
            if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
            let Some(mut task) = state.tasks.get(&id).cloned() else { return Resp::err("task not found"); };
            if task.owner != actor && board_check_master_locked(server, &state, &actor).is_err() { return Resp::err("only the task owner or current master may describe it"); }
            let Some(mut details) = state.board_details.get(&id).cloned() else { return Resp::err("task description record missing"); };
            if details.revision != expected_revision { return Resp::err("BOARD_STALE_REVISION: reread the board"); }
            let long_horizon = match long_horizon.merge(details.long_horizon.as_ref()) {
                Ok(spec) => spec, Err(error) => return Resp::err(error),
            };
            if [&title, &description, &delivery_condition, &test_condition].iter().any(|text| text.trim().is_empty()) { return Resp::err("description and conditions must not be empty"); }
            if task.status == "invited" && (details.title != title || details.description != description
                || details.delivery_condition != delivery_condition || details.test_condition != test_condition) {
                return Resp::err("withdraw the invitation before changing its contract");
            }
            if matches!(task.status.as_str(),"merged"|"closed"|"cancelled") {return Resp::err("WORK_CONTRACT_TERMINAL: create a follow-up task");}
            let changed=details.title!=title || details.description!=description
                || details.delivery_condition!=delivery_condition || details.test_condition!=test_condition
                || details.long_horizon!=long_horizon;
            let mut events=Vec::new();
            if changed {
                if let Some(mut record)=state.task_lifecycle.get(&id).cloned() {
                    events.extend(invalidate_work_review(&state,&task,&mut record,details.work_policy.reviewer_name.clone()));
                    events.push(Event::TaskLifecycleUpdated {task_id:id.clone(),record});
                    if matches!(task.status.as_str(),"accepted"|"delivered") {task.status="rework".into();}
                }
            }
            details.title = title; details.description = description;
            details.delivery_condition = delivery_condition; details.test_condition = test_condition;
            details.long_horizon = long_horizon;
            // Editing a claimed public task does not make it a private child assignment.
            details.public_visibility |= !is_managed_subagent(&state, &actor); details.revision += 1;
            task.updated_ms=now_ms();
            events.extend([Event::TaskUpdated {task},Event::BoardDetailsChanged { task_id: id.clone(), details }]);
            if let Some(event) = ticket_rework_phase_event(server, &state, &id) {
                events.push(event);
            }
            if let Err(error) = server.commit_locked_checked(&mut state, &events) {
                return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}"));
            }
            board_task_response(server, &state, &id, &actor)
        },
        BoardCommand::Invite { id, to, expected_revision } => board_invite(server, actor, token, id, to, expected_revision),
        BoardCommand::Respond { id, accept, decline, expected_revision, reason } => {
            if accept == decline { return Resp::err("choose exactly one of accept or decline"); }
            board_respond(server, actor, token, id, expected_revision, accept, reason)
        },
        BoardCommand::DeclineAssigned { id, expected_revision, reason } => board_decline_assigned(server, actor, token, id, expected_revision, reason),
        BoardCommand::Withdraw { id, expected_revision, reason } => board_withdraw(server, actor, token, id, expected_revision, reason),
        BoardCommand::Dispatch {id,expected_revision,long_horizon} => board_dispatch_work(server,actor,token,id,expected_revision,long_horizon),
        BoardCommand::Policy {scope,id,work_kind,prefer_role,executor,reviewer,clear_prefer_role,clear_executor,clear_reviewer,expected_revision} => {
            let patch=match parse_work_policy(work_kind,prefer_role,executor,reviewer,clear_prefer_role,clear_executor,clear_reviewer) {
                Ok(policy)=>policy,Err(error)=>return error,
            };
            board_work_policy(server,actor,token,scope,id,patch,expected_revision)
        },
    }
}
