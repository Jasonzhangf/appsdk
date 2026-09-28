use super::*;

pub(super) fn validate_notification_objects(root: &Path) -> Result<Value, String> {
    assert_project_root_safe(root);
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    assert_no_symlink_components(root, &mailbox, "communication_mailbox");
    let raw = fs::read_to_string(&mailbox)
        .map_err(|error| format!("COMMUNICATION_MAILBOX_READ_FAILED:{error}"))?;
    let events = mailbox_events(&raw)?;
    let groups = notification_object_groups(&events)?;
    let graph = parse_graph_json(NOTIFICATION_LIFECYCLE_GRAPH)
        .map_err(|error| format!("DAGPIPE_GRAPH_INVALID:{error}"))?;
    ensure_single_source_single_sink(&graph)?;
    let mut registry = Registry::default();
    register_notification_operator(&mut registry)?;
    let capabilities = BTreeSet::new();
    let compiled = compile(graph, &registry, &capabilities)
        .map_err(|error| format!("DAGPIPE_COMPILE_FAILED:{error}"))?;
    let runtime = Runtime::new(capabilities);
    let mut objects = Vec::new();
    for (key, object_events) in groups {
        let input = notification_input(&key, &object_events)?;
        let identity = Identity {
            project_id: "appsdk".to_owned(),
            graph_id: compiled.id().to_owned(),
            graph_version: compiled.version().to_owned(),
            execution_id: format!("notification-object-{}", UtcStamp::now()),
            attempt_id: "1".to_owned(),
        };
        let mut inputs = HashMap::new();
        inputs.insert("notification_object".to_owned(), input);
        let result = runtime
            .run(&compiled, identity, inputs, &Cancellation::default())
            .map_err(|failure| format!("DAGPIPE_EXECUTION_FAILED:{failure}"))?;
        let output = result
            .outputs
            .get("notification_terminal")
            .map(|arc| arc.payload.clone())
            .ok_or_else(|| "DAGPIPE_OUTPUT_MISSING:notification_terminal".to_owned())?;
        let terminal = output
            .get("terminal")
            .cloned()
            .ok_or_else(|| format!("DAGPIPE_NOTIFICATION_TERMINAL_MISSING:{key}"))?;
        if terminal_requires_repair(&terminal) {
            return Err(format!(
                "NOTIFICATION_OBJECT_TERMINAL_MISSING:{key}:{}",
                terminal
            ));
        }
        objects.push(json!({
            "key": key,
            "single_source_single_sink": true,
            "terminal": terminal,
            "journal": result.journal,
        }));
    }
    Ok(json!({
        "graph": {"id": compiled.id(), "version": compiled.version()},
        "single_source_single_sink": true,
        "objects": objects,
    }))
}

fn mailbox_events(raw: &str) -> Result<Vec<Value>, String> {
    if !raw.is_empty() && !raw.ends_with('\n') {
        return Err("COMMUNICATION_MAILBOX_EVENT_INVALID:missing_trailing_newline".to_owned());
    }
    let schema: Value = serde_json::from_str(COMMUNICATION_EVENT_SCHEMA)
        .map_err(|error| format!("COMMUNICATION_EVENT_SCHEMA_INVALID:{error}"))?;
    let validator = jsonschema::validator_for(&schema)
        .map_err(|error| format!("COMMUNICATION_EVENT_SCHEMA_INVALID:{error}"))?;
    let valid_kinds = schema
        .pointer("/properties/kind/enum")
        .and_then(Value::as_array)
        .ok_or_else(|| "COMMUNICATION_EVENT_SCHEMA_INVALID:kind_enum".to_owned())?
        .iter()
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    let mut event_ids = BTreeSet::new();
    let mut events = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            return Err(format!(
                "COMMUNICATION_MAILBOX_EVENT_INVALID:empty_line:{}",
                index + 1
            ));
        }
        let event: Value = serde_json::from_str(line)
            .map_err(|error| format!("COMMUNICATION_MAILBOX_EVENT_INVALID:{error}"))?;
        if !valid_kinds.contains(event["kind"].as_str().unwrap_or("")) {
            return Err(format!(
                "COMMUNICATION_MAILBOX_EVENT_KIND_INVALID:{}:{}",
                index + 1,
                event["kind"].as_str().unwrap_or("")
            ));
        }
        let event_id = event
            .get("eventId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("COMMUNICATION_MAILBOX_EVENT_INVALID:eventId:{}", index + 1))?;
        if !event_ids.insert(event_id.to_owned()) {
            return Err(format!(
                "COMMUNICATION_MAILBOX_EVENT_DUPLICATE:eventId:{event_id}"
            ));
        }
        if event.get("protocol").and_then(Value::as_str) != Some("appsdk-comm/v1") {
            return Err(format!(
                "COMMUNICATION_MAILBOX_EVENT_PROTOCOL_INVALID:{}",
                index + 1
            ));
        }
        if event
            .get("at")
            .and_then(Value::as_str)
            .map_or(true, |value| value.trim().is_empty())
        {
            return Err(format!(
                "COMMUNICATION_MAILBOX_EVENT_INVALID:at:{}",
                index + 1
            ));
        }
        if let Err(error) = validator.validate(&event) {
            let kind = event["kind"].as_str().unwrap_or("");
            return Err(format!(
                "COMMUNICATION_MAILBOX_EVENT_INVALID:{}:{kind}:{error}",
                index + 1
            ));
        }
        events.push(event);
    }
    Ok(events)
}

