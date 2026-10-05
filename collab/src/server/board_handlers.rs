use crate::board::{BoardCommand, BoardMemberView, BoardTaskDetails, BoardTaskView};

fn board_public_task(state: &State, task_id: &str) -> bool {
    state.board_details.get(task_id).is_some_and(|details| details.public_visibility)
}

fn board_task_view(state: &State, task: &TaskRec) -> Option<BoardTaskView> {
    let details = state.board_details.get(&task.id)?;
    if !details.public_visibility {
        return None;
    }
    let lifecycle = state.task_lifecycle.get(&task.id);
    let cleanup = state.cleanup_receipts.get(&task.id);
    Some(BoardTaskView {
        id: task.id.clone(),
        title: details.title.clone(),
        description: details.description.clone(),
        delivery_condition: details.delivery_condition.clone(),
        test_condition: details.test_condition.clone(),
        revision: details.revision,
        owner: task.owner.clone(),
        publisher: task.created_by.clone(),
        priority: task.priority.clone(),
        status: task.status.clone(),
        next_step: task.next_step.clone(),
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
    })
}

fn board_task_response(state: &State, task_id: &str) -> Resp {
    match state.tasks.get(task_id).and_then(|task| board_task_view(state, task)) {
        Some(task) => Resp::data(json!({ "task": task, "notification": "none" })),
        None => Resp::err("BOARD_TASK_NOT_PUBLIC: task does not belong to the public task board"),
    }
}

fn handle_board_show(server: &Server) -> Resp {
    // Native observations run outside the reducer lock. They never determine
    // task owner, status, or the persisted invitation reservation.
    let candidates: Vec<_> = {
        let state = server.state.lock().unwrap();
        state.workers.values().filter(|worker| !is_managed_subagent(&state, &worker.id)).cloned().collect()
    };
    let observations: std::collections::HashMap<_, _> = candidates.into_iter().map(|worker| {
        let status = match worker_presence(server, &worker) {
            IdentityPresence::Present => "online",
            IdentityPresence::Cold => "cold",
            IdentityPresence::Missing => "offline",
            IdentityPresence::Unknown => "unknown",
        };
        (worker.id, status)
    }).collect();
    let state = server.state.lock().unwrap();
    let route = match server_route_scope(server, &state) {
        Ok(route) => route,
        Err(error) => return Resp::err(error),
    };
    let master = current_master_worker_id(&state, route.as_ref());
    let mut tasks: Vec<_> = state.tasks.values().filter_map(|task| board_task_view(&state, task)).collect();
    tasks.sort_by(|left, right| left.id.cmp(&right.id));
    let mut workers: Vec<_> = state.workers.values().filter(|worker| !is_managed_subagent(&state, &worker.id)).map(|worker| {
        BoardMemberView {
            id: worker.id.clone(),
            role: if master.as_deref() == Some(worker.id.as_str()) { "master" } else { "peer" }.into(),
            status: observations.get(&worker.id).copied().unwrap_or("unknown").into(),
            task_ids: tasks.iter().filter(|task| task.owner == worker.id && board_execution_responsibility(&task.status)).map(|task| task.id.clone()).collect(),
            invitation_ids: tasks.iter().filter(|task| task.status == "invited" && task.invited_peer.as_deref() == Some(worker.id.as_str())).map(|task| task.id.clone()).collect(),
        }
    }).collect();
    workers.sort_by(|left, right| left.id.cmp(&right.id));
    Resp::data(json!({
        "schema_version": 1,
        "project": server.root.file_name().map(|name| name.to_string_lossy()),
        "observed_at": iso(now_ms()),
        "tasks": tasks,
        "workers": workers,
    }))
}

fn board_execution_responsibility(status: &str) -> bool {
    keepalive::unfinished(status)
}

/// One capacity predicate for public dispatch and final acceptance. Waiting,
/// merged-but-not-cleaned, and verification still belong to the peer.
fn board_peer_has_responsibility(state: &State, peer: &str, except_task: Option<&str>) -> bool {
    state.tasks.values().any(|task| {
        Some(task.id.as_str()) != except_task && (
            (task.owner == peer && board_execution_responsibility(&task.status))
            || (task.status == "invited" && state.board_details.get(&task.id)
                .and_then(|details| details.invitation.as_ref()).is_some_and(|invite| invite.peer_id == peer))
        )
    })
}

fn board_check_revision(state: &State, task_id: &str, expected: u64) -> Result<BoardTaskDetails, Resp> {
    let details = state.board_details.get(task_id).filter(|details| details.public_visibility)
        .ok_or_else(|| Resp::err("BOARD_TASK_NOT_PUBLIC: no public task with this id"))?;
    if details.revision != expected {
        return Err(Resp::err_data("BOARD_STALE_REVISION: reread the board before updating", json!({ "task_id": task_id, "expected_revision": expected, "current_revision": details.revision })));
    }
    Ok(details.clone())
}

fn board_check_actor(state: &State, actor: &str, token: &str) -> Result<(), Resp> {
    verify(state, actor, token)?;
    if is_managed_subagent(state, actor) {
        return Err(Resp::err("BOARD_PRIVATE_ACTOR: the public board is only for master and ordinary peers"));
    }
    Ok(())
}

