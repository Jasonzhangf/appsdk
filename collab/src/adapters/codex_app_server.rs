//! Codex App Server transport over the host-owned Unix WebSocket endpoint.
//!
//! This module speaks the native JSON-RPC surface exposed by Codex TUI and
//! Desktop. It does not start an App Server, invent a namespace, or treat
//! turn acceptance as execution. Explicit coordination and background wakeups
//! use `turn/start` or `turn/steer`. Every operation is
//! bounded and preserves the exact native error on failure.

use super::{AdapterCapabilities, AdapterError, EndpointKind, WakeMode};
use crate::identity::NativeThreadId;
use crate::proto::{AppServerCandidate, SelectedTransport, TransportKind};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const APPSERVER_SOCKET_ENV: &str = "COLLAB_APPSERVER_SOCKET";
pub const APPSERVER_NAMESPACE_ENV: &str = "COLLAB_APPSERVER_NAMESPACE";
pub const APPSERVER_TIMEOUT_MS_ENV: &str = "COLLAB_APPSERVER_TIMEOUT_MS";
pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_OUTGOING_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_INCOMING_FRAME_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppServerCapabilities {
    pub discover_sessions: bool,
    pub session_status: bool,
    pub send_message: bool,
    pub read_thread: bool,
    pub wait_reply: bool,
    pub ack: bool,
}

impl AppServerCapabilities {
    pub fn native() -> Self {
        Self {
            discover_sessions: true,
            session_status: true,
            send_message: true,
            read_thread: true,
            wait_reply: true,
            ack: false,
        }
    }

    pub fn adapter(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            endpoint: EndpointKind::Tui,
            submit: self.send_message,
            interrupt: false,
            wake: WakeMode::Native,
        }
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut values = Vec::new();
        if self.discover_sessions {
            values.push("discover_sessions");
        }
        if self.session_status {
            values.push("session_status");
        }
        if self.send_message {
            values.push("send_message_to_thread");
        }
        if self.read_thread {
            values.push("read_thread");
        }
        if self.wait_reply {
            values.push("wait_reply");
        }
        if self.ack {
            values.push("ack");
        }
        values
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveAppServer {
    socket_path: PathBuf,
    namespace: String,
    thread_id: NativeThreadId,
    timeout: Duration,
    capabilities: AppServerCapabilities,
}

impl LiveAppServer {
    pub fn detect() -> Result<Option<Self>, AdapterError> {
        let Some(thread_id) = std::env::var("CODEX_THREAD_ID")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        let thread_id =
            NativeThreadId::new(thread_id).map_err(|error| AdapterError::InvalidBinding {
                detail: format!("CODEX_THREAD_ID is invalid: {error}"),
            })?;
        let Some(socket_path) = socket_candidate() else {
            return Ok(None);
        };
        if !socket_path.is_absolute() || !socket_path.exists() {
            return Ok(None);
        }
        let namespace = std::env::var(APPSERVER_NAMESPACE_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "codex_tui".into());
        if !matches!(namespace.as_str(), "codex_tui" | "codex_app") {
            return Err(AdapterError::Unknown {
                operation: "detect",
                detail: format!("unsupported App Server namespace {namespace}"),
            });
        }
        let timeout = match std::env::var(APPSERVER_TIMEOUT_MS_ENV) {
            Ok(value) => Duration::from_millis(value.trim().parse::<u64>().map_err(|error| {
                AdapterError::Unknown {
                    operation: "detect",
                    detail: format!(
                        "{APPSERVER_TIMEOUT_MS_ENV} must be a positive integer: {error}"
                    ),
                }
            })?),
            Err(std::env::VarError::NotPresent) => Duration::from_millis(DEFAULT_TIMEOUT_MS),
            Err(error) => {
                return Err(AdapterError::Unknown {
                    operation: "detect",
                    detail: format!("cannot read {APPSERVER_TIMEOUT_MS_ENV}: {error}"),
                })
            }
        };
        if timeout.is_zero() {
            return Err(AdapterError::Unknown {
                operation: "detect",
                detail: format!("{APPSERVER_TIMEOUT_MS_ENV} must be greater than zero"),
            });
        }
        let mut client = Client::connect(&socket_path, timeout)?;
        client.initialize()?;
        let response = client.call("thread/read", json!({"threadId": thread_id.as_str()}))?;
        let observed = response
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| AdapterError::Unknown {
                operation: "thread/read",
                detail: "response is missing thread.id".into(),
            })?;
        if observed != thread_id.as_str() {
            return Err(AdapterError::Unknown {
                operation: "thread/read",
                detail: format!(
                    "thread identity mismatch: expected {}, observed {}",
                    thread_id, observed
                ),
            });
        }
        Ok(Some(Self {
            socket_path,
            namespace,
            thread_id,
            timeout,
            capabilities: AppServerCapabilities::native(),
        }))
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn thread_id(&self) -> &NativeThreadId {
        &self.thread_id
    }

    pub fn capabilities(&self) -> &AppServerCapabilities {
        &self.capabilities
    }

    pub fn send(
        &self,
        thread_id: &NativeThreadId,
        body: &str,
        client_user_message_id: &str,
    ) -> Result<Value, AdapterError> {
        if !self.capabilities.send_message {
            return Err(AdapterError::CapabilityUnavailable {
                endpoint: EndpointKind::Tui,
                operation: "send_message_to_thread",
            });
        }
        let mut client = Client::connect(&self.socket_path, self.timeout)?;
        client.initialize()?;
        client.call(
            "turn/start",
            json!({
                "threadId": thread_id.as_str(),
                "input": [{"type": "text", "text": body}],
                "clientUserMessageId": client_user_message_id,
            }),
        )
    }

    pub fn status(&self, thread_id: &NativeThreadId) -> Result<Value, AdapterError> {
        let mut client = Client::connect(&self.socket_path, self.timeout)?;
        client.initialize()?;
        client.call("thread/read", json!({"threadId": thread_id.as_str()}))
    }

    pub fn read_items(
        &self,
        thread_id: &NativeThreadId,
        cursor: Option<&str>,
    ) -> Result<Value, AdapterError> {
        let mut client = Client::connect(&self.socket_path, self.timeout)?;
        client.initialize()?;
        client.call(
            "thread/items/list",
            json!({
                "threadId": thread_id.as_str(),
                "limit": 100,
                "cursor": cursor,
                "sortDirection": "desc",
            }),
        )
    }

