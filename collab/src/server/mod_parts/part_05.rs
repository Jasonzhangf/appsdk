pub(crate) fn load_host_route_records(path: &Path) -> Result<Vec<HostRouteRecord>, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "HOST_ROUTE_REPLAY_FAILED: read route journal: {error}"
            ))
        }
    };
    if content.is_empty() {
        return Ok(Vec::new());
    }
    if !content.ends_with('\n') {
        return Err("HOST_ROUTE_REPLAY_FAILED: route journal must end with a newline".into());
    }
    let mut records = Vec::new();
    let mut seen_keys = std::collections::BTreeMap::<RouteKey, usize>::new();
    let mut seen_storage_roots = std::collections::BTreeMap::<String, (RouteKey, usize)>::new();
    for (index, chunk) in content.split_inclusive('\n').enumerate() {
        let line = chunk
            .strip_suffix('\n')
            .expect("split_inclusive always returns a newline-terminated chunk");
        // Accept the conventional JSONL CRLF representation while treating
        // every other empty/whitespace-only physical line as corruption. A
        // trailing newline is framing, not an additional blank record.
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() {
            return Err(format!(
                "HOST_ROUTE_REPLAY_FAILED: empty route journal line {}",
                index + 1
            ));
        }
        let record = serde_json::from_str::<HostRouteRecord>(line).map_err(|error| {
            format!(
                "HOST_ROUTE_REPLAY_FAILED: route journal line {}: {error}",
                index + 1
            )
        })?;
        let key = (record.app_scope_id.clone(), record.project_scope.clone());
        if let Some(previous_line) = seen_keys.insert(key.clone(), index + 1) {
            return Err(format!(
                "HOST_ROUTE_REPLAY_FAILED: duplicate route key ({}, {}) at lines {} and {}",
                key.0,
                key.1,
                previous_line,
                index + 1
            ));
        }
        if let Some((previous_key, previous_line)) =
            seen_storage_roots.insert(record.storage_root.clone(), (key.clone(), index + 1))
        {
            return Err(format!(
                "HOST_ROUTE_REPLAY_FAILED: duplicate runtime storage root {} for routes ({}, {}) and ({}, {}) at lines {} and {}",
                record.storage_root,
                previous_key.0,
                previous_key.1,
                key.0,
                key.1,
                previous_line,
                index + 1
            ));
        }
        records.push(record);
    }
    Ok(records)
}

pub(crate) fn validate_host_route_record(
    record: &HostRouteRecord,
) -> Result<(RouteKey, PathBuf, PathBuf), String> {
    if record.version != 1 || record.op != "register" {
        return Err(format!(
            "HOST_ROUTE_REPLAY_FAILED: unsupported route record version/op {}/{}",
            record.version, record.op
        ));
    }
    let app_scope = AppServerId::new(record.app_scope_id.clone())
        .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: app scope: {error}"))?;
    let project_scope = ProjectScopeId::new(record.project_scope.clone())
        .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: project scope: {error}"))?;
    let root = std::fs::canonicalize(&record.canonical_root).map_err(|error| {
        format!(
            "HOST_ROUTE_REPLAY_FAILED: canonical root {}: {error}",
            record.canonical_root
        )
    })?;
    let expected_root = std::fs::canonicalize(project_scope.as_str()).map_err(|error| {
        format!(
            "HOST_ROUTE_REPLAY_FAILED: canonical project scope {}: {error}",
            project_scope.as_str()
        )
    })?;
    if root != expected_root {
        return Err(
            "HOST_ROUTE_REPLAY_FAILED: route project scope does not match canonical root".into(),
        );
    }
    let storage_root = PathBuf::from(&record.storage_root);
    let storage_root =
        validate_runtime_storage_root(&root, &storage_root, "HOST_ROUTE_REPLAY_FAILED")?;
    Ok((
        (
            app_scope.as_str().to_owned(),
            project_scope.as_str().to_owned(),
        ),
        root,
        storage_root,
    ))
}

fn route_record_is_replayable(record: &HostRouteRecord) -> Result<bool, String> {
    match std::fs::canonicalize(&record.canonical_root) {
        Ok(root) => Ok(root.join(".agent-collab").is_dir()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "HOST_ROUTE_REPLAY_FAILED: canonical root {}: {error}",
            record.canonical_root
        )),
    }
}

fn validate_command_id(value: &str) -> Result<(), notification_contract::JournalError> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(notification_contract::JournalError::InvalidCommand(
            "command and operation ids must be non-empty, <=256 bytes, and control-free".into(),
        ));
    }
    Ok(())
}

