use super::*;
pub(super) fn worker_idle_message(
    current: &AgentRecord,
    master_address: Address,
    transition_at: &str,
) -> MessageRequest {
    MessageRequest {
        from: current.address(),
        to: master_address,
        title: format!("worker idle: {}", current.agent_id),
        priority: "p2".into(),
        body: format!(
            "{} entered idle; inspect Appsdk mailbox facts for the latest result",
            current.agent_id
        ),
        delivery_mode: Some("idle".into()),
        coalesce_key: Some(format!("idle:{}", current.address().key())),
        issue_id: None,
        conversation_id: None,
        message_id: Some(worker_idle_message_id(current, transition_at)),
        created_at: Some(transition_at.into()),
        adapter_id: None,
    }
}

pub(super) fn worker_idle_message_id(current: &AgentRecord, transition_at: &str) -> String {
    worker_idle_message_id_for_address(&current.address(), transition_at)
}

pub(super) fn worker_idle_message_id_for_address(address: &Address, transition_at: &str) -> String {
    let address_key = address.key();
    format!(
        "worker-idle:{}",
        structured_key(&[&address_key, transition_at])
    )
}

pub(super) fn worker_idle_signal_key(address: &Address) -> String {
    format!("worker-idle:{}", address.key())
}

pub(super) fn worker_idle_coalesce_key(address: &Address) -> String {
    format!("idle:{}", address.key())
}

pub(super) fn migrate_wake_signal(
    signal: &MasterWakeSignal,
    from: &Address,
    to: &Address,
) -> MasterWakeSignal {
    let mut migrated = signal.clone();
    if migrated.source.as_ref() == Some(from) {
        migrated.source = Some(to.clone());
        if migrated.kind == "worker_idle" {
            let pre_remap_key = migrated.key.clone();
            let pre_remap_signal_id = migrated.signal_id.clone();
            migrated.identity_key.get_or_insert(pre_remap_key);
            migrated
                .identity_signal_id
                .get_or_insert(pre_remap_signal_id);
            migrated.key = worker_idle_signal_key(to);
            migrated.signal_id = worker_idle_message_id_for_address(to, &migrated.observed_at);
        }
    }
    migrated
}

pub(super) fn migrate_wake_signal_map(
    signals: &BTreeMap<String, MasterWakeSignal>,
    from: &Address,
    to: &Address,
) -> CommResult<BTreeMap<String, MasterWakeSignal>> {
    let mut migrated = BTreeMap::new();
    for signal in signals.values() {
        let signal = migrate_wake_signal(signal, from, to);
        if migrated.insert(signal.key.clone(), signal).is_some() {
            return Err(CommError::new(
                "event_data_invalid",
                "agent rebound creates duplicate master wake signal keys",
            ));
        }
    }
    Ok(migrated)
}

pub(super) fn insert_identity_update(
    updates: &mut BTreeMap<String, String>,
    from: String,
    to: String,
    kind: &str,
) -> CommResult<()> {
    if from == to {
        return Ok(());
    }
    if let Some(existing) = updates.insert(from.clone(), to.clone()) {
        if existing != to {
            return Err(CommError::new(
                "event_data_invalid",
                format!("agent rebound creates conflicting {kind} identity mappings: {from}"),
            ));
        }
    }
    Ok(())
}

pub(super) fn master_wake_covers_notification(
    signal: &MasterWakeSignal,
    notification: &NotificationRecord,
) -> bool {
    let worker_idle = signal.kind == "worker_idle"
        && signal.source.as_ref().is_some_and(|source| {
            notification.coalesce_key.as_deref() == Some(worker_idle_coalesce_key(source).as_str())
        });
    let bug = signal.issue_id.as_deref().is_some_and(|issue_id| {
        notification.issue_id.as_deref() == Some(issue_id)
            && notification.coalesce_key.as_deref() == Some("bug")
    });
    worker_idle || bug
}