fn board_check_master_locked(server: &Server, state: &State, actor: &str) -> Result<(), Resp> {
    let route = server_route_scope(server, state).map_err(Resp::err)?;
    if current_master_worker_id(state, route.as_ref()).as_deref() != Some(actor) {
        return Err(Resp::err("BOARD_MASTER_REQUIRED: only the current project master may publish or invite"));
    }
    Ok(())
}

fn handle_board_command(server: &Server, actor: String, token: String, command: BoardCommand) -> Resp {
    {
        let state = server.state.lock().unwrap();
        if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
    }
    if matches!(command, BoardCommand::Publish { .. } | BoardCommand::Invite { .. } | BoardCommand::Withdraw { .. }) {
        match live_master_worker_snapshot(server) {
            Ok(Some(master)) if master.id == actor => {},
            Ok(_) => return Resp::err("BOARD_LIVE_MASTER_REQUIRED: project master is not live"),
            Err(error) => return Resp::err(error),
        }
    }
    match command {
        BoardCommand::Show => handle_board_show(server),
        BoardCommand::Publish { id, title, description, delivery_condition, test_condition, priority } => {
            if [&id, &title, &description, &delivery_condition, &test_condition].iter().any(|text| text.trim().is_empty()) {
                return Resp::err("BOARD_DESCRIPTION_REQUIRED: provide task id, title, description, delivery condition and test condition");
            }
            if !["p0", "p1", "p2", "p3", "p4"].contains(&priority.as_str()) {
                return Resp::err("invalid priority; expected p0, p1, p2, p3 or p4");
            }
            let mut state = server.state.lock().unwrap();
            if let Err(error) = board_check_actor(&state, &actor, &token).and_then(|()| board_check_master_locked(server, &state, &actor)) { return error; }
            if state.tasks.contains_key(&id) { return Resp::err("task id already exists"); }
            let now = now_ms();
            let task = TaskRec {
                id: id.clone(), owner: actor.clone(), created_by: actor,
                feature_id: None, worktree_path: None, branch: None, base_commit: None,
                priority, status: "pending".into(), next_step: None, wait: None,
                created_ms: now, updated_ms: now,
            };
            let details = BoardTaskDetails {
                title, description, delivery_condition, test_condition,
                revision: 1, public_visibility: true, ..BoardTaskDetails::default()
            };
            if let Err(error) = server.commit_locked_checked(&mut state, &[Event::TaskCreated { task }, Event::BoardDetailsChanged { task_id: id.clone(), details }]) {
                return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}"));
            }
            board_task_response(&state, &id)
        },
        BoardCommand::Update { id, expected_revision, status, next } => {
            let mut state = server.state.lock().unwrap();
            if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
            if let Err(error) = board_check_revision(&state, &id, expected_revision) { return error; }
            if status.is_none() && next.is_none() { return Resp::err("provide --status or --next"); }
            let result = task_update_locked(server, &mut state, actor, token, id.clone(), status, next);
            if !result.ok { return result; }
            board_task_response(&state, &id)
        },
        BoardCommand::Describe { id, expected_revision, title, description, delivery_condition, test_condition } => {
            let mut state = server.state.lock().unwrap();
            if let Err(error) = board_check_actor(&state, &actor, &token) { return error; }
            let Some(task) = state.tasks.get(&id) else { return Resp::err("task not found"); };
            if task.owner != actor { return Resp::err("only the task owner may describe it"); }
            let Some(mut details) = state.board_details.get(&id).cloned() else { return Resp::err("task description record missing"); };
            if details.revision != expected_revision { return Resp::err("BOARD_STALE_REVISION: reread the board"); }
            if [&title, &description, &delivery_condition, &test_condition].iter().any(|text| text.trim().is_empty()) { return Resp::err("description and conditions must not be empty"); }
            if task.status == "invited" { return Resp::err("withdraw the invitation before changing its contract"); }
            details.title = title; details.description = description;
            details.delivery_condition = delivery_condition; details.test_condition = test_condition;
            details.public_visibility = true; details.revision += 1;
            if let Err(error) = server.commit_locked_checked(&mut state, &[Event::BoardDetailsChanged { task_id: id.clone(), details }]) {
                return Resp::err(format!("BOARD_DURABILITY_FAILED: {error}"));
            }
            board_task_response(&state, &id)
        },
        BoardCommand::Invite { id, to, expected_revision } => board_invite(server, actor, token, id, to, expected_revision),
        BoardCommand::Respond { id, accept, decline, expected_revision, reason } => {
            if accept == decline { return Resp::err("choose exactly one of accept or decline"); }
            board_respond(server, actor, token, id, expected_revision, accept, reason)
        },
        BoardCommand::DeclineAssigned { id, expected_revision, reason } => board_decline_assigned(server, actor, token, id, expected_revision, reason),
        BoardCommand::Withdraw { id, expected_revision, reason } => board_withdraw(server, actor, token, id, expected_revision, reason),
    }
}
