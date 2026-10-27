/// Subject of the current master's standing project patrol. Only this exact
/// event, subject, and interval are ensured idempotently; every other
/// subscription keeps the ordinary finite-schedule semantics.
const PROJECT_PATROL_SUBJECT: &str = "project-patrol";
const PROJECT_PATROL_INTERVAL_MS: i64 = 900_000;

pub(crate) fn is_exact_project_patrol(subscription: &NotificationSubscription) -> bool {
    subscription.event == "master-idle"
        && subscription.subject.as_deref() == Some(PROJECT_PATROL_SUBJECT)
        && subscription.interval_ms == Some(PROJECT_PATROL_INTERVAL_MS)
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
    let target = thread_id.to_owned();
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
        && interval_ms == Some(crate::board_patrol_policy::BOARD_IDLE_PATROL_INTERVAL_MS)
        && trigger_ms.is_none()
        && trigger_times_ms.is_empty()
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
                existing.trigger_ms = Some(now.saturating_add(PROJECT_PATROL_INTERVAL_MS));
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
        if !matches!(interval_ms, Some(900_000 | 3_600_000)) {
            return Resp::err("master-idle interval must be exactly 900000 or 3600000 ms");
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
    let state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, &worker_id, &token) {
        return error;
    }
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
            let Some(current_owner_key) =
                crate::server::state::board_patrol_current_owner_key(&state, current_worker_id)
            else {
                continue;
            };
            let Some(eligible_task_ids) =
                crate::server::state::board_patrol_eligible_task_ids(&state, current_worker_id)
            else {
                continue;
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
    let owner_key = crate::server::state::board_patrol_owner_key(&state, &worker_id);
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
                "owner_key": crate::server::state::board_patrol_owner_key(&state, &worker_id),
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
                "reason": subscription.status_reason,
                "updated_ms": subscription.updated_ms,
                "eligibility": eligibility,
            })},
            None => json!({
                "status": "missing",
                "armed": false,
                "owner_key": crate::server::state::board_patrol_owner_key(&state, &worker_id),
                "worker_id": worker_id,
                "subscription_id": serde_json::Value::Null,
                "event": "master-idle",
                "subject": PROJECT_PATROL_SUBJECT,
                "method": serde_json::Value::Null,
                "target": serde_json::Value::Null,
                "interval_ms": PROJECT_PATROL_INTERVAL_MS,
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
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, &worker_id, &token) {
        return error;
    }
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
                "owner_key": crate::server::state::board_patrol_owner_key(&state, &worker_id),
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
            "owner_key": crate::server::state::board_patrol_owner_key(&state, &worker_id),
            "subscription_id": subscription.id,
            "status": "cancelled",
            "changed": true,
            "reason": subscription.status_reason,
            "resume_condition": resume,
            "eligibility": eligibility,
        }));
    }
    let owner_key = crate::server::state::board_patrol_owner_key(&state, &worker_id);
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
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, &worker_id, &token) {
        return error;
    }
    let owner_key = crate::server::state::board_patrol_owner_key(&state, &worker_id);
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
    if goal_id.trim().is_empty() {
        return Resp::err("GOAL_SCHEDULE_ID_REQUIRED");
    }
    if goal_owner.trim().is_empty() {
        return Resp::err("GOAL_SCHEDULE_OWNER_REQUIRED");
    }
    if !matches!(desired.as_str(), "active" | "ended") {
        return Resp::err("GOAL_SCHEDULE_DESIRED_INVALID: expected active or ended");
    }
    // A retained historical subject is adopted only as an exact goal:<name>
    // string. It is never interpreted as a subscription ID: matching below
    // still requires the exact subject and the authenticated caller's own
    // stable-name-scoped deadline subscription.
    let retained_subject = match goal_subject.as_deref() {
        None => None,
        Some(raw) if raw.trim().is_empty() => {
            return Resp::err("GOAL_SCHEDULE_SUBJECT_REQUIRED");
        }
        Some(raw) if raw != raw.trim() => {
            return Resp::err(
                "GOAL_SCHEDULE_SUBJECT_NOT_EXACT: subject must not carry surrounding whitespace",
            );
        }
        Some(raw) if !raw.starts_with("goal:") || raw.len() <= "goal:".len() => {
            return Resp::err(
                "GOAL_SCHEDULE_SUBJECT_INVALID: subject must be an exact goal:<name>",
            );
        }
        Some(raw) => Some(raw.to_owned()),
    };
    let now = now_ms();
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, &worker_id, &token) {
        return error;
    }
    let route_scope = match server_route_scope(server, &state) {
        Ok(scope) => scope,
        Err(error) => return Resp::err(error),
    };
    let Some(route_scope) = route_scope else {
        return Resp::err("GOAL_SCHEDULE_PROJECT_SCOPE_MISSING");
    };
    let owns_name = state
        .name_occupancies
        .get(route_scope.project_scope_id.as_str())
        .and_then(|names| names.get(&goal_owner))
        .is_some_and(|holder| holder.worker_id == worker_id);
    if !owns_name {
        return Resp::err(format!(
            "GOAL_SCHEDULE_OWNER_MISMATCH: caller does not own stable name {goal_owner}"
        ));
    }
    let Some(worker) = state.workers.get(&worker_id).cloned() else {
        return Resp::err("goal schedule reconcile requires a registered worker");
    };
    let Some(transport) = selected_transport_for_worker(&worker) else {
        return Resp::err("registered worker has no server-selected transport");
    };
    let canonical_subject = format!("goal:{goal_id}");
    let mut subjects = vec![canonical_subject];
    if let Some(retained) = retained_subject.as_ref() {
        if !subjects.contains(retained) {
            subjects.push(retained.clone());
        }
    }
    let master_holder = current_master_holder(server, &state).ok().flatten();
    let caller_is_master = master_holder.as_deref() == Some(worker_id.as_str());
    let matching = state
        .notification_subscriptions
        .values()
        .filter(|subscription| {
            subscription.worker_id == worker_id
                && subscription.event == "deadline"
                && subscription
                    .subject
                    .as_deref()
                    .is_some_and(|subject| subjects.iter().any(|candidate| candidate == subject))
        })
        .cloned()
        .collect::<Vec<_>>();

    if desired == "active" {
        if is_managed_subagent(&state, &worker_id) {
            return Resp::err("managed subworkers cannot create independent deadline schedules");
        }
        let Some(trigger_ms) = trigger_ms else {
            return Resp::err("GOAL_SCHEDULE_TRIGGER_REQUIRED");
        };
        let Some(ttl_seconds) = ttl_seconds else {
            return Resp::err("GOAL_SCHEDULE_TTL_REQUIRED");
        };
        if ttl_seconds == 0 || ttl_seconds > MAX_NOTIFICATION_TTL_SECONDS {
            return Resp::err(format!(
                "ttl_seconds must be between 1 and {MAX_NOTIFICATION_TTL_SECONDS}"
            ));
        }
        let expires_ms = now.saturating_add((ttl_seconds as i64).saturating_mul(1000));
        if trigger_ms <= now || trigger_ms >= expires_ms {
            return Resp::err("goal deadline trigger must be in the future and before expiry");
        }
        let Some(target) = notification_target(&transport) else {
            return Resp::err("selected transport has no route address");
        };
        if let Some(mut existing) = matching
            .iter()
            .find(|subscription| {
                subscription.status == "armed"
                    && subscription.trigger_ms == Some(trigger_ms)
                    && subscription.trigger_times_ms.is_empty()
                    && subscription.interval_ms.is_none()
                    && subscription.repeat_count == 1
                    && subscription.expires_ms > now
            })
            .cloned()
        {
            let retained_id = existing.id.clone();
            let mut events = Vec::new();
            let mut retired = Vec::new();
            let mut superseded = Vec::new();
            if existing.method != transport.kind.as_str() || existing.target != target {
                existing.method = transport.kind.as_str().into();
                existing.target = target.to_owned();
                existing.updated_ms = now;
                events.push(Event::NotificationSubscribed {
                    subscription: existing.clone(),
                });
            }
            for subscription in matching.iter().filter(|subscription| {
                subscription.id != retained_id
                    && matches!(
                        subscription.status.as_str(),
                        "armed" | "consumed" | "transport-lost"
                    )
            }) {
                retired.push(subscription.id.clone());
                if matches!(subscription.status.as_str(), "armed" | "transport-lost") {
                    events.push(Event::NotificationSuppressed {
                        subscription_id: subscription.id.clone(),
                        status: "cancelled".into(),
                        reason: "goal-schedule-rearmed".into(),
                        updated_ms: now,
                    });
                }
                superseded.extend(pending_messages_for_subscription(&state, &subscription.id));
            }
            superseded.sort();
            superseded.dedup();
            if !superseded.is_empty() {
                events.push(Event::Superseded {
                    ids: superseded.clone(),
                });
            }
            if caller_is_master {
                if let Some(event) =
                    cleared_master_goal_due_event(&state, &worker_id, &retired, now)
                {
                    events.push(event);
                }
            }
            if !events.is_empty() {
                if let Err(error) = server.commit_locked(&mut state, &events) {
                    return Resp::err(format!("NOTIFICATION_DURABILITY_FAILED: {error}"));
                }
            }
            return Resp::data(json!({
                "goal_id": goal_id,
                "goal_owner": goal_owner,
                "desired": "active",
                "status": "armed",
                "subscription": existing,
                "retired_subscription_ids": retired,
                "superseded_message_ids": superseded,
                "deduplicated": events.is_empty(),
                "one_shot": true,
                "max_repeat_count": crate::server::state::MAX_NOTIFICATION_REPEATS,
            }));
        }
        let mut events = Vec::new();
        let mut retired = Vec::new();
        let mut superseded = Vec::new();
        for subscription in matching.iter().filter(|subscription| {
            matches!(
                subscription.status.as_str(),
                "armed" | "consumed" | "transport-lost"
            )
        }) {
            retired.push(subscription.id.clone());
            if matches!(subscription.status.as_str(), "armed" | "transport-lost") {
                events.push(Event::NotificationSuppressed {
                    subscription_id: subscription.id.clone(),
                    status: "cancelled".into(),
                    reason: "goal-schedule-rearmed".into(),
                    updated_ms: now,
                });
            }
            superseded.extend(pending_messages_for_subscription(&state, &subscription.id));
        }
        if !superseded.is_empty() {
            events.push(Event::Superseded {
                ids: superseded.clone(),
            });
        }
        let subscription = NotificationSubscription {
            id: format!("sub-{}", gen_msg_id()),
            worker_id: worker_id.clone(),
            event: "deadline".into(),
            subject: Some(format!("goal:{goal_id}")),
            target: target.to_owned(),
            method: transport.kind.as_str().into(),
            trigger_ms: Some(trigger_ms),
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms,
            status: "armed".into(),
            created_ms: now,
            updated_ms: now,
            status_reason: None,
        };
        events.push(Event::NotificationSubscribed {
            subscription: subscription.clone(),
        });
        if caller_is_master {
            if let Some(event) = cleared_master_goal_due_event(&state, &worker_id, &retired, now) {
                events.push(event);
            }
        }
        if let Err(error) = server.commit_locked(&mut state, &events) {
            return Resp::err(format!("NOTIFICATION_DURABILITY_FAILED: {error}"));
        }
        return Resp::data(json!({
            "goal_id": goal_id,
            "goal_owner": goal_owner,
            "desired": "active",
            "status": "armed",
            "subscription": subscription,
            "retired_subscription_ids": retired,
            "superseded_message_ids": superseded,
            "deduplicated": false,
            "one_shot": true,
            "max_repeat_count": crate::server::state::MAX_NOTIFICATION_REPEATS,
        }));
    }

    let mut events = Vec::new();
    let mut retired = Vec::new();
    let mut superseded = Vec::new();
    for subscription in &matching {
        if matches!(
            subscription.status.as_str(),
            "armed" | "consumed" | "transport-lost"
        ) {
            retired.push(subscription.id.clone());
        }
        if matches!(subscription.status.as_str(), "armed" | "transport-lost") {
            events.push(Event::NotificationSuppressed {
                subscription_id: subscription.id.clone(),
                status: "cancelled".into(),
                reason: reason
                    .clone()
                    .unwrap_or_else(|| "goal-schedule-ended".into()),
                updated_ms: now,
            });
        }
        superseded.extend(pending_messages_for_subscription(&state, &subscription.id));
    }
    superseded.sort();
    superseded.dedup();
    if !superseded.is_empty() {
        events.push(Event::Superseded {
            ids: superseded.clone(),
        });
    }
    if caller_is_master {
        if let Some(event) = cleared_master_goal_due_event(&state, &worker_id, &retired, now) {
            events.push(event);
        }
    }
    if !events.is_empty() {
        if let Err(error) = server.commit_locked(&mut state, &events) {
            return Resp::err(format!("NOTIFICATION_DURABILITY_FAILED: {error}"));
        }
    }
    Resp::data(json!({
        "goal_id": goal_id,
        "goal_owner": goal_owner,
        "desired": "ended",
        "status": "ended",
        "subscription": serde_json::Value::Null,
        "retired_subscription_ids": retired,
        "superseded_message_ids": superseded,
        "deduplicated": events.is_empty(),
    }))
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