pub(super) fn bug_wake_signal_key(bug_id: &str) -> String {
    format!("bug:{}", bug_id)
}

pub(super) fn bug_wake_signal(bug: &BugRecord, direct_dispatched: bool) -> MasterWakeSignal {
    MasterWakeSignal {
        signal_id: format!("bug:{}:{}:{}", bug.bug_id, bug.status, bug.updated_at),
        key: bug_wake_signal_key(&bug.bug_id),
        identity_key: None,
        identity_signal_id: None,
        kind: "bug".into(),
        title: format!("bug {}: {}", bug.status, bug.title),
        priority: bug.priority.clone(),
        summary: bug.description.clone(),
        issue_id: Some(bug.bug_id.clone()),
        source: Some(bug.reporter.clone()),
        observed_at: bug.updated_at.clone(),
        direct_dispatched,
    }
}

pub(super) fn loop_error_wake_signal(loop_record: &LoopRecord, error: &ErrorRecord) -> MasterWakeSignal {
    MasterWakeSignal {
        signal_id: format!("loop-error:{}:{}", loop_record.loop_id, error.at),
        key: format!("loop-error:{}", loop_record.loop_id),
        identity_key: None,
        identity_signal_id: None,
        kind: "loop_error".into(),
        title: format!("loop blocked: {}", loop_record.loop_id),
        priority: Priority::P1,
        summary: format!("{}: {}", error.code, error.message),
        issue_id: None,
        source: None,
        observed_at: error.at.clone(),
        direct_dispatched: false,
    }
}

pub(super) fn validate_master_wake_signal_request(request: &MasterWakeSignalRequest) -> CommResult<()> {
    validate_non_empty(&request.key, "key")?;
    validate_non_empty(&request.kind, "kind")?;
    validate_non_empty(&request.title, "title")?;
    validate_non_empty(&request.summary, "summary")?;
    if request.title.chars().count() > 200 {
        return Err(CommError::new(
            "master_wake_title_too_long",
            "master wake signal title must be at most 200 characters",
        ));
    }
    if let Some(signal_id) = request.signal_id.as_deref() {
        validate_non_empty(signal_id, "signalId")?;
    }
    if let Some(source) = request.source.as_ref() {
        validate_address(source)?;
    }
    if let Some(observed_at) = request.observed_at.as_deref() {
        validate_time(observed_at)?;
    }
    Ok(())
}

pub(super) fn master_wake_signal_matches(left: &MasterWakeSignal, right: &MasterWakeSignal) -> bool {
    master_wake_signal_identity_matches(left, right)
        && left.direct_dispatched == right.direct_dispatched
}

pub(super) fn master_wake_signal_identity_matches(left: &MasterWakeSignal, right: &MasterWakeSignal) -> bool {
    left.signal_id == right.signal_id
        && left.key == right.key
        && left.kind == right.kind
        && left.title == right.title
        && left.priority == right.priority
        && left.summary == right.summary
        && left.issue_id == right.issue_id
        && left.source == right.source
        && left.observed_at == right.observed_at
}

pub(super) fn master_wake_accumulator_matches(
    left: &MasterWakeAccumulator,
    right: &MasterWakeAccumulator,
) -> bool {
    left.address == right.address
        && left.generation == right.generation
        && left.pending == right.pending
        && left.first_observed_at == right.first_observed_at
        && left.last_observed_at == right.last_observed_at
        && left.next_due_at == right.next_due_at
        && left.reminders_sent == right.reminders_sent
        && left.stopped == right.stopped
        && left.last_briefing_generation == right.last_briefing_generation
        && left.last_briefing_at == right.last_briefing_at
        && left.held == right.held
        && left.signals.len() == right.signals.len()
        && left.signals.iter().all(|(key, signal)| {
            right
                .signals
                .get(key)
                .is_some_and(|candidate| master_wake_signal_matches(signal, candidate))
        })
        && left.consumed_signals.len() == right.consumed_signals.len()
        && left.consumed_signals.iter().all(|(key, signal)| {
            right
                .consumed_signals
                .get(key)
                .is_some_and(|candidate| master_wake_signal_matches(signal, candidate))
        })
}

