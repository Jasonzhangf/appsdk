//! DAGpipe-backed fix lifecycle execution.
//!
//! The graph contract is the topology owner; the Operators below validate the
//! AppSDK record chain before the lifecycle state machine advances.

use pipeline_runtime::{
    compile, graph_topology, parse_graph_json, Cancellation, EffectReplay, Identity, Operator,
    OperatorContext, Registry, Runtime, StateMachine, Transition, ValueType,
};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use crate::{
    assert_identifier, assert_no_symlink_components, assert_project_root_safe, fail,
    module_record_name,
};

const FIX_LIFECYCLE_GRAPH: &str = include_str!("../../contracts/dagpipe/fix-lifecycle.graph.json");
const NOTIFICATION_LIFECYCLE_GRAPH: &str =
    include_str!("../../contracts/dagpipe/notification.graph.json");
const DAGPIPE_GRAPH_MANIFEST: &str = include_str!("../../contracts/dagpipe/manifest.json");
const COMMUNICATION_EVENT_SCHEMA: &str =
    include_str!("../../contracts/communication/communication-event.schema.json");

pub(crate) fn run_cli(args: &mut std::iter::Peekable<std::vec::IntoIter<String>>) {
    let subcommand = args
        .next()
        .unwrap_or_else(|| {
            fail("USAGE: appsdk dagpipe fix [project] [--module <id>] | validate | validate-notifications <project>")
        });
    if subcommand == "validate" {
        let result = validate_graph_contracts()
            .unwrap_or_else(|error| fail(format!("DAGPIPE_GRAPH_VALIDATION_FAILED:{error}")));
        println!(
            "{}",
            serde_json::to_string_pretty(&result).expect("dagpipe result is serializable")
        );
        return;
    }
    if subcommand == "validate-notifications" {
        let root = std::path::PathBuf::from(
            args.next()
                .unwrap_or_else(|| fail("USAGE: appsdk dagpipe validate-notifications <project>")),
        );
        if args.next().is_some() {
            fail("USAGE: appsdk dagpipe validate-notifications <project>");
        }
        let result = validate_notification_objects(&root)
            .unwrap_or_else(|error| fail(format!("DAGPIPE_NOTIFICATION_OBJECT_INVALID:{error}")));
        println!(
            "{}",
            serde_json::to_string_pretty(&result).expect("dagpipe result is serializable")
        );
        return;
    }
    if subcommand != "fix" {
        fail("USAGE: appsdk dagpipe fix [project] [--module <id>] | validate | validate-notifications <project>");
    }

    let mut root = std::path::PathBuf::from(".");
    if let Some(first) = args.peek() {
        if first != "--module" {
            root = std::path::PathBuf::from(args.next().expect("peeked"));
        }
    }
    let mut module_id = None;
    while let Some(option) = args.next() {
        match option.as_str() {
            "--module" => {
                if module_id.is_some() {
                    fail("USAGE: appsdk dagpipe fix [project] [--module <id>]");
                }
                module_id = Some(args.next().unwrap_or_else(|| {
                    fail("USAGE: appsdk dagpipe fix [project] [--module <id>]")
                }));
            }
            _ => fail("USAGE: appsdk dagpipe fix [project] [--module <id>]"),
        }
    }

    let module_id = module_id.unwrap_or_else(|| fail("DAGPIPE_MODULE_REQUIRED"));
    let result = run_fix_lifecycle(&root, &module_id)
        .unwrap_or_else(|error| fail(format!("DAGPIPE_FIX_LIFECYCLE_FAILED:{error}")));
    println!(
        "{}",
        serde_json::to_string_pretty(&result).expect("dagpipe result is serializable")
    );
}