pub(crate) fn purge_expired_storage(server: &Server, now: i64) -> usize {
    if server.state.lock().unwrap().admission_frozen() {
        return 0;
    }
    let cutoff = server.config.retention.cutoff_ms(now);
    let mut st = server.state.lock().unwrap();
    let expired: Vec<String> = st
        .msgs
        .values()
        .filter(|message| message.created_ms <= cutoff)
        .map(|message| message.id.clone())
        .collect();
    mailbox::purge_message_snapshot_files(&server.storage_root, &expired, &st);
    if expired.is_empty() {
        return 0;
    }
    for id in &expired {
        st.drop_message(id);
    }
    if let Err(error) = server.rewrite_journal_locked(&st) {
        st.journal_poison = Some(error.to_string());
        append_log(
            &server.log_path(),
            &format!("JOURNAL_COMPACTION_FAILED: {error}"),
        );
    }
    expired.len()
}

pub fn gen_msg_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("m{}-{}", now_ms(), n)
}

fn worktree_binding_for_task(server: &Server, task: &TaskRec) -> Option<WorktreeBinding> {
    let worktree_root = task.worktree_path.as_ref()?.clone();
    let owning_project_scope = server.root.to_str()?.to_owned();
    Some(WorktreeBinding {
        worktree_root,
        owning_project_scope,
        task_id: task.id.clone(),
        owner_agent_id: task.owner.clone(),
        binding_id: format!("binding-task-{}", task.id),
        base_commit: task.base_commit.clone().unwrap_or_default(),
    })
}

/// Normalize every whitespace-delimited token in a handoff body to its
/// leading path substring.  Relative (`playground/...`, `./...`, `../...`),
/// absolute, markdown link (`[x](/abs/...)`), and parenthesized forms all
/// reduce to the same candidate here; tokens with no path separator are prose
/// and are skipped.
fn handoff_referenced_paths(body: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for token in body.split_whitespace() {
        let token = token.trim_matches(|c: char| {
            matches!(
                c,
                '`' | '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ';' | ':'
            )
        });
        // A markdown link carries its path after `](`; the label itself is
        // prose, so only the target is inspected.
        let token = match token.find("](") {
            Some(index) => &token[index + 2..],
            None => token,
        };
        // Keep relative references intact, including spellings that do not
        // start with `playground/`.  Resolving them against the project root
        // is what binds them to project-owned worktree identity; collapsing
        // them to their first separator would silently relocate `./x` to `/x`.
        let candidate = if token.starts_with("./") || token.starts_with("../") {
            token
        } else if token.to_ascii_lowercase().starts_with("playground/") {
            token
        } else {
            let Some(start) = token.find('/') else {
                continue;
            };
            &token[start..]
        };
        let candidate = candidate.trim_end_matches(|c: char| {
            matches!(
                c,
                '.' | ',' | ';' | ':' | ')' | ']' | '}' | '\'' | '"' | '`'
            )
        });
        if candidate.len() <= 1 {
            continue;
        }
        if !paths.iter().any(|existing| existing == candidate) {
            paths.push(candidate.to_owned());
        }
    }
    paths
}

/// Resolve a handoff reference to a project worktree, if it is one.  Ownership
/// is bound to project identity only: the reference must live under this
/// project's playground or match a registered task worktree exactly.  A path
/// leaf is never evidence of ownership, so an unrelated absolute path or a
/// prose URL that merely ends in a worktree-like segment is not our business
/// and never fails a send.
fn handoff_worktree_reference(server: &Server, state: &State, candidate: &str) -> Option<PathBuf> {
    let playground = server.root.join("playground");
    let raw = Path::new(candidate);
    let absolute = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        server.root.join(raw)
    };
    let normalized = normalize_path_lexically(&absolute);
    let playground_normalized = normalize_path_lexically(&playground);
    if normalized.starts_with(&playground_normalized) {
        return Some(normalized);
    }
    for task in state.tasks.values() {
        let Some(registered) = task.worktree_path.as_deref() else {
            continue;
        };
        // A registered worktree path is project-scoped, so a relative spelling
        // resolves against the owning project root before equality is tested.
        let registered = Path::new(registered);
        let registered = if registered.is_absolute() {
            registered.to_path_buf()
        } else {
            server.root.join(registered)
        };
        let registered = normalize_path_lexically(&registered);
        if normalized == registered {
            return Some(normalized);
        }
    }
    None
}

/// Resolve a path with `.`/`..` collapsed so containment checks do not
/// depend on the path already existing.
fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn handoff_unresolved_recipient(to: &str) -> (String, serde_json::Value) {
    (
        format!("HANDOFF_TARGET_UNRESOLVED: recipient {to} not registered"),
        json!({
            "error_code": "HANDOFF_TARGET_UNRESOLVED",
            "unresolved": {"kind": "recipient", "value": to},
            "recipient": to,
            "reason": "recipient_not_registered",
            "recorded": false,
        }),
    )
}

