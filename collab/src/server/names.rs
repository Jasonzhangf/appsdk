#[path = "name_resolution.rs"]
pub(crate) mod name_resolution;

#[path = "name_mailbox.rs"]
pub(crate) mod name_mailbox;

#[cfg(test)]
#[path = "name_mailbox_tests.rs"]
mod name_mailbox_tests;

#[cfg(test)]
#[path = "name_resolution_tests.rs"]
mod name_resolution_tests;

#[cfg(test)]
#[path = "name_presence_lock_tests.rs"]
mod name_presence_lock_tests;

/// Current occupancy wins over compatibility labels. Internal worker IDs stay
/// unchanged for wire routing. A duplicate old label cannot impersonate a holder.
fn worker_display_name(state: &State, id: &str, is_master: bool) -> String {
    let current: Vec<_> = state
        .name_occupancies
        .values()
        .flat_map(|names| names.values())
        .filter(|holder| holder.worker_id == id)
        .collect();
    let scoped = state
        .workers
        .get(id)
        .and_then(|worker| state.name_occupancies.get(&worker.cwd))
        .and_then(|names| names.values().find(|holder| holder.worker_id == id));
    if let Some(holder) = scoped.or_else(|| (current.len() == 1).then(|| current[0])) {
        return holder.name.clone();
    }
    if let Some(name) = state.worker_names.get(id) {
        if !state
            .name_occupancies
            .values()
            .any(|names| names.contains_key(name))
        {
            return name.clone();
        }
    }
    if is_master {
        return "Jenny".into();
    }
    state
        .workers
        .get(id)
        .and_then(|worker| worker.transport.as_ref())
        .and_then(|transport| transport.tmux_endpoint.as_ref())
        .map(|pane| pane.pane_id.clone())
        .unwrap_or_else(|| id.into())
}

fn handle_worker_name(server: &Server, actor: String, token: String, name: Option<String>) -> Resp {
    let name = match name {
        Some(name) => {
            if name.chars().any(char::is_control) || name.trim().is_empty() {
                return Resp::err(
                    "WORKER_NAME_INVALID: provide a nonempty name without control characters",
                );
            }
            Some(name.trim().to_owned())
        }
        None => None,
    };
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, &actor, &token) {
        return error;
    }
    if state
        .name_occupancies
        .values()
        .any(|names| names.values().any(|holder| holder.worker_id == actor))
    {
        return Resp::err_data(
            "WORKER_NAME_RESTORE_REQUIRED: current work names are controlled by collab context",
            json!({"next_invocation":"collab context --approve-grant '{\"action\":\"restore_name\",\"requested_name\":\"<name>\",\"authorization\":\"<user authorization>\"}'"}),
        );
    }
    let route = match server_route_scope(server, &state) {
        Ok(route) => route,
        Err(error) => return Resp::err(error),
    };
    if state.worker_names.get(&actor) != name.as_ref() {
        if let Err(error) = server.commit_locked_checked(
            &mut state,
            &[Event::WorkerNamed {
                worker_id: actor.clone(),
                name,
            }],
        ) {
            return Resp::err(format!("WORKER_NAME_DURABILITY_FAILED: {error}"));
        }
    }
    let master = current_master_worker_id(&state, route.as_ref());
    Resp::data(json!({
        "worker_id": actor,
        "display_name": worker_display_name(&state, &actor, master.as_deref() == Some(actor.as_str())),
    }))
}
