use super::*;
use super::helpers::*;
use super::validation::*;

impl CommunicationStore {
    pub(super) fn send(&mut self, request: MessageRequest) -> CommResult<Value> {
        validate_message_request(&request)?;
        let source = self.require_live_agent(&request.from)?.clone();
        let target = self.resolve_live_agent(&request.to)?;
        self.require_agent_runtime(&source)?;
        self.require_agent_runtime(&target)?;
        let route = self.resolve_route(&source, &target)?;
        self.enqueue_message(request, route, None)
    }

    pub(super) fn record_delivery(&mut self, request: DeliveryRequest) -> CommResult<Value> {
        validate_non_empty(&request.message_id, "messageId")?;
        validate_non_empty(&request.attempt_id, "attemptId")?;
        validate_non_empty(&request.nonce, "nonce")?;
        validate_non_empty(&request.runtime_id, "runtimeId")?;
        validate_non_empty(&request.state, "state")?;
        if request
            .evidence
            .as_object()
            .is_none_or(|evidence| evidence.is_empty())
        {
            return Err(CommError::new(
                "delivery_evidence_required",
                "delivery evidence must be a non-empty object",
            ));
        }
        let state = request.state.trim().to_ascii_lowercase();
        if !matches!(
            state.as_str(),
            "delivered" | "executed" | "replied" | "read" | "consumed" | "unknown"
        ) {
            return Err(CommError::new(
                "invalid_delivery_state",
                format!("unsupported delivery state: {}", request.state),
            ));
        }
        let message = self
            .projection
            .messages
            .get(&request.message_id)
            .cloned()
            .ok_or_else(|| {
                CommError::new(
                    "message_not_found",
                    format!("message not found: {}", request.message_id),
                )
            })?;
        let adapter = self.require_adapter(&message.adapter_id)?;
        validate_adapter_delivery_receipt(
            &adapter.kind,
            &request.evidence,
            &request.runtime_id,
            &state,
        )?;
        // An exact replay of an already committed receipt is idempotent even
        // when the runtime has since refreshed its volatile transport fields.
        // It creates no new fact; a new state still goes through the current
        // runtime and attempt validation below.
        if message.evidence.iter().any(|evidence| {
            evidence.state == state
                && evidence.details.get("runtimeId").and_then(Value::as_str)
                    == Some(request.runtime_id.as_str())
                && evidence.details.get("receipt") == Some(&request.evidence)
                && evidence.details.get("attemptId").and_then(Value::as_str)
                    == Some(request.attempt_id.as_str())
                && evidence.details.get("nonce").and_then(Value::as_str)
                    == Some(request.nonce.as_str())
        }) {
            return Ok(json!({ "message": message, "idempotent": true }));
        }
        let target = self.resolve_live_agent(&message.to)?;
        let target_runtime_id = target.runtime_id.as_deref().ok_or_else(|| {
            CommError::new(
                "runtime_registration_required",
                format!(
                    "message target has no runtime identity: {}",
                    message.to.key()
                ),
            )
        })?;
        if target_runtime_id != request.runtime_id {
            return Err(CommError::new(
                "delivery_runtime_mismatch",
                format!(
                    "delivery runtimeId does not match target runtime: {}",
                    request.runtime_id
                ),
            ));
        }
        let runtime = global_registry::runtime(&request.runtime_id)
            .map_err(|error| CommError::new("runtime_registration_required", error))?;
        let attempt = self
            .projection
            .message_delivery_attempts
            .get(&request.message_id)
            .cloned()
            .ok_or_else(|| {
                CommError::new(
                    "delivery_attempt_required",
                    format!(
                        "message has no persisted delivery attempt: {}",
                        request.message_id
                    ),
                )
            })?;
        validate_message_delivery_attempt(
            &attempt,
            &message,
            &target,
            &runtime,
            self.require_adapter(&message.adapter_id)?,
            false,
        )?;
        if attempt.attempt_id != request.attempt_id {
            return Err(CommError::new(
                "delivery_attempt_mismatch",
                format!(
                    "delivery attempt {} does not match persisted attempt {}",
                    request.attempt_id, attempt.attempt_id
                ),
            ));
        }
        if attempt.nonce != request.nonce {
            return Err(CommError::new(
                "delivery_attempt_nonce_mismatch",
                "delivery receipt nonce does not match persisted delivery attempt",
            ));
        }
        let at = request
            .observed_at
            .as_deref()
            .map(validate_time)
            .transpose()?
            .unwrap_or_else(now);
        let details = json!({
            "runtimeId": request.runtime_id,
            "runtimeFingerprint": attempt.runtime_fingerprint,
            "attemptId": request.attempt_id,
            "nonce": request.nonce,
            "adapterId": attempt.adapter_id,
            "target": attempt.target,
            "receipt": request.evidence
        });
        validate_delivery_state_transition(&message.state, &state)?;
        let evidence = DeliveryEvidence {
            state: state.clone(),
            at: at.clone(),
            details,
        };
        self.commit(
            "message.state",
            json!({
                "messageId": request.message_id,
                "state": state,
                "evidence": evidence,
                "attemptId": request.attempt_id,
                "nonce": request.nonce
            }),
        )?;
        Ok(json!({
            "message": self.projection.messages.get(&request.message_id),
            "idempotent": false,
            "observedAt": at
        }))
    }

