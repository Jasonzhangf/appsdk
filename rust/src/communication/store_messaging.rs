use super::*;
use super::helpers::*;
use super::validation::*;

impl CommunicationStore {
    pub(super) fn enqueue_message(
        &mut self,
        request: MessageRequest,
        route: RouteRecord,
        available_at_override: Option<&str>,
    ) -> CommResult<Value> {
        validate_message_request(&request)?;
        let priority = Priority::parse(&request.priority)?;
        let delivery_mode = DeliveryMode::parse(request.delivery_mode.as_deref())?;
        let created_at = request
            .created_at
            .as_deref()
            .map(validate_time)
            .transpose()?
            .unwrap_or_else(now);
        let message_id = request.message_id.clone().unwrap_or_else(|| new_id("msg"));
        let adapter_id = request
            .adapter_id
            .clone()
            .unwrap_or_else(|| "mailbox".into());
        if let Some(existing) = self.projection.messages.get(&message_id).cloned() {
            if message_matches_request(&existing, &request, &priority, &delivery_mode, &adapter_id)
            {
                return self.recover_idempotent_message(existing);
            }
            return Err(CommError::new(
                "message_id_conflict",
                format!("messageId already identifies a different message: {message_id}"),
            ));
        }
        self.adapter_for(&adapter_id, Some(&request.to))?;
        let message = MessageRecord {
            protocol: PROTOCOL.into(),
            message_id: message_id.clone(),
            conversation_id: request.conversation_id.unwrap_or_else(|| new_id("conv")),
            from: request.from,
            to: request.to,
            title: request.title,
            priority: priority.clone(),
            body: request.body,
            delivery_mode: delivery_mode.clone(),
            coalesce_key: request.coalesce_key,
            issue_id: request.issue_id,
            adapter_id,
            delivery_attempt_required: true,
            created_at: created_at.clone(),
            state: "created".into(),
            evidence: Vec::new(),
            route,
            last_error: None,
        };
        self.commit("message.created", serde_json::to_value(&message).unwrap())?;
        let accepted = DeliveryEvidence {
            state: "accepted".into(),
            at: created_at.clone(),
            details: json!({ "transport": "appsdk-internal", "durable": true }),
        };
        self.commit(
            "message.state",
            json!({
                "messageId": message_id,
                "state": "accepted",
                "evidence": accepted
            }),
        )?;
        let current = self
            .projection
            .messages
            .get(&message.message_id)
            .cloned()
            .unwrap();
        let delivery_attempt = self.ensure_message_delivery_attempt(&current)?;
        let notification = self.notification_for(&current, &created_at, available_at_override)?;
        let direct = notification
            .as_ref()
            .filter(|notification| notification.status == "emitted")
            .map(NotificationRecord::summary);
        Ok(json!({
            "message": current,
            "route": current.route,
            "deliveryAttempt": delivery_attempt,
            "notification": direct,
            "idempotent": false
        }))
    }

    pub(super) fn recover_idempotent_message(&mut self, existing: MessageRecord) -> CommResult<Value> {
        let current = self.recover_message_state(existing)?;
        let delivery_attempt = self.ensure_message_delivery_attempt(&current)?;
        let notification = self.recover_message_notification(&current)?;
        let direct = notification
            .as_ref()
            .filter(|notification| notification.status == "emitted")
            .map(NotificationRecord::summary);
        Ok(json!({
            "message": current,
            "route": current.route,
            "deliveryAttempt": delivery_attempt,
            "notification": direct,
            "idempotent": true
        }))
    }

    pub(super) fn recover_message_state(&mut self, existing: MessageRecord) -> CommResult<MessageRecord> {
        if existing.state == "created"
            && !existing
                .evidence
                .iter()
                .any(|evidence| evidence.state == "accepted")
        {
            let accepted = DeliveryEvidence {
                state: "accepted".into(),
                at: existing.created_at.clone(),
                details: json!({ "transport": "appsdk-internal", "durable": true, "recovered": true }),
            };
            self.commit(
                "message.state",
                json!({
                    "messageId": existing.message_id,
                    "state": "accepted",
                    "evidence": accepted
                }),
            )?;
        }
        self.projection
            .messages
            .get(&existing.message_id)
            .cloned()
            .ok_or_else(|| {
                CommError::new(
                    "message_recovery_failed",
                    format!(
                        "message disappeared during recovery: {}",
                        existing.message_id
                    ),
                )
            })
    }

