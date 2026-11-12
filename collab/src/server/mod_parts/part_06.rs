/// Subject of the current master's standing project patrol. Only this exact
/// event, subject, and interval are ensured idempotently; every other
/// subscription keeps the ordinary finite-schedule semantics.
const PROJECT_PATROL_SUBJECT: &str = "project-patrol";
const PROJECT_PATROL_TARGET_INTERVAL_MS: i64 = 1_500_000;
const PROJECT_PATROL_COMPAT_INTERVAL_MS: i64 = 900_000;

pub(crate) fn is_project_patrol_interval(interval_ms: Option<i64>) -> bool {
    matches!(
        interval_ms,
        Some(PROJECT_PATROL_TARGET_INTERVAL_MS | PROJECT_PATROL_COMPAT_INTERVAL_MS)
    )
}

pub(crate) fn is_exact_project_patrol(subscription: &NotificationSubscription) -> bool {
    subscription.event == "master-idle"
        && subscription.subject.as_deref() == Some(PROJECT_PATROL_SUBJECT)
        && is_project_patrol_interval(subscription.interval_ms)
}

/// The current master's one standing project patrol record.
///
/// Callers that move or recover this resource must share this selector so the
/// exact event/subject/interval contract never drifts between the cancel entry,
/// the subscribe rearm, the daemon auto-recovery, and the name handoff.
/// Historical expired records never mask the current cancelled or live record.
pub(crate) fn exact_project_patrol<'a>(
    subscriptions: &'a std::collections::HashMap<String, NotificationSubscription>,
    worker_id: &str,
    now: i64,
) -> Option<&'a NotificationSubscription> {
    subscriptions
        .values()
        .filter(|subscription| {
            subscription.worker_id == worker_id
                && is_exact_project_patrol(subscription)
                && (subscription.status == "cancelled"
                    || (subscription.status == "armed" && subscription.expires_ms > now))
        })
        .min_by_key(|subscription| (subscription.created_ms, subscription.id.clone()))
}

fn canonical_worker_project(state: &State, worker_id: &str) -> Option<String> {
    state
        .workers
        .get(worker_id)
        .and_then(|worker| std::fs::canonicalize(&worker.cwd).ok())
        .map(|root| root.to_string_lossy().into_owned())
}

fn project_task_ids(state: &State, project: &str) -> Vec<String> {
    let mut task_ids: Vec<_> = state
        .tasks
        .values()
        .filter(|task| canonical_worker_project(state, &task.owner).as_deref() == Some(project))
        .map(|task| task.id.clone())
        .collect();
    task_ids.sort();
    task_ids
}

fn project_patrol_eligibility(
    server: &Server,
    state: &State,
) -> Result<serde_json::Value, String> {
    let project = route_scope_for_root(&server.root, state)
        .map_err(|error| format!("PROJECT_PATROL_PROJECT_SCOPE_UNAVAILABLE: {error}"))?
        .ok_or_else(|| "PROJECT_PATROL_PROJECT_SCOPE_UNAVAILABLE: project route is not registered".to_string())?
        .project_scope_id
        .as_str()
        .to_owned();
    let task_ids = project_task_ids(state, &project);
    let mut waiting = Vec::new();
    let mut blocked = Vec::new();
    for task_id in &task_ids {
        let Some(task) = state.tasks.get(task_id) else {
            continue;
        };
        if matches!(task.status.as_str(), "closed" | "cancelled") {
            continue;
        }
        if task.status == "waiting" && task.wait.is_some() {
            waiting.push(task.id.clone());
        } else {
            blocked.push(json!({
                "task_id": task.id,
                "owner": task.owner,
                "status": task.status,
                "wait": task.wait,
            }));
        }
    }
    let progress_revision = state
        .ceo_report_state
        .projects
        .get(&project)
        .map(|progress| progress.revision)
        .unwrap_or(0);
    let eligible = blocked.is_empty() && progress_revision >= 1;
    let blocker = if progress_revision == 0 {
        Some("PROJECT_PATROL_PROGRESS_NOT_RECORDED")
    } else if !blocked.is_empty() {
        Some("PROJECT_PATROL_WORK_REMAINS")
    } else {
        None
    };
    Ok(json!({
        "eligible": eligible,
        "project_scope": project,
        "progress_revision": progress_revision,
        "waiting_task_ids": waiting,
        "blocking_tasks": blocked,
        "blocker": blocker,
    }))
}

