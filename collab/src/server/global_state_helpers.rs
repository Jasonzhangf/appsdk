use super::global_state_models::*;
use std::path::Path;
pub(super) fn validate_project_scope(scope: &ProjectScopeId) -> Result<(), StateError> {
    ProjectScopeId::new(scope.as_str().to_owned())
        .map_err(|error| StateError::invalid("project scope", error.to_string()))
        .map(|_| ())
}

pub(super) fn current_thread_route_address(
    session_id: &SessionId,
    native_thread_id: &NativeThreadId,
    tmux_endpoint: Option<&TmuxEndpoint>,
) -> (String, String) {
    match tmux_endpoint {
        Some(endpoint)
            if endpoint.codex_session_id.is_none() && endpoint.codex_thread_id.is_none() =>
        {
            tmux_route_address(endpoint)
        }
        _ => (
            session_id.as_str().to_owned(),
            native_thread_id.as_str().to_owned(),
        ),
    }
}

pub(super) fn tmux_route_address(endpoint: &TmuxEndpoint) -> (String, String) {
    let socket_path = &endpoint.socket_path;
    let session_id = &endpoint.tmux_session_id;
    let pane_id = &endpoint.pane_id;
    (
        format!(
            "\0tmux\0{}:{}\0{}\0{}:{}\0{}:{}\0{}",
            socket_path.len(),
            socket_path,
            endpoint.server_pid,
            session_id.len(),
            session_id,
            pane_id.len(),
            pane_id,
            endpoint.pane_pid,
        ),
        pane_id.clone(),
    )
}

pub(super) fn tmux_route_address_for_lookup(endpoint: &TmuxEndpoint) -> (String, String) {
    match (&endpoint.codex_session_id, &endpoint.codex_thread_id) {
        (Some(session), Some(thread)) => (session.clone(), thread.clone()),
        _ => tmux_route_address(endpoint),
    }
}

pub(super) fn current_route_address_key(
    session_id: &SessionId,
    native_thread_id: &NativeThreadId,
    tmux_endpoint: Option<&TmuxEndpoint>,
) -> String {
    let address = current_thread_route_address(session_id, native_thread_id, tmux_endpoint);
    format!("{}\0{}", address.0, address.1)
}

pub(super) fn tmux_route_address_key(endpoint: &TmuxEndpoint) -> String {
    let address = tmux_route_address(endpoint);
    format!("{}\0{}", address.0, address.1)
}

/// The durable retirement key for one claim: its route address.
///
/// The key is the address and not the binding id, so a retirement covers the
/// exact address the operator named and never a later generation at that
/// address. It lives here because both `global_state_impl` and
/// `global_state_impl_part2` read and write the retirement map.
pub(super) fn retired_route_claim_key(
    binding: &RuntimeBinding,
) -> Result<String, StateError> {
    let session_id = binding.session_id.as_ref().ok_or_else(|| {
        StateError::invalid("retired route claim", "requires a session id")
    })?;
    let native_thread_id = binding.native_thread_id.as_ref().ok_or_else(|| {
        StateError::invalid("retired route claim", "requires a native thread id")
    })?;
    Ok(current_route_address_key(
        session_id,
        native_thread_id,
        binding.tmux_endpoint.as_ref(),
    ))
}

pub(super) fn tmux_route_address_key_for_lookup(endpoint: &TmuxEndpoint) -> String {
    let address = tmux_route_address_for_lookup(endpoint);
    format!("{}\0{}", address.0, address.1)
}

pub(super) fn validate_tmux_route_endpoint(endpoint: &TmuxEndpoint) -> Result<(), StateError> {
    if endpoint.socket_path.is_empty()
        || !Path::new(&endpoint.socket_path).is_absolute()
        || endpoint.socket_path.chars().any(char::is_control)
        || endpoint.server_pid == 0
        || endpoint.pane_pid == 0
    {
        return Err(StateError::invalid(
            "tmux route endpoint",
            "requires an absolute socket path and non-zero server/pane process ids",
        ));
    }
    SessionId::new(endpoint.tmux_session_id.clone())
        .map_err(|error| StateError::invalid("tmux session id", error.to_string()))?;
    NativeThreadId::new(endpoint.pane_id.clone())
        .map_err(|error| StateError::invalid("tmux pane id", error.to_string()))?;
    let pane_suffix = endpoint.pane_id.strip_prefix('%').unwrap_or_default();
    if pane_suffix.is_empty() || !pane_suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(StateError::invalid("tmux pane id", "must use tmux %N form"));
    }
    for (field, value) in [
        ("Codex session id", endpoint.codex_session_id.as_deref()),
        ("Codex thread id", endpoint.codex_thread_id.as_deref()),
    ] {
        if let Some(value) = value {
            crate::identity::validate_id_for_protocol(value)
                .map_err(|error| StateError::invalid(field, error.to_string()))?;
        }
    }
    Ok(())
}

pub(super) fn validate_route_scope(scope: &RouteScope) -> Result<(), StateError> {
    validate_app_scope(&scope.app_scope_id)?;
    validate_project_scope(&scope.project_scope_id)
}

pub(super) fn validate_app_scope(scope: &AppServerId) -> Result<(), StateError> {
    AppServerId::new(scope.as_str().to_owned())
        .map_err(|error| StateError::invalid("app scope", error.to_string()))
        .map(|_| ())
}

pub(super) fn validate_agent_id(id: &AgentId) -> Result<(), StateError> {
    AgentId::new(id.as_str().to_owned())
        .map_err(|error| StateError::invalid("agent id", error.to_string()))
        .map(|_| ())
}

