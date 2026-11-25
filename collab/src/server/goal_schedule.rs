use state::{GoalScheduleBinding, GoalScheduleContent};

#[derive(Clone, Copy, PartialEq, Eq)]
enum GoalScheduleOperation {
    Explicit,
    Edit,
    Rearm,
    Derived,
}

impl GoalScheduleOperation {
    fn from_request(operation: Option<String>) -> Result<Self, Resp> {
        match operation.as_deref() {
            None => Ok(Self::Explicit),
            Some("edit" | "update") => Ok(Self::Edit),
            Some("rearm") => Ok(Self::Rearm),
            Some(operation) => Err(Resp::err(format!(
                "GOAL_SCHEDULE_OPERATION_INVALID: unknown operation {operation}"
            ))),
        }
    }
}

struct GoalScheduleChange {
    goal_id: String,
    goal_owner: String,
    desired: String,
    trigger_ms: Option<i64>,
    ttl_seconds: Option<u64>,
    goal_subject: Option<String>,
    expected_subscription_id: Option<String>,
    reason: Option<String>,
    interval_ms: Option<i64>,
    repeat_count: Option<u32>,
    content: Option<GoalScheduleContent>,
    operation: GoalScheduleOperation,
}

impl GoalScheduleChange {
    fn validate_intent(&self) -> Result<(), Resp> {
        if !matches!(self.desired.as_str(), "active" | "ended") {
            return Err(Resp::err(
                "GOAL_SCHEDULE_DESIRED_INVALID: expected active or ended",
            ));
        }
        if self.operation == GoalScheduleOperation::Edit {
            if self.desired != "active" {
                return Err(Resp::err(
                    "GOAL_SCHEDULE_EDIT_REQUIRES_ACTIVE: edit keeps desired=active",
                ));
            }
            if self.trigger_ms.is_some() {
                return Err(Resp::err(
                    "GOAL_SCHEDULE_EDIT_TRIGGER_UNSUPPORTED: edit cannot set --at-ms",
                ));
            }
        }
        if self
            .expected_subscription_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty() || self.operation != GoalScheduleOperation::Edit)
        {
            return Err(Resp::err("GOAL_SCHEDULE_EXPECTED_SUBSCRIPTION_INVALID: a nonempty subscription ID is only valid for edit"));
        }
        if self.operation == GoalScheduleOperation::Rearm
            && (self.desired != "active"
                || self.trigger_ms.is_none()
                || self.interval_ms.is_some()
                || self.repeat_count.is_some())
        {
            return Err(Resp::err("GOAL_SCHEDULE_REARM_FIELDS_INVALID: rearm requires active and --at-ms for a one-shot goal"));
        }
        if self.desired == "ended"
            && (self.trigger_ms.is_some()
                || self.interval_ms.is_some()
                || self.repeat_count.is_some()
                || self.ttl_seconds.is_some()
                || self.content.is_some())
            && self.operation != GoalScheduleOperation::Derived
        {
            return Err(Resp::err(
                "GOAL_SCHEDULE_END_FIELDS_INVALID: ended cannot edit schedule fields",
            ));
        }
        Ok(())
    }
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
    let control = binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .map_err(Resp::err)?;
    goal_schedule_target_with_control(server, state, name, &control)
}

fn goal_schedule_target_with_control(
    server: &Server,
    state: &State,
    name: &str,
    control: &binding_slot::BindingControlSet,
) -> Result<String, Resp> {
    let route = server_route_scope(server, state)
        .map_err(Resp::err)?
        .ok_or_else(|| Resp::err("GOAL_SCHEDULE_PROJECT_SCOPE_MISSING"))?;
    let slot = control
        .current_for_name(&route.project_scope_id, &route.app_scope_id, name)
        .map_err(Resp::err)?
        .and_then(|slot| slot.active_binding())
        .ok_or_else(|| Resp::err("GOAL_SCHEDULE_TARGET_UNAVAILABLE"))?;
    let target = slot.binding.agent_id.as_str();
    if !state.workers.contains_key(target) {
        return Err(Resp::err("GOAL_SCHEDULE_TARGET_UNAVAILABLE"));
    }
    Ok(target.to_owned())
}

