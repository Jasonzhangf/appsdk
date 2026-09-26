use super::*;
use super::helpers::*;
use super::validation::*;

impl CommunicationStore {
    pub fn tick(&mut self, at: Option<&str>) -> CommResult<Value> {
        let at = at.map(validate_time).transpose()?.unwrap_or_else(now);
        let mut addresses = BTreeMap::new();
        for wakeup in self.projection.wakeup.values() {
            addresses.insert(wakeup.address.key(), wakeup.address.clone());
        }
        for accumulator in self.projection.master_wake.values() {
            addresses.insert(accumulator.address.key(), accumulator.address.clone());
        }
        let mut changed = Vec::new();
        let mut master_wake_changed = Vec::new();
        for (_, address) in addresses {
            if let Some(accumulator) = self.projection.master_wake.get(&address.key()).cloned() {
                if accumulator.pending {
                    if let Some(updated) = self.process_master_wake(&accumulator, &at)? {
                        master_wake_changed.push(updated);
                    }
                    continue;
                }
            }
            let Some(wakeup) = self.projection.wakeup.get(&address.key()).cloned() else {
                continue;
            };
            if wakeup.stopped || wakeup.reminders_sent >= DEFAULT_MASTER_REMINDER_LIMIT {
                continue;
            }
            let agent = match self.live_idle_master_at(&wakeup.address, &at) {
                Some(agent) => agent,
                None => continue,
            };
            let due = match wakeup.next_due_at.as_deref() {
                Some(next_due) => parse_time(&at)? >= parse_time(next_due)?,
                None => false,
            };
            if !due {
                continue;
            }
            if self.wakeup_delivery_in_flight(&wakeup)? {
                continue;
            }
            let reminders_sent = wakeup.reminders_sent + 1;
            let stopped = reminders_sent >= DEFAULT_MASTER_REMINDER_LIMIT;
            let next_wakeup = WakeupRecord {
                address: wakeup.address.clone(),
                identity_origin: wakeup.identity_origin.clone(),
                idle_since: wakeup.idle_since.clone(),
                reminders_sent,
                next_due_at: if stopped {
                    None
                } else {
                    Some(add_seconds(&at, DEFAULT_BATCH_WINDOW_SECONDS)?)
                },
                stopped,
                last_reminder_at: Some(at.clone()),
            };
            let (message_id, conversation_id) = wakeup_message_identity(&wakeup, reminders_sent)?;
            let mut message = self.system_message(
                &agent.address(),
                format!(
                    "master idle reminder {reminders_sent}/{}",
                    DEFAULT_MASTER_REMINDER_LIMIT
                ),
                "p1",
                "Master remains idle. Inspect active Bugs and Loop state before the next scheduled round.",
                "master-idle",
                &at,
                "mailbox",
            )?;
            message.message_id = message_id;
            message.conversation_id = conversation_id;
            let message = self.prepare_wakeup_message(message)?;
            let mut notification =
                self.build_notification(&message, &message.created_at, Some(&message.created_at))?;
            notification.notification_id = format!("wakeup-notification-{}", message.message_id);
            let notification_key = self.notification_key(&message, &notification, true);
            if let Some(existing) = self
                .projection
                .notifications
                .get(&notification_key)
                .cloned()
            {
                if existing.message_id == message.message_id {
                    notification = existing;
                } else {
                    self.commit(
                        "notification.queued",
                        json!({
                            "key": notification_key,
                            "notification": notification.clone()
                        }),
                    )?;
                }
            } else {
                self.commit(
                    "notification.queued",
                    json!({
                        "key": notification_key,
                        "notification": notification.clone()
                    }),
                )?;
            }
            if notification.message_id == message.message_id && notification.status == "emitted" {
                let completed_attempt_id = self
                    .projection
                    .completed_attempts
                    .get(&notification_key)
                    .cloned();
                if let Some(attempt_id) = completed_attempt_id.as_deref() {
                    self.commit_wakeup_reminder(
                        &next_wakeup,
                        &message,
                        &notification_key,
                        &notification,
                        Some(attempt_id),
                        notification.transport_receipt.as_ref(),
                    )?;
                    let updated = self
                        .projection
                        .wakeup
                        .get(&wakeup.address.key())
                        .cloned()
                        .ok_or_else(|| {
                            CommError::new("wakeup_not_found", "wakeup update was not projected")
                        })?;
                    changed.push(updated);
                }
                continue;
            }
            if notification.message_id == message.message_id
                && notification.status == "pending"
                && notification.delivery_attempt.is_none()
                && notification.last_error.is_some()
            {
                let completed_attempt_id = self
                    .projection
                    .completed_attempts
                    .get(&notification_key)
                    .cloned();
                if let Some(attempt_id) = completed_attempt_id.as_deref() {
                    self.commit_wakeup_reminder(
                        &next_wakeup,
                        &message,
                        &notification_key,
                        &notification,
                        Some(attempt_id),
                        None,
                    )?;
                    let updated = self
                        .projection
                        .wakeup
                        .get(&wakeup.address.key())
                        .cloned()
                        .ok_or_else(|| {
                            CommError::new("wakeup_not_found", "wakeup update was not projected")
                        })?;
                    changed.push(updated);
                }
                continue;
            }
            if notification.status == "unknown" || notification.delivery_attempt.is_some() {
                continue;
            }
            let adapter = match self.adapter_for(&message.adapter_id, Some(&agent.address())) {
                Ok(adapter) => adapter,
                Err(error) => {
                    let error = adapter_error(&error, &message.adapter_id, "wakeup.reminder");
                    notification.last_error = Some(adapter_error_record(
                        &error,
                        &message.adapter_id,
                        "wakeup.reminder",
                    ));
                    self.commit_wakeup_reminder(
                        &next_wakeup,
                        &message,
                        &notification_key,
                        &notification,
                        None,
                        None,
                    )?;
                    return Err(error);
                }
            };
            let attempt =
                new_delivery_attempt_at(&message.adapter_id, "notification.emitted", None, &at);
            let attempt_id = attempt.attempt_id.clone();
            let notification_id = notification.notification_id.clone();
            self.commit(
                "notification.delivery_attempt",
                json!({
                    "attemptId": attempt_id.clone(),
                    "keys": [notification_key.clone()],
                    "attempt": attempt
                }),
            )?;
            let receipt = match adapter.deliver(&message) {
                Ok(receipt) => receipt,
                Err(error) => {
                    let error = adapter_error(&error, &message.adapter_id, "wakeup.reminder");
                    if let Err(record_error) = self.record_notification_failure(
                        std::slice::from_ref(&notification_key),
                        &message.adapter_id,
                        &error,
                        "notification.emitted",
                        json!({
                            "messageId": message.message_id,
                            "notificationKey": notification_key.clone(),
                            "notificationId": notification_id,
                            "attemptId": attempt_id.clone()
                        }),
                    ) {
                        return Err(with_secondary_error(
                            error,
                            record_error,
                            "notification.delivery_failed",
                        ));
                    }
                    let notification = self
                        .projection
                        .notifications
                        .get(&notification_key)
                        .cloned()
                        .ok_or_else(|| {
                            CommError::new(
                                "notification_recovery_failed",
                                format!("notification disappeared during wakeup failure: {notification_key}"),
                            )
                        })?;
                    self.commit_wakeup_reminder(
                        &next_wakeup,
                        &message,
                        &notification_key,
                        &notification,
                        Some(&attempt_id),
                        None,
                    )?;
                    return Err(error);
                }
            };
            self.commit(
                "notification.emitted",
                json!({
                    "attemptId": attempt_id.clone(),
                    "keys": [notification_key.clone()],
                    "at": at.clone(),
                    "receipt": receipt.clone()
                }),
            )?;
            let notification = self
                .projection
                .notifications
                .get(&notification_key)
                .cloned()
                .ok_or_else(|| {
                    CommError::new(
                        "notification_recovery_failed",
                        format!("notification disappeared during wakeup: {notification_key}"),
                    )
                })?;
            self.commit_wakeup_reminder(
                &next_wakeup,
                &message,
                &notification_key,
                &notification,
                Some(&attempt_id),
                Some(&receipt),
            )?;
            let updated = self
                .projection
                .wakeup
                .get(&wakeup.address.key())
                .cloned()
                .ok_or_else(|| {
                    CommError::new("wakeup_not_found", "wakeup update was not projected")
                })?;
            changed.push(updated);
        }
        let wakeup = self.projection.wakeup.values().cloned().collect::<Vec<_>>();
        let master_wake = self
            .projection
            .master_wake
            .values()
            .cloned()
            .collect::<Vec<_>>();
        Ok(json!({
            "at": at,
            "wakeup": wakeup,
            "masterWake": master_wake,
            "changed": changed,
            "masterWakeChanged": master_wake_changed
        }))
    }

