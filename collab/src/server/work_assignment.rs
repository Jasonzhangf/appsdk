fn work_executor_gate(
    server: &Server,
    state: &State,
    task_id: &str,
    actor: &str,
) -> Result<(), Resp> {
    if let Some(name) = state.effective_work_policy(task_id).executor_name {
        let route = server_route_scope(server, state)
            .map_err(Resp::err)?
            .ok_or_else(|| Resp::err("WORK_EXECUTOR_PROJECT_REQUIRED"))?;
        let matches = state
            .name_occupancy(route.project_scope_id.as_str(), &name)
            .is_some_and(|entry| entry.worker_id == actor);
        if !matches {
            return Err(Resp::err_data(
                "WORK_EXECUTOR_REQUIRED: task is pinned to another stable name",
                json!({"task_id":task_id,"executor_name":name,"delivery_complete":false}),
            ));
        }
    }
    Ok(())
}

fn parse_work_policy(
    kind: Option<String>,
    prefer_role: Option<String>,
    executor: Option<String>,
    reviewer: Option<String>,
    clear_roles: bool,
    clear_executor: bool,
    clear_reviewer: bool,
) -> Result<crate::board::WorkPolicy, Resp> {
    fn name(value: Option<String>) -> Result<Option<String>, Resp> {
        value.map(|value| {
            let value=value.trim();
            if value.is_empty() || value.chars().any(char::is_control) {
                Err(Resp::err("WORK_POLICY_INVALID: values must be nonempty and contain no control characters"))
            } else { Ok(value.to_owned()) }
        }).transpose()
    }
    if (clear_roles && prefer_role.is_some())
        || (clear_executor && executor.is_some())
        || (clear_reviewer && reviewer.is_some())
    {
        return Err(Resp::err(
            "WORK_POLICY_INVALID: choose a value or clear, not both",
        ));
    }
    let preferred_roles = prefer_role
        .map(|text| {
            let mut values = Vec::new();
            for value in text.split(',') {
                values.push(name(Some(value.to_lowercase()))?.unwrap());
            }
            values.sort();
            values.dedup();
            Ok::<_, Resp>(values)
        })
        .transpose()?;
    let mut cleared_fields = Vec::new();
    if clear_roles {
        cleared_fields.push("preferred_roles".into());
    }
    if clear_executor {
        cleared_fields.push("executor_name".into());
    }
    if clear_reviewer {
        cleared_fields.push("reviewer_name".into());
    }
    Ok(crate::board::WorkPolicy {
        work_kind: name(kind)?,
        preferred_roles,
        executor_name: name(executor)?,
        reviewer_name: name(reviewer)?,
        cleared_fields,
    })
}

fn overlay_work_policy(base: &mut crate::board::WorkPolicy, patch: &crate::board::WorkPolicy) {
    let cleared = |field: &str| patch.cleared_fields.iter().any(|value| value == field);
    crate::board::apply_policy_field(&patch.work_kind, cleared("work_kind"), &mut base.work_kind);
    crate::board::apply_policy_field(
        &patch.preferred_roles,
        cleared("preferred_roles"),
        &mut base.preferred_roles,
    );
    crate::board::apply_policy_field(
        &patch.executor_name,
        cleared("executor_name"),
        &mut base.executor_name,
    );
    crate::board::apply_policy_field(
        &patch.reviewer_name,
        cleared("reviewer_name"),
        &mut base.reviewer_name,
    );
    for field in [
        "work_kind",
        "preferred_roles",
        "executor_name",
        "reviewer_name",
    ] {
        let supplied = match field {
            "work_kind" => patch.work_kind.is_some(),
            "preferred_roles" => patch.preferred_roles.is_some(),
            "executor_name" => patch.executor_name.is_some(),
            _ => patch.reviewer_name.is_some(),
        };
        if supplied {
            base.cleared_fields.retain(|entry| entry != field);
        }
        if cleared(field) && !base.cleared_fields.iter().any(|entry| entry == field) {
            base.cleared_fields.push(field.into());
        }
    }
}