    pub(super) fn ensure_message_delivery_attempt(
        &mut self,
        message: &MessageRecord,
    ) -> CommResult<MessageDeliveryAttempt> {
        if let Some(existing) = self
            .projection
            .message_delivery_attempts
            .get(&message.message_id)
            .cloned()
        {
            let target = self.resolve_live_agent(&message.to)?;
            let runtime_id = target.runtime_id.as_deref().ok_or_else(|| {
                CommError::new(
                    "runtime_registration_required",
                    format!(
                        "message target has no runtime identity: {}",
                        message.to.key()
                    ),
                )
            })?;
            let runtime = global_registry::runtime(runtime_id)
                .map_err(|error| CommError::new("runtime_registration_required", error))?;
            validate_message_delivery_attempt(
                &existing,
                message,
                &target,
                &runtime,
                self.require_adapter(&message.adapter_id)?,
                false,
            )?;
            return Ok(existing);
        }

        let target = self.resolve_live_agent(&message.to)?;
        let runtime_id = target.runtime_id.clone().ok_or_else(|| {
            CommError::new(
                "runtime_registration_required",
                format!(
                    "message target has no runtime identity: {}",
                    message.to.key()
                ),
            )
        })?;
        let runtime = global_registry::runtime(&runtime_id)
            .map_err(|error| CommError::new("runtime_registration_required", error))?;
        let adapter = self.require_adapter(&message.adapter_id)?;
        let attempt = MessageDeliveryAttempt {
            attempt_id: new_id("attempt"),
            message_id: message.message_id.clone(),
            operation: "message.delivery".into(),
            adapter_id: message.adapter_id.clone(),
            runtime_id,
            runtime_fingerprint: runtime.fingerprint.clone(),
            target: adapter.target.clone(),
            nonce: new_id("nonce"),
            started_at: now(),
        };
        validate_message_delivery_attempt(&attempt, message, &target, &runtime, adapter, false)?;
        self.commit(
            "message.delivery_attempt",
            json!({
                "messageId": message.message_id,
                "attempt": attempt
            }),
        )?;
        Ok(self
            .projection
            .message_delivery_attempts
            .get(&message.message_id)
            .cloned()
            .expect("message delivery attempt committed"))
    }

    pub fn set_agent_state(
        &mut self,
        address: Address,
        state: &str,
        at: Option<&str>,
    ) -> CommResult<Value> {
        let next = AgentState::parse(state)?;
        let at = at.map(validate_time).transpose()?.unwrap_or_else(now);
        let current = self.require_agent(&address)?.clone();
        if current.state == next {
            if current.role == "master" {
                self.repair_master_wakeup(&current, &next)?;
                self.sync_master_wake_schedule(&current.address(), &next, &at)?;
            } else if current.role != "master" && next == AgentState::Idle {
                let master_address = self.scope_master_address(&current.scope_id)?;
                let signal_key = worker_idle_signal_key(&current.address());
                let message =
                    worker_idle_message(&current, master_address.clone(), &current.last_state_at);
                let message_id = message
                    .message_id
                    .as_deref()
                    .expect("worker idle message must have a deterministic message id");
                let signal_recorded =
                    self.master_wake_signal_recorded(&master_address, &signal_key, message_id);
                if !signal_recorded {
                    // The state edge was persisted before its wake signal (for example when
                    // the scope had no master). Repair only that missing step. A consumed edge
                    // remains consumed and must never be reactivated by observing idle again.
                    self.accumulate_worker_idle(&current, &master_address, &current.last_state_at)?;
                }
                if !self.master_wake_signal_consumed(&master_address, &signal_key, message_id) {
                    let notification_key = structured_key(&[
                        &current.address().key(),
                        &master_address.key(),
                        "mailbox",
                        &format!("idle:{}", current.address().key()),
                    ]);
                    let notification_matches_message = self
                        .projection
                        .notifications
                        .get(&notification_key)
                        .is_some_and(|notification| notification.message_id == message_id);
                    if !self.projection.messages.contains_key(message_id)
                        || !notification_matches_message
                    {
                        let notification = self.send(message)?;
                        return Ok(json!({
                            "agent": current,
                            "idempotent": true,
                            "notification": notification
                        }));
                    }
                }
            }
            return Ok(json!({
                "agent": current,
                "idempotent": true,
                "notification": Value::Null
            }));
        }
        self.commit(
            "agent.state",
            json!({ "address": address, "state": next, "at": at }),
        )?;

        if current.role == "master" {
            let wakeup = if next == AgentState::Idle {
                WakeupRecord {
                    address: current.address(),
                    identity_origin: None,
                    idle_since: Some(at.clone()),
                    reminders_sent: 0,
                    next_due_at: Some(add_seconds(&at, DEFAULT_BATCH_WINDOW_SECONDS)?),
                    stopped: false,
                    last_reminder_at: None,
                }
            } else {
                WakeupRecord {
                    address: current.address(),
                    identity_origin: None,
                    idle_since: None,
                    reminders_sent: 0,
                    next_due_at: None,
                    stopped: false,
                    last_reminder_at: None,
                }
            };
            self.commit("wakeup.updated", serde_json::to_value(&wakeup).unwrap())?;
            self.sync_master_wake_schedule(&current.address(), &next, &at)?;
            return Ok(json!({
                "agent": self.require_agent(&address)?,
                "idempotent": false,
                "notification": Value::Null
            }));
        }

        if next == AgentState::Idle {
            let master_address = self.scope_master_address(&current.scope_id)?;
            self.accumulate_worker_idle(&current, &master_address, &at)?;
            let message = worker_idle_message(&current, master_address, &at);
            let notification = match self.send(message) {
                Ok(value) => value,
                Err(error) => return Err(error),
            };
            return Ok(json!({
                "agent": self.require_agent(&address)?,
                "idempotent": false,
                "notification": notification
            }));
        }

        if next == AgentState::Working {
            self.clear_worker_idle(&current.address(), &at)?;
        }

        Ok(json!({
            "agent": self.require_agent(&address)?,
            "idempotent": false,
            "notification": Value::Null
        }))
    }