fn handle_notification_subscribe(
    server: &Server,
    worker_id: String,
    token: String,
    event: String,
    subject: Option<String>,
    trigger_ms: Option<i64>,
    trigger_times_ms: Vec<i64>,
    interval_ms: Option<i64>,
    repeat_count: u32,
    ttl_seconds: u64,
) -> Resp {
    if !NOTIFICATION_EVENTS.contains(&event.as_str()) {
        return Resp::err(format!(
            "unsupported notification event {}; expected one of {:?}",
            event, NOTIFICATION_EVENTS
        ));
    }
    if ttl_seconds == 0 || ttl_seconds > MAX_NOTIFICATION_TTL_SECONDS {
        return Resp::err(format!(
            "ttl_seconds must be between 1 and {}",
            MAX_NOTIFICATION_TTL_SECONDS
        ));
    }
    let exact_subject_required = event != "direct-message";
    if exact_subject_required != subject.as_deref().is_some_and(|value| !value.is_empty()) {
        return Resp::err(if exact_subject_required {
            "this notification event requires a non-empty exact subject"
        } else {
            "direct-message subscription must not specify a subject"
        });
    }
    if event != "deadline"
        && event != "master-idle"
        && (trigger_ms.is_some()
            || !trigger_times_ms.is_empty()
            || interval_ms.is_some()
            || repeat_count != 1)
    {
        return Resp::err("schedule options are valid only for deadline subscriptions");
    }
    if event == "deadline" && trigger_ms.is_some() && !trigger_times_ms.is_empty() {
        return Resp::err("use at-ms or trigger-ms, not both");
    }
    let goal_deadline = subject
        .as_deref()
        .is_some_and(|value| value.starts_with("goal:"));
    if event == "deadline"
        && goal_deadline
        && (interval_ms.is_some()
            || repeat_count != 1
            || trigger_times_ms.len() > 1
            || (trigger_ms.is_none() && trigger_times_ms.is_empty()))
    {
        return Resp::err("goal deadline subscriptions are one-shot and require one at-ms trigger");
    }
    let now = now_ms();
    let expires_ms = now.saturating_add((ttl_seconds as i64).saturating_mul(1000));
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, &worker_id, &token) {
        return error;
    }
    if event == "deadline" && is_managed_subagent(&state, &worker_id) {
        return Resp::err("managed subworkers cannot create independent deadline schedules");
    }
    if event == "master-idle" {
        // Only project-master idle reminders follow the current typed grant.
        // Each ordinary peer owns its own goal deadlines.
        // Whether that holder is reachable is a separate communication fact and
        // must not decide authority.
        let holder = match current_master_holder(server, &state) {
            Ok(master) => master,
            Err(error) => return Resp::err(error),
        };
        if holder.as_deref() != Some(worker_id.as_str()) {
            return Resp::err("master-idle subscription requires the current registered master");
        }
    }
    let Some(worker) = state.workers.get(&worker_id).cloned() else {
        return Resp::err("notification subscription requires a registered worker");
    };
    let Some(transport) = selected_transport_for_worker(&worker) else {
        return Resp::err("registered worker has no server-selected transport");
    };
    let Some(thread_id) = notification_target(&transport) else {
        return Resp::err("selected transport has no route address");
    };
    let target = thread_id.into_owned();
    if goal_deadline {
        let requested_key = trigger_ms
            .or_else(|| trigger_times_ms.first().copied())
            .and_then(|trigger| {
                subject
                    .clone()
                    .map(|subject| (worker_id.clone(), subject, trigger))
            });
        if let Some(mut existing) = state
            .notification_subscriptions
            .values()
            .filter(|subscription| {
                goal_deadline_key(subscription).as_ref() == requested_key.as_ref()
                    && matches!(subscription.status.as_str(), "armed" | "consumed")
                    && subscription.expires_ms > now
            })
            .min_by_key(|subscription| (subscription.created_ms, subscription.id.clone()))
            .cloned()
        {
            if existing.method != transport.kind.as_str() || existing.target != target {
                existing.method = transport.kind.as_str().into();
                existing.target = target.clone();
                existing.updated_ms = now;
                if let Err(error) = server.commit_locked(
                    &mut state,
                    &[Event::NotificationSubscribed {
                        subscription: existing.clone(),
                    }],
                ) {
                    return Resp::err(format!("NOTIFICATION_DURABILITY_FAILED: {error}"));
                }
            }
            return Resp::data(json!({
                "subscription": existing,
                "one_shot": true,
                "max_repeat_count": crate::server::state::MAX_NOTIFICATION_REPEATS,
                "deduplicated": true,
            }));
        }
    }
    if event == "master-idle"
        && subject.as_deref() == Some(PROJECT_PATROL_SUBJECT)
        && is_project_patrol_interval(interval_ms)
        && trigger_ms.is_none() && trigger_times_ms.is_empty()
        && (1..=crate::server::state::MAX_NOTIFICATION_REPEATS).contains(&repeat_count)
    {
        let existing =
            exact_project_patrol(&state.notification_subscriptions, &worker_id, now).cloned();
        if let Some(mut existing) = existing {
            let was_cancelled = existing.status == "cancelled";
            let transport_changed =
                existing.method != transport.kind.as_str() || existing.target != target;
            if was_cancelled {
                existing.status = "armed".into();
                existing.status_reason = None;
                existing.interval_ms = interval_ms;
                existing.trigger_ms = Some(now.saturating_add(
                    interval_ms.unwrap_or(PROJECT_PATROL_TARGET_INTERVAL_MS),
                ));
                existing.repeat_count = existing
                    .repeat_count
                    .max(existing.fired_count.saturating_add(100));
                existing.expires_ms = now.saturating_add(
                    (ttl_seconds as i64).saturating_mul(1000),
                );
            }
            if was_cancelled || transport_changed {
                existing.method = transport.kind.as_str().into();
                existing.target = target;
                existing.updated_ms = now;
                if let Err(error) = server.commit_locked(
                    &mut state,
                    &[Event::NotificationSubscribed {
                        subscription: existing.clone(),
                    }],
                ) {
                    return Resp::err(format!("NOTIFICATION_DURABILITY_FAILED: {error}"));
                }
            }
            return Resp::data(json!({
                "subscription": existing,
                "one_shot": false,
                "max_repeat_count": crate::server::state::MAX_NOTIFICATION_REPEATS,
                "deduplicated": !was_cancelled,
                "changed": was_cancelled || transport_changed,
                "status": if was_cancelled { "rearmed" } else { "armed" },
            }));
        }
    }
    let active = state
        .notification_subscriptions
        .values()
        .filter(|s| s.worker_id == worker_id && s.status == "armed")
        .count();
    if active >= MAX_ACTIVE_SUBSCRIPTIONS_PER_WORKER {
        return Resp::err("maximum 3 active subscriptions per agent");
    }
    if event == "master-idle" {
        if trigger_ms.is_some() || !trigger_times_ms.is_empty() {
            return Resp::err("master-idle requires a recurring interval, not an absolute trigger");
        }
        if !is_project_patrol_interval(interval_ms) && !matches!(interval_ms, Some(3_600_000)) {
            return Resp::err(
                "master-idle interval must be project-patrol 1500000, compatibility 900000, or 3600000 ms",
            );
        }
        if repeat_count == 0 || repeat_count > crate::server::state::MAX_NOTIFICATION_REPEATS {
            return Resp::err("repeat_count must be between 1 and 100");
        }
    }
    if event == "deadline" {
        if interval_ms.is_some() && (!trigger_times_ms.is_empty() || trigger_ms.is_some()) {
            return Resp::err("periodic schedule cannot include an absolute time list");
        }
        if interval_ms.is_none() && trigger_times_ms.is_empty() && trigger_ms.is_none() {
            return Resp::err("deadline requires at-ms or every-ms");
        }
        if repeat_count == 0 || repeat_count > crate::server::state::MAX_NOTIFICATION_REPEATS {
            return Resp::err("repeat_count must be between 1 and 100");
        }
        if interval_ms.is_some_and(|ms| ms <= 0) {
            return Resp::err("every-ms must be positive");
        }
        if interval_ms.is_some()
            && trigger_times_ms.is_empty()
            && trigger_ms.is_none()
            && repeat_count == 1
        {}
        if !trigger_times_ms.is_empty() && (interval_ms.is_some() || repeat_count != 1) {
            return Resp::err(
                "absolute schedule uses at-ms values and repeat_count is their length",
            );
        }
        let times = if trigger_times_ms.is_empty() {
            trigger_ms.into_iter().collect()
        } else {
            trigger_times_ms.clone()
        };
        if times.len() > crate::server::state::MAX_NOTIFICATION_REPEATS as usize {
            return Resp::err("absolute schedule supports at most 100 times");
        }
        if times
            .iter()
            .any(|trigger| *trigger <= now || *trigger >= expires_ms)
        {
            return Resp::err("absolute trigger times must be in the future and before expiry");
        }
        if interval_ms.is_some_and(|ms| now.saturating_add(ms) >= expires_ms) {
            return Resp::err("every-ms must fire before subscription expiry");
        }
    }
    let id = format!("sub-{}", gen_msg_id());
    let subscription = NotificationSubscription {
        id: id.clone(),
        worker_id,
        event,
        subject,
        target,
        method: transport.kind.as_str().into(),
        trigger_ms,
        trigger_times_ms,
        interval_ms,
        repeat_count,
        fired_count: 0,
        expires_ms,
        status: "armed".into(),
        created_ms: now,
        updated_ms: now,
        status_reason: None,
        patrol_idle_since_ms: None,
        patrol_last_push_ms: 0,
        periodic_declared: false,
        goal_content: None,
    };
    if let Err(error) = server.commit_locked(
        &mut state,
        &[Event::NotificationSubscribed {
            subscription: subscription.clone(),
        }],
    ) {
        return Resp::err(format!("NOTIFICATION_DURABILITY_FAILED: {error}"));
    }
    Resp::data(
        json!({"subscription": subscription, "one_shot": goal_deadline, "max_repeat_count": crate::server::state::MAX_NOTIFICATION_REPEATS}),
    )
}

