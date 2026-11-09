use state::{GoalScheduleBinding, GoalScheduleContent};

struct GoalScheduleChange {
    goal_id: String,
    goal_owner: String,
    desired: String,
    trigger_ms: Option<i64>,
    ttl_seconds: Option<u64>,
    goal_subject: Option<String>,
    reason: Option<String>,
    interval_ms: Option<i64>,
    repeat_count: Option<u32>,
    content: Option<GoalScheduleContent>,
    renew_ended: bool,
}

fn handle_goal_schedule_for_actor(
    server: &Server,
    actor: &str,
    token: &str,
    change: GoalScheduleChange,
) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, actor, token) {
        return error;
    }
    let target = match goal_schedule_target(server, &state, &change.goal_owner) {
        Ok(target) => target,
        Err(error) => return error,
    };
    if actor != target {
        match current_master_holder(server, &state) {
            Ok(Some(master)) if master == actor => {},
            Ok(_) => return Resp::err("GOAL_SCHEDULE_OWNER_MISMATCH: only the owner, project master or current CEO may manage this goal"),
            Err(error) => return Resp::err(error),
        }
    }
    commit_goal_schedule(server, &mut state, &target, change)
}

fn goal_schedule_target(server: &Server, state: &State, name: &str) -> Result<String, Resp> {
    let route = server_route_scope(server, state)
        .map_err(Resp::err)?
        .ok_or_else(|| Resp::err("GOAL_SCHEDULE_PROJECT_SCOPE_MISSING"))?;
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .map_err(Resp::err)?;
    let slot = control
        .current_for_name(&route.project_scope_id, &route.app_scope_id, name)
        .map_err(Resp::err)?
        .and_then(|slot| slot.active_binding())
        .ok_or_else(|| Resp::err("GOAL_SCHEDULE_TARGET_UNAVAILABLE"))?;
    let target = slot.binding.agent_id.as_str();
    authoritative_send_binding(state, &route, target).map_err(Resp::err)?;
    Ok(target.to_owned())
}

fn commit_goal_schedule(
    server: &Server,
    state: &mut State,
    target: &str,
    change: GoalScheduleChange,
) -> Resp {
    if !matches!(change.desired.as_str(), "active" | "ended") {
        return Resp::err("GOAL_SCHEDULE_DESIRED_INVALID: expected active or ended");
    }
    if let Some(task_id) = change
        .goal_id
        .strip_prefix("task:")
        .filter(|id| state.board_details.contains_key(*id))
    {
        let mut details = state.board_details[task_id].clone();
        let task = match state.tasks.get(task_id) {
            Some(task) => task,
            None => return Resp::err("task not found"),
        };
        if details.long_horizon.is_none() {
            return Resp::err("BOARD_LONG_HORIZON_CONTENT_REQUIRED");
        }
        let current = task_schedule_target(server, state, task, &details);
        match current {
            Ok(Some(current)) if current == target => {}
            Ok(_) => return Resp::err("GOAL_SCHEDULE_TASK_OWNER_MISMATCH"),
            Err(error) => return error,
        }
        let spec = details.long_horizon.as_mut().unwrap();
        if let Some(GoalScheduleContent::Inline { content }) = &change.content {
            if content.trim().is_empty() {
                return Resp::err("GOAL_SCHEDULE_CONTENT_REQUIRED");
            }
            spec.content = content.clone();
        }
        if let Some(interval) = change.interval_ms {
            spec.interval_ms = interval;
        }
        if let Some(repeat) = change.repeat_count {
            spec.repeat_count = repeat;
        }
        if let Some(ttl) = change.ttl_seconds {
            spec.ttl_seconds = ttl;
        }
        details.long_horizon =
            match crate::board::LongHorizonArgs::default().merge(details.long_horizon.as_ref()) {
                Ok(spec) => spec,
                Err(error) => return Resp::err(error),
            };
        let (mut events, response) = match task_goal_events(
            server,
            state,
            task,
            &details,
            Some(target),
            change.desired == "ended",
            true,
        ) {
            Ok(result) => result,
            Err(error) => return error,
        };
        if state.board_details[task_id].long_horizon != details.long_horizon {
            if let Some(mut record) = state.task_lifecycle.get(task_id).cloned() {
                events.extend(invalidate_work_review(
                    state,
                    task,
                    &mut record,
                    details.work_policy.reviewer_name.clone(),
                ));
                events.push(Event::TaskLifecycleUpdated {
                    task_id: task_id.into(),
                    record,
                });
            }
            details.revision += 1;
            events.push(Event::BoardDetailsChanged {
                task_id: task_id.into(),
                details,
            });
        }
        if let Err(error) = server.commit_locked_checked(state, &events) {
            return Resp::err(format!("NOTIFICATION_DURABILITY_FAILED: {error}"));
        }
        return Resp::data(response);
    }
    let (events, response) = match prepare_goal_schedule(state, target, &change, now_ms()) {
        Ok(result) => result,
        Err(error) => return error,
    };
    if let Err(error) = server.commit_locked_checked(state, &events) {
        return Resp::err(format!("NOTIFICATION_DURABILITY_FAILED: {error}"));
    }
    response
}