    pub fn as_json(&self) -> Value {
        json!({
            "selected": "appserver",
            "priority": 100,
            "endpoint": format!("unix://{}", self.socket_path.display()),
            "namespace": self.namespace,
            "thread_id": self.thread_id.as_str(),
            "capabilities": self.capabilities.names(),
            "accepted_semantics": "turn/start accepted the immediate notification; execution and reply are observed separately"
        })
    }
}

/// Collect an App Server endpoint/thread candidate from the current process
/// environment without probing it. The daemon owns candidate self-check and
/// transport selection; a client-side probe must not be able to suppress a
/// candidate that the server could otherwise validate.
pub fn candidate_from_env() -> Result<Option<AppServerCandidate>, AdapterError> {
    let Some(thread_id) = std::env::var("CODEX_THREAD_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let session_id = std::env::var("CODEX_SESSION_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AdapterError::InvalidBinding {
            detail: "CODEX_SESSION_ID is required when an App Server thread is selected".into(),
        })?;
    NativeThreadId::new(thread_id.clone()).map_err(|error| AdapterError::InvalidBinding {
        detail: format!("CODEX_THREAD_ID is invalid: {error}"),
    })?;
    let Some(socket_path) = socket_candidate() else {
        return Ok(None);
    };
    let namespace = std::env::var(APPSERVER_NAMESPACE_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "codex_tui".into());
    let cwd = std::env::current_dir()
        .and_then(|path| path.canonicalize())
        .map_err(|error| AdapterError::InvalidBinding {
            detail: format!("cannot resolve candidate cwd: {error}"),
        })?;
    let cwd = cwd
        .to_str()
        .ok_or_else(|| AdapterError::InvalidBinding {
            detail: "candidate cwd must be valid UTF-8".into(),
        })?
        .to_owned();
    Ok(Some(AppServerCandidate {
        endpoint: format!("unix://{}", socket_path.display()),
        namespace,
        session_id,
        thread_id,
        cwd,
    }))
}

/// Independently verify one worker-proposed App Server endpoint. The worker
/// only supplies a candidate; this function is the daemon-owned admission
/// check that establishes the native thread identity and required methods.
pub fn verify_candidate(candidate: &AppServerCandidate) -> Result<SelectedTransport, AdapterError> {
    let socket_path = endpoint_path(&candidate.endpoint)?;
    if !socket_path.is_absolute() {
        return Err(AdapterError::Unknown {
            operation: "verify_candidate",
            detail: "App Server endpoint must be an absolute unix socket path".into(),
        });
    }
    if !matches!(candidate.namespace.as_str(), "codex_tui" | "codex_app") {
        return Err(AdapterError::Unknown {
            operation: "verify_candidate",
            detail: format!("unsupported App Server namespace {}", candidate.namespace),
        });
    }
    if candidate.session_id.trim().is_empty() {
        return Err(AdapterError::InvalidBinding {
            detail: "candidate session_id is required".into(),
        });
    }
    let candidate_cwd =
        std::fs::canonicalize(&candidate.cwd).map_err(|error| AdapterError::InvalidBinding {
            detail: format!("candidate cwd is invalid: {error}"),
        })?;
    let candidate_cwd = candidate_cwd
        .to_str()
        .ok_or_else(|| AdapterError::InvalidBinding {
            detail: "candidate cwd must be valid UTF-8".into(),
        })?;
    let candidate_cwd = candidate_cwd.to_owned();
    let thread_id = NativeThreadId::new(candidate.thread_id.clone()).map_err(|error| {
        AdapterError::InvalidBinding {
            detail: format!("candidate thread_id is invalid: {error}"),
        }
    })?;
    let timeout = Duration::from_millis(DEFAULT_TIMEOUT_MS);
    let mut client = Client::connect(&socket_path, timeout)?;
    client.initialize()?;
    // Thread identity is established from `thread/read`, which serves
    // persisted threads.  `thread/loaded/list` is not usable as an admission
    // gate: on the host control endpoint it is always empty because threads
    // are owned by the client process, so requiring membership there would
    // reject every genuine thread.  A cold thread is loaded by the native
    // load-and-start call at delivery time instead.
    let response = match client.call("thread/read", json!({"threadId": thread_id.as_str()})) {
        Ok(response) => response,
        Err(AdapterError::Unknown { operation, detail })
            if is_thread_not_loaded_error(&operation, &detail, thread_id.as_str()) =>
        {
            return Err(AdapterError::RouteUnavailable {
                detail: format!(
                    "thread {} is persisted but not loaded by the App Server",
                    thread_id.as_str()
                ),
            })
        }
        Err(AdapterError::Unknown { operation, detail })
            if is_thread_not_found_error(&operation, &detail, thread_id.as_str()) =>
        {
            return Err(AdapterError::RouteUnavailable { detail })
        }
        Err(error) => return Err(error),
    };
    let observed_thread_id = response
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::Unknown {
            operation: "thread/read",
            detail: "response is missing thread.id".into(),
        })?;
    if observed_thread_id != thread_id.as_str() {
        return Err(AdapterError::Unknown {
            operation: "thread/read",
            detail: format!(
                "thread identity mismatch: expected {}, observed {}",
                thread_id, observed_thread_id
            ),
        });
    }
    let observed_session_id = response
        .pointer("/thread/sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::Unknown {
            operation: "thread/read",
            detail: "response is missing thread.sessionId".into(),
        })?;
    if observed_session_id != candidate.session_id {
        return Err(AdapterError::Unknown {
            operation: "thread/read",
            detail: format!(
                "thread session mismatch: expected {}, observed {}",
                candidate.session_id, observed_session_id
            ),
        });
    }
    let observed_cwd = response
        .pointer("/thread/cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::Unknown {
            operation: "thread/read",
            detail: "response is missing thread.cwd".into(),
        })?;
    let observed_cwd =
        std::fs::canonicalize(observed_cwd).map_err(|error| AdapterError::Unknown {
            operation: "thread/read",
            detail: format!("cannot canonicalize thread.cwd {observed_cwd}: {error}"),
        })?;
    let observed_cwd = observed_cwd.to_str().ok_or_else(|| AdapterError::Unknown {
        operation: "thread/read",
        detail: "thread.cwd must be valid UTF-8".into(),
    })?;
    if observed_cwd != candidate_cwd {
        return Err(AdapterError::Unknown {
            operation: "thread/read",
            detail: format!(
                "thread cwd mismatch: expected {candidate_cwd}, observed {observed_cwd}"
            ),
        });
    }
    // `notLoaded` is a normal state for a persisted thread on this endpoint
    // and is not a rejection reason: delivery uses `turn/start`, which loads
    // the thread.  Identity above is what registration must actually prove.
    // Item history is a diagnostic capability, not a registration or wake
    // requirement. Some App Server builds expose thread/read and notification
    // methods but return method-not-found for items/list; that must not block peer
    // registration. Snapshot calls still fail explicitly if the method is
    // unavailable.
    let _items_available = method_exists(
        &mut client,
        "thread/items/list",
        json!({
            "threadId": thread_id.as_str(),
            "limit": 1,
            "sortDirection": "desc",
        }),
    )?;
    let immediate_notify = method_exists(
        &mut client,
        "turn/start",
        json!({"threadId": "", "input": []}),
    )?;
    if !immediate_notify {
        return Err(AdapterError::CapabilityUnavailable {
            endpoint: EndpointKind::Tui,
            operation: "turn/start",
        });
    }
    let steer = method_exists(
        &mut client,
        "turn/steer",
        json!({
            "threadId": "",
            "expectedTurnId": "",
            "input": [],
        }),
    )?;
    if !steer {
        return Err(AdapterError::CapabilityUnavailable {
            endpoint: EndpointKind::Tui,
            operation: "turn/steer",
        });
    }
    let turns_list = method_exists(
        &mut client,
        "thread/turns/list",
        json!({
            "threadId": thread_id.as_str(),
            "limit": 1,
            "sortDirection": "desc",
        }),
    )?;
    if !turns_list {
        return Err(AdapterError::CapabilityUnavailable {
            endpoint: EndpointKind::Tui,
            operation: "thread/turns/list",
        });
    }
    Ok(SelectedTransport {
        kind: TransportKind::AppServer,
        endpoint: Some(format!("unix://{}", socket_path.display())),
        namespace: Some(candidate.namespace.clone()),
        session_id: Some(candidate.session_id.clone()),
        thread_id: Some(thread_id.to_string()),
        tmux_endpoint: None,
        capabilities: vec![
            "session_status".into(),
            "read_thread".into(),
            "send_message_to_thread".into(),
            "wait_reply".into(),
        ],
        self_check:
            "initialize, thread/read identity, turn/start, turn/steer, and thread/turns/list method probes passed"
                .into(),
    })
}

/// Start or steer one immediate notification through a server-selected App
/// Server transport. A successful result means the native App Server accepted
/// the turn; execution and reply are observed separately.
pub fn immediate_notify(
    transport: &SelectedTransport,
    source_thread_id: Option<&str>,
    body: &str,
    client_user_message_id: &str,
) -> Result<Value, AdapterError> {
    if transport.kind != TransportKind::AppServer {
        return Err(AdapterError::CapabilityUnavailable {
            endpoint: EndpointKind::Tui,
            operation: "turn/start",
        });
    }
    let endpoint = transport
        .endpoint
        .as_deref()
        .ok_or_else(|| AdapterError::InvalidBinding {
            detail: "selected App Server transport has no endpoint".into(),
        })?;
    let thread_id = transport
        .thread_id
        .as_deref()
        .ok_or_else(|| AdapterError::InvalidBinding {
            detail: "selected App Server transport has no thread_id".into(),
        })?;
    let socket_path = endpoint_path(endpoint)?;
    let thread_id = NativeThreadId::new(thread_id.to_owned()).map_err(|error| {
        AdapterError::InvalidBinding {
            detail: format!("selected App Server thread_id is invalid: {error}"),
        }
    })?;
    let mut client = Client::connect(&socket_path, Duration::from_millis(DEFAULT_TIMEOUT_MS))?;
    client.initialize()?;
    // Thread residency is per-connection: a thread created or resumed on one
    // WebSocket is not loaded for a different connection, and `turn/start`
    // answers "thread not found" for a thread this connection does not hold.
    // Resume on this connection first so the immediate notification can be
    // delivered, which is the native load step for a persisted thread.
    let resumed_here = match client.call("thread/resume", json!({"threadId": thread_id.as_str()})) {
        Ok(_) => true,
        Err(AdapterError::Unknown { operation, detail })
            if is_thread_not_found_error(&operation, &detail, thread_id.as_str()) =>
        {
            return Err(AdapterError::RouteUnavailable { detail })
        }
        Err(AdapterError::Unknown { operation, detail })
            if is_thread_not_loaded_error(&operation, &detail, thread_id.as_str()) =>
        {
            false
        }
        Err(AdapterError::Unknown { operation, detail })
            if is_thread_rollout_missing_error(&operation, &detail, thread_id.as_str()) =>
        {
            false
        }
        // `thread/resume` is not universal across App Server builds; a method
        // that does not exist leaves the existing status-based path intact.
        Err(AdapterError::CapabilityUnavailable { .. }) => false,
        Err(error) => return Err(error),
    };
    let status = match thread_metadata(&mut client, thread_id.as_str()) {
        Ok(thread) => thread_status_from_metadata(&thread)?,
        // A cold thread is loaded by the immediate notification itself:
        // `turn/start` is the native load-and-start call, so a persisted
        // thread reports a not-loaded status that resolves to Start rather
        // than an invented queue or a refusal.  A genuinely missing thread
        // still fails closed below.
        Err(AdapterError::Unknown { operation, detail })
            if is_thread_not_loaded_error(&operation, &detail, thread_id.as_str()) =>
        {
            if resumed_here {
                return Err(AdapterError::Unknown {
                    operation: "thread/read",
                    detail: format!(
                        "thread {thread_id} was resumed on this connection but still reports not loaded"
                    ),
                });
            }
            "notLoaded".to_string()
        }
        Err(AdapterError::Unknown { operation, detail })
            if is_thread_not_found_error(&operation, &detail, thread_id.as_str()) =>
        {
            return Err(AdapterError::RouteUnavailable { detail });
        }
        Err(error) => return Err(error),
    };
    let active_turn_id = match status.as_str() {
        "active" => active_turn_id(&mut client, thread_id.as_str())?,
        _ => None,
    };
    let action = notification_action(&status, active_turn_id)?;
    match action {
        NotificationAction::Start => {
            let receipt = client.call(
                "turn/start",
                json!({
                    "threadId": thread_id.as_str(),
                    "input": [],
                    "toolOutput": {
                        "name": "send_message_to_thread",
                        "namespace": "codex_tui",
                        "output": delegated_prompt(
                            source_thread_id,
                            client_user_message_id,
                            body,
                        ),
                    },
                    "clientUserMessageId": client_user_message_id,
                }),
            )?;
            validate_immediate_receipt(&receipt)?;
            Ok(receipt)
        }
        NotificationAction::Steer(expected_turn_id) => {
            let receipt = client.call(
                "turn/steer",
                json!({
                    "threadId": thread_id.as_str(),
                    "expectedTurnId": expected_turn_id,
                    "input": [{"type": "text", "text": body, "text_elements": []}],
                    "clientUserMessageId": client_user_message_id,
                }),
            )?;
            validate_steer_receipt(&receipt, &expected_turn_id)?;
            Ok(receipt)
        }
    }
}