fn notification_object_groups(events: &[Value]) -> Result<Vec<(String, Vec<Value>)>, String> {
    struct NotificationObject {
        key: String,
        message_id: String,
        notification_id: String,
        generation: Option<u64>,
        source_index: usize,
        attempt_id: Option<String>,
        terminal_count: usize,
        events: Vec<Value>,
    }

    let mut objects = Vec::<NotificationObject>::new();
    for (index, event) in events.iter().enumerate() {
        if event["kind"] == "notification.queued" {
            let key = event["data"]["key"]
                .as_str()
                .ok_or_else(|| "NOTIFICATION_QUEUED_KEY_MISSING".to_owned())?
                .to_owned();
            let notification = event["data"]["notification"]
                .as_object()
                .ok_or_else(|| "NOTIFICATION_QUEUED_NOTIFICATION_MISSING".to_owned())?;
            let message_id = notification
                .get("messageId")
                .and_then(Value::as_str)
                .ok_or_else(|| "NOTIFICATION_QUEUED_MESSAGE_MISSING".to_owned())?
                .to_owned();
            let notification_id = notification
                .get("notificationId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let generation = notification.get("generation").and_then(Value::as_u64);
            if let Some(existing) = objects
                .iter_mut()
                .rev()
                .find(|object| object.key == key && object.generation == generation)
            {
                if existing.terminal_count > 0 {
                    // A terminal closes this key/generation.  A later queue for
                    // the same logical object is a replayed source, not a new
                    // generation, and must fail closed instead of opening a
                    // second object with its own terminal.
                    return Err(format!("NOTIFICATION_OBJECT_SOURCE_DUPLICATE:{key}"));
                }
                existing.events.push(event.clone());
                // Coalesced queue replacement retargets the object at the new
                // source message; the validator must close against the latest
                // notification record, not the obsolete first queue.
                existing.message_id = message_id;
                existing.notification_id = notification_id;
                continue;
            }
            objects.push(NotificationObject {
                key,
                message_id,
                notification_id,
                generation,
                source_index: index,
                attempt_id: None,
                terminal_count: 0,
                events: vec![event.clone()],
            });
            continue;
        }

        let kind = event
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| "COMMUNICATION_EVENT_KIND_MISSING".to_owned())?;
        let data = event
            .get("data")
            .ok_or_else(|| "COMMUNICATION_EVENT_DATA_MISSING".to_owned())?;

        let keys = data
            .get("keys")
            .or_else(|| data.get("notificationKeys"))
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut notification_ids = data
            .get("notificationIds")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(notification_id) = data.get("notificationId").and_then(Value::as_str) {
            notification_ids.push(notification_id.to_owned());
        }
        let mut message_ids = data
            .get("messageId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .into_iter()
            .collect::<Vec<_>>();
        if let Some(batch) = data.get("batch").and_then(Value::as_object) {
            for item in batch
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(message_id) = item.get("messageId").and_then(Value::as_str) {
                    message_ids.push(message_id.to_owned());
                }
                if let Some(notification_id) = item.get("notificationId").and_then(Value::as_str) {
                    notification_ids.push(notification_id.to_owned());
                }
            }
        }
        let attempt_id = data
            .get("attemptId")
            .and_then(Value::as_str)
            .or_else(|| {
                data.get("attempt")
                    .and_then(|attempt| attempt.get("attemptId"))
                    .and_then(Value::as_str)
            })
            .map(str::to_owned);
        let generation = data.get("generation").and_then(Value::as_u64);

        let mut candidates = Vec::<usize>::new();
        for (object_index, object) in objects.iter().enumerate() {
            if object.source_index >= index {
                continue;
            }
            let key_matches = keys.iter().any(|key| key == &object.key)
                || data
                    .get("notificationKey")
                    .and_then(Value::as_str)
                    .is_some_and(|key| key == object.key);
            let id_matches = notification_ids
                .iter()
                .any(|id| id == &object.notification_id);
            let message_matches = message_ids.iter().any(|id| id == &object.message_id);
            // notification.superseded carries the master-wake generation, not
            // the notification object generation, so it must match by key/id.
            let generation_matches = kind == "notification.superseded"
                || generation.is_none()
                || object.generation.is_none()
                || generation == object.generation;
            if (key_matches || id_matches || message_matches) && generation_matches {
                candidates.push(object_index);
            }
        }

        let terminal = matches!(
            kind,
            "notification.emitted" | "notification.batch_emitted" | "notification.superseded"
        );
        let select =
            |candidates: &[usize]| -> Option<usize> {
                match (&attempt_id, kind) {
                    (Some(event_attempt), _) => candidates
                        .iter()
                        .copied()
                        .find(|object_index| {
                            objects[*object_index].attempt_id.as_deref() == Some(event_attempt)
                        })
                        .or_else(|| {
                            candidates.iter().rev().copied().find(|object_index| {
                                !terminal || objects[*object_index].terminal_count == 0
                            })
                        }),
                    (_, "notification.delivery_attempt") => candidates
                        .iter()
                        .rev()
                        .copied()
                        .find(|object_index| objects[*object_index].attempt_id.is_none()),
                    (_, _) => candidates.iter().rev().copied().find(|object_index| {
                        !terminal || objects[*object_index].terminal_count == 0
                    }),
                }
            };
        // A batch event names several notification keys; fan out one target per
        // distinct object key while still choosing the right generation.
        let mut targets = Vec::new();
        let mut seen_keys = BTreeSet::new();
        for candidate in &candidates {
            if seen_keys.insert(objects[*candidate].key.clone()) {
                let same_key = candidates
                    .iter()
                    .copied()
                    .filter(|object_index| objects[*object_index].key == objects[*candidate].key)
                    .collect::<Vec<_>>();
                if let Some(object_index) = select(&same_key) {
                    targets.push(object_index);
                }
            }
        }

        if targets.is_empty() {
            if kind.starts_with("notification.") {
                let event_id = event
                    .get("eventId")
                    .and_then(Value::as_str)
                    .unwrap_or("<unknown>");
                return Err(format!(
                    "NOTIFICATION_OBJECT_EVENT_UNATTACHED:{kind}:{event_id}"
                ));
            }
            continue;
        }
        if kind.starts_with("notification.") {
            let mut claimed = keys
                .iter()
                .cloned()
                .map(|key| ("key".to_owned(), key))
                .collect::<Vec<_>>();
            if let Some(key) = data.get("notificationKey").and_then(Value::as_str) {
                claimed.push(("key".to_owned(), key.to_owned()));
            }
            claimed.extend(
                notification_ids
                    .iter()
                    .cloned()
                    .map(|id| ("notificationId".to_owned(), id)),
            );
            claimed.extend(
                message_ids
                    .iter()
                    .cloned()
                    .map(|id| ("messageId".to_owned(), id)),
            );
            for (claim_kind, claim) in claimed {
                let matched = targets.iter().any(|object_index| {
                    let object = &objects[*object_index];
                    match claim_kind.as_str() {
                        "key" => object.key == claim,
                        "notificationId" => object.notification_id == claim,
                        _ => object.message_id == claim,
                    }
                });
                if !matched {
                    return Err(format!(
                        "NOTIFICATION_OBJECT_EVENT_UNATTACHED:{kind}:{claim_kind}:{claim}"
                    ));
                }
            }
        }
        for object_index in targets {
            let object = &mut objects[object_index];
            object.events.push(event.clone());
            if kind == "notification.delivery_attempt" {
                if let Some(attempt_id) = attempt_id.as_ref() {
                    object.attempt_id = Some(attempt_id.clone());
                }
            }
            if kind == "notification.delivery_failed"
                && event["data"]["attemptId"]
                    .as_str()
                    .zip(object.attempt_id.as_deref())
                    .is_some_and(|(failed_id, pending_id)| failed_id == pending_id)
            {
                object.attempt_id = None;
            }
            if terminal {
                object.terminal_count += 1;
            }
        }
    }

    for event in events {
        if let Some(message_id) = event["data"].get("messageId").and_then(Value::as_str) {
            if matches!(
                event["kind"].as_str(),
                Some("message.created" | "message.state")
            ) {
                for object in &mut objects {
                    if object.message_id == message_id {
                        object.events.push(event.clone());
                    }
                }
            }
        }
    }

    Ok(objects
        .into_iter()
        .map(|object| (object.key, object.events))
        .collect())
}