    pub fn flush_notifications(&mut self, at: Option<&str>) -> CommResult<Value> {
        let at = at.map(validate_time).transpose()?.unwrap_or_else(now);
        let now_time = parse_time(&at)?;
        let mut groups: BTreeMap<String, (Address, String, Vec<(String, NotificationRecord)>)> =
            BTreeMap::new();
        for (key, notification) in &self.projection.notifications {
            if notification.status != "pending" {
                continue;
            }
            let message = self
                .projection
                .messages
                .get(&notification.message_id)
                .ok_or_else(|| {
                    CommError::new(
                        "notification_message_missing",
                        format!(
                            "pending notification {} references missing message {}",
                            key, notification.message_id
                        ),
                    )
                })?;
            if matches!(message.delivery_mode, DeliveryMode::Direct)
                || message.priority.is_breakthrough()
                || notification.priority.is_breakthrough()
            {
                // Direct and P0 notifications have their own delivery retry
                // path.  Keeping them out of this idle batch is essential:
                // an adapter failure must never be silently demoted to a
                // lower urgency transport.
                continue;
            }
            if self.notification_held_for_master_wake(notification)? {
                continue;
            }
            if parse_time(&notification.available_at)? > now_time {
                continue;
            }
            groups
                .entry(structured_key(&[
                    &notification.recipient.key(),
                    &notification.adapter_id,
                ]))
                .or_insert_with(|| {
                    (
                        notification.recipient.clone(),
                        notification.adapter_id.clone(),
                        Vec::new(),
                    )
                })
                .2
                .push((key.clone(), notification.clone()));
        }
        let mut batches = Vec::new();
        for (_, (recipient, adapter_id, mut notifications)) in groups {
            notifications.sort_by(|left, right| {
                left.1
                    .priority
                    .cmp(&right.1.priority)
                    .then_with(|| left.1.created_at.cmp(&right.1.created_at))
                    .then_with(|| left.1.notification_id.cmp(&right.1.notification_id))
            });
            let batch = NotificationBatch {
                batch_id: new_id("batch"),
                recipient: recipient.clone(),
                created_at: at.clone(),
                adapter_id: adapter_id.clone(),
                items: notifications
                    .iter()
                    .map(|(_, notification)| notification.summary())
                    .collect(),
            };
            let keys: Vec<String> = notifications.iter().map(|(key, _)| key.clone()).collect();
            let adapter = match self.adapter_for(&adapter_id, Some(&recipient)) {
                Ok(adapter) => adapter,
                Err(error) => {
                    let error = adapter_error(&error, &adapter_id, "notification.batch_emitted");
                    if let Err(record_error) = self.record_notification_failure(
                        &keys,
                        &adapter_id,
                        &error,
                        "notification.batch_emitted",
                        json!({
                            "batchId": batch.batch_id,
                            "recipient": batch.recipient,
                            "notificationKeys": keys
                        }),
                    ) {
                        return Err(with_secondary_error(
                            error,
                            record_error,
                            "notification.delivery_failed",
                        ));
                    }
                    return Err(error);
                }
            };
            let attempt = new_delivery_attempt(
                &adapter_id,
                "notification.batch_emitted",
                Some(&batch.batch_id),
            );
            let attempt_id = attempt.attempt_id.clone();
            self.commit(
                "notification.delivery_attempt",
                json!({
                    "attemptId": attempt_id.clone(),
                    "keys": keys,
                    "attempt": attempt
                }),
            )?;
            let receipt = match adapter.emit_batch(&batch) {
                Ok(receipt) => receipt,
                Err(error) => {
                    let error = adapter_error(&error, &adapter_id, "notification.batch_emitted");
                    if let Err(record_error) = self.record_notification_failure(
                        &keys,
                        &adapter_id,
                        &error,
                        "notification.batch_emitted",
                        json!({
                            "batchId": batch.batch_id,
                            "recipient": batch.recipient,
                            "notificationKeys": keys,
                            "attemptId": attempt_id.clone()
                        }),
                    ) {
                        return Err(with_secondary_error(
                            error,
                            record_error,
                            "notification.delivery_failed",
                        ));
                    }
                    return Err(error);
                }
            };
            self.commit(
                "notification.batch_emitted",
                json!({
                    "batch": batch,
                    "notificationKeys": keys,
                    "at": at,
                    "attemptId": attempt_id.clone(),
                    "receipt": receipt
                }),
            )?;
            batches.push(batch);
        }
        Ok(json!({ "flushedAt": at, "batches": batches }))
    }