fn handle_notification_status(server: &Server, worker_id: String, token: String) -> Resp {
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
        Ok(control) => control, Err(error) => return Resp::err(error),
    };
    let state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, &worker_id, &token) {
        return error;
    }
    let route = match server_route_scope(server, &state) { Ok(route) => route, Err(error) => return Resp::err(error) };
    let owner_key = match state::board_patrol_current_owner_key(&state, &control, route.as_ref(), &worker_id) {
        Ok(Some(name)) => name, Ok(None) => return Resp::err("IDENTITY_MAP_NOT_FOUND: patrol owner has no current name"), Err(error) => return Resp::err(error),
    };
    let mut subscriptions: Vec<&NotificationSubscription> = state
        .notification_subscriptions
        .values()
        .filter(|subscription| subscription.worker_id == worker_id)
        .collect();
    subscriptions.sort_by_key(|subscription| (subscription.created_ms, &subscription.id));
    let is_current_master = current_master_holder(server, &state)
        .ok()
        .flatten()
        .as_deref()
        == Some(worker_id.as_str());
    let mut project_board_patrol: Vec<(String, String, serde_json::Value)> = Vec::new();
    if is_current_master {
        for current_worker_id in state.workers.keys() {
            let current_owner_key = match state::board_patrol_current_owner_key(&state, &control, route.as_ref(), current_worker_id) {
                Ok(Some(name)) => name, Ok(None) => continue, Err(error) => return Resp::err(error),
            };
            let eligible_task_ids = match state::board_patrol_eligible_task_ids(&state, &control, route.as_ref(), current_worker_id) {
                Ok(Some(tasks)) => tasks, Ok(None) => continue, Err(error) => return Resp::err(error),
            };
            let owned_unclosed = crate::server::state::board_patrol_owned_unclosed_task_ids(
                &state,
                current_worker_id,
                &current_owner_key,
            );
            let mut cancelled_records: Vec<_> = state
                .cancelled_board_patrol_tasks
                .values()
                .filter(|record| {
                    record.owner_key == current_owner_key
                        && owned_unclosed.contains(&record.task_id)
                })
                .collect();
            cancelled_records.sort_by(|left, right| left.task_id.cmp(&right.task_id));
            let cancelled_tasks: Vec<serde_json::Value> = cancelled_records
                .into_iter()
                .map(|record| {
                    json!({
                        "task_id": record.task_id.clone(),
                        "reason": record.reason.clone(),
                        "resume_condition": record.resume_condition.clone(),
                        "cancelled_ms": record.cancelled_ms,
                    })
                })
                .collect();
            project_board_patrol.push((
                current_owner_key.clone(),
                current_worker_id.clone(),
                json!({
                    "worker_id": current_worker_id,
                    "owner_key": current_owner_key,
                    "eligible_task_ids": eligible_task_ids,
                    "cancelled_tasks": cancelled_tasks,
                }),
            ));
        }
        project_board_patrol.sort_by(|left, right| {
            (left.0.as_str(), left.1.as_str()).cmp(&(right.0.as_str(), right.1.as_str()))
        });
    }
    let owned_unclosed =
        crate::server::state::board_patrol_owned_unclosed_task_ids(&state, &worker_id, &owner_key);
    let mut cancelled_tasks: Vec<String> = state
        .cancelled_board_patrol_tasks
        .values()
        .filter(|record| record.owner_key == owner_key)
        .map(|record| record.task_id.clone())
        .collect();
    cancelled_tasks.sort();
    let eligible_task_ids: Vec<String> = owned_unclosed
        .iter()
        .filter(|task_id| {
            !state
                .cancelled_board_patrol_tasks
                .get(*task_id)
                .is_some_and(|record| record.owner_key == owner_key)
        })
        .cloned()
        .collect();
    let mut body = json!({
        "subscriptions": subscriptions,
        "board_patrol": {
            "owner_key": owner_key,
            "cancelled_tasks": cancelled_tasks,
            "eligible_task_ids": eligible_task_ids,
            "interval_ms": crate::board_patrol_policy::BOARD_IDLE_PATROL_INTERVAL_MS,
            "periodic_fallback": true,
            "realtime_connected": false,
        }
    });
    if is_current_master {
        body["project_board_patrol"] = serde_json::json!(project_board_patrol
            .into_iter()
            .map(|(_, _, record)| record)
            .collect::<Vec<_>>());
        let project_patrol =
            exact_project_patrol(&state.notification_subscriptions, &worker_id, now_ms());
        let eligibility = match project_patrol_eligibility(server, &state) {
            Ok(eligibility) => eligibility,
            Err(error) => json!({
                "eligible": false,
                "blocker": "PROJECT_PATROL_PROJECT_SCOPE_UNAVAILABLE",
                "error": error,
            }),
        };
        body["project_patrol"] = match project_patrol {
            Some(subscription) => {
                let status = if subscription.status == "armed"
                    && subscription.expires_ms <= now_ms()
                {
                    "expired"
                } else {
                    subscription.status.as_str()
                };
                json!({
                "status": status,
                "armed": status == "armed",
                "owner_key": owner_key,
                "worker_id": subscription.worker_id,
                "session_id": state.workers.get(&worker_id)
                    .and_then(|worker| worker.transport.as_ref())
                    .and_then(|transport| transport.session_id.clone()),
                "subscription_id": subscription.id,
                "event": subscription.event,
                "subject": subscription.subject,
                "method": subscription.method,
                "target": subscription.target,
                "interval_ms": subscription.interval_ms,
                "target_interval_ms": PROJECT_PATROL_TARGET_INTERVAL_MS,
                "cadence_runtime_gap": if subscription.interval_ms
                    == Some(PROJECT_PATROL_COMPAT_INTERVAL_MS)
                {
                    json!("PATROL_CADENCE_RUNTIME_GAP")
                } else {
                    serde_json::Value::Null
                },
                "reason": subscription.status_reason,
                "updated_ms": subscription.updated_ms,
                "eligibility": eligibility,
            })},
            None => json!({
                "status": "missing",
                "armed": false,
                "owner_key": owner_key,
                "worker_id": worker_id,
                "subscription_id": serde_json::Value::Null,
                "event": "master-idle",
                "subject": PROJECT_PATROL_SUBJECT,
                "method": serde_json::Value::Null,
                "target": serde_json::Value::Null,
                "interval_ms": PROJECT_PATROL_TARGET_INTERVAL_MS,
                "target_interval_ms": PROJECT_PATROL_TARGET_INTERVAL_MS,
                "cadence_runtime_gap": serde_json::Value::Null,
                "reason": serde_json::Value::Null,
                "updated_ms": serde_json::Value::Null,
                "eligibility": eligibility,
            }),
        };
    }
    Resp::data(body)
}