pub(super) fn master_wake_message_identity(
    accumulator: &MasterWakeAccumulator,
    reminder: u8,
) -> (String, String) {
    let identity_address = accumulator.identity_address().key().to_owned();
    let generation = accumulator.generation.to_string();
    let reminder = reminder.to_string();
    let cycle = structured_key(&[&identity_address, &generation, &reminder]);
    let conversation = structured_key(&[&identity_address, &generation]);
    (
        format!("master-wake-message-{cycle}"),
        format!("master-wake-conversation-{conversation}"),
    )
}

pub(super) fn master_wake_direct_message_identity(
    accumulator: &MasterWakeAccumulator,
    signal: &MasterWakeSignal,
) -> (String, String) {
    let identity_address = accumulator.identity_address().key().to_owned();
    let (signal_key, signal_id) = master_wake_signal_identity(signal);
    let identity = structured_key(&[&identity_address, &signal_key, &signal_id]);
    let conversation = structured_key(&[&identity_address, &signal_key]);
    (
        format!("master-wake-signal-message-{identity}"),
        format!("master-wake-signal-conversation-{conversation}"),
    )
}

/// Signal key/id as they were when the signal's messages were minted. Only
/// worker-sourced signals remap their key/id on rebind, so the pinned pre-remap
/// pair reproduces the identity that existing messages were stored under.
pub(super) fn master_wake_signal_identity(signal: &MasterWakeSignal) -> (String, String) {
    (
        signal
            .identity_key
            .clone()
            .unwrap_or_else(|| signal.key.clone()),
        signal
            .identity_signal_id
            .clone()
            .unwrap_or_else(|| signal.signal_id.clone()),
    )
}

pub(super) fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

pub(super) fn message_matches_request(
    existing: &MessageRecord,
    request: &MessageRequest,
    priority: &Priority,
    delivery_mode: &DeliveryMode,
    adapter_id: &str,
) -> bool {
    existing.from == request.from
        && existing.to == request.to
        && existing.title == request.title
        && existing.priority == *priority
        && existing.body == request.body
        && existing.delivery_mode == *delivery_mode
        && existing.coalesce_key == request.coalesce_key
        && existing.issue_id == request.issue_id
        && existing.adapter_id == adapter_id
        && request
            .conversation_id
            .as_ref()
            .map_or(true, |conversation_id| {
                existing.conversation_id == *conversation_id
            })
        && request
            .created_at
            .as_deref()
            .map(validate_time)
            .transpose()
            .ok()
            .flatten()
            .map_or(true, |created_at| existing.created_at == created_at)
}

pub(super) fn validate_loop_request(request: &LoopRequest) -> CommResult<()> {
    validate_non_empty(&request.loop_id, "loopId")?;
    validate_non_empty(&request.kind, "kind")?;
    validate_address(&request.owner)?;
    for (value, name) in [
        (&request.trigger, "trigger"),
        (&request.work, "work"),
        (&request.gate, "gate"),
        (&request.state, "state"),
        (&request.stop, "stop"),
    ] {
        validate_non_empty(value, name)?;
    }
    if request.max_iterations == 0 {
        return Err(CommError::new(
            "loop_max_iterations_invalid",
            "maxIterations must be positive",
        ));
    }
    Ok(())
}

