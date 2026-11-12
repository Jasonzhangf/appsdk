/// Final public work-start gate shared by board acceptance and legal legacy
/// assignments. No caller may start a peer's work by changing status alone.
fn board_execution_gate(server: &Server, state: &State, actor: &str, task_id: &str) -> Result<RuntimeBinding, Resp> {
    work_executor_gate(server,state,task_id,actor)?;
    board_execution_gate_after_executor(server, state, actor, task_id)
}

fn board_execution_gate_with_control(
    server: &Server,
    state: &State,
    control: &binding_slot::BindingControlSet,
    actor: &str,
    task_id: &str,
) -> Result<RuntimeBinding, Resp> {
    let route = server_route_scope(server, state)
        .map_err(Resp::err)?
        .ok_or_else(|| Resp::err("BOARD_ROUTE_REQUIRED: public execution requires a unique project route"))?;
    work_executor_gate_with_control(state, control, &route, task_id, actor)?;
    board_execution_gate_after_executor(server, state, actor, task_id)
}

fn board_execution_gate_after_executor(
    server: &Server,
    state: &State,
    actor: &str,
    task_id: &str,
) -> Result<RuntimeBinding, Resp> {
    board_resume_gate(state, actor, task_id)?;
    let route = server_route_scope(server, state).map_err(Resp::err)?
        .ok_or_else(|| Resp::err("BOARD_ROUTE_REQUIRED: public execution requires a unique project route"))?;
    let binding = authoritative_send_binding(state, &route, actor).map_err(Resp::err)?.clone();
    Ok(binding)
}

/// Peers orchestrate multiple goals. Admission checks the task's actual
/// resource ownership, rather than treating another goal as a busy slot.
pub(crate) fn board_resume_gate(
    state: &State,
    actor: &str,
    task_id: &str,
) -> Result<(), Resp> {
    if state.worktree_bindings.values().any(|resource| resource.task_id == task_id && resource.owner_agent_id != actor) {
        return Err(Resp::err("BOARD_RESOURCE_OWNER_CONFLICT: assignment resource belongs to another actor"));
    }
    if state.pending_merges.get(task_id).is_some_and(|request| request.owner != actor) {
        return Err(Resp::err("BOARD_RESOURCE_OWNER_CONFLICT: pending merge belongs to another actor"));
    }
    if let Some(task) = state.tasks.get(task_id) {
        if state.tasks.values().any(|other| other.id != task_id && task_resource_active(&other.status)
            && (task.feature_id.is_some() && task.feature_id == other.feature_id
                || task.worktree_path.is_some() && task.worktree_path == other.worktree_path)) {
            return Err(Resp::err("TASK_RESOURCE_CONFLICT: another active task owns this feature/worktree"));
        }
    }
    Ok(())
}

fn ticket_stale_binding_error(
    state: &State,
    task_id: &str,
    slot_name: &str,
    current_generation: u64,
    attempt: u64,
) -> Option<Resp> {
    use crate::server::ticket_state::{TicketPhaseKind, TicketPhaseStatus};

    state
        .ticket_phases
        .iter()
        .find(|phase| {
            phase.task_id == task_id
                && phase.phase == TicketPhaseKind::Execute
                && phase.slot_name.as_deref() == Some(slot_name)
                && phase.attempt_generation == attempt
                && phase.binding_generation > 0
                && phase.binding_generation < current_generation
                && matches!(
                    phase.status,
                    TicketPhaseStatus::Claimed
                        | TicketPhaseStatus::Running
                        | TicketPhaseStatus::Revoked
                )
        })
        .map(|phase| {
            Resp::err_data(
                "TICKET_STALE_BINDING: the attempt belongs to an older identity generation",
                json!({
                    "task_id": task_id,
                    "attempt": attempt,
                    "claimed_generation": phase.binding_generation,
                    "current_generation": current_generation,
                }),
            )
        })
}