fn board_work_policy(
    server: &Server,
    actor: String,
    token: String,
    scope: String,
    id: Option<String>,
    patch: crate::board::WorkPolicy,
    expected_revision: Option<u64>,
) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, &actor, &token) {
        return error;
    }
    if scope == "project" {
        if let Err(error) = board_check_master_locked(server, &state, &actor) {
            return error;
        }
        let Some(kind) = patch.work_kind.as_ref() else {
            return Resp::err("project policy requires --work-kind");
        };
        if id.is_some() || expected_revision.is_some() {
            return Resp::err("project policy does not select a task or revision");
        }
        let mut policy = state
            .project_work_policy
            .get(kind)
            .cloned()
            .unwrap_or_default();
        overlay_work_policy(&mut policy, &patch);
        if let Err(error) = server.commit_locked_checked(
            &mut state,
            &[Event::ProjectWorkPolicyUpdated {
                work_kind: kind.clone(),
                policy: policy.clone(),
            }],
        ) {
            return Resp::err(format!("WORK_POLICY_DURABILITY_FAILED: {error}"));
        }
        return Resp::data(
            json!({"outcome":"rules_updated","scope":"project","work_kind":kind,"policy":policy}),
        );
    }
    if !matches!(scope.as_str(), "goal" | "task") {
        return Resp::err("policy scope must be project, goal or task");
    }
    let Some(id) = id else {
        return Resp::err("goal/task policy requires --id");
    };
    let Some(revision) = expected_revision else {
        return Resp::err("goal/task policy requires --expected-revision");
    };
    let mut details = match board_check_revision(&state, &id, revision) {
        Ok(details) => details,
        Err(error) => return error,
    };
    let Some(mut task) = state.tasks.get(&id).cloned() else {
        return Resp::err("task not found");
    };
    if !board_task_visible(&state, &task, Some(&actor)) {
        return Resp::err("BOARD_PRIVATE_TASK");
    }
    if matches!(task.status.as_str(), "merged" | "closed" | "cancelled") {
        return Resp::err("WORK_POLICY_TERMINAL: create a follow-up task instead");
    }
    if details.invitation.is_some() {
        return Resp::err("withdraw the invitation before changing its contract");
    }
    let mut events = Vec::new();
    if scope == "goal" {
        if let Err(error) = board_check_master_locked(server, &state, &actor) {
            return error;
        }
        let Some(kind) = patch
            .work_kind
            .as_ref()
            .or(details.work_policy.work_kind.as_ref())
            .cloned()
        else {
            return Resp::err("goal policy requires --work-kind");
        };
        let policy = details.child_work_policies.entry(kind).or_default();
        overlay_work_policy(policy, &patch);
    } else {
        if task.owner != actor {
            if let Err(error) = board_check_master_locked(server, &state, &actor) {
                return error;
            }
        }
        let previous = details.work_policy.clone();
        overlay_work_policy(&mut details.work_policy, &patch);
        if previous != details.work_policy {
            details.dispatch_selection = None;
            details.dispatch_reason = None;
            // Only a changed review requirement invalidates current acceptance;
            // executor reassignment and selection preferences preserve evidence.
            if previous.reviewer_name != details.work_policy.reviewer_name {
                let mut record = state.task_lifecycle.get(&id).cloned().unwrap_or_default();
                events.extend(invalidate_work_review(
                    &state,
                    &task,
                    &mut record,
                    details.work_policy.reviewer_name.clone(),
                ));
                if matches!(task.status.as_str(), "accepted" | "delivered") {
                    task.status = "rework".into();
                }
                if record.delivered_ms.is_some() {
                    events.push(Event::TaskLifecycleUpdated {
                        task_id: id.clone(),
                        record,
                    });
                }
            }
        }
    }
    details.revision += 1;
    task.updated_ms = now_ms();
    events.extend([
        Event::TaskUpdated { task },
        Event::BoardDetailsChanged {
            task_id: id.clone(),
            details,
        },
    ]);
    if let Err(error) = server.commit_locked_checked(&mut state, &events) {
        return Resp::err(format!("WORK_POLICY_DURABILITY_FAILED: {error}"));
    }
    board_task_response(server, &state, &id, &actor)
}