fn migration_issues(server: &Server, state: &State) -> Vec<String> {
    let mut issues = Vec::new();
    for worker in state.workers.values() {
        match worker_presence_in_state(server, state, worker) {
            IdentityPresence::Present => {}
            IdentityPresence::Cold => {}
            IdentityPresence::Missing => issues.push(format!(
                "worker {} has no live registered tmux pane",
                worker.id
            )),
            IdentityPresence::Unknown => issues.push(format!(
                "worker {} transport liveness is unknown",
                worker.id
            )),
        }
    }
    for task in state.tasks.values() {
        if task.worktree_path.is_some()
            && matches!(task.status.as_str(), "merged" | "closed" | "cancelled")
            && !state.cleanup_receipts.contains_key(&task.id)
        {
            issues.push(format!(
                "TASK_CLEANUP_INCOMPLETE:{}:{}",
                task.id,
                task.worktree_path.as_deref().unwrap_or("unknown")
            ));
        }
        if let Some(receipt) = state.cleanup_receipts.get(&task.id) {
            if receipt.task_id != task.id
                || receipt.worktree_path != task.worktree_path
                || receipt.branch != task.branch
            {
                issues.push(format!("TASK_CLEANUP_RECEIPT_MISMATCH:{}", task.id));
            }
            if let Some(path) = task.worktree_path.as_deref() {
                let worktree = Path::new(path);
                let worktree = if worktree.is_absolute() {
                    worktree.to_path_buf()
                } else {
                    server
                        .root
                        .join(worktree.strip_prefix("./").unwrap_or(worktree))
                };
                if worktree.exists() {
                    issues.push(format!("TASK_CLEANUP_INCOMPLETE:{}:{}", task.id, path));
                }
            }
        }
        if task.status == "available" {
            issues.push(format!(
                "task {} uses deprecated available/dispatch state and needs an explicit owner decision",
                task.id
            ));
        }
        if let Some(wait) = task.wait.as_ref() {
            if task.status != "waiting" {
                issues.push(format!(
                    "task {} has wait metadata outside waiting",
                    task.id
                ));
            }
            if wait.waiter != task.owner || !state.workers.contains_key(&wait.waiter) {
                issues.push(format!("task {} wait has no valid waiter", task.id));
            }
            if wait.responsible_actor.trim().is_empty()
                || !state.workers.contains_key(&wait.responsible_actor)
            {
                issues.push(format!("task {} wait has no responsible actor", task.id));
            }
            match state.tasks.get(&wait.waiting_for) {
                None => issues.push(format!(
                    "task {} wait points to missing blocking task {}",
                    task.id, wait.waiting_for
                )),
                Some(blocking) => {
                    if !task_resource_active(&blocking.status) {
                        issues.push(format!(
                            "task {} wait points to inactive blocking task {}",
                            task.id, blocking.id
                        ));
                    }
                    if wait.responsible_actor != blocking.owner {
                        issues.push(format!(
                            "task {} wait responsible actor does not own blocking task {}",
                            task.id, blocking.id
                        ));
                    }
                    let same_feature =
                        task.feature_id.is_some() && task.feature_id == blocking.feature_id;
                    let same_worktree = task.worktree_path.is_some()
                        && task.worktree_path == blocking.worktree_path;
                    if !same_feature && !same_worktree {
                        issues.push(format!(
                            "task {} wait has no matching active resource on blocking task {}",
                            task.id, blocking.id
                        ));
                    }
                }
            }
            if wait.deadline_ms <= now_ms() {
                issues.push(format!(
                    "task {} wait deadline is missing or expired",
                    task.id
                ));
            }
            if wait.resume_on.is_empty() || wait.escalation.trim().is_empty() {
                issues.push(format!(
                    "task {} wait has no resume/escalation path",
                    task.id
                ));
            }
            if wait_cycle(&state.tasks, &task.id, &wait.waiting_for) {
                issues.push(format!("task {} participates in a wait cycle", task.id));
            }
        } else if task.status == "waiting" {
            issues.push(format!("task {} is waiting without WaitSpec", task.id));
        }
    }
    issues.sort();
    issues.dedup();
    issues
}