    pub(super) fn report_bug(&mut self, request: BugRequest) -> CommResult<Value> {
        validate_non_empty(&request.bug_id, "bugId")?;
        validate_non_empty(&request.scope_id, "scopeId")?;
        validate_non_empty(&request.title, "title")?;
        validate_non_empty(&request.description, "description")?;
        if request.reporter.scope_id != request.scope_id {
            return Err(CommError::new(
                "bug_reporter_scope_mismatch",
                "bug reporter must belong to the bug scope",
            ));
        }
        self.require_live_agent(&request.reporter)?;
        let priority = Priority::parse(&request.priority)?;
        let at = now();
        let loop_id = format!("bug-loop-{}", request.bug_id);
        let owner = self.scope_master_address(&request.scope_id)?;
        if let Some(existing) = self.projection.bugs.get(&request.bug_id).cloned() {
            if existing.scope_id != request.scope_id || existing.loop_id != loop_id {
                return Err(CommError::new(
                    "bug_loop_conflict",
                    format!(
                        "bug is bound outside deterministic loop: {}",
                        existing.loop_id
                    ),
                ));
            }
            if existing.title == request.title
                && existing.description == request.description
                && existing.priority == priority
                && existing.reporter == request.reporter
                && existing.worktree_id == request.worktree_id
            {
                return self.recover_idempotent_bug(existing);
            }
            return Err(CommError::new(
                "bug_conflict",
                format!("bug already exists: {}", request.bug_id),
            ));
        }
        if let Some(loop_record) = self.projection.loops.get(&loop_id) {
            validate_bug_loop_binding(loop_record, &request.bug_id, &request.scope_id, &owner)?;
        } else {
            let loop_record = LoopRecord {
                loop_id: loop_id.clone(),
                kind: "bug".into(),
                owner: owner.clone(),
                trigger: BUG_LOOP_TRIGGER.into(),
                work: BUG_LOOP_WORK.into(),
                gate: BUG_LOOP_GATE.into(),
                state: BUG_LOOP_STATE.into(),
                stop: BUG_LOOP_STOP.into(),
                max_iterations: 100,
                deadline_at: None,
                phase: "discover".into(),
                status: "active".into(),
                iteration: 0,
                created_at: at.clone(),
                updated_at: at.clone(),
                completion_evidence: None,
            };
            self.commit("loop.created", serde_json::to_value(&loop_record).unwrap())?;
        }
        let bug = BugRecord {
            bug_id: request.bug_id.clone(),
            scope_id: request.scope_id.clone(),
            title: request.title,
            priority: priority.clone(),
            description: request.description,
            reporter: request.reporter.clone(),
            status: "active".into(),
            worktree_id: request.worktree_id,
            loop_id,
            created_at: at.clone(),
            updated_at: at.clone(),
            resolution_evidence: None,
        };
        self.commit("bug.reported", serde_json::to_value(&bug).unwrap())?;
        let notification = self.system_notification(
            &owner,
            format!("bug reported: {}", bug.title),
            &priority,
            &bug.description,
            Some(&bug.bug_id),
            "bug",
            &at,
            None,
            "mailbox",
        )?;
        let signal = bug_wake_signal(&bug, priority.is_breakthrough());
        let master_wake = self.accumulate_master_wake(&owner, signal)?;
        Ok(json!({
            "bug": bug,
            "notification": notification,
            "masterWake": master_wake,
            "idempotent": false
        }))
    }