pub(super) fn validate_resolution_evidence(evidence: Option<&Value>) -> CommResult<Value> {
    let evidence = evidence.ok_or_else(|| {
        CommError::new(
            "bug_resolution_evidence_required",
            "resolving or closing a bug requires fix, verification and merge evidence",
        )
    })?;
    let object = evidence.as_object().ok_or_else(|| {
        CommError::new(
            "bug_resolution_evidence_invalid",
            "bug resolution evidence must be a JSON object",
        )
    })?;
    if object.is_empty() {
        return Err(CommError::new(
            "bug_resolution_evidence_invalid",
            "bug resolution evidence must contain fix, verification and merge evidence",
        ));
    }
    for field in ["fix", "verification", "merge"] {
        let value = object.get(field).ok_or_else(|| {
            CommError::new(
                "bug_resolution_evidence_required",
                format!("bug resolution evidence is missing {field}"),
            )
        })?;
        if value.is_null() || value.as_str().is_some_and(|text| text.trim().is_empty()) {
            return Err(CommError::new(
                "bug_resolution_evidence_invalid",
                format!("bug resolution evidence {field} must be non-empty"),
            ));
        }
        if !is_valid_loop_evidence_value(value) {
            return Err(CommError::new(
                "bug_resolution_evidence_invalid",
                format!("bug resolution evidence {field} must identify a recognized result"),
            ));
        }
    }
    Ok(evidence.clone())
}

pub(super) fn validate_bug_loop_binding(
    loop_record: &LoopRecord,
    bug_id: &str,
    scope_id: &str,
    owner: &Address,
) -> CommResult<()> {
    let expected_loop_id = format!("bug-loop-{bug_id}");
    let semantic_match = loop_record.kind == "bug"
        && loop_record.owner == *owner
        && loop_record.owner.scope_id == scope_id
        && loop_record.trigger == BUG_LOOP_TRIGGER
        && loop_record.work == BUG_LOOP_WORK
        && loop_record.gate == BUG_LOOP_GATE
        && loop_record.state == BUG_LOOP_STATE
        && loop_record.stop == BUG_LOOP_STOP;
    if loop_record.loop_id != expected_loop_id || !semantic_match {
        return Err(CommError::new(
            "bug_loop_conflict",
            format!(
                "loop is not the deterministic bug loop: {}",
                loop_record.loop_id
            ),
        ));
    }
    Ok(())
}

pub(super) fn validate_loop_completion_evidence(evidence: Option<&Value>) -> CommResult<Value> {
    let evidence = evidence.ok_or_else(|| {
        CommError::new(
            "loop_gate_evidence_required",
            "completing a loop requires gate evidence",
        )
    })?;
    let object = evidence.as_object().ok_or_else(|| {
        CommError::new(
            "loop_gate_evidence_invalid",
            "loop gate evidence must be a JSON object",
        )
    })?;
    let mut found_result = false;
    for field in ["gate", "verification"] {
        let Some(value) = object.get(field) else {
            continue;
        };
        found_result = true;
        if !is_valid_loop_evidence_value(value) {
            return Err(CommError::new(
                "loop_gate_evidence_invalid",
                "loop gate evidence must identify a recognized passing gate and verification results",
            ));
        }
    }
    if !found_result {
        return Err(CommError::new(
            "loop_gate_evidence_required",
            "loop gate evidence must identify a non-empty gate or verification result",
        ));
    }
    Ok(evidence.clone())
}

pub(super) fn is_valid_loop_evidence_value(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
        Value::String(text) => is_valid_descriptive_evidence_string(text),
        Value::Array(values) => {
            !values.is_empty() && values.iter().all(is_valid_loop_evidence_value)
        }
        Value::Object(values) => {
            if values.is_empty() {
                return false;
            }
            if !values.keys().any(|key| is_loop_evidence_result_field(key)) {
                return false;
            }
            values.iter().all(|(key, value)| {
                if is_loop_evidence_result_field(key) {
                    is_valid_loop_evidence_result_field(key, value)
                } else {
                    is_valid_loop_evidence_metadata(value)
                }
            })
        }
    }
}

pub(super) fn is_loop_evidence_result_field(key: &str) -> bool {
    matches!(
        key,
        "status" | "result" | "outcome" | "state" | "passed" | "success" | "ok" | "verified"
    )
}

pub(super) fn is_valid_loop_evidence_result_field(key: &str, value: &Value) -> bool {
    match key {
        "passed" | "success" | "ok" | "verified" => matches!(value, Value::Bool(true)),
        "status" | "result" | "outcome" | "state" => is_valid_loop_evidence_result_value(value),
        _ => false,
    }
}

