/// Explicit labels are control state; defaults follow the current typed role
/// and bound pane. Stable IDs remain the only routing and ownership keys.
fn worker_display_name(state: &State, id: &str, is_master: bool) -> String {
    if let Some(name) = state.worker_names.get(id) {
        return name.clone();
    }
    if is_master { return "Jenny".into(); }
    state.workers.get(id)
        .and_then(|worker| worker.transport.as_ref())
        .and_then(|transport| transport.tmux_endpoint.as_ref())
        .map(|pane| pane.pane_id.clone())
        .unwrap_or_else(|| id.into())
}

fn handle_worker_name(server: &Server, actor: String, token: String, name: Option<String>) -> Resp {
    let name = match name {
        Some(name) => {
            if name.chars().any(char::is_control) || name.trim().is_empty() {
                return Resp::err("WORKER_NAME_INVALID: provide a nonempty name without control characters");
            }
            Some(name.trim().to_owned())
        },
        None => None,
    };
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, &actor, &token) { return error; }
    let route = match server_route_scope(server, &state) {
        Ok(route) => route, Err(error) => return Resp::err(error),
    };
    if state.worker_names.get(&actor) != name.as_ref() {
        if let Err(error) = server.commit_locked_checked(&mut state, &[
            Event::WorkerNamed { worker_id: actor.clone(), name },
        ]) { return Resp::err(format!("WORKER_NAME_DURABILITY_FAILED: {error}")); }
    }
    let master = current_master_worker_id(&state, route.as_ref());
    Resp::data(json!({
        "worker_id": actor,
        "display_name": worker_display_name(&state, &actor, master.as_deref() == Some(actor.as_str())),
    }))
}