    pub(super) fn accumulate_worker_idle(
        &mut self,
        worker: &AgentRecord,
        master: &Address,
        at: &str,
    ) -> CommResult<MasterWakeAccumulator> {
        let source = worker.address();
        let key = worker_idle_signal_key(&source);
        let signal = MasterWakeSignal {
            signal_id: worker_idle_message_id(worker, at),
            key,
            identity_key: None,
            identity_signal_id: None,
            kind: "worker_idle".into(),
            title: format!("worker idle: {}", worker.agent_id),
            priority: Priority::P2,
            summary: format!(
                "{} entered idle; inspect the JSONL facts for its latest result",
                worker.agent_id
            ),
            issue_id: None,
            source: Some(source),
            observed_at: validate_time(at)?,
            direct_dispatched: false,
        };
        self.accumulate_master_wake(master, signal)
    }

    pub(super) fn clear_worker_idle(&mut self, worker: &Address, at: &str) -> CommResult<()> {
        let Some(master) = self.scope_master_address_unchecked(&worker.scope_id) else {
            return Ok(());
        };
        self.clear_master_wake_signal(&master, &worker_idle_signal_key(worker), at)
    }

    pub(super) fn accumulate_master_wake(
        &mut self,
        master: &Address,
        signal: MasterWakeSignal,
    ) -> CommResult<MasterWakeAccumulator> {
        validate_address(master)?;
        validate_non_empty(&signal.signal_id, "signalId")?;
        validate_non_empty(&signal.key, "key")?;
        validate_non_empty(&signal.kind, "kind")?;
        validate_non_empty(&signal.title, "title")?;
        validate_non_empty(&signal.summary, "summary")?;
        if signal.title.chars().count() > 200 {
            return Err(CommError::new(
                "master_wake_title_too_long",
                "master wake signal title must be at most 200 characters",
            ));
        }
        let master_agent = self.require_agent(master)?.clone();
        if master_agent.role != "master"
            || self
                .projection
                .scopes
                .get(&master.scope_id)
                .and_then(|scope| scope.master_session_id.as_deref())
                != Some(master.session_id.as_str())
        {
            return Err(CommError::new(
                "master_wake_target_required",
                "master wake accumulator requires the registered scope master",
            ));
        }

        let observed_at = validate_time(&signal.observed_at)?;
        let key = master.key();
        let existing = self.projection.master_wake.get(&key).cloned();
        if let Some(existing_signal) = existing
            .as_ref()
            .and_then(|accumulator| accumulator.signals.get(&signal.key))
        {
            if master_wake_signal_identity_matches(existing_signal, &signal) {
                return Ok(existing.expect("master wake accumulator exists"));
            }
            if existing_signal.signal_id == signal.signal_id {
                return Err(CommError::new(
                    "master_wake_signal_conflict",
                    format!("master wake signal id conflicts: {}", signal.signal_id),
                ));
            }
        }
        if let Some(existing_signal) = existing
            .as_ref()
            .and_then(|accumulator| accumulator.consumed_signals.get(&signal.key))
        {
            if master_wake_signal_identity_matches(existing_signal, &signal) {
                return Ok(existing.expect("master wake accumulator exists"));
            }
            if existing_signal.signal_id == signal.signal_id {
                return Err(CommError::new(
                    "master_wake_signal_conflict",
                    format!(
                        "consumed master wake signal id conflicts: {}",
                        signal.signal_id
                    ),
                ));
            }
        }

        let mut accumulator = existing.unwrap_or_else(|| MasterWakeAccumulator {
            address: master.clone(),
            identity_origin: None,
            generation: 0,
            pending: false,
            first_observed_at: None,
            last_observed_at: None,
            next_due_at: None,
            reminders_sent: 0,
            stopped: false,
            last_briefing_generation: None,
            last_briefing_at: None,
            held: false,
            signals: BTreeMap::new(),
            consumed_signals: BTreeMap::new(),
        });
        let cycle_start = if accumulator.pending {
            accumulator
                .first_observed_at
                .clone()
                .unwrap_or_else(|| observed_at.clone())
        } else {
            observed_at.clone()
        };
        accumulator.generation = accumulator.generation.checked_add(1).ok_or_else(|| {
            CommError::new(
                "master_wake_generation_exhausted",
                format!("master wake generation exhausted: {}", master.key()),
            )
        })?;
        accumulator.first_observed_at = Some(cycle_start.clone());
        accumulator.last_observed_at = Some(observed_at.clone());
        accumulator
            .signals
            .insert(signal.key.clone(), signal.clone());
        accumulator.consumed_signals.remove(&signal.key);
        accumulator.pending = accumulator
            .signals
            .values()
            .any(|value| !value.direct_dispatched);
        accumulator.reminders_sent = 0;
        accumulator.stopped = false;
        accumulator.held = false;
        accumulator.last_briefing_generation = None;
        accumulator.last_briefing_at = None;
        accumulator.next_due_at = if accumulator.pending {
            if signal.priority.is_breakthrough() {
                Some(observed_at)
            } else {
                Some(add_seconds(&cycle_start, DEFAULT_BATCH_WINDOW_SECONDS)?)
            }
        } else {
            None
        };
        self.commit(
            "master_wake.updated",
            serde_json::to_value(&accumulator).unwrap(),
        )?;
        Ok(accumulator)
    }