fn snapshot_hash(state: &State) -> String {
    let mut workers: Vec<_> = state
        .workers
        .values()
        .map(|worker| worker.id.clone())
        .collect();
    workers.sort();
    let mut tasks: Vec<_> = state.tasks.values().cloned().collect();
    tasks.sort_by(|left, right| left.id.cmp(&right.id));
    let mut messages: Vec<_> = state.msgs.values().cloned().collect();
    messages.sort_by(|left, right| left.id.cmp(&right.id));
    let mut delivery_modes: Vec<_> = state
        .delivery_modes
        .iter()
        .map(|(id, mode)| (id.clone(), mode.clone()))
        .collect();
    delivery_modes.sort();
    let bytes = serde_json::to_vec(&(workers, tasks, messages, delivery_modes))
        .expect("serialize deterministic migration snapshot");
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

fn migration_peer(state: &State, worker_id: &str, token: &str) -> Result<WorkerRec, Resp> {
    verify(state, worker_id, token)
}

fn verify_migration_lease(state: &State, worker_id: &str) -> Result<(), Resp> {
    if let Some(migration) = state.migration.as_ref().filter(|migration| {
        migration.operator != worker_id && matches!(migration.phase.as_str(), "planned" | "applied")
    }) {
        return Err(Resp::err_data(
            "MIGRATION_TRANSACTION_HELD_BY_ANOTHER_PEER",
            json!({
                "migration": migration,
                "holder": migration.operator,
                "requester": worker_id,
                "admission_frozen": state.admission_frozen(),
                "retry_allowed": false,
                "next": "do not retry plan/apply/verify; query collab migrate inspect, then let the holder complete the current migration or coordinate ownership transfer",
            }),
        ));
    }
    Ok(())
}

fn migration_state_rejection(state: &State, message: &str) -> Resp {
    Resp::err_data(
        message,
        json!({
            "migration": state.migration,
            "admission_frozen": state.admission_frozen(),
            "retry_allowed": false,
            "next": "run collab migrate inspect; do not create a new migration until the current record is resolved",
        }),
    )
}

fn migration_view(state: &State, issues: Vec<String>) -> serde_json::Value {
    json!({
        "migration": state.migration,
        "admissible": issues.is_empty(),
        "issues": issues,
        "state": {
            "workers": state.workers.len(),
            "tasks": state.tasks.len(),
            "messages": state.msgs.len(),
            "snapshot_hash": snapshot_hash(state),
        },
        "deprecated_paths": [
            "delete .agent-collab",
            "manual task/claim/journal JSON edits",
            "clear mailbox",
            "copy worker tokens",
            "start a second daemon",
            "mixed runtime writers",
            "guess thread identity",
        ],
    })
}

fn handle_migration_inspect(server: &Server, worker_id: String, token: String) -> Resp {
    let state = server.state.lock().unwrap();
    if let Err(error) = migration_peer(&state, &worker_id, &token) {
        return error;
    }
    let issues = migration_issues(server, &state);
    Resp::data(migration_view(&state, issues))
}

fn handle_migration_plan(server: &Server, worker_id: String, token: String) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = migration_peer(&state, &worker_id, &token) {
        return error;
    }
    if let Err(error) = verify_migration_lease(&state, &worker_id) {
        return error;
    }
    if state.admission_frozen() {
        return Resp::err("migration admission is already frozen");
    }
    let issues = migration_issues(server, &state);
    let now = now_ms();
    let migration = MigrationRecord {
        id: format!("migration-{now}"),
        from_version: "v1-legacy".into(),
        to_version: "v1-low-intervention".into(),
        phase: if issues.is_empty() {
            "planned".into()
        } else {
            "migration_needs_operator".into()
        },
        admission_frozen: false,
        snapshot_hash: None,
        worker_count: state.workers.len(),
        task_count: state.tasks.len(),
        message_count: state.msgs.len(),
        operator: worker_id,
        issues: issues.clone(),
        created_ms: now,
        updated_ms: now,
    };
    if let Err(error) = server.commit_locked(
        &mut state,
        &[Event::MigrationUpdated {
            migration: migration.clone(),
        }],
    ) {
        return Resp::err(format!("MIGRATION_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "migration": migration,
        "admissible": issues.is_empty(),
        "issues": issues,
        "next": if state.migration.as_ref().is_some_and(|record| record.phase == "planned") {
            "collab migrate apply"
        } else {
            "resolve every issue, then run collab migrate plan again"
        },
    }))
}