fn board_dispatch_work(
    server: &Server,
    actor: String,
    token: String,
    id: String,
    expected_revision: u64,
) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = board_check_actor(&state, &actor, &token)
        .and_then(|()| board_check_master_locked(server, &state, &actor))
    {
        return error;
    }
    let mut details = match board_check_revision(&state, &id, expected_revision) {
        Ok(details) => details,
        Err(error) => return error,
    };
    let Some(task) = state.tasks.get(&id) else {
        return Resp::err("task not found");
    };
    if !details.public_visibility
        || matches!(task.status.as_str(), "closed" | "cancelled" | "merged")
    {
        return Resp::err("WORK_DISPATCH_UNAVAILABLE: task must be public and open");
    }
    if details.invitation.is_some() {
        return Resp::err("task already has a pending invitation");
    }
    let route = match server_route_scope(server, &state) {
        Ok(Some(route)) => route,
        _ => return Resp::err("project route is not ready"),
    };
    let policy = details.work_policy.clone();
    let available = |worker: &WorkerRec| {
        worker.id != actor
            && !is_managed_subagent(&state, &worker.id)
            && authoritative_send_binding(&state, &route, &worker.id).is_ok()
            && !state
                .agent_presence(route.project_scope_id.as_str(), &worker.id)
                .is_some_and(|presence| presence.presence == name_state::AgentPresence::Offline)
    };
    let selected = if let Some(name) = policy.executor_name.as_deref() {
        state
            .name_occupancy(route.project_scope_id.as_str(), name)
            .and_then(|entry| state.workers.get(&entry.worker_id))
            .filter(|worker| available(worker))
            .map(|worker| {
                (
                    name.to_owned(),
                    worker.id.clone(),
                    "explicit executor".to_owned(),
                )
            })
    } else {
        let mut candidates: Vec<_> = state
            .workers
            .values()
            .filter(|worker| available(worker))
            .map(|worker| {
                let name = state
                    .name_occupancies
                    .get(route.project_scope_id.as_str())
                    .and_then(|names| names.values().find(|entry| entry.worker_id == worker.id))
                    .map(|entry| entry.name.clone())
                    .unwrap_or_else(|| worker.id.clone());
                let roles = worker_work_roles(&state, route.project_scope_id.as_str(), &worker.id);
                let matched = policy
                    .preferred_roles
                    .as_ref()
                    .is_some_and(|preferred| preferred.iter().any(|role| roles.contains(role)));
                let unassigned = !state.tasks.values().any(|task| {
                    !matches!(task.status.as_str(), "closed" | "cancelled")
                        && (task.owner == worker.id
                            || state.board_details.get(&task.id)
                                .and_then(|details| details.invitation.as_ref())
                                .is_some_and(|invite| invite.peer_id == worker.id))
                });
                (matched, unassigned, name, worker.id.clone())
            })
            .collect();
        candidates.sort_by(|left, right| right.0.cmp(&left.0)
            .then(right.1.cmp(&left.1)).then(left.2.cmp(&right.2)));
        candidates.first().map(|(matched, unassigned, name, id)| {
            let reason = if *matched {
                "matched preferred work role"
            } else if policy
                .preferred_roles
                .as_ref()
                .is_some_and(|roles| !roles.is_empty())
            {
                "no matching work role; selected another available peer"
            } else if *unassigned {
                "selected unassigned peer"
            } else {
                "selected available peer"
            };
            (name.clone(), id.clone(), reason.to_owned())
        })
    };
    let Some((name, peer, reason)) = selected else {
        let reason = policy
            .executor_name
            .as_ref()
            .map(|name| format!("specified executor {name} is unavailable"))
            .unwrap_or_else(|| "no available peer".into());
        details.dispatch_selection = None;
        details.dispatch_reason = Some(reason.clone());
        details.revision += 1;
        if let Err(error) = server.commit_locked_checked(
            &mut state,
            &[Event::BoardDetailsChanged {
                task_id: id.clone(),
                details,
            }],
        ) {
            return Resp::err(format!("WORK_DISPATCH_DURABILITY_FAILED: {error}"));
        }
        return Resp::data(
            json!({"outcome":"waiting","task_id":id,"reason":reason,"delivery_complete":false}),
        );
    };
    drop(state);
    board_invite_with_selection(
        server,
        actor,
        token,
        id,
        peer,
        expected_revision,
        Some((name, reason)),
    )
}

#[cfg(test)]
mod work_policy_state_tests {
    use super::*;

    #[test]
    fn category_defaults_overrides_and_explicit_clear_survive_compaction() {
        let mut state = State::default();
        state
            .apply_checked(&Event::ProjectWorkPolicyUpdated {
                work_kind: "frontend".into(),
                policy: parse_work_policy(
                    Some("frontend".into()),
                    Some("designer".into()),
                    Some("Alice".into()),
                    Some("Bob".into()),
                    false,
                    false,
                    false,
                )
                .unwrap(),
            })
            .unwrap();
        let goal = crate::board::BoardTaskDetails {
            work_policy: crate::board::WorkPolicy {
                work_kind: Some("frontend".into()),
                ..Default::default()
            },
            child_work_policies: std::collections::BTreeMap::from([
                (
                    "frontend".into(),
                    parse_work_policy(None, None, Some("Bob".into()), None, false, false, false)
                        .unwrap(),
                ),
                (
                    "backend".into(),
                    parse_work_policy(None, None, Some("Carol".into()), None, false, false, false)
                        .unwrap(),
                ),
            ]),
            ..Default::default()
        };
        state
            .apply_checked(&Event::BoardDetailsChanged {
                task_id: "goal".into(),
                details: goal,
            })
            .unwrap();
        let override_policy =
            parse_work_policy(None, None, Some("Alice".into()), None, true, false, false).unwrap();
        let resolved = state.materialize_work_policy(None, Some("goal"), &override_policy);
        assert_eq!(resolved.executor_name.as_deref(), Some("Alice"));
        assert_eq!(resolved.reviewer_name.as_deref(), Some("Bob"));
        assert!(resolved.preferred_roles.is_none());
        assert_eq!(
            state
                .materialize_work_policy(Some("backend"), Some("goal"), &Default::default())
                .executor_name
                .as_deref(),
            Some("Carol")
        );
        state
            .apply_checked(&Event::BoardDetailsChanged {
                task_id: "child".into(),
                details: crate::board::BoardTaskDetails {
                    parent_task_id: Some("goal".into()),
                    work_policy: resolved.clone(),
                    ..Default::default()
                },
            })
            .unwrap();
        let mut replay = State::default();
        for event in state.snapshot_events() {
            replay.apply_checked(&event).unwrap();
        }
        assert_eq!(replay.effective_work_policy("child"), resolved);
        assert_eq!(
            replay
                .materialize_work_policy(Some("frontend"), None, &Default::default())
                .reviewer_name
                .as_deref(),
            Some("Bob")
        );
        replay.project_work_policy.clear();
        assert_eq!(
            replay.effective_work_policy("child"),
            resolved,
            "saved tasks never re-resolve changed defaults"
        );
    }
}