/// The only finite goal-schedule reducer. Callers authorize the target, prepare
/// their other business changes, then commit these events in the same batch.
fn prepare_goal_schedule(
    state: &State,
    target: &str,
    change: &GoalScheduleChange,
    now: i64,
) -> Result<(Vec<Event>, Resp), Resp> {
    if change.goal_id.trim().is_empty() {
        return Err(Resp::err("GOAL_SCHEDULE_ID_REQUIRED"));
    }
    if change.goal_owner.trim().is_empty() && change.desired != "ended" {
        return Err(Resp::err("GOAL_SCHEDULE_OWNER_REQUIRED"));
    }
    if !matches!(change.desired.as_str(), "active" | "ended") {
        return Err(Resp::err(
            "GOAL_SCHEDULE_DESIRED_INVALID: expected active or ended",
        ));
    }
    if let Some(subject) = change.goal_subject.as_deref() {
        if subject.trim().is_empty() {
            return Err(Resp::err("GOAL_SCHEDULE_SUBJECT_REQUIRED"));
        }
        if subject != subject.trim() {
            return Err(Resp::err("GOAL_SCHEDULE_SUBJECT_NOT_EXACT"));
        }
        if !subject.starts_with("goal:") || subject.len() <= 5 {
            return Err(Resp::err("GOAL_SCHEDULE_SUBJECT_INVALID"));
        }
    }
    let subject = format!("goal:{}", change.goal_id);
    let task_id = match change.content.as_ref() {
        Some(GoalScheduleContent::BoardTask { task_id }) => Some(task_id),
        _ => None,
    };
    let mut matching: Vec<_> = state.notification_subscriptions.values().filter(|subscription| {
        subscription.event == "deadline"
            && (subscription.subject.as_deref() == Some(subject.as_str())
                || change.goal_subject.as_ref().is_some_and(|old| subscription.subject.as_ref() == Some(old)))
            && (subscription.worker_id == target || task_id.is_some_and(|id| matches!(
                subscription.goal_content.as_ref().and_then(|binding| binding.source.as_ref()), Some(GoalScheduleContent::BoardTask { task_id }) if task_id == id)))
    }).cloned().collect();
    matching.sort_by(|left, right| {
        right
            .created_ms
            .cmp(&left.created_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut events = Vec::new();
    let mut retired = Vec::new();
    let mut superseded = Vec::new();
    let mut active = None;
    if change.desired == "active" {
        if is_managed_subagent(state, target) {
            return Err(Resp::err(
                "managed subworkers cannot create independent deadline schedules",
            ));
        }
        let worker = state
            .workers
            .get(target)
            .ok_or_else(|| Resp::err("goal schedule target is not registered"))?;
        let transport = selected_transport_for_worker(worker)
            .ok_or_else(|| Resp::err("registered worker has no server-selected transport"))?;
        let address = notification_target(&transport)
            .ok_or_else(|| Resp::err("selected transport has no route address"))?;
        let periodic_request = change.interval_ms.is_some()
            || change.trigger_ms.is_none()
                && matching
                    .iter()
                    .any(|subscription| subscription.periodic_declared);
        let existing = matching
            .iter()
            .find(|subscription| {
                subscription.status == "armed"
                    && subscription.expires_ms > now
                    && if periodic_request {
                        subscription.periodic_declared
                    } else {
                        !subscription.periodic_declared
                            && subscription.trigger_ms == change.trigger_ms
                    }
            })
            .or_else(|| {
                matching.iter().find(|subscription| {
                    periodic_request
                        && subscription.periodic_declared
                        && subscription.status == "transport-lost"
                        && subscription.expires_ms > now
                })
            });
        if task_id.is_some() && existing.is_none() && !matching.is_empty() && !change.renew_ended {
            return Err(Resp::err(
                "GOAL_SCHEDULE_RENEWAL_REQUIRED: task batch has ended",
            ));
        }
        let interval = change.interval_ms.or_else(|| {
            existing
                .filter(|_| change.trigger_ms.is_none())
                .and_then(|old| old.interval_ms)
        });
        if interval.is_some_and(|interval| interval <= 0) {
            return Err(Resp::err("interval_ms must be greater than zero"));
        }
        if interval.is_none() && change.repeat_count.is_some() {
            return Err(Resp::err(
                "GOAL_SCHEDULE_PERIOD_INCOMPLETE: repeat requires interval_ms",
            ));
        }
        let repeat = if interval.is_some() {
            change
                .repeat_count
                .or_else(|| existing.map(|old| old.repeat_count))
                .unwrap_or(100)
        } else {
            1
        };
        if !(1..=state::MAX_NOTIFICATION_REPEATS).contains(&repeat) {
            return Err(Resp::err("repeat_count must be between 1 and 100"));
        }
        let ttl = change.ttl_seconds;
        if ttl.is_some_and(|ttl| ttl == 0 || ttl > MAX_NOTIFICATION_TTL_SECONDS) {
            return Err(Resp::err(format!(
                "ttl_seconds must be between 1 and {MAX_NOTIFICATION_TTL_SECONDS}"
            )));
        }
        let previous_ttl = existing.map(|old| {
            old.goal_content.as_ref().map_or_else(
                || old.expires_ms.saturating_sub(old.created_ms).max(0) as u64 / 1000,
                |binding| binding.ttl_seconds,
            )
        });
        let expiry = match existing {
            Some(old) if ttl.is_none() || ttl == previous_ttl => old.expires_ms,
            Some(_) => now.saturating_add((ttl.unwrap() as i64).saturating_mul(1000)),
            None => now.saturating_add(
                (ttl.ok_or_else(|| Resp::err("GOAL_SCHEDULE_TTL_REQUIRED"))? as i64)
                    .saturating_mul(1000),
            ),
        };
        let trigger = match existing {
            Some(old) if old.interval_ms == interval => old.trigger_ms,
            _ => change
                .trigger_ms
                .or_else(|| interval.map(|interval| now.saturating_add(interval))),
        }
        .ok_or_else(|| Resp::err("GOAL_SCHEDULE_TRIGGER_REQUIRED"))?;
        if trigger >= expiry
            || existing.is_none() && trigger <= now
            || existing.is_some_and(|old| old.interval_ms != interval)
                && interval.is_some_and(|interval| now.saturating_add(interval) >= expiry)
        {
            return Err(Resp::err(
                "goal deadline trigger must be in the future and before expiry",
            ));
        }
        if let Some(GoalScheduleContent::Inline { content }) = &change.content {
            if content.trim().is_empty() {
                return Err(Resp::err("GOAL_SCHEDULE_CONTENT_REQUIRED"));
            }
        }
        let other_active = state
            .notification_subscriptions
            .values()
            .filter(|subscription| {
                subscription.worker_id == target
                    && subscription.status == "armed"
                    && !matching.iter().any(|old| old.id == subscription.id)
            })
            .count();
        if other_active >= MAX_ACTIVE_SUBSCRIPTIONS_PER_WORKER {
            return Err(Resp::err("maximum 3 active subscriptions per agent"));
        }
        let mut subscription = existing
            .cloned()
            .unwrap_or_else(|| NotificationSubscription {
                id: format!("sub-{}", gen_msg_id()),
                worker_id: target.into(),
                event: "deadline".into(),
                subject: Some(subject.clone()),
                target: address.to_string(),
                method: transport.kind.as_str().into(),
                trigger_ms: Some(trigger),
                trigger_times_ms: Vec::new(),
                interval_ms: interval,
                repeat_count: repeat,
                fired_count: 0,
                expires_ms: expiry,
                status: "armed".into(),
                created_ms: now,
                updated_ms: now,
                status_reason: None,
                patrol_idle_since_ms: None,
                patrol_last_push_ms: 0,
                periodic_declared: interval.is_some(),
                goal_content: None,
            });
        if subscription.worker_id != target
            || subscription.target != address
            || subscription.method != transport.kind.as_str()
        {
            superseded.extend(pending_messages_for_subscription(state, &subscription.id));
        }
        subscription.worker_id = target.into();
        subscription.target = address.to_string();
        subscription.method = transport.kind.as_str().into();
        subscription.subject = Some(subject);
        subscription.trigger_ms = Some(trigger);
        subscription.expires_ms = expiry;
        subscription.interval_ms = interval;
        subscription.repeat_count = repeat;
        subscription.periodic_declared = interval.is_some();
        subscription.status = if subscription.fired_count >= repeat {
            "consumed"
        } else {
            "armed"
        }
        .into();
        if subscription.status != "armed" {
            superseded.extend(pending_messages_for_subscription(state, &subscription.id));
        }
        subscription.status_reason = None;
        subscription.goal_content = Some(GoalScheduleBinding {
            source: change.content.clone().or_else(|| {
                existing
                    .and_then(|old| old.goal_content.as_ref())
                    .and_then(|binding| binding.source.clone())
            }),
            ttl_seconds: ttl.or(previous_ttl).unwrap(),
        });
        let changed = existing != Some(&subscription);
        if changed {
            subscription.updated_ms = now;
            events.push(Event::NotificationSubscribed {
                subscription: subscription.clone(),
            });
        }
        active = Some(subscription);
    }
    for old in &matching {
        if active.as_ref().is_some_and(|current| current.id == old.id) {
            continue;
        }
        if matches!(old.status.as_str(), "armed" | "transport-lost") {
            retired.push(old.id.clone());
            events.push(Event::NotificationSuppressed {
                subscription_id: old.id.clone(),
                status: "cancelled".into(),
                reason: change
                    .reason
                    .clone()
                    .unwrap_or_else(|| "goal-schedule-ended".into()),
                updated_ms: now,
            });
        }
        superseded.extend(pending_messages_for_subscription(state, &old.id));
    }
    superseded.sort();
    superseded.dedup();
    if !superseded.is_empty() {
        events.push(Event::Superseded {
            ids: superseded.clone(),
        });
    }
    if let Some(event) = cleared_master_goal_due_event(state, target, &retired, now) {
        events.push(event);
    }
    let response = Resp::data(json!({
        "goal_id": change.goal_id, "goal_owner": (!change.goal_owner.is_empty()).then_some(&change.goal_owner), "desired": change.desired,
        "status": active.as_ref().map_or("ended", |subscription| subscription.status.as_str()),
        "one_shot": active.as_ref().is_some_and(|subscription| !subscription.periodic_declared),
        "subscription": active, "retired_subscription_ids": retired, "superseded_message_ids": superseded,
        "deduplicated": events.is_empty(), "max_repeat_count": state::MAX_NOTIFICATION_REPEATS,
    }));
    Ok((events, response))
}

fn task_goal_events(
    server: &Server,
    state: &State,
    task: &TaskRec,
    details: &crate::board::BoardTaskDetails,
    selected: Option<&str>,
    ended: bool,
    renew_ended: bool,
) -> Result<(Vec<Event>, serde_json::Value), Resp> {
    let Some(spec) = &details.long_horizon else {
        return Ok((Vec::new(), serde_json::Value::Null));
    };
    let subject = format!("goal:task:{}", task.id);
    let existing = state
        .notification_subscriptions
        .values()
        .find(|subscription| subscription.subject.as_deref() == Some(subject.as_str()));
    let target = selected
        .or_else(|| {
            details
                .invitation
                .as_ref()
                .map(|invite| invite.peer_id.as_str())
        })
        .or_else(|| existing.map(|subscription| subscription.worker_id.as_str()))
        .or_else(|| {
            (!matches!(task.status.as_str(), "pending" | "invited")).then_some(task.owner.as_str())
        });
    let Some(target) = target else {
        return Ok((Vec::new(), serde_json::Value::Null));
    };
    let route = server_route_scope(server, state)
        .map_err(Resp::err)?
        .ok_or_else(|| Resp::err("GOAL_SCHEDULE_PROJECT_SCOPE_MISSING"))?;
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .map_err(Resp::err)?;
    let name = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, target)
        .map_err(Resp::err)?
        .map(|slot| slot.identity_key.name.clone());
    let name = match name {
        Some(name) => name,
        None if ended => state
            .task_assignment(&task.id)
            .map(|assignment| assignment.name.clone())
            .unwrap_or_default(),
        None => return Err(Resp::err("GOAL_SCHEDULE_TARGET_UNAVAILABLE")),
    };
    let change = GoalScheduleChange {
        goal_id: format!("task:{}", task.id),
        goal_owner: name,
        desired: if ended { "ended" } else { "active" }.into(),
        trigger_ms: None,
        ttl_seconds: Some(spec.ttl_seconds),
        goal_subject: None,
        reason: None,
        interval_ms: Some(spec.interval_ms),
        repeat_count: Some(spec.repeat_count),
        content: Some(GoalScheduleContent::BoardTask {
            task_id: task.id.clone(),
        }),
        renew_ended,
    };
    let (events, response) = prepare_goal_schedule(state, target, &change, now_ms())?;
    Ok((events, response.data))
}

fn task_goal_receipt(state: &State, task_id: &str) -> serde_json::Value {
    let subject = format!("goal:task:{task_id}");
    let subscription = state
        .notification_subscriptions
        .values()
        .filter(|subscription| subscription.subject.as_deref() == Some(subject.as_str()))
        .max_by_key(|subscription| subscription.created_ms);
    json!({ "armed": subscription.is_some_and(|subscription| subscription.status == "armed"), "subscription": subscription })
}

fn task_schedule_target(
    server: &Server,
    state: &State,
    task: &TaskRec,
    details: &crate::board::BoardTaskDetails,
) -> Result<Option<String>, Resp> {
    if let Some(invite) = &details.invitation {
        return Ok(Some(invite.peer_id.clone()));
    }
    if let Some(phase) = state
        .ticket_phases
        .iter()
        .filter(|phase| {
            phase.task_id == task.id
                && phase.phase == ticket_state::TicketPhaseKind::Execute
                && phase.status != ticket_state::TicketPhaseStatus::Revoked
        })
        .max_by_key(|phase| phase.batch)
    {
        if !matches!(
            phase.status,
            ticket_state::TicketPhaseStatus::Queued
                | ticket_state::TicketPhaseStatus::WaitingAssignment
        ) {
            return phase
                .slot_name
                .as_deref()
                .map(|name| goal_schedule_target(server, state, name))
                .transpose();
        }
    }
    Ok((!matches!(task.status.as_str(), "pending" | "invited")).then(|| task.owner.clone()))
}

/// Extend the existing candidate-state transaction with linked task schedules.
/// This also covers ticket assignment, decline, reassignment and final closure.
fn task_goal_transition_events(
    server: &Server,
    before: &State,
    after: &State,
    changes: &[Event],
) -> Result<Vec<Event>, Resp> {
    let mut task_ids = std::collections::BTreeSet::new();
    for change in changes {
        match change {
            Event::TaskCreated { task } | Event::TaskUpdated { task } => {
                task_ids.insert(task.id.as_str());
            }
            Event::BoardDetailsChanged { task_id, .. } => {
                task_ids.insert(task_id.as_str());
            }
            Event::TicketPhaseRecorded { phase } => {
                task_ids.insert(phase.task_id.as_str());
            }
            _ => {}
        }
    }
    let mut events = Vec::new();
    for id in task_ids {
        let (Some(task), Some(details)) = (after.tasks.get(id), after.board_details.get(id)) else {
            continue;
        };
        if details.long_horizon.is_none() {
            continue;
        }
        let explicit_goal_change = changes.iter().any(|change| {
            let subscription = match change {
                Event::NotificationSubscribed { subscription } => Some(subscription),
                Event::NotificationSuppressed {
                    subscription_id, ..
                } => before.notification_subscriptions.get(subscription_id),
                _ => None,
            };
            subscription.is_some_and(|subscription| {
                matches!(subscription.goal_content.as_ref().and_then(|binding| binding.source.as_ref()),
                Some(GoalScheduleContent::BoardTask { task_id }) if task_id == id)
            })
        });
        if explicit_goal_change {
            continue;
        }
        let old_details = before.board_details.get(id);
        let old_task = before.tasks.get(id);
        let first_invitation_ended = old_task.is_some_and(|task| task.status == "invited")
            && task.status == "pending"
            && details.invitation.is_none();
        let ended =
            first_invitation_ended || matches!(task.status.as_str(), "closed" | "cancelled");
        let target = if ended {
            None
        } else {
            task_schedule_target(server, after, task, details)?
        };
        let old_target = match (old_task.filter(|_| !ended), old_details) {
            (Some(task), Some(details)) => task_schedule_target(server, before, task, details)?,
            _ => None,
        };
        let form_changed = old_details.and_then(|details| details.long_horizon.as_ref())
            != details.long_horizon.as_ref();
        if !ended && !form_changed && old_target == target {
            continue;
        }
        if !ended && target.is_none() {
            continue;
        }
        let explicit_invitation = old_details.is_some_and(|details| details.invitation.is_none())
            && details.invitation.is_some();
        let (goal_events, _) = task_goal_events(
            server,
            after,
            task,
            details,
            target.as_deref(),
            ended,
            explicit_invitation,
        )?;
        events.extend(goal_events);
    }
    Ok(events)
}

impl ProjectRuntimeManager {
    fn dispatch_goal_schedule(
        &self,
        context: &ProjectContext,
        request: &Req,
        actor: &str,
        token: &str,
        project: Option<&str>,
        app: Option<&str>,
        change: GoalScheduleChange,
    ) -> (Arc<Server>, Resp) {
        let (source, name) = match self.ceo_actor(context, actor, token) {
            Ok(source) => source,
            Err(error) => return (self.host.clone(), error),
        };
        if let Err(error) = validate_request_context(&source, request, Some(context)) {
            return (source, Resp::err(error));
        }
        let is_ceo = self.require_ceo(context, &name).is_ok();
        let root = project.unwrap_or(&context.canonical_root);
        let canonical = match std::fs::canonicalize(root) {
            Ok(root) => root,
            Err(error) => {
                return (
                    source,
                    Resp::err(format!("GOAL_SCHEDULE_PROJECT_INVALID: {error}")),
                )
            }
        };
        let target_app = app.unwrap_or(context.app_scope_id.as_str());
        let same_scope = canonical == source.root && target_app == context.app_scope_id.as_str();
        if !same_scope && !is_ceo {
            return (
                source,
                Resp::err(
                    "CEO_AUTHORITY_REQUIRED: cross-project goal management requires current CEO",
                ),
            );
        }
        let search_project_apps = is_ceo && project.is_some() && app.is_none();
        let target = if same_scope && !search_project_apps {
            source.clone()
        } else {
            let runtimes = match self.ceo_project_runtimes(root, app) {
                Ok(runtimes) => runtimes,
                Err(error) => return (source, error),
            };
            let mut matching = Vec::new();
            for runtime in runtimes {
                let result = goal_schedule_target(
                    &runtime,
                    &runtime.state.lock().unwrap(),
                    &change.goal_owner,
                );
                match result {
                    Ok(_) => matching.push(runtime),
                    Err(error)
                        if error.error.as_deref() == Some("GOAL_SCHEDULE_TARGET_UNAVAILABLE") => {}
                    Err(error) => return (source, error),
                }
            }
            if matching.len() > 1 {
                return (
                    source,
                    Resp::err("GOAL_SCHEDULE_TARGET_AMBIGUOUS: provide --app"),
                );
            }
            if matching.is_empty() {
                return (source, Resp::err("GOAL_SCHEDULE_TARGET_SCOPE_UNAVAILABLE"));
            }
            matching.remove(0)
        };
        let response = if is_ceo {
            let mut state = target.state.lock().unwrap();
            match goal_schedule_target(&target, &state, &change.goal_owner) {
                Ok(worker) => commit_goal_schedule(&target, &mut state, &worker, change),
                Err(error) => error,
            }
        } else {
            handle_goal_schedule_for_actor(&target, actor, token, change)
        };
        (target, response)
    }
}