/// Resolve the authenticated worker's current binding slot and, when its
/// declared attempt still matches an older generation, return the exact stale
/// binding error. `Ok(None)` means the current scope/slot is absent and no
/// stale diagnosis can be made; control/route read errors stay explicit.
pub(crate) fn current_ticket_stale_binding_error(
    server: &Server,
    state: &State,
    actor: &str,
    task_id: &str,
    attempt: u64,
) -> Result<Option<Resp>, Resp> {
    let route = match server_route_scope(server, state) {
        Ok(Some(route)) => route,
        Ok(None) => return Ok(None),
        Err(error) => return Err(Resp::err(error)),
    };
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
        Ok(control) => control,
        Err(error) => return Err(Resp::err(error)),
    };
    let slot = match control.current_for_worker(
        &route.project_scope_id,
        &route.app_scope_id,
        actor,
    ) {
        Ok(Some(slot)) => slot,
        Ok(None) => return Ok(None),
        Err(error) => return Err(Resp::err(error)),
    };
    Ok(ticket_stale_binding_error(
        state,
        task_id,
        &slot.identity_key.name,
        slot.generation,
        attempt,
    ))
}

fn board_claim(server: &Server, actor: String, token: String, id: String, expected_revision: u64) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
    let mut details = match board_check_revision(&state, &id, expected_revision) { Ok(details) => details, Err(error) => return error };
    let Some(mut task) = state.tasks.get(&id).cloned() else { return Resp::err("task not found"); };
    if !details.public_visibility && task.owner != actor {
        return Resp::err("BOARD_PRIVATE_TASK: private queue work is claimable only by its publisher");
    }
    let ordinary_pending = task.status == "pending" && task.owner == task.created_by;
    let not_pending_error = Resp::err_data(
        "BOARD_TASK_NOT_PENDING: only pending publisher-owned work may be claimed",
        json!({"task_id": id, "status": task.status}),
    );
    let claimable;
    let name_session;
    if ordinary_pending {
        if let Err(error) = board_unstarted_resources(&state, &task) { return error; }
        let binding = match board_execution_gate(server, &state, &actor, &id) {
            Ok(binding) => binding, Err(error) => return error,
        };
        let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
            Ok(control) => control, Err(error) => return Resp::err(error),
        };
        name_session = match control.current_for_worker(&binding.project_scope, &binding.app_scope_id, &actor) {
            Ok(slot) => slot.and_then(|slot| slot.active_binding().and_then(|current|
                current.binding.session_id.as_ref().map(|session| (
                    slot.identity_key.name.clone(),
                    session.to_string(),
                    slot.generation,
                )))),
            Err(error) => return Resp::err(error),
        };
        // An admitted ticket's pending phase is its queue position. A peer may
        // only advance the phase queued on its own slot; a WaitingAssignment
        // phase has no slot and stays open to any claimer. The current manager
        // may still pull another slot's queued ticket, and that claim moves the
        // phase to the manager's slot so the phase and the task never disagree.
        let admitted_public = state
            .board_details
            .get(&id)
            .is_some_and(|details| details.admitted);
        let queued_phase = state
            .ticket_phases
            .iter()
            .find(|phase| {
                phase.task_id == id
                    && phase.phase == crate::server::ticket_state::TicketPhaseKind::Execute
                    && matches!(
                        phase.status,
                        crate::server::ticket_state::TicketPhaseStatus::Queued
                            | crate::server::ticket_state::TicketPhaseStatus::WaitingAssignment
                    )
            })
            .cloned();
        let slot_claim = name_session
            .as_ref()
            .and_then(|(slot_name, _, _)| {
                queued_phase.as_ref().filter(|phase| {
                    phase.status == crate::server::ticket_state::TicketPhaseStatus::WaitingAssignment
                        || phase.slot_name.as_deref() == Some(slot_name.as_str())
                })
            })
            .cloned();
        let manager_claim = matches!(
            current_master_holder(server, &state),
            Ok(Some(master)) if master == actor
        );
        claimable = slot_claim.or_else(|| manager_claim.then_some(queued_phase).flatten());
        if admitted_public && claimable.is_none() {
            return Resp::err_data(
                "TICKET_SLOT_MISMATCH: pending ticket is queued on another slot",
                json!({"task_id": id}),
            );
        }
    } else {
        // Only the exact recovered working+Queued responsibility may bypass the
        // pending publisher-owned claim rule. This branch has no manager
        // override: the current stable-name carrier is the sole claimant.
        if !state
            .board_details
            .get(&id)
            .is_some_and(|details| details.admitted)
        {
            return not_pending_error;
        }
        let route = match server_route_scope(server, &state) {
            Ok(Some(route)) => route,
            Ok(None) => return not_pending_error,
            Err(error) => return Resp::err(error),
        };
        let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
            Ok(control) => control, Err(error) => return Resp::err(error),
        };
        let slot = match control.current_for_worker(&route.project_scope_id, &route.app_scope_id, &actor) {
            Ok(Some(slot)) => slot,
            Ok(None) => return not_pending_error,
            Err(error) => return Resp::err(error),
        };
        let Some(current) = slot.active_binding() else {
            return not_pending_error;
        };
        let Some(session) = current.binding.session_id.as_ref() else {
            return not_pending_error;
        };
        let slot_name = slot.identity_key.name.clone();
        let binding_generation = slot.generation;
        let Some(recovered) = ticket_recovery_claim_phase(
            &state,
            &slot_name,
            session.as_str(),
            &id,
            &actor,
            binding_generation,
        )
        .or_else(|| {
            ticket_stale_active_claim_phase(
                &state,
                &slot_name,
                session.as_str(),
                &id,
                &actor,
                binding_generation,
            )
        }) else {
            return not_pending_error;
        };
        if let Err(error) = board_execution_gate(server, &state, &actor, &id) {
            return error;
        }
        claimable = Some(recovered);
        name_session = Some((slot_name, session.to_string(), binding_generation));
    }
    task.owner = actor.clone();
    task.status = "working".into();
    task.updated_ms = now_ms();
    details.revision += 1;
    let mut events = vec![
        Event::TaskUpdated { task },
        Event::BoardDetailsChanged { task_id: id.clone(), details },
    ];
    // Advance the admitted execute phase with the same claim. Legacy TaskRec
    // status stays "working" for old entry compatibility; the phase record is
    // the ticket-level claim fact consumed by queue projections.
    if let (Some(phase), Some((slot_name, _, binding_generation))) =
        (claimable, name_session.as_ref())
    {
        let mut phase = phase;
        if matches!(
            phase.status,
            crate::server::ticket_state::TicketPhaseStatus::Claimed
                | crate::server::ticket_state::TicketPhaseStatus::Running
        ) {
            let mut revoked = phase.clone();
            revoked.status = crate::server::ticket_state::TicketPhaseStatus::Revoked;
            revoked.wait_reason = None;
            events.push(Event::TicketPhaseRecorded { phase: revoked });
            phase.batch = match phase.batch.checked_add(1) {
                Some(batch) => batch,
                None => {
                    return Resp::err_data(
                        "TICKET_BATCH_EXHAUSTED: cannot allocate a recovery batch",
                        json!({"task_id": id, "batch": phase.batch}),
                    )
                }
            };
            phase.result = None;
        }
        let mut claimed = phase;
        claimed.status = crate::server::ticket_state::TicketPhaseStatus::Claimed;
        claimed.slot_name = Some(slot_name.clone());
        claimed.binding_generation = *binding_generation;
        claimed.attempt_generation = match claimed.attempt_generation.checked_add(1) {
            Some(attempt) => attempt,
            None => {
                return Resp::err_data(
                    "TICKET_ATTEMPT_EXHAUSTED: cannot allocate a new claim attempt",
                    json!({"task_id": id, "attempt": claimed.attempt_generation}),
                )
            }
        };
        claimed.wait_reason = None;
        events.push(Event::TicketPhaseRecorded { phase: claimed });
    }
    if let Some((name, session_id, _)) = name_session {
        events.push(Event::TaskAssignmentUpdated { assignment: name_state::TaskAssignment {
            task_id: id.clone(), name, session_id,
            assignment_revision: state.task_assignment(&id).map_or(1, |assignment| assignment.assignment_revision + 1),
            reflow: None, updated_ms: now_ms(),
        } });
    }
    if let Err(error) = server.commit_locked_checked(&mut state, &events) { return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}")); }
    board_task_response(server, &state, &id, &actor)
}

