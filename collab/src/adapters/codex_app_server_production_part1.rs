use super::{AdapterCapabilities, AdapterError, EndpointKind, WakeMode};
use crate::identity::NativeThreadId;
use crate::proto::{AppServerCandidate, IdentityFacts, SelectedTransport, TransportKind};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const APPSERVER_SOCKET_ENV: &str = "COLLAB_APPSERVER_SOCKET";
pub const APPSERVER_NAMESPACE_ENV: &str = "COLLAB_APPSERVER_NAMESPACE";
pub const APPSERVER_TIMEOUT_MS_ENV: &str = "COLLAB_APPSERVER_TIMEOUT_MS";
const CODEX_ORIGINATOR_ENV: &str = "CODEX_INTERNAL_ORIGINATOR_OVERRIDE";
pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_OUTGOING_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_INCOMING_FRAME_BYTES: usize = 64 * 1024 * 1024;

fn appserver_namespace(
    explicit: Option<&str>,
    originator: Option<&str>,
    tmux_pane: Option<&str>,
) -> Result<&'static str, AdapterError> {
    if let Some(explicit) = explicit.map(str::trim).filter(|value| !value.is_empty()) {
        return match explicit {
            "codex_tui" => Ok("codex_tui"),
            "codex_app" => Ok("codex_app"),
            namespace => Err(AdapterError::Unknown {
                operation: "detect",
                detail: format!("unsupported App Server namespace {namespace}"),
            }),
        };
    }

    match originator.map(str::trim).filter(|value| !value.is_empty()) {
        Some("Codex Desktop") => Ok("codex_app"),
        Some("Codex CLI" | "Codex TUI") => Ok("codex_tui"),
        Some(originator) => Err(AdapterError::Unknown {
            operation: "detect",
            detail: format!("unsupported Codex host originator {originator}"),
        }),
        None if tmux_pane.map(str::trim).is_some_and(|value| !value.is_empty()) => {
            Ok("codex_tui")
        }
        None => Err(AdapterError::Unknown {
            operation: "detect",
            detail: format!(
                "cannot identify Codex host; set {APPSERVER_NAMESPACE_ENV} only for a nonstandard runtime"
            ),
        }),
    }
}

fn appserver_namespace_from_env() -> Result<&'static str, AdapterError> {
    let explicit = std::env::var(APPSERVER_NAMESPACE_ENV).ok();
    let originator = std::env::var(CODEX_ORIGINATOR_ENV).ok();
    let tmux_pane = std::env::var("TMUX_PANE").ok();
    appserver_namespace(
        explicit.as_deref(),
        originator.as_deref(),
        tmux_pane.as_deref(),
    )
}

fn selected_namespace(transport: &SelectedTransport) -> Result<&str, AdapterError> {
    let namespace = transport
        .namespace
        .as_deref()
        .filter(|value| matches!(*value, "codex_tui" | "codex_app"))
        .ok_or_else(|| AdapterError::InvalidBinding {
            detail: "selected App Server transport has no supported namespace".into(),
        })?;
    Ok(namespace)
}

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
        let namespace = appserver_namespace_from_env()?;
        let Some(socket_path) = socket_candidate(namespace) else {
            return Ok(None);
        };
        if !socket_path.is_absolute() || !socket_path.exists() {
            return Ok(None);
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
            namespace: namespace.to_owned(),
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
    let explicit_endpoint = [APPSERVER_SOCKET_ENV, "CODEX_APP_SERVER_SOCKET"]
        .into_iter()
        .filter_map(std::env::var_os)
        .any(|value| !value.is_empty());
    let namespace = match appserver_namespace_from_env() {
        Ok(namespace) => namespace,
        Err(error) if explicit_endpoint => return Err(error),
        Err(_) => return Ok(None),
    };
    let Some(socket_path) = socket_candidate(namespace) else {
        return Ok(None);
    };
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
        namespace: namespace.to_owned(),
        session_id,
        thread_id,
        cwd,
    }))
}