/// Validate a handoff path reference before any durable record is written.
/// Every worktree this send actually references must still exist; otherwise
/// the caller gets a typed error naming the unresolved target instead of
/// being told the handoff was recorded.
fn resolve_handoff_target(
    server: &Server,
    state: &State,
    to: &str,
    body: &str,
) -> Result<(), (String, serde_json::Value)> {
    for candidate in handoff_referenced_paths(body) {
        let Some(path) = handoff_worktree_reference(server, state, &candidate) else {
            continue;
        };
        if !path.exists() {
            return Err((
                format!(
                    "HANDOFF_TARGET_UNRESOLVED: worktree {} does not exist",
                    path.display()
                ),
                json!({
                    "error_code": "HANDOFF_TARGET_UNRESOLVED",
                    "unresolved": {"kind": "worktree", "value": path.display().to_string()},
                    "recipient": to,
                    "reason": "worktree_path_missing",
                    "recorded": false,
                }),
            ));
        }
    }
    Ok(())
}

fn iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

fn default_direct_message_events(
    state: &State,
    worker_id: &str,
    transport: &SelectedTransport,
    now: i64,
) -> Vec<Event> {
    let mut events = Vec::new();
    let default_id = default_direct_message_id(worker_id);
    // Only an explicit owner unsubscribe is a durable stop for the
    // system-owned default lease. Any other `cancelled` state (for example a
    // legacy automatic cancel, or an untyped status write) is recoverable and
    // is re-armed below, so a retired default lease can never leave a
    // registered peer permanently wake-less.
    if state
        .notification_subscriptions
        .get(&default_id)
        .is_some_and(|subscription| {
            subscription.status == "cancelled"
                && subscription.status_reason.as_deref() == Some(EXPLICIT_UNSUBSCRIBE_REASON)
        })
    {
        return events;
    }
    let refresh_after_ms = DEFAULT_DIRECT_MESSAGE_TTL_SECONDS as i64 * 1000 / 2;
    let mut current_is_fresh = false;
    let mut current_is_armed = false;
    let mut refresh_due = false;
    for subscription in state.notification_subscriptions.values().filter(|sub| {
        sub.worker_id == worker_id && sub.event == "direct-message" && sub.status == "armed"
    }) {
        if subscription.id == default_id {
            current_is_armed = true;
            if subscription_matches_transport(subscription, transport) {
                current_is_fresh = true;
                refresh_due = subscription.expires_ms - now < refresh_after_ms;
            }
            continue;
        }
        events.push(Event::NotificationStatus {
            subscription_id: subscription.id.clone(),
            status: "rebound".into(),
            updated_ms: now,
        });
    }
    if current_is_fresh {
        if refresh_due {
            let Some(thread_id) = transport.thread_id.as_deref() else {
                return events;
            };
            events.push(Event::NotificationSubscribed {
                subscription: NotificationSubscription {
                    id: default_id,
                    worker_id: worker_id.into(),
                    event: "direct-message".into(),
                    subject: None,
                    target: thread_id.into(),
                    method: transport.kind.as_str().into(),
                    trigger_ms: None,
                    trigger_times_ms: Vec::new(),
                    interval_ms: None,
                    repeat_count: 1,
                    fired_count: 0,
                    expires_ms: now
                        .saturating_add(DEFAULT_DIRECT_MESSAGE_TTL_SECONDS as i64 * 1000),
                    status: "armed".into(),
                    created_ms: now,
                    updated_ms: now,
                    status_reason: None,
                },
            });
        }
        return events;
    }
    // The default lease is system-owned and must follow the registered
    // transport. Keep an explicit journal transition before replacing a
    // retired target so upgrades cannot leave the peer permanently unwoken.
    if current_is_armed {
        let Some(thread_id) = transport.thread_id.as_deref() else {
            return events;
        };
        events.push(Event::NotificationStatus {
            subscription_id: default_id.clone(),
            status: "transport-lost".into(),
            updated_ms: now,
        });
        events.push(Event::NotificationSubscribed {
            subscription: NotificationSubscription {
                id: default_id,
                worker_id: worker_id.into(),
                event: "direct-message".into(),
                subject: None,
                target: thread_id.into(),
                method: transport.kind.as_str().into(),
                trigger_ms: None,
                trigger_times_ms: Vec::new(),
                interval_ms: None,
                repeat_count: 1,
                fired_count: 0,
                expires_ms: now.saturating_add(DEFAULT_DIRECT_MESSAGE_TTL_SECONDS as i64 * 1000),
                status: "armed".into(),
                created_ms: now,
                updated_ms: now,
                status_reason: None,
            },
        });
        return events;
    }
    let Some(thread_id) = transport.thread_id.as_deref() else {
        return events;
    };
    events.push(Event::NotificationSubscribed {
        subscription: NotificationSubscription {
            id: default_id,
            worker_id: worker_id.into(),
            event: "direct-message".into(),
            subject: None,
            target: thread_id.into(),
            method: transport.kind.as_str().into(),
            trigger_ms: None,
            trigger_times_ms: Vec::new(),
            interval_ms: None,
            repeat_count: 1,
            fired_count: 0,
            expires_ms: now.saturating_add(DEFAULT_DIRECT_MESSAGE_TTL_SECONDS as i64 * 1000),
            status: "armed".into(),
            created_ms: now,
            updated_ms: now,
            status_reason: None,
        },
    });
    events
}