fn board_start(
    server: &Server,
    actor: String,
    token: String,
    id: String,
    expected_revision: u64,
    attempt: u64,
) -> Resp {
    let mut state = server.state.lock().unwrap();
    board_start_locked(server, &mut state, None, actor, token, id, expected_revision, attempt)
}

fn board_start_locked(
    server: &Server,
    state: &mut State,
    control: Option<&binding_slot::BindingControlSet>,
    actor: String,
    token: String,
    id: String,
    expected_revision: u64,
    attempt: u64,
) -> Resp {
    if let Err(error) = board_check_actor(&state, &actor, &token) {
        return error;
    }
    match current_ticket_stale_binding_error(server, &state, &actor, &id, attempt) {
        Ok(Some(error)) => return error,
        Ok(None) => {}
        Err(error) => return error,
    }
    if let Err(error) = board_check_revision(&state, &id, expected_revision) {
        return error;
    }
    let Some(task) = state.tasks.get(&id).cloned() else {
        return Resp::err("task not found");
    };
    if let Err(error) =
        task_owner_gate_maybe_with_control(server, state, control, &id, &actor)
    {
        return error;
    }
    let binding_result = match control {
        Some(control) => board_execution_gate_with_control(server, state, control, &actor, &id),
        None => board_execution_gate(server, state, &actor, &id),
    };
    let binding = match binding_result {
        Ok(binding) => binding,
        Err(error) => return error,
    };
    let loaded;
    let control = match control {
        Some(control) => control,
        None => {
            loaded = match binding_store::BindingControlStore::new(server.host_paths.state_root())
                .load()
            {
                Ok(control) => control,
                Err(error) => return Resp::err(error),
            };
            &loaded
        }
    };
    let slot = match control.current_for_worker(
        &binding.project_scope,
        &binding.app_scope_id,
        &actor,
    ) {
        Ok(Some(slot)) => slot,
        Ok(None) => {
            return Resp::err("TICKET_SLOT_MISSING: current identity has no execution slot")
        }
        Err(error) => return Resp::err(error),
    };
    let Some(current) = slot.active_binding() else {
        return Resp::err("TICKET_SLOT_INACTIVE: current identity binding is not active");
    };
    if current.binding.session_id.is_none() {
        return Resp::err("TICKET_SESSION_MISSING: current identity binding has no session");
    }
    let slot_name = slot.identity_key.name.clone();
    let Some(phase) = state.ticket_phases.iter().find(|phase| {
        phase.task_id == id
            && phase.phase == crate::server::ticket_state::TicketPhaseKind::Execute
            && phase.status == crate::server::ticket_state::TicketPhaseStatus::Claimed
            && phase.slot_name.as_deref() == Some(slot_name.as_str())
    }) else {
        return Resp::err("TICKET_PHASE_NOT_CLAIMED: no claimed execute phase belongs to this slot");
    };
    if phase.attempt_generation != attempt {
        return Resp::err_data(
            "TICKET_STALE_ATTEMPT: only the current attempt may start",
            json!({
                "task_id": id,
                "attempt": attempt,
                "current_attempt": phase.attempt_generation,
            }),
        );
    }
    if phase.binding_generation != slot.generation {
        return Resp::err_data(
            "TICKET_STALE_BINDING: the claim belongs to an older identity generation",
            json!({
                "task_id": id,
                "claimed_generation": phase.binding_generation,
                "current_generation": slot.generation,
            }),
        );
    }
    let mut running = phase.clone();
    running.status = crate::server::ticket_state::TicketPhaseStatus::Running;
    running.wait_reason = None;
    let events = [Event::TicketPhaseRecorded { phase: running }];
    let commit = server
        .commit_locked_checked_with_control(state, &events, control)
        .map(|_| ())
        .map_err(|error| error.to_string());
    if let Err(error) = commit {
        return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}"));
    }
    let mut response = board_task_response(server, &state, &id, &actor);
    if let Some(object) = response.data.as_object_mut() {
        object.insert("ticket".into(), ticket_projection(&state, &id));
    }
    response
}