fn run_fix_lifecycle(root: &Path, module_id: &str) -> Result<Value, String> {
    assert_project_root_safe(root);
    assert_identifier(module_id, "DAGPIPE_MODULE_IDENTIFIER_INVALID");
    let graph = parse_graph_json(FIX_LIFECYCLE_GRAPH)
        .map_err(|error| format!("DAGPIPE_GRAPH_INVALID:{error}"))?;
    ensure_single_source_single_sink(&graph)?;
    let mut registry = Registry::default();
    register_fix_operators(&mut registry)?;
    let capabilities = BTreeSet::new();
    let compiled = compile(graph, &registry, &capabilities)
        .map_err(|error| format!("DAGPIPE_COMPILE_FAILED:{error}"))?;

    let input = lifecycle_input(root, module_id)?;
    let mut inputs = HashMap::new();
    inputs.insert("lifecycle_state".to_owned(), input.clone());
    let identity = Identity {
        project_id: "appsdk".to_owned(),
        graph_id: compiled.id().to_owned(),
        graph_version: compiled.version().to_owned(),
        execution_id: format!("fix-lifecycle-{}", UtcStamp::now()),
        attempt_id: "1".to_owned(),
    };
    let runtime = Runtime::new(capabilities);
    let result = runtime
        .run(&compiled, identity, inputs, &Cancellation::default())
        .map_err(|failure| format!("DAGPIPE_EXECUTION_FAILED:{failure}"))?;

    let mut state = "open".to_owned();
    let mut journal = Vec::new();
    let machine = lifecycle_state_machine()
        .map_err(|error| format!("DAGPIPE_STATE_MACHINE_INVALID:{error}"))?;
    for (node, event) in [
        ("admission_claim", "claim"),
        ("admission_candidate", "candidate"),
        ("admission_review", "review_pass"),
        ("admission_effectiveness", "effectiveness_replay"),
        ("admission_remote", "remote_receipt"),
        ("emit_promotion", "promote"),
    ] {
        ensure_node_completed(&result.journal, node)?;
        state = machine
            .apply(&state, event, &mut journal)
            .map_err(|error| format!("DAGPIPE_TRANSITION_FAILED:{error}"))?;
    }
    if state != "promoted" {
        return Err(format!("DAGPIPE_TERMINAL_STATE_INVALID:{state}"));
    }
    let last = result
        .outputs
        .get("promotion_state")
        .map(|value| value.payload.clone())
        .ok_or_else(|| "DAGPIPE_OUTPUT_MISSING:promotion_state".to_owned())?;
    Ok(json!({
        "graph": {"id": compiled.id(), "version": compiled.version()},
        "state": state,
        "authority": "advisory_projection",
        "authoritative_gate": "appsdk verify <project>",
        "single_source_single_sink": {"inputs": 1, "outputs": 1},
        "node_order": compiled.node_ids().collect::<Vec<_>>(),
        "runtime_journal": result.journal,
        "state_journal": journal,
        "result": last,
    }))
}

fn validate_graph_contracts() -> Result<Value, String> {
    let manifest: Value = serde_json::from_str(DAGPIPE_GRAPH_MANIFEST)
        .map_err(|error| format!("DAGPIPE_GRAPH_MANIFEST_INVALID:{error}"))?;
    if manifest.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err("DAGPIPE_GRAPH_MANIFEST_INVALID:schema_version".to_owned());
    }
    let entries = manifest
        .get("graphs")
        .and_then(Value::as_array)
        .ok_or_else(|| "DAGPIPE_GRAPH_MANIFEST_INVALID:graphs".to_owned())?;
    let embedded = embedded_graph_sources();
    if entries.len() != embedded.len() {
        return Err(format!(
            "DAGPIPE_GRAPH_MANIFEST_MISMATCH:manifest={} embedded={}",
            entries.len(),
            embedded.len()
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut graphs = Vec::new();
    for entry in entries {
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "DAGPIPE_GRAPH_MANIFEST_INVALID:id".to_owned())?;
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "DAGPIPE_GRAPH_MANIFEST_INVALID:path".to_owned())?;
        if !seen.insert(path.to_owned()) {
            return Err(format!("DAGPIPE_GRAPH_MANIFEST_DUPLICATE_PATH:{path}"));
        }
        let source = embedded
            .iter()
            .find(|(embedded_path, _)| *embedded_path == path)
            .map(|(_, source)| *source)
            .ok_or_else(|| format!("DAGPIPE_GRAPH_MANIFEST_MISSING_SOURCE:{path}"))?;
        let graph = parse_graph_json(source)
            .map_err(|error| format!("DAGPIPE_GRAPH_INVALID:{id}:{error}"))?;
        if graph.id != id {
            return Err(format!("DAGPIPE_GRAPH_ID_MISMATCH:{id}:{}", graph.id));
        }
        ensure_single_source_single_sink(&graph).map_err(|error| format!("{id}:{error}"))?;
        graphs.push(json!({
            "id": id,
            "version": graph.version,
            "single_source_single_sink": {"inputs": graph.inputs.len(), "outputs": graph.outputs.len()}
        }));
    }
    for (path, _) in embedded {
        if !seen.contains(path) {
            return Err(format!("DAGPIPE_GRAPH_MANIFEST_MISSING_PATH:{path}"));
        }
    }
    Ok(json!({
        "single_source_single_sink": true,
        "graphs": graphs,
        "manifest": manifest,
    }))
}