pub(super) fn validate_runtime_id(id: &RuntimeId) -> Result<(), StateError> {
    RuntimeId::new(id.as_str().to_owned())
        .map_err(|error| StateError::invalid("runtime id", error.to_string()))
        .map(|_| ())
}

pub(super) fn validate_binding_id(id: &BindingId) -> Result<(), StateError> {
    BindingId::new(id.as_str().to_owned())
        .map_err(|error| StateError::invalid("binding id", error.to_string()))
        .map(|_| ())
}

pub(super) fn validate_native_thread_id(id: &NativeThreadId) -> Result<(), StateError> {
    NativeThreadId::new(id.as_str().to_owned())
        .map_err(|error| StateError::invalid("native thread id", error.to_string()))
        .map(|_| ())
}

pub(super) fn validate_session_id(id: &SessionId) -> Result<(), StateError> {
    let value = id.as_str();
    if value.is_empty() {
        return Err(StateError::invalid("session id", "must not be empty"));
    }
    if value.len() > 256 {
        return Err(StateError::invalid("session id", "exceeds 256 bytes"));
    }
    if value.chars().any(char::is_control) {
        return Err(StateError::invalid(
            "session id",
            "must not contain control characters",
        ));
    }
    Ok(())
}

pub(super) fn validate_command_id(id: &CommandId) -> Result<(), StateError> {
    CommandId::new(id.as_str().to_owned())
        .map_err(|error| StateError::invalid("command id", error.to_string()))
        .map(|_| ())
}

pub(super) fn validate_operation_id(id: &OperationId) -> Result<(), StateError> {
    OperationId::new(id.as_str().to_owned())
        .map_err(|error| StateError::invalid("operation id", error.to_string()))
        .map(|_| ())
}

pub(super) fn validate_non_empty_text(field: &'static str, value: &str) -> Result<(), StateError> {
    if value.trim().is_empty() {
        return Err(StateError::invalid(field, "must not be empty"));
    }
    if value.chars().any(char::is_control) {
        return Err(StateError::invalid(
            field,
            "must not contain control characters",
        ));
    }
    Ok(())
}

pub(super) fn validate_migration_identifier(field: &'static str, value: &str) -> Result<(), StateError> {
    if value.trim().is_empty() {
        return Err(StateError::invalid(field, "must not be empty"));
    }
    if value
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(StateError::invalid(
            field,
            "must not contain whitespace or control characters",
        ));
    }
    Ok(())
}

pub(super) fn validate_migration_epoch(
    source_epoch: Option<u64>,
    target_epoch: u64,
) -> Result<(), StateError> {
    if source_epoch == Some(0) {
        return Err(StateError::invalid(
            "migration source epoch",
            "must be non-zero when present",
        ));
    }
    if target_epoch == 0 {
        return Err(StateError::invalid(
            "migration target epoch",
            "must be non-zero",
        ));
    }
    if source_epoch == Some(target_epoch) {
        return Err(StateError::invalid(
            "migration source epoch",
            "must differ from target epoch",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_migration_receipt_identity(
    migration_id: &str,
    source_project_id: &str,
    project_scope: &ProjectScopeId,
    source_epoch: Option<u64>,
    target_epoch: u64,
    source_snapshot_digest: &str,
    agent_id: &AgentId,
    app_scope_id: &AppServerId,
    runtime_id: &RuntimeId,
    binding_id: &BindingId,
    endpoint_generation: u64,
    operation_id: &OperationId,
    fencing_token: u64,
    committed_revision: u64,
) -> Result<(), StateError> {
    validate_migration_identifier("migration id", migration_id)?;
    validate_migration_identifier("source project id", source_project_id)?;
    validate_project_scope(project_scope)?;
    validate_migration_identifier("source snapshot digest", source_snapshot_digest)?;
    validate_migration_epoch(source_epoch, target_epoch)?;
    validate_agent_id(agent_id)?;
    validate_app_scope(app_scope_id)?;
    validate_runtime_id(runtime_id)?;
    validate_binding_id(binding_id)?;
    validate_operation_id(operation_id)?;
    if endpoint_generation == 0 {
        return Err(StateError::invalid(
            "migration endpoint generation",
            "must be non-zero",
        ));
    }
    if fencing_token == 0 {
        return Err(StateError::invalid(
            "migration fencing token",
            "must be non-zero",
        ));
    }
    if committed_revision == 0 {
        return Err(StateError::invalid(
            "migration receipt revision",
            "must be non-zero",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_migration_context(
    kind: &str,
    observed_migration_id: &str,
    observed_source_project_id: &str,
    observed_source_snapshot_digest: &str,
    observed_target_epoch: u64,
    migration_id: &str,
    source_project_id: &str,
    source_snapshot_digest: &str,
    target_epoch: u64,
) -> Result<(), StateError> {
    if observed_migration_id != migration_id {
        return Err(StateError::Invariant(format!(
            "{kind} receipt belongs to migration {}, expected {}",
            observed_migration_id, migration_id
        )));
    }
    if observed_source_project_id != source_project_id {
        return Err(StateError::Invariant(format!(
            "{kind} receipt belongs to source project {}, expected {}",
            observed_source_project_id, source_project_id
        )));
    }
    if observed_source_snapshot_digest != source_snapshot_digest {
        return Err(StateError::Invariant(format!(
            "{kind} receipt belongs to source digest {}, expected {}",
            observed_source_snapshot_digest, source_snapshot_digest
        )));
    }
    if observed_target_epoch != target_epoch {
        return Err(StateError::Invariant(format!(
            "{kind} receipt belongs to target epoch {}, expected {}",
            observed_target_epoch, target_epoch
        )));
    }
    Ok(())
}