fn notification_input(key: &str, events: &[Value]) -> Result<Value, String> {
    let queues = events
        .iter()
        .filter(|event| event["kind"] == "notification.queued")
        .filter(|event| event["data"]["key"] == key)
        .collect::<Vec<_>>();
    if queues.is_empty() {
        return Err(format!("NOTIFICATION_OBJECT_SOURCE_MISSING:{key}"));
    }
    let message_id = queues.last().expect("queues checked non-empty")["data"]["notification"]
        ["messageId"]
        .as_str()
        .ok_or_else(|| "NOTIFICATION_MESSAGE_ID_MISSING".to_owned())?
        .to_owned();
    Ok(json!({
        "key": key,
        "messageId": message_id,
        "notificationId": queues
            .last()
            .expect("queues checked non-empty")["data"]["notification"]["notificationId"]
            .as_str()
            .unwrap_or(""),
        "events": events,
    }))
}

pub(super) fn register_notification_operator(registry: &mut Registry) -> Result<(), String> {
    registry
        .register(NotificationObjectValidateOperator)
        .map_err(|error| format!("DAGPIPE_OPERATOR_REGISTER_FAILED:{error}"))
}

struct NotificationObjectValidateOperator;

impl Operator for NotificationObjectValidateOperator {
    fn name(&self) -> &'static str {
        "appsdk.communication.notification_object_validate"
    }

    fn version(&self) -> &'static str {
        "1"
    }

    fn replay(&self) -> EffectReplay {
        EffectReplay::NonReplayable
    }

    fn execute(&self, input: Value, _: &OperatorContext) -> Result<Value, String> {
        let key = input
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| "DAGPIPE_NOTIFICATION_KEY_MISSING".to_owned())?;
        let message_id = input
            .get("messageId")
            .and_then(Value::as_str)
            .ok_or_else(|| "DAGPIPE_NOTIFICATION_MESSAGE_MISSING".to_owned())?;
        let notification_id = input
            .get("notificationId")
            .and_then(Value::as_str)
            .unwrap_or("");
        let events = input
            .get("events")
            .and_then(Value::as_array)
            .ok_or_else(|| "DAGPIPE_NOTIFICATION_EVENTS_MISSING".to_owned())?;
        let queues = events
            .iter()
            .filter(|event| event["kind"] == "notification.queued")
            .filter(|event| event["data"]["key"] == key)
            .collect::<Vec<_>>();
        if queues.is_empty() {
            return Err(format!("NOTIFICATION_OBJECT_SOURCE_MISSING:{key}"));
        }
        let created = events
            .iter()
            .filter(|event| event["kind"] == "message.created")
            .filter(|event| event["data"]["messageId"] == message_id)
            .count();
        if created != 1 {
            return Err(format!("NOTIFICATION_OBJECT_MESSAGE_SOURCE_MISMATCH:{key}"));
        }
        let accepted = events.iter().any(|event| match event["kind"].as_str() {
            Some("message.state") => {
                event["data"]["messageId"] == message_id && event["data"]["state"] == "accepted"
            }
            Some("message.created") => {
                event["data"]["messageId"] == message_id
                    && event["data"]["state"] == "accepted"
                    && event["data"]["evidence"]
                        .as_array()
                        .is_some_and(|evidence| {
                            evidence.iter().any(|record| record["state"] == "accepted")
                        })
            }
            _ => false,
        });
        if !accepted {
            return Err(format!("NOTIFICATION_OBJECT_MESSAGE_ACCEPT_MISSING:{key}"));
        }
        let terminal = events
            .iter()
            .filter(|event| {
                matches!(
                    event["kind"].as_str(),
                    Some("notification.emitted")
                        | Some("notification.batch_emitted")
                        | Some("notification.superseded")
                )
            })
            .filter(|event| {
                let data = &event["data"];
                let keys = data
                    .get("keys")
                    .or_else(|| data.get("notificationKeys"))
                    .and_then(Value::as_array);
                let key_match =
                    keys.is_some_and(|keys| keys.iter().any(|candidate| candidate == &json!(key)));
                let notification_match = data
                    .get("notificationId")
                    .or_else(|| data.get("notificationKey"))
                    .and_then(Value::as_str)
                    .is_some_and(|candidate| candidate == notification_id)
                    || data
                        .get("notificationIds")
                        .and_then(Value::as_array)
                        .is_some_and(|ids| {
                            ids.iter()
                                .any(|candidate| candidate == &json!(notification_id))
                        });
                key_match || notification_match
            })
            .cloned()
            .collect::<Vec<_>>();
        if terminal.len() > 1 {
            return Err(format!("NOTIFICATION_OBJECT_MULTIPLE_SINKS:{key}"));
        }
        if let Some(event) = terminal.first() {
            validate_terminal_event(&events, event, key, notification_id, message_id)?;
        }
        validate_delivery_failures(&events, key, notification_id, message_id)?;
        let failures = events
            .iter()
            .filter(|event| event["kind"] == "notification.delivery_failed")
            .filter(|event| {
                event["data"]["keys"]
                    .as_array()
                    .is_some_and(|keys| keys.iter().any(|candidate| candidate == &json!(key)))
            })
            .count();
        let terminal = if terminal.is_empty() {
            let attempts = events
                .iter()
                .filter(|event| event["kind"] == "notification.delivery_attempt")
                .filter(|event| {
                    event["data"]["keys"]
                        .as_array()
                        .is_some_and(|keys| keys.iter().any(|candidate| candidate == &json!(key)))
                })
                .count();
            if failures > 0 {
                json!({
                    "status": "pending_retry",
                    "key": key,
                    "messageId": message_id,
                    "retry_required": true,
                    "mailbox_only": false,
                })
            } else if attempts > 0 {
                json!({
                    "status": "unknown",
                    "key": key,
                    "messageId": message_id,
                    "repair_required": true,
                    "mailbox_only": false,
                })
            } else {
                json!({
                    "status": "pending",
                    "key": key,
                    "messageId": message_id,
                    "mailbox_only": true,
                    "repair_required": true,
                })
            }
        } else {
            terminal[0].clone()
        };
        Ok(json!({
            "key": key,
            "messageId": message_id,
            "single_source_single_sink": true,
            "terminal": terminal,
        }))
    }
}