fn validate_notification_objects(root: &Path) -> Result<Value, String> {
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
            .is_none_or(|value| value.trim().is_empty())
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

fn register_notification_operator(registry: &mut Registry) -> Result<(), String> {
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
            if !delivery_attempt_matches_before(
                events,
                terminal_index,
                attempt_id,
                key,
                "notification.emitted",
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
            let id_matches = data
                .get("notificationKeys")
                .or_else(|| data.get("notificationIds"))
                .and_then(Value::as_array)
                .is_some_and(|values| {
                    values.iter().any(|candidate| {
                        candidate == &json!(key)
                            || (!notification_id.is_empty() && candidate == &json!(notification_id))
                    })
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
            let Some(attempt) = delivery_attempt_event_before(
                events,
                terminal_index,
                attempt_id,
                key,
                "notification.batch_emitted",
            ) else {
                return Err(format!(
                    "NOTIFICATION_TERMINAL_INVALID:{key}:{kind}:attempt_missing:{attempt_id}"
                ));
            };
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

fn delivery_attempt_event_before<'a>(
    events: &'a [Value],
    terminal_index: usize,
    attempt_id: &str,
    key: &str,
    operation: &str,
) -> Option<&'a Value> {
    events[..terminal_index].iter().find(|event| {
        event["kind"] == "notification.delivery_attempt"
            && event["data"]["attemptId"].as_str() == Some(attempt_id)
            && event["data"]["keys"]
                .as_array()
                .is_some_and(|keys| keys.iter().any(|candidate| candidate == &json!(key)))
            && event["data"]["attempt"]["operation"].as_str() == Some(operation)
            && event["data"]["attempt"].get("batchId").is_some()
                == (operation == "notification.batch_emitted")
    })
}

fn delivery_attempt_matches_before(
    events: &[Value],
    terminal_index: usize,
    attempt_id: &str,
    key: &str,
    operation: &str,
) -> bool {
    delivery_attempt_event_before(events, terminal_index, attempt_id, key, operation).is_some()
}

fn embedded_graph_sources() -> [(&'static str, &'static str); 2] {
    [
        (
            "contracts/dagpipe/fix-lifecycle.graph.json",
            FIX_LIFECYCLE_GRAPH,
        ),
        (
            "contracts/dagpipe/notification.graph.json",
            NOTIFICATION_LIFECYCLE_GRAPH,
        ),
    ]
}

fn ensure_node_completed(
    journal: &[pipeline_runtime::Event],
    expected_node: &str,
) -> Result<(), String> {
    if journal.iter().any(
        |event| matches!(event, pipeline_runtime::Event::NodeCompleted { node_id, .. } if node_id == expected_node),
    ) {
        return Ok(());
    }
    Err(format!("DAGPIPE_NODE_NOT_COMPLETED:{expected_node}"))
}

fn ensure_single_source_single_sink(graph: &pipeline_runtime::Graph) -> Result<(), String> {
    if graph.inputs.len() != 1 || graph.outputs.len() != 1 {
        return Err(format!(
            "DAGPIPE_GRAPH_MUST_BE_SINGLE_SOURCE_SINGLE_SINK:inputs={} outputs={}",
            graph.inputs.len(),
            graph.outputs.len()
        ));
    }
    let mut consumers = std::collections::HashMap::<&str, Vec<&str>>::new();
    for node in &graph.nodes {
        if node.inputs.len() != 1 {
            return Err(format!(
                "DAGPIPE_NODE_MUST_HAVE_SINGLE_INPUT:{} inputs={}",
                node.id,
                node.inputs.len()
            ));
        }
        consumers
            .entry(node.inputs[0].as_str())
            .or_default()
            .push(node.id.as_str());
    }
    for (arc, nodes) in &consumers {
        if nodes.len() != 1 {
            return Err(format!(
                "DAGPIPE_ARC_MUST_HAVE_SINGLE_SINK:{} sinks={}",
                arc,
                nodes.len()
            ));
        }
    }
    let mut producers = std::collections::HashMap::<&str, &str>::new();
    for node in &graph.nodes {
        if producers
            .insert(node.output.id.as_str(), node.id.as_str())
            .is_some()
        {
            return Err(format!(
                "DAGPIPE_ARC_MUST_HAVE_SINGLE_SOURCE:{}",
                node.output.id
            ));
        }
    }
    if !producers.contains_key(graph.outputs[0].as_str()) {
        return Err(format!("DAGPIPE_GRAPH_OUTPUT_UNBOUND:{}", graph.outputs[0]));
    }
    let declared_output = graph.outputs[0].as_str();
    for (arc, _) in &producers {
        if !consumers.contains_key(arc) && declared_output != *arc {
            return Err(format!("DAGPIPE_ARC_UNCONSUMED:{arc}"));
        }
    }
    let mut edge_arcs = std::collections::HashSet::new();
    for edge in &graph.edges {
        if !edge_arcs.insert(edge.arc_id.as_str()) {
            return Err(format!(
                "DAGPIPE_ARC_MUST_HAVE_SINGLE_SOURCE:{}",
                edge.arc_id
            ));
        }
        if consumers.get(edge.arc_id.as_str()).map(Vec::len) != Some(1) {
            return Err(format!("DAGPIPE_ARC_MUST_HAVE_SINGLE_SINK:{}", edge.arc_id));
        }
    }
    // Single-input/single-output per node is necessary but not sufficient for
    // SESE: only the shared topology validator rejects cycles, edge endpoint
    // mismatches, dead nodes, and undeclared input arcs.
    graph_topology(graph).map_err(|error| format!("DAGPIPE_GRAPH_TOPOLOGY_INVALID:{error}"))?;
    Ok(())
}

fn register_fix_operators(registry: &mut Registry) -> Result<(), String> {
    for operator in [
        FixOperator::new("appsdk.lifecycle.claim", LifecycleStep::Claim),
        FixOperator::new("appsdk.lifecycle.candidate", LifecycleStep::Candidate),
        FixOperator::new("appsdk.lifecycle.review", LifecycleStep::Review),
        FixOperator::new(
            "appsdk.lifecycle.effectiveness",
            LifecycleStep::Effectiveness,
        ),
        FixOperator::new("appsdk.lifecycle.remote", LifecycleStep::Remote),
        FixOperator::new("appsdk.lifecycle.promotion", LifecycleStep::Promotion),
    ] {
        registry
            .register(operator)
            .map_err(|error| format!("DAGPIPE_OPERATOR_REGISTER_FAILED:{error}"))?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum LifecycleStep {
    Claim,
    Candidate,
    Review,
    Effectiveness,
    Remote,
    Promotion,
}

struct FixOperator {
    name: &'static str,
    step: LifecycleStep,
}

impl FixOperator {
    fn new(name: &'static str, step: LifecycleStep) -> Self {
        Self { name, step }
    }
}

impl Operator for FixOperator {
    fn name(&self) -> &'static str {
        self.name
    }

    fn version(&self) -> &'static str {
        "1"
    }

    fn input_type(&self) -> ValueType {
        ValueType::Any
    }

    fn output_type(&self) -> ValueType {
        ValueType::Any
    }

    fn replay(&self) -> EffectReplay {
        EffectReplay::NonReplayable
    }

    fn execute(&self, input: Value, context: &OperatorContext) -> Result<Value, String> {
        let root = input
            .get("_root")
            .and_then(Value::as_str)
            .ok_or_else(|| "DAGPIPE_ROOT_MISSING".to_owned())?;
        let module_id = input
            .get("_module_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "DAGPIPE_MODULE_MISSING".to_owned())?;
        let mut output = input.clone();
        let step = match self.step {
            LifecycleStep::Claim => validate_claim(Path::new(root), module_id, &mut output),
            LifecycleStep::Candidate => validate_candidate(Path::new(root), module_id, &mut output),
            LifecycleStep::Review => validate_review(Path::new(root), module_id, &mut output),
            LifecycleStep::Effectiveness => {
                validate_effectiveness(Path::new(root), module_id, &mut output)
            }
            LifecycleStep::Remote => validate_remote(Path::new(root), module_id, &mut output),
            LifecycleStep::Promotion => validate_promotion(Path::new(root), module_id, &mut output),
        };
        step.map_err(|error| format!("{}:{error}", context.node_id))?;
        Ok(output)
    }
}

fn lifecycle_input(root: &Path, module_id: &str) -> Result<Value, String> {
    let worktree_name = module_record_name("worktree-record", module_id);
    let worktree = read_json_record(root, &worktree_name)?;
    let issue_id = required_str(&worktree, "/issue_id", &worktree_name)?;
    Ok(json!({
        "_root": root.to_string_lossy(),
        "_module_id": module_id,
        "issue_id": issue_id,
    }))
}

fn validate_claim(root: &Path, module_id: &str, output: &mut Value) -> Result<(), String> {
    let name = module_record_name("worktree-record", module_id);
    let record = read_json_record(root, &name)?;
    require_str(&record, "/worktree_id", &name)?;
    require_str(&record, "/issue_id", &name)?;
    require_str(&record, "/module_id", &name)?;
    require_str(&record, "/base_commit", &name)?;
    require_str(&record, "/branch", &name)?;
    require_str(&record, "/head_commit", &name)?;
    require_str(&record, "/scope_hash", &name)?;
    if record.get("initial_clean") != Some(&Value::Bool(true))
        || record.get("final_clean") != Some(&Value::Bool(true))
        || record.get("isolation_mode").and_then(Value::as_str) != Some("isolated_worktree")
    {
        return Err("FIX_WORKTREE_NOT_CLEAN_ISOLATED".to_owned());
    }
    output["worktree"] = record;
    Ok(())
}

fn validate_candidate(root: &Path, module_id: &str, output: &mut Value) -> Result<(), String> {
    let worktree_name = module_record_name("worktree-record", module_id);
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let worktree = output
        .get("worktree")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &worktree_name))?;
    let candidate = read_json_record(root, &candidate_name)?;
    for path in [
        "/fix_candidate_id",
        "/issue_id",
        "/module_id",
        "/worktree_id",
        "/base_commit",
        "/head_commit",
        "/tree_hash",
        "/diff_hash",
        "/design_id",
        "/owner",
        "/scope_hash",
    ] {
        require_str(&candidate, path, &candidate_name)?;
    }
    require_array_or_empty(&candidate, "/changed_paths", &candidate_name)?;
    require_array(&candidate, "/verification_evidence_ids", &candidate_name)?;
    if candidate.get("worktree_id") != worktree.get("worktree_id")
        || candidate.get("issue_id") != worktree.get("issue_id")
        || candidate.get("base_commit") != worktree.get("base_commit")
        || candidate.get("scope_hash") != worktree.get("scope_hash")
    {
        return Err("FIX_CANDIDATE_WORKTREE_MISMATCH".to_owned());
    }
    output["candidate"] = candidate;
    Ok(())
}

fn validate_review(root: &Path, module_id: &str, output: &mut Value) -> Result<(), String> {
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let validation_name = module_record_name("pre-review-validation-record", module_id);
    let review_name = module_record_name("review-record", module_id);
    let candidate = output
        .get("candidate")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &candidate_name))?;
    let validation = read_json_record(root, &validation_name)?;
    for path in [
        "/validation_id",
        "/issue_id",
        "/module_id",
        "/fix_candidate_id",
        "/candidate_commit",
        "/candidate_tree_hash",
        "/artifact_hash",
    ] {
        require_str(&validation, path, &validation_name)?;
    }
    require_object(&validation, "/whitebox_producer", &validation_name)?;
    require_array(&validation, "/whitebox_evidence_ids", &validation_name)?;
    require_array(&validation, "/blackbox_evidence_ids", &validation_name)?;
    if validation.get("result").and_then(Value::as_str) != Some("pass")
        || validation.get("source_unchanged") != Some(&Value::Bool(true))
        || validation.get("fix_candidate_id") != candidate.get("fix_candidate_id")
        || validation.get("candidate_commit") != candidate.get("head_commit")
        || validation.get("candidate_tree_hash") != candidate.get("tree_hash")
    {
        return Err("PRE_REVIEW_VALIDATION_MISMATCH".to_owned());
    }
    let review = read_json_record(root, &review_name)?;
    for path in [
        "/review_id",
        "/issue_id",
        "/promotion_id",
        "/fix_candidate_id",
        "/pre_review_validation_id",
        "/reviewed_commit",
        "/reviewed_tree_hash",
        "/reviewed_diff_hash",
        "/reviewed_artifact_hash",
        "/reviewed_scope_hash",
        "/resource_map_hash",
        "/function_map_hash",
        "/mainline_call_map_hash",
        "/verification_map_hash",
    ] {
        require_str(&review, path, &review_name)?;
    }
    require_object(&review, "/reviewer", &review_name)?;
    require_array(&review, "/evidence_ids", &review_name)?;
    if review.get("review_kind").and_then(Value::as_str) != Some("architecture")
        || review.get("verdict").and_then(Value::as_str) != Some("pass")
        || review.get("fix_candidate_id") != candidate.get("fix_candidate_id")
        || review.get("pre_review_validation_id") != validation.get("validation_id")
        || review.get("reviewed_commit") != candidate.get("head_commit")
        || review.get("reviewed_tree_hash") != candidate.get("tree_hash")
        || review.get("reviewed_scope_hash") != candidate.get("scope_hash")
    {
        return Err("ARCHITECTURE_REVIEW_INPUT_MISMATCH".to_owned());
    }
    output["review"] = review;
    output["validation"] = validation;
    Ok(())
}