    pub(super) fn recover_message_notification(
        &mut self,
        message: &MessageRecord,
    ) -> CommResult<Option<NotificationRecord>> {
        self.ensure_message_delivery_attempt(message)?;
        let immediate = matches!(message.delivery_mode, DeliveryMode::Direct)
            || message.priority.is_breakthrough();
        let existing = if immediate {
            self.projection
                .notifications
                .iter()
                .find(|(_, notification)| notification.message_id == message.message_id)
                .map(|(key, notification)| (key.clone(), notification.clone()))
        } else {
            let key = self.message_notification_key(message);
            self.projection
                .notifications
                .get(&key)
                .cloned()
                .map(|notification| (key, notification))
        };
        let Some((key, notification)) = existing else {
            return self.notification_for(message, &message.created_at, None);
        };

        if !immediate {
            let current_ordinal = self
                .projection
                .message_ordinals
                .get(&notification.message_id)
                .ok_or_else(|| {
                    CommError::new(
                        "journal_corrupt",
                        format!(
                            "notification {key} references message without a durable creation fact: {}",
                            notification.message_id
                        ),
                    )
                })?;
            let requested_ordinal = self
                .projection
                .message_ordinals
                .get(&message.message_id)
                .ok_or_else(|| {
                    CommError::new(
                        "journal_corrupt",
                        format!(
                            "message {} has no durable creation fact for notification recovery",
                            message.message_id
                        ),
                    )
                })?;
            if notification.message_id == message.message_id {
                return Ok(Some(notification));
            }
            // The current projection is the latest state of the coalescing
            // bucket.  Compare replay-established message creation order:
            // unlike timestamps, it distinguishes a same-time new message
            // from a retry of an older message.  Missing order is corruption;
            // generation/time are not allowed to reconstruct this control
            // fact and silently swallow a newer message prefix.
            if current_ordinal > requested_ordinal {
                return Ok(Some(notification));
            }
            return self.notification_for(message, &message.created_at, None);
        }

        if matches!(notification.status.as_str(), "emitted" | "unknown") {
            return Ok(Some(notification));
        }

        self.retry_immediate_notification(message, &key, notification)
            .map(Some)
    }