fn handle_notification_cancel(
    server: &Server,
    worker_id: String,
    token: String,
    task_ids: Vec<String>,
    event: String,
    reason: Option<String>,
    resume_condition: Option<String>,
) -> Resp {
    if !matches!(event.as_str(), "board-idle" | "master-idle") {
        return Resp::err(
            "NOTIFY_CANCEL_SCOPE_FORBIDDEN: only caller-owned board-idle or current-master project-patrol can be cancelled via collab notify cancel",
        );
    }
    if event == "master-idle" && !task_ids.is_empty() {
        return Resp::err("--task-id is valid only for --event board-idle");
    }
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
        Ok(control) => control, Err(error) => return Resp::err(error),
    };
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, &worker_id, &token) {
        return error;
    }
    let route = match server_route_scope(server, &state) { Ok(route) => route, Err(error) => return Resp::err(error) };
    let owner_key = match state::board_patrol_current_owner_key(&state, &control, route.as_ref(), &worker_id) {
        Ok(Some(name)) => name, Ok(None) => return Resp::err("IDENTITY_MAP_NOT_FOUND: patrol owner has no current name"), Err(error) => return Resp::err(error),
    };
    if event == "master-idle" {
        let holder = match current_master_holder(server, &state) {
            Ok(master) => master,
            Err(error) => return Resp::err(error),
        };
        if holder.as_deref() != Some(worker_id.as_str()) {
            return Resp::err("PROJECT_PATROL_MASTER_REQUIRED: only the current master can cancel this project patrol");
        }
        let now = now_ms();
        let subscription =
            exact_project_patrol(&state.notification_subscriptions, &worker_id, now).cloned();
        let Some(mut subscription) = subscription else {
            return Resp::err("PROJECT_PATROL_NOT_FOUND: no exact standing project-patrol subscription exists");
        };
        if subscription.status == "cancelled" {
            return Resp::data(json!({
                "event": "master-idle",
                "subject": PROJECT_PATROL_SUBJECT,
                "owner_key": owner_key,
                "subscription_id": subscription.id,
                "status": "cancelled",
                "changed": false,
                "reason": subscription.status_reason,
                "resume_condition": "explicit exact subscribe or relevant project work transition",
            }));
        }
        let eligibility = match project_patrol_eligibility(server, &state) {
            Ok(eligibility) => eligibility,
            Err(error) => return Resp::err(error),
        };
        if eligibility["eligible"] != true {
            return Resp::err_data(
                format!(
                    "PROJECT_PATROL_CANCEL_NOT_ELIGIBLE: {}",
                    eligibility["blocker"].as_str().unwrap_or("unknown")
                ),
                eligibility,
            );
        }
        let reason = reason
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("no actionable project work");
        let resume = resume_condition
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("new project work or a durable wait release");
        let status_reason = format!("{reason}; resume_condition={resume}");
        let pending_ids: Vec<String> = state
            .wake_bindings
            .iter()
            .filter_map(|(message_id, subscription_id)| {
                (subscription_id == &subscription.id
                    && state.msgs.get(message_id).is_some_and(|message| {
                        message.state == "pending"
                            && !state
                                .notification_delivery_accepted
                                .contains_key(message_id)
                    }))
                .then(|| message_id.clone())
            })
            .collect();
        let mut events = vec![Event::NotificationSuppressed {
            subscription_id: subscription.id.clone(),
            status: "cancelled".into(),
            reason: status_reason.clone(),
            updated_ms: now,
        }];
        if !pending_ids.is_empty() {
            events.push(Event::Superseded { ids: pending_ids });
        }
        if let Err(error) = server.commit_locked(&mut state, &events) {
            return Resp::err(format!("PROJECT_PATROL_DURABILITY_FAILED: {error}"));
        }
        subscription.status = "cancelled".into();
        subscription.status_reason = Some(status_reason);
        subscription.updated_ms = now;
        return Resp::data(json!({
            "event": "master-idle",
            "subject": PROJECT_PATROL_SUBJECT,
            "owner_key": owner_key,
            "subscription_id": subscription.id,
            "status": "cancelled",
            "changed": true,
            "reason": subscription.status_reason,
            "resume_condition": resume,
            "eligibility": eligibility,
        }));
    }
    let owned_unclosed =
        crate::server::state::board_patrol_owned_unclosed_task_ids(&state, &worker_id, &owner_key);
    let mut next = state.cancelled_board_patrol_tasks.clone();
    let cancelled = match crate::board_patrol_policy::apply_cancel(
        &mut next,
        &owned_unclosed,
        &task_ids,
        &owner_key,
        reason,
        resume_condition,
        now_ms(),
    ) {
        Ok(cancelled) => cancelled,
        Err(error) => return Resp::err(error),
    };
    let events: Vec<Event> = cancelled
        .iter()
        .filter_map(|task_id| {
            next.get(task_id)
                .cloned()
                .map(|record| Event::BoardPatrolCancelled { record })
        })
        .collect();
    if !events.is_empty() {
        if let Err(error) = server.commit_locked(&mut state, &events) {
            return Resp::err(format!("BOARD_PATROL_DURABILITY_FAILED: {error}"));
        }
    }
    Resp::data(json!({
        "event": "board-idle",
        "status": "cancelled",
        "cancelled_task_ids": cancelled,
    }))
}