fn is_thread_not_loaded_error(operation: &str, detail: &str, thread_id: &str) -> bool {
    operation == "rpc"
        && (detail == "thread not loaded" || detail == format!("thread not loaded: {thread_id}"))
}

fn is_thread_rollout_missing_error(operation: &str, detail: &str, thread_id: &str) -> bool {
    operation == "rpc" && detail == format!("no rollout found for thread id {thread_id}")
}

/// The App Server holds one writer per thread.  A second client is refused
/// with this message, which is terminal for the current endpoint: the thread
/// belongs to another live process, and no retry can take it over.
pub fn is_thread_writer_conflict(detail: &str) -> bool {
    detail.contains("already has an active writer")
}

fn is_thread_not_found_error(operation: &str, detail: &str, thread_id: &str) -> bool {
    operation == "rpc"
        && (detail == "thread not found"
            || detail == format!("thread not found: {thread_id}")
            || detail == "rpc unknown: thread not found"
            || detail == format!("rpc unknown: thread not found: {thread_id}"))
}

fn transport_client(transport: &SelectedTransport) -> Result<Client, AdapterError> {
    if transport.kind != TransportKind::AppServer {
        return Err(AdapterError::CapabilityUnavailable {
            endpoint: EndpointKind::Tui,
            operation: "App Server transport",
        });
    }
    let endpoint = transport
        .endpoint
        .as_deref()
        .ok_or_else(|| AdapterError::InvalidBinding {
            detail: "selected App Server transport has no endpoint".into(),
        })?;
    let socket_path = endpoint_path(endpoint)?;
    let mut client = Client::connect(&socket_path, Duration::from_millis(DEFAULT_TIMEOUT_MS))?;
    client.initialize()?;
    Ok(client)
}

pub fn start_thread(
    transport: &SelectedTransport,
    cwd: &Path,
    model: Option<&str>,
) -> Result<NativeThreadId, AdapterError> {
    let mut client = transport_client(transport)?;
    let mut params = json!({
        "cwd": cwd,
        "approvalPolicy": "never",
        "sandbox": "danger-full-access",
        "sessionStartSource": "startup"
    });
    if let Some(model) = model {
        params["model"] = json!(model);
    }
    let response = client.call("thread/start", params)?;
    let thread_id = response
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::Unknown {
            operation: "thread/start",
            detail: "response is missing thread.id".into(),
        })?;
    NativeThreadId::new(thread_id.to_owned()).map_err(|error| AdapterError::InvalidBinding {
        detail: format!("thread/start returned an invalid thread id: {error}"),
    })
}

pub fn archive_thread(
    transport: &SelectedTransport,
    thread_id: &str,
) -> Result<Value, AdapterError> {
    let mut client = transport_client(transport)?;
    client.call("thread/archive", json!({"threadId": thread_id}))
}

pub fn read_thread_items(
    transport: &SelectedTransport,
    thread_id: &str,
    limit: usize,
) -> Result<Value, AdapterError> {
    read_thread_items_page(transport, thread_id, limit, None)
}

pub fn read_thread_items_page(
    transport: &SelectedTransport,
    thread_id: &str,
    limit: usize,
    cursor: Option<&str>,
) -> Result<Value, AdapterError> {
    let mut client = transport_client(transport)?;
    client.call(
        "thread/items/list",
        json!({
            "threadId": thread_id,
            "limit": limit,
            "cursor": cursor,
            "sortDirection": "desc",
        }),
    )
}

pub fn read_thread_turns_with_items_page(
    transport: &SelectedTransport,
    thread_id: &str,
    cursor: Option<&str>,
) -> Result<Value, AdapterError> {
    let mut client = transport_client(transport)?;
    client.call(
        "thread/turns/list",
        json!({
            "threadId": thread_id,
            "limit": 100,
            "cursor": cursor,
            "sortDirection": "desc",
            "itemsView": "full",
        }),
    )
}

pub fn read_thread_status(
    transport: &SelectedTransport,
    thread_id: &str,
) -> Result<Value, AdapterError> {
    let mut client = transport_client(transport)?;
    client.call("thread/read", json!({"threadId": thread_id}))
}

pub fn read_turn_statuses(
    transport: &SelectedTransport,
    thread_id: &str,
) -> Result<Value, AdapterError> {
    read_turn_statuses_page(transport, thread_id, None)
}

pub fn read_turn_statuses_page(
    transport: &SelectedTransport,
    thread_id: &str,
    cursor: Option<&str>,
) -> Result<Value, AdapterError> {
    let mut client = transport_client(transport)?;
    client.call(
        "thread/turns/list",
        json!({
            "threadId": thread_id,
            "limit": 100,
            "cursor": cursor,
            "sortDirection": "desc",
        }),
    )
}

fn endpoint_path(endpoint: &str) -> Result<PathBuf, AdapterError> {
    let path = endpoint
        .strip_prefix("unix://")
        .ok_or_else(|| AdapterError::Unknown {
            operation: "verify_candidate",
            detail: "only unix:// App Server endpoints are supported".into(),
        })?;
    if path.is_empty() {
        return Err(AdapterError::Unknown {
            operation: "verify_candidate",
            detail: "App Server endpoint has no socket path".into(),
        });
    }
    Ok(PathBuf::from(path))
}

fn thread_metadata(client: &mut Client, thread_id: &str) -> Result<Value, AdapterError> {
    let receipt = client.call("thread/read", json!({"threadId": thread_id}))?;
    let thread = receipt.get("thread").ok_or_else(|| AdapterError::Unknown {
        operation: "thread/read",
        detail: "response is missing thread".into(),
    })?;
    let observed = thread
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AdapterError::Unknown {
            operation: "thread/read",
            detail: "response is missing thread.id".into(),
        })?;
    if observed != thread_id {
        return Err(AdapterError::Unknown {
            operation: "thread/read",
            detail: format!("thread identity mismatch: expected {thread_id}, observed {observed}"),
        });
    }
    Ok(thread.clone())
}

fn thread_status_from_metadata(thread: &Value) -> Result<String, AdapterError> {
    thread
        .pointer("/status/type")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| AdapterError::Unknown {
            operation: "thread/read",
            detail: "response is missing thread.status.type".into(),
        })
}

pub(crate) fn escape_delegated_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn delegated_prompt(
    source_thread_id: Option<&str>,
    client_user_message_id: &str,
    body: &str,
) -> String {
    let source = source_thread_id
        .map(|source_thread_id| {
            format!(
                "  <source_thread_id>{}</source_thread_id>\n",
                escape_delegated_text(source_thread_id)
            )
        })
        .unwrap_or_default();
    format!(
        "<codex_delegation>\n{}  <client_message_id>{}</client_message_id>\n  <input>{}</input>\n</codex_delegation>",
        source,
        escape_delegated_text(client_user_message_id),
        escape_delegated_text(body)
    )
}

fn active_turn_id(client: &mut Client, thread_id: &str) -> Result<Option<String>, AdapterError> {
    let page = client.call(
        "thread/turns/list",
        json!({
            "threadId": thread_id,
            "limit": 100,
            "sortDirection": "desc",
        }),
    )?;
    active_turn_id_from_page(&page)
}

fn active_turn_id_from_page(page: &Value) -> Result<Option<String>, AdapterError> {
    let turns =
        page.get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| AdapterError::Unknown {
                operation: "thread/turns/list",
                detail: "response is missing data array".into(),
            })?;
    let mut active = Vec::new();
    for (index, turn) in turns.iter().enumerate() {
        match turn.get("status").and_then(Value::as_str) {
            Some("inProgress") => {}
            Some("completed" | "interrupted" | "failed") => continue,
            Some(status) => {
                return Err(AdapterError::Unknown {
                    operation: "turn/steer",
                    detail: format!(
                        "AUTO_NOTIFY_UNSUPPORTED_TURN_STATUS: turn at data[{index}] has status {status}"
                    ),
                })
            }
            None => {
                return Err(AdapterError::Unknown {
                    operation: "turn/steer",
                    detail: format!("turn at data[{index}] is missing status"),
                })
            }
        }
        let turn_id = turn
            .get("id")
            .ok_or_else(|| AdapterError::Unknown {
                operation: "turn/steer",
                detail: format!("inProgress turn at data[{index}] is missing id"),
            })?
            .as_str()
            .ok_or_else(|| AdapterError::Unknown {
                operation: "turn/steer",
                detail: format!("inProgress turn at data[{index}] id must be a JSON string"),
            })?;
        if turn_id.trim().is_empty() {
            return Err(AdapterError::Unknown {
                operation: "turn/steer",
                detail: format!("inProgress turn at data[{index}] id must be non-empty after trim"),
            });
        }
        if turn_id.chars().any(char::is_whitespace) {
            return Err(AdapterError::Unknown {
                operation: "turn/steer",
                detail: format!("inProgress turn at data[{index}] id must not contain whitespace"),
            });
        }
        active.push(turn_id);
    }
    match active.len() {
        0 => Ok(None),
        1 => {
            let turn_id = active.remove(0);
            Ok(Some(turn_id.to_owned()))
        }
        _ => Err(AdapterError::Unknown {
            operation: "turn/steer",
            detail: format!(
                "STEER_ACTIVE_TURN_AMBIGUOUS: recipient has {} inProgress turns",
                active.len()
            ),
        }),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum NotificationAction {
    Start,
    Steer(String),
}

fn notification_action(
    thread_status: &str,
    active_turn_id: Option<String>,
) -> Result<NotificationAction, AdapterError> {
    match thread_status {
        "active" => Ok(match active_turn_id {
            Some(turn_id) => NotificationAction::Steer(turn_id),
            None => NotificationAction::Start,
        }),
        "idle" => Ok(NotificationAction::Start),
        // `thread/read` reports notLoaded for a persisted thread that is cold
        // on this endpoint.  `turn/start` is the native load-and-start call,
        // so an immediate notification loads it instead of refusing.
        "notLoaded" => Ok(NotificationAction::Start),
        status => Err(AdapterError::Unknown {
            operation: "thread/read",
            detail: format!(
                "AUTO_NOTIFY_UNSUPPORTED_THREAD_STATUS: cannot deliver to thread status {status}"
            ),
        }),
    }
}

fn validate_steer_receipt(receipt: &Value, expected_turn_id: &str) -> Result<(), AdapterError> {
    let turn_id = receipt
        .get("turnId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AdapterError::Unknown {
            operation: "turn/steer",
            detail: "response is missing turnId".into(),
        })?;
    if turn_id != expected_turn_id {
        return Err(AdapterError::Unknown {
            operation: "turn/steer",
            detail: format!(
                "turn identity mismatch: expected {expected_turn_id}, observed {turn_id}"
            ),
        });
    }
    Ok(())
}