fn handle_migration_apply(server: &Server, worker_id: String, token: String) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = migration_peer(&state, &worker_id, &token) {
        return error;
    }
    if let Err(error) = verify_migration_lease(&state, &worker_id) {
        return error;
    }
    let Some(mut migration) = state.migration.clone() else {
        return Resp::err("run collab migrate plan before apply");
    };
    if migration.phase != "planned" || !migration.issues.is_empty() {
        return Resp::err("migration plan is not admissible");
    }
    let issues = migration_issues(server, &state);
    if !issues.is_empty() {
        return Resp::err(format!(
            "migration admission changed: {}",
            issues.join("; ")
        ));
    }
    migration.phase = "applied".into();
    migration.admission_frozen = true;
    migration.snapshot_hash = Some(snapshot_hash(&state));
    migration.worker_count = state.workers.len();
    migration.task_count = state.tasks.len();
    migration.message_count = state.msgs.len();
    migration.updated_ms = now_ms();
    if let Err(error) = server.commit_locked(
        &mut state,
        &[Event::MigrationUpdated {
            migration: migration.clone(),
        }],
    ) {
        return Resp::err(format!("MIGRATION_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "migration": migration,
        "admission_frozen": true,
        "next": "verify the applied migration against the current service; identity restoration uses collab context and does not repeat registration",
    }))
}