fn handle_notification_board_rearm(
    server: &Server,
    worker_id: String,
    token: String,
    task_ids: Vec<String>,
) -> Resp {
    let control = match binding_store::BindingControlStore::new(server.host_paths.state_root()).load() {
        Ok(control) => control, Err(error) => return Resp::err(error),
    };
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, &worker_id, &token) {
        return error;
    }
    let route = match server_route_scope(server, &state) { Ok(route) => route, Err(error) => return Resp::err(error) };
    let owner_key = match state::board_patrol_current_owner_key(&state, &control, route.as_ref(), &worker_id) {
        Ok(Some(name)) => name, Ok(None) => return Resp::err("IDENTITY_MAP_NOT_FOUND: patrol owner has no current name"), Err(error) => return Resp::err(error),
    };
    let owned_unclosed =
        crate::server::state::board_patrol_owned_unclosed_task_ids(&state, &worker_id, &owner_key);
    let requested: Vec<String> = if task_ids.is_empty() {
        owned_unclosed
            .iter()
            .filter(|task_id| {
                state
                    .cancelled_board_patrol_tasks
                    .get(*task_id)
                    .is_some_and(|record| record.owner_key == owner_key)
            })
            .cloned()
            .collect()
    } else {
        task_ids
            .into_iter()
            .filter(|task_id| owned_unclosed.contains(task_id))
            .collect()
    };
    let mut next = state.cancelled_board_patrol_tasks.clone();
    let rearmed = crate::board_patrol_policy::apply_rearm(&mut next, &requested, &owner_key);
    let events: Vec<Event> = rearmed
        .iter()
        .map(|task_id| Event::BoardPatrolRearmed {
            task_id: task_id.clone(),
            owner_key: owner_key.clone(),
        })
        .collect();
    if !events.is_empty() {
        if let Err(error) = server.commit_locked(&mut state, &events) {
            return Resp::err(format!("BOARD_PATROL_DURABILITY_FAILED: {error}"));
        }
    }
    Resp::data(json!({
        "event": "board-idle",
        "status": "armed",
        "rearmed_task_ids": rearmed,
    }))
}

/// Test helper for an explicit single-shot goal request.
#[cfg(test)]
fn handle_goal_schedule_reconcile(
    server: &Server,
    worker_id: String,
    token: String,
    goal_id: String,
    goal_owner: String,
    desired: String,
    trigger_ms: Option<i64>,
    ttl_seconds: Option<u64>,
    goal_subject: Option<String>,
    reason: Option<String>,
) -> Resp {
    handle_goal_schedule_reconcile_with_period(
        server, worker_id, token, goal_id, goal_owner, desired, trigger_ms, ttl_seconds,
        goal_subject, reason, None, None,
    )
}

/// Test adapter for the same authenticated goal control entry.
#[cfg(test)]
fn handle_goal_schedule_reconcile_with_period(
    server: &Server,
    worker_id: String,
    token: String,
    goal_id: String,
    goal_owner: String,
    desired: String,
    trigger_ms: Option<i64>,
    ttl_seconds: Option<u64>,
    goal_subject: Option<String>,
    reason: Option<String>,
    interval_ms: Option<i64>,
    repeat_count: Option<u32>,
) -> Resp {
    handle_goal_schedule_for_actor(server, &worker_id, &token, GoalScheduleChange {
        goal_id, goal_owner, desired, trigger_ms, ttl_seconds, goal_subject, reason,
        interval_ms, repeat_count, content: None,
        renew_ended: true,
    })
}

fn pending_messages_for_subscription(state: &State, subscription_id: &str) -> Vec<String> {
    state
        .wake_bindings
        .iter()
        .filter_map(|(message_id, bound_subscription)| {
            (bound_subscription == subscription_id
                && state
                    .msgs
                    .get(message_id)
                    .is_some_and(|message| message.state == "pending"))
            .then_some(message_id.clone())
        })
        .collect()
}

/// Reconcile the master goal-due accumulator against the committed outcome of
/// one lifecycle transition. `retired` names the occurrences this transition
/// removes from the recoverable set, so an already-due occurrence being ended
/// or replaced cannot keep the latch raised for itself. Only the master's own
/// due goals are applicable; an unrelated peer goal must not preserve it.
fn cleared_master_goal_due_event(
    state: &State,
    master_worker: &str,
    retired: &[String],
    now: i64,
) -> Option<Event> {
    // A delivery may clear `goal_due` while keeping the notified revision so
    // a same-revision replay cannot raise the latch again. Retiring that
    // occurrence still ends its revision, so a delivered-but-unconsumed due
    // goal cannot leave a stale active revision behind the ended receipt.
    let retired_revision = state
        .master_wake
        .active_goal_revision
        .is_some_and(|revision| {
            retired.iter().any(|subscription_id| {
                state
                    .notification_subscriptions
                    .get(subscription_id)
                    .and_then(crate::server::state::subscription_goal_deadline_revision)
                    == Some(revision)
            })
        });
    if !state.master_wake.goal_due && !retired_revision {
        return None;
    }
    let other_due_goal = state
        .notification_subscriptions
        .values()
        .any(|subscription| {
            subscription.worker_id == master_worker
                && is_goal_deadline(subscription)
                && subscription.status == "armed"
                && !retired.iter().any(|retired| retired == &subscription.id)
                && subscription
                    .trigger_ms
                    .is_some_and(|trigger| trigger <= now)
        });
    if other_due_goal {
        return None;
    }
    let mut accumulator = state.master_wake.clone();
    accumulator.goal_due = false;
    accumulator.active_goal_revision = None;
    accumulator.last_updated_ms = now;
    Some(Event::MasterWakeUpdated { accumulator })
}