pub(super) fn is_valid_loop_evidence_result_value(value: &Value) -> bool {
    match value {
        Value::String(text) => {
            let normalized = text.trim().to_ascii_lowercase();
            matches!(
                normalized.as_str(),
                "passed" | "pass" | "success" | "ok" | "verified" | "true"
            )
        }
        Value::Array(values) => {
            !values.is_empty() && values.iter().all(is_valid_loop_evidence_result_value)
        }
        Value::Object(values) => {
            !values.is_empty()
                && values.keys().any(|key| is_loop_evidence_result_field(key))
                && values.iter().all(|(key, value)| {
                    if is_loop_evidence_result_field(key) {
                        is_valid_loop_evidence_result_field(key, value)
                    } else {
                        is_valid_loop_evidence_metadata(value)
                    }
                })
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

pub(super) fn is_valid_loop_evidence_metadata(value: &Value) -> bool {
    match value {
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(values) => {
            !values.is_empty() && values.iter().all(is_valid_loop_evidence_metadata)
        }
        Value::Object(values) => {
            !values.is_empty()
                && values.iter().all(|(key, value)| {
                    if is_loop_evidence_result_field(key) {
                        is_valid_loop_evidence_result_field(key, value)
                    } else {
                        is_valid_loop_evidence_metadata(value)
                    }
                })
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => true,
    }
}

pub(super) fn is_valid_descriptive_evidence_string(text: &str) -> bool {
    let normalized = text.trim().to_ascii_lowercase();
    !normalized.is_empty()
        && !matches!(
            normalized.as_str(),
            "unknown"
                | "pending"
                | "failed"
                | "failure"
                | "fail"
                | "error"
                | "invalid"
                | "false"
                | "null"
                | "unverified"
                | "not_run"
                | "not run"
                | "timeout"
                | "blocked"
        )
}

pub(super) fn validate_address(address: &Address) -> CommResult<()> {
    validate_non_empty(&address.scope_id, "address.scopeId")?;
    validate_non_empty(&address.session_id, "address.sessionId")
}

pub(super) fn validate_non_empty(value: &str, name: &str) -> CommResult<()> {
    if value.trim().is_empty() {
        return Err(CommError::new(
            "invalid_request",
            format!("{name} must be non-empty"),
        ));
    }
    Ok(())
}

pub(super) fn decode<T: DeserializeOwned>(value: &Value, name: &str) -> CommResult<T> {
    serde_json::from_value(value.clone())
        .map_err(|error| CommError::new("invalid_request", format!("{name}: {error}")))
}

pub(super) fn require_event_field<'a>(data: &'a Value, field: &str) -> CommResult<&'a Value> {
    data.get(field).ok_or_else(|| {
        CommError::new(
            "event_data_invalid",
            format!("master wake event field is missing: {field}"),
        )
    })
}

pub(super) fn decode_master_wake_accumulator_event(data: &Value) -> CommResult<MasterWakeAccumulator> {
    for field in [
        "address",
        "generation",
        "pending",
        "firstObservedAt",
        "lastObservedAt",
        "nextDueAt",
        "remindersSent",
        "stopped",
        "lastBriefingGeneration",
        "lastBriefingAt",
        "held",
        "signals",
        "consumedSignals",
    ] {
        require_event_field(data, field)?;
    }
    decode(data, "master wake")
}

pub(super) fn validate_time(value: &str) -> CommResult<String> {
    let parsed = parse_time(value)?;
    Ok(parsed.to_rfc3339_opts(SecondsFormat::Millis, true))
}

pub(super) fn parse_time(value: &str) -> CommResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|error| CommError::new("invalid_timestamp", error.to_string()))
}

pub(super) fn add_seconds(value: &str, seconds: i64) -> CommResult<String> {
    Ok((parse_time(value)? + Duration::seconds(seconds))
        .to_rfc3339_opts(SecondsFormat::Millis, true))
}