fn validate_effectiveness(root: &Path, module_id: &str, output: &mut Value) -> Result<(), String> {
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let review_name = module_record_name("review-record", module_id);
    let effectiveness_name = module_record_name("effectiveness-record", module_id);
    let candidate = output
        .get("candidate")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &candidate_name))?;
    let review = output
        .get("review")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &review_name))?;
    let effectiveness = read_json_record(root, &effectiveness_name)?;
    for path in [
        "/effectiveness_id",
        "/issue_id",
        "/module_id",
        "/fix_candidate_id",
        "/architecture_review_id",
        "/reviewed_commit",
        "/reviewed_tree_hash",
        "/baseline_evidence_id",
        "/fixed_replay_evidence_id",
    ] {
        require_str(&effectiveness, path, &effectiveness_name)?;
    }
    for path in [
        "/reproduction_input_hashes",
        "/positive_evidence_ids",
        "/negative_evidence_ids",
        "/blackbox_evidence_ids",
    ] {
        require_array(&effectiveness, path, &effectiveness_name)?;
    }
    if effectiveness.get("result").and_then(Value::as_str) != Some("pass")
        || effectiveness.get("source_unchanged_since_review") != Some(&Value::Bool(true))
        || effectiveness.get("fix_candidate_id") != candidate.get("fix_candidate_id")
        || effectiveness.get("architecture_review_id") != review.get("review_id")
        || effectiveness.get("reviewed_commit") != candidate.get("head_commit")
        || effectiveness.get("reviewed_tree_hash") != candidate.get("tree_hash")
    {
        return Err("POST_ARCHITECTURE_EFFECTIVENESS_MISMATCH".to_owned());
    }
    output["effectiveness"] = effectiveness;
    Ok(())
}