    pub(super) fn recover_idempotent_bug(&mut self, bug: BugRecord) -> CommResult<Value> {
        let owner = self.scope_master_address(&bug.scope_id)?;
        let loop_record = self.recover_bug_loop(&bug, &owner)?;
        let notification = self.recover_bug_notification(&bug, &owner)?;
        let master_wake = self.accumulate_master_wake(
            &owner,
            bug_wake_signal(&bug, bug.priority.is_breakthrough()),
        )?;
        Ok(json!({
            "bug": bug,
            "loop": loop_record,
            "notification": notification,
            "masterWake": master_wake,
            "idempotent": true
        }))
    }

    pub(super) fn recover_bug_loop(&mut self, bug: &BugRecord, owner: &Address) -> CommResult<LoopRecord> {
        if let Some(existing) = self.projection.loops.get(&bug.loop_id).cloned() {
            if !bug_loop_matches(&existing, owner) {
                return Err(CommError::new(
                    "bug_loop_conflict",
                    format!("bug loop is already used by another loop: {}", bug.loop_id),
                ));
            }
            return Ok(existing);
        }
        let at = now();
        let loop_record = LoopRecord {
            loop_id: bug.loop_id.clone(),
            kind: "bug".into(),
            owner: owner.clone(),
            trigger: BUG_LOOP_TRIGGER.into(),
            work: BUG_LOOP_WORK.into(),
            gate: BUG_LOOP_GATE.into(),
            state: BUG_LOOP_STATE.into(),
            stop: BUG_LOOP_STOP.into(),
            max_iterations: 100,
            deadline_at: None,
            phase: "discover".into(),
            status: "active".into(),
            iteration: 0,
            created_at: at.clone(),
            updated_at: at,
            completion_evidence: None,
        };
        self.commit("loop.created", serde_json::to_value(&loop_record).unwrap())?;
        Ok(loop_record)
    }

