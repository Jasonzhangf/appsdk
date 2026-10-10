use super::helpers::*;
use super::*;
pub(super) fn validate_communication_root_input(root: &Path) -> CommResult<()> {
    if !is_lexically_canonical_absolute(root) {
        return Err(CommError::new(
            "communication_root_not_canonical",
            format!(
                "communication root must be an absolute canonical path: {}",
                root.display()
            ),
        ));
    }
    reject_symlink_components(root, "communication_root")?;
    if root.exists() && !root.is_dir() {
        return Err(CommError::new(
            "communication_root_not_directory",
            format!("communication root is not a directory: {}", root.display()),
        ));
    }
    Ok(())
}

pub(super) fn validate_communication_root(root: &Path) -> CommResult<PathBuf> {
    validate_communication_root_input(root)?;
    let canonical = root.canonicalize().map_err(|error| {
        CommError::new(
            "communication_root_canonicalize_failed",
            format!("{}: {error}", root.display()),
        )
    })?;
    if canonical != root && !is_platform_root_alias(root, &canonical) {
        return Err(CommError::new(
            "communication_root_not_canonical",
            format!(
                "communication root is not canonical: expected {}, got {}",
                canonical.display(),
                root.display()
            ),
        ));
    }
    Ok(canonical)
}

pub(super) fn infer_project_root(mailbox_path: &Path) -> CommResult<PathBuf> {
    if !is_lexically_canonical_absolute(mailbox_path) {
        return Err(CommError::new(
            "communication_mailbox_not_canonical",
            format!(
                "communication mailbox must be an absolute canonical path: {}",
                mailbox_path.display()
            ),
        ));
    }
    let communication = mailbox_path.parent().ok_or_else(|| {
        CommError::new(
            "communication_mailbox_layout_invalid",
            "communication mailbox has no parent directory",
        )
    })?;
    let control = communication.parent().ok_or_else(|| {
        CommError::new(
            "communication_mailbox_layout_invalid",
            "communication mailbox has no control directory",
        )
    })?;
    let root = control.parent().ok_or_else(|| {
        CommError::new(
            "communication_mailbox_layout_invalid",
            "communication mailbox has no project root",
        )
    })?;
    if mailbox_path.file_name().and_then(|name| name.to_str()) != Some("mailbox.jsonl")
        || communication.file_name().and_then(|name| name.to_str()) != Some("communication")
        || control.file_name().and_then(|name| name.to_str()) != Some(".appsdk-control")
    {
        return Err(CommError::new(
            "communication_mailbox_layout_invalid",
            format!(
                "communication mailbox must be <project>/.appsdk-control/communication/mailbox.jsonl: {}",
                mailbox_path.display()
            ),
        ));
    }
    validate_communication_root(root)
}

pub(super) fn reject_symlink_components(path: &Path, label: &str) -> CommResult<()> {
    if !path.is_absolute() {
        return Err(CommError::new(
            "communication_path_not_absolute",
            format!("{label} path must be absolute: {}", path.display()),
        ));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let canonical = fs::canonicalize(&current).ok();
                if canonical
                    .as_deref()
                    .is_none_or(|canonical| !is_platform_root_alias(&current, canonical))
                {
                    return Err(CommError::new(
                        "communication_path_symlink",
                        format!("{label} path contains symlink: {}", current.display()),
                    ));
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(CommError::new(
                    "communication_path_stat_failed",
                    format!("{label} path stat failed at {}: {error}", current.display()),
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn is_platform_root_alias(path: &Path, canonical: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(relative) = path.strip_prefix("/").ok() else {
            return false;
        };
        return canonical == Path::new("/private").join(relative);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, canonical);
        false
    }
}

pub(super) fn is_lexically_canonical_absolute(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let Some(raw) = path.to_str() else {
        return false;
    };
    let separator = std::path::MAIN_SEPARATOR_STR;
    if raw.len() > separator.len() && raw.ends_with(separator) {
        return false;
    }
    let mut normalized = PathBuf::new();
    // Byte length of the leading prefix and root. On Windows the native
    // canonical prefix (`\\?\C:`) carries its own separator, so the
    // redundant-separator scan below must start after it instead of treating
    // that structural separator as an empty component.
    let mut prefix_root_bytes = 0;
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => {
                prefix_root_bytes += prefix.as_os_str().len();
                normalized.push(prefix.as_os_str());
            }
            std::path::Component::RootDir => {
                prefix_root_bytes += separator.len();
                normalized.push(Path::new(separator));
            }
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::CurDir | std::path::Component::ParentDir => return false,
        }
    }
    let remainder = raw.get(prefix_root_bytes..).unwrap_or("");
    if remainder
        .split(separator)
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return false;
    }
    normalized == path
}

