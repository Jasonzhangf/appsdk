use crate::proto::{SelectedTransport, TransportKind};
use crate::scope::{HostPaths, ProjectScopeId, Scope};
use anyhow::Context;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> anyhow::Result<Self> {
                let value = value.into();
                validate_id(&value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

const MAX_ID_LENGTH: usize = 256;

fn validate_id(value: &str) -> anyhow::Result<()> {
    if value.is_empty() {
        anyhow::bail!("identifier must not be empty");
    }
    if value.len() > MAX_ID_LENGTH {
        anyhow::bail!("identifier exceeds {MAX_ID_LENGTH} bytes");
    }
    if value.chars().any(char::is_control) {
        anyhow::bail!("identifier must not contain control characters");
    }
    Ok(())
}

string_id!(AgentId);
string_id!(RuntimeId);
string_id!(AppServerId);
string_id!(BindingId);
string_id!(NativeThreadId);
string_id!(SessionId);
string_id!(TurnId);
string_id!(MessageId);
string_id!(DispatchId);
string_id!(CommandId);
string_id!(OperationId);

pub(crate) fn validate_id_for_protocol(value: &str) -> anyhow::Result<()> {
    validate_id(value)
}

/// The stable AppServer route owned by the CLI adapter. A first registration
/// has no persisted runtime binding yet, so it may use this contract only to
/// construct the request context. Existing bindings retain the AppServer
/// scope established by their native endpoint.
pub const CLI_APP_SERVER_ID: &str = "appserver-cli";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeIdentity {
    pub agent_id: AgentId,
    pub runtime_id: RuntimeId,
    pub appserver_id: AppServerId,
    pub endpoint_generation: u64,
    pub binding_id: BindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_thread_id: Option<NativeThreadId>,
}

fn validate_registration_transport(
    transport: &SelectedTransport,
    runtime: &RuntimeIdentity,
) -> anyhow::Result<()> {
    let endpoint = transport
        .endpoint
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("selected transport has no endpoint"))?;
    let thread_id = transport
        .thread_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("selected transport has no pane/thread address"))?;
    let session_id = transport
        .session_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("selected transport has no session address"))?;
    if runtime
        .native_thread_id
        .as_ref()
        .map(NativeThreadId::as_str)
        != Some(thread_id)
    {
        anyhow::bail!("selected transport thread/pane address does not match typed binding");
    }
    if runtime.session_id.as_ref().map(SessionId::as_str) != Some(session_id) {
        anyhow::bail!("selected transport session address does not match typed binding");
    }
    match transport.kind {
        TransportKind::AppServer => {
            let namespace = transport
                .namespace
                .as_deref()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow::anyhow!("selected App Server transport has no namespace"))?;
            if !matches!(namespace, "codex_tui" | "codex_app") {
                anyhow::bail!(
                    "selected App Server transport has unsupported namespace {namespace}"
                );
            }
            if !endpoint.starts_with("unix://") {
                anyhow::bail!("selected App Server transport endpoint is not unix://");
            }
            if let Some(recovery) = transport.tmux_endpoint.as_ref() {
                if recovery.socket_path.is_empty()
                    || recovery.tmux_session_id.is_empty()
                    || recovery.pane_id.is_empty()
                {
                    anyhow::bail!("selected App Server recovery anchor is incomplete");
                }
            }
        }
        TransportKind::Tmux => {
            let tmux = transport
                .tmux_endpoint
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("selected tmux transport has no tmux endpoint"))?;
            let expected_session = tmux
                .codex_session_id
                .as_deref()
                .unwrap_or(&tmux.tmux_session_id);
            let expected_thread = tmux.codex_thread_id.as_deref().unwrap_or(&tmux.pane_id);
            if endpoint != tmux.socket_path
                || thread_id != expected_thread
                || session_id != expected_session
            {
                anyhow::bail!("selected tmux address does not match its endpoint");
            }
        }
    }
    if transport.self_check.trim().is_empty() {
        anyhow::bail!("selected transport is missing its server self-check");
    }
    Ok(())
}

impl RuntimeIdentity {
    /// Construct the only provisional identity permitted before registration.
    /// Its binding and generation are never treated as a registered runtime;
    /// they exist solely so the first Register request can carry a validated
    /// app/project context.
    pub fn cli_adapter(worker_id: &str) -> anyhow::Result<Self> {
        let identity = Self {
            agent_id: AgentId::new(worker_id.to_owned())?,
            runtime_id: RuntimeId::new(format!("runtime-{worker_id}"))?,
            appserver_id: AppServerId::new(CLI_APP_SERVER_ID)?,
            endpoint_generation: 0,
            binding_id: BindingId::new(format!("binding-{worker_id}"))?,
            session_id: None,
            native_thread_id: None,
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_id(self.agent_id.as_str())?;
        validate_id(self.runtime_id.as_str())?;
        validate_id(self.appserver_id.as_str())?;
        validate_id(self.binding_id.as_str())?;
        if let Some(session_id) = &self.session_id {
            validate_id(session_id.as_str())?;
        }
        if let Some(native_thread_id) = &self.native_thread_id {
            validate_id(native_thread_id.as_str())?;
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct RegistrationBinding {
    project_scope: String,
    app_scope_id: AppServerId,
    agent_id: AgentId,
    runtime_id: RuntimeId,
    binding_id: BindingId,
    endpoint_generation: u64,
    #[serde(default)]
    session_id: Option<SessionId>,
    #[serde(default)]
    native_thread_id: Option<NativeThreadId>,
}

/// Recover the current runtime identity from the typed Register response.
///
/// The daemon may include a human-readable runtime channel beside the typed
/// command. That channel is deliberately ignored: only
/// `typed.command.binding` contains the runtime/binding fields that authorize
/// later commands. The project root and worker are checked before the
/// identity can be persisted; the caller compares the returned app scope with
/// the scope used for its request.
pub fn runtime_from_registration_receipt(
    receipt: &serde_json::Value,
    expected_worker_id: &str,
    expected_root: &Path,
) -> anyhow::Result<RuntimeIdentity> {
    registration_from_receipt(receipt, expected_worker_id, expected_root)
        .map(|(runtime, _)| runtime)
}

pub fn registration_from_receipt(
    receipt: &serde_json::Value,
    expected_worker_id: &str,
    expected_root: &Path,
) -> anyhow::Result<(RuntimeIdentity, SelectedTransport)> {
    if receipt.get("typed").and_then(serde_json::Value::as_bool) != Some(true) {
        anyhow::bail!("registration receipt is missing typed=true");
    }
    let worker_id = receipt
        .get("worker_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("registration receipt is missing worker_id"))?;
    if worker_id != expected_worker_id {
        anyhow::bail!(
            "registration receipt worker_id mismatch: expected {expected_worker_id}, observed {worker_id}"
        );
    }

    let binding_value = receipt
        .get("command")
        .and_then(serde_json::Value::as_object)
        .and_then(|command| command.get("binding"))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("registration receipt is missing typed command.binding"))?;
    let binding: RegistrationBinding = serde_json::from_value(binding_value)
        .context("registration receipt command.binding has an invalid typed shape")?;

    let canonical_root = std::fs::canonicalize(expected_root)?;
    let canonical_root = canonical_root.to_str().ok_or_else(|| {
        anyhow::anyhow!("registered project root must be valid UTF-8 for the wire context")
    })?;
    if binding.project_scope != canonical_root {
        anyhow::bail!(
            "registration receipt project scope mismatch: expected {canonical_root}, observed {}",
            binding.project_scope
        );
    }
    let runtime = RuntimeIdentity {
        agent_id: binding.agent_id,
        runtime_id: binding.runtime_id,
        appserver_id: binding.app_scope_id,
        endpoint_generation: binding.endpoint_generation,
        binding_id: binding.binding_id,
        session_id: binding.session_id,
        native_thread_id: binding.native_thread_id,
    };
    runtime.validate()?;
    if runtime.agent_id.as_str() != expected_worker_id {
        anyhow::bail!(
            "registration receipt binding agent mismatch: expected {expected_worker_id}, observed {}",
            runtime.agent_id
        );
    }
    let selected_value = receipt
        .get("transport_selected")
        .ok_or_else(|| anyhow::anyhow!("registration receipt is missing transport_selected"))?;
    if !selected_value.is_object() {
        anyhow::bail!("registration receipt transport_selected must be a JSON object");
    }
    let selected: SelectedTransport = serde_json::from_value(selected_value.clone())
        .context("registration receipt transport_selected has an invalid typed shape")?;
    validate_registration_transport(&selected, &runtime)?;
    Ok((runtime, selected))
}

pub fn role_brief_from_registration_receipt(
    receipt: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let role_brief = receipt
        .get("role_brief")
        .cloned()
        .filter(serde_json::Value::is_object)
        .ok_or_else(|| anyhow::anyhow!("registration receipt is missing role_brief"))?;
    if role_brief
        .get("role")
        .and_then(serde_json::Value::as_str)
        .is_none_or(str::is_empty)
    {
        anyhow::bail!("registration receipt role_brief is missing role");
    }
    for field in [
        "role_task",
        "blocked_boundary",
        "completion_action",
        "next_action",
        "notification_rule",
    ] {
        if role_brief
            .get(field)
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty)
        {
            anyhow::bail!("registration receipt role_brief is missing {field}");
        }
    }
    let responsibilities = role_brief
        .get("responsibilities")
        .and_then(serde_json::Value::as_array)
        .filter(|items| {
            !items.is_empty()
                && items
                    .iter()
                    .all(|item| item.as_str().is_some_and(|value| !value.trim().is_empty()))
        })
        .ok_or_else(|| {
            anyhow::anyhow!("registration receipt role_brief is missing responsibilities")
        })?;
    if responsibilities.is_empty() {
        anyhow::bail!("registration receipt role_brief responsibilities are empty");
    }
    let authority = role_brief
        .get("authority")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("registration receipt role_brief is missing authority"))?;
    for field in [
        "managed_subagent",
        "must_obey_master",
        "may_decline_master_invite",
    ] {
        if !authority
            .get(field)
            .is_some_and(serde_json::Value::is_boolean)
        {
            anyhow::bail!("registration receipt role_brief authority is missing {field}");
        }
    }
    let derivation = role_brief
        .get("derivation")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("registration receipt role_brief is missing derivation"))?;
    if derivation
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .is_none_or(str::is_empty)
    {
        anyhow::bail!("registration receipt role_brief derivation is missing kind");
    }
    if !derivation
        .get("parent")
        .is_some_and(|parent| parent.is_null() || parent.as_str().is_some())
    {
        anyhow::bail!("registration receipt role_brief derivation is missing parent");
    }
    Ok(role_brief)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingValidationError {
    StaleGeneration {
        expected: u64,
        observed: u64,
    },
    Mismatch {
        field: &'static str,
        expected: String,
        observed: String,
    },
}

impl fmt::Display for BindingValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleGeneration { expected, observed } => {
                write!(
                    f,
                    "stale endpoint generation: expected {expected}, observed {observed}"
                )
            }
            Self::Mismatch {
                field,
                expected,
                observed,
            } => write!(
                f,
                "runtime binding mismatch for {field}: expected {expected}, observed {observed}"
            ),
        }
    }
}