    pub(super) fn sync_master_wake_schedule(
        &mut self,
        master: &Address,
        state: &AgentState,
        _at: &str,
    ) -> CommResult<()> {
        let key = master.key();
        let Some(existing) = self.projection.master_wake.get(&key).cloned() else {
            return Ok(());
        };
        let mut updated = existing.clone();
        match state {
            AgentState::Working => {}
            AgentState::Idle => {
                if updated.pending
                    && !updated.stopped
                    && !updated.held
                    && updated.next_due_at.is_none()
                {
                    let due_origin = updated
                        .last_briefing_at
                        .as_deref()
                        .or(updated.first_observed_at.as_deref())
                        .ok_or_else(|| {
                            CommError::new(
                                "master_wake_schedule_origin_missing",
                                format!("master wake has no durable schedule origin: {key}"),
                            )
                        })?;
                    updated.next_due_at =
                        Some(add_seconds(due_origin, DEFAULT_BATCH_WINDOW_SECONDS)?);
                }
            }
        }
        if master_wake_accumulator_matches(&existing, &updated) {
            return Ok(());
        }
        self.commit(
            "master_wake.updated",
            serde_json::to_value(&updated).unwrap(),
        )?;
        Ok(())
    }

    pub(super) fn clear_master_wake_signal(
        &mut self,
        master: &Address,
        signal_key: &str,
        at: &str,
    ) -> CommResult<()> {
        let key = master.key();
        let Some(existing) = self.projection.master_wake.get(&key).cloned() else {
            return Ok(());
        };
        if !existing.signals.contains_key(signal_key) {
            return Ok(());
        }
        let mut updated = existing.clone();
        if let Some(signal) = updated.signals.remove(signal_key) {
            updated.consumed_signals.insert(signal_key.into(), signal);
        }
        updated.generation = updated.generation.checked_add(1).ok_or_else(|| {
            CommError::new(
                "master_wake_generation_exhausted",
                format!("master wake generation exhausted: {key}"),
            )
        })?;
        updated.last_observed_at = Some(validate_time(at)?);
        updated.pending = updated
            .signals
            .values()
            .any(|value| !value.direct_dispatched);
        updated.next_due_at = if updated.pending && !updated.held {
            updated.next_due_at.clone()
        } else {
            None
        };
        self.commit(
            "master_wake.updated",
            serde_json::to_value(&updated).unwrap(),
        )?;
        Ok(())
    }

    pub(super) fn record_master_wake(
        &mut self,
        master: Address,
        request: MasterWakeSignalRequest,
    ) -> CommResult<Value> {
        validate_master_wake_signal_request(&request)?;
        let observed_at = request
            .observed_at
            .as_deref()
            .map(validate_time)
            .transpose()?
            .unwrap_or_else(now);
        let priority = Priority::parse(&request.priority)?;
        let key = request.key.clone();
        let signal = MasterWakeSignal {
            signal_id: request
                .signal_id
                .unwrap_or_else(|| structured_key(&[&request.kind, &request.key, &observed_at])),
            key,
            identity_key: None,
            identity_signal_id: None,
            kind: request.kind,
            title: request.title,
            priority,
            summary: request.summary,
            issue_id: request.issue_id,
            source: request.source,
            observed_at,
            direct_dispatched: false,
        };
        let idempotent =
            self.projection
                .master_wake
                .get(&master.key())
                .is_some_and(|accumulator| {
                    accumulator
                        .signals
                        .get(&signal.key)
                        .or_else(|| accumulator.consumed_signals.get(&signal.key))
                        .is_some_and(|existing| {
                            master_wake_signal_identity_matches(existing, &signal)
                        })
                });
        let accumulator = self.accumulate_master_wake(&master, signal.clone())?;
        let accumulator = if signal.priority.is_breakthrough() {
            self.dispatch_breakthrough_master_wake(&master, &signal.key)?
        } else {
            accumulator
        };
        Ok(json!({
            "masterWake": accumulator,
            "idempotent": idempotent
        }))
    }