// ---------- handlers ----------

fn verify(state: &State, worker_id: &str, token: &str) -> Result<WorkerRec, Resp> {
    match state.workers.get(worker_id) {
        Some(w) if w.token == token => Ok(w.clone()),
        Some(_) => Err(Resp::err(
            "token mismatch: identity does not own this worker_id",
        )),
        None => Err(Resp::err(format!("worker {} not registered", worker_id))),
    }
}

fn prepare_register_typed(
    server: &Server,
    worker_id: &str,
    token: &str,
    transport: &SelectedTransport,
    cwd: &str,
    project_scope: Option<ProjectScopeId>,
    app_scope: Option<AppServerId>,
    reuse_existing: bool,
) -> Result<PreparedRegisterEnvelope, String> {
    match (project_scope, app_scope) {
        (Some(project_scope), Some(app_scope)) => server.prepare_register_envelope_for_scope(
            worker_id,
            token,
            transport,
            project_scope,
            cwd,
            app_scope,
            reuse_existing,
        ),
        (Some(project_scope), None) => {
            let app_scope = AppServerId::new("tui-default").map_err(|error| error.to_string())?;
            server.prepare_register_envelope_for_scope(
                worker_id,
                token,
                transport,
                project_scope,
                cwd,
                app_scope,
                reuse_existing,
            )
        }
        (None, Some(app_scope)) => {
            let project_scope = GlobalState::canonical_project_scope(Path::new(cwd))
                .map_err(|error| error.to_string())?;
            server.prepare_register_envelope_for_scope(
                worker_id,
                token,
                transport,
                project_scope,
                cwd,
                app_scope,
                reuse_existing,
            )
        }
        (None, None) if reuse_existing => {
            let project_scope = GlobalState::canonical_project_scope(Path::new(cwd))
                .map_err(|error| error.to_string())?;
            let app_scope = AppServerId::new("tui-default").map_err(|error| error.to_string())?;
            server.prepare_register_envelope_for_scope(
                worker_id,
                token,
                transport,
                project_scope,
                cwd,
                app_scope,
                true,
            )
        }
        (None, None) => {
            Err("collab registration requires an app scope for a tmux transport".into())
        }
    }
}

fn consume_register_typed(
    server: &Server,
    worker_id: &str,
    transport: &SelectedTransport,
    prepared: PreparedRegisterEnvelope,
    name_tmux_session_name: Option<String>,
    name_pane_anchor: Option<crate::server::name_state::NamePaneAnchor>,
    consumed: Option<state::TypedOutcome>,
) -> Resp {
    let registered_at = match &prepared.typed.command {
        TypedCommand::RegisterWorker { worker, .. } => worker.registered_ms,
    };
    let command = prepared.typed.command.clone();
    let committed = match consumed {
        Some(outcome) => Ok(outcome),
        None => server.consume_register(prepared, None),
    };
    match committed {
        Ok(outcome) => {
            // Map publication already chose the name. Registration never
            // makes a second name or role decision after the business commit.
            let name_resolution = (|| -> Result<(serde_json::Value, binding_slot::BindingControlSet), String> {
                let map = crate::server::binding_store::BindingControlStore::new(server.host_paths.state_root()).load()?;
                let TypedCommand::RegisterWorker { binding, .. } = &command;
                let key = map.key_for_worker(&binding.project_scope, &binding.app_scope_id, worker_id)?
                    .ok_or("IDENTITY_MAP_NOT_FOUND: registration has no published mapping")?;
                let result = json!({"outcome":"registered", "name":key.name,"session_id":binding.session_id});
                Ok((result, map))
            })();
            let (name_resolution, control) = match name_resolution {
                Ok(value) => value,
                Err(error) => return Resp::err_data(error, json!({"registration_committed":true,
                    "command_id":outcome.receipt.command_id.as_str(),"operation_id":outcome.receipt.operation_id.as_str()})),
            };
            let (role_brief, _) = {
                let st = server.state.lock().unwrap();
                let brief = match role_brief(server, &st, &control, worker_id) {
                    Ok(brief) => brief, Err(error) => return Resp::err_data(error, json!({"registration_committed":true})),
                };
                (brief, ())
            };
            Resp::data(json!({
                "worker_id": worker_id,
                "identity_kind": "peer",
                "transport_selected": transport,
                "registered_at": iso(registered_at),
                "role_brief": role_brief,
                "name": name_resolution["name"],
                "session_id": name_resolution["session_id"],
                "name_resolution": name_resolution,
                "typed": true,
                "command_id": outcome.receipt.command_id.as_str(),
                "operation_id": outcome.receipt.operation_id.as_str(),
                "sequence": outcome.receipt.sequence,
                "revision": outcome.receipt.revision,
                "replayed": outcome.replayed,
                "command": command,
            }))
        }
        Err(error) => Resp::err(format!("typed registrar rejected registration: {error}")),
    }
}

/// Prepare then, before consume, hand the exact prepared envelope to
/// `before_consume`. A hook error aborts the operation without consuming, so a
/// failed outer durable binding can never dispatch Register.
fn register_typed_observed(
    server: &Server,
    worker_id: &str,
    token: &str,
    transport: &SelectedTransport,
    cwd: &str,
    project_scope: Option<ProjectScopeId>,
    app_scope: Option<AppServerId>,
    reuse_existing: bool,
    name_tmux_session_name: Option<String>,
    name_pane_anchor: Option<crate::server::name_state::NamePaneAnchor>,
    before_consume: Option<
        &mut dyn FnMut(
            &PreparedRegisterEnvelope,
        ) -> Result<Option<state::TypedOutcome>, String>,
    >,
) -> Resp {
    let prepared = match prepare_register_typed(
        server,
        worker_id,
        token,
        transport,
        cwd,
        project_scope,
        app_scope,
        reuse_existing,
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            return Resp::err(format!("typed registrar failed to build command: {error}"))
        }
    };
    let consumed = match before_consume {
        Some(hook) => match hook(&prepared) {
            Ok(outcome) => outcome,
            Err(error) => return Resp::err(error),
        },
        None => None,
    };
    let response =
        consume_register_typed(
            server,
            worker_id,
            transport,
            prepared,
            name_tmux_session_name,
            name_pane_anchor,
            consumed,
        );
    response
}

