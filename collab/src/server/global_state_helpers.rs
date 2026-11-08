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
        Some(endpoint) => tmux_route_address(endpoint),
        _ => (
            session_id.as_str().to_owned(),
            native_thread_id.as_str().to_owned(),
        ),
    }
}

pub(super) fn tmux_route_address(endpoint: &TmuxEndpoint) -> (String, String) {
    let socket_path = &endpoint.socket_path;
    let pane_id = &endpoint.pane_id;
    (
        format!("\0tmux\0{}:{}\0{}", socket_path.len(), socket_path, pane_id,),
        pane_id.clone(),
    )
}

pub(super) fn tmux_route_address_for_lookup(endpoint: &TmuxEndpoint) -> (String, String) {
    tmux_route_address(endpoint)
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

/// The route address key of an already-installed route.
///
/// Pane routes do not require native runtime IDs.
pub(super) fn binding_route_address(binding: &RuntimeBinding) -> Option<(String, String)> {
    if let Some(endpoint) = binding.tmux_endpoint.as_ref() {
        return Some(tmux_route_address(endpoint));
    }
    if binding.native_thread_id.is_none() {
        let session = binding.session_id.as_ref()?;
        return Some((
            "\0session".to_owned(),
            serde_json::to_string(&(&binding.project_scope, &binding.app_scope_id, session))
                .expect("session route contains only typed strings"),
        ));
    }
    Some(current_thread_route_address(
        binding.session_id.as_ref()?,
        binding.native_thread_id.as_ref()?,
        None,
    ))
}

pub(crate) fn installed_route_address_key(binding: &RuntimeBinding) -> Option<String> {
    let address = binding_route_address(binding)?;
    Some(format!("{}\0{}", address.0, address.1))
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