fn board_ticket_result_event(
    server: &Server,
    state: &State,
    actor: &str,
    task_id: &str,
    evidence: &str,
    attempt: Option<u64>,
) -> Result<Option<Event>, Resp> {
    use crate::server::ticket_state::{
        TicketPhaseKind, TicketPhaseRecord, TicketPhaseResult, TicketPhaseStatus,
    };

    // Legacy task-register entries and private child assignments never write a
    // ticket phase; keep the old silent delivery path for them. An admitted
    // public ticket atomically records an execute phase at publish, so a missing
    // phase must not be mistaken for completion or a legacy entry.
    // BoardTaskDetails.admitted is the authoritative admission marker; it is set
    // only in the publish batch that also creates the execute phase.
    let admitted_public = state
        .board_details
        .get(task_id)
        .is_some_and(|details| details.admitted);

    if admitted_public {
        if let Some(attempt) = attempt {
            match current_ticket_stale_binding_error(server, state, actor, task_id, attempt) {
                Ok(Some(error)) => return Err(error),
                Ok(None) => {}
                Err(error) => return Err(error),
            }
        }
    }

    let Some(phase) = state
        .ticket_phases
        .iter()
        .filter(|phase| {
            phase.task_id == task_id
                && phase.phase == TicketPhaseKind::Execute
                && phase.status != TicketPhaseStatus::Revoked
        })
        .max_by_key(|phase| phase.batch)
    else {
        if admitted_public {
            return Err(Resp::err_data(
                "TICKET_PHASE_MISSING: admitted public ticket has no execute phase record",
                json!({"task_id": task_id}),
            ));
        }
        return Ok(None);
    };
    let Some(attempt) = attempt else {
        return Err(Resp::err_data(
            "TICKET_ATTEMPT_REQUIRED: admitted ticket results require --attempt",
            json!({"task_id": task_id, "current_attempt": phase.attempt_generation}),
        ));
    };
    if phase.status == TicketPhaseStatus::Completed {
        return Err(Resp::err_data(
            "TICKET_RESULT_ALREADY_COMMITTED: this execute phase is already complete",
            json!({"task_id": task_id, "attempt": attempt}),
        ));
    }
    if phase.status != TicketPhaseStatus::Running {
        return Err(Resp::err_data(
            "TICKET_PHASE_NOT_RUNNING: submit Started before the result",
            json!({
                "task_id": task_id,
                "status": phase.status,
                "attempt": attempt,
            }),
        ));
    }
    if phase.attempt_generation != attempt {
        return Err(Resp::err_data(
            "TICKET_STALE_ATTEMPT: only the current attempt may submit a result",
            json!({
                "task_id": task_id,
                "attempt": attempt,
                "current_attempt": phase.attempt_generation,
            }),
        ));
    }

    let binding = board_execution_gate(server, state, actor, task_id)?;
    let control =
        match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
            Ok(control) => control,
            Err(error) => return Err(Resp::err(error)),
        };
    let slot = match control.current_for_worker(
        &binding.project_scope,
        &binding.app_scope_id,
        actor,
    ) {
        Ok(Some(slot)) => slot,
        Ok(None) => {
            return Err(Resp::err(
                "TICKET_SLOT_MISSING: current identity has no execution slot",
            ))
        }
        Err(error) => return Err(Resp::err(error)),
    };
    let slot_name = slot.identity_key.name.clone();
    if phase.slot_name.as_deref() != Some(slot_name.as_str()) {
        return Err(Resp::err_data(
            "TICKET_SLOT_MISMATCH: result actor does not hold the claimed slot",
            json!({
                "task_id": task_id,
                "claimed_slot": phase.slot_name,
                "current_slot": slot_name,
            }),
        ));
    }
    if phase.binding_generation != slot.generation {
        return Err(Resp::err_data(
            "TICKET_STALE_BINDING: the result belongs to an older identity generation",
            json!({
                "task_id": task_id,
                "claimed_generation": phase.binding_generation,
                "current_generation": slot.generation,
            }),
        ));
    }
    let subject_revision = state
        .board_details
        .get(task_id)
        .map_or(0, |details| details.revision);
    let mut completed: TicketPhaseRecord = phase.clone();
    completed.status = TicketPhaseStatus::Completed;
    completed.wait_reason = None;
    completed.result = Some(TicketPhaseResult {
        outcome: "pass".into(),
        subject_revision,
        submitted_by: actor.into(),
        submitted_ms: now_ms(),
        evidence_path: Some(evidence.into()),
    });
    Ok(Some(Event::TicketPhaseRecorded { phase: completed }))
}