pub(super) fn add_millis(value: &str, millis: i64) -> CommResult<String> {
    Ok((parse_time(value)? + Duration::milliseconds(millis))
        .to_rfc3339_opts(SecondsFormat::Millis, true))
}

pub(super) fn priority_then_time_bug(left: &BugRecord, right: &BugRecord) -> Ordering {
    left.priority
        .cmp(&right.priority)
        .then_with(|| left.created_at.cmp(&right.created_at))
        .then_with(|| left.bug_id.cmp(&right.bug_id))
}

pub(super) fn bug_loop_matches(loop_record: &LoopRecord, owner: &Address) -> bool {
    loop_record.kind == "bug"
        && loop_record.owner == *owner
        && loop_record.trigger == "event:bug.reported"
        && loop_record.work == "triage -> fix in an independent worktree"
        && loop_record.gate == "project verification and review"
        && loop_record.state == "persist bug evidence and next action"
        && loop_record.stop == "resolved, merged, and reporter notified"
}

pub(super) fn message_notification_key_for(message: &MessageRecord) -> String {
    structured_key(&[
        &message.from.key(),
        &message.to.key(),
        &message.adapter_id,
        message.coalesce_key.as_deref().unwrap_or("notification"),
    ])
}

pub(super) fn wakeup_message_identity(
    wakeup: &WakeupRecord,
    reminder_number: u8,
) -> CommResult<(String, String)> {
    let identity_address = wakeup
        .identity_origin
        .as_ref()
        .unwrap_or(&wakeup.address)
        .key();
    let idle_since = wakeup.idle_since.as_deref().ok_or_else(|| {
        CommError::new(
            "wakeup_cycle_missing",
            format!("master wakeup has no idle cycle: {}", wakeup.address.key()),
        )
    })?;
    let reminder_number = reminder_number.to_string();
    let cycle = structured_key(&[identity_address.as_str(), idle_since, &reminder_number]);
    let conversation = structured_key(&[identity_address.as_str(), idle_since]);
    Ok((
        format!("wakeup-message-{cycle}"),
        format!("wakeup-conversation-{conversation}"),
    ))
}

pub(super) fn synchronize_master_wakeup(
    wakeup: WakeupRecord,
    action: &str,
    accumulator: &MasterWakeAccumulator,
) -> WakeupRecord {
    let mut synchronized = wakeup;
    if action == "schedule" {
        synchronized.next_due_at = accumulator.next_due_at.clone();
        synchronized.stopped = false;
        synchronized.reminders_sent = 0;
        synchronized.last_reminder_at = None;
    } else {
        synchronized.next_due_at = None;
        synchronized.stopped = true;
    }
    synchronized
}

pub(super) fn wakeup_message_matches(existing: &MessageRecord, expected: &MessageRecord) -> bool {
    existing.protocol == expected.protocol
        && existing.message_id == expected.message_id
        && existing.conversation_id == expected.conversation_id
        && existing.from == expected.from
        && existing.to == expected.to
        && existing.title == expected.title
        && existing.priority == expected.priority
        && existing.body == expected.body
        && existing.delivery_mode == expected.delivery_mode
        && existing.coalesce_key == expected.coalesce_key
        && existing.issue_id == expected.issue_id
        && existing.adapter_id == expected.adapter_id
        && existing.route.mode == expected.route.mode
        && existing.route.same_appserver == expected.route.same_appserver
        && existing.route.same_project == expected.route.same_project
        && existing.route.source_role == expected.route.source_role
        && existing.route.target_role == expected.route.target_role
}

