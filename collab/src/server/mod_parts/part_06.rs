/// Test helper retained for the App Server-only notification call sites.
#[cfg(test)]
pub(crate) fn attempt_notification_with_default(
    server: &Server,
    message_id: &str,
    subscription_id: &str,
    can_receive: &dyn Fn(&str) -> bool,
    deliver: &dyn Fn(&str, &str) -> bool,
) -> bool {
    if !server.config.notifications.enabled {
        return false;
    }
    let (recipient, transport, source_thread_id, delay, explicit) = {
        let mut state = server.state.lock().unwrap();
        let Some(seed) = state.msgs.get(message_id) else {
            return false;
        };
        if !state.scheduler_message_deliverable(message_id) {
            return false;
        }
        let recipient = seed.to.clone();
        let Some(subscription) = state.notification_subscriptions.get(subscription_id) else {
            return false;
        };
        if subscription.worker_id != recipient {
            return false;
        }
        let delay = notification_delivery_delay_ms(
            &state,
            message_id,
            &subscription.event,
            &server.config.notifications,
        );
        let Some(transport) = state
            .workers
            .get(&recipient)
            .and_then(selected_transport_for_worker)
        else {
            return false;
        };
        if !subscription_matches_transport(subscription, &transport) {
            return false;
        }
        let source_thread_id = state
            .delivery_source_threads
            .get(message_id)
            .cloned()
            .or_else(|| {
                state
                    .workers
                    .get(&seed.from)
                    .and_then(selected_transport_for_worker)
                    .and_then(|transport| transport.thread_id)
            });
        let explicit = is_explicit_notification(&state, seed);
        (recipient, transport, source_thread_id, delay, explicit)
    };
    attempt_tmux_notification_with_at(
        server,
        message_id,
        subscription_id,
        &recipient,
        &transport,
        source_thread_id.as_deref(),
        delay,
        explicit,
        now_ms(),
        false,
        &|transport, source_thread_id, text, message_id, explicit, _mode| {
            let target = transport.thread_id.as_deref().unwrap_or("appserver");
            if !can_receive(target) {
                return Err("test can_receive returned false".into());
            }
            if deliver(target, text) {
                Ok(json!({
                    "accepted": true,
                    "sourceThreadId": source_thread_id,
                    "messageId": message_id,
                    "explicit": explicit,
                }))
            } else {
                Err("test deliver returned false".into())
            }
        },
    )
    .accepted()
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
    if matches!(event.as_str(), "deadline" | "master-idle") {
        let live_master = match live_master_id(server, &state) {
            Ok(master) => master,
            Err(error) => return Resp::err(error),
        };
        if live_master.as_deref() != Some(worker_id.as_str()) {
            return Resp::err(if event == "master-idle" {
                "master-idle subscription requires the live registered master"
            } else if live_master.is_some() {
                "master authority required for deadline subscriptions"
            } else {
                "no live master; deadline subscriptions require an approved live master"
            });
        }
    }
    let Some(worker) = state.workers.get(&worker_id).cloned() else {
        return Resp::err("notification subscription requires a registered worker");
    };
    let Some(transport) = selected_transport_for_worker(&worker) else {
        return Resp::err("registered worker has no server-selected transport");
    };
    let Some(thread_id) = transport.thread_id.clone() else {
        return Resp::err("selected transport has no route address");
    };
    let target = thread_id;
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
                server.commit_locked(
                    &mut state,
                    &[Event::NotificationSubscribed {
                        subscription: existing.clone(),
                    }],
                );
            }
            return Resp::data(json!({
                "subscription": existing,
                "one_shot": true,
                "max_repeat_count": crate::server::state::MAX_NOTIFICATION_REPEATS,
                "deduplicated": true,
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
    server.commit_locked(
        &mut state,
        &[Event::NotificationSubscribed {
            subscription: subscription.clone(),
        }],
    );
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
    Resp::data(json!({"subscriptions": subscriptions}))
}

fn handle_notification_unsubscribe(
    server: &Server,
    worker_id: String,
    token: String,
    subscription_id: String,
) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, &worker_id, &token) {
        return error;
    }
    let Some(subscription) = state.notification_subscriptions.get(&subscription_id) else {
        return Resp::err(format!(
            "notification subscription {} not found",
            subscription_id
        ));
    };
    if subscription.worker_id != worker_id {
        return Resp::err("only the subscription owner may unsubscribe");
    }
    let mut events = vec![Event::NotificationSuppressed {
        subscription_id: subscription_id.clone(),
        status: "cancelled".into(),
        reason: EXPLICIT_UNSUBSCRIBE_REASON.into(),
        updated_ms: now_ms(),
    }];
    let pending = state
        .wake_bindings
        .iter()
        .filter_map(|(message_id, bound_subscription)| {
            (bound_subscription == &subscription_id
                && state
                    .msgs
                    .get(message_id)
                    .is_some_and(|message| message.state == "pending"))
            .then_some(message_id.clone())
        })
        .collect::<Vec<_>>();
    if !pending.is_empty() {
        events.push(Event::Superseded { ids: pending });
    }
    server.commit_locked(&mut state, &events);
    Resp::data(json!({"subscription_id": subscription_id, "status": "cancelled"}))
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
        match worker_presence(server, worker) {
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
    server.commit_locked(
        &mut state,
        &[Event::MigrationUpdated {
            migration: migration.clone(),
        }],
    );
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
    server.commit_locked(
        &mut state,
        &[Event::MigrationUpdated {
            migration: migration.clone(),
        }],
    );
    Resp::data(json!({
        "migration": migration,
        "admission_frozen": true,
        "next": "upgrade/restart the single daemon, re-register existing peers from their current tmux panes, then run collab migrate verify",
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
    server.commit_locked(
        &mut state,
        &[Event::MigrationUpdated {
            migration: migration.clone(),
        }],
    );
    Resp::data(json!({
        "migration": migration,
        "verified": issues.is_empty(),
        "resumed": issues.is_empty(),
        "issues": issues,
        "current_snapshot_hash": current_hash,
    }))
}

fn register_typed(
    server: &Server,
    worker_id: &str,
    token: &str,
    transport: &SelectedTransport,
    cwd: &str,
    project_scope: Option<ProjectScopeId>,
    app_scope: Option<AppServerId>,
    reuse_existing: bool,
) -> Resp {
    let typed = match (project_scope, app_scope) {
        (Some(project_scope), Some(app_scope)) => server.typed_register_envelope_for_scope(
            worker_id,
            token,
            transport,
            project_scope,
            cwd,
            app_scope,
            reuse_existing,
        ),
        (Some(project_scope), None) => {
            let app_scope = AppServerId::new("tui-default").map_err(|error| error.to_string());
            match app_scope {
                Ok(app_scope) => server.typed_register_envelope_for_scope(
                    worker_id,
                    token,
                    transport,
                    project_scope,
                    cwd,
                    app_scope,
                    reuse_existing,
                ),
                Err(error) => Err(error),
            }
        }
        (None, Some(app_scope)) => match GlobalState::canonical_project_scope(Path::new(cwd)) {
            Ok(project_scope) => server.typed_register_envelope_for_scope(
                worker_id,
                token,
                transport,
                project_scope,
                cwd,
                app_scope,
                reuse_existing,
            ),
            Err(error) => Err(error.to_string()),
        },
        (None, None) if reuse_existing => {
            let project_scope = GlobalState::canonical_project_scope(Path::new(cwd))
                .map_err(|error| error.to_string());
            let app_scope = AppServerId::new("tui-default").map_err(|error| error.to_string());
            match (project_scope, app_scope) {
                (Ok(project_scope), Ok(app_scope)) => server.typed_register_envelope_for_scope(
                    worker_id,
                    token,
                    transport,
                    project_scope,
                    cwd,
                    app_scope,
                    true,
                ),
                (Err(error), _) | (_, Err(error)) => Err(error),
            }
        }
        (None, None) => {
            Err("collab registration requires an app scope for a tmux transport".into())
        }
    };
    match typed {
        Ok(typed) => match server.typed_dispatch(typed.clone()) {
            Ok(outcome) => {
                let (role_brief, registered_at) = {
                    let st = server.state.lock().unwrap();
                    let registered_at = match &typed.command {
                        TypedCommand::RegisterWorker { worker, .. } => worker.registered_ms,
                    };
                    (role_brief(server, &st, worker_id), registered_at)
                };
                Resp::data(json!({
                    "worker_id": worker_id,
                    "identity_kind": "peer",
                    "transport_selected": transport,
                    "registered_at": iso(registered_at),
                    "role_brief": role_brief,
                    "typed": true,
                    "command_id": outcome.receipt.command_id.as_str(),
                    "operation_id": outcome.receipt.operation_id.as_str(),
                    "sequence": outcome.receipt.sequence,
                    "revision": outcome.receipt.revision,
                    "replayed": outcome.replayed,
                    "command": typed.command,
                }))
            }
            Err(error) => Resp::err(format!("typed registrar rejected registration: {error}")),
        },
        Err(error) => Resp::err(format!("typed registrar failed to build command: {error}")),
    }
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
    let native_thread_id = binding.native_thread_id.as_ref().ok_or_else(|| {
        format!(
            "ROUTE_TRANSITION_INVALID: successful registration for {worker_id} has no native App Server thread"
        )
    })?;
    let session_id = binding.session_id.as_ref().ok_or_else(|| {
        format!(
            "ROUTE_TRANSITION_INVALID: successful registration for {worker_id} has no session id"
        )
    })?;
    if route_owner
        .state
        .lock()
        .unwrap()
        .global
        .lookup_current_thread_route(session_id, native_thread_id)
        == Some(&binding)
    {
        return Ok(());
    }
    // A successful registration is live activity. It deliberately re-activates
    // an address an operator retired, and `set_current_thread_route` clears the
    // retirement record for exactly that address. The retirement exists to stop
    // the reconcilers and the replay helper from republishing a claim from the
    // project side, which is a different signal from a peer registering again.
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
            TransportKind::Dsh => selected_transport_for_worker(&existing).is_some_and(|old| {
                old.kind == TransportKind::Dsh
                    && old.endpoint == selected.endpoint
                    && old.namespace == selected.namespace
            }),
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
        let same_thread = existing_key
            .as_ref()
            .and_then(|(_, thread, _)| thread.as_deref())
            == selected.thread_id.as_deref();
        if !same_runtime_key {
            let binding_id =
                match BindingId::new(sanitize_identifier(&format!("binding-{worker_id}"))) {
                    Ok(binding_id) => binding_id,
                    Err(error) => return Resp::err(error.to_string()),
                };
            let is_master_binding = existing_route_scope.as_ref().is_some_and(|route| {
                st.global
                    .lookup_master_grant_for(route, &binding_id)
                    .is_some()
            });
            let same_pane_tmux_recovery = existing.token == token
                && selected.kind == TransportKind::Tmux
                && selected.session_id.is_some()
                && selected.thread_id.is_some()
                && selected.tmux_endpoint.as_ref().is_some_and(|candidate| {
                    selected_transport_for_worker(&existing).is_some_and(|old| {
                        old.kind == TransportKind::Tmux
                            && old.tmux_endpoint.as_ref().is_some_and(|endpoint| {
                                crate::client::adapters::tmux::same_pane_route(endpoint, candidate)
                            })
                    }) && st
                        .global
                        .lookup_unique_tmux_pane_route(candidate)
                        .is_some_and(|binding| {
                            binding.agent_id.as_str() == worker_id
                                && binding.binding_id == binding_id
                        })
                });
            // A dsh peer has no pane to point at, so the tmux arm can never
            // hold for it. Once a changed gateway address is a rebind, a dsh
            // master that returns on a new socket would reach this fence and be
            // refused as a foreign promotion, which would leave it unable to
            // re-register at all. The token authenticates the principal and the
            // agent id names it, so a dsh transport carrying the same token and
            // the same agent is that principal recovering at a new address; the
            // address itself is what changed and cannot be part of the test.
            let same_dsh_agent_recovery = existing.token == token
                && selected.kind == TransportKind::Dsh
                && selected.thread_id.is_some()
                && selected_transport_for_worker(&existing).is_some_and(|old| {
                    old.kind == TransportKind::Dsh && old.thread_id == selected.thread_id
                });
            if is_master_binding && !same_pane_tmux_recovery && !same_dsh_agent_recovery {
                match live_master_id(server, &st) {
                    Ok(None) => {}
                    Ok(Some(live_master)) => {
                        return Resp::err(format!(
                            "MASTER_RECOVERY_BLOCKED_LIVE: live master {live_master} exists; do not auto-recover or promote another identity"
                        ));
                    }
                    Err(error) => {
                        return Resp::err(format!(
                            "MASTER_RECOVERY_BLOCKED_UNKNOWN: {error}; do not auto-recover or promote"
                        ));
                    }
                }
            }
        }
        if existing.token != token && (!recover_existing || !same_thread) {
            return Resp::err(format!(
                "worker_id {} already registered by another token",
                worker_id
            ));
        }
        let reuse_existing = same_runtime_key && !recover_existing;
        drop(st);
        let mut resp = register_typed(
            server,
            &worker_id,
            &token,
            &selected,
            &cwd,
            existing_project_scope,
            app_scope,
            reuse_existing,
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
    register_typed(
        server, &worker_id, &token, &selected, &cwd, None, app_scope, false,
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

fn server_route_scope(server: &Server, state: &State) -> Result<Option<RouteScope>, &'static str> {
    route_scope_for_root(&server.root, state)
}

fn master_grant_for_worker(
    state: &State,
    route_scope: &RouteScope,
    worker_id: &str,
    granted_by: &str,
    approval: &str,
) -> Result<crate::server::global_state::MasterGrant, String> {
    let project = state
        .global
        .lookup_project_for_route(route_scope)
        .ok_or_else(|| {
            format!(
                "MASTER_AUTHORITY_REQUIRES_REGISTERED_ROUTE: {} / {}",
                route_scope.project_scope_id.as_str(),
                route_scope.app_scope_id
            )
        })?;
    let mut bindings = project.runtime_bindings.values().filter(|binding| {
        binding.project_scope == route_scope.project_scope_id
            && binding.app_scope_id == route_scope.app_scope_id
            && binding.agent_id.as_str() == worker_id
    });
    let Some(binding) = bindings.next() else {
        return Err(format!(
            "MASTER_AUTHORITY_REQUIRES_RUNTIME_BINDING: worker {worker_id} has no runtime binding"
        ));
    };
    if bindings.next().is_some() {
        return Err(format!(
            "MASTER_AUTHORITY_AMBIGUOUS_BINDING: worker {worker_id} has multiple runtime bindings"
        ));
    }
    crate::server::global_state::MasterGrant::new(
        binding.project_scope.clone(),
        binding.app_scope_id.clone(),
        binding.agent_id.clone(),
        "project",
        granted_by,
        approval,
        binding.binding_id.clone(),
        binding.endpoint_generation,
        now_ms(),
    )
    .map_err(|error| error.to_string())
}

fn master_authority_transfer_events(
    state: &State,
    route_scope: &RouteScope,
    grant: crate::server::global_state::MasterGrant,
) -> Vec<Event> {
    let mut events = state
        .global
        .lookup_project_for_route(route_scope)
        .into_iter()
        .flat_map(|project| project.master_grants.values())
        .filter(|current| {
            current.project_scope == route_scope.project_scope_id
                && current.app_scope_id == route_scope.app_scope_id
        })
        .map(|current| Event::GlobalMasterRevoked {
            project_scope: current.project_scope.clone(),
            binding_id: current.binding_id.clone(),
        })
        .collect::<Vec<_>>();
    events.push(Event::GlobalMasterGranted { grant });
    events
}

fn current_master_grant(
    state: &State,
    route_scope: Option<&RouteScope>,
) -> Option<crate::server::global_state::MasterGrant> {
    if let Some(route_scope) = route_scope {
        let project = state.global.lookup_project_for_route(route_scope)?;
        let mut grants = project.master_grants.values().filter(|grant| {
            grant.project_scope == route_scope.project_scope_id
                && grant.app_scope_id == route_scope.app_scope_id
                && state
                    .global
                    .lookup_master_grant(&grant.project_scope, &grant.binding_id)
                    .is_some_and(|current| current == *grant)
        });
        let grant = grants.next()?.clone();
        return grants.next().is_none().then_some(grant);
    }
    let mut grants = state
        .global
        .projects
        .values()
        .flat_map(|project| project.master_grants.values())
        .filter(|grant| {
            state
                .global
                .lookup_master_grant(&grant.project_scope, &grant.binding_id)
                .is_some_and(|current| current == *grant)
        });
    let grant = grants.next()?.clone();
    grants.next().is_none().then_some(grant)
}

fn current_master_worker_id(state: &State, route_scope: Option<&RouteScope>) -> Option<String> {
    current_master_grant(state, route_scope).map(|grant| grant.agent_id.as_str().to_owned())
}

fn communication_recovery_brief() -> serde_json::Value {
    json!({
        "on_error": "Preserve the exact communication error and durable IDs; an ACK, notification acceptance, daemon health, or timeout is not delivery.",
        "steps": [
            "Run `collab context` and inspect the named route, identity, daemon, task, and inbox state.",
            "After a daemon restart, identity mismatch, or missing route, run `collab context` from the canonical project main tree; do not run `collab worker recover` or inject recovery into a foreign thread.",
            "If `collab context` still fails with `IDENTITY_REBIND_UNPROVEN`, preserve that error and the worker_id from the error or the last successful `collab context`; report both to the live master with `COLLAB_WORKER=<worker_id> collab sendmessage --from <worker_id> --to <master> --subject blocker \"<exact error; worker_id=<worker_id>; cause; decision needed>\"`; if that command cannot authenticate, report out-of-band to the live master through a healthy peer or the human. For `IDENTITY_RESTORE_CROSS_PROJECT`, report out-of-band to the live master through a healthy peer or the human instead of retrying `collab sendmessage` through the same failing identity path. For `TOKEN_MISMATCH` no `collab` command can be the repair, because every command run as that worker re-authenticates through `me()` and re-sends the rejected token: escalate out of band with the concrete `worker_id` to the project owner, or to the live master through a healthy peer.",
            "If recovery still fails, report the exact error, root cause, proposed fix, and decision needed to the live master; do not edit routes, journal, mailbox, tokens, or start a second daemon."
        ],
        "close_only_when": [
            "the same native target produces a result item",
            "the durable receipt for that result is consumed through the canonical receive/consume operation; read-only inspection alone does not close",
            "the bug or feature record is updated with the full evidence"
        ]
    })
}

fn role_brief(server: &Server, state: &State, worker_id: &str) -> serde_json::Value {
    let route_scope = server_route_scope(server, state).ok().flatten();
    if current_master_worker_id(state, route_scope.as_ref()).as_deref() == Some(worker_id) {
        return json!({
            "role": "master",
            "role_task": "Orchestrate the project; implementation is not your primary job.",
            "responsibilities": [
                "Run `appsdk longhorizon show` to reconstruct goal, tasks, workers, blockers, and bugs.",
                "Split work into independent scopes; assign tasks and resources; keep useful worker capacity loaded.",
                "Before ending each scheduling turn, saturate every live present peer first, then schedule managed subagents within the configured cap; never stay idle while eligible capacity remains.",
                "Delivery, merge, or a review verdict is not a lifecycle endpoint; drive review/integration/cleanup/close and assign the next ready P0/P1 task.",
                "Own worker blockers: investigate, unblock, reassign, or close. Do not wait for someone else.",
                "Drive test, verification, commit, merge, worktree cleanup, and task closure.",
                "Continue under the standing goal without waiting for user input; hold wakes only for a true external approval or dependency gate."
            ],
            "communication_recovery": communication_recovery_brief(),
            "authority": {
                "managed_subagent": false,
                "must_obey_master": false,
                "may_decline_master_invite": true
            },
            "derivation": {
                "kind": "project-master",
                "parent": null
            },
            "blocked_boundary": "Investigate and unblock first; only pause for a true external approval or dependency gate.",
            "completion_action": "Drive the project to verified merge, cleanup, task closure, and final acceptance.",
            "next_action": "Run `appsdk longhorizon show`, saturate live peers first, then schedule managed subagents within the configured cap; do not end the scheduling turn while eligible capacity remains idle. Delivery or review triggers review/integration/cleanup/dispatch, not an endpoint.",
            "notification_rule": "A notification is an interrupt, not completion. Do its P0/P1/P2 action, then resume scheduling; never stop on ACK/read/summary."
        });
    }
    if is_managed_subagent(state, worker_id) {
        let parent = state
            .subagents
            .values()
            .find(|record| record.peer == worker_id)
            .map(|record| record.parent.clone());
        return json!({
            "role": "managed-subagent",
            "role_task": "Execute the assigned independent task and return evidence to parent/master.",
            "responsibilities": [
                "Stay inside the assigned task, worktree, file scope, delivery conditions, and tests.",
                "Accept and execute master/parent instructions for this assignment; do not create a global schedule.",
                "On trouble, investigate first. Send root cause, attempted actions, proposed fix, and any required decision to the live master; copy parent when different.",
                "Complete implementation, tests, commit, delivery evidence, and resource cleanup; do not stop at code-written or ACK."
            ],
            "communication_recovery": communication_recovery_brief(),
            "authority": {
                "managed_subagent": true,
                "must_obey_master": true,
                "may_decline_master_invite": false
            },
            "derivation": {
                "kind": "managed-subagent",
                "parent": parent
            },
            "blocked_boundary": "Stay within the assigned task and report a concrete root cause, proposed fix, and required decision to parent/master.",
            "completion_action": "Return the completed scoped task with implementation, tests, commit, delivery evidence, and resource cleanup.",
            "next_action": "Continue the assigned task; report ready when idle.",
            "notification_rule": "Handle the named priority action, then resume your assigned task. Reading or ACK is never task progress."
        });
    }
    json!({
        "role": "worker",
        "role_task": "Own and complete your independent task; collaborate with the master without abandoning existing ownership.",
        "responsibilities": [
            "Execute your registered task end to end within its worktree and file scope: implement, test, commit, deliver evidence, and close resources.",
            "Evaluate master collaboration requests against current ownership and capacity. Accept ready non-conflicting work; decline or negotiate conflicts explicitly instead of silently ignoring them.",
            "On trouble, investigate first. Report root cause, attempted actions, proposed fix, and the exact decision needed to the live master.",
            "Do not wait passively and do not stop on ACK/read/summary; after handling a notification, resume your current task."
        ],
        "communication_recovery": communication_recovery_brief(),
        "authority": {
            "managed_subagent": false,
            "must_obey_master": false,
            "may_decline_master_invite": true
        },
        "derivation": {
            "kind": "peer",
            "parent": null
        },
        "blocked_boundary": "Protect current ownership and capacity; negotiate conflicts explicitly instead of silently accepting or ignoring them.",
        "completion_action": "Own the task through implementation, verification, delivery evidence, and resource closure.",
        "next_action": "Resume the registered task or remain available for an explicit dispatch.",
        "notification_rule": "P0 preempts P1, P1 preempts P2. Higher priority interrupts but does not cancel your owned task."
    })
}