impl std::error::Error for BindingValidationError {}

/// Compare an incoming runtime binding with the currently registered binding.
/// This is intentionally pure: callers decide whether a failed command is
/// rejected or whether a separately authorized reconnect should rebind.
pub fn validate_binding(
    registered: &RuntimeIdentity,
    incoming: &RuntimeIdentity,
) -> Result<(), BindingValidationError> {
    if registered.endpoint_generation != incoming.endpoint_generation {
        return Err(BindingValidationError::StaleGeneration {
            expected: registered.endpoint_generation,
            observed: incoming.endpoint_generation,
        });
    }
    for (field, expected, observed) in [
        (
            "agent_id",
            registered.agent_id.as_str(),
            incoming.agent_id.as_str(),
        ),
        (
            "runtime_id",
            registered.runtime_id.as_str(),
            incoming.runtime_id.as_str(),
        ),
        (
            "appserver_id",
            registered.appserver_id.as_str(),
            incoming.appserver_id.as_str(),
        ),
        (
            "binding_id",
            registered.binding_id.as_str(),
            incoming.binding_id.as_str(),
        ),
    ] {
        if expected != observed {
            return Err(BindingValidationError::Mismatch {
                field,
                expected: expected.into(),
                observed: observed.into(),
            });
        }
    }
    if registered.native_thread_id != incoming.native_thread_id {
        return Err(BindingValidationError::Mismatch {
            field: "native_thread_id",
            expected: registered
                .native_thread_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            observed: incoming
                .native_thread_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
        });
    }
    if registered.session_id != incoming.session_id {
        return Err(BindingValidationError::Mismatch {
            field: "session_id",
            expected: registered
                .session_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            observed: incoming
                .session_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub worker_id: String,
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_scope: Option<ProjectScopeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<SelectedTransport>,
}

fn hex(n: usize) -> String {
    (0..n)
        .map(|_| format!("{:02x}", rand::thread_rng().gen::<u8>()))
        .collect()
}

fn identity_path(scope: &Scope, worker_id: &str) -> anyhow::Result<PathBuf> {
    let _ = scope;
    identity_path_at(&HostPaths::resolve()?, worker_id)
}

fn identity_path_at(host_paths: &HostPaths, worker_id: &str) -> anyhow::Result<PathBuf> {
    validate_id(worker_id)?;
    Ok(host_paths
        .state_root()
        .join("identities")
        .join(worker_id)
        .join("identity.json"))
}

/// Read the persisted host identity for one exact worker.
///
/// This is intentionally read-only. Recovery callers must prove that the
/// requested token and runtime still match the global identity record before
/// rebuilding a missing resident worker projection.
pub(crate) fn read_persisted(
    host_paths: &HostPaths,
    worker_id: &str,
) -> anyhow::Result<Option<Identity>> {
    read_identity(&identity_path_at(host_paths, worker_id)?)
}

fn identity_temp_path(path: &std::path::Path) -> PathBuf {
    path.parent().unwrap().join(format!(
        "identity.json.tmp.{}.{}",
        std::process::id(),
        hex(8)
    ))
}

fn write_identity(path: &std::path::Path, ident: &Identity) -> anyhow::Result<()> {
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    let tmp = identity_temp_path(path);
    std::fs::write(&tmp, serde_json::to_string_pretty(ident)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn persist_identity(scope: &Scope, ident: &Identity) -> anyhow::Result<()> {
    write_identity(&identity_path(scope, &ident.worker_id)?, ident)?;
    Ok(())
}

/// Persist a runtime binding recovered from a successful typed registration.
/// Update the in-memory identity only after every mirrored identity file has
/// been written successfully.
pub fn persist_runtime(
    scope: &Scope,
    ident: &mut Identity,
    runtime: RuntimeIdentity,
) -> anyhow::Result<()> {
    runtime.validate()?;
    if runtime.agent_id.as_str() != ident.worker_id {
        anyhow::bail!(
            "runtime binding agent does not match identity worker: expected {}, observed {}",
            ident.worker_id,
            runtime.agent_id
        );
    }
    let mut updated = ident.clone();
    updated.project_scope = Some(
        scope
            .route_scope(runtime.appserver_id.clone())?
            .project_scope_id,
    );
    updated.runtime = Some(runtime);
    persist_identity(scope, &updated)?;
    *ident = updated;
    Ok(())
}

/// Persist the server-selected transport alongside the typed runtime binding.
/// The selection is an output of server admission, never a client preference.
pub fn persist_registration(
    scope: &Scope,
    ident: &mut Identity,
    runtime: RuntimeIdentity,
    transport: SelectedTransport,
) -> anyhow::Result<()> {
    persist_registration_at(&HostPaths::resolve()?, scope, ident, runtime, transport)
}

fn persist_registration_at(
    host_paths: &HostPaths,
    scope: &Scope,
    ident: &mut Identity,
    runtime: RuntimeIdentity,
    transport: SelectedTransport,
) -> anyhow::Result<()> {
    validate_registration_transport(&transport, &runtime)?;
    let mut updated = ident.clone();
    updated.project_scope = Some(
        scope
            .route_scope(runtime.appserver_id.clone())?
            .project_scope_id,
    );
    updated.runtime = Some(runtime);
    updated.transport = Some(transport);
    write_identity(&identity_path_at(host_paths, &updated.worker_id)?, &updated)?;
    *ident = updated;
    Ok(())
}

fn read_identity(path: &std::path::Path) -> anyhow::Result<Option<Identity>> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&std::fs::read_to_string(path)?)?))
}

fn identities_by_runtime_key_at(
    host_paths: &HostPaths,
    session_id: &str,
    native_thread_id: &str,
) -> anyhow::Result<Vec<Identity>> {
    let identities_root = host_paths.state_root().join("identities");
    if !identities_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut identities = BTreeMap::new();
    for entry in std::fs::read_dir(identities_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Some(identity) = read_identity(&entry.path().join("identity.json"))? {
            identities.insert(identity.worker_id.clone(), identity);
        }
    }
    let thread_matches = identities
        .into_values()
        .filter(|identity| {
            identity
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.native_thread_id.as_ref())
                .is_some_and(|thread_id| thread_id.as_str() == native_thread_id)
        })
        .collect::<Vec<_>>();
    let strict = thread_matches
        .iter()
        .filter(|identity| {
            identity
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.session_id.as_ref())
                .is_some_and(|candidate| candidate.as_str() == session_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    if !strict.is_empty() {
        return Ok(strict);
    }
    // No persisted identity carries this exact session/thread pair. A durable
    // record with the same native thread but no session id is a legacy
    // identity from before the dual key existed; it is recoverable only when
    // the thread match is unique. A different non-null session is never
    // treated as legacy.
    Ok(thread_matches
        .into_iter()
        .filter(|identity| {
            identity
                .runtime
                .as_ref()
                .is_some_and(|runtime| runtime.session_id.is_none())
        })
        .collect())
}

fn identity_by_tmux_anchor_at(
    host_paths: &HostPaths,
    scope: &Scope,
    candidate: &crate::proto::TmuxCandidate,
) -> anyhow::Result<Option<Identity>> {
    identity_by_current_anchors_at(host_paths, scope, Some(candidate))
}

fn identity_by_current_anchors_at(
    host_paths: &HostPaths,
    scope: &Scope,
    candidate: Option<&crate::proto::TmuxCandidate>,
) -> anyhow::Result<Option<Identity>> {
    let identities_root = host_paths.state_root().join("identities");
    if !identities_root.is_dir() {
        return Ok(None);
    }
    let mut anchors = Vec::new();
    let session_id = candidate
        .and_then(|candidate| candidate.endpoint.codex_session_id.clone())
        .or_else(|| {
            candidate
                .is_none()
                .then(|| std::env::var("CODEX_SESSION_ID").ok())
                .flatten()
        });
    if let Some(value) = session_id {
        anchors.push(("codex_session_id", value));
    }
    let thread_id = candidate
        .and_then(|candidate| candidate.endpoint.codex_thread_id.clone())
        .or_else(|| {
            candidate
                .is_none()
                .then(|| std::env::var("CODEX_THREAD_ID").ok())
                .flatten()
        });
    if let Some(value) = thread_id {
        anchors.push(("codex_thread_id", value));
    }
    if let Some(candidate) = candidate {
        anchors.push(("tmux_pane_id", candidate.endpoint.pane_id.clone()));
    }
    let mut matches = BTreeMap::<String, BTreeMap<String, Identity>>::new();
    for entry in std::fs::read_dir(identities_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(identity) = read_identity(&entry.path().join("identity.json"))? else {
            continue;
        };
        let persisted_endpoint = identity
            .transport
            .as_ref()
            .and_then(|transport| transport.tmux_endpoint.as_ref());
        for (name, value) in &anchors {
            let matched =
                match *name {
                    "codex_session_id" => {
                        persisted_endpoint
                            .and_then(|persisted| persisted.codex_session_id.as_ref())
                            .is_some_and(|persisted| persisted == value)
                            || (identity.transport.as_ref().is_some_and(|transport| {
                                transport.kind == TransportKind::AppServer
                            }) && identity
                                .runtime
                                .as_ref()
                                .and_then(|runtime| runtime.session_id.as_ref())
                                .is_some_and(|persisted| persisted.as_str() == value))
                    }
                    "codex_thread_id" => {
                        persisted_endpoint
                            .and_then(|persisted| persisted.codex_thread_id.as_ref())
                            .is_some_and(|persisted| persisted == value)
                            || (identity.transport.as_ref().is_some_and(|transport| {
                                transport.kind == TransportKind::AppServer
                            }) && identity
                                .runtime
                                .as_ref()
                                .and_then(|runtime| runtime.native_thread_id.as_ref())
                                .is_some_and(|persisted| persisted.as_str() == value))
                    }
                    "tmux_pane_id" => persisted_endpoint.is_some_and(|persisted| {
                        candidate.is_some_and(|candidate| {
                            let runtime_ids_absent = candidate.endpoint.codex_session_id.is_none()
                                && candidate.endpoint.codex_thread_id.is_none();
                            let transport_kind_matches =
                                identity.transport.as_ref().is_some_and(|transport| {
                                    transport.kind == TransportKind::Tmux
                                        || (transport.kind == TransportKind::AppServer
                                            && runtime_ids_absent)
                                });
                            transport_kind_matches
                                && crate::client::adapters::tmux::same_pane_route(
                                    persisted,
                                    &candidate.endpoint,
                                )
                        })
                    }),
                    _ => false,
                };
            if matched {
                matches
                    .entry((*name).to_owned())
                    .or_default()
                    .insert(identity.worker_id.clone(), identity.clone());
            }
        }
    }
    let mut matched_workers = BTreeMap::<String, Identity>::new();
    for (anchor, identities) in matches {
        if identities.len() > 1 {
            anyhow::bail!("IDENTITY_RESTORE_AMBIGUOUS: {anchor} matches multiple persisted peers");
        }
        matched_workers.extend(identities);
    }
    match matched_workers.len() {
        0 => Ok(None),
        1 => {
            let identity = matched_workers.into_values().next().unwrap();
            let expected_scope = scope
                .route_scope(AppServerId::new(CLI_APP_SERVER_ID)?)?
                .project_scope_id;
            if identity.project_scope.as_ref() != Some(&expected_scope) {
                anyhow::bail!("IDENTITY_RESTORE_CROSS_PROJECT: a unique tmux/Codex anchor belongs to another project");
            }
            Ok(Some(identity))
        }
        _ => anyhow::bail!(
            "IDENTITY_RESTORE_CONFLICT: supplied tmux/Codex anchors identify different peers"
        ),
    }
}

/// Load or create one Codex thread identity.
pub fn load_or_create(
    scope: &Scope,
    worker_id: Option<String>,
    _endpoint_override: Option<String>,
) -> anyhow::Result<Identity> {
    let _ = _endpoint_override;
    load_or_create_resolved(scope, worker_id, true)
}

/// Load the identity selected by the current worker/thread without creating or
/// mutating any identity state. Read-only commands use this before deciding
/// whether the caller is registered.
pub fn load_existing(scope: &Scope, worker_id: Option<String>) -> anyhow::Result<Option<Identity>> {
    load_existing_at(&HostPaths::resolve()?, scope, worker_id)
}

pub(crate) fn load_existing_at(
    host_paths: &HostPaths,
    scope: &Scope,
    worker_id: Option<String>,
) -> anyhow::Result<Option<Identity>> {
    let tmux_candidate = if std::env::var_os("TMUX_PANE").is_some() {
        Some(crate::client::adapters::tmux::candidate_from_env().map_err(anyhow::Error::msg)?)
    } else {
        None
    };
    let explicit_worker = worker_id.or_else(|| {
        std::env::var("COLLAB_WORKER")
            .ok()
            .filter(|value| !value.trim().is_empty())
    });
    let anchored_identity =
        identity_by_current_anchors_at(host_paths, scope, tmux_candidate.as_ref())?;
    if let (Some(explicit_worker), Some(identity)) =
        (explicit_worker.as_deref(), anchored_identity.as_ref())
    {
        if explicit_worker != identity.worker_id {
            anyhow::bail!(
                "IDENTITY_RESTORE_CONFLICT: explicit worker {explicit_worker} conflicts with the current tmux/Codex anchors for {}",
                identity.worker_id
            );
        }
    }
    if explicit_worker.is_none() && anchored_identity.is_some() {
        return Ok(anchored_identity);
    }
    let Some(worker_id) = explicit_worker.or_else(|| {
        tmux_candidate
            .as_ref()
            .map(|candidate| format!("codex-{}", candidate.endpoint.pane_id))
    }) else {
        return Ok(None);
    };
    if let Some(identity) = read_identity(&identity_path_at(host_paths, &worker_id)?)? {
        return Ok(Some(identity));
    }
    Ok(None)
}

/// Load or create the identity used by `collab init`. Initialization binds to
/// the process cwd and the Codex thread. The server remains the sole owner of
/// channel assignment, so `ensure_registration` decides whether a persisted
/// binding must be replaced; identity loading itself never clears a binding
/// before the replacement is durably accepted.
pub fn load_or_create_for_init(scope: &Scope) -> anyhow::Result<Identity> {
    load_or_create_for_init_at(&HostPaths::resolve()?, scope)
}

fn load_or_create_for_init_at(host_paths: &HostPaths, scope: &Scope) -> anyhow::Result<Identity> {
    load_or_create_resolved_at(host_paths, scope, None, true)
}

fn load_or_create_resolved(
    scope: &Scope,
    worker_id: Option<String>,
    allow_scope_rebind: bool,
) -> anyhow::Result<Identity> {
    load_or_create_resolved_at(&HostPaths::resolve()?, scope, worker_id, allow_scope_rebind)
}

/// Why identity loading may not mint a brand-new peer for this project.
enum ScopeRebindOutcome {
    /// One durable identity matches a current tmux/Codex anchor.
    Adopted(Identity),
    /// No durable record to protect: first registration for this project.
    NoCandidate,
    /// A durable record exists but its state cannot be established. Minting
    /// here would silently orphan it, so the caller must fail closed.
    Unproven(String),
}

/// Find a persisted identity matching one current pane, session, or thread
/// anchor. Project membership alone is not authorization; ambiguity, cross-
/// project matches, and mismatched anchors fail closed.
fn identity_for_scope_rebind_at(
    host_paths: &HostPaths,
    scope: &Scope,
) -> anyhow::Result<ScopeRebindOutcome> {
    let project_scope = scope
        .route_scope(AppServerId::new(CLI_APP_SERVER_ID)?)?
        .project_scope_id;
    let candidate = if std::env::var_os("TMUX_PANE").is_some() {
        Some(crate::client::adapters::tmux::candidate_from_env().map_err(anyhow::Error::msg)?)
    } else {
        None
    };
    if let Some(identity) = identity_by_current_anchors_at(host_paths, scope, candidate.as_ref())? {
        return Ok(ScopeRebindOutcome::Adopted(identity));
    }
    let identities_root = host_paths.state_root().join("identities");
    if !identities_root.is_dir() {
        return Ok(ScopeRebindOutcome::NoCandidate);
    }
    let mut persisted = Vec::new();
    for entry in std::fs::read_dir(identities_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(identity) = read_identity(&entry.path().join("identity.json"))? else {
            continue;
        };
        if identity.project_scope.as_ref() != Some(&project_scope) {
            continue;
        }
        persisted.push(identity.worker_id);
    }
    match persisted.len() {
        0 => Ok(ScopeRebindOutcome::NoCandidate),
        _ => Ok(ScopeRebindOutcome::Unproven(format!(
            "persisted peers exist in this project ({}) but none matches the current pane, Codex session, or Codex thread",
            persisted.join(", ")
        ))),
    }
}

fn load_or_create_resolved_at(
    host_paths: &HostPaths,
    scope: &Scope,
    worker_id: Option<String>,
    allow_scope_rebind: bool,
) -> anyhow::Result<Identity> {
    let tmux_candidate = if std::env::var_os("TMUX_PANE").is_some() {
        Some(crate::client::adapters::tmux::candidate_from_env().map_err(anyhow::Error::msg)?)
    } else {
        None
    };
    let explicit_worker = worker_id.or_else(|| {
        std::env::var("COLLAB_WORKER")
            .ok()
            .filter(|value| !value.trim().is_empty())
    });
    let candidate = tmux_candidate.as_ref().ok_or_else(|| {
        anyhow::anyhow!("TMUX_ENDPOINT_MISSING: collab identity requires a current tmux pane or an explicit worker id")
    });
    let anchored_identity =
        identity_by_current_anchors_at(host_paths, scope, candidate.as_ref().ok().copied())?;
    if let (Some(explicit_worker), Some(identity)) =
        (explicit_worker.as_deref(), anchored_identity.as_ref())
    {
        if explicit_worker != identity.worker_id {
            anyhow::bail!(
                "IDENTITY_RESTORE_CONFLICT: explicit worker {explicit_worker} conflicts with the current tmux/Codex anchors for {}",
                identity.worker_id
            );
        }
    }
    if explicit_worker.is_none() {
        if let Some(identity) = anchored_identity {
            return Ok(identity);
        }
        if allow_scope_rebind {
            match identity_for_scope_rebind_at(host_paths, scope)? {
                ScopeRebindOutcome::Adopted(identity) => return Ok(identity),
                ScopeRebindOutcome::NoCandidate => {}
                ScopeRebindOutcome::Unproven(detail) => {
                    anyhow::bail!("IDENTITY_REBIND_UNPROVEN: {detail}")
                }
            }
        }
        candidate?;
    }
    let worker_id = explicit_worker
        .clone()
        .or_else(|| {
            tmux_candidate
                .as_ref()
                .map(|candidate| format!("codex-{}", candidate.endpoint.pane_id))
        })
        .ok_or_else(|| {
            anyhow::anyhow!("collab identity requires TMUX_PANE or an explicit worker id")
        })?;
    if let Some(ident) = read_identity(&identity_path_at(host_paths, &worker_id)?)? {
        return Ok(ident);
    }
    let ident = Identity {
        worker_id,
        token: hex(16),
        project_scope: None,
        runtime: None,
        transport: None,
    };
    write_identity(&identity_path_at(host_paths, &ident.worker_id)?, &ident)?;
    Ok(ident)
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: &std::sync::Mutex<()> = &crate::scope::TEST_ENV_LOCK;

    fn test_scope(root: PathBuf) -> Scope {
        Scope { root }
    }

    fn canonical_test_scope(scope: &Scope) -> String {
        scope
            .route_scope(AppServerId::new(CLI_APP_SERVER_ID).unwrap())
            .unwrap()
            .project_scope_id
            .as_str()
            .to_owned()
    }

    fn test_root(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn saved_identity(
        host_paths: &HostPaths,
        scope: &Scope,
        worker_id: &str,
        session_id: Option<&str>,
        thread_id: Option<&str>,
        tmux_endpoint: Option<crate::proto::TmuxEndpoint>,
    ) {
        let runtime = RuntimeIdentity {
            agent_id: AgentId::new(worker_id).unwrap(),
            runtime_id: RuntimeId::new(format!("runtime-{worker_id}")).unwrap(),
            appserver_id: AppServerId::new(CLI_APP_SERVER_ID).unwrap(),
            endpoint_generation: 1,
            binding_id: BindingId::new(format!("binding-{worker_id}")).unwrap(),
            session_id: session_id.map(|value| SessionId::new(value).unwrap()),
            native_thread_id: thread_id.map(|value| NativeThreadId::new(value).unwrap()),
        };
        let kind = if tmux_endpoint.is_some() {
            TransportKind::Tmux
        } else {
            TransportKind::AppServer
        };
        let transport = SelectedTransport {
            kind,
            endpoint: Some(tmux_endpoint.as_ref().map_or_else(
                || "unix:///tmp/codex.sock".to_owned(),
                |endpoint| endpoint.socket_path.clone(),
            )),
            namespace: Some(
                if tmux_endpoint.is_some() {
                    "$7"
                } else {
                    "codex_tui"
                }
                .into(),
            ),
            session_id: Some(tmux_endpoint.as_ref().map_or_else(
                || session_id.unwrap_or("session-old").to_owned(),
                |endpoint| endpoint.tmux_session_id.clone(),
            )),
            thread_id: Some(tmux_endpoint.as_ref().map_or_else(
                || thread_id.unwrap_or("thread-old").to_owned(),
                |endpoint| endpoint.pane_id.clone(),
            )),
            tmux_endpoint,
            capabilities: vec![],
            self_check: "test transport".into(),
        };
        let project_scope = scope
            .route_scope(runtime.appserver_id.clone())
            .unwrap()
            .project_scope_id;
        let identity = Identity {
            worker_id: worker_id.into(),
            token: format!("token-{worker_id}"),
            project_scope: Some(project_scope),
            runtime: Some(runtime),
            transport: Some(transport),
        };
        write_identity(&identity_path_at(host_paths, worker_id).unwrap(), &identity).unwrap();
    }

    fn tmux_candidate(
        codex_session_id: Option<&str>,
        codex_thread_id: Option<&str>,
        pane_id: &str,
    ) -> crate::proto::TmuxCandidate {
        crate::proto::TmuxCandidate {
            endpoint: crate::proto::TmuxEndpoint {
                socket_path: "/tmp/tmux-test.sock".into(),
                server_pid: 42,
                tmux_session_id: "$7".into(),
                pane_id: pane_id.into(),
                pane_pid: 99,
                codex_session_id: codex_session_id.map(str::to_owned),
                codex_thread_id: codex_thread_id.map(str::to_owned),
            },
            cwd: "/tmp/project".into(),
        }
    }

    #[test]
    fn tmux_identity_recovers_by_each_unique_anchor() {
        let root = test_root("ci-tmux-anchor");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();

        saved_identity(
            &host_paths,
            &scope,
            "session-peer",
            Some("session-1"),
            None,
            None,
        );
        let found = identity_by_tmux_anchor_at(
            &host_paths,
            &scope,
            &tmux_candidate(Some("session-1"), None, "%1"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(found.worker_id, "session-peer");

        saved_identity(
            &host_paths,
            &scope,
            "thread-peer",
            None,
            Some("thread-2"),
            None,
        );
        let found = identity_by_tmux_anchor_at(
            &host_paths,
            &scope,
            &tmux_candidate(None, Some("thread-2"), "%2"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(found.worker_id, "thread-peer");

        let endpoint = tmux_candidate(None, None, "%3").endpoint;
        saved_identity(
            &host_paths,
            &scope,
            "pane-peer",
            None,
            None,
            Some(endpoint.clone()),
        );
        let found =
            identity_by_tmux_anchor_at(&host_paths, &scope, &tmux_candidate(None, None, "%3"))
                .unwrap()
                .unwrap();
        assert_eq!(found.worker_id, "pane-peer");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tmux_pane_identity_does_not_survive_server_or_pane_pid_reuse() {
        let root = test_root("ci-tmux-anchor-pid-reuse");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
        let persisted_endpoint = tmux_candidate(None, None, "%3").endpoint;
        saved_identity(
            &host_paths,
            &scope,
            "stale-pane-peer",
            None,
            None,
            Some(persisted_endpoint.clone()),
        );

        for changed in [
            crate::proto::TmuxEndpoint {
                server_pid: persisted_endpoint.server_pid + 1,
                ..persisted_endpoint.clone()
            },
            crate::proto::TmuxEndpoint {
                pane_pid: persisted_endpoint.pane_pid + 1,
                ..persisted_endpoint.clone()
            },
        ] {
            let mut candidate = tmux_candidate(None, None, "%3");
            candidate.endpoint = changed;
            assert!(identity_by_tmux_anchor_at(&host_paths, &scope, &candidate)
                .unwrap()
                .is_none());
        }

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn pane_only_candidate_adopts_appserver_identity_with_pane_recovery_anchor() {
        let root = test_root("ci-pane-only-appserver-identity");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();

        let endpoint = crate::proto::TmuxEndpoint {
            socket_path: "/tmp/appserver-pane.sock".into(),
            server_pid: 77,
            tmux_session_id: "$8".into(),
            pane_id: "%44".into(),
            pane_pid: 88,
            codex_session_id: Some("session-appserver".into()),
            codex_thread_id: Some("thread-appserver".into()),
        };
        let runtime = RuntimeIdentity {
            agent_id: AgentId::new("appserver-peer").unwrap(),
            runtime_id: RuntimeId::new("runtime-appserver-peer").unwrap(),
            appserver_id: AppServerId::new(CLI_APP_SERVER_ID).unwrap(),
            endpoint_generation: 1,
            binding_id: BindingId::new("binding-appserver-peer").unwrap(),
            session_id: Some(SessionId::new("session-appserver").unwrap()),
            native_thread_id: Some(NativeThreadId::new("thread-appserver").unwrap()),
        };
        let project_scope = scope
            .route_scope(runtime.appserver_id.clone())
            .unwrap()
            .project_scope_id;
        let identity = Identity {
            worker_id: "appserver-peer".into(),
            token: "token-appserver-peer".into(),
            project_scope: Some(project_scope),
            runtime: Some(runtime),
            transport: Some(SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some("unix:///tmp/appserver.sock".into()),
                namespace: Some("codex_tui".into()),
                session_id: Some("session-appserver".into()),
                thread_id: Some("thread-appserver".into()),
                tmux_endpoint: Some(endpoint.clone()),
                capabilities: vec![],
                self_check: "test transport".into(),
            }),
        };
        write_identity(
            &identity_path_at(&host_paths, "appserver-peer").unwrap(),
            &identity,
        )
        .unwrap();
        let found = identity_by_tmux_anchor_at(
            &host_paths,
            &scope,
            &crate::proto::TmuxCandidate {
                endpoint: crate::proto::TmuxEndpoint {
                    codex_session_id: None,
                    codex_thread_id: None,
                    ..endpoint.clone()
                },
                cwd: "/tmp/project".into(),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(found.worker_id, "appserver-peer");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tmux_identity_recovery_rejects_anchor_conflict_and_cross_project() {
        let root = test_root("ci-tmux-conflict");
        let other = root.join("other-project");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(other.join(".agent-collab")).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
        saved_identity(
            &host_paths,
            &scope,
            "session-peer",
            Some("session-1"),
            None,
            None,
        );
        let endpoint = tmux_candidate(None, None, "%4").endpoint;
        saved_identity(&host_paths, &scope, "pane-peer", None, None, Some(endpoint));
        let conflict = identity_by_tmux_anchor_at(
            &host_paths,
            &scope,
            &tmux_candidate(Some("session-1"), None, "%4"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            conflict.starts_with("IDENTITY_RESTORE_CONFLICT:"),
            "{conflict}"
        );

        let other_scope = test_scope(other.clone());
        saved_identity(
            &host_paths,
            &other_scope,
            "foreign-peer",
            Some("session-foreign"),
            None,
            None,
        );
        let cross_project = identity_by_tmux_anchor_at(
            &host_paths,
            &scope,
            &tmux_candidate(Some("session-foreign"), None, "%5"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            cross_project.starts_with("IDENTITY_RESTORE_CROSS_PROJECT:"),
            "{cross_project}"
        );

        saved_identity(
            &host_paths,
            &scope,
            "duplicate-peer-a",
            Some("session-duplicate"),
            None,
            None,
        );
        saved_identity(
            &host_paths,
            &scope,
            "duplicate-peer-b",
            Some("session-duplicate"),
            None,
            None,
        );
        let ambiguous = identity_by_tmux_anchor_at(
            &host_paths,
            &scope,
            &tmux_candidate(Some("session-duplicate"), None, "%6"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            ambiguous.starts_with("IDENTITY_RESTORE_AMBIGUOUS:"),
            "{ambiguous}"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tmux_identity_rebind_refuses_unknown_pane_when_project_identity_exists() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = test_root("ci-tmux-unknown");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(root.join("global")).unwrap();
        saved_identity(
            &host_paths,
            &scope,
            "known-peer",
            Some("known-session"),
            None,
            None,
        );
        let previous_pane = std::env::var_os("TMUX_PANE");
        std::env::remove_var("TMUX_PANE");
        let result = identity_for_scope_rebind_at(&host_paths, &scope);
        match previous_pane {
            Some(value) => std::env::set_var("TMUX_PANE", value),
            None => std::env::remove_var("TMUX_PANE"),
        }
        assert!(matches!(result, Ok(ScopeRebindOutcome::Unproven(_))));
        assert_eq!(
            std::fs::read_dir(host_paths.state_root().join("identities"))
                .unwrap()
                .count(),
            1
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_lives_in_the_global_state_root() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = test_root("ci-global");
        let state_root = root.join("global");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let identity =
            load_or_create_resolved_at(&host_paths, &scope, Some("thread-worker".into()), false)
                .unwrap();
        let path = identity_path_at(&host_paths, &identity.worker_id).unwrap();
        assert!(path.starts_with(state_root.join("identities")));
        assert!(!root
            .join(".agent-collab/runs")
            .join(&identity.worker_id)
            .exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_reuses_the_unique_persisted_binding_for_a_codex_thread() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = test_root("ci-reuse");
        let state_root = root.join("global");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let mut identity =
            load_or_create_resolved_at(&host_paths, &scope, Some("managed-worker".into()), false)
                .unwrap();
        persist_registration_at(
            &host_paths,
            &scope,
            &mut identity,
            runtime_identity(4, "binding-managed"),
            SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some("unix:///tmp/codex.sock".into()),
                namespace: Some("codex_tui".into()),
                session_id: Some("session-1".into()),
                thread_id: Some("thread-1".into()),
                tmux_endpoint: None,
                capabilities: vec!["send_message".into()],
                self_check: "ok".into(),
            },
        )
        .unwrap();

        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        let previous_session = std::env::var_os("CODEX_SESSION_ID");
        let previous_worker = std::env::var_os("COLLAB_WORKER");
        let previous_pane = std::env::var_os("TMUX_PANE");
        std::env::set_var("CODEX_THREAD_ID", "thread-1");
        std::env::set_var("CODEX_SESSION_ID", "session-1");
        std::env::remove_var("COLLAB_WORKER");
        std::env::remove_var("TMUX_PANE");
        let resolved = load_or_create_resolved_at(&host_paths, &scope, None, false).unwrap();
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        match previous_session {
            Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
            None => std::env::remove_var("CODEX_SESSION_ID"),
        }
        match previous_worker {
            Some(value) => std::env::set_var("COLLAB_WORKER", value),
            None => std::env::remove_var("COLLAB_WORKER"),
        }
        match previous_pane {
            Some(value) => std::env::set_var("TMUX_PANE", value),
            None => std::env::remove_var("TMUX_PANE"),
        }

        assert_eq!(resolved.worker_id, "managed-worker");
        assert_eq!(resolved.token, identity.token);
        assert_eq!(resolved.runtime, identity.runtime);
        assert_eq!(resolved.transport, identity.transport);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_can_restore_by_a_unique_thread_anchor_after_session_rotation() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = test_root("ci-session-thread-key");
        let state_root = root.join("global");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let mut identity =
            load_or_create_resolved_at(&host_paths, &scope, Some("managed-worker".into()), false)
                .unwrap();
        let mut runtime = runtime_identity(4, "binding-managed");
        runtime.session_id = Some(SessionId::new("session-old").unwrap());
        runtime.native_thread_id = Some(NativeThreadId::new("thread-shared").unwrap());
        persist_registration_at(
            &host_paths,
            &scope,
            &mut identity,
            runtime,
            SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some("unix:///tmp/codex.sock".into()),
                namespace: Some("codex_tui".into()),
                session_id: Some("session-old".into()),
                thread_id: Some("thread-shared".into()),
                tmux_endpoint: None,
                capabilities: vec!["send_message".into()],
                self_check: "ok".into(),
            },
        )
        .unwrap();

        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        let previous_session = std::env::var_os("CODEX_SESSION_ID");
        let previous_worker = std::env::var_os("COLLAB_WORKER");
        std::env::set_var("CODEX_THREAD_ID", "thread-shared");
        std::env::set_var("CODEX_SESSION_ID", "session-new");
        std::env::remove_var("COLLAB_WORKER");
        let resolved = load_existing_at(&host_paths, &scope, None).unwrap();
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        match previous_session {
            Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
            None => std::env::remove_var("CODEX_SESSION_ID"),
        }
        match previous_worker {
            Some(value) => std::env::set_var("COLLAB_WORKER", value),
            None => std::env::remove_var("COLLAB_WORKER"),
        }

        assert_eq!(resolved.unwrap().worker_id, "managed-worker");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_selection_rejects_an_unscoped_legacy_thread_binding() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = test_root("ci-legacy-thread-recover");
        let state_root = root.join("global");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        // A durable record from before the dual key existed: it has a native
        // thread but no persisted session id. It is written directly because
        // the current registration validator refuses to create that shape;
        // only the upgrade path may transition an existing record forward.
        let mut runtime = runtime_identity(3, "binding-legacy");
        runtime.session_id = None;
        runtime.native_thread_id = Some(NativeThreadId::new("thread-legacy").unwrap());
        write_identity(
            &identity_path_at(&host_paths, "legacy-worker").unwrap(),
            &Identity {
                worker_id: "legacy-worker".into(),
                token: "legacy-token".into(),
                project_scope: None,
                runtime: Some(runtime),
                transport: Some(SelectedTransport {
                    kind: TransportKind::AppServer,
                    endpoint: Some("unix:///tmp/codex.sock".into()),
                    namespace: Some("codex_tui".into()),
                    session_id: Some("session-host".into()),
                    thread_id: Some("thread-legacy".into()),
                    tmux_endpoint: None,
                    capabilities: vec!["send_message".into()],
                    self_check: "ok".into(),
                }),
            },
        )
        .unwrap();

        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        let previous_session = std::env::var_os("CODEX_SESSION_ID");
        let previous_worker = std::env::var_os("COLLAB_WORKER");
        std::env::set_var("CODEX_THREAD_ID", "thread-legacy");
        std::env::set_var("CODEX_SESSION_ID", "session-host");
        std::env::remove_var("COLLAB_WORKER");
        let resolved = load_existing_at(&host_paths, &scope, None);
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        match previous_session {
            Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
            None => std::env::remove_var("CODEX_SESSION_ID"),
        }
        match previous_worker {
            Some(value) => std::env::set_var("COLLAB_WORKER", value),
            None => std::env::remove_var("COLLAB_WORKER"),
        }

        let error = resolved.unwrap_err().to_string();
        assert!(
            error.starts_with("IDENTITY_RESTORE_CROSS_PROJECT:"),
            "{error}"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_selection_can_recover_by_thread_when_session_differs() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = test_root("ci-legacy-session-mismatch");
        let state_root = root.join("global");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let mut identity =
            load_or_create_resolved_at(&host_paths, &scope, Some("dual-worker".into()), false)
                .unwrap();
        let mut runtime = runtime_identity(4, "binding-dual");
        runtime.session_id = Some(SessionId::new("session-bound").unwrap());
        runtime.native_thread_id = Some(NativeThreadId::new("thread-bound").unwrap());
        persist_registration_at(
            &host_paths,
            &scope,
            &mut identity,
            runtime,
            SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some("unix:///tmp/codex.sock".into()),
                namespace: Some("codex_tui".into()),
                session_id: Some("session-bound".into()),
                thread_id: Some("thread-bound".into()),
                tmux_endpoint: None,
                capabilities: vec!["send_message".into()],
                self_check: "ok".into(),
            },
        )
        .unwrap();

        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        let previous_session = std::env::var_os("CODEX_SESSION_ID");
        let previous_worker = std::env::var_os("COLLAB_WORKER");
        std::env::set_var("CODEX_THREAD_ID", "thread-bound");
        std::env::set_var("CODEX_SESSION_ID", "session-other");
        std::env::remove_var("COLLAB_WORKER");
        let resolved = load_existing_at(&host_paths, &scope, None).unwrap();
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        match previous_session {
            Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
            None => std::env::remove_var("CODEX_SESSION_ID"),
        }
        match previous_worker {
            Some(value) => std::env::set_var("COLLAB_WORKER", value),
            None => std::env::remove_var("COLLAB_WORKER"),
        }

        // The thread is an independently valid unique recovery anchor.
        assert_eq!(resolved.unwrap().worker_id, "dual-worker");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_selection_recovers_by_thread_without_a_session_anchor() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = test_root("ci-session-thread-required");
        let state_root = root.join("global");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let mut identity =
            load_or_create_resolved_at(&host_paths, &scope, Some("managed-worker".into()), false)
                .unwrap();
        persist_registration_at(
            &host_paths,
            &scope,
            &mut identity,
            runtime_identity(4, "binding-managed"),
            SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some("unix:///tmp/codex.sock".into()),
                namespace: Some("codex_tui".into()),
                session_id: Some("session-1".into()),
                thread_id: Some("thread-1".into()),
                tmux_endpoint: None,
                capabilities: vec!["send_message".into()],
                self_check: "ok".into(),
            },
        )
        .unwrap();

        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        let previous_session = std::env::var_os("CODEX_SESSION_ID");
        let previous_worker = std::env::var_os("COLLAB_WORKER");
        let previous_pane = std::env::var_os("TMUX_PANE");
        std::env::set_var("CODEX_THREAD_ID", "thread-1");
        std::env::remove_var("CODEX_SESSION_ID");
        std::env::remove_var("COLLAB_WORKER");
        std::env::remove_var("TMUX_PANE");

        let existing = load_existing_at(&host_paths, &scope, None)
            .unwrap()
            .expect("the thread anchor uniquely identifies the peer");
        let created = load_or_create_resolved_at(&host_paths, &scope, None, false).unwrap();

        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        match previous_session {
            Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
            None => std::env::remove_var("CODEX_SESSION_ID"),
        }
        match previous_worker {
            Some(value) => std::env::set_var("COLLAB_WORKER", value),
            None => std::env::remove_var("COLLAB_WORKER"),
        }
        match previous_pane {
            Some(value) => std::env::set_var("TMUX_PANE", value),
            None => std::env::remove_var("TMUX_PANE"),
        }

        assert_eq!(existing.worker_id, "managed-worker");
        assert_eq!(created.worker_id, "managed-worker");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn existing_identity_prefers_the_global_thread_binding_over_a_stale_alias() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = test_root("ci-existing-thread-authority");
        let state_root = root.join("global");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let mut authoritative =
            load_or_create_resolved_at(&host_paths, &scope, Some("authoritative".into()), false)
                .unwrap();
        let mut runtime = runtime_identity(7, "binding-authoritative");
        runtime.native_thread_id = Some(NativeThreadId::new("thread-current").unwrap());
        persist_registration_at(
            &host_paths,
            &scope,
            &mut authoritative,
            runtime,
            SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some("unix:///tmp/codex.sock".into()),
                namespace: Some("codex_tui".into()),
                session_id: Some("session-1".into()),
                thread_id: Some("thread-current".into()),
                tmux_endpoint: None,
                capabilities: vec!["send_message".into()],
                self_check: "ok".into(),
            },
        )
        .unwrap();
        write_identity(
            &identity_path_at(&host_paths, "codex-thread-current").unwrap(),
            &Identity {
                worker_id: "stale-alias".into(),
                token: "stale-token".into(),
                project_scope: None,
                runtime: None,
                transport: None,
            },
        )
        .unwrap();

        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        let previous_session = std::env::var_os("CODEX_SESSION_ID");
        let previous_worker = std::env::var_os("COLLAB_WORKER");
        std::env::set_var("CODEX_THREAD_ID", "thread-current");
        std::env::set_var("CODEX_SESSION_ID", "session-1");
        std::env::remove_var("COLLAB_WORKER");
        let resolved = load_existing_at(&host_paths, &scope, None).unwrap();
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        match previous_session {
            Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
            None => std::env::remove_var("CODEX_SESSION_ID"),
        }
        match previous_worker {
            Some(value) => std::env::set_var("COLLAB_WORKER", value),
            None => std::env::remove_var("COLLAB_WORKER"),
        }

        assert_eq!(resolved.unwrap().worker_id, "authoritative");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_requires_a_codex_thread_when_no_worker_is_given() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = test_root("ci-thread");
        std::fs::create_dir_all(root.join(".agent-collab/runs")).unwrap();
        let state_root = root.join(".collab-state");
        std::fs::create_dir_all(&state_root).unwrap();
        let scope = test_scope(root.clone());
        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        let previous_worker = std::env::var_os("COLLAB_WORKER");
        let previous_pane = std::env::var_os("TMUX_PANE");
        std::env::remove_var("CODEX_THREAD_ID");
        std::env::remove_var("COLLAB_WORKER");
        std::env::remove_var("TMUX_PANE");
        let result = load_or_create(&scope, None, None);
        if let Some(value) = previous_thread {
            std::env::set_var("CODEX_THREAD_ID", value);
        }
        if let Some(value) = previous_worker {
            std::env::set_var("COLLAB_WORKER", value);
        }
        match previous_pane {
            Some(value) => std::env::set_var("TMUX_PANE", value),
            None => std::env::remove_var("TMUX_PANE"),
        }
        assert!(result.is_err());
        assert!(!state_root.join("identities").exists());
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    fn runtime_identity(generation: u64, binding: &str) -> RuntimeIdentity {
        RuntimeIdentity {
            agent_id: AgentId::new("agent-1").unwrap(),
            runtime_id: RuntimeId::new("runtime-1").unwrap(),
            appserver_id: AppServerId::new("appserver-1").unwrap(),
            endpoint_generation: generation,
            binding_id: BindingId::new(binding).unwrap(),
            session_id: Some(SessionId::new("session-1").unwrap()),
            native_thread_id: Some(NativeThreadId::new("thread-1").unwrap()),
        }
    }

    #[test]
    fn runtime_identity_serializes_typed_fields() {
        let identity = runtime_identity(3, "binding-1");
        let encoded = serde_json::to_value(&identity).unwrap();
        assert_eq!(
            encoded,
            serde_json::json!({
                "agent_id": "agent-1",
                "runtime_id": "runtime-1",
                "appserver_id": "appserver-1",
                "endpoint_generation": 3,
                "binding_id": "binding-1",
                "session_id": "session-1",
                "native_thread_id": "thread-1"
            })
        );
        assert_eq!(
            serde_json::from_value::<RuntimeIdentity>(encoded).unwrap(),
            identity
        );
    }

    #[test]
    fn cli_adapter_identity_has_one_stable_app_scope() {
        let first = RuntimeIdentity::cli_adapter("worker-1").unwrap();
        let second = RuntimeIdentity::cli_adapter("worker-1").unwrap();
        assert_eq!(first, second);
        assert_eq!(first.appserver_id.as_str(), CLI_APP_SERVER_ID);
        assert_eq!(first.endpoint_generation, 0);
    }

    #[test]
    fn registration_receipt_recovers_typed_command_binding() {
        let root = std::env::temp_dir().join(format!(
            "collab-registration-receipt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let canonical_root = std::fs::canonicalize(&root).unwrap();
        let receipt = serde_json::json!({
            "typed": true,
            "worker_id": "worker-1",
            "runtime": "untrusted-channel-label",
            "transport_selected": {
                "kind": "appserver",
                "endpoint": "unix:///tmp/codex.sock",
                "namespace": "codex_tui",
                "session_id": "session-1",
                "thread_id": "thread-1",
                "capabilities": ["session_status", "read_thread", "send_message_to_thread"],
                "self_check": "server verified"
            },
            "command": {
                "cmd": "RegisterWorker",
                "binding": {
                    "project_scope": canonical_root.to_str().unwrap(),
                    "app_scope_id": CLI_APP_SERVER_ID,
                    "agent_id": "worker-1",
                    "runtime_id": "runtime-thread-1",
                    "binding_id": "binding-1",
                    "endpoint_generation": 4,
                    "session_id": "session-1",
                    "native_thread_id": "thread-1"
                }
            }
        });

        let runtime = runtime_from_registration_receipt(&receipt, "worker-1", &root).unwrap();
        assert_eq!(runtime.agent_id.as_str(), "worker-1");
        assert_eq!(runtime.runtime_id.as_str(), "runtime-thread-1");
        assert_eq!(runtime.appserver_id.as_str(), CLI_APP_SERVER_ID);
        assert_eq!(runtime.endpoint_generation, 4);
        assert_eq!(runtime.binding_id.as_str(), "binding-1");
        assert_eq!(
            runtime.native_thread_id.as_ref().unwrap().as_str(),
            "thread-1"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn role_brief_receipt_requires_the_complete_contract() {
        let missing_authority = serde_json::json!({
            "role": "managed-subagent",
            "role_task": "Execute the assigned task.",
            "responsibilities": ["Stay in scope."],
            "derivation": {"kind": "managed-subagent", "parent": "parent-1"},
            "blocked_boundary": "Report a concrete blocker.",
            "completion_action": "Return evidence.",
            "next_action": "Continue.",
            "notification_rule": "Handle priority actions."
        });
        let error = role_brief_from_registration_receipt(&serde_json::json!({
            "role_brief": missing_authority
        }))
        .unwrap_err();
        assert!(error.to_string().contains("authority"), "{error}");

        let missing_responsibilities = serde_json::json!({
            "role": "worker",
            "role_task": "Own the task.",
            "authority": {
                "managed_subagent": false,
                "must_obey_master": false,
                "may_decline_master_invite": true
            },
            "derivation": {"kind": "peer", "parent": null},
            "blocked_boundary": "Negotiate conflicts.",
            "completion_action": "Close the task.",
            "next_action": "Resume.",
            "notification_rule": "Handle priority actions."
        });
        let error = role_brief_from_registration_receipt(&serde_json::json!({
            "role_brief": missing_responsibilities
        }))
        .unwrap_err();
        assert!(error.to_string().contains("responsibilities"), "{error}");
    }

    #[test]
    fn registration_receipt_rejects_missing_runtime_binding() {
        let root = std::env::temp_dir().join(format!(
            "collab-registration-missing-binding-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let error = runtime_from_registration_receipt(
            &serde_json::json!({"typed": true, "worker_id": "worker-1"}),
            "worker-1",
            &root,
        )
        .unwrap_err();
        assert!(error.to_string().contains("typed command.binding"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn registration_receipt_rejects_transport_binding_mismatch() {
        let root = std::env::temp_dir().join(format!(
            "collab-registration-mismatch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let canonical_root = std::fs::canonicalize(&root).unwrap();
        let receipt = serde_json::json!({
            "typed": true,
            "worker_id": "worker-1",
            "transport_selected": {
                "kind": "appserver",
                "endpoint": "unix:///tmp/codex.sock",
                "namespace": "codex_tui",
                "session_id": "session-selected",
                "thread_id": "thread-selected",
                "capabilities": ["send_message_to_thread"],
                "self_check": "server verified"
            },
            "command": {
                "cmd": "RegisterWorker",
                "binding": {
                    "project_scope": canonical_root.to_str().unwrap(),
                    "app_scope_id": "app-1",
                    "agent_id": "worker-1",
                    "runtime_id": "runtime-1",
                    "binding_id": "binding-1",
                    "endpoint_generation": 1,
                    "session_id": "session-selected",
                    "native_thread_id": "thread-other"
                }
            }
        });
        let error = runtime_from_registration_receipt(&receipt, "worker-1", &root).unwrap_err();
        assert!(error.to_string().contains("thread/pane address"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn appserver_identity_uses_codex_thread() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = test_root("ci-appserver");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let state_root = root.join(".collab-state");
        std::fs::create_dir_all(&state_root).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let identity =
            load_or_create_resolved_at(&host_paths, &scope, Some("thread-worker".into()), false)
                .unwrap();
        assert_eq!(identity.worker_id, "thread-worker");
        assert_eq!(identity.runtime, None);
        assert_eq!(identity.transport, None);
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn init_preserves_a_persisted_binding_until_registration_replaces_it() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = test_root("ci-init");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let state_root = root.join(".collab-state");
        std::fs::create_dir_all(&state_root).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let mut ident =
            load_or_create_resolved_at(&host_paths, &scope, Some("codex-thread-1".into()), false)
                .unwrap();
        persist_registration_at(
            &host_paths,
            &scope,
            &mut ident,
            runtime_identity(4, "binding-appserver"),
            SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some("unix:///tmp/codex.sock".into()),
                namespace: Some("codex_tui".into()),
                session_id: Some("session-1".into()),
                thread_id: Some("thread-1".into()),
                tmux_endpoint: None,
                capabilities: vec!["send_message".into()],
                self_check: "ok".into(),
            },
        )
        .unwrap();

        let previous_worker = std::env::var_os("COLLAB_WORKER");
        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        std::env::remove_var("CODEX_THREAD_ID");
        std::env::set_var("COLLAB_WORKER", "codex-thread-1");
        let resolved = load_or_create_for_init_at(&host_paths, &scope).unwrap();
        assert_eq!(resolved.worker_id, "codex-thread-1");
        assert_eq!(resolved.token, ident.token);
        assert_eq!(resolved.runtime, ident.runtime);
        assert_eq!(resolved.transport, ident.transport);
        match previous_worker {
            Some(value) => std::env::set_var("COLLAB_WORKER", value),
            None => std::env::remove_var("COLLAB_WORKER"),
        }
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    /// A different thread cannot adopt a persisted peer by project scope alone.
    /// tmux recovery requires at least one matching durable session/thread/pane
    /// anchor; App Server route liveness is no longer an identity oracle.
    #[test]
    fn init_rejects_identity_recovery_without_a_matching_anchor() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = short_test_root();
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let state_root = root.join("global");
        std::fs::create_dir_all(&state_root).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        persist_peer_at(
            &host_paths,
            &scope,
            "other-peer",
            "session-other",
            "thread-other",
            1,
        );

        let adopted = with_current_address("thread-intruder", "session-intruder", || {
            load_or_create_resolved_at(&host_paths, &scope, None, true)
        });

        assert!(adopted
            .unwrap_err()
            .to_string()
            .contains("IDENTITY_REBIND_UNPROVEN"));
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    /// A dead App Server route cannot substitute for one of the approved tmux
    /// identity anchors; mismatched session/thread must not recover this peer.
    #[test]
    fn init_rejects_old_route_death_without_a_matching_tmux_anchor() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = short_test_root();
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let state_root = root.join("global");
        std::fs::create_dir_all(&state_root).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let original = persist_peer_at(
            &host_paths,
            &scope,
            "agent-peer",
            "session-old",
            "thread-old",
            1,
        );
        let _original_token = original.token.clone();

        let restored = with_current_address("thread-new", "session-new", || {
            load_or_create_resolved_at(&host_paths, &scope, None, true)
        });

        assert!(restored
            .unwrap_err()
            .to_string()
            .contains("IDENTITY_REBIND_UNPROVEN"));
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    /// Explicit identity selection is the deterministic recovery path: it needs
    /// no liveness proof because the caller named the identity.
    #[test]
    fn explicit_worker_selection_rebinds_without_a_liveness_probe() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = short_test_root();
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let state_root = root.join("global");
        std::fs::create_dir_all(&state_root).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let original = persist_peer_at(
            &host_paths,
            &scope,
            "agent-peer",
            "session-old",
            "thread-old",
            1,
        );
        let original_token = original.token.clone();

        // No route authority is listening: explicit selection must not need one.
        let selected = with_current_address("thread-new", "session-new", || {
            load_or_create_resolved_at(&host_paths, &scope, Some("agent-peer".into()), true)
        });

        let selected = selected.unwrap();
        assert_eq!(selected.worker_id, "agent-peer");
        assert_eq!(selected.token, original_token);

        // The recovery skill names the identity through the environment, not
        // through an argument: that path must behave identically.
        let previous_worker = std::env::var_os("COLLAB_WORKER");
        std::env::set_var("COLLAB_WORKER", "agent-peer");
        let selected_env = load_or_create_resolved_at(&host_paths, &scope, None, true);
        match previous_worker {
            Some(value) => std::env::set_var("COLLAB_WORKER", value),
            None => std::env::remove_var("COLLAB_WORKER"),
        }
        let selected_env = selected_env.unwrap();
        assert_eq!(selected_env.worker_id, "agent-peer");
        assert_eq!(selected_env.token, original_token);
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn explicit_worker_selection_rejects_conflicting_identity_anchor() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = short_test_root();
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let state_root = root.join("global");
        std::fs::create_dir_all(&state_root).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        persist_peer_at(
            &host_paths,
            &scope,
            "anchored-peer",
            "session-a",
            "thread-a",
            1,
        );
        persist_peer_at(
            &host_paths,
            &scope,
            "explicit-peer",
            "session-b",
            "thread-b",
            1,
        );

        let result = with_current_address("thread-a", "session-a", || {
            load_or_create_resolved_at(&host_paths, &scope, Some("explicit-peer".into()), true)
        });

        let error = result.unwrap_err().to_string();
        assert!(error.starts_with("IDENTITY_RESTORE_CONFLICT:"), "{error}");
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    /// With multiple persisted peers and no matching stable anchor, init must
    /// fail closed. Anchor ambiguity itself is covered by the tmux recovery
    /// tests above.
    #[test]
    fn init_fails_closed_when_multiple_peers_exist_without_matching_anchor() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = short_test_root();
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let state_root = root.join("global");
        std::fs::create_dir_all(&state_root).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        persist_peer_at(&host_paths, &scope, "agent-a", "session-a", "thread-a", 1);
        persist_peer_at(&host_paths, &scope, "agent-b", "session-b", "thread-b", 1);

        let restored = with_current_address("thread-new", "session-new", || {
            load_or_create_resolved_at(&host_paths, &scope, None, true)
        });

        let error = restored.unwrap_err().to_string();
        assert!(error.contains("IDENTITY_REBIND_UNPROVEN"), "{error}");
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    /// An old App Server identity without a matching tmux/Codex anchor cannot
    /// be recovered by route-death inference.
    #[test]
    fn ordinary_commands_reject_appserver_only_identity_match() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = short_test_root();
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let state_root = root.join("global");
        std::fs::create_dir_all(&state_root).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        persist_peer_at(
            &host_paths,
            &scope,
            "agent-peer",
            "session-old",
            "thread-old",
            1,
        );
        let resolved = with_current_address("thread-new", "session-new", || {
            load_or_create_resolved_at(&host_paths, &scope, None, true)
        });
        assert!(resolved
            .unwrap_err()
            .to_string()
            .starts_with("IDENTITY_REBIND_UNPROVEN"));
        assert!(
            !state_root
                .join("identities")
                .join("codex-thread-new")
                .exists(),
            "ordinary commands must not mint a second identity for a rotated address"
        );
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    /// An ordinary command with a persisted project identity but no matching
    /// tmux/Codex anchor must not silently mint a second identity.
    #[test]
    fn ordinary_commands_fail_closed_without_a_matching_tmux_anchor() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = short_test_root();
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let state_root = root.join("global");
        std::fs::create_dir_all(&state_root).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let victim = persist_peer_at(
            &host_paths,
            &scope,
            "victim-peer",
            "session-victim",
            "thread-victim",
            1,
        );

        let outcome = with_current_address("thread-new", "session-new", || {
            load_or_create_resolved_at(&host_paths, &scope, None, true)
        });

        let error = outcome.unwrap_err().to_string();
        assert!(error.starts_with("IDENTITY_REBIND_UNPROVEN"), "{error}");
        assert!(!error.contains(&victim.token), "{error}");
        assert!(
            !state_root
                .join("identities")
                .join("codex-thread-new")
                .exists(),
            "an unreachable authority must not mint a replacement identity"
        );
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    /// A live App Server route is not consulted or adopted when the current
    /// tmux/Codex identity anchors do not match.
    #[test]
    fn ordinary_commands_do_not_adopt_a_project_peer_without_anchor() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = short_test_root();
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let state_root = root.join("global");
        std::fs::create_dir_all(&state_root).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let victim = persist_peer_at(
            &host_paths,
            &scope,
            "victim-peer",
            "session-victim",
            "thread-victim",
            1,
        );

        let resolved = with_current_address("thread-intruder", "session-intruder", || {
            load_or_create_resolved_at(&host_paths, &scope, None, true)
        });
        let error = resolved.unwrap_err().to_string();
        assert!(error.starts_with("IDENTITY_REBIND_UNPROVEN"), "{error}");
        assert!(!error.contains(&victim.token));
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn persist_runtime_updates_all_identity_state_after_validation() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = test_root("ci-persist");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let state_root = root.join(".collab-state");
        std::fs::create_dir_all(&state_root).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let mut identity = Identity {
            worker_id: "agent-1".into(),
            token: "token-1".into(),
            project_scope: None,
            runtime: None,
            transport: None,
        };
        let runtime = runtime_identity(7, "binding-7");
        persist_registration_at(
            &host_paths,
            &scope,
            &mut identity,
            runtime.clone(),
            SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some("unix:///tmp/codex.sock".into()),
                namespace: Some("codex_tui".into()),
                session_id: Some("session-1".into()),
                thread_id: Some("thread-1".into()),
                tmux_endpoint: None,
                capabilities: vec!["send_message".into()],
                self_check: "server verified".into(),
            },
        )
        .unwrap();
        assert_eq!(identity.runtime, Some(runtime.clone()));
        assert_eq!(
            identity.transport.as_ref().unwrap().kind,
            TransportKind::AppServer
        );
        let persisted = read_identity(&identity_path_at(&host_paths, "agent-1").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(persisted.runtime, Some(runtime));
        std::fs::remove_dir_all(state_root).ok();
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_writes_use_distinct_temporary_paths() {
        let path = std::path::Path::new("/tmp/collab-identity/identity.json");
        let first = identity_temp_path(path);
        let second = identity_temp_path(path);
        assert_ne!(first, second);
    }

    #[test]
    fn persisted_registration_records_the_current_project_scope() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root = test_root("ci-project-scope");
        std::fs::create_dir_all(root.join(".agent-collab")).unwrap();
        let state_root = root.join(".collab-state");
        std::fs::create_dir_all(&state_root).unwrap();
        let scope = test_scope(root.clone());
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let mut identity = Identity {
            worker_id: "agent-1".into(),
            token: "token-1".into(),
            project_scope: None,
            runtime: None,
            transport: None,
        };

        persist_registration_at(
            &host_paths,
            &scope,
            &mut identity,
            runtime_identity(7, "binding-7"),
            SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some("unix:///tmp/codex.sock".into()),
                namespace: Some("codex_tui".into()),
                session_id: Some("session-1".into()),
                thread_id: Some("thread-1".into()),
                tmux_endpoint: None,
                capabilities: vec!["send_message".into()],
                self_check: "server verified".into(),
            },
        )
        .unwrap();

        let expected = scope
            .route_scope(AppServerId::new("appserver-1").unwrap())
            .unwrap()
            .project_scope_id;
        assert_eq!(identity.project_scope.as_ref(), Some(&expected));
        let persisted = read_identity(&identity_path_at(&host_paths, "agent-1").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(persisted.project_scope.as_ref(), Some(&expected));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn binding_validation_rejects_stale_generation_without_mutating_inputs() {
        let registered = runtime_identity(4, "binding-1");
        let incoming = runtime_identity(3, "binding-1");
        let registered_before = registered.clone();
        let incoming_before = incoming.clone();

        assert!(matches!(
            validate_binding(&registered, &incoming),
            Err(BindingValidationError::StaleGeneration {
                expected: 4,
                observed: 3
            })
        ));
        assert_eq!(registered, registered_before);
        assert_eq!(incoming, incoming_before);
    }

    #[test]
    fn binding_validation_rejects_wrong_binding_without_mutating_inputs() {
        let registered = runtime_identity(4, "binding-1");
        let incoming = runtime_identity(4, "binding-2");
        let registered_before = registered.clone();
        let incoming_before = incoming.clone();

        assert!(matches!(
            validate_binding(&registered, &incoming),
            Err(BindingValidationError::Mismatch {
                field: "binding_id",
                ..
            })
        ));
        assert_eq!(registered, registered_before);
        assert_eq!(incoming, incoming_before);
    }

    #[test]
    fn identifier_validation_rejects_empty_and_control_values() {
        assert!(AgentId::new("").is_err());
        assert!(RuntimeId::new("runtime\n1").is_err());
        assert!(DispatchId::new("d".repeat(MAX_ID_LENGTH + 1)).is_err());
    }

    /// A short temp project root: the fake route-authority socket lives under the
    /// state root, so the combined path must stay under `sockaddr_un`'s limit.
    fn short_test_root() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        // Keep the whole `<root>/global/server.sock` path short: unix socket paths
        // are limited to ~104 bytes and the macOS temp dir already uses ~48.
        std::env::temp_dir().join(format!(
            "cs{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// Build one persisted identity whose dual key is the given address.
    fn persist_peer_at(
        host_paths: &HostPaths,
        scope: &Scope,
        worker: &str,
        session: &str,
        thread: &str,
        generation: u64,
    ) -> Identity {
        let mut identity =
            load_or_create_resolved_at(host_paths, scope, Some(worker.into()), false).unwrap();
        let mut runtime = runtime_identity(generation, &format!("binding-{worker}"));
        runtime.session_id = Some(SessionId::new(session).unwrap());
        runtime.native_thread_id = Some(NativeThreadId::new(thread).unwrap());
        persist_registration_at(
            host_paths,
            scope,
            &mut identity,
            runtime,
            SelectedTransport {
                kind: TransportKind::AppServer,
                endpoint: Some("unix:///tmp/codex.sock".into()),
                namespace: Some("codex_tui".into()),
                session_id: Some(session.into()),
                thread_id: Some(thread.into()),
                tmux_endpoint: None,
                capabilities: vec!["send_message".into()],
                self_check: "server verified".into(),
            },
        )
        .unwrap();
        identity
    }
    fn with_current_address<T>(thread: &str, session: &str, body: impl FnOnce() -> T) -> T {
        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        let previous_session = std::env::var_os("CODEX_SESSION_ID");
        let previous_worker = std::env::var_os("COLLAB_WORKER");
        std::env::set_var("CODEX_THREAD_ID", thread);
        std::env::set_var("CODEX_SESSION_ID", session);
        std::env::remove_var("COLLAB_WORKER");
        let result = body();
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        match previous_session {
            Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
            None => std::env::remove_var("CODEX_SESSION_ID"),
        }
        match previous_worker {
            Some(value) => std::env::set_var("COLLAB_WORKER", value),
            None => std::env::remove_var("COLLAB_WORKER"),
        }
        result
    }
}