fn registered_peer_default_events(state: &State, now: i64) -> Vec<Event> {
    let mut workers = state.workers.values().collect::<Vec<_>>();
    workers.sort_by_key(|worker| worker.id.as_str());
    workers
        .into_iter()
        .filter_map(|worker| {
            selected_transport_for_worker(worker).map(|transport| (worker.id.as_str(), transport))
        })
        .flat_map(|(worker_id, transport)| {
            default_direct_message_events(state, worker_id, &transport, now)
        })
        .collect()
}

fn restore_registered_peer_default_leases(server: &Server) {
    let events = {
        let state = server.state.lock().unwrap();
        registered_peer_default_events(&state, now_ms())
    };
    if !events.is_empty() {
        server.commit(&events);
    }
}

fn selected_transport_for_worker(worker: &WorkerRec) -> Option<SelectedTransport> {
    worker.transport.clone()
}

fn subscription_matches_transport(
    subscription: &NotificationSubscription,
    transport: &SelectedTransport,
) -> bool {
    subscription.method == transport.kind.as_str()
        && transport
            .thread_id
            .as_deref()
            .is_some_and(|thread_id| subscription.target == thread_id)
}

pub(crate) fn subscription_matches_transport_by_worker(
    server: &Server,
    subscription_id: &str,
    worker_id: &str,
    transport: &SelectedTransport,
) -> bool {
    let state = server.state.lock().unwrap();
    let Some(subscription) = state.notification_subscriptions.get(subscription_id) else {
        return false;
    };
    subscription.worker_id == worker_id
        && subscription_matches_transport(subscription, transport)
        && state
            .workers
            .get(worker_id)
            .is_some_and(|worker| selected_transport_for_worker(worker).as_ref() == Some(transport))
}

fn attempt_tmux_notification_with_at(
    server: &Server,
    message_id: &str,
    subscription_id: &str,
    recipient: &str,
    transport: &SelectedTransport,
    source_thread_id: Option<&str>,
    delay: i64,
    explicit: bool,
    now: i64,
    explicit_retry: bool,
    deliver: &dyn Fn(
        &SelectedTransport,
        Option<&str>,
        &str,
        &str,
        bool,
        &str,
    ) -> Result<serde_json::Value, String>,
) -> NotificationAttempt {
    attempt_tmux_notification_with_retry(
        server,
        message_id,
        subscription_id,
        recipient,
        transport,
        source_thread_id,
        delay,
        explicit,
        now,
        false,
        explicit_retry,
        deliver,
    )
}

fn notification_transport_method(transport: &SelectedTransport) -> &'static str {
    match transport.kind {
        TransportKind::Tmux => "tmux",
        TransportKind::AppServer => "appserver",
    }
}

fn notification_method_for_worker(state: &State, worker_id: &str) -> &'static str {
    state
        .workers
        .get(worker_id)
        .and_then(selected_transport_for_worker)
        .map(|transport| notification_transport_method(&transport))
        .unwrap_or("tmux")
}

