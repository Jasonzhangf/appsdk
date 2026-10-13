fn handle_worker_close(
    server: &Server,
    worker_id: String,
    token: String,
    target_id: String,
    reason: String,
) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify_master_actor(server, &state, &worker_id, &token) {
        return error;
    }
    if reason.trim().is_empty() {
        return Resp::err("worker close requires a non-empty --reason");
    }
    if target_id == worker_id {
        return Resp::err("master cannot close itself; delegate first");
    }
    let Some(target) = state.workers.get(&target_id).cloned() else {
        let Some(closed) = state.worker_closures.get(&target_id).cloned() else {
            return Resp::err(format!("target worker {} not registered", target_id));
        };
        return Resp::data(json!({
            "closed": closed.worker_id,
            "closed_by": closed.closed_by,
            "reason": closed.reason,
            "snapshot_captured_ms": closed.snapshot_captured_ms,
            "at_ms": closed.at_ms,
            "reused": true,
        }));
    };
    let route_scope = match server_route_scope(server, &state) {
        Ok(Some(route_scope)) => route_scope,
        Ok(None) => return Resp::err("worker close requires an exact project route"),
        Err(error) => return Resp::err(error),
    };
    let binding_id = match BindingId::new(sanitize_identifier(&format!("binding-{target_id}"))) {
        Ok(binding_id) => binding_id,
        Err(error) => return Resp::err(format!("WORKER_CLOSE_INVALID_BINDING: {error}")),
    };
    let binding = match state
        .global
        .lookup_binding_for(&route_scope, &binding_id)
        .filter(|binding| binding.agent_id.as_str() == target_id)
        .cloned()
    {
        Some(binding) => binding,
        None => {
            return Resp::err(format!(
                "worker {target_id} has no exact runtime binding in the current project route"
            ))
        }
    };
    let observed_target = close_endpoint_target(&state, &target_id);
    drop(state);
    let endpoint_presence = close_endpoint_presence(server, &observed_target);
    let mut state = server.state.lock().unwrap();
    if close_endpoint_target(&state, &target_id) != observed_target {
        return Resp::err("WORKER_CLOSE_TARGET_CHANGED: observed worker or binding changed");
    }
    if let Err(error) = verify_master_actor(server, &state, &worker_id, &token) {
        return error;
    }
    let current_route_scope = match server_route_scope(server, &state) {
        Ok(scope) => scope,
        Err(error) => return Resp::err(error),
    };
    if current_route_scope.as_ref() != Some(&route_scope) {
        return Resp::err("WORKER_CLOSE_ROUTE_CHANGED: current project route changed");
    }
    // Closing a worker that still owns live work would strand the task and its
    // worktree. The task lifecycle must be resolved first.
    let owned: Vec<String> = state
        .tasks
        .values()
        .filter(|task| task.owner == target_id && keepalive::unfinished(&task.status))
        .map(|task| task.id.clone())
        .collect();
    if !owned.is_empty() {
        return Resp::err(format!(
            "worker {} still owns {}; close or force-close the task first",
            target_id,
            owned.join(", ")
        ));
    }

    let target_thread_id = target
        .transport
        .as_ref()
        .and_then(|transport| transport.thread_id.as_deref());
    let snapshot_captured_ms = target_thread_id.and_then(|thread_id| {
        state
            .worker_snapshots
            .get(&target_id)
            .filter(|receipt| receipt.thread_id == thread_id)
            .map(|receipt| receipt.captured_ms)
    });
    let snapshot_captured_ms = match (endpoint_presence, snapshot_captured_ms) {
        (_, Some(captured_ms)) => Some(captured_ms),
        // A definitively Missing address has no live thread to snapshot. The
        // master may retire the registration without fabricating evidence.
        (IdentityPresence::Missing, None) if target.transport.is_some() => None,
        (_, None) if target_thread_id.is_none() => {
            return Resp::err(format!(
                "worker {} has no bound App Server thread; snapshot evidence is unavailable",
                target_id
            ))
        }
        (_, None) => {
            return Resp::err(format!(
                "worker {} requires a successful worker snapshot for its bound App Server thread before close",
                target_id
            ))
        }
    };

    let now = now_ms();
    let operation_id = format!(
        "close-{}-{}-{}",
        sanitize_identifier(&binding.binding_id.as_str()),
        binding.endpoint_generation,
        sanitize_identifier(&worker_id)
    );
    let fence = Event::ResponsibilityFenceSet {
        fence: state::ResponsibilityFence {
            operation_id: operation_id.clone(),
            worker_id: target_id.clone(),
            project_scope: binding.project_scope.as_str().to_owned(),
            app_scope: binding.app_scope_id.as_str().to_owned(),
            binding_id: binding.binding_id.as_str().to_owned(),
            endpoint_generation: binding.endpoint_generation,
            snapshot: state::responsibility_snapshot(&state, &target_id),
            state: "closing".into(),
            created_ms: now,
        },
    };
    let close_event = Event::WorkerClosed {
        worker_id: target_id.clone(),
        closed_by: worker_id.clone(),
        reason: reason.clone(),
        snapshot_captured_ms,
        at_ms: now,
    };
    // The fence is durable before the close record. The record itself is a
    // safe target-owned retirement while the fence stays active for a later
    // lifecycle owner that must prove host terminal and exact readback.
    if let Err(error) = server.commit_locked(&mut state, &[fence, close_event]) {
        drop(state);
        return Resp::err(format!("WORKER_CLOSE_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "closed": target_id,
        "closed_by": worker_id,
        "reason": reason,
        "snapshot_captured_ms": snapshot_captured_ms,
        "transport": target.transport,
    }))
}