    pub(super) fn dispatch_breakthrough_master_wake(
        &mut self,
        master: &Address,
        signal_key: &str,
    ) -> CommResult<MasterWakeAccumulator> {
        let Some(accumulator) = self.projection.master_wake.get(&master.key()).cloned() else {
            return Err(CommError::new(
                "master_wake_not_found",
                format!("master wake not found: {}", master.key()),
            ));
        };
        let Some(signal) = accumulator.signals.get(signal_key).cloned() else {
            return Ok(accumulator);
        };
        if signal.direct_dispatched {
            return Ok(accumulator);
        }
        let Some(agent) = self.require_live_agent(master).ok().cloned() else {
            return Ok(accumulator);
        };
        let (message_id, conversation_id) =
            master_wake_direct_message_identity(&accumulator, &signal);
        let message = if let Some(existing) = self.projection.messages.get(&message_id).cloned() {
            if existing.conversation_id != conversation_id
                || existing.to != agent.address()
                || existing.from
                    != (Address {
                        scope_id: "appsdk".into(),
                        session_id: "daemon".into(),
                    })
                || existing.delivery_mode != DeliveryMode::Direct
                || existing.coalesce_key.as_deref() != Some("master-wake-direct")
                || existing.adapter_id != "mailbox"
                || existing.issue_id.is_some()
            {
                return Err(CommError::new(
                    "wakeup_message_conflict",
                    format!("direct master wake message id identifies a different message: {message_id}"),
                ));
            }
            self.prepare_wakeup_message(existing)?
        } else {
            let mut message = self.system_message(
                &agent.address(),
                signal.title.clone(),
                &format_priority(&signal.priority),
                &signal.summary,
                "master-wake-direct",
                &signal.observed_at,
                "mailbox",
            )?;
            message.message_id = message_id;
            message.conversation_id = conversation_id;
            message.delivery_mode = DeliveryMode::Direct;
            self.prepare_wakeup_message(message)?
        };
        let notification =
            self.notification_for(&message, &signal.observed_at, Some(&signal.observed_at))?;
        let Some(notification) = notification else {
            return Ok(accumulator);
        };
        if notification.status != "emitted" {
            return Ok(accumulator);
        }
        let mut updated = accumulator;
        if let Some(stored) = updated.signals.get_mut(signal_key) {
            stored.direct_dispatched = true;
        }
        updated.pending = updated
            .signals
            .values()
            .any(|value| !value.direct_dispatched);
        updated.next_due_at = if updated.pending && !updated.held {
            updated.next_due_at.clone()
        } else {
            None
        };
        self.commit(
            "master_wake.updated",
            serde_json::to_value(&updated).unwrap(),
        )?;
        Ok(updated)
    }

    pub(super) fn decide_master_wake(
        &mut self,
        master: Address,
        generation: u64,
        action: &str,
        at: Option<&str>,
    ) -> CommResult<Value> {
        validate_address(&master)?;
        validate_non_empty(action, "action")?;
        let actor = self.require_live_agent(&master)?.clone();
        if actor.role != "master"
            || self
                .projection
                .scopes
                .get(&master.scope_id)
                .and_then(|scope| scope.master_session_id.as_deref())
                != Some(master.session_id.as_str())
        {
            return Err(CommError::new(
                "master_wake_actor_required",
                "only the registered scope master may decide its wake",
            ));
        }
        let key = master.key();
        let existing = self
            .projection
            .master_wake
            .get(&key)
            .cloned()
            .ok_or_else(|| {
                CommError::new(
                    "master_wake_not_found",
                    format!("master wake not found: {key}"),
                )
            })?;
        if existing.generation != generation {
            return Err(CommError::new(
                "master_wake_generation_conflict",
                format!(
                    "master wake generation is {}, not {}",
                    existing.generation, generation
                ),
            ));
        }
        let at = at.map(validate_time).transpose()?.unwrap_or_else(now);
        let mut updated = existing;
        let action = action.trim().to_ascii_lowercase();
        let terminal_decision = matches!(
            action.as_str(),
            "dispatch" | "handled" | "complete" | "completed"
        );
        let superseded_keys = if terminal_decision {
            self.pending_notification_keys_for_master_wake(&updated)
        } else {
            Vec::new()
        };
        match action.as_str() {
            "hold" => {
                updated.held = true;
                updated.next_due_at = None;
            }
            "dispatch" | "handled" | "complete" | "completed" => {
                for (signal_key, signal) in updated.signals.clone() {
                    updated.consumed_signals.insert(signal_key, signal);
                }
                updated.pending = false;
                updated.next_due_at = None;
                updated.last_briefing_generation = Some(generation);
                updated.last_briefing_at = Some(at.clone());
                updated.reminders_sent = 0;
                updated.stopped = false;
                updated.held = false;
                updated.signals.clear();
            }
            "schedule" => {
                if updated.pending {
                    updated.generation = updated.generation.checked_add(1).ok_or_else(|| {
                        CommError::new(
                            "master_wake_generation_exhausted",
                            format!("master wake generation exhausted: {key}"),
                        )
                    })?;
                }
                updated.held = false;
                updated.stopped = false;
                updated.reminders_sent = 0;
                updated.last_briefing_generation = None;
                updated.last_briefing_at = None;
                updated.next_due_at = if updated.pending {
                    if actor.state == AgentState::Idle {
                        Some(at.clone())
                    } else {
                        Some(add_seconds(&at, DEFAULT_BATCH_WINDOW_SECONDS)?)
                    }
                } else {
                    None
                };
            }
            other => {
                return Err(CommError::new(
                    "master_wake_action_invalid",
                    format!("unsupported master wake action: {other}"),
                ))
            }
        }
        if !superseded_keys.is_empty() {
            self.commit(
                "notification.superseded",
                json!({
                    "keys": superseded_keys,
                    "generation": generation,
                    "reason": "master_wake_decision"
                }),
            )?;
        }
        self.commit(
            "master_wake.decided",
            json!({ "accumulator": updated, "action": action, "at": at }),
        )?;
        if let Some(wakeup) = self.projection.wakeup.get(&key).cloned() {
            let synchronized = synchronize_master_wakeup(wakeup, &action, &updated);
            self.commit(
                "wakeup.updated",
                serde_json::to_value(synchronized).unwrap(),
            )?;
        }
        Ok(json!({
            "masterWake": self.projection.master_wake.get(&key),
            "action": action,
            "generation": generation
        }))
    }