fn attempt_tmux_notification_with_retry(
    server: &Server,
    message_id: &str,
    subscription_id: &str,
    recipient: &str,
    transport: &SelectedTransport,
    source_thread_id: Option<&str>,
    delay: i64,
    explicit: bool,
    now: i64,
    allow_retry: bool,
    retry: bool,
    deliver: &dyn Fn(
        &SelectedTransport,
        Option<&str>,
        &str,
        &str,
        bool,
        &str,
    ) -> Result<serde_json::Value, String>,
) -> NotificationAttempt {
    let mut state = server.state.lock().unwrap();
    let Some(seed_id) = state.msgs.get(message_id).map(|message| message.id.clone()) else {
        return NotificationAttempt::NotAttempted("durable message not found".into());
    };
    if !state.scheduler_message_deliverable(message_id) {
        return NotificationAttempt::NotAttempted("scheduler admission is not deliverable".into());
    }
    let unacked_notifications = state
        .msgs
        .values()
        .filter(|message| message.to == recipient && message.state == "delivered")
        .count();
    if unacked_notifications >= server.config.notifications.max_unacked as usize {
        return NotificationAttempt::NotAttempted(
            "recipient has too many unacked notifications".into(),
        );
    }
    let Some(subscription) = state.notification_subscriptions.get(subscription_id) else {
        return NotificationAttempt::NotAttempted("notification subscription not found".into());
    };
    if subscription.worker_id != recipient
        || !subscription_matches_transport(subscription, transport)
    {
        return NotificationAttempt::NotAttempted(
            format!(
                "subscription does not match the selected {method} transport",
                method = notification_transport_method(transport)
            )
            .into(),
        );
    }
    let delivery_mode = state
        .delivery_modes
        .get(&seed_id)
        .cloned()
        .unwrap_or_else(|| "immediate".into());
    let mut batch = state
        .msgs
        .values()
        .filter_map(|message| {
            let binding = state.wake_bindings.get(&message.id)?;
            let sub = state.notification_subscriptions.get(binding)?;
            let message_explicit = is_explicit_notification(&state, message);
            (message.to == recipient
                && state.scheduler_message_deliverable(&message.id)
                && message_explicit == explicit
                && state
                    .delivery_modes
                    .get(&message.id)
                    .map(String::as_str)
                    .unwrap_or("immediate")
                    == delivery_mode.as_str()
                && notification_delivery_delay_ms(
                    &state,
                    &message.id,
                    &sub.event,
                    &server.config.notifications,
                ) == delay
                && message.state == "pending"
                && (if retry {
                    message.id == seed_id
                } else {
                    allow_retry || message.wake_attempt_count < MAX_WAKE_ATTEMPTS
                })
                && sub.worker_id == recipient
                && subscription_matches_transport(sub, transport)
                && sub.status == "armed"
                && sub.expires_ms > now)
                .then(|| {
                    notification_text(message).map(|text| {
                        (
                            message.created_ms,
                            message.id.clone(),
                            binding.clone(),
                            sub.event.clone(),
                            text,
                        )
                    })
                })
                .flatten()
        })
        .collect::<Vec<_>>();
    batch.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let Some(window_start_ms) = batch.first().map(|candidate| candidate.0) else {
        return NotificationAttempt::NotAttempted("no notification batch is ready".into());
    };
    let (batch, remaining) = mailbox::select_batch(batch, delay, window_start_ms);
    let Some(first) = batch.first() else {
        return NotificationAttempt::NotAttempted("notification batch is empty".into());
    };
    let last_attempt = state
        .msgs
        .values()
        .filter_map(|message| {
            let binding = state.wake_bindings.get(&message.id)?;
            let sub = state.notification_subscriptions.get(binding)?;
            let message_explicit = is_explicit_notification(&state, message);
            (message.to == recipient
                && message_explicit == explicit
                && state
                    .delivery_modes
                    .get(&message.id)
                    .map(String::as_str)
                    .unwrap_or("immediate")
                    == delivery_mode.as_str()
                && notification_delivery_delay_ms(
                    &state,
                    &message.id,
                    &sub.event,
                    &server.config.notifications,
                ) == delay
                && sub.worker_id == recipient
                && subscription_matches_transport(sub, transport))
            .then_some(message.last_wake_attempt_ms)
        })
        .max()
        .unwrap_or(0);
    if now.saturating_sub(window_start_ms) < delay || now.saturating_sub(last_attempt) < delay {
        return NotificationAttempt::NotAttempted(
            "notification delivery window has not elapsed".into(),
        );
    }
    server.commit_locked(
        &mut state,
        &[Event::WakeAttempted {
            ids: batch.iter().map(|message| message.1.clone()).collect(),
            attempted_ms: now,
            retry,
        }],
    );
    drop(state);

    let text = truncate_notification(compose_notification(
        &first.1,
        "notification-batch",
        &batch_notification_text(&batch, remaining),
    ));
    match deliver(
        transport,
        source_thread_id,
        &text,
        &format!("collab-notification-{}", first.1),
        explicit,
        &delivery_mode,
    ) {
        Ok(receipt) => {
            append_log(
                &server.log_path(),
                &format!(
                    "{method_label}_WAKE_SUBMITTED recipient={recipient} message={seed_id} explicit={explicit} receipt={receipt}",
                    method_label = match transport.kind {
                        TransportKind::Tmux => "TMUX",
                        TransportKind::AppServer => "APPSERVER",
                    }
                ),
            );
            let accepted_ms = now_ms();
            let mut state = server.state.lock().unwrap();
            server.commit_locked(
                &mut state,
                &batch
                    .iter()
                    .map(|message| Event::NotificationDeliveryAccepted {
                        message_id: message.1.clone(),
                        accepted_ms,
                        evidence: Some(receipt.clone()),
                    })
                    .collect::<Vec<_>>(),
            );
            NotificationAttempt::Accepted
        }
        Err(error) => {
            append_log(
                &server.log_path(),
                &format!(
                    "{method_label}_WAKE_FAILED recipient={recipient} message={seed_id} error={error}",
                    method_label = match transport.kind {
                        TransportKind::Tmux => "TMUX",
                        TransportKind::AppServer => "APPSERVER",
                    }
                ),
            );
            let mut state = server.state.lock().unwrap();
            server.commit_locked(
                &mut state,
                &[Event::NotificationDeliveryFailed {
                    message_id: seed_id,
                    operation: if explicit {
                        "notification.emitted".into()
                    } else {
                        "notification.batch_emitted".into()
                    },
                    error: error.clone(),
                    failed_ms: now,
                    retryable: matches!(
                        crate::client::adapters::AdapterError::notification_class_from_display(
                            &error
                        ),
                        crate::client::adapters::NotificationDeliveryClass::KnownNotDelivered
                    ),
                }],
            );
            NotificationAttempt::Rejected(error)
        }
    }
}

