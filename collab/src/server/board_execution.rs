/// Final public work-start gate shared by board acceptance and legal legacy
/// assignments. No caller may start a peer's work by changing status alone.
fn board_execution_gate(server: &Server, state: &State, actor: &str, task_id: &str) -> Result<RuntimeBinding, Resp> {
    board_resume_gate(state, actor, task_id)?;
    let route = server_route_scope(server, state).map_err(Resp::err)?
        .ok_or_else(|| Resp::err("BOARD_ROUTE_REQUIRED: public execution requires a unique project route"))?;
    let binding = authoritative_send_binding(state, &route, actor).map_err(Resp::err)?.clone();
    Ok(binding)
}

/// A blocked owner may resume only after its other active obligation ends.
/// Resource ownership remains intact while blocked and is checked here again.
fn board_resume_gate(state: &State, actor: &str, task_id: &str) -> Result<(), Resp> {
    if board_peer_has_execution_reservation(state, actor, Some(task_id)) {
        return Err(Resp::err("BOARD_PEER_BUSY: finish or decline your other responsibility before accepting"));
    }
    if state.worktree_bindings.values().any(|resource| resource.task_id == task_id && resource.owner_agent_id != actor) {
        return Err(Resp::err("BOARD_RESOURCE_OWNER_CONFLICT: assignment resource belongs to another actor"));
    }
    Ok(())
}

fn board_claim(server: &Server, actor: String, token: String, id: String, expected_revision: u64) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
    let mut details = match board_check_revision(&state, &id, expected_revision) { Ok(details) => details, Err(error) => return error };
    let Some(mut task) = state.tasks.get(&id).cloned() else { return Resp::err("task not found"); };
    if !details.public_visibility && task.owner != actor {
        return Resp::err("BOARD_PRIVATE_TASK: private queue work is claimable only by its publisher");
    }
    if task.status != "pending" || task.owner != task.created_by {
        return Resp::err_data(
            "BOARD_TASK_NOT_PENDING: only pending publisher-owned work may be claimed",
            json!({"task_id": id, "status": task.status}),
        );
    }
    if let Err(error) = board_unstarted_resources(&state, &task) { return error; }
    if let Err(error) = board_execution_gate(server, &state, &actor, &id) { return error; }
    task.owner = actor.clone();
    task.status = "working".into();
    task.updated_ms = now_ms();
    details.revision += 1;
    if let Err(error) = server.commit_locked_checked(&mut state, &[
        Event::TaskUpdated { task },
        Event::BoardDetailsChanged { task_id: id.clone(), details },
    ]) { return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}")); }
    board_task_response(server, &state, &id, &actor)
}

fn board_decline_assigned(server: &Server, actor: String, token: String, id: String, expected_revision: u64, reason: String) -> Resp {
    if reason.trim().is_empty() { return Resp::err("decline requires a nonempty reason"); }
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
    let mut details = match board_check_revision(&state, &id, expected_revision) { Ok(details) => details, Err(error) => return error };
    let Some(mut task) = state.tasks.get(&id).cloned() else { return Resp::err("task not found"); };
    if task.owner != actor || task.status != "assigned" { return Resp::err("only the owner may reject an unstarted legacy assignment"); }
    let Some(admission) = state.scheduler_admissions.values().find(|admission| admission.task_id == id && admission.status == "succeeded" && admission.managed_subagent_id.is_none()).cloned() else {
        return Resp::err("legacy scheduler assignment provenance is missing");
    };
    if let Err(error) = board_unstarted_resources(&state, &task) { return error; }
    task.owner = task.created_by.clone(); task.status = "pending".into(); task.updated_ms = now_ms();
    details.revision += 1; details.last_response = Some(format!("legacy assignment declined by {actor}: {reason}"));
    if let Err(error) = server.commit_locked_checked(&mut state, &[
        Event::TaskUpdated { task }, Event::BoardDetailsChanged { task_id: id.clone(), details },
        Event::Superseded { ids: vec![admission.message_id] },
    ]) { return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}")); }
    board_task_response(server, &state, &id, &actor)
}