    pub(super) fn recover_bug_notification(&mut self, bug: &BugRecord, owner: &Address) -> CommResult<Value> {
        if let Some(notification) = self
            .projection
            .notifications
            .values()
            .find(|notification| {
                notification.issue_id.as_deref() == Some(bug.bug_id.as_str())
                    && notification.recipient == *owner
                    && notification.coalesce_key.as_deref() == Some("bug")
            })
            .cloned()
        {
            let message = self
                .projection
                .messages
                .get(&notification.message_id)
                .cloned();
            if let Some(message) = message.as_ref() {
                self.ensure_message_delivery_attempt(message)?;
            }
            return Ok(json!({
                "message": message,
                "notification": notification.summary()
            }));
        }

        if let Some(message) = self
            .projection
            .messages
            .values()
            .find(|message| {
                message.issue_id.as_deref() == Some(bug.bug_id.as_str())
                    && message.to == *owner
                    && message.coalesce_key.as_deref() == Some("bug")
            })
            .cloned()
        {
            let notification = self.recover_message_notification(&message)?;
            return Ok(json!({
                "message": message,
                "notification": notification.map(|value| value.summary())
            }));
        }

        self.system_notification(
            owner,
            format!("bug reported: {}", bug.title),
            &bug.priority,
            &bug.description,
            Some(&bug.bug_id),
            "bug",
            &bug.created_at,
            None,
            "mailbox",
        )
    }