fn terminal_requires_repair(terminal: &Value) -> bool {
    terminal.get("repair_required").and_then(Value::as_bool) == Some(true)
        || terminal.get("mailbox_only").and_then(Value::as_bool) == Some(true)
}

fn notification_event_matches_object(
    event: &Value,
    key: &str,
    notification_id: &str,
    message_id: &str,
) -> bool {
    let data = &event["data"];
    if data["keys"]
        .as_array()
        .or_else(|| data["notificationKeys"].as_array())
        .is_some_and(|keys| keys.iter().any(|candidate| candidate == &json!(key)))
    {
        return true;
    }
    if !notification_id.is_empty()
        && (data["notificationIds"].as_array().is_some_and(|ids| {
            ids.iter()
                .any(|candidate| candidate == &json!(notification_id))
        }) || data["notificationId"].as_str() == Some(notification_id))
    {
        return true;
    }
    data["messageId"].as_str() == Some(message_id)
}

fn validate_delivery_failures(
    events: &[Value],
    key: &str,
    notification_id: &str,
    message_id: &str,
) -> Result<(), String> {
    let mut pending: Option<(String, String)> = None;
    let mut terminal_seen = false;
    for event in events {
        let kind = event["kind"].as_str();
        if !matches!(
            kind,
            Some("notification.delivery_attempt")
                | Some("notification.emitted")
                | Some("notification.batch_emitted")
                | Some("notification.superseded")
                | Some("notification.delivery_failed")
        ) || !notification_event_matches_object(event, key, notification_id, message_id)
        {
            continue;
        }
        match kind {
            Some("notification.delivery_attempt") => {
                if terminal_seen {
                    return Err(format!(
                        "NOTIFICATION_OBJECT_DELIVERY_ATTEMPT_AFTER_TERMINAL:{key}"
                    ));
                }
                let attempt_id = event["data"]["attemptId"]
                    .as_str()
                    .ok_or_else(|| format!("NOTIFICATION_OBJECT_ATTEMPT_ID_MISSING:{key}"))?
                    .to_owned();
                let operation = event["data"]["attempt"]["operation"]
                    .as_str()
                    .ok_or_else(|| format!("NOTIFICATION_OBJECT_ATTEMPT_OPERATION_MISSING:{key}"))?
                    .to_owned();
                pending = Some((attempt_id, operation));
            }
            Some("notification.emitted")
            | Some("notification.batch_emitted")
            | Some("notification.superseded") => {
                pending = None;
                terminal_seen = true;
            }
            Some("notification.delivery_failed") => {
                let operation = event["data"]["operation"]
                    .as_str()
                    .ok_or_else(|| format!("NOTIFICATION_OBJECT_FAILURE_OPERATION_MISSING:{key}"))?
                    .to_owned();
                if let Some(attempt_id) = event["data"]["attemptId"].as_str() {
                    if pending.as_ref() != Some(&(attempt_id.to_owned(), operation.clone())) {
                        return Err(format!(
                            "NOTIFICATION_OBJECT_DELIVERY_FAILURE_MISMATCH:{key}:{attempt_id}"
                        ));
                    }
                    pending = None;
                } else if pending.is_some() || terminal_seen {
                    // Pre-adapter failures legitimately omit attemptId, but only
                    // before any delivery attempt has started and before a
                    // terminal receipt has been recorded for this object.
                    return Err(format!(
                        "NOTIFICATION_OBJECT_DELIVERY_FAILURE_MISMATCH:{key}"
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_terminal_event(
    events: &[Value],
    terminal: &Value,
    key: &str,
    notification_id: &str,
    message_id: &str,
) -> Result<(), String> {
    let kind = terminal
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "NOTIFICATION_TERMINAL_KIND_MISSING".to_owned())?;
    let data = terminal
        .get("data")
        .ok_or_else(|| format!("NOTIFICATION_TERMINAL_DATA_MISSING:{kind}"))?;
    match kind {
        "notification.emitted" => {
            let terminal_index = events
                .iter()
                .position(|event| event == terminal)
                .unwrap_or(0);
            let attempt_id = data
                .get("attemptId")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!("NOTIFICATION_TERMINAL_INVALID:{key}:{kind}:attemptId_missing")
                })?;
            if data.get("at").and_then(Value::as_str).is_none() {
                return Err(format!(
                    "NOTIFICATION_TERMINAL_INVALID:{key}:{kind}:at_missing"
                ));
            }
            if !data
                .get("keys")
                .and_then(Value::as_array)
                .is_some_and(|keys| keys.iter().any(|candidate| candidate == &json!(key)))
            {
                return Err(format!(
                    "NOTIFICATION_TERMINAL_INVALID:{key}:{kind}:keys_missing"
                ));
            }
            if !terminal_attempt_matches_before(
                events,
                terminal_index,
                attempt_id,
                key,
                "notification.emitted",
                notification_id,
                message_id,
            ) {
                return Err(format!(
                    "NOTIFICATION_TERMINAL_INVALID:{key}:{kind}:attempt_missing:{attempt_id}"
                ));
            }
        }
        "notification.batch_emitted" => {
            let terminal_index = events
                .iter()
                .position(|event| event == terminal)
                .unwrap_or(0);
            let attempt_id = data
                .get("attemptId")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!("NOTIFICATION_TERMINAL_INVALID:{key}:{kind}:attemptId_missing")
                })?;
            if data.get("at").and_then(Value::as_str).is_none() {
                return Err(format!(
                    "NOTIFICATION_TERMINAL_INVALID:{key}:{kind}:at_missing"
                ));
            }
            let batch_matches = data
                .get("batch")
                .and_then(Value::as_object)
                .and_then(|batch| batch.get("items"))
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty());
            let notification_keys = data
                .get("notificationKeys")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let notification_ids = data
                .get("notificationIds")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let id_matches = notification_keys
                .iter()
                .chain(&notification_ids)
                .any(|candidate| {
                    candidate == &json!(key)
                        || (!notification_id.is_empty() && candidate == &json!(notification_id))
                });
            if !batch_matches || !id_matches {
                return Err(format!(
                    "NOTIFICATION_TERMINAL_INVALID:{key}:{kind}:batch_or_ids_missing"
                ));
            }
            let batch = data
                .get("batch")
                .and_then(Value::as_object)
                .expect("batch_matches checked");
            let batch_id = batch
                .get("batchId")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!("NOTIFICATION_TERMINAL_INVALID:{key}:{kind}:batchId_missing")
                })?;
            let batch_item_matches =
                batch
                    .get("items")
                    .and_then(Value::as_array)
                    .is_some_and(|items| {
                        items.iter().any(|item| {
                            item.get("notificationId").and_then(Value::as_str)
                                == Some(notification_id)
                                || item.get("messageId").and_then(Value::as_str) == Some(message_id)
                        })
                    });
            if !batch_item_matches {
                return Err(format!(
                    "NOTIFICATION_TERMINAL_INVALID:{key}:{kind}:batch_item_missing"
                ));
            }
            if !terminal_attempt_matches_before(
                events,
                terminal_index,
                attempt_id,
                key,
                "notification.batch_emitted",
                notification_id,
                message_id,
            ) {
                return Err(format!(
                    "NOTIFICATION_TERMINAL_INVALID:{key}:{kind}:attempt_missing:{attempt_id}"
                ));
            }
            let (_, attempt) = latest_delivery_attempt_before(
                events,
                terminal_index,
                key,
                "notification.batch_emitted",
                notification_id,
                message_id,
            )
            .expect("terminal_attempt_matches_before checked");
            if attempt["data"]["attempt"]["batchId"].as_str() != Some(batch_id) {
                return Err(format!(
                    "NOTIFICATION_TERMINAL_INVALID:{key}:{kind}:batch_id_mismatch"
                ));
            }
        }
        "notification.superseded" => {
            let reason = data.get("reason").and_then(Value::as_str);
            if data.get("generation").and_then(Value::as_u64).is_none()
                || !matches!(
                    reason,
                    Some("master_wake_briefing" | "master_wake_decision")
                )
                || !data
                    .get("keys")
                    .and_then(Value::as_array)
                    .is_some_and(|keys| keys.iter().any(|candidate| candidate == &json!(key)))
            {
                return Err(format!(
                    "NOTIFICATION_TERMINAL_INVALID:{key}:{kind}:schema_missing"
                ));
            }
        }
        _ => {
            return Err(format!(
                "NOTIFICATION_TERMINAL_INVALID:{key}:{kind}:unknown"
            ));
        }
    }
    Ok(())
}