    pub(super) fn scope_master_address_unchecked(&self, scope_id: &str) -> Option<Address> {
        self.projection
            .scopes
            .get(scope_id)
            .and_then(|scope| scope.master_session_id.as_ref())
            .map(|session_id| Address {
                scope_id: scope_id.into(),
                session_id: session_id.clone(),
            })
    }

    pub(super) fn master_wake_signal_recorded(
        &self,
        master: &Address,
        signal_key: &str,
        signal_id: &str,
    ) -> bool {
        self.projection
            .master_wake
            .get(&master.key())
            .is_some_and(|accumulator| {
                accumulator
                    .signals
                    .get(signal_key)
                    .is_some_and(|signal| signal.signal_id == signal_id)
                    || accumulator
                        .consumed_signals
                        .get(signal_key)
                        .is_some_and(|signal| signal.signal_id == signal_id)
            })
    }

    pub(super) fn master_wake_signal_consumed(
        &self,
        master: &Address,
        signal_key: &str,
        signal_id: &str,
    ) -> bool {
        self.projection
            .master_wake
            .get(&master.key())
            .and_then(|accumulator| accumulator.consumed_signals.get(signal_key))
            .is_some_and(|signal| signal.signal_id == signal_id)
    }

    pub(super) fn master_wake_signal_delivery_unknown(
        &self,
        accumulator: &MasterWakeAccumulator,
        signal: &MasterWakeSignal,
    ) -> bool {
        let (message_id, _) = master_wake_direct_message_identity(accumulator, signal);
        self.projection.notifications.values().any(|notification| {
            notification.message_id == message_id && notification.status == "unknown"
        })
    }

    pub(super) fn master_wake_signal_delivery_emitted(
        &self,
        accumulator: &MasterWakeAccumulator,
        signal: &MasterWakeSignal,
    ) -> bool {
        let (message_id, _) = master_wake_direct_message_identity(accumulator, signal);
        self.projection.notifications.values().any(|notification| {
            notification.message_id == message_id && notification.status == "emitted"
        })
    }