    pub fn update_bug(
        &mut self,
        bug_id: &str,
        status: &str,
        actor: Address,
        evidence: Option<Value>,
    ) -> CommResult<Value> {
        validate_non_empty(bug_id, "bugId")?;
        let current =
            self.projection.bugs.get(bug_id).cloned().ok_or_else(|| {
                CommError::new("bug_not_found", format!("bug not found: {bug_id}"))
            })?;
        let actor_record = self.require_live_agent(&actor)?.clone();
        if actor.scope_id != current.scope_id {
            return Err(CommError::new(
                "bug_actor_scope_mismatch",
                "bug update actor must belong to the bug scope",
            ));
        }
        if !matches!(status, "active" | "resolved" | "closed") {
            return Err(CommError::new(
                "invalid_bug_status",
                format!("unsupported bug status: {status}"),
            ));
        }
        let is_scope_master = self
            .projection
            .scopes
            .get(&current.scope_id)
            .and_then(|scope| scope.master_session_id.as_deref())
            == Some(actor.session_id.as_str());
        if matches!(status, "resolved" | "closed")
            && (actor_record.role != "master" || !is_scope_master)
        {
            return Err(CommError::new(
                "bug_resolution_master_required",
                "only the scope master may resolve or close a bug",
            ));
        }
        let owner = self.scope_master_address(&current.scope_id)?;
        let expected_loop_id = format!("bug-loop-{}", current.bug_id);
        if current.loop_id != expected_loop_id {
            return Err(CommError::new(
                "bug_loop_conflict",
                format!("bug is bound to an unexpected loop: {}", current.loop_id),
            ));
        }
        let current_loop = self
            .projection
            .loops
            .get(&expected_loop_id)
            .ok_or_else(|| {
                CommError::new(
                    "bug_loop_conflict",
                    format!("deterministic bug loop is missing: {expected_loop_id}"),
                )
            })?;
        validate_bug_loop_binding(current_loop, &current.bug_id, &current.scope_id, &owner)?;
        let resolution_evidence = if matches!(status, "resolved" | "closed") {
            Some(validate_resolution_evidence(evidence.as_ref())?)
        } else {
            None
        };
        let mut updated = current.clone();
        updated.status = status.into();
        updated.updated_at = now();
        updated.resolution_evidence = resolution_evidence.clone();
        let mut updated_loop = Some(current_loop.clone());
        if matches!(status, "resolved" | "closed") {
            if let Some(loop_record) = updated_loop.as_mut() {
                loop_record.status = "completed".into();
                loop_record.phase = "completed".into();
                loop_record.updated_at = updated.updated_at.clone();
                loop_record.completion_evidence = resolution_evidence.clone();
            }
        } else if status == "active" {
            if let Some(loop_record) = updated_loop.as_mut() {
                loop_record.status = "active".into();
                loop_record.phase = "discover".into();
                loop_record.updated_at = updated.updated_at.clone();
                loop_record.completion_evidence = None;
            }
        }
        self.commit(
            "bug.updated",
            json!({ "bug": updated, "loop": updated_loop, "evidence": resolution_evidence }),
        )?;
        let notification = if matches!(status, "resolved" | "closed") {
            Some(self.system_notification(
                &current.reporter,
                format!("bug {}: {}", status, current.title),
                &current.priority,
                "Bug state changed; inspect the JSONL facts for the merge and verification evidence.",
                Some(&current.bug_id),
                "bug-resolution",
                &updated.updated_at,
                None,
                "mailbox",
            )?)
        } else if status == "active" && updated.priority.is_breakthrough() {
            Some(self.system_notification(
                &owner,
                format!("bug active: {}", updated.title),
                &updated.priority,
                &updated.description,
                Some(&updated.bug_id),
                "bug",
                &updated.updated_at,
                Some(&updated.updated_at),
                "mailbox",
            )?)
        } else {
            None
        };
        let master_wake = if updated.status == "active" {
            Some(self.accumulate_master_wake(
                &owner,
                bug_wake_signal(&updated, updated.priority.is_breakthrough()),
            )?)
        } else {
            self.clear_master_wake_signal(
                &owner,
                &bug_wake_signal_key(&updated.bug_id),
                &updated.updated_at,
            )?;
            self.projection.master_wake.get(&owner.key()).cloned()
        };
        Ok(json!({
            "bug": updated,
            "loop": updated_loop,
            "notification": notification,
            "masterWake": master_wake
        }))
    }

    pub(super) fn create_loop(&mut self, request: LoopRequest) -> CommResult<Value> {
        validate_loop_request(&request)?;
        if request.loop_id.starts_with("bug-loop-") {
            return Err(CommError::new(
                "bug_loop_reserved",
                "bug-loop identifiers are reserved for report_bug",
            ));
        }
        self.require_live_agent(&request.owner)?;
        if self.projection.loops.contains_key(&request.loop_id) {
            return Err(CommError::new(
                "loop_conflict",
                format!("loop already exists: {}", request.loop_id),
            ));
        }
        if let Some(deadline) = request.deadline_at.as_deref() {
            validate_time(deadline)?;
        }
        let at = now();
        let loop_record = LoopRecord {
            loop_id: request.loop_id.clone(),
            kind: request.kind,
            owner: request.owner,
            trigger: request.trigger,
            work: request.work,
            gate: request.gate,
            state: request.state,
            stop: request.stop,
            max_iterations: request.max_iterations,
            deadline_at: request.deadline_at,
            phase: "discover".into(),
            status: "active".into(),
            iteration: 0,
            created_at: at.clone(),
            updated_at: at,
            completion_evidence: None,
        };
        self.commit("loop.created", serde_json::to_value(&loop_record).unwrap())?;
        Ok(json!({ "loop": loop_record }))
    }

