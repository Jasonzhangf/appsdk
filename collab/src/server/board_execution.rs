/// Final public work-start gate shared by board acceptance and legal legacy
/// assignments. No caller may start a peer's work by changing status alone.
fn board_execution_gate(server: &Server, state: &State, actor: &str, task_id: &str) -> Result<RuntimeBinding, Resp> {
    if is_managed_subagent(state, actor) {
        return Err(Resp::err("managed assignments use their private parent protocol"));
    }
    if board_peer_has_responsibility(state, actor, Some(task_id)) {
        return Err(Resp::err("BOARD_PEER_BUSY: finish or decline your other responsibility before accepting"));
    }
    let route = server_route_scope(server, state).map_err(Resp::err)?
        .ok_or_else(|| Resp::err("BOARD_ROUTE_REQUIRED: public execution requires a unique project route"))?;
    let binding = authoritative_send_binding(state, &route, actor).map_err(Resp::err)?.clone();
    if state.worktree_bindings.values().any(|resource| resource.task_id == task_id && resource.owner_agent_id != actor) {
        return Err(Resp::err("BOARD_RESOURCE_OWNER_CONFLICT: assignment resource belongs to another actor"));
    }
    Ok(binding)
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
    board_task_response(&state, &id)
}