    pub(super) fn reconcile_breakthrough_master_wake(
        &mut self,
        accumulator: &MasterWakeAccumulator,
    ) -> CommResult<MasterWakeAccumulator> {
        let mut updated = accumulator.clone();
        let mut changed = false;
        for (signal_key, signal) in accumulator.signals.iter() {
            if signal.priority.is_breakthrough()
                && !signal.direct_dispatched
                && self.master_wake_signal_delivery_emitted(accumulator, signal)
            {
                if let Some(stored) = updated.signals.get_mut(signal_key) {
                    stored.direct_dispatched = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return Ok(updated);
        }
        updated.pending = updated
            .signals
            .values()
            .any(|value| !value.direct_dispatched);
        updated.next_due_at = if updated.pending && !updated.held {
            updated.next_due_at.clone()
        } else {
            None
        };
        self.commit(
            "master_wake.updated",
            serde_json::to_value(&updated).unwrap(),
        )?;
        Ok(updated)
    }

    pub(super) fn validate_master_wake_message_identity(
        &self,
        message: &MessageRecord,
        accumulator: &MasterWakeAccumulator,
        reminder: u8,
        target: &Address,
        conversation_id: &str,
    ) -> CommResult<()> {
        let (message_id, _) = master_wake_message_identity(accumulator, reminder);
        if message.message_id != message_id
            || message.conversation_id != conversation_id
            || message.from
                != (Address {
                    scope_id: "appsdk".into(),
                    session_id: "daemon".into(),
                })
            || message.to != *target
            || message.delivery_mode != DeliveryMode::Direct
            || message.coalesce_key.as_deref() != Some("master-wake")
            || message.issue_id.is_some()
            || message.adapter_id != "mailbox"
        {
            return Err(CommError::new(
                "wakeup_message_conflict",
                format!("wakeup message id identifies a different master wake: {message_id}"),
            ));
        }
        Ok(())
    }

    pub(super) fn process_master_wake(
        &mut self,
        accumulator: &MasterWakeAccumulator,
        at: &str,
    ) -> CommResult<Option<MasterWakeAccumulator>> {
        let accumulator = self.reconcile_breakthrough_master_wake(accumulator)?;
        if !accumulator.pending
            || accumulator.held
            || accumulator.stopped
            || accumulator.reminders_sent >= DEFAULT_MASTER_REMINDER_LIMIT
        {
            return Ok(None);
        }
        if !accumulator.signals.values().any(|candidate| {
            !candidate.direct_dispatched
                && !self.master_wake_signal_delivery_unknown(&accumulator, candidate)
        }) {
            // An uncertain direct delivery is never replayed as a briefing. Keep the
            // signal in the accumulator for explicit recovery or a new signal generation.
            return Ok(None);
        }
        let Some(agent) = self.live_idle_master_at(&accumulator.address, at) else {
            return Ok(None);
        };
        let Some(next_due_at) = accumulator.next_due_at.as_deref() else {
            return Ok(None);
        };
        if parse_time(at)? < parse_time(next_due_at)? {
            return Ok(None);
        }
        let reminder = accumulator.reminders_sent + 1;
        let (message_id, conversation_id) = master_wake_message_identity(&accumulator, reminder);
        let message = if let Some(existing) = self.projection.messages.get(&message_id).cloned() {
            self.validate_master_wake_message_identity(
                &existing,
                &accumulator,
                reminder,
                &agent.address(),
                &conversation_id,
            )?;
            self.prepare_wakeup_message(existing)?
        } else {
            let (title, body, priority) = self.master_wake_briefing(&accumulator, reminder);
            let mut message = self.system_message(
                &agent.address(),
                title,
                &format_priority(&priority),
                &body,
                "master-wake",
                at,
                "mailbox",
            )?;
            message.message_id = message_id;
            message.conversation_id = conversation_id;
            message.delivery_mode = DeliveryMode::Direct;
            self.prepare_wakeup_message(message)?
        };
        let notification = self.notification_for(&message, at, Some(at))?;
        let Some(notification) = notification else {
            return Ok(None);
        };
        if notification.status != "emitted" {
            return Ok(None);
        }

        let mut next = accumulator.clone();
        next.reminders_sent = reminder;
        next.last_briefing_generation = Some(accumulator.generation);
        next.last_briefing_at = Some(at.into());
        next.stopped = reminder >= DEFAULT_MASTER_REMINDER_LIMIT;
        next.next_due_at = if next.stopped {
            None
        } else {
            Some(add_seconds(at, DEFAULT_BATCH_WINDOW_SECONDS)?)
        };
        let superseded_keys = self.pending_notification_keys_for_master_wake(&accumulator);
        if !superseded_keys.is_empty() {
            self.commit(
                "notification.superseded",
                json!({
                    "keys": superseded_keys,
                    "generation": accumulator.generation,
                    "reason": "master_wake_briefing"
                }),
            )?;
        }
        self.commit(
            "master_wake.briefing",
            json!({
                "accumulator": next,
                "message": message,
                "notification": notification,
                "generation": accumulator.generation,
                "reminder": reminder
            }),
        )?;
        if let Some(wakeup) = self
            .projection
            .wakeup
            .get(&accumulator.address.key())
            .cloned()
        {
            let mut synchronized = wakeup;
            synchronized.reminders_sent = reminder;
            synchronized.last_reminder_at = Some(at.into());
            synchronized.next_due_at = next.next_due_at.clone();
            synchronized.stopped = next.stopped;
            self.commit(
                "wakeup.updated",
                serde_json::to_value(synchronized).unwrap(),
            )?;
        }
        Ok(Some(next))
    }

    pub(super) fn pending_notification_keys_for_master_wake(
        &self,
        accumulator: &MasterWakeAccumulator,
    ) -> Vec<String> {
        self.projection
            .notifications
            .iter()
            .filter(|(_, notification)| {
                notification.status == "pending"
                    && notification.recipient == accumulator.address
                    && accumulator
                        .signals
                        .values()
                        .any(|signal| master_wake_covers_notification(signal, notification))
            })
            .map(|(key, _)| key.clone())
            .collect()
    }

    pub(super) fn notification_held_for_master_wake(
        &self,
        notification: &NotificationRecord,
    ) -> CommResult<bool> {
        let Some(master) = self.scope_master_address_unchecked(&notification.recipient.scope_id)
        else {
            return Ok(false);
        };
        if master != notification.recipient {
            return Ok(false);
        }
        let Some(agent) = self.projection.agents.get(&master.key()) else {
            return Ok(false);
        };
        if agent.role != "master" {
            return Ok(false);
        }
        Ok(self
            .projection
            .master_wake
            .get(&master.key())
            .is_some_and(|accumulator| {
                accumulator.pending
                    && accumulator
                        .signals
                        .values()
                        .any(|signal| master_wake_covers_notification(signal, notification))
            }))
    }

    pub(super) fn master_wake_briefing(
        &self,
        accumulator: &MasterWakeAccumulator,
        reminder: u8,
    ) -> (String, String, Priority) {
        let mut signals: Vec<&MasterWakeSignal> = accumulator
            .signals
            .values()
            .filter(|signal| {
                !signal.direct_dispatched
                    && !self.master_wake_signal_delivery_unknown(accumulator, signal)
            })
            .collect();
        signals.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left.observed_at.cmp(&right.observed_at))
                .then_with(|| left.key.cmp(&right.key))
        });
        let priority = signals
            .first()
            .map(|signal| signal.priority.clone())
            .unwrap_or(Priority::P1);
        let title = format!(
            "master wake: {} update{} (reminder {}/{})",
            signals.len(),
            if signals.len() == 1 { "" } else { "s" },
            reminder,
            DEFAULT_MASTER_REMINDER_LIMIT
        );
        let mut lines = vec![format!(
            "Generation {} observed {}; inspect mailbox JSONL before dispatching the next loop.",
            accumulator.generation,
            accumulator.last_observed_at.as_deref().unwrap_or("unknown")
        )];
        if signals.is_empty() {
            lines.push(
                "No undelivered signal remains; the direct signal is retained as history.".into(),
            );
        } else {
            lines.push("Signals:".into());
            for signal in signals.iter().take(12) {
                lines.push(format!(
                    "- [{}] {}: {} ({})",
                    format_priority(&signal.priority),
                    signal.title,
                    signal.summary,
                    signal.kind
                ));
            }
            if signals.len() > 12 {
                lines.push(format!(
                    "- ... {} more signals in JSONL",
                    signals.len() - 12
                ));
            }
        }

