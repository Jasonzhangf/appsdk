use super::*;
use std::collections::HashSet;
use std::time::{Duration, Instant};

pub(crate) const LIVE_CLOSURE_TIMEOUT_MS_ENV: &str = "COLLAB_LIVE_CLOSURE_TIMEOUT_MS";
pub(crate) const DEFAULT_LIVE_CLOSURE_TIMEOUT_MS: u64 = 180_000;
pub(crate) const MAX_LIVE_CLOSURE_TIMEOUT_MS: u64 = 3_600_000;

pub(crate) fn live_closure_timeout_from_value(value: Option<&str>) -> anyhow::Result<Duration> {
    let milliseconds = match value {
        None => DEFAULT_LIVE_CLOSURE_TIMEOUT_MS,
        Some(value) => value.parse::<u64>().map_err(|error| {
            anyhow::anyhow!(
                "COLLAB_LIVE_CLOSURE_TIMEOUT_INVALID:{LIVE_CLOSURE_TIMEOUT_MS_ENV}:{error}"
            )
        })?,
    };
    if milliseconds == 0 || milliseconds > MAX_LIVE_CLOSURE_TIMEOUT_MS {
        anyhow::bail!(
            "COLLAB_LIVE_CLOSURE_TIMEOUT_INVALID:{LIVE_CLOSURE_TIMEOUT_MS_ENV}:must_be_between_1_and_{MAX_LIVE_CLOSURE_TIMEOUT_MS}_milliseconds"
        );
    }
    Ok(Duration::from_millis(milliseconds))
}

pub(crate) fn live_closure_timeout() -> anyhow::Result<Duration> {
    live_closure_timeout_from_value(std::env::var(LIVE_CLOSURE_TIMEOUT_MS_ENV).ok().as_deref())
}

pub(crate) fn live_closure_receipt_consumed(
    receipt: &serde_json::Value,
    message_id: &str,
    challenge: &str,
) -> bool {
    receipt.get("id").and_then(serde_json::Value::as_str) == Some(message_id)
        && receipt.get("state").and_then(serde_json::Value::as_str) == Some("read")
        && receipt.get("body").and_then(serde_json::Value::as_str) == Some(challenge)
}