fn handle_migration_verify(server: &Server, worker_id: String, token: String) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = migration_peer(&state, &worker_id, &token) {
        return error;
    }
    if let Err(error) = verify_migration_lease(&state, &worker_id) {
        return error;
    }
    let Some(mut migration) = state.migration.clone() else {
        return migration_state_rejection(&state, "no migration record to verify");
    };
    if migration.phase == "verified" && !migration.admission_frozen {
        let current_snapshot_hash = snapshot_hash(&state);
        return Resp::data(json!({
            "migration": migration,
            "verified": true,
            "resumed": false,
            "idempotent": true,
            "issues": [],
            "current_snapshot_hash": current_snapshot_hash,
            "next": "migration already verified; continue task lifecycle; do not rerun plan or apply",
        }));
    }
    if migration.phase != "applied" || !migration.admission_frozen {
        return migration_state_rejection(
            &state,
            "migration must be applied and frozen before verify",
        );
    }
    let mut issues = migration_issues(server, &state);
    let current_hash = snapshot_hash(&state);
    if migration.snapshot_hash.as_deref() != Some(current_hash.as_str()) {
        issues.push("migration snapshot hash mismatch".into());
    }
    if migration.worker_count != state.workers.len()
        || migration.task_count != state.tasks.len()
        || migration.message_count != state.msgs.len()
    {
        issues.push("migration state counts changed during admission freeze".into());
    }
    issues.sort();
    issues.dedup();
    migration.updated_ms = now_ms();
    migration.issues = issues.clone();
    if issues.is_empty() {
        migration.phase = "verified".into();
        migration.admission_frozen = false;
    } else {
        migration.phase = "migration_needs_operator".into();
    }
    if let Err(error) = server.commit_locked(
        &mut state,
        &[Event::MigrationUpdated {
            migration: migration.clone(),
        }],
    ) {
        return Resp::err(format!("MIGRATION_DURABILITY_FAILED: {error}"));
    }
    Resp::data(json!({
        "migration": migration,
        "verified": issues.is_empty(),
        "resumed": issues.is_empty(),
        "issues": issues,
        "current_snapshot_hash": current_hash,
    }))
}

