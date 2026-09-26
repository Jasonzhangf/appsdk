use super::*;
use super::helpers::*;
use super::validation::*;

impl CommunicationStore {
    pub(super) fn apply_event(&mut self, event: &EventRecord) -> CommResult<()> {
        self.projection.event_ordinal =
            self.projection
                .event_ordinal
                .checked_add(1)
                .ok_or_else(|| {
                    CommError::new("journal_corrupt", "communication event ordinal exhausted")
                })?;
        let event_ordinal = self.projection.event_ordinal;
        match event.kind.as_str() {
            "scope.registered" => {
                let record: ScopeRecord = decode(&event.data, "scope")?;
                self.validate_project_root(&record.project_root)?;
                self.projection
                    .scopes
                    .insert(record.scope_id.clone(), record);
            }
            "scope.unregistered" => {
                let scope_id = event
                    .data
                    .get("scopeId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("event_data_invalid", "scopeId missing"))?;
                self.projection.scopes.remove(scope_id);
            }
            "adapter.registered" => {
                let record: AdapterRecord = decode(&event.data, "adapter")?;
                self.projection
                    .adapters
                    .insert(record.adapter_id.clone(), record);
            }
            "agent.registered" => {
                let record: AgentRecord = decode(&event.data, "agent")?;
                let key = record.address().key();
                if self.projection.agent_tombstones.contains_key(&key) {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "agent registration attempts to reuse a rebound address",
                    ));
                }
                if record.role == "master" {
                    if let Some(scope) = self.projection.scopes.get_mut(&record.scope_id) {
                        scope.master_session_id = Some(record.session_id.clone());
                    }
                }
                self.projection.agents.insert(key, record);
            }
            "agent.state" => {
                let address: Address =
                    decode(event.data.get("address").unwrap_or(&Value::Null), "address")?;
                let state: AgentState =
                    decode(event.data.get("state").unwrap_or(&Value::Null), "state")?;
                let at = event
                    .data
                    .get("at")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "agent state at missing")
                    })?;
                let agent = self
                    .projection
                    .agents
                    .get_mut(&address.key())
                    .ok_or_else(|| CommError::new("event_data_invalid", "agent not found"))?;
                agent.state = state;
                agent.last_state_at = at.into();
            }
            "agent.refreshed" => {
                let record: AgentRecord = decode(&event.data, "agent")?;
                if self
                    .projection
                    .agent_tombstones
                    .contains_key(&record.address().key())
                {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "agent refresh attempts to update a rebound address",
                    ));
                }
                self.projection
                    .agents
                    .insert(record.address().key(), record);
            }
            "agent.rebound" => self.apply_agent_rebound_event(&event.data)?,
            "discovery.pending" => {
                let pending: DiscoveryPendingRecord = decode(&event.data, "discovery pending")?;
                if self
                    .projection
                    .discovery_pending
                    .insert(pending.pending_id.clone(), pending)
                    .is_some()
                {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "discovery pending operation is duplicated",
                    ));
                }
            }
            "discovery.reconciled" => {
                let pending_id = event
                    .data
                    .get("pendingId")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            "discovery reconciled pendingId missing",
                        )
                    })?;
                if self
                    .projection
                    .discovery_pending
                    .remove(pending_id)
                    .is_none()
                {
                    return Err(CommError::new(
                        "event_data_invalid",
                        format!("discovery pending operation is missing: {pending_id}"),
                    ));
                }
            }
            "message.created" => {
                let record: MessageRecord = decode(&event.data, "message")?;
                self.projection
                    .message_ordinals
                    .entry(record.message_id.clone())
                    .or_insert(event_ordinal);
                self.projection
                    .messages
                    .insert(record.message_id.clone(), record);
            }
            "message.delivery_attempt" => {
                let message_id = event
                    .data
                    .get("messageId")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            "message delivery attempt messageId missing",
                        )
                    })?;
                let attempt: MessageDeliveryAttempt = decode(
                    event.data.get("attempt").unwrap_or(&Value::Null),
                    "message delivery attempt",
                )?;
                if attempt.message_id != message_id {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "message delivery attempt messageId does not match attempt record",
                    ));
                }
                let message = self
                    .projection
                    .messages
                    .get(message_id)
                    .cloned()
                    .ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            format!("message delivery attempt message not found: {message_id}"),
                        )
                    })?;
                let target = self.resolve_agent(&message.to).map_err(|error| {
                    CommError::new(
                        "event_data_invalid",
                        format!(
                            "message delivery attempt target agent is missing: {}",
                            error.message
                        ),
                    )
                })?;
                let runtime_id = target.runtime_id.as_deref().ok_or_else(|| {
                    CommError::new(
                        "event_data_invalid",
                        "message delivery attempt target has no runtime identity",
                    )
                })?;
                let runtime =
                    global_registry::runtime_for_replay(runtime_id, &attempt.runtime_fingerprint)
                        .map_err(|error| CommError::new("event_data_invalid", error))?;
                let adapter = self.require_adapter(&message.adapter_id)?;
                validate_message_delivery_attempt(
                    &attempt, &message, &target, &runtime, adapter, false,
                )?;
                if let Some(existing) = self.projection.message_delivery_attempts.get(message_id) {
                    if existing != &attempt {
                        return Err(CommError::new(
                            "event_data_invalid",
                            format!("message delivery attempt already differs: {message_id}"),
                        ));
                    }
                    return Ok(());
                }
                self.projection
                    .message_delivery_attempts
                    .insert(message_id.into(), attempt);
            }
            "message.state" => {
                let message_id = event
                    .data
                    .get("messageId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("event_data_invalid", "messageId missing"))?;
                let state = event
                    .data
                    .get("state")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("event_data_invalid", "message state missing"))?;
                let evidence: DeliveryEvidence = decode(
                    event.data.get("evidence").unwrap_or(&Value::Null),
                    "evidence",
                )?;
                if evidence.state != state {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "message state evidence does not match message state",
                    ));
                }
                let message_record = self
                    .projection
                    .messages
                    .get(message_id)
                    .cloned()
                    .ok_or_else(|| CommError::new("event_data_invalid", "message not found"))?;
                let target = self.resolve_agent(&message_record.to).map_err(|error| {
                    CommError::new(
                        "event_data_invalid",
                        format!("message target agent is missing: {}", error.message),
                    )
                })?;
                let adapter = self.require_adapter(&message_record.adapter_id)?.clone();
                let attempt_id_value = event.data.get("attemptId");
                let nonce_value = event.data.get("nonce");
                if attempt_id_value.is_some_and(|value| !value.is_string()) {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "message state attemptId must be a string",
                    ));
                }
                if nonce_value.is_some_and(|value| !value.is_string()) {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "message state nonce must be a string",
                    ));
                }
                let attempt_id = attempt_id_value.and_then(Value::as_str);
                let nonce = nonce_value.and_then(Value::as_str);
                validate_replayed_delivery_evidence(
                    state,
                    &evidence,
                    &target,
                    message_id,
                    &message_record,
                    self.projection.message_delivery_attempts.get(message_id),
                    attempt_id,
                    nonce,
                    &adapter,
                )?;
                let message = self
                    .projection
                    .messages
                    .get_mut(message_id)
                    .ok_or_else(|| CommError::new("event_data_invalid", "message not found"))?;
                validate_delivery_state_transition(&message.state, state)?;
                message.state = state.into();
                message.evidence.push(evidence);
            }
            "notification.queued" => {
                let key = event
                    .data
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification key missing")
                    })?;
                let notification: NotificationRecord = decode(
                    event.data.get("notification").unwrap_or(&Value::Null),
                    "notification",
                )?;
                self.projection
                    .notifications
                    .insert(key.into(), notification);
            }
            "notification.superseded" => {
                let keys = event
                    .data
                    .get("keys")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification superseded keys missing")
                    })?;
                for value in keys {
                    let key = value.as_str().ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            "notification superseded key is not a string",
                        )
                    })?;
                    let notification =
                        self.projection.notifications.get_mut(key).ok_or_else(|| {
                            CommError::new(
                                "event_data_invalid",
                                format!("notification key not found: {key}"),
                            )
                        })?;
                    if notification.status == "pending" {
                        notification.status = "superseded".into();
                        notification.delivery_attempt = None;
                    }
                }
            }
            "notification.delivery_attempt" => {
                let keys = event
                    .data
                    .get("keys")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "delivery attempt keys missing")
                    })?;
                if keys.is_empty() {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "delivery attempt keys must not be empty",
                    ));
                }
                let attempt_id = event
                    .data
                    .get("attemptId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "delivery attempt id missing")
                    })?;
                let attempt: DeliveryAttempt = decode(
                    event.data.get("attempt").unwrap_or(&Value::Null),
                    "delivery attempt",
                )?;
                if attempt_id != attempt.attempt_id {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "delivery attempt id does not match attempt record",
                    ));
                }
                if !matches!(
                    attempt.operation.as_str(),
                    "notification.emitted" | "notification.batch_emitted"
                ) {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "delivery attempt operation is unsupported",
                    ));
                }
                if (attempt.operation == "notification.batch_emitted") != attempt.batch_id.is_some()
                {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "delivery attempt batch id does not match operation",
                    ));
                }
                for value in keys {
                    let key = value.as_str().ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            "delivery attempt notification key is not a string",
                        )
                    })?;
                    let notification =
                        self.projection.notifications.get_mut(key).ok_or_else(|| {
                            CommError::new(
                                "event_data_invalid",
                                format!("notification key not found: {key}"),
                            )
                        })?;
                    if notification.adapter_id != attempt.adapter_id {
                        return Err(CommError::new(
                            "event_data_invalid",
                            format!(
                                "delivery attempt adapter mismatch for notification key: {key}"
                            ),
                        ));
                    }
                    notification.delivery_attempt = Some(attempt.clone());
                    notification.status = "pending".into();
                }
            }
            "notification.emitted" => {
                let keys = event
                    .data
                    .get("keys")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification keys missing")
                    })?;
                let at = event
                    .data
                    .get("at")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification emitted at missing")
                    })?;
                let receipt = event
                    .data
                    .get("receipt")
                    .map(|value| decode::<TransportReceipt>(value, "receipt"))
                    .transpose()?;
                let attempt_id = event
                    .data
                    .get("attemptId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            "notification emitted attempt id missing or is not a string",
                        )
                    })?;
                let completed_attempt_id = attempt_id.to_owned();
                for value in keys {
                    let key = value.as_str().ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification key is not a string")
                    })?;
                    {
                        let notification =
                            self.projection.notifications.get_mut(key).ok_or_else(|| {
                                CommError::new(
                                    "event_data_invalid",
                                    format!("notification key not found: {key}"),
                                )
                            })?;
                        validate_terminal_attempt(
                            notification,
                            Some(attempt_id),
                            "notification.emitted",
                        )?;
                        notification.status = "emitted".into();
                        notification.emitted_at = Some(at.into());
                        notification.delivery_attempt = None;
                        if let Some(receipt) = receipt.clone() {
                            notification.transport_receipt = Some(receipt);
                        }
                    }
                    self.projection
                        .completed_attempts
                        .insert(key.into(), completed_attempt_id.clone());
                }
            }
            "notification.batch_emitted" => {
                let batch: NotificationBatch =
                    decode(event.data.get("batch").unwrap_or(&Value::Null), "batch")?;
                let keys = event
                    .data
                    .get("notificationKeys")
                    .or_else(|| event.data.get("notificationIds"))
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification keys missing")
                    })?;
                let at = event
                    .data
                    .get("at")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification batch at missing")
                    })?;
                let receipt = event
                    .data
                    .get("receipt")
                    .map(|value| decode::<TransportReceipt>(value, "receipt"))
                    .transpose()?;
                let attempt_id = event
                    .data
                    .get("attemptId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            "notification batch attempt id missing or is not a string",
                        )
                    })?;
                let completed_attempt_id = attempt_id.to_owned();
                for value in keys {
                    let key = value.as_str().ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification key is not a string")
                    })?;
                    {
                        let notification = if let Some(notification) =
                            self.projection.notifications.get_mut(key)
                        {
                            notification
                        } else {
                            self.projection
                                .notifications
                                .values_mut()
                                .find(|notification| notification.notification_id == key)
                                .ok_or_else(|| {
                                    CommError::new(
                                        "event_data_invalid",
                                        format!("notification key not found: {key}"),
                                    )
                                })?
                        };
                        validate_terminal_attempt(
                            notification,
                            Some(attempt_id),
                            "notification.batch_emitted",
                        )?;
                        notification.status = "emitted".into();
                        notification.emitted_at = Some(at.into());
                        notification.delivery_attempt = None;
                        if let Some(receipt) = receipt.clone() {
                            notification.transport_receipt = Some(receipt);
                        }
                    }
                    self.projection
                        .completed_attempts
                        .insert(key.into(), completed_attempt_id.clone());
                }
                self.projection.batches.push(batch);
            }
            "wakeup.updated" => {
                let wakeup: WakeupRecord = decode(&event.data, "wakeup")?;
                self.projection.wakeup.insert(wakeup.address.key(), wakeup);
            }
            "master_wake.updated" => {
                let accumulator = decode_master_wake_accumulator_event(&event.data)?;
                self.projection
                    .master_wake
                    .insert(accumulator.address.key(), accumulator);
            }
            "master_wake.decided" => {
                self.apply_master_wake_decided_event(&event.data)?;
            }
            "master_wake.briefing" => {
                self.apply_master_wake_briefing_event(&event.data)?;
            }
            "wakeup.reminder" => {
                let wakeup: WakeupRecord =
                    decode(event.data.get("wakeup").unwrap_or(&Value::Null), "wakeup")?;
                let message: MessageRecord =
                    decode(event.data.get("message").unwrap_or(&Value::Null), "message")?;
                let notification: NotificationRecord = decode(
                    event.data.get("notification").unwrap_or(&Value::Null),
                    "notification",
                )?;
                if let Some(receipt) = event.data.get("receipt").filter(|value| !value.is_null()) {
                    let _: TransportReceipt = decode(receipt, "receipt")?;
                }
                let key = event
                    .data
                    .get("notificationKey")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        format!(
                            "{}",
                            structured_key(&[
                                "appsdk/daemon",
                                &notification.recipient.key(),
                                notification
                                    .coalesce_key
                                    .as_deref()
                                    .unwrap_or("master-idle"),
                            ])
                        )
                    });
                let attempt_id = event
                    .data
                    .get("attemptId")
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        value.as_str().ok_or_else(|| {
                            CommError::new(
                                "event_data_invalid",
                                "wakeup reminder attempt id is not a string",
                            )
                        })
                    })
                    .transpose()?;
                if let Some(attempt_id) = attempt_id {
                    match self.projection.completed_attempts.get(&key) {
                        Some(completed_attempt_id) if completed_attempt_id == attempt_id => {}
                        Some(completed_attempt_id) => {
                            return Err(CommError::new(
                                "delivery_attempt_mismatch",
                                format!(
                                    "wakeup reminder attempt {attempt_id} does not match completed attempt {completed_attempt_id}"
                                ),
                            ));
                        }
                        None => {
                            return Err(CommError::new(
                                "delivery_attempt_mismatch",
                                format!(
                                    "wakeup reminder attempt {attempt_id} has no completed terminal event"
                                ),
                            ));
                        }
                    }
                }
                self.projection.wakeup.insert(wakeup.address.key(), wakeup);
                self.projection
                    .message_ordinals
                    .entry(message.message_id.clone())
                    .or_insert(event_ordinal);
                self.projection
                    .messages
                    .insert(message.message_id.clone(), message);
                self.projection.notifications.insert(key, notification);
            }
            "notification.delivery_failed" => {
                let keys = event
                    .data
                    .get("keys")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification failure keys missing")
                    })?;
                let attempt_id = event
                    .data
                    .get("attemptId")
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        value.as_str().ok_or_else(|| {
                            CommError::new(
                                "event_data_invalid",
                                "notification failure attempt id is not a string",
                            )
                        })
                    })
                    .transpose()?;
                let operation = event
                    .data
                    .get("operation")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            "notification failure operation missing or is not a string",
                        )
                    })?;
                if !matches!(
                    operation,
                    "notification.emitted" | "notification.batch_emitted"
                ) {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "notification failure operation is unsupported",
                    ));
                }
                let error: ErrorRecord = decode(
                    event.data.get("error").unwrap_or(&Value::Null),
                    "notification failure error",
                )?;
                let completed_attempt_id = attempt_id.map(str::to_owned);
                for value in keys {
                    let key = value.as_str().ok_or_else(|| {
                        CommError::new("event_data_invalid", "notification key is not a string")
                    })?;
                    {
                        let notification = if let Some(notification) =
                            self.projection.notifications.get_mut(key)
                        {
                            notification
                        } else {
                            self.projection
                                .notifications
                                .values_mut()
                                .find(|notification| notification.notification_id == key)
                                .ok_or_else(|| {
                                    CommError::new(
                                        "event_data_invalid",
                                        format!("notification key not found: {key}"),
                                    )
                                })?
                        };
                        validate_terminal_attempt(notification, attempt_id, operation)?;
                        notification.last_error = Some(error.clone());
                        notification.status = "pending".into();
                        notification.delivery_attempt = None;
                    }
                    if let Some(attempt_id) = completed_attempt_id.as_ref() {
                        self.projection
                            .completed_attempts
                            .insert(key.into(), attempt_id.clone());
                    }
                }
            }
            "bug.reported" => {
                let bug: BugRecord = decode(&event.data, "bug")?;
                self.projection.bugs.insert(bug.bug_id.clone(), bug);
            }
            "bug.updated" => {
                let bug: BugRecord = decode(event.data.get("bug").unwrap_or(&event.data), "bug")?;
                self.projection.bugs.insert(bug.bug_id.clone(), bug);
                if let Some(loop_value) = event.data.get("loop").filter(|value| !value.is_null()) {
                    let loop_record: LoopRecord = decode(loop_value, "loop")?;
                    self.projection
                        .loops
                        .insert(loop_record.loop_id.clone(), loop_record);
                }
            }
            "loop.created" | "loop.updated" => {
                let loop_record: LoopRecord = decode(&event.data, "loop")?;
                self.projection
                    .loops
                    .insert(loop_record.loop_id.clone(), loop_record);
            }
            "error.recorded" => {
                let _: ErrorRecord =
                    decode(event.data.get("error").unwrap_or(&event.data), "error")?;
                if let Some(loop_value) = event.data.get("loop").filter(|value| !value.is_null()) {
                    let loop_record: LoopRecord = decode(loop_value, "loop")?;
                    self.projection
                        .loops
                        .insert(loop_record.loop_id.clone(), loop_record);
                }
            }
            other => {
                return Err(CommError::new(
                    "journal_unknown_event",
                    format!("unknown communication event: {other}"),
                ))
            }
        }
        Ok(())
    }

    pub(super) fn dispatch(&mut self, request: &Value) -> CommResult<Value> {
        let op = request
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| CommError::new("operation_missing", "request op is required"))?;
        match op {
            "capabilities" => Ok(capabilities()),
            "status" => Ok(self.status()),
            "register_runtime" | "register-runtime" => self.register_runtime(decode(
                request.get("runtime").unwrap_or(request),
                "runtime",
            )?),
            "register_adapter" | "register-adapter" => self.register_adapter(decode(
                request.get("adapter").unwrap_or(request),
                "adapter",
            )?),
            "register_scope" | "register-scope" => {
                self.register_scope(decode(request.get("scope").unwrap_or(request), "scope")?)
            }
            "register_agent" | "register-agent" => {
                self.register_agent(decode(request.get("agent").unwrap_or(request), "agent")?)
            }
            "refresh_agent" | "refresh-agent" => {
                let address: Address =
                    decode(request.get("address").unwrap_or(&Value::Null), "address")?;
                self.refresh_agent(address, request.get("at").and_then(Value::as_str))
            }
            "rebind_agent" | "rebind-agent" => {
                self.rebind_agent(decode(request.get("rebind").unwrap_or(request), "rebind")?)
            }
            "send" => self.send(decode(
                request.get("message").unwrap_or(request),
                "message",
            )?),
            "record_delivery" | "record-delivery" => self.record_delivery(decode(
                request.get("delivery").unwrap_or(request),
                "delivery",
            )?),
            "set_agent_state" | "set-agent-state" => {
                let address: Address =
                    decode(request.get("address").unwrap_or(&Value::Null), "address")?;
                let state = request
                    .get("state")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("state_missing", "state is required"))?;
                self.set_agent_state(address, state, request.get("at").and_then(Value::as_str))
            }
            "tick" => self.tick(request.get("now").and_then(Value::as_str)),
            "accumulate_wake" | "accumulate-wake" | "record_wake" | "record-wake" => {
                let master: Address = decode(
                    request
                        .get("master")
                        .or_else(|| request.get("address"))
                        .unwrap_or(&Value::Null),
                    "master",
                )?;
                let signal: MasterWakeSignalRequest =
                    decode(request.get("signal").unwrap_or(request), "signal")?;
                self.record_master_wake(master, signal)
            }
            "master_wake_decide" | "master-wake-decide" => {
                let master: Address = decode(
                    request
                        .get("master")
                        .or_else(|| request.get("address"))
                        .unwrap_or(&Value::Null),
                    "master",
                )?;
                let generation = request
                    .get("generation")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| {
                        CommError::new("master_wake_generation_required", "generation is required")
                    })?;
                let action = request
                    .get("action")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new("master_wake_action_required", "action is required")
                    })?;
                self.decide_master_wake(
                    master,
                    generation,
                    action,
                    request.get("at").and_then(Value::as_str),
                )
            }
            "flush_notifications" | "flush-notifications" => {
                self.flush_notifications(request.get("now").and_then(Value::as_str))
            }
            "report_bug" | "report-bug" => {
                self.report_bug(decode(request.get("bug").unwrap_or(request), "bug")?)
            }
            "update_bug" | "update-bug" => {
                let bug_id = request
                    .get("bugId")
                    .or_else(|| request.get("bug_id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("bug_id_missing", "bugId is required"))?;
                let status = request
                    .get("status")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("bug_status_missing", "status is required"))?;
                let actor: Address = decode(request.get("actor").unwrap_or(&Value::Null), "actor")?;
                self.update_bug(bug_id, status, actor, request.get("evidence").cloned())
            }
            "create_loop" | "create-loop" => {
                self.create_loop(decode(request.get("loop").unwrap_or(request), "loop")?)
            }
            "advance_loop" | "advance-loop" => {
                let loop_id = request
                    .get("loopId")
                    .or_else(|| request.get("loop_id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("loop_id_missing", "loopId is required"))?;
                self.advance_loop(
                    loop_id,
                    request
                        .get("complete")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    request
                        .get("blocked")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    request
                        .get("actor")
                        .map(|value| decode(value, "actor"))
                        .transpose()?,
                    request.get("evidence").cloned(),
                    request.get("now").and_then(Value::as_str),
                )
            }
            "record_error" | "record-error" => self.record_error(
                request
                    .get("code")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommError::new("error_code_missing", "code is required"))?,
                request
                    .get("message")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CommError::new("error_message_missing", "message is required")
                    })?,
                request.get("context").cloned().unwrap_or(Value::Null),
                request
                    .get("loopId")
                    .or_else(|| request.get("loop_id"))
                    .and_then(Value::as_str),
            ),
            other => Err(CommError::new(
                "unknown_operation",
                format!("unknown communication operation: {other}"),
            )),
        }
    }
}
