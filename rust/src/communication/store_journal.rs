use super::helpers::*;
use super::validation::*;
use super::*;

impl CommunicationStore {
    pub(super) fn commit(&mut self, kind: &str, data: Value) -> CommResult<String> {
        if kind == "agent.rebound" {
            self.validate_agent_rebound_event(&data)?;
        }
        let event = EventRecord {
            protocol: PROTOCOL.into(),
            event_id: new_id("event"),
            at: now(),
            kind: kind.into(),
            data,
        };
        let line = serde_json::to_string(&event)
            .map_err(|error| CommError::new("journal_encode_failed", error.to_string()))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.mailbox_path)
            .map_err(|error| {
                CommError::new(
                    "journal_write_failed",
                    format!("{}: {error}", self.mailbox_path.display()),
                )
            })?;
        let record = format!("{line}\n");
        file.write_all(record.as_bytes())
            .map_err(|error| CommError::new("journal_write_failed", error.to_string()))?;
        file.sync_data()
            .map_err(|error| CommError::new("journal_sync_failed", error.to_string()))?;
        self.apply_event(&event)?;
        Ok(event.event_id)
    }

    pub(super) fn record_error_event(&mut self, error: &CommError) -> CommResult<()> {
        let record = ErrorRecord {
            code: error.code.clone(),
            message: error.message.clone(),
            context: error.context.clone(),
            at: now(),
        };
        self.commit("error.recorded", serde_json::to_value(record).unwrap())
            .map(|_| ())
    }

    pub(super) fn replay(&mut self) -> CommResult<()> {
        let text = match fs::read_to_string(&self.mailbox_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(CommError::new(
                    "journal_read_failed",
                    format!("{}: {error}", self.mailbox_path.display()),
                ))
            }
        };
        if !text.is_empty() && !text.ends_with('\n') {
            return Err(CommError::new(
                "journal_corrupt",
                "communication JSONL must end with a newline",
            ));
        }
        let mut event_ids = BTreeMap::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                return Err(CommError::new(
                    "journal_corrupt",
                    format!("empty JSONL line at line {}", index + 1),
                ));
            }
            let event: EventRecord = serde_json::from_str(line).map_err(|error| {
                CommError::new(
                    "journal_corrupt",
                    format!("invalid JSONL at line {}: {error}", index + 1),
                )
            })?;
            validate_event_envelope(&event).map_err(|error| {
                CommError::new(
                    "journal_corrupt",
                    format!("invalid envelope at line {}: {error}", index + 1),
                )
            })?;
            if let Some(previous_line) = event_ids.insert(event.event_id.clone(), index + 1) {
                return Err(CommError::new(
                    "journal_corrupt",
                    format!(
                        "duplicate eventId {} at line {} (already present at line {})",
                        event.event_id,
                        index + 1,
                        previous_line
                    ),
                ));
            }
            if event.protocol != PROTOCOL {
                return Err(CommError::new(
                    "journal_protocol_mismatch",
                    format!("unsupported communication protocol at line {}", index + 1),
                ));
            }
            self.apply_event(&event).map_err(|error| {
                CommError::new(
                    "journal_corrupt",
                    format!("invalid event at line {}: {}", index + 1, error.message),
                )
            })?;
        }
        for notification in self.projection.notifications.values_mut() {
            if notification.delivery_attempt.is_some() {
                notification.status = "unknown".into();
            }
        }
        Ok(())
    }

    pub(super) fn replay_identity_only(&mut self) -> CommResult<()> {
        let text = fs::read_to_string(&self.mailbox_path).map_err(|error| {
            CommError::new(
                "journal_read_failed",
                format!("{}: {error}", self.mailbox_path.display()),
            )
        })?;
        if !text.is_empty() && !text.ends_with('\n') {
            return Err(CommError::new(
                "journal_corrupt",
                "communication JSONL must end with a newline",
            ));
        }
        let mut event_ids = BTreeMap::new();
        for (index, line) in text.lines().enumerate() {
            let line_number = index + 1;
            if line.trim().is_empty() {
                return Err(CommError::new(
                    "journal_corrupt",
                    format!("empty JSONL line at line {line_number}"),
                ));
            }
            let event: EventRecord = serde_json::from_str(line).map_err(|error| {
                CommError::new(
                    "journal_corrupt",
                    format!("invalid JSONL at line {line_number}: {error}"),
                )
            })?;
            validate_event_envelope(&event).map_err(|error| {
                CommError::new(
                    "journal_corrupt",
                    format!("invalid envelope at line {line_number}: {error}"),
                )
            })?;
            if event.protocol != PROTOCOL {
                return Err(CommError::new(
                    "journal_protocol_mismatch",
                    format!("unsupported communication protocol at line {line_number}"),
                ));
            }
            if event_ids
                .insert(event.event_id.clone(), line_number)
                .is_some()
            {
                return Err(CommError::new(
                    "journal_corrupt",
                    format!("duplicate eventId at line {line_number}"),
                ));
            }
            match event.kind.as_str() {
                "scope.registered" | "scope.unregistered" | "agent.registered" | "agent.state"
                | "agent.refreshed" | "agent.rebound" => {
                    self.apply_event(&event).map_err(|error| {
                        CommError::new(
                            "journal_corrupt",
                            format!(
                                "invalid identity event at line {line_number}: {}",
                                error.message
                            ),
                        )
                    })?
                }
                "discovery.pending" => {
                    let _: DiscoveryPendingRecord = decode(&event.data, "discovery pending")
                        .map_err(|error| {
                            CommError::new(
                                "journal_corrupt",
                                format!(
                                    "invalid discovery pending event at line {line_number}: {}",
                                    error.message
                                ),
                            )
                        })?;
                }
                "discovery.reconciled" => {
                    if event
                        .data
                        .get("pendingId")
                        .and_then(Value::as_str)
                        .is_none_or(|value| value.trim().is_empty())
                    {
                        return Err(CommError::new(
                            "journal_corrupt",
                            format!(
                                "invalid discovery reconciled event at line {line_number}: pendingId missing"
                            ),
                        ));
                    }
                }
                "adapter.registered"
                | "message.created"
                | "message.delivery_attempt"
                | "message.state"
                | "notification.queued"
                | "notification.superseded"
                | "notification.delivery_attempt"
                | "notification.emitted"
                | "notification.batch_emitted"
                | "notification.delivery_failed"
                | "wakeup.updated"
                | "master_wake.updated"
                | "master_wake.decided"
                | "master_wake.briefing"
                | "wakeup.reminder"
                | "bug.reported"
                | "bug.updated"
                | "loop.created"
                | "loop.updated"
                | "error.recorded" => {}
                other => {
                    return Err(CommError::new(
                        "journal_unknown_event",
                        format!("unknown communication event: {other}"),
                    ));
                }
            }
        }
        Ok(())
    }

    pub(super) fn apply_master_wake_decided_event(&mut self, data: &Value) -> CommResult<()> {
        let accumulator =
            decode_master_wake_accumulator_event(require_event_field(data, "accumulator")?)?;
        let action = require_event_field(data, "action")?
            .as_str()
            .ok_or_else(|| {
                CommError::new("event_data_invalid", "master wake action is not a string")
            })?;
        let at = require_event_field(data, "at")?.as_str().ok_or_else(|| {
            CommError::new(
                "event_data_invalid",
                "master wake decision at is not a string",
            )
        })?;
        validate_time(at)?;
        let previous = self
            .projection
            .master_wake
            .get(&accumulator.address.key())
            .cloned()
            .ok_or_else(|| {
                CommError::new(
                    "event_data_invalid",
                    format!(
                        "master wake decision has no prior accumulator: {}",
                        accumulator.address.key()
                    ),
                )
            })?;
        let action = action.trim().to_ascii_lowercase();
        let generation_is_new_schedule = action == "schedule"
            && previous.pending
            && previous
                .generation
                .checked_add(1)
                .is_some_and(|generation| generation == accumulator.generation);
        let generation_mismatch = if action == "schedule" && previous.pending {
            !generation_is_new_schedule
        } else {
            previous.generation != accumulator.generation
        };
        if generation_mismatch {
            return Err(CommError::new(
                "event_data_invalid",
                "master wake decision generation does not match prior accumulator",
            ));
        }
        match action.as_str() {
            "hold" => {
                if !accumulator.held || accumulator.next_due_at.is_some() {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "held master wake decision must remain held without a due time",
                    ));
                }
            }
            "dispatch" | "handled" | "complete" | "completed" => {
                if accumulator.pending || !accumulator.signals.is_empty() {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "terminal master wake decision must clear active signals",
                    ));
                }
            }
            "schedule" => {
                if accumulator.held
                    || accumulator.stopped
                    || accumulator.reminders_sent != 0
                    || accumulator.last_briefing_generation.is_some()
                    || accumulator.last_briefing_at.is_some()
                    || (previous.pending && !generation_is_new_schedule)
                    || accumulator.pending != previous.pending
                    || accumulator.first_observed_at != previous.first_observed_at
                    || accumulator.last_observed_at != previous.last_observed_at
                    || accumulator.signals != previous.signals
                    || accumulator.consumed_signals != previous.consumed_signals
                    || (!accumulator.pending && accumulator.next_due_at.is_some())
                    || (accumulator.pending && accumulator.next_due_at.is_none())
                {
                    return Err(CommError::new(
                        "event_data_invalid",
                        "scheduled master wake decision has inconsistent state",
                    ));
                }
            }
            other => {
                return Err(CommError::new(
                    "event_data_invalid",
                    format!("unsupported master wake decision action: {other}"),
                ))
            }
        }
        let key = accumulator.address.key();
        self.projection
            .master_wake
            .insert(key.clone(), accumulator.clone());
        if let Some(wakeup) = self.projection.wakeup.get(&key).cloned() {
            let synchronized = synchronize_master_wakeup(wakeup, &action, &accumulator);
            self.projection.wakeup.insert(key, synchronized);
        }
        Ok(())
    }

    pub(super) fn apply_master_wake_briefing_event(&mut self, data: &Value) -> CommResult<()> {
        let accumulator =
            decode_master_wake_accumulator_event(require_event_field(data, "accumulator")?)?;
        let message: MessageRecord = decode(require_event_field(data, "message")?, "message")?;
        let notification: NotificationRecord =
            decode(require_event_field(data, "notification")?, "notification")?;
        let generation = require_event_field(data, "generation")?
            .as_u64()
            .ok_or_else(|| {
                CommError::new(
                    "event_data_invalid",
                    "master wake generation is not an integer",
                )
            })?;
        let reminder = require_event_field(data, "reminder")?
            .as_u64()
            .ok_or_else(|| {
                CommError::new(
                    "event_data_invalid",
                    "master wake reminder is not an integer",
                )
            })?;
        let reminder = u8::try_from(reminder).map_err(|_| {
            CommError::new("event_data_invalid", "master wake reminder is out of range")
        })?;
        if generation != accumulator.generation
            || reminder == 0
            || reminder > DEFAULT_MASTER_REMINDER_LIMIT
            || accumulator.reminders_sent != reminder
            || accumulator.last_briefing_generation != Some(generation)
        {
            return Err(CommError::new(
                "event_data_invalid",
                "master wake briefing generation or reminder is inconsistent",
            ));
        }
        let (message_id, conversation_id) = master_wake_message_identity(&accumulator, reminder);
        self.validate_master_wake_message_identity(
            &message,
            &accumulator,
            reminder,
            &accumulator.address,
            &conversation_id,
        )?;
        if message.message_id != message_id {
            return Err(CommError::new(
                "event_data_invalid",
                "master wake briefing message identity is inconsistent",
            ));
        }
        let projected_message = self
            .projection
            .messages
            .get(&message.message_id)
            .ok_or_else(|| {
                CommError::new(
                    "event_data_invalid",
                    format!("master wake briefing message is not durable: {message_id}"),
                )
            })?;
        if serde_json::to_value(projected_message).unwrap()
            != serde_json::to_value(&message).unwrap()
        {
            return Err(CommError::new(
                "event_data_invalid",
                "master wake briefing message does not match its durable record",
            ));
        }
        if notification.message_id != message.message_id
            || notification.recipient != accumulator.address
            || notification.status != "emitted"
            || notification.delivery_attempt.is_some()
        {
            return Err(CommError::new(
                "event_data_invalid",
                "master wake briefing notification is not terminal for its message",
            ));
        }
        let (notification_key, projected_notification) = self
            .projection
            .notifications
            .iter()
            .find(|(_, value)| value.notification_id == notification.notification_id)
            .or_else(|| {
                self.projection
                    .notifications
                    .iter()
                    .find(|(_, value)| value.message_id == notification.message_id)
            })
            .ok_or_else(|| {
                CommError::new(
                    "event_data_invalid",
                    format!(
                        "master wake briefing notification is not durable: {}",
                        notification.notification_id
                    ),
                )
            })?;
        if serde_json::to_value(projected_notification).unwrap()
            != serde_json::to_value(&notification).unwrap()
        {
            return Err(CommError::new(
                "event_data_invalid",
                "master wake briefing notification does not match its durable record",
            ));
        }
        if !self
            .projection
            .completed_attempts
            .contains_key(notification_key)
        {
            return Err(CommError::new(
                "event_data_invalid",
                "master wake briefing lacks a completed delivery attempt",
            ));
        }
        self.projection
            .master_wake
            .insert(accumulator.address.key(), accumulator);
        Ok(())
    }

    pub(super) fn apply_agent_rebound_event(&mut self, data: &Value) -> CommResult<()> {
        let rebound: AgentReboundEvent = decode(data, "agent rebound")?;
        for (field, address) in [
            ("from", rebound.from.address()),
            ("to", rebound.to.address()),
            ("tombstone.address", rebound.tombstone.address.clone()),
            ("tombstone.reboundTo", rebound.tombstone.rebound_to.clone()),
        ] {
            validate_address(&address).map_err(|error| {
                CommError::new(
                    "event_data_invalid",
                    format!(
                        "agent rebound {field} address is invalid: {}",
                        error.message
                    ),
                )
            })?;
        }
        let from_key = rebound.from.address().key();
        let to_key = rebound.to.address().key();

        if from_key == to_key {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound must change the session address",
            ));
        }
        if rebound.from.scope_id != rebound.to.scope_id {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound must keep the same scope",
            ));
        }
        if rebound.from.agent_id != rebound.to.agent_id
            || rebound.from.role != rebound.to.role
            || rebound.from.master_grant != rebound.to.master_grant
            || rebound.from.parent != rebound.to.parent
            || rebound.from.lease_ms != rebound.to.lease_ms
            || rebound.from.registered_at != rebound.to.registered_at
            || rebound.from.state != rebound.to.state
            || rebound.from.last_state_at != rebound.to.last_state_at
            || rebound.from.runtime_id != rebound.to.runtime_id
        {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound changed stable agent identity or logical state",
            ));
        }
        let runtime_id = rebound.from.runtime_id.as_deref().ok_or_else(|| {
            CommError::new(
                "event_data_invalid",
                "agent rebound requires a verified runtime identity",
            )
        })?;
        if rebound.tombstone.address != rebound.from.address()
            || rebound.tombstone.rebound_to != rebound.to.address()
            || rebound.tombstone.agent_id != rebound.from.agent_id
            || rebound.tombstone.runtime_id != runtime_id
            || rebound.tombstone.rebound_at != rebound.to.last_observed_at
        {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound tombstone does not match the before and after records",
            ));
        }
        validate_time(&rebound.to.last_observed_at)?;
        let expected_expires =
            add_millis(&rebound.to.last_observed_at, rebound.to.lease_ms as i64)?;
        if rebound.to.expires_at != expected_expires {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound lease expiry does not match the rebind observation time",
            ));
        }

        let scope = self
            .projection
            .scopes
            .get(&rebound.from.scope_id)
            .ok_or_else(|| {
                CommError::new("event_data_invalid", "agent rebound scope is missing")
            })?;
        if !scope.session_ids.is_empty() && !scope.session_ids.contains(&rebound.to.session_id) {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound target session is not declared in the scope",
            ));
        }
        let current = self.projection.agents.get(&from_key).ok_or_else(|| {
            CommError::new("event_data_invalid", "agent rebound source is missing")
        })?;
        if current != &rebound.from {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound source does not match the current projection",
            ));
        }
        if self.projection.agents.contains_key(&to_key)
            || self.projection.agent_tombstones.contains_key(&to_key)
            || self.projection.agent_tombstones.contains_key(&from_key)
        {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound source or target is already tombstoned or occupied",
            ));
        }
        if rebound.from.role == "master"
            && scope.master_session_id.as_deref() != Some(rebound.from.session_id.as_str())
        {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound master source does not match the scope master",
            ));
        }

        self.projection.agents.remove(&from_key);
        self.projection
            .agents
            .insert(to_key.clone(), rebound.to.clone());
        self.projection
            .agent_tombstones
            .insert(from_key.clone(), rebound.tombstone.clone());

        self.migrate_rebound_references(&rebound.from.address(), &rebound.to.address())?;

        if rebound.from.role == "master" {
            if let Some(scope) = self.projection.scopes.get_mut(&rebound.from.scope_id) {
                scope.master_session_id = Some(rebound.to.session_id.clone());
            }
        }
        Ok(())
    }

    pub(super) fn validate_agent_rebound_event(&mut self, data: &Value) -> CommResult<()> {
        let original = self.projection.clone();
        let result = self.apply_agent_rebound_event(data);
        self.projection = original;
        result
    }

    pub(super) fn migrate_rebound_references(
        &mut self,
        from: &Address,
        to: &Address,
    ) -> CommResult<()> {
        for agent in self.projection.agents.values_mut() {
            if agent.parent.as_ref() == Some(from) {
                agent.parent = Some(to.clone());
            }
        }

        for adapter in self.projection.adapters.values_mut() {
            if adapter.recipient.as_ref() == Some(from) {
                adapter.recipient = Some(to.clone());
            }
        }

        let old_messages = self.projection.messages.clone();
        let old_master_wake = self.projection.master_wake.clone();
        let mut migrated_master_wake = BTreeMap::new();
        for old_accumulator in old_master_wake.values() {
            let mut new_accumulator = old_accumulator.clone();
            if new_accumulator.address == *from {
                // The rebound address is the new delivery target; the identities of
                // already persisted wake messages stay bound to the address they
                // were minted from, so pin that origin before the address moves.
                new_accumulator
                    .identity_origin
                    .get_or_insert_with(|| old_accumulator.address.clone());
                new_accumulator.address = to.clone();
            }
            new_accumulator.signals = migrate_wake_signal_map(&old_accumulator.signals, from, to)?;
            new_accumulator.consumed_signals =
                migrate_wake_signal_map(&old_accumulator.consumed_signals, from, to)?;
            let key = new_accumulator.address.key();
            if migrated_master_wake
                .insert(key.clone(), new_accumulator)
                .is_some()
            {
                return Err(CommError::new(
                    "event_data_invalid",
                    format!("agent rebound creates duplicate master wake accumulator: {key}"),
                ));
            }
        }

        let old_wakeup = self.projection.wakeup.clone();
        let mut migrated_wakeup = BTreeMap::new();
        for old_record in old_wakeup.values() {
            let mut new_record = old_record.clone();
            if new_record.address == *from {
                new_record
                    .identity_origin
                    .get_or_insert_with(|| old_record.address.clone());
                new_record.address = to.clone();
            }
            let key = new_record.address.key();
            if migrated_wakeup.insert(key.clone(), new_record).is_some() {
                return Err(CommError::new(
                    "event_data_invalid",
                    format!("agent rebound creates duplicate wakeup record: {key}"),
                ));
            }
        }

        let messages = std::mem::take(&mut self.projection.messages);
        for (old_key, mut message) in messages {
            let old_message = old_messages.get(&old_key).ok_or_else(|| {
                CommError::new(
                    "event_data_invalid",
                    format!("message projection is missing its old record: {old_key}"),
                )
            })?;
            if message.from == *from {
                message.from = to.clone();
            }
            if message.to == *from {
                message.to = to.clone();
            }
            if message.coalesce_key.as_deref() == Some(worker_idle_coalesce_key(from).as_str())
                && message.from == *to
            {
                message.coalesce_key = Some(worker_idle_coalesce_key(to));
            }
            let new_key = message.message_id.clone();
            if self.projection.messages.insert(new_key, message).is_some() {
                return Err(CommError::new(
                    "event_data_invalid",
                    "agent rebound creates duplicate message projection keys",
                ));
            }
            if old_message.message_id != old_key {
                return Err(CommError::new(
                    "event_data_invalid",
                    "message projection key does not match message identity",
                ));
            }
        }

        for bug in self.projection.bugs.values_mut() {
            if bug.reporter == *from {
                bug.reporter = to.clone();
            }
        }
        for loop_record in self.projection.loops.values_mut() {
            if loop_record.owner == *from {
                loop_record.owner = to.clone();
            }
        }
        for batch in &mut self.projection.batches {
            if batch.recipient == *from {
                batch.recipient = to.clone();
            }
        }

        let mut notification_key_updates = BTreeMap::new();
        let notifications = std::mem::take(&mut self.projection.notifications);
        for (old_key, mut notification) in notifications {
            let old_message = old_messages.get(&notification.message_id);
            let old_notification_key = old_message.map(message_notification_key_for);
            if notification.recipient == *from {
                notification.recipient = to.clone();
            }
            if notification.coalesce_key.as_deref() == Some(worker_idle_coalesce_key(from).as_str())
                && notification.recipient == *to
            {
                notification.coalesce_key = Some(worker_idle_coalesce_key(to));
            }
            let new_key = if old_notification_key.as_deref() == Some(old_key.as_str()) {
                let message = self
                    .projection
                    .messages
                    .get(&notification.message_id)
                    .ok_or_else(|| {
                        CommError::new(
                            "event_data_invalid",
                            format!(
                                "notification message disappeared during rebind: {}",
                                notification.message_id
                            ),
                        )
                    })?;
                let new_key = message_notification_key_for(message);
                insert_identity_update(
                    &mut notification_key_updates,
                    old_key.clone(),
                    new_key.clone(),
                    "notification",
                )?;
                new_key
            } else {
                old_key
            };
            if self
                .projection
                .notifications
                .insert(new_key, notification)
                .is_some()
            {
                return Err(CommError::new(
                    "event_data_invalid",
                    "agent rebound creates duplicate notification projection keys",
                ));
            }
        }

        let completed_attempts = std::mem::take(&mut self.projection.completed_attempts);
        for (key, attempt_id) in completed_attempts {
            let key = notification_key_updates.get(&key).cloned().unwrap_or(key);
            if self
                .projection
                .completed_attempts
                .insert(key, attempt_id)
                .is_some()
            {
                return Err(CommError::new(
                    "event_data_invalid",
                    "agent rebound creates duplicate completed notification attempts",
                ));
            }
        }
        self.projection.master_wake = migrated_master_wake;
        self.projection.wakeup = migrated_wakeup;
        Ok(())
    }
}