fn board_decline_assigned(server: &Server, actor: String, token: String, id: String, expected_revision: u64, reason: String) -> Resp {
    if reason.trim().is_empty() { return Resp::err("decline requires a nonempty reason"); }
    let mut state = server.state.lock().unwrap();
    board_decline_assigned_locked(
        server,
        &mut state,
        None,
        actor,
        token,
        id,
        expected_revision,
        reason,
    )
}

fn board_decline_assigned_locked(
    server: &Server,
    state: &mut State,
    control: Option<&binding_slot::BindingControlSet>,
    actor: String,
    token: String,
    id: String,
    expected_revision: u64,
    reason: String,
) -> Resp {
    if reason.trim().is_empty() { return Resp::err("decline requires a nonempty reason"); }
    if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
    let mut details = match board_check_revision(&state, &id, expected_revision) { Ok(details) => details, Err(error) => return error };
    let Some(mut task) = state.tasks.get(&id).cloned() else { return Resp::err("task not found"); };
    if task.status != "assigned" {
        return Resp::err("only the current owner may reject an unstarted legacy assignment");
    }
    if let Err(error) =
        task_owner_gate_maybe_with_control(server, state, control, &id, &actor)
    {
        return error;
    }
    let Some(admission) = state.scheduler_admissions.values().find(|admission| admission.task_id == id && admission.status == "succeeded" && admission.managed_subagent_id.is_none()).cloned() else {
        return Resp::err("legacy scheduler assignment provenance is missing");
    };
    if let Err(error) = board_unstarted_resources(&state, &task) { return error; }
    task.owner = task.created_by.clone(); task.status = "pending".into(); task.updated_ms = now_ms();
    details.revision += 1; details.last_response = Some(format!("legacy assignment declined by {actor}: {reason}"));
    let events = [
        Event::TaskUpdated { task }, Event::BoardDetailsChanged { task_id: id.clone(), details },
        Event::Superseded { ids: vec![admission.message_id] },
    ];
    let commit = match control {
        Some(control) => server
            .commit_locked_checked_with_control(state, &events, control)
            .map(|_| ())
            .map_err(|error| error.to_string()),
        None => server
            .commit_locked_checked(state, &events)
            .map(|_| ())
            .map_err(|error| error.to_string()),
    };
    if let Err(error) = commit { return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}")); }
    board_task_response(server, &state, &id, &actor)
}