fn validate_remote(root: &Path, module_id: &str, output: &mut Value) -> Result<(), String> {
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let effectiveness_name = module_record_name("effectiveness-record", module_id);
    let merge_name = module_record_name("merge-record", module_id);
    let candidate = output
        .get("candidate")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &candidate_name))?;
    let effectiveness = output
        .get("effectiveness")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &effectiveness_name))?;
    let merge = read_json_record(root, &merge_name)?;
    for path in [
        "/merge_id",
        "/issue_id",
        "/module_id",
        "/fix_candidate_id",
        "/effectiveness_id",
        "/mainline_ref",
        "/candidate_commit",
        "/merge_commit",
        "/candidate_tree_hash",
        "/merged_tree_hash",
        "/change_identity",
    ] {
        require_str(&merge, path, &merge_name)?;
    }
    if merge.get("result").and_then(Value::as_str) != Some("pass")
        || merge.get("fix_candidate_id") != candidate.get("fix_candidate_id")
        || merge.get("candidate_commit") != candidate.get("head_commit")
        || merge.get("candidate_tree_hash") != candidate.get("tree_hash")
        || merge.get("effectiveness_id") != effectiveness.get("effectiveness_id")
    {
        return Err("MERGE_CANDIDATE_IDENTITY_MISMATCH".to_owned());
    }
    output["merge"] = merge;
    Ok(())
}