pub(super) fn next_loop_phase(loop_record: &mut LoopRecord) -> CommResult<String> {
    let phase = match loop_record.phase.as_str() {
        "discover" => "hand_off",
        "hand_off" => "verify",
        "verify" => "persist",
        "persist" => "schedule",
        "schedule" => {
            loop_record.iteration += 1;
            if loop_record.iteration >= loop_record.max_iterations {
                loop_record.status = "stopped".into();
                "max_iterations"
            } else {
                "discover"
            }
        }
        other => {
            return Err(CommError::new(
                "loop_phase_invalid",
                format!("invalid loop phase: {other}"),
            ))
        }
    };
    Ok(phase.into())
}

pub(super) fn default_mailbox_adapter() -> AdapterRecord {
    AdapterRecord {
        adapter_id: "mailbox".into(),
        kind: "mailbox".into(),
        target: None,
        enabled: true,
        execute: false,
        recipient: None,
        registered_at: "built-in".into(),
    }
}

pub(super) fn adapter_error(error: &CommError, adapter_id: &str, operation: &str) -> CommError {
    let mut enriched = error.clone();
    enriched.context = json!({
        "adapterId": adapter_id,
        "operation": operation,
        "cause": error.context
    });
    enriched
}

pub(super) fn with_secondary_error(mut primary: CommError, secondary: CommError, stage: &str) -> CommError {
    primary.context = json!({
        "cause": primary.context,
        "stage": stage,
        "secondaryError": {
            "code": secondary.code,
            "message": secondary.message,
            "context": secondary.context
        }
    });
    primary
}

pub(super) fn adapter_error_record(error: &CommError, adapter_id: &str, operation: &str) -> ErrorRecord {
    ErrorRecord {
        code: error.code.clone(),
        message: error.message.clone(),
        context: json!({
            "adapterId": adapter_id,
            "operation": operation,
            "cause": error.context
        }),
        at: now(),
    }
}

pub(super) fn new_delivery_attempt(
    adapter_id: &str,
    operation: &str,
    batch_id: Option<&str>,
) -> DeliveryAttempt {
    new_delivery_attempt_at(adapter_id, operation, batch_id, &now())
}

pub(super) fn new_delivery_attempt_at(
    adapter_id: &str,
    operation: &str,
    batch_id: Option<&str>,
    started_at: &str,
) -> DeliveryAttempt {
    DeliveryAttempt {
        attempt_id: new_id("attempt"),
        operation: operation.into(),
        adapter_id: adapter_id.into(),
        started_at: started_at.into(),
        batch_id: batch_id.map(str::to_owned),
    }
}

pub(super) fn validate_terminal_attempt(
    notification: &NotificationRecord,
    event_attempt_id: Option<&str>,
    operation: &str,
) -> CommResult<()> {
    match (notification.delivery_attempt.as_ref(), event_attempt_id) {
        (None, None) => Ok(()),
        (Some(attempt), Some(attempt_id))
            if attempt.attempt_id == attempt_id && attempt.operation == operation =>
        {
            Ok(())
        }
        (Some(attempt), Some(attempt_id)) => Err(CommError::new(
            "delivery_attempt_mismatch",
            format!(
                "{operation} attempt {attempt_id} does not match pending attempt {}",
                attempt.attempt_id
            ),
        )),
        (Some(attempt), None) => Err(CommError::new(
            "delivery_attempt_mismatch",
            format!(
                "{operation} is missing pending attempt {}",
                attempt.attempt_id
            ),
        )),
        (None, Some(attempt_id)) => Err(CommError::new(
            "delivery_attempt_mismatch",
            format!("{operation} references unknown attempt {attempt_id}"),
        )),
    }
}

pub(super) fn format_priority(priority: &Priority) -> String {
    match priority {
        Priority::P0 => "p0",
        Priority::P1 => "p1",
        Priority::P2 => "p2",
        Priority::P3 => "p3",
    }
    .into()
}

pub(super) fn structured_key(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| format!("{}#{}", part.len(), part))
        .collect::<Vec<_>>()
        .join("|")
}

pub(super) fn default_max_iterations() -> u32 {
    100
}

pub(super) fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub(super) fn new_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = ID_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    format!("{prefix}-{nanos:x}-{counter:x}")
}