    pub(super) fn notification_for(
        &mut self,
        message: &MessageRecord,
        created_at: &str,
        available_at_override: Option<&str>,
    ) -> CommResult<Option<NotificationRecord>> {
        let immediate = matches!(message.delivery_mode, DeliveryMode::Direct)
            || message.priority.is_breakthrough();
        let adapter = if immediate {
            Some(self.adapter_for(&message.adapter_id, Some(&message.to))?)
        } else {
            None
        };
        let mut notification =
            self.build_notification(message, created_at, available_at_override)?;
        let existing_immediate = if immediate {
            self.projection
                .notifications
                .iter()
                .find(|(_, existing)| existing.message_id == message.message_id)
                .map(|(key, existing)| (key.clone(), existing.clone()))
        } else {
            None
        };
        let key = existing_immediate
            .as_ref()
            .map(|(key, _)| key.clone())
            .unwrap_or_else(|| self.notification_key(message, &notification, !immediate));
        let mut reused_pending = false;
        if let Some(existing) = self.projection.notifications.get(&key) {
            if matches!(
                existing.status.as_str(),
                "emitted" | "unknown" | "superseded"
            ) {
                if existing.message_id == message.message_id || immediate {
                    return Ok(Some(existing.clone()));
                }
                notification.generation = existing.generation.checked_add(1).ok_or_else(|| {
                    CommError::new(
                        "notification_generation_exhausted",
                        format!("notification generation exhausted: {key}"),
                    )
                })?;
            }
            if existing.status == "pending" {
                if immediate {
                    notification = existing.clone();
                    reused_pending = true;
                } else {
                    notification.available_at = existing.available_at.clone();
                    notification.generation = existing.generation;
                }
            }
        }
        let attempt = immediate
            .then(|| new_delivery_attempt(&message.adapter_id, "notification.emitted", None));
        let attempt_id = attempt.as_ref().map(|attempt| attempt.attempt_id.clone());
        let notification_id = notification.notification_id.clone();
        if !reused_pending {
            self.commit(
                "notification.queued",
                json!({ "key": key, "notification": notification }),
            )?;
        }
        if let Some(attempt) = attempt.as_ref() {
            self.commit(
                "notification.delivery_attempt",
                json!({
                    "attemptId": attempt_id.clone(),
                    "keys": [key],
                    "attempt": attempt
                }),
            )?;
        }
        if immediate {
            let adapter = adapter.expect("immediate notification adapter is initialized");
            let receipt = match adapter.deliver(message) {
                Ok(receipt) => receipt,
                Err(error) => {
                    let error = adapter_error(&error, &message.adapter_id, "notification.emitted");
                    if let Err(record_error) = self.record_notification_failure(
                        std::slice::from_ref(&key),
                        &message.adapter_id,
                        &error,
                        "notification.emitted",
                        json!({
                            "messageId": message.message_id,
                            "notificationKey": key,
                            "notificationId": notification_id,
                            "attemptId": attempt_id
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
                "notification.emitted",
                json!({
                    "attemptId": attempt_id,
                    "keys": [key],
                    "at": created_at,
                    "receipt": receipt
                }),
            )?;
        }
        Ok(self.projection.notifications.get(&key).cloned())
    }

    pub(super) fn retry_immediate_notification(
        &mut self,
        message: &MessageRecord,
        key: &str,
        existing: NotificationRecord,
    ) -> CommResult<NotificationRecord> {
        let adapter = match self.adapter_for(&message.adapter_id, Some(&message.to)) {
            Ok(adapter) => adapter,
            Err(error) => {
                let error = adapter_error(&error, &message.adapter_id, "notification.emitted");
                if let Err(record_error) = self.record_notification_failure(
                    std::slice::from_ref(&key.to_string()),
                    &message.adapter_id,
                    &error,
                    "notification.emitted",
                    json!({
                        "messageId": message.message_id,
                        "notificationKey": key,
                        "notificationId": existing.notification_id
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
        let attempt = new_delivery_attempt(&message.adapter_id, "notification.emitted", None);
        let attempt_id = attempt.attempt_id.clone();
        let attempt_started_at = attempt.started_at.clone();
        let notification_id = existing.notification_id.clone();
        self.commit(
            "notification.queued",
            json!({ "key": key, "notification": existing }),
        )?;
        self.commit(
            "notification.delivery_attempt",
            json!({
                "attemptId": attempt_id.clone(),
                "keys": [key],
                "attempt": attempt
            }),
        )?;
        let receipt = match adapter.deliver(message) {
            Ok(receipt) => receipt,
            Err(error) => {
                let error = adapter_error(&error, &message.adapter_id, "notification.emitted");
                if let Err(record_error) = self.record_notification_failure(
                    std::slice::from_ref(&key.to_string()),
                    &message.adapter_id,
                    &error,
                    "notification.emitted",
                    json!({
                        "messageId": message.message_id,
                        "notificationKey": key,
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
                return Err(error);
            }
        };
        self.commit(
            "notification.emitted",
            json!({
                "attemptId": attempt_id,
                "keys": [key],
                "at": attempt_started_at,
                "receipt": receipt
            }),
        )?;
        self.projection
            .notifications
            .get(key)
            .cloned()
            .ok_or_else(|| {
                CommError::new(
                    "notification_recovery_failed",
                    format!("notification disappeared during recovery: {key}"),
                )
            })
    }

    pub(super) fn build_notification(
        &self,
        message: &MessageRecord,
        created_at: &str,
        available_at_override: Option<&str>,
    ) -> CommResult<NotificationRecord> {
        let immediate = matches!(message.delivery_mode, DeliveryMode::Direct)
            || message.priority.is_breakthrough();
        let available_at = if let Some(value) = available_at_override {
            validate_time(value)?
        } else if immediate {
            created_at.to_string()
        } else {
            add_seconds(created_at, DEFAULT_BATCH_WINDOW_SECONDS)?
        };
        Ok(NotificationRecord {
            notification_id: new_id("notification"),
            message_id: message.message_id.clone(),
            generation: 0,
            recipient: message.to.clone(),
            title: message.title.clone(),
            priority: message.priority.clone(),
            issue_id: message.issue_id.clone(),
            coalesce_key: message.coalesce_key.clone(),
            body: message.body.clone(),
            created_at: created_at.to_string(),
            available_at,
            status: "pending".into(),
            emitted_at: None,
            adapter_id: message.adapter_id.clone(),
            transport_receipt: None,
            last_error: None,
            delivery_attempt: None,
        })
    }

    pub(super) fn notification_key(
        &self,
        message: &MessageRecord,
        notification: &NotificationRecord,
        force_coalesce: bool,
    ) -> String {
        if !force_coalesce {
            return notification.notification_id.clone();
        }
        self.message_notification_key(message)
    }

    pub(super) fn message_notification_key(&self, message: &MessageRecord) -> String {
        message_notification_key_for(message)
    }

    pub(super) fn wakeup_delivery_in_flight(&self, wakeup: &WakeupRecord) -> CommResult<bool> {
        let Some(idle_since) = wakeup.idle_since.as_deref() else {
            return Ok(false);
        };
        let idle_since = parse_time(idle_since)?;
        let daemon = Address {
            scope_id: "appsdk".into(),
            session_id: "daemon".into(),
        };
        let key = structured_key(&[
            &daemon.key(),
            &wakeup.address.key(),
            "mailbox",
            "master-idle",
        ]);
        let Some(notification) = self.projection.notifications.get(&key) else {
            return Ok(false);
        };
        if notification.status != "unknown" && notification.delivery_attempt.is_none() {
            return Ok(false);
        }
        Ok(parse_time(&notification.created_at)? >= idle_since)
    }

    pub(super) fn prepare_wakeup_message(&mut self, expected: MessageRecord) -> CommResult<MessageRecord> {
        if let Some(existing) = self.projection.messages.get(&expected.message_id).cloned() {
            if !wakeup_message_matches(&existing, &expected) {
                return Err(CommError::new(
                    "wakeup_message_conflict",
                    format!(
                        "wakeup message id already identifies a different message: {}",
                        expected.message_id
                    ),
                ));
            }
            if existing.state == "created" {
                let accepted = DeliveryEvidence {
                    state: "accepted".into(),
                    at: existing.created_at.clone(),
                    details: json!({
                        "transport": "appsdk-internal",
                        "durable": true,
                        "recovered": true
                    }),
                };
                self.commit(
                    "message.state",
                    json!({
                        "messageId": existing.message_id,
                        "state": "accepted",
                        "evidence": accepted
                    }),
                )?;
            } else if delivery_state_rank(&existing.state).is_none() {
                return Err(CommError::new(
                    "wakeup_message_state_invalid",
                    format!(
                        "wakeup message {} has unsupported state: {}",
                        existing.message_id, existing.state
                    ),
                ));
            }
            let current = self
                .projection
                .messages
                .get(&expected.message_id)
                .cloned()
                .ok_or_else(|| {
                    CommError::new(
                        "message_recovery_failed",
                        format!(
                            "wakeup message disappeared during recovery: {}",
                            expected.message_id
                        ),
                    )
                })?;
            self.ensure_message_delivery_attempt(&current)?;
            return Ok(current);
        }

        let mut created = expected.clone();
        created.state = "created".into();
        created.evidence.clear();
        self.commit("message.created", serde_json::to_value(&created).unwrap())?;
        let accepted = expected.evidence.first().cloned().ok_or_else(|| {
            CommError::new(
                "message_evidence_missing",
                "system message accepted evidence missing",
            )
        })?;
        self.commit(
            "message.state",
            json!({
                "messageId": expected.message_id,
                "state": "accepted",
                "evidence": accepted
            }),
        )?;
        let current = self
            .projection
            .messages
            .get(&created.message_id)
            .cloned()
            .ok_or_else(|| {
                CommError::new(
                    "message_recovery_failed",
                    format!(
                        "wakeup message disappeared during creation: {}",
                        created.message_id
                    ),
                )
            })?;
        self.ensure_message_delivery_attempt(&current)?;
        Ok(self
            .projection
            .messages
            .get(&created.message_id)
            .cloned()
            .expect("wakeup message remains after delivery attempt"))
    }

    pub(super) fn commit_wakeup_reminder(
        &mut self,
        wakeup: &WakeupRecord,
        message: &MessageRecord,
        notification_key: &str,
        notification: &NotificationRecord,
        attempt_id: Option<&str>,
        receipt: Option<&TransportReceipt>,
    ) -> CommResult<()> {
        self.commit(
            "wakeup.reminder",
            json!({
                "wakeup": wakeup,
                "message": message,
                "notificationKey": notification_key,
                "notification": notification,
                "attemptId": attempt_id,
                "receipt": receipt
            }),
        )
        .map(|_| ())
    }

    pub(super) fn system_message(
        &self,
        target: &Address,
        title: String,
        priority: &str,
        body: &str,
        coalesce_key: &str,
        at: &str,
        adapter_id: &str,
    ) -> CommResult<MessageRecord> {
        Ok(MessageRecord {
            protocol: PROTOCOL.into(),
            message_id: new_id("msg"),
            conversation_id: new_id("conv"),
            from: Address {
                scope_id: "appsdk".into(),
                session_id: "daemon".into(),
            },
            to: target.clone(),
            title,
            priority: Priority::parse(priority)?,
            body: body.into(),
            delivery_mode: DeliveryMode::Idle,
            coalesce_key: Some(coalesce_key.into()),
            issue_id: None,
            adapter_id: adapter_id.into(),
            delivery_attempt_required: true,
            created_at: at.into(),
            state: "accepted".into(),
            evidence: vec![DeliveryEvidence {
                state: "accepted".into(),
                at: at.into(),
                details: json!({ "transport": "appsdk-internal", "source": "daemon" }),
            }],
            route: RouteRecord {
                mode: "system".into(),
                same_appserver: false,
                same_project: false,
                source_role: "daemon".into(),
                target_role: "master".into(),
            },
            last_error: None,
        })
    }

    pub(super) fn system_notification(
        &mut self,
        target: &Address,
        title: String,
        priority: &Priority,
        body: &str,
        issue_id: Option<&str>,
        coalesce_key: &str,
        at: &str,
        available_at_override: Option<&str>,
        adapter_id: &str,
    ) -> CommResult<Value> {
        let mut message = self.system_message(
            target,
            title,
            &format_priority(priority),
            body,
            coalesce_key,
            at,
            adapter_id,
        )?;
        message.issue_id = issue_id.map(str::to_string);
        self.commit("message.created", serde_json::to_value(&message).unwrap())?;
        self.ensure_message_delivery_attempt(&message)?;
        let notification = self.notification_for(&message, at, available_at_override)?;
        Ok(json!({
            "message": message,
            "notification": notification.map(|value| value.summary())
        }))
    }

    pub(super) fn require_adapter(&self, adapter_id: &str) -> CommResult<&AdapterRecord> {
        self.projection.adapters.get(adapter_id).ok_or_else(|| {
            CommError::new(
                "adapter_not_registered",
                format!("communication adapter not registered: {adapter_id}"),
            )
        })
    }

    pub(super) fn adapter_for(
        &self,
        adapter_id: &str,
        recipient: Option<&Address>,
    ) -> CommResult<Box<dyn CommunicationAdapter>> {
        let record = self.require_adapter(adapter_id)?;
        if !record.enabled {
            return Err(CommError::new(
                "adapter_disabled",
                format!("communication adapter is disabled: {adapter_id}"),
            ));
        }
        if let Some(bound_recipient) = record.recipient.as_ref() {
            if recipient != Some(bound_recipient) {
                return Err(CommError::new(
                    "adapter_recipient_mismatch",
                    format!(
                        "adapter {adapter_id} is bound to {}, not {}",
                        bound_recipient.key(),
                        recipient
                            .map(Address::key)
                            .unwrap_or_else(|| "<none>".into())
                    ),
                ));
            }
        }
        let runtime = if record.kind == "mailbox" {
            None
        } else {
            let bound_recipient = record.recipient.as_ref().ok_or_else(|| {
                CommError::new(
                    "adapter_recipient_required",
                    format!("adapter {adapter_id} has no registered recipient address"),
                )
            })?;
            let agent = self.require_live_agent(bound_recipient)?.clone();
            let runtime = self.runtime_for_agent(&agent)?;
            let target = record
                .target
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    CommError::new(
                        "appserver_target_required",
                        "appserver adapter requires endpoint",
                    )
                })?;
            validate_adapter_runtime_target(&record.kind, target, &runtime, true)?;
            Some(runtime)
        };
        match record.kind.as_str() {
            "mailbox" => Ok(Box::new(MailboxAdapter {
                adapter_id: record.adapter_id.clone(),
                path: self.mailbox_path.clone(),
            })),
            "appserver" => {
                let endpoint = record
                    .target
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        CommError::new(
                            "appserver_target_required",
                            "appserver adapter requires endpoint",
                        )
                    })?;
                Ok(Box::new(AppserverAdapter {
                    adapter_id: record.adapter_id.clone(),
                    runtime_id: runtime
                        .as_ref()
                        .expect("appserver adapter runtime was validated")
                        .identity
                        .runtime_id
                        .clone(),
                    endpoint,
                    capability: APPSERVER_SEND_CAPABILITY.into(),
                }))
            }
            other => Err(CommError::new(
                "invalid_adapter_kind",
                format!("adapter kind is unsupported: {other}"),
            )),
        }
    }

    pub(super) fn record_notification_failure(
        &mut self,
        keys: &[String],
        adapter_id: &str,
        error: &CommError,
        operation: &str,
        identity: Value,
    ) -> CommResult<()> {
        let record = adapter_error_record(error, adapter_id, operation);
        let mut data = json!({ "keys": keys, "adapterId": adapter_id, "operation": operation, "error": record });
        if let (Some(data), Some(identity)) = (data.as_object_mut(), identity.as_object()) {
            for (key, value) in identity {
                data.insert(key.clone(), value.clone());
            }
        }
        self.commit("notification.delivery_failed", data)
            .map(|_| ())
    }

    pub(super) fn require_scope(&self, scope_id: &str) -> CommResult<&ScopeRecord> {
        self.projection.scopes.get(scope_id).ok_or_else(|| {
            CommError::new("scope_not_found", format!("scope not found: {scope_id}"))
        })
    }

    pub(super) fn require_agent(&self, address: &Address) -> CommResult<&AgentRecord> {
        validate_address(address)?;
        if let Some(agent) = self.projection.agents.get(&address.key()) {
            return Ok(agent);
        }
        if let Some(tombstone) = self.projection.agent_tombstones.get(&address.key()) {
            let mut error = CommError::new(
                "agent_address_rebound",
                format!(
                    "agent address was rebound to {}",
                    tombstone.rebound_to.key()
                ),
            );
            error.context = json!({
                "oldAddress": tombstone.address,
                "newAddress": tombstone.rebound_to,
                "agentId": tombstone.agent_id,
                "runtimeId": tombstone.runtime_id,
                "reboundAt": tombstone.rebound_at
            });
            return Err(error);
        }
        Err(CommError::new(
            "agent_not_registered",
            format!("agent not registered: {}", address.key()),
        ))
    }

    pub(super) fn require_live_agent(&self, address: &Address) -> CommResult<&AgentRecord> {
        let at = now();
        self.require_live_agent_at(address, &at)
    }

    pub(super) fn require_live_agent_at(&self, address: &Address, at: &str) -> CommResult<&AgentRecord> {
        let agent = self.require_agent(address)?;
        if !agent.live_at(at) {
            return Err(CommError::new(
                "agent_lease_expired",
                format!("agent lease expired: {}", address.key()),
            ));
        }
        Ok(agent)
    }

    pub(super) fn live_idle_master_at(&self, address: &Address, at: &str) -> Option<AgentRecord> {
        let agent = self.require_live_agent_at(address, at).ok()?;
        if agent.role != "master" || agent.state != AgentState::Idle {
            return None;
        }
        let scope = self.projection.scopes.get(&agent.scope_id)?;
        if scope.master_session_id.as_deref() != Some(agent.session_id.as_str()) {
            return None;
        }
        Some(agent.clone())
    }

    pub(super) fn scope_master_address(&self, scope_id: &str) -> CommResult<Address> {
        let scope = self.require_scope(scope_id)?;
        let session_id = scope.master_session_id.clone().ok_or_else(|| {
            CommError::new(
                "master_not_registered",
                format!("scope has no master: {scope_id}"),
            )
        })?;
        let address = Address {
            scope_id: scope_id.into(),
            session_id,
        };
        self.require_live_agent(&address)?;
        Ok(address)
    }

    pub(super) fn resolve_route(&self, source: &AgentRecord, target: &AgentRecord) -> CommResult<RouteRecord> {
        let source_scope = self.require_scope(&source.scope_id)?;
        let target_scope = self.resolve_scope_for_agent(target)?;
        let same_scope = source.scope_id == target.scope_id;
        let same_appserver = source_scope.appserver_id == target_scope.appserver_id;
        let same_project = source_scope.project_root == target_scope.project_root;
        if !same_scope {
            if source.role != "master"
                || source_scope.master_session_id.as_deref() != Some(source.session_id.as_str())
            {
                return Err(CommError::new(
                    "cross_scope_master_required",
                    "cross-scope communication requires the source scope master",
                ));
            }
            if target.role != "master"
                || target_scope.master_session_id.as_deref() != Some(target.session_id.as_str())
            {
                return Err(CommError::new(
                    "cross_scope_target_must_be_master",
                    "cross-scope target must be the target scope master session",
                ));
            }
            return Ok(RouteRecord {
                mode: "cross-scope-master".into(),
                same_appserver,
                same_project,
                source_role: source.role.clone(),
                target_role: target.role.clone(),
            });
        }
        if source.role == "subagent" && target.role == "subagent" {
            return Err(CommError::new(
                "subagent_to_subagent_forbidden",
                "subagents cannot communicate with another subagent",
            ));
        }
        if source.role == "subagent" && !self.parent_or_master_allows(source, target) {
            return Err(CommError::new(
                "subagent_parent_required",
                "subagent may communicate only with its parent or master ancestor",
            ));
        }
        if target.role == "subagent" && !self.parent_or_master_allows(target, source) {
            return Err(CommError::new(
                "subagent_parent_required",
                "peer may communicate only with its bound subagent",
            ));
        }
        if source.role == "peer" && target.role == "peer" && !(same_appserver && same_project) {
            return Err(CommError::new(
                "peer_scope_forbidden",
                "peers may communicate only inside the same App Server and project",
            ));
        }
        let mode = if source.role == "master" || target.role == "master" {
            "same-scope-master"
        } else if source.role == "subagent" || target.role == "subagent" {
            "same-scope-parent"
        } else {
            "same-scope-peer"
        };
        Ok(RouteRecord {
            mode: mode.into(),
            same_appserver,
            same_project,
            source_role: source.role.clone(),
            target_role: target.role.clone(),
        })
    }

    pub(super) fn parent_or_master_allows(&self, child: &AgentRecord, other: &AgentRecord) -> bool {
        if child
            .parent
            .as_ref()
            .is_some_and(|parent| self.addresses_match_after_rebind(parent, &other.address()))
        {
            return true;
        }
        if other.role != "master" {
            return false;
        }
        let mut current = child.parent.clone();
        while let Some(parent) = current {
            if self.addresses_match_after_rebind(&parent, &other.address()) {
                return true;
            }
            current = self
                .canonical_address(&parent)
                .and_then(|address| self.projection.agents.get(&address.key()))
                .and_then(|agent| agent.parent.clone());
        }
        self.projection
            .scopes
            .get(&child.scope_id)
            .and_then(|scope| scope.master_session_id.as_ref())
            .is_some_and(|master| master == &other.session_id)
    }

    pub(super) fn canonical_address(&self, address: &Address) -> Option<Address> {
        let mut current = address.clone();
        let mut visited = BTreeMap::new();
        while let Some(tombstone) = self.projection.agent_tombstones.get(&current.key()) {
            if visited.insert(current.key(), true).is_some() {
                return None;
            }
            current = tombstone.rebound_to.clone();
        }
        Some(current)
    }

    pub(super) fn addresses_match_after_rebind(&self, left: &Address, right: &Address) -> bool {
        self.canonical_address(left)
            .zip(self.canonical_address(right))
            .is_some_and(|(left, right)| left == right)
    }
}