fn validate_promotion(root: &Path, module_id: &str, output: &mut Value) -> Result<(), String> {
    let worktree_name = module_record_name("worktree-record", module_id);
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let review_name = module_record_name("review-record", module_id);
    let effectiveness_name = module_record_name("effectiveness-record", module_id);
    let merge_name = module_record_name("merge-record", module_id);
    let promotion_name = module_record_name("promotion-record", module_id);
    let worktree = output
        .get("worktree")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &worktree_name))?;
    let candidate = output
        .get("candidate")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &candidate_name))?;
    let review = output
        .get("review")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &review_name))?;
    let effectiveness = output
        .get("effectiveness")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &effectiveness_name))?;
    let merge = output
        .get("merge")
        .cloned()
        .map(Ok)
        .unwrap_or_else(|| read_json_record(root, &merge_name))?;
    let promotion = read_json_record(root, &promotion_name)?;
    for path in [
        "/promotion_id",
        "/issue_id",
        "/module_id",
        "/worktree_record_id",
        "/reproduction_record_id",
        "/fix_candidate_id",
        "/architecture_review_id",
        "/effectiveness_record_id",
        "/merge_record_id",
        "/candidate_commit",
        "/merged_commit",
        "/source_commit",
        "/new_active_version",
        "/review_id",
        "/change_set_id",
        "/compatibility_level",
        "/root_cause",
        "/design_id",
        "/change_reason_comment",
        "/playground_cleanup_record_id",
    ] {
        require_str(&promotion, path, &promotion_name)?;
    }
    require_array(&promotion, "/evidence_ids", &promotion_name)?;
    require_array(&promotion, "/required_gate_results", &promotion_name)?;
    if promotion.get("bug_closure_verified") != Some(&Value::Bool(true))
        || promotion.get("worktree_record_id") != worktree.get("worktree_id")
        || promotion.get("fix_candidate_id") != candidate.get("fix_candidate_id")
        || promotion.get("architecture_review_id") != review.get("review_id")
        || promotion.get("effectiveness_record_id") != effectiveness.get("effectiveness_id")
        || promotion.get("merge_record_id") != merge.get("merge_id")
        || promotion.get("candidate_commit") != candidate.get("head_commit")
        || promotion.get("merged_commit") != merge.get("merge_commit")
    {
        return Err("PROMOTION_FIX_LIFECYCLE_REFERENCE_MISMATCH".to_owned());
    }
    output["promotion"] = promotion;
    output["state"] = Value::String("promoted".to_owned());
    Ok(())
}