/// Observe partial identity facts without granting or selecting an identity.
///
/// Missing session/thread fields are valid here because the daemon decides
/// which facts it needs and whether the caller may establish a new identity.
pub(crate) fn identity_facts_from_env() -> Result<IdentityFacts, AdapterError> {
    let mut facts = IdentityFacts::default();
    facts.session_id = std::env::var("CODEX_SESSION_ID")
        .ok()
        .filter(|value| !value.trim().is_empty());
    facts.thread_id = std::env::var("CODEX_THREAD_ID")
        .ok()
        .filter(|value| !value.trim().is_empty());

    let explicit_endpoint = [APPSERVER_SOCKET_ENV, "CODEX_APP_SERVER_SOCKET"]
        .into_iter()
        .find_map(|key| std::env::var_os(key).filter(|value| !value.is_empty()));
    if let Some(endpoint) = explicit_endpoint.as_ref() {
        facts.endpoint = Some(format!("unix://{}", PathBuf::from(endpoint).display()));
    }
    let explicit_namespace = std::env::var(APPSERVER_NAMESPACE_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty());
    match appserver_namespace_from_env() {
        Ok(namespace) => {
            facts.namespace = Some(namespace.to_owned());
            if facts.endpoint.is_none() {
                if let Some(socket_path) = socket_candidate(namespace) {
                    facts.endpoint = Some(format!("unix://{}", socket_path.display()));
                }
            }
        }
        Err(error) if explicit_namespace.is_some() => return Err(error),
        Err(_) => {}
    }

    if std::env::var_os("TMUX_PANE").is_some() {
        facts.tmux = Some(crate::client::adapters::tmux::candidate_from_env().map_err(
            |error| AdapterError::InvalidBinding {
                detail: format!("cannot collect tmux candidate: {error}"),
            },
        )?);
    }
    // The dsh adapter designs `DSH_SESSION_ID` as the agent's primary anchor.
    // Only the anchor is observable here: the gateway control socket address
    // and its runtime/agent ids belong to the gateway, which registers its own
    // peer. A missing anchor is not an error, exactly like a missing CODEX one.
    facts.dsh_session_id = std::env::var("DSH_SESSION_ID")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    Ok(facts)
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
    let namespace = selected_namespace(transport)?;
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
                        "namespace": namespace,
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
        NotificationAction::Queue => Err(AdapterError::CapabilityUnavailable {
            endpoint: EndpointKind::Tui,
            operation: "thread/queue/add",
        }),
    }
}

/// Queue one notification through a server-selected App Server transport.
/// A successful result means the App Server accepted the queued submission;
/// execution and reply are observed separately.
pub fn queued_notify(
    transport: &SelectedTransport,
    source_thread_id: Option<&str>,
    body: &str,
    client_user_message_id: &str,
) -> Result<Value, AdapterError> {
    if transport.kind != TransportKind::AppServer {
        return Err(AdapterError::CapabilityUnavailable {
            endpoint: EndpointKind::Tui,
            operation: "thread/queue/add",
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
    let namespace = selected_namespace(transport)?;
    let socket_path = endpoint_path(endpoint)?;
    let thread_id = NativeThreadId::new(thread_id.to_owned()).map_err(|error| {
        AdapterError::InvalidBinding {
            detail: format!("selected App Server thread_id is invalid: {error}"),
        }
    })?;
    let mut client = Client::connect(&socket_path, Duration::from_millis(DEFAULT_TIMEOUT_MS))?;
    client.initialize()?;
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
        Err(AdapterError::CapabilityUnavailable { .. }) => false,
        Err(error) => return Err(error),
    };
    let status = match thread_metadata(&mut client, thread_id.as_str()) {
        Ok(thread) => thread_status_from_metadata(&thread)?,
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
    let action = queued_notification_action(&status, active_turn_id)?;
    match action {
        NotificationAction::Start => {
            let receipt = client.call(
                "turn/start",
                json!({
                    "threadId": thread_id.as_str(),
                    "input": [],
                    "toolOutput": {
                        "name": "send_message_to_thread",
                        "namespace": namespace,
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
        NotificationAction::Queue => {
            let receipt = client.call(
                "thread/queue/add",
                json!({
                    "threadId": thread_id.as_str(),
                    "input": [{"type": "text", "text": body, "text_elements": []}],
                    "clientUserMessageId": client_user_message_id,
                }),
            )?;
            validate_queue_receipt(&receipt)?;
            Ok(receipt)
        }
        NotificationAction::Steer(_) => unreachable!("queued notify never steers"),
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