        let mut idle_workers: Vec<&AgentRecord> = self
            .projection
            .agents
            .values()
            .filter(|agent| {
                agent.scope_id == accumulator.address.scope_id
                    && agent.role != "master"
                    && agent.state == AgentState::Idle
            })
            .collect();
        idle_workers.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        lines.push(format!("Idle workers: {}", idle_workers.len()));
        for worker in idle_workers.iter().take(12) {
            lines.push(format!(
                "- {} ({})",
                worker.agent_id,
                worker.address().key()
            ));
        }
        if idle_workers.len() > 12 {
            lines.push(format!(
                "- ... {} more idle workers in status",
                idle_workers.len() - 12
            ));
        }

        let mut active_bugs: Vec<&BugRecord> = self
            .projection
            .bugs
            .values()
            .filter(|bug| bug.scope_id == accumulator.address.scope_id && bug.status == "active")
            .collect();
        active_bugs.sort_by(|left, right| priority_then_time_bug(left, right));
        lines.push(format!("Active bugs: {}", active_bugs.len()));
        for bug in active_bugs.iter().take(12) {
            lines.push(format!(
                "- [{}] {}: {} ({})",
                format_priority(&bug.priority),
                bug.bug_id,
                bug.title,
                bug.loop_id
            ));
        }
        if active_bugs.len() > 12 {
            lines.push(format!(
                "- ... {} more active bugs in status",
                active_bugs.len() - 12
            ));
        }

        let mut active_loops: Vec<&LoopRecord> = self
            .projection
            .loops
            .values()
            .filter(|loop_record| {
                loop_record.owner.scope_id == accumulator.address.scope_id
                    && loop_record.status == "active"
            })
            .collect();
        active_loops.sort_by(|left, right| left.updated_at.cmp(&right.updated_at));
        lines.push(format!("Active loops: {}", active_loops.len()));
        for loop_record in active_loops.iter().take(12) {
            lines.push(format!(
                "- {} phase={} iteration={}",
                loop_record.loop_id, loop_record.phase, loop_record.iteration
            ));
        }
        if active_loops.len() > 12 {
            lines.push(format!(
                "- ... {} more loops in status",
                active_loops.len() - 12
            ));
        }
        lines.push(
            "Action: consume this generation, prioritize P0/P1, and dispatch only within ownership; use master_wake_decide after a scheduling decision.".into(),
        );
        (title, truncate_chars(&lines.join("\n"), 3_600), priority)
    }

    pub(super) fn repair_master_wakeup(
        &mut self,
        current: &AgentRecord,
        state: &AgentState,
    ) -> CommResult<()> {
        let key = current.address().key();
        let existing = self.projection.wakeup.get(&key).cloned();
        let wakeup = match state {
            AgentState::Working => {
                let valid = existing.as_ref().is_some_and(|wakeup| {
                    wakeup.idle_since.is_none()
                        && wakeup.next_due_at.is_none()
                        && wakeup.reminders_sent == 0
                        && !wakeup.stopped
                        && wakeup.last_reminder_at.is_none()
                });
                if valid {
                    return Ok(());
                }
                WakeupRecord {
                    address: current.address(),
                    identity_origin: None,
                    idle_since: None,
                    reminders_sent: 0,
                    next_due_at: None,
                    stopped: false,
                    last_reminder_at: None,
                }
            }
            AgentState::Idle => {
                let idle_since = current.last_state_at.clone();
                let same_cycle = existing.as_ref().is_some_and(|wakeup| {
                    wakeup.idle_since.as_deref() == Some(idle_since.as_str())
                });
                if same_cycle
                    && existing
                        .as_ref()
                        .is_some_and(|wakeup| wakeup.stopped && wakeup.next_due_at.is_none())
                {
                    return Ok(());
                }
                let (reminders_sent, stopped, last_reminder_at) = if same_cycle {
                    let wakeup = existing.as_ref().expect("same cycle wakeup exists");
                    (
                        wakeup.reminders_sent,
                        wakeup.reminders_sent >= DEFAULT_MASTER_REMINDER_LIMIT,
                        if wakeup.reminders_sent == 0 {
                            None
                        } else {
                            wakeup.last_reminder_at.clone()
                        },
                    )
                } else {
                    (0, false, None)
                };
                let next_due_at = if reminders_sent == 0 {
                    add_seconds(&idle_since, DEFAULT_BATCH_WINDOW_SECONDS)?
                } else if let Some(last_reminder_at) = last_reminder_at.as_deref() {
                    add_seconds(last_reminder_at, DEFAULT_BATCH_WINDOW_SECONDS)?
                } else {
                    existing
                        .as_ref()
                        .and_then(|wakeup| wakeup.next_due_at.clone())
                        .ok_or_else(|| {
                            CommError::new(
                                "wakeup_schedule_missing",
                                format!("master wakeup has no next due time: {key}"),
                            )
                        })?
                };
                let valid = same_cycle
                    && existing.as_ref().is_some_and(|wakeup| {
                        wakeup.next_due_at.as_deref() == Some(next_due_at.as_str())
                            && wakeup.reminders_sent == reminders_sent
                            && wakeup.stopped == stopped
                            && wakeup.last_reminder_at == last_reminder_at
                    });
                if valid {
                    return Ok(());
                }
                WakeupRecord {
                    address: current.address(),
                    identity_origin: None,
                    idle_since: Some(idle_since),
                    reminders_sent,
                    next_due_at: Some(next_due_at),
                    stopped,
                    last_reminder_at,
                }
            }
        };
        self.commit("wakeup.updated", serde_json::to_value(&wakeup).unwrap())?;
        Ok(())
    }
}