fn require_str(record: &Value, path: &str, name: &str) -> Result<(), String> {
    required_str(record, path, name).map(|_| ())
}

fn required_str<'a>(record: &'a Value, path: &str, name: &str) -> Result<&'a str, String> {
    record
        .pointer(path)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("INVALID_RECORD:{name}:{path}"))
}

fn read_json_record(root: &Path, name: &str) -> Result<Value, String> {
    assert_no_symlink_components(
        root,
        &root.join(".appsdk").join("records"),
        "record_control",
    );
    let file = root.join(".appsdk").join("records").join(name);
    if fs::symlink_metadata(&file)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!("GOVERNANCE_PATH_SYMLINK:record:{name}"));
    }
    let bytes = fs::read(&file).map_err(|_| format!("MISSING_RECORD:{name}"))?;
    serde_json::from_slice(&bytes).map_err(|_| format!("INVALID_RECORD:{name}"))
}

fn require_array(record: &Value, path: &str, name: &str) -> Result<(), String> {
    if record
        .pointer(path)
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .is_none()
    {
        return Err(format!("INVALID_RECORD:{name}:{path}"));
    }
    Ok(())
}

fn require_array_or_empty(record: &Value, path: &str, name: &str) -> Result<(), String> {
    if record.pointer(path).and_then(Value::as_array).is_none() {
        return Err(format!("INVALID_RECORD:{name}:{path}"));
    }
    Ok(())
}

fn require_object(record: &Value, path: &str, name: &str) -> Result<(), String> {
    if record.pointer(path).and_then(Value::as_object).is_none() {
        return Err(format!("INVALID_RECORD:{name}:{path}"));
    }
    Ok(())
}

fn lifecycle_state_machine() -> Result<StateMachine, pipeline_runtime::CompileError> {
    StateMachine::new(
        [
            "open",
            "claimed",
            "candidate_verified",
            "architecture_reviewed",
            "effectiveness_verified",
            "remote_verified",
            "promoted",
        ]
        .into_iter()
        .map(str::to_owned),
        vec![
            transition("open", "claim", "claimed"),
            transition("claimed", "candidate", "candidate_verified"),
            transition("candidate_verified", "review_pass", "architecture_reviewed"),
            transition(
                "architecture_reviewed",
                "effectiveness_replay",
                "effectiveness_verified",
            ),
            transition(
                "effectiveness_verified",
                "remote_receipt",
                "remote_verified",
            ),
            transition("remote_verified", "promote", "promoted"),
        ],
    )
}

fn transition(from: &str, event: &str, to: &str) -> Transition {
    Transition {
        from: from.to_owned(),
        event: event.to_owned(),
        to: to.to_owned(),
    }
}

struct UtcStamp;

impl UtcStamp {
    fn now() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fix_and_notification_graphs_are_single_source_single_sink() {
        for source in [FIX_LIFECYCLE_GRAPH, NOTIFICATION_LIFECYCLE_GRAPH] {
            let graph = parse_graph_json(source).unwrap();
            ensure_single_source_single_sink(&graph).unwrap();
        }
    }