fn validate_immediate_receipt(receipt: &Value) -> Result<(), AdapterError> {
    let turn_id = receipt
        .pointer("/turn/id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AdapterError::Unknown {
            operation: "turn/start",
            detail: "response is missing turn.id".into(),
        })?;
    let status = receipt
        .pointer("/turn/status")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::Unknown {
            operation: "turn/start",
            detail: "response is missing turn.status".into(),
        })?;
    if !matches!(
        status,
        "inProgress" | "completed" | "interrupted" | "failed"
    ) {
        return Err(AdapterError::Unknown {
            operation: "turn/start",
            detail: format!("response returned unsupported turn status {status}"),
        });
    }
    if turn_id.chars().any(char::is_whitespace) {
        return Err(AdapterError::Unknown {
            operation: "turn/start",
            detail: "response returned an invalid turn.id".into(),
        });
    }
    Ok(())
}

fn method_exists(client: &mut Client, method: &str, params: Value) -> Result<bool, AdapterError> {
    match client.call_raw(method, params)? {
        Ok(_) => Ok(true),
        Err(error) => Ok(error.code != -32601),
    }
}

fn socket_candidate() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(APPSERVER_SOCKET_ENV).filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(value));
    }
    if let Some(value) =
        std::env::var_os("CODEX_APP_SERVER_SOCKET").filter(|value| !value.is_empty())
    {
        return Some(PathBuf::from(value));
    }
    let codex_home = std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".codex"))
        })?;
    Some(codex_home.join("app-server-control/app-server-control.sock"))
}

struct Client {
    stream: UnixStream,
    timeout: Duration,
    next_id: u64,
}

#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
}

impl Client {
    fn connect(path: &Path, timeout: Duration) -> Result<Self, AdapterError> {
        let stream = UnixStream::connect(path).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
            ) {
                AdapterError::RouteUnavailable {
                    detail: format!("{}: {error}", path.display()),
                }
            } else {
                AdapterError::Unknown {
                    operation: "connect",
                    detail: format!("{}: {error}", path.display()),
                }
            }
        })?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|error| AdapterError::Unknown {
                operation: "connect",
                detail: format!("set read timeout: {error}"),
            })?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|error| AdapterError::Unknown {
                operation: "connect",
                detail: format!("set write timeout: {error}"),
            })?;
        let mut client = Self {
            stream,
            timeout,
            next_id: 1,
        };
        client.handshake()?;
        Ok(client)
    }

    fn handshake(&mut self) -> Result<(), AdapterError> {
        let key = websocket_key();
        let request = format!(
            "GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        self.stream
            .write_all(request.as_bytes())
            .map_err(|error| transport("websocket handshake write", error))?;
        let mut reader = BufReader::new(
            self.stream
                .try_clone()
                .map_err(|error| transport("websocket handshake clone", error))?,
        );
        let mut status = String::new();
        reader
            .read_line(&mut status)
            .map_err(|error| transport("websocket handshake status", error))?;
        if !status.starts_with("HTTP/1.1 101") && !status.starts_with("HTTP/1.0 101") {
            return Err(AdapterError::Unknown {
                operation: "websocket handshake",
                detail: format!("upgrade rejected: {}", status.trim()),
            });
        }
        loop {
            let mut header = String::new();
            reader
                .read_line(&mut header)
                .map_err(|error| transport("websocket handshake header", error))?;
            if header == "\r\n" || header == "\n" || header.is_empty() {
                break;
            }
        }
        Ok(())
    }

    fn initialize(&mut self) -> Result<Value, AdapterError> {
        let result = self.call(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "collab",
                    "title": "Collab",
                    "version": env!("COLLAB_VERSION")
                },
                "capabilities": {"experimentalApi": true}
            }),
        )?;
        self.notify("initialized", json!({}))?;
        Ok(result)
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, AdapterError> {
        match self.call_raw(method, params)? {
            Ok(value) => Ok(value),
            // One thread admits one writer.  The refusal means a different
            // live App Server process owns the rollout, so this endpoint can
            // never take the thread over; report it as its own terminal class
            // instead of an opaque rpc failure.
            Err(error) if is_thread_writer_conflict(&error.message) => {
                Err(AdapterError::ThreadWriterConflict {
                    detail: error.message,
                })
            }
            Err(error) => Err(AdapterError::Unknown {
                operation: "rpc",
                detail: error.message,
            }),
        }
    }

    fn call_raw(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Result<Value, RpcError>, AdapterError> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.write_json(&json!({"method": method, "id": id, "params": params}))?;
        loop {
            let value = self.read_json()?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Ok(Err(RpcError {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("native JSON-RPC error")
                        .to_owned(),
                }));
            }
            return Ok(Ok(value.get("result").cloned().ok_or_else(|| {
                AdapterError::Unknown {
                    operation: "rpc",
                    detail: "native response is missing result".into(),
                }
            })?));
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), AdapterError> {
        self.write_json(&json!({"method": method, "params": params}))
    }

    fn write_json(&mut self, value: &Value) -> Result<(), AdapterError> {
        let payload = serde_json::to_vec(value).map_err(|error| AdapterError::Unknown {
            operation: "encode",
            detail: error.to_string(),
        })?;
        if payload.len() > MAX_OUTGOING_FRAME_BYTES {
            return Err(AdapterError::Unknown {
                operation: "encode",
                detail: "native frame exceeds maximum size".into(),
            });
        }
        self.stream
            .write_all(&encode_frame(0x1, &payload))
            .map_err(|error| transport("websocket write", error))?;
        self.stream
            .flush()
            .map_err(|error| transport("websocket flush", error))
    }

    fn read_json(&mut self) -> Result<Value, AdapterError> {
        loop {
            let payload = self.read_frame()?;
            let value: Value =
                serde_json::from_slice(&payload).map_err(|error| AdapterError::Unknown {
                    operation: "decode",
                    detail: error.to_string(),
                })?;
            if value.get("id").is_some() {
                return Ok(value);
            }
        }
    }

    fn read_frame(&mut self) -> Result<Vec<u8>, AdapterError> {
        let mut header = [0_u8; 2];
        self.stream
            .read_exact(&mut header)
            .map_err(|error| transport("websocket header", error))?;
        let opcode = header[0] & 0x0f;
        let masked = header[1] & 0x80 != 0;
        let mut length = (header[1] & 0x7f) as u64;
        if length == 126 {
            let mut bytes = [0_u8; 2];
            self.stream
                .read_exact(&mut bytes)
                .map_err(|error| transport("websocket length", error))?;
            length = u16::from_be_bytes(bytes) as u64;
        } else if length == 127 {
            let mut bytes = [0_u8; 8];
            self.stream
                .read_exact(&mut bytes)
                .map_err(|error| transport("websocket length", error))?;
            length = u64::from_be_bytes(bytes);
        }
        if length > MAX_INCOMING_FRAME_BYTES as u64 {
            return Err(AdapterError::Unknown {
                operation: "websocket frame",
                detail: "native frame exceeds maximum size".into(),
            });
        }
        let length = usize::try_from(length).map_err(|_| AdapterError::Unknown {
            operation: "websocket frame",
            detail: "native frame exceeds maximum size".into(),
        })?;
        let mut mask = [0_u8; 4];
        if masked {
            self.stream
                .read_exact(&mut mask)
                .map_err(|error| transport("websocket mask", error))?;
        }
        let mut payload = vec![0_u8; length];
        self.stream
            .read_exact(&mut payload)
            .map_err(|error| transport("websocket payload", error))?;
        if masked {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
        match opcode {
            0x1 => Ok(payload),
            0x8 => Err(AdapterError::Unknown {
                operation: "websocket",
                detail: "native App Server closed the connection".into(),
            }),
            0x9 => {
                self.stream
                    .write_all(&encode_frame(0xA, &payload))
                    .map_err(|error| transport("websocket pong", error))?;
                self.read_frame()
            }
            _ => self.read_frame(),
        }
    }
}

fn transport(operation: &'static str, error: std::io::Error) -> AdapterError {
    if matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        AdapterError::Timeout { operation }
    } else {
        AdapterError::Unknown {
            operation,
            detail: error.to_string(),
        }
    }
}

fn encode_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x80 | opcode);
    let mask = [
        rand::random::<u8>(),
        rand::random::<u8>(),
        rand::random::<u8>(),
        rand::random::<u8>(),
    ];
    match payload.len() {
        length if length < 126 => frame.push(0x80 | length as u8),
        length if length <= u16::MAX as usize => {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        }
        length => {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % 4]),
    );
    frame
}