    pub fn advance_loop(
        &mut self,
        loop_id: &str,
        complete: bool,
        blocked: bool,
        actor: Option<Address>,
        evidence: Option<Value>,
        at: Option<&str>,
    ) -> CommResult<Value> {
        let at = at.map(validate_time).transpose()?.unwrap_or_else(now);
        let current = self.projection.loops.get(loop_id).cloned().ok_or_else(|| {
            CommError::new("loop_not_found", format!("loop not found: {loop_id}"))
        })?;
        if current.status != "active" {
            return Err(CommError::new(
                "loop_not_active",
                format!("loop is not active: {loop_id}"),
            ));
        }
        let actor = actor.unwrap_or_else(|| current.owner.clone());
        let actor_record = self.require_live_agent(&actor)?.clone();
        if actor.scope_id != current.owner.scope_id {
            return Err(CommError::new(
                "loop_actor_scope_mismatch",
                "loop actor must belong to the loop owner scope",
            ));
        }
        if actor != current.owner && actor_record.role != "master" {
            return Err(CommError::new(
                "loop_actor_forbidden",
                "only the loop owner or scope master may advance a loop",
            ));
        }
        let mut updated = current.clone();
        let deadline_reached = updated
            .deadline_at
            .as_deref()
            .map(|deadline| {
                parse_time(&at).and_then(|at| parse_time(deadline).map(|deadline| at >= deadline))
            })
            .transpose()?
            .unwrap_or(false);
        if deadline_reached {
            updated.status = "stopped".into();
            updated.phase = "deadline".into();
        } else if complete {
            updated.completion_evidence =
                Some(validate_loop_completion_evidence(evidence.as_ref())?);
            updated.status = "completed".into();
            updated.phase = "completed".into();
        } else if blocked {
            updated.status = "blocked".into();
            updated.phase = "blocked".into();
        } else {
            updated.phase = next_loop_phase(&mut updated)?;
        }
        updated.updated_at = at;
        self.commit("loop.updated", serde_json::to_value(&updated).unwrap())?;
        Ok(json!({ "loop": updated }))
    }

    pub fn record_error(
        &mut self,
        code: &str,
        message: &str,
        context: Value,
        loop_id: Option<&str>,
    ) -> CommResult<Value> {
        validate_non_empty(code, "code")?;
        validate_non_empty(message, "message")?;
        let error = ErrorRecord {
            code: code.into(),
            message: message.into(),
            context,
            at: now(),
        };
        let updated_loop = if let Some(loop_id) = loop_id {
            let current = self.projection.loops.get(loop_id).cloned().ok_or_else(|| {
                CommError::new("loop_not_found", format!("loop not found: {loop_id}"))
            })?;
            if current.status != "active" {
                return Err(CommError::new(
                    "loop_not_active",
                    format!("loop is not active: {loop_id}"),
                ));
            }
            let mut updated = current;
            updated.status = "blocked".into();
            updated.phase = "blocked".into();
            updated.updated_at = error.at.clone();
            Some(updated)
        } else {
            None
        };
        let event_id = self.commit(
            "error.recorded",
            json!({ "error": error, "loop": updated_loop }),
        )?;
        let master_wake = if let Some(loop_record) = updated_loop.as_ref() {
            self.scope_master_address_unchecked(&loop_record.owner.scope_id)
                .map(|master| {
                    self.accumulate_master_wake(
                        &master,
                        loop_error_wake_signal(loop_record, &error),
                    )
                })
                .transpose()?
        } else {
            None
        };
        Ok(json!({
            "error": error,
            "loop": updated_loop,
            "masterWake": master_wake,
            "eventId": event_id
        }))
    }
}