pub(crate) fn wait_live_closure_receipt<F>(
    mut read: F,
    deadline: Instant,
    message_id: &str,
    challenge: &str,
) -> anyhow::Result<serde_json::Value>
where
    F: FnMut() -> anyhow::Result<serde_json::Value>,
{
    loop {
        let receipt = read()?;
        if live_closure_receipt_consumed(&receipt, message_id, challenge) {
            return Ok(receipt);
        }
        if Instant::now() >= deadline {
            anyhow::bail!("COLLAB_LIVE_CLOSURE_RECEIPT_NOT_CONSUMED");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[derive(Debug)]
#[cfg(test)]
pub(crate) struct LiveClosureExpectedNativeInputs {
    pub(crate) exact: Vec<String>,
    pub(crate) batch_category: String,
}

#[cfg(test)]
pub(crate) fn live_closure_item_text(item: &serde_json::Value) -> Option<&str> {
    let payload = live_closure_item_payload(item);
    let item_type = payload.get("type").and_then(serde_json::Value::as_str);
    if !matches!(item_type, Some("userMessage") | Some("user_message")) {
        return None;
    }
    let content = payload
        .get("content")
        .and_then(serde_json::Value::as_array)?;
    if content.len() != 1
        || content[0].get("type").and_then(serde_json::Value::as_str) != Some("text")
    {
        return None;
    }
    content[0].get("text").and_then(serde_json::Value::as_str)
}

#[cfg(test)]
pub(crate) fn live_closure_item_contains_challenge(
    item: &serde_json::Value,
    challenge: &str,
) -> bool {
    live_closure_item_text(item) == Some(challenge)
}

#[cfg(test)]
pub(crate) fn live_closure_item_payload(item: &serde_json::Value) -> &serde_json::Value {
    item.get("item").unwrap_or(item)
}

#[cfg(test)]
pub(crate) fn live_closure_item_turn_id(item: &serde_json::Value) -> Option<&str> {
    let turn_id = item
        .get("turnId")
        .or_else(|| item.get("turn_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty());
    if turn_id.is_some() {
        return turn_id;
    }
    let payload = item.get("item")?;
    payload
        .get("turnId")
        .or_else(|| payload.get("turn_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
pub(crate) fn live_closure_item_message_id(item: &serde_json::Value) -> Option<&str> {
    let payload = live_closure_item_payload(item);
    payload
        .get("clientUserMessageId")
        .or_else(|| payload.get("clientId"))
        .and_then(serde_json::Value::as_str)
        .and_then(|value| value.strip_prefix("collab-notification-").or(Some(value)))
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
pub(crate) fn live_closure_function_call_output_fields(
    item: &serde_json::Value,
) -> Option<(&str, &str)> {
    let payload = live_closure_item_payload(item);
    if payload.get("type").and_then(serde_json::Value::as_str) != Some("functionCallOutput")
        || payload.get("name").and_then(serde_json::Value::as_str) != Some("send_message_to_thread")
    {
        return None;
    }
    live_closure_item_turn_id(item)?;
    let output = payload.get("output").and_then(serde_json::Value::as_str)?;
    let mut lines = output.lines();
    if lines.next()? != "<codex_delegation>" {
        return None;
    }
    let mut next_line = lines.next()?;
    if let Some(source_thread_id) = next_line
        .strip_prefix("  <source_thread_id>")
        .and_then(|value| value.strip_suffix("</source_thread_id>"))
    {
        if source_thread_id.is_empty()
            || source_thread_id.trim() != source_thread_id
            || source_thread_id.contains('<')
            || source_thread_id.contains('>')
            || source_thread_id.contains('&')
        {
            return None;
        }
        next_line = lines.next()?;
    }
    let client_message_id = next_line
        .strip_prefix("  <client_message_id>")?
        .strip_suffix("</client_message_id>")?;
    let client_message_id = client_message_id
        .strip_prefix("collab-notification-")
        .unwrap_or(client_message_id);
    if client_message_id.is_empty()
        || client_message_id.trim() != client_message_id
        || client_message_id.contains('<')
        || client_message_id.contains('>')
        || client_message_id.contains('&')
    {
        return None;
    }
    let challenge = lines
        .next()?
        .strip_prefix("  <input>")?
        .strip_suffix("</input>")?;
    if challenge.is_empty() || challenge.trim() != challenge {
        return None;
    }
    if lines.next()? != "</codex_delegation>" || lines.next().is_some() {
        return None;
    }
    Some((client_message_id, challenge))
}

#[cfg(test)]
pub(crate) fn live_closure_item_matches_input(
    item: &serde_json::Value,
    expected_inputs: &LiveClosureExpectedNativeInputs,
    message_id: &str,
) -> bool {
    live_closure_item_turn_id(item).is_some()
        && (live_closure_item_text(item).is_some_and(|text| {
            (live_closure_item_message_id(item) == Some(message_id)
                && expected_inputs
                    .exact
                    .iter()
                    .any(|expected_input| text == expected_input))
                || live_closure_batch_input_contains_message(
                    text,
                    message_id,
                    &expected_inputs.batch_category,
                    false,
                )
        }) || live_closure_function_call_output_fields(item).is_some_and(
            |(observed_message_id, observed)| {
                (observed_message_id == message_id
                    && expected_inputs.exact.iter().any(|expected_input| {
                        observed
                            == client::adapters::codex_app_server::escape_delegated_text(
                                expected_input,
                            )
                    }))
                    || live_closure_batch_input_contains_message(
                        observed,
                        message_id,
                        &expected_inputs.batch_category,
                        true,
                    )
            },
        ))
}

#[cfg(test)]
pub(crate) fn live_closure_expected_native_inputs(
    receipt: &serde_json::Value,
    message_id: &str,
    challenge: &str,
) -> anyhow::Result<LiveClosureExpectedNativeInputs> {
    let receipt_type = receipt
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let supported_type = matches!(receipt_type, "notify" | "notification");
    if receipt.get("id").and_then(serde_json::Value::as_str) != Some(message_id)
        || receipt.get("body").and_then(serde_json::Value::as_str) != Some(challenge)
        || receipt.get("subject").and_then(serde_json::Value::as_str) != Some(challenge)
        || !supported_type
    {
        anyhow::bail!("COLLAB_LIVE_CLOSURE_MESSAGE_BINDING_MISMATCH");
    }
    let message = server::state::Message {
        id: message_id.to_owned(),
        from: receipt
            .get("from")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_MESSAGE_SENDER_MISSING"))?
            .to_owned(),
        to: receipt
            .get("to")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_MESSAGE_RECIPIENT_MISSING"))?
            .to_owned(),
        mtype: receipt_type.to_owned(),
        subject: Some(challenge.to_owned()),
        body: challenge.to_owned(),
        in_reply_to: None,
        created_ms: 0,
        state: receipt
            .get("state")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("pending")
            .to_owned(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    let notification = server::mailbox::notification_text(&message)
        .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_NATIVE_INPUT_UNAVAILABLE"))?;
    let batch_category = live_closure_notification_category(&notification)
        .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_NATIVE_INPUT_UNAVAILABLE"))?
        .to_owned();
    let batch = server::mailbox::truncate_notification(server::mailbox::compose_notification(
        message_id,
        "notification-batch",
        &server::mailbox::batch_notification_text(
            &[(
                0,
                message_id.to_owned(),
                String::new(),
                "direct-message".to_owned(),
                notification.clone(),
            )],
            0,
        ),
    ));
    Ok(LiveClosureExpectedNativeInputs {
        exact: vec![notification, batch],
        batch_category,
    })
}

#[cfg(test)]
pub(crate) fn live_closure_notification_category(notification: &str) -> Option<&str> {
    notification
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(category, _)| category)
}

#[cfg(test)]
pub(crate) fn live_closure_batch_input_contains_message(
    observed: &str,
    message_id: &str,
    expected_category: &str,
    xml_escaped: bool,
) -> bool {
    let expected_category = if xml_escaped {
        client::adapters::codex_app_server::escape_delegated_text(expected_category)
    } else {
        expected_category.to_owned()
    };
    let Some((_, body)) = observed.split_once(" [notification-batch] Batch wake: message_ids=")
    else {
        return false;
    };
    let Some((ids, rest)) = body.split_once(" task_ids=none action_categories=") else {
        return false;
    };
    let Some((categories, _)) = rest.split_once(". Read full durable details from collab inbox;")
    else {
        return false;
    };
    ids.split(',')
        .zip(categories.split(','))
        .any(|(id, category)| id == message_id && category == expected_category)
}

#[cfg(test)]
pub(crate) fn live_closure_page_cursor(page: &serde_json::Value) -> anyhow::Result<Option<String>> {
    for key in ["backwardsCursor", "nextCursor"] {
        let Some(value) = page.get(key) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        let cursor = value
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_PAGE_CURSOR_INVALID:{key}"))?;
        if !cursor.trim().is_empty() {
            return Ok(Some(cursor.to_owned()));
        }
    }
    Ok(None)
}

#[cfg(test)]
pub(crate) fn read_live_closure_pages<F>(mut read_page: F) -> anyhow::Result<Vec<serde_json::Value>>
where
    F: FnMut(Option<&str>) -> anyhow::Result<serde_json::Value>,
{
    let mut cursor = None;
    let mut seen_cursors = HashSet::new();
    let mut values = Vec::new();
    loop {
        let page = read_page(cursor.as_deref())?;
        let data = page
            .get("data")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_TARGET_ITEMS_INVALID"))?;
        values.extend(data.iter().cloned());
        let Some(next_cursor) = live_closure_page_cursor(&page)? else {
            return Ok(values);
        };
        if !seen_cursors.insert(next_cursor.clone()) {
            // Some App Server versions keep the backwards cursor anchored to
            // the same ordinal while returning the next slice. Keep the
            // current page, then stop this scan; the outer observation retry
            // remains bounded and still requires exact native correlation.
            return Ok(values);
        }
        cursor = Some(next_cursor);
    }
}

#[cfg(test)]
pub(crate) fn live_closure_turn_items(
    turns: &[serde_json::Value],
) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut items = Vec::new();
    for (turn_index, turn) in turns.iter().enumerate() {
        let turn_id = turn
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("COLLAB_LIVE_CLOSURE_TARGET_TURN_ID_MISSING:data[{turn_index}]")
            })?;
        let turn_items = turn
            .get("items")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                anyhow::anyhow!("COLLAB_LIVE_CLOSURE_TARGET_TURN_ITEMS_INVALID:data[{turn_index}]")
            })?;
        for (item_index, item) in turn_items.iter().enumerate() {
            let mut item = item.clone();
            let nested_payload = item.get("item");
            if nested_payload.is_some_and(|payload| !payload.is_object()) {
                anyhow::bail!(
                    "COLLAB_LIVE_CLOSURE_TARGET_ITEM_INVALID:data[{turn_index}].items[{item_index}]"
                );
            }
            for (field, value) in [
                ("turnId", item.get("turnId")),
                ("turn_id", item.get("turn_id")),
                (
                    "item.turnId",
                    nested_payload.and_then(|payload| payload.get("turnId")),
                ),
                (
                    "item.turn_id",
                    nested_payload.and_then(|payload| payload.get("turn_id")),
                ),
            ] {
                let Some(value) = value else {
                    continue;
                };
                let observed_turn_id = value
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "COLLAB_LIVE_CLOSURE_TARGET_ITEM_TURN_ID_INVALID:data[{turn_index}].items[{item_index}].{field}"
                        )
                    })?;
                if observed_turn_id != turn_id {
                    anyhow::bail!(
                        "COLLAB_LIVE_CLOSURE_TARGET_ITEM_TURN_MISMATCH:data[{turn_index}].items[{item_index}]"
                    );
                }
            }
            let object = item.as_object_mut().ok_or_else(|| {
                anyhow::anyhow!(
                    "COLLAB_LIVE_CLOSURE_TARGET_ITEM_INVALID:data[{turn_index}].items[{item_index}]"
                )
            })?;
            object.insert("turnId".into(), json!(turn_id));
            items.push(item);
        }
    }
    Ok(items)
}

#[cfg(test)]
pub(crate) fn live_closure_turns_read_error(
    error: client::adapters::AdapterError,
) -> anyhow::Error {
    if matches!(
        &error,
        client::adapters::AdapterError::Unknown { operation: "rpc", detail }
            if detail.as_str() == "list_turns is not supported yet"
    ) {
        return anyhow::anyhow!(
            "COLLAB_LIVE_CLOSURE_TARGET_EXECUTION_PENDING:fresh_thread_materialization:{error}"
        );
    }
    anyhow::anyhow!("COLLAB_LIVE_CLOSURE_TARGET_TURNS_READ:{error}")
}

#[cfg(test)]
pub(crate) fn wait_live_closure_fresh_thread_materialization<F>(
    mut observe: F,
    deadline: Instant,
    retry_delay: Duration,
) -> anyhow::Result<serde_json::Value>
where
    F: FnMut() -> anyhow::Result<serde_json::Value>,
{
    loop {
        match observe() {
            Ok(execution) => return Ok(execution),
            Err(error)
                if error.to_string().starts_with(
                    "COLLAB_LIVE_CLOSURE_TARGET_EXECUTION_PENDING:fresh_thread_materialization:",
                ) =>
            {
                if Instant::now() >= deadline {
                    anyhow::bail!(
                        "COLLAB_LIVE_CLOSURE_TARGET_EXECUTION_TIMEOUT:fresh_thread_materialization"
                    );
                }
                std::thread::sleep(retry_delay);
            }
            Err(error) => return Err(error),
        }
    }
}

pub(crate) fn live_closure_observe(
    scope: &Scope,
    to: String,
    challenge: String,
    message_id: String,
) -> anyhow::Result<()> {
    for (name, value) in [
        ("to", &to),
        ("challenge", &challenge),
        ("message_id", &message_id),
    ] {
        if value.trim().is_empty() {
            anyhow::bail!("COLLAB_LIVE_CLOSURE_OBSERVE_MISSING:{name}");
        }
    }
    let ident = me(scope, None)?;
    let workers: serde_json::Value = call_project(scope, &ident, &Req::Workers)?;
    let target = workers
        .get("workers")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| {
            items.iter().find(|worker| {
                worker.get("id").and_then(serde_json::Value::as_str) == Some(to.as_str())
                    && worker
                        .get("endpoint_live")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
                    && worker
                        .get("identity_valid")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
            })
        })
        .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_TARGET_ROUTE_UNAVAILABLE:{to}"))?;
    let observation_deadline = Instant::now() + live_closure_timeout()?;
    let receipt = wait_live_closure_receipt(
        || {
            call_project(
                scope,
                &ident,
                &Req::MsgStatus {
                    msg_id: message_id.clone(),
                },
            )
        },
        observation_deadline,
        &message_id,
        &challenge,
    )?;
    out(&json!({
        "status": "target_receive_committed",
        "closure_claim": false,
        "target_worker_id": to,
        "receipt": receipt,
        "source": "collab recv durable consumption receipt"
    }));
    Ok(())
}

pub(crate) fn live_closure_daemon_producer_path(path: &str) -> bool {
    path.starts_with("daemon_") || path == "restart_replay"
}

pub(crate) fn live_closure_probe(
    scope: &Scope,
    closure_id: String,
    source_commit: String,
    artifact_hash: String,
    environment_id: String,
    path: String,
    to: String,
    to_project: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    const PATHS: [&str; 7] = [
        "peer_to_peer",
        "peer_to_master",
        "master_to_peer",
        "master_to_master",
        "daemon_to_peer",
        "daemon_to_master",
        "restart_replay",
    ];
    if !PATHS.contains(&path.as_str()) {
        anyhow::bail!("COLLAB_LIVE_CLOSURE_PROBE_INVALID_PATH:{path}");
    }
    for (name, value) in [
        ("closure_id", &closure_id),
        ("source_commit", &source_commit),
        ("artifact_hash", &artifact_hash),
        ("environment_id", &environment_id),
        ("to", &to),
    ] {
        if value.trim().is_empty() {
            anyhow::bail!("COLLAB_LIVE_CLOSURE_PROBE_MISSING:{name}");
        }
    }

    let ident = me(scope, None)?;
    let context: serde_json::Value = call_project(
        scope,
        &ident,
        &Req::Context {
            worker_id: ident.worker_id.clone(),
            token: ident.token.clone(),
        },
    )?;
    let workers: serde_json::Value = call_project(scope, &ident, &Req::Workers)?;
    let master: serde_json::Value = call_project(scope, &ident, &Req::MasterStatus)?;
    let daemon_producer = live_closure_daemon_producer_path(&path);
    let endpoint_generation = ident
        .runtime
        .as_ref()
        .map(|runtime| runtime.endpoint_generation)
        .unwrap_or_default();
    let first_failure = |code: &str, detail: &str| {
        out(&json!({
            "status": "failed",
            "closure_claim": false,
            "entrypoint": "collab live-closure probe",
            "path": path,
            "first_failure": {"code": code, "detail": detail},
            "identity": ident.worker_id.clone(),
            "endpoint_generation": endpoint_generation,
        }));
    };
    if daemon_producer && to_project.is_some() {
        first_failure(
            "COLLAB_LIVE_CLOSURE_TARGET_PROJECT_UNSUPPORTED",
            "daemon-produced live-closure probes are resident-daemon contracts for the current project; cross-project daemon production requires a separate target-side contract",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_TARGET_PROJECT_UNSUPPORTED");
    }
    let target_scope = if path == "master_to_master" {
        let target = to_project
            .ok_or_else(|| anyhow::anyhow!("COLLAB_LIVE_CLOSURE_PROBE_MISSING:to_project"))?
            .canonicalize()?;
        if target == scope.root.canonicalize()? {
            anyhow::bail!("COLLAB_LIVE_CLOSURE_CROSS_PROJECT_REQUIRED");
        }
        if !target.join(".agent-collab").is_dir() {
            anyhow::bail!(
                "COLLAB_LIVE_CLOSURE_TARGET_PROJECT_UNREGISTERED:{}",
                target.display()
            );
        }
        Some(Scope { root: target })
    } else {
        if to_project.is_some() {
            anyhow::bail!("COLLAB_LIVE_CLOSURE_TARGET_PROJECT_ONLY_FOR_MASTER_TO_MASTER");
        }
        None
    };
    let target_workers: serde_json::Value = if let Some(target_scope) = target_scope.as_ref() {
        client::call_with_context(
            &target_scope.sock_path(),
            &Req::Workers,
            Some(cli_project_context(&target_scope.root)?),
        )?
    } else {
        workers.clone()
    };
    let target_master: serde_json::Value = if let Some(target_scope) = target_scope.as_ref() {
        client::call_with_context(
            &target_scope.sock_path(),
            &Req::MasterStatus,
            Some(cli_project_context(&target_scope.root)?),
        )?
    } else {
        master.clone()
    };
    if context
        .get("registered")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
        || context
            .pointer("/liveness/live")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    {
        first_failure(
            "COLLAB_LIVE_CLOSURE_DAEMON_NOT_LIVE",
            "the probe requires an existing registered route and resident daemon",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_DAEMON_NOT_LIVE");
    }
    let target = target_workers
        .get("workers")
        .and_then(serde_json::Value::as_array)
        .and_then(|items| {
            items.iter().find(|worker| {
                worker.get("id").and_then(serde_json::Value::as_str) == Some(to.as_str())
            })
        });
    let Some(target) = target.filter(|worker| {
        worker
            .get("endpoint_live")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
            && worker
                .get("identity_valid")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
    }) else {
        first_failure(
            "COLLAB_LIVE_CLOSURE_TARGET_ROUTE_UNAVAILABLE",
            "target must already be a live authenticated worker",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_TARGET_ROUTE_UNAVAILABLE:{to}");
    };
    let master_id = master
        .pointer("/master/worker_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let target_master_id = target_master
        .pointer("/master/worker_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let sender_role = if daemon_producer {
        "daemon"
    } else if ident.worker_id == master_id {
        "master"
    } else {
        "peer"
    };
    let expected_sender_role = path.split("_to_").next().unwrap_or_default();
    if !daemon_producer && sender_role != expected_sender_role {
        first_failure(
            "COLLAB_LIVE_CLOSURE_SENDER_ROLE_MISMATCH",
            "the probe only sends as the current authenticated worker",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_SENDER_ROLE_MISMATCH");
    }
    if !daemon_producer && ident.worker_id == to {
        first_failure(
            "COLLAB_LIVE_CLOSURE_TARGET_SELF",
            "a closure path requires a distinct target worker",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_TARGET_SELF");
    }
    if path.ends_with("_to_master") && path != "master_to_master" && to != master_id {
        first_failure(
            "COLLAB_LIVE_CLOSURE_MASTER_ROUTE_MISMATCH",
            "the target is not the current live master",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_MASTER_ROUTE_MISMATCH");
    }
    if matches!(path.as_str(), "daemon_to_peer" | "restart_replay")
        && !master_id.is_empty()
        && to == master_id
    {
        first_failure(
            "COLLAB_LIVE_CLOSURE_PEER_ROUTE_MISMATCH",
            "the selected daemon-produced path requires a non-master peer target",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_PEER_ROUTE_MISMATCH");
    }
    if path == "master_to_master" && to != target_master_id {
        first_failure(
            "COLLAB_LIVE_CLOSURE_MASTER_ROUTE_MISMATCH",
            "the target project route is not owned by the requested live master",
        );
        anyhow::bail!("COLLAB_LIVE_CLOSURE_MASTER_ROUTE_MISMATCH");
    }
    let challenge = format!(
        "appsdk-collab-live:{closure_id}:{path}:{source_commit}:{artifact_hash}:{environment_id}:{endpoint_generation}"
    );
    let timeout = live_closure_timeout()?;
    let response: serde_json::Value = if let Some(target_scope) = target_scope.as_ref() {
        let assigned_by = master
            .get("master")
            .and_then(|value| value.get("assigned_by"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let assigned_ms = master
            .get("master")
            .and_then(|value| value.get("assigned_ms"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_default();
        let approval = master
            .get("master")
            .and_then(|value| value.get("approval"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        client::call_with_context(
            &target_scope.sock_path(),
            &Req::CrossProjectSend {
                from: ident.worker_id.clone(),
                from_project: scope.root.display().to_string(),
                source_master_assigned_by: assigned_by.to_owned(),
                source_master_approval: approval,
                source_master_assigned_ms: assigned_ms,
                to: to.clone(),
                subject: challenge.clone(),
                body: challenge.clone(),
                in_reply_to: None,
            },
            Some(cli_project_context(&target_scope.root)?),
        )?
    } else if daemon_producer {
        call_project(
            scope,
            &ident,
            &Req::LiveClosureDaemonSend {
                worker_id: ident.worker_id.clone(),
                token: ident.token.clone(),
                to: to.clone(),
                path: path.clone(),
                subject: challenge.clone(),
                body: challenge.clone(),
                restart_replay_pending: path == "restart_replay",
            },
        )?
    } else {
        let command = command_envelope(scope, &ident)?;
        call_project(
            scope,
            &ident,
            &Req::Send {
                from: ident.worker_id.clone(),
                worker_id: Some(ident.worker_id.clone()),
                token: Some(ident.token.clone()),
                command: Some(command),
                to: to.clone(),
                mtype: "notify".into(),
                subject: Some(challenge.clone()),
                body: challenge.clone(),
                in_reply_to: None,
                delivery: "immediate".into(),
            },
        )?
    };
    let message_id = response
        .get("message_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| response.get("msg_id").and_then(serde_json::Value::as_str))
        .or_else(|| {
            response
                .pointer("/message/id")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or_default();
    if message_id.is_empty() {
        anyhow::bail!("COLLAB_LIVE_CLOSURE_PROBE_MESSAGE_ID_MISSING");
    }
    let observation_deadline = Instant::now() + timeout;
    let receipt = wait_live_closure_receipt(
        || {
            if let Some(target_scope) = target_scope.as_ref() {
                client::call_with_context(
                    &target_scope.sock_path(),
                    &Req::MsgStatus {
                        msg_id: message_id.to_owned(),
                    },
                    Some(cli_project_context(&target_scope.root)?),
                )
            } else {
                call_project(
                    scope,
                    &ident,
                    &Req::MsgStatus {
                        msg_id: message_id.to_owned(),
                    },
                )
            }
        },
        observation_deadline,
        message_id,
        &challenge,
    )
    .map_err(|error| {
        first_failure(
            "COLLAB_LIVE_CLOSURE_RECEIPT_NOT_CONSUMED",
            &error.to_string(),
        );
        error
    })?;
    let target_master_route = target_scope.as_ref().map(|_| {
        json!({
            "worker_id": to,
            "role": "master",
            "endpoint_live": target.get("endpoint_live").and_then(serde_json::Value::as_bool),
            "identity_valid": target.get("identity_valid").and_then(serde_json::Value::as_bool),
            "transport": target.get("transport").cloned().unwrap_or_else(|| json!({})),
        })
    });
    let restart_replay_pending = response
        .get("restart_replay_pending")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    let closure_claim = !restart_replay_pending;
    let status = if restart_replay_pending {
        "restart_replay_pending_observed"
    } else {
        "closure_observed"
    };
    out(&json!({
        "status": status,
        "closure_claim": closure_claim,
        "entrypoint": "collab live-closure probe",
        "path": path,
        "challenge": challenge,
        "message_id": message_id,
        "sender_worker_id": if daemon_producer {
            "collab-server".to_owned()
        } else {
            ident.worker_id.clone()
        },
        "requester_worker_id": ident.worker_id,
        "sender_role": sender_role,
        "target_worker_id": to,
        "target_project_scope": target_scope
            .as_ref()
            .map(|scope| scope.root.display().to_string()),
        "target_master_route": target_master_route,
        "message_project_scope": target_scope
            .as_ref()
            .map(|scope| scope.root.display().to_string()),
        "message_sender": if daemon_producer {
            "collab-server".to_owned()
        } else if target_scope.is_some() {
            format!("{}@{}", ident.worker_id, scope.root.display())
        } else {
            ident.worker_id.clone()
        },
        "daemon_sender": daemon_producer,
        "restart_replay_pending": restart_replay_pending,
        "restart_replay_contract": restart_replay_pending.then(|| json!({
            "status": "pending_daemon_restart",
            "reason": "the challenge was produced by the resident daemon and observed by the target, but no daemon restart/replay occurred in this probe"
        })),
        "endpoint_generation": endpoint_generation,
        "receipt": receipt,
        "source": "collab daemon route + collab recv durable consumption receipt"
    }));
    Ok(())
}