fn attempt_notification_with(
    server: &Server,
    message_id: &str,
    subscription_id: &str,
    _can_receive: &dyn Fn(&str) -> bool,
    _deliver: &dyn Fn(&str, &str) -> bool,
    _owns_transport: &dyn Fn(&str, &str) -> Result<bool, ()>,
) -> bool {
    attempt_notification_with_at(server, message_id, subscription_id, now_ms())
}

fn attempt_notification_with_at(
    server: &Server,
    message_id: &str,
    subscription_id: &str,
    now: i64,
) -> bool {
    attempt_notification_detailed_with_at(server, message_id, subscription_id, now).accepted()
}

fn attempt_notification_detailed_with_at(
    server: &Server,
    message_id: &str,
    subscription_id: &str,
    now: i64,
) -> NotificationAttempt {
    attempt_notification_detailed_with_mode_at(server, message_id, subscription_id, now, false)
}

fn attempt_notification_detailed_with_mode_at(
    server: &Server,
    message_id: &str,
    subscription_id: &str,
    now: i64,
    explicit_retry: bool,
) -> NotificationAttempt {
    if !server.config.notifications.enabled {
        return NotificationAttempt::NotAttempted("notifications are disabled".into());
    }
    let (recipient, transport, source_thread_id, delay, explicit) = {
        let mut state = server.state.lock().unwrap();
        let Some(seed) = state.msgs.get(message_id) else {
            return NotificationAttempt::NotAttempted("durable message not found".into());
        };
        if !state.scheduler_message_deliverable(message_id) {
            return NotificationAttempt::NotAttempted(
                "scheduler admission is not deliverable".into(),
            );
        }
        let recipient = seed.to.clone();
        let Some(subscription) = state.notification_subscriptions.get(subscription_id) else {
            return NotificationAttempt::NotAttempted("notification subscription not found".into());
        };
        if subscription.worker_id != recipient {
            return NotificationAttempt::NotAttempted(
                "subscription does not belong to the recipient".into(),
            );
        }
        let delay = notification_delivery_delay_ms(
            &state,
            message_id,
            &subscription.event,
            &server.config.notifications,
        );
        let worker_transport = state
            .workers
            .get(&recipient)
            .and_then(selected_transport_for_worker);
        let Some(transport) = worker_transport else {
            return NotificationAttempt::NotAttempted(
                "registered worker has no server-selected transport".into(),
            );
        };
        if !subscription_matches_transport(subscription, &transport) {
            server.commit_locked(
                &mut state,
                &[
                    Event::NotificationStatus {
                        subscription_id: subscription_id.to_string(),
                        status: "transport-lost".into(),
                        updated_ms: now,
                    },
                    Event::NotificationDeliveryFailed {
                        message_id: message_id.to_string(),
                        operation: "notification.not_attempted".into(),
                        error: format!(
                            "subscription does not match the selected {method} transport",
                            method = notification_transport_method(&transport)
                        )
                        .into(),
                        failed_ms: now,
                        retryable: true,
                    },
                ],
            );
            return NotificationAttempt::NotAttempted(
                format!(
                    "subscription does not match the selected {method} transport",
                    method = notification_transport_method(&transport)
                )
                .into(),
            );
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
    let attempt = attempt_tmux_notification_with_at(
        server,
        message_id,
        subscription_id,
        &recipient,
        &transport,
        source_thread_id.as_deref(),
        delay,
        explicit,
        now,
        explicit_retry,
        &|transport, source_thread_id, text, message_id, explicit, mode| {
            (notification_sink(server))(
                transport,
                source_thread_id,
                text,
                message_id,
                explicit,
                mode,
            )
        },
    );
    if let NotificationAttempt::NotAttempted(error) = &attempt {
        let mut state = server.state.lock().unwrap();
        // Never let a later pre-delivery skip overwrite the class of an
        // earlier native attempt. Accepted attempts leave no failure record
        // at all; unknown and decode failures keep their non-retryable class.
        if !state
            .notification_delivery_failures
            .contains_key(message_id)
        {
            server.commit_locked(
                &mut state,
                &[Event::NotificationDeliveryFailed {
                    message_id: message_id.to_string(),
                    operation: "notification.not_attempted".into(),
                    error: error.clone(),
                    failed_ms: now,
                    retryable: true,
                }],
            );
        }
    }
    attempt
}

fn notification_delivery_delay_ms(
    state: &State,
    message_id: &str,
    event: &str,
    config: &crate::config::Notifications,
) -> i64 {
    match state.delivery_modes.get(message_id).map(String::as_str) {
        Some(
            "explicit-notification"
            | "immediate"
            | "queued"
            | DAEMON_LIVE_CLOSURE_MODE
            | RESTART_REPLAY_PENDING_MODE,
        ) => 0,
        _ => config.delay_ms(event),
    }
}

/// Why one explicit recovery attempt may or may not be issued for a durable
/// message. A refusal is kept as a precise machine-readable reason so a caller
/// never has to guess that an accepted or unknown native attempt was skipped.
fn explicit_retry_refusal(state: &State, message_id: &str) -> Option<&'static str> {
    let message = state.msgs.get(message_id)?;
    if message.state != "pending" {
        return Some("original message is no longer pending");
    }
    if message.retry_attempted {
        return Some("original message was already explicitly retried");
    }
    if state
        .notification_delivery_accepted
        .contains_key(message_id)
    {
        return Some("original native attempt was accepted");
    }
    match state.notification_delivery_failures.get(message_id) {
        Some(failure) if failure.retryable => None,
        Some(_) => Some("original attempt is not known to be undelivered"),
        None => Some("original attempt has no durable delivery failure"),
    }
}

fn attempt_notification(server: &Server, message_id: &str, subscription_id: &str) -> bool {
    attempt_notification_with_at(server, message_id, subscription_id, now_ms())
}

fn notification_send_response(
    mut data: serde_json::Value,
    subscription_present: bool,
    notification_method: &str,
    notification: &NotificationAttempt,
) -> Resp {
    if !subscription_present {
        data["durable"] = json!(true);
        data["notification"] = json!("mailbox-only-no-subscription");
        data["notification_error"] = json!(NOTIFICATION_SUBSCRIPTION_MISSING_ERROR);
        data["failure"] = json!("notification_subscription_missing");
        data["repair_required"] = json!(true);
        data["escalation"] = json!(MAILBOX_ONLY_ESCALATION);
        return Resp::data(data);
    }
    match notification {
        NotificationAttempt::Accepted => {
            data["durable"] = json!(true);
            data["notification"] = if notification_method == "appserver" {
                json!("appserver-input-submitted")
            } else {
                json!("tmux-input-submitted")
            };
            data["consumed"] = json!(false);
            Resp::data(data)
        }
        NotificationAttempt::Rejected(error) => {
            data["durable"] = json!(true);
            data["notification"] = json!("subscribed-not-sent");
            data["notification_error"] = json!(error);
            data["failure"] = json!("notification_delivery_failed");
            data["repair_required"] = json!(true);
            data["escalation"] = json!(
                "the message is durable but this wake has an ambiguous submission outcome; do not retry this wake, have the recipient run collab recv, and send a new message if another wake is needed"
            );
            Resp::err_data(
                notification_rejected_label(notification_method, &format!("{error}")),
                data,
            )
        }
        NotificationAttempt::NotAttempted(error) => {
            data["durable"] = json!(true);
            data["notification"] = json!("subscribed-not-sent");
            data["notification_error"] = json!(error);
            data["failure"] = json!("notification_delivery_failed");
            data["repair_required"] = json!(true);
            data["escalation"] = json!(
                "report the exact notification_error and durable msg_id to the live master; repair or rebind the selected wake transport, then retry explicitly"
            );
            Resp::err_data(
                notification_rejected_label(notification_method, &format!("{error}")),
                data,
            )
        }
    }
}

fn notification_rejected_label(notification_method: &str, error: &str) -> String {
    if notification_method == "appserver" {
        format!("APPSERVER_NOTIFICATION_REJECTED: {error}")
    } else {
        format!("TMUX_NOTIFICATION_REJECTED: {error}")
    }
}

/// `mailbox-only` is never a silent success: it is an explicit repair terminal
/// carrying the same reason and repair fields as `notification_send_response`.
fn apply_mailbox_only_repair_fields(data: &mut serde_json::Value) {
    data["notification_error"] = json!(NOTIFICATION_SUBSCRIPTION_MISSING_ERROR);
    data["failure"] = json!("notification_subscription_missing");
    data["repair_required"] = json!(true);
    data["escalation"] = json!(MAILBOX_ONLY_ESCALATION);
}

fn attempt_scheduler_notification(
    server: &Server,
    request_id: &str,
    message_id: &str,
    subscription_id: &str,
) -> SchedulerNotificationAttempt {
    if !server.config.notifications.enabled {
        return SchedulerNotificationAttempt::Unavailable;
    }
    let claim_ms = {
        let mut state = server.state.lock().unwrap();
        let Some(admission) = state.scheduler_admissions.get(request_id) else {
            return SchedulerNotificationAttempt::Unavailable;
        };
        if admission.status == "notifying" {
            if now_ms().saturating_sub(admission.updated_ms) < state::REQUEST_COOLDOWN_MS {
                return SchedulerNotificationAttempt::InFlight;
            }
        } else if admission.status != "pending" {
            return SchedulerNotificationAttempt::Unavailable;
        }
        let claim_ms = now_ms();
        server.commit_locked(
            &mut state,
            &[Event::SchedulerAdmissionStatus {
                request_id: request_id.into(),
                status: "notifying".into(),
                error: None,
                updated_ms: claim_ms,
            }],
        );
        claim_ms
    };
    let delivery = {
        let state = server.state.lock().unwrap();
        state.msgs.get(message_id).and_then(|seed| {
            // A scheduler recovery may re-knock once after a known-undelivered
            // attempt. An accepted or unknown native outcome has no retryable
            // durable failure, so it must stop instead of resending blindly.
            let allow_retry = seed.wake_attempt_count >= MAX_WAKE_ATTEMPTS
                && explicit_retry_refusal(&state, message_id).is_none();
            let recipient = seed.to.clone();
            let subscription = state.notification_subscriptions.get(subscription_id)?;
            if subscription.worker_id != recipient {
                return None;
            }
            let delay = notification_delivery_delay_ms(
                &state,
                message_id,
                &subscription.event,
                &server.config.notifications,
            );
            let transport = state
                .workers
                .get(&recipient)
                .and_then(selected_transport_for_worker)?;
            if !subscription_matches_transport(subscription, &transport) {
                return None;
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
            Some((
                recipient,
                transport,
                source_thread_id,
                delay,
                is_explicit_notification(&state, seed),
                allow_retry,
            ))
        })
    };
    let Some((recipient, transport, source_thread_id, delay, explicit, allow_retry)) = delivery
    else {
        clear_scheduler_notification_claim(server, request_id, claim_ms);
        return SchedulerNotificationAttempt::Rejected;
    };
    let notified = attempt_tmux_notification_with_retry(
        server,
        message_id,
        subscription_id,
        &recipient,
        &transport,
        source_thread_id.as_deref(),
        delay,
        explicit,
        now_ms(),
        allow_retry,
        false,
        &|transport, source_thread_id, text, message_id, explicit, mode| {
            (notification_sink(server))(
                transport,
                source_thread_id,
                text,
                message_id,
                explicit,
                mode,
            )
        },
    );
    if notified.accepted() {
        let mut state = server.state.lock().unwrap();
        if scheduler_notification_claim_is_current(&state, request_id, claim_ms) {
            server.commit_locked(
                &mut state,
                &[Event::SchedulerAdmissionStatus {
                    request_id: request_id.into(),
                    status: "succeeded".into(),
                    error: None,
                    updated_ms: now_ms(),
                }],
            );
        }
        SchedulerNotificationAttempt::Accepted
    } else {
        clear_scheduler_notification_claim(server, request_id, claim_ms);
        SchedulerNotificationAttempt::Rejected
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchedulerNotificationAttempt {
    Accepted,
    Rejected,
    InFlight,
    Unavailable,
}

fn scheduler_notification_claim_is_current(state: &State, request_id: &str, claim_ms: i64) -> bool {
    state
        .scheduler_admissions
        .get(request_id)
        .is_some_and(|admission| {
            admission.status == "notifying" && admission.updated_ms == claim_ms
        })
}

fn clear_scheduler_notification_claim(server: &Server, request_id: &str, claim_ms: i64) {
    let mut state = server.state.lock().unwrap();
    if scheduler_notification_claim_is_current(&state, request_id, claim_ms) {
        let error = state
            .scheduler_admissions
            .get(request_id)
            .and_then(|admission| admission.error.clone());
        server.commit_locked(
            &mut state,
            &[Event::SchedulerAdmissionStatus {
                request_id: request_id.into(),
                status: "pending".into(),
                error,
                updated_ms: now_ms(),
            }],
        );
    }
}