    #[test]
    fn graph_manifest_covers_every_embedded_graph_and_validate_reports_both() {
        let result = validate_graph_contracts().unwrap();
        let graph_ids = result["graphs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|graph| graph["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(graph_ids.contains(&"appsdk-fix-lifecycle"));
        assert!(graph_ids.contains(&"appsdk-notification-object"));
        assert!(result["manifest"]["graphs"].as_array().unwrap().len() == 2);
    }

    #[test]
    fn single_source_single_sink_rejects_fan_out() {
        let mut graph = parse_graph_json(NOTIFICATION_LIFECYCLE_GRAPH).unwrap();
        graph.nodes[0].inputs = vec![
            graph.nodes[0].inputs[0].clone(),
            "notification_draft".to_owned(),
        ];
        let error = ensure_single_source_single_sink(&graph).unwrap_err();
        assert!(error.starts_with("DAGPIPE_NODE_MUST_HAVE_SINGLE_INPUT"));
    }

    #[test]
    fn single_source_single_sink_rejects_multiple_graph_outputs() {
        let mut graph = parse_graph_json(FIX_LIFECYCLE_GRAPH).unwrap();
        graph.outputs.push("candidate_state".to_owned());
        let error = ensure_single_source_single_sink(&graph).unwrap_err();
        assert!(error.starts_with("DAGPIPE_GRAPH_MUST_BE_SINGLE_SOURCE_SINGLE_SINK"));
    }

    #[test]
    fn single_source_single_sink_rejects_unbound_or_unconsumed_outputs() {
        let mut graph = parse_graph_json(FIX_LIFECYCLE_GRAPH).unwrap();
        graph.outputs = vec!["missing_state".to_owned()];
        let error = ensure_single_source_single_sink(&graph).unwrap_err();
        assert!(error.starts_with("DAGPIPE_GRAPH_OUTPUT_UNBOUND"));

        let mut graph = parse_graph_json(FIX_LIFECYCLE_GRAPH).unwrap();
        graph.nodes.last_mut().unwrap().inputs[0] = "orphan_state".to_owned();
        let error = ensure_single_source_single_sink(&graph).unwrap_err();
        assert!(error.starts_with("DAGPIPE_ARC_UNCONSUMED"));
    }

    #[test]
    fn single_source_single_sink_rejects_cycle_that_passes_local_shape_checks() {
        let graph = parse_graph_json(
            r#"{
              "id": "cycle",
              "version": "0.1.0",
              "inputs": [{"id": "source_arc", "schema": "Any"}],
              "nodes": [
                {"id": "a", "operator": "x", "operator_version": "1", "inputs": ["arc_b"],
                 "output": {"id": "arc_a", "schema": "Any"},
                 "input_selector": {"include": [], "exclude": [], "predicate": null},
                 "output_selector": {"include": [], "exclude": [], "predicate": null},
                 "iterator": "Whole"},
                {"id": "b", "operator": "y", "operator_version": "1", "inputs": ["arc_a"],
                 "output": {"id": "arc_b", "schema": "Any"},
                 "input_selector": {"include": [], "exclude": [], "predicate": null},
                 "output_selector": {"include": [], "exclude": [], "predicate": null},
                 "iterator": "Whole"}
              ],
              "edges": [
                {"from": "a", "to": "b", "arc_id": "arc_a"},
                {"from": "b", "to": "a", "arc_id": "arc_b"}
              ],
              "outputs": ["arc_a"]
            }"#,
        )
        .unwrap();
        let error = ensure_single_source_single_sink(&graph).unwrap_err();
        assert!(
            error.starts_with("DAGPIPE_GRAPH_TOPOLOGY_INVALID"),
            "{error}"
        );
        assert!(error.contains("cycle"), "{error}");
    }

    #[test]
    fn single_source_single_sink_rejects_edge_arc_endpoint_mismatch() {
        // Passes the local per-node/per-ARC shape checks: every node has one
        // input, every produced ARC is consumed, and the declared output is
        // bound. Only the shared topology validator rejects the edge whose
        // `arc_id` is not the source node's output.
        let graph = parse_graph_json(
            r#"{
              "id": "endpoint_mismatch",
              "version": "0.1.0",
              "inputs": [{"id": "source_arc", "schema": "Any"}],
              "nodes": [
                {"id": "a", "operator": "x", "operator_version": "1", "inputs": ["source_arc"],
                 "output": {"id": "arc_a", "schema": "Any"},
                 "input_selector": {"include": [], "exclude": [], "predicate": null},
                 "output_selector": {"include": [], "exclude": [], "predicate": null},
                 "iterator": "Whole"},
                {"id": "b", "operator": "y", "operator_version": "1", "inputs": ["arc_a"],
                 "output": {"id": "arc_b", "schema": "Any"},
                 "input_selector": {"include": [], "exclude": [], "predicate": null},
                 "output_selector": {"include": [], "exclude": [], "predicate": null},
                 "iterator": "Whole"},
                {"id": "c", "operator": "z", "operator_version": "1", "inputs": ["arc_b"],
                 "output": {"id": "arc_c", "schema": "Any"},
                 "input_selector": {"include": [], "exclude": [], "predicate": null},
                 "output_selector": {"include": [], "exclude": [], "predicate": null},
                 "iterator": "Whole"}
              ],
              "edges": [
                {"from": "a", "to": "b", "arc_id": "arc_b"}
              ],
              "outputs": ["arc_c"]
            }"#,
        )
        .unwrap();
        let error = ensure_single_source_single_sink(&graph).unwrap_err();
        assert!(
            error.starts_with("DAGPIPE_GRAPH_TOPOLOGY_INVALID"),
            "{error}"
        );
    }
}