fn latest_delivery_attempt_before<'a>(
    events: &'a [Value],
    terminal_index: usize,
    key: &str,
    operation: &str,
    notification_id: &str,
    message_id: &str,
) -> Option<(usize, &'a Value)> {
    events[..terminal_index]
        .iter()
        .enumerate()
        .rev()
        .find(|(_, event)| {
            event["kind"] == "notification.delivery_attempt"
                && event["data"]["attempt"]["operation"].as_str() == Some(operation)
                && event["data"]["attempt"].get("batchId").is_some()
                    == (operation == "notification.batch_emitted")
                && notification_event_matches_object(event, key, notification_id, message_id)
        })
}

fn terminal_attempt_matches_before(
    events: &[Value],
    terminal_index: usize,
    attempt_id: &str,
    key: &str,
    operation: &str,
    notification_id: &str,
    message_id: &str,
) -> bool {
    let Some((latest_index, latest_attempt)) = latest_delivery_attempt_before(
        events,
        terminal_index,
        key,
        operation,
        notification_id,
        message_id,
    ) else {
        return false;
    };
    if latest_attempt["data"]["attemptId"].as_str() != Some(attempt_id) {
        return false;
    }
    !events[latest_index + 1..terminal_index]
        .iter()
        .any(|event| {
            event["kind"] == "notification.delivery_failed"
                && event["data"]["attemptId"].as_str() == Some(attempt_id)
                && notification_event_matches_object(event, key, notification_id, message_id)
        })
}