/// Ordinary Create has already reserved an exact peer id. Its execution cwd
/// may be a linked worktree; registration and route identity remain the main.
pub(crate) fn register_created_peer(
    manager: &ProjectRuntimeManager,
    server: &Server,
    worker_id: &str,
    token: &str,
    context: &ProjectContext,
    candidate: &crate::proto::AppServerCandidate,
) -> Resp {
    let route = match crate::scope::canonical_route_for_identity(
        &server.host_paths,
        Path::new(&candidate.cwd),
        &context.app_scope_id,
    ) {
        Ok(route) if route.root == PathBuf::from(context.project_scope.as_str()) => route,
        _ => return Resp::err("PEER_LIFECYCLE_CREATE_SCOPE_MISMATCH"),
    };
    let selected = match crate::client::adapters::verify_candidate(candidate) {
        Ok(selected) => selected,
        Err(error) => return Resp::err(format!("APPSERVER_ENDPOINT_REJECTED: {error}")),
    };
    let _identity = manager.identity_gate.lock().unwrap();
    if server.state.lock().unwrap().workers.contains_key(worker_id) {
        return Resp::err("PEER_LIFECYCLE_PEER_RESERVED");
    }
    let main = route.root.to_string_lossy().into_owned();
    let mut effect_id = None;
    let mut before_consume = |prepared: &PreparedRegisterEnvelope| {
        let (id, outcome) = manager.publish_and_consume_register_control(
            server,
            prepared,
            None,
            None,
            None,
        )?;
        effect_id = Some(id);
        Ok(Some(outcome))
    };
    let response = register_typed_observed(
        server,
        worker_id,
        token,
        &selected,
        &main,
        Some(context.project_scope.clone()),
        Some(context.app_scope_id.clone()),
        false,
        None,
        None,
        Some(&mut before_consume),
    );
    if !response.ok {
        return response;
    }
    if let Err(error) = manager.consume_register_effect(effect_id.as_deref()) {
        return Resp::err_data(format!("CONTROL_COMMITTED_EFFECTS_PENDING: {error}"), response.data);
    }
    response
}

pub(crate) fn handle_register_with_app_scope_unfinalized(
    server: &Server,
    worker_id: String,
    token: String,
    cwd: String,
    app_scope: Option<AppServerId>,
    candidates: Option<TransportCandidates>,
) -> Resp {
    handle_register_with_app_scope_inner(
        server, worker_id, token, cwd, app_scope, candidates, false,
    )
}

pub(crate) fn commit_current_thread_route_for_runtime(
    route_owner: &Server,
    runtime: &Server,
    worker_id: &str,
    cwd: &str,
    app_scope: Option<&AppServerId>,
) -> Result<(), String> {
    let project_scope = GlobalState::canonical_project_scope(Path::new(cwd))
        .map_err(|error| format!("ROUTE_TRANSITION_INVALID: {error}"))?;
    let app_scope = app_scope.cloned().unwrap_or_else(|| {
        AppServerId::new("tui-default").expect("static App Server scope must be valid")
    });
    let binding_id = BindingId::new(sanitize_identifier(&format!("binding-{worker_id}")))
        .map_err(|error| format!("ROUTE_TRANSITION_INVALID: {error}"))?;
    let route_scope = RouteScope {
        app_scope_id: app_scope,
        project_scope_id: project_scope,
    };
    let binding = runtime
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(&route_scope, &binding_id)
        .filter(|binding| binding.agent_id.as_str() == worker_id)
        .cloned()
        .ok_or_else(|| {
            format!(
                "ROUTE_TRANSITION_INVALID: successful registration for {worker_id} has no matching runtime binding"
            )
        })?;
    let current = {
        let state = route_owner.state.lock().unwrap();
        if let Some(endpoint) = binding.tmux_endpoint.as_ref() {
            state.global.lookup_unique_tmux_pane_route(endpoint) == Some(&binding)
        } else if let (Some(session), Some(thread)) =
            (&binding.session_id, &binding.native_thread_id)
        {
            state.global.lookup_current_thread_route(session, thread) == Some(&binding)
        } else if binding.session_id.is_some() {
            state.global.current_thread_routes.values().any(|current| current == &binding)
        } else {
            return Err("ROUTE_TRANSITION_INVALID: registration has no route address".into());
        }
    };
    if current {
        return Ok(());
    }
    // A successful registration is live activity, so it is the later writer for
    // its address and for its pane: the reducer evicts whatever claimed either
    // before it. The reconcilers and the replay helper are the paths that must
    // not republish a superseded claim; this path is not one of them.
    route_owner
        .commit_checked(&[Event::GlobalCurrentThreadRouteSet { binding }])
        .map(|_| ())
        .map_err(|error| format!("ROUTE_TRANSITION_DURABILITY_FAILED: {error}"))
}

pub(crate) fn retire_current_thread_route_after_launch_failure(
    route_owner: &Server,
    runtime: &Server,
    worker_id: &str,
    cwd: &str,
    app_scope: Option<&AppServerId>,
) -> Result<(), String> {
    let project_scope = GlobalState::canonical_project_scope(Path::new(cwd))
        .map_err(|error| format!("ROUTE_CLEANUP_INVALID: {error}"))?;
    let route_scope = RouteScope {
        app_scope_id: app_scope.cloned().unwrap_or_else(|| {
            AppServerId::new("tui-default").expect("static App Server scope must be valid")
        }),
        project_scope_id: project_scope,
    };
    let binding_id = BindingId::new(sanitize_identifier(&format!("binding-{worker_id}")))
        .map_err(|error| format!("ROUTE_CLEANUP_INVALID: {error}"))?;
    let binding = runtime
        .state
        .lock()
        .unwrap()
        .global
        .lookup_binding_for(&route_scope, &binding_id)
        .filter(|binding| binding.agent_id.as_str() == worker_id)
        .cloned()
        .ok_or_else(|| format!("ROUTE_CLEANUP_INVALID: no binding exists for {worker_id}"))?;
    route_owner
        .commit_checked(&[Event::GlobalCurrentThreadRouteRetired { binding }])
        .map(|_| ())
        .map_err(|error| format!("ROUTE_CLEANUP_DURABILITY_FAILED: {error}"))
}

fn handle_register_with_app_scope_inner(
    server: &Server,
    worker_id: String,
    token: String,
    cwd: String,
    app_scope: Option<AppServerId>,
    candidates: Option<TransportCandidates>,
    recover_existing: bool,
) -> Resp {
    handle_register_with_app_scope_inner_bound(
        server,
        worker_id,
        token,
        cwd,
        app_scope,
        candidates,
        recover_existing,
        None,
        None,
    )
}