fn websocket_key() -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: [u8; 16] = rand::random();
    let mut output = String::with_capacity(24);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let value = (b0 << 16) | (b1 << 8) | b2;
        output.push(ALPHABET[((value >> 18) & 0x3f) as usize] as char);
        output.push(ALPHABET[((value >> 12) & 0x3f) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(value & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Shutdown;
    use std::os::unix::net::UnixListener;
    use std::thread;

    fn assert_malformed_active_turn(page: Value, expected_detail: &str) {
        match active_turn_id_from_page(&page).unwrap_err() {
            AdapterError::Unknown { operation, detail } => {
                assert_eq!(operation, "turn/steer");
                assert!(detail.contains(expected_detail), "{detail}");
            }
            error => panic!("expected AdapterError::Unknown, got {error:?}"),
        }
    }

    #[test]
    fn frame_round_trip_uses_masked_client_frames() {
        let frame = encode_frame(0x1, b"hello");
        assert_eq!(frame[0], 0x81);
        assert_eq!(frame[1] & 0x80, 0x80);
        assert_eq!(frame[1] & 0x7f, 5);
        let mask = &frame[2..6];
        let decoded = frame[6..]
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % 4])
            .collect::<Vec<_>>();
        assert_eq!(decoded, b"hello");
    }

    #[test]
    fn client_handshake_and_rpc_round_trip() {
        let socket = std::env::temp_dir().join(format!(
            "collab-appserver-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
                .unwrap();
            let payload = read_client_frame(&mut stream);
            let request: Value = serde_json::from_slice(&payload).unwrap();
            let response = json!({"id": request["id"], "result": {"ok": true}});
            stream
                .write_all(&encode_frame(0x1, &serde_json::to_vec(&response).unwrap()))
                .unwrap();
            stream.shutdown(Shutdown::Both).ok();
        });

        let mut client = Client::connect(&socket, Duration::from_secs(2)).unwrap();
        let value = client.call("initialize", json!({})).unwrap();
        assert_eq!(value["ok"], true);
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn client_accepts_large_appserver_rpc_response() {
        let socket = std::env::temp_dir().join(format!(
            "caslr-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            let payload = read_client_frame(&mut stream);
            let request: Value = serde_json::from_slice(&payload).unwrap();
            let body = "x".repeat(MAX_OUTGOING_FRAME_BYTES + 1024);
            let response = json!({"id": request["id"], "result": {"body": body}});
            stream
                .write_all(&encode_frame(0x1, &serde_json::to_vec(&response).unwrap()))
                .unwrap();
            stream.shutdown(Shutdown::Both).ok();
        });

        let mut client = Client::connect(&socket, Duration::from_secs(2)).unwrap();
        let value = client.call("initialize", json!({})).unwrap();
        assert_eq!(
            value["body"].as_str().unwrap().len(),
            MAX_OUTGOING_FRAME_BYTES + 1024
        );
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn client_rejects_oversized_appserver_rpc_response() {
        let socket = std::env::temp_dir().join(format!(
            "cosar-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            let _payload = read_client_frame(&mut stream);
            stream.write_all(&[0x81, 0x7f]).unwrap();
            stream
                .write_all(&((MAX_INCOMING_FRAME_BYTES as u64 + 1).to_be_bytes()))
                .unwrap();
            stream.shutdown(Shutdown::Both).ok();
        });

        let mut client = Client::connect(&socket, Duration::from_secs(2)).unwrap();
        let error = client.call("initialize", json!({})).unwrap_err();
        match error {
            AdapterError::Unknown { operation, detail } => {
                assert_eq!(operation, "websocket frame");
                assert_eq!(detail, "native frame exceeds maximum size");
            }
            error => panic!("expected oversized websocket frame error, got {error:?}"),
        }
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_does_not_require_appserver_queue_wakeup_method() {
        let socket = std::env::temp_dir().join(format!(
            "collab-queue-required-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            loop {
                let payload = read_client_frame(&mut stream);
                let request: Value = serde_json::from_slice(&payload).unwrap();
                let Some(id) = request.get("id").cloned() else {
                    continue;
                };
                let method = request["method"].as_str().unwrap();
                let response = match method {
                    "initialize" => json!({"id": id, "result": {}}),
                    "thread/loaded/list" => {
                        json!({"id": id, "result": {"data": ["thread-1"]}})
                    }
                    "thread/read" => {
                        json!({
                            "id": id,
                            "result": {
                                "thread": {
                                    "id": "thread-1",
                                    "sessionId": "session-1",
                                    "cwd": env!("CARGO_MANIFEST_DIR")
                                }
                            }
                        })
                    }
                    "thread/items/list" => {
                        json!({"id": id, "error": {"code": -32601, "message": "unsupported"}})
                    }
                    "turn/start" => {
                        json!({"id": id, "error": {"code": -32600, "message": "invalid params"}})
                    }
                    "turn/steer" => {
                        json!({"id": id, "error": {"code": -32600, "message": "invalid params"}})
                    }
                    "thread/turns/list" => {
                        json!({"id": id, "result": {"data": []}})
                    }
                    _ => unreachable!("{method}"),
                };
                stream
                    .write_all(&encode_frame(0x1, &serde_json::to_vec(&response).unwrap()))
                    .unwrap();
                if method == "thread/turns/list" {
                    break;
                }
            }
            stream.shutdown(Shutdown::Both).ok();
        });

        let candidate = AppServerCandidate {
            endpoint: format!("unix://{}", socket.display()),
            namespace: "codex_tui".into(),
            session_id: "session-1".into(),
            thread_id: "thread-1".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
        };
        let selected = verify_candidate(&candidate).unwrap();
        assert!(!selected
            .capabilities
            .iter()
            .any(|capability| capability == "queue_wakeup"));
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_rejects_missing_appserver_thread_as_route_unavailable() {
        let socket = std::env::temp_dir().join(format!(
            "collab-missing-thread-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
                .unwrap();
            loop {
                let payload = read_client_frame(&mut stream);
                let request: Value = serde_json::from_slice(&payload).unwrap();
                let Some(id) = request.get("id").cloned() else {
                    continue;
                };
                let response = match request["method"].as_str().unwrap() {
                    "initialize" => json!({"id": id, "result": {}}),
                    "thread/loaded/list" => {
                        json!({"id": id, "result": {"data": ["missing-thread"]}})
                    }
                    "thread/read" => {
                        json!({"id": id, "error": {"code": -32602, "message": "thread not found"}})
                    }
                    method => panic!("unexpected method after missing thread: {method}"),
                };
                stream
                    .write_all(&encode_frame(0x1, &serde_json::to_vec(&response).unwrap()))
                    .unwrap();
                if request["method"] == "thread/read" {
                    break;
                }
            }
            stream.shutdown(Shutdown::Both).ok();
        });

        let candidate = AppServerCandidate {
            endpoint: format!("unix://{}", socket.display()),
            namespace: "codex_tui".into(),
            session_id: "session-1".into(),
            thread_id: "missing-thread".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
        };
        let error = verify_candidate(&candidate).unwrap_err();
        assert!(matches!(error, AdapterError::RouteUnavailable { .. }));
        assert!(error.to_string().contains("ADAPTER_ROUTE_UNAVAILABLE"));
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_verifies_persisted_cold_thread_through_thread_read() {
        let socket = std::env::temp_dir().join(format!(
            "collab-unloaded-thread-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            // Identity is established from `thread/read`, which serves
            // persisted threads.  A cold thread reports notLoaded status and
            // is loaded later by the native load-and-start call.
            let read = next_request(&mut stream);
            assert_eq!(read["method"], "thread/read");
            assert_eq!(read["params"]["threadId"], "persisted-thread");
            respond(
                &mut stream,
                json!({
                    "id": read["id"],
                    "result": {
                        "thread": {
                            "id": "persisted-thread",
                            "sessionId": "session-1",
                            "cwd": env!("CARGO_MANIFEST_DIR"),
                            "status": {"type": "notLoaded"}
                        }
                    }
                }),
            );
            let items = next_request(&mut stream);
            assert_eq!(items["method"], "thread/items/list");
            respond(
                &mut stream,
                json!({"id": items["id"], "result": {"data": []}}),
            );
            let turn_start = next_request(&mut stream);
            assert_eq!(turn_start["method"], "turn/start");
            respond(
                &mut stream,
                json!({"id": turn_start["id"], "error": {"code": -32600, "message": "invalid params"}}),
            );
            let steer = next_request(&mut stream);
            assert_eq!(steer["method"], "turn/steer");
            respond(
                &mut stream,
                json!({"id": steer["id"], "error": {"code": -32600, "message": "invalid params"}}),
            );
            let turns = next_request(&mut stream);
            assert_eq!(turns["method"], "thread/turns/list");
            respond(
                &mut stream,
                json!({"id": turns["id"], "result": {"data": []}}),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let candidate = AppServerCandidate {
            endpoint: format!("unix://{}", socket.display()),
            namespace: "codex_tui".into(),
            session_id: "session-1".into(),
            thread_id: "persisted-thread".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
        };
        let selected = verify_candidate(&candidate).unwrap();
        assert_eq!(selected.thread_id.as_deref(), Some("persisted-thread"));
        assert_eq!(selected.session_id.as_deref(), Some("session-1"));
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_rejects_thread_whose_read_reports_not_loaded() {
        let socket = std::env::temp_dir().join(format!(
            "collab-unloaded-race-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            let read = next_request(&mut stream);
            assert_eq!(read["method"], "thread/read");
            assert_eq!(read["params"]["threadId"], "persisted-thread");
            respond(
                &mut stream,
                json!({
                    "id": read["id"],
                    "error": {
                        "code": -32602,
                        "message": "thread not loaded: persisted-thread"
                    }
                }),
            );
            let mut byte = [0_u8; 1];
            assert_eq!(
                stream.read(&mut byte).unwrap(),
                0,
                "a thread whose read reports notLoaded must not issue another method"
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let candidate = AppServerCandidate {
            endpoint: format!("unix://{}", socket.display()),
            namespace: "codex_tui".into(),
            session_id: "session-persisted-thread".into(),
            thread_id: "persisted-thread".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
        };
        let error = verify_candidate(&candidate).unwrap_err();
        assert!(
            matches!(error, AdapterError::RouteUnavailable { .. }),
            "{error}"
        );
        assert!(error.to_string().contains("persisted but not loaded"));
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_accepts_thread_verified_by_thread_read_identity() {
        let socket = PathBuf::from("/tmp").join(format!(
            "collab-candidate-loaded-thread-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            let read = next_request(&mut stream);
            assert_eq!(read["method"], "thread/read");
            assert_eq!(read["params"]["threadId"], "thread-1");
            respond(
                &mut stream,
                json!({
                    "id": read["id"],
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "sessionId": "session-1",
                            "cwd": env!("CARGO_MANIFEST_DIR"),
                            "status": {"type": "idle"}
                        }
                    }
                }),
            );
            let items = next_request(&mut stream);
            assert_eq!(items["method"], "thread/items/list");
            respond(
                &mut stream,
                json!({"id": items["id"], "error": {"code": -32601, "message": "unsupported"}}),
            );
            let turn_start = next_request(&mut stream);
            assert_eq!(turn_start["method"], "turn/start");
            respond(
                &mut stream,
                json!({"id": turn_start["id"], "error": {"code": -32600, "message": "invalid params"}}),
            );
            let steer = next_request(&mut stream);
            assert_eq!(steer["method"], "turn/steer");
            respond(
                &mut stream,
                json!({"id": steer["id"], "error": {"code": -32600, "message": "invalid params"}}),
            );
            let turns = next_request(&mut stream);
            assert_eq!(turns["method"], "thread/turns/list");
            respond(
                &mut stream,
                json!({"id": turns["id"], "result": {"data": []}}),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let candidate = AppServerCandidate {
            endpoint: format!("unix://{}", socket.display()),
            namespace: "codex_tui".into(),
            session_id: "session-1".into(),
            thread_id: "thread-1".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
        };
        let selected = verify_candidate(&candidate).unwrap();
        assert_eq!(selected.thread_id.as_deref(), Some("thread-1"));
        assert!(selected.self_check.contains("thread/read"));
        assert!(!selected.self_check.contains("thread/queue/add"));
        assert!(!selected
            .capabilities
            .iter()
            .any(|capability| capability == "queue_wakeup"));
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_rejects_thread_session_mismatch() {
        let socket = PathBuf::from("/tmp").join(format!(
            "c-session-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            let read = next_request(&mut stream);
            assert_eq!(read["method"], "thread/read");
            respond(
                &mut stream,
                json!({
                    "id": read["id"],
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "sessionId": "different-session",
                            "cwd": env!("CARGO_MANIFEST_DIR")
                        }
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let candidate = AppServerCandidate {
            endpoint: format!("unix://{}", socket.display()),
            namespace: "codex_tui".into(),
            session_id: "session-1".into(),
            thread_id: "thread-1".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
        };
        let error = verify_candidate(&candidate).unwrap_err();
        assert!(
            error.to_string().contains("thread session mismatch"),
            "{error}"
        );
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_rejects_thread_cwd_mismatch() {
        let socket = PathBuf::from("/tmp").join(format!(
            "c-cwd-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            let read = next_request(&mut stream);
            assert_eq!(read["method"], "thread/read");
            respond(
                &mut stream,
                json!({
                    "id": read["id"],
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "sessionId": "session-1",
                            "cwd": "/tmp"
                        }
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let candidate = AppServerCandidate {
            endpoint: format!("unix://{}", socket.display()),
            namespace: "codex_tui".into(),
            session_id: "session-1".into(),
            thread_id: "thread-1".into(),
            cwd: env!("CARGO_MANIFEST_DIR").into(),
        };
        let error = verify_candidate(&candidate).unwrap_err();
        assert!(error.to_string().contains("thread cwd mismatch"), "{error}");
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn candidate_rejects_appserver_without_steer_or_turns_list_methods() {
        for missing_method in ["turn/steer", "thread/turns/list"] {
            let socket = std::env::temp_dir().join(format!(
                "collab-candidate-method-{}-{}.sock",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let Some(listener) = bind_test_socket(&socket) else {
                return;
            };
            let missing_method = missing_method.to_string();
            let server_missing_method = missing_method.clone();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                handshake(&mut stream);
                loop {
                    let payload = read_client_frame(&mut stream);
                    let request: Value = serde_json::from_slice(&payload).unwrap();
                    let Some(id) = request.get("id").cloned() else {
                        continue;
                    };
                    let method = request["method"].as_str().unwrap();
                    let response = if method == server_missing_method {
                        json!({"id": id, "error": {"code": -32601, "message": "unsupported"}})
                    } else {
                        match method {
                            "initialize" => json!({"id": id, "result": {}}),
                            "thread/loaded/list" => {
                                json!({"id": id, "result": {"data": ["thread-1"]}})
                            }
                            "thread/read" => {
                                json!({
                                    "id": id,
                                    "result": {
                                        "thread": {
                                            "id": "thread-1",
                                            "sessionId": "session-1",
                                            "cwd": env!("CARGO_MANIFEST_DIR")
                                        }
                                    }
                                })
                            }
                            "thread/items/list" => {
                                json!({"id": id, "error": {"code": -32601, "message": "unsupported"}})
                            }
                            "turn/start" | "turn/steer" => {
                                json!({"id": id, "error": {"code": -32600, "message": "invalid params"}})
                            }
                            "thread/turns/list" => {
                                json!({"id": id, "result": {"data": []}})
                            }
                            _ => unreachable!("{method}"),
                        }
                    };
                    stream
                        .write_all(&encode_frame(0x1, &serde_json::to_vec(&response).unwrap()))
                        .unwrap();
                    if method == server_missing_method {
                        break;
                    }
                }
                stream.shutdown(Shutdown::Both).ok();
            });

            let candidate = AppServerCandidate {
                endpoint: format!("unix://{}", socket.display()),
                namespace: "codex_tui".into(),
                session_id: "session-1".into(),
                thread_id: "thread-1".into(),
                cwd: env!("CARGO_MANIFEST_DIR").into(),
            };
            let error = verify_candidate(&candidate).unwrap_err();
            assert!(
                matches!(
                    &error,
                    AdapterError::CapabilityUnavailable {
                        operation,
                        ..
                    } if *operation == missing_method
                ),
                "{missing_method}: {error}"
            );
            server.join().unwrap();
            std::fs::remove_file(socket).ok();
        }
    }

    #[test]
    fn immediate_receipt_requires_turn_identity_and_protocol_status() {
        validate_immediate_receipt(&json!({
            "turn": {"id": "turn-1", "status": "inProgress"}
        }))
        .unwrap();

        for malformed in [
            json!({}),
            json!({"turn": {"status": "inProgress"}}),
            json!({"turn": {"id": "turn-1"}}),
            json!({"turn": {"id": "turn-1", "status": "queued"}}),
            json!({"turn": {"id": "bad turn", "status": "completed"}}),
        ] {
            assert!(
                validate_immediate_receipt(&malformed).is_err(),
                "{malformed}"
            );
        }
    }

    #[test]
    fn steer_receipt_requires_matching_turn_identity() {
        validate_steer_receipt(&json!({"turnId": "turn-1"}), "turn-1").unwrap();

        for malformed in [
            json!({}),
            json!({"turnId": ""}),
            json!({"turnId": "turn-2"}),
            json!({"turnId": "bad turn"}),
        ] {
            assert!(
                validate_steer_receipt(&malformed, "turn-1").is_err(),
                "{malformed}"
            );
        }
    }

    #[test]
    fn active_turn_selection_only_accepts_in_progress_turns() {
        assert_eq!(
            active_turn_id_from_page(&json!({
                "data": [{"id": "turn-active", "status": "inProgress"}]
            }))
            .unwrap()
            .as_deref(),
            Some("turn-active")
        );
        assert_eq!(
            active_turn_id_from_page(&json!({
                "data": [{"id": "turn-interrupted", "status": "interrupted"}]
            }))
            .unwrap(),
            None
        );
        assert_eq!(
            active_turn_id_from_page(&json!({
                "data": [
                    {"id": "turn-interrupted", "status": "interrupted"},
                    {"id": "turn-completed", "status": "completed"}
                ]
            }))
            .unwrap(),
            None
        );
    }

    #[test]
    fn active_turn_selection_rejects_unknown_and_missing_status() {
        for turn in [
            json!({"id": "turn-queued", "status": "queued"}),
            json!({"id": "turn-1"}),
        ] {
            let error = active_turn_id_from_page(&json!({"data": [turn]})).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("AUTO_NOTIFY_UNSUPPORTED_TURN_STATUS")
                    || error.to_string().contains("is missing status"),
                "{error}"
            );
        }
    }

    #[test]
    fn active_turn_selection_rejects_missing_id() {
        assert_malformed_active_turn(
            json!({"data": [{"status": "inProgress"}]}),
            "data[0] is missing id",
        );
    }

    #[test]
    fn active_turn_selection_rejects_non_string_id() {
        assert_malformed_active_turn(
            json!({"data": [{"id": 7, "status": "inProgress"}]}),
            "data[0] id must be a JSON string",
        );
    }

    #[test]
    fn active_turn_selection_rejects_empty_id() {
        for id in ["", "   "] {
            assert_malformed_active_turn(
                json!({"data": [{"id": id, "status": "inProgress"}]}),
                "data[0] id must be non-empty after trim",
            );
        }
    }

    #[test]
    fn active_turn_selection_rejects_whitespace_containing_id() {
        assert_malformed_active_turn(
            json!({"data": [{"id": "turn active", "status": "inProgress"}]}),
            "data[0] id must not contain whitespace",
        );
    }

    #[test]
    fn active_turn_selection_handles_valid_zero_and_multiple_turns() {
        assert_eq!(
            active_turn_id_from_page(&json!({"data": []})).unwrap(),
            None
        );
        assert_eq!(
            active_turn_id_from_page(&json!({
                "data": [{"id": "turn-active", "status": "inProgress"}]
            }))
            .unwrap()
            .as_deref(),
            Some("turn-active")
        );
        let error = active_turn_id_from_page(&json!({
            "data": [
                {"id": "turn-1", "status": "inProgress"},
                {"id": "turn-2", "status": "inProgress"}
            ]
        }))
        .unwrap_err();
        assert!(
            error.to_string().contains("STEER_ACTIVE_TURN_AMBIGUOUS"),
            "{error}"
        );
    }

    #[test]
    fn notification_action_uses_turn_start_when_active_has_no_in_progress_turn() {
        assert_eq!(
            notification_action("active", None).unwrap(),
            NotificationAction::Start
        );
        assert_eq!(
            notification_action("active", Some("turn-active".into())).unwrap(),
            NotificationAction::Steer("turn-active".into())
        );
        assert_eq!(
            notification_action("idle", None).unwrap(),
            NotificationAction::Start
        );
        // A cold thread is loaded by `turn/start`, which is the native
        // load-and-start call, so notLoaded resolves to Start.
        assert_eq!(
            notification_action("notLoaded", None).unwrap(),
            NotificationAction::Start
        );
        for status in ["systemError", "unknown", ""] {
            let error = notification_action(status, None).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("AUTO_NOTIFY_UNSUPPORTED_THREAD_STATUS"),
                "{error}"
            );
        }
    }

    #[test]
    fn active_interrupted_only_and_in_progress_turn_actions_are_explicit() {
        let interrupted_only = active_turn_id_from_page(&json!({
            "data": [{"id": "turn-interrupted", "status": "interrupted"}]
        }))
        .unwrap();
        assert_eq!(
            notification_action("active", interrupted_only).unwrap(),
            NotificationAction::Start
        );

        let active = active_turn_id_from_page(&json!({
            "data": [{"id": "turn-active", "status": "inProgress"}]
        }))
        .unwrap();
        assert_eq!(
            notification_action("active", active).unwrap(),
            NotificationAction::Steer("turn-active".into())
        );
    }

    #[test]
    fn immediate_notify_routes_active_thread_to_steer() {
        let socket = temp_socket("notify-active");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "active", "activeFlags": []}
                        }
                    }
                }),
            );
            let turns_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": turns_id,
                    "result": {
                        "data": [
                            {"id": "turn-active", "status": "inProgress", "items": []}
                        ]
                    }
                }),
            );
            let request = next_request(&mut stream);
            assert_eq!(request["method"], "turn/steer");
            assert_eq!(request["params"]["threadId"], "thread-1");
            assert_eq!(request["params"]["expectedTurnId"], "turn-active");
            assert_eq!(request["params"]["clientUserMessageId"], "message-active");
            respond(
                &mut stream,
                json!({"id": request["id"], "result": {"turnId": "turn-active"}}),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let receipt = immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-active",
        )
        .unwrap();
        assert_eq!(receipt["turnId"], "turn-active");
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_starts_active_thread_with_only_interrupted_turn() {
        let socket = temp_socket("notify-interrupted");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "active", "activeFlags": []}
                        }
                    }
                }),
            );
            let turns_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": turns_id,
                    "result": {
                        "data": [
                            {"id": "turn-interrupted", "status": "interrupted", "items": []}
                        ]
                    }
                }),
            );
            let request = next_request(&mut stream);
            assert_eq!(request["method"], "turn/start");
            assert_eq!(request["params"]["threadId"], "thread-1");
            assert_eq!(
                request["params"]["clientUserMessageId"],
                "message-interrupted"
            );
            respond(
                &mut stream,
                json!({
                    "id": request["id"],
                    "result": {
                        "turn": {"id": "turn-started", "status": "inProgress", "items": []}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let receipt = immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-interrupted",
        )
        .unwrap();
        assert_eq!(receipt["turn"]["id"], "turn-started");
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_rejects_multiple_in_progress_turns() {
        let socket = temp_socket("notify-multiple-active");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "active", "activeFlags": []}
                        }
                    }
                }),
            );
            let turns_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": turns_id,
                    "result": {
                        "data": [
                            {"id": "turn-1", "status": "inProgress", "items": []},
                            {"id": "turn-2", "status": "inProgress", "items": []}
                        ]
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let error = immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-multiple-active",
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("STEER_ACTIVE_TURN_AMBIGUOUS"),
            "{error}"
        );
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_routes_idle_thread_to_turn_start() {
        let socket = temp_socket("notify-start-idle");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "idle"}
                        }
                    }
                }),
            );
            let request = next_request(&mut stream);
            assert_eq!(request["method"], "turn/start");
            assert_eq!(request["params"]["threadId"], "thread-1");
            assert_eq!(request["params"]["input"], json!([]));
            assert_eq!(
                request["params"]["toolOutput"]["name"],
                "send_message_to_thread"
            );
            assert_eq!(request["params"]["toolOutput"]["namespace"], "codex_tui");
            assert_eq!(
                request["params"]["toolOutput"]["output"],
                "<codex_delegation>\n  <source_thread_id>sender-thread</source_thread_id>\n  <client_message_id>message-start</client_message_id>\n  <input>notify body</input>\n</codex_delegation>"
            );
            respond(
                &mut stream,
                json!({
                    "id": request["id"],
                    "result": {
                        "turn": {"id": "turn-started", "status": "inProgress", "items": []}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-start",
        )
        .unwrap();
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_attributes_turn_start_to_sender_not_recipient() {
        let socket = temp_socket("notify-source-thread");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "recipient-thread",
                            "status": {"type": "idle"}
                        }
                    }
                }),
            );
            let request = next_request(&mut stream);
            assert_eq!(request["method"], "turn/start");
            assert_eq!(request["params"]["threadId"], "recipient-thread");
            assert_eq!(
                request["params"]["toolOutput"]["output"],
                "<codex_delegation>\n  <source_thread_id>sender-thread</source_thread_id>\n  <client_message_id>message-source-thread</client_message_id>\n  <input>notify body</input>\n</codex_delegation>"
            );
            respond(
                &mut stream,
                json!({
                    "id": request["id"],
                    "result": {
                        "turn": {"id": "turn-started", "status": "inProgress", "items": []}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let mut transport = selected_transport(&socket);
        transport.thread_id = Some("recipient-thread".into());
        immediate_notify(
            &transport,
            Some("sender-thread"),
            "notify body",
            "message-source-thread",
        )
        .unwrap();
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_loads_not_loaded_thread_through_turn_start() {
        let socket = temp_socket("notify-not-loaded-start");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "notLoaded"}
                        }
                    }
                }),
            );
            // A cold thread is loaded by the immediate notification itself:
            // `turn/start` is the native load-and-start call.
            let start = next_request(&mut stream);
            assert_eq!(start["method"], "turn/start");
            assert_eq!(start["params"]["threadId"], "thread-1");
            respond(
                &mut stream,
                json!({
                    "id": start["id"],
                    "result": {
                        "turn": {"id": "turn-loaded", "status": "inProgress", "items": []}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-start",
        )
        .unwrap();
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_materializes_missing_rollout_through_turn_start() {
        let socket = temp_socket("notify-missing-rollout-start");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "missingRollout");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "idle"}
                        }
                    }
                }),
            );
            let start = next_request(&mut stream);
            assert_eq!(start["method"], "turn/start");
            assert_eq!(start["params"]["threadId"], "thread-1");
            assert_eq!(start["params"]["clientUserMessageId"], "message-rollout");
            respond(
                &mut stream,
                json!({
                    "id": start["id"],
                    "result": {
                        "turn": {"id": "turn-materialized", "status": "inProgress", "items": []}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let receipt = immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-rollout",
        )
        .unwrap();
        assert_eq!(receipt["turn"]["id"], "turn-materialized");
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_refuses_when_resumed_thread_still_reports_not_loaded() {
        let socket = temp_socket("notify-read-not-loaded-start");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "error": {
                        "code": -32602,
                        "message": "thread not loaded: thread-1"
                    }
                }),
            );
            // A thread that this connection just resumed must not still report
            // not-loaded; delivery refuses instead of starting a turn.
            let mut byte = [0_u8; 1];
            assert_eq!(
                stream.read(&mut byte).unwrap(),
                0,
                "a resumed-but-still-cold thread must not issue turn/start"
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let error = immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-start",
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("still reports not loaded"),
            "{error}"
        );
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_rejects_missing_thread_without_turn_start() {
        for response in [
            json!({
                "error": {
                    "code": -32602,
                    "message": "thread not found: thread-1"
                }
            }),
            json!({
                "error": {
                    "code": -32602,
                    "message": "thread not found"
                }
            }),
        ] {
            let socket = temp_socket("notify-not-loaded-reject");
            let Some(listener) = bind_test_socket(&socket) else {
                return;
            };
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                handshake(&mut stream);
                initialize(&mut stream);
                prepare_recipient_thread(&mut stream, "ok");
                let read_id = next_request_id(&mut stream);
                let mut response = response;
                response["id"] = json!(read_id);
                respond(&mut stream, response);
                let mut byte = [0_u8; 1];
                assert_eq!(
                    stream.read(&mut byte).unwrap(),
                    0,
                    "not-loaded admission must not issue another App Server method"
                );
                stream.shutdown(Shutdown::Both).ok();
            });

            let error = immediate_notify(
                &selected_transport(&socket),
                Some("sender-thread"),
                "notify body",
                "message-start",
            )
            .unwrap_err();
            assert!(
                matches!(error, AdapterError::RouteUnavailable { .. }),
                "{error}"
            );
            server.join().unwrap();
            std::fs::remove_file(socket).ok();
        }
    }

    #[test]
    fn immediate_notify_rejects_wrapped_missing_thread_without_turn_start() {
        let socket = temp_socket("notify-wrapped-missing-thread");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "error": {
                        "code": -32602,
                        "message": "rpc unknown: thread not found"
                    }
                }),
            );
            let mut byte = [0_u8; 1];
            assert_eq!(
                stream.read(&mut byte).unwrap(),
                0,
                "missing route must not issue another App Server method"
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let error = immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-start",
        )
        .unwrap_err();
        assert!(
            matches!(error, AdapterError::RouteUnavailable { .. }),
            "{error}"
        );
        assert_eq!(
            error.to_string(),
            "ADAPTER_ROUTE_UNAVAILABLE: rpc unknown: thread not found"
        );
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn immediate_notify_reports_thread_writer_conflict_as_terminal() {
        let socket = temp_socket("notify-writer-conflict");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            let request = next_request(&mut stream);
            assert_eq!(request["method"], "thread/resume");
            respond(
                &mut stream,
                json!({
                    "id": request["id"],
                    "error": {
                        "code": -32600,
                        "message": "thread thread-1 already has an active writer"
                    }
                }),
            );
            let mut byte = [0_u8; 1];
            assert_eq!(
                stream.read(&mut byte).unwrap(),
                0,
                "a writer conflict must not issue another App Server method"
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        let error = immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-start",
        )
        .unwrap_err();
        // The recipient is owned by another live App Server process, so this
        // endpoint must report a terminal conflict instead of an opaque rpc
        // failure or a retryable route error.
        assert!(
            matches!(error, AdapterError::ThreadWriterConflict { .. }),
            "{error}"
        );
        let rendered = error.to_string();
        assert!(
            rendered.starts_with("APPSERVER_THREAD_WRITER_CONFLICT: "),
            "{rendered}"
        );
        assert!(
            rendered.contains("thread thread-1 already has an active writer"),
            "{rendered}"
        );
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    #[test]
    fn thread_writer_conflict_detection_matches_only_the_writer_refusal() {
        assert!(is_thread_writer_conflict(
            "thread 01a0a530-cca3-76e3-9364-699d66ded6f0 already has an active writer"
        ));
        assert!(is_thread_writer_conflict(
            "thread-store conflict: thread thread-1 already has an active writer"
        ));
        for detail in [
            "thread not found: thread-1",
            "thread not loaded: thread-1",
            "thread thread-1 is closing; retry thread/resume after the thread is closed",
            "",
        ] {
            assert!(!is_thread_writer_conflict(detail), "{detail}");
        }
    }

    #[test]
    fn wrapped_missing_thread_error_is_route_unavailable() {
        for detail in [
            "thread not found: thread-1",
            "rpc unknown: thread not found",
            "rpc unknown: thread not found: thread-1",
        ] {
            assert!(is_thread_not_found_error("rpc", detail, "thread-1"));
        }
        for detail in [
            "thread not found: other-thread",
            "rpc unknown: thread not found: other-thread",
            "thread not loaded: thread-1",
            "rpc unknown: thread not loaded: thread-1",
        ] {
            assert!(!is_thread_not_found_error("rpc", detail, "thread-1"));
        }
        assert!(!is_thread_not_found_error(
            "thread/read",
            "rpc unknown: thread not found: thread-1",
            "thread-1"
        ));
    }

    #[test]
    fn delegated_prompt_escapes_xml_special_characters() {
        assert_eq!(
            delegated_prompt(Some("thread-1"), "message-1", "a & b < c > d"),
            "<codex_delegation>\n  <source_thread_id>thread-1</source_thread_id>\n  <client_message_id>message-1</client_message_id>\n  <input>a &amp; b &lt; c &gt; d</input>\n</codex_delegation>"
        );
        assert_eq!(
            delegated_prompt(None, "message-1", "automatic"),
            "<codex_delegation>\n  <client_message_id>message-1</client_message_id>\n  <input>automatic</input>\n</codex_delegation>"
        );
    }

    #[test]
    #[ignore]
    fn live_immediate_notify_accepts_loaded_thread() {
        let Some(candidate) = candidate_from_env().unwrap() else {
            panic!("CODEX_THREAD_ID is required");
        };
        let transport = verify_candidate(&candidate).expect("live App Server candidate");
        let receipt = immediate_notify(
            &transport,
            Some(candidate.thread_id.as_str()),
            "Reply with exactly COLLAB_START_PROBE_OK and do not run tools.",
            "collab-start-probe",
        )
        .expect("live immediate notify");
        assert_eq!(receipt["turn"]["status"], "inProgress");
    }

    #[test]
    fn immediate_notify_rejects_queued_submission_as_success() {
        let socket = temp_socket("notify-queued");
        let Some(listener) = bind_test_socket(&socket) else {
            return;
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            handshake(&mut stream);
            initialize(&mut stream);
            prepare_recipient_thread(&mut stream, "ok");
            let read_id = next_request_id(&mut stream);
            respond(
                &mut stream,
                json!({
                    "id": read_id,
                    "result": {
                        "thread": {
                            "id": "thread-1",
                            "status": {"type": "idle"}
                        }
                    }
                }),
            );
            let request = next_request(&mut stream);
            assert_eq!(request["method"], "turn/start");
            respond(
                &mut stream,
                json!({
                    "id": request["id"],
                    "result": {
                        "queuedSubmission": {"id": "queue-1"}
                    }
                }),
            );
            stream.shutdown(Shutdown::Both).ok();
        });

        assert!(immediate_notify(
            &selected_transport(&socket),
            Some("sender-thread"),
            "notify body",
            "message-queued"
        )
        .is_err());
        server.join().unwrap();
        std::fs::remove_file(socket).ok();
    }

    fn selected_transport(socket: &Path) -> SelectedTransport {
        SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some(format!("unix://{}", socket.display())),
            namespace: Some("codex_tui".into()),
            session_id: Some("session-1".into()),
            thread_id: Some("thread-1".into()),
            tmux_endpoint: None,
            capabilities: vec!["send_message_to_thread".into()],
            self_check: "test".into(),
        }
    }

    fn temp_socket(tag: &str) -> PathBuf {
        PathBuf::from(format!(
            "collab-{tag}-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn bind_test_socket(socket: &Path) -> Option<UnixListener> {
        match UnixListener::bind(socket) {
            Ok(listener) => Some(listener),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!(
                    "SKIP socket integration assertion: sandbox denied unix socket bind at {}",
                    socket.display()
                );
                None
            }
            Err(error) => panic!("bind {}: {error}", socket.display()),
        }
    }

    fn handshake(stream: &mut UnixStream) {
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        stream
            .write_all(
                b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
            )
            .unwrap();
    }

    fn initialize(stream: &mut UnixStream) {
        let request = next_request(stream);
        assert_eq!(request["method"], "initialize");
        respond(stream, json!({"id": request["id"], "result": {}}));
        let initialized = read_client_frame(stream);
        let initialized: Value = serde_json::from_slice(&initialized).unwrap();
        assert_eq!(initialized["method"], "initialized");
    }

    /// Answer the per-connection `thread/resume` that immediate delivery issues
    /// before it can start a turn.  Test fixtures that model a usable thread
    /// answer success; the ones modelling a missing thread answer not-found,
    /// which delivery must surface unchanged.
    fn prepare_recipient_thread(stream: &mut UnixStream, outcome: &str) {
        let request = next_request(stream);
        assert_eq!(request["method"], "thread/resume");
        match outcome {
            "ok" => respond(
                stream,
                json!({
                    "id": request["id"],
                    "result": {"thread": {"id": request["params"]["threadId"]}}
                }),
            ),
            "notFound" => respond(
                stream,
                json!({
                    "id": request["id"],
                    "error": {"code": -32600, "message": "thread not found"}
                }),
            ),
            "missingRollout" => respond(
                stream,
                json!({
                    "id": request["id"],
                    "error": {
                        "code": -32602,
                        "message": format!("no rollout found for thread id {}", request["params"]["threadId"].as_str().unwrap())
                    }
                }),
            ),
            other => panic!("unsupported recipient thread outcome {other}"),
        }
    }

    fn next_request(stream: &mut UnixStream) -> Value {
        serde_json::from_slice(&read_client_frame(stream)).unwrap()
    }

    fn next_request_id(stream: &mut UnixStream) -> Value {
        next_request(stream)["id"].clone()
    }

    fn respond(stream: &mut UnixStream, value: Value) {
        stream
            .write_all(&encode_frame(0x1, &serde_json::to_vec(&value).unwrap()))
            .unwrap();
    }

    fn read_client_frame(stream: &mut UnixStream) -> Vec<u8> {
        try_read_client_frame(stream).unwrap()
    }

    fn try_read_client_frame(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
        let mut header = [0_u8; 2];
        stream.read_exact(&mut header)?;
        let masked = header[1] & 0x80 != 0;
        let mut length = (header[1] & 0x7f) as usize;
        if length == 126 {
            let mut bytes = [0_u8; 2];
            stream.read_exact(&mut bytes)?;
            length = u16::from_be_bytes(bytes) as usize;
        }
        let mut mask = [0_u8; 4];
        if masked {
            stream.read_exact(&mut mask)?;
        }
        let mut payload = vec![0_u8; length];
        stream.read_exact(&mut payload)?;
        if masked {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
        Ok(payload)
    }
}