fn commit_goal_schedule(
    server: &Server,
    state: &mut State,
    target: &str,
    change: GoalScheduleChange,
) -> Resp {
    if let Err(error) = change.validate_intent() {
        return error;
    }
    if let Some(task_id) = change.goal_id.strip_prefix("task:") {
        let Some(mut details) = state.board_details.get(task_id).cloned() else {
            return Resp::err(
                "GOAL_SCHEDULE_TASK_NOT_FOUND: task goal requires an existing board task",
            );
        };
        let task = match state.tasks.get(task_id) {
            Some(task) => task,
            None => {
                return Resp::err(
                    "GOAL_SCHEDULE_TASK_NOT_FOUND: task goal requires an existing board task",
                )
            }
        };
        if change.desired == "active" && matches!(task.status.as_str(), "closed" | "cancelled") {
            return Resp::err(
                "GOAL_SCHEDULE_TASK_TERMINAL: closed or cancelled tasks cannot start a schedule",
            );
        }
        if change.trigger_ms.is_some()
            || change.goal_subject.is_some()
            || change.operation == GoalScheduleOperation::Rearm
        {
            return Resp::err("GOAL_SCHEDULE_TASK_FIELDS_INVALID: task schedules use the board goal and periodic cadence");
        }
        if details.long_horizon.is_none() {
            return Resp::err("BOARD_LONG_HORIZON_CONTENT_REQUIRED");
        }
        let current = task_schedule_target(server, state, task, &details, None);
        match current {
            Ok(Some(current)) if current == target => {}
            Ok(_) => return Resp::err("GOAL_SCHEDULE_TASK_OWNER_MISMATCH"),
            Err(error) => return error,
        }
        let spec = details.long_horizon.as_mut().unwrap();
        if change.desired == "active" {
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
            change.operation,
            change.reason.as_deref(),
            change.expected_subscription_id.as_deref(),
            None,
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
                    None,
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

fn effective_goal_schedule_status(subscription: &NotificationSubscription, now: i64) -> &str {
    let status = subscription.status.as_str();
    if !matches!(status, "armed" | "transport-lost" | "waiting-owner") {
        return status;
    }
    if subscription.expires_ms <= now {
        "expired"
    } else if subscription.fired_count >= subscription.repeat_count {
        "consumed"
    } else {
        status
    }
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
    if change
        .goal_subject
        .as_deref()
        .is_some_and(|old| old != subject)
    {
        return Err(Resp::err(
            "GOAL_SCHEDULE_SUBJECT_CONFLICT: use the existing deadline's exact goal ID",
        ));
    }
    let task_id = match change.content.as_ref() {
        Some(GoalScheduleContent::BoardTask { task_id }) => Some(task_id),
        _ => None,
    };
    let mut matching: Vec<_> = state.notification_subscriptions.values().filter(|subscription| {
        subscription.event == "deadline"
            && subscription.subject.as_deref() == Some(subject.as_str())
            && (subscription.worker_id == target || task_id.is_some_and(|id| matches!(
                subscription.goal_content.as_ref().and_then(|binding| binding.source.as_ref()), Some(GoalScheduleContent::BoardTask { task_id }) if task_id == id)))
    }).cloned().collect();
    matching.sort_by(|left, right| {
        right
            .created_ms
            .cmp(&left.created_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    let current =
        if change.operation != GoalScheduleOperation::Derived || change.desired == "active" {
            let mut current = matching.iter().filter(|subscription| {
                matches!(
                    effective_goal_schedule_status(subscription, now),
                    "armed" | "transport-lost" | "waiting-owner"
                )
            });
            let selected = current.next();
            if current.next().is_some() {
                let operation = if change.operation == GoalScheduleOperation::Edit {
                    "edit"
                } else {
                    "reconcile"
                };
                return Err(Resp::err(format!(
                    "GOAL_SCHEDULE_TARGET_AMBIGUOUS: {operation} requires one current goal"
                )));
            }
            selected
        } else {
            None
        };
    let current = if change.operation == GoalScheduleOperation::Rearm {
        let selected = current.filter(|row| row.status == "armed").ok_or_else(|| {
            Resp::err("GOAL_SCHEDULE_REARM_REQUIRES_CURRENT: rearm requires an armed one-shot")
        })?;
        if selected.periodic_declared {
            return Err(Resp::err("GOAL_SCHEDULE_REARM_REQUIRES_ONE_SHOT"));
        }
        let trigger = selected
            .trigger_ms
            .or_else(|| selected.trigger_times_ms.first().copied())
            .ok_or_else(|| Resp::err("GOAL_SCHEDULE_REARM_TRIGGER_MISSING"))?;
        (trigger > now).then_some(selected)
    } else {
        current
    };
    if change.operation == GoalScheduleOperation::Edit && matching.is_empty() {
        return Err(Resp::err(
            "GOAL_SCHEDULE_TARGET_NOT_FOUND: edit requires an existing exact goal",
        ));
    }
    if let Some(expected) = change.expected_subscription_id.as_deref() {
        if current
            .or_else(|| matching.first())
            .is_none_or(|selected| selected.id != expected)
        {
            return Err(Resp::err("GOAL_SCHEDULE_SELECTION_CHANGED: read the current exact batch before retrying the edit"));
        }
    }
    if change.operation == GoalScheduleOperation::Edit && current.is_none() {
        return terminal_goal_edit_schedule(change, target, &subject, &matching[0], now);
    }
    if change.operation == GoalScheduleOperation::Derived
        && task_id.is_some()
        && !matching.is_empty()
        && matching.iter().all(|subscription| {
            matches!(
                effective_goal_schedule_status(subscription, now),
                "cancelled" | "consumed" | "expired" | "suppressed"
            )
        })
    {
        return Ok((Vec::new(), Resp::data(serde_json::Value::Null)));
    }
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
        let projection = notification_route_projection(worker);
        let route_available = !projection.0.is_empty() && !projection.1.is_empty();
        let existing = current.or_else(|| {
            // A task-linked schedule keeps its progress while the owner rebinds.
            matching.iter().find(|subscription| {
                task_id.is_some()
                    && effective_goal_schedule_status(subscription, now) == "waiting-owner"
                    && subscription.periodic_declared
            })
        });
        // Older callers update an existing periodic batch without --at-ms.
        // A start carrying an initial trigger and a one-shot reconcile retain
        // their policy. The explicit edit operation handles both shapes.
        let retain_policy = existing.is_some_and(|old| {
            change.operation == GoalScheduleOperation::Rearm
                || (change.operation == GoalScheduleOperation::Explicit
                    && (change.trigger_ms.is_some() || !old.periodic_declared))
        });
        let interval = change
            .interval_ms
            .filter(|_| !retain_policy)
            .or_else(|| existing.and_then(|old| old.interval_ms));
        if interval.is_some_and(|interval| interval <= 0) {
            return Err(Resp::err("interval_ms must be greater than zero"));
        }
        if !retain_policy && interval.is_none() && change.repeat_count.is_some() {
            return Err(Resp::err(
                "GOAL_SCHEDULE_PERIOD_INCOMPLETE: repeat requires interval_ms",
            ));
        }
        let repeat = if interval.is_some() {
            change
                .repeat_count
                .filter(|_| !retain_policy)
                .or_else(|| existing.map(|old| old.repeat_count))
                .unwrap_or(100)
        } else {
            1
        };
        if !(1..=state::MAX_NOTIFICATION_REPEATS).contains(&repeat) {
            return Err(Resp::err("repeat_count must be between 1 and 100"));
        }
        let ttl = change.ttl_seconds.filter(|_| !retain_policy);
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
            Some(old) if old.interval_ms == interval => old
                .trigger_ms
                .or_else(|| old.trigger_times_ms.first().copied()),
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
        let mut subscription = existing
            .cloned()
            .unwrap_or_else(|| NotificationSubscription {
                id: format!("sub-{}", gen_msg_id()),
                worker_id: target.into(),
                event: "deadline".into(),
                subject: Some(subject.clone()),
                target: String::new(),
                method: String::new(),
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
        if subscription.worker_id != target {
            superseded.extend(retractable_messages_for_subscription(
                state,
                &subscription.id,
            ));
        }
        subscription.worker_id = target.into();
        if change.operation != GoalScheduleOperation::Edit
            && (change.operation != GoalScheduleOperation::Rearm || existing.is_none())
            && route_available
        {
            let (method, address) = projection;
            subscription.method = method;
            subscription.target = address;
        }
        subscription.subject = Some(subject);
        subscription.trigger_ms = Some(trigger);
        subscription.expires_ms = expiry;
        subscription.interval_ms = interval;
        subscription.repeat_count = repeat;
        subscription.periodic_declared = existing.map_or(interval.is_some(), |old| {
            old.periodic_declared
                || (change.operation == GoalScheduleOperation::Edit && change.interval_ms.is_some())
        });
        subscription.status = if subscription.fired_count >= repeat {
            "consumed"
        } else if change.operation == GoalScheduleOperation::Edit
            && existing.is_some_and(|old| old.status == "waiting-owner")
        {
            "waiting-owner"
        } else if existing.is_some_and(|old| old.status == "transport-lost")
            && (change.operation == GoalScheduleOperation::Edit || !route_available)
        {
            "transport-lost"
        } else {
            "armed"
        }
        .into();
        if subscription.status == "armed"
            && existing.is_none_or(|old| old.status != "armed" || old.worker_id != target)
        {
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
        }
        if subscription.status == "consumed" {
            superseded.extend(retractable_messages_for_subscription(
                state,
                &subscription.id,
            ));
        }
        if !matches!(
            subscription.status.as_str(),
            "transport-lost" | "waiting-owner"
        ) {
            subscription.status_reason = None;
        }
        subscription.goal_content = Some(GoalScheduleBinding {
            source: change
                .content
                .clone()
                .filter(|_| !retain_policy)
                .or_else(|| {
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
    for old in matching.iter().filter(|_| {
        change.operation != GoalScheduleOperation::Edit
            && !(change.desired == "active"
                && current.is_some()
                && matches!(
                    change.operation,
                    GoalScheduleOperation::Explicit | GoalScheduleOperation::Rearm
                )
                && task_id.is_none())
    }) {
        if active.as_ref().is_some_and(|current| current.id == old.id) {
            continue;
        }
        if matches!(
            effective_goal_schedule_status(old, now),
            "armed" | "transport-lost" | "waiting-owner"
        ) {
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
        superseded.extend(retractable_messages_for_subscription(state, &old.id));
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
    let ended_subscription = if change.desired == "ended" {
        current.or_else(|| matching.first()).map(|old| {
            let mut selection = old.clone();
            selection.status = effective_goal_schedule_status(old, now).into();
            if retired.contains(&old.id) {
                selection.status = "cancelled".into();
                selection.status_reason = Some(
                    change
                        .reason
                        .clone()
                        .unwrap_or_else(|| "goal-schedule-ended".into()),
                );
                selection.updated_ms = now;
            }
            selection
        })
    } else {
        None
    };
    let response = Resp::data(json!({
        "goal_id": change.goal_id, "goal_owner": (!change.goal_owner.is_empty()).then_some(&change.goal_owner), "desired": change.desired,
        "status": active.as_ref().map_or("ended", |subscription| subscription.status.as_str()),
        "one_shot": active.as_ref().is_some_and(|subscription| !subscription.periodic_declared),
        "subscription": active, "ended_subscription": ended_subscription,
        "retired_subscription_ids": retired, "superseded_message_ids": superseded,
        "deduplicated": events.is_empty(), "max_repeat_count": state::MAX_NOTIFICATION_REPEATS,
    }));
    Ok((events, response))
}

fn terminal_goal_edit_schedule(
    change: &GoalScheduleChange,
    target: &str,
    subject: &str,
    existing: &NotificationSubscription,
    now: i64,
) -> Result<(Vec<Event>, Resp), Resp> {
    if let Some(GoalScheduleContent::Inline { content }) = &change.content {
        if content.trim().is_empty() {
            return Err(Resp::err("GOAL_SCHEDULE_CONTENT_REQUIRED"));
        }
    }
    if change.interval_ms.is_some_and(|interval| interval <= 0) {
        return Err(Resp::err("interval_ms must be greater than zero"));
    }
    if change
        .repeat_count
        .is_some_and(|repeat| !(1..=state::MAX_NOTIFICATION_REPEATS).contains(&repeat))
    {
        return Err(Resp::err("repeat_count must be between 1 and 100"));
    }
    if change
        .ttl_seconds
        .is_some_and(|ttl| ttl == 0 || ttl > MAX_NOTIFICATION_TTL_SECONDS)
    {
        return Err(Resp::err(format!(
            "ttl_seconds must be between 1 and {MAX_NOTIFICATION_TTL_SECONDS}"
        )));
    }
    let interval = change.interval_ms.or(existing.interval_ms);
    if interval.is_none() && change.repeat_count.is_some() {
        return Err(Resp::err(
            "GOAL_SCHEDULE_PERIOD_INCOMPLETE: repeat requires interval_ms",
        ));
    }

    let mut subscription = existing.clone();
    subscription.subject = Some(subject.to_owned());
    subscription.status = effective_goal_schedule_status(existing, now).into();
    subscription.worker_id = target.into();
    subscription.interval_ms = interval;
    subscription.repeat_count = change.repeat_count.unwrap_or(existing.repeat_count);
    subscription.periodic_declared = existing.periodic_declared || change.interval_ms.is_some();
    if let Some(content) = change.content.clone() {
        let binding = subscription
            .goal_content
            .get_or_insert_with(|| GoalScheduleBinding {
                source: None,
                ttl_seconds: change.ttl_seconds.unwrap_or(0),
            });
        binding.source = Some(content);
    }
    if let Some(ttl_seconds) = change.ttl_seconds {
        let binding = subscription
            .goal_content
            .get_or_insert_with(|| GoalScheduleBinding {
                source: change.content.clone(),
                ttl_seconds,
            });
        binding.ttl_seconds = ttl_seconds;
    }
    let changed = &subscription != existing;
    let mut events = Vec::new();
    if changed {
        subscription.updated_ms = now;
        events.push(Event::NotificationSubscribed {
            subscription: subscription.clone(),
        });
    }
    let response = Resp::data(json!({
        "goal_id": change.goal_id,
        "goal_owner": (!change.goal_owner.is_empty()).then_some(&change.goal_owner),
        "desired": change.desired,
        "status": subscription.status,
        "one_shot": !subscription.periodic_declared,
        "subscription": subscription,
        "retired_subscription_ids": [],
        "superseded_message_ids": [],
        "deduplicated": events.is_empty(),
        "max_repeat_count": state::MAX_NOTIFICATION_REPEATS,
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
    operation: GoalScheduleOperation,
    reason: Option<&str>,
    expected_subscription_id: Option<&str>,
    current_control: Option<&binding_slot::BindingControlSet>,
) -> Result<(Vec<Event>, serde_json::Value), Resp> {
    let Some(spec) = &details.long_horizon else {
        return Ok((Vec::new(), serde_json::Value::Null));
    };
    let subject = format!("goal:task:{}", task.id);
    let existing = state
        .notification_subscriptions
        .values()
        .find(|subscription| subscription.subject.as_deref() == Some(subject.as_str()));
    // The caller supplies a derived target for a first invitation or a
    // committed assignment. The existing subscription supplies its own
    // target when an invitation is withdrawn or the task ends.
    let target = selected
        .or_else(|| existing.map(|subscription| subscription.worker_id.as_str()))
        .or_else(|| {
            (!matches!(task.status.as_str(), "pending" | "invited")).then_some(task.owner.as_str())
        });
    let Some(target) = target else {
        return Ok((Vec::new(), serde_json::Value::Null));
    };
    // A task-linked schedule whose holder retired keeps waiting when the new
    // committed owner has no live transport yet. The business commit for that
    // task must still succeed.
    if !state.workers.contains_key(target)
        && state
            .notification_subscriptions
            .values()
            .any(|subscription| {
                subscription.subject.as_deref() == Some(subject.as_str())
                    && subscription.status == "waiting-owner"
            })
    {
        return Ok((Vec::new(), serde_json::Value::Null));
    }
    let route = server_route_scope(server, state)
        .map_err(Resp::err)?
        .ok_or_else(|| Resp::err("GOAL_SCHEDULE_PROJECT_SCOPE_MISSING"))?;
    let loaded;
    let control = match current_control {
        Some(control) => control,
        None => {
            loaded = binding_store::BindingControlStore::new(server.host_paths.state_root())
                .load()
                .map_err(Resp::err)?;
            &loaded
        }
    };
    let slot = control
        .current_for_worker(&route.project_scope_id, &route.app_scope_id, target)
        .map_err(Resp::err)?;
    if task.status == "invited" && !ended {
        let binding = authoritative_send_binding(state, &route, target).map_err(Resp::err)?;
        if !details.invitation.as_ref().is_some_and(|invitation| {
            invitation.peer_id == target
                && invitation.binding_id == binding.binding_id.as_str()
                && slot
                    .and_then(|slot| slot.active_binding())
                    .is_some_and(|current| &current.binding == binding)
        }) {
            return Err(Resp::err(
                "BOARD_STALE_BINDING: goal target is not the current invited peer",
            ));
        }
    }
    let name = slot.map(|slot| slot.identity_key.name.clone());
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
        expected_subscription_id: expected_subscription_id.map(str::to_owned),
        reason: reason.map(str::to_owned),
        interval_ms: Some(spec.interval_ms),
        repeat_count: Some(spec.repeat_count),
        content: Some(GoalScheduleContent::BoardTask {
            task_id: task.id.clone(),
        }),
        operation,
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

#[derive(PartialEq, Eq)]
enum TaskScheduleSelection {
    Worker(String),
    Name(String),
    WaitingAssignment(Option<String>),
}

fn task_schedule_selection(
    state: &State,
    task: &TaskRec,
    details: &crate::board::BoardTaskDetails,
) -> Option<TaskScheduleSelection> {
    if task.status == "invited" {
        return details
            .invitation
            .as_ref()
            .map(|invitation| TaskScheduleSelection::Worker(invitation.peer_id.clone()));
    }
    let task_owner = || (task.status != "pending").then(|| task.owner.clone());
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
        if phase.status == ticket_state::TicketPhaseStatus::WaitingAssignment {
            return Some(TaskScheduleSelection::WaitingAssignment(task_owner()));
        }
        if phase.status != ticket_state::TicketPhaseStatus::Queued {
            return phase.slot_name.clone().map(TaskScheduleSelection::Name);
        }
    }
    task_owner().map(TaskScheduleSelection::Worker)
}

fn resolve_task_schedule_selection(
    server: &Server,
    state: &State,
    selection: Option<TaskScheduleSelection>,
    control: Option<&binding_slot::BindingControlSet>,
) -> Result<Option<String>, Resp> {
    match selection {
        Some(TaskScheduleSelection::Name(name)) => match control {
            Some(control) => goal_schedule_target_with_control(server, state, &name, control),
            None => goal_schedule_target(server, state, &name),
        }
        .map(Some),
        Some(TaskScheduleSelection::Worker(worker)) => Ok(Some(worker)),
        Some(TaskScheduleSelection::WaitingAssignment(_)) | None => Ok(None),
    }
}

fn task_schedule_target(
    server: &Server,
    state: &State,
    task: &TaskRec,
    details: &crate::board::BoardTaskDetails,
    control: Option<&binding_slot::BindingControlSet>,
) -> Result<Option<String>, Resp> {
    let selection = task_schedule_selection(state, task, details);
    if let Some(TaskScheduleSelection::WaitingAssignment(owner)) = selection {
        // An explicit edit can still address the task's existing owner.
        return Ok(owner);
    }
    resolve_task_schedule_selection(server, state, selection, control)
}

/// Extend the existing candidate-state transaction with linked task schedules.
/// This also covers ticket assignment, decline, reassignment and final closure.
fn task_goal_transition_events(
    server: &Server,
    before: &State,
    after: &State,
    changes: &[Event],
    control: Option<&binding_slot::BindingControlSet>,
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
        let selection = if ended {
            None
        } else {
            task_schedule_selection(after, task, details)
        };
        let old_selection = match (old_task.filter(|_| !ended), old_details) {
            (Some(task), Some(details)) => task_schedule_selection(before, task, details),
            _ => None,
        };
        let form_changed = old_details.and_then(|details| details.long_horizon.as_ref())
            != details.long_horizon.as_ref();
        let first_dispatch = task.status == "invited"
            && details.invitation.as_ref().is_some_and(|invitation| {
                old_details
                    .and_then(|details| details.invitation.as_ref())
                    .is_none_or(|old| old.message_id != invitation.message_id)
            });
        if !ended && !first_dispatch && !form_changed && old_selection == selection {
            continue;
        }
        let target = resolve_task_schedule_selection(server, after, selection, control)?;
        if !ended && target.is_none() {
            continue;
        }
        let (goal_events, _) = task_goal_events(
            server,
            after,
            task,
            details,
            target.as_deref(),
            ended,
            if first_dispatch {
                GoalScheduleOperation::Explicit
            } else {
                GoalScheduleOperation::Derived
            },
            None,
            None,
            control,
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