fn handle_register_with_app_scope_inner_bound(
    server: &Server,
    worker_id: String,
    token: String,
    cwd: String,
    app_scope: Option<AppServerId>,
    candidates: Option<TransportCandidates>,
    recover_existing: bool,
    outer_binding: Option<&RegisterOuterBinding>,
    register_control_first: Option<
        &mut dyn FnMut(
            &PreparedRegisterEnvelope,
        ) -> Result<Option<state::TypedOutcome>, String>,
    >,
) -> Resp {
    let candidates = candidates.unwrap_or_default();
    let agent_session = outer_binding.and_then(RegisterOuterBinding::identity_facts)
        .and_then(|facts| facts.session_id.as_deref());
    let selected = match validate_transport_candidates_for_session(server, &candidates, &cwd, agent_session) {
        Ok(selected) => selected,
        Err(error) => return Resp::err(error),
    };
    let st = server.state.lock().unwrap();
    if st.admission_frozen() && !st.workers.contains_key(&worker_id) {
        return Resp::err("PROJECT_RESET_PENDING: only an existing identity may rebind while the committed reset completes");
    }
    if let Some(existing) = st.workers.get(&worker_id).cloned() {
        let existing_route_scope = match existing_route_scope(&st, &worker_id) {
            Ok(scope) => scope,
            Err(error) => return error,
        };
        let existing_project_scope = existing_route_scope
            .as_ref()
            .map(|route| route.project_scope_id.clone());
        let app_scope = app_scope.or_else(|| {
            existing_route_scope
                .as_ref()
                .map(|route| route.app_scope_id.clone())
        });
        // Transport is a server-selected capability, not a permanent worker
        // identity. Re-registering the same session/thread/cwd key is
        // idempotent and keeps the current generation. Any changed key is a
        // rebind: it advances the endpoint generation so every context bound
        // to the old address is fenced.
        //
        // A dsh peer's address is its gateway control socket plus the runtime id
        // the gateway reports, and both live in the transport rather than in the
        // route key: a dsh binding stores `tmux_endpoint` as `None`, so the key
        // alone reduces to (session, agent). Without the address, a peer that
        // came back on a new socket after a gateway restart looked like an
        // idempotent repeat. The first register command was then replayed
        // verbatim, and the dead address stayed in place, so the agent could
        // still send but could never receive again.
        let existing_key = existing_route_scope.as_ref().and_then(|route| {
            st.global
                .lookup_binding_for(
                    route,
                    &BindingId::new(sanitize_identifier(&format!("binding-{worker_id}"))).ok()?,
                )
                .map(|binding| {
                    (
                        binding
                            .session_id
                            .as_ref()
                            .map(|session| session.as_str().to_owned()),
                        binding
                            .native_thread_id
                            .as_ref()
                            .map(|thread| thread.as_str().to_owned()),
                        binding.tmux_endpoint.clone(),
                    )
                })
        });
        let same_transport_address = match selected.kind {
            TransportKind::Dsh | TransportKind::DesktopBridge => {
                selected_transport_for_worker(&existing).is_some_and(|old| {
                    old.kind == selected.kind
                        && old.endpoint == selected.endpoint
                        && old.namespace == selected.namespace
                })
            }
            _ => true,
        };
        let same_runtime_key = same_transport_address
            && existing_key
                .as_ref()
                .is_some_and(|(session, thread, endpoint)| {
                    session.as_deref() == selected.session_id.as_deref()
                        && thread.as_deref() == selected.thread_id.as_deref()
                        && endpoint.as_ref() == selected.tmux_endpoint.as_ref()
                });
        if existing.token != token {
            return Resp::err(format!("TOKEN_MISMATCH: worker {} is registered by another token", worker_id));
        }
        let reuse_existing = same_runtime_key && !recover_existing;
        drop(st);
        let mut resp = register_typed_observed(
            server,
            &worker_id,
            &token,
            &selected,
            &cwd,
            existing_project_scope,
            app_scope,
            reuse_existing,
            outer_binding
                .and_then(|binding| binding.name_tmux_session_name().map(str::to_owned)),
            outer_binding.and_then(|binding| binding.name_pane_anchor().cloned()),
            register_control_first,
        );
        if resp.ok {
            if reuse_existing {
                resp.data["reused"] = json!(true);
            } else {
                resp.data["recovered"] = json!(true);
            }
        }
        return resp;
    }
    drop(st);
    register_typed_observed(
        server,
        &worker_id,
        &token,
        &selected,
        &cwd,
        None,
        app_scope,
        false,
        outer_binding.and_then(|binding| binding.name_tmux_session_name().map(str::to_owned)),
        outer_binding.and_then(|binding| binding.name_pane_anchor().cloned()),
        register_control_first,
    )
}

fn existing_route_scope(state: &State, worker_id: &str) -> Result<Option<RouteScope>, Resp> {
    let mut found = None;
    for project in state.global.projects.values() {
        for binding in project.runtime_bindings.values() {
            if binding.agent_id.as_str() != worker_id {
                continue;
            }
            let route = binding.route_scope();
            if found.as_ref().is_some_and(|scope| scope != &route) {
                return Err(Resp::err(format!(
                    "worker {} has ambiguous registered route scope",
                    worker_id
                )));
            }
            found = Some(route);
        }
    }
    Ok(found)
}

fn route_scope_for_root(root: &Path, state: &State) -> Result<Option<RouteScope>, &'static str> {
    let project_scope = GlobalState::canonical_project_scope(root)
        .map_err(|_| "server project root has no canonical scope")?;
    let Some(project) = state.global.lookup_project(&project_scope) else {
        return Ok(None);
    };
    let mut app_scopes = project
        .runtime_bindings
        .values()
        .map(|binding| binding.app_scope_id.clone())
        .collect::<Vec<_>>();
    app_scopes.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    app_scopes.dedup();
    if app_scopes.is_empty() {
        return match project.registrations.values().next() {
            Some(registration) if project.registrations.len() == 1 => {
                Ok(Some(registration.route_scope()))
            }
            None => Ok(None),
            Some(_) => Err("server project has multiple registered app scopes"),
        };
    }
    if app_scopes.len() != 1 {
        return Err("server project has multiple runtime-bound app scopes");
    }
    Ok(app_scopes.pop().map(|app_scope_id| RouteScope {
        app_scope_id,
        project_scope_id: project_scope,
    }))
}

include!("part_06/master_recovery.rs");