pub(super) fn validate_event_envelope(event: &EventRecord) -> CommResult<()> {
    if event.event_id.trim().is_empty() {
        return Err(CommError::new(
            "event_envelope_invalid",
            "eventId must be non-empty",
        ));
    }
    validate_time(&event.at).map_err(|error| {
        CommError::new(
            "event_envelope_invalid",
            format!("at must be a valid RFC3339 timestamp: {error}"),
        )
    })?;
    Ok(())
}

pub(super) fn validate_scope_request(request: &ScopeRequest) -> CommResult<()> {
    validate_non_empty(&request.scope_id, "scopeId")?;
    validate_non_empty(&request.appserver_id, "appserverId")?;
    validate_non_empty(&request.endpoint, "endpoint")?;
    validate_non_empty(&request.project_root, "projectRoot")?;
    if !matches!(request.namespace.as_str(), "codex_app" | "codex_tui") {
        return Err(CommError::new(
            "invalid_namespace",
            format!(
                "namespace must be codex_app or codex_tui: {}",
                request.namespace
            ),
        ));
    }
    for session in &request.session_ids {
        validate_non_empty(session, "sessionIds[]")?;
    }
    if request
        .runtime_id
        .as_deref()
        .is_none_or(|runtime_id| runtime_id.trim().is_empty())
    {
        return Err(CommError::new(
            "runtime_registration_required",
            "scope registration requires runtimeId",
        ));
    }
    Ok(())
}