/// Prepare the exact Register envelope from one reducer snapshot without
/// consuming it. The caller owns the following sequence: durable outer
/// `Validating` with the returned nested IDs, then
/// [`consume_prepared_register_typed`] with this same value.
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

/// Consume one prepared Register envelope and project its receipt. It never
/// regenerates the nested identifiers.
fn consume_prepared_register_typed(
    server: &Server,
    worker_id: &str,
    transport: &SelectedTransport,
    prepared: PreparedRegisterEnvelope,
    approval: Option<&RegisterApprovalProof>,
) -> Resp {
    consume_register_typed(server, worker_id, transport, prepared, approval)
}

fn consume_register_typed(
    server: &Server,
    worker_id: &str,
    transport: &SelectedTransport,
    prepared: PreparedRegisterEnvelope,
    approval: Option<&RegisterApprovalProof>,
) -> Resp {
    let registered_at = match &prepared.typed.command {
        TypedCommand::RegisterWorker { worker, .. } => worker.registered_ms,
    };
    let command = prepared.typed.command.clone();
    let committed = server.consume_prepared_register_with_approval(prepared, approval);
    match committed {
        Ok(outcome) => {
            let name_resolution = match name_resolution::resolve_registered_name(
                server,
                worker_id,
                outcome.receipt.operation_id.as_str(),
                "register",
                None,
            ) {
                Ok(resolution) => resolution,
                Err(error) => {
                    return Resp::err_data(
                        error,
                        json!({
                            "command_id":outcome.receipt.command_id.as_str(),
                            "operation_id":outcome.receipt.operation_id.as_str(),
                            "registration_committed":true,
                        }),
                    )
                }
            };
            let (role_brief, _) = {
                let st = server.state.lock().unwrap();
                (role_brief(server, &st, worker_id), ())
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

/// Read the existing scoped Register owner. This never prepares, consumes,
/// probes, or projects an invented command from a receipt.
fn committed_register_readback(
    server: &Server,
    binding: &RuntimeBinding,
    worker_id: &str,
    token: &str,
) -> Result<(global_state::CommandReceipt, SelectedTransport), String> {
    let state = server.state.lock().unwrap();
    if binding.agent_id.as_str() != worker_id
        || state
            .global
            .lookup_binding_for(&binding.route_scope(), &binding.binding_id)
            != Some(binding)
    {
        return Err("RECOVERY_RECONCILE_REQUIRED: committed staged binding changed".into());
    }
    let worker = state
        .workers
        .get(worker_id)
        .ok_or("RECOVERY_RECONCILE_REQUIRED: committed staged worker missing")?;
    if worker.token != token {
        return Err("TOKEN_MISMATCH: staged readback credential does not own worker".into());
    }
    let transport = selected_transport_for_worker(worker)
        .ok_or("RECOVERY_RECONCILE_REQUIRED: committed selected adaptor missing")?;
    if transport.kind != TransportKind::Tmux || transport.tmux_endpoint != binding.tmux_endpoint {
        return Err(
            "RECOVERY_RECONCILE_REQUIRED: committed selected adaptor differs from binding".into(),
        );
    }
    let receipt = state
        .global
        .command_receipts
        .values()
        .filter(|receipt| register_receipt_names_binding(receipt, binding))
        .max_by_key(|receipt| (receipt.revision, receipt.sequence))
        .ok_or("RECOVERY_RECONCILE_REQUIRED: exact committed Register receipt missing")?;
    if receipt.outcome.get("scope") != Some(&json!(binding.route_scope())) {
        return Err(
            "RECOVERY_RECONCILE_REQUIRED: committed Register receipt scope missing or different"
                .into(),
        );
    }
    Ok((receipt.clone(), transport))
}
include!("part_06/register_typed.rs");

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
    register_approval: Option<&RegisterApprovalProof>,
    before_consume: Option<&mut dyn FnMut(&PreparedRegisterEnvelope) -> Result<(), String>>,
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
    if let Some(before_consume) = before_consume {
        if let Err(error) = before_consume(&prepared) {
            return Resp::err(error);
        }
    }
    let response =
        consume_register_typed(server, worker_id, transport, prepared, register_approval);
    clear_context_cancel_operation();
    crate::server::clear_context_register_start_binding();
    response
}

/// Ordinary Create has already reserved an exact peer id. Its execution cwd
/// may be a linked worktree; registration and route identity remain the main.
pub(crate) fn register_created_peer(
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
    if server.state.lock().unwrap().workers.contains_key(worker_id) {
        return Resp::err("PEER_LIFECYCLE_PEER_RESERVED");
    }
    let main = route.root.to_string_lossy().into_owned();
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
    );
    if !response.ok {
        return response;
    }
    if let Err(error) = commit_current_thread_route_for_runtime(
        server,
        server,
        worker_id,
        &main,
        Some(&context.app_scope_id),
    ) {
        let cleanup = retire_runtime_binding_after_route_failure(
            server,
            worker_id,
            &main,
            Some(&context.app_scope_id),
            worker_id,
            "Create route publication failed",
        );
        return Resp::err(format!("{error}; route cleanup: {cleanup:?}"));
    }
    response
}

pub(crate) fn handle_register_with_app_scope(
    server: &Server,
    worker_id: String,
    token: String,
    cwd: String,
    app_scope: Option<AppServerId>,
    candidates: Option<TransportCandidates>,
) -> Resp {
    let route_worker_id = worker_id.clone();
    let route_cwd = cwd.clone();
    let route_app_scope = app_scope.clone();
    let response = handle_register_with_app_scope_inner(
        server, worker_id, token, cwd, app_scope, candidates, false,
    );
    if !response.ok {
        return response;
    }
    if let Err(error) = commit_current_thread_route_for_runtime(
        server,
        server,
        &route_worker_id,
        &route_cwd,
        route_app_scope.as_ref(),
    ) {
        let cleanup = retire_runtime_binding_after_route_failure(
            server,
            &route_worker_id,
            &route_cwd,
            route_app_scope.as_ref(),
            &route_worker_id,
            "registration route publication failed",
        );
        let cleanup_status = cleanup
            .map(|_| "registration binding retired".to_owned())
            .unwrap_or_else(|cleanup_error| {
                format!("registration cleanup failed: {cleanup_error}")
            });
        return Resp::err(format!("{error}; {cleanup_status}"));
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

pub(crate) fn retire_runtime_binding_after_route_failure(
    runtime: &Server,
    worker_id: &str,
    cwd: &str,
    app_scope: Option<&AppServerId>,
    closed_by: &str,
    reason: &str,
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
        .cloned()
        .ok_or_else(|| format!("ROUTE_CLEANUP_INVALID: no binding exists for {worker_id}"))?;
    let next_generation = binding.endpoint_generation.checked_add(1).ok_or_else(|| {
        format!("ROUTE_CLEANUP_INVALID: endpoint generation overflow for {worker_id}")
    })?;
    let mut retired = binding;
    retired.endpoint_generation = next_generation;
    retired.native_thread_id = None;
    retired.tmux_endpoint = None;
    runtime
        .commit_checked(&[
            Event::GlobalRuntimeBound { binding: retired },
            Event::WorkerClosed {
                worker_id: worker_id.to_owned(),
                closed_by: closed_by.to_owned(),
                reason: reason.to_owned(),
                snapshot_captured_ms: None,
                at_ms: now_ms(),
            },
        ])
        .map(|_| ())
        .map_err(|error| format!("ROUTE_CLEANUP_DURABILITY_FAILED: {error}"))
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
    )
}

/// The daemon-issued approval proof may admit the approved target's Register
/// even when its credential is stale, but only for the exact target and route
/// scope the proof names. The proof never substitutes for the real binding the
/// reducer commits; it only relaxes the ordinary token-ownership check.
fn approved_register_authorized(
    proof: Option<&RegisterApprovalProof>,
    worker_id: &str,
    project_scope: Option<&ProjectScopeId>,
    app_scope: Option<&AppServerId>,
) -> bool {
    match (proof, project_scope, app_scope) {
        (Some(proof), Some(project_scope), Some(app_scope)) => {
            proof.authorizes(worker_id, project_scope.as_str(), app_scope.as_str())
        }
        _ => false,
    }
}

/// Recheck the exact approved incumbent fence at the Register owner boundary,
/// immediately before effects. The proof records the committed incumbent the
/// identity owner validated; if the reducer has since moved to another binding
/// or generation, the approval is stale and must fail closed with the accepted
/// typed conflict instead of committing an unapproved replacement.
fn approved_register_fence_current(
    state: &State,
    proof: &RegisterApprovalProof,
    project_scope: &ProjectScopeId,
    app_scope: &AppServerId,
) -> bool {
    if proof.incumbent_binding_id.is_empty() {
        return true;
    }
    let route_scope = RouteScope {
        app_scope_id: app_scope.clone(),
        project_scope_id: project_scope.clone(),
    };
    let Ok(binding_id) = BindingId::new(proof.incumbent_binding_id.clone()) else {
        return false;
    };
    state
        .global
        .lookup_binding_for(&route_scope, &binding_id)
        .is_some_and(|binding| {
            binding.binding_id.as_str() == proof.incumbent_binding_id
                && binding.endpoint_generation == proof.incumbent_endpoint_generation
        })
}

/// Register owner with an optional outer-operation binder. When the binder is
/// present, the outer `Validating` phase with the exact nested IDs is synced
/// before the same prepared envelope is consumed.
fn handle_register_with_app_scope_inner_bound(
    server: &Server,
    worker_id: String,
    token: String,
    cwd: String,
    app_scope: Option<AppServerId>,
    candidates: Option<TransportCandidates>,
    recover_existing: bool,
    outer_binding: Option<&RegisterOuterBinding>,
) -> Resp {
    let candidates = candidates.unwrap_or_default();
    let selected = match validate_transport_candidates(server, &candidates, &cwd) {
        Ok(selected) => selected,
        Err(error) => return Resp::err(error),
    };
    let st = server.state.lock().unwrap();
    if st.admission_frozen() && !st.workers.contains_key(&worker_id) {
        return Resp::err("MIGRATION_ADMISSION_FROZEN: only an existing identity may rebind");
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
        let approval = outer_binding.and_then(RegisterOuterBinding::approval);
        let approved_register = approved_register_authorized(
            approval,
            &worker_id,
            existing_project_scope.as_ref(),
            app_scope.as_ref(),
        );
        if existing.token != token && !approved_register {
            return Resp::err(format!(
                "TOKEN_MISMATCH: worker {} is registered by another token",
                worker_id
            ));
        }
        // The proof only relaxes the token check. It must still name exactly
        // the committed incumbent, or the approval is stale and the owner
        // refuses before any prepare/consume.
        if existing.token != token {
            let Some(proof) = approval else {
                return Resp::err("APPROVAL_STALE_CONFLICT");
            };
            let (Some(project_scope), Some(app_scope)) =
                (existing_project_scope.as_ref(), app_scope.as_ref())
            else {
                return Resp::err("APPROVAL_STALE_CONFLICT");
            };
            if !approved_register_fence_current(&st, proof, project_scope, app_scope) {
                return Resp::err("APPROVAL_STALE_CONFLICT");
            }
        }
        let reuse_existing = same_runtime_key && !recover_existing && !approved_register;
        drop(st);
        let mut before_consume = |prepared: &PreparedRegisterEnvelope| match outer_binding {
            Some(binding) => {
                binding.bind_validating(prepared)?;
                binding.arbitrate_owner_start()?;
                binding.promote_after_start()
            }
            None => Ok(()),
        };
        let mut resp = register_typed_observed(
            server,
            &worker_id,
            &token,
            &selected,
            &cwd,
            existing_project_scope,
            app_scope,
            reuse_existing,
            outer_binding.and_then(RegisterOuterBinding::approval),
            Some(&mut before_consume),
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
    let mut before_consume = |prepared: &PreparedRegisterEnvelope| match outer_binding {
        Some(binding) => {
            binding.bind_validating(prepared)?;
            binding.arbitrate_owner_start()?;
            binding.promote_after_start()
        }
        None => Ok(()),
    };
    register_typed_observed(
        server,
        &worker_id,
        &token,
        &selected,
        &cwd,
        None,
        app_scope,
        false,
        outer_binding.and_then(RegisterOuterBinding::approval),
        Some(&mut before_consume),
    )
}

/// Compatibility entry point for direct in-process callers.  Wire requests
/// use `handle_register_with_app_scope` so the app scope comes from their
/// validated ProjectContext; this adapter retains the historical tui route
/// only when no wire context exists.
pub(crate) fn handle_register(
    server: &Server,
    worker_id: String,
    token: String,
    cwd: String,
) -> Resp {
    handle_register_with_app_scope(server, worker_id, token, cwd, None, None)
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