pub(super) fn validate_adapter_runtime_target(
    kind: &str,
    target: &str,
    runtime: &global_registry::RuntimeRecord,
    stale: bool,
) -> CommResult<()> {
    match kind {
        "appserver" => {
            if target != runtime.identity.endpoint {
                return Err(CommError::new(
                    if stale {
                        "appserver_target_stale"
                    } else {
                        "appserver_target_mismatch"
                    },
                    format!(
                        "appserver adapter target {target} does not match recipient runtime endpoint {}",
                        runtime.identity.endpoint
                    ),
                ));
            }
            if !runtime
                .identity
                .capabilities
                .iter()
                .any(|capability| capability == APPSERVER_SEND_CAPABILITY)
            {
                return Err(CommError::new(
                    "appserver_capability_missing",
                    format!(
                        "recipient runtime {} does not declare capability {}",
                        runtime.identity.runtime_id, APPSERVER_SEND_CAPABILITY
                    ),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn validate_message_request(request: &MessageRequest) -> CommResult<()> {
    validate_address(&request.from)?;
    validate_address(&request.to)?;
    validate_non_empty(&request.title, "title")?;
    if request.title.chars().count() > 200 {
        return Err(CommError::new(
            "title_too_long",
            "message title must be at most 200 characters",
        ));
    }
    validate_non_empty(&request.body, "body")?;
    if let Some(key) = request.coalesce_key.as_deref() {
        validate_non_empty(key, "coalesceKey")?;
    }
    Ok(())
}

pub(super) fn delivery_state_rank(state: &str) -> Option<u8> {
    match state {
        "created" => Some(0),
        "accepted" | "intent" => Some(1),
        "delivered" => Some(2),
        "executed" => Some(3),
        "replied" => Some(4),
        "read" => Some(5),
        "consumed" => Some(6),
        "unknown" => Some(0),
        _ => None,
    }
}

pub(super) fn validate_delivery_state_transition(current: &str, next: &str) -> CommResult<()> {
    let current_rank = delivery_state_rank(current).ok_or_else(|| {
        CommError::new(
            "delivery_state_invalid",
            format!("message has invalid current state: {current}"),
        )
    })?;
    let next_rank = delivery_state_rank(next).ok_or_else(|| {
        CommError::new(
            "delivery_state_invalid",
            format!("message has invalid next state: {next}"),
        )
    })?;
    let delivered_rank = delivery_state_rank("delivered").expect("known delivery state");
    if (next == "unknown" && current_rank >= delivered_rank)
        || (next != "unknown" && next_rank < current_rank)
    {
        return Err(CommError::new(
            "delivery_state_regression",
            format!("cannot move message from {current} to {next}"),
        ));
    }
    Ok(())
}

pub(super) fn validate_message_delivery_attempt(
    attempt: &MessageDeliveryAttempt,
    message: &MessageRecord,
    target: &AgentRecord,
    runtime: &global_registry::RuntimeRecord,
    adapter: &AdapterRecord,
    require_current_runtime: bool,
) -> CommResult<()> {
    validate_non_empty(&attempt.attempt_id, "attemptId")?;
    validate_non_empty(&attempt.message_id, "messageId")?;
    validate_non_empty(&attempt.operation, "operation")?;
    validate_non_empty(&attempt.adapter_id, "adapterId")?;
    validate_non_empty(&attempt.runtime_id, "runtimeId")?;
    validate_non_empty(&attempt.runtime_fingerprint, "runtimeFingerprint")?;
    validate_non_empty(&attempt.nonce, "nonce")?;
    validate_time(&attempt.started_at)?;
    if attempt.operation != "message.delivery" {
        return Err(CommError::new(
            "delivery_attempt_operation_invalid",
            format!(
                "unsupported message delivery attempt operation: {}",
                attempt.operation
            ),
        ));
    }
    if attempt.message_id != message.message_id {
        return Err(CommError::new(
            "delivery_attempt_message_mismatch",
            "delivery attempt does not match message",
        ));
    }
    if attempt.adapter_id != message.adapter_id || attempt.adapter_id != adapter.adapter_id {
        return Err(CommError::new(
            "delivery_attempt_adapter_mismatch",
            "delivery attempt does not match message adapter",
        ));
    }
    let target_runtime_id = target.runtime_id.as_deref().ok_or_else(|| {
        CommError::new(
            "runtime_registration_required",
            format!(
                "message target has no runtime identity: {}",
                message.to.key()
            ),
        )
    })?;
    if attempt.runtime_id != target_runtime_id || runtime.identity.runtime_id != attempt.runtime_id
    {
        return Err(CommError::new(
            "delivery_attempt_runtime_mismatch",
            "delivery attempt does not match message target runtime",
        ));
    }
    if require_current_runtime {
        if attempt.runtime_fingerprint != runtime.fingerprint {
            return Err(CommError::new(
                "delivery_attempt_runtime_stale",
                "delivery attempt runtime binding is stale",
            ));
        }
    } else if !global_registry::runtime_fingerprint_known(
        &attempt.runtime_id,
        &attempt.runtime_fingerprint,
    )
    .map_err(|error| CommError::new("runtime_registration_required", error))?
    {
        return Err(CommError::new(
            "delivery_attempt_runtime_unknown",
            "delivery attempt runtime fingerprint is not registered",
        ));
    }
    if attempt.target != adapter.target {
        return Err(CommError::new(
            "delivery_attempt_target_mismatch",
            "delivery attempt target does not match adapter target",
        ));
    }
    Ok(())
}

pub(super) fn validate_adapter_delivery_receipt(
    kind: &str,
    receipt: &Value,
    runtime_id: &str,
    state: &str,
) -> CommResult<()> {
    let object = receipt.as_object().ok_or_else(|| {
        CommError::new(
            "delivery_evidence_invalid",
            "delivery evidence must match the registered adapter receipt contract",
        )
    })?;
    match kind {
        "mailbox" => {
            if object.get("durable").and_then(Value::as_bool) != Some(true) {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "mailbox delivery evidence must confirm a durable mailbox receipt",
                ));
            }
            if object
                .get("format")
                .and_then(Value::as_str)
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "mailbox delivery evidence must include a non-empty receipt format",
                ));
            }
        }
        "appserver" => {
            if object.get("hostMustExecute").and_then(Value::as_bool) != Some(true) {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "appserver delivery evidence must confirm host must execute",
                ));
            }
            if object.get("capability").and_then(Value::as_str) != Some(APPSERVER_SEND_CAPABILITY) {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "appserver delivery evidence must include the send capability contract",
                ));
            }
            if object.get("runtimeId").and_then(Value::as_str) != Some(runtime_id) {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "appserver delivery evidence runtimeId does not match delivery request",
                ));
            }
            if matches!(
                state,
                "delivered" | "executed" | "replied" | "read" | "consumed"
            ) && object.get("hostExecuted").and_then(Value::as_bool) != Some(true)
            {
                return Err(CommError::new(
                    "delivery_evidence_invalid",
                    "appserver terminal delivery evidence must include independent host execution evidence",
                ));
            }
        }
        other => {
            return Err(CommError::new(
                "invalid_adapter_kind",
                format!("adapter kind is unsupported: {other}"),
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_replayed_delivery_evidence(
    state: &str,
    evidence: &DeliveryEvidence,
    target: &AgentRecord,
    message_id: &str,
    message: &MessageRecord,
    attempt: Option<&MessageDeliveryAttempt>,
    event_attempt_id: Option<&str>,
    event_nonce: Option<&str>,
    adapter: &AdapterRecord,
) -> CommResult<()> {
    if !matches!(
        state,
        "delivered" | "executed" | "replied" | "read" | "consumed" | "unknown"
    ) {
        return Ok(());
    }
    let details = evidence.details.as_object().ok_or_else(|| {
        CommError::new(
            "event_data_invalid",
            "external delivery evidence must be a non-empty object",
        )
    })?;
    if details.is_empty() {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence must be a non-empty object",
        ));
    }
    let runtime_id = details
        .get("runtimeId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CommError::new(
                "event_data_invalid",
                "external delivery evidence runtimeId is missing",
            )
        })?;
    let fingerprint = details
        .get("runtimeFingerprint")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CommError::new(
                "event_data_invalid",
                "external delivery evidence runtimeFingerprint is missing",
            )
        })?;
    if details
        .get("receipt")
        .and_then(Value::as_object)
        .is_none_or(|value| value.is_empty())
    {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence receipt must be a non-empty object",
        ));
    }
    let target_runtime_id = target.runtime_id.as_deref().ok_or_else(|| {
        CommError::new(
            "event_data_invalid",
            "external delivery evidence target has no runtime identity",
        )
    })?;
    if target_runtime_id != runtime_id {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence runtimeId does not match message target",
        ));
    }
    let runtime = global_registry::runtime_for_replay(runtime_id, fingerprint)
        .map_err(|error| CommError::new("event_data_invalid", error))?;
    let known = global_registry::runtime_fingerprint_known(runtime_id, fingerprint)
        .map_err(|error| CommError::new("event_data_invalid", error))?;
    if !known {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence runtimeFingerprint is not registered",
        ));
    }

    // Messages written before the persisted-attempt contract are identified
    // by the absence of the explicit marker. Their historical receipts remain
    // replayable under the old runtime/fingerprint/receipt contract. Any
    // attempt metadata, or any persisted attempt, moves the event to the
    // strict contract instead of silently treating missing fields as legacy.
    let has_attempt_metadata = event_attempt_id.is_some()
        || event_nonce.is_some()
        || details.contains_key("attemptId")
        || details.contains_key("nonce")
        || details.contains_key("adapterId")
        || details.contains_key("target");
    let receipt = details.get("receipt").expect("receipt was checked above");
    if !message.delivery_attempt_required && attempt.is_none() && !has_attempt_metadata {
        if adapter.kind == "appserver"
            && matches!(
                state,
                "delivered" | "executed" | "replied" | "read" | "consumed"
            )
        {
            validate_adapter_delivery_receipt(&adapter.kind, receipt, runtime_id, state)?;
        }
        return Ok(());
    }
    validate_adapter_delivery_receipt(&adapter.kind, receipt, runtime_id, state)?;

    let attempt_id = event_attempt_id
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CommError::new(
                "event_data_invalid",
                "external delivery evidence attemptId is missing",
            )
        })?;
    let nonce = event_nonce
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CommError::new(
                "event_data_invalid",
                "external delivery evidence nonce is missing",
            )
        })?;
    if details.get("attemptId").and_then(Value::as_str) != Some(attempt_id)
        || details.get("nonce").and_then(Value::as_str) != Some(nonce)
    {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence attempt identity does not match message state",
        ));
    }
    let adapter_id = details
        .get("adapterId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            CommError::new(
                "event_data_invalid",
                "external delivery evidence adapterId is missing",
            )
        })?;
    if adapter_id != message.adapter_id {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence adapterId does not match message",
        ));
    }
    let expected_target = serde_json::to_value(&adapter.target).unwrap();
    let observed_target = details.get("target").cloned().unwrap_or(Value::Null);
    if observed_target != expected_target {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence target does not match adapter",
        ));
    }
    let attempt = attempt.ok_or_else(|| {
        CommError::new(
            "event_data_invalid",
            format!("message {message_id} has no persisted delivery attempt"),
        )
    })?;
    if attempt.attempt_id != attempt_id || attempt.nonce != nonce {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence does not match persisted delivery attempt",
        ));
    }
    if attempt.runtime_fingerprint != fingerprint {
        return Err(CommError::new(
            "event_data_invalid",
            "external delivery evidence runtimeFingerprint does not match attempt",
        ));
    }
    validate_message_delivery_attempt(attempt, message, target, &runtime, adapter, false)?;
    Ok(())
}
